// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken} from "../test/mocks/Mocks.sol";

/// Local end-to-end only (anvil): a vault with the mock verifier and a 6-decimal test token.
/// The vault derives its cfgId from the chain id, its own address and the LEZ ids below, and
/// this script prints it as CFG_ID=0x... for the LEZ bridge's cfg_id.
/// Env: PROGRAM_ID (bytes32, required), LEZ_CHAIN_ID and ZONE_ID (bytes32, default zero).
contract LocalDeploy is Script {
    function run() external {
        bytes32 lezChainId = vm.envOr("LEZ_CHAIN_ID", bytes32(0));
        bytes32 zoneId = vm.envOr("ZONE_ID", bytes32(0));
        bytes32 programId = vm.envBytes32("PROGRAM_ID");
        vm.startBroadcast();
        MockVerifier mv = new MockVerifier();
        TestToken usdc = new TestToken("USDC", 6);
        BridgeVault v = new BridgeVault(
            IRiscZeroVerifier(address(mv)),
            keccak256("g2"),
            keccak256("g3"),
            lezChainId,
            zoneId,
            programId,
            bytes32(0),
            0,
            msg.sender,
            msg.sender,
            msg.sender
        );
        uint256[] memory d = new uint256[](2);
        d[0] = 100e6;
        d[1] = 1000e6;
        uint256[] memory f = new uint256[](2);
        f[0] = 1e6;
        f[1] = 10e6;
        v.registerToken(address(usdc), 6, d, f, f, 0);
        usdc.mint(msg.sender, 1_000_000e6);
        usdc.approve(address(v), type(uint256).max);
        vm.stopBroadcast();
        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
    }
}
