// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signed LEZ checkpoints for the "sequencer signatures" withdrawal route (branch `seq-sig`).
//!
//! A checkpoint names the LEZ block an outbound G2-lite update ends at, bound to one bridge
//! deployment and to the update's starting point:
//!
//! ```text
//! message = SHA256("RFP022/seqsig/checkpoint/v1" | cfgId | u64be(prevId) | prevHash | u64be(blockId) | blockHash)
//! ```
//!
//! - `cfgId` is the bridge's config id (`BridgeVault.cfgId`: Ethereum chain id, vault address, LEZ
//!   channel id, root source and bridge program), so a signature cannot be replayed for another
//!   vault, Ethereum chain, LEZ chain or bridge program;
//! - `prevId`/`prevHash` are the LEZ tip of the G2 state the update starts from (the previous
//!   checkpoint), so it cannot be replayed onto another history;
//! - `blockId`/`blockHash` are the block the update ends at (the TrustedTip witness).
//!
//! Signers sign the 32-byte message with Ed25519 (RFC 8032, `ed25519-dalek` 2.2.0,
//! `verify_strict`, which also refuses a non-canonical S; inside the guests on RISC Zero's
//! accelerated `curve25519-dalek` 4.1.3 fork). A signer set is a list of n Ed25519 public keys in
//! strictly ascending byte order, each a canonical encoding of a point that is not of small order
//! (so no holder gets two votes through two encodings of one key, and no key signs for anyone):
//!
//! ```text
//! setHash = SHA256("RFP022/seqsig/set/v1" | u32be(n) | key_0 | ... | key_{n-1})
//! ```
//!
//! **What a signature must vouch for.** The G2 guest takes private transactions on the sequencer's
//! word (`SequencerTrusted`): a signer that only attests "this block was published" would let one
//! block producer's made-up private burn through. A signer must re-execute blocks `prevId+1..blockId`
//! and check every privacy receipt before it signs. The repository's attestor (`seqsig attest`)
//! re-derives the hash chain and the claim/burn extraction natively; it does NOT verify privacy
//! receipts (LEZ rc3 makes them with risc0 3.0.5, which a 3.1 host refuses) or public execution.
//!
//! [`check`] accepts a checkpoint when at least `threshold` DISTINCT keys of the set that are not
//! ejected (`ejected`, a bitmask over the set's indexes, kept by the Ethereum gate) signed it. A key
//! that signs twice counts once; a signature that does not verify, or one by an ejected key,
//! refuses the whole witness. The G2 guest `outbound_guest_seqsig` runs it on the checkpoint it
//! derives from its own input and appends [`journal_tail`] (set hash, threshold, ejected mask, the
//! signer bitmap, the checkpoint) to the vault's journal. The evidence guest commits
//! [`evidence_journal`]: which keys signed a checkpoint (the gate compares it with a pending update).
//!
//! STAND-INS: in this repository's demo the keys are keys of ours made by `seqsig keygen`, not
//! LEZ sequencer keys. LEZ v0.4.0-rc3 standalone block headers carry no signature, and the
//! inscription signatures LEZ sequencers make on the Logos Blockchain are not available on a
//! local chain.

use borsh::{BorshDeserialize, BorshSerialize};
use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};

pub const CHECKPOINT_TAG: &[u8] = b"RFP022/seqsig/checkpoint/v1";
pub const SET_TAG: &[u8] = b"RFP022/seqsig/set/v1";
/// Largest signer set accepted (bitmaps are u64; the gate has the same limit).
pub const MAX_SIGNERS: usize = 64;
/// Bytes [`journal_tail`] appends to the vault journal.
pub const TAIL_LEN: usize = 256;
/// Bytes of [`evidence_journal`].
pub const EVIDENCE_LEN: usize = 224;

/// What one signature covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Checkpoint {
    pub cfg_id: [u8; 32],
    pub prev_id: u64,
    pub prev_hash: [u8; 32],
    pub block_id: u64,
    pub block_hash: [u8; 32],
}

impl Checkpoint {
    /// The 32 bytes every signer signs.
    #[must_use]
    pub fn message(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(CHECKPOINT_TAG);
        h.update(self.cfg_id);
        h.update(self.prev_id.to_be_bytes());
        h.update(self.prev_hash);
        h.update(self.block_id.to_be_bytes());
        h.update(self.block_hash);
        h.finalize().into()
    }
}

