//! Reliability #5 as a property: for random sequences of deposits, claims, burns, outbound
//! updates and releases made through the derived path in the bridge's whole-transaction
//! simulator (`bridge/tests/common`, included unchanged), mixed with other users' activity and
//! failed transactions, a FRESH client holding only the account keys rediscovers exactly the
//! claimable deposits and the unreleased burns: nothing missing, nothing extra, each in the right
//! state. Its next allocation never shares a secret or commitment with an existing position.

#[path = "../../bridge/tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bridge_client::{
    chain::{DepositEvent, ExitLeafEvent, LezTx, ReleasedEvent, VaultView},
    kdf::{self, AccountRoot},
    scan::{
        ClaimState, Dest, ExitState, IncrementalTree, ScanParams, TokenInfo, WalletAccount, allocate, dep_key, deposit_candidate,
        exit_candidate, exit_key, recover,
    },
};
use common::*;
use proptest::prelude::*;

const ME: [AccountId; 3] = [ALICE, BOB, CAROL];
const DENOMS: [u128; 2] = [100_000_000, 1_000_000_000];
const USER_DEST: EthAddress = [0x77; 20];
const GAP: u64 = 5;

fn root_of(i: usize) -> AccountRoot {
    AccountRoot::from_account_secret(&[0x51 + i as u8; 32])
}

#[derive(Debug, Clone)]
enum Op {
    Deposit(usize, usize),
    /// A deposit request handed out and never paid.
    Abandon(usize, usize),
    Claim(u16),
    /// Burn one denomination, to a derived address (false) or the user's own (true).
    Burn(usize, usize, bool),
    /// A burn the bridge refuses (not a denomination): never on chain, consumes nothing.
    FailedBurn(usize),
    /// Someone else deposits, claims and burns.
    Foreign(u64),
    /// The outbound update puts every burned exit into the vault's exit tree.
    Update,
    Release(u16),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0usize..3, 0usize..2).prop_map(|(w, d)| Op::Deposit(w, d)),
        1 => (0usize..3, 0usize..2).prop_map(|(w, d)| Op::Abandon(w, d)),
        3 => any::<u16>().prop_map(Op::Claim),
        3 => (0usize..3, 0usize..2, any::<bool>()).prop_map(|(w, d, u)| Op::Burn(w, d, u)),
        1 => (0usize..3).prop_map(Op::FailedBurn),
        2 => any::<u64>().prop_map(Op::Foreign),
        1 => Just(Op::Update),
        2 => any::<u16>().prop_map(Op::Release),
    ]
}

struct TrueDeposit {
    who: usize,
    di: usize,
    key_index: u64,
    leaf_index: u64,
    secret: [u8; 32],
    claimed: bool,
}

struct TrueExit {
    who: usize,
    key_index: u64,
    e: [u8; 32],
    nf: [u8; 32],
    dest: EthAddress,
    leaf: [u8; 32],
    exit_index: Option<u64>,
    released: bool,
}

struct Sim {
    w: World,
    tree: IncrementalTree,
    leaves: Vec<[u8; 32]>,
    root: [u8; 32],
    view: VaultView,
    txs: Vec<LezTx>,
    block: u64,
    deposits: Vec<TrueDeposit>,
    exits: Vec<TrueExit>,
    foreign_e: Vec<[u8; 32]>,
    exit_count: u64,
    next_dep: BTreeMap<(usize, usize), u64>,
    unused_run: BTreeMap<(usize, usize), u64>,
    next_exit: BTreeMap<(usize, usize, bool), u64>,
    seed: u64,
}

impl Sim {
    fn new() -> Self {
        Self {
            w: setup(),
            tree: IncrementalTree::default(),
            leaves: vec![],
            root: [0; 32],
            view: VaultView::default(),
            txs: vec![],
            block: 1,
            deposits: vec![],
            exits: vec![],
            foreign_e: vec![],
            exit_count: 0,
            next_dep: BTreeMap::new(),
            unused_run: BTreeMap::new(),
            next_exit: BTreeMap::new(),
            seed: 1,
        }
    }

