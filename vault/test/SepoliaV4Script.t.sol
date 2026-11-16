// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {UpdateGatedVerifier, IGatedVault} from "../src/UpdateGatedVerifier.sol";
import {SepoliaV4} from "../script/SepoliaV4.s.sol";
import {DemoUSDC} from "../script/DemoUSDC.sol";
import {MockGroth16Verifier} from "./mocks/Mocks.sol";

/// script/SepoliaV4.s.sol with ONE change: the expected deployer is Anvil account 0 (a public
/// test key). Also used for the throwaway-chain rehearsal (public-demo/v4/rehearse-v4.sh) on an
/// anvil started with --chain-id 11155111, with a MockGroth16Verifier standing in ONLY for RISC
/// Zero's Groth16 check:
///
///   SEPOLIA_PK=<anvil 0 key> RISC0_VERIFIER=<mock> SUBMITTER=... RELEASE_IMAGE_ID=... \
///   UPDATE_IMAGE_ID=... PROGRAM_ID=... LEZ_CHAIN_ID=... ZONE_ID=... INITIAL_G2_STATE=... \
///   forge script test/SepoliaV4Script.t.sol:SepoliaV4Harness --rpc-url http://127.0.0.1:<anvil> --broadcast
contract SepoliaV4Harness is SepoliaV4 {
    function _deployer() internal pure override returns (address) {
        return 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    }
}

