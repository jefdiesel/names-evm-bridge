// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken} from "../test/mocks/Mocks.sol";

/// Local round trip only (anvil, lez-bridge/scripts/e2e-roundtrip.sh): a vault with the TEST
/// MockVerifier (accepts a seal equal to abi.encodePacked(imageId, sha256(journal))) whose
/// `updateImageId` is the G2-lite TrustedTip guest and `releaseImageId` the G3 guest, both real
/// image ids from outbound_host. "mock-verified: proofs executed, not proved; LEZ finality
/// trusted for the test."
///
/// Two tokens: a 6-decimal test USDC (100 / 1000) and ETH (0.01 / 0.1), with 1% deposit fees,
/// 0.5% withdraw fees, relay-fee menus of 0.5% and 1%, minReleaseDelay 0 and a per-token release
/// delay of RELEASE_DELAY seconds (default 20), so the round trip waits in real time and never
/// warps the shared anvil's clock.
///
/// Env: PROGRAM_ID, UPDATE_IMAGE_ID, RELEASE_IMAGE_ID, G2_STATE (bytes32, required);
/// LEZ_CHAIN_ID, ZONE_ID (default zero); RELEASE_DELAY (default 20); FEE_RECIPIENT (default the
/// broadcaster). Prints VAULT=, USDC=, MOCK=, CFG_ID=.
contract RoundTripDeploy is Script {
    function run() external {
        bytes32 programId = vm.envBytes32("PROGRAM_ID");
        bytes32 updateImageId = vm.envBytes32("UPDATE_IMAGE_ID");
        bytes32 releaseImageId = vm.envBytes32("RELEASE_IMAGE_ID");
        bytes32 g2State = vm.envBytes32("G2_STATE");
        bytes32 lezChainId = vm.envOr("LEZ_CHAIN_ID", bytes32(0));
        bytes32 zoneId = vm.envOr("ZONE_ID", bytes32(0));
        uint64 delay = uint64(vm.envOr("RELEASE_DELAY", uint256(20)));

        vm.startBroadcast();
        address feeRecipient = vm.envOr("FEE_RECIPIENT", msg.sender);
        MockVerifier mv = new MockVerifier();
        TestToken usdc = new TestToken("USDC", 6);
        BridgeVault v = new BridgeVault(
            IRiscZeroVerifier(address(mv)),
            updateImageId,
            releaseImageId,
            lezChainId,
            zoneId,
            programId,
            g2State,
            0,
            msg.sender,
            msg.sender,
            feeRecipient
        );

        uint256[] memory d = new uint256[](2);
        uint256[] memory df = new uint256[](2);
        uint256[] memory wf = new uint256[](2);
        uint256[][] memory menu = new uint256[][](2);

        d[0] = 100e6;
        d[1] = 1000e6;
        df[0] = 1e6;
        df[1] = 10e6;
        wf[0] = 0.5e6;
        wf[1] = 5e6;
        v.registerToken(address(usdc), 6, d, df, wf, delay);
        menu[0] = _two(0.5e6, 1e6);
        menu[1] = _two(5e6, 10e6);
        v.setRelayFeeMenu(address(usdc), menu);

        d[0] = 0.01 ether;
        d[1] = 0.1 ether;
        df[0] = 0.0001 ether;
        df[1] = 0.001 ether;
        wf[0] = 0.00005 ether;
        wf[1] = 0.0005 ether;
        v.registerToken(address(0), 18, d, df, wf, delay);
        menu[0] = _two(0.00005 ether, 0.0001 ether);
        menu[1] = _two(0.0005 ether, 0.001 ether);
        v.setRelayFeeMenu(address(0), menu);

        usdc.mint(msg.sender, 1_000_000e6);
        usdc.approve(address(v), type(uint256).max);
        vm.stopBroadcast();

        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("MOCK=%s", address(mv));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("G2_STATE=%s", vm.toString(v.g2State()));
    }

    function _two(uint256 a, uint256 b) internal pure returns (uint256[] memory m) {
        m = new uint256[](2);
        m[0] = a;
        m[1] = b;
    }
}
