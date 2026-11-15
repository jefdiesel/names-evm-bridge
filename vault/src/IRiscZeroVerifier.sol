// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

/// Subset of RISC Zero's verifier interface (risc0-ethereum v3.0.0; unchanged in v3.1.0).
/// A vault is bound at deployment to an immutable verifier, never the router or the emergency-stop
/// wrapper, so verification can't be swapped: RISC Zero's bare RiscZeroGroth16Verifier (3.1.0 only:
/// 3.0.x is under GHSA-g6qg-9j8p-7j87), or, for a TrustedTip update image, an UpdateGatedVerifier
/// in front of it (v4).
interface IRiscZeroVerifier {
    /// Reverts unless `seal` proves an execution of `imageId` whose journal hashes to `journalDigest`.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view;
}
