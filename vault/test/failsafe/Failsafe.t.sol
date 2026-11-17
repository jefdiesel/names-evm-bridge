// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {BridgeVaultFailsafe as V} from "../../src/failsafe/BridgeVaultFailsafe.sol";
import {MockVerifier, TestToken} from "../mocks/Mocks.sol";

/// Shared setup: a failsafe vault with the MockVerifier (a seal is imageId | sha256(journal)), so
/// any journal can be "proved" here: these tests are about what the VAULT accepts and pays,
/// whatever a proof says. What the G4 guest itself refuses (a claimed deposit, a wrong secret, a
/// deposit not in the tree) is tested in lez-bridge/reclaim/host/tests/reclaim.rs, and its real
/// journals are replayed in ReclaimVector.t.sol.
contract FailsafeBase is Test {
    bytes32 constant G2 = keccak256("g2");
    bytes32 constant G3 = keccak256("g3");
    bytes32 constant G4 = keccak256("g4");
    bytes32 constant STATE0 = keccak256("state0");
    address constant ETH = address(0);

    V v;
    MockVerifier mv;
    TestToken usdc;
    address admin = makeAddr("admin");
    address freezer = makeAddr("freezer");
    address feeTo = makeAddr("feeTo");
    address alice = makeAddr("alice");
    address relayer = makeAddr("relayer");
    uint256 stateNonce;

    function setUp() public virtual {
        mv = new MockVerifier();
        v = new V(mv, G2, G3, G4, keccak256("lez"), keccak256("zone"), keccak256("prog"), STATE0, 0, admin, freezer, feeTo);
        usdc = new TestToken("USDC", 6);
        uint256[] memory d = new uint256[](2);
        (d[0], d[1]) = (100e6, 1000e6);
        uint256[] memory df = new uint256[](2);
        (df[0], df[1]) = (1e6, 10e6);
        uint256[] memory wf = new uint256[](2);
        (wf[0], wf[1]) = (0.5e6, 5e6);
        uint256[] memory ed = new uint256[](1);
        ed[0] = 1 ether;
        uint256[] memory z1 = new uint256[](1);
        vm.startPrank(admin);
        v.registerToken(address(usdc), 6, d, df, wf, 0);
        v.registerToken(ETH, 18, ed, z1, z1, 0);
        uint256[][] memory menu = new uint256[][](2);
        menu[0] = new uint256[](1);
        menu[0][0] = 1e6;
        menu[1] = new uint256[](0);
        v.setRelayFeeMenu(address(usdc), menu);
        vm.stopPrank();
        usdc.mint(alice, 1e15);
        vm.prank(alice);
        usdc.approve(address(v), type(uint256).max);
        vm.deal(alice, 100 ether);
    }

    function _deposit(uint32 di, bytes32 c) internal {
        vm.prank(alice);
        v.deposit(address(usdc), di, c);
    }

    function _depositETH(bytes32 c) internal {
        vm.prank(alice);
        v.depositETH{value: 1 ether}(0, c);
    }

    function _journal(V.ClaimDelta[] memory cl, V.Exit[] memory ex) internal returns (bytes memory j) {
        bytes32 next = keccak256(abi.encode("state", ++stateNonce));
        V.UpdateJournal memory u = V.UpdateJournal(v.cfgId(), v.g2State(), next, new bytes32[](0), cl, ex);
        j = abi.encode(u);
    }

    function _update(V.ClaimDelta[] memory cl, V.Exit[] memory ex) internal returns (bytes32 newState) {
        bytes memory j = _journal(cl, ex);
        v.update(j, mv.sealFor(G2, sha256(j)));
        return v.g2State();
    }

    function _claims(address t, uint32 di, uint64 n) internal pure returns (V.ClaimDelta[] memory cl) {
        cl = new V.ClaimDelta[](1);
        cl[0] = V.ClaimDelta(t, di, n);
    }

    function _none() internal pure returns (V.ClaimDelta[] memory) {
        return new V.ClaimDelta[](0);
    }

    function _noExits() internal pure returns (V.Exit[] memory) {
        return new V.Exit[](0);
    }

    function _exit(address t, uint32 di, bytes32 e) internal pure returns (V.Exit[] memory ex) {
        ex = new V.Exit[](1);
        ex[0] = V.Exit(t, di, e);
    }

    /// The seal the MockVerifier accepts for a reclaim journal against `fs`.
    function _seal(bytes32 fs, bytes32 root, bytes32 nf, address t, uint256 denom, address to, address rel, uint256 fee)
        internal
        view
        returns (bytes memory)
    {
        return mv.sealFor(G4, sha256(abi.encode(v.cfgId(), root, fs, nf, t, denom, to, rel, fee)));
    }

    function _reclaim(bytes32 root, bytes32 nf, uint32 di, uint256 denom, address to) internal {
        v.reclaim(_seal(v.finalState(), root, nf, address(usdc), denom, to, address(0), 0), root, nf, address(usdc), di, to, address(0), 0);
    }

    /// Three USDC 100 deposits, one claimed on LEZ, wind-down, the final update.
    function _threeDepositsOneClaimedFinal() internal returns (bytes32 root) {
        for (uint256 i; i < 3; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        root = v.depositRoot();
        _update(_claims(address(usdc), 0, 1), _noExits());
        vm.prank(admin);
        v.windDown();
        _update(_none(), _noExits());
        assertTrue(v.finalState() != bytes32(0), "final fixed");
    }

    function _principal() internal view returns (uint256 p) {
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        p = usdc.balanceOf(address(v)) - fees;
    }
}

contract FailsafeTest is FailsafeBase {
    function test_ReclaimOfAnUnclaimedDepositPaysOnce() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        address r1 = makeAddr("fresh1");
        uint256 before = usdc.balanceOf(address(v));
        _reclaim(root, keccak256("nf0"), 0, 100e6, r1);
        assertEq(usdc.balanceOf(r1), 100e6, "the full denomination, no withdraw fee");
        assertEq(usdc.balanceOf(address(v)), before - 100e6);
        assertEq(v.reclaims(address(usdc), 0), 1);
        assertTrue(v.reclaimNullifierSpent(keccak256("nf0")));
        // the same deposit again (same reclaim nullifier), even to another address: refused
        address r2 = makeAddr("fresh2");
        bytes memory s = _seal(v.finalState(), root, keccak256("nf0"), address(usdc), 100e6, r2, address(0), 0);
        vm.expectRevert(V.NullifierSpent.selector);
        v.reclaim(s, root, keccak256("nf0"), address(usdc), 0, r2, address(0), 0);
        assertEq(usdc.balanceOf(r2), 0);
    }

    function test_ReclaimRefusedBeforeWindDownAndBeforeFinal() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32 root = v.depositRoot();
        address r = makeAddr("r");
        // (seals are built before expectRevert: building one calls the vault and the mock)
        bytes memory s0 = _seal(bytes32(0), root, keccak256("nf"), address(usdc), 100e6, r, address(0), 0);
        vm.expectRevert(V.NotWoundDown.selector);
        v.reclaim(s0, root, keccak256("nf"), address(usdc), 0, r, address(0), 0);
        vm.prank(admin);
        v.windDown();
        vm.expectRevert(V.NotFinal.selector);
        v.reclaim(s0, root, keccak256("nf"), address(usdc), 0, r, address(0), 0);
        // the next update fixes the final; a proof made against the state before it is refused
        bytes32 before = v.g2State();
        assertEq(v.windDownState(), before);
        bytes32 fin = _update(_none(), _noExits());
        assertEq(v.finalState(), fin);
        assertEq(v.finalAt(), block.timestamp);
        bytes memory sOld = _seal(before, root, keccak256("nf"), address(usdc), 100e6, r, address(0), 0);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(sOld, root, keccak256("nf"), address(usdc), 0, r, address(0), 0);
        _reclaim(root, keccak256("nf"), 0, 100e6, r);
        assertEq(usdc.balanceOf(r), 100e6);
    }

    function test_DepositsRefusedForGoodAfterWindDown() public {
        _deposit(0, keccak256("c0"));
        vm.prank(admin);
        v.windDown();
        vm.expectRevert(V.IsWoundDown.selector);
        _deposit(0, keccak256("c1"));
        vm.expectRevert(V.IsWoundDown.selector);
        _depositETH(keccak256("c2"));
        // nothing the admin can do reopens them
        vm.startPrank(admin);
        v.unfreezeAll(true, true);
        v.unfreeze(address(usdc), true, true);
        vm.expectRevert(V.AlreadyWoundDown.selector);
        v.windDown();
        vm.stopPrank();
        vm.expectRevert(V.IsWoundDown.selector);
        _deposit(0, keccak256("c3"));
    }

    function test_OnlyTheAdminWindsDown() public {
        vm.prank(freezer);
        vm.expectRevert(V.NotAdmin.selector);
        v.windDown();
        vm.prank(alice);
        vm.expectRevert(V.NotAdmin.selector);
        v.windDown();
        assertEq(v.windDownAt(), 0);
    }

    function test_ClaimedDepositsCannotBeReclaimedPastTheCount() public {
        // 3 deposits, 2 claimed: one reclaim fits, a second (a forged proof the mock accepts) does not
        for (uint256 i; i < 3; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32 root = v.depositRoot();
        _update(_claims(address(usdc), 0, 2), _noExits());
        vm.prank(admin);
        v.windDown();
        _update(_none(), _noExits());
        _reclaim(root, keccak256("nfA"), 0, 100e6, makeAddr("a"));
        bytes32 fs = v.finalState();
        address b = makeAddr("b");
        bytes memory s = _seal(fs, root, keccak256("nfB"), address(usdc), 100e6, b, address(0), 0);
        vm.expectRevert(V.ReclaimsExceedDeposits.selector);
        v.reclaim(s, root, keccak256("nfB"), address(usdc), 0, b, address(0), 0);
        assertEq(usdc.balanceOf(b), 0);
    }

    function test_ARelayerCannotChangePayeeFeeOrAmount() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        bytes32 fs = v.finalState();
        address to = makeAddr("to");
        bytes32 nf = keccak256("nf");
        bytes memory s = _seal(fs, root, nf, address(usdc), 100e6, to, relayer, 1e6);
        address thief = makeAddr("thief");
        vm.startPrank(relayer);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(s, root, nf, address(usdc), 0, thief, relayer, 1e6); // payee
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(s, root, nf, address(usdc), 0, to, thief, 1e6); // fee to someone else
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(s, root, nf, address(usdc), 0, to, relayer, 0); // another fee
        vm.expectRevert(V.RelayFeeNotOnMenu.selector);
        v.reclaim(s, root, nf, address(usdc), 1, to, relayer, 1e6); // another denomination (no fee menu there)
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(s, root, nf, address(usdc), 1, to, address(0), 0); // another denomination, no fee
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(s, root, keccak256("other"), address(usdc), 0, to, relayer, 1e6); // another nullifier
        // as made: pays exactly the denomination, split as the proof says
        v.reclaim(s, root, nf, address(usdc), 0, to, relayer, 1e6);
        vm.stopPrank();
        assertEq(usdc.balanceOf(to), 99e6);
        assertEq(usdc.balanceOf(relayer), 1e6);
        assertEq(usdc.balanceOf(thief), 0);
    }

    function test_RelayChecks() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        bytes32 fs = v.finalState();
        address to = makeAddr("to");
        bytes32 nf = keccak256("nf");
        address u = address(usdc);
        bytes memory s1 = _seal(fs, root, nf, u, 100e6, to, relayer, 2e6);
        bytes memory s2 = _seal(fs, root, nf, u, 100e6, to, address(0), 1e6);
        bytes memory s3 = _seal(fs, root, nf, u, 100e6, to, address(v), 1e6);
        bytes memory s4 = _seal(fs, root, nf, u, 100e6, address(0), address(0), 0);
        bytes memory s5 = _seal(fs, root, nf, u, 100e6, address(v), address(0), 0);
        bytes memory s6 = _seal(fs, root, nf, u, 100e6, to, relayer, 101e6);
        bytes memory s7 = _seal(fs, root, nf, u, 100e6, to, address(0), 0);
        vm.expectRevert(V.RelayFeeNotOnMenu.selector);
        v.reclaim(s1, root, nf, u, 0, to, relayer, 2e6);
        vm.expectRevert(V.ZeroRelayer.selector);
        v.reclaim(s2, root, nf, u, 0, to, address(0), 1e6);
        vm.expectRevert(V.BadRelayer.selector);
        v.reclaim(s3, root, nf, u, 0, to, address(v), 1e6);
        vm.expectRevert(V.BadRecipient.selector);
        v.reclaim(s4, root, nf, u, 0, address(0), address(0), 0);
        vm.expectRevert(V.BadRecipient.selector);
        v.reclaim(s5, root, nf, u, 0, address(v), address(0), 0);
        vm.expectRevert(V.RelayFeeTooHigh.selector);
        v.reclaim(s6, root, nf, u, 0, to, relayer, 101e6);
        vm.expectRevert(V.BadDenomination.selector);
        v.reclaim(s7, root, nf, u, 2, to, address(0), 0);
        // nothing was spent by the refusals
        v.reclaim(s7, root, nf, u, 0, to, address(0), 0);
        assertEq(usdc.balanceOf(to), 100e6);
    }

    function test_UnknownDepositRootAndOtherFinalStateRefused() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        address to = makeAddr("to");
        bytes32 bogus = keccak256("not a root");
        bytes memory sb = _seal(v.finalState(), bogus, keccak256("nf"), address(usdc), 100e6, to, address(0), 0);
        vm.expectRevert(V.UnknownDepositRoot.selector);
        v.reclaim(sb, bogus, keccak256("nf"), address(usdc), 0, to, address(0), 0);
        // a proof against any other G2 state (an older accumulator, before a claim landed)
        bytes memory so = _seal(STATE0, root, keccak256("nf"), address(usdc), 100e6, to, address(0), 0);
        vm.expectRevert(MockVerifier.BadSeal.selector);
        v.reclaim(so, root, keccak256("nf"), address(usdc), 0, to, address(0), 0);
    }

    function test_ClaimsAfterTheFinalCountNowhere() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        bytes32 fs = v.finalState();
        (, uint64 claimsBefore,,) = v.counters(address(usdc), 0);
        uint256 cvBefore = v.claimedValue(address(usdc));
        vm.expectEmit(true, false, false, true, address(v));
        emit V.ClaimsIgnored(address(usdc), 0, 2);
        _update(_claims(address(usdc), 0, 2), _noExits());
        (, uint64 claimsAfter,,) = v.counters(address(usdc), 0);
        assertEq(claimsAfter, claimsBefore, "claims frozen at the final");
        assertEq(v.claimedValue(address(usdc)), cvBefore);
        assertEq(v.finalState(), fs, "a later update never moves the final");
        // so both unclaimed deposits still reclaim
        _reclaim(root, keccak256("nf0"), 0, 100e6, makeAddr("x"));
        _reclaim(root, keccak256("nf2"), 0, 100e6, makeAddr("y"));
        assertEq(_principal(), 100e6, "only the claimed deposit stays, backing its wrapped tokens");
    }

    function test_OverMintRevertsWhileActiveClampsAfterWindDown() public {
        for (uint256 i; i < 2; ++i) _deposit(0, keccak256(abi.encode("c", i)));
        bytes32 root = v.depositRoot();
        bytes memory j = _journal(_claims(address(usdc), 0, 5), _noExits());
        bytes memory s = mv.sealFor(G2, sha256(j));
        vm.expectRevert(V.ClaimsExceedDeposits.selector);
        v.update(j, s);
        vm.prank(admin);
        v.windDown();
        // the same batch now lands (clamped) and is the final
        vm.expectEmit(true, false, false, true, address(v));
        emit V.ClaimsClamped(address(usdc), 0, 3);
        v.update(j, s);
        (uint64 dep, uint64 cl,,) = v.counters(address(usdc), 0);
        assertEq(cl, dep);
        assertEq(v.claimedValue(address(usdc)), 200e6);
        // forged mints used up the room: no reclaim fits (the reclaimers bear it, the vault stays solvent)
        address to = makeAddr("to");
        bytes memory rs = _seal(v.finalState(), root, keccak256("nf"), address(usdc), 100e6, to, address(0), 0);
        vm.expectRevert(V.ReclaimsExceedDeposits.selector);
        v.reclaim(rs, root, keccak256("nf"), address(usdc), 0, to, address(0), 0);
    }

    function test_UnbackedExitDroppedAfterWindDown() public {
        _deposit(0, keccak256("c0"));
        _update(_claims(address(usdc), 0, 1), _noExits());
        // active: an exit above claims reverts the update
        bytes memory j = _journal(_none(), _exit(address(usdc), 1, keccak256("e1000")));
        bytes memory s = mv.sealFor(G2, sha256(j));
        vm.expectRevert(V.BurnsExceedClaims.selector);
        v.update(j, s);
        vm.prank(admin);
        v.windDown();
        vm.expectEmit(true, false, false, true, address(v));
        emit V.ExitUnbacked(address(usdc), 1, keccak256("e1000"));
        v.update(j, s);
        assertEq(v.exitCount(), 0, "not inserted");
        (,, uint64 burns,) = v.counters(address(usdc), 1);
        assertEq(burns, 0, "not counted");
        // a backed exit after the final still goes in (holders can still exit if outbound is sound)
        _update(_none(), _exit(address(usdc), 0, keccak256("e100")));
        assertEq(v.exitCount(), 1);
    }

    function test_FinalLandsUnderTheGlobalReleaseFreeze() public {
        _deposit(0, keccak256("c0"));
        vm.prank(freezer);
        v.freezeAll(true, true);
        bytes memory j = _journal(_none(), _noExits());
        bytes memory s = mv.sealFor(G2, sha256(j));
        vm.expectRevert(V.IsFrozen.selector);
        v.update(j, s);
        vm.prank(admin);
        v.windDown();
        v.update(j, s);
        assertTrue(v.finalState() != bytes32(0));
    }

    function test_ReclaimFreezeIsLivenessOnly() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        vm.prank(freezer);
        v.freezeReclaims();
        address to = makeAddr("to");
        bytes memory s = _seal(v.finalState(), root, keccak256("nf"), address(usdc), 100e6, to, address(0), 0);
        vm.expectRevert(V.IsFrozen.selector);
        v.reclaim(s, root, keccak256("nf"), address(usdc), 0, to, address(0), 0);
        vm.prank(freezer);
        vm.expectRevert(V.NotAdmin.selector);
        v.unfreezeReclaims();
        vm.prank(admin);
        v.unfreezeReclaims();
        v.reclaim(s, root, keccak256("nf"), address(usdc), 0, to, address(0), 0);
        assertEq(usdc.balanceOf(to), 100e6, "the depositor's address, not the freezer's or the admin's");
    }

    function test_ReclaimETH() public {
        _depositETH(keccak256("e0"));
        bytes32 root = v.depositRoot();
        vm.prank(admin);
        v.windDown();
        _update(_none(), _noExits());
        address payable to = payable(makeAddr("ethTo"));
        bytes32 nf = keccak256("nfe");
        v.reclaim(_seal(v.finalState(), root, nf, ETH, 1 ether, to, address(0), 0), root, nf, ETH, 0, to, address(0), 0);
        assertEq(to.balance, 1 ether);
        assertEq(address(v).balance, 0);
    }

    function test_ReclaimCountsAgainstReleaseCaps() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 150e6);
        _reclaim(root, keccak256("nf0"), 0, 100e6, makeAddr("a"));
        address b = makeAddr("b");
        bytes memory s = _seal(v.finalState(), root, keccak256("nf2"), address(usdc), 100e6, b, address(0), 0);
        vm.expectRevert(V.CapExceeded.selector);
        v.reclaim(s, root, keccak256("nf2"), address(usdc), 0, b, address(0), 0);
        vm.warp(block.timestamp + 2 days);
        v.reclaim(s, root, keccak256("nf2"), address(usdc), 0, b, address(0), 0);
        assertEq(usdc.balanceOf(b), 100e6);
    }
}

