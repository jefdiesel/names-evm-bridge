// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {DepositDesk} from "../src/desk/DepositDesk.sol";

/// Change the Sepolia DepositDesk's bridge fee (owner only; the desk caps it at 300 bps).
///
/// FIRST STEP, before any run that sends: `node scripts/check-versions.mjs --onchain --deployment <the desk's deployment>`
/// from the repository root passes (VERSIONS.toml; AGENTS.md "Version alignment").
///
/// Env (the key mechanism of the other Sepolia scripts):
///   SEPOLIA_PK   the desk owner's key (never printed)
///   FEE_BPS      the new fee in basis points, 0..300 (30 = 0.3%)
///   DESK         optional, default the deployed desk below
///
///   forge script script/SepoliaDeskSetFee.s.sol --rpc-url <sepolia>               # simulate only
///   ... --broadcast --slow --with-gas-price <cap> --priority-gas-price <tip>       # send
contract SepoliaDeskSetFee is Script {
    uint256 constant SEPOLIA = 11155111;
    address constant DESK_V2 = 0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2;

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        DepositDesk desk = DepositDesk(vm.envOr("DESK", DESK_V2));
        uint256 next = vm.envUint("FEE_BPS");
        require(next <= desk.MAX_FEE_BPS(), "above the desk's cap");
        uint256 pk = vm.envUint("SEPOLIA_PK");
        require(vm.addr(pk) == desk.owner(), "not the desk owner");
        console.log("FEE_BPS_BEFORE=%s", desk.feeBps());
        vm.startBroadcast(pk);
        desk.setFeeBps(next);
        vm.stopBroadcast();
        console.log("FEE_BPS_AFTER=%s", desk.feeBps());
    }
}