    fn rand32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for b in &mut out {
            self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = (self.seed >> 33) as u8;
        }
        out
    }

    /// A LEZ transaction: on success, its public footprint (every message's address, program and
    /// bytes) becomes a block's transaction, as the real chain publishes them.
    fn lez<M: borsh::BorshSerialize>(&mut self, signer: AccountId, program: AccountId, m: &M) -> Result<(), String> {
        self.block += 1;
        let r = self.w.tx(signer, program, m);
        if r.is_ok() {
            let bytes: Vec<u8> = self
                .w
                .trace
                .iter()
                .flat_map(|(_, to, msg, _)| [to.account_id.value().as_slice(), to.program_account_id.value(), msg].concat())
                .collect();
            self.txs.push(LezTx { block: self.block, bytes });
        }
        r
    }

    fn vault_deposit(&mut self, di: usize, c: [u8; 32]) -> u64 {
        self.block += 1;
        let leaf = deposit_leaf(&cfg(), &USDC, DENOMS[di], &c);
        let leaf_index = self.tree.count;
        self.root = self.tree.insert(leaf);
        self.leaves.push(leaf);
        self.view.deposits.push(DepositEvent {
            token: USDC,
            denom_index: di as u32,
            commitment: c,
            leaf_index,
            root: self.root,
            block: self.block,
            tx_hash: [0; 32],
        });
        leaf_index
    }

    fn claim_leaf(&mut self, who: AccountId, secret: [u8; 32], di: usize, leaf_index: u64) -> Result<(), String> {
        let root = self.root;
        self.lez(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
        let m = instr(Instruction::Claim {
            eth_token: USDC,
            denom_index: di as u32,
            denom: DENOMS[di],
            root,
            leaf_index,
            path: merkle_path(&self.leaves, leaf_index as usize),
            secret,
            cfg_id: cfg(),
            recipient_opening: None,
        });
        self.lez(who, BRIDGE, &m)
    }

    fn burn_e(&mut self, who: AccountId, amount: u128, e: [u8; 32]) -> Result<(), String> {
        let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
        self.lez(who, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount, notify, memo: e })
    }

    fn apply(&mut self, op: &Op) {
        match *op {
            Op::Deposit(who, di) => {
                let j = self.next_dep.get(&(who, di)).copied().unwrap_or(0);
                let c = deposit_candidate(&root_of(who), &cfg(), &ME[who], &USDC, DENOMS[di], j);
                let leaf_index = self.vault_deposit(di, c.commitment);
                self.deposits.push(TrueDeposit { who, di, key_index: j, leaf_index, secret: c.secret, claimed: false });
                self.next_dep.insert((who, di), j + 1);
                self.unused_run.insert((who, di), 0);
            }
            Op::Abandon(who, di) => {
                // Fewer than GAP unused indexes in a row: the documented limit of any gap scan.
                let run = self.unused_run.get(&(who, di)).copied().unwrap_or(0);
                if run + 1 < GAP {
                    *self.next_dep.entry((who, di)).or_default() += 1;
                    self.unused_run.insert((who, di), run + 1);
                }
            }
            Op::Claim(pick) => {
                let open: Vec<usize> = (0..self.deposits.len()).filter(|i| !self.deposits[*i].claimed).collect();
                if open.is_empty() {
                    return;
                }
                let i = open[pick as usize % open.len()];
                let (who, secret, di, leaf) = (self.deposits[i].who, self.deposits[i].secret, self.deposits[i].di, self.deposits[i].leaf_index);
                self.claim_leaf(ME[who], secret, di, leaf).unwrap();
                self.deposits[i].claimed = true;
            }
            Op::Burn(who, di, user) => {
                if self.w.wrapped(ME[who]) < DENOMS[di] {
                    return;
                }
                let j = self.next_exit.get(&(who, di, user)).copied().unwrap_or(0);
                let (dest, fee) = if user { (Dest::Address(USER_DEST), 0) } else { (Dest::Derived, kdf::max_relay_fee_cap(DENOMS[di])) };
                let c = exit_candidate(&root_of(who), &cfg(), &ME[who], &USDC, DENOMS[di], &dest, fee, j);
                self.burn_e(ME[who], DENOMS[di], c.exit_commitment).unwrap();
                self.exits.push(TrueExit {
                    who,
                    key_index: j,
                    e: c.exit_commitment,
                    nf: c.release_nullifier,
                    dest: c.eth_dest,
                    leaf: exit_leaf(&cfg(), &USDC, DENOMS[di], &c.exit_commitment),
                    exit_index: None,
                    released: false,
                });
                self.next_exit.insert((who, di, user), j + 1);
            }
            Op::FailedBurn(who) => {
                let j = self.next_exit.get(&(who, 0, false)).copied().unwrap_or(0);
                let c = exit_candidate(&root_of(who), &cfg(), &ME[who], &USDC, DENOMS[0], &Dest::Derived, kdf::max_relay_fee_cap(DENOMS[0]), j);
                assert!(self.burn_e(ME[who], 50_000_000, c.exit_commitment).is_err());
            }
            Op::Foreign(n) => {
                let (k, di) = (self.rand32(), (n % 2) as usize);
                let c = deposit_commitment(&k, &DAVE, &USDC, DENOMS[di], &cfg());
                let leaf = self.vault_deposit(di, c);
                self.claim_leaf(DAVE, k, di, leaf).unwrap();
                if n % 3 == 0 {
                    let e = self.rand32();
                    self.burn_e(DAVE, DENOMS[di], e).unwrap();
                    self.foreign_e.push(e);
                }
            }
            Op::Update => {
                self.block += 1;
                let mut pending: Vec<[u8; 32]> = self.exits.iter().filter(|e| e.exit_index.is_none()).map(|e| e.leaf).collect();
                pending.extend(self.foreign_e.drain(..).map(|e| exit_leaf(&cfg(), &USDC, DENOMS[0], &e)));
                for leaf in pending {
                    if let Some(x) = self.exits.iter_mut().find(|x| x.leaf == leaf) {
                        x.exit_index = Some(self.exit_count);
                    }
                    self.view.exit_leaves.push(ExitLeafEvent { leaf, exit_index: self.exit_count, exit_root: [0; 32], block: self.block });
                    self.exit_count += 1;
                }
            }
            Op::Release(pick) => {
                let ready: Vec<usize> = (0..self.exits.len()).filter(|i| self.exits[*i].exit_index.is_some() && !self.exits[*i].released).collect();
                if ready.is_empty() {
                    return;
                }
                let i = ready[pick as usize % ready.len()];
                self.block += 1;
                let x = &mut self.exits[i];
                x.released = true;
                self.view.released.insert(
                    x.nf,
                    ReleasedEvent { nullifier: x.nf, token: USDC, denom_index: 0, recipient: x.dest, block: self.block, tx_hash: [1; 32] },
                );
            }
        }
    }
}

