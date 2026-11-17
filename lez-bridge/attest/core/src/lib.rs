//! RFP-021 Ethereum state attestation for LEZ 0.4: the light half.
//!
//! This crate holds everything a *consumer* of the attestation program needs, and nothing that
//! needs BLS12-381: the account layout and addresses, the message types, the verified statements,
//! and the inclusion library ([`inclusion`]) that a consumer runs inside its own execution. The
//! light-client verification itself (Helios) lives in the `eth_attest` program crate, which a
//! consumer never links.
//!
//! # Accounts (all PDAs of the attestation program, per instance)
//!
//! - **config** `config_address(program, instance_id)`: the [`InstanceConfig`], written once by
//!   [`Effect::Create`]. No message rewrites it. The instance id is
//!   `SHA256(INSTANCE_TAG ‖ program ‖ borsh(config))` ([`instance_id`]), so the config is bound
//!   into the address itself: another config is another instance.
//! - **store** `store_address(program, instance_id)`: the light-client store as an [`LcSummary`]:
//!   the finalized beacon header with its execution anchor, and SHA-256 commitments to the current
//!   and next sync committees (not the 2 × 24,624 key bytes; see [`committee_hash`]).
//! - **anchor** `anchor_address(program, instance_id, anchor)`: one per execution anchor the
//!   instance ever finalized ([`Anchor::BlockHash`] always, [`Anchor::StateRoot`] when the
//!   finalized light-client header carries the state root, i.e. before Gloas). Never deleted, so a
//!   claim against an old anchor never expires.
//!
//! # Consuming an attestation
//!
//! A consumer program, inside its own (normally private) execution:
//! 1. verifies the inclusion of an account field, storage slot or receipt log under the anchor
//!    with [`inclusion`] and evaluates its predicate, getting a statement;
//! 2. sends [`assert_anchor`] to the attestation program: a public, read-only check that the
//!    anchor is one this instance finalized. It is the *only* public trace of the attestation:
//!    (attestation program, instance id, chain id, anchor). The account, slot, value, log and the
//!    consumer's own account stay inside the consumer's execution.
//!
//! Both happen in the consumer's transaction, so the action and the verification it rests on
//! cannot come apart (RFP-021 Functionality #7).

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, SyncMessage},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

pub mod inclusion;
#[cfg(feature = "testkit")]
pub mod testkit;

/// Sync-committee size on every Ethereum network (mainnet preset).
pub const SYNC_COMMITTEE_SIZE: u16 = 512;
/// The smallest participation that is a 2/3 supermajority of 512: `ceil(2 · 512 / 3)`.
pub const MIN_SUPERMAJORITY: u16 = 342;
/// SSZ size of a `SyncCommittee`: 512 compressed G1 keys plus the aggregate key.
pub const COMMITTEE_SSZ_LEN: usize = 512 * 48 + 48;

const INSTANCE_TAG: &[u8] = b"RFP021/instance/v1";
const COMMITTEE_TAG: &[u8] = b"RFP021/committee/v1";
const CONFIG_SEED_DOMAIN: &[u8] = b"/RFP021/attest/config/v1";
const STORE_SEED_DOMAIN: &[u8] = b"/RFP021/attest/store/v1";
const ANCHOR_SEED_DOMAIN: &[u8] = b"/RFP021/attest/anchor/v1";

// ---------------------------------------------------------------------------------------------
// Hashes

#[must_use]
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let bytes: Vec<u8> = parts.concat();
    Impl::hash_bytes(&bytes)
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

/// The commitment the store keeps for a sync committee: `SHA256(COMMITTEE_TAG ‖ ssz)`, over the
/// committee's 24,624-byte SSZ encoding (fixed size, so the encoding is canonical). A flat hash
/// costs about a fifth of the committee's SSZ `hash_tree_root` in the zkVM; the submitter checks
/// the committee it fetched from a beacon node against it.
#[must_use]
pub fn committee_hash(ssz: &[u8]) -> [u8; 32] {
    sha256(&[COMMITTEE_TAG, ssz])
}

