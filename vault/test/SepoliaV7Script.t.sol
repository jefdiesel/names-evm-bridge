// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {SeqSigGate} from "../src/SeqSigGate.sol";
import {SepoliaV7} from "../script/SepoliaV7.s.sol";
import {DemoUSDC} from "../script/DemoUSDC.sol";
import {MockGroth16Verifier} from "./mocks/Mocks.sol";

/// script/SepoliaV7.s.sol with ONE change: the expected deployer is Anvil account 0 (a public test
/// key). Used by the throwaway rehearsal (public-demo/v7/rehearse-v7.sh, anvil --chain-id 11155111
/// with a MockGroth16Verifier) and the Sepolia-fork hand-off test (public-demo/v7/handoff-fork.sh,
/// the real verifier 3.1.0).
contract SepoliaV7Harness is SepoliaV7 {
    function _deployer() internal pure override returns (address) {
        return 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    }
}

contract SepoliaV7ScriptTest is Test {
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    uint256 constant SEPOLIA = 11155111;
    bytes32 constant UPDATE_ID = keccak256("test: seq-sig update image id");
    bytes32 constant EVIDENCE_ID = keccak256("test: evidence image id");
    bytes32 constant RELEASE_ID = keccak256("test: release image id");
    bytes32 constant PROGRAM_ID = keccak256("test: v7 bridge program id");
    bytes32 constant LEZ_CHAIN_ID = 0x2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374;
    bytes32 constant ZONE_ID = 0x21617f2f28040bbbaa8eed893bd78b65c2e5056ab7f55fda7b3f736a97c03fff;
    bytes32 constant G2_STATE0 = keccak256("test: G2 genesis state");

    SepoliaV7Harness h;
    MockGroth16Verifier r0;
    address bob = address(0xB0B);
    address anyone = address(0xA11);

    function setUp() public {
        vm.chainId(SEPOLIA);
        vm.warp(1_763_000_000);
        h = new SepoliaV7Harness();
        r0 = new MockGroth16Verifier(0xe71e6620);
        vm.deal(ANVIL0, 10 ether);
    }

    /// Three keys, strictly ascending as uint256, canonical (y < p) and not of small order.
    function _keys() internal pure returns (bytes32[] memory k) {
        k = new bytes32[](3);
        k[0] = bytes32(uint256(0x11) << 8 | 0x05);
        k[1] = bytes32(uint256(0x22) << 8 | 0x07);
        k[2] = bytes32(uint256(0x33) << 8 | 0x09);
    }

    function _p(uint64 window, bool flag) internal view returns (SepoliaV7.Params memory p) {
        p.risc0Verifier = address(r0);
        p.updateImageId = UPDATE_ID;
        p.evidenceImageId = EVIDENCE_ID;
        p.releaseImageId = RELEASE_ID;
        p.programId = PROGRAM_ID;
        p.lezChainId = LEZ_CHAIN_ID;
        p.zoneId = ZONE_ID;
        p.initialG2State = G2_STATE0;
        p.signerKeys = _keys();
        p.threshold = 2;
        p.contestWindow = window;
        p.testOnlyShortWindow = flag;
    }

    function _tail(SeqSigGate g, uint256 bitmap, uint256 blockId) internal view returns (bytes memory) {
        return abi.encode(g.SIGNER_SET_HASH(), g.THRESHOLD(), uint256(0), bitmap, uint256(100), keccak256("prev"), blockId, keccak256(abi.encode(blockId)));
    }

    /// Deploy and configure as the script does, then the v7 round trip through the gate: a 2-of-3
    /// journal staged by an account with no role, the 600 s window, finalize, release.
    function test_DeployConfigureAndRoundTrip() public {
        (SeqSigGate g, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p(600, true));
        assertEq(address(v.verifier()), address(g));
        assertEq(address(g.VAULT()), address(v));
        assertEq(g.CFG_ID(), v.cfgId());
        assertEq(g.THRESHOLD(), 2);
        assertEq(g.SIGNERS(), 3);
        assertEq(g.CONTEST_WINDOW(), 600);
        assertTrue(g.TEST_ONLY_SHORT_WINDOW());
        assertEq(v.updateImageId(), UPDATE_ID);
        assertEq(v.minReleaseDelay(), 0);
        assertEq(v.admin(), ANVIL0);
        assertEq(v.freezer(), ANVIL0);

        h.configureVault(ANVIL0_PK, v, address(usdc), 0);
        (bool reg,,,, uint64 delay, uint256[] memory denoms,,,) = v.tokenConfig(address(0));
        assertTrue(reg);
        assertEq(delay, 0);
        assertEq(denoms[0], 0.01 ether);
        h.configureVault(ANVIL0_PK, v, address(usdc), 0); // re-runnable

        vm.prank(ANVIL0);
        v.depositETH{value: 0.01 ether}(0, keccak256("C"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 0, keccak256("E"));
        bytes memory vj = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), G2_STATE0, keccak256("s1"), new bytes32[](0), cl, ex));

        // 1 of 3: the guest would refuse it; a journal claiming it is refused at the gate
        bytes memory j1 = bytes.concat(vj, _tail(g, 0x1, 200));
        bytes memory s1 = r0.sealFor(UPDATE_ID, sha256(j1));
        vm.prank(anyone);
        vm.expectRevert(SeqSigGate.BadSignerBitmap.selector);
        g.submit(j1, s1);

        bytes memory j = bytes.concat(vj, _tail(g, 0x5, 200)); // keys 0 and 2
        bytes memory s = r0.sealFor(UPDATE_ID, sha256(j));
        vm.prank(anyone);
        g.submit(j, s);
        assertEq(v.g2State(), G2_STATE0);
        vm.expectRevert(SeqSigGate.ContestWindowOpen.selector);
        g.finalize(vj);
        vm.warp(block.timestamp + 600);
        vm.prank(bob);
        g.finalize(vj);
        assertEq(v.g2State(), keccak256("s1"));

        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256("nf");
        bytes memory rs = r0.sealFor(RELEASE_ID, sha256(abi.encode(v.cfgId(), root, nf, address(0), uint256(0.01 ether), bob, address(0), uint256(0))));
        v.release(rs, root, nf, address(0), 0, bob, address(0), 0);
        assertEq(bob.balance, 0.01 ether);
    }

    /// A 600 s window without the visible test flag is refused by the gate (its minimum is 6 h).
    function test_ShortWindowNeedsTheFlag() public {
        SepoliaV7.Params memory p = _p(600, false);
        vm.expectRevert(SeqSigGate.WindowTooShort.selector);
        h.deploy(ANVIL0_PK, p);
    }

    function test_RefusesUnsortedKeysAndBadThreshold() public {
        SepoliaV7.Params memory p = _p(600, true);
        (p.signerKeys[0], p.signerKeys[1]) = (p.signerKeys[1], p.signerKeys[0]);
        vm.expectRevert(bytes("SIGNER_KEYS not strictly ascending"));
        h.check(p);
        p = _p(600, true);
        p.threshold = 1;
        vm.expectRevert(bytes("THRESHOLD outside (n/2, n]"));
        h.check(p);
    }

    /// The vault configure() of v4-v6 refuses this vault (its KIND check), the v7 one accepts it.
    function test_ConfigureNeedsASeqSigVault() public {
        (, BridgeVault v, DemoUSDC usdc) = h.deploy(ANVIL0_PK, _p(6 hours, false));
        h.configureVault(ANVIL0_PK, v, address(usdc), 0);
    }
}