/// One signature in a witness: which key of the set (index) and the 64-byte Ed25519 signature.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Sig {
    pub key_index: u32,
    pub sig: [u8; 64],
}

/// The signature witness of one update: the set, the threshold and the ejected mask the gate
/// holds (the gate checks both against its own), and the signatures.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Witness {
    pub keys: Vec<[u8; 32]>,
    pub threshold: u32,
    pub ejected: u64,
    pub sigs: Vec<Sig>,
}

/// Evidence: keys of the set that signed `checkpoint` (one key for equivocation, a quorum for a
/// halt).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Evidence {
    pub checkpoint: Checkpoint,
    pub keys: Vec<[u8; 32]>,
    pub sigs: Vec<Sig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigError {
    EmptySet,
    SetTooLarge,
    /// Keys not in strictly ascending order (a duplicate key in the set is this too).
    SetNotSorted,
    /// A key that is not a canonical point encoding, or is of small order.
    BadKey { index: u32 },
    BadThreshold { threshold: u32, n: u32 },
    /// The ejected mask names an index outside the set.
    BadEjected,
    BadIndex { index: u32 },
    EjectedKey { index: u32 },
    InvalidSignature { index: u32 },
    NoSignatures,
    BelowThreshold { signers: u32, threshold: u32 },
}

impl core::fmt::Display for SigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptySet => write!(f, "empty signer set"),
            Self::SetTooLarge => write!(f, "signer set larger than {MAX_SIGNERS}"),
            Self::SetNotSorted => write!(f, "signer set keys not strictly ascending"),
            Self::BadKey { index } => write!(f, "set key {index} is not a canonical point encoding of large order"),
            Self::BadThreshold { threshold, n } => write!(f, "threshold {threshold} not in (n/2, n] for n = {n}"),
            Self::BadEjected => write!(f, "ejected mask names a key outside the set"),
            Self::BadIndex { index } => write!(f, "signature names key {index}, not in the set"),
            Self::EjectedKey { index } => write!(f, "key {index} is ejected; its signature does not count"),
            Self::InvalidSignature { index } => write!(f, "signature for key {index} does not verify"),
            Self::NoSignatures => write!(f, "no signatures"),
            Self::BelowThreshold { signers, threshold } => {
                write!(f, "{signers} distinct valid signers, threshold {threshold}")
            }
        }
    }
}

/// A canonical encoding of a point that is not of small order.
#[must_use]
pub fn key_ok(key: &[u8; 32]) -> bool {
    // VerifyingKey keeps the bytes it was given, so re-compress the decompressed point to see a
    // non-canonical encoding (y >= p, or x = 0 with the sign bit set).
    match CompressedEdwardsY(*key).decompress() {
        Some(p) => p.compress().to_bytes() == *key && !p.is_small_order(),
        None => false,
    }
}

/// `SHA256(SET_TAG | u32be(n) | keys)`, keys strictly ascending, each passing [`key_ok`].
pub fn set_hash(keys: &[[u8; 32]]) -> Result<[u8; 32], SigError> {
    if keys.is_empty() {
        return Err(SigError::EmptySet);
    }
    if keys.len() > MAX_SIGNERS {
        return Err(SigError::SetTooLarge);
    }
    if keys.windows(2).any(|w| w[0] >= w[1]) {
        return Err(SigError::SetNotSorted);
    }
    if let Some(i) = keys.iter().position(|k| !key_ok(k)) {
        return Err(SigError::BadKey { index: i as u32 });
    }
    let mut h = Sha256::new();
    h.update(SET_TAG);
    h.update((keys.len() as u32).to_be_bytes());
    for k in keys {
        h.update(k);
    }
    Ok(h.finalize().into())
}

/// Ed25519 `verify_strict` of `sig` by `key` over `msg`.
#[must_use]
pub fn verify_one(key: &[u8; 32], msg: &[u8; 32], sig: &[u8; 64]) -> bool {
    match VerifyingKey::from_bytes(key) {
        Ok(k) => k.verify_strict(msg, &Signature::from_bytes(sig)).is_ok(),
        Err(_) => false,
    }
}