// ---------------------------------------------------------------------------------------------
// Instance configuration (immutable)

/// A consensus fork: activation epoch and fork version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Fork {
    pub epoch: u64,
    pub version: [u8; 4],
}

/// Everything that varies between Ethereum networks, fixed when an instance is created
/// (RFP-021 Functionality #9, #13, #14). Its hash, with the program id, is the instance id.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InstanceConfig {
    /// EIP-155 chain id of the execution chain (1 mainnet, 11155111 Sepolia, ...).
    pub evm_chain_id: u64,
    /// The consensus network's genesis validators root (signing domain).
    pub genesis_validators_root: [u8; 32],
    /// Beacon genesis time in seconds and the slot length; bound the signature slot against the
    /// LEZ block time (an update signed for a future slot is refused by the sequencer).
    pub genesis_time: u64,
    pub seconds_per_slot: u64,
    /// genesis, altair, bellatrix, capella, deneb, electra, fulu, gloas, in that order. A fork
    /// not scheduled has epoch `u64::MAX`.
    pub forks: [Fork; 8],
    /// The weak-subjectivity checkpoint: the beacon block root the bootstrap must hash to.
    pub checkpoint: [u8; 32],
    /// Finality policy: the fewest sync-committee signers an update may have. At least
    /// [`MIN_SUPERMAJORITY`].
    pub min_participants: u16,
    /// Free; lets two otherwise identical configurations be separate instances.
    pub salt: [u8; 32],
}

impl InstanceConfig {
    /// Panics unless the configuration is usable. Checked when the instance is created.
    pub fn validate(&self) {
        assert!(self.evm_chain_id != 0, "chain id 0");
        assert!(self.seconds_per_slot != 0, "slot length 0");
        assert!(
            (MIN_SUPERMAJORITY..=SYNC_COMMITTEE_SIZE).contains(&self.min_participants),
            "participation policy must be a 2/3 supermajority of 512 or more"
        );
        assert!(
            self.forks.windows(2).all(|w| w[0].epoch <= w[1].epoch),
            "fork epochs must not decrease"
        );
    }
}

/// `SHA256(INSTANCE_TAG ‖ program ‖ borsh(config))`.
#[must_use]
pub fn instance_id(program: &AccountId, config: &InstanceConfig) -> [u8; 32] {
    sha256(&[INSTANCE_TAG, program.value(), &borsh::to_vec(config).unwrap_or_else(|_| panic!("config serializes"))])
}

// ---------------------------------------------------------------------------------------------
// Light-client store

/// A beacon block header (SSZ field order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
pub struct BeaconHeader {
    pub slot: u64,
    pub proposer_index: u64,
    pub parent_root: [u8; 32],
    pub state_root: [u8; 32],
    pub body_root: [u8; 32],
}

/// The execution block a finalized beacon header commits to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
pub struct ExecutionAnchor {
    /// Always present. On Gloas this is the light-client header's `execution_block_hash` (the
    /// last delivered payload); before it, the payload header's `block_hash`.
    pub block_hash: [u8; 32],
    /// Only before Gloas, where the light-client header carries the payload header.
    pub state_root: Option<[u8; 32]>,
    pub block_number: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
pub struct Finalized {
    pub beacon: BeaconHeader,
    /// `hash_tree_root(beacon)`.
    pub beacon_root: [u8; 32],
    pub execution: ExecutionAnchor,
}

impl Finalized {
    /// The anchors this header lets consumers use.
    #[must_use]
    pub fn anchors(&self) -> Vec<Anchor> {
        let mut out = vec![Anchor::BlockHash(self.execution.block_hash)];
        if let Some(root) = self.execution.state_root {
            out.push(Anchor::StateRoot(root));
        }
        out
    }
}

/// The store PDA's state. Small on purpose (259 bytes on Gloas, about 300 before): the committees are kept as
/// commitments, and the submitter supplies the keys as a witness checked against them.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LcSummary {
    pub instance_id: [u8; 32],
    pub evm_chain_id: u64,
    pub finalized: Finalized,
    /// [`committee_hash`] of the current sync committee.
    pub current_committee: [u8; 32],
    /// [`committee_hash`] of the next sync committee, once an update has proven it.
    pub next_committee: Option<[u8; 32]>,
    /// Updates applied since the instance was created.
    pub updates: u64,
}

