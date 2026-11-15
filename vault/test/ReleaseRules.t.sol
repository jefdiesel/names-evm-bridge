// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {MockVerifier} from "./mocks/Mocks.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A7: release-delay changes (F#12) under random interleavings.
/// A8: relay-fee menu acceptance (F#13).
/// A10: release retry and replay (R#3).
/// Plus release to the vault or address(0), which is refused (BadRecipient / BadRelayer).
contract ReleaseRulesTest is VaultFixture {
    // a literal, never block.timestamp: via-IR may re-read TIMESTAMP after a warp
    uint256 constant T0 = 1_000_000;

    uint256 internal probeNonce;

    /// Garbage seal, fresh nullifier, fee 0: refused with ReleaseTooEarly while the root waits,
    /// and with BadSeal once every check before the proof passes. Consumes nothing.
    function _probe(bytes32 root, address token) internal returns (bytes4) {
        bytes32 nf = keccak256(abi.encode("probe", ++probeNonce));
        try v.release(hex"00", root, nf, token, 0, bob, address(0), 0) {
            revert("probe cannot succeed");
        } catch (bytes memory err) {
            return bytes4(err);
        }
    }

    function _releasable(bytes32 root, address token) internal returns (bool) {
        bytes4 e = _probe(root, token);
        if (e == BridgeVault.ReleaseTooEarly.selector) return false;
        assertEq(e, MockVerifier.BadSeal.selector, "unexpected probe error");
        return true;
    }

    function _releasable(bytes32 root) internal returns (bool) {
        return _releasable(root, address(usdc));
    }

    // ------------------------------------------------------------------ A7: delay changes

    /// Test-side model of one token's delay history: every effective change, brute force.
    struct DelayModel {
        uint256 current;
        uint256 changedAt;
        uint256 n;
        uint256[24] at;
        uint256[24] delay;
    }

    /// The lowest delay in force at any moment of [t, now]: the one in force just before t (the
    /// last change strictly before t, else the initial delay) and every delay set at or after t.
    /// A change in the same second as the root counts as after it, so the root gets the lower.
    function _modelWait(DelayModel memory m, uint256 t) internal pure returns (uint256 w) {
        w = DELAY;
        for (uint256 i; i < m.n; ++i) if (m.at[i] < t) w = m.delay[i];
        for (uint256 i; i < m.n; ++i) if (m.at[i] >= t && m.delay[i] < w) w = m.delay[i];
    }

    function _pickDelay(uint256 s, uint256 current) internal pure returns (uint64) {
        uint256 k = (s >> 128) % 4;
        if (k == 0) return uint64(current); // a re-set to the same value: a no-op
        if (k == 1) return [uint64(1 hours), 1 days, 3 days, 7 days][(s >> 136) % 4];
        return uint64(bound(s >> 8, 1 hours, 7 days));
    }

    /// F#12, the full property. Random interleavings of delay changes on two tokens (raises,
    /// lowerings and no-op re-sets), new exit roots (USDC exits only: one root is judged per
    /// token, whatever it holds) and warps (zero, a few seconds or up to 3 days, so roots and
    /// changes often share a second). After every step, for every root recorded at t and both
    /// tokens, with m = the lowest delay in force for that token at any moment of [t, now]
    /// (brute force over the model's whole history):
    ///  - the root is releasable now exactly when now >= t + m, never later and never earlier;
    ///  - m >= minReleaseDelay, so nothing is ever released before t + minReleaseDelay;
    ///  - `releasableAt` reports t + m;
    ///  - a root that was releasable stays releasable;
    ///  - if it waits, it is refused at t + m - 1 and accepted at t + m (on a snapshot, no
    ///    further change in between).
    /// The current delay and the time of the last effective change must match the view.
    function testFuzz_DelayChangesNeverReblock(uint256 seed) public {
        uint256 clock = T0;
        vm.warp(clock);
        address[2] memory toks = [address(usdc), address(usdt)];
        DelayModel[2] memory m;
        m[0].current = DELAY;
        m[1].current = DELAY;
        bytes32[24] memory roots;
        uint256[24] memory seen;
        bool[48] memory was;
        uint256 n;
        for (uint256 step; step < 24; ++step) {
            uint256 s = uint256(keccak256(abi.encode(seed, step)));
            uint256 op = s % 6;
            if (op == 0 || n == 0) {
                roots[n] = _batch(1, keccak256(abi.encode(seed, step, "root")));
                seen[n++] = clock;
            } else if (op <= 2) {
                DelayModel memory dm = m[op - 1];
                uint64 d = _pickDelay(s, dm.current);
                vm.prank(admin);
                v.setReleaseDelay(toks[op - 1], d);
                if (d != dm.current) {
                    dm.at[dm.n] = clock;
                    dm.delay[dm.n++] = d;
                    (dm.current, dm.changedAt) = (d, clock);
                }
            } else {
                uint256 k = (s >> 200) % 4;
                clock += k == 0 ? 0 : k == 1 ? bound(s >> 8, 1, 600) : bound(s >> 8, 0, 3 days);
                vm.warp(clock);
            }

            for (uint256 j; j < 2; ++j) {
                (uint64 delay, uint64 changedAt,) = v.releaseDelayState(toks[j]);
                assertEq(delay, m[j].current, "stored delay");
                assertEq(changedAt, m[j].changedAt, "stored change time");
                for (uint256 i; i < n; ++i) _checkRoot(roots[i], toks[j], seen[i], _modelWait(m[j], seen[i]), clock, was, 2 * i + j);
            }
        }
    }

    function _checkRoot(bytes32 r, address token, uint256 t, uint256 w, uint256 clock, bool[48] memory was, uint256 slot)
        internal
    {
        assertGe(w, v.minReleaseDelay(), "model wait below the minimum");
        assertEq(v.releasableAt(token, r), t + w, "releasableAt");
        bool ok = _releasable(r, token);
        assertEq(ok, clock >= t + w, "releasable exactly from t + min delay since t");
        if (was[slot]) assertTrue(ok, "a releasable root became ReleaseTooEarly");
        if (ok) {
            was[slot] = true;
            return;
        }
        uint256 snap = vm.snapshotState();
        vm.warp(t + w - 1);
        assertFalse(_releasable(r, token), "released before t + min delay");
        vm.warp(t + w);
        assertTrue(_releasable(r, token), "not released at t + min delay");
        vm.revertToState(snap);
        vm.warp(clock);
    }

    /// F#12: the sequence the old design got wrong. Delay 1 day; root seen at T0; raise to 7 days
    /// at T0+1h; lower to 3 days at T0+2h. The root was recorded under 1 day and neither change
    /// may lengthen that: it is releasable at T0 + 1 day. A root recorded during the raise waits
    /// min(7 days, 3 days). A later emergency raise applies only to roots recorded after it.
    function test_LaterLoweringNeverLengthensRecordedWait() public {
        vm.warp(T0);
        bytes32 r = _batch(1, "r"); // recorded under 1 day
        vm.warp(T0 + 1 hours);
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 7 days);
        vm.warp(T0 + 1 hours + 1);
        bytes32 r7 = _batch(1, "r7"); // recorded under 7 days
        vm.warp(T0 + 2 hours);
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 3 days); // a lowering
        assertEq(v.releasableAt(address(usdc), r), T0 + DELAY);
        assertEq(v.releasableAt(address(usdc), r7), T0 + 1 hours + 1 + 3 days);

        vm.warp(T0 + DELAY - 1);
        assertFalse(_releasable(r));
        vm.warp(T0 + DELAY);
        assertTrue(_releasable(r), "the lowering must not lengthen the 1-day wait");
        _rel(r, keccak256("nf-r"));
        assertEq(usdc.balanceOf(bob), 99e6);

        vm.warp(T0 + 1 hours + 1 + 3 days - 1);
        assertFalse(_releasable(r7));
        vm.warp(T0 + 1 hours + 1 + 3 days);
        assertTrue(_releasable(r7), "the lowering applies to a root recorded under the raise");

        // emergency raise: roots already recorded keep their wait, later ones wait 30 days
        vm.prank(admin);
        v.setReleaseDelay(address(usdc), 30 days);
        assertTrue(_releasable(r));
        assertTrue(_releasable(r7));
        uint256 t = T0 + 1 hours + 2 + 3 days;
        vm.warp(t);
        bytes32 r30 = _batch(1, "r30");
        assertEq(v.releasableAt(address(usdc), r30), t + 30 days);
        vm.warp(t + 3 days);
        assertFalse(_releasable(r30), "the raise must apply to roots recorded after it");
        vm.warp(t + 30 days);
        assertTrue(_releasable(r30));
    }

    // ------------------------------------------------------------------ A8: relay-fee menu

    /// Up to 4 distinct fees in [1, 2% of denom].
    function _randomMenu(uint256 seed, uint256 denom) internal pure returns (uint256[] memory m) {
        uint256 k = seed % 5;
        m = new uint256[](k);
        uint256 got;
        for (uint256 i; i < k; ++i) {
            uint256 f = 1 + uint256(keccak256(abi.encode(seed, i))) % (denom * 200 / 10_000);
            bool dup;
            for (uint256 j; j < got; ++j) dup = dup || m[j] == f;
            if (!dup) m[got++] = f;
        }
        assembly {
            mstore(m, got)
        }
    }

    function _onMenu(uint256[] memory m, uint256 fee) internal pure returns (bool) {
        for (uint256 i; i < m.length; ++i) if (m[i] == fee) return true;
        return false;
    }

    /// Fee 0 is always accepted; a non-zero fee is accepted iff it is on the current menu for
    /// that token and denomination. Checked against two successive random menus, with fees
    /// drawn from the current menu, the previous menu, the other denomination's menu, and
    /// anywhere in [1, denom]. Accepted releases are real (valid seal) and pay exactly.
    function testFuzz_MenuAcceptance(uint256 seed, uint256 f1, uint256 f2, uint256 f3) public {
        vm.warp(T0);
        // 4 exits of 100 and 2 of 1000
        for (uint256 i; i < 6; ++i) _deposit(i < 4 ? 0 : 1, keccak256(abi.encode("c", i)));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 4);
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, 2);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](6);
        for (uint256 i; i < 6; ++i) ex[i] = BridgeVault.Exit(address(usdc), i < 4 ? 0 : 1, keccak256(abi.encode("e", i)));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(T0 + DELAY);

        uint256[][] memory prev = _menu2(_one(2e6), new uint256[](0)); // the fixture's menu
        uint256[3] memory fs = [f1, f2, f3];
        for (uint256 round; round < 2; ++round) {
            uint256 rs = uint256(keccak256(abi.encode(seed, round)));
            uint256[][] memory menus = _menu2(_randomMenu(rs, 100e6), _randomMenu(rs >> 64, 1000e6));
            vm.prank(admin);
            v.setRelayFeeMenu(address(usdc), menus);
            assertEq(v.relayFeeMenu(address(usdc), 0), menus[0]);
            assertEq(v.relayFeeMenu(address(usdc), 1), menus[1]);
            for (uint256 k; k < 3; ++k) {
                uint256 fseed = uint256(keccak256(abi.encode(fs[k], round)));
                uint32 d = uint32(fseed % 2);
                uint256 denom = d == 0 ? 100e6 : 1000e6;
                uint256 fee;
                uint256 mode = (fseed >> 8) % 5;
                uint256[] memory src = mode == 1 ? menus[d] : mode == 2 ? prev[d] : mode == 3 ? menus[1 - d] : new uint256[](0);
                if (mode == 0) fee = 0;
                else if (src.length != 0) fee = src[(fseed >> 16) % src.length];
                else fee = 1 + (fseed >> 16) % denom;
                bool expectOk = fee == 0 || _onMenu(menus[d], fee);
                assertEq(v.relayFeeAllowed(address(usdc), d, fee), fee != 0 && expectOk);
                _tryRelease(root, d, denom, fee, expectOk, keccak256(abi.encode(seed, round, k)));
            }
            prev = menus;
        }
    }

    function _tryRelease(bytes32 root, uint32 d, uint256 denom, uint256 fee, bool expectOk, bytes32 nf) internal {
        // the 1000 withdraw fee is 10, the 100 one is 1
        uint256 wfee = d == 0 ? 1e6 : 10e6;
        // a bounded fixture: once the burns of a denomination are used up, test acceptance only
        (,, uint64 burns, uint64 rels) = v.counters(address(usdc), d);
        bytes memory seal = _releaseSeal(root, nf, address(usdc), denom, bob, relayer, fee);
        if (!expectOk) {
            try v.release(seal, root, nf, address(usdc), d, bob, relayer, fee) {
                fail();
            } catch (bytes memory err) {
                bytes4 e = bytes4(err);
                // above denom - withdraw fee the defensive RelayFeeTooHigh fires first
                if (fee > denom - wfee) assertEq(e, BridgeVault.RelayFeeTooHigh.selector);
                else assertEq(e, BridgeVault.RelayFeeNotOnMenu.selector);
            }
            return;
        }
        if (rels == burns) {
            vm.expectRevert(BridgeVault.ReleasesExceedBurns.selector); // past the menu and proof checks
            v.release(seal, root, nf, address(usdc), d, bob, relayer, fee);
            return;
        }
        uint256 bobBefore = usdc.balanceOf(bob);
        uint256 relBefore = usdc.balanceOf(relayer);
        v.release(seal, root, nf, address(usdc), d, bob, relayer, fee);
        assertEq(usdc.balanceOf(bob) - bobBefore, denom - wfee - fee);
        assertEq(usdc.balanceOf(relayer) - relBefore, fee);
    }

    /// The menu is per token: a fee listed for USDT's 100 denomination is refused for USDC's
    /// 100 denomination (same amount, same index) until USDC lists it too.
    function test_MenuIsPerToken() public {
        vm.warp(T0);
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(T0 + DELAY);
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdt), _menu2(_one(1.5e6), new uint256[](0)));
        assertTrue(v.relayFeeAllowed(address(usdt), 0, 1.5e6));
        assertFalse(v.relayFeeAllowed(address(usdc), 0, 1.5e6));
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 1.5e6);
        vm.expectRevert(BridgeVault.RelayFeeNotOnMenu.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 1.5e6);
        vm.prank(admin);
        v.setRelayFeeMenu(address(usdc), _menu2(_one(1.5e6), new uint256[](0)));
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 1.5e6);
        assertEq(usdc.balanceOf(relayer), 1.5e6);
    }

    // ------------------------------------------------------------------ A10: retry and replay

    struct Args {
        bytes32 root;
        address token;
        uint32 d;
        uint256 denom;
        address to;
        address rel;
        uint256 fee;
    }

    function _seal(Args memory a, bytes32 nf) internal view returns (bytes memory) {
        return _releaseSeal(a.root, nf, a.token, a.denom, a.to, a.rel, a.fee);
    }

    /// After a nullifier is released, every retry with that nullifier and a valid seal is
    /// refused with NullifierSpent: fee 0, another relayer, another recipient, a later exit
    /// root that still contains the exit, another token, another denomination. Control: each
    /// of those argument sets releases with a fresh nullifier (on a snapshot), so it really is
    /// the nullifier check that refuses them.
    function test_ReleaseReplayVariantsAllNullifierSpent() public {
        vm.warp(T0);
        // USDC: 3 x 100, 1 x 1000; USDT: 1 x 100. Exits: USDC 2 x 100, 1 x 1000; USDT 1 x 100.
        for (uint256 i; i < 3; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        _deposit(1, keccak256("c1000"));
        vm.prank(alice);
        v.deposit(address(usdt), 0, keccak256("t0"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](3);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 3);
        cl[1] = BridgeVault.ClaimDelta(address(usdc), 1, 1);
        cl[2] = BridgeVault.ClaimDelta(address(usdt), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](4);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("e1"));
        ex[2] = BridgeVault.Exit(address(usdc), 1, keccak256("e2"));
        ex[3] = BridgeVault.Exit(address(usdt), 0, keccak256("e3"));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root1 = v.exitRoot();
        vm.warp(T0 + DELAY);

        bytes32 nf = keccak256("the nullifier");
        Args memory first = Args(root1, address(usdc), 0, 100e6, bob, relayer, 2e6);
        v.release(_seal(first, nf), root1, nf, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 97e6);

        // a later root (the tree is append-only, so it still holds the released exit)
        BridgeVault.Exit[] memory ex2 = new BridgeVault.Exit[](1);
        ex2[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e4"));
        _update(_journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), ex2));
        bytes32 root2 = v.exitRoot();
        assertTrue(root2 != root1);
        vm.warp(T0 + 2 * DELAY + 1);

        Args[7] memory tries = [
            Args(root1, address(usdc), 0, 100e6, bob, relayer, 2e6), // the identical submission
            Args(root1, address(usdc), 0, 100e6, bob, address(0), 0), // fee 0, self-submitted
            Args(root1, address(usdc), 0, 100e6, bob, address(0xBEEF), 2e6), // another relayer
            Args(root1, address(usdc), 0, 100e6, address(0xCAFE), relayer, 0), // another recipient
            Args(root2, address(usdc), 0, 100e6, bob, relayer, 0), // a later root
            Args(root2, address(usdt), 0, 100e6, bob, relayer, 0), // another token
            Args(root2, address(usdc), 1, 1000e6, bob, relayer, 0) // another denomination
        ];
        for (uint256 i; i < tries.length; ++i) {
            Args memory a = tries[i];
            bytes memory seal = _seal(a, nf);
            vm.expectRevert(BridgeVault.NullifierSpent.selector);
            v.release(seal, a.root, nf, a.token, a.d, a.to, a.rel, a.fee);

            uint256 snap = vm.snapshotState();
            bytes32 fresh = keccak256(abi.encode("fresh", i));
            v.release(_seal(a, fresh), a.root, fresh, a.token, a.d, a.to, a.rel, a.fee);
            assertTrue(v.releaseNullifierSpent(fresh));
            vm.revertToState(snap);
        }
        // nothing moved: one release only
        (,,, uint64 r0) = v.counters(address(usdc), 0);
        (,,, uint64 r1) = v.counters(address(usdc), 1);
        (,,, uint64 t0) = v.counters(address(usdt), 0);
        assertEq(r0, 1);
        assertEq(r1, 0);
        assertEq(t0, 0);
        assertEq(usdc.balanceOf(bob), 97e6);
        assertEq(usdt.balanceOf(bob), 0);
    }

    /// A failed release (frozen, capped, proof refused, payout refused) leaves the nullifier
    /// unspent, so the same seal works on retry (R#2).
    function test_FailedReleaseKeepsEntitlement() public {
        vm.warp(T0);
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(T0 + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 2e6);
        vm.prank(freezer);
        v.freeze(address(usdc), false, true);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);
        vm.prank(admin);
        v.unfreeze(address(usdc), false, true);
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 50e6);
        vm.expectRevert(BridgeVault.CapExceeded.selector);
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);
        vm.prank(admin);
        v.setCaps(address(usdc), 0, 0, 0);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.release(seal, root, nf, address(usdc), 0, address(0xBAD), relayer, 2e6);
        assertFalse(v.releaseNullifierSpent(nf));
        v.release(seal, root, nf, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 97e6);
    }

    // ------------------------------------------------------------------ release to the vault or address(0)

    /// `release` refuses recipient == the vault or address(0) (BadRecipient), with a valid seal,
    /// before the proof is checked, and a fee-earning relayer == the vault (BadRelayer). Paid to
    /// the vault, an ERC-20 release would leave the funds stranded outside accruedFees and an ETH
    /// one would always fail; address(0) burns ETH. Nothing is consumed, so the nullifier stays
    /// unspent. (The recipient is fixed in the exit commitment, so such an exit can never be
    /// released: the user's own error, which clients must refuse to build.) A relayer == the
    /// vault with fee 0 is paid nothing and is allowed.
    function test_ReleaseToVaultOrZeroRefused() public {
        vm.warp(T0);
        bytes32 root = _bridgeOut(1, 1);
        vm.warp(T0 + DELAY);
        bytes32 nf = keccak256("nf");
        address[2] memory bad = [address(v), address(0)];
        uint256 vaultBefore = usdc.balanceOf(address(v));
        for (uint256 i; i < 2; ++i) {
            bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bad[i], address(0), 0);
            vm.expectRevert(BridgeVault.BadRecipient.selector);
            v.release(seal, root, nf, address(usdc), 0, bad[i], address(0), 0);
            // and with a relay fee
            seal = _releaseSeal(root, nf, address(usdc), 100e6, bad[i], relayer, 2e6);
            vm.expectRevert(BridgeVault.BadRecipient.selector);
            v.release(seal, root, nf, address(usdc), 0, bad[i], relayer, 2e6);
        }
        bytes memory s = _releaseSeal(root, nf, address(usdc), 100e6, bob, address(v), 2e6);
        vm.expectRevert(BridgeVault.BadRelayer.selector);
        v.release(s, root, nf, address(usdc), 0, bob, address(v), 2e6);
        // both are checked before the proof: a garbage seal gets the same errors, not BadSeal
        vm.expectRevert(BridgeVault.BadRecipient.selector);
        v.release(hex"00", root, nf, address(usdc), 0, address(v), address(0), 0);
        vm.expectRevert(BridgeVault.BadRecipient.selector);
        v.release(hex"00", root, nf, address(usdc), 0, address(0), relayer, 2e6);
        vm.expectRevert(BridgeVault.BadRelayer.selector);
        v.release(hex"00", root, nf, address(usdc), 0, bob, address(v), 2e6);
        assertFalse(v.releaseNullifierSpent(nf));
        (,,, uint64 rel) = v.counters(address(usdc), 0);
        assertEq(rel, 0);
        assertEq(usdc.balanceOf(address(v)), vaultBefore);

        // relayer == vault with fee 0: nothing goes to it; the release pays the recipient
        v.release(_releaseSeal(root, nf, address(usdc), 100e6, bob, address(v), 0), root, nf, address(usdc), 0, bob, address(v), 0);
        assertEq(usdc.balanceOf(bob), 99e6);
        (uint64 dep,,, uint64 rel2) = v.counters(address(usdc), 0);
        assertEq(usdc.balanceOf(address(v)), uint256(dep - rel2) * 100e6 + _fees(address(usdc)), "nothing stranded");

        // ETH: the same refusals
        vm.prank(alice);
        v.depositETH{value: 1 ether}(1, keccak256("eth"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(0), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(0), 1, keccak256("eth-exit"));
        _update(_journal(v.g2State(), keccak256("s9"), new bytes32[](0), cl, ex));
        bytes32 r2 = v.exitRoot();
        vm.warp(T0 + 3 * DELAY);
        bytes32 nf2 = keccak256("nf2");
        for (uint256 i; i < 2; ++i) {
            bytes memory seal = _releaseSeal(r2, nf2, address(0), 1 ether, bad[i], address(0), 0);
            vm.expectRevert(BridgeVault.BadRecipient.selector);
            v.release(seal, r2, nf2, address(0), 1, bad[i], address(0), 0);
        }
        s = _releaseSeal(r2, nf2, address(0), 1 ether, bob, address(v), 0.002 ether);
        vm.expectRevert(BridgeVault.BadRelayer.selector);
        v.release(s, r2, nf2, address(0), 1, bob, address(v), 0.002 ether);
        assertFalse(v.releaseNullifierSpent(nf2));
        assertEq(address(v).balance, 1 ether);
        // the same exit to a real recipient goes through
        v.release(_releaseSeal(r2, nf2, address(0), 1 ether, bob, relayer, 0.002 ether), r2, nf2, address(0), 1, bob, relayer, 0.002 ether);
        assertEq(bob.balance, 1 ether - 0.01 ether - 0.002 ether);
    }
}
