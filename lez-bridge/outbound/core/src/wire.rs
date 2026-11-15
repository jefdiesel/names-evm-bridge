//! LEZ 0.4 block and transaction wire formats, and the guest's parser.
//!
//! The wrapper types live in LEZ crates that do not build for a guest (`common`, `lee`); they are
//! mirrored here field for field from v0.4.0-rc1, unchanged in v0.4.0-rc3 bafd07be (`lez/common/src/block.rs`,
//! `lez/common/src/transaction.rs`, `lee/state_machine/src/{public,privacy_preserving}_transaction`,
//! `lee/state_machine/src/transaction_message.rs`, `fees.rs`, `signature/`). The inner types
//! (`PublicExecutionContext`, `RootCall`, `ProvenExecution`, ...) are `lee_core`'s own. The host
//! checks the mirror on every block of the local chain: strict decode, byte-identical re-encode,
//! recomputed hash equal to the stored one.
//!
//! The block hash is `SHA256(PREFIX | borsh(HashableBlockData))` where `HashableBlockData =
//! {block_id, prev_block_hash, timestamp, transactions, reverted}`: the header without its own
//! `hash`, the body, and not `bedrock_status`. In the borsh bytes of a `Block` those are the
//! slices `[0..40] | [72..80] | [80..len - 1]`, so the hash is computed over the bytes as served,
//! with no re-serialisation.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    ProvenExecution, RootCall,
    account::{AccountId, Nonce},
    execution_state::PublicExecutionContext,
    program::PdaSeed,
};
use risc0_zkvm::sha::rust_crypto::{Digest as _, Sha256};
use std::collections::BTreeMap;

/// `HashableBlockData::compute_hash`'s domain prefix (`lez/common/src/block.rs`).
pub const BLOCK_HASH_PREFIX: &[u8; 32] = b"/LEE/v0.3/Message/Block/\x00\x00\x00\x00\x00\x00\x00\x00";