impl LcSummary {
    /// Whether `next` is progress over `self`: a later finalized header, or the next committee
    /// learned. Helios's `apply_*` silently skips an update that is neither (or that lacks a 2/3
    /// supermajority), so this is asserted, never assumed.
    #[must_use]
    pub fn is_advanced_by(&self, next: &Self) -> bool {
        next.finalized.beacon.slot > self.finalized.beacon.slot
            || (self.next_committee.is_none() && next.next_committee.is_some())
    }
}

// ---------------------------------------------------------------------------------------------
// Anchors

/// A finalized execution commitment a consumer proves inclusion against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
pub enum Anchor {
    /// An execution block hash; the consumer supplies the header RLP to reach its roots.
    BlockHash([u8; 32]),
    /// An execution state root, directly (recorded only for pre-Gloas finalized headers).
    StateRoot([u8; 32]),
}

impl Anchor {
    #[must_use]
    pub const fn kind(&self) -> u8 {
        match self {
            Self::BlockHash(_) => 1,
            Self::StateRoot(_) => 2,
        }
    }

    #[must_use]
    pub const fn value(&self) -> &[u8; 32] {
        match self {
            Self::BlockHash(v) | Self::StateRoot(v) => v,
        }
    }
}

/// An anchor PDA's state.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AnchorRecord {
    pub instance_id: [u8; 32],
    pub evm_chain_id: u64,
    pub anchor: Anchor,
    /// The finalized beacon header that first committed to it.
    pub beacon_slot: u64,
    pub beacon_root: [u8; 32],
    pub block_number: Option<u64>,
}

// ---------------------------------------------------------------------------------------------
// Addresses

#[must_use]
pub fn config_seed(instance_id: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[CONFIG_SEED_DOMAIN, instance_id]))
}

#[must_use]
pub fn store_seed(instance_id: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[STORE_SEED_DOMAIN, instance_id]))
}

#[must_use]
pub fn anchor_seed(instance_id: &[u8; 32], anchor: &Anchor) -> PdaSeed {
    PdaSeed::new(sha256(&[ANCHOR_SEED_DOMAIN, instance_id, &[anchor.kind()], anchor.value()]))
}

#[must_use]
pub fn config_address(program: &AccountId, instance_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &config_seed(instance_id))
}

#[must_use]
pub fn store_address(program: &AccountId, instance_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &store_seed(instance_id))
}

#[must_use]
pub fn anchor_address(program: &AccountId, instance_id: &[u8; 32], anchor: &Anchor) -> AccountId {
    AccountId::for_public_pda(program, &anchor_seed(instance_id, anchor))
}

// ---------------------------------------------------------------------------------------------
// Messages

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// From a transaction, to the signer's own actor of this program.
    Instruction(Instruction),
    /// Between this program's own actors only.
    Effect(Effect),
    /// From anyone, including other programs: read-only checks that panic unless they hold.
    Query(Query),
}

