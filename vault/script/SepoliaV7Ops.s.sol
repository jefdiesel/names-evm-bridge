// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {SeqSigGate} from "../src/SeqSigGate.sol";

/// The v7 operations anyone may send (no role): stage a signed update, finalize it after the
/// contest window, contest it with evidence, release an exit. The sender is SEPOLIA_PK, any funded
/// key (the v7 run used the deployer; the gate gives it no role). Each function checks what it
/// can read first and prints what changed.
///
///   GATE=… JOURNAL=$(cat journal.hex) SEAL=$(cat seal.hex) forge script script/SepoliaV7Ops.s.sol --sig "submit()" --rpc-url <rpc> [--broadcast …]
///   GATE=… VAULT_JOURNAL=$(cat vault-journal.hex)          forge script script/SepoliaV7Ops.s.sol --sig "finalize()" …
///   GATE=… EVIDENCE=$(cat journal.hex) SEAL=$(cat seal.hex) forge script script/SepoliaV7Ops.s.sol --sig "contest()" …
///   VAULT=… ROOT= NULLIFIER= TOKEN= DENOM_INDEX= RECIPIENT= RELAYER= RELAY_FEE= (prove-release-rc3.sh's release.env) SEAL=… --sig "release()" …
/// Values are 0x-hex, as `seqsig update`, `seqsig evidence` and the wrap write them to files (passed in the
/// environment: forge scripts may not read files outside the project).
contract SepoliaV7Ops is Script {
    function _pk() internal view returns (uint256) {
        return vm.envUint("SEPOLIA_PK");
    }

    function submit() external {
        SeqSigGate g = SeqSigGate(vm.envAddress("GATE"));
        BridgeVault v = BridgeVault(address(g.VAULT()));
        bytes memory j = vm.envBytes("JOURNAL");
        bytes memory seal = vm.envBytes("SEAL");
        require(!g.halted() && !g.dead(), "gate halted or dead");
        require(g.finalizableAt() == 0, "an update is already pending");
        bytes32 prev;
        bytes32 next;
        assembly {
            prev := mload(add(j, 96)) // vault journal word 2 (after the offset word and cfgId)
            next := mload(add(j, 128))
        }
        require(prev == v.g2State(), "the journal's prevState is not the vault's g2State (stale)");
        console.log("sender %s submits: prevState %s -> newState %s", vm.addr(_pk()), vm.toString(prev), vm.toString(next));
        console.log("journal %s bytes, sha256 %s; seal %s bytes", j.length, vm.toString(sha256(j)), seal.length);
        vm.startBroadcast(_pk());
        g.submit(j, seal);
        vm.stopBroadcast();
        SeqSigGate.Pending memory p = g.pending();
        console.log("PENDING: blockId %s signers bitmap %s", uint256(p.blockId), uint256(p.signers));
        console.log("finalizable at %s in this simulation (window %s s); read finalizableAt() from the chain after the broadcast", g.finalizableAt(), uint256(g.CONTEST_WINDOW()));
    }

    function finalize() external {
        SeqSigGate g = SeqSigGate(vm.envAddress("GATE"));
        BridgeVault v = BridgeVault(address(g.VAULT()));
        bytes memory vj = vm.envBytes("VAULT_JOURNAL");
        uint256 at = g.finalizableAt();
        require(at != 0, "nothing pending");
        require(block.timestamp >= at, "the contest window is still open");
        SeqSigGate.Pending memory p = g.pending();
        require(sha256(vj) == p.journalDigest, "VAULT_JOURNAL is not the pending update's vault journal");
        vm.startBroadcast(_pk());
        g.finalize(vj);
        vm.stopBroadcast();
        require(v.g2State() == p.newState, "g2State did not move to the update's newState");
        console.log("APPLIED: g2State %s exitCount %s exitRoot %s", vm.toString(v.g2State()), uint256(v.exitCount()), vm.toString(v.exitRoot()));
    }

    function contest() external {
        SeqSigGate g = SeqSigGate(vm.envAddress("GATE"));
        bytes memory e = vm.envBytes("EVIDENCE");
        bytes memory seal = vm.envBytes("SEAL");
        require(g.finalizableAt() != 0 && block.timestamp < g.finalizableAt(), "no pending update inside its window");
        uint64 before = g.ejected();
        vm.startBroadcast(_pk());
        g.contest(e, seal);
        vm.stopBroadcast();
        console.log("CONTESTED: ejected mask %s -> %s; halted %s", uint256(before), uint256(g.ejected()), g.halted() ? "true" : "false");
        console.log("pending after: finalizableAt %s; active signers %s", g.finalizableAt(), g.activeSigners());
    }

    function release() external {
        BridgeVault v = BridgeVault(vm.envAddress("VAULT"));
        bytes memory seal = vm.envBytes("SEAL");
        bytes32 root = vm.envBytes32("ROOT");
        bytes32 nf = vm.envBytes32("NULLIFIER");
        address token = vm.envAddress("TOKEN");
        uint32 di = uint32(vm.envUint("DENOM_INDEX"));
        address to = vm.envAddress("RECIPIENT");
        address relayer = vm.envAddress("RELAYER");
        uint256 fee = vm.envUint("RELAY_FEE");
        require(v.exitRootTime(root) != 0, "the vault never recorded this exit root (finalize first)");
        uint256 b0 = to.balance;
        vm.startBroadcast(_pk());
        v.release(seal, root, nf, token, di, to, relayer, fee);
        vm.stopBroadcast();
        console.log("RELEASED to %s: +%s wei", to, to.balance - b0);
    }
}
