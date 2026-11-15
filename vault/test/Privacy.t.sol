// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Vm} from "forge-std/Vm.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A4: privacy of the Ethereum side (F#2, P#3, P#5). For a deposit and a release, the full
/// footprint is recorded (calldata of every call frame, every log, every storage write with
/// its new value) and checked:
///  - the vault emits exactly one log with exactly the documented fields;
///  - the secret values (deposit: LEZ destination D and secret K; release: exit secret k_j and
///    the exit commitment / leaf that would link the release to its burn) appear nowhere, as
///    any 32-byte or 20-byte window;
///  - the vault writes only slots on an allowlist derived from the compiled storage layout,
///    and the token only the balances (and allowance) of the parties.
/// A positive control shows the scanner does find values that are meant to be public.
contract PrivacyTest is VaultFixture {
    uint256 constant T0 = 1_000_000;
    string constant VAULT_JSON = "out/BridgeVault.sol/BridgeVault.json";
    string constant TOKEN_JSON = "out/Mocks.sol/TestToken.json";

    bytes32 constant K = keccak256("deposit secret k_i");
    bytes32 constant D = keccak256("LEZ destination account");
    bytes32 constant KJ = keccak256("exit secret k_j");

    // ------------------------------------------------------------------ storage layout

    /// Slot of a top-level state variable, read from the artifact's storageLayout.
    function _slot(string memory json, string memory label) internal view returns (uint256 slot) {
        for (uint256 i;; ++i) {
            string memory p = string.concat(".storageLayout.storage[", vm.toString(i), "]");
            require(vm.keyExistsJson(json, p), string.concat("no such variable: ", label));
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(p, ".label")))) == keccak256(bytes(label))) {
                return vm.parseUint(vm.parseJsonString(json, string.concat(p, ".slot")));
            }
        }
    }

    /// Slot offset of a TokenConfig member (via the type of the `_tokens` mapping's value).
    function _tokenField(string memory json, string memory label) internal view returns (uint256 slot) {
        string memory p;
        for (uint256 i;; ++i) {
            p = string.concat(".storageLayout.storage[", vm.toString(i), "]");
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(p, ".label")))) == keccak256("_tokens")) break;
        }
        string memory mapType = vm.parseJsonString(json, string.concat(p, ".type"));
        string memory structType = vm.parseJsonString(json, string.concat(".storageLayout.types['", mapType, "'].value"));
        for (uint256 k;; ++k) {
            string memory q = string.concat(".storageLayout.types['", structType, "'].members[", vm.toString(k), "]");
            require(vm.keyExistsJson(json, q), string.concat("no such field: ", label));
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(q, ".label")))) == keccak256(bytes(label))) {
                return vm.parseUint(vm.parseJsonString(json, string.concat(q, ".slot")));
            }
        }
    }

    function _map(bytes32 key, uint256 slot) internal pure returns (bytes32) {
        return keccak256(abi.encode(key, slot));
    }

    function _mapA(address key, uint256 slot) internal pure returns (bytes32) {
        return keccak256(abi.encode(key, slot));
    }

    // ------------------------------------------------------------------ scanning

    function _has32(bytes memory hay, bytes32 x) internal pure returns (bool) {
        if (hay.length < 32) return false;
        for (uint256 i; i + 32 <= hay.length; ++i) {
            bytes32 w;
            assembly {
                w := mload(add(add(hay, 32), i))
            }
            if (w == x) return true;
        }
        return false;
    }

    function _has20(bytes memory hay, bytes20 x) internal pure returns (bool) {
        if (hay.length < 20) return false;
        for (uint256 i; i + 20 <= hay.length; ++i) {
            bytes20 w;
            assembly {
                w := mload(add(add(hay, 32), i))
            }
            if (w == x) return true;
        }
        return false;
    }

    /// True if `x` (or its first or last 20 bytes) occurs in `hay`.
    function _leaks(bytes memory hay, bytes32 x) internal pure returns (bool) {
        return _has32(hay, x) || _has20(hay, bytes20(x)) || _has20(hay, bytes20(uint160(uint256(x))));
    }

    /// Every byte the transaction made public: calldata and value of every frame, every log's
    /// topics and data, every storage write's slot and new value.
    function _footprint(Vm.AccountAccess[] memory acc, Vm.Log[] memory logs) internal pure returns (bytes memory out) {
        for (uint256 i; i < acc.length; ++i) {
            out = bytes.concat(out, acc[i].data, abi.encode(acc[i].value));
            Vm.StorageAccess[] memory st = acc[i].storageAccesses;
            for (uint256 k; k < st.length; ++k) {
                if (st[k].isWrite) out = bytes.concat(out, st[k].slot, st[k].newValue);
            }
        }
        for (uint256 i; i < logs.length; ++i) {
            for (uint256 k; k < logs[i].topics.length; ++k) out = bytes.concat(out, logs[i].topics[k]);
            out = bytes.concat(out, logs[i].data);
        }
    }

    function _inList(bytes32 x, bytes32[] memory list) internal pure returns (bool) {
        for (uint256 i; i < list.length; ++i) if (list[i] == x) return true;
        return false;
    }

    /// Every non-reverted storage write is on `account` within `vaultAllowed`, or on `token`
    /// within `tokenAllowed`; nothing else is written. Returns the distinct vault slots written.
    function _assertWrites(
        Vm.AccountAccess[] memory acc,
        address token,
        bytes32[] memory vaultAllowed,
        bytes32[] memory tokenAllowed
    ) internal view returns (uint256 vaultWrites) {
        bytes32[] memory seen = new bytes32[](64);
        for (uint256 i; i < acc.length; ++i) {
            Vm.StorageAccess[] memory st = acc[i].storageAccesses;
            for (uint256 k; k < st.length; ++k) {
                if (!st[k].isWrite || st[k].reverted) continue;
                if (st[k].account == address(v)) {
                    assertTrue(_inList(st[k].slot, vaultAllowed), string.concat("vault wrote an unexpected slot ", vm.toString(st[k].slot)));
                    bool dup;
                    for (uint256 j; j < vaultWrites; ++j) dup = dup || seen[j] == st[k].slot;
                    if (!dup) seen[vaultWrites++] = st[k].slot;
                } else if (st[k].account == token) {
                    assertTrue(_inList(st[k].slot, tokenAllowed), "token wrote an unexpected slot");
                } else {
                    revert("a third account was written");
                }
            }
        }
    }

    function _enableCaps() internal {
        vm.startPrank(admin);
        v.setCaps(address(usdc), 1 days, type(uint128).max, type(uint128).max);
        v.setGlobalCaps(1 days, type(uint128).max, type(uint128).max);
        v.setCapWeight(address(usdc), 1e18);
        vm.stopPrank();
    }

    // ------------------------------------------------------------------ deposit

    function test_DepositRevealsNoDestinationOrSecret() public {
        vm.warp(T0);
        _enableCaps();
        // off-chain: C = SHA256("RFP022/dep" | k_i | dest | token | denom | cfgId)
        bytes32 c = sha256(abi.encodePacked("RFP022/dep", K, D, address(usdc), uint256(100e6), v.cfgId()));
        bytes memory callData = abi.encodeCall(BridgeVault.deposit, (address(usdc), 0, c));

        vm.recordLogs();
        vm.startStateDiffRecording();
        vm.prank(alice);
        (bool ok,) = address(v).call(callData);
        Vm.AccountAccess[] memory acc = vm.stopAndReturnStateDiff();
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertTrue(ok);
        bytes32 root = v.depositRoot();

        // exactly one vault log: Deposit(token indexed, denomIndex, commitment, leafIndex, root)
        uint256 vaultLogs;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].emitter != address(v)) continue;
            vaultLogs++;
            assertEq(logs[i].topics.length, 2);
            assertEq(logs[i].topics[0], BridgeVault.Deposit.selector);
            assertEq(logs[i].topics[1], bytes32(uint256(uint160(address(usdc)))));
            assertEq(logs[i].data, abi.encode(uint32(0), c, uint64(0), root));
        }
        assertEq(vaultLogs, 1);

        bytes memory all = bytes.concat(callData, _footprint(acc, logs));
        assertFalse(_leaks(all, D), "destination D is visible");
        assertFalse(_leaks(all, K), "secret K is visible");
        // positive control: the scanner sees what is public
        assertTrue(_has32(callData, c) && _has32(_footprint(acc, logs), c), "control: C");
        assertTrue(_has32(_footprint(acc, logs), root), "control: root");
        assertTrue(_has20(_footprint(acc, logs), bytes20(alice)), "control: depositor");

        // storage writes
        string memory json = vm.readFile(VAULT_JSON);
        bytes32 tokenBase = _mapA(address(usdc), _slot(json, "_tokens"));
        uint256 tree = _slot(json, "_depositTree");
        bytes32[] memory allowed = new bytes32[](10);
        allowed[0] = bytes32(_slot(json, "_status")); // reentrancy guard
        allowed[1] = _map(c, _slot(json, "commitmentUsed"));
        allowed[2] = bytes32(uint256(tokenBase) + _tokenField(json, "accruedFees"));
        allowed[3] = bytes32(uint256(tokenBase) + _tokenField(json, "dep")); // token cap bucket
        allowed[4] = bytes32(_slot(json, "_globalDep")); // global cap bucket
        allowed[5] = keccak256(abi.encode(uint256(0), _mapA(address(usdc), _slot(json, "counters"))));
        allowed[6] = bytes32(tree); // nextIndex
        allowed[7] = bytes32(tree + 1); // root
        allowed[8] = bytes32(tree + 2); // filled[0] (first insert)
        allowed[9] = _map(root, _slot(json, "isDepositRoot"));
        string memory tjson = vm.readFile(TOKEN_JSON);
        bytes32[] memory tokenAllowed = new bytes32[](3);
        tokenAllowed[0] = _mapA(alice, _slot(tjson, "_balances"));
        tokenAllowed[1] = _mapA(address(v), _slot(tjson, "_balances"));
        tokenAllowed[2] = _mapA(address(v), uint256(_mapA(alice, _slot(tjson, "_allowances"))));
        uint256 n = _assertWrites(acc, address(usdc), allowed, tokenAllowed);
        assertEq(n, allowed.length, "every allowlisted slot is written (the allowlist is tight)");
    }

    // ------------------------------------------------------------------ release

    function test_ReleaseRevealsNoExitSecretOrLink() public {
        vm.warp(T0);
        _enableCaps();
        _deposit(0, keccak256("c"));
        // off-chain: E = SHA256("RFP022/exit" | k_j | ethDest | token | denom | maxRelayFee | cfgId),
        // nullifier from k_j alone
        bytes32 e = sha256(abi.encodePacked("RFP022/exit", KJ, bob, address(usdc), uint256(100e6), uint256(2e6), v.cfgId()));
        bytes32 nf = sha256(abi.encodePacked("RFP022/nf", KJ));
        bytes32 leaf = v.exitLeaf(address(usdc), 100e6, e);
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdc), 0, e);
        _update(_journal(S0, keccak256("s1"), new bytes32[](0), cl, ex));
        bytes32 root = v.exitRoot();
        vm.warp(T0 + DELAY);

        bytes memory seal = _releaseSeal(root, nf, address(usdc), 100e6, bob, relayer, 2e6);
        bytes memory callData = abi.encodeCall(BridgeVault.release, (seal, root, nf, address(usdc), 0, bob, relayer, 2e6));
        vm.recordLogs();
        vm.startStateDiffRecording();
        (bool ok,) = address(v).call(callData);
        Vm.AccountAccess[] memory acc = vm.stopAndReturnStateDiff();
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertTrue(ok);

        // exactly one vault log: Released(nullifier indexed, token indexed, denomIndex, recipient, relayer, fee)
        uint256 vaultLogs;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].emitter != address(v)) continue;
            vaultLogs++;
            assertEq(logs[i].topics.length, 3);
            assertEq(logs[i].topics[0], BridgeVault.Released.selector);
            assertEq(logs[i].topics[1], nf);
            assertEq(logs[i].topics[2], bytes32(uint256(uint160(address(usdc)))));
            assertEq(logs[i].data, abi.encode(uint32(0), bob, relayer, uint256(2e6)));
        }
        assertEq(vaultLogs, 1);

        bytes memory all = bytes.concat(callData, _footprint(acc, logs));
        assertFalse(_leaks(all, KJ), "exit secret visible");
        // nothing names which exit was released (only the root, shared by every exit in it)
        assertFalse(_has32(all, e), "exit commitment visible");
        assertFalse(_has32(all, leaf), "exit leaf visible");
        // positive control
        assertTrue(_has32(all, nf) && _has32(all, root), "control: nullifier and root");
        assertTrue(_has20(_footprint(acc, logs), bytes20(bob)), "control: recipient");

        string memory json = vm.readFile(VAULT_JSON);
        bytes32 tokenBase = _mapA(address(usdc), _slot(json, "_tokens"));
        bytes32[] memory allowed = new bytes32[](6);
        allowed[0] = bytes32(_slot(json, "_status"));
        allowed[1] = _map(nf, _slot(json, "releaseNullifierSpent"));
        allowed[2] = keccak256(abi.encode(uint256(0), _mapA(address(usdc), _slot(json, "counters"))));
        allowed[3] = bytes32(uint256(tokenBase) + _tokenField(json, "rel"));
        allowed[4] = bytes32(_slot(json, "_globalRel"));
        allowed[5] = bytes32(uint256(tokenBase) + _tokenField(json, "accruedFees"));
        string memory tjson = vm.readFile(TOKEN_JSON);
        bytes32[] memory tokenAllowed = new bytes32[](3);
        tokenAllowed[0] = _mapA(address(v), _slot(tjson, "_balances"));
        tokenAllowed[1] = _mapA(bob, _slot(tjson, "_balances"));
        tokenAllowed[2] = _mapA(relayer, _slot(tjson, "_balances"));
        uint256 n = _assertWrites(acc, address(usdc), allowed, tokenAllowed);
        assertEq(n, allowed.length, "every allowlisted slot is written (the allowlist is tight)");
    }

    // ------------------------------------------------------------------ P#5: error paths

    /// Every custom error of the vault takes no arguments, so a revert is 4 bytes and can't
    /// carry a commitment, nullifier, root or amount: a refused claim, a repeat and a cap
    /// rejection differ only in which fixed selector they return. The one error in the ABI with
    /// an argument is OpenZeppelin's SafeERC20FailedOperation(token), which carries only the
    /// token address (public in the call anyway).
    function test_ErrorsCarryNoData() public view {
        string memory json = vm.readFile(VAULT_JSON);
        uint256 errors;
        for (uint256 i;; ++i) {
            string memory p = string.concat(".abi[", vm.toString(i), "]");
            if (!vm.keyExistsJson(json, p)) break;
            if (keccak256(bytes(vm.parseJsonString(json, string.concat(p, ".type")))) != keccak256("error")) continue;
            errors++;
            string memory name = vm.parseJsonString(json, string.concat(p, ".name"));
            if (keccak256(bytes(name)) == keccak256("SafeERC20FailedOperation")) {
                assertEq(vm.parseJsonString(json, string.concat(p, ".inputs[0].type")), "address");
                assertFalse(vm.keyExistsJson(json, string.concat(p, ".inputs[1]")));
                continue;
            }
            assertFalse(vm.keyExistsJson(json, string.concat(p, ".inputs[0]")), name);
        }
        assertGt(errors, 20);
    }

    /// The refusals a stranger can provoke for someone else's exit return exactly 4 bytes and
    /// the same bytes whichever nullifier or root is involved.
    function test_RefusalsAreFourBytesIndependentOfTheEntitlement() public {
        vm.warp(T0);
        bytes32 root = _bridgeOut(2, 2);
        vm.prank(admin);
        v.setCaps(address(usdc), 1 days, type(uint256).max, 50e6);
        vm.warp(T0 + DELAY);
        bytes32[2] memory nfs = [keccak256("nf-a"), keccak256("nf-b")];
        bytes memory first;
        for (uint256 i; i < 2; ++i) {
            bytes memory seal = _releaseSeal(root, nfs[i], address(usdc), 100e6, bob, relayer, 0);
            (bool ok, bytes memory err) = address(v).call(abi.encodeCall(BridgeVault.release, (seal, root, nfs[i], address(usdc), 0, bob, relayer, 0)));
            assertFalse(ok);
            assertEq(err.length, 4);
            assertEq(bytes4(err), BridgeVault.CapExceeded.selector);
            if (i == 0) first = err;
            else assertEq(err, first);
        }
    }
}
