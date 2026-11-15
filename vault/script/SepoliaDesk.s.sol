// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {DepositDesk, IBridgeVaultDeposits} from "../src/desk/DepositDesk.sol";

/// DepositDesk on Sepolia (chain 11155111) in front of the LIVE v2 demo vault, which is
/// immutable and is not redeployed. A demo-network convenience: the desk takes a bridge fee of
/// DESK_FEE_BPS (default 300 = 3%, at most 300) of each denomination, in the deposited asset, on
/// top of it, and forwards the deposit to the vault in the same call (`src/desk/DepositDesk.sol`).
///
/// Owner (withdraws fees, may set the fee within 3%): the throwaway demo deployer, who is also
/// the vault's admin, freezer and fee recipient.
///
/// FIRST STEP, before any run that sends: `node scripts/check-versions.mjs --onchain --deployment <the vault's deployment>`
/// from the repository root passes (VERSIONS.toml; AGENTS.md "Version alignment").
///
/// Env (the same key mechanism as SepoliaDeploy.s.sol):
///   SEPOLIA_PK     deployer key (never printed); must be DEPLOYER below
///   DESK_FEE_BPS   optional, default 300; 0..300
///   EXPECTED_DESK  optional: the desk address the simulation predicted; aborts on mismatch
///   DESK_VAULT, DESK_VAULT_CFG_ID   optional, together: another vault and its cfgId (the v3
///                  vault, public-demo/V3-PLAN.md); default the v2 vault below
///
///   forge script script/SepoliaDesk.s.sol --rpc-url <sepolia>                 # simulate
///   ... --broadcast --slow --with-gas-price <cap> --priority-gas-price <tip>   # send
///
/// GAS: Sepolia prices state creation under Glamsterdam (DEPLOYMENTS.md); take the limit from
/// eth_estimateGas or pass a large --gas-estimate-multiplier, and cap the fee.
contract SepoliaDesk is Script {
    address constant DEPLOYER = 0x37a46E7334Dd061E0C7E2298247EE9278d0d321F;
    uint256 constant SEPOLIA = 11155111;
    /// The live Sepolia v2 demo vault (vault/DEPLOYMENTS.md) and its cfgId.
    address constant VAULT_V2 = 0x8C8b2664De28e5287158db4DB65D448638871b74;
    bytes32 constant CFG_ID_V2 = 0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11;

    /// Virtual only so the test harness (test/SepoliaDeskScript.t.sol) can run this logic with a
    /// public test key and a local vault.
    function _deployer() internal view virtual returns (address) {
        return DEPLOYER;
    }

    function _vault() internal view virtual returns (address) {
        return vm.envOr("DESK_VAULT", VAULT_V2);
    }

    function _cfgId() internal view virtual returns (bytes32) {
        address v = vm.envOr("DESK_VAULT", VAULT_V2);
        // Another vault must name its cfgId too: the check below then binds the desk to it.
        return v == VAULT_V2 ? CFG_ID_V2 : vm.envBytes32("DESK_VAULT_CFG_ID");
    }

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        uint256 pk = vm.envUint("SEPOLIA_PK");
        uint256 bps = vm.envOr("DESK_FEE_BPS", uint256(300));
        DepositDesk d = deploy(pk, bps);
        address want = vm.envOr("EXPECTED_DESK", address(0));
        if (want != address(0)) require(address(d) == want, "desk address differs from EXPECTED_DESK");
    }

    function deploy(uint256 pk, uint256 bps) public returns (DepositDesk d) {
        require(vm.addr(pk) == _deployer(), "SEPOLIA_PK is not the expected deployer");
        require(bps <= 300, "DESK_FEE_BPS above 300");
        BridgeVault v = BridgeVault(payable(_vault()));
        require(address(v).code.length > 0, "no vault code");
        require(v.cfgId() == _cfgId(), "vault cfgId is not the expected one");

        vm.startBroadcast(pk);
        d = new DepositDesk(IBridgeVaultDeposits(address(v)), _deployer(), bps);
        vm.stopBroadcast();

        require(address(d.vault()) == address(v) && d.owner() == _deployer() && d.feeBps() == bps, "desk state");
        console.log("DESK=%s", address(d));
        console.log("DESK_VAULT=%s", address(v));
        console.log("DESK_OWNER=%s", d.owner());
        console.log("DESK_FEE_BPS=%s", d.feeBps());
    }
}
