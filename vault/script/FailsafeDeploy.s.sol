// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVaultFailsafe} from "../src/failsafe/BridgeVaultFailsafe.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken} from "../test/mocks/Mocks.sol";

/// Local failsafe demo only (anvil, lez-bridge/scripts/e2e-failsafe.sh): the failsafe vault
/// variant with the TEST MockVerifier (accepts a seal equal to abi.encodePacked(imageId,
/// sha256(journal))); `updateImageId` is the G2-lite TrustedTip guest, `releaseImageId` the G3
/// guest and `reclaimImageId` the G4 reclaim guest, all real image ids from their hosts.
/// "mock-verified: proofs executed, not proved; LEZ finality trusted for the test."
///
/// One token: a 6-decimal test USDC (100 / 1000) with 1% deposit fees, 0.5% withdraw fees, relay
/// fees 0.5% and 1% on the menu, release delay 0. The broadcaster is admin and freezer.
///
/// Env: PROGRAM_ID, UPDATE_IMAGE_ID, RELEASE_IMAGE_ID, RECLAIM_IMAGE_ID, G2_STATE, LEZ_CHAIN_ID,
/// ZONE_ID (bytes32, required). Prints VAULT=, USDC=, MOCK=, CFG_ID=, G2_STATE=.
contract FailsafeDeploy is Script {
    function run() external {
        vm.startBroadcast();
        MockVerifier mv = new MockVerifier();
        TestToken usdc = new TestToken("USDC", 6);
        BridgeVaultFailsafe v = new BridgeVaultFailsafe(
            IRiscZeroVerifier(address(mv)),
            vm.envBytes32("UPDATE_IMAGE_ID"),
            vm.envBytes32("RELEASE_IMAGE_ID"),
            vm.envBytes32("RECLAIM_IMAGE_ID"),
            vm.envBytes32("LEZ_CHAIN_ID"),
            vm.envBytes32("ZONE_ID"),
            vm.envBytes32("PROGRAM_ID"),
            vm.envBytes32("G2_STATE"),
            0,
            msg.sender,
            msg.sender,
            msg.sender
        );
        uint256[] memory d = new uint256[](2);
        uint256[] memory df = new uint256[](2);
        uint256[] memory wf = new uint256[](2);
        uint256[][] memory menu = new uint256[][](2);
        (d[0], d[1]) = (100e6, 1000e6);
        (df[0], df[1]) = (1e6, 10e6);
        (wf[0], wf[1]) = (0.5e6, 5e6);
        v.registerToken(address(usdc), 6, d, df, wf, 0);
        menu[0] = new uint256[](2);
        (menu[0][0], menu[0][1]) = (0.5e6, 1e6);
        menu[1] = new uint256[](2);
        (menu[1][0], menu[1][1]) = (5e6, 10e6);
        v.setRelayFeeMenu(address(usdc), menu);
        usdc.mint(msg.sender, 1_000_000e6);
        usdc.approve(address(v), type(uint256).max);
        vm.stopBroadcast();

        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("MOCK=%s", address(mv));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("G2_STATE=%s", vm.toString(v.g2State()));
    }
}
