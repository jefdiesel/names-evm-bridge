//! The recovery scan: from the wallet's account keys plus public chain data alone, rediscover
//! every deposit made through the derived path and every exit burned through it, and classify
//! them (crate docs, "Recovery scan").
//!
//! Pure: it takes what [`crate::chain`] collected ([`VaultView`] + the bridge's LEZ
//! transactions) and returns [`Recovery`]. No network, no state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bridge_core::{
    EthAddress, claim_nullifier, deposit_commitment, deposit_leaf, exit_commitment, exit_leaf, nullifier_address, release_nullifier,
};
use lee_core::account::AccountId;
use serde::{Deserialize, Serialize};

use crate::{
    chain::{DepositEvent, LezTx, VaultView, find_in_txs},
    hexser::{self, hex},
    kdf::{self, AccountRoot},
};

/// One of the wallet's accounts, with its derivation root.
#[derive(Debug)]
pub struct WalletAccount {
    pub id: AccountId,
    pub root: AccountRoot,
    pub private: bool,
}

/// A registered token and its denominations, in the vault's order (from the bridge config).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenInfo {
    pub token: EthAddress,
    pub denoms: Vec<u128>,
}

/// Where an exit pays out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dest {
    /// A fresh seed-derived Ethereum address per exit ([`kdf::release_key`]): recoverable from
    /// the seed alone. The default.
    Derived,
    /// An address the user chose: recovery needs the user to name it again.
    Address(EthAddress),
}

impl Dest {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Derived => "derived".into(),
            Self::Address(a) => format!("0x{}", hex(a)),
        }
    }
}

/// The index-sequence key of a deposit tuple (the cache's map key).
#[must_use]
pub fn dep_key(account: &AccountId, token: &EthAddress, denom: u128) -> String {
    format!("dep/{}/0x{}/{denom}", hex(account.value()), hex(token))
}

/// The index-sequence key of an exit tuple.
#[must_use]
pub fn exit_key(account: &AccountId, token: &EthAddress, denom: u128, dest: &Dest, max_relay_fee: u128) -> String {
    format!("exit/{}/0x{}/{denom}/{}/{max_relay_fee}", hex(account.value()), hex(token), dest.label())
}

/// One deposit's derived values.
pub struct DepositCandidate {
    /// The master secret `s` (never published).
    pub master: [u8; 32],
    /// The claim secret `k = SHA256("RFP022/ck" | s)`.
    pub secret: [u8; 32],
    pub commitment: [u8; 32],
}

#[must_use]
pub fn deposit_candidate(root: &AccountRoot, cfg_id: &[u8; 32], account: &AccountId, token: &EthAddress, denom: u128, index: u64) -> DepositCandidate {
    let master = kdf::deposit_master(root, cfg_id, account, token, denom, index);
    let secret = kdf::claim_secret_of(&master);
    let commitment = deposit_commitment(&secret, account, token, denom, cfg_id);
    DepositCandidate { master, secret, commitment }
}

