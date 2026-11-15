//! Shared types for the lcbench spike (EXECUTOR ONLY, branch `lc-cycles`).
//!
//! Every byte string here is prepared by the host from the recorded fixtures; the guests
//! measure cycle counts of alternative signature paths over the same data.

use borsh::{BorshDeserialize, BorshSerialize};

/// SSZ depth of `Vector[BLSPubkey, 512]`.
pub const KEY_DEPTH: usize = 9;

/// One committee member whose key the signature path needs (a non-signer).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct KeyWitness {
    /// Position in the committee (0..512).
    pub index: u16,
    /// The committed bytes (`SyncCommittee.pubkeys[index]`).
    pub compressed: [u8; 48],
    /// The same point uncompressed (x ‖ y, 96 bytes, big endian, ZCash flags clear).
    pub uncompressed: [u8; 96],
    /// SSZ branch from `hash_tree_root(pubkey)` up to the pubkeys vector root, leaf level first.
    pub branch: [[u8; 32]; KEY_DEPTH],
}

/// The keys an aggregate-minus-non-signers check needs.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Keys {
    pub agg_compressed: [u8; 48],
    pub agg_uncompressed: [u8; 96],
    /// Non-signers in index order.
    pub absent: Vec<KeyWitness>,
    /// `hash_tree_root(pubkeys)`; with the aggregate's leaf it hashes to the committee root.
    pub pubkeys_root: [u8; 32],
    /// `hash_tree_root(SyncCommittee)`, the root the beacon state commits.
    pub committee_root: [u8; 32],
}

/// A real signature check: the Sepolia finality update fixture.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct SigCase {
    pub keys: Keys,
    /// The update's sync-committee bits (512).
    pub bits: Vec<bool>,
    /// `compute_signing_root(attested header root, domain)`, computed natively by the host.
    pub signing_root: [u8; 32],
    pub signature: [u8; 96],
}

/// Input of the zkcrypto guest.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct ZkInput {
    /// borsh of the `ReceiveInput` the program gets for this update (as `attest/cycles` builds it).
    pub receive_input: Vec<u8>,
    /// The committee SSZ (24,624 bytes) the program is given as witness.
    pub committee_ssz: Vec<u8>,
    pub sig: SigCase,
    /// Synthetic 342/512 participation on the same committee: 170 non-signers (cost only, no
    /// signature exists for it).
    pub floor: Keys,
}

/// Input of the blst guest (no Helios, no LEZ types).
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct BlstInput {
    pub sig: SigCase,
    pub floor: Keys,
}

/// What a guest commits: marks (label, cycles since the previous mark) and verdicts.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct Report {
    pub marks: Vec<(String, u64)>,
    pub verdicts: Vec<(String, bool)>,
}
