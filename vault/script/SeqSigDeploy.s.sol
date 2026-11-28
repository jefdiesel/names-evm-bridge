// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {SeqSigGate, ISeqSigVault} from "../src/SeqSigGate.sol";
import {MockVerifier} from "../test/mocks/Mocks.sol";

/// Local seq-sig run only (anvil, lez-bridge/scripts/e2e-seqsig.sh): the TEST MockVerifier, a
/// SeqSigGate bound to the vault deployed right after it (predicted CREATE address) in front of it (update image = the seq-sig G2 guest, evidence image = the evidence
/// guest, release image, the STAND-IN signer keys (the gate hashes them), THRESHOLD, CONTEST_WINDOW
/// seconds; a window under the gate's minimum needs TEST_ONLY_SHORT_WINDOW=true), and an unchanged
/// BridgeVault whose verifier is the gate. One token: ETH (0.01 / 0.1), 1% deposit and 0.5%
/// withdraw fees, release delay 0 (the gate's window is the wait).
/// "mock-verified: proofs executed, not proved; signer keys are stand-ins."
///
/// Env: PROGRAM_ID, UPDATE_IMAGE_ID, EVIDENCE_IMAGE_ID, RELEASE_IMAGE_ID, G2_STATE, SIGNER_KEYS (sorted,
/// comma-separated), THRESHOLD, CONTEST_WINDOW, TEST_ONLY_SHORT_WINDOW, LEZ_CHAIN_ID, ZONE_ID. Prints VAULT=,
/// GATE=, MOCK=, CFG_ID=, SET_HASH=.
contract SeqSigDeploy is Script {
    function run() external {
        vm.startBroadcast();
        MockVerifier mv = new MockVerifier();
        // The gate is bound to the vault, and the vault's constructor takes the gate: deploy the gate
        // with the vault's predicted address (the next CREATE after the gate), then the vault, and
        // check it landed there.
        (, address deployer,) = vm.readCallers();
        address predicted = vm.computeCreateAddress(deployer, vm.getNonce(deployer) + 1);
        SeqSigGate gate = new SeqSigGate(
            mv,
            ISeqSigVault(predicted),
            vm.envBytes32("LEZ_CHAIN_ID"),
            vm.envBytes32("ZONE_ID"),
            vm.envBytes32("PROGRAM_ID"),
            vm.envBytes32("UPDATE_IMAGE_ID"),
            vm.envBytes32("EVIDENCE_IMAGE_ID"),
            vm.envBytes32("RELEASE_IMAGE_ID"),
            vm.envBytes32("SIGNER_KEYS", ","),
            vm.envUint("THRESHOLD"),
            uint64(vm.envUint("CONTEST_WINDOW")),
            vm.envOr("TEST_ONLY_SHORT_WINDOW", false)
        );
        // Local anvil default: MIN_ADMIN_DELAY. Any real deploy's adminDelay is immutable: ask the owner before deploy.
        uint64 adminDelay = uint64(vm.envOr("ADMIN_DELAY_SECONDS", uint256(1 hours)));
        BridgeVault v = new BridgeVault(
            IRiscZeroVerifier(address(gate)),
            vm.envBytes32("UPDATE_IMAGE_ID"),
            vm.envBytes32("RELEASE_IMAGE_ID"),
            vm.envBytes32("LEZ_CHAIN_ID"),
            vm.envBytes32("ZONE_ID"),
            vm.envBytes32("PROGRAM_ID"),
            vm.envBytes32("G2_STATE"),
            0,
            msg.sender,
            msg.sender,
            msg.sender,
            adminDelay
        );
        require(address(v) == predicted, "vault not at the address the gate is bound to");
        require(v.cfgId() == gate.CFG_ID(), "gate CFG_ID differs from the vault's");
        uint256[] memory d = new uint256[](2);
        uint256[] memory df = new uint256[](2);
        uint256[] memory wf = new uint256[](2);
        d[0] = 0.01 ether;
        d[1] = 0.1 ether;
        df[0] = 0.0001 ether;
        df[1] = 0.001 ether;
        wf[0] = 0.00005 ether;
        wf[1] = 0.0005 ether;
        v.registerToken(address(0), 18, d, df, wf, 0);
        vm.stopBroadcast();

        console.log("VAULT=%s", address(v));
        console.log("GATE=%s", address(gate));
        console.log("MOCK=%s", address(mv));
        console.log("CFG_ID=%s", vm.toString(v.cfgId()));
        console.log("SET_HASH=%s", vm.toString(gate.SIGNER_SET_HASH()));
    }
}