/// One exit's derived values.
pub struct ExitCandidate {
    pub master: [u8; 32],
    /// `k' = SHA256("RFP022/ek" | s')`.
    pub secret: [u8; 32],
    pub eth_dest: EthAddress,
    pub exit_commitment: [u8; 32],
    pub release_nullifier: [u8; 32],
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn exit_candidate(
    root: &AccountRoot,
    cfg_id: &[u8; 32],
    account: &AccountId,
    token: &EthAddress,
    denom: u128,
    dest: &Dest,
    max_relay_fee: u128,
    index: u64,
) -> ExitCandidate {
    let eth_dest = match dest {
        Dest::Derived => kdf::release_key(root, cfg_id, account, token, denom, max_relay_fee, index).address,
        Dest::Address(a) => *a,
    };
    let master = kdf::exit_master(root, cfg_id, account, token, denom, &eth_dest, max_relay_fee, index);
    let secret = kdf::exit_secret_of(&master);
    ExitCandidate {
        master,
        secret,
        eth_dest,
        exit_commitment: exit_commitment(&secret, &eth_dest, token, denom, max_relay_fee, cfg_id),
        release_nullifier: release_nullifier(&secret),
    }
}

/// What the scan needs besides the chain data.
#[derive(Clone, Debug)]
pub struct ScanParams {
    pub cfg_id: [u8; 32],
    /// The bridge program (its nullifier PDAs).
    pub bridge: AccountId,
    /// Stop a sequence after this many consecutive unused indexes (HD-wallet style).
    pub gap: u64,
    /// Release addresses the user chose, tried besides the derived ones.
    pub eth_dests: Vec<EthAddress>,
    /// More `maxRelayFee` values to try (besides 0, the 2% cap and every menu value ever set).
    pub extra_fees: Vec<u128>,
    /// Ethereum's finalized block, to mark deposits finalized.
    pub finalized_block: Option<u64>,
    /// Next indexes a local cache knows of (for the beyond-gap warning). May be empty.
    pub known_next: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClaimState {
    /// Deposit seen on Ethereum, claim nullifier unspent on LEZ.
    Claimable,
    /// The claim nullifier was spent in this LEZ block. (Not `block`: this enum is flattened
    /// into [`DepositPosition`], whose `block` is the Ethereum deposit block.)
    Claimed { claim_block: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositPosition {
    #[serde(with = "hexser::arr")]
    pub account: [u8; 32],
    pub key_index: u64,
    #[serde(with = "hexser::arr")]
    pub token: EthAddress,
    pub denom_index: u32,
    #[serde(with = "hexser::dec")]
    pub denom: u128,
    #[serde(with = "hexser::arr")]
    pub commitment: [u8; 32],
    #[serde(with = "hexser::arr")]
    pub claim_nullifier: [u8; 32],
    pub leaf_index: u64,
    /// The vault root right after this deposit (any later root also holds it).
    #[serde(with = "hexser::arr")]
    pub root_after: [u8; 32],
    pub block: u64,
    #[serde(with = "hexser::arr")]
    pub tx_hash: [u8; 32],
    pub finalized: Option<bool>,
    #[serde(flatten)]
    pub claim: ClaimState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ExitState {
    /// Burned on LEZ, not yet in the vault's exit tree (waits for the outbound update, G2).
    Burned,
    /// In the vault's exit tree at `exit_index`, release nullifier unspent: releasable once the
    /// root's delay has passed.
    Unreleased { exit_index: u64 },
    /// The release nullifier was spent on Ethereum.
    Released {
        release_block: u64,
        #[serde(with = "hexser::arr")]
        tx_hash: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitPosition {
    #[serde(with = "hexser::arr")]
    pub account: [u8; 32],
    pub key_index: u64,
    #[serde(with = "hexser::arr")]
    pub token: EthAddress,
    pub denom_index: u32,
    #[serde(with = "hexser::dec")]
    pub denom: u128,
    #[serde(with = "hexser::arr")]
    pub eth_dest: EthAddress,
    /// The destination is a seed-derived release address.
    pub dest_derived: bool,
    #[serde(with = "hexser::dec")]
    pub max_relay_fee: u128,
    #[serde(with = "hexser::arr")]
    pub exit_commitment: [u8; 32],
    #[serde(with = "hexser::arr")]
    pub release_nullifier: [u8; 32],
    /// The first LEZ block that burned it.
    pub burn_block: u64,
    /// More than 1: the same commitment was burned again, and only one of them can ever be
    /// released (the release nullifier is shared).
    pub burns: usize,
    #[serde(flatten)]
    pub exit: ExitState,
}

/// The result of a scan. Indexes and public values only, never a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    pub deposits: Vec<DepositPosition>,
    pub exits: Vec<ExitPosition>,
    /// The next unused index of every sequence that has a used one.
    pub next: BTreeMap<String, u64>,
    /// Per sequence scanned, the first index not checked.
    pub scanned_to: BTreeMap<String, u64>,
    /// Derived candidates computed (cost measure).
    pub candidates: u64,
    /// The deposit tree rebuilt from the logs matched every logged root.
    pub tree_ok: bool,
    pub warnings: Vec<String>,
}

impl Recovery {
    #[must_use]
    pub fn claimable(&self) -> impl Iterator<Item = &DepositPosition> {
        self.deposits.iter().filter(|d| d.claim == ClaimState::Claimable)
    }

    #[must_use]
    pub fn unreleased(&self) -> impl Iterator<Item = &ExitPosition> {
        self.exits.iter().filter(|e| !matches!(e.exit, ExitState::Released { .. }))
    }
}

/// The SHA-256 deposit tree, incrementally, as the vault builds it.
#[derive(Clone, Debug)]
pub struct IncrementalTree {
    filled: Vec<[u8; 32]>,
    zeros: [[u8; 32]; bridge_core::TREE_DEPTH + 1],
    pub count: u64,
}

impl Default for IncrementalTree {
    fn default() -> Self {
        Self { filled: vec![[0; 32]; bridge_core::TREE_DEPTH], zeros: bridge_core::zero_hashes(), count: 0 }
    }
}

impl IncrementalTree {
    /// Appends `leaf`, returns the new root.
    pub fn insert(&mut self, leaf: [u8; 32]) -> [u8; 32] {
        let mut node = leaf;
        let mut idx = self.count;
        for h in 0..bridge_core::TREE_DEPTH {
            node = if idx & 1 == 0 {
                self.filled[h] = node;
                bridge_core::sha256(&[&node, &self.zeros[h]])
            } else {
                bridge_core::sha256(&[&self.filled[h], &node])
            };
            idx >>= 1;
        }
        self.count += 1;
        node
    }
}

fn denom_of(registry: &[TokenInfo], token: &EthAddress, denom_index: u32) -> Option<u128> {
    registry.iter().find(|t| &t.token == token)?.denoms.get(denom_index as usize).copied()
}

/// The vault's deposit leaves in order, rebuilt from the `Deposit` logs, checked against every
/// logged root. `Err` names the first problem (a gap in leaf indexes, a token or denomination the
/// registry does not know, a root that does not match).
///
/// # Errors
/// As above.
pub fn rebuild_leaves(deposits: &[DepositEvent], registry: &[TokenInfo], cfg_id: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
    let mut tree = IncrementalTree::default();
    let mut leaves = Vec::with_capacity(deposits.len());
    for (i, d) in deposits.iter().enumerate() {
        if d.leaf_index != i as u64 {
            return Err(format!("deposit logs skip from leaf {i} to {}: a block range was not fetched", d.leaf_index));
        }
        let denom = denom_of(registry, &d.token, d.denom_index)
            .ok_or_else(|| format!("leaf {i}: token 0x{} denomination {} is not in the bridge registry", hex(&d.token), d.denom_index))?;
        let leaf = deposit_leaf(cfg_id, &d.token, denom, &d.commitment);
        if tree.insert(leaf) != d.root {
            return Err(format!("leaf {i}: the rebuilt root differs from the logged root (wrong cfg id or registry?)"));
        }
        leaves.push(leaf);
    }
    Ok(leaves)
}

/// The fees an exit sequence may have used: 0, the 2% cap, every menu value ever set for it, and
/// `extra`.
#[must_use]
pub fn fee_candidates(view: &VaultView, token: &EthAddress, denom_index: u32, denom: u128, extra: &[u128]) -> BTreeSet<u128> {
    let mut fees: BTreeSet<u128> = [0, kdf::max_relay_fee_cap(denom)].into();
    if let Some(menu) = view.menu_fees.get(&(*token, denom_index)) {
        fees.extend(menu);
    }
    fees.extend(extra);
    fees
}

/// The scan. `spent_override`: claim-nullifier PDAs known spent from point queries (with the
/// block, 0 if unknown), instead of searching `lez_txs`.
#[must_use]
pub fn recover(
    accounts: &[WalletAccount],
    registry: &[TokenInfo],
    view: &VaultView,
    lez_txs: &[LezTx],
    params: &ScanParams,
    spent_override: Option<&HashMap<[u8; 32], u64>>,
) -> Recovery {
    let mut out = Recovery { tree_ok: true, ..Recovery::default() };
    if let Err(e) = rebuild_leaves(&view.deposits, registry, &params.cfg_id) {
        out.tree_ok = false;
        out.warnings.push(format!("deposit tree check: {e}"));
    }
    let gap = params.gap.max(1);

    // ---- deposits: direct lookup by commitment
    let by_c: HashMap<[u8; 32], &DepositEvent> = view.deposits.iter().map(|d| (d.commitment, d)).collect();
    let mut found_deps: Vec<(DepositPosition, [u8; 32])> = Vec::new();
    for acct in accounts {
        for t in registry {
            for (di, &denom) in t.denoms.iter().enumerate() {
                let di = di as u32;
                let key = dep_key(&acct.id, &t.token, denom);
                let (mut j, mut misses, mut last) = (0u64, 0u64, None::<u64>);
                while misses < gap {
                    let c = deposit_candidate(&acct.root, &params.cfg_id, &acct.id, &t.token, denom, j);
                    out.candidates += 1;
                    match by_c.get(&c.commitment) {
                        Some(d) if d.token == t.token && d.denom_index == di => {
                            let nf = claim_nullifier(&c.secret, &acct.id, &t.token, denom);
                            found_deps.push((
                                DepositPosition {
                                    account: *acct.id.value(),
                                    key_index: j,
                                    token: t.token,
                                    denom_index: di,
                                    denom,
                                    commitment: c.commitment,
                                    claim_nullifier: nf,
                                    leaf_index: d.leaf_index,
                                    root_after: d.root,
                                    block: d.block,
                                    tx_hash: d.tx_hash,
                                    finalized: params.finalized_block.map(|f| d.block <= f),
                                    claim: ClaimState::Claimable,
                                },
                                *nullifier_address(&params.bridge, &nf).value(),
                            ));
                            last = Some(j);
                            misses = 0;
                        }
                        Some(d) => {
                            out.warnings.push(format!(
                                "{key} index {j}: its commitment was deposited as token 0x{} denomination {} by someone else \
                                 (a copied commitment): not claimable; the index counts as used",
                                hex(&d.token),
                                d.denom_index
                            ));
                            last = Some(j);
                            misses = 0;
                        }
                        None => misses += 1,
                    }
                    j += 1;
                }
                out.scanned_to.insert(key.clone(), j);
                if let Some(l) = last {
                    out.next.insert(key, l + 1);
                }
            }
        }
    }
    let spent: HashMap<[u8; 32], u64> = match spent_override {
        Some(s) => s.clone(),
        None => {
            let wanted: HashSet<[u8; 32]> = found_deps.iter().map(|(_, pda)| *pda).collect();
            find_in_txs(lez_txs, &wanted).into_iter().map(|(pda, blocks)| (pda, blocks[0])).collect()
        }
    };
    for (mut p, pda) in found_deps {
        if let Some(block) = spent.get(&pda) {
            p.claim = ClaimState::Claimed { claim_block: *block };
        }
        out.deposits.push(p);
    }
    out.deposits.sort_by_key(|d| d.leaf_index);

    // ---- exits: the commitments are only in LEZ transaction bytes, so search in rounds
    struct Seq<'a> {
        acct: &'a WalletAccount,
        token: EthAddress,
        di: u32,
        denom: u128,
        dest: Dest,
        fee: u128,
        key: String,
        lo: u64,
        hi: u64,
        used: BTreeMap<u64, (ExitCandidate, Vec<u64>)>,
    }
    let mut dests = vec![Dest::Derived];
    dests.extend(params.eth_dests.iter().map(|a| Dest::Address(*a)));
    let mut seqs: Vec<Seq<'_>> = Vec::new();
    for acct in accounts {
        for t in registry {
            for (di, &denom) in t.denoms.iter().enumerate() {
                let di = di as u32;
                for dest in &dests {
                    for fee in fee_candidates(view, &t.token, di, denom, &params.extra_fees) {
                        seqs.push(Seq {
                            acct,
                            token: t.token,
                            di,
                            denom,
                            dest: *dest,
                            fee,
                            key: exit_key(&acct.id, &t.token, denom, dest, fee),
                            lo: 0,
                            hi: gap,
                            used: BTreeMap::new(),
                        });
                    }
                }
            }
        }
    }
    loop {
        let mut cand: HashMap<[u8; 32], (usize, u64, ExitCandidate)> = HashMap::new();
        for (s, seq) in seqs.iter().enumerate() {
            for j in seq.lo..seq.hi {
                let c = exit_candidate(&seq.acct.root, &params.cfg_id, &seq.acct.id, &seq.token, seq.denom, &seq.dest, seq.fee, j);
                out.candidates += 1;
                cand.insert(c.exit_commitment, (s, j, c));
            }
        }
        if cand.is_empty() {
            break;
        }
        let wanted: HashSet<[u8; 32]> = cand.keys().copied().collect();
        let mut found = find_in_txs(lez_txs, &wanted);
        for (e, (s, j, c)) in cand {
            if let Some(blocks) = found.remove(&e) {
                seqs[s].used.insert(j, (c, blocks));
            }
        }
        for seq in &mut seqs {
            seq.lo = seq.hi;
            if let Some((&l, _)) = seq.used.last_key_value() {
                seq.hi = seq.hi.max(l + 1 + gap);
            }
        }
    }
    let leaves: HashMap<[u8; 32], u64> = view.exit_leaves.iter().map(|l| (l.leaf, l.exit_index)).collect();
    for seq in seqs {
        out.scanned_to.insert(seq.key.clone(), seq.hi);
        if let Some((&l, _)) = seq.used.last_key_value() {
            out.next.insert(seq.key.clone(), l + 1);
        }
        for (j, (c, blocks)) in seq.used {
            let leaf = exit_leaf(&params.cfg_id, &seq.token, seq.denom, &c.exit_commitment);
            let exit = match (view.released.get(&c.release_nullifier), leaves.get(&leaf)) {
                (Some(r), _) => ExitState::Released { release_block: r.block, tx_hash: r.tx_hash },
                (None, Some(&exit_index)) => ExitState::Unreleased { exit_index },
                (None, None) => ExitState::Burned,
            };
            if blocks.len() > 1 {
                out.warnings.push(format!(
                    "{} index {j}: burned {} times with the same exit commitment; only one can ever be released",
                    seq.key,
                    blocks.len()
                ));
            }
            out.exits.push(ExitPosition {
                account: *seq.acct.id.value(),
                key_index: j,
                token: seq.token,
                denom_index: seq.di,
                denom: seq.denom,
                eth_dest: c.eth_dest,
                dest_derived: seq.dest == Dest::Derived,
                max_relay_fee: seq.fee,
                exit_commitment: c.exit_commitment,
                release_nullifier: c.release_nullifier,
                burn_block: blocks[0],
                burns: blocks.len(),
                exit,
            });
        }
    }
    out.exits.sort_by_key(|e| (e.burn_block, e.account, e.key_index));

    for (key, &n) in &params.known_next {
        if let Some(&to) = out.scanned_to.get(key)
            && n > to
        {
            out.warnings.push(format!(
                "{key}: the local cache handed out index {} but the scan stopped at {to} (gap {gap}): rescan with --gap {}",
                n - 1,
                n + gap
            ));
        }
    }
    out
}

/// The index to hand out next for `key`: past every index the chain shows used and every index
/// the local cache handed out.
#[must_use]
pub fn allocate(key: &str, recovered: &Recovery, cache_next: &BTreeMap<String, u64>) -> u64 {
    recovered.next.get(key).copied().unwrap_or(0).max(cache_next.get(key).copied().unwrap_or(0))
}
