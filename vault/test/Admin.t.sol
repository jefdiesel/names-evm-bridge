// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {TestToken} from "./mocks/Mocks.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A5: the admin surface. S#8: verification can't be swapped.
contract AdminTest is VaultFixture {
    BridgeVault v2; // a second, independent configuration (F#16)
    address admin2 = address(0xA2);
    address freezer2 = address(0xF2);
    address stranger = address(0x5757);
    TestToken newToken;

    function setUp() public override {
        super.setUp();
        v2 = new BridgeVault(IRiscZeroVerifier(address(mv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, S0, 1 hours, admin2, freezer2, feeTo);
        newToken = new TestToken("NEW", 18);
    }

    // ------------------------------------------------------------------ the classification

    /// Every admin-only function, with arguments that succeed when the admin sends them.
    function _adminCalls() internal view returns (bytes[] memory c) {
        uint256[][] memory menus = _menu2(_one(1e6), new uint256[](0));
        c = new bytes[](14);
        c[0] = abi.encodeCall(BridgeVault.registerToken, (address(newToken), 18, _arr(1 ether, 2 ether), _arr(0, 0), _arr(0, 0), DELAY));
        c[1] = abi.encodeCall(BridgeVault.deregisterToken, (address(usdc)));
        c[2] = abi.encodeCall(BridgeVault.addDenomination, (address(usdc), 10_000e6, 0, 0));
        c[3] = abi.encodeCall(BridgeVault.setFees, (address(usdc), _arr(0, 0), _arr(0, 0)));
        c[4] = abi.encodeCall(BridgeVault.setRelayFeeMenu, (address(usdc), menus));
        c[5] = abi.encodeCall(BridgeVault.setCaps, (address(usdc), 1 days, 1, 1));
        c[6] = abi.encodeCall(BridgeVault.setGlobalCaps, (1 days, 1, 1));
        c[7] = abi.encodeCall(BridgeVault.setCapWeight, (address(usdc), 1e18));
        c[8] = abi.encodeCall(BridgeVault.setReleaseDelay, (address(usdc), 2 days));
        c[9] = abi.encodeCall(BridgeVault.unfreeze, (address(usdc), true, true));
        c[10] = abi.encodeCall(BridgeVault.unfreezeAll, (true, true));
        c[11] = abi.encodeCall(BridgeVault.proposeAdmin, (bob));
        c[12] = abi.encodeCall(BridgeVault.setFreezer, (bob));
        c[13] = abi.encodeCall(BridgeVault.setFeeRecipient, (bob));
    }

    function _freezerCalls() internal pure returns (bytes[] memory c) {
        c = new bytes[](2);
        c[0] = abi.encodeCall(BridgeVault.freeze, (address(0xdead), true, true));
        c[1] = abi.encodeCall(BridgeVault.freezeAll, (true, true));
    }

    /// The complete list of state-changing functions, by access rule. S#8 checks it against the
    /// compiled ABI, so a new function (e.g. a verifier setter) fails the test until classified.
    function _stateChanging() internal pure returns (string[] memory s, uint8[] memory rule) {
        s = new string[](22);
        rule = new uint8[](22);
        // 0 admin, 1 freezer or admin, 2 pending admin, 3 anyone
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
    }

    // ------------------------------------------------------------------ A5: access control

    function _expectRefused(address caller, bytes memory data, bytes4 err) internal {
        vm.prank(caller);
        (bool ok, bytes memory ret) = address(v).call(data);
        assertFalse(ok, "call should be refused");
        assertEq(bytes4(ret), err);
    }

    /// Each admin call succeeds for the admin (so the arguments are valid and only access is
    /// tested) and reverts NotAdmin for a stranger, the freezer, and the second vault's admin
    /// and freezer.
    function test_AdminOnlyFunctionsRefuseEveryoneElse() public {
        bytes[] memory calls = _adminCalls();
        address[4] memory others = [stranger, freezer, admin2, freezer2];
        for (uint256 i; i < calls.length; ++i) {
            uint256 snap = vm.snapshotState();
            vm.prank(admin);
            (bool ok,) = address(v).call(calls[i]);
            assertTrue(ok, "admin call failed");
            vm.revertToState(snap);
            for (uint256 k; k < others.length; ++k) _expectRefused(others[k], calls[i], BridgeVault.NotAdmin.selector);
        }
    }

    /// The freezer (and admin) can freeze; a stranger and the other vault's roles cannot.
    function test_FreezeFunctionsRefuseNonFreezers() public {
        bytes[] memory calls = _freezerCalls();
        address[3] memory others = [stranger, admin2, freezer2];
        for (uint256 i; i < calls.length; ++i) {
            for (uint256 k; k < others.length; ++k) _expectRefused(others[k], calls[i], BridgeVault.NotFreezer.selector);
            uint256 snap = vm.snapshotState();
            vm.prank(freezer);
            (bool ok,) = address(v).call(calls[i]);
            assertTrue(ok);
            vm.revertToState(snap);
            vm.prank(admin);
            (ok,) = address(v).call(calls[i]);
            assertTrue(ok);
        }
    }

    /// The freezer can do nothing but freeze: every admin call, and accepting adminship
    /// it wasn't offered, is refused. Freezing one vault doesn't touch the other.
    function test_FreezerCanOnlyFreeze() public {
        vm.startPrank(freezer);
        v.freeze(address(usdc), true, true);
        v.freezeAll(true, true);
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.unfreeze(address(usdc), true, true);
        vm.expectRevert(BridgeVault.NotAdmin.selector);
        v.unfreezeAll(true, true);
        vm.expectRevert(BridgeVault.NotPendingAdmin.selector);
        v.acceptAdmin();
        vm.stopPrank();
        assertTrue(v.globalDepositsFrozen());
        assertFalse(v2.globalDepositsFrozen());
        assertFalse(v2.globalReleasesFrozen());
        // the second vault's freezer can freeze only its own vault
        vm.prank(freezer2);
        v2.freezeAll(true, false);
        assertTrue(v2.globalDepositsFrozen());
    }

    function test_AcceptAdminOnlyByPending() public {
        vm.prank(admin);
        v.proposeAdmin(bob);
        address[4] memory others = [stranger, freezer, admin2, admin];
        for (uint256 k; k < others.length; ++k) {
            _expectRefused(others[k], abi.encodeCall(BridgeVault.acceptAdmin, ()), BridgeVault.NotPendingAdmin.selector);
        }
        vm.prank(bob);
        v.acceptAdmin();
        assertEq(v.admin(), bob);
        assertEq(v.pendingAdmin(), address(0));
        // the old admin has lost every power
        bytes[] memory calls = _adminCalls();
        for (uint256 i; i < calls.length; ++i) _expectRefused(admin, calls[i], BridgeVault.NotAdmin.selector);
        // a second vault's admin cannot be offered this vault by its own admin
        vm.prank(admin2);
        v2.proposeAdmin(admin2);
        _expectRefused(admin2, abi.encodeCall(BridgeVault.acceptAdmin, ()), BridgeVault.NotPendingAdmin.selector);
    }

    // ------------------------------------------------------------------ A5: fees

    /// Fees set to zero: a deposit takes exactly the denomination and a release pays exactly
    /// the denomination minus the relay fee. Non-zero fees take exactly what is set.
    function test_SetFeesZeroAndNonZeroExact() public {
        vm.warp(1_000_000);
        vm.prank(admin);
        v.setFees(address(usdc), _arr(0, 0), _arr(0, 0));
        uint256 a0 = usdc.balanceOf(alice);
        _deposit(0, keccak256("z0"));
        assertEq(a0 - usdc.balanceOf(alice), 100e6);
        assertEq(usdc.balanceOf(address(v)), 100e6);
        assertEq(_fees(address(usdc)), 0);

        // a second deposit with non-zero fees (2% max on deposit, 0.5% on withdraw)
        vm.prank(admin);
        v.setFees(address(usdc), _arr(2e6, 0), _arr(0.5e6, 0));
        a0 = usdc.balanceOf(alice);
        _deposit(0, keccak256("z1"));
        assertEq(a0 - usdc.balanceOf(alice), 102e6);
        assertEq(_fees(address(usdc)), 2e6);

        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 2);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](2);
        ex[0] = BridgeVault.Exit(address(usdc), 0, keccak256("e0"));
        ex[1] = BridgeVault.Exit(address(usdc), 0, keccak256("e1"));
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(1_000_000 + DELAY);

        // release under the 0.5 withdraw fee: 100 - 0.5 - 2
        bytes32 nf1 = keccak256("nf1");
        v.release(_releaseSeal(root, nf1, address(usdc), 100e6, bob, relayer, 2e6), root, nf1, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 97.5e6);
        assertEq(usdc.balanceOf(relayer), 2e6);
        assertEq(_fees(address(usdc)), 2.5e6);

        // back to zero: the release pays exactly denomination - relay fee
        vm.prank(admin);
        v.setFees(address(usdc), _arr(0, 0), _arr(0, 0));
        bytes32 nf2 = keccak256("nf2");
        v.release(_releaseSeal(root, nf2, address(usdc), 100e6, bob, relayer, 2e6), root, nf2, address(usdc), 0, bob, relayer, 2e6);
        assertEq(usdc.balanceOf(bob), 97.5e6 + 98e6);
        assertEq(usdc.balanceOf(relayer), 4e6);
        assertEq(_fees(address(usdc)), 2.5e6);
        // nothing but fees left
        assertEq(usdc.balanceOf(address(v)), 2.5e6);
    }

    function test_SetFeesBounds() public {
        vm.startPrank(admin);
        // exactly 2% is allowed on either side
        v.setFees(address(usdc), _arr(2e6, 20e6), _arr(2e6, 20e6));
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.setFees(address(usdc), _arr(2e6 + 1, 0), _arr(0, 0));
        vm.expectRevert(BridgeVault.FeeTooHigh.selector);
        v.setFees(address(usdc), _arr(0, 0), _arr(0, 20e6 + 1));
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.setFees(address(usdc), _one(0), _arr(0, 0));
        vm.expectRevert(BridgeVault.NotRegistered.selector);
        v.setFees(address(0xdead), _arr(0, 0), _arr(0, 0));
        vm.stopPrank();
        // the failed calls left the 2% fees in place
        (,,,,,, uint256[] memory df, uint256[] memory wf,) = v.tokenConfig(address(usdc));
        assertEq(df[1], 20e6);
        assertEq(wf[0], 2e6);
    }

    // ------------------------------------------------------------------ S#8: no verification setter

    function _contains(string memory hay, string memory needle) internal pure returns (bool) {
        bytes memory h = bytes(hay);
        bytes memory n = bytes(needle);
        if (n.length > h.length) return false;
        for (uint256 i; i + n.length <= h.length; ++i) {
            bool m = true;
            for (uint256 j; j < n.length && m; ++j) m = h[i + j] == n[j];
            if (m) return true;
        }
        return false;
    }

    /// The state-changing functions in the compiled ABI (out/BridgeVault.sol/BridgeVault.json,
    /// readable through fs_permissions) are exactly the classified list above: none sets the
    /// verifier, an image id, the LEZ ids or cfgId.
    function test_NoSetterForVerificationInAbi() public view {
        string memory json = vm.readFile("out/BridgeVault.sol/BridgeVault.json");
        (string[] memory known,) = _stateChanging();
        bool[] memory seen = new bool[](known.length);
        uint256 functions;
        uint256 mutating;
        for (uint256 i;; ++i) {
            string memory p = string.concat(".abi[", vm.toString(i), "]");
            if (!vm.keyExistsJson(json, p)) break;
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(p, ".type")))) != keccak256("function")) continue;
            functions++;
            string memory name = vm.parseJsonString(json, string.concat(p, ".name"));
            string memory sig = string.concat(name, "(");
            for (uint256 k;; ++k) {
                string memory q = string.concat(p, ".inputs[", vm.toString(k), "].type");
                if (!vm.keyExistsJson(json, q)) break;
                sig = string.concat(sig, k == 0 ? "" : ",", vm.parseJsonString(json, q));
            }
            sig = string.concat(sig, ")");
            // the signature is in methodIdentifiers with the selector it hashes to
            string memory id = vm.parseJsonString(json, string.concat(".methodIdentifiers['", sig, "']"));
            assertEq(vm.parseBytes(string.concat("0x", id)), abi.encodePacked(bytes4(keccak256(bytes(sig)))), sig);

            string memory mut = vm.parseJsonString(json, string.concat(p, ".stateMutability"));
            if (keccak256(bytes(mut)) == keccak256("view") || keccak256(bytes(mut)) == keccak256("pure")) continue;
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
            // belt and braces: no mutating function even mentions what verification depends on
            string[7] memory banned = ["erifier", "mageId", "cfg", "Cfg", "programId", "zoneId", "lezChain"];
            for (uint256 b; b < banned.length; ++b) assertFalse(_contains(sig, banned[b]), sig);
        }
        for (uint256 k; k < known.length; ++k) assertTrue(seen[k], string.concat("listed but not in ABI: ", known[k]));
        assertEq(mutating, known.length);
        assertGt(functions, mutating); // views were seen too, so the walk covered the ABI
    }

    /// The calls the access tests use cover exactly the functions classified as admin-only
    /// (rule 0) and freezer (rule 1), and the classification is checked against the ABI above,
    /// so "every admin function" is enforced, not hand-listed twice.
    function test_AccessTestsCoverTheClassification() public view {
        (string[] memory sigs, uint8[] memory rule) = _stateChanging();
        bytes[] memory a = _adminCalls();
        bytes[] memory f = _freezerCalls();
        uint256 nAdmin;
        uint256 nFreezer;
        for (uint256 i; i < sigs.length; ++i) {
            bytes4 sel = bytes4(keccak256(bytes(sigs[i])));
            bytes[] memory list = rule[i] == 0 ? a : f;
            if (rule[i] > 1) continue;
            if (rule[i] == 0) nAdmin++;
            else nFreezer++;
            bool found;
            for (uint256 k; k < list.length; ++k) found = found || bytes4(list[k]) == sel;
            assertTrue(found, string.concat("no access test for ", sigs[i]));
        }
        assertEq(a.length, nAdmin);
        assertEq(f.length, nFreezer);
    }

    /// Every admin and freezer function, then an admin handover, called one after another:
    /// the verifier, both image ids, the LEZ ids, cfgId and the runtime code are unchanged.
    function test_VerificationUnchangedAfterEveryAdminCall() public {
        bytes32 codeHash = address(v).codehash;
        bytes32 cfg = v.cfgId();
        bytes[] memory calls = _adminCalls();
        // addDenomination (2) after setFees/setRelayFeeMenu (they take one entry per denomination),
        // deregisterToken (1) after the calls that need USDC registered; proposeAdmin, setFreezer
        // and setFeeRecipient are last in the list already
        uint8[14] memory order = [0, 3, 4, 5, 6, 7, 8, 9, 10, 2, 1, 11, 12, 13];
        for (uint256 i; i < order.length; ++i) {
            vm.prank(admin);
            (bool ok,) = address(v).call(calls[order[i]]);
            assertTrue(ok, vm.toString(uint256(order[i])));
            _assertVerificationFixed(codeHash, cfg);
        }
        calls = _freezerCalls();
        for (uint256 i; i < calls.length; ++i) {
            vm.prank(bob); // bob is the freezer now
            (bool ok,) = address(v).call(calls[i]);
            assertTrue(ok);
            _assertVerificationFixed(codeHash, cfg);
        }
        vm.prank(bob);
        v.acceptAdmin();
        _assertVerificationFixed(codeHash, cfg);
    }

    function _assertVerificationFixed(bytes32 codeHash, bytes32 cfg) internal view {
        assertEq(address(v.verifier()), address(mv));
        assertEq(v.updateImageId(), G2);
        assertEq(v.releaseImageId(), G3);
        assertEq(v.lezChainId(), LEZ_CHAIN);
        assertEq(v.zoneId(), ZONE);
        assertEq(v.programId(), PROGRAM);
        assertEq(v.cfgId(), cfg);
        assertEq(v.minReleaseDelay(), 1 hours);
        assertEq(address(v).codehash, codeHash);
    }
}
