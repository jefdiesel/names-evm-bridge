// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// M4 / red-team B1.5: who may do what under the admin timelock. Cancel by the admin or the
/// freezer (the freezer's veto), wrong callers refused, freezing instant and unfreezing
/// timelocked (an unfreeze scheduled before the latest freeze is refused), the admin
/// handover and fee recipient through the timelock, and the known limit that a freezer key
/// can veto its own replacement.
contract AdminTimelockRolesTest is VaultFixture {
    uint256 constant T0 = 1_000_000;

    // ------------------------------------------------------------------ cancel / freezer veto

    function test_FreezerCanVetoHostileAdminActions() public {
        vm.warp(T0);
        // Freezer halts the vault immediately on seeing trouble.
        vm.prank(freezer);
        v.freezeAll(true, true);

        // Compromised admin queues replacing the freezer and lifting the global freeze.
        bytes memory swapFreezer = abi.encode(bob);
        bytes memory liftFreeze = abi.encode(true, true);
        vm.startPrank(admin);
        bytes32 idSwap = v.schedule(BridgeVault.Action.SetFreezer, swapFreezer);
        bytes32 idLift = v.schedule(BridgeVault.Action.UnfreezeAll, liftFreeze);
        vm.stopPrank();

        // Freezer vetoes both before eta.
        vm.warp(T0 + 1 hours);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionCancelled(idSwap, freezer);
        vm.prank(freezer);
        v.cancel(idSwap);

        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionCancelled(idLift, freezer);
        vm.prank(freezer);
        v.cancel(idLift);

        assertEq(v.scheduledEta(idSwap), 0);
        assertEq(v.scheduledEta(idLift), 0);

        // At eta, neither hostile action can execute.
        vm.warp(T0 + ADMIN_DELAY);
        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFreezer, swapFreezer);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.UnfreezeAll, liftFreeze);
        vm.stopPrank();

        assertEq(v.freezer(), freezer);
        assertTrue(v.globalDepositsFrozen());
        assertTrue(v.globalReleasesFrozen());
    }

    function test_AdminCanCancelOwnAction() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);
        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.ProposeAdmin, params);

        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionCancelled(id, admin);
        vm.prank(admin);
        v.cancel(id);

        assertEq(v.scheduledEta(id), 0);
        vm.warp(T0 + ADMIN_DELAY);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.ProposeAdmin, params);
        assertEq(v.pendingAdmin(), address(0));
    }

    function test_CancelUnscheduledAndConsumedReverts() public {
        vm.warp(T0);
        bytes32 unknown = keccak256("never-scheduled");
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.cancel(unknown);
        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.cancel(unknown);

        // Cancel twice -> second reverts.
        bytes memory p1 = abi.encode(bob);
        vm.prank(admin);
        bytes32 id1 = v.schedule(BridgeVault.Action.SetFreezer, p1);
        vm.prank(freezer);
        v.cancel(id1);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.cancel(id1);

        // Cancel after execute -> reverts.
        bytes memory p2 = abi.encode(alice);
        vm.prank(admin);
        bytes32 id2 = v.schedule(BridgeVault.Action.SetFeeRecipient, p2);
        vm.warp(T0 + ADMIN_DELAY);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFeeRecipient, p2);
        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.cancel(id2);

        // Clearing an expired entry via cancel still works because scheduledEta != 0.
        bytes memory p3 = abi.encode(bob);
        vm.prank(admin);
        bytes32 id3 = v.schedule(BridgeVault.Action.ProposeAdmin, p3);
        uint64 eta3 = v.scheduledEta(id3);
        vm.warp(uint256(eta3) + v.ADMIN_GRACE_PERIOD());
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionCancelled(id3, freezer);
        vm.prank(freezer);
        v.cancel(id3);
        assertEq(v.scheduledEta(id3), 0);
    }

    // ------------------------------------------------------------------ access control

    function test_WrongCallersRefused() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);
        address[5] memory nonAdmins = [freezer, alice, bob, feeTo, relayer];
        for (uint256 i; i < nonAdmins.length; ++i) {
            vm.prank(nonAdmins[i]);
            vm.expectRevert(BridgeVault.NotAdmin.selector);
            v.schedule(BridgeVault.Action.SetFreezer, params);
        }

        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetFreezer, params);
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        // Non-freezers (neither admin nor freezer) cannot cancel.
        address[4] memory nonFreezers = [alice, bob, feeTo, relayer];
        for (uint256 i; i < nonFreezers.length; ++i) {
            vm.prank(nonFreezers[i]);
            vm.expectRevert(BridgeVault.NotFreezer.selector);
            v.cancel(id);
        }
        assertEq(v.scheduledEta(id), eta);

        // Non-admins (including freezer) cannot execute even at eta.
        vm.warp(eta);
        for (uint256 i; i < nonAdmins.length; ++i) {
            vm.prank(nonAdmins[i]);
            vm.expectRevert(BridgeVault.NotAdmin.selector);
            v.execute(BridgeVault.Action.SetFreezer, params);
        }
        assertEq(v.scheduledEta(id), eta);
        assertEq(v.freezer(), freezer);

        // Execute SetFreezer(bob); verify old freezer loses freeze/cancel and bob gains them.
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFreezer, params);
        assertEq(v.freezer(), bob);

        vm.prank(admin);
        bytes32 id2 = v.schedule(BridgeVault.Action.SetFeeRecipient, abi.encode(alice));

        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.cancel(id2);
        vm.prank(freezer);
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.freezeAll(true, false);

        vm.prank(bob);
        v.cancel(id2);
        assertEq(v.scheduledEta(id2), 0);

        // Clearing freezer to address(0) works via timelock; admin retains freeze & cancel.
        _execAsAdmin(BridgeVault.Action.SetFreezer, abi.encode(address(0)));
        assertEq(v.freezer(), address(0));
        vm.prank(bob);
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.freezeAll(true, false);
        vm.prank(admin);
        v.freezeAll(true, false);
        assertTrue(v.globalDepositsFrozen());
    }

    // ------------------------------------------------------------------ freeze instant, unfreeze delayed

    function test_FreezeInstantUnfreezeTimelocked() public {
        vm.warp(T0);

        // Freezer freezes token deposits & releases immediately, emitting Frozen.
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Frozen(address(usdc), true, true);
        vm.prank(freezer);
        v.freeze(address(usdc), true, true);

        // Admin freezes global deposits & releases immediately.
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Frozen(v.GLOBAL(), true, true);
        vm.prank(admin);
        v.freezeAll(true, true);

        // Deposits are blocked right away.
        _depExpect(address(usdc), bytes32(uint256(1)), BridgeVault.IsFrozen.selector);

        // Schedule partial token unfreeze (deposits only) and global deposits unfreeze.
        bytes memory pTok = abi.encode(address(usdc), true, false);
        bytes memory pGlob = abi.encode(true, false);
        vm.startPrank(admin);
        bytes32 idTok = v.schedule(BridgeVault.Action.Unfreeze, pTok);
        bytes32 idGlob = v.schedule(BridgeVault.Action.UnfreezeAll, pGlob);
        vm.stopPrank();
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        vm.warp(eta - 1);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ActionTooEarly.selector);
        v.execute(BridgeVault.Action.Unfreeze, pTok);

        vm.warp(eta);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Unfrozen(address(usdc), true, false);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idTok, BridgeVault.Action.Unfreeze, pTok, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.Unfreeze, pTok);

        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.Unfrozen(v.GLOBAL(), true, false);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idGlob, BridgeVault.Action.UnfreezeAll, pGlob, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.UnfreezeAll, pGlob);

        // Deposits work again; release freezes remain intact.
        (, bool depF, bool relF,,,,,,) = v.tokenConfig(address(usdc));
        assertFalse(depF);
        assertTrue(relF);
        assertFalse(v.globalDepositsFrozen());
        assertTrue(v.globalReleasesFrozen());
        _deposit(0, bytes32(uint256(2)));
    }

    function test_UnfreezeScheduledBeforeFreezeRefused() public {
        vm.warp(T0);
        // A (compromised) admin queues unfreezes and a fee-recipient change before any freeze.
        bytes memory pAll = abi.encode(true, true);
        bytes memory pTok = abi.encode(address(usdc), true, true);
        bytes memory pFee = abi.encode(alice);
        vm.startPrank(admin);
        bytes32 idAll = v.schedule(BridgeVault.Action.UnfreezeAll, pAll);
        bytes32 idTok = v.schedule(BridgeVault.Action.Unfreeze, pTok);
        v.schedule(BridgeVault.Action.SetFeeRecipient, pFee);
        vm.stopPrank();
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        // An incident: the freezer freezes one second before those unfreezes come due.
        vm.warp(eta - 1);
        vm.startPrank(freezer);
        v.freezeAll(true, true);
        v.freeze(address(usdc), true, true);
        vm.stopPrank();
        assertEq(v.lastFreezeAt(), eta - 1);

        // The unfreezes queued before the freeze can't lift it, in the window or later.
        vm.warp(eta);
        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.ScheduledBeforeFreeze.selector);
        v.execute(BridgeVault.Action.UnfreezeAll, pAll);
        vm.expectRevert(BridgeVault.ScheduledBeforeFreeze.selector);
        v.execute(BridgeVault.Action.Unfreeze, pTok);
        vm.warp(uint256(eta) + v.ADMIN_GRACE_PERIOD() - 1);
        vm.expectRevert(BridgeVault.ScheduledBeforeFreeze.selector);
        v.execute(BridgeVault.Action.UnfreezeAll, pAll);
        vm.stopPrank();
        assertTrue(v.globalDepositsFrozen() && v.globalReleasesFrozen());
        (, bool depF, bool relF,,,,,,) = v.tokenConfig(address(usdc));
        assertTrue(depF && relF);
        assertEq(v.scheduledEta(idAll), eta, "the refused entry stays until cancelled or expired");

        // The rule is for unfreezes only: the other action queued before the freeze still runs.
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFeeRecipient, pFee);
        assertEq(v.feeRecipient(), alice);

        // Lifting the freeze needs a new schedule, at or after the freeze, and a full delay.
        uint256 t1 = uint256(eta) + v.ADMIN_GRACE_PERIOD() - 1;
        vm.startPrank(admin);
        v.cancel(idAll);
        v.cancel(idTok);
        v.schedule(BridgeVault.Action.UnfreezeAll, pAll);
        vm.warp(t1 + ADMIN_DELAY - 1);
        vm.expectRevert(BridgeVault.ActionTooEarly.selector);
        v.execute(BridgeVault.Action.UnfreezeAll, pAll);
        vm.warp(t1 + ADMIN_DELAY);
        v.execute(BridgeVault.Action.UnfreezeAll, pAll);
        vm.stopPrank();
        assertFalse(v.globalDepositsFrozen() || v.globalReleasesFrozen());
    }

    /// Known limit (SECURITY.md, "Admin timelock"): the freezer's veto has no exception, so a
    /// stolen freezer key can freeze and then cancel every unfreeze and every change of the
    /// freezer. The vault stays frozen (no funds move); the remedy is a new vault.
    function test_KnownLimit_FreezerCanVetoItsOwnReplacement() public {
        vm.warp(T0);
        vm.prank(freezer);
        v.freezeAll(true, true);

        vm.startPrank(admin);
        bytes32 idSwap = v.schedule(BridgeVault.Action.SetFreezer, abi.encode(bob));
        bytes32 idLift = v.schedule(BridgeVault.Action.UnfreezeAll, abi.encode(true, true));
        vm.stopPrank();

        vm.startPrank(freezer);
        v.cancel(idSwap);
        v.cancel(idLift);
        vm.stopPrank();

        vm.warp(T0 + ADMIN_DELAY);
        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFreezer, abi.encode(bob));
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.UnfreezeAll, abi.encode(true, true));
        vm.stopPrank();
        assertEq(v.freezer(), freezer);
        assertTrue(v.globalDepositsFrozen() && v.globalReleasesFrozen());
    }

    // ------------------------------------------------------------------ admin handover & fee recipient

    function test_AdminHandoverRequiresTimelock() public {
        vm.warp(T0);
        bytes memory pProp = abi.encode(bob);
        vm.prank(admin);
        bytes32 idProp = v.schedule(BridgeVault.Action.ProposeAdmin, pProp);
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        // Before eta, bob cannot accept.
        vm.prank(bob);
        vm.expectRevert(BridgeVault.NotPendingAdmin.selector);
        v.acceptAdmin();

        // Also queue another action before handover to check authority transfer.
        bytes memory pFee = abi.encode(alice);
        vm.prank(admin);
        bytes32 idFee = v.schedule(BridgeVault.Action.SetFeeRecipient, pFee);

        vm.warp(eta);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.AdminProposed(bob);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idProp, BridgeVault.Action.ProposeAdmin, pProp, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.ProposeAdmin, pProp);
        assertEq(v.pendingAdmin(), bob);

        vm.prank(bob);
        v.acceptAdmin();
        assertEq(v.admin(), bob);
        assertEq(v.pendingAdmin(), address(0));

        // Old admin can no longer schedule, execute, or cancel.
        vm.startPrank(admin);
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.schedule(BridgeVault.Action.SetFreezer, abi.encode(admin));
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.execute(BridgeVault.Action.SetFeeRecipient, pFee);
        vm.expectRevert(BridgeVault.NotFreezer.selector);
        v.cancel(idFee);
        vm.stopPrank();

        // New admin (bob) can execute the action queued before handover.
        vm.prank(bob);
        v.execute(BridgeVault.Action.SetFeeRecipient, pFee);
        assertEq(v.feeRecipient(), alice);
    }

    function test_SetFeeRecipientRequiresTimelockAndRejectsZero() public {
        vm.warp(T0);
        // Zero address reverts at execute (and leaves schedule intact until cancelled/expired).
        bytes memory pZero = abi.encode(address(0));
        vm.prank(admin);
        bytes32 idZero = v.schedule(BridgeVault.Action.SetFeeRecipient, pZero);
        uint64 eta = uint64(T0) + ADMIN_DELAY;
        vm.warp(eta);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ZeroAddress.selector);
        v.execute(BridgeVault.Action.SetFeeRecipient, pZero);
        assertEq(v.feeRecipient(), feeTo);
        assertEq(v.scheduledEta(idZero), eta);

        // Valid recipient succeeds.
        bytes memory pBob = abi.encode(bob);
        vm.prank(admin);
        bytes32 idBob = v.schedule(BridgeVault.Action.SetFeeRecipient, pBob);
        uint64 etaBob = v.scheduledEta(idBob);
        vm.warp(etaBob);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.FeeRecipientSet(bob);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idBob, BridgeVault.Action.SetFeeRecipient, pBob, etaBob);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFeeRecipient, pBob);
        assertEq(v.feeRecipient(), bob);
    }
}
