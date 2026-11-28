// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {DemoAttestedVerifier} from "../src/demo/DemoAttestedVerifier.sol";
import {DemoUSDC} from "./DemoUSDC.sol";

/// DEMO v3 on Sepolia (chain 11155111): the OPERATOR-ATTESTED stage. NOT TRUSTLESS.
///
/// Deploys a `DemoAttestedVerifier` (one immutable attestor key, time-boxed) and a new, unchanged
/// `BridgeVault` that uses it, so withdrawals work end to end on the testnet before on-chain
/// verification of the G2 update proof exists. Every `update` and `release` this vault accepts
/// was authorised by the attestor's signature, not by a zk proof checked on chain: whoever holds
/// that key can take the vault's deposits (within caps, after the delay). See
/// `src/demo/README.md`. Never present this as the trustless design.
///
/// The image ids are still the REAL guest image ids (the attestor signs only for them, and the
/// vault binds them), so the vault runs exactly the logic it will run with real proofs:
///   RELEASE_IMAGE_ID: the G3 release guest (methods/release_guest) the users prove with;
///   UPDATE_IMAGE_ID:  the G2-lite outbound guest the attestor runs (today the trusted-tip build,
///                     `outbound_guest_trusted_tip`: the only build that accepts a batch).
/// The old placeholder `keccak256("RFP022/demo/no-update-image")` is refused.
///
/// Release delay: 10 minutes for both `minReleaseDelay` (immutable) and the tokens' delay. A
/// DEMO setting, so a round trip fits in a session; it gives no human time to react to a bad
/// attestor signature. Real deployments use a day or more.
///
/// FIRST STEP, before any run that sends: `node scripts/check-versions.mjs --onchain --deployment <the deployment it creates>`
/// from the repository root passes (VERSIONS.toml; AGENTS.md "Version alignment").
/// Declare that deployment in VERSIONS.toml first (verifier, image ids and the ELFs they come from,
/// a non-zero genesis G2 state), then record the vault and run the check again before the first
/// deposit.
///
/// Env:
///   SEPOLIA_PK        deployer key (never printed); must be DEPLOYER below
///   ATTESTOR          the attestor address (the operator daemon's key); not the deployer
///   DEMO_LIFETIME     optional: seconds until the verifier stops accepting anything
///                     (default 180 days, at most 365 days, enforced by the verifier)
///   RELEASE_IMAGE_ID  real G3 image id (bytes32)
///   UPDATE_IMAGE_ID   real G2-lite image id (bytes32); not zero, not the placeholder
///   PROGRAM_ID        the LEZ bridge program id as bytes32 (`lez-bridge program-id`)
///   LEZ_CHAIN_ID      the LEZ chain's Bedrock channel id (`lez-bridge chain-id`), not zero: the
///                     bridge's `Initialize` refuses zero and binds it into cfgId (security review
///                     2026-11-14, M1)
///   ZONE_ID           the bridge's root-source id (`lez-bridge source-id`, with the attestation
///                     instance for RFP-021), not zero; the program derives the same value at init
///   INITIAL_G2_STATE  the G2 genesis state commitment the first `update` must start from
///                     (`outbound_host genesis --block <tip before the bridge's Initialize>`), not
///                     zero: no G2-lite journal has prevState zero, so a vault deployed with zero can
///                     never take an update (v3 was, 2026-11-14; found 2026-11-15)
///   EXPECTED_VERIFIER optional: the verifier address the simulation predicted
///   EXPECTED_VAULT    optional: the vault address the simulation predicted
///   EXPECTED_CFG_ID   optional: `lez-bridge cfg-id` for that vault address; aborts on mismatch
///   REUSE_USDC        optional: an existing DemoUSDC minted by DEPLOYER, used instead of a new one
///
///   forge script script/SepoliaDemoV3.s.sol --rpc-url <sepolia>               # simulate
///   ... --broadcast --slow --with-gas-price <cap> --priority-gas-price <tip>   # send
///   ... --sig "configure()"  with VAULT, USDC set (registry, menus, caps, 10-minute delay)
///
/// GAS: Sepolia prices state creation under Glamsterdam (DEPLOYMENTS.md); take gas limits from
/// eth_estimateGas or pass a large --gas-estimate-multiplier, and cap the fee.
contract SepoliaDemoV3 is Script {
    address constant DEPLOYER = 0x37a46E7334Dd061E0C7E2298247EE9278d0d321F;
    uint256 constant SEPOLIA = 11155111;
    bytes32 constant PLACEHOLDER_UPDATE_IMAGE_ID = keccak256("RFP022/demo/no-update-image");
    /// DEMO setting (real deployments: a day or more).
    uint64 constant MIN_RELEASE_DELAY = 10 minutes;
    uint64 constant RELEASE_DELAY = 10 minutes;
    uint64 constant CAP_WINDOW = 1 days;
    /// PLACEHOLDER admin timelock delay, not a decided value: adminDelay is immutable, ask the owner before deploy.
    uint64 constant ADMIN_DELAY = 2 days;
    uint256 constant DEFAULT_LIFETIME = 180 days;
    bytes32 constant DEMO_KIND_HASH = keccak256("demo-attested: an operator checked the proof off chain; NOT trustless");

    string constant BANNER =
        "!!! OPERATOR-ATTESTED DEMO STAGE (v3): an operator key, NOT an on-chain zk proof, authorises every update and release. NOT TRUSTLESS. TESTNET ONLY. !!!";

    struct Params {
        address attestor;
        uint256 lifetime;
        bytes32 releaseImageId;
        bytes32 updateImageId;
        bytes32 programId;
        address reuseUsdc;
        bytes32 lezChainId;
        bytes32 zoneId;
        bytes32 initialG2State;
    }

    /// The expected deployer (= admin, freezer, fee recipient). Virtual only so the test harness
    /// (test/SepoliaDemoV3Script.t.sol) can run this script's logic with a public test key.
    function _deployer() internal view virtual returns (address) {
        return DEPLOYER;
    }

    function _pk() internal view returns (uint256 pk) {
        pk = vm.envUint("SEPOLIA_PK");
        require(vm.addr(pk) == _deployer(), "SEPOLIA_PK is not the expected deployer");
    }

    function params() public view returns (Params memory p) {
        p.attestor = vm.envAddress("ATTESTOR");
        p.lifetime = vm.envOr("DEMO_LIFETIME", DEFAULT_LIFETIME);
        p.releaseImageId = vm.envBytes32("RELEASE_IMAGE_ID");
        p.updateImageId = vm.envBytes32("UPDATE_IMAGE_ID");
        p.programId = vm.envBytes32("PROGRAM_ID");
        p.reuseUsdc = vm.envOr("REUSE_USDC", address(0));
        p.lezChainId = vm.envBytes32("LEZ_CHAIN_ID");
        p.zoneId = vm.envBytes32("ZONE_ID");
        p.initialG2State = vm.envBytes32("INITIAL_G2_STATE");
    }

    /// Refuses everything that would make the demo vault useless or misleading.
    function check(Params memory p) public view {
        require(p.attestor != address(0), "ATTESTOR is zero");
        // The deployer is admin and freezer: the freeze is the only brake on a bad attestor
        // signature, so it must not sit with the same key.
        require(p.attestor != _deployer(), "ATTESTOR must not be the deployer/admin/freezer");
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
        (DemoAttestedVerifier dv, BridgeVault v, DemoUSDC usdc) = deploy(pk, p);

        address wantVer = vm.envOr("EXPECTED_VERIFIER", address(0));
        if (wantVer != address(0)) require(address(dv) == wantVer, "verifier address differs from EXPECTED_VERIFIER");
        address wantVault = vm.envOr("EXPECTED_VAULT", address(0));
        if (wantVault != address(0)) require(address(v) == wantVault, "vault address differs from EXPECTED_VAULT");
        bytes32 wantCfg = vm.envOr("EXPECTED_CFG_ID", bytes32(0));
        if (wantCfg != bytes32(0)) require(v.cfgId() == wantCfg, "cfgId differs from EXPECTED_CFG_ID");

        console.log("VERIFIER=%s (DemoAttestedVerifier: operator-attested, NOT a zk verifier)", address(dv));
        console.log("ATTESTOR=%s", dv.ATTESTOR());
        console.log("EXPIRES_AT=%s (unix; nothing verifies from then on)", dv.EXPIRES_AT());
        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("UPDATE_IMAGE_ID=%s", vm.toString(p.updateImageId));
        console.log("RELEASE_IMAGE_ID=%s", vm.toString(p.releaseImageId));
        console.log("G2_STATE=%s (the first update's prevState)", vm.toString(v.g2State()));
        console.log("MIN_RELEASE_DELAY=%s s (demo setting)", uint256(MIN_RELEASE_DELAY));
        console.log(BANNER);
    }

    /// Creates the verifier, the vault and (unless reused) the DemoUSDC from `pk`, and checks
    /// them. Public so the test harness can call it without the env.
    function deploy(uint256 pk, Params memory p)
        public
        returns (DemoAttestedVerifier dv, BridgeVault v, DemoUSDC usdc)
    {
        require(block.chainid == SEPOLIA, "not Sepolia");
        check(p);
        address me = _deployer();
        vm.startBroadcast(pk);
        usdc = p.reuseUsdc != address(0) ? DemoUSDC(p.reuseUsdc) : new DemoUSDC();
        dv = new DemoAttestedVerifier(p.attestor, p.lifetime);
        v = new BridgeVault(
            IRiscZeroVerifier(address(dv)),
            p.updateImageId,
            p.releaseImageId,
            p.lezChainId, // the LEZ chain's channel id (`lez-bridge chain-id`)
            p.zoneId, // the bridge's root-source id (`lez-bridge source-id`)
            p.programId,
            p.initialG2State, // the G2 genesis state (`outbound_host genesis`)
            MIN_RELEASE_DELAY,
            me, // admin
            me, // freezer
            me, // fee recipient
            ADMIN_DELAY // placeholder: ask the owner before deploy
        );
        vm.stopBroadcast();

        require(address(v.verifier()) == address(dv), "vault verifier");
        require(keccak256(bytes(dv.KIND())) == DEMO_KIND_HASH, "verifier KIND");
        require(dv.ATTESTOR() == p.attestor, "ATTESTOR");
        require(dv.EXPIRES_AT() == block.timestamp + p.lifetime, "EXPIRES_AT");
        require(v.updateImageId() == p.updateImageId && v.releaseImageId() == p.releaseImageId, "image ids");
        require(v.minReleaseDelay() == MIN_RELEASE_DELAY, "minReleaseDelay");
        require(v.adminDelay() == ADMIN_DELAY, "adminDelay");
        require(v.g2State() == p.initialG2State, "g2State");
        require(v.cfgId() == v.computeCfgId(SEPOLIA, address(v), p.lezChainId, p.zoneId, p.programId), "cfgId");
    }

    /// Registry, relay-fee menus, caps and the 10-minute demo delay, as in SepoliaDeploy.s.sol.
    /// Re-runnable: tokens already registered are skipped (the setters set the same values again).
    function configure() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        console.log(BANNER);
        configureVault(_pk(), BridgeVault(vm.envAddress("VAULT")), vm.envAddress("USDC"));
    }

    function configureVault(uint256 pk, BridgeVault v, address usdc) public {
        require(v.admin() == _deployer(), "not our vault");
        require(
            keccak256(bytes(DemoAttestedVerifier(address(v.verifier())).KIND())) == DEMO_KIND_HASH,
            "not a demo-attested vault"
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
        // No protocol fees (deposit and withdraw fees 0). Release delay: the 10-minute demo value.
        if (!uReg) v.registerToken(usdc, 6, ud, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        if (!eReg) v.registerToken(address(0), 18, ed, new uint256[](3), new uint256[](3), RELEASE_DELAY);
        // Relay-fee menus: 0.5% and 1% of each denomination (bound is 2%; fee 0 always allowed).
        v.setRelayFeeMenu(usdc, _menu(ud));
        v.setRelayFeeMenu(address(0), _menu(ed));
        // Caps over a 1-day window, the v1/v2 demo values. With an attestor key as the only
        // check, the release caps are what bound a bad signature per day (close to 2x a cap
        // can pass within one window: the two-bucket estimate, DESIGN 6.1).
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
