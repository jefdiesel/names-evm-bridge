// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {UpdateGatedVerifier} from "../src/UpdateGatedVerifier.sol";
import {DemoUSDC} from "./DemoUSDC.sol";

/// The selector-bearing part of RISC Zero's `RiscZeroGroth16Verifier` (risc0-ethereum v3.1.0).
interface IRiscZeroGroth16Selector {
    function SELECTOR() external view returns (bytes4);
}

/// v4 on Sepolia (chain 11155111): a vault whose proofs RISC Zero's Groth16 verifier checks on
/// chain (`public-demo/V4-PLAN.md`).
///
/// Deploys an `UpdateGatedVerifier` in front of RISC Zero's BARE `RiscZeroGroth16Verifier` 3.1.0
/// (selector 0xe71e6620; not the router or an emergency stop, which RISC Zero can switch off),
/// then a new, unchanged `BridgeVault` that uses the gate. Release proofs (G3) are permissionless
/// and checked by the Groth16 verifier. Update proofs (G2-lite, TrustedTip) are checked by it too,
/// but count only when SUBMITTER sends them through the gate: the TrustedTip image proves the
/// bridge's logic over the LEZ blocks it is given, not that they are the real chain's, so which
/// blocks count is the submitter's word until a LEZ finality proof exists
/// (`src/UpdateGatedVerifier.sol`).
///
/// The 3.0.0 verifier (0x2a09…a84D, selector 0x73c457ba, used by v1/v2) is refused: RISC Zero
/// advisory GHSA-g6qg-9j8p-7j87 (2026-11-11) lets anyone with one valid receipt have any claim
/// accepted under it, and RISC Zero will emergency-stop it. Receipts for 3.1.0 need provers on
/// risc0-zkvm 3.1.0.
///
/// Env:
///   SEPOLIA_PK        deployer key (never printed); must be DEPLOYER below
///   RISC0_VERIFIER    optional: the bare Groth16 verifier (default 0x8c91…aB3b, 3.1.0); its
///                     SELECTOR() must be 0xe71e6620
///   SUBMITTER         the only account that may submit updates: a fresh operator key, not the
///                     deployer (admin/freezer), not zero
///   RELEASE_IMAGE_ID  G3 release guest image id (bytes32), an INPUT: no default
///   UPDATE_IMAGE_ID   G2-lite update guest image id (bytes32), an INPUT: no default
///   PROGRAM_ID        the v4 LEZ bridge program id (`lez-bridge program-id --bridge <BP4>`)
///   LEZ_CHAIN_ID      the LEZ chain's Bedrock channel id (`lez-bridge chain-id`), not zero
///   ZONE_ID           the bridge's root-source id (`lez-bridge source-id`), not zero
///   INITIAL_G2_STATE  `outbound_host genesis --block <tip after the v4 bridge deploy, before its
///                     Initialize> --bridge <BP4>` (G2_STATE=), not zero
///   EXPECTED_GATE, EXPECTED_VAULT, EXPECTED_CFG_ID   optional: what the simulation predicted
///   REUSE_USDC        optional: an existing DemoUSDC minted by DEPLOYER
///
///   forge script script/SepoliaV4.s.sol --rpc-url <sepolia>                  # simulate
///   ... --broadcast --slow --gas-estimate-multiplier 1000 --with-gas-price <cap> --priority-gas-price <tip>
///   ... --sig "configure()"  with VAULT, USDC set (registry, menus, caps, delay)
contract SepoliaV4 is Script {
    address constant DEPLOYER = 0x37a46E7334Dd061E0C7E2298247EE9278d0d321F;
    uint256 constant SEPOLIA = 11155111;
    /// RISC Zero's bare RiscZeroGroth16Verifier 3.1.0 on Sepolia (risc0-ethereum
    /// contracts/deployment.toml; SELECTOR, control root and BN254 id read on chain 2026-11-15).
    address constant RISC0_GROTH16_V3_1 = 0x8c915b5071206C6E328BC18529586c513408aB3b;
    bytes4 constant SELECTOR_V3_1 = 0xe71e6620;
    bytes4 constant SELECTOR_V3_0 = 0x73c457ba; // vulnerable (GHSA-g6qg-9j8p-7j87)
    bytes32 constant PLACEHOLDER_UPDATE_IMAGE_ID = keccak256("RFP022/demo/no-update-image");
    /// One hour, not v3's 10 minutes: the delay is the freezer's window against an update over a
    /// forged LEZ tip (a stolen SUBMITTER key), and every release already waits hours for its
    /// Groth16 proof, so the hour costs users little. Real deployments: a day or more.
    uint64 constant MIN_RELEASE_DELAY = 1 hours;
    uint64 constant RELEASE_DELAY = 1 hours;
    uint64 constant CAP_WINDOW = 1 days;
    bytes32 constant GATE_KIND_HASH = keccak256(
        "update-gated: RISC Zero Groth16 verifies every proof; updates only from SUBMITTER (LEZ finality is its word)"
    );

    string constant BANNER =
        "v4: proofs checked on chain by RISC Zero's Groth16 verifier 3.1.0; updates only from SUBMITTER (LEZ finality is its word). TESTNET.";

    struct Params {
        address risc0Verifier;
        address submitter;
        bytes32 releaseImageId;
        bytes32 updateImageId;
        bytes32 programId;
        address reuseUsdc;
        bytes32 lezChainId;
        bytes32 zoneId;
        bytes32 initialG2State;
    }

    /// Virtual only so the test harness can use a public test key (test/SepoliaV4Script.t.sol).
    function _deployer() internal view virtual returns (address) {
        return DEPLOYER;
    }

    function _pk() internal view returns (uint256 pk) {
        pk = vm.envUint("SEPOLIA_PK");
        require(vm.addr(pk) == _deployer(), "SEPOLIA_PK is not the expected deployer");
    }

    function params() public view returns (Params memory p) {
        p.risc0Verifier = vm.envOr("RISC0_VERIFIER", RISC0_GROTH16_V3_1);
        p.submitter = vm.envAddress("SUBMITTER");
        p.releaseImageId = vm.envBytes32("RELEASE_IMAGE_ID");
        p.updateImageId = vm.envBytes32("UPDATE_IMAGE_ID");
        p.programId = vm.envBytes32("PROGRAM_ID");
        p.reuseUsdc = vm.envOr("REUSE_USDC", address(0));
        p.lezChainId = vm.envBytes32("LEZ_CHAIN_ID");
        p.zoneId = vm.envBytes32("ZONE_ID");
        p.initialG2State = vm.envBytes32("INITIAL_G2_STATE");
    }

    /// Refuses everything that would strand the vault or weaken it.
    function check(Params memory p) public view {
        require(p.risc0Verifier.code.length > 0, "RISC0_VERIFIER has no code");
        bytes4 sel = IRiscZeroGroth16Selector(p.risc0Verifier).SELECTOR();
        require(sel != SELECTOR_V3_0, "RISC0_VERIFIER is the 3.0.0 verifier (GHSA-g6qg-9j8p-7j87)");
        require(sel == SELECTOR_V3_1, "RISC0_VERIFIER selector is not 0xe71e6620 (3.1.0)");
        require(p.submitter != address(0), "SUBMITTER is zero");
        // The deployer is admin and freezer: the freeze is the brake on a bad update, so it must
        // not sit with the key that submits updates.
        require(p.submitter != _deployer(), "SUBMITTER must not be the deployer/admin/freezer");
        require(p.releaseImageId != bytes32(0), "RELEASE_IMAGE_ID is zero");
        require(p.updateImageId != bytes32(0), "UPDATE_IMAGE_ID is zero");
        require(p.updateImageId != PLACEHOLDER_UPDATE_IMAGE_ID, "UPDATE_IMAGE_ID is the old placeholder");
        require(p.releaseImageId != PLACEHOLDER_UPDATE_IMAGE_ID, "RELEASE_IMAGE_ID is the old placeholder");
        require(p.releaseImageId != p.updateImageId, "RELEASE_IMAGE_ID == UPDATE_IMAGE_ID");
        require(p.programId != bytes32(0), "PROGRAM_ID is zero");
        require(p.lezChainId != bytes32(0), "LEZ_CHAIN_ID is zero");
        require(p.zoneId != bytes32(0), "ZONE_ID is zero");
        // The G2-lite guest's prevState is SHA256(tag | state), never zero: zero here means the
        // vault can never accept an update (the v3 deployment's mistake).
        require(p.initialG2State != bytes32(0), "INITIAL_G2_STATE is zero");
        if (p.reuseUsdc != address(0)) {
            require(p.reuseUsdc.code.length > 0 && DemoUSDC(p.reuseUsdc).minter() == _deployer(), "REUSE_USDC");
        }
    }

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        console.log(BANNER);
        uint256 pk = _pk();
        Params memory p = params();
        (UpdateGatedVerifier g, BridgeVault v, DemoUSDC usdc) = deploy(pk, p);

        address wantGate = vm.envOr("EXPECTED_GATE", address(0));
        if (wantGate != address(0)) require(address(g) == wantGate, "gate address differs from EXPECTED_GATE");
        address wantVault = vm.envOr("EXPECTED_VAULT", address(0));
        if (wantVault != address(0)) require(address(v) == wantVault, "vault address differs from EXPECTED_VAULT");
        bytes32 wantCfg = vm.envOr("EXPECTED_CFG_ID", bytes32(0));
        if (wantCfg != bytes32(0)) require(v.cfgId() == wantCfg, "cfgId differs from EXPECTED_CFG_ID");

        console.log("RISC0_VERIFIER=%s (bare RiscZeroGroth16Verifier, selector 0xe71e6620)", address(g.VERIFIER()));
        console.log("GATE=%s (UpdateGatedVerifier; the vault's verifier)", address(g));
        console.log("SUBMITTER=%s", g.SUBMITTER());
        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("PROGRAM_ID=%s", vm.toString(v.programId()));
        console.log("UPDATE_IMAGE_ID=%s", vm.toString(v.updateImageId()));
        console.log("RELEASE_IMAGE_ID=%s", vm.toString(v.releaseImageId()));
        console.log("G2_STATE=%s (the first update's prevState)", vm.toString(v.g2State()));
        console.log("MIN_RELEASE_DELAY=%s s", uint256(MIN_RELEASE_DELAY));
    }

    /// Creates the gate, the vault and (unless reused) the DemoUSDC from `pk`, and checks them.
    function deploy(uint256 pk, Params memory p)
        public
        returns (UpdateGatedVerifier g, BridgeVault v, DemoUSDC usdc)
    {
        require(block.chainid == SEPOLIA, "not Sepolia");
        check(p);
        address me = _deployer();
        vm.startBroadcast(pk);
        usdc = p.reuseUsdc != address(0) ? DemoUSDC(p.reuseUsdc) : new DemoUSDC();
        g = new UpdateGatedVerifier(IRiscZeroVerifier(p.risc0Verifier), p.updateImageId, p.submitter);
        v = new BridgeVault(
            IRiscZeroVerifier(address(g)),
            p.updateImageId,
            p.releaseImageId,
            p.lezChainId,
            p.zoneId,
            p.programId,
            p.initialG2State,
            MIN_RELEASE_DELAY,
            me, // admin
            me, // freezer
            me // fee recipient
        );
        vm.stopBroadcast();

        require(address(v.verifier()) == address(g), "vault verifier is not the gate");
        require(address(g.VERIFIER()) == p.risc0Verifier, "gate VERIFIER");
        require(g.UPDATE_IMAGE_ID() == v.updateImageId(), "gate and vault update image ids differ");
        require(g.SUBMITTER() == p.submitter, "gate SUBMITTER");
        require(keccak256(bytes(g.KIND())) == GATE_KIND_HASH, "gate KIND");
        require(v.updateImageId() == p.updateImageId && v.releaseImageId() == p.releaseImageId, "image ids");
        require(v.minReleaseDelay() == MIN_RELEASE_DELAY, "minReleaseDelay");
        require(v.g2State() == p.initialG2State, "g2State");
        require(v.cfgId() == v.computeCfgId(SEPOLIA, address(v), p.lezChainId, p.zoneId, p.programId), "cfgId");
    }

    /// Registry, relay-fee menus, caps and the 1-hour delay. Values as v3 except the delay.
    /// Re-runnable: tokens already registered are skipped (the setters set the same values again).
    function configure() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        console.log(BANNER);
        configureVault(_pk(), BridgeVault(vm.envAddress("VAULT")), vm.envAddress("USDC"));
    }

    function configureVault(uint256 pk, BridgeVault v, address usdc) public {
        require(v.admin() == _deployer(), "not our vault");
        require(
            keccak256(bytes(UpdateGatedVerifier(address(v.verifier())).KIND())) == GATE_KIND_HASH,
            "not a v4 (update-gated) vault"
        );

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
        if (!uReg) v.registerToken(usdc, 6, ud, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        if (!eReg) v.registerToken(address(0), 18, ed, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        // Relay-fee menus: 0.5% and 1% of each denomination (bound is 2%; fee 0 always allowed).
        v.setRelayFeeMenu(usdc, _menu(ud));
        v.setRelayFeeMenu(address(0), _menu(ed));
        // Caps over a 1-day window, the v1-v3 demo values: they bound what an update over a
        // forged LEZ tip (a stolen SUBMITTER key) could release per day.
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
}
