// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {MockVerifier, TestToken} from "./mocks/Mocks.sol";

/// Prints fixed vectors that the LEZ program's tests check byte for byte.
contract VectorsTest is Test {
    // cfgId inputs: pinned chain id and vault address, distinct non-zero LEZ ids so a field
    // order mistake on the other side shows.
    uint256 constant CHAIN_ID = 31337;
    address constant VAULT = address(0xBB);
    bytes32 constant LEZ_CHAIN_ID = bytes32(uint256(0x11));
    bytes32 constant ZONE_ID = bytes32(uint256(0x22));
    bytes32 constant PROGRAM_ID = bytes32(uint256(0x33));

    function test_PrintVectors() public {
        vm.chainId(CHAIN_ID);
        MockVerifier mv = new MockVerifier();
        bytes memory args = abi.encode(
            address(mv), bytes32(0), bytes32(0), LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID, bytes32(0), uint64(0),
            address(this), address(this), address(this), uint64(1 hours)
        );
        deployCodeTo("BridgeVault.sol:BridgeVault", args, VAULT);
        BridgeVault v = BridgeVault(VAULT);

        bytes memory pre = abi.encodePacked(keccak256("RFP022/cfg/v1"), CHAIN_ID, VAULT, LEZ_CHAIN_ID, ZONE_ID, PROGRAM_ID);
        assertEq(pre.length, 180);
        assertEq(v.cfgId(), sha256(pre));
        assertEq(v.CFG_TAG(), 0x26e962302b1dc9417c466ff56ef206a578cd054a0a099172cff8b80460ab5a45);
        assertEq(v.cfgId(), 0x5da920fc38f4f4750c2ed8cc5eb8771f3547ecb4b99a8b7eefbf767731b5cfbc);

        console.log("CFG_TAG");
        console.logBytes32(v.CFG_TAG());
        console.log("cfg inputs: chainid %d vault %s", CHAIN_ID, VAULT);
        console.log("lezChainId / zoneId / programId");
        console.logBytes32(LEZ_CHAIN_ID);
        console.logBytes32(ZONE_ID);
        console.logBytes32(PROGRAM_ID);
        console.log("cfg preimage (180 bytes)");
        console.logBytes(pre);
        console.log("cfgId");
        console.logBytes32(v.cfgId());

        address token = address(0x00000000000000000000000000000000000000AA);
        vm.etch(token, address(new TestToken("T", 6)).code);
        uint256[] memory d = new uint256[](1);
        d[0] = 100e6;
        uint256[] memory f = new uint256[](1);
        // decimals() reads an immutable in code, so the etched copy reports 6
        v.registerToken(token, 6, d, f, f, 0);
        TestToken(token).mint(address(this), 1000e6);
        TestToken(token).approve(address(v), type(uint256).max);
        for (uint256 i; i < 3; ++i) {
            bytes32 c = bytes32(uint256(i + 1));
            console.log("leaf (cfgId above)");
            console.logBytes32(v.depositLeaf(token, 100e6, c));
            v.deposit(token, 0, c);
            console.log("root");
            console.logBytes32(v.depositRoot());
        }
    }

    /// The leaf formula is unchanged: the vectors pinned in lez-bridge/bridge/tests/flow.rs
    /// (made with cfgId = 0x22 before cfgId was derived) still follow from it.
    function test_LegacyLeafVectorsCfg22() public pure {
        bytes32 cfg = bytes32(uint256(0x22));
        address token = address(0x00000000000000000000000000000000000000AA);
        bytes32 tag = keccak256("RFP022/deposit-leaf/v1");
        assertEq(
            sha256(abi.encodePacked(tag, cfg, token, uint256(100e6), bytes32(uint256(1)))),
            0x989d86fd05b092a4c8813f55dadd7fe91f22613ed224ed048e975e85b03d7f9b
        );
        assertEq(
            sha256(abi.encodePacked(tag, cfg, token, uint256(100e6), bytes32(uint256(2)))),
            0x268dcd2d37cd543db8910d79638b314baf930072f9d94af49036453fc3084d2b
        );
        assertEq(
            sha256(abi.encodePacked(tag, cfg, token, uint256(100e6), bytes32(uint256(3)))),
            0xdb3c6052763f80a5fb65302f8a3bc9c9d264be65e6b3d9996a6c75e0be38c9e7
        );
    }
}
