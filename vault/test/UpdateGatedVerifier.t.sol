// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {UpdateGatedVerifier, IGatedVault} from "../src/UpdateGatedVerifier.sol";
import {MockGroth16Verifier, TestToken} from "./mocks/Mocks.sol";
import {OutboundVector as OV} from "./fixtures/OutboundVector.sol";

/// A submitter that is a contract: submits through the gate, then tries a direct `update` in the
/// SAME transaction (transient storage lives for the whole transaction, so the gate must have
/// cleared its mark), or reenters the gate.
contract SubmitterBot {
    UpdateGatedVerifier public gate;

    function setGate(UpdateGatedVerifier g) external {
        gate = g;
    }

    function submitThenDirect(IGatedVault v, bytes calldata j1, bytes calldata s1, bytes calldata j2, bytes calldata s2)
        external
    {
        gate.submitUpdate(v, j1, s1);
        v.update(j2, s2);
    }

    function submit(IGatedVault v, bytes calldata j, bytes calldata s) external {
        gate.submitUpdate(v, j, s);
    }

    /// Called back by EvilVault while the gate is open.
    function reenter(IGatedVault v, bytes calldata j, bytes calldata s) external {
        gate.submitUpdate(v, j, s);
    }
}

/// Claims to be a gated vault; inside `update` it does whatever `mode` says.
contract EvilVault {
    UpdateGatedVerifier immutable gate;
    bytes32 public immutable updateImageId;
    uint8 public mode; // 1: reenter the gate through the bot, 2: borrow the gate for `victim`
    SubmitterBot bot;
    IGatedVault victim;

    constructor(UpdateGatedVerifier g) {
        gate = g;
        updateImageId = g.UPDATE_IMAGE_ID();
    }

    function verifier() external view returns (IRiscZeroVerifier) {
        return gate;
    }

    function arm(uint8 m, SubmitterBot b, IGatedVault v) external {
        (mode, bot, victim) = (m, b, v);
    }

    function update(bytes calldata journal, bytes calldata seal) external {
        if (mode == 1) bot.reenter(IGatedVault(address(this)), journal, seal);
        if (mode == 2) victim.update(journal, seal);
    }
}

