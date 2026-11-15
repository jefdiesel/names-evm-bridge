// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken, NoReturnToken} from "./mocks/Mocks.sol";

/// Gas per operation (P#3). Verification cost is the mock's; add the RISC Zero Groth16
/// verifier's cost (measured separately) to update and release. `GasTest` runs with
/// per-token and global caps enabled on every token; `GasTestNoCaps` with all caps off.
abstract contract GasBase is Test {
    BridgeVault v;
    MockVerifier mv;
    TestToken usdc;
    NoReturnToken usdt;
    address alice = address(0xA1);
    bytes32 constant G2 = keccak256("g2");
    bytes32 constant G3 = keccak256("g3");

    function setUp() public {
        mv = new MockVerifier();
        v = new BridgeVault(IRiscZeroVerifier(address(mv)), G2, G3, bytes32(0), bytes32(0), keccak256("program"), bytes32(0), 1 hours, address(this), address(this), address(this));
        usdc = new TestToken("USDC", 6);
        usdt = new NoReturnToken();
        uint256[] memory d = new uint256[](1);
        d[0] = 100e6;
        uint256[] memory f = new uint256[](1);
        f[0] = 1e6;
        v.registerToken(address(usdc), 6, d, f, f, 1 hours);
        v.registerToken(address(usdt), 6, d, f, f, 1 hours);
        uint256[] memory de = new uint256[](1);
        de[0] = 1 ether;
        uint256[] memory fe = new uint256[](1);
        v.registerToken(address(0), 18, de, fe, fe, 1 hours);
        // the 1 USDC relay fee the release measurement pays must be on the menu (F#13)
        uint256[][] memory menu = new uint256[][](1);
        menu[0] = f;
        v.setRelayFeeMenu(address(usdc), menu);
        usdc.mint(alice, type(uint128).max);
        usdt.mint(alice, type(uint128).max);
        vm.deal(alice, 1_000_000 ether);
        vm.startPrank(alice);
        usdc.approve(address(v), type(uint256).max);
        usdt.approve(address(v), type(uint256).max);
        vm.stopPrank();
        if (capsOn()) {
            // high enough never to bind here; every check and bucket write still runs
            v.setCaps(address(usdc), 1 days, 1e30, 1e30);
            v.setCaps(address(usdt), 1 days, 1e30, 1e30);
            v.setCaps(address(0), 1 days, 1e30, 1e30);
            v.setGlobalCaps(1 days, 1e30, 1e30);
            v.setCapWeight(address(usdc), 1e18);
            v.setCapWeight(address(usdt), 1e18);
            v.setCapWeight(address(0), 3e9);
        }
        console.log("caps enabled (per-token and global): %s", capsOn());
    }

    function capsOn() internal pure virtual returns (bool);

    function _dep(address t, uint256 i) internal returns (uint256 g) {
        vm.prank(alice);
        uint256 g0 = gasleft();
        if (t == address(0)) v.depositETH{value: 1 ether}(0, keccak256(abi.encode(i)));
        else v.deposit(t, 0, keccak256(abi.encode(i)));
        g = g0 - gasleft();
    }

    function test_GasDeposits() public {
        uint256 g;
        for (uint256 i; i < 300; ++i) {
            g = _dep(address(usdc), i);
            if (i == 0 || i == 1 || i == 2 || i == 3 || i == 127 || i == 255 || i == 299) {
                console.log("ERC20 deposit #%d gas %d", i, g);
            }
        }
        // commitments are unique across tokens, so these take fresh ones
        console.log("USDT-style deposit gas %d", _dep(address(usdt), 1001));
        console.log("USDT-style deposit gas %d", _dep(address(usdt), 1002));
        console.log("ETH deposit gas %d", _dep(address(0), 1003));
        console.log("ETH deposit gas %d", _dep(address(0), 1004));
        // first deposit after the cap buckets roll over (bucket slots rewritten)
        vm.warp(block.timestamp + 2 days);
        console.log("ERC20 deposit, first in a new cap bucket, gas %d", _dep(address(usdc), 2001));
        console.log("ERC20 deposit, same bucket, gas %d", _dep(address(usdc), 2002));
    }

    function test_GasUpdateAndRelease() public {
        for (uint256 i; i < 64; ++i) _dep(address(usdc), i);
        bytes32 cfg = v.cfgId();
        for (uint256 n = 1; n <= 32; n *= 2) {
            BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
            cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
            bytes32[] memory roots = new bytes32[](1);
            roots[0] = v.depositRoot();
            BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n);
            for (uint256 i; i < n; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode(n, i)));
            bytes memory j = abi.encode(BridgeVault.UpdateJournal(cfg, v.g2State(), keccak256(abi.encode(n)), roots, cl, ex));
            bytes memory seal = mv.sealFor(G2, sha256(j));
            uint256 g0 = gasleft();
            v.update(j, seal);
            uint256 g = g0 - gasleft();
            console.log("update with %d exits: gas %d (%d per exit)", n, g, g / n);
        }
        bytes32 root = v.exitRoot();
        vm.warp(block.timestamp + 1 hours);
        for (uint256 i; i < 2; ++i) {
            // (no vm.cool here, so these numbers stay comparable with the earlier runs)
            bytes32 nf = keccak256(abi.encode("nf", i));
            bytes memory s = mv.sealFor(G3, sha256(abi.encode(cfg, root, nf, address(usdc), uint256(100e6), address(0xB0B), address(0x7E1A), uint256(1e6))));
            uint256 g0 = gasleft();
            v.release(s, root, nf, address(usdc), 0, address(0xB0B), address(0x7E1A), 1e6);
            console.log("release (ERC20, relayer paid) gas %d", g0 - gasleft());
        }
    }

    /// One root with 3 USDC exits recorded at t0, then `n` raises (each pushes one history step).
    function _rootThenRaises(uint256 n, uint256 t0) internal returns (bytes32 root) {
        vm.warp(t0);
        root = _exitBatch(t0, 3);
        for (uint256 i; i < n; ++i) {
            vm.warp(t0 + 1 + i);
            v.setReleaseDelay(address(usdc), uint64(2 hours + i * 1 hours));
        }
    }

    function _exitBatch(uint256 tag, uint256 n) internal returns (bytes32) {
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n);
        for (uint256 i; i < n; ++i) {
            _dep(address(usdc), tag + i);
            ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode("e", tag, i)));
        }
        bytes memory j = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), v.g2State(), keccak256(abi.encode(tag)), new bytes32[](0), cl, ex));
        v.update(j, mv.sealFor(G2, sha256(j)));
        return v.exitRoot();
    }

    /// Release gas, all storage cold (vm.cool on the vault and the token), as one call.
    function _coldRelease(bytes32 root, bytes32 nf) internal returns (uint256 g) {
        bytes memory s = mv.sealFor(G3, sha256(abi.encode(v.cfgId(), root, nf, address(usdc), uint256(100e6), address(0xB0B), address(0x7E1A), uint256(1e6))));
        vm.cool(address(v));
        vm.cool(address(usdc));
        uint256 g0 = gasleft();
        v.release(s, root, nf, address(usdc), 0, address(0xB0B), address(0x7E1A), 1e6);
        g = g0 - gasleft();
    }

    /// F#12: release gas against the token's delay history (0, 1, 8, 32 raises, the only kind
    /// of change that grows it). Every storage slot cold, as in a real transaction, and never
    /// the first release of the denomination (so counter writes are the steady-state cost; the
    /// 21,000 base and calldata are not included). Slow path: a root recorded before every raise, released at its
    /// original 1-hour delay, so the binary search runs. Fast path: a root recorded after the
    /// last change, released at the current delay (no history read). Also the cost of one more
    /// raise (O(1)) and of a lowering that drops every step but the first (binary search).
    function test_GasReleaseVsDelayHistory() public {
        uint256[4] memory ns = [uint256(0), 1, 8, 32];
        for (uint256 k; k < ns.length; ++k) {
            uint256 n = ns[k];
            uint256 snap = vm.snapshotState();
            uint256 t0 = 1_000_000;
            bytes32 oldRoot = _rootThenRaises(n, t0);
            (uint64 cur,, uint256 steps) = v.releaseDelayState(address(usdc));
            require(steps == n, "history length");
            vm.warp(t0 + 1 hours);
            // a first release (not measured), so both measured ones update non-zero counters
            _coldRelease(oldRoot, keccak256(abi.encode("warm-up", n)));
            uint256 slow = _coldRelease(oldRoot, keccak256(abi.encode("slow", n)));
            bytes32 newRoot = _exitBatch(7000 + 10 * n, 1);
            vm.warp(t0 + 1 hours + cur);
            uint256 fast = _coldRelease(newRoot, keccak256(abi.encode("fast", n)));
            vm.cool(address(v));
            uint256 g0 = gasleft();
            v.setReleaseDelay(address(usdc), cur + 1 hours);
            uint256 raise = g0 - gasleft();
            vm.cool(address(v));
            g0 = gasleft();
            v.setReleaseDelay(address(usdc), 1 hours + 30 minutes);
            uint256 lower = g0 - gasleft();
            console.log("delay history %d steps: release (old root, search) gas %d", n, slow);
            console.log("  release (root after last change) gas %d", fast);
            console.log("  setReleaseDelay raise gas %d, lowering to 90 min gas %d", raise, lower);
            vm.revertToState(snap);
        }
    }
}

contract GasTest is GasBase {
    function capsOn() internal pure override returns (bool) {
        return true;
    }
}

contract GasTestNoCaps is GasBase {
    function capsOn() internal pure override returns (bool) {
        return false;
    }
}
