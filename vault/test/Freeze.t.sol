// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A9 / F#14: all 16 combinations of (global deposits, global releases, USDC deposits, USDC
/// releases) frozen, against an explicit expected-outcome table for a USDC deposit, a USDC
/// release, `update`, and a USDT deposit and release (the "other token").
contract FreezeMatrixTest is VaultFixture {
    uint256 constant T0 = 1_000_000;

    struct Row {
        // freeze flags
        bool gDep;
        bool gRel;
        bool tDep;
        bool tRel;
        // expected: true = goes through, false = IsFrozen
        bool depositT;
        bool releaseT;
        bool update;
        bool depositOther;
        bool releaseOther;
    }

    /// Written out by hand from the rules (deposit: !gDep && !tDep; release: !gRel && !tRel;
    /// update: !gRel only; other token: the global flag only), not computed from them.
    function _table() internal pure returns (Row[16] memory t) {
        //          gDep   gRel   tDep   tRel  | depT   relT   update depO   relO
        t[0] = Row(false, false, false, false, true, true, true, true, true);
        t[1] = Row(true, false, false, false, false, true, true, false, true);
        t[2] = Row(false, true, false, false, true, false, false, true, false);
        t[3] = Row(true, true, false, false, false, false, false, false, false);
        t[4] = Row(false, false, true, false, false, true, true, true, true);
        t[5] = Row(true, false, true, false, false, true, true, false, true);
        t[6] = Row(false, true, true, false, false, false, false, true, false);
        t[7] = Row(true, true, true, false, false, false, false, false, false);
        t[8] = Row(false, false, false, true, true, false, true, true, true);
        t[9] = Row(true, false, false, true, false, false, true, false, true);
        t[10] = Row(false, true, false, true, true, false, false, true, false);
        t[11] = Row(true, true, false, true, false, false, false, false, false);
        t[12] = Row(false, false, true, true, false, false, true, true, true);
        t[13] = Row(true, false, true, true, false, false, true, false, true);
        t[14] = Row(false, true, true, true, false, false, false, true, false);
        t[15] = Row(true, true, true, true, false, false, false, false, false);
    }

    function _check(bool expectOk, bool ok, bytes memory err, string memory what, uint256 row) internal pure {
        string memory label = string.concat(what, " row ", vm.toString(row));
        assertEq(ok, expectOk, label);
        if (!ok) assertEq(bytes4(err), BridgeVault.IsFrozen.selector, label);
    }

    function test_FreezeMatrix() public {
        vm.warp(T0);
        // releasable exits of USDC and USDT, and a pending (empty) update
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        vm.prank(alice);
        v.deposit(address(usdt), 0, keccak256("t"));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        cl[1] = BridgeVault.ClaimDelta(address(usdt), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](2);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e0"));
        ex[1] = BridgeVault.Exit(address(usdt), 0, keccak256("e1"));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(T0 + DELAY);
        bytes memory j = _journal(v.g2State(), keccak256("s2"), new bytes32[](0), new BridgeVault.ClaimDelta[](0), new BridgeVault.Exit[](0));
        bytes memory jSeal = mv.sealFor(G2, sha256(j));
        bytes32 nfT = keccak256("nfT");
        bytes32 nfO = keccak256("nfO");
        bytes memory sealT = _releaseSeal(root, nfT, address(usdc), 100e6, bob, relayer, 0);
        bytes memory sealO = _releaseSeal(root, nfO, address(usdt), 100e6, bob, relayer, 0);

        Row[16] memory t = _table();
        for (uint256 i; i < 16; ++i) {
            Row memory r = t[i];
            // the row index is the bit pattern gDep | gRel << 1 | tDep << 2 | tRel << 3
            assertEq(i, (r.gDep ? 1 : 0) | (r.gRel ? 2 : 0) | (r.tDep ? 4 : 0) | (r.tRel ? 8 : 0));
            uint256 snap = vm.snapshotState();
            vm.startPrank(freezer);
            v.freezeAll(r.gDep, r.gRel);
            v.freeze(address(usdc), r.tDep, r.tRel);
            vm.stopPrank();
            assertEq(v.globalDepositsFrozen(), r.gDep);
            assertEq(v.globalReleasesFrozen(), r.gRel);
            (, bool df, bool rf,,,,,,) = v.tokenConfig(address(usdc));
            assertEq(df, r.tDep);
            assertEq(rf, r.tRel);

            bool ok;
            bytes memory err;
            vm.prank(alice);
            (ok, err) = address(v).call(abi.encodeCall(BridgeVault.deposit, (address(usdc), 0, keccak256(abi.encode("d", i)))));
            _check(r.depositT, ok, err, "USDC deposit", i);
            (ok, err) = address(v).call(abi.encodeCall(BridgeVault.release, (sealT, root, nfT, address(usdc), 0, bob, relayer, 0)));
            _check(r.releaseT, ok, err, "USDC release", i);
            (ok, err) = address(v).call(abi.encodeCall(BridgeVault.update, (j, jSeal)));
            _check(r.update, ok, err, "update", i);
            vm.prank(alice);
            (ok, err) = address(v).call(abi.encodeCall(BridgeVault.deposit, (address(usdt), 0, keccak256(abi.encode("o", i)))));
            _check(r.depositOther, ok, err, "USDT deposit", i);
            (ok, err) = address(v).call(abi.encodeCall(BridgeVault.release, (sealO, root, nfO, address(usdt), 0, bob, relayer, 0)));
            _check(r.releaseOther, ok, err, "USDT release", i);

            // unfreezing everything restores every operation that was refused
            bytes memory pAll = abi.encode(true, true);
            bytes memory pTok = abi.encode(address(usdc), true, true);
            vm.startPrank(admin);
            bytes32 idAll = v.schedule(BridgeVault.Action.UnfreezeAll, pAll);
            v.schedule(BridgeVault.Action.Unfreeze, pTok);
            vm.warp(v.scheduledEta(idAll));
            v.execute(BridgeVault.Action.UnfreezeAll, pAll);
            v.execute(BridgeVault.Action.Unfreeze, pTok);
            vm.stopPrank();
            vm.startPrank(alice);
            if (!r.depositT) v.deposit(address(usdc), 0, keccak256(abi.encode("d", i)));
            if (!r.depositOther) v.deposit(address(usdt), 0, keccak256(abi.encode("o", i)));
            vm.stopPrank();
            if (!r.releaseT) v.release(sealT, root, nfT, address(usdc), 0, bob, relayer, 0);
            if (!r.releaseOther) v.release(sealO, root, nfO, address(usdt), 0, bob, relayer, 0);
            if (!r.update) v.update(j, jSeal);
            vm.revertToState(snap);
        }
    }
}
