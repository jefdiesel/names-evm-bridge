//! G2-lite, the RFP-022 outbound state proof without the Logos Blockchain finality layer.
//!
//! # What one update proves
//!
//! Input: the previous G2 state (its preimage, [`state::G2State`]), the bridge view it commits to
//! ([`view::BridgeView`]), a batch of consecutive LEZ blocks as the sequencer serves them (borsh
//! `common::block::Block`), one insertion witness per spent claim nullifier, and a witness for the
//! [`finality::FinalitySource`] the guest was built with. The statement ([`update::update`]):
//!
//! 1. **Linkage.** Block `i` has id `tip_id + i + 1`, its `prev_block_hash` is the previous block's
//!    hash, and its hash is recomputed here as LEZ does (`SHA256(prefix | borsh(HashableBlockData))`,
//!    `lez/common/src/block.rs`); the header's stored hash must equal it.
//! 2. **Finality.** The finality source accepts the batch. The only implementation that works
//!    today is [`finality::TrustedTip`] (test only, `trusted-tip` feature); the Logos Blockchain
//!    light client is a placeholder that refuses.
//! 3. **Extraction.** Every transaction not listed in the block's `reverted` set is parsed. Those
//!    that reach OUR bridge program (its id is in the state; the vault's `cfgId` binds it) or OUR
//!    token program (from the bridge's `Init`) give claims, burns, stand-in roots and registry
//!    changes ([`view`]). A private transaction that reaches them has its privacy-circuit receipt
//!    checked first ([`update::ReceiptVerifier`]: `env::verify` in the guest).
//! 4. **Accumulator.** Each claim's nullifier PDA is inserted, with proof, into the sorted
//!    nullifier tree (the `claim_acc` crate, an indexed Merkle tree); a nullifier already in it is refused.
//! 5. **Output.** The new state commitment and the journal the vault's `update` verifies:
//!    `abi.encode(UpdateJournal)` ([`journal`]), so `sha256(journal)` is the digest it checks.
//!
//! # Trust (see also the crate README in DESIGN §4)
//!
//! Cryptographic here: block linkage and hashes, the boundary messages of private transactions
//! (their receipt), the accumulator, the journal. Resting on the LEZ sequencer committee, as LEZ
//! 0.4 itself does: that a block's public execution is valid (LEZ never proves it), so a public
//! instruction that is not marked reverted did what the bridge program does with it; that a
//! private transaction's roots, nullifiers and program image claims passed the sequencer's
//! checks; and, until the light client exists, that the blocks are final at all.
//!
//! A transaction the sequencer marked successful but that the bridge program would have refused
//! (a second spend of a nullifier, a burn that is not one denomination, burns above claims, an
//! unregistered token) is a contradiction: the update fails and no journal exists. G2 stops rather
//! than prove past a block it cannot explain.

pub mod finality;
pub mod journal;
pub mod state;
pub mod update;
pub mod view;
pub mod wire;

pub use claim_acc;

use risc0_zkvm::sha::{Impl, Sha256 as _};

/// Image id of LEZ v0.4.0-rc3's privacy-preserving circuit (`lee::PRIVACY_PRESERVING_CIRCUIT_ID`,
/// the image id of `artifacts/lee/privacy_preserving_circuit/privacy_preserving_circuit.bin` at
/// bafd07be), as the 32 bytes of its RISC Zero digest. Pinned: a private transaction's boundary
/// counts only if its receipt verifies against this image. `0x631f0c96…0c38`: `outbound_host
/// circuit-id` of the rc3 artifact (the CLI's `circuit_tests` checks LEZ's own constant gives the
/// same). The circuit changed from rc1 (a program-image witness may claim an account only once).
pub const PRIVACY_PRESERVING_CIRCUIT_ID: [u8; 32] = [
    0x63, 0x1f, 0x0c, 0x96, 0x57, 0x2a, 0x86, 0xce, 0x63, 0xd1, 0xb9, 0xc4, 0xe9, 0x8c, 0x83, 0x83, 0xca, 0xbb, 0x01, 0x6b, 0x3c, 0x85, 0x20, 0x17, 0x10, 0x14, 0x52, 0xb9, 0x92, 0xcf, 0x0c, 0x38,
];

/// LEZ v0.4.0-rc1's circuit (`0x8fc948d1…f257`, `outbound_host circuit-id` of the rc1 artifact).
/// Only for the rc1 private fixtures in `outbound/fixtures/private`, whose receipts verify under it
/// natively; the guest never uses it.
pub const PRIVACY_PRESERVING_CIRCUIT_ID_RC1: [u8; 32] = [
    0x8f, 0xc9, 0x48, 0xd1, 0x17, 0xc1, 0x1d, 0x55, 0xf2, 0x03, 0x67, 0x48, 0x01, 0x3b, 0x3d, 0xc5, 0x99, 0xd6, 0x99, 0x55, 0x45, 0xf0, 0xb0, 0x20, 0xd0, 0x90, 0xf5, 0x44, 0x08, 0x1c, 0xf2, 0x57,
];

/// SHA-256 of the concatenation (RISC Zero's accelerated implementation in the guest).
#[must_use]
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let bytes: Vec<u8> = parts.concat();
    let mut out = [0u8; 32];
    out.copy_from_slice(Impl::hash_bytes(&bytes).as_bytes());
    out
}

/// SHA-256 of two 32-byte words (a Merkle node), without a heap allocation.
#[must_use]
pub fn sha256_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(a);
    buf[32..].copy_from_slice(b);
    let mut out = [0u8; 32];
    out.copy_from_slice(Impl::hash_bytes(&buf).as_bytes());
    out
}

/// Why an update is refused. The guest panics with its `Display`; no journal exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum G2Error {
    /// The input does not decode (wrong layout, trailing bytes).
    Input(&'static str),
    /// The previous state's view or accumulator does not match its commitment.
    PrevState(&'static str),
    /// A block breaks the chain: wrong id, wrong parent, or a stored hash that is not its hash.
    Linkage { block_id: u64, what: &'static str },
    /// A block or transaction does not decode as LEZ 0.4 borsh.
    Decode { block_id: u64, what: &'static str },
    /// The finality source refused the batch.
    Finality(finality::FinalityError),
    /// A private transaction's receipt does not verify against the pinned circuit.
    Receipt { block_id: u64, tx: usize },
    /// A transaction the sequencer marked successful that the bridge program would have refused.
    Inconsistent { block_id: u64, tx: usize, what: &'static str },
    /// An accumulator insertion witness is wrong (or the key is already in the tree).
    Accumulator { key: [u8; 32], what: &'static str },
    /// The batch is empty: an update must advance the tip.
    EmptyBatch,
}

impl core::fmt::Display for G2Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Input(w) => write!(f, "input: {w}"),
            Self::PrevState(w) => write!(f, "previous state: {w}"),
            Self::Linkage { block_id, what } => write!(f, "block {block_id}: linkage: {what}"),
            Self::Decode { block_id, what } => write!(f, "block {block_id}: decode: {what}"),
            Self::Finality(e) => write!(f, "finality: {e}"),
            Self::Receipt { block_id, tx } => {
                write!(f, "block {block_id} tx {tx}: privacy-circuit receipt does not verify")
            }
            Self::Inconsistent { block_id, tx, what } => {
                write!(f, "block {block_id} tx {tx}: applied by the sequencer but the bridge would refuse it: {what}")
            }
            Self::Accumulator { key, what } => {
                write!(f, "accumulator: key {}: {what}", hex(key))
            }
            Self::EmptyBatch => write!(f, "empty batch"),
        }
    }
}

/// Lower-case hex, for messages.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}