fn wallet() -> Vec<WalletAccount> {
    // The three accounts plus one that never did anything.
    (0..4).map(|i| WalletAccount { id: if i < 3 { ME[i] } else { FUNDER }, root: root_of(i), private: false }).collect()
}

fn check(sim: &Sim) {
    let registry = vec![TokenInfo { token: USDC, denoms: DENOMS.to_vec() }];
    let params = ScanParams {
        cfg_id: cfg(),
        bridge: BRIDGE,
        gap: GAP,
        eth_dests: vec![USER_DEST],
        extra_fees: vec![],
        finalized_block: None,
        known_next: BTreeMap::new(),
    };
    let r = recover(&wallet(), &registry, &sim.view, &sim.txs, &params, None);
    assert!(r.tree_ok, "{:?}", r.warnings);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    // Deposits: exactly ours, each with its true leaf and claim state.
    let got: BTreeSet<(usize, u64, u64, bool)> = r
        .deposits
        .iter()
        .map(|d| (ME.iter().position(|a| a.value() == &d.account).unwrap(), d.key_index, d.leaf_index, d.claim != ClaimState::Claimable))
        .collect();
    let want: BTreeSet<(usize, u64, u64, bool)> = sim.deposits.iter().map(|d| (d.who, d.key_index, d.leaf_index, d.claimed)).collect();
    assert_eq!(got, want);
    let _ = sim.deposits.iter().map(|d| d.di).count();
    // The claim state agrees with the simulator's own nullifier PDAs (point-query mode too).
    let spent: HashMap<[u8; 32], u64> = r
        .deposits
        .iter()
        .map(|d| *nullifier_address(&BRIDGE, &d.claim_nullifier).value())
        .filter(|pda| sim.w.exists(AccountId::new(*pda)))
        .map(|pda| (pda, 0))
        .collect();
    let r2 = recover(&wallet(), &registry, &sim.view, &[], &params, Some(&spent));
    assert_eq!(r2.claimable().count(), r.claimable().count());

    // Exits: exactly ours, in the right state.
    let got: BTreeSet<(usize, u64, [u8; 32], String)> = r
        .exits
        .iter()
        .map(|e| {
            let state = match e.exit {
                ExitState::Burned => "burned".to_owned(),
                ExitState::Unreleased { exit_index } => format!("tree {exit_index}"),
                ExitState::Released { .. } => "released".to_owned(),
            };
            (ME.iter().position(|a| a.value() == &e.account).unwrap(), e.key_index, e.exit_commitment, state)
        })
        .collect();
    let want: BTreeSet<(usize, u64, [u8; 32], String)> = sim
        .exits
        .iter()
        .map(|x| {
            let state = match (x.released, x.exit_index) {
                (true, _) => "released".to_owned(),
                (false, Some(i)) => format!("tree {i}"),
                (false, None) => "burned".to_owned(),
            };
            (x.who, x.key_index, x.e, state)
        })
        .collect();
    assert_eq!(got, want);

    // A fresh client's next allocation never reproduces an existing commitment or nullifier.
    let used_c: BTreeSet<[u8; 32]> = sim.view.deposits.iter().map(|d| d.commitment).collect();
    let used_e: BTreeSet<[u8; 32]> = sim.exits.iter().map(|x| x.e).collect();
    for who in 0..3 {
        for di in 0..2 {
            let j = allocate(&dep_key(&ME[who], &USDC, DENOMS[di]), &r, &BTreeMap::new());
            let c = deposit_candidate(&root_of(who), &cfg(), &ME[who], &USDC, DENOMS[di], j);
            assert!(!used_c.contains(&c.commitment));
            for (dest, fee) in [(Dest::Derived, kdf::max_relay_fee_cap(DENOMS[di])), (Dest::Address(USER_DEST), 0)] {
                let j = allocate(&exit_key(&ME[who], &USDC, DENOMS[di], &dest, fee), &r, &BTreeMap::new());
                let e = exit_candidate(&root_of(who), &cfg(), &ME[who], &USDC, DENOMS[di], &dest, fee, j);
                assert!(!used_e.contains(&e.exit_commitment));
                assert!(sim.exits.iter().all(|x| x.nf != e.release_nullifier));
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn a_fresh_client_recovers_exactly_the_open_positions(ops in prop::collection::vec(op(), 1..50)) {
        let mut sim = Sim::new();
        for op in &ops {
            sim.apply(op);
        }
        check(&sim);
    }
}

/// One long fixed history, so the property's shape is also exercised deterministically.
#[test]
fn a_long_fixed_history() {
    let mut sim = Sim::new();
    let ops = [
        Op::Deposit(0, 0),
        Op::Deposit(0, 1),
        Op::Abandon(1, 0),
        Op::Abandon(1, 0),
        Op::Deposit(1, 0),
        Op::Foreign(3),
        Op::Claim(0),
        Op::Claim(0),
        Op::Burn(0, 0, false),
        Op::Burn(0, 0, true),
        Op::FailedBurn(0),
        Op::Update,
        Op::Release(0),
        Op::Deposit(2, 1),
        Op::Foreign(6),
        Op::Claim(5),
        Op::Burn(0, 0, false),
        Op::Update,
    ];
    for op in &ops {
        sim.apply(op);
    }
    assert!(sim.exits.len() >= 2);
    check(&sim);
}