contract SepoliaV4ScriptTest is Test {
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    uint256 constant SEPOLIA = 11155111;
    // Stand-in values: the plan passes the real ids as inputs (none is a constant of the script).
    bytes32 constant RELEASE_ID = keccak256("test: release image id");
    bytes32 constant UPDATE_ID = keccak256("test: update image id");
    bytes32 constant PROGRAM_ID = keccak256("test: v4 bridge program id");
    bytes32 constant LEZ_CHAIN_ID = 0x2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374;
    bytes32 constant ZONE_ID = 0x4bd1c1bd6ea22e722a0cad604c306cce32cf6001d735888ec0ea5d1dd0882afb;
    bytes32 constant G2_STATE0 = keccak256("test: G2 genesis state");

    SepoliaV4Harness h;
    MockGroth16Verifier r0;
    address submitter = address(0x5AB);
    address bob = address(0xB0B);

    function setUp() public {
        vm.chainId(SEPOLIA);
        vm.warp(1_763_000_000);
        h = new SepoliaV4Harness();
        r0 = new MockGroth16Verifier(0xe71e6620);
        vm.deal(ANVIL0, 10 ether);
    }

    function _p() internal view returns (SepoliaV4.Params memory) {
        return SepoliaV4.Params(
            address(r0), submitter, RELEASE_ID, UPDATE_ID, PROGRAM_ID, address(0), LEZ_CHAIN_ID, ZONE_ID, G2_STATE0,
            1 hours, 1 hours // v4's delays
        );
    }

    /// Deploy and configure exactly as the script does; the first update must start from the
    /// genesis state (prevState zero is refused), through the gate; then a release after the hour.
    function test_DeployConfigureAndRoundTrip() public {
        (UpdateGatedVerifier g, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p());
        assertEq(address(v.verifier()), address(g));
        assertEq(address(g.VERIFIER()), address(r0));
        assertEq(g.SUBMITTER(), submitter);
        assertEq(g.UPDATE_IMAGE_ID(), UPDATE_ID);
        assertEq(v.updateImageId(), UPDATE_ID);
        assertEq(v.releaseImageId(), RELEASE_ID);
        assertEq(v.minReleaseDelay(), 1 hours);
        assertEq(v.admin(), ANVIL0);
        assertEq(v.freezer(), ANVIL0);
        assertEq(v.g2State(), G2_STATE0);
        assertEq(v.cfgId(), v.computeCfgId(SEPOLIA, address(v), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID));

        h.configureVault(ANVIL0_PK, v, address(usdc), 1 hours);
        (bool reg,,,, uint64 delay, uint256[] memory denoms,,,) = v.tokenConfig(address(0));
        assertTrue(reg);
        assertEq(delay, 1 hours);
        assertEq(denoms[0], 0.01 ether);
        h.configureVault(ANVIL0_PK, v, address(usdc), 1 hours); // re-runnable

        vm.prank(ANVIL0);
        v.depositETH{value: 0.01 ether}(0, keccak256("C"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 0, keccak256("E"));

        bytes memory j0 = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), 0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes memory s0 = r0.sealFor(UPDATE_ID, sha256(j0));
        vm.prank(submitter);
        vm.expectRevert(BridgeVault.StaleState.selector);
        g.submitUpdate(IGatedVault(address(v)), j0, s0);

        bytes memory j = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), G2_STATE0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes memory s = r0.sealFor(UPDATE_ID, sha256(j));
        vm.expectRevert(UpdateGatedVerifier.UpdateNotThroughGate.selector);
        v.update(j, s); // not through the gate
        vm.prank(submitter);
        g.submitUpdate(IGatedVault(address(v)), j, s);
        assertEq(v.g2State(), keccak256("s1"));

        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256("nf");
        bytes memory rs = r0.sealFor(RELEASE_ID, sha256(abi.encode(v.cfgId(), root, nf, address(0), uint256(0.01 ether), bob, address(0), uint256(0))));
        vm.warp(block.timestamp + 1 hours - 1);
        vm.expectRevert(BridgeVault.ReleaseTooEarly.selector);
        v.release(rs, root, nf, address(0), 0, bob, address(0), 0);
        vm.warp(block.timestamp + 1);
        v.release(rs, root, nf, address(0), 0, bob, address(0), 0);
        assertEq(bob.balance, 0.01 ether);
    }

    function _expectDeployRevert(SepoliaV4.Params memory p, string memory why) internal {
        vm.expectRevert(bytes(why));
        h.deploy(ANVIL0_PK, p);
    }

    function test_Refusals() public {
        SepoliaV4.Params memory p = _p();
        p.initialG2State = 0; // v3's mistake: no update could ever apply
        _expectDeployRevert(p, "INITIAL_G2_STATE is zero");
        p = _p();
        p.updateImageId = 0;
        _expectDeployRevert(p, "UPDATE_IMAGE_ID is zero");
        p = _p();
        p.releaseImageId = 0;
        _expectDeployRevert(p, "RELEASE_IMAGE_ID is zero");
        p = _p();
        p.updateImageId = keccak256("RFP022/demo/no-update-image");
        _expectDeployRevert(p, "UPDATE_IMAGE_ID is the old placeholder");
        p = _p();
        p.releaseImageId = keccak256("RFP022/demo/no-update-image");
        _expectDeployRevert(p, "RELEASE_IMAGE_ID is the old placeholder");
        p = _p();
        p.releaseImageId = UPDATE_ID;
        _expectDeployRevert(p, "RELEASE_IMAGE_ID == UPDATE_IMAGE_ID");
        p = _p();
        p.programId = 0;
        _expectDeployRevert(p, "PROGRAM_ID is zero");
        p = _p();
        p.lezChainId = 0;
        _expectDeployRevert(p, "LEZ_CHAIN_ID is zero");
        p = _p();
        p.zoneId = 0;
        _expectDeployRevert(p, "ZONE_ID is zero");
        p = _p();
        p.submitter = address(0);
        _expectDeployRevert(p, "SUBMITTER is zero");
        p = _p();
        p.submitter = ANVIL0;
        _expectDeployRevert(p, "SUBMITTER must not be the deployer/admin/freezer");
        p = _p();
        p.risc0Verifier = address(0xDEAD);
        _expectDeployRevert(p, "RISC0_VERIFIER has no code");
        p = _p();
        p.risc0Verifier = address(new MockGroth16Verifier(0x73c457ba));
        _expectDeployRevert(p, "RISC0_VERIFIER is the 3.0.0 verifier (GHSA-g6qg-9j8p-7j87)");
        p = _p();
        p.risc0Verifier = address(new MockGroth16Verifier(0x12345678));
        _expectDeployRevert(p, "RISC0_VERIFIER selector is not 0xe71e6620 (3.1.0)");
        p = _p();
        p.reuseUsdc = address(0xDEAD);
        _expectDeployRevert(p, "REUSE_USDC");
        p = _p();
        p.minReleaseDelay = 2 hours; // tokens' delay below the vault's minimum
        _expectDeployRevert(p, "RELEASE_DELAY_SECONDS < MIN_RELEASE_DELAY_SECONDS");
        p = _p();
        p.releaseDelay = 31 days;
        p.minReleaseDelay = 31 days;
        _expectDeployRevert(p, "delay above 30 days");
        vm.chainId(1);
        _expectDeployRevert(_p(), "not Sepolia");
    }

    /// configure() refuses a vault that is not behind an update gate (e.g. the v3 demo vault).
    function test_ConfigureRefusesOtherVaults() public {
        (, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p());
        BridgeVault plain = new BridgeVault(
            r0, UPDATE_ID, RELEASE_ID, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, G2_STATE0, 0, ANVIL0, ANVIL0, ANVIL0
        );
        vm.expectRevert();
        h.configureVault(ANVIL0_PK, plain, address(usdc), 1 hours);
        h.configureVault(ANVIL0_PK, v, address(usdc), 1 hours);
    }

    /// The only test that touches the process environment, with values no other test reads.
    function test_ParamsFromEnv() public {
        vm.setEnv("RISC0_VERIFIER", vm.toString(address(r0)));
        vm.setEnv("SUBMITTER", vm.toString(submitter));
        vm.setEnv("RELEASE_IMAGE_ID", vm.toString(RELEASE_ID));
        vm.setEnv("UPDATE_IMAGE_ID", vm.toString(UPDATE_ID));
        vm.setEnv("PROGRAM_ID", vm.toString(PROGRAM_ID));
        vm.setEnv("LEZ_CHAIN_ID", vm.toString(LEZ_CHAIN_ID));
        vm.setEnv("ZONE_ID", vm.toString(ZONE_ID));
        vm.setEnv("INITIAL_G2_STATE", vm.toString(G2_STATE0));
        vm.setEnv("MIN_RELEASE_DELAY_SECONDS", "0");
        vm.setEnv("RELEASE_DELAY_SECONDS", "0");
        SepoliaV4.Params memory p = h.params();
        assertEq(p.minReleaseDelay, 0);
        assertEq(p.releaseDelay, 0);
        assertEq(p.risc0Verifier, address(r0));
        assertEq(p.submitter, submitter);
        assertEq(p.releaseImageId, RELEASE_ID);
        assertEq(p.updateImageId, UPDATE_ID);
        assertEq(p.programId, PROGRAM_ID);
        assertEq(p.lezChainId, LEZ_CHAIN_ID);
        assertEq(p.zoneId, ZONE_ID);
        assertEq(p.initialG2State, G2_STATE0);
    }

    /// v5: no release delay (0 / 0). The first update starts from the genesis state through the
    /// gate, and its exit is released in the SAME block; a release with a wrong proof still fails,
    /// and so does a replay. Nothing else in the vault assumes a delay above zero.
    function test_ZeroDelay_ReleaseInTheSameBlock() public {
        SepoliaV4.Params memory p = _p();
        p.minReleaseDelay = 0;
        p.releaseDelay = 0;
        (UpdateGatedVerifier g, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, p);
        assertEq(v.minReleaseDelay(), 0);
        h.configureVault(ANVIL0_PK, v, address(usdc), 0);
        (,,,, uint64 delay,,,,) = v.tokenConfig(address(0));
        assertEq(delay, 0);

        vm.prank(ANVIL0);
        v.depositETH{value: 0.01 ether}(0, keccak256("C"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 0, keccak256("E"));
        bytes memory j = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), G2_STATE0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes memory s = r0.sealFor(UPDATE_ID, sha256(j));
        uint256 t0 = block.timestamp;
        vm.prank(submitter);
        g.submitUpdate(IGatedVault(address(v)), j, s);
        bytes32 root = v.exitRoot();
        assertEq(v.releasableAt(address(0), root), t0, "releasable at the update's own timestamp");

        bytes32 nf = keccak256("nf");
        bytes memory bad = r0.sealFor(RELEASE_ID, sha256(abi.encode(v.cfgId(), root, nf, address(0), uint256(0.01 ether), address(0xBAD), address(0), uint256(0))));
        vm.expectRevert(MockGroth16Verifier.BadSeal.selector);
        v.release(bad, root, nf, address(0), 0, bob, address(0), 0); // a proof for another recipient
        bytes memory rs = r0.sealFor(RELEASE_ID, sha256(abi.encode(v.cfgId(), root, nf, address(0), uint256(0.01 ether), bob, address(0), uint256(0))));
        v.release(rs, root, nf, address(0), 0, bob, address(0), 0); // same block, same timestamp
        assertEq(block.timestamp, t0);
        assertEq(bob.balance, 0.01 ether);
        vm.expectRevert(BridgeVault.NullifierSpent.selector);
        v.release(rs, root, nf, address(0), 0, bob, address(0), 0);
        // The admin can still raise the delay later (an emergency brake); a lowering back to 0 is allowed.
        vm.prank(ANVIL0);
        v.setReleaseDelay(address(0), 1 hours);
        vm.prank(ANVIL0);
        v.setReleaseDelay(address(0), 0);
    }
}
