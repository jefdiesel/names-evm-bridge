// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken, FeeOnTransferToken, EthRejecter} from "./mocks/Mocks.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// Deployment, constants and helpers live in VaultFixture.
contract BridgeVaultTest is VaultFixture {

    // ------------------------------------------------------------------ tree

    function test_EmptyRootMatchesReferenceAndIsKnown() public view {
        bytes32[] memory none = new bytes32[](0);
        assertEq(v.depositRoot(), _refRoot(none));
        assertEq(v.depositRoot(), 0xc6f67e02e6e4e1bdefb994c6098953f34636ba2b6ca20a4721d2b26a886722ff);
        assertTrue(v.isDepositRoot(v.depositRoot()));
    }

    function test_TreeMatchesReference() public {
        bytes32[] memory leaves = new bytes32[](7);
        for (uint256 i; i < 7; ++i) {
            bytes32 c = keccak256(abi.encode(i));
            _deposit(0, c);
            leaves[i] = v.depositLeaf(address(usdc), 100e6, c);
            bytes32[] memory prefix = new bytes32[](i + 1);
            for (uint256 j; j <= i; ++j) prefix[j] = leaves[j];
            assertEq(v.depositRoot(), _refRoot(prefix));
            assertTrue(v.isDepositRoot(v.depositRoot()));
        }
        assertEq(v.depositCount(), 7);
    }

    function test_LeafBindsDenomination() public view {
        bytes32 c = keccak256("c");
        assertTrue(v.depositLeaf(address(usdc), 100e6, c) != v.depositLeaf(address(usdc), 1000e6, c));
        assertTrue(v.depositLeaf(address(usdc), 100e6, c) != v.depositLeaf(address(usdt), 100e6, c));
    }

    // ------------------------------------------------------------------ deposits

    function test_DepositERC20() public {
        uint256 before = usdc.balanceOf(alice);
        uint64 idx = _deposit(1, keccak256("c"));
        assertEq(idx, 0);
        assertEq(usdc.balanceOf(alice), before - 1010e6);
        assertEq(usdc.balanceOf(address(v)), 1010e6);
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        assertEq(fees, 10e6);
        (uint64 dep,,,) = v.counters(address(usdc), 1);
        assertEq(dep, 1);
    }

    function test_DepositNoReturnTokenLikeUSDT() public {
        vm.prank(alice);
        v.deposit(address(usdt), 0, keccak256("c"));
        assertEq(usdt.balanceOf(address(v)), 100e6);
    }

    function test_RevertFeeOnTransfer() public {
        FeeOnTransferToken fot = new FeeOnTransferToken();
        vm.prank(admin);
        v.registerToken(address(fot), 18, _arr(1 ether, 10 ether), _arr(0, 0), _arr(0, 0), DELAY);
        fot.mint(alice, 100 ether);
        vm.startPrank(alice);
        fot.approve(address(v), type(uint256).max);
        vm.expectRevert(BridgeVault.WrongAmount.selector);
        v.deposit(address(fot), 0, keccak256("c"));
        vm.stopPrank();
    }

    function test_RevertWrongDecimals() public {
        TestToken t = new TestToken("X", 8);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.BadDecimals.selector);
        v.registerToken(address(t), 18, _arr(1, 2), _arr(0, 0), _arr(0, 0), DELAY);
    }

    function test_RevertFeeAboveMax() public {
        TestToken t = new TestToken("X", 18);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.registerToken(address(t), 18, _arr(100, 1000), _arr(3, 0), _arr(0, 0), DELAY);
    }

    function test_RevertDelayBelowMinimum() public {
        TestToken t = new TestToken("X", 18);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.DelayTooShort.selector);
        v.registerToken(address(t), 18, _arr(100, 1000), _arr(0, 0), _arr(0, 0), 10 minutes);
    }

    function test_RevertBadDenomIndex() public {
        vm.prank(alice);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.deposit(address(usdc), 5, keccak256("c"));
    }

    function test_DepositETH() public {
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("c"));
        assertEq(address(v).balance, 1 ether);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.WrongAmount.selector);
        v.depositETH{value: 0.5 ether}(1, keccak256("c2"));
    }

    function test_RevertUnregisteredAndDeregistered() public {
        TestToken t = new TestToken("X", 18);
        vm.expectRevert(BridgeVault.NotRegistered.selector);
        v.deposit(address(t), 0, keccak256("c"));
        vm.prank(admin);
        v.deregisterToken(address(usdc));
        vm.prank(alice);
        vm.expectRevert(BridgeVault.NotRegistered.selector);
        v.deposit(address(usdc), 0, keccak256("c"));
    }

    function test_DepositCap() public {
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, 250e6, 1_000_000e6);
        _deposit(0, keccak256("1"));
        _deposit(0, keccak256("2"));
        vm.prank(alice);
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.deposit(address(usdc), 0, keccak256("3"));
        // rolling estimate: after one window the previous bucket still counts in part,
        // after two it is gone
        vm.warp(block.timestamp + 2 days);
        _deposit(0, keccak256("3"));
    }

    /// A commitment can back only one deposit, whatever the token or denomination: the LEZ
    /// claim nullifier depends on the secret alone, so a second one could never be claimed.
    function test_RevertCommitmentReused() public {
        bytes32 c = keccak256("c");
        _deposit(0, c);
        assertTrue(v.commitmentUsed(c));
        vm.startPrank(alice);
        vm.expectRevert(BridgeVault.CommitmentUsed.selector);
        v.deposit(address(usdc), 0, c); // same token, same denomination
        vm.expectRevert(BridgeVault.CommitmentUsed.selector);
        v.deposit(address(usdc), 1, c); // other denomination
        vm.expectRevert(BridgeVault.CommitmentUsed.selector);
        v.deposit(address(usdt), 0, c); // other token
        vm.expectRevert(BridgeVault.CommitmentUsed.selector);
        v.depositETH{value: 0.1 ether}(0, c); // ETH
        vm.stopPrank();
        (uint64 dep,,,) = v.counters(address(usdc), 0);
        assertEq(dep, 1);
        assertEq(v.depositCount(), 1);
        // a reverted deposit doesn't burn its commitment
        bytes32 c2 = keccak256("c2");
        vm.prank(alice);
        vm.expectRevert(BridgeVault.WrongAmount.selector);
        v.depositETH{value: 0.5 ether}(0, c2);
        assertFalse(v.commitmentUsed(c2));
        vm.prank(alice);
        v.depositETH{value: 0.1 ether}(0, c2);
        assertTrue(v.commitmentUsed(c2));
    }

    function test_RevertDuplicateDenomination() public {
        TestToken t = new TestToken("X", 18);
        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.registerToken(address(t), 18, _arr(100, 100), _arr(0, 0), _arr(0, 0), DELAY);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.addDenomination(address(usdc), 1000e6, 0, 0);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.addDenomination(address(usdc), 100e6, 0, 0);
        v.addDenomination(address(usdc), 10_000e6, 0, 0);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.addDenomination(address(usdc), 10_000e6, 0, 0);
        vm.stopPrank();
        (,,,,, uint256[] memory denoms,,,) = v.tokenConfig(address(usdc));
        assertEq(denoms.length, 3);
    }

    // ------------------------------------------------------------------ config id

    function _cfgRef(uint256 chainId, address vault, bytes32 a, bytes32 b, bytes32 c) internal pure returns (bytes32) {
        bytes memory pre = abi.encodePacked(keccak256("RFP022/cfg/v1"), chainId, vault, a, b, c);
        assertEq(pre.length, 180);
        return sha256(pre);
    }

    function test_CfgIdDerived() public view {
        assertEq(v.CFG_TAG(), keccak256("RFP022/cfg/v1"));
        assertEq(v.lezChainId(), LEZ_CHAIN);
        assertEq(v.zoneId(), ZONE);
        assertEq(v.programId(), PROGRAM);
        assertEq(v.cfgId(), _cfgRef(block.chainid, address(v), LEZ_CHAIN, ZONE, PROGRAM));
        assertEq(v.cfgId(), v.computeCfgId(block.chainid, address(v), LEZ_CHAIN, ZONE, PROGRAM));
    }

    /// Same constructor arguments on another chain, or at another address, give another cfgId.
    function test_CfgIdBindsChainAndAddress() public {
        bytes32 base = v.cfgId();
        BridgeVault other = new BridgeVault(
            IRiscZeroVerifier(address(mv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin, freezer, feeTo, ADMIN_DELAY
        );
        assertTrue(address(other) != address(v));
        assertTrue(other.cfgId() != base);
        assertEq(other.cfgId(), _cfgRef(block.chainid, address(other), LEZ_CHAIN, ZONE, PROGRAM));

        // same address, other chain id: redeploy at v's address with chain id changed
        uint256 chain0 = block.chainid;
        bytes memory args =
            abi.encode(address(mv), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, uint64(1 hours), admin, freezer, feeTo, uint64(ADMIN_DELAY));
        address at = address(0xC0FFEE);
        deployCodeTo("BridgeVault.sol:BridgeVault", args, at);
        bytes32 here = BridgeVault(at).cfgId();
        assertEq(here, _cfgRef(chain0, at, LEZ_CHAIN, ZONE, PROGRAM));
        vm.chainId(chain0 + 1);
        deployCodeTo("BridgeVault.sol:BridgeVault", args, at);
        bytes32 there = BridgeVault(at).cfgId();
        assertTrue(there != here);
        assertEq(there, _cfgRef(chain0 + 1, at, LEZ_CHAIN, ZONE, PROGRAM));

        // each LEZ id is bound too
        assertTrue(v.computeCfgId(chain0, address(v), ZONE, LEZ_CHAIN, PROGRAM) != base);
        assertTrue(v.computeCfgId(chain0, address(v), LEZ_CHAIN, ZONE, keccak256("p2")) != base);
    }

    // ------------------------------------------------------------------ freeze and roles

    function test_FreezeRules() public {
        vm.prank(freezer);
        v.freeze(address(usdc), true, true);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.deposit(address(usdc), 0, keccak256("c"));
        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.schedule(BridgeVault.Action.Unfreeze, abi.encode(address(usdc), true, true));
        _execAsAdmin(BridgeVault.Action.Unfreeze, abi.encode(address(usdc), true, true));
        _deposit(0, keccak256("c"));

        vm.prank(freezer);
        v.freezeAll(true, true);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.depositETH{value: 1 ether}(1, keccak256("c"));
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.freeze(address(usdc), true, true);
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.freezeAll(true, false);
        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.schedule(BridgeVault.Action.UnfreezeAll, abi.encode(true, true));
    }

    function test_AdminTwoStep() public {
        _execAsAdmin(BridgeVault.Action.ProposeAdmin, abi.encode(bob));
        vm.expectRevert(BridgeVault.NotPendingAdmin.selector);
        v.acceptAdmin();
        vm.prank(bob);
        v.acceptAdmin();
        assertEq(v.admin(), bob);
    }

    // ------------------------------------------------------------------ updates (G2)

    function test_UpdateAppendsExits() public {
        bytes32 root = _bridgeOut(3, 2);
        assertEq(v.exitCount(), 2);
        assertTrue(v.exitRootTime(root) != 0);
        (uint64 d, uint64 c, uint64 b,) = v.counters(address(usdc), 0);
        assertEq(d, 3);
        assertEq(c, 3);
        assertEq(b, 2);
        bytes32[] memory leaves = new bytes32[](2);
        leaves[0] = v.exitLeaf(address(usdc), 100e6, keccak256(abi.encode("e", uint256(0))));
        leaves[1] = v.exitLeaf(address(usdc), 100e6, keccak256(abi.encode("e", uint256(1))));
        assertEq(root, _refRoot(leaves));
    }

    function test_RevertClaimsExceedDeposits() public {
        _deposit(0, keccak256("c"));
        bytes32[] memory roots = new bytes32[](0);
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        bytes memory j = _journal(S0, keccak256("s1"), roots, cl, new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.ClaimsExceedDeposits.selector);
        v.update(j, seal);
    }

    function test_RevertBurnsExceedClaims() public {
        _deposit(0, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](2);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("e1"));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        v.update(j, seal);
    }

    /// Wrapped tokens are fungible on LEZ, so burns are bounded by value per token, not per
    /// denomination: one 1000 claim can back ten 100 exits, all of which release.
    function test_BurnInOtherDenominationByValue() public {
        _deposit(1, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](10);
        for (uint256 i; i < 10; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("e", i)));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        assertEq(v.claimedValue(address(usdc)), 1000e6);
        assertEq(v.burnedValue(address(usdc)), 1000e6);
        (, uint64 c1,,) = v.counters(address(usdc), 1);
        (, uint64 c0, uint64 b0,) = v.counters(address(usdc), 0);
        assertEq(c1, 1);
        assertEq(c0, 0);
        assertEq(b0, 10);

        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        for (uint256 i; i < 10; ++i) {
            bytes32 nf = keccak256(abi.encode("nf", i));
            v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 0), root, nf, address(usdc), 0, bob, relayer, 0);
        }
        assertEq(usdc.balanceOf(bob), 10 * 99e6);
        // only fees are left: 10 deposit fee + 10 x 1 withdraw fee
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        assertEq(fees, 20e6);
        assertEq(usdc.balanceOf(address(v)), fees);
        // an 11th release has no burn behind it
        bytes32 nf11 = keccak256("nf11");
        bytes memory s11 = _releaseSeal(root, nf11, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.ReleasesExceedBurns.selector);
        v.release(s11, root, nf11, address(usdc), 0, bob, relayer, 0);

        // the value is used up: one more 100 exit is refused
        BridgeVault.Exit[] memory ex2 = new BridgeVault.Exit[](1);
        ex2[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e10"));
        bytes memory j2 = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), ex2);
        bytes memory seal2 = mv.sealFor(G2, sha256(j2));
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        v.update(j2, seal2);
    }

    /// The other way round: ten 100 claims back one 1000 exit, but one 100 claim doesn't.
    function test_BurnLargerDenominationByValue() public {
        _deposit(0, keccak256("c0"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 1, keccak256("e0"));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        v.update(j, seal);

        for (uint256 i = 1; i < 10; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 10);
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        v.release(_releaseSeal(root, nf, address(usdc), 1000e6, bob, relayer, 0), root, nf, address(usdc), 1, bob, relayer, 0);
        assertEq(usdc.balanceOf(bob), 990e6);
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        assertEq(usdc.balanceOf(address(v)), fees);
    }

    /// Value is per token: USDT claims don't back USDC burns.
    function test_RevertBurnAgainstOtherTokenValue() public {
        vm.prank(alice);
        v.deposit(address(usdt), 1, keccak256("c"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdt), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e0"));
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        v.update(j, seal);
    }

    function test_RevertClaimBadDenomIndex() public {
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 7, 0);
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), cl, new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.update(j, seal);
    }

    function test_RevertUnknownDepositRoot() public {
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = keccak256("fake");
        bytes memory j = _journal(S0, keccak256("s1"), roots, new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.UnknownDepositRoot.selector);
        v.update(j, seal);
    }

    function test_RevertStaleStateAndReplay() public {
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G2, sha256(j));
        v.update(j, seal);
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j, seal);
    }

    /// The vault's outbound bookkeeping for USDC denomination 0, to compare before and after.
    function _outboundSnapshot() internal view returns (bytes memory) {
        (, uint64 claims, uint64 burns,) = v.counters(address(usdc), 0);
        return abi.encode(
            v.g2State(), claims, burns, v.claimedValue(address(usdc)), v.burnedValue(address(usdc)), v.exitCount(), v.exitRoot()
        );
    }

    /// A journal with newState == prevState is refused (NoStateAdvance) even with a valid seal,
    /// and changes nothing. Without the check it stayed valid after it was applied, so anyone
    /// could replay it: each replay re-added its claims and appended its exit leaves again
    /// (new exit root, new first index, duplicate StateUpdated). A stale one stays StaleState.
    function test_RevertNoStateAdvance() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        bytes memory before = _outboundSnapshot();

        bytes memory j = _journal(S0, S0, roots, cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(j, seal);
        // an empty one too
        bytes memory empty = _journal(S0, S0, new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory emptySeal = mv.sealFor(G2, sha256(empty));
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(empty, emptySeal);
        // stale and non-advancing: the stale check comes first
        bytes32 other = keccak256("other");
        bytes memory stale = _journal(other, other, roots, cl, ex);
        bytes memory staleSeal = mv.sealFor(G2, sha256(stale));
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(stale, staleSeal);
        assertEq(_outboundSnapshot(), before, "a refused update changed the vault");

        // after a real advance, the new state can't be "advanced" to itself either
        bytes32 s1 = keccak256("s1");
        _update(_journal(S0, s1, roots, cl, ex));
        before = _outboundSnapshot();
        bytes memory again = _journal(s1, s1, roots, cl, ex);
        bytes memory againSeal = mv.sealFor(G2, sha256(again));
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(again, againSeal);
        assertEq(_outboundSnapshot(), before, "a refused update changed the vault");
    }

    /// The normal path is unchanged: an advancing update applies claims and exits and emits
    /// StateUpdated; its replay is stale; an empty but advancing journal is still accepted.
    function test_AdvancingUpdateStillApplies() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E0"));
        bytes32 s1 = keccak256("s1");
        bytes memory j = _journal(S0, s1, new bytes32[](0), cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        bytes32[] memory leaves = new bytes32[](1);
        leaves[0] = v.exitLeaf(address(usdc), 100e6, keccak256("E0"));
        bytes32[] memory expectRoot = new bytes32[](1);
        expectRoot[0] = leaves[0];
        vm.expectEmit(false, false, false, true, address(v));
        emit BridgeVault.StateUpdated(S0, s1, _refRoot(expectRoot), 0, leaves);
        v.update(j, seal);
        assertEq(v.g2State(), s1);
        (, uint64 claims, uint64 burns,) = v.counters(address(usdc), 0);
        assertEq(claims, 2);
        assertEq(burns, 1);
        assertEq(v.exitCount(), 1);
        assertGt(v.exitRootTime(v.exitRoot()), 0);

        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j, seal);

        bytes32 s2 = keccak256("s2");
        _update(_journal(s1, s2, new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0)));
        assertEq(v.g2State(), s2);
        assertEq(v.exitCount(), 1);
    }

    /// Any state, any batch (claims up to the deposits, exits up to the claims): the
    /// non-advancing journal is refused and changes nothing; the same batch moving to a new
    /// state then applies.
    function testFuzz_NoStateAdvanceNeverApplies(bytes32 s, uint8 nDep, uint8 nClaim, uint8 nExit) public {
        uint256 d = bound(nDep, 0, 4);
        uint256 c = bound(nClaim, 0, d);
        uint256 e = bound(nExit, 0, c);
        for (uint256 i; i < d; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        if (s != S0) _update(_journal(S0, s, new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0)));
        assertEq(v.g2State(), s);
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](c == 0 ? 0 : 1);
        if (c != 0) cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(c));
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](e);
        for (uint256 i; i < e; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("e", i)));
        bytes memory before = _outboundSnapshot();

        bytes memory j = _journal(s, s, new bytes32[](0), cl, ex);
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.NoStateAdvance.selector);
        v.update(j, seal);
        assertEq(_outboundSnapshot(), before, "a refused update changed the vault");

        bytes32 next = keccak256(abi.encode(s));
        _update(_journal(s, next, new bytes32[](0), cl, ex));
        assertEq(v.g2State(), next);
        assertEq(v.exitCount(), e);
        (, uint64 claims,,) = v.counters(address(usdc), 0);
        assertEq(claims, c);
    }

    function test_RevertWrongConfig() public {
        bytes memory j = abi.encode(
            BridgeVault.UpdateJournal(keccak256("other"), S0, keccak256("s1"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0))
        );
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.WrongConfig.selector);
        v.update(j, seal);
    }

    function test_RevertUpdateBadSeal() public {
        bytes memory j = _journal(S0, keccak256("s1"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G3, sha256(j)); // proof of the wrong program
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.update(j, seal);
    }

    // ------------------------------------------------------------------ release (G3)

    function test_ReleasePaysRecipientAndRelayer() public {
        bytes32 root = _bridgeOut(1, 1);
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 2e6);

        vm.expectRevert(BridgeVault.ReleaseTooEarly.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);

        vm.warp(block.timestamp + DELAY);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 100e6 - 1e6 - 2e6);
        assertEq(usdc.balanceOf(relayer), 2e6);
        assertTrue(v.releaseNullifierSpent(nf));

        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);
    }

    function test_RevertRelayerRedirectsOrOvercharges() public {
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 2e6);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.release(seal, root, nf, address(usdc), 0, address(0xBAD), relayer, 2e6);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, address(0xBAD), 2e6);
        // an overcharge is off the menu, so it is refused before the proof is checked
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 5e6);
    }

    // ------------------------------------------------------------------ zero relayer

    /// One 1 ETH exit (denomination 1), releasable now.
    function _ethExit() internal returns (bytes32 root) {
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("c"));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 1, keccak256("e"));
        _update(_journal(v.g2State(), keccak256("s1"), roots, cl, ex));
        root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
    }

    /// relayer == 0 with a fee: refused before verification (a garbage seal gets ZeroRelayer,
    /// not BadSeal), even with a seal that proves exactly those arguments. Fee 0 still works.
    function test_RevertZeroRelayerWithFeeERC20() public {
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        vm.expectRevert(BridgeVault.ZeroRelayer.selector);
        v.release(hex"deadbeef", root, nf, address(usdc), 0, bob, address(0), 2e6);
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, address(0), 2e6);
        vm.expectRevert(BridgeVault.ZeroRelayer.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, address(0), 2e6);
        v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, address(0), 0), root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 99e6);
    }

    function test_RevertZeroRelayerWithFeeETH() public {
        bytes32 root = _ethExit();
        bytes32 nf = keccak256("nf");
        uint256 burnt = address(0).balance;
        vm.expectRevert(BridgeVault.ZeroRelayer.selector);
        v.release(hex"deadbeef", root, nf, address(0), 1, bob, address(0), 0.002 ether);
        bytes memory seal = _releaseSeal(root, nf, address(0), 1 ether, bob, address(0), 0.002 ether);
        vm.expectRevert(BridgeVault.ZeroRelayer.selector);
        v.release(seal, root, nf, address(0), 1, bob, address(0), 0.002 ether);
        v.release(_releaseSeal(root, nf, address(0), 1 ether, bob, address(0), 0), root, nf, address(0), 1, bob, address(0), 0);
        assertEq(bob.balance, 1 ether - 0.01 ether);
        assertEq(address(0).balance, burnt);
    }

    // ------------------------------------------------------------------ relay-fee menu (F#13)

    function test_RelayFeeMenuSetAndView() public {
        uint256[] memory m0 = new uint256[](4);
        (m0[0], m0[1], m0[2], m0[3]) = (0.5e6, 1e6, 1.5e6, 2e6);
        uint256[][] memory menus = _menu2(m0, _one(20e6));
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.RelayFeeMenuSet(address(usdc), menus);
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), menus);
        assertEq(v.relayFeeMenu(address(usdc), 0), m0);
        assertEq(v.relayFeeMenu(address(usdc), 1), _one(20e6));
        assertTrue(v.relayFeeAllowed(address(usdc), 0, 1.5e6));
        assertTrue(v.relayFeeAllowed(address(usdc), 1, 20e6));
        assertFalse(v.relayFeeAllowed(address(usdc), 1, 2e6));

        // replacing drops the old values
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(1e6), new uint256[](0)));
        assertFalse(v.relayFeeAllowed(address(usdc), 0, 2e6));
        assertFalse(v.relayFeeAllowed(address(usdc), 1, 20e6));
        assertTrue(v.relayFeeAllowed(address(usdc), 0, 1e6));
        assertEq(v.relayFeeMenu(address(usdc), 1).length, 0);
    }

    function test_RevertBadRelayFeeMenu() public {
        uint256[] memory e = new uint256[](0);
        uint256[] memory five = new uint256[](5);
        (five[0], five[1], five[2], five[3], five[4]) = (1, 2, 3, 4, 5);
        uint256[] memory dup = new uint256[](2);
        (dup[0], dup[1]) = (1e6, 1e6);

        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(e, e));

        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.NotRegistered.selector);
        v.setRelayFeeMenu(address(0xDEAD), _menu2(e, e));
        uint256[][] memory short = new uint256[][](1);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.setRelayFeeMenu(address(usdc), short);
        vm.expectRevert(BridgeVault.BadRelayFeeMenu.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(five, e)); // more than 4
        vm.expectRevert(BridgeVault.BadRelayFeeMenu.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(0), e)); // 0 is implicit
        vm.expectRevert(BridgeVault.BadRelayFeeMenu.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(dup, e));
        // 2% of 100 USDC is 2 USDC
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(2e6 + 1), e));
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.setRelayFeeMenu(address(usdc), _menu2(e, _one(20e6 + 1)));
        vm.stopPrank();
        // a failed call leaves the menu as it was
        assertEq(v.relayFeeMenu(address(usdc), 0), _one(2e6));
    }

    /// A valid proof with a fee that isn't on the menu is refused, before verification.
    function test_RevertRelayFeeNotOnMenu() public {
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(hex"deadbeef", root, nf, address(usdc), 0, bob, relayer, 1.5e6);
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 1.5e6);
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 1.5e6);
        // a value on another denomination's menu doesn't count here
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(2e6), _one(1.5e6)));
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 1.5e6);
    }

    /// Exits burned on LEZ with maxRelayFee = 2 USDC. Menu changes after the burn: a seal
    /// naming a removed fee stops working, but the nullifier is unspent, so the same exit
    /// releases with any fee still on the menu (<= its max) or with fee 0, even after the
    /// menu is wiped. Also holds for a deregistered token.
    function test_MenuChangeNeverStrandsExit() public {
        bytes32 root = _bridgeOut(3, 3);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf1 = keccak256("nf1");
        bytes32 nf2 = keccak256("nf2");
        bytes32 nf3 = keccak256("nf3");
        bytes memory old1 = _releaseSeal(root, nf1, address(usdc), 100e6, bob, relayer, 2e6);
        bytes memory old2 = _releaseSeal(root, nf2, address(usdc), 100e6, bob, relayer, 2e6);

        // 2 USDC removed, 1 USDC added
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(1e6), new uint256[](0)));
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(old1, root, nf1, address(usdc), 0, bob, relayer, 2e6);
        assertFalse(v.releaseNullifierSpent(nf1));
        v.release(_releaseSeal(root, nf1, address(usdc), 100e6, bob, relayer, 1e6), root, nf1, address(usdc), 0, bob, relayer, 1e6);
        assertEq(usdc.balanceOf(relayer), 1e6);

        // menu wiped: only self-submission with fee 0 is left, and it works
        vm.startPrank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(new uint256[](0), new uint256[](0)));
        v.deregisterToken(address(usdc));
        vm.stopPrank();
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(old2, root, nf2, address(usdc), 0, bob, relayer, 2e6);
        vm.prank(bob);
        v.release(_releaseSeal(root, nf2, address(usdc), 100e6, bob, address(0), 0), root, nf2, address(usdc), 0, bob, address(0), 0);

        // the menu can still be set on a deregistered token, and listed values work again
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(2e6), new uint256[](0)));
        v.release(_releaseSeal(root, nf3, address(usdc), 100e6, bob, relayer, 2e6), root, nf3, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 3 * 99e6 - 1e6 - 2e6);
        assertEq(usdc.balanceOf(relayer), 3e6);
    }

    function test_RevertUnknownExitRoot() public {
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector);
        v.release("", keccak256("x"), keccak256("nf"), address(usdc), 0, bob, relayer, 0);
    }

    /// Even with a broken release proof system, releases can't exceed proven burns.
    function test_RevertReleasesExceedBurns() public {
        bytes32 root = _bridgeOut(2, 1);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf1 = keccak256("nf1");
        bytes32 nf2 = keccak256("nf2");
        v.release(_releaseSeal(root, nf1, address(usdc), 100e6, bob, relayer, 0), root, nf1, address(usdc), 0, bob, relayer, 0);
        bytes memory s2 = _releaseSeal(root, nf2, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.ReleasesExceedBurns.selector);
        v.release(s2, root, nf2, address(usdc), 0, bob, relayer, 0);
    }

    function test_ReleaseETH() public {
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("c"));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 1, keccak256("e"));
        _update(_journal(S0, keccak256("s1"), roots, cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        v.release(_releaseSeal(root, nf, address(0), 1 ether, bob, relayer, 0.002 ether), root, nf, address(0), 1, bob, relayer, 0.002 ether);
        assertEq(bob.balance, 1 ether - 0.01 ether - 0.002 ether);
        assertEq(relayer.balance, 0.002 ether);
    }

    function test_RevertEthPayoutRejected() public {
        EthRejecter rej = new EthRejecter();
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("c"));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 1, keccak256("e"));
        _update(_journal(S0, keccak256("s1"), roots, cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(0), 1 ether, address(rej), relayer, 0);
        vm.expectRevert(BridgeVault.EthTransferFailed.selector);
        v.release(seal, root, nf, address(0), 1, address(rej), relayer, 0);
        assertFalse(v.releaseNullifierSpent(nf));
    }

    function test_ReleaseCap() public {
        bytes32 root = _bridgeOut(3, 3);
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 150e6);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf1 = keccak256("nf1");
        bytes32 nf2 = keccak256("nf2");
        v.release(_releaseSeal(root, nf1, address(usdc), 100e6, bob, relayer, 0), root, nf1, address(usdc), 0, bob, relayer, 0);
        bytes memory s2 = _releaseSeal(root, nf2, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.release(s2, root, nf2, address(usdc), 0, bob, relayer, 0);
    }

    function test_DeregisteredTokenStillReleases() public {
        bytes32 root = _bridgeOut(1, 1);
        vm.prank(admin);
        v.deregisterToken(address(usdc));
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 0), root, nf, address(usdc), 0, bob, relayer, 0);
        assertEq(usdc.balanceOf(bob), 99e6);
    }

    // ------------------------------------------------------------------ fees and solvency

    function test_FeesWithdrawOnlyFees() public {
        bytes32 root = _bridgeOut(2, 1);
        vm.warp(block.timestamp + DELAY);
        bytes32 nf = keccak256("nf");
        v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 0), root, nf, address(usdc), 0, bob, relayer, 0);
        v.withdrawFees(address(usdc));
        // 2 deposit fees + 1 withdraw fee
        assertEq(usdc.balanceOf(feeTo), 3e6);
        // exactly one outstanding denomination of principal remains
        assertEq(usdc.balanceOf(address(v)), 100e6);
    }

    /// Fuzz: whatever mix of deposits, claims, exits and releases is attempted, in both
    /// denominations and with exits and releases crossing denominations, the vault never pays
    /// out more principal than was deposited and its balance always covers what is left.
    function testFuzz_Solvency(
        uint8 dep0,
        uint8 dep1,
        uint8 cl0,
        uint8 cl1,
        uint8 ex0,
        uint8 ex1,
        uint8 rel0,
        uint8 rel1
    ) public {
        uint256 d0 = bound(dep0, 0, 12);
        uint256 d1 = bound(dep1, 0, 3);
        for (uint256 i; i < d0; ++i) _deposit(0, keccak256(abi.encode("c0", i)));
        for (uint256 i; i < d1; ++i) _deposit(1, keccak256(abi.encode("c1", i)));

        // claims and exits in separate updates, so an oversized claim doesn't hide the exits
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(bound(cl0, 0, 16)));
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, uint64(bound(cl1, 0, 5)));
        bytes memory j = _journal(v.g2State(), keccak256("s1"), new bytes32[](0), cl, new BridgeVault.Exit[](0));
        try v.update(j, mv.sealFor(G2, sha256(j))) {} catch {}

        uint256 n0 = bound(ex0, 0, 40);
        uint256 n1 = bound(ex1, 0, 5);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n0 + n1);
        for (uint256 i; i < n0 + n1; ++i) {
            ex[i] = BridgeVault.Exit(address(usdc), i < n0 ? 0 : 1, keccak256(abi.encode("e", i)));
        }
        j = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), ex);
        try v.update(j, mv.sealFor(G2, sha256(j))) {} catch {}

        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + DELAY);
        uint256 r0 = bound(rel0, 0, 45);
        uint256 r1 = bound(rel1, 0, 6);
        for (uint256 i; i < r0 + r1; ++i) {
            uint32 di = i < r0 ? 0 : 1;
            uint256 denom = di == 0 ? 100e6 : 1000e6;
            bytes32 nf = keccak256(abi.encode("nf", i));
            bytes memory s = _releaseSeal(root, nf, address(usdc), denom, bob, relayer, 0);
            try v.release(s, root, nf, address(usdc), di, bob, relayer, 0) {} catch {}
        }

        uint256 deposited;
        uint256 released;
        uint256[2] memory denoms = [uint256(100e6), uint256(1000e6)];
        for (uint256 k; k < 2; ++k) {
            (uint64 d, uint64 c, uint64 b, uint64 rel) = v.counters(address(usdc), k);
            assertLe(c, d);
            assertLe(rel, b);
            deposited += uint256(d) * denoms[k];
            released += uint256(rel) * denoms[k];
        }
        uint256 burned = v.burnedValue(address(usdc));
        uint256 claimed = v.claimedValue(address(usdc));
        assertLe(released, burned);
        assertLe(burned, claimed);
        assertLe(claimed, deposited);
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        assertEq(usdc.balanceOf(address(v)), deposited - released + fees);
        // what bob got is principal minus withdraw fees, never more than was deposited
        assertLe(usdc.balanceOf(bob), released);
    }

    // ------------------------------------------------------------------ rolling caps (F#11)

    /// The old tumbling window let 2x the cap through in two seconds across its boundary.
    /// The rolling estimate still counts the burst just before the boundary.
    function test_CapBurstAcrossBucketBoundary() public {
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, 250e6, type(uint256).max);
        vm.warp(1 days - 1); // last second of bucket [0, 1 day)
        _deposit(0, keccak256("1"));
        _deposit(0, keccak256("2"));
        vm.warp(1 days + 1); // two seconds later, in the next bucket
        _depExpect(address(usdc), keccak256("3"), BridgeVault.CapExceeded.selector);
        (,,, uint256 used,) = v.capState(address(usdc));
        assertEq(used, uint256(200e6) * (1 days - 1) / 1 days);
        // halfway through the bucket the earlier burst counts half: 100 + 100 fits, 300 doesn't
        vm.warp(1 days + 12 hours);
        _deposit(0, keccak256("3"));
        _depExpect(address(usdc), keccak256("4"), BridgeVault.CapExceeded.selector);
        (,,, used,) = v.capState(address(usdc));
        assertEq(used, 200e6);
        // two buckets later everything has aged out
        vm.warp(3 days);
        _deposit(0, keccak256("4"));
        _deposit(0, keccak256("5"));
    }

    /// A release refused by a cap consumes nothing and goes through unchanged later.
    function test_CapRejectedReleaseRetriesLater() public {
        bytes32 root = _bridgeOut(3, 3);
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 150e6);
        vm.warp(block.timestamp + DELAY);
        _rel(root, keccak256("nf1"));
        bytes32 nf2 = keccak256("nf2");
        bytes memory s2 = _releaseSeal(root, nf2, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.release(s2, root, nf2, address(usdc), 0, bob, relayer, 0);
        assertFalse(v.releaseNullifierSpent(nf2));
        (,,, uint64 rel) = v.counters(address(usdc), 0);
        assertEq(rel, 1);
        vm.warp(block.timestamp + 2 days);
        v.release(s2, root, nf2, address(usdc), 0, bob, relayer, 0);
        assertTrue(v.releaseNullifierSpent(nf2));
    }

    /// Two tokens each under their own cap together hit the global cap; weight 0 exempts.
    function test_GlobalDepositCapAcrossTokens() public {
        vm.startPrank(admin);
        v.setCaps(address(usdc), 1 days, 200e6, type(uint256).max);
        v.setCaps(address(usdt), 1 days, 200e6, type(uint256).max);
        v.setGlobalCaps(1 days, 250e6, type(uint256).max);
        v.setCapWeight(address(usdc), 1e18);
        v.setCapWeight(address(usdt), 1e18);
        vm.stopPrank();
        _deposit(0, keccak256("u1"));
        vm.prank(alice);
        v.deposit(address(usdt), 0, keccak256("t1"));
        (uint64 w, uint256 dc, uint256 rc, uint256 used, uint256 relUsed) = v.globalCapState();
        assertEq(w, 1 days);
        assertEq(dc, 250e6);
        assertEq(rc, type(uint256).max);
        assertEq(used, 200e6);
        assertEq(relUsed, 0);
        // USDC is at 100 of its own 200, but the global total would be 300 > 250
        _depExpect(address(usdc), keccak256("u2"), BridgeVault.CapExceeded.selector);
        _depExpect(address(usdt), keccak256("t2"), BridgeVault.CapExceeded.selector);
        // ETH has weight 0: not counted
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("e1"));
        // ETH at $3000 in 6-decimal USD units: 3000e6 per 1e18 wei, so weight 3e9
        vm.prank(admin);
        v.setCapWeight(address(0), 3e9);
        assertEq(v.capWeight(address(0)), 3e9);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.depositETH{value: 0.1 ether}(0, keccak256("e2")); // 300e6 units
        // only registered tokens take a weight
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotRegistered.selector);
        v.setCapWeight(address(0xDEAD), 1e18);
    }

    function test_GlobalReleaseCapAndExemption() public {
        uint256 t0 = 1_000_000;
        vm.warp(t0);
        bytes32 root = _bridgeOut(3, 3);
        bytes memory pWeight0 = abi.encode(address(usdc), uint256(0));
        vm.startPrank(admin);
        v.setGlobalCaps(1 days, type(uint256).max, 300e6);
        v.setCapWeight(address(usdc), 2e18); // 100 USDC counts 200 units
        bytes32 id = v.schedule(BridgeVault.Action.SetCapWeight, pWeight0);
        vm.stopPrank();
        vm.warp(v.scheduledEta(id));
        _rel(root, keccak256("nf1"));
        (,,,, uint256 relUsed) = v.globalCapState();
        assertEq(relUsed, 200e6);
        _relExpect(root, keccak256("nf2"), BridgeVault.CapExceeded.selector);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetCapWeight, pWeight0);
        _rel(root, keccak256("nf2"));
        _rel(root, keccak256("nf3"));
    }

    /// Lowering, raising, re-windowing and disabling caps never leaves releases stuck.
    function test_ChangingCapsDoesNotBrickReleases() public {
        uint256 t0 = 1_000_000;
        vm.warp(t0);
        bytes32 root = _bridgeOut(6, 6);
        bytes memory pRaise = abi.encode(address(usdc), uint64(1 days), type(uint256).max, uint256(1000e6));
        bytes memory pShort = abi.encode(address(usdc), uint64(1 hours), type(uint256).max, uint256(250e6));
        bytes memory pOff = abi.encode(address(usdc), uint64(0), uint256(0), uint256(0));
        bytes memory pGOff = abi.encode(uint64(0), uint256(0), uint256(0));
        bytes memory pLift = abi.encode(address(usdc), uint64(1 days), type(uint256).max, type(uint256).max);
        vm.startPrank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 150e6);
        v.setGlobalCaps(1 days, type(uint256).max, 1000e6);
        v.setCapWeight(address(usdc), 1e18);
        // Pre-schedule the weakening cap changes so they execute at t1 without rolling cap buckets.
        v.schedule(BridgeVault.Action.SetCaps, pRaise);
        v.schedule(BridgeVault.Action.SetCaps, pShort);
        v.schedule(BridgeVault.Action.SetCaps, pOff);
        v.schedule(BridgeVault.Action.SetGlobalCaps, pGOff);
        v.schedule(BridgeVault.Action.SetCaps, pLift);
        vm.stopPrank();
        uint256 t1 = t0 + ADMIN_DELAY;
        vm.warp(t1);
        _rel(root, keccak256("nf1"));
        // lower below current usage: refused, not broken (tightening is instant)
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 50e6);
        _relExpect(root, keccak256("nf2"), BridgeVault.CapExceeded.selector);
        // raise (timelocked): goes through once executed
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetCaps, pRaise);
        _rel(root, keccak256("nf2"));
        // shorter window (timelocked): usage (200) carries over, and a cap of 250 refuses a third 100
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetCaps, pShort);
        (,,,, uint256 relUsed) = v.capState(address(usdc));
        assertEq(relUsed, 200e6);
        _relExpect(root, keccak256("nf3"), BridgeVault.CapExceeded.selector);
        vm.warp(t1 + 2 hours);
        _rel(root, keccak256("nf3"));
        // longer global window (instant; usage carries over), then the token cap off entirely (timelocked)
        vm.startPrank(admin);
        v.setGlobalCaps(7 days, type(uint256).max, 1000e6);
        v.execute(BridgeVault.Action.SetCaps, pOff);
        vm.stopPrank();
        (,,,, uint256 gUsed) = v.globalCapState();
        assertEq(gUsed, 300e6);
        _rel(root, keccak256("nf4"));
        // re-enable with a window it never had (instant): starts empty
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 100e6);
        _rel(root, keccak256("nf5"));
        _relExpect(root, keccak256("nf6"), BridgeVault.CapExceeded.selector);
        // global off too, token cap lifted (both timelocked)
        vm.startPrank(admin);
        v.execute(BridgeVault.Action.SetGlobalCaps, pGOff);
        v.execute(BridgeVault.Action.SetCaps, pLift);
        vm.stopPrank();
        _rel(root, keccak256("nf6"));
    }

    // ------------------------------------------------------------------ freeze (F#14)

    function test_FreezeTokenDepositsOnlyReleasesFlow() public {
        bytes32 root = _bridgeOut(2, 2);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Frozen(address(usdc), true, false);
        vm.prank(freezer);
        v.freeze(address(usdc), true, false);
        (, bool df, bool rf,,,,,,) = v.tokenConfig(address(usdc));
        assertTrue(df);
        assertFalse(rf);
        _depExpect(address(usdc), keccak256("x"), BridgeVault.IsFrozen.selector);
        vm.prank(alice);
        v.deposit(address(usdt), 0, keccak256("t")); // other token unaffected
        vm.warp(block.timestamp + DELAY);
        _rel(root, keccak256("nf1"));
        // update still runs
        _update(_journal(v.g2State(), keccak256("s9"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0)));
    }

    function test_FreezeTokenReleasesOnlyDepositsFlow() public {
        bytes32 root = _bridgeOut(2, 2);
        vm.prank(freezer);
        v.freeze(address(usdc), false, true);
        (, bool df, bool rf,,,,,,) = v.tokenConfig(address(usdc));
        assertFalse(df);
        assertTrue(rf);
        vm.warp(block.timestamp + DELAY);
        _relExpect(root, keccak256("nf1"), BridgeVault.IsFrozen.selector);
        _deposit(0, keccak256("x"));
        // a per-token release freeze doesn't stop update
        _batch(1, "b1");
        // unfreezing deposits only leaves releases frozen
        _execAsAdmin(BridgeVault.Action.Unfreeze, abi.encode(address(usdc), true, false));
        _relExpect(root, keccak256("nf1"), BridgeVault.IsFrozen.selector);
        bytes memory pRel = abi.encode(address(usdc), false, true);
        vm.prank(admin);
        bytes32 idRel = v.schedule(BridgeVault.Action.Unfreeze, pRel);
        vm.warp(v.scheduledEta(idRel));
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Unfrozen(address(usdc), false, true);
        vm.prank(admin);
        v.execute(BridgeVault.Action.Unfreeze, pRel);
        _rel(root, keccak256("nf1"));
    }

    function test_FreezeAllDepositsOnly() public {
        bytes32 root = _bridgeOut(2, 2);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Frozen(v.GLOBAL(), true, false);
        vm.prank(freezer);
        v.freezeAll(true, false);
        assertTrue(v.globalDepositsFrozen());
        assertFalse(v.globalReleasesFrozen());
        _depExpect(address(usdc), keccak256("x"), BridgeVault.IsFrozen.selector);
        _depExpect(address(usdt), keccak256("y"), BridgeVault.IsFrozen.selector);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.depositETH{value: 0.1 ether}(0, keccak256("z"));
        // update and release keep working
        _update(_journal(v.g2State(), keccak256("s9"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0)));
        vm.warp(block.timestamp + DELAY);
        _rel(root, keccak256("nf1"));
        _execAsAdmin(BridgeVault.Action.UnfreezeAll, abi.encode(true, false));
        _deposit(0, keccak256("x"));
    }

    function test_FreezeAllReleasesOnly() public {
        bytes32 root = _bridgeOut(2, 2);
        vm.prank(freezer);
        v.freezeAll(false, true);
        vm.warp(block.timestamp + DELAY);
        _relExpect(root, keccak256("nf1"), BridgeVault.IsFrozen.selector);
        // the global release freeze stops state proofs too
        bytes memory j = _journal(v.g2State(), keccak256("s9"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory seal = mv.sealFor(G2, sha256(j));
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.update(j, seal);
        // deposits flow
        _deposit(0, keccak256("x"));
        vm.prank(alice);
        v.depositETH{value: 0.1 ether}(0, keccak256("z"));
        // clearing the deposit flag leaves the release flag set
        _execAsAdmin(BridgeVault.Action.UnfreezeAll, abi.encode(true, false));
        _relExpect(root, keccak256("nf1"), BridgeVault.IsFrozen.selector);
        _execAsAdmin(BridgeVault.Action.UnfreezeAll, abi.encode(false, true));
        v.update(j, seal);
        _rel(root, keccak256("nf1"));
    }

    /// Global and per-token flags are independent: either one blocks.
    function test_FreezeGlobalAndTokenIndependent() public {
        bytes32 root = _bridgeOut(2, 2);
        vm.warp(block.timestamp + DELAY);
        vm.startPrank(freezer);
        v.freeze(address(usdc), false, true);
        v.freezeAll(false, true);
        vm.stopPrank();
        _execAsAdmin(BridgeVault.Action.UnfreezeAll, abi.encode(false, true));
        _relExpect(root, keccak256("nf1"), BridgeVault.IsFrozen.selector); // token flag still set
        _execAsAdmin(BridgeVault.Action.Unfreeze, abi.encode(address(usdc), false, true));
        _rel(root, keccak256("nf1"));
        // admin can freeze too
        vm.prank(admin);
        v.freeze(address(usdc), true, false);
        _depExpect(address(usdc), keccak256("x"), BridgeVault.IsFrozen.selector);
    }

    // ------------------------------------------------------------------ release delay changes (F#12)

    /// Raising the delay: a root already releasable stays releasable, a root seen before the
    /// change keeps its shorter delay, a root seen after waits the new one.
    function test_RaiseDelayKeepsValidReleases() public {
        uint256 t0 = 1_000_000;
        vm.warp(t0);
        bytes32 r0 = _bridgeOut(2, 2); // seen at t0
        vm.warp(t0 + DELAY - 12 hours);
        bytes32 r1 = _batch(2, "b1"); // seen 12h before r0 becomes releasable
        vm.warp(t0 + DELAY); // r0 releasable, r1 not yet
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 7 days);
        vm.warp(t0 + DELAY + 1 hours);
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 7 days); // a no-op re-set keeps r1's shorter delay
        _rel(r0, keccak256("nf0")); // still releasable
        _relExpect(r1, keccak256("nf1"), BridgeVault.ReleaseTooEarly.selector);
        // a root in the same block as the change counts as before it, so step one second
        vm.warp(t0 + DELAY + 1);
        bytes32 r2 = _batch(2, "b2"); // after the change
        vm.warp(t0 + 2 * DELAY - 12 hours); // r1 is DELAY old: min(old, new)
        _rel(r1, keccak256("nf1"));
        vm.warp(t0 + DELAY + 1 + 7 days - 1);
        _relExpect(r2, keccak256("nf2"), BridgeVault.ReleaseTooEarly.selector);
        vm.warp(t0 + DELAY + 1 + 7 days);
        _rel(r2, keccak256("nf2"));
    }

    function test_LowerDelayAppliesToAll() public {
        // a literal, not block.timestamp: via-IR may re-read TIMESTAMP after a warp
        uint256 t0 = 1_000_000;
        bytes memory lowerParams = abi.encode(address(usdc), uint64(2 hours));
        vm.warp(t0 + 1 hours - ADMIN_DELAY);
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetReleaseDelay, lowerParams);
        vm.warp(t0);
        bytes32 r0 = _bridgeOut(2, 2);
        vm.warp(t0 + 1 hours);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetReleaseDelay, lowerParams);
        bytes32 r1 = _batch(2, "b1");
        vm.warp(t0 + 2 hours);
        _rel(r0, keccak256("nf0")); // old root, new shorter delay
        _relExpect(r1, keccak256("nf1"), BridgeVault.ReleaseTooEarly.selector);
        vm.warp(t0 + 3 hours);
        _rel(r1, keccak256("nf1"));
        vm.prank(admin);
        vm.expectRevert(BridgeVault.DelayTooShort.selector);
        v.setReleaseDelay(address(usdc), 30 minutes);
    }

    /// An emergency raise followed by an adjustment doesn't hand the roots seen during the
    /// raise the original short delay.
    function test_RaiseThenAdjustDoesNotWeaken() public {
        // a literal, not block.timestamp: via-IR may re-read TIMESTAMP after a warp
        uint256 t0 = 1_000_000;
        vm.warp(t0);
        bytes32 r0 = _bridgeOut(2, 2);
        vm.warp(t0 + DELAY); // r0 releasable
        bytes memory adjParams = abi.encode(address(usdc), uint64(3 days));
        vm.startPrank(admin);
        v.setReleaseDelay(address(usdc), 7 days);
        v.schedule(BridgeVault.Action.SetReleaseDelay, adjParams);
        vm.stopPrank();
        vm.warp(t0 + DELAY + 1);
        bytes32 r1 = _batch(2, "b1"); // seen under 7 days
        vm.warp(t0 + DELAY + 2 days);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetReleaseDelay, adjParams);
        _rel(r0, keccak256("nf0"));
        vm.warp(t0 + DELAY + 1 + 3 days - 1);
        _relExpect(r1, keccak256("nf1"), BridgeVault.ReleaseTooEarly.selector); // not DELAY
        vm.warp(t0 + DELAY + 1 + 3 days);
        _rel(r1, keccak256("nf1")); // min(7 days, 3 days)
    }

    /// Two raises in a row: what was releasable at either change stays releasable.
    function test_TwoRaises() public {
        // a literal, not block.timestamp: via-IR may re-read TIMESTAMP after a warp
        uint256 t0 = 1_000_000;
        vm.warp(t0);
        bytes32 r0 = _bridgeOut(2, 2);
        vm.warp(t0 + 12 hours);
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 3 days); // r0 keeps 1 day
        vm.warp(t0 + 18 hours);
        bytes32 r1 = _batch(2, "b1"); // waits 3 days
        vm.warp(t0 + DELAY); // r0 releasable now
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 7 days);
        (uint64 d, uint64 at, uint256 steps) = v.releaseDelayState(address(usdc));
        assertEq(d, 7 days);
        assertEq(at, t0 + DELAY);
        // history: 1 day until the first raise, 3 days until the second
        assertEq(steps, 2);
        (uint64 sd, uint64 until) = v.delaySteps(address(usdc), 0);
        assertEq(sd, DELAY);
        assertEq(until, t0 + 12 hours);
        (sd, until) = v.delaySteps(address(usdc), 1);
        assertEq(sd, 3 days);
        assertEq(until, t0 + DELAY);
        assertEq(v.releasableAt(address(usdc), r0), t0 + DELAY);
        assertEq(v.releasableAt(address(usdc), r1), t0 + 18 hours + 3 days);
        _rel(r0, keccak256("nf0"));
        vm.warp(t0 + 18 hours + 3 days - 1);
        _relExpect(r1, keccak256("nf1"), BridgeVault.ReleaseTooEarly.selector);
        vm.warp(t0 + 18 hours + 3 days);
        _rel(r1, keccak256("nf1"));
        // a third change long after: everything seen before it was releasable, nothing waits again
        bytes32 r2 = _batch(2, "b2");
        vm.warp(block.timestamp + 7 days);
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 30 days);
        _rel(r0, keccak256("nf0b"));
        _rel(r2, keccak256("nf2"));
    }
}
