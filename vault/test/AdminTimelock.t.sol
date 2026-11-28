// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// M4 / red-team B1.5: the admin timelock's lifecycle. Constructor delay bounds, schedule (id,
/// eta, event, no overwrite while active), the execution window (too early, exact eta, last
/// second of grace, expired), double execute and unscheduled actions refused, and a fuzz over
/// delays and times. Split from the role and rule tests so no test contract hits the
/// compiler's size limit (solc 0.8.28 via-IR: "Tag too large for reserved space").
contract AdminTimelockTest is VaultFixture {
    uint256 constant T0 = 1_000_000;

    function _deployWithDelay(uint64 delay_) internal returns (BridgeVault) {
        return new BridgeVault(
            IRiscZeroVerifier(address(mv)),
            G2,
            G3,
            LEZ_CHAIN,
            ZONE,
            PROGRAM,
            S0,
            1 hours,
            admin,
            freezer,
            feeTo,
            delay_
        );
    }

    // ------------------------------------------------------------------ constructor bounds

    function test_ConstructorAdminDelayBounds() public {
        assertEq(v.adminDelay(), ADMIN_DELAY, "fixture adminDelay");

        uint64 minD = v.MIN_ADMIN_DELAY();
        uint64 maxD = v.MAX_ADMIN_DELAY();
        assertEq(minD, 1 hours);
        assertEq(maxD, 30 days);
        assertEq(v.ADMIN_GRACE_PERIOD(), 7 days);

        vm.expectRevert(BridgeVault.AdminDelayOutOfBounds.selector);
        _deployWithDelay(0);

        vm.expectRevert(BridgeVault.AdminDelayOutOfBounds.selector);
        _deployWithDelay(minD - 1);

        vm.expectRevert(BridgeVault.AdminDelayOutOfBounds.selector);
        _deployWithDelay(maxD + 1);

        vm.expectRevert(BridgeVault.AdminDelayOutOfBounds.selector);
        _deployWithDelay(type(uint64).max);

        BridgeVault vMin = _deployWithDelay(minD);
        assertEq(vMin.adminDelay(), minD);

        BridgeVault vMax = _deployWithDelay(maxD);
        assertEq(vMax.adminDelay(), maxD);
    }

    // ------------------------------------------------------------------ schedule, events, duplicates

    function test_ScheduleRecordsEtaAndEmitsEvent() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);
        bytes32 expectedId = v.actionId(BridgeVault.Action.SetFreezer, params);
        assertEq(expectedId, keccak256(abi.encode(BridgeVault.Action.SetFreezer, params)));
        uint64 expectedEta = uint64(T0) + ADMIN_DELAY;

        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionScheduled(expectedId, BridgeVault.Action.SetFreezer, params, expectedEta);

        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetFreezer, params);

        assertEq(id, expectedId);
        assertEq(v.scheduledEta(id), expectedEta);
        assertEq(v.freezer(), freezer, "schedule alone must not apply action");
    }

    function test_RescheduleRules() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);
        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetFreezer, params);
        uint64 eta1 = uint64(T0) + ADMIN_DELAY;

        // Refused immediately, at eta, and on the last second of the grace period.
        vm.prank(admin);
        vm.expectRevert(BridgeVault.AlreadyScheduled.selector);
        v.schedule(BridgeVault.Action.SetFreezer, params);

        vm.warp(eta1);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.AlreadyScheduled.selector);
        v.schedule(BridgeVault.Action.SetFreezer, params);

        vm.warp(uint256(eta1) + v.ADMIN_GRACE_PERIOD() - 1);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.AlreadyScheduled.selector);
        v.schedule(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), eta1, "eta unchanged after refused reschedules");

        // Allowed once expired (at eta + ADMIN_GRACE_PERIOD).
        uint256 tExp = uint256(eta1) + v.ADMIN_GRACE_PERIOD();
        vm.warp(tExp);
        uint64 eta2 = uint64(tExp) + ADMIN_DELAY;
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionScheduled(id, BridgeVault.Action.SetFreezer, params, eta2);
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), eta2);

        // Allowed after cancel.
        vm.prank(freezer);
        v.cancel(id);
        uint256 tAfterCancel = tExp + 10;
        vm.warp(tAfterCancel);
        uint64 eta3 = uint64(tAfterCancel) + ADMIN_DELAY;
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), eta3);

        // Allowed after execute.
        vm.warp(eta3);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), 0);

        uint256 tAfterExec = uint256(eta3) + 10;
        vm.warp(tAfterExec);
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), uint64(tAfterExec) + ADMIN_DELAY);
    }

    // ------------------------------------------------------------------ timing boundaries & double execute

    function test_ExecuteTooEarlyAndExactEta() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);
        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetFreezer, params);
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        // At schedule time: too early.
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ActionTooEarly.selector);
        v.execute(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), eta);
        assertEq(v.freezer(), freezer);

        // One second before eta: still too early.
        vm.warp(eta - 1);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ActionTooEarly.selector);
        v.execute(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), eta);
        assertEq(v.freezer(), freezer);

        // At exact eta: succeeds and emits both inner event and ActionExecuted with all fields.
        vm.warp(eta);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.FreezerSet(bob);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(id, BridgeVault.Action.SetFreezer, params, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFreezer, params);

        assertEq(v.freezer(), bob);
        assertEq(v.scheduledEta(id), 0);
    }

    function test_ExecuteGracePeriodBoundaryAndExpired() public {
        vm.warp(T0);
        bytes memory paramsA = abi.encode(bob);
        bytes memory paramsB = abi.encode(alice);
        vm.startPrank(admin);
        bytes32 idA = v.schedule(BridgeVault.Action.SetFreezer, paramsA);
        bytes32 idB = v.schedule(BridgeVault.Action.SetFeeRecipient, paramsB);
        vm.stopPrank();
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        // Last valid second (eta + ADMIN_GRACE_PERIOD - 1): succeeds and emits original eta.
        vm.warp(uint256(eta) + v.ADMIN_GRACE_PERIOD() - 1);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.FreezerSet(bob);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idA, BridgeVault.Action.SetFreezer, paramsA, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFreezer, paramsA);
        assertEq(v.freezer(), bob);

        // Exact expiry boundary (eta + ADMIN_GRACE_PERIOD): refused.
        vm.warp(uint256(eta) + v.ADMIN_GRACE_PERIOD());
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ActionExpired.selector);
        v.execute(BridgeVault.Action.SetFeeRecipient, paramsB);
        assertEq(v.feeRecipient(), feeTo);
        assertEq(v.scheduledEta(idB), eta);

        // Well past expiry: still refused.
        vm.warp(uint256(eta) + v.ADMIN_GRACE_PERIOD() + 10 days);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.ActionExpired.selector);
        v.execute(BridgeVault.Action.SetFeeRecipient, paramsB);
        assertEq(v.feeRecipient(), feeTo);
    }

    function test_DoubleExecuteAndUnscheduledRefused() public {
        vm.warp(T0);
        bytes memory params = abi.encode(bob);

        // Never scheduled.
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFreezer, params);

        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetFreezer, params);
        uint64 eta = uint64(T0) + ADMIN_DELAY;
        vm.warp(eta);

        // Different params or different action id: not scheduled.
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFreezer, abi.encode(alice));

        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFeeRecipient, params);

        // First execute succeeds.
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFreezer, params);
        assertEq(v.scheduledEta(id), 0);

        // Second execute of the same action is refused.
        vm.prank(admin);
        vm.expectRevert(BridgeVault.NotScheduled.selector);
        v.execute(BridgeVault.Action.SetFreezer, params);
    }

    // ------------------------------------------------------------------ fuzz: timing window

    function testFuzz_TimingWindow(uint64 delay, uint256 dt) public {
        delay = uint64(bound(delay, v.MIN_ADMIN_DELAY(), v.MAX_ADMIN_DELAY()));
        uint64 grace = v.ADMIN_GRACE_PERIOD();
        dt = bound(dt, 0, uint256(delay) + uint256(grace) + 30 days);

        BridgeVault vault = _deployWithDelay(delay);
        vm.warp(T0);

        bytes memory params = abi.encode(bob);
        vm.prank(admin);
        bytes32 id = vault.schedule(BridgeVault.Action.SetFreezer, params);
        uint64 eta = uint64(T0) + delay;
        assertEq(vault.scheduledEta(id), eta);

        uint256 t = T0 + dt;
        vm.warp(t);

        if (t < eta) {
            vm.prank(admin);
            vm.expectRevert(BridgeVault.ActionTooEarly.selector);
            vault.execute(BridgeVault.Action.SetFreezer, params);
            assertEq(vault.scheduledEta(id), eta);
            assertEq(vault.freezer(), freezer);
        } else if (t < uint256(eta) + grace) {
            vm.prank(admin);
            vault.execute(BridgeVault.Action.SetFreezer, params);
            assertEq(vault.freezer(), bob);
            assertEq(vault.scheduledEta(id), 0);

            // Double execute refused.
            vm.prank(admin);
            vm.expectRevert(BridgeVault.NotScheduled.selector);
            vault.execute(BridgeVault.Action.SetFreezer, params);
        } else {
            vm.prank(admin);
            vm.expectRevert(BridgeVault.ActionExpired.selector);
            vault.execute(BridgeVault.Action.SetFreezer, params);
            assertEq(vault.freezer(), freezer);
        }
    }
}
