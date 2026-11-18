// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {SeqSigGate, ISeqSigVault} from "../src/SeqSigGate.sol";
import {DemoUSDC} from "./DemoUSDC.sol";

interface IRiscZeroGroth16SelectorV7 {
    function SELECTOR() external view returns (bytes4);
}

/// v7 on Sepolia (chain 11155111): v6's bridge (deposit roots from the RFP-021 light client) with
/// withdrawals on a THRESHOLD of signer keys instead of one submitter key (branch `v7-seqsig`,
/// `public-demo/prove/v7-seqsig-20261118/README.md`).
///
/// Deploys a `SeqSigGate` in front of RISC Zero's BARE `RiscZeroGroth16Verifier` 3.1.0 (selector
/// 0xe71e6620), bound to the vault deployed right after it (the deployer's next CREATE address),
/// then an unchanged `BridgeVault` whose verifier is the gate, and checks the vault landed at that
/// address with the gate's CFG_ID. No submitter, no gate admin: anyone may submit an update the
/// signers signed. The vault keeps v5/v6's admin = freezer = fee recipient = the deployer.
///
/// Env (every value a stated input; nothing defaults except the verifier and REUSE_USDC):
///   SEPOLIA_PK          deployer key (never printed); must be DEPLOYER below
///   RISC0_VERIFIER      optional: default 0x8c91…aB3b (3.1.0); its SELECTOR() must be 0xe71e6620
///   UPDATE_IMAGE_ID     the seq-sig G2 guest (`outbound_guest_seqsig`) image id: the vault's
///                       updateImageId and the gate's UPDATE_IMAGE_ID
///   EVIDENCE_IMAGE_ID   the contest evidence guest (`seqsig_evidence_guest`) image id
///   RELEASE_IMAGE_ID    the G3 release guest (v5's)
///   PROGRAM_ID, LEZ_CHAIN_ID, ZONE_ID, INITIAL_G2_STATE   as SepoliaV4.s.sol (the v7 bridge header,
///                       the chain, the light-client instance's source id, the genesis G2 state)
///   SIGNER_KEYS         the signer set's Ed25519 public keys, comma-separated, STRICTLY ASCENDING
///                       (`seqsig set` prints them sorted); a key's index in this list is its bit
///                       in every signer bitmap
///   THRESHOLD           T, with n/2 < T <= n - 1 (T = n only with TEST_ONLY_SHORT_WINDOW)
///   CONTEST_WINDOW      seconds (>= the gate's MIN_CONTEST_WINDOW, 6 h, unless the flag below)
///   TEST_ONLY_SHORT_WINDOW  REQUIRED, no default: true or false, an immutable visible on chain.
///                       A window under 6 h is accepted by the gate only with true.
///   MIN_RELEASE_DELAY_SECONDS, RELEASE_DELAY_SECONDS   REQUIRED (v5/v6: 0 / 0)
///   REUSE_USDC          optional: the existing DemoUSDC minted by DEPLOYER (v6 reused v5's)
///   EXPECTED_GATE, EXPECTED_VAULT, EXPECTED_CFG_ID, EXPECTED_SET_HASH   optional pins: the
///                       broadcast refuses unless the deployment lands exactly there
///
///   forge script script/SepoliaV7.s.sol --rpc-url <sepolia>                      # simulate
///   ... --broadcast --slow --gas-estimate-multiplier 1000 --with-gas-price <cap> --priority-gas-price <tip>
///   ... --sig "configure()" with VAULT, USDC, RELEASE_DELAY_SECONDS (registry, menus, caps, delay)
contract SepoliaV7 is Script {
    address constant DEPLOYER = 0x37a46E7334Dd061E0C7E2298247EE9278d0d321F;
    uint256 constant SEPOLIA = 11155111;
    address constant RISC0_GROTH16_V3_1 = 0x8c915b5071206C6E328BC18529586c513408aB3b;
    bytes4 constant SELECTOR_V3_1 = 0xe71e6620;
    bytes4 constant SELECTOR_V3_0 = 0x73c457ba; // vulnerable (GHSA-g6qg-9j8p-7j87)
    uint64 constant MAX_DELAY = 30 days;
    uint64 constant CAP_WINDOW = 1 days;
    bytes32 constant SEQSIG_KIND_HASH = keccak256(
        "seq-sig: RISC Zero verifies every proof; an update needs THRESHOLD of SIGNERS keys over its LEZ end block (in the proof), anyone submits; contest window: equivocators ejected, two quorums halt"
    );

    string constant BANNER =
        "v7: updates count when THRESHOLD of the signer keys signed the LEZ block they end at (SeqSigGate, anyone submits, contest window); proofs checked by RISC Zero's Groth16 verifier 3.1.0. TESTNET.";

    struct Params {
        address risc0Verifier;
        bytes32 updateImageId;
        bytes32 evidenceImageId;
        bytes32 releaseImageId;
        bytes32 programId;
        bytes32 lezChainId;
        bytes32 zoneId;
        bytes32 initialG2State;
        bytes32[] signerKeys;
        uint256 threshold;
        uint64 contestWindow;
        bool testOnlyShortWindow;
        address reuseUsdc;
        uint64 minReleaseDelay;
        uint64 releaseDelay;
    }

    /// Virtual only so the test harness can use a public test key (test/SepoliaV7Script.t.sol).
    function _deployer() internal view virtual returns (address) {
        return DEPLOYER;
    }

    function _pk() internal view returns (uint256 pk) {
        pk = vm.envUint("SEPOLIA_PK");
        require(vm.addr(pk) == _deployer(), "SEPOLIA_PK is not the expected deployer");
    }

    function _delayEnv(string memory name) internal view returns (uint64) {
        uint256 s = vm.envUint(name);
        require(s <= MAX_DELAY, string.concat(name, " above 30 days (seconds, not milliseconds?)"));
        return uint64(s);
    }

    function params() public view returns (Params memory p) {
        p.risc0Verifier = vm.envOr("RISC0_VERIFIER", RISC0_GROTH16_V3_1);
        p.updateImageId = vm.envBytes32("UPDATE_IMAGE_ID");
        p.evidenceImageId = vm.envBytes32("EVIDENCE_IMAGE_ID");
        p.releaseImageId = vm.envBytes32("RELEASE_IMAGE_ID");
        p.programId = vm.envBytes32("PROGRAM_ID");
        p.lezChainId = vm.envBytes32("LEZ_CHAIN_ID");
        p.zoneId = vm.envBytes32("ZONE_ID");
        p.initialG2State = vm.envBytes32("INITIAL_G2_STATE");
        p.signerKeys = vm.envBytes32("SIGNER_KEYS", ",");
        p.threshold = vm.envUint("THRESHOLD");
        uint256 w = vm.envUint("CONTEST_WINDOW");
        require(w <= type(uint64).max, "CONTEST_WINDOW");
        p.contestWindow = uint64(w);
        p.testOnlyShortWindow = vm.envBool("TEST_ONLY_SHORT_WINDOW"); // required: no envOr
        p.reuseUsdc = vm.envOr("REUSE_USDC", address(0));
        p.minReleaseDelay = _delayEnv("MIN_RELEASE_DELAY_SECONDS");
        p.releaseDelay = _delayEnv("RELEASE_DELAY_SECONDS");
    }

    function _print(Params memory p) internal pure {
        console.log("==================================================================");
        console.log("SIGNERS n=%s THRESHOLD=%s CONTEST_WINDOW=%s s", p.signerKeys.length, p.threshold, uint256(p.contestWindow));
        console.log("TEST_ONLY_SHORT_WINDOW=%s (immutable, readable on chain)", p.testOnlyShortWindow ? "true" : "false");
        console.log("RELEASE DELAY: MIN_RELEASE_DELAY_SECONDS=%s RELEASE_DELAY_SECONDS=%s", uint256(p.minReleaseDelay), uint256(p.releaseDelay));
        console.log("==================================================================");
    }

    /// Refuses what would strand or weaken the vault; the gate's constructor re-checks the set,
    /// the threshold and the window itself.
    function check(Params memory p) public view {
        require(p.risc0Verifier.code.length > 0, "RISC0_VERIFIER has no code");
        bytes4 sel = IRiscZeroGroth16SelectorV7(p.risc0Verifier).SELECTOR();
        require(sel != SELECTOR_V3_0, "RISC0_VERIFIER is the 3.0.0 verifier (GHSA-g6qg-9j8p-7j87)");
        require(sel == SELECTOR_V3_1, "RISC0_VERIFIER selector is not 0xe71e6620 (3.1.0)");
        require(p.updateImageId != bytes32(0) && p.evidenceImageId != bytes32(0) && p.releaseImageId != bytes32(0), "an image id is zero");
        require(
            p.updateImageId != p.evidenceImageId && p.updateImageId != p.releaseImageId && p.evidenceImageId != p.releaseImageId,
            "image ids must differ"
        );
        require(p.programId != bytes32(0) && p.lezChainId != bytes32(0) && p.zoneId != bytes32(0), "PROGRAM_ID/LEZ_CHAIN_ID/ZONE_ID zero");
        require(p.initialG2State != bytes32(0), "INITIAL_G2_STATE is zero");
        uint256 n = p.signerKeys.length;
        require(n >= 2, "SIGNER_KEYS: at least 2 keys");
        for (uint256 i = 1; i < n; ++i) {
            require(uint256(p.signerKeys[i - 1]) < uint256(p.signerKeys[i]), "SIGNER_KEYS not strictly ascending");
        }
        require(2 * p.threshold > n && p.threshold <= n, "THRESHOLD outside (n/2, n]");
        require(p.releaseDelay >= p.minReleaseDelay, "RELEASE_DELAY_SECONDS < MIN_RELEASE_DELAY_SECONDS");
        if (p.reuseUsdc != address(0)) {
            require(p.reuseUsdc.code.length > 0 && DemoUSDC(p.reuseUsdc).minter() == _deployer(), "REUSE_USDC");
        }
    }

    function run() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        console.log(BANNER);
        uint256 pk = _pk();
        Params memory p = params();
        _print(p);
        (SeqSigGate g, BridgeVault v, DemoUSDC usdc) = deploy(pk, p);

        address wantGate = vm.envOr("EXPECTED_GATE", address(0));
        if (wantGate != address(0)) require(address(g) == wantGate, "gate address differs from EXPECTED_GATE");
        address wantVault = vm.envOr("EXPECTED_VAULT", address(0));
        if (wantVault != address(0)) require(address(v) == wantVault, "vault address differs from EXPECTED_VAULT");
        bytes32 wantCfg = vm.envOr("EXPECTED_CFG_ID", bytes32(0));
        if (wantCfg != bytes32(0)) require(v.cfgId() == wantCfg, "cfgId differs from EXPECTED_CFG_ID");
        bytes32 wantSet = vm.envOr("EXPECTED_SET_HASH", bytes32(0));
        if (wantSet != bytes32(0)) require(g.SIGNER_SET_HASH() == wantSet, "set hash differs from EXPECTED_SET_HASH");

        console.log("RISC0_VERIFIER=%s (bare RiscZeroGroth16Verifier, selector 0xe71e6620)", address(g.VERIFIER()));
        console.log("GATE=%s (SeqSigGate; the vault's verifier)", address(g));
        console.log("VAULT=%s", address(v));
        console.log("USDC=%s", address(usdc));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("SET_HASH=%s", vm.toString(g.SIGNER_SET_HASH()));
        console.log("PROGRAM_ID=%s", vm.toString(v.programId()));
        console.log("UPDATE_IMAGE_ID=%s", vm.toString(v.updateImageId()));
        console.log("EVIDENCE_IMAGE_ID=%s", vm.toString(g.EVIDENCE_IMAGE_ID()));
        console.log("RELEASE_IMAGE_ID=%s", vm.toString(v.releaseImageId()));
        console.log("G2_STATE=%s (the first update's prevState)", vm.toString(v.g2State()));
        console.log("THRESHOLD=%s SIGNERS=%s CONTEST_WINDOW=%s", g.THRESHOLD(), g.SIGNERS(), uint256(g.CONTEST_WINDOW()));
    }

    /// Creates (unless reused) the DemoUSDC, then the gate bound to the predicted vault address,
    /// then the vault, and checks every immutable.
    function deploy(uint256 pk, Params memory p) public returns (SeqSigGate g, BridgeVault v, DemoUSDC usdc) {
        require(block.chainid == SEPOLIA, "not Sepolia");
        check(p);
        address me = _deployer();
        vm.startBroadcast(pk);
        usdc = p.reuseUsdc != address(0) ? DemoUSDC(p.reuseUsdc) : new DemoUSDC();
        // the gate takes nonce N, the vault N + 1
        address predicted = vm.computeCreateAddress(me, uint256(vm.getNonce(me)) + 1);
        g = new SeqSigGate(
            IRiscZeroVerifier(p.risc0Verifier),
            ISeqSigVault(predicted),
            p.lezChainId,
            p.zoneId,
            p.programId,
            p.updateImageId,
            p.evidenceImageId,
            p.releaseImageId,
            p.signerKeys,
            p.threshold,
            p.contestWindow,
            p.testOnlyShortWindow
        );
        v = new BridgeVault(
            IRiscZeroVerifier(address(g)),
            p.updateImageId,
            p.releaseImageId,
            p.lezChainId,
            p.zoneId,
            p.programId,
            p.initialG2State,
            p.minReleaseDelay,
            me, // admin
            me, // freezer
            me // fee recipient
        );
        vm.stopBroadcast();

        require(address(v) == predicted, "vault not at the address the gate is bound to");
        require(address(g.VAULT()) == address(v), "gate VAULT");
        require(g.CFG_ID() == v.cfgId(), "gate CFG_ID differs from the vault's");
        require(address(v.verifier()) == address(g), "vault verifier is not the gate");
        require(address(g.VERIFIER()) == p.risc0Verifier, "gate VERIFIER");
        require(g.UPDATE_IMAGE_ID() == p.updateImageId && v.updateImageId() == p.updateImageId, "update image ids");
        require(g.EVIDENCE_IMAGE_ID() == p.evidenceImageId, "evidence image id");
        require(g.RELEASE_IMAGE_ID() == p.releaseImageId && v.releaseImageId() == p.releaseImageId, "release image ids");
        require(g.SIGNERS() == p.signerKeys.length && g.THRESHOLD() == p.threshold, "signers/threshold");
        require(g.CONTEST_WINDOW() == p.contestWindow && g.TEST_ONLY_SHORT_WINDOW() == p.testOnlyShortWindow, "window");
        require(
            g.SIGNER_SET_HASH() == sha256(abi.encodePacked("RFP022/seqsig/set/v1", uint32(p.signerKeys.length), p.signerKeys)),
            "set hash"
        );
        require(keccak256(bytes(g.KIND())) == SEQSIG_KIND_HASH, "gate KIND");
        require(v.minReleaseDelay() == p.minReleaseDelay, "minReleaseDelay");
        require(v.g2State() == p.initialG2State, "g2State");
        require(v.cfgId() == v.computeCfgId(SEPOLIA, address(v), p.lezChainId, p.zoneId, p.programId), "cfgId");
        require(v.admin() == me && v.freezer() == me, "admin/freezer");
    }

    /// Registry, relay-fee menus, caps and the tokens' release delay: SepoliaV4.s.sol's values
    /// exactly (v5/v6), for a vault behind a SeqSigGate.
    function configure() external {
        require(block.chainid == SEPOLIA, "not Sepolia");
        console.log(BANNER);
        BridgeVault v = BridgeVault(vm.envAddress("VAULT"));
        uint64 delay = _delayEnv("RELEASE_DELAY_SECONDS");
        console.log("RELEASE_DELAY_SECONDS=%s (vault minReleaseDelay %s)", uint256(delay), uint256(v.minReleaseDelay()));
        configureVault(_pk(), v, vm.envAddress("USDC"), delay);
    }

    function configureVault(uint256 pk, BridgeVault v, address usdc, uint64 releaseDelay) public {
        require(v.admin() == _deployer(), "not our vault");
        require(releaseDelay >= v.minReleaseDelay(), "RELEASE_DELAY_SECONDS < the vault's minReleaseDelay");
        require(keccak256(bytes(SeqSigGate(address(v.verifier())).KIND())) == SEQSIG_KIND_HASH, "not a v7 (seq-sig) vault");

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
        if (!uReg) v.registerToken(usdc, 6, ud, new uint256[](3), new uint256[](3), releaseDelay);
        if (!eReg) v.registerToken(address(0), 18, ed, new uint256[](3), new uint256[](3), releaseDelay);
        v.setRelayFeeMenu(usdc, _menu(ud));
        v.setRelayFeeMenu(address(0), _menu(ed));
        v.setCaps(usdc, CAP_WINDOW, 50_000e6, 50_000e6);
        v.setCaps(address(0), CAP_WINDOW, 5 ether, 5 ether);
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