contract UpdateGatedVerifierTest is Test {
    bytes4 constant SEL = 0xe71e6620;
    bytes32 constant UPDATE_ID = keccak256("test: update image");
    bytes32 constant RELEASE_ID = keccak256("test: release image");
    bytes32 constant PROGRAM_ID = keccak256("test: program");
    bytes32 constant LEZ_CHAIN_ID = keccak256("test: lez chain");
    bytes32 constant ZONE_ID = keccak256("test: zone");
    bytes32 constant G0 = keccak256("test: genesis state");

    MockGroth16Verifier r0;
    UpdateGatedVerifier gate;
    BridgeVault v;
    TestToken usdc;
    address submitter = address(0x5AB);
    address bob = address(0xB0B);
    address anyone = address(0xA11CE);

    function _vault(UpdateGatedVerifier g, bytes32 genesis) internal returns (BridgeVault) {
        return new BridgeVault(
            IRiscZeroVerifier(address(g)), UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, genesis, 0,
            address(this), address(this), address(this), 1 hours
        );
    }

    function setUp() public {
        r0 = new MockGroth16Verifier(SEL);
        gate = new UpdateGatedVerifier(r0, UPDATE_ID, submitter);
        v = _vault(gate, G0);
        usdc = new TestToken("USDC", 6);
        uint256[] memory d = new uint256[](1);
        d[0] = 100e6;
        v.registerToken(address(usdc), 6, d, new uint256[](1), new uint256[](1), 0);
        usdc.mint(address(this), 100e6);
        usdc.approve(address(v), 100e6);
        v.deposit(address(usdc), 0, keccak256("C"));
    }

    /// One claim and one exit of the deposit, from `prev` to `next`.
    function _journal(BridgeVault vault, bytes32 prev, bytes32 next) internal view returns (bytes memory) {
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E"));
        return abi.encode(BridgeVault.UpdateJournal(vault.cfgId(), prev, next, new bytes32[](0), cl, ex));
    }

    /// An update with nothing in it, from `prev` to `next`.
    function _empty(BridgeVault vault, bytes32 prev, bytes32 next) internal view returns (bytes memory) {
        return abi.encode(
            BridgeVault.UpdateJournal(
                vault.cfgId(), prev, next, new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0)
            )
        );
    }

    function _seal(bytes32 img, bytes memory journal) internal view returns (bytes memory) {
        return r0.sealFor(img, sha256(journal));
    }

    function test_Constructor() public {
        assertEq(address(gate.VERIFIER()), address(r0));
        assertEq(gate.UPDATE_IMAGE_ID(), UPDATE_ID);
        assertEq(gate.SUBMITTER(), submitter);
        vm.expectRevert(UpdateGatedVerifier.ZeroAddress.selector);
        new UpdateGatedVerifier(IRiscZeroVerifier(address(0)), UPDATE_ID, submitter);
        vm.expectRevert(UpdateGatedVerifier.ZeroAddress.selector);
        new UpdateGatedVerifier(r0, UPDATE_ID, address(0));
        vm.expectRevert(UpdateGatedVerifier.ZeroImageId.selector);
        new UpdateGatedVerifier(r0, bytes32(0), submitter);
    }

    /// The v4 order: the first update starts from the genesis state, through the gate; then a
    /// release proof passes straight through, sent by anyone.
    function test_SubmitFromGenesisThenPermissionlessRelease() public {
        bytes memory j = _journal(v, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(submitter);
        gate.submitUpdate(IGatedVault(address(v)), j, s);
        assertEq(v.g2State(), keccak256("s1"));
        assertEq(v.exitCount(), 1);

        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256("nf");
        bytes32 dig = sha256(abi.encode(v.cfgId(), root, nf, address(usdc), uint256(100e6), bob, address(0), uint256(0)));
        bytes memory rs = r0.sealFor(RELEASE_ID, dig);
        vm.prank(anyone);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    function test_PrevStateZeroRefusedEvenThroughTheGate() public {
        bytes memory j = _journal(v, bytes32(0), keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(submitter);
        vm.expectRevert(BridgeVault.StaleState.selector);
        gate.submitUpdate(IGatedVault(address(v)), j, s);
    }

    function test_DirectUpdateRefusedForEveryone() public {
        bytes memory j = _journal(v, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(anyone);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        v.update(j, s);
        // the submitter too: only `submitUpdate` opens the gate
        vm.prank(submitter);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        v.update(j, s);
        // and `verify` for the update image, called directly
        bytes32 dj = sha256(j);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        gate.verify(s, UPDATE_ID, dj);
    }

    function test_OnlyTheSubmitterSubmits() public {
        bytes memory j = _journal(v, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(anyone);
        vm.expectRevert(UpdateGatedVerifier.NotSubmitter.selector);
        gate.submitUpdate(IGatedVault(address(v)), j, s);
    }

    function test_TheGateStillNeedsTheRealVerifier() public {
        bytes memory j = _journal(v, G0, keccak256("s1"));
        // another image's seal, and a seal over another journal
        bytes memory other = _seal(RELEASE_ID, j);
        bytes memory j2 = _journal(v, G0, keccak256("s2"));
        bytes memory s2 = _seal(UPDATE_ID, j2);
        vm.startPrank(submitter);
        vm.expectRevert(MockGroth16Verifier.BadSeal.selector);
        gate.submitUpdate(IGatedVault(address(v)), j, other);
        vm.expectRevert(MockGroth16Verifier.BadSeal.selector);
        gate.submitUpdate(IGatedVault(address(v)), j, s2);
        vm.stopPrank();
        // a release seal for the wrong digest is refused by the verifier behind the gate
        bytes memory jj = _journal(v, G0, keccak256("s1"));
        bytes memory ss = _seal(UPDATE_ID, jj);
        vm.prank(submitter);
        gate.submitUpdate(IGatedVault(address(v)), jj, ss);
        bytes32 root = v.exitRoot();
        bytes memory bad = r0.sealFor(RELEASE_ID, keccak256("other"));
        vm.expectRevert(MockGroth16Verifier.BadSeal.selector);
        v.release(bad, root, keccak256("nf"), address(usdc), 0, bob, address(0), 0);
    }

    /// The transient mark is cleared before `submitUpdate` returns: a direct `update` later in the
    /// same transaction is refused.
    function test_MarkClearedWithinTheTransaction() public {
        SubmitterBot bot = new SubmitterBot();
        UpdateGatedVerifier g = new UpdateGatedVerifier(r0, UPDATE_ID, address(bot));
        bot.setGate(g);
        BridgeVault v2 = _vault(g, G0);
        bytes memory j1 = _empty(v2, G0, keccak256("s1"));
        bytes memory j2 = _empty(v2, keccak256("s1"), keccak256("s2"));
        bytes memory s1 = _seal(UPDATE_ID, j1);
        bytes memory s2 = _seal(UPDATE_ID, j2);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        bot.submitThenDirect(IGatedVault(address(v2)), j1, s1, j2, s2);
        // the same two updates, both through the gate, apply
        bot.submit(IGatedVault(address(v2)), j1, s1);
        bot.submit(IGatedVault(address(v2)), j2, s2);
        assertEq(v2.g2State(), keccak256("s2"));
    }

    /// No nested submit while the gate is open (a vault calling back into the submitter).
    function test_NoReentry() public {
        SubmitterBot bot = new SubmitterBot();
        UpdateGatedVerifier g = new UpdateGatedVerifier(r0, UPDATE_ID, address(bot));
        bot.setGate(g);
        EvilVault evil = new EvilVault(g);
        evil.arm(1, bot, IGatedVault(address(0)));
        vm.expectRevert(UpdateGatedVerifier.Busy.selector);
        bot.submit(IGatedVault(address(evil)), hex"01", hex"02");
    }

    /// The mark names the vault it was opened for: a submit for vault A cannot be used to
    /// update vault B (which uses the same gate) with the same journal.
    function test_ASecondVaultCannotBorrowTheMark() public {
        SubmitterBot bot = new SubmitterBot();
        UpdateGatedVerifier g = new UpdateGatedVerifier(r0, UPDATE_ID, address(bot));
        bot.setGate(g);
        BridgeVault victim = _vault(g, G0);
        EvilVault evil = new EvilVault(g);
        evil.arm(2, bot, IGatedVault(address(victim)));
        bytes memory j = _empty(victim, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        bot.submit(IGatedVault(address(evil)), j, s);
        assertEq(victim.g2State(), G0);
    }

    /// A vault that does not use this gate, or pins another update image, is refused.
    function test_WrongVaultRefused() public {
        UpdateGatedVerifier other = new UpdateGatedVerifier(r0, UPDATE_ID, submitter);
        BridgeVault elsewhere = _vault(other, G0);
        bytes memory j = _empty(elsewhere, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(submitter);
        vm.expectRevert(UpdateGatedVerifier.WrongVault.selector);
        gate.submitUpdate(IGatedVault(address(elsewhere)), j, s);

        UpdateGatedVerifier g2 = new UpdateGatedVerifier(r0, keccak256("another update image"), submitter);
        BridgeVault mismatched = _vault(g2, G0); // pins UPDATE_ID, the gate holds back another id
        vm.prank(submitter);
        vm.expectRevert(UpdateGatedVerifier.WrongVault.selector);
        g2.submitUpdate(IGatedVault(address(mismatched)), j, s);
    }

    /// The freezer's brake: a global release freeze stops releases and updates, even through the gate.
    function test_FreezeStopsGatedUpdates() public {
        v.freezeAll(false, true);
        bytes memory j = _journal(v, G0, keccak256("s1"));
        bytes memory s = _seal(UPDATE_ID, j);
        vm.prank(submitter);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        gate.submitUpdate(IGatedVault(address(v)), j, s);
    }
}

/// Against RISC Zero's real RiscZeroGroth16Verifier 3.1.0 on a Sepolia fork, with RISC Zero's own
/// test receipt (risc0-ethereum v3.1.0 `contracts/test/TestReceiptV3_1.sol`, Apache-2.0). Skipped
/// unless SEPOLIA_FORK_RPC is set (CI has no network):
///   SEPOLIA_FORK_RPC=https://sepolia.gateway.tenderly.co forge test --mc UpdateGatedVerifierForkTest
contract UpdateGatedVerifierForkTest is Test {
    address constant V3_1 = 0x8c915b5071206C6E328BC18529586c513408aB3b;
    bytes constant SEAL =
        hex"e71e662021a0c81a5cc1c5cddabc3eefe6a3841315ad1a1cd74da3f34db1db91d01240a70b220d069e774da52f06c0d0ca166b219f0e1b0548a08270fa88f5e71a55bb2911fba8d9c52968295894f9b6e1804afcda16b66956ad973a290cb8d6736813502b336b4ec0382ab1a7508781f3e1a8d7ea73d4491382cae39ec9f14aff07f53623bc5d3d6a718a686588bd6027b1713110031c85fa34117a4fed94d98a41fe8e0bcf572d3389eedcc34ec00bd1aa571b0bcaa80af13b30bf3cb26741e56deabb23d1abf7efdfba3f55523687ce10ef220c8d6d879f2e9161684f044e6e3597a2087d050b687d2242476dde08f789d726635f51ccd6fc216091297f0b7b416a6a";
    bytes constant JOURNAL = hex"6a75737420612073696d706c652072656365697074";
    bytes32 constant IMAGE_ID = 0xc92414d80cdeb43ba0f4ff4882447c21f18b547d41ffa506a84da164e8e31f7d;

    function setUp() public {
        string memory rpc = vm.envOr("SEPOLIA_FORK_RPC", string(""));
        if (bytes(rpc).length == 0) vm.skip(true);
        vm.createSelectFork(rpc);
    }

    function test_RealVerifierBehindTheGate() public {
        bytes memory seal = SEAL;
        // a release-path image passes straight through to RISC Zero's verifier
        UpdateGatedVerifier g = new UpdateGatedVerifier(IRiscZeroVerifier(V3_1), keccak256("another image"), address(this));
        bytes32 dj = sha256(JOURNAL);
        bytes32 other = sha256("another journal");
        g.verify(seal, IMAGE_ID, dj);
        vm.expectRevert(bytes4(0x439cc0cd)); // VerificationFailed()
        g.verify(seal, IMAGE_ID, other);
        // the same receipt as the update image: refused outside submitUpdate
        UpdateGatedVerifier h = new UpdateGatedVerifier(IRiscZeroVerifier(V3_1), IMAGE_ID, address(this));
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        h.verify(seal, IMAGE_ID, dj);
        // ... and accepted inside submitUpdate, checked by RISC Zero's verifier with the real seal
        ForkVault fv = new ForkVault(h);
        h.submitUpdate(IGatedVault(address(fv)), JOURNAL, seal);
        assertTrue(fv.updated());
    }
}

/// The smallest vault that calls its verifier as BridgeVault.update does.
contract ForkVault {
    UpdateGatedVerifier immutable gate;
    bool public updated;

    constructor(UpdateGatedVerifier g) {
        gate = g;
    }

    function verifier() external view returns (IRiscZeroVerifier) {
        return gate;
    }

    function updateImageId() external view returns (bytes32) {
        return gate.UPDATE_IMAGE_ID();
    }

    function update(bytes calldata journal, bytes calldata seal) external {
        gate.verify(seal, gate.UPDATE_IMAGE_ID(), sha256(journal));
        updated = true;
    }
}

/// The real G2-lite journals of test/OutboundVector.t.sol (TrustedTip guest, executed on LEZ
/// blocks 2893..2904 of run public9) through the gate: the first starts from the vault's genesis
/// state, both apply only through `submitUpdate`. The vault is rebuilt as in OutboundVectorTest
/// (same address, chain id and deposits) with the gate as its verifier.
contract OutboundVectorGatedTest is Test {
    address constant VAULT = 0x1429859428C0aBc9C2C47C8Ee9FBaf82cFA0F20f;
    address constant USDC = 0xcbEAF3BDe82155F56486Fb5a1072cb8baAf547cc;
    bytes32 constant PROGRAM_ID = 0xe50aa81eca63f47552b2d56a12fb279941b51aa8a74ba814a59532b28e8b6242;
    bytes32 constant CFG_ID = 0x08897d4523bbf36bacbd0b7403e7e0b1c04efa0b3bdd1ad9450661ba6371c580;

    BridgeVault v;
    MockGroth16Verifier r0;
    UpdateGatedVerifier gate;
    address submitter = address(0x5AB);

    function setUp() public {
        vm.chainId(31337);
        r0 = new MockGroth16Verifier(0xe71e6620);
        gate = new UpdateGatedVerifier(r0, OV.UPDATE_IMAGE_ID, submitter);
        bytes memory args = abi.encode(
            address(gate), OV.UPDATE_IMAGE_ID, keccak256("g3"), bytes32(0), bytes32(0), PROGRAM_ID, OV.GENESIS_STATE,
            uint64(0), address(this), address(this), address(this), uint64(1 hours)
        );
        deployCodeTo("BridgeVault.sol:BridgeVault", args, VAULT);
        v = BridgeVault(VAULT);
        assertEq(v.cfgId(), CFG_ID);
        vm.etch(USDC, address(new TestToken("USDC", 6)).code);
        uint256[] memory d = new uint256[](2);
        d[0] = 100e6;
        d[1] = 1000e6;
        uint256[] memory f = new uint256[](2);
        v.registerToken(USDC, 6, d, f, f, 0);
        TestToken(USDC).mint(address(this), 1100e6);
        TestToken(USDC).approve(VAULT, type(uint256).max);
        v.deposit(USDC, 0, 0xc44725d218317976838f42c13d8a281490b76b98074c80d0dfd5e4b41bb5a2eb);
        v.deposit(USDC, 1, 0x7678c51bd5a2fa92e78db027890009818ac932c3433d1b2854cc9c8220f5045c);
    }

    function test_GuestJournalsFromGenesisThroughTheGate() public {
        bytes memory j1 = OV.JOURNAL_1;
        bytes memory j2 = OV.JOURNAL_2;
        bytes memory s1 = r0.sealFor(OV.UPDATE_IMAGE_ID, sha256(j1));
        bytes memory s2 = r0.sealFor(OV.UPDATE_IMAGE_ID, sha256(j2));
        assertEq(v.g2State(), OV.GENESIS_STATE);
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        v.update(j1, s1);
        vm.startPrank(submitter);
        vm.expectRevert(BridgeVault.StaleState.selector); // the second batch cannot come first
        gate.submitUpdate(IGatedVault(VAULT), j2, s2);
        gate.submitUpdate(IGatedVault(VAULT), j1, s1);
        assertEq(v.g2State(), OV.STATE_1);
        gate.submitUpdate(IGatedVault(VAULT), j2, s2);
        vm.stopPrank();
        assertEq(v.g2State(), OV.STATE_2);
        assertEq(v.exitCount(), 1);
        assertEq(v.burnedValue(USDC), 100e6);
    }
}
