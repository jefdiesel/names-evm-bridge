// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A6 / F#11, S#4: random sequences of (time step, token, denomination, deposit or release)
/// under per-token and global caps. A test-side ledger of every accepted amount checks the
/// documented bound of the two-bucket estimate: the value accepted in any one bucket never
/// exceeds the cap, and in any window-length interval never exceeds 2x the cap. Every
/// CapExceeded rejection changes no state (all relevant views compared before and after),
/// and the refused call goes through unchanged once the window has passed.
contract CapsFuzzTest is VaultFixture {
    uint256 constant T0 = 10_000_000;
    uint256 constant N_OPS = 40;

    struct Entry {
        uint256 time;
        uint256 amount;
    }

    struct Ledger {
        Entry[] e;
        uint256 n;
        uint256 cap;
    }

    struct Views {
        bytes32 tokenCap;
        bytes32 globalCap;
        bytes32 counters;
        uint256 depositCount;
        bytes32 depositRoot;
        uint256 vaultUsdc;
        uint256 vaultUsdt;
        uint256 aliceUsdc;
        uint256 bobUsdc;
        uint256 feesUsdc;
        uint256 feesUsdt;
        bool commitmentUsed;
        bool nullifierSpent;
    }

    function _views(address token, uint32 d, bytes32 c, bytes32 nf) internal view returns (Views memory s) {
        (uint64 w, uint256 dc, uint256 rc, uint256 du, uint256 ru) = v.capState(token);
        s.tokenCap = keccak256(abi.encode(w, dc, rc, du, ru));
        (w, dc, rc, du, ru) = v.globalCapState();
        s.globalCap = keccak256(abi.encode(w, dc, rc, du, ru));
        (uint64 a, uint64 b, uint64 x, uint64 y) = v.counters(token, d);
        s.counters = keccak256(abi.encode(a, b, x, y, v.claimedValue(token), v.burnedValue(token)));
        s.depositCount = v.depositCount();
        s.depositRoot = v.depositRoot();
        s.vaultUsdc = usdc.balanceOf(address(v));
        s.vaultUsdt = usdt.balanceOf(address(v));
        s.aliceUsdc = usdc.balanceOf(alice);
        s.bobUsdc = usdc.balanceOf(bob);
        s.feesUsdc = _fees(address(usdc));
        s.feesUsdt = _fees(address(usdt));
        s.commitmentUsed = v.commitmentUsed(c);
        s.nullifierSpent = v.releaseNullifierSpent(nf);
    }

    function _push(Ledger memory l, uint256 time, uint256 amount) internal pure {
        l.e[l.n++] = Entry(time, amount);
    }

    /// Per bucket <= cap; per window-length interval (checked from every entry's time, which
    /// covers the worst case) <= 2 x cap.
    function _assertBounds(Ledger memory l, uint256 window, string memory what) internal pure {
        for (uint256 i; i < l.n; ++i) {
            uint256 bucket;
            uint256 interval;
            for (uint256 k; k < l.n; ++k) {
                if (l.e[k].time / window == l.e[i].time / window) bucket += l.e[k].amount;
                if (l.e[k].time >= l.e[i].time && l.e[k].time < l.e[i].time + window) interval += l.e[k].amount;
            }
            assertLe(bucket, l.cap, string.concat(what, ": bucket over the cap"));
            assertLe(interval, 2 * l.cap, string.concat(what, ": window over 2x the cap"));
        }
    }

    struct Run {
        uint256 window;
        uint256 clock;
        Ledger usdcDep;
        Ledger usdtDep;
        Ledger usdcRel;
        Ledger globalDep;
        Ledger globalRel;
        uint256 accepted;
        uint256 refused;
        bool retried;
    }

    function testFuzz_CapsBoundAcceptedValue(uint256 seed, uint8 capA, uint8 capB, uint8 capG, bool longWindow) public {
        Run memory r;
        r.clock = T0;
        vm.warp(r.clock);
        // 30 USDC exits of 100 and 6 of 1000, created before the caps (setCaps starts empty)
        for (uint256 i; i < 36; ++i) _deposit(i < 30 ? 0 : 1, keccak256(abi.encode("pre", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 30);
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, 6);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](36);
        for (uint256 i; i < 36; ++i) ex[i] = BridgeVault.Exit(address(usdc), i < 30 ? 0 : 1, keccak256(abi.encode("e", i)));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();

        r.window = longWindow ? 1 days : 1 hours;
        uint256 tokenCap = 100e6 * bound(capA, 1, 25);
        uint256 usdtCap = 100e6 * bound(capB, 1, 25);
        uint256 globalCap = 100e6 * bound(capG, 1, 40);
        vm.startPrank(admin);
        v.setCaps(address(usdc), uint64(r.window), tokenCap, tokenCap);
        v.setCaps(address(usdt), uint64(r.window), usdtCap, usdtCap);
        v.setGlobalCaps(uint64(r.window), globalCap, globalCap);
        v.setCapWeight(address(usdc), 1e18);
        v.setCapWeight(address(usdt), 1e18);
        v.setReleaseDelay(address(usdc), 1 hours);
        vm.stopPrank();
        r.clock += 1 hours; // the exits become releasable
        vm.warp(r.clock);

        r.usdcDep = Ledger(new Entry[](N_OPS), 0, tokenCap);
        r.usdtDep = Ledger(new Entry[](N_OPS), 0, usdtCap);
        r.usdcRel = Ledger(new Entry[](N_OPS), 0, tokenCap);
        r.globalDep = Ledger(new Entry[](N_OPS), 0, globalCap);
        r.globalRel = Ledger(new Entry[](N_OPS), 0, globalCap);

        for (uint256 i; i < N_OPS; ++i) {
            uint256 s = uint256(keccak256(abi.encode(seed, i)));
            r.clock += bound(s >> 8, 0, r.window);
            vm.warp(r.clock);
            uint32 d = (s >> 64) % 4 == 0 ? 1 : 0;
            uint256 amount = d == 0 ? 100e6 : 1000e6;
            uint256 kind = s % 3; // 0 USDC deposit, 1 USDT deposit, 2 USDC release
            address token = kind == 1 ? address(usdt) : address(usdc);
            bytes32 c = keccak256(abi.encode("op", i));
            bytes memory call_;
            if (kind == 2) {
                call_ = abi.encodeCall(
                    BridgeVault.release,
                    (_releaseSeal(root, c, address(usdc), amount, bob, relayer, 0), root, c, address(usdc), d, bob, relayer, 0)
                );
            } else {
                call_ = abi.encodeCall(BridgeVault.deposit, (token, d, c));
            }
            Views memory before = _views(token, d, c, c);
            vm.prank(alice);
            (bool ok, bytes memory err) = address(v).call(call_);
            if (ok) {
                r.accepted++;
                if (kind == 0) _push(r.usdcDep, r.clock, amount);
                if (kind == 1) _push(r.usdtDep, r.clock, amount);
                if (kind == 2) _push(r.usdcRel, r.clock, amount);
                if (kind == 2) _push(r.globalRel, r.clock, amount);
                else _push(r.globalDep, r.clock, amount);
                continue;
            }
            if (kind == 2 && bytes4(err) == BridgeVault.ReleasesExceedBurns.selector) continue; // burns used up
            assertEq(bytes4(err), BridgeVault.CapExceeded.selector, "only caps may refuse here");
            r.refused++;
            assertEq(keccak256(abi.encode(_views(token, d, c, c))), keccak256(abi.encode(before)), "a CapExceeded call changed state");
            // the same call goes through unchanged once usage has aged out (if the amount fits)
            if (!r.retried && amount <= globalCap && amount <= (kind == 1 ? usdtCap : tokenCap)) {
                r.retried = true;
                uint256 snap = vm.snapshotState();
                vm.warp(r.clock + 2 * r.window);
                vm.prank(alice);
                (ok,) = address(v).call(call_);
                assertTrue(ok, "refused call did not go through later");
                vm.revertToState(snap);
            }
        }
        _assertBounds(r.usdcDep, r.window, "USDC deposits");
        _assertBounds(r.usdtDep, r.window, "USDT deposits");
        _assertBounds(r.usdcRel, r.window, "USDC releases");
        _assertBounds(r.globalDep, r.window, "global deposits");
        _assertBounds(r.globalRel, r.window, "global releases");
        assertGt(r.accepted, 0);
    }

    /// Pinned example of why the bound is 2x and not 1x: with a cap of 200 per day, 200 at the
    /// last second of one bucket and 100 more twelve hours later (the old bucket then weighs
    /// half) are both accepted, so one window-length interval holds 300 = 1.5x the cap. The
    /// current bucket alone never exceeds the cap: near the end of the window, with the old
    /// bucket weighing almost nothing, a further 100 is still refused because cur is 100.
    function test_RollingEstimateAdmitsAboveCapPerWindowBelowTwice() public {
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, 200e6, type(uint256).max);
        vm.warp(10 days - 1); // last second of bucket 9
        _deposit(0, keccak256("a1"));
        _deposit(0, keccak256("a2"));
        vm.warp(10 days + 12 hours); // 12 h + 1 s later: the old 200 weighs ~100
        _deposit(0, keccak256("b1"));
        _depExpect(address(usdc), keccak256("b2"), BridgeVault.CapExceeded.selector);
        vm.warp(11 days - 2); // still inside the window that started with a1
        _depExpect(address(usdc), keccak256("b2"), BridgeVault.CapExceeded.selector);
        (,,, uint256 used,) = v.capState(address(usdc));
        assertEq(used, 100e6 + uint256(200e6) * 2 / 1 days);
        // 300 accepted within [10d - 1, 11d - 1)
        assertEq(usdc.balanceOf(address(v)), 3 * 101e6);
    }
}
