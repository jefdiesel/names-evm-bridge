// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken, NoReturnToken} from "./mocks/Mocks.sol";

/// Shared deployment and helpers for the vault tests (moved out of BridgeVault.t.sol so the
/// newer suites can reuse them without re-running that suite's tests).
///
/// Setup: a 6-decimal USDC (100 / 1000, 1% fees both ways), a USDT-style token with no return
/// values (100 / 1000, no fees) and ETH (0.1 / 1, withdraw fee 1%), release delay 1 day, min
/// delay 1 hour. Relay-fee menu: 2 USDC on 100 USDC, 0.002 ETH on 1 ETH.
abstract contract VaultFixture is Test {
    BridgeVault v;
    MockVerifier mv;
    TestToken usdc;
    NoReturnToken usdt;

    bytes32 constant G2 = keccak256("g2-image");
    bytes32 constant G3 = keccak256("g3-image");
    bytes32 constant LEZ_CHAIN = keccak256("lez-chain");
    bytes32 constant ZONE = keccak256("zone");
    bytes32 constant PROGRAM = keccak256("program");
    bytes32 constant S0 = keccak256("checkpoint");
    uint64 constant DELAY = 1 days;
    uint64 constant ADMIN_DELAY = 2 days;

    address admin = address(0xA11CE);
    address freezer = address(0xF2EE);
    address feeTo = address(0xFEE);
    address alice = address(0xA1);
    address bob = address(0xB0B);
    address relayer = address(0x7E1A);

    function setUp() public virtual {
        mv = new MockVerifier();
        v = new BridgeVault(
            IRiscZeroVerifier(address(mv)),
            G2,
            G3,
            LEZ_CHAIN,
            ZONE,
            PROGRAM,
            S0,
            1 hours,
            admin,
            freezer,
            feeTo,
            ADMIN_DELAY
        );
        usdc = new TestToken("USDC", 6);
        usdt = new NoReturnToken();

        vm.startPrank(admin);
        v.registerToken(address(usdc), 6, _arr(100e6, 1000e6), _arr(1e6, 10e6), _arr(1e6, 10e6), DELAY);
        v.registerToken(address(usdt), 6, _arr(100e6, 1000e6), _arr(0, 0), _arr(0, 0), DELAY);
        v.registerToken(address(0), 18, _arr(0.1 ether, 1 ether), _arr(0, 0), _arr(0.001 ether, 0.01 ether), DELAY);
        // relay fees the tests use (F#13 menu): 2 USDC on 100 USDC, 0.002 ETH on 1 ETH
        v.setRelayFeeMenu(address(usdc), _menu2(_one(2e6), new uint256[](0)));
        v.setRelayFeeMenu(address(0), _menu2(new uint256[](0), _one(0.002 ether)));
        vm.stopPrank();

        usdc.mint(alice, 1_000_000e6);
        usdt.mint(alice, 1_000_000e6);
        vm.deal(alice, 100 ether);
        vm.startPrank(alice);
        usdc.approve(address(v), type(uint256).max);
        usdt.approve(address(v), type(uint256).max);
        vm.stopPrank();
    }

    // ------------------------------------------------------------------ helpers

    function _arr(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        r[0] = a;
        r[1] = b;
    }

    function _one(uint256 a) internal pure returns (uint256[] memory r) {
        r = new uint256[](1);
        r[0] = a;
    }

    function _menu2(uint256[] memory a, uint256[] memory b) internal pure returns (uint256[][] memory m) {
        m = new uint256[][](2);
        m[0] = a;
        m[1] = b;
    }

    function _zero(uint256 level) internal pure returns (bytes32 z) {
        z = bytes32(0);
        for (uint256 i; i < level; ++i) z = sha256(abi.encodePacked(z, z));
    }

    /// Independent reference: build the full depth-32 root level by level.
    function _refRoot(bytes32[] memory leaves) internal pure returns (bytes32) {
        bytes32[] memory layer = leaves;
        for (uint256 level; level < 32; ++level) {
            uint256 n = (layer.length + 1) / 2;
            if (n == 0) n = 1;
            bytes32[] memory next = new bytes32[](n);
            for (uint256 i; i < n; ++i) {
                bytes32 l = 2 * i < layer.length ? layer[2 * i] : _zero(level);
                bytes32 r = 2 * i + 1 < layer.length ? layer[2 * i + 1] : _zero(level);
                next[i] = sha256(abi.encodePacked(l, r));
            }
            layer = next;
        }
        return layer[0];
    }

    function _deposit(uint32 d, bytes32 c) internal returns (uint64) {
        vm.prank(alice);
        return v.deposit(address(usdc), d, c);
    }

    function _journal(
        bytes32 prev,
        bytes32 next,
        bytes32[] memory roots,
        BridgeVault.ClaimDelta[] memory claims,
        BridgeVault.Exit[] memory exits
    ) internal view returns (bytes memory) {
        return abi.encode(BridgeVault.UpdateJournal(v.cfgId(), prev, next, roots, claims, exits));
    }

    function _update(bytes memory journal) internal {
        v.update(journal, mv.sealFor(G2, sha256(journal)));
    }

    /// Deposit `n` of USDC denom 0, claim them all, exit `exits` of them.
    function _bridgeOut(uint256 n, uint256 exits) internal returns (bytes32 exitRoot_) {
        for (uint256 i; i < n; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](exits);
        for (uint256 i; i < exits; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("e", i)));
        _update(_journal(v.g2State(), keccak256(abi.encode(v.g2State())), roots, cl, ex));
        return v.exitRoot();
    }

    function _releaseSeal(
        bytes32 root,
        bytes32 nf,
        address token,
        uint256 denom,
        address to,
        address rel,
        uint256 fee
    ) internal view returns (bytes memory) {
        return mv.sealFor(G3, sha256(abi.encode(v.cfgId(), root, nf, token, denom, to, rel, fee)));
    }

    /// Deposit, claim and exit `n` USDC denom-0 under tag-unique commitments; returns the new exit root.
    function _batch(uint256 n, bytes32 tag) internal returns (bytes32) {
        for (uint256 i; i < n; ++i) _deposit(0, keccak256(abi.encode(tag, "c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n);
        for (uint256 i; i < n; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode(tag, "e", i)));
        _update(_journal(v.g2State(), keccak256(abi.encode(v.g2State())), new bytes32[](0), cl, ex));
        return v.exitRoot();
    }

    function _rel(bytes32 root, bytes32 nf) internal {
        v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 0), root, nf, address(usdc), 0, bob, relayer, 0);
    }

    function _relExpect(bytes32 root, bytes32 nf, bytes4 err) internal {
        bytes memory s = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 0);
        vm.expectRevert(err);
        v.release(s, root, nf, address(usdc), 0, bob, relayer, 0);
    }

    function _depExpect(address token, bytes32 c, bytes4 err) internal {
        vm.prank(alice);
        vm.expectRevert(err);
        v.deposit(token, 0, c);
    }

    /// The accrued fees of `token` (last field of `tokenConfig`).
    function _fees(address token) internal view returns (uint256 f) {
        (,,,,,,,, f) = v.tokenConfig(token);
    }

    function _bal(address token, address who) internal view returns (uint256) {
        if (token == address(0)) return who.balance;
        return TestToken(token).balanceOf(who); // same selector on NoReturnToken
    }

    /// Schedule and execute a timelocked admin action on `v` (warps to `scheduledEta`).
    function _execAsAdmin(BridgeVault.Action action, bytes memory params) internal {
        _execTimelocked(v, admin, action, params);
    }

    function _execTimelocked(BridgeVault vault, address who, BridgeVault.Action action, bytes memory params) internal {
        vm.prank(who);
        bytes32 id = vault.schedule(action, params);
        vm.warp(vault.scheduledEta(id));
        vm.prank(who);
        vault.execute(action, params);
    }
}