// ------------------------------------------------------------------------------- mirror types

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FeeDeclaration {
    pub payer: AccountId,
    pub gas_limit: u64,
    pub tip: u64,
    pub max_fee: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum PublicAccountEvidence {
    /// `lee::PublicKey`, 32 bytes.
    Key([u8; 32]),
    Pda { program: AccountId, seed: PdaSeed },
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct TransactionMessage<P> {
    pub public_context: PublicExecutionContext,
    pub execution: P,
    pub nonces: BTreeMap<AccountId, Nonce>,
    pub admission_evidence: Vec<PublicAccountEvidence>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PublicExecution {
    pub root: RootCall,
    pub fee: Option<FeeDeclaration>,
}

/// `(lee::Signature { value: [u8; 64] }, lee::PublicKey([u8; 32]))`.
pub type SignatureAndKey = ([u8; 64], [u8; 32]);

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PublicWitnessSet {
    pub signatures_and_public_keys: Vec<SignatureAndKey>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PrivateWitnessSet {
    pub signatures_and_public_keys: Vec<SignatureAndKey>,
    /// `Proof(borsh(risc0 InnerReceipt))`, a succinct receipt of the privacy circuit.
    pub proof: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PublicTransaction {
    pub message: TransactionMessage<PublicExecution>,
    pub witness_set: PublicWitnessSet,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct PrivacyPreservingTransaction {
    pub message: TransactionMessage<ProvenExecution>,
    pub witness_set: PrivateWitnessSet,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub enum LeeTransaction {
    Public(PublicTransaction),
    PrivacyPreserving(PrivacyPreservingTransaction),
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub block_id: u64,
    pub prev_block_hash: [u8; 32],
    pub hash: [u8; 32],
    pub timestamp: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct BlockBody {
    pub transactions: Vec<LeeTransaction>,
    /// Indices of the charged transactions whose action reverted, ascending.
    pub reverted: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

/// `common::block::Block`, for whole-block decode/re-encode checks on the host.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub body: BlockBody,
    pub bedrock_status: BedrockStatus,
}

// ------------------------------------------------------------------------------- the parser

/// One transaction as G2 needs it.
pub enum ParsedTx<'a> {
    /// A public transaction: only its root call matters (its effects are the program's, run by
    /// the sequencer and never proven).
    Public { root: RootCall },
    /// A privacy-preserving transaction: its proven execution (the boundary messages), the bytes
    /// of `public_context | execution` (the receipt's journal is their 4-byte-length frame), and
    /// the proof bytes (`borsh(InnerReceipt)`), borrowed from the block.
    Private { execution: ProvenExecution, journal_payload: &'a [u8], proof: &'a [u8] },
}

/// A parsed block with its recomputed hash.
pub struct ParsedBlock<'a> {
    pub header: BlockHeader,
    /// `SHA256(PREFIX | borsh(HashableBlockData))`, recomputed from the bytes.
    pub hash: [u8; 32],
    pub txs: Vec<ParsedTx<'a>>,
    pub reverted: Vec<u64>,
}

impl ParsedBlock<'_> {
    #[must_use]
    pub fn is_reverted(&self, i: usize) -> bool {
        self.reverted.contains(&(i as u64))
    }
}

/// Parse failures, by the part that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

fn de<T: BorshDeserialize>(cur: &mut &[u8], what: &'static str) -> Result<T, DecodeError> {
    T::deserialize(cur).map_err(|_| DecodeError(what))
}

fn take<'a>(cur: &mut &'a [u8], n: usize, what: &'static str) -> Result<&'a [u8], DecodeError> {
    if cur.len() < n {
        return Err(DecodeError(what));
    }
    let (a, b) = cur.split_at(n);
    *cur = b;
    Ok(a)
}

fn len_u32(cur: &mut &[u8], what: &'static str) -> Result<usize, DecodeError> {
    let b = take(cur, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// Recomputes a block's hash from its borsh bytes (see the module docs).
pub fn block_hash(raw: &[u8]) -> Result<[u8; 32], DecodeError> {
    if raw.len() < 81 {
        return Err(DecodeError("block shorter than a header"));
    }
    let mut h = Sha256::new();
    h.update(BLOCK_HASH_PREFIX);
    h.update(&raw[0..40]); // block_id, prev_block_hash
    h.update(&raw[72..80]); // timestamp (skipping the stored hash)
    h.update(&raw[80..raw.len() - 1]); // body (without bedrock_status)
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    Ok(out)
}

/// Parses a borsh `common::block::Block` exactly (no trailing bytes), keeping only what G2 reads.
pub fn parse_block(raw: &[u8]) -> Result<ParsedBlock<'_>, DecodeError> {
    let mut cur = raw;
    let header: BlockHeader = de(&mut cur, "header")?;
    let n = len_u32(&mut cur, "transaction count")?;
    let mut txs = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let tag = take(&mut cur, 1, "transaction tag")?[0];
        match tag {
            0 => {
                let tx: PublicTransaction = de(&mut cur, "public transaction")?;
                txs.push(ParsedTx::Public { root: tx.message.execution.root });
            }
            1 => {
                let start = cur;
                let _context: PublicExecutionContext = de(&mut cur, "private public_context")?;
                let execution: ProvenExecution = de(&mut cur, "private execution")?;
                let journal_payload = &start[..start.len() - cur.len()];
                let _nonces: BTreeMap<AccountId, Nonce> = de(&mut cur, "private nonces")?;
                let _evidence: Vec<PublicAccountEvidence> = de(&mut cur, "private admission evidence")?;
                let sigs = len_u32(&mut cur, "private signature count")?;
                take(&mut cur, sigs.checked_mul(96).ok_or(DecodeError("signature count"))?, "private signatures")?;
                let proof_len = len_u32(&mut cur, "proof length")?;
                let proof = take(&mut cur, proof_len, "proof")?;
                txs.push(ParsedTx::Private { execution, journal_payload, proof });
            }
            _ => return Err(DecodeError("unknown transaction kind")),
        }
    }
    let reverted: Vec<u64> = de(&mut cur, "reverted")?;
    if reverted.windows(2).any(|w| w[0] >= w[1]) || reverted.iter().any(|&i| i >= n as u64) {
        return Err(DecodeError("reverted indices not ascending or out of range"));
    }
    let status = take(&mut cur, 1, "bedrock status")?[0];
    if status > 2 {
        return Err(DecodeError("bedrock status"));
    }
    if !cur.is_empty() {
        return Err(DecodeError("trailing bytes after the block"));
    }
    let hash = block_hash(raw)?;
    Ok(ParsedBlock { header, hash, txs, reverted })
}

/// The receipt journal of a private transaction: `to_borsh_frame(&(public_context, execution))`,
/// i.e. the 4-byte little-endian length then those bytes (`lee_core::to_frame`).
#[must_use]
pub fn journal_frame(payload: &[u8]) -> Vec<u8> {
    lee_core::to_frame(payload)
}