/// The bitmap of distinct keys whose signatures over `cp` verify. Every signature must verify and
/// name a key of the set.
fn signer_bitmap(cp: &Checkpoint, keys: &[[u8; 32]], sigs: &[Sig]) -> Result<u64, SigError> {
    let msg = cp.message();
    let mut bitmap = 0u64;
    for s in sigs {
        let key = keys.get(s.key_index as usize).ok_or(SigError::BadIndex { index: s.key_index })?;
        if !verify_one(key, &msg, &s.sig) {
            return Err(SigError::InvalidSignature { index: s.key_index });
        }
        bitmap |= 1u64 << s.key_index;
    }
    Ok(bitmap)
}

/// What [`check`] accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub set_hash: [u8; 32],
    pub threshold: u32,
    pub ejected: u64,
    /// Distinct keys with a valid signature (bit i = key i).
    pub bitmap: u64,
    pub signers: u32,
}

/// The threshold rule the gate also enforces: `n/2 < threshold <= n`.
pub fn check_threshold(threshold: u32, n: usize) -> Result<(), SigError> {
    let n = n as u32;
    if threshold == 0 || threshold > n || 2 * threshold <= n {
        return Err(SigError::BadThreshold { threshold, n });
    }
    Ok(())
}

/// Accepts `cp` when at least `w.threshold` distinct, not ejected keys of `w.keys` signed it.
pub fn check(cp: &Checkpoint, w: &Witness) -> Result<Accepted, SigError> {
    let set_hash = set_hash(&w.keys)?;
    check_threshold(w.threshold, w.keys.len())?;
    let n = w.keys.len();
    if n < 64 && w.ejected >> n != 0 {
        return Err(SigError::BadEjected);
    }
    for s in &w.sigs {
        if (s.key_index as usize) < n && w.ejected & (1u64 << s.key_index) != 0 {
            return Err(SigError::EjectedKey { index: s.key_index });
        }
    }
    let bitmap = signer_bitmap(cp, &w.keys, &w.sigs)?;
    let signers = bitmap.count_ones();
    if signers < w.threshold {
        return Err(SigError::BelowThreshold { signers, threshold: w.threshold });
    }
    Ok(Accepted { set_hash, threshold: w.threshold, ejected: w.ejected, bitmap, signers })
}

fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// The 256 bytes the seq-sig guest appends to `abi.encode(UpdateJournal)`:
/// `abi.encode(bytes32 signerSetHash, uint256 threshold, uint256 ejectedMask, uint256 signerBitmap,
/// uint256 prevId, bytes32 prevHash, uint256 blockId, bytes32 blockHash)`.
#[must_use]
pub fn journal_tail(a: &Accepted, cp: &Checkpoint) -> [u8; TAIL_LEN] {
    let mut t = [0u8; TAIL_LEN];
    t[..32].copy_from_slice(&a.set_hash);
    t[32..64].copy_from_slice(&word_u64(u64::from(a.threshold)));
    t[64..96].copy_from_slice(&word_u64(a.ejected));
    t[96..128].copy_from_slice(&word_u64(a.bitmap));
    t[128..160].copy_from_slice(&word_u64(cp.prev_id));
    t[160..192].copy_from_slice(&cp.prev_hash);
    t[192..224].copy_from_slice(&word_u64(cp.block_id));
    t[224..].copy_from_slice(&cp.block_hash);
    t
}

/// Checks evidence (every signature must verify; at least one) and returns its journal (224 bytes):
/// `abi.encode(bytes32 cfgId, bytes32 signerSetHash, uint256 signerBitmap, uint256 prevId,
/// bytes32 prevHash, uint256 blockId, bytes32 blockHash)`.
pub fn evidence_journal(e: &Evidence) -> Result<[u8; EVIDENCE_LEN], SigError> {
    let set_hash = set_hash(&e.keys)?;
    if e.sigs.is_empty() {
        return Err(SigError::NoSignatures);
    }
    let bitmap = signer_bitmap(&e.checkpoint, &e.keys, &e.sigs)?;
    Ok(encode_evidence(&set_hash, bitmap, &e.checkpoint))
}

