// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {MockVerifier} from "./mocks/Mocks.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A2 / F#15: a G2 journal and a G3 release seal made for vault A are refused by every other
/// configuration: the same constructor arguments at another address, the same address on
/// another chain id, and the same address and chain with another programId. The journal
/// fails with WrongConfig (its cfgId field); the release fails the proof check, because the
/// release digest binds cfgId. Positive control: A accepts both.
contract CrossConfigTest is VaultFixture {
    address constant AT = address(0xC0FFEE00); // where A (and the same-address variants) live
    address constant OTHER = address(0xBEEF0000);
    uint256 constant T0 = 1_000_000;
    /// storage slot of the exitRootTime mapping (forge inspect BridgeVault storageLayout)
    uint256 constant EXIT_ROOT_TIME_SLOT = 84;

    bytes32 constant NF = keccak256("nf-A");
    bytes32 constant COMMIT = keccak256("deposit-commitment");
    bytes32 constant EXIT_COMMIT = keccak256("exit-commitment");

    function _args(bytes32 program) internal view returns (bytes memory) {
        return abi.encode(address(mv), G2, G3, LEZ_CHAIN, ZONE, program, S0, uint64(1 hours), admin, freezer, feeTo, uint64(ADMIN_DELAY));
    }

    /// Deploy at `at` and register USDC with the fixture's terms; alice approves it.
    function _deploy(address at, bytes32 program) internal returns (BridgeVault x) {
        deployCodeTo("BridgeVault.sol:BridgeVault", _args(program), at);
        x = BridgeVault(at);
        vm.prank(admin);
        x.registerToken(address(usdc), 6, _arr(100e6, 1000e6), _arr(1e6, 10e6), _arr(1e6, 10e6), DELAY);
        vm.prank(alice);
        usdc.approve(at, type(uint256).max);
    }

    /// One USDC deposit, and the journal that claims it and exits it on `x` (not applied).
    function _depositAndJournal(BridgeVault x) internal returns (bytes memory j) {
        vm.prank(alice);
        x.deposit(address(usdc), 0, COMMIT);
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = x.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, EXIT_COMMIT);
        j = abi.encode(BridgeVault.UpdateJournal(x.cfgId(), x.g2State(), keccak256("s1"), roots, cl, ex));
    }

    function _relSeal(BridgeVault x, bytes32 root) internal view returns (bytes memory) {
        return mv.sealFor(G3, sha256(abi.encode(x.cfgId(), root, NF, address(usdc), uint256(100e6), bob, relayer, uint256(0))));
    }

    struct FromA {
        bytes journal;
        bytes journalSeal;
        bytes32 root;
        bytes releaseSeal;
        bytes32 cfg;
    }

    /// Build A's payloads at AT on the current chain and run the positive control on A.
    function _buildOnA() internal returns (FromA memory a) {
        BridgeVault va = _deploy(AT, PROGRAM);
        a.cfg = va.cfgId();
        a.journal = _depositAndJournal(va);
        a.journalSeal = mv.sealFor(G2, sha256(a.journal));
        va.update(a.journal, a.journalSeal); // positive control: A accepts its journal
        a.root = va.exitRoot();
        a.releaseSeal = _relSeal(va, a.root);
        vm.warp(T0 + DELAY);
        uint256 before = usdc.balanceOf(bob);
        va.release(a.releaseSeal, a.root, NF, address(usdc), 0, bob, relayer, 0); // and its seal
        assertEq(usdc.balanceOf(bob) - before, 99e6);
        assertTrue(va.releaseNullifierSpent(NF));
    }

    /// Everything A's payloads can do to `vb` fails; the same arguments under vb's own cfgId pass.
    function _assertRefused(BridgeVault vb, FromA memory a) internal {
        assertTrue(vb.cfgId() != a.cfg, "variant must have another cfgId");

        // the journal: verified (same image), then refused on its cfgId
        vm.expectRevert(BridgeVault.WrongConfig.selector);
        vb.update(a.journal, a.journalSeal);

        // vb's own identical history gives another exit root (exit leaves bind cfgId) ...
        vm.warp(T0);
        bytes memory jb = _depositAndJournal(vb);
        vb.update(jb, mv.sealFor(G2, sha256(jb)));
        bytes32 rootB = vb.exitRoot();
        assertTrue(rootB != a.root, "exit root must differ");
        assertEq(vb.exitRootTime(a.root), 0);
        // ... and A's journal is still refused after vb has state
        vm.expectRevert(BridgeVault.WrongConfig.selector);
        vb.update(a.journal, a.journalSeal);

        vm.warp(T0 + DELAY);
        // A's seal with A's root: that root is unknown here
        vm.expectRevert(BridgeVault.UnknownExitRoot.selector);
        vb.release(a.releaseSeal, a.root, NF, address(usdc), 0, bob, relayer, 0);
        // A's seal under vb's own root: the proof fails
        vm.expectRevert(MockVerifier.BadSeal.selector);
        vb.release(a.releaseSeal, rootB, NF, address(usdc), 0, bob, relayer, 0);

        // isolate cfgId: make A's root known to vb (as if both trees matched), so every check
        // before the proof passes and only the digest's cfgId differs
        bytes32 slot = keccak256(abi.encode(a.root, EXIT_ROOT_TIME_SLOT));
        assertEq(uint256(vm.load(address(vb), keccak256(abi.encode(rootB, EXIT_ROOT_TIME_SLOT)))), vb.exitRootTime(rootB), "slot layout");
        vm.store(address(vb), slot, bytes32(uint256(T0)));
        assertEq(vb.exitRootTime(a.root), T0);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        vb.release(a.releaseSeal, a.root, NF, address(usdc), 0, bob, relayer, 0);
        assertFalse(vb.releaseNullifierSpent(NF));
        // control: the same arguments proven for vb's cfgId go through
        bytes memory sealB = _relSeal(vb, a.root);
        uint256 before = usdc.balanceOf(bob);
        vb.release(sealB, a.root, NF, address(usdc), 0, bob, relayer, 0);
        assertEq(usdc.balanceOf(bob) - before, 99e6);
    }

    function test_PositiveControlOnA() public {
        vm.warp(T0);
        FromA memory a = _buildOnA();
        // replaying on A is refused for the usual reasons
        vm.expectRevert(BridgeVault.StaleState.selector);
        BridgeVault(AT).update(a.journal, a.journalSeal);
        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        BridgeVault(AT).release(a.releaseSeal, a.root, NF, address(usdc), 0, bob, relayer, 0);
    }

    /// Same constructor arguments, same chain, another address.
    function test_RefusedByVaultAtOtherAddress() public {
        vm.warp(T0);
        FromA memory a = _buildOnA();
        BridgeVault vb = _deploy(OTHER, PROGRAM);
        assertEq(vb.cfgId(), vb.computeCfgId(block.chainid, OTHER, LEZ_CHAIN, ZONE, PROGRAM));
        _assertRefused(vb, a);
    }

    /// Same address and constructor arguments on another chain id. The snapshot is taken
    /// before A exists, so the variant starts with clean storage (deployCodeTo keeps storage).
    function test_RefusedBySameAddressOnOtherChain() public {
        uint256 chain0 = block.chainid;
        uint256 snap = vm.snapshotState();
        vm.warp(T0);
        FromA memory a = _buildOnA();
        vm.revertToState(snap);
        vm.chainId(chain0 + 1);
        BridgeVault vb = _deploy(AT, PROGRAM);
        assertEq(address(vb), AT);
        assertEq(vb.depositCount(), 0, "clean storage");
        assertEq(vb.cfgId(), vb.computeCfgId(chain0 + 1, AT, LEZ_CHAIN, ZONE, PROGRAM));
        _assertRefused(vb, a);
    }

    /// Same address, same chain, another programId (the only variable).
    function test_RefusedBySameAddressOtherProgram() public {
        uint256 snap = vm.snapshotState();
        vm.warp(T0);
        FromA memory a = _buildOnA();
        vm.revertToState(snap);
        BridgeVault vb = _deploy(AT, keccak256("another program"));
        assertEq(vb.depositCount(), 0, "clean storage");
        _assertRefused(vb, a);
    }
}