/// Nothing gives the admin or the freezer a way to pay out, and nothing undoes the wind-down.
contract FailsafeAdminTest is FailsafeBase {
    /// 0 admin, 1 freezer or admin, 2 pending admin, 3 anyone. BridgeVault's 22 plus the four
    /// failsafe functions; the ABI walk below fails on any function not listed here.
    function _stateChanging() internal pure returns (string[] memory s, uint8[] memory rule) {
        s = new string[](26);
        rule = new uint8[](26);
        (s[0], rule[0]) = ("registerToken(address,uint8,uint256[],uint256[],uint256[],uint64)", 0);
        (s[1], rule[1]) = ("deregisterToken(address)", 0);
        (s[2], rule[2]) = ("addDenomination(address,uint256,uint256,uint256)", 0);
        (s[3], rule[3]) = ("setFees(address,uint256[],uint256[])", 0);
        (s[4], rule[4]) = ("setRelayFeeMenu(address,uint256[][])", 0);
        (s[5], rule[5]) = ("setCaps(address,uint64,uint256,uint256)", 0);
        (s[6], rule[6]) = ("setGlobalCaps(uint64,uint256,uint256)", 0);
        (s[7], rule[7]) = ("setCapWeight(address,uint256)", 0);
        (s[8], rule[8]) = ("setReleaseDelay(address,uint64)", 0);
        (s[9], rule[9]) = ("unfreeze(address,bool,bool)", 0);
        (s[10], rule[10]) = ("unfreezeAll(bool,bool)", 0);
        (s[11], rule[11]) = ("proposeAdmin(address)", 0);
        (s[12], rule[12]) = ("setFreezer(address)", 0);
        (s[13], rule[13]) = ("setFeeRecipient(address)", 0);
        (s[14], rule[14]) = ("freeze(address,bool,bool)", 1);
        (s[15], rule[15]) = ("freezeAll(bool,bool)", 1);
        (s[16], rule[16]) = ("acceptAdmin()", 2);
        (s[17], rule[17]) = ("deposit(address,uint32,bytes32)", 3);
        (s[18], rule[18]) = ("depositETH(uint32,bytes32)", 3);
        (s[19], rule[19]) = ("update(bytes,bytes)", 3);
        (s[20], rule[20]) = ("release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)", 3);
        (s[21], rule[21]) = ("withdrawFees(address)", 3);
        (s[22], rule[22]) = ("windDown()", 0);
        (s[23], rule[23]) = ("unfreezeReclaims()", 0);
        (s[24], rule[24]) = ("freezeReclaims()", 1);
        (s[25], rule[25]) = ("reclaim(bytes,bytes32,bytes32,address,uint32,address,address,uint256)", 3);
    }

    /// Every admin and freezer function with valid arguments, in an order where each succeeds:
    /// the three freezer calls first (made by the freezer), `setFreezer` last. `windDown` (index
    /// 7) is the only call expected to fail: a second wind-down.
    uint256 constant N_FREEZER = 3;
    uint256 constant I_WINDDOWN = 7;

    function _privileged() internal returns (bytes[] memory c) {
        c = new bytes[](20);
        uint256[] memory two = new uint256[](2);
        uint256[][] memory menu = new uint256[][](2);
        menu[0] = new uint256[](0);
        menu[1] = new uint256[](0);
        uint256[] memory d1 = new uint256[](1);
        d1[0] = 7e6;
        c[0] = abi.encodeCall(V.freeze, (address(usdc), true, true));
        c[1] = abi.encodeCall(V.freezeAll, (true, true));
        c[2] = abi.encodeCall(V.freezeReclaims, ());
        c[3] = abi.encodeCall(V.setFees, (address(usdc), two, two));
        c[4] = abi.encodeCall(V.setRelayFeeMenu, (address(usdc), menu));
        c[5] = abi.encodeCall(V.registerToken, (address(new TestToken("X", 6)), 6, d1, new uint256[](1), new uint256[](1), 0));
        c[6] = abi.encodeCall(V.addDenomination, (address(usdc), 5000e6, 0, 0));
        c[7] = abi.encodeCall(V.windDown, ());
        c[8] = abi.encodeCall(V.setCaps, (address(usdc), 1 days, 1, 1));
        c[9] = abi.encodeCall(V.setGlobalCaps, (1 days, 1, 1));
        c[10] = abi.encodeCall(V.setCapWeight, (address(usdc), 1e18));
        c[11] = abi.encodeCall(V.setReleaseDelay, (address(usdc), 365 days));
        c[12] = abi.encodeCall(V.unfreeze, (address(usdc), true, true));
        c[13] = abi.encodeCall(V.unfreezeAll, (true, true));
        c[14] = abi.encodeCall(V.unfreezeReclaims, ());
        c[15] = abi.encodeCall(V.proposeAdmin, (alice));
        c[16] = abi.encodeCall(V.setFeeRecipient, (admin));
        c[17] = abi.encodeCall(V.deregisterToken, (address(usdc)));
        c[18] = abi.encodeCall(V.withdrawFees, (address(usdc)));
        c[19] = abi.encodeCall(V.setFreezer, (admin));
    }

    function test_AbiHasNoOtherStateChangingFunction() public view {
        string memory json = vm.readFile("out/BridgeVaultFailsafe.sol/BridgeVaultFailsafe.json");
        (string[] memory known,) = _stateChanging();
        bool[] memory seen = new bool[](known.length);
        uint256 mutating;
        for (uint256 i;; ++i) {
            string memory p = string.concat(".abi[", vm.toString(i), "]");
            if (!vm.keyExistsJson(json, p)) break;
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(p, ".type")))) != keccak256("function")) continue;
            string memory mut = vm.parseJsonString(json, string.concat(p, ".stateMutability"));
            if (keccak256(bytes(mut)) == keccak256("view") || keccak256(bytes(mut)) == keccak256("pure")) continue;
            string memory sig = string.concat(vm.parseJsonString(json, string.concat(p, ".name")), "(");
            for (uint256 k;; ++k) {
                string memory q = string.concat(p, ".inputs[", vm.toString(k), "].type");
                if (!vm.keyExistsJson(json, q)) break;
                sig = string.concat(sig, k == 0 ? "" : ",", vm.parseJsonString(json, q));
            }
            sig = string.concat(sig, ")");
            mutating++;
            bool found;
            for (uint256 k; k < known.length; ++k) {
                if (keccak256(bytes(known[k])) == keccak256(bytes(sig))) {
                    assertFalse(seen[k], sig);
                    seen[k] = true;
                    found = true;
                }
            }
            assertTrue(found, string.concat("unclassified state-changing function: ", sig));
        }
        for (uint256 k; k < known.length; ++k) assertTrue(seen[k], string.concat("listed but not in ABI: ", known[k]));
        assertEq(mutating, known.length);
    }

    /// After the final, with two unclaimed deposits waiting: every admin and freezer call, in
    /// order, from the admin (then from the freezer where allowed). Principal never moves, the
    /// admin and freezer receive nothing but the fee recipient's accrued fees (deposit fees,
    /// never principal), the wind-down and the final stay, deposits stay refused, and both
    /// depositors still reclaim to their own addresses afterwards.
    function test_PrivilegedCallsNeverPayPrincipalNorUndoTheWindDown() public {
        bytes32 root = _threeDepositsOneClaimedFinal();
        uint256 principal = _principal();
        assertEq(principal, 300e6);
        (,,,,,,,, uint256 fees) = v.tokenConfig(address(usdc));
        uint64 wAt = v.windDownAt();
        bytes32 fs = v.finalState();
        bytes[] memory c = _privileged();
        for (uint256 i; i < c.length; ++i) {
            vm.prank(i < N_FREEZER ? freezer : admin);
            (bool ok, bytes memory ret) = address(v).call(c[i]);
            if (i == I_WINDDOWN) {
                assertFalse(ok, "windDown twice");
                assertEq(bytes4(ret), V.AlreadyWoundDown.selector);
            } else {
                assertTrue(ok, string.concat("privileged call ", vm.toString(i), " should succeed"));
            }
            assertEq(_principal(), principal, "principal moved");
            assertEq(v.windDownAt(), wAt);
            assertEq(v.finalState(), fs);
        }
        assertEq(usdc.balanceOf(admin), fees, "the admin got exactly the accrued deposit fees (as fee recipient), no principal");
        assertEq(usdc.balanceOf(freezer), 0);
        vm.prank(admin);
        v.unfreezeAll(true, true);
        vm.expectRevert(V.IsWoundDown.selector);
        _deposit(0, keccak256("again"));
        // undo the liveness settings the calls above made, then both depositors reclaim
        vm.startPrank(admin);
        v.unfreezeReclaims();
        v.unfreeze(address(usdc), true, true);
        v.setCaps(address(usdc), 0, 0, 0);
        v.setGlobalCaps(0, 0, 0);
        vm.stopPrank();
        _reclaim(root, keccak256("nf0"), 0, 100e6, makeAddr("d0"));
        _reclaim(root, keccak256("nf2"), 0, 100e6, makeAddr("d2"));
        assertEq(usdc.balanceOf(makeAddr("d0")), 100e6);
        assertEq(usdc.balanceOf(makeAddr("d2")), 100e6);
        assertEq(_principal(), 100e6);
    }
}