/// The evidence journal's bytes (no checks): `abi.encode(cfgId, signerSetHash, signerBitmap, prevId,
/// prevHash, blockId, blockHash)`.
#[must_use]
pub fn encode_evidence(set_hash: &[u8; 32], bitmap: u64, c: &Checkpoint) -> [u8; EVIDENCE_LEN] {
    let mut t = [0u8; EVIDENCE_LEN];
    t[..32].copy_from_slice(&c.cfg_id);
    t[32..64].copy_from_slice(set_hash);
    t[64..96].copy_from_slice(&word_u64(bitmap));
    t[96..128].copy_from_slice(&word_u64(c.prev_id));
    t[128..160].copy_from_slice(&c.prev_hash);
    t[160..192].copy_from_slice(&word_u64(c.block_id));
    t[192..].copy_from_slice(&c.block_hash);
    t
}

/// The public key of a 32-byte Ed25519 secret (seed).
#[must_use]
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(secret).verifying_key().to_bytes()
}

/// Signs a checkpoint (host side: the attestor tool).
#[must_use]
pub fn sign(secret: &[u8; 32], cp: &Checkpoint) -> [u8; 64] {
    SigningKey::from_bytes(secret).sign(&cp.message()).to_bytes()
}

/// Test vectors shared by the native and executor tests.
pub mod vectors {
    /// The identity point (order 1), canonical encoding.
    pub const SMALL_ORDER_IDENTITY: [u8; 32] = {
        let mut k = [0u8; 32];
        k[0] = 1;
        k
    };
    /// A point of order 8 (canonical encoding, from "Taming the many EdDSAs").
    pub const SMALL_ORDER_8: [u8; 32] = [
        0xc7, 0x17, 0x6a, 0x70, 0x3d, 0x4d, 0xd8, 0x4f, 0xba, 0x3c, 0x0b, 0x76, 0x0d, 0x10, 0x67, 0x0f, 0x2a, 0x20, 0x53, 0xfa, 0x2c, 0x39, 0xcc, 0xc6, 0x4e, 0xc7, 0xfd, 0x77, 0x92, 0xac, 0x03, 0x7a,
    ];
    /// The group order L, little-endian (added to S to make a non-canonical S).
    pub const L: [u8; 32] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];

    /// `s + L` (little-endian 256-bit add): the same signature with a non-canonical S.
    #[must_use]
    pub fn non_canonical_s(sig: &[u8; 64]) -> [u8; 64] {
        let mut out = *sig;
        let mut carry = 0u16;
        for i in 0..32 {
            let v = u16::from(sig[32 + i]) + u16::from(L[i]) + carry;
            out[32 + i] = v as u8;
            carry = v >> 8;
        }
        out
    }

    /// A non-canonical encoding of a valid point of large order: y + p for some small y on the
    /// curve (y in 2..19, so y + p < 2^255), found by trial.
    #[must_use]
    pub fn non_canonical_key() -> Option<[u8; 32]> {
        // p = 2^255 - 19, little-endian
        for y in 2u8..19 {
            let mut k = [0xffu8; 32];
            k[31] = 0x7f;
            k[0] = 0xed; // p's low byte
            // k = p; add y
            let (lo, c) = k[0].overflowing_add(y);
            k[0] = lo;
            if c {
                // p + y with y < 19 never carries past byte 0 (0xed + 18 = 0xff)
                continue;
            }
            if let Some(p) = super::CompressedEdwardsY(k).decompress() {
                if p.compress().to_bytes() != k && !p.is_small_order() {
                    return Some(k);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn keys(n: u8) -> (Vec<[u8; 32]>, Vec<[u8; 32]>) {
        let mut pairs: Vec<([u8; 32], [u8; 32])> = (1..=n).map(|i| {
            let s = [i; 32];
            (public_key(&s), s)
        }).collect();
        pairs.sort();
        (pairs.iter().map(|p| p.0).collect(), pairs.iter().map(|p| p.1).collect())
    }

    fn cp() -> Checkpoint {
        Checkpoint { cfg_id: [7; 32], prev_id: 10, prev_hash: [1; 32], block_id: 20, block_hash: [2; 32] }
    }

    fn witness(t: u32, signers: &[usize]) -> Witness {
        let (pk, sk) = keys(4);
        let sigs = signers.iter().map(|&i| Sig { key_index: i as u32, sig: sign(&sk[i], &cp()) }).collect();
        Witness { keys: pk, threshold: t, ejected: 0, sigs }
    }

    #[test]
    fn three_of_four_accepted() {
        let a = check(&cp(), &witness(3, &[0, 1, 3])).unwrap();
        assert_eq!(a.signers, 3);
        assert_eq!(a.bitmap, 0b1011);
        assert_eq!(a.set_hash, set_hash(&keys(4).0).unwrap());
    }

    #[test]
    fn two_of_four_refused() {
        assert_eq!(check(&cp(), &witness(3, &[0, 2])), Err(SigError::BelowThreshold { signers: 2, threshold: 3 }));
    }

    #[test]
    fn duplicate_key_counted_once() {
        assert_eq!(check(&cp(), &witness(3, &[1, 1, 2])), Err(SigError::BelowThreshold { signers: 2, threshold: 3 }));
    }

    #[test]
    fn ejected_key_does_not_count() {
        let mut w = witness(3, &[0, 1, 2]);
        w.ejected = 0b0010;
        assert_eq!(check(&cp(), &w), Err(SigError::EjectedKey { index: 1 }));
        let mut w = witness(3, &[0, 2, 3]);
        w.ejected = 0b0010;
        assert_eq!(check(&cp(), &w).unwrap().ejected, 0b0010);
        w.ejected = 1 << 4;
        assert_eq!(check(&cp(), &w), Err(SigError::BadEjected));
    }

    #[test]
    fn key_outside_the_set_refused() {
        let mut w = witness(3, &[0, 1]);
        let outsider = [99u8; 32];
        w.sigs.push(Sig { key_index: 2, sig: sign(&outsider, &cp()) });
        assert_eq!(check(&cp(), &w), Err(SigError::InvalidSignature { index: 2 }));
        w.sigs.pop();
        w.sigs.push(Sig { key_index: 4, sig: sign(&outsider, &cp()) });
        assert_eq!(check(&cp(), &w), Err(SigError::BadIndex { index: 4 }));
    }

    #[test]
    fn signatures_bind_cfg_and_history() {
        let w = witness(3, &[0, 1, 2]);
        for other in [
            Checkpoint { cfg_id: [8; 32], ..cp() },
            Checkpoint { prev_id: 11, ..cp() },
            Checkpoint { prev_hash: [9; 32], ..cp() },
            Checkpoint { block_id: 21, ..cp() },
            Checkpoint { block_hash: [3; 32], ..cp() },
        ] {
            assert!(matches!(check(&other, &w), Err(SigError::InvalidSignature { .. })));
        }
    }

    #[test]
    fn threshold_and_set_rules() {
        let (mut pk, _) = keys(4);
        for t in [0, 1, 2, 5] {
            assert!(check_threshold(t, 4).is_err(), "t = {t}");
        }
        assert!(check_threshold(3, 4).is_ok() && check_threshold(4, 4).is_ok());
        assert!(check_threshold(2, 3).is_ok() && check_threshold(1, 1).is_ok() && check_threshold(1, 2).is_err());
        pk.swap(0, 1);
        assert_eq!(set_hash(&pk), Err(SigError::SetNotSorted));
        let (pk, _) = keys(4);
        assert_eq!(set_hash(&[pk[0], pk[0]]), Err(SigError::SetNotSorted));
        assert_eq!(set_hash(&[]), Err(SigError::EmptySet));
    }

    /// T = n, and the largest set (64 keys, threshold 64; one missing signature refuses).
    #[test]
    fn edges_t_equals_n_and_max_signers() {
        assert!(check(&cp(), &witness(4, &[0, 1, 2, 3])).is_ok());
        assert!(check(&cp(), &witness(4, &[0, 1, 2])).is_err());
        let (pk, sk) = keys(64);
        let all: Vec<Sig> = (0..64).map(|i| Sig { key_index: i as u32, sig: sign(&sk[i], &cp()) }).collect();
        let w = Witness { keys: pk.clone(), threshold: 64, ejected: 0, sigs: all.clone() };
        assert_eq!(check(&cp(), &w).unwrap().bitmap, u64::MAX);
        let w = Witness { keys: pk.clone(), threshold: 64, ejected: 0, sigs: all[1..].to_vec() };
        assert!(check(&cp(), &w).is_err());
        let (pk65, _) = keys(65);
        assert_eq!(set_hash(&pk65), Err(SigError::SetTooLarge));
    }

    #[test]
    fn bad_keys_refused() {
        let (mut pk, _) = keys(3);
        for bad in [vectors::SMALL_ORDER_IDENTITY, vectors::SMALL_ORDER_8, vectors::non_canonical_key().unwrap()] {
            pk.push(bad);
            pk.sort();
            assert!(matches!(set_hash(&pk), Err(SigError::BadKey { .. })), "{bad:?}");
            pk.retain(|k| *k != bad);
        }
    }

    #[test]
    fn non_canonical_s_refused() {
        let (pk, sk) = keys(4);
        let good = sign(&sk[0], &cp());
        assert!(verify_one(&pk[0], &cp().message(), &good));
        assert!(!verify_one(&pk[0], &cp().message(), &vectors::non_canonical_s(&good)));
    }

    #[test]
    fn evidence() {
        let (pk, sk) = keys(4);
        let c = Checkpoint { block_hash: [5; 32], ..cp() };
        let e = Evidence { checkpoint: c, keys: pk.clone(), sigs: vec![Sig { key_index: 2, sig: sign(&sk[2], &c) }, Sig { key_index: 3, sig: sign(&sk[3], &c) }] };
        let j = evidence_journal(&e).unwrap();
        assert_eq!(&j[..32], &[7; 32]);
        assert_eq!(&j[32..64], &set_hash(&pk).unwrap());
        assert_eq!(j[95], 0b1100);
        assert_eq!(j[127], 10);
        assert_eq!(&j[128..160], &[1; 32]);
        assert_eq!(j[191], 20);
        assert_eq!(&j[192..], &[5; 32]);
        let bad = Evidence { sigs: vec![Sig { key_index: 1, sig: sign(&sk[2], &c) }], ..e.clone() };
        assert!(evidence_journal(&bad).is_err());
        assert_eq!(evidence_journal(&Evidence { sigs: vec![], ..e }), Err(SigError::NoSignatures));
    }

    /// Golden vectors, the same bytes as `vault/test/SeqSigGate.t.sol` `test_GoldenVectorsMatchRust`.
    #[test]
    fn golden_abi_vectors() {
        let cp = Checkpoint { cfg_id: [0x44; 32], prev_id: 0x0102030405060708, prev_hash: [0x22; 32], block_id: 0x1112131415161718, block_hash: [0x33; 32] };
        let a = Accepted { set_hash: [0x11; 32], threshold: 3, ejected: 2, bitmap: 0xd, signers: 3 };
        let want_tail = concat!(
            "1111111111111111111111111111111111111111111111111111111111111111",
            "0000000000000000000000000000000000000000000000000000000000000003",
            "0000000000000000000000000000000000000000000000000000000000000002",
            "000000000000000000000000000000000000000000000000000000000000000d",
            "0000000000000000000000000000000000000000000000000102030405060708",
            "2222222222222222222222222222222222222222222222222222222222222222",
            "0000000000000000000000000000000000000000000000001112131415161718",
            "3333333333333333333333333333333333333333333333333333333333333333",
        );
        assert_eq!(hexs(&journal_tail(&a, &cp)), want_tail);
        let ev = encode_evidence(&[0x11; 32], 5, &Checkpoint { block_hash: [0x55; 32], ..cp });
        let want_ev = concat!(
            "4444444444444444444444444444444444444444444444444444444444444444",
            "1111111111111111111111111111111111111111111111111111111111111111",
            "0000000000000000000000000000000000000000000000000000000000000005",
            "0000000000000000000000000000000000000000000000000102030405060708",
            "2222222222222222222222222222222222222222222222222222222222222222",
            "0000000000000000000000000000000000000000000000001112131415161718",
            "5555555555555555555555555555555555555555555555555555555555555555",
        );
        assert_eq!(hexs(&ev), want_ev);
    }

    /// RFC 8032 section 7.1, TEST 2 (one-byte message 0x72): the implementation is standard Ed25519.
    #[test]
    fn rfc8032_test2() {
        let sk: [u8; 32] = hex32("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb");
        let pk = public_key(&sk);
        assert_eq!(pk, hex32("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"));
        let sig = SigningKey::from_bytes(&sk).sign(&[0x72]).to_bytes();
        let want = "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00";
        assert_eq!(hexs(&sig), want);
    }

    fn hex32(s: &str) -> [u8; 32] {
        let mut o = [0u8; 32];
        for i in 0..32 {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    }

    fn hexs(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
