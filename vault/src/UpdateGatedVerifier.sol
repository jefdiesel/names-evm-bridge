// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {IRiscZeroVerifier} from "./IRiscZeroVerifier.sol";

/// The vault side of `BridgeVault.update`, used to call it through the gate.
interface IGatedVault {
    function update(bytes calldata journal, bytes calldata seal) external;
    function verifier() external view returns (IRiscZeroVerifier);
    function updateImageId() external view returns (bytes32);
}

/// A vault's verifier for the TrustedTip stage (v4): RISC Zero's bare Groth16 verifier checks
/// every proof on chain, but an UPDATE proof counts only when it arrives through
/// `submitUpdate` from one fixed submitter.
///
/// Why the gate. The pinned G2-lite update image is the TrustedTip build: it proves that the
/// bridge's transactions in a hash-linked run of LEZ blocks yield the journal's exits and claims,
/// but not that those blocks are the real chain's (LEZ rc3 headers carry no sequencer signature,
/// and the Logos Blockchain light client that would prove finality does not exist,
/// `lez-bridge/outbound/core/src/finality.rs`). With `update` open to anyone, anyone with RISC
/// Zero's public prover could prove an update over made-up blocks: the vault's G2 state would
/// fork (every honest update then fails `StaleState`) and made-up burns could release deposits
/// within the caps. So which LEZ blocks count is the submitter's word; everything else is proved.
///
/// - `verify` for any image other than `UPDATE_IMAGE_ID` (the G3 release proofs) passes straight
///   to `VERIFIER`: releases stay permissionless and proof-checked.
/// - `verify` for `UPDATE_IMAGE_ID` passes only while `submitUpdate` is running, for the vault it
///   was called with (the caller of `verify`) and the journal digest it was given, and then also
///   needs `VERIFIER` to accept the seal. The mark lives in transient storage and is cleared
///   before `submitUpdate` returns.
/// - Everything is immutable: no admin, no upgrade, no rotation. A lost submitter key stops
///   updates for good (burns then wait); a stolen one can push an update over a forged LEZ tip,
///   bounded by the vault's caps and release delay, during which the vault's freezer can freeze
///   releases (`freezeAll(false, true)`, which also blocks `update`).
contract UpdateGatedVerifier is IRiscZeroVerifier {
    /// RISC Zero's bare `RiscZeroGroth16Verifier` (not the router, not an emergency stop: those
    /// can be switched off by RISC Zero, which would strand the vault).
    IRiscZeroVerifier public immutable VERIFIER;
    /// The vault's `updateImageId`: the only image the gate holds back.
    bytes32 public immutable UPDATE_IMAGE_ID;
    /// The one account that may submit updates (an operator key, not the vault's admin).
    address public immutable SUBMITTER;

    string public constant KIND =
        "update-gated: RISC Zero Groth16 verifies every proof; updates only from SUBMITTER (LEZ finality is its word)";

    /// keccak256(vault, journalDigest) of the update being submitted; zero otherwise.
    bytes32 private transient _pending;

    error ZeroAddress();
    error ZeroImageId();
    error NotSubmitter();
    error UpdateNotThroughGate();
    error Busy();
    error WrongVault();

    event UpdateSubmitted(address indexed vault, bytes32 journalDigest);

    constructor(IRiscZeroVerifier verifier_, bytes32 updateImageId_, address submitter_) {
        if (address(verifier_) == address(0) || submitter_ == address(0)) revert ZeroAddress();
        if (updateImageId_ == bytes32(0)) revert ZeroImageId();
        VERIFIER = verifier_;
        UPDATE_IMAGE_ID = updateImageId_;
        SUBMITTER = submitter_;
    }

    /// Calls `vault.update(journal, seal)` with this gate open for exactly that vault and journal.
    /// The vault must use this gate as its verifier and pin the same update image.
    function submitUpdate(IGatedVault vault, bytes calldata journal, bytes calldata seal) external {
        if (msg.sender != SUBMITTER) revert NotSubmitter();
        if (_pending != bytes32(0)) revert Busy();
        if (address(vault.verifier()) != address(this) || vault.updateImageId() != UPDATE_IMAGE_ID) revert WrongVault();
        bytes32 digest = sha256(journal);
        _pending = keccak256(abi.encode(address(vault), digest));
        vault.update(journal, seal);
        _pending = bytes32(0);
        emit UpdateSubmitted(address(vault), digest);
    }

    /// IRiscZeroVerifier. Reverts unless `VERIFIER` accepts the seal and, for the update image,
    /// the call comes from the vault `submitUpdate` is running for, for that journal.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (imageId == UPDATE_IMAGE_ID && _pending != keccak256(abi.encode(msg.sender, journalDigest))) {
            revert UpdateNotThroughGate();
        }
        VERIFIER.verify(seal, imageId, journalDigest);
    }
}
