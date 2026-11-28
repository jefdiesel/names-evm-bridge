//! One G2 update: previous state + consecutive LEZ blocks -> new state + the vault's journal.

use crate::{
    G2Error,
    finality::{BlockRef, FinalitySource},
    journal::{JournalVersion, UpdateJournal},
    state::G2State,
    view::{At, BatchOut, BridgeView, Extractor},
    wire::{ParsedTx, journal_frame, parse_block},
};
use borsh::{BorshDeserialize, BorshSerialize};
use claim_acc::{AccError, AccState, InsertWitness};
use lee_core::account::AccountId;

/// Checks that a private transaction's boundary is what the privacy circuit proved.
pub trait ReceiptVerifier {
    /// `journal`: the receipt journal (`journal_frame` of `public_context | execution`).
    /// `proof`: the transaction's proof bytes (`borsh(InnerReceipt)`). The guest ignores `proof`
    /// and calls `env::verify(PRIVACY_PRESERVING_CIRCUIT_ID, journal)`; the receipt itself reaches
    /// the prover as an assumption. The host verifies the receipt natively.
    fn verify(&mut self, journal: &[u8], proof: &[u8]) -> bool;
}

/// Accepts every receipt. Unit tests of extraction only.
pub struct AcceptAll;

/// The TRUSTED-TIP build's receipt policy (the guest's `unchecked-private` feature, on in
/// `outbound_guest_trusted_tip` only): a private transaction that reaches the bridge is extracted
/// from its public part (boundary messages, public root) as the sequencer applied it, with no
/// privacy-circuit receipt checked, the same footing as a public transaction's execution, which
/// G2-lite already takes on the sequencer's word (crate docs, "Trust"). Why: LEZ v0.4.0-rc3 makes
/// its privacy receipts with risc0 3.0.5, which RISC Zero's advisory GHSA-g6qg-9j8p-7j87 covers
/// and which a risc0 3.1 prover cannot use as an assumption, so a checking guest could never prove
/// past one private claim or burn. The light-client build keeps the check ([`ReceiptVerifier`]
/// with `env::verify`); it needs LEZ on a patched risc0 first.
#[derive(Default)]
pub struct SequencerTrusted;
impl ReceiptVerifier for SequencerTrusted {
    fn verify(&mut self, _: &[u8], _: &[u8]) -> bool {
        true
    }
}
impl ReceiptVerifier for AcceptAll {
    fn verify(&mut self, _: &[u8], _: &[u8]) -> bool {
        true
    }
}

/// The claim-nullifier accumulator as the update sees it.
pub trait Accumulator {
    fn state(&self) -> AccState;
    fn insert(&mut self, key: &[u8; 32]) -> Result<(), AccError>;
}

/// The guest's accumulator: the root and count only, each insert checked against a witness.
pub struct WitnessedAcc {
    pub state: AccState,
    witnesses: std::vec::IntoIter<InsertWitness>,
}

impl WitnessedAcc {
    #[must_use]
    pub fn new(state: AccState, witnesses: Vec<InsertWitness>) -> Self {
        Self { state, witnesses: witnesses.into_iter() }
    }

    /// Unused witnesses left over (must be none).
    #[must_use]
    pub fn leftover(&self) -> usize {
        self.witnesses.len()
    }
}

impl Accumulator for WitnessedAcc {
    fn state(&self) -> AccState {
        self.state
    }

    fn insert(&mut self, key: &[u8; 32]) -> Result<(), AccError> {
        let w = self.witnesses.next().ok_or(AccError::LowNotInTree)?;
        self.state = claim_acc::verify_insert(&self.state, key, &w)?;
        Ok(())
    }
}

/// The host's accumulator: the whole tree, recording the witnesses the guest will need.
pub struct HostAcc {
    pub tree: claim_acc::Tree,
    pub witnesses: Vec<InsertWitness>,
}

impl Accumulator for HostAcc {
    fn state(&self) -> AccState {
        self.tree.state()
    }

    fn insert(&mut self, key: &[u8; 32]) -> Result<(), AccError> {
        let before = self.tree.state();
        let w = self.tree.insert(key)?;
        debug_assert!(claim_acc::verify_insert(&before, key, &w).is_ok());
        self.witnesses.push(w);
        Ok(())
    }
}

