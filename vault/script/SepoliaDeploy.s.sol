// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {DemoUSDC} from "./DemoUSDC.sol";

interface IVersioned {
    function VERSION() external view returns (string memory);
    function SELECTOR() external view returns (bytes4);
}

/// DEMO deployment on Sepolia (chain 11155111): deposits only.
///
/// `updateImageId` is a PLACEHOLDER, keccak256("RFP022/demo/no-update-image"): the G2 outbound
/// state proof is not built, so no `update` can ever verify, no exit can ever be recorded and
/// nothing can ever be released from this vault. Exits need a new vault version with the real
/// G2 image. `releaseImageId` is the G3 guest from a local, non-reproducible build on the test machine.
/// The admin, freezer and fee recipient are a throwaway test key.
///
/// FIRST STEP, before any run that sends: `node scripts/check-versions.mjs --onchain --deployment v2`
/// from the repository root passes (VERSIONS.toml; AGENTS.md "Version alignment").
/// This script pins the 3.0.0 verifier (advisory GHSA-mm6c-35cm-g4x2): it is v2's record; never use
/// it for a new vault.
///
/// Env:
///   SEPOLIA_PK       deployer key (never printed); must be DEPLOYER below
///   PROGRAM_ID       the LEZ bridge program id as bytes32 (`lez-bridge program-id`)
///   EXPECTED_VAULT   optional: the vault address the simulation predicted
///   EXPECTED_CFG_ID  optional: `lez-bridge cfg-id` for that address; the run aborts on mismatch
///   REUSE_USDC       optional: an existing DemoUSDC minted by DEPLOYER, used instead of a new one
///
///   forge script script/SepoliaDeploy.s.sol --rpc-url <sepolia>                 # simulate
///   ... --broadcast --slow --with-gas-price <cap> --priority-gas-price <tip>     # send
///   ... --sig "configure()"  with VAULT, USDC set (registry, menus, caps)
///   ... --sig "deposits()"   with C_V100A C_V100B C_V1000 C_P100 C_VETH, VAULT, USDC set
///
/// GAS: forge's local EVM under-prices Sepolia today (cause not confirmed; on 11-12 the vault
/// create took ~36.1M on chain vs ~5.1M locally; DemoUSDC ~4.1M vs 0.62M). Take limits from
/// eth_estimateGas, or pass a large --gas-estimate-multiplier (1000 was used), and cap the fee.
contract SepoliaDeploy is Script {
    address constant DEPLOYER = 0x37a46E7334Dd061E0C7E2298247EE9278d0d321F;
    uint256 constant SEPOLIA = 11155111;
    /// risc0-ethereum RiscZeroGroth16Verifier v3.0.0 (bare, immutable), selector 0x73c457ba.
    address constant VERIFIER = 0x2a098988600d87650Fb061FfAff08B97149Fa84D;
    /// G3 release guest (methods/release_guest), RELEASE_GUEST_ID from the test machine's
    /// target-attest build of 2026-11-12 14:10 (release_guest.bin sha256 4238085e...fe05).
    bytes32 constant RELEASE_IMAGE_ID = 0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4;
    /// Placeholder: no G2 image exists. NO EXITS ARE POSSIBLE on this vault.
    bytes32 constant UPDATE_IMAGE_ID = keccak256("RFP022/demo/no-update-image");
    uint64 constant MIN_RELEASE_DELAY = 1 days;
    uint64 constant RELEASE_DELAY = 1 days;
    uint64 constant CAP_WINDOW = 1 days;
    /// PLACEHOLDER admin timelock delay, not a decided value (historical v2 script, kept compilable): ask the owner before deploy.
    uint64 constant ADMIN_DELAY = 2 days;

    function _pk() internal view returns (uint256 pk) {
        pk = vm.envUint("SEPOLIA_PK");
        require(vm.addr(pk) == DEPLOYER, "SEPOLIA_PK is not the expected deployer");
    }

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        uint256 pk = _pk();
        bytes32 programId = vm.envBytes32("PROGRAM_ID");
        require(programId != bytes32(0), "PROGRAM_ID is zero");
        require(VERIFIER.code.length > 0, "no verifier code");
        require(keccak256(bytes(IVersioned(VERIFIER).VERSION())) == keccak256("3.0.0"), "verifier VERSION");
        require(IVersioned(VERIFIER).SELECTOR() == bytes4(0x73c457ba), "verifier SELECTOR");

        // [v2, 2026-11-13] REUSE_USDC (optional): reuse an existing DemoUSDC (one this deployer
        // created, so it can still mint) instead of deploying another.
        address reuse = vm.envOr("REUSE_USDC", address(0));
        if (reuse != address(0)) require(reuse.code.length > 0 && DemoUSDC(reuse).minter() == DEPLOYER, "REUSE_USDC");

        vm.startBroadcast(pk);
        DemoUSDC usdc = reuse != address(0) ? DemoUSDC(reuse) : new DemoUSDC();
        BridgeVault v = new BridgeVault(
            IRiscZeroVerifier(VERIFIER),
            UPDATE_IMAGE_ID,
            RELEASE_IMAGE_ID,
            bytes32(0), // lezChainId: the local LEZ 0.4 test chain has none
            bytes32(0), // zoneId: likewise
            programId,
            bytes32(0), // initial G2 state
            MIN_RELEASE_DELAY,
            DEPLOYER, // admin
            DEPLOYER, // freezer
            DEPLOYER, // fee recipient
            ADMIN_DELAY // placeholder: ask the owner before deploy
        );
        vm.stopBroadcast();

        require(v.cfgId() == v.computeCfgId(SEPOLIA, address(v), bytes32(0), bytes32(0), programId), "cfgId");
        address wantVault = vm.envOr("EXPECTED_VAULT", address(0));
        if (wantVault != address(0)) require(address(v) == wantVault, "vault address differs from EXPECTED_VAULT");
        bytes32 wantCfg = vm.envOr("EXPECTED_CFG_ID", bytes32(0));
        if (wantCfg != bytes32(0)) require(v.cfgId() == wantCfg, "cfgId differs from EXPECTED_CFG_ID");

        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("UPDATE_IMAGE_ID=%s (placeholder: no exits possible)", vm.toString(UPDATE_IMAGE_ID));
        console.log("RELEASE_IMAGE_ID=%s", vm.toString(RELEASE_IMAGE_ID));
        console.log("DEPOSIT_ROOT=%s", vm.toString(v.depositRoot()));
    }

    /// Registry, relay-fee menus and caps. Re-runnable: tokens already registered are skipped
    /// (the setters just set the same values again).
    function configure() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        uint256 pk = _pk();
        BridgeVault v = BridgeVault(vm.envAddress("VAULT"));
        address usdc = vm.envAddress("USDC");
        require(v.admin() == DEPLOYER, "not our vault");

        // USDC: 100, 1,000, 10,000 (6 decimals); ETH: 0.01, 0.1, 1 (small: test ETH is scarce).
        uint256[] memory ud = new uint256[](3);
        ud[0] = 100e6;
        ud[1] = 1_000e6;
        ud[2] = 10_000e6;
        uint256[] memory ed = new uint256[](3);
        ed[0] = 0.01 ether;
        ed[1] = 0.1 ether;
        ed[2] = 1 ether;
        (bool uReg,,,,,,,,) = v.tokenConfig(usdc);
        (bool eReg,,,,,,,,) = v.tokenConfig(address(0));

        vm.startBroadcast(pk);
        // No protocol fees (deposit and withdraw fees 0).
        if (!uReg) v.registerToken(usdc, 6, ud, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        if (!eReg) v.registerToken(address(0), 18, ed, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        // Relay-fee menus: 0.5% and 1% of each denomination (bound is 2%; fee 0 always allowed).
        v.setRelayFeeMenu(usdc, _menu(ud));
        v.setRelayFeeMenu(address(0), _menu(ed));
        // Caps over a 1-day window. Our own demo values (no recommended defaults are recorded
        // yet; DESIGN S4 is open): each cap is above the largest denomination.
        v.setCaps(usdc, CAP_WINDOW, 50_000e6, 50_000e6);
        v.setCaps(address(0), CAP_WINDOW, 5 ether, 5 ether);
        // Global caps in USD with 6 decimals: USDC weight 1e18, ETH at $3000 = 3e9.
        v.setGlobalCaps(CAP_WINDOW, 100_000e6, 100_000e6);
        v.setCapWeight(usdc, 1e18);
        v.setCapWeight(address(0), 3e9);
        vm.stopBroadcast();
    }

    function _menu(uint256[] memory denoms) internal pure returns (uint256[][] memory m) {
        m = new uint256[][](denoms.length);
        for (uint256 i; i < denoms.length; ++i) {
            m[i] = new uint256[](2);
            m[i][0] = denoms[i] / 200; // 0.5%
            m[i][1] = denoms[i] / 100; // 1%
        }
    }

    /// Demo deposits with commitments computed by `lez-bridge commitment` on the test machine (the
    /// secrets never leave it): 2 x 100 and 1 x 1,000 USDC and 0.01 ETH for the private account
    /// V, 1 x 100 USDC for the public account P.
    function deposits() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        uint256 pk = _pk();
        BridgeVault v = BridgeVault(vm.envAddress("VAULT"));
        DemoUSDC usdc = DemoUSDC(vm.envAddress("USDC"));
        bytes32[5] memory c = [
            vm.envBytes32("C_V100A"),
            vm.envBytes32("C_V100B"),
            vm.envBytes32("C_V1000"),
            vm.envBytes32("C_P100"),
            vm.envBytes32("C_VETH")
        ];
        address[5] memory tok = [address(usdc), address(usdc), address(usdc), address(usdc), address(0)];
        uint32[5] memory di = [uint32(0), 0, 1, 0, 0];
        uint256[5] memory amt = [uint256(100e6), 100e6, 1_000e6, 100e6, 0.01 ether];
        // Re-runnable: deposits whose commitment is already used are skipped.
        uint256 need = 100e6; // 100 left over with allowance, so a repeated-commitment eth_call
        // reaches the CommitmentUsed check instead of failing on balance or allowance
        for (uint256 i; i < 5; ++i) {
            for (uint256 k; k < i; ++k) require(c[k] != c[i], "duplicate commitment");
            if (!v.commitmentUsed(c[i]) && tok[i] != address(0)) need += amt[i];
        }
        uint256 bal = usdc.balanceOf(DEPLOYER);
        vm.startBroadcast(pk);
        if (bal < need) usdc.mint(DEPLOYER, need - bal);
        if (usdc.allowance(DEPLOYER, address(v)) < need) usdc.approve(address(v), need);
        for (uint256 i; i < 5; ++i) {
            if (v.commitmentUsed(c[i])) continue;
            uint64 leaf = tok[i] == address(0)
                ? v.depositETH{value: amt[i]}(di[i], c[i])
                : v.deposit(tok[i], di[i], c[i]);
            console.log("DEPOSIT %s leaf %s", i, leaf);
        }
        vm.stopBroadcast();
        console.log("DEPOSIT_ROOT=%s", vm.toString(v.depositRoot()));
        console.log("DEPOSIT_COUNT=%s", v.depositCount());
    }
}
