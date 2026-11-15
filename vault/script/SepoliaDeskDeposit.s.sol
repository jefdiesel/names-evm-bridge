// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {DepositDesk} from "../src/desk/DepositDesk.sol";

/// One native-ETH deposit through the Sepolia DepositDesk for a commitment made elsewhere (for
/// the relayed-claim test: the secret stays where the commitment was made; only C comes here).
/// It names no LEZ account to the desk (lezPublicKey = 0), as the deposit page does.
///
/// FIRST STEP, before any run that sends: `node scripts/check-versions.mjs --onchain --deployment <the desk's deployment>`
/// from the repository root passes (VERSIONS.toml; AGENTS.md "Version alignment").
///
/// Env (the key mechanism of the other Sepolia scripts):
///   SEPOLIA_PK    the depositor's key (never printed)
///   COMMITMENT    0x… 32 bytes (lez-bridge commitment)
///   DENOM_INDEX   optional, default 0 (0.01 ETH on the v2 vault)
///   DESK          optional, default the deployed desk below
///
///   forge script script/SepoliaDeskDeposit.s.sol --rpc-url <sepolia>              # simulate only
///   ... --broadcast --slow --with-gas-price <cap> --priority-gas-price <tip> --gas-estimate-multiplier 250   # send
///
/// Pass `--gas-estimate-multiplier 250`: forge's estimate can be far too low. On 2026-11-14 it set
/// a 279,152 gas limit for the leaf-8 deposit, which ran out of gas at 276,405 (that failed
/// transaction cost only gas); the real need was about 431,000, because leaf 8 starts a new level
/// of the vault's tree.
///
/// Before sending it checks: Sepolia; the desk forwards to the v2 demo vault, whose cfgId is the
/// demo's; the commitment is unused; then it sends exactly quote(ETH, i).total (denomination +
/// the vault's fee + the bridge fee, read on chain). It prints the leaf index the call returns.
/// The transaction hash is in forge's own output (and broadcast/…/run-latest.json).
contract SepoliaDeskDeposit is Script {
    uint256 constant SEPOLIA = 11155111;
    address constant ETH = address(0);
    address constant DESK_V2 = 0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2;
    address constant VAULT_V2 = 0x8C8b2664De28e5287158db4DB65D448638871b74;
    bytes32 constant CFG_ID_V2 = 0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11;

    /// Virtual only so the test harness (test/SepoliaDeskDepositScript.t.sol) can use a local vault.
    /// DESK_VAULT / DESK_VAULT_CFG_ID (env, optional) name another demo vault (v3) and its cfgId.
    function _vault() internal view virtual returns (address) {
        return vm.envOr("DESK_VAULT", VAULT_V2);
    }

    function _cfgId() internal view virtual returns (bytes32) {
        return vm.envOr("DESK_VAULT_CFG_ID", CFG_ID_V2);
    }

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        uint256 pk = vm.envUint("SEPOLIA_PK");
        bytes32 c = vm.envBytes32("COMMITMENT");
        uint32 i = uint32(vm.envOr("DENOM_INDEX", uint256(0)));
        address desk = vm.envOr("DESK", DESK_V2);
        deposit(pk, DepositDesk(payable(desk)), i, c);
    }

    function deposit(uint256 pk, DepositDesk d, uint32 denomIndex, bytes32 c) public returns (uint64 leaf) {
        require(address(d).code.length > 0, "no desk code");
        require(address(d.vault()) == _vault(), "the desk does not forward to the expected demo vault");
        BridgeVault v = BridgeVault(payable(_vault()));
        require(v.cfgId() == _cfgId(), "vault cfgId is not the expected demo vault's");
        require(c != bytes32(0), "COMMITMENT is zero");
        require(!v.commitmentUsed(c), "commitment already used in the vault");
        (uint256 denom, uint256 vaultFee, uint256 bridgeFee, uint256 total) = d.quote(ETH, denomIndex);
        console.log("depositor %s", vm.addr(pk));
        console.log("denomination %s wei, vault fee %s, bridge fee %s", denom, vaultFee, bridgeFee);
        console.log("sending %s wei to the desk %s", total, address(d));

        vm.startBroadcast(pk);
        leaf = d.depositETHWithFee{value: total}(denomIndex, c, bytes32(0));
        vm.stopBroadcast();

        require(v.commitmentUsed(c), "the vault did not record the commitment");
        console.log("LEAF_INDEX=%s", uint256(leaf));
        console.logBytes32(c);
    }
}