/// Cycle counter (the guest's `env::cycle_count`; zero natively).
pub trait Meter {
    fn cycles(&self) -> u64 {
        0
    }
}

pub struct NoMeter;
impl Meter for NoMeter {}

/// Where the cycles went (guest only; written to stdout, never to the journal).
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Stats {
    pub blocks: u64,
    pub txs: u64,
    pub private_verified: u64,
    /// Parsing and hashing blocks.
    pub parse_hash: u64,
    /// Applying transactions to the view.
    pub extract: u64,
    /// `env::verify` calls (the in-guest part; the recursion that resolves them is extra).
    pub verify: u64,
    /// Accumulator inserts, and how many.
    pub acc: u64,
    pub inserts: u64,
    /// Per block: (id, bytes, cycles for parse + hash + extract).
    pub per_block: Vec<(u64, u32, u64)>,
}

/// Everything one update reads, besides the accumulator witnesses.
pub struct UpdateInput<'a> {
    pub prev: G2State,
    pub view: BridgeView,
    /// The finality source's witness (borsh).
    pub finality: Vec<u8>,
    /// Raw borsh blocks, consecutive from `prev.tip.id + 1`.
    pub blocks: Vec<&'a [u8]>,
}

pub struct UpdateOutput {
    pub journal: UpdateJournal,
    pub next: G2State,
    pub view: BridgeView,
    pub batch: BatchOut,
    pub stats: Stats,
}

/// The whole statement. See the crate docs for what it proves.
pub fn update<F: FinalitySource>(
    input: &UpdateInput<'_>,
    acc: &mut impl Accumulator,
    verifier: &mut impl ReceiptVerifier,
    meter: &impl Meter,
) -> Result<UpdateOutput, G2Error> {
    let prev = &input.prev;
    if prev.finality_kind != F::KIND {
        return Err(G2Error::PrevState("made by another finality source"));
    }
    if input.view.hash() != prev.view_hash {
        return Err(G2Error::PrevState("the view does not match its hash"));
    }
    if acc.state() != prev.claim_acc {
        return Err(G2Error::PrevState("the accumulator does not match"));
    }
    if input.blocks.is_empty() {
        return Err(G2Error::EmptyBatch);
    }
    let bridge = AccountId::new(prev.bridge_program);
    let mut view = input.view.clone();
    // M6: a wind-down read in an earlier batch still refuses claims in this one
    let mut batch = BatchOut { wound_down: prev.wound_down.contains(&bridge_core::GLOBAL), ..BatchOut::default() };
    let mut stats = Stats::default();
    let mut tip = prev.tip;
    let mut refs = Vec::with_capacity(input.blocks.len());

    for raw in &input.blocks {
        let c0 = meter.cycles();
        let id = tip.id + 1;
        let b = parse_block(raw).map_err(|e| G2Error::Decode { block_id: id, what: e.0 })?;
        if b.header.block_id != id {
            return Err(G2Error::Linkage { block_id: b.header.block_id, what: "not the next block id" });
        }
        if b.header.prev_block_hash != tip.hash {
            return Err(G2Error::Linkage { block_id: id, what: "prev_block_hash is not the previous block's hash" });
        }
        if b.header.hash != b.hash {
            return Err(G2Error::Linkage { block_id: id, what: "stored hash is not the block's hash" });
        }
        tip = BlockRef { id, hash: b.hash };
        refs.push(tip);
        let c1 = meter.cycles();
        stats.parse_hash += c1 - c0;
        stats.blocks += 1;
        stats.txs += b.txs.len() as u64;
        let first_new = batch.nullifiers.len();
        for (i, tx) in b.txs.iter().enumerate() {
            if b.is_reverted(i) {
                continue; // charged, effects undone: counts for nothing
            }
            let at = At { block_id: id, tx: i };
            let mut x = Extractor::new(bridge, &mut view, &mut batch);
            match tx {
                ParsedTx::Public { root } => x.root(at, root)?,
                ParsedTx::Private { execution, journal_payload, proof } => {
                    if x.private_touches(execution) {
                        let v0 = meter.cycles();
                        if !verifier.verify(&journal_frame(journal_payload), proof) {
                            return Err(G2Error::Receipt { block_id: id, tx: i });
                        }
                        stats.verify += meter.cycles() - v0;
                        stats.private_verified += 1;
                        x.private(at, execution)?;
                    }
                }
            }
        }
        let c2 = meter.cycles();
        for key in &batch.nullifiers[first_new..] {
            acc.insert(key).map_err(|e| G2Error::Accumulator {
                key: *key,
                what: match e {
                    AccError::NotCovered => "claim nullifier already spent (sequencer accepted a repeat claim)",
                    AccError::ZeroKey => "zero key",
                    AccError::Full => "tree full",
                    AccError::LowNotInTree | AccError::SlotNotEmpty => "bad insertion witness",
                },
            })?;
            stats.inserts += 1;
        }
        let c3 = meter.cycles();
        stats.acc += c3 - c2;
        stats.extract += c2 - c1;
        stats.per_block.push((id, raw.len() as u32, c2 - c0));
    }

    let l1 = F::finalize(&prev.l1_state, &refs, &input.finality).map_err(G2Error::Finality)?;
    if !view.initialized {
        return Err(G2Error::Input("the bridge is not initialised by the end of the batch"));
    }
    let mut wound_down = prev.wound_down.clone();
    if batch.wound_down && !wound_down.contains(&bridge_core::GLOBAL) {
        wound_down.push(bridge_core::GLOBAL);
    }
    let next = G2State {
        finality_kind: F::KIND,
        l1_state: l1,
        bridge_program: prev.bridge_program,
        tip,
        claim_acc: acc.state(),
        view_hash: view.hash(),
        exit_count: prev.exit_count + batch.exits.len() as u64,
        wound_down: wound_down.clone(),
    };
    let journal = UpdateJournal {
        cfg_id: view.cfg_id,
        prev_state: prev.commitment(),
        new_state: next.commitment(),
        deposit_roots_used: batch.roots.clone(),
        claims: batch.claims.clone(),
        exits: batch.exits.clone(),
        claim_acc_root: next.claim_acc.root,
        claim_acc_count: next.claim_acc.count,
        wound_down,
        mint_stopped_block: batch.mint_stopped_block,
    };
    Ok(UpdateOutput { journal, next, view, batch, stats })
}

