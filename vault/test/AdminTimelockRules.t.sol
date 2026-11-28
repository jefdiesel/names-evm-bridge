// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// M4 / red-team B1.5: tightening is instant, weakening needs the admin timelock, for
/// per-token caps, global caps, cap weights, release delays and fees.
contract AdminTimelockRulesTest is VaultFixture {
    uint256 constant T0 = 1_000_000;

    // ------------------------------------------------------------------ caps: tightening instant, weakening timelocked

    function test_PerTokenCapsTighteningInstantWeakeningTimelocked() public {
        vm.warp(T0);
        vm.startPrank(admin);

        // Enabling from disabled (window == 0) is instant.
        v.setCaps(address(usdc), 1 hours, 1000e6, 1000e6);
        // Tightening (lower caps, longer window, or identical values) is instant.
        v.setCaps(address(usdc), 1 hours, 1000e6, 1000e6);
        v.setCaps(address(usdc), 2 hours, 800e6, 600e6);

        // Every weakening direction is refused directly:
        // 1. Raise deposit cap
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCaps(address(usdc), 2 hours, 801e6, 600e6);
        // 2. Raise release cap
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCaps(address(usdc), 2 hours, 800e6, 601e6);
        // 3. Shorten window
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCaps(address(usdc), 2 hours - 1, 800e6, 600e6);
        // 4. Disable caps (window == 0)
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCaps(address(usdc), 0, 0, 0);
        vm.stopPrank();

        // Weakening via timelock succeeds and emits CapsSet + ActionExecuted.
        bytes memory pRaise = abi.encode(address(usdc), uint64(1 hours), uint256(5000e6), uint256(4000e6));
        vm.prank(admin);
        bytes32 idRaise = v.schedule(BridgeVault.Action.SetCaps, pRaise);
        uint64 eta = uint64(T0) + ADMIN_DELAY;
        vm.warp(eta);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.CapsSet(address(usdc), 1 hours, 5000e6, 4000e6);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idRaise, BridgeVault.Action.SetCaps, pRaise, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetCaps, pRaise);

        (uint64 w, uint256 dc, uint256 rc,,) = v.capState(address(usdc));
        assertEq(w, 1 hours);
        assertEq(dc, 5000e6);
        assertEq(rc, 4000e6);

        // Disabling caps via timelock succeeds.
        _execAsAdmin(BridgeVault.Action.SetCaps, abi.encode(address(usdc), uint64(0), uint256(0), uint256(0)));
        (w, dc, rc,,) = v.capState(address(usdc));
        assertEq(w, 0);
        assertEq(dc, 0);
        assertEq(rc, 0);
    }

    function test_GlobalCapsTighteningInstantWeakeningTimelocked() public {
        vm.warp(T0);
        vm.startPrank(admin);

        // Disabling when already disabled still requires timelock (_isCapTightening returns false for newWindow == 0).
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setGlobalCaps(0, 0, 0);

        // Enabling and tightening are instant.
        v.setGlobalCaps(1 hours, 10_000e6, 10_000e6);
        v.setGlobalCaps(2 hours, 5_000e6, 4_000e6);

        // Weakening directly is refused in all 4 directions.
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setGlobalCaps(2 hours, 5_001e6, 4_000e6);
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setGlobalCaps(2 hours, 5_000e6, 4_001e6);
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setGlobalCaps(2 hours - 1, 5_000e6, 4_000e6);
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setGlobalCaps(0, 0, 0);
        vm.stopPrank();

        // Weakening via Action.SetGlobalCaps succeeds.
        bytes memory pLoosen = abi.encode(uint64(1 hours), uint256(20_000e6), uint256(15_000e6));
        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetGlobalCaps, pLoosen);
        uint64 eta = uint64(T0) + ADMIN_DELAY;
        vm.warp(eta);
        vm.expectEmit(false, false, false, true, address(v));
        emit BridgeVault.GlobalCapsSet(1 hours, 20_000e6, 15_000e6);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(id, BridgeVault.Action.SetGlobalCaps, pLoosen, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetGlobalCaps, pLoosen);

        (uint64 w, uint256 dc, uint256 rc,,) = v.globalCapState();
        assertEq(w, 1 hours);
        assertEq(dc, 20_000e6);
        assertEq(rc, 15_000e6);

        // Disabling via Action.SetGlobalCaps succeeds.
        _execAsAdmin(BridgeVault.Action.SetGlobalCaps, abi.encode(uint64(0), uint256(0), uint256(0)));
        (w, dc, rc,,) = v.globalCapState();
        assertEq(w, 0);
    }

    // ------------------------------------------------------------------ capWeight, releaseDelay, fees

    function test_CapWeightTighteningInstantWeakeningTimelocked() public {
        vm.warp(T0);
        vm.startPrank(admin);
        // Raising or keeping weight is instant.
        v.setCapWeight(address(usdc), 1e18);
        v.setCapWeight(address(usdc), 2e18);
        v.setCapWeight(address(usdc), 2e18);

        // Lowering (partially or to 0) requires timelock.
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCapWeight(address(usdc), 2e18 - 1);
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setCapWeight(address(usdc), 0);
        vm.stopPrank();

        bytes memory pLower = abi.encode(address(usdc), uint256(0));
        vm.prank(admin);
        bytes32 id = v.schedule(BridgeVault.Action.SetCapWeight, pLower);
        uint64 eta = uint64(T0) + ADMIN_DELAY;
        vm.warp(eta);
        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.CapWeightSet(address(usdc), 0);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(id, BridgeVault.Action.SetCapWeight, pLower, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetCapWeight, pLower);
        assertEq(v.capWeight(address(usdc)), 0);
    }

    function test_ReleaseDelayTighteningInstantWeakeningTimelocked() public {
        vm.warp(T0);
        vm.startPrank(admin);
        // Raising or keeping release delay is instant (emergency brake).
        v.setReleaseDelay(address(usdc), 2 days);
        v.setReleaseDelay(address(usdc), 2 days);

        // Below immutable minReleaseDelay reverts DelayTooShort (not TimelockRequired).
        vm.expectRevert(BridgeVault.DelayTooShort.selector);
        v.setReleaseDelay(address(usdc), 1 hours - 1);

        // Lowering to >= minReleaseDelay directly reverts TimelockRequired.
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setReleaseDelay(address(usdc), 1 hours);
        vm.stopPrank();

        // Timelock still cannot go below minReleaseDelay.
        bytes memory pTooShort = abi.encode(address(usdc), uint64(1 hours - 1));
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetReleaseDelay, pTooShort);
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        bytes memory pLower = abi.encode(address(usdc), uint64(1 hours));
        vm.prank(admin);
        bytes32 idLower = v.schedule(BridgeVault.Action.SetReleaseDelay, pLower);

        vm.warp(eta);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.DelayTooShort.selector);
        v.execute(BridgeVault.Action.SetReleaseDelay, pTooShort);

        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.ReleaseDelaySet(address(usdc), 1 hours);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idLower, BridgeVault.Action.SetReleaseDelay, pLower, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetReleaseDelay, pLower);

        (uint64 delay,,) = v.releaseDelayState(address(usdc));
        assertEq(delay, 1 hours);
    }

    function test_FeesTighteningInstantWeakeningTimelocked() public {
        vm.warp(T0);
        // Fixture starts usdc fees at deposit [1e6, 10e6], withdraw [1e6, 10e6] (1% on 100/1000).
        vm.startPrank(admin);
        // Lowering or keeping unchanged is instant.
        v.setFees(address(usdc), _arr(0, 5e6), _arr(1e6, 5e6));
        v.setFees(address(usdc), _arr(0, 5e6), _arr(1e6, 5e6));

        // Raising any deposit fee or withdraw fee directly reverts TimelockRequired.
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setFees(address(usdc), _arr(1, 5e6), _arr(1e6, 5e6));
        vm.expectRevert(BridgeVault.TimelockRequired.selector);
        v.setFees(address(usdc), _arr(0, 5e6), _arr(1e6, 5e6 + 1));

        // Exceeding MAX_FEE_BPS (2%) still reverts FeeTooHigh directly.
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.setFees(address(usdc), _arr(2e6 + 1, 5e6), _arr(1e6, 5e6));
        vm.stopPrank();

        // Exceeding MAX_FEE_BPS via timelock still reverts FeeTooHigh at execute.
        bytes memory pTooHigh = abi.encode(address(usdc), _arr(2e6 + 1, 20e6), _arr(1e6, 10e6));
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetFees, pTooHigh);

        // Raising within MAX_FEE_BPS via timelock succeeds.
        uint256[] memory nextDep = _arr(2e6, 20e6);
        uint256[] memory nextW = _arr(2e6, 20e6);
        bytes memory pRaise = abi.encode(address(usdc), nextDep, nextW);
        vm.prank(admin);
        bytes32 idRaise = v.schedule(BridgeVault.Action.SetFees, pRaise);
        uint64 eta = uint64(T0) + ADMIN_DELAY;

        vm.warp(eta);
        vm.prank(admin);
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.execute(BridgeVault.Action.SetFees, pTooHigh);

        vm.expectEmit(true, false, false, true, address(v));
        emit BridgeVault.FeesSet(address(usdc), nextDep, nextW);
        vm.expectEmit(true, true, false, true, address(v));
        emit BridgeVault.ActionExecuted(idRaise, BridgeVault.Action.SetFees, pRaise, eta);
        vm.prank(admin);
        v.execute(BridgeVault.Action.SetFees, pRaise);

        (,,,,,, uint256[] memory df, uint256[] memory wf,) = v.tokenConfig(address(usdc));
        assertEq(df[0], 2e6);
        assertEq(df[1], 20e6);
        assertEq(wf[0], 2e6);
        assertEq(wf[1], 20e6);
    }
}
