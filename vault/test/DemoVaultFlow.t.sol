// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {console} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {DemoAttestedVerifier as DAV} from "../src/demo/DemoAttestedVerifier.sol";
import {MockVerifier, TestToken, NoReturnToken} from "./mocks/Mocks.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// The UNCHANGED BridgeVault deployed with the operator-attested DEMO verifier: the full
/// deposit -> attested update -> delay -> attested release loop, forgeries, cross-vault and
/// replay behaviour, what the vault still bounds when the attestor is rogue, expiry, and gas
/// against the MockVerifier.
///
/// Reuses VaultFixture's helpers and constants but replaces its setUp: the vault's verifier is
/// a DemoAttestedVerifier (attestor key `attPk`). `mv` is still created, only to build the
/// forged `imageId | digest` seals the MockVerifier would accept. The fixture's `_update`,
/// `_bridgeOut`, `_batch`, `_rel` and `_releaseSeal` build MockVerifier seals and are not used
/// for successful calls here.
contract DemoVaultFlowTest is VaultFixture {
    DAV dv;
    uint256 attPk = uint256(keccak256("demo attestor key"));
    address att;
    uint256 constant LIFETIME = 180 days;

    function setUp() public override {
        vm.warp(1_763_000_000);
        att = vm.addr(attPk);
        mv = new MockVerifier();
        dv = new DAV(att, LIFETIME);
        v = new BridgeVault(
            IRiscZeroVerifier(address(dv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        usdc = new TestToken("USDC", 6);
        usdt = new NoReturnToken();
        _registerFixtureTokens(v);

        usdc.mint(alice, 1_000_000e6);
        usdt.mint(alice, 1_000_000e6);
        vm.deal(alice, 100 ether);
        vm.startPrank(alice);
        usdc.approve(address(v), type(uint256).max);
        usdt.approve(address(v), type(uint256).max);
        vm.stopPrank();
    }

    /// VaultFixture's registry, menus and delays, on any vault administered by `admin`.
    function _registerFixtureTokens(BridgeVault x) internal {
        vm.startPrank(admin);
        x.registerToken(address(usdc), 6, _arr(100e6, 1000e6), _arr(1e6, 10e6), _arr(1e6, 10e6), DELAY);
        x.registerToken(address(usdt), 6, _arr(100e6, 1000e6), _arr(0, 0), _arr(0, 0), DELAY);
        x.registerToken(address(0), 18, _arr(0.1 ether, 1 ether), _arr(0, 0), _arr(0.001 ether, 0.01 ether), DELAY);
        x.setRelayFeeMenu(address(usdc), _menu2(_one(2e6), new uint256[](0)));
        x.setRelayFeeMenu(address(0), _menu2(new uint256[](0), _one(0.002 ether)));
        vm.stopPrank();
    }

    // ------------------------------------------------------------------ attestation helpers

    /// The attestor's seal, built here from the EIP-712 definition with NO external call (so it
    /// can sit inside an expectRevert'd call; the cheatcode applies to the next external call).
    /// DemoAttestedVerifier.t.sol checks this encoding against the contract's own views.
    function _attest(DAV ver, uint256 pk, bytes32 img, bytes32 dig) internal view returns (bytes memory) {
        bytes32 ds = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("RFP022 DemoAttestedVerifier"),
                keccak256("1"),
                block.chainid,
                address(ver)
            )
        );
        bytes32 sh = keccak256(
            abi.encode(
                keccak256("DemoAttestation(bytes32 tag,bytes32 imageId,bytes32 journalDigest)"),
                keccak256("RFP022/demo-attested-verifier/v1"),
                img,
                dig
            )
        );
        (uint8 sv, bytes32 r, bytes32 s) = vm.sign(pk, keccak256(abi.encodePacked(hex"1901", ds, sh)));
        return abi.encodePacked(bytes4(0x7fa98d9a), r, s, sv);
    }

    function _att(bytes32 img, bytes32 dig) internal view returns (bytes memory) {
        return _attest(dv, attPk, img, dig);
    }

    function _journalFor(
        BridgeVault x,
        bytes32 prev,
        bytes32 next,
        bytes32[] memory roots,
        BridgeVault.ClaimDelta[] memory claims,
        BridgeVault.Exit[] memory exits
    ) internal view returns (bytes memory) {
        return abi.encode(BridgeVault.UpdateJournal(x.cfgId(), prev, next, roots, claims, exits));
    }

    function _relDigest(BridgeVault x, bytes32 root, bytes32 nf, address token, uint256 denom, address to, address rel, uint256 fee)
        internal
        view
        returns (bytes32)
    {
        return sha256(abi.encode(x.cfgId(), root, nf, token, denom, to, rel, fee));
    }

    function _relSeal(bytes32 root, bytes32 nf, address token, uint256 denom, address to, address rel, uint256 fee)
        internal
        view
        returns (bytes memory)
    {
        return _att(G3, _relDigest(v, root, nf, token, denom, to, rel, fee));
    }

    /// Deposit `n` USDC denom 0 and apply one attested update claiming them and exiting `exits`.
    function _attestedBridgeOut(uint256 n, uint256 exits) internal returns (bytes memory journal, bytes memory seal) {
        for (uint256 i; i < n; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](exits);
        for (uint256 i; i < exits; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("e", i)));
        journal = _journal(v.g2State(), keccak256(abi.encode(v.g2State())), roots, cl, ex);
        seal = _att(G2, sha256(journal));
        v.update(journal, seal);
    }

    /// Per token: the vault holds at least the accrued fees plus every deposited denomination not
    /// yet released, and released value <= burned value <= claimed value <= deposited value.
    function _assertSolvent(address token) internal view {
        (,,,,, uint256[] memory denoms,,, uint256 fees) = v.tokenConfig(token);
        uint256 dep;
        uint256 rel;
        for (uint256 i; i < denoms.length; ++i) {
            (uint64 nd, uint64 nc, uint64 nb, uint64 nr) = v.counters(token, i);
            assertLe(nr, nb, "releases <= burns");
            assertLe(nc, nd, "claims <= deposits");
            dep += nd * denoms[i];
            rel += nr * denoms[i];
        }
        assertLe(v.burnedValue(token), v.claimedValue(token), "burned <= claimed");
        assertLe(v.claimedValue(token), dep, "claimed <= deposited");
        assertLe(rel, v.burnedValue(token), "released <= burned");
        assertGe(_bal(token, address(v)), fees + dep - rel, "vault holds fees + unreleased principal");
    }

    // ------------------------------------------------------------------ the full loop

    function test_FullLoopWithAttestedSeals() public {
        // 1. deposits: 3 x 100 USDC, 1 x 1000 USDC, 1 x 1 ETH
        for (uint256 i; i < 3; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        _deposit(1, keccak256("c1000"));
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("ceth"));
        assertEq(usdc.balanceOf(address(v)), 3 * 101e6 + 1010e6);

        // 2. attested G2 update: claims for all of them, exits for 2 x 100 USDC and the ETH
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](3);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 3);
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, 1);
        cl[2] = BridgeVault.ClaimDelta(address(0), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](3);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("E1"));
        ex[2] = BridgeVault.Exit(address(0), 1, keccak256("E2"));
        bytes32 s1 = keccak256("state-1");
        bytes memory j = _journal(S0, s1, roots, cl, ex);
        bytes memory seal = _att(G2, sha256(j));
        v.update(j, seal);
        assertEq(v.g2State(), s1);
        assertEq(v.exitCount(), 3);
        bytes32 root = v.exitRoot();
        assertEq(v.exitRootTime(root), block.timestamp);

        // replaying the same attested update: the state moved on
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j, seal);

        // 3. attested release of a 100 USDC exit to bob, 2 USDC relay fee (on the menu)
        bytes32 nf = keccak256("nf-usdc");
        bytes memory rs = _relSeal(root, nf, address(usdc), 100e6, bob, relayer, 2e6);
        vm.expectRevert(BridgeVault.ReleaseTooEarly.selector); // the delay still applies
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 2e6);
        vm.warp(block.timestamp + DELAY);
        // a copied seal cannot be redirected
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.release(rs, root, nf, address(usdc), 0, alice, relayer, 2e6);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, alice, 2e6);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 97e6);
        assertEq(usdc.balanceOf(relayer), 2e6);

        // replaying the release: nullifier spent
        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 2e6);

        // 4. attested ETH release, self-submitted (fee 0)
        bytes32 nfe = keccak256("nf-eth");
        uint256 before = bob.balance;
        v.release(_relSeal(root, nfe, address(0), 1 ether, bob, address(0), 0), root, nfe, address(0), 1, bob, address(0), 0);
        assertEq(bob.balance - before, 0.99 ether);

        // 5. a second update chains on s1
        BridgeVault.Exit[] memory ex2 = new BridgeVault.Exit[](1);
        ex2[0] = BridgeVault.Exit(address(usdc), 1, keccak256("E3"));
        bytes memory j2 = _journal(s1, keccak256("state-2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), ex2);
        v.update(j2, _att(G2, sha256(j2)));
        assertEq(v.exitCount(), 4);

        _assertSolvent(address(usdc));
        _assertSolvent(address(0));
        (,, uint64 burns, uint64 releases) = v.counters(address(usdc), 0);
        assertEq(burns, 2);
        assertEq(releases, 1);
    }

    // ------------------------------------------------------------------ forgeries

    /// The seals anyone can make for the MockVerifier (`imageId | digest`) are refused.
    function test_ForgedMockSealRejected() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](2);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("E1"));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory forged = mv.sealFor(G2, sha256(j));
        vm.expectRevert(abi.encodeWithSelector(DAV.BadSealLength.selector, 64));
        v.update(j, forged);
        // the same forgery wrapped in the demo prefix is still not a signature
        vm.expectRevert();
        v.update(j, abi.encodePacked(bytes4(0x7fa98d9a), forged, uint8(27)));

        v.update(j, _att(G2, sha256(j)));
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory mseal = mv.sealFor(G3, _relDigest(v, root, nf, address(usdc), 100e6, bob, relayer, 0));
        vm.expectRevert(abi.encodeWithSelector(DAV.BadSealLength.selector, 64));
        v.release(mseal, root, nf, address(usdc), 0, bob, relayer, 0);
        // a seal by any other key
        bytes memory other = _attest(dv, uint256(keccak256("thief")), G3, _relDigest(v, root, nf, address(usdc), 100e6, bob, relayer, 0));
        vm.expectRevert(abi.encodeWithSelector(DAV.NotAttestor.selector, vm.addr(uint256(keccak256("thief")))));
        v.release(other, root, nf, address(usdc), 0, bob, relayer, 0);
        // a "real" RISC Zero Groth16 seal is never taken as verified here
        vm.expectRevert(DAV.ZkProofNotCheckedHere.selector);
        v.release(abi.encodePacked(bytes4(0x73c457ba), new bytes(256)), root, nf, address(usdc), 0, bob, relayer, 0);
        assertEq(usdc.balanceOf(bob), 0);
    }

    /// An attestation for the release image never passes as an update (and the other way).
    function test_UpdateAndReleaseAttestationsDontCross() public {
        _deposit(0, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, new BridgeVault.Exit[](0));
        bytes memory sx1 = _att(G3, sha256(j));
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.update(j, sx1);
    }

    // ------------------------------------------------------------------ other vaults

    /// Two demo vaults with their own verifier instances and the SAME attestor key: an
    /// attestation made for one verifier fails at the other (the EIP-712 domain names it).
    function test_UpdateSignedForAnotherVaultRejected_SeparateVerifiers() public {
        DAV dv2 = new DAV(att, LIFETIME);
        BridgeVault v2 = new BridgeVault(
            IRiscZeroVerifier(address(dv2)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        _registerFixtureTokens(v2);
        _deposit(0, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, new BridgeVault.Exit[](0));
        // signed for v2's verifier, submitted to v
        bytes memory forV2 = _attest(dv2, attPk, G2, sha256(j));
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.update(j, forV2);
        // and v2's own journal (its cfgId) attested for v2 is not accepted by v either
        bytes memory j2 = _journalFor(v2, S0, keccak256("s1"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory s2 = _attest(dv2, attPk, G2, sha256(j2));
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        v.update(j2, s2);
        v2.update(j2, s2); // control: it is valid where it was meant to go
        // control: the right verifier's attestation works on v
        v.update(j, _att(G2, sha256(j)));
    }

    /// Two vaults sharing ONE verifier: the attestation verifies at both, but the journal's
    /// cfgId binds the vault, so the other vault refuses it (WrongConfig). Same for a release:
    /// the digest includes cfgId.
    function test_UpdateSignedForAnotherVaultRejected_SharedVerifier() public {
        BridgeVault v3 = new BridgeVault(
            IRiscZeroVerifier(address(dv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        _registerFixtureTokens(v3);
        assertTrue(v3.cfgId() != v.cfgId());
        _deposit(0, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory seal = _att(G2, sha256(j));
        vm.expectRevert(BridgeVault.WrongConfig.selector);
        v3.update(j, seal);
        v.update(j, seal);

        // a release attested for v can't be used at v3 (its digest has v3's cfgId)
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory rs = _relSeal(root, nf, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector); // v3 never saw that root
        v3.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
    }

    // ------------------------------------------------------------------ replay

    function test_ReplayedUpdateIsStale() public {
        (bytes memory j, bytes memory seal) = _attestedBridgeOut(3, 2);
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j, seal);
        // still stale after later updates (states never revisit)
        bytes memory j2 = _journal(v.g2State(), keccak256("next"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        v.update(j2, _att(G2, sha256(j2)));
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j, seal);
        bytes memory sx2 = _att(G2, sha256(j2));
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j2, sx2);
    }

    /// A journal with newState == prevState is refused by the vault (NoStateAdvance) even when
    /// the attestor signed it. `verify` is stateless, so before that check such a journal stayed
    /// valid after it was applied and anyone could replay it, re-adding its claims and exits each
    /// time until a vault bound stopped it. G2-lite refuses an empty batch, so a journal it really
    /// produced always advances; the attestor still checks it (README), and the vault now does too.
    function test_NonAdvancingJournalRefusedEvenIfAttested() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        bytes memory bad = _journal(S0, S0, new bytes32[](0), cl, ex);
        bytes memory seal = _att(G2, sha256(bad));
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(bad, seal);
        vm.prank(bob); // anyone
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(bad, seal);
        (, uint64 claims, uint64 burns,) = v.counters(address(usdc), 0);
        assertEq(claims, 0, "no claim counted");
        assertEq(burns, 0, "no burn counted");
        assertEq(v.exitCount(), 0, "no exit leaf added");
        assertEq(v.g2State(), S0);
        // the same batch, advancing, applies once
        bytes memory good = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        v.update(good, _att(G2, sha256(good)));
        assertEq(v.exitCount(), 1);
        _assertSolvent(address(usdc));
    }

    function test_ReplayedReleaseIsSpent() public {
        _attestedBridgeOut(2, 2);
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory rs = _relSeal(root, nf, address(usdc), 100e6, bob, relayer, 0);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        // the same exit under a later root: same nullifier (it depends on the exit secret only)
        bytes memory j2 = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        v.update(j2, _att(G2, sha256(j2)));
        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        assertEq(usdc.balanceOf(bob), 99e6);
    }

    // ------------------------------------------------------------------ rogue attestor

    /// What a compromised attestor key CAN do: sign claims and exits that never happened on LEZ
    /// and drain the vault, up to what was deposited, after the delay and within the caps.
    function test_RogueAttestorCanDrainDepositedPrincipal() public {
        for (uint256 i; i < 5; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 5);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](5);
        for (uint256 i; i < 5; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("fake", i)));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        address thief = address(0xBAD);
        v.update(j, _att(G2, sha256(j)));
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        for (uint256 i; i < 5; ++i) {
            bytes32 nf = keccak256(abi.encode("nf", i));
            v.release(_relSeal(root, nf, address(usdc), 100e6, thief, thief, 0), root, nf, address(usdc), 0, thief, thief, 0);
        }
        assertEq(usdc.balanceOf(thief), 5 * 99e6);
        // a sixth release has no burn behind it
        bytes32 nf6 = keccak256("nf6");
        bytes memory s6 = _relSeal(root, nf6, address(usdc), 100e6, thief, thief, 0);
        vm.expectRevert(BridgeVault.ReleasesExceedBurns.selector);
        v.release(s6, root, nf6, address(usdc), 0, thief, thief, 0);
        // only the fees are left
        assertEq(usdc.balanceOf(address(v)), _fees(address(usdc)));
        _assertSolvent(address(usdc));
    }

    /// What it CANNOT do: the vault's own bounds hold whatever is signed.
    function test_RogueAttestorBoundedByVault() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32 s1 = keccak256("s1");

        // claims above deposits
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 3);
        bytes memory j = _journal(S0, s1, new bytes32[](0), cl, new BridgeVault.Exit[](0));
        bytes memory sx3 = _att(G2, sha256(j));
        vm.expectRevert(BridgeVault.ClaimsExceedDeposits.selector);
        v.update(j, sx3);

        // exits above claimed value
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](2);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("E1"));
        j = _journal(S0, s1, new bytes32[](0), cl, ex);
        bytes memory sx4 = _att(G2, sha256(j));
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        v.update(j, sx4);

        // a deposit root the vault never had
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = keccak256("made-up root");
        j = _journal(S0, s1, roots, cl, new BridgeVault.Exit[](0));
        bytes memory sx5 = _att(G2, sha256(j));
        vm.expectRevert(BridgeVault.UnknownDepositRoot.selector);
        v.update(j, sx5);

        // a release under a root the vault never recorded
        bytes32 fake = keccak256("fake exit root");
        bytes32 nf = keccak256("nf");
        bytes memory fs = _relSeal(fake, nf, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector);
        v.release(fs, fake, nf, address(usdc), 0, bob, relayer, 0);

        // a real update, then: delay, caps and freeze still apply
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        j = _journal(S0, s1, new bytes32[](0), cl, ex);
        v.update(j, _att(G2, sha256(j)));
        bytes32 root = v.exitRoot();
        bytes memory rs = _relSeal(root, nf, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.ReleaseTooEarly.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        vm.warp(block.timestamp + DELAY);

        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, 1e30, 50e6); // release cap below one denomination
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        _execAsAdmin(BridgeVault.Action.SetCaps, abi.encode(address(usdc), uint64(0), uint256(0), uint256(0)));

        vm.prank(freezer);
        v.freeze(address(usdc), false, true);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        vm.prank(freezer);
        v.freezeAll(false, true); // the global release freeze also stops update
        bytes memory j2 = _journal(s1, keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal2 = _att(G2, sha256(j2));
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.update(j2, seal2);
        bytes memory pAll = abi.encode(false, true);
        bytes memory pTok = abi.encode(address(usdc), false, true);
        vm.startPrank(admin);
        bytes32 idAll = v.schedule(BridgeVault.Action.UnfreezeAll, pAll);
        v.schedule(BridgeVault.Action.Unfreeze, pTok);
        vm.warp(v.scheduledEta(idAll));
        v.execute(BridgeVault.Action.UnfreezeAll, pAll);
        v.execute(BridgeVault.Action.Unfreeze, pTok);
        vm.stopPrank();
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        v.update(j2, seal2);
        _assertSolvent(address(usdc));
    }

    /// Whatever the attestor signs (random claim and exit counts, random releases), principal
    /// paid out never exceeds principal deposited, per token and denomination.
    function testFuzz_RogueAttestorSolvency(uint8 dep0, uint8 cl0, uint8 ex0, uint8 ex1, uint8 rel0, uint8 rel1) public {
        uint256 d0 = bound(dep0, 0, 10);
        for (uint256 i; i < d0; ++i) _deposit(0, keccak256(abi.encode("c0", i)));
        _deposit(1, keccak256("c1"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(bound(cl0, 0, 14)));
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, 1);
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, new BridgeVault.Exit[](0));
        try v.update(j, _att(G2, sha256(j))) {} catch {}
        uint256 n0 = bound(ex0, 0, 25);
        uint256 n1 = bound(ex1, 0, 3);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n0 + n1);
        for (uint256 i; i < n0 + n1; ++i) ex[i] = BridgeVault.Exit(address(usdc), i < n0 ? 0 : 1, keccak256(abi.encode("e", i)));
        j = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), ex);
        try v.update(j, _att(G2, sha256(j))) {} catch {}
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        uint256 r0 = bound(rel0, 0, 28);
        uint256 r1 = bound(rel1, 0, 4);
        for (uint256 i; i < r0 + r1; ++i) {
            uint32 di = i < r0 ? 0 : 1;
            uint256 denom = di == 0 ? 100e6 : 1000e6;
            bytes32 nf = keccak256(abi.encode("nf", i));
            bytes memory s = _relSeal(root, nf, address(usdc), denom, bob, relayer, 0);
            try v.release(s, root, nf, address(usdc), di, bob, relayer, 0) {} catch {}
        }
        _assertSolvent(address(usdc));
    }

    // ------------------------------------------------------------------ expiry

    /// After EXPIRES_AT nothing verifies: no update, no release. Whatever is still in the vault
    /// stays there (this is a testnet demo stage; see the README).
    function test_ExpiryStopsUpdatesAndReleases() public {
        _attestedBridgeOut(2, 1);
        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256("nf");
        bytes memory rs = _relSeal(root, nf, address(usdc), 100e6, bob, relayer, 0);
        bytes memory j2 = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal2 = _att(G2, sha256(j2));
        uint256 exp = dv.EXPIRES_AT();
        vm.warp(exp);
        vm.expectRevert(abi.encodeWithSelector(DAV.Expired.selector, exp));
        v.release(rs, root, nf, address(usdc), 0, bob, relayer, 0);
        vm.expectRevert(abi.encodeWithSelector(DAV.Expired.selector, exp));
        v.update(j2, seal2);
        assertEq(usdc.balanceOf(address(v)), 2 * 101e6, "funds stay in the vault");
        // deposits still work (the vault doesn't know): the UI must stop offering them
        _deposit(0, keccak256("late deposit"));
    }

    // ------------------------------------------------------------------ gas

    /// Gas of `update` (1 claim, 1 exit) and `release` (fee 0) with the demo verifier against
    /// the MockVerifier. Both vaults and the demo verifier are created inside this test, so their
    /// storage is equally warm; the mock's seal is built locally so neither verifier account is
    /// touched before the measured call; the mock is measured FIRST; each recipient is fresh; and
    /// the gas is the call frame's own (`lastCallGas`). The difference is what the demo verifier
    /// adds. Local EVM rules (Osaka-like), not Sepolia's Glamsterdam pricing; absolute numbers
    /// here are low because the vault's slots are warm (see Gas.t.sol for real per-op figures).
    function test_GasDemoVsMock() public {
        DAV dvG = new DAV(att, LIFETIME);
        BridgeVault vd = new BridgeVault(
            IRiscZeroVerifier(address(dvG)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        BridgeVault m = new BridgeVault(
            IRiscZeroVerifier(address(mv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        _registerFixtureTokens(m);
        _registerFixtureTokens(vd);
        vm.startPrank(alice);
        usdc.approve(address(m), type(uint256).max);
        usdc.approve(address(vd), type(uint256).max);
        vm.stopPrank();

        (uint256 uM, uint256 rM) = _measure(m, DAV(address(0)));
        (uint256 uD, uint256 rD) = _measure(vd, dvG);
        console.log("update (1 claim, 1 exit): demo %d, mock %d, diff %d", uD, uM, uD - uM);
        console.log("release (fee 0):          demo %d, mock %d, diff %d", rD, rM, rD - rM);
        assertLt(uD - uM, 10_000);
        assertLt(rD - rM, 10_000);
    }

    /// `vm.lastCallGas().gasTotalUsed`, read by a raw call: the installed forge (1.5.1) returns a
    /// 5-field struct, while this forge-std's `Vm.Gas` has 6, so the typed call fails to decode.
    function _lastCallGas() internal view returns (uint256 used) {
        (bool ok, bytes memory r) = address(vm).staticcall(abi.encodeWithSignature("lastCallGas()"));
        require(ok, "lastCallGas");
        (, used) = abi.decode(r, (uint64, uint64));
    }

    function _measure(BridgeVault x, DAV ver) internal returns (uint256 gUpdate, uint256 gRelease) {
        bool demo = address(ver) != address(0);
        address to = address(uint160(uint256(keccak256(abi.encode("recipient", address(x))))));
        vm.prank(alice);
        x.deposit(address(usdc), 0, keccak256(abi.encode("gas", address(x))));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E"));
        bytes memory j = _journalFor(x, x.g2State(), keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory seal = demo ? _attest(ver, attPk, G2, sha256(j)) : abi.encodePacked(G2, sha256(j));
        x.update(j, seal);
        gUpdate = _lastCallGas();
        bytes32 root = x.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256(abi.encode("nf", address(x)));
        bytes32 dig = _relDigest(x, root, nf, address(usdc), 100e6, to, address(0), 0);
        bytes memory rs = demo ? _attest(ver, attPk, G3, dig) : abi.encodePacked(G3, dig);
        x.release(rs, root, nf, address(usdc), 0, to, address(0), 0);
        gRelease = _lastCallGas();
        vm.warp(block.timestamp - DELAY);
    }
}
