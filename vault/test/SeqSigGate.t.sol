// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {SeqSigGate, ISeqSigVault} from "../src/SeqSigGate.sol";
import {MockVerifier, TestToken} from "./mocks/Mocks.sol";

/// Finalizes through the gate, then tries a direct `update` in the SAME transaction (transient
/// storage lives for the whole transaction, so the gate must have cleared its mark).
contract FinalizeThenDirect {
    function run(SeqSigGate g, BridgeVault v, bytes calldata vj, bytes calldata vj2) external {
        g.finalize(vj);
        v.update(vj2, "");
    }
}

/// Claims to be a vault wired to the gate (its cfgId, update image and an old g2State), and tries
/// to get the gate to accept its `update` through `verify`.
contract FakeVault {
    SeqSigGate immutable gate;
    bytes32 public immutable cfgId;
    bytes32 public immutable updateImageId;
    bytes32 public g2State;

    constructor(SeqSigGate g, bytes32 genesis) {
        (gate, cfgId, updateImageId, g2State) = (g, g.CFG_ID(), g.UPDATE_IMAGE_ID(), genesis);
    }

    function verifier() external view returns (IRiscZeroVerifier) {
        return gate;
    }

    function update(bytes calldata journal, bytes calldata seal) external view {
        gate.verify(seal, updateImageId, sha256(journal));
    }
}

