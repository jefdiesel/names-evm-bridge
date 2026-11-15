// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {DemoAttestedVerifier as DAV} from "../src/demo/DemoAttestedVerifier.sol";
import {SepoliaDemoV3} from "../script/SepoliaDemoV3.s.sol";
import {DemoUSDC} from "../script/DemoUSDC.sol";

/// script/SepoliaDemoV3.s.sol with ONE change: the expected deployer is Anvil account 0 (a public
/// test key) instead of the real throwaway deployer, whose key is not on this machine. Used by
/// the tests below and for an offline `forge script` simulation:
///
///   SEPOLIA_PK=<anvil 0 key> ATTESTOR=... RELEASE_IMAGE_ID=... UPDATE_IMAGE_ID=... PROGRAM_ID=... \
///   forge script test/SepoliaDemoV3Script.t.sol:SepoliaDemoV3Harness --chain-id 11155111 --offline
contract SepoliaDemoV3Harness is SepoliaDemoV3 {
    function _deployer() internal pure override returns (address) {
        return 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    }
}

contract SepoliaDemoV3ScriptTest is Test {
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    uint256 constant SEPOLIA = 11155111;
    // the G3 image id of the v1/v2 demo vaults; the G2 id here is a stand-in value for the test
    bytes32 constant RELEASE_ID = 0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4;
    bytes32 constant UPDATE_ID = keccak256("test: outbound_guest_trusted_tip image id");
    bytes32 constant PROGRAM_ID = 0xfff8054169fcabf078493cfda8214d14f03a7294be509dfd9180d29e33fde910;
    /// A chain's channel id and the stand-in root-source id (`lez-bridge source-id`), test values.
    bytes32 constant LEZ_CHAIN_ID = 0xd416ed12bbf19a0542b1b1eff38591f9527a5179b5e7fda5445f9127e14df31d;
    bytes32 constant ZONE_ID = 0x4bd1c1bd6ea22e722a0cad604c306cce32cf6001d735888ec0ea5d1dd0882afb;
    /// A stand-in G2 genesis state (any non-zero value; `outbound_host genesis` gives the real one).
    bytes32 constant G2_STATE0 = keccak256("test: G2 genesis state");

    SepoliaDemoV3Harness h;
    uint256 attPk = uint256(keccak256("v3 attestor"));
    address bob = address(0xB0B);

    function setUp() public {
        vm.chainId(SEPOLIA);
        vm.warp(1_763_000_000);
        h = new SepoliaDemoV3Harness();
        vm.deal(ANVIL0, 10 ether);
    }

    function _p() internal view returns (SepoliaDemoV3.Params memory) {
        return SepoliaDemoV3.Params(vm.addr(attPk), 180 days, RELEASE_ID, UPDATE_ID, PROGRAM_ID, address(0), LEZ_CHAIN_ID, ZONE_ID, G2_STATE0);
    }

    function _seal(DAV dv, bytes32 img, bytes32 dig) internal view returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(attPk, dv.attestationDigest(img, dig));
        return abi.encodePacked(dv.SEAL_PREFIX(), r, s, v);
    }

    /// Deploy and configure exactly as the script does, then run one round trip on it with a
    /// 10-minute delay: deposit, attested update, attested release.
    function test_DeployConfigureAndRoundTrip() public {
        (DAV dv, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p());
        assertEq(address(v.verifier()), address(dv));
        assertEq(dv.ATTESTOR(), vm.addr(attPk));
        assertEq(dv.EXPIRES_AT(), block.timestamp + 180 days);
        assertEq(v.updateImageId(), UPDATE_ID);
        assertEq(v.releaseImageId(), RELEASE_ID);
        assertEq(v.minReleaseDelay(), 10 minutes);
        assertEq(v.admin(), ANVIL0);
        assertEq(v.freezer(), ANVIL0);
        assertEq(v.cfgId(), v.computeCfgId(SEPOLIA, address(v), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID));
        assertEq(v.lezChainId(), LEZ_CHAIN_ID);
        assertEq(v.zoneId(), ZONE_ID);
        assertEq(v.g2State(), G2_STATE0);
        assertEq(usdc.minter(), ANVIL0);

        h.configureVault(ANVIL0_PK, v, address(usdc));
        (bool reg,,,, uint64 delay, uint256[] memory denoms,,,) = v.tokenConfig(address(usdc));
        assertTrue(reg);
        assertEq(delay, 10 minutes);
        assertEq(denoms.length, 3);
        (uint64 w, uint256 dc, uint256 rc,,) = v.capState(address(usdc));
        assertEq(w, 1 days);
        assertEq(dc, 50_000e6);
        assertEq(rc, 50_000e6);
        assertEq(v.relayFeeMenu(address(usdc), 0)[1], 1e6);
        h.configureVault(ANVIL0_PK, v, address(usdc)); // re-runnable

        // round trip
        vm.startPrank(ANVIL0);
        usdc.mint(ANVIL0, 100e6);
        usdc.approve(address(v), 100e6);
        v.deposit(address(usdc), 0, keccak256("C"));
        vm.stopPrank();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("E"));
        // a journal from a zero state is refused: the vault starts at G2_STATE0
        bytes memory j0 = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), 0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes memory s0 = _seal(dv, UPDATE_ID, sha256(j0));
        vm.expectRevert(BridgeVault.StaleState.selector);
        v.update(j0, s0);
        bytes memory j = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), G2_STATE0, keccak256("s1"), new bytes32[](0), cl, ex));
        v.update(j, _seal(dv, UPDATE_ID, sha256(j)));
        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256("nf");
        bytes memory rs = _seal(dv, RELEASE_ID, sha256(abi.encode(v.cfgId(), root, nf, address(usdc), 100e6, bob, address(0), 0)));
        vm.warp(block.timestamp + 10 minutes - 1);
        vm.expectRevert(BridgeVault.ReleaseTooEarly.selector);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        vm.warp(block.timestamp + 1);
        v.release(rs, root, nf, address(usdc), 0, bob, address(0), 0);
        assertEq(usdc.balanceOf(bob), 100e6);
    }

    function _expectDeployRevert(SepoliaDemoV3.Params memory p, string memory why) internal {
        vm.expectRevert(bytes(why));
        h.deploy(ANVIL0_PK, p);
    }

    function test_Refusals() public {
        SepoliaDemoV3.Params memory p = _p();
        p.updateImageId = 0;
        _expectDeployRevert(p, "UPDATE_IMAGE_ID is zero");
        p = _p();
        p.updateImageId = keccak256("RFP022/demo/no-update-image");
        _expectDeployRevert(p, "UPDATE_IMAGE_ID is the old placeholder");
        p = _p();
        p.releaseImageId = 0;
        _expectDeployRevert(p, "RELEASE_IMAGE_ID is zero");
        p = _p();
        p.releaseImageId = keccak256("RFP022/demo/no-update-image");
        _expectDeployRevert(p, "RELEASE_IMAGE_ID is the old placeholder");
        p = _p();
        p.releaseImageId = UPDATE_ID;
        _expectDeployRevert(p, "RELEASE_IMAGE_ID == UPDATE_IMAGE_ID");
        p = _p();
        p.attestor = address(0);
        _expectDeployRevert(p, "ATTESTOR is zero");
        p = _p();
        p.attestor = ANVIL0;
        _expectDeployRevert(p, "ATTESTOR must not be the deployer/admin/freezer");
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
        p.initialG2State = 0; // what v3 was deployed with: no update could ever apply
        _expectDeployRevert(p, "INITIAL_G2_STATE is zero");
        p = _p();
        p.reuseUsdc = address(0xDEAD); // no code
        _expectDeployRevert(p, "REUSE_USDC");
        p = _p();
        p.lifetime = 366 days; // the verifier's own bound
        vm.expectRevert(DAV.BadLifetime.selector);
        h.deploy(ANVIL0_PK, p);
        vm.chainId(1);
        _expectDeployRevert(_p(), "not Sepolia");
    }

    function test_ReuseUsdc() public {
        (,, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p());
        SepoliaDemoV3.Params memory p = _p();
        p.reuseUsdc = address(usdc);
        (,, DemoUSDC again) = h.deploy(ANVIL0_PK, p);
        assertEq(address(again), address(usdc));
        // a DemoUSDC someone else can mint is refused
        vm.prank(address(0xBEEF));
        DemoUSDC foreign = new DemoUSDC();
        p.reuseUsdc = address(foreign);
        _expectDeployRevert(p, "REUSE_USDC");
    }

    /// The only test that touches the process environment (vm.setEnv is process-wide), with
    /// values no other test reads.
    function test_ParamsFromEnv() public {
        vm.setEnv("ATTESTOR", vm.toString(vm.addr(attPk)));
        vm.setEnv("DEMO_LIFETIME", "864000");
        vm.setEnv("RELEASE_IMAGE_ID", vm.toString(RELEASE_ID));
        vm.setEnv("UPDATE_IMAGE_ID", vm.toString(UPDATE_ID));
        vm.setEnv("PROGRAM_ID", vm.toString(PROGRAM_ID));
        vm.setEnv("LEZ_CHAIN_ID", vm.toString(LEZ_CHAIN_ID));
        vm.setEnv("ZONE_ID", vm.toString(ZONE_ID));
        vm.setEnv("INITIAL_G2_STATE", vm.toString(G2_STATE0));
        SepoliaDemoV3.Params memory p = h.params();
        assertEq(p.attestor, vm.addr(attPk));
        assertEq(p.lifetime, 10 days);
        assertEq(p.releaseImageId, RELEASE_ID);
        assertEq(p.updateImageId, UPDATE_ID);
        assertEq(p.programId, PROGRAM_ID);
        assertEq(p.reuseUsdc, address(0));
        assertEq(p.lezChainId, LEZ_CHAIN_ID);
        assertEq(p.zoneId, ZONE_ID);
        assertEq(p.initialG2State, G2_STATE0);
    }
}
