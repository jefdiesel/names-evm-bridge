// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IRiscZeroVerifier} from "../IRiscZeroVerifier.sol";

/// @title DemoAttestedVerifier: OPERATOR-ATTESTED stand-in for the RISC Zero verifier.
///
/// DEMO / TESTNET ONLY. NOT TRUSTLESS. This contract checks no zero-knowledge proof. It accepts
/// `verify(seal, imageId, journalDigest)` when `seal` carries an ECDSA signature by one fixed
/// operator key (`ATTESTOR`) over an EIP-712 message that names this chain, this contract,
/// `imageId` and `journalDigest`. Whoever holds that key can make `BridgeVault` accept any
/// `update` and any `release` it signs. A vault built on this verifier is an operator-attested
/// demo stage, never the trustless design (see `src/demo/README.md`).
///
/// It exists so the whole loop (deposit on Sepolia, claim on the Logos test chain, burn, state
/// update, release on Sepolia) can run on a public testnet before on-chain proof verification of
/// the update (G2) exists. Without some check here anyone could drain the demo vault's test funds
/// (with RISC Zero's `MockVerifier`-style seals, `seal == imageId | digest`, anyone can forge one).
///
/// What the attestor must do off chain before signing (normative; see the README):
/// - RELEASE (`imageId == releaseImageId`): verify a real RISC Zero succinct receipt of the
///   release guest (G3) for that image id whose journal is exactly the 256 bytes the vault hashes
///   (`abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)`), and sign
///   `sha256(journal)`. The user's exit secret stays on the user's machine: the operator sees only
///   the receipt and its journal, which the `release` call publishes anyway.
/// - UPDATE (`imageId == updateImageId`): run the outbound state guest (G2-lite, the build that
///   accepts batches today) over the LEZ blocks it accepts as final, starting from the vault's
///   current `g2State`, and sign `sha256(journal)` of the journal it produced. Prove it when
///   feasible. NEVER sign a journal whose `newState == prevState` or that revisits a past state:
///   `verify` is stateless, so such a journal could be replayed by anyone (G2-lite refuses an
///   empty batch, so a journal it really produced always advances the state). The vault refuses
///   `newState == prevState` itself (`NoStateAdvance`); a revisited state is only this rule.
/// - Sign nothing else (no other image id), rate-limit, and log every signature with its inputs.
///
/// What a malicious or compromised attestor can do: sign any update or release, so take up to
/// the vault's balance, within the release caps and after the release delay, while the freezer
/// does not act. That is exactly the trust the real design removes. What the vault still enforces
/// whatever is signed: per token, releases <= burns <= claims <= deposits (so only deposited
/// value can leave), `prevState` chaining, release nullifiers, caps, the release delay and the
/// freeze. The image ids pinned in the vault remain the real guest image ids, so the same vault
/// logic runs as it will with real proofs.
///
/// Design (see the README for the reasons):
/// - One immutable attestor. No owner, no rotation, no upgrade, no pause (the vault's freeze is
///   the pause). A leaked key means deploying a new demo vault, as the real design's "new
///   version" rule does for any change of verification.
/// - Time-boxed: `verify` always reverts from `EXPIRES_AT` on (at most `MAX_LIFETIME` after
///   deployment), so this stage cannot quietly become permanent. After expiry the vault can take
///   no update and pay no release: anything still inside stays there.
/// - Seal = `SEAL_PREFIX (4 bytes) | r (32) | s (32) | v (1)`, 69 bytes, v in {27, 28}, low s.
///   A seal starting with RISC Zero's Groth16 selector `0x73c457ba` is refused with
///   `ZkProofNotCheckedHere`, so a real proof can never be mistaken as verified here.
/// - `verify` is stateless (`view`, as the interface requires): a signature for (imageId,
///   digest) can be reused. That is harmless for the vault: an update replay fails `StaleState`
///   (a non-advancing journal fails `NoStateAdvance`, and the attestor never signs a revisited
///   state), a release replay fails `NullifierSpent`,
///   and any change of recipient, relayer, fee, root or nullifier changes the digest.
contract DemoAttestedVerifier is IRiscZeroVerifier {
    // ------------------------------------------------------------------ constants

    /// What this verifier is, for explorers, scripts and UIs.
    string public constant KIND = "demo-attested: an operator checked the proof off chain; NOT trustless";
    string public constant VERSION = "1.0.0";

    /// Domain tag, also the `tag` field of every signed message.
    bytes32 public constant TAG = keccak256("RFP022/demo-attested-verifier/v1");
    /// First 4 bytes of every seal this verifier accepts: bytes4(TAG) = 0x7fa98d9a.
    bytes4 public constant SEAL_PREFIX = bytes4(TAG);
    /// RISC Zero Groth16 verifier selector (risc0-ethereum v3.0.0, the vault's real verifier).
    bytes4 public constant RISC_ZERO_GROTH16_SELECTOR = 0x73c457ba;
    /// 4-byte prefix + r + s + v.
    uint256 public constant SEAL_LENGTH = 69;
    /// The longest this demo stage can live.
    uint256 public constant MAX_LIFETIME = 365 days;

    bytes32 public constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    string public constant DOMAIN_NAME = "RFP022 DemoAttestedVerifier";
    string public constant DOMAIN_VERSION = "1";
    bytes32 public constant ATTESTATION_TYPEHASH =
        keccak256("DemoAttestation(bytes32 tag,bytes32 imageId,bytes32 journalDigest)");

    /// secp256k1 group order / 2: a signature with a larger s is the malleable twin.
    uint256 internal constant HALF_N = 0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0;

    // ------------------------------------------------------------------ immutables

    /// The one key whose signatures this verifier accepts.
    address public immutable ATTESTOR;
    /// From this time on (block.timestamp >= EXPIRES_AT) every `verify` reverts.
    uint256 public immutable EXPIRES_AT;

    // ------------------------------------------------------------------ errors

    error ZeroAttestor();
    /// Constructor: lifetime 0 or above MAX_LIFETIME.
    error BadLifetime();
    /// The demo stage is over: nothing verifies any more.
    error Expired(uint256 expiresAt);
    /// The seal starts with RISC Zero's Groth16 selector. This verifier does not check zk
    /// proofs; such a seal is never accepted here, valid or not.
    error ZkProofNotCheckedHere();
    error BadSealLength(uint256 length);
    error UnknownSealPrefix(bytes4 prefix);
    /// v must be 27 or 28.
    error BadV(uint8 v);
    /// s above secp256k1n / 2 (the malleable form).
    error HighS();
    /// ecrecover returned address(0): not a valid signature.
    error InvalidSignature();
    /// A valid signature, but not by ATTESTOR over this chain, this verifier, imageId and
    /// journalDigest. A wrong chain id, verifier address, image id or digest all end here.
    error NotAttestor(address recovered);

    // ------------------------------------------------------------------ constructor

    /// @param attestor the operator key; immutable.
    /// @param lifetime seconds from deployment until `verify` stops accepting anything
    ///        (1 .. MAX_LIFETIME; the Sepolia script defaults to 180 days).
    constructor(address attestor, uint256 lifetime) {
        if (attestor == address(0)) revert ZeroAttestor();
        if (lifetime == 0 || lifetime > MAX_LIFETIME) revert BadLifetime();
        ATTESTOR = attestor;
        EXPIRES_AT = block.timestamp + lifetime;
    }

    // ------------------------------------------------------------------ verify

    /// Reverts unless `seal` is ATTESTOR's signature over `attestationDigest(imageId,
    /// journalDigest)` and the stage has not expired. Checks, in order: expiry, RISC Zero
    /// selector, length, prefix, v, s, ecrecover != 0, signer == ATTESTOR.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (block.timestamp >= EXPIRES_AT) revert Expired(EXPIRES_AT);
        if (seal.length >= 4 && bytes4(seal[:4]) == RISC_ZERO_GROTH16_SELECTOR) revert ZkProofNotCheckedHere();
        if (seal.length != SEAL_LENGTH) revert BadSealLength(seal.length);
        if (bytes4(seal[:4]) != SEAL_PREFIX) revert UnknownSealPrefix(bytes4(seal[:4]));
        bytes32 r = bytes32(seal[4:36]);
        bytes32 s = bytes32(seal[36:68]);
        uint8 v = uint8(seal[68]);
        if (v != 27 && v != 28) revert BadV(v);
        if (uint256(s) > HALF_N) revert HighS();
        address signer = ecrecover(attestationDigest(imageId, journalDigest), v, r, s);
        if (signer == address(0)) revert InvalidSignature();
        if (signer != ATTESTOR) revert NotAttestor(signer);
    }

    // ------------------------------------------------------------------ views (what to sign)

    /// EIP-712 domain separator, computed from the live chain id and this address on every call
    /// (never cached), so a signature for another chain or another verifier never matches.
    function domainSeparator() public view returns (bytes32) {
        return keccak256(
            abi.encode(
                DOMAIN_TYPEHASH,
                keccak256(bytes(DOMAIN_NAME)),
                keccak256(bytes(DOMAIN_VERSION)),
                block.chainid,
                address(this)
            )
        );
    }

    /// The 32 bytes the attestor signs (raw, no further hashing or "\x19Ethereum Signed
    /// Message" prefix): keccak256(0x1901 | domainSeparator() | structHash).
    function attestationDigest(bytes32 imageId, bytes32 journalDigest) public view returns (bytes32) {
        bytes32 structHash = keccak256(abi.encode(ATTESTATION_TYPEHASH, TAG, imageId, journalDigest));
        return keccak256(abi.encodePacked(hex"1901", domainSeparator(), structHash));
    }

    /// The seal for a signature (r, s, v): SEAL_PREFIX | r | s | v.
    function encodeSeal(bytes32 r, bytes32 s, uint8 v) external pure returns (bytes memory) {
        return abi.encodePacked(SEAL_PREFIX, r, s, v);
    }
}