/// The seq-sig gate in front of an unchanged BridgeVault, with the test MockVerifier.
///
/// Signature COUNTING happens inside the update guest (`methods/outbound_guest_seqsig`): with too
/// few signers, a duplicate, an outside or ejected key, a small-order or non-canonical key, or a
/// non-canonical S, the guest commits no journal, so no seal exists. Those cases are tested in Rust
/// (`outbound-seqsig/core`, and both guests in RISC Zero's executor in
/// `outbound-seqsig/host/tests/seqsig.rs`) and in the end-to-end run. Here the journals stand for what
/// the guests commit; the tests check the on-chain layer.
contract SeqSigGateTest is Test {
    bytes32 constant UPDATE_ID = keccak256("test: seq-sig update image");
    bytes32 constant EVIDENCE_ID = keccak256("test: seq-sig evidence image");
    bytes32 constant RELEASE_ID = keccak256("test: release image");
    bytes32 constant TRUSTED_TIP_ID = keccak256("test: TrustedTip update image");
    bytes32 constant PROGRAM_ID = keccak256("test: program");
    bytes32 constant LEZ_CHAIN_ID = keccak256("test: lez chain");
    bytes32 constant ZONE_ID = keccak256("test: zone");
    bytes32 constant G0 = keccak256("test: genesis state");
    uint256 constant T = 3;
    uint64 constant WINDOW = 6 hours;
    uint256 constant PREV_ID = 100;
    bytes32 constant PREV_HASH = keccak256("test: LEZ block 100");
    uint256 constant BLOCK = 120;
    bytes32 constant HASH = keccak256("test: LEZ block 120");
    bytes32 constant OTHER_HASH = keccak256("test: a conflicting LEZ block 120");
    bytes32 constant HASH2 = keccak256("test: LEZ block 120 on the next round");

    MockVerifier mv;
    SeqSigGate gate;
    BridgeVault v;
    TestToken usdc;
    bytes32[] keys;
    bytes32 set4;
    address anyone = address(0xA11CE);
    address other = address(0x07E5);
    address bob = address(0xB0B);

    function _keys(uint256 n) internal pure returns (bytes32[] memory k) {
        k = new bytes32[](n);
        for (uint256 i; i < n; ++i) k[i] = keccak256(abi.encode("test key", i)) >> 8; // y < p, sorted below
        for (uint256 i; i < n; ++i) {
            for (uint256 j = i + 1; j < n; ++j) {
                if (uint256(k[j]) < uint256(k[i])) (k[i], k[j]) = (k[j], k[i]);
            }
        }
    }

    /// A gate alone, bound to a placeholder vault address (constructor tests).
    function _gate(bytes32[] memory k, uint256 t, uint64 window, bool testOnly) internal returns (SeqSigGate) {
        return new SeqSigGate(
            mv, ISeqSigVault(address(0xDEAD)), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, k, t, window, testOnly
        );
    }

    /// Expects the gate's constructor to revert with `err`. (A vm.expectRevert before an internal
    /// helper that creates the gate is not portable across Foundry versions: newer ones end the test
    /// at that revert, silently skipping the rest.)
    function makeGate(bytes32[] memory k, uint256 t, uint64 window, bool testOnly) external returns (SeqSigGate) {
        return _gate(k, t, window, testOnly);
    }

    function _gateReverts(bytes32[] memory k, uint256 t, uint64 window, bool testOnly, bytes memory err) internal {
        try this.makeGate(k, t, window, testOnly) {
            fail();
        } catch (bytes memory r) {
            assertEq(r, err);
        }
    }

    /// A gate bound to the vault deployed right after it (the predicted CREATE address), and that
    /// vault with the gate as its verifier: the deploy order of script/SeqSigDeploy.s.sol.
    function _deploy(bytes32[] memory k, uint256 t, uint64 window, bool testOnly)
        internal
        returns (SeqSigGate g, BridgeVault w)
    {
        address predicted = vm.computeCreateAddress(address(this), vm.getNonce(address(this)) + 1);
        g = new SeqSigGate(
            mv, ISeqSigVault(predicted), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, k, t, window, testOnly
        );
        w = new BridgeVault(
            IRiscZeroVerifier(address(g)), UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, G0, 0,
            address(this), address(this), address(this), 1 hours
        );
        assertEq(address(w), predicted);
        assertEq(w.cfgId(), g.CFG_ID());
    }

    function setUp() public {
        mv = new MockVerifier();
        keys = _keys(4);
        usdc = new TestToken("USDC", 6);
        (gate, v) = _deploy(keys, T, WINDOW, false);
        set4 = gate.SIGNER_SET_HASH();
        _register(v);
        usdc.mint(address(this), 1000e6);
        for (uint256 i; i < 4; ++i) v.deposit(address(usdc), 0, keccak256(abi.encode("C", i)));
    }

    function _register(BridgeVault w) internal {
        uint256[] memory d = new uint256[](1);
        d[0] = 100e6;
        w.registerToken(address(usdc), 6, d, new uint256[](1), new uint256[](1), 0);
        usdc.approve(address(w), type(uint256).max);
    }

    // ------------------------------------------------------------------ helpers

    function _vjFor(BridgeVault w, bytes32 cfg, bytes32 prev, bytes32 next, uint64 claims, uint256 exits)
        internal
        view
        returns (bytes memory)
    {
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = w.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](claims == 0 ? 0 : 1);
        if (claims != 0) cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, claims);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](exits);
        for (uint256 i; i < exits; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("E", next, i)));
        return abi.encode(BridgeVault.UpdateJournal(cfg, prev, next, roots, cl, ex));
    }

    function _vj(bytes32 cfg, bytes32 prev, bytes32 next, uint64 claims, uint256 exits) internal view returns (bytes memory) {
        return _vjFor(v, cfg, prev, next, claims, exits);
    }

    function _tail(bytes32 set, uint256 t, uint256 ej, uint256 signers, uint256 blockId, bytes32 h)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encode(set, t, ej, signers, PREV_ID, PREV_HASH, blockId, h);
    }

    function _tailAt(bytes32 set, uint256 ej, uint256 signers, uint256 prevId, uint256 blockId) internal pure returns (bytes memory) {
        return abi.encode(set, T, ej, signers, prevId, PREV_HASH, blockId, HASH);
    }

    function _full(bytes memory vj, bytes memory tail) internal pure returns (bytes memory) {
        return bytes.concat(vj, tail);
    }

    /// MockVerifier's seal (computed here: an external call would be the call vm.expectRevert watches).
    function _seal(bytes32 img, bytes memory journal) internal pure returns (bytes memory) {
        return abi.encodePacked(img, sha256(journal));
    }

    /// A good update for `w` behind `g` from its current state: 1 claim, 1 exit, signed by `signers`.
    function _goodFor(SeqSigGate g, BridgeVault w, uint256 signers)
        internal
        view
        returns (bytes memory vj, bytes memory full)
    {
        vj = _vjFor(w, w.cfgId(), w.g2State(), keccak256(abi.encode("s", w.g2State())), 1, 1);
        full = _full(vj, _tail(g.SIGNER_SET_HASH(), g.THRESHOLD(), g.ejected(), signers, BLOCK, HASH));
    }

    function _good() internal view returns (bytes memory vj, bytes memory full) {
        return _goodFor(gate, v, 0x7);
    }

    function _submitTo(SeqSigGate g, BridgeVault, address who, bytes memory full) internal {
        bytes memory s = _seal(UPDATE_ID, full);
        vm.prank(who);
        g.submit(full, s);
    }

    function _submit(address who, bytes memory full) internal {
        _submitTo(gate, v, who, full);
    }

    function _ev(bytes32 cfg, bytes32 set, uint256 signers, uint256 prevId, bytes32 prevHash, uint256 blockId, bytes32 h)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encode(cfg, set, signers, prevId, prevHash, blockId, h);
    }

    /// Evidence that `signers` signed (PREV, BLOCK, OTHER_HASH) for `v`.
    function _conflict(uint256 signers) internal view returns (bytes memory) {
        return _ev(gate.CFG_ID(), set4, signers, PREV_ID, PREV_HASH, BLOCK, OTHER_HASH);
    }

    function _contest(bytes memory ev) internal {
        bytes memory s = _seal(EVIDENCE_ID, ev);
        vm.prank(other);
        gate.contest(ev, s);
    }

    function _releaseArgs(BridgeVault w, bytes32 root, bytes32 tag) internal view returns (bytes memory seal, bytes32 nf) {
        nf = keccak256(abi.encode("nf", root, tag));
        bytes32 dig = sha256(abi.encode(w.cfgId(), root, nf, address(usdc), uint256(100e6), bob, address(0), uint256(0)));
        seal = mv.sealFor(RELEASE_ID, dig);
    }

    function _pendingAt(BridgeVault) internal view returns (uint64) {
        return gate.pending().acceptedAt;
    }

    // ------------------------------------------------------------------ constructor

    function test_Constructor() public {
        assertEq(gate.SIGNERS(), 4);
        assertEq(gate.THRESHOLD(), T);
        assertEq(gate.CONTEST_WINDOW(), WINDOW);
        assertFalse(gate.TEST_ONLY_SHORT_WINDOW());
        assertEq(gate.SIGNER_SET_HASH(), sha256(abi.encodePacked("RFP022/seqsig/set/v1", uint32(4), keys)));
        assertEq(gate.activeSigners(), 4);
        assertEq(address(gate.VAULT()), address(v));
        assertEq(gate.CFG_ID(), gate.CFG_ID());
        vm.expectRevert(SeqSigGate.ZeroAddress.selector);
        new SeqSigGate(IRiscZeroVerifier(address(0)), ISeqSigVault(address(v)), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, keys, T, WINDOW, false);
        vm.expectRevert(SeqSigGate.ZeroAddress.selector);
        new SeqSigGate(mv, ISeqSigVault(address(0)), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, keys, T, WINDOW, false);
        vm.expectRevert(SeqSigGate.BadImmutable.selector);
        new SeqSigGate(mv, ISeqSigVault(address(v)), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, UPDATE_ID, RELEASE_ID, keys, T, WINDOW, false);
        vm.expectRevert(SeqSigGate.BadImmutable.selector);
        new SeqSigGate(mv, ISeqSigVault(address(v)), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, bytes32(0), keys, T, WINDOW, false);
        // threshold must be a strict majority of the keys passed, and at most n
        _gateReverts(keys, 2, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        _gateReverts(keys, 5, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        // a window under the minimum only behind the visible test-only flag
        _gateReverts(keys, T, WINDOW - 1, false, abi.encodeWithSelector(SeqSigGate.WindowTooShort.selector));
        assertTrue(_gate(keys, T, 0, true).TEST_ONLY_SHORT_WINDOW());
        // keys: sorted, distinct, no empty set
        bytes32[] memory k = _keys(4);
        (k[0], k[1]) = (k[1], k[0]);
        _gateReverts(k, T, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadSignerSet.selector));
        k = _keys(4);
        k[1] = k[0];
        _gateReverts(k, T, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadSignerSet.selector));
        _gateReverts(new bytes32[](0), 1, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadSignerSet.selector));
    }

    /// Small-order points (identity, order 2, order 4, both order-8 y values, either sign bit) and
    /// non-canonical encodings (y >= p) are refused, so no holder gets two votes or a key that
    /// signs for anyone.
    function test_ConstructorRefusesBadKeys() public {
        bytes32[9] memory bad = [
            bytes32(hex"0100000000000000000000000000000000000000000000000000000000000000"), // identity
            bytes32(hex"0100000000000000000000000000000000000000000000000000000000000080"), // identity, sign bit
            bytes32(hex"ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"), // order 2 (y = p - 1)
            bytes32(hex"0000000000000000000000000000000000000000000000000000000000000000"), // order 4 (y = 0)
            bytes32(hex"0000000000000000000000000000000000000000000000000000000000000080"), // order 4, sign bit
            bytes32(hex"c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a"), // order 8
            bytes32(hex"26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85"), // order 8, other y
            bytes32(hex"eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"), // y = p + 1: non-canonical
            bytes32(hex"f0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f") // y = p + 3: non-canonical
        ];
        for (uint256 i; i < bad.length; ++i) {
            assertFalse(gate.keyOk(bad[i]), "bad key accepted");
            bytes32[] memory one = new bytes32[](1);
            one[0] = bad[i];
            _gateReverts(one, 1, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadKey.selector, 0));
        }
        for (uint256 i; i < 4; ++i) assertTrue(gate.keyOk(keys[i]));
    }

    /// The set hash the gate computes equals the one `seqsig set` printed for the run's keys
    /// (run-summary.txt of public-demo/prove/seq-sig-20261117, the stand-in keys k0..k3, sorted).
    function test_SetHashMatchesTheRustTool() public {
        bytes32[] memory k = new bytes32[](4);
        k[0] = 0x3852ef146795a649d1d77f30ed950d8d4676b6eff76cd7ab555e7dbcd6b1cf80;
        k[1] = 0x3ecf54cbe2e2565da75db44e7de96dfe9696214c5dcfd9973e609235b75d5ac9;
        k[2] = 0x42aa4a0649fe09f7ac28dbdb72e02ec4f792b8f772aa88f1a127a06c30bcb970;
        k[3] = 0x7baaddb70397b4bfdb9b4a164c0744898746cb8a55fc196538393f07ea6ec681;
        assertEq(_gate(k, 3, WINDOW, false).SIGNER_SET_HASH(), 0xf4aaec267c30f0c729f6dac776429debb5c3d5e3baa22181794f1120d9aa9019);
    }

    /// MAX_SIGNERS, T = n - 1 and T = n edges: a real gate needs n/2 < T <= n - 1; T = n only with
    /// the test-only flag (one silent key would stop every update).
    function test_EdgesMaxSignersAndThreshold() public {
        bytes32[] memory k64 = _keys(64);
        assertEq(_gate(k64, 63, WINDOW, false).THRESHOLD(), 63);
        assertEq(_gate(k64, 33, WINDOW, false).THRESHOLD(), 33);
        _gateReverts(k64, 64, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        assertEq(_gate(k64, 64, WINDOW, true).SIGNERS(), 64);
        _gateReverts(k64, 32, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        _gateReverts(_keys(65), 33, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadSignerSet.selector));
        bytes32[] memory k3 = _keys(3);
        assertEq(_gate(k3, 2, WINDOW, false).THRESHOLD(), 2);
        _gateReverts(k3, 3, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        bytes32[] memory k2 = _keys(2);
        _gateReverts(k2, 2, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector)); // n = 2 has no T with n/2 < T <= n - 1
        _gateReverts(k2, 1, WINDOW, false, abi.encodeWithSelector(SeqSigGate.BadThreshold.selector));
        // T = n = 4 (test flag): an update needs every key; one equivocation kills the gate
        (SeqSigGate g, BridgeVault w) = _deploy(keys, 4, 1, true); // window 1 s: contests in the same block
        _register(w);
        w.deposit(address(usdc), 0, keccak256("C-w"));
        w.deposit(address(usdc), 0, keccak256("C-w2")); // two updates below, one claim each
        (bytes memory vj, bytes memory full) = _goodFor(g, w, 0xf);
        _submitTo(g, w, anyone, full);
        vm.warp(block.timestamp + 1);
        g.finalize(vj);
        (, full) = _goodFor(g, w, 0x7);
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        g.submit(full, s);
        (vj, full) = _goodFor(g, w, 0xf);
        _submitTo(g, w, anyone, full);
        bytes memory ev = _ev(w.cfgId(), g.SIGNER_SET_HASH(), 0x1, PREV_ID, PREV_HASH, BLOCK, OTHER_HASH);
        g.contest(ev, _seal(EVIDENCE_ID, ev));
        assertTrue(g.dead());
        assertEq(g.pending().acceptedAt, 0, "cancelled: 3 active signers < 4");
        (, full) = _goodFor(g, w, 0xe);
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.GateDead.selector);
        g.submit(full, s);
    }

    // ------------------------------------------------------------------ accepted

    /// 3 of 4: submitted by an account with no role, finalized by another after the window, released.
    function test_ThreeOfFourAcceptedFromAnyAccount() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        SeqSigGate.Pending memory p = gate.pending();
        assertEq(p.journalDigest, sha256(vj));
        assertEq(p.signers, 0x7);
        assertEq(p.blockHash, HASH);
        assertEq(p.prevHash, PREV_HASH);
        assertEq(gate.finalizableAt(), block.timestamp + WINDOW);
        assertEq(v.g2State(), G0, "pending: the vault has not moved");

        vm.warp(block.timestamp + WINDOW);
        vm.prank(other);
        gate.finalize(vj);
        assertEq(v.g2State(), keccak256(abi.encode("s", G0)));
        assertEq(v.exitCount(), 1);
        assertEq(_pendingAt(v), 0);

        bytes32 root = v.exitRoot();
        (bytes memory rs, bytes32 nf) = _releaseArgs(v, root, "r");
        vm.prank(anyone);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    // ------------------------------------------------------------------ refused journals

    /// 2 of 4: with threshold 3 the guest refuses (no journal). A guest run with threshold 2 commits
    /// threshold 2, refused here; a bitmap of fewer than THRESHOLD keys is refused too.
    function test_TwoOfFourRefused() public {
        (bytes memory vj,) = _good();
        bytes memory full = _full(vj, _tail(set4, 2, 0, 0x3, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.WrongThreshold.selector);
        gate.submit(full, s);
        full = _full(vj, _tail(set4, T, 0, 0x3, BLOCK, HASH));
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        gate.submit(full, s);
        full = _full(vj, _tail(set4, T, 0, 0x17, BLOCK, HASH)); // a bit outside the set
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        gate.submit(full, s);
    }

    function test_OtherSignerSetRefused() public {
        (bytes memory vj,) = _good();
        bytes memory full = _full(vj, _tail(keccak256("a set with an outsider"), T, 0, 0x7, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.WrongSignerSet.selector);
        gate.submit(full, s);
    }

    /// The seal covers the whole journal; a seal of another image (the TrustedTip guest's, over
    /// the vault journal or the whole journal) is refused.
    function test_SealBindsTheTailAndTheImage() public {
        (bytes memory vj, bytes memory full) = _good();
        bytes memory forged = _full(vj, _tail(set4, T, 0, 0x7, BLOCK, OTHER_HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        gate.submit(forged, s);
        s = _seal(UPDATE_ID, vj);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        gate.submit(full, s);
        s = _seal(TRUSTED_TIP_ID, vj);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        gate.submit(full, s);
        s = _seal(TRUSTED_TIP_ID, full);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        gate.submit(full, s);
    }

    /// Only the release image passes through `verify`; any other image (evidence, TrustedTip) is refused.
    function test_VerifyPassesOnlyTheReleaseImage() public {
        bytes32 d = sha256("x");
        gate.verify(abi.encodePacked(RELEASE_ID, d), RELEASE_ID, d);
        bytes memory s = abi.encodePacked(EVIDENCE_ID, d);
        vm.expectRevert(SeqSigGate.UnknownImage.selector);
        gate.verify(s, EVIDENCE_ID, d);
        s = abi.encodePacked(TRUSTED_TIP_ID, d);
        vm.expectRevert(SeqSigGate.UnknownImage.selector);
        gate.verify(s, TRUSTED_TIP_ID, d);
    }

    function test_WrongCfgIdRefused() public {
        bytes memory vj = _vj(keccak256("another vault's cfgId"), G0, keccak256("s1"), 1, 1);
        bytes memory full = _full(vj, _tail(set4, T, 0, 0x7, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.WrongConfig.selector);
        gate.submit(full, s);
    }

    function test_ShortJournalRefused() public {
        bytes memory full = _tail(set4, T, 0, 0x7, BLOCK, HASH);
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.BadJournal.selector);
        gate.submit(full, s);
    }

    function test_ReplayOfOldCheckpointRefused() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj);
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.StaleState.selector);
        gate.submit(full, s);
        vm.expectRevert(SeqSigGate.NoPending.selector);
        gate.finalize(vj);
    }

    function test_NoStateAdvanceRefused() public {
        bytes memory vj = _vj(gate.CFG_ID(), G0, G0, 0, 0);
        bytes memory full = _full(vj, _tail(set4, T, 0, 0x7, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.NoStateAdvance.selector);
        gate.submit(full, s);
    }

    function test_OnePendingAtATime() public {
        (, bytes memory full) = _good();
        _submit(anyone, full);
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.PendingExists.selector);
        gate.submit(full, s);
    }

    function test_DirectUpdateRefused() public {
        (bytes memory vj,) = _good();
        bytes memory s = _seal(UPDATE_ID, vj);
        vm.prank(anyone);
        vm.expectRevert(SeqSigGate.UpdateNotThroughGate.selector);
        v.update(vj, s);
        vm.expectRevert(SeqSigGate.OnlySelf.selector);
        gate.simulate(vj);
    }

    /// The gate checks the vault it is bound to is wired to it with its cfgId: a vault deployed at
    /// the predicted address with another verifier, or with other LEZ ids (another cfgId), is refused.
    function test_MiswiredVaultRefused() public {
        address predicted = vm.computeCreateAddress(address(this), vm.getNonce(address(this)) + 1);
        SeqSigGate g = new SeqSigGate(
            mv, ISeqSigVault(predicted), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, keys, T, WINDOW, false
        );
        BridgeVault w = new BridgeVault(
            IRiscZeroVerifier(address(mv)), UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, G0, 0,
            address(this), address(this), address(this), 1 hours
        );
        assertEq(address(w), predicted);
        (, bytes memory full) = _goodFor(g, w, 0x7);
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.WrongVault.selector);
        g.submit(full, s);
        predicted = vm.computeCreateAddress(address(this), vm.getNonce(address(this)) + 1);
        g = new SeqSigGate(
            mv, ISeqSigVault(predicted), keccak256("another LEZ chain"), ZONE_ID, PROGRAM_ID, UPDATE_ID, EVIDENCE_ID, RELEASE_ID, keys, T, WINDOW, false
        );
        w = new BridgeVault(
            IRiscZeroVerifier(address(g)), UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, G0, 0,
            address(this), address(this), address(this), 1 hours
        );
        (, full) = _goodFor(g, w, 0x7);
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.WrongVault.selector);
        g.submit(full, s);
    }

    /// The gate serves one vault: another vault using it as its verifier, or a fake vault that
    /// claims the gate's cfgId and an old g2State, gets no update through `verify`, and there is no
    /// way to name another vault to `submit`, `contest` or `finalize`. An already finalized journal
    /// cannot be staged again (the bound vault's state has moved).
    function test_ASecondOrFakeVaultCannotUseThisGate() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj);

        BridgeVault w = new BridgeVault(
            IRiscZeroVerifier(address(gate)), UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, G0, 0,
            address(this), address(this), address(this), 1 hours
        );
        bytes memory vjw = _vjFor(w, w.cfgId(), G0, keccak256("w1"), 0, 0);
        bytes memory s = _seal(UPDATE_ID, vjw);
        vm.expectRevert(SeqSigGate.UpdateNotThroughGate.selector);
        w.update(vjw, s);

        FakeVault fake = new FakeVault(gate, G0);
        vm.expectRevert(SeqSigGate.UpdateNotThroughGate.selector);
        fake.update(vj, "");

        s = _seal(UPDATE_ID, full);
        vm.expectRevert(SeqSigGate.StaleState.selector);
        gate.submit(full, s);
        assertEq(gate.ejected(), 0);
        assertFalse(gate.halted());
    }

    // ------------------------------------------------------------------ window and release

    function test_ReleaseRefusedDuringWindowAllowedAfter() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        uint256 snap = vm.snapshotState();
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj);
        bytes32 root = v.exitRoot();
        vm.revertToState(snap);

        (bytes memory rs, bytes32 nf) = _releaseArgs(v, root, "r");
        vm.warp(block.timestamp + WINDOW - 1);
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        vm.expectRevert(SeqSigGate.ContestWindowOpen.selector);
        gate.finalize(vj);
        vm.warp(block.timestamp + 1);
        bytes memory wrong = _vj(gate.CFG_ID(), G0, keccak256("x"), 1, 1);
        vm.expectRevert(SeqSigGate.WrongJournal.selector);
        gate.finalize(wrong);
        gate.finalize(vj);
        assertEq(v.exitRoot(), root);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    /// A release freeze between submit and finalize delays finalize (the vault refuses update while
    /// frozen); the update stays pending and applies once unfrozen.
    function test_FreezeAndUnfreezeBetweenSubmitAndFinalize() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        v.freezeAll(false, true);
        vm.warp(block.timestamp + WINDOW);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        gate.finalize(vj);
        assertTrue(_pendingAt(v) != 0, "still pending");
        bytes memory p = abi.encode(false, true);
        bytes32 id = v.schedule(BridgeVault.Action.UnfreezeAll, p);
        vm.warp(v.scheduledEta(id));
        v.execute(BridgeVault.Action.UnfreezeAll, p);
        gate.finalize(vj);
        assertEq(v.exitCount(), 1);
    }

    // ------------------------------------------------------------------ contest: equivocation

    /// Key 1 signed the pending update (bitmap 0x7) and a conflicting hash for the same
    /// (cfgId, previous tip, block): it is ejected, the update cancelled, the vault unchanged, and
    /// that checkpoint can never be staged again.
    function test_EquivocationEjectsTheKeyAndCancels() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        uint256 snap = vm.snapshotState();
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj);
        bytes32 root = v.exitRoot();
        vm.revertToState(snap);

        _contest(_conflict(0x2));
        assertEq(gate.ejected(), 0x2);
        assertEq(gate.activeSigners(), 3);
        assertEq(_pendingAt(v), 0);
        assertEq(v.g2State(), G0);
        assertEq(v.exitCount(), 0);
        assertTrue(gate.refuted(keccak256(abi.encode(PREV_ID, PREV_HASH, BLOCK, HASH))));

        vm.warp(block.timestamp + WINDOW);
        vm.expectRevert(SeqSigGate.NoPending.selector);
        gate.finalize(vj);
        (bytes memory rs, bytes32 nf) = _releaseArgs(v, root, "r");
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        // the same journal with the new mask: the checkpoint is refuted
        bytes memory again = _full(vj, _tail(set4, T, 0x2, 0xd, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, again);
        vm.expectRevert(SeqSigGate.Refuted.selector);
        gate.submit(again, s);
    }

    /// A conflicting hash signed only by a key that did NOT sign the update cancels nothing.
    function test_ConflictFromANonSignerCancelsNothing() public {
        (, bytes memory full) = _good(); // keys 0, 1, 2
        _submit(anyone, full);
        bytes memory ev = _conflict(0x8); // key 3
        bytes memory s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NoEquivocation.selector);
        gate.contest(ev, s);
        assertTrue(_pendingAt(v) != 0);
        assertEq(gate.ejected(), 0);
    }

    /// M-2: an equivocator is ejected, but an update that keeps THRESHOLD active signers stays staged
    /// and finalizes (4 of 4 at T = 3, one equivocator).
    function test_EquivocatorEjectedButQuorumKeepsTheUpdate() public {
        (bytes memory vj, bytes memory full) = _goodFor(gate, v, 0xf);
        _submit(anyone, full);
        _contest(_conflict(0x2));
        assertEq(gate.ejected(), 0x2);
        assertTrue(_pendingAt(v) != 0, "still staged: 3 active signers remain");
        assertFalse(gate.refuted(keccak256(abi.encode(PREV_ID, PREV_HASH, BLOCK, HASH))));
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj);
        assertEq(v.exitCount(), 1);
    }

    /// Halt evidence counts only active keys: 1 ejected + 2 active keys at T = 3 do not halt (the 2
    /// active ones that also signed the update are ejected instead).
    function test_HaltEvidenceWithAnEjectedKeyDoesNotHalt() public {
        (, bytes memory full) = _good(); // keys 0, 1, 2
        _submit(anyone, full);
        _contest(_conflict(0x2)); // key 1 ejected; 2 signers left: cancelled
        bytes memory vj = _vj(gate.CFG_ID(), G0, keccak256("s-b"), 1, 1);
        full = _full(vj, _tail(set4, T, 0x2, 0xd, BLOCK, HASH2)); // keys 0, 2, 3
        _submit(anyone, full);
        bytes memory ev = _ev(gate.CFG_ID(), set4, 0x7, PREV_ID, PREV_HASH, BLOCK, OTHER_HASH); // keys 0, 1, 2
        _contest(ev);
        assertFalse(gate.halted());
        assertEq(gate.ejected(), 0x7);
        assertTrue(gate.dead());
    }

    /// The contest window is open until acceptedAt + WINDOW - 1 inclusive.
    function test_ContestAtTheLastSecondOfTheWindow() public {
        (, bytes memory full) = _good();
        _submit(anyone, full);
        vm.warp(block.timestamp + WINDOW - 1);
        _contest(_conflict(0x2));
        assertEq(_pendingAt(v), 0);
    }

    /// A proof made before an ejection stays valid if it did not count the ejected key; one that did,
    /// or one whose mask names a key that is not ejected, is refused.
    function test_EjectionInvalidatesOnlyProofsThatCountedTheKey() public {
        bytes32 next = keccak256(abi.encode("s", G0));
        bytes memory vjA = _vj(gate.CFG_ID(), G0, next, 1, 1);
        bytes memory before = _full(vjA, _tail(set4, T, 0, 0xd, BLOCK + 1, HASH)); // keys 0, 2, 3; mask 0
        bytes memory counted = _full(vjA, _tail(set4, T, 0, 0x7, BLOCK + 1, HASH)); // keys 0, 1, 2
        (, bytes memory full) = _good();
        _submit(anyone, full);
        _contest(_conflict(0x2)); // key 1 ejected, update cancelled
        bytes memory s = _seal(UPDATE_ID, counted);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        gate.submit(counted, s);
        bytes memory future = _full(vjA, _tail(set4, T, 0xa, 0x5, BLOCK + 1, HASH));
        s = _seal(UPDATE_ID, future);
        vm.expectRevert(SeqSigGate.WrongEjectedMask.selector);
        gate.submit(future, s);
        _submit(anyone, before);
        assertTrue(_pendingAt(v) != 0);
    }

    /// prevId or blockId above uint64 is refused.
    function test_IdsAboveUint64Refused() public {
        (bytes memory vj,) = _good();
        bytes memory f = _full(vj, _tailAt(set4, 0, 0x7, uint256(type(uint64).max) + 1, BLOCK));
        bytes memory s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.BadJournal.selector);
        gate.submit(f, s);
        f = _full(vj, _tailAt(set4, 0, 0x7, PREV_ID, uint256(type(uint64).max) + 1));
        s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.BadJournal.selector);
        gate.submit(f, s);
    }

    /// Golden vectors shared with Rust (`seqsig_core` test `golden_abi_vectors`): the 256-byte
    /// journal tail and the 224-byte evidence journal, decoded with the gate's layouts.
    function test_GoldenVectorsMatchRust() public pure {
        bytes memory tail = hex"1111111111111111111111111111111111111111111111111111111111111111"
            hex"0000000000000000000000000000000000000000000000000000000000000003"
            hex"0000000000000000000000000000000000000000000000000000000000000002"
            hex"000000000000000000000000000000000000000000000000000000000000000d"
            hex"0000000000000000000000000000000000000000000000000102030405060708"
            hex"2222222222222222222222222222222222222222222222222222222222222222"
            hex"0000000000000000000000000000000000000000000000001112131415161718"
            hex"3333333333333333333333333333333333333333333333333333333333333333";
        SeqSigGate.Tail memory t = abi.decode(tail, (SeqSigGate.Tail));
        assertEq(t.setHash, bytes32(0x1111111111111111111111111111111111111111111111111111111111111111));
        assertEq(t.threshold, 3);
        assertEq(t.ejected, 2);
        assertEq(t.signers, 0xd);
        assertEq(t.prevId, 0x0102030405060708);
        assertEq(t.prevHash, bytes32(0x2222222222222222222222222222222222222222222222222222222222222222));
        assertEq(t.blockId, 0x1112131415161718);
        assertEq(t.blockHash, bytes32(0x3333333333333333333333333333333333333333333333333333333333333333));
        assertEq(abi.encode(t), tail);
        bytes memory ev = hex"4444444444444444444444444444444444444444444444444444444444444444"
            hex"1111111111111111111111111111111111111111111111111111111111111111"
            hex"0000000000000000000000000000000000000000000000000000000000000005"
            hex"0000000000000000000000000000000000000000000000000102030405060708"
            hex"2222222222222222222222222222222222222222222222222222222222222222"
            hex"0000000000000000000000000000000000000000000000001112131415161718"
            hex"5555555555555555555555555555555555555555555555555555555555555555";
        SeqSigGate.Ev memory e = abi.decode(ev, (SeqSigGate.Ev));
        assertEq(e.cfgId, bytes32(0x4444444444444444444444444444444444444444444444444444444444444444));
        assertEq(e.signers, 5);
        assertEq(e.prevId, 0x0102030405060708);
        assertEq(e.blockId, 0x1112131415161718);
        assertEq(e.blockHash, bytes32(0x5555555555555555555555555555555555555555555555555555555555555555));
        assertEq(abi.encode(e), ev);
    }

    /// Evidence signed in advance for a future block id, or over another previous tip (a reorged or
    /// re-genesis history), is no conflict with the pending update.
    function test_PreProvedOrOtherHistoryEvidenceIsUseless() public {
        (, bytes memory full) = _good();
        _submit(anyone, full);
        bytes32 cfg = gate.CFG_ID();
        bytes memory ev = _ev(cfg, set4, 0x2, PREV_ID, PREV_HASH, BLOCK + 50, OTHER_HASH);
        bytes memory s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NotTheSameCheckpoint.selector);
        gate.contest(ev, s);
        ev = _ev(cfg, set4, 0x2, PREV_ID - 1, PREV_HASH, BLOCK, OTHER_HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NotTheSameCheckpoint.selector);
        gate.contest(ev, s);
        ev = _ev(cfg, set4, 0x2, PREV_ID, keccak256("another history"), BLOCK, OTHER_HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NotTheSameCheckpoint.selector);
        gate.contest(ev, s);
    }

    function test_ContestRefusedWithInvalidEvidence() public {
        bytes32 cfg = gate.CFG_ID();
        bytes memory ev = _conflict(0x2);
        bytes memory s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NoPending.selector);
        gate.contest(ev, s);

        (, bytes memory full) = _good();
        _submit(anyone, full);
        ev = _ev(cfg, set4, 0x2, PREV_ID, PREV_HASH, BLOCK, HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NoConflict.selector);
        gate.contest(ev, s);
        ev = _ev(cfg, keccak256("other set"), 0x2, PREV_ID, PREV_HASH, BLOCK, OTHER_HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.WrongSignerSet.selector);
        gate.contest(ev, s);
        ev = _ev(keccak256("other cfg"), set4, 0x2, PREV_ID, PREV_HASH, BLOCK, OTHER_HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.WrongConfig.selector);
        gate.contest(ev, s);
        ev = _conflict(0);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.BadEvidence.selector);
        gate.contest(ev, s);
        ev = _conflict(0x2);
        s = _seal(UPDATE_ID, ev);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        gate.contest(ev, s);
        s = _seal(EVIDENCE_ID, ev);
        bytes memory longer = bytes.concat(ev, bytes1(0));
        vm.expectRevert(SeqSigGate.BadEvidence.selector);
        gate.contest(longer, s);
        vm.warp(block.timestamp + WINDOW);
        vm.expectRevert(SeqSigGate.ContestWindowClosed.selector);
        gate.contest(ev, s);
        assertTrue(_pendingAt(v) != 0, "still pending");
    }

    /// Griefing is bounded: an equivocating key is ejected once; afterwards its signatures do not
    /// count (a journal with the old mask or with it in the bitmap is refused), its evidence cancels
    /// nothing, and the next update goes through without it. A second equivocator takes the active
    /// keys below THRESHOLD: the gate is dead.
    function test_RepeatedCancelGriefingIsBoundedByEjection() public {
        (, bytes memory full) = _good(); // 0, 1, 2
        _submit(anyone, full);
        _contest(_conflict(0x2)); // key 1 equivocates
        bytes32 g1 = keccak256("s after ejection");
        bytes memory vj = _vj(gate.CFG_ID(), G0, g1, 1, 1);
        bytes memory f = _full(vj, _tail(set4, T, 0x8, 0xd, BLOCK + 1, HASH)); // a mask with a key not ejected
        bytes memory s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.WrongEjectedMask.selector);
        gate.submit(f, s);
        f = _full(vj, _tail(set4, T, 0x2, 0x7, BLOCK + 1, HASH)); // key 1 in the bitmap
        s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        gate.submit(f, s);
        f = _full(vj, _tail(set4, T, 0x2, 0xd, BLOCK + 1, HASH)); // keys 0, 2, 3
        _submit(anyone, f);
        // key 1 again: ejected, so its conflict is nobody's equivocation
        bytes memory ev = _ev(gate.CFG_ID(), set4, 0x2, PREV_ID, PREV_HASH, BLOCK + 1, OTHER_HASH);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.NoEquivocation.selector);
        gate.contest(ev, s);
        // key 3 equivocates: ejected; 2 active keys < 3: dead
        ev = _ev(gate.CFG_ID(), set4, 0x8, PREV_ID, PREV_HASH, BLOCK + 1, OTHER_HASH);
        _contest(ev);
        assertEq(gate.ejected(), 0xa);
        assertTrue(gate.dead());
        f = _full(vj, _tail(set4, T, 0xa, 0x5, BLOCK + 2, HASH));
        s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.GateDead.selector);
        gate.submit(f, s);
    }

    /// THRESHOLD active keys signed a conflicting checkpoint: two quorums conflict; the gate halts
    /// for good. Releases of exits applied before still pay.
    function test_TwoQuorumsHaltTheGate() public {
        (bytes memory vj0, bytes memory f0) = _good();
        _submit(anyone, f0);
        vm.warp(block.timestamp + WINDOW);
        gate.finalize(vj0);
        bytes32 root = v.exitRoot();

        bytes32 next = keccak256("s2");
        bytes memory vj = _vj(gate.CFG_ID(), v.g2State(), next, 1, 1);
        bytes memory f = _full(vj, _tail(set4, T, 0, 0x7, BLOCK, HASH));
        _submit(anyone, f);
        _contest(_conflict(0xb)); // keys 0, 1, 3: a quorum on another hash
        assertTrue(gate.halted());
        assertTrue(gate.dead());
        assertEq(_pendingAt(v), 0);
        bytes memory s = _seal(UPDATE_ID, f);
        vm.expectRevert(SeqSigGate.Halted.selector);
        gate.submit(f, s);
        vm.expectRevert(SeqSigGate.Halted.selector);
        gate.finalize(vj);
        bytes memory ev = _conflict(0x2);
        s = _seal(EVIDENCE_ID, ev);
        vm.expectRevert(SeqSigGate.Halted.selector);
        gate.contest(ev, s);
        (bytes memory rs, bytes32 nf) = _releaseArgs(v, root, "r");
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    // ------------------------------------------------------------------ transient mark

    function test_MarkClearedWithinTheSameTransaction() public {
        (bytes memory vj, bytes memory full) = _good();
        _submit(anyone, full);
        vm.warp(block.timestamp + WINDOW);
        bytes memory vj2 = _vj(gate.CFG_ID(), keccak256(abi.encode("s", G0)), keccak256("s2"), 0, 0);
        FinalizeThenDirect bot = new FinalizeThenDirect();
        vm.expectRevert(SeqSigGate.UpdateNotThroughGate.selector);
        bot.run(gate, v, vj, vj2);
    }

    // ------------------------------------------------------------------ conservation

    function test_VaultConservationChecksStillApply() public {
        bytes32 cfg = gate.CFG_ID();
        bytes32 n1 = keccak256("n1");
        bytes memory full = _full(_vj(cfg, G0, n1, 5, 0), _tail(set4, T, 0, 0x7, BLOCK, HASH));
        bytes memory s = _seal(UPDATE_ID, full);
        vm.expectRevert(BridgeVault.ClaimsExceedDeposits.selector);
        gate.submit(full, s);
        full = _full(_vj(cfg, G0, n1, 1, 2), _tail(set4, T, 0, 0x7, BLOCK, HASH));
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(BridgeVault.BurnsExceedClaims.selector);
        gate.submit(full, s);
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = keccak256("not a root");
        bytes memory vj = abi.encode(
            BridgeVault.UpdateJournal(cfg, G0, n1, roots, new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0))
        );
        full = _full(vj, _tail(set4, T, 0, 0x7, BLOCK, HASH));
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(BridgeVault.UnknownDepositRoot.selector);
        gate.submit(full, s);
        v.freezeAll(false, true);
        (, full) = _good();
        s = _seal(UPDATE_ID, full);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        gate.submit(full, s);
    }

    /// Several rounds through the gate, each with random claims, exits, a contest or not, and
    /// releases of what was burned: per token released <= burned <= claimed <= deposited, always.
    function testFuzz_ConservationThroughGateOverSeveralUpdates(uint256 seed) public {
        uint256 released;
        for (uint256 r; r < 4; ++r) {
            uint256 x = uint256(keccak256(abi.encode(seed, r)));
            uint64 claims = uint64(x % 3);
            uint256 exits = (x >> 8) % 3;
            bool contestIt = (x >> 16) % 4 == 0;
            bytes32 prev = v.g2State();
            bytes memory vj = _vj(gate.CFG_ID(), prev, keccak256(abi.encode("f", seed, r)), claims, exits);
            uint256 ej = gate.ejected();
            uint256 bm = 0xf & ~ej;
            bytes memory full = _full(vj, _tail(set4, T, ej, bm, BLOCK + r, HASH));
            bytes memory s = _seal(UPDATE_ID, full);
            try gate.submit(full, s) {
                if (contestIt && gate.activeSigners() > T) {
                    // the lowest active key of the bitmap equivocates
                    bytes memory ev = _ev(gate.CFG_ID(), set4, bm & (~bm + 1), PREV_ID, PREV_HASH, BLOCK + r, OTHER_HASH);
                    gate.contest(ev, _seal(EVIDENCE_ID, ev));
                    if (gate.pending().acceptedAt != 0) {
                        vm.warp(block.timestamp + WINDOW);
                        gate.finalize(vj);
                    }
                } else {
                    vm.warp(block.timestamp + WINDOW);
                    gate.finalize(vj);
                }
            } catch {}
            (,, uint64 bu, uint64 rel) = v.counters(address(usdc), 0);
            if (bu > rel) {
                bytes32 root = v.exitRoot();
                (bytes memory rs, bytes32 nf) = _releaseArgs(v, root, bytes32(r));
                v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
                ++released;
            }
            (uint64 dep, uint64 cl, uint64 bu2, uint64 rel2) = v.counters(address(usdc), 0);
            assertLe(rel2, bu2);
            assertLe(v.burnedValue(address(usdc)), v.claimedValue(address(usdc)));
            assertLe(cl, dep);
            assertEq(v.claimedValue(address(usdc)), uint256(cl) * 100e6);
            assertEq(rel2, released);
        }
        assertEq(usdc.balanceOf(bob), released * 100e6);
    }
}
