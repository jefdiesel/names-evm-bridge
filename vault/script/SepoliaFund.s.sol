// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Script, console2} from "forge-std/Script.sol";

/// Sends test ETH from the deployer to one address (e.g. the v4 update submitter).
/// SEPOLIA_PK=$(cat <key file>) TO=0x… AMOUNT_WEI=… forge script script/SepoliaFund.s.sol --rpc-url <rpc> [--broadcast]
contract SepoliaFund is Script {
    function run() external {
        require(block.chainid == 11155111, "Sepolia only");
        address to = vm.envAddress("TO");
        uint256 amount = vm.envUint("AMOUNT_WEI");
        require(to != address(0) && amount != 0 && amount <= 0.2 ether, "TO or AMOUNT_WEI");
        vm.startBroadcast(vm.envUint("SEPOLIA_PK"));
        (bool ok,) = to.call{value: amount}("");
        require(ok, "transfer failed");
        vm.stopBroadcast();
        console2.log("sent wei", amount);
        console2.log("to", to);
    }
}