/// The journal version the guest commits: V2 (failsafe) with feature `journal-v2`, else V1 (`BridgeVault`).
#[cfg(feature = "journal-v2")]
pub const JOURNAL_VERSION: JournalVersion = JournalVersion::V2;
#[cfg(not(feature = "journal-v2"))]
pub const JOURNAL_VERSION: JournalVersion = JournalVersion::V1;

// ------------------------------------------------------------------------------- guest input

/// The guest input besides the blocks.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct GuestHeader {
    pub prev: G2State,
    pub view: BridgeView,
    pub finality: Vec<u8>,
    pub inserts: Vec<InsertWitness>,
}

/// `u32le(len(borsh header)) | borsh header | u32le(n) | n x (u32le(len) | block)`, zero-padded
/// to a multiple of 4. The guest reads it with one `read_slice` and borrows the blocks from it.
#[must_use]
pub fn encode_input(header: &GuestHeader, blocks: &[Vec<u8>]) -> Vec<u8> {
    let h = borsh::to_vec(header).unwrap_or_default();
    let mut out = Vec::with_capacity(h.len() + 8 + blocks.iter().map(|b| b.len() + 4).sum::<usize>());
    out.extend_from_slice(&(h.len() as u32).to_le_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for b in blocks {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

/// Inverse of [`encode_input`].
pub fn decode_input(buf: &[u8]) -> Result<(GuestHeader, Vec<&[u8]>), G2Error> {
    let mut cur = buf;
    let mut take = |n: usize| -> Result<&[u8], G2Error> {
        if cur.len() < n {
            return Err(G2Error::Input("truncated"));
        }
        let (a, b) = cur.split_at(n);
        cur = b;
        Ok(a)
    };
    let u32le = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let hl = u32le(take(4)?);
    let header: GuestHeader = borsh::from_slice(take(hl)?).map_err(|_| G2Error::Input("header"))?;
    let n = u32le(take(4)?);
    let mut blocks = Vec::with_capacity(n);
    for _ in 0..n {
        let l = u32le(take(4)?);
        blocks.push(take(l)?);
    }
    if cur.len() >= 4 || cur.iter().any(|&b| b != 0) {
        return Err(G2Error::Input("trailing bytes"));
    }
    Ok((header, blocks))
}