/// Light-client update material, SSZ exactly as a beacon node serves it.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LcUpdate {
    /// `/eth/v1/beacon/light_client/finality_update`.
    Finality(Vec<u8>),
    /// One item of `/eth/v1/beacon/light_client/updates` (carries the next sync committee).
    Period(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Create an instance: verify `bootstrap` (`/eth/v1/beacon/light_client/bootstrap/{root}`
    /// SSZ) against `config.checkpoint`, then write the config and the first store. Anyone may
    /// create an instance; it is refused if this config's instance already exists.
    Init { config: InstanceConfig, bootstrap: Vec<u8> },
    /// Advance an instance by one update. Permissionless. Verification runs in the submitter's
    /// own actor (so it can be a private, locally proved execution); only the resulting
    /// [`Effect::Advance`] reaches the instance. `prev` must be the store's current state, and
    /// the committees are the witness for its commitments.
    Update {
        config: InstanceConfig,
        prev: LcSummary,
        current_committee: Vec<u8>,
        next_committee: Option<Vec<u8>>,
        update: LcUpdate,
    },
    /// Advance an instance by one finality update in a PUBLIC transaction (branch
    /// `public-update`, `DESIGN.md` §9.3a): the same Helios checks as [`Instruction::Update`],
    /// with the signature checked as the committee's aggregate key minus the non-signers' keys,
    /// given uncompressed in `keys` and bound to the committee witness byte for byte. Only a
    /// finality update signed in the store's own period while the next committee is unknown;
    /// anything else is refused (use [`Instruction::Update`]). Its result is the same
    /// [`Effect::Advance`], checked by the store exactly as for `Update`. Programs built without
    /// the `public-update` feature refuse it.
    UpdatePublic {
        config: InstanceConfig,
        prev: LcSummary,
        /// The current committee's SSZ (24,624 bytes), checked against `prev.current_committee`.
        current_committee: Vec<u8>,
        keys: KeyWitnesses,
        /// `/eth/v1/beacon/light_client/finality_update` SSZ.
        update: Vec<u8>,
    },
}

/// An uncompressed BLS12-381 G1 point: x ‖ y, 48 bytes each, big endian, flag bits clear.
pub type G1Uncompressed = [u8; 96];

/// The keys [`Instruction::UpdatePublic`] needs, so the program never decompresses a key:
/// the committee's aggregate key and each non-signer's key, uncompressed. Each must compress to
/// the committed bytes at its position in the committee witness.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct KeyWitnesses {
    /// `SyncCommittee.aggregate_pubkey`, uncompressed.
    pub aggregate: G1Uncompressed,
    /// Every non-signer, in increasing index order: (index 0..512, key uncompressed).
    pub absent: Vec<(u16, G1Uncompressed)>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// To the config PDA, from an initialiser's actor that verified the bootstrap.
    Create { config: InstanceConfig, genesis: LcSummary },
    /// To the store PDA, from the config PDA.
    Open { genesis: LcSummary },
    /// To the store PDA, from a submitter's actor that verified an update against `prev`.
    Advance { prev: LcSummary, next: LcSummary },
    /// To an anchor PDA, from the store PDA.
    RecordAnchor(AnchorRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Query {
    /// To an anchor PDA: panics unless `anchor` was finalized by `instance_id`, for `evm_chain_id`.
    AssertAnchor {
        instance_id: [u8; 32],
        evm_chain_id: u64,
        anchor: Anchor,
    },
    /// To a config PDA: panics unless the instance exists and is for `evm_chain_id`.
    AssertInstance { instance_id: [u8; 32], evm_chain_id: u64 },
}

/// The sync message a consumer sends to check that `anchor` is finalized by an instance.
#[must_use]
pub fn assert_anchor(attest_program: &AccountId, instance_id: &[u8; 32], evm_chain_id: u64, anchor: &Anchor) -> SyncMessage {
    SyncMessage::new(
        Actor::new(anchor_address(attest_program, instance_id, anchor), *attest_program),
        &Message::Query(Query::AssertAnchor {
            instance_id: *instance_id,
            evm_chain_id,
            anchor: *anchor,
        }),
    )
}

/// The sync message a consumer sends to check an instance exists for a chain (e.g. once, when
/// the consumer is configured).
#[must_use]
pub fn assert_instance(attest_program: &AccountId, instance_id: &[u8; 32], evm_chain_id: u64) -> SyncMessage {
    SyncMessage::new(
        Actor::new(config_address(attest_program, instance_id), *attest_program),
        &Message::Query(Query::AssertInstance {
            instance_id: *instance_id,
            evm_chain_id,
        }),
    )
}

/// Event emitted by the store each time it advances.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Advanced {
    pub instance_id: [u8; 32],
    pub beacon_slot: u64,
    pub block_hash: [u8; 32],
    pub next_committee_known: bool,
}
