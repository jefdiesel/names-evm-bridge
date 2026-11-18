// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {SepoliaDesk} from "../script/SepoliaDesk.s.sol";

/// script/SepoliaDesk.s.sol with ONE change: the expected deployer is Anvil account 0 (a public
/// test key). The vault and its cfgId come from the env as in the script (DESK_VAULT,
/// DESK_VAULT_CFG_ID). For throwaway rehearsals on an anvil started with --chain-id 11155111
/// (public-demo/v6/rehearse-v6.sh):
///
///   SEPOLIA_PK=<anvil 0 key> DESK_FEE_BPS=30 DESK_VAULT=<vault> DESK_VAULT_CFG_ID=<cfgId> \
///   forge script test/SepoliaDeskEnvHarness.t.sol:SepoliaDeskEnvHarness --rpc-url http://127.0.0.1:<anvil> --broadcast
contract SepoliaDeskEnvHarness is SepoliaDesk {
    function _deployer() internal pure override returns (address) {
        return 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    }
}
