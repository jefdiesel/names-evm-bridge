//! Types shared by the host and the two guests (risc0 serde, plain data only).
#![no_std]
extern crate alloc;
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

/// Named cycle marks: (label, cycles spent in that step), from `env::cycle_count()` deltas.
/// `cycle_count()` is user cycles inside the guest; it excludes paging and padding,
/// which the host reads from the Session.
pub type Marks = Vec<(String, u64)>;

/// Guest A input header. Bulk data (SSZ objects) follows as frames.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConsensusParams {
    /// 0 = verify only (headline), 1 = verify + BLS/SSZ breakdown replay.
    pub mode: u32,
    pub genesis_validators_root: [u8; 32],
    /// Trusted weak-subjectivity checkpoint: beacon block root the bootstrap must match.
    pub checkpoint: [u8; 32],
    /// (epoch, fork_version) for genesis, altair, bellatrix, capella, deneb, electra, fulu, gloas.
    pub forks: [(u64, [u8; 4]); 8],
    /// Upper bound on signature_slot (Helios `expected_current_slot`). LEZ would take this
    /// from its own clock; here the host passes the finality update's signature slot.
    pub expected_current_slot: u64,
    /// Number of committee-period updates (`light_client/updates` items) that follow the bootstrap.
    pub n_updates: u32,
}

/// Guest A journal.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConsensusOutput {
    pub genesis_validators_root: [u8; 32],
    pub checkpoint: [u8; 32],
    pub finalized_header_root: [u8; 32],
    pub finalized_slot: u64,
    /// 0 = Gloas: `execution_block_hash` (bid.parent_block_hash; the state root needs the EL header),
    /// 1 = Capella..Fulu: execution payload header `state_root` directly.
    pub execution_kind: u8,
    pub execution_commitment: [u8; 32],
    pub current_sync_committee_root: [u8; 32],
    pub next_sync_committee_root: [u8; 32],
    pub marks: Marks,
}

/// Guest B input header. Header RLP and proof nodes follow as frames.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InclusionParams {
    pub chain_id: u64,
    /// Execution block hash committed by guest A (finalized).
    pub block_hash: [u8; 32],
    pub address: [u8; 20],
    /// Solidity mapping(bytes32 => bool) key and the mapping's declared slot index N.
    pub mapping_key: [u8; 32],
    pub mapping_slot_index: u64,
    /// Claimed account fields (checked by the account proof).
    pub nonce: u64,
    pub balance: [u8; 32],
    pub storage_root: [u8; 32],
    pub code_hash: [u8; 32],
    /// Claimed storage value (big-endian U256; checked by the storage proof).
    pub value: [u8; 32],
    pub n_account_nodes: u32,
    pub n_storage_nodes: u32,
}

/// Guest B journal.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InclusionOutput {
    pub chain_id: u64,
    pub block_hash: [u8; 32],
    pub block_number: u64,
    pub state_root: [u8; 32],
    pub address: [u8; 20],
    pub slot_key: [u8; 32],
    pub value: [u8; 32],
    pub marks: Marks,
}
