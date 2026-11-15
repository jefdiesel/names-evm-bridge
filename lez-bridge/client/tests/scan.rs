//! The recovery scan on hand-built chain data: gap limit, foreign deposits, copied commitments,
//! claim and exit classification, the deposit-tree check.

use std::collections::{BTreeMap, HashMap};

use bridge_client::{
    chain::{DepositEvent, ExitLeafEvent, LezTx, ReleasedEvent, VaultView},
    kdf::{self, AccountRoot},
    scan::{
        ClaimState, Dest, ExitState, IncrementalTree, ScanParams, TokenInfo, WalletAccount, allocate, dep_key, deposit_candidate,
        exit_candidate, exit_key, rebuild_leaves, recover,
    },
};
use bridge_core::{claim_nullifier, config_address, exit_leaf, merkle_path, merkle_root, nullifier_address};
use lee_core::account::AccountId;

const CFG: [u8; 32] = [0xC0; 32];
const USDC: [u8; 20] = [0xAA; 20];
const DAI: [u8; 20] = [0xDA; 20];
const BRIDGE: AccountId = AccountId::new([0xB0; 32]);
const D100: u128 = 100_000_000;
const D1000: u128 = 1_000_000_000;

fn registry() -> Vec<TokenInfo> {
    vec![TokenInfo { token: USDC, denoms: vec![D100, D1000] }, TokenInfo { token: DAI, denoms: vec![10u128.pow(20)] }]
}

fn account(n: u8) -> WalletAccount {
    WalletAccount { id: AccountId::new([n; 32]), root: AccountRoot::from_account_secret(&[n.wrapping_add(100); 32]), private: n % 2 == 0 }
}

fn params(gap: u64) -> ScanParams {
    ScanParams {
        cfg_id: CFG,
        bridge: BRIDGE,
        gap,
        eth_dests: vec![],
        extra_fees: vec![],
        finalized_block: Some(1_000_000),
        known_next: BTreeMap::new(),
    }
}

/// A vault that records deposits like BridgeVault (leaf, root, events).
#[derive(Default)]
struct Vault {
    tree: IncrementalTree,
    view: VaultView,
}

impl Vault {
    fn deposit(&mut self, token: [u8; 20], denom_index: u32, commitment: [u8; 32]) -> u64 {
        let denom = registry().iter().find(|t| t.token == token).unwrap().denoms[denom_index as usize];
        let leaf = bridge_core::deposit_leaf(&CFG, &token, denom, &commitment);
        let leaf_index = self.tree.count;
        let root = self.tree.insert(leaf);
        self.view.deposits.push(DepositEvent { token, denom_index, commitment, leaf_index, root, block: 100 + leaf_index, tx_hash: [leaf_index as u8; 32] });
        leaf_index
    }
    fn ours(&mut self, a: &WalletAccount, token: [u8; 20], denom_index: u32, index: u64) -> u64 {
        let denom = registry().iter().find(|t| t.token == token).unwrap().denoms[denom_index as usize];
        let c = deposit_candidate(&a.root, &CFG, &a.id, &token, denom, index);
        self.deposit(token, denom_index, c.commitment)
    }
}

fn rand32(seed: &mut u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    for b in &mut out {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (*seed >> 33) as u8;
    }
    out
}

#[test]
fn the_incremental_tree_matches_the_bridge_path_math() {
    let leaves: Vec<[u8; 32]> = (0u8..9).map(|i| [i; 32]).collect();
    let mut t = IncrementalTree::default();
    for (i, l) in leaves.iter().enumerate() {
        let root = t.insert(*l);
        let prefix = &leaves[..=i];
        assert_eq!(root, merkle_root(l, i as u64, &merkle_path(prefix, i)));
    }
}

#[test]
fn sparse_usage_within_the_gap_is_found() {
    let a = account(1);
    let mut v = Vault::default();
    for i in [0, 3, 7] {
        v.ours(&a, USDC, 0, i);
    }
    let r = recover(&[a], &registry(), &v.view, &[], &params(5), None);
    let idx: Vec<u64> = r.deposits.iter().map(|d| d.key_index).collect();
    assert_eq!(idx, vec![0, 3, 7]);
    assert!(r.tree_ok);
    assert!(r.deposits.iter().all(|d| d.claim == ClaimState::Claimable && d.finalized == Some(true)));
    let key = dep_key(&AccountId::new([1; 32]), &USDC, D100);
    assert_eq!(r.next[&key], 8);
    assert_eq!(r.scanned_to[&key], 13);
}

#[test]
fn exactly_at_the_gap_is_found_and_beyond_is_reported() {
    let gap = 4;
    // Indexes 1..=3 unused (gap - 1 of them), then index 4: found.
    let a = account(1);
    let mut v = Vault::default();
    v.ours(&a, USDC, 0, 0);
    v.ours(&a, USDC, 0, gap);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(gap), None);
    assert_eq!(r.deposits.len(), 2);

    // Indexes 1..=4 unused (gap of them): index 5 is past the window. Not found, and a cache that
    // handed out index 5 makes the scan say so.
    let mut v = Vault::default();
    v.ours(&a, USDC, 0, 0);
    v.ours(&a, USDC, 0, gap + 1);
    let key = dep_key(&a.id, &USDC, D100);
    let mut p = params(gap);
    p.known_next.insert(key.clone(), gap + 2);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &p, None);
    assert_eq!(r.deposits.len(), 1);
    assert!(r.warnings.iter().any(|w| w.contains(&key) && w.contains("rescan with --gap")), "{:?}", r.warnings);
    // A wider gap finds it.
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(gap + 1), None);
    assert_eq!(r.deposits.len(), 2);
}

#[test]
fn thousands_of_foreign_deposits_hide_nothing_and_add_nothing() {
    let (a, b) = (account(1), account(2));
    let mut v = Vault::default();
    let mut seed = 7;
    let mut ours = Vec::new();
    for n in 0..5_000u32 {
        let c = rand32(&mut seed);
        v.deposit(if n % 3 == 0 { DAI } else { USDC }, if n % 3 == 0 { 0 } else { n % 2 }, c);
        if n == 10 {
            ours.push(v.ours(&a, USDC, 1, 0));
        }
        if n == 2_500 {
            ours.push(v.ours(&b, DAI, 0, 0));
        }
        if n == 4_999 {
            ours.push(v.ours(&a, USDC, 1, 1));
        }
    }
    let r = recover(&[account(1), account(2), account(3)], &registry(), &v.view, &[], &params(20), None);
    assert!(r.tree_ok, "{:?}", r.warnings);
    let mut found: Vec<u64> = r.deposits.iter().map(|d| d.leaf_index).collect();
    found.sort_unstable();
    assert_eq!(found, ours);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    // Every found deposit's leaf is the vault's.
    let leaves = rebuild_leaves(&v.view.deposits, &registry(), &CFG).unwrap();
    for d in &r.deposits {
        assert_eq!(leaves[d.leaf_index as usize], bridge_core::deposit_leaf(&CFG, &d.token, d.denom, &d.commitment));
    }
}

#[test]
fn a_copied_commitment_under_another_denomination_is_not_a_position() {
    let a = account(1);
    let mut v = Vault::default();
    let c = deposit_candidate(&a.root, &CFG, &a.id, &USDC, D100, 0).commitment;
    v.deposit(USDC, 1, c); // someone deposited our C as a 1000
    v.ours(&a, USDC, 0, 1);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(5), None);
    assert_eq!(r.deposits.iter().map(|d| d.key_index).collect::<Vec<_>>(), vec![1]);
    assert!(r.warnings.iter().any(|w| w.contains("copied commitment")));
}

#[test]
fn claims_are_classified_from_the_nullifier_pda_in_transactions() {
    let a = account(1);
    let mut v = Vault::default();
    v.ours(&a, USDC, 0, 0);
    v.ours(&a, USDC, 0, 1);
    let k0 = deposit_candidate(&a.root, &CFG, &a.id, &USDC, D100, 0).secret;
    let pda = nullifier_address(&BRIDGE, &claim_nullifier(&k0, &a.id, &USDC, D100));
    let cfg_addr = config_address(&BRIDGE);
    let tx = LezTx { block: 42, bytes: [&[9u8; 7][..], cfg_addr.value(), &[1, 2, 3], pda.value(), &[4]].concat() };
    let r = recover(&[account(1)], &registry(), &v.view, &[tx], &params(5), None);
    assert_eq!(r.deposits[0].claim, ClaimState::Claimed { claim_block: 42 });
    assert_eq!(r.deposits[1].claim, ClaimState::Claimable);
    assert_eq!(r.claimable().count(), 1);
    // Point-query mode gives the same answer from a set of spent PDAs.
    let spent = HashMap::from([(*pda.value(), 0u64)]);
    let r2 = recover(&[account(1)], &registry(), &v.view, &[], &params(5), Some(&spent));
    assert_eq!(r2.claimable().count(), 1);
    // A claimed deposit survives the cache's JSON round trip (its LEZ block is not the
    // deposit's Ethereum `block`).
    let back: bridge_client::scan::Recovery = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
}

#[test]
fn exits_are_found_and_classified_in_all_three_states() {
    let a = account(4);
    let cfg_addr = config_address(&BRIDGE);
    let cap = kdf::max_relay_fee_cap(D100);
    let user = [0x77; 20];
    let mk = |dest: Dest, fee: u128, j: u64| exit_candidate(&a.root, &CFG, &a.id, &USDC, D100, &dest, fee, j);
    let e0 = mk(Dest::Derived, cap, 0); // burned only
    let e1 = mk(Dest::Derived, cap, 1); // in the exit tree
    let e2 = mk(Dest::Derived, cap, 3); // released (index 2 skipped)
    let e3 = mk(Dest::Address(user), 0, 0); // user-chosen address, fee 0
    let burn = |block: u64, e: [u8; 32]| LezTx { block, bytes: [cfg_addr.value().as_slice(), &[0, 1], &e].concat() };
    let txs = vec![burn(10, e0.exit_commitment), burn(11, e1.exit_commitment), burn(12, e2.exit_commitment), burn(13, e3.exit_commitment)];
    let mut view = VaultView::default();
    view.exit_leaves.push(ExitLeafEvent { leaf: exit_leaf(&CFG, &USDC, D100, &e1.exit_commitment), exit_index: 5, exit_root: [1; 32], block: 50 });
    view.exit_leaves.push(ExitLeafEvent { leaf: exit_leaf(&CFG, &USDC, D100, &e2.exit_commitment), exit_index: 6, exit_root: [1; 32], block: 50 });
    view.released.insert(
        e2.release_nullifier,
        ReleasedEvent { nullifier: e2.release_nullifier, token: USDC, denom_index: 0, recipient: e2.eth_dest, block: 60, tx_hash: [6; 32] },
    );

    // Without the user's address: the three derived exits only.
    let r = recover(&[account(4)], &registry(), &view, &txs, &params(5), None);
    let states: Vec<(u64, ExitState)> = r.exits.iter().map(|e| (e.key_index, e.exit.clone())).collect();
    assert_eq!(
        states,
        vec![(0, ExitState::Burned), (1, ExitState::Unreleased { exit_index: 5 }), (3, ExitState::Released { release_block: 60, tx_hash: [6; 32] })]
    );
    assert!(r.exits.iter().all(|e| e.dest_derived && e.max_relay_fee == cap));
    assert_eq!(r.unreleased().count(), 2);
    // Every state survives the cache's JSON round trip (flattened tags must not collide).
    let back: bridge_client::scan::Recovery = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back, r);
    assert_eq!(r.next[&exit_key(&a.id, &USDC, D100, &Dest::Derived, cap)], 4);

    // Naming the address finds the fourth.
    let mut p = params(5);
    p.eth_dests.push(user);
    let r = recover(&[account(4)], &registry(), &view, &txs, &p, None);
    assert_eq!(r.exits.len(), 4);
    let e = r.exits.iter().find(|e| !e.dest_derived).unwrap();
    assert_eq!((e.eth_dest, e.max_relay_fee, e.exit.clone()), (user, 0, ExitState::Burned));
}

#[test]
fn a_menu_fee_set_once_is_still_tried_after_the_menu_changed() {
    let a = account(4);
    let fee = 500_000;
    let e = exit_candidate(&a.root, &CFG, &a.id, &USDC, D100, &Dest::Derived, fee, 0);
    let txs = vec![LezTx { block: 3, bytes: [config_address(&BRIDGE).value().as_slice(), &e.exit_commitment].concat() }];
    let mut view = VaultView::default();
    let r = recover(&[account(4)], &registry(), &view, &txs, &params(5), None);
    assert!(r.exits.is_empty());
    // The menu held 500_000 at some point (rebuilt from RelayFeeMenuSet logs, not the current menu).
    view.menu_fees.entry((USDC, 0)).or_default().insert(fee);
    let r = recover(&[account(4)], &registry(), &view, &txs, &params(5), None);
    assert_eq!(r.exits.len(), 1);
}

#[test]
fn a_twice_burned_exit_commitment_is_reported() {
    let a = account(4);
    let cap = kdf::max_relay_fee_cap(D100);
    let e = exit_candidate(&a.root, &CFG, &a.id, &USDC, D100, &Dest::Derived, cap, 0);
    let tx = |b| LezTx { block: b, bytes: e.exit_commitment.to_vec() };
    let r = recover(&[account(4)], &registry(), &VaultView::default(), &[tx(1), tx(2)], &params(5), None);
    assert_eq!(r.exits[0].burns, 2);
    assert!(r.warnings.iter().any(|w| w.contains("only one can ever be released")));
}

#[test]
fn the_tree_check_catches_a_missing_range() {
    let a = account(1);
    let mut v = Vault::default();
    for i in 0..5 {
        v.ours(&a, USDC, 0, i);
    }
    v.view.deposits.remove(2);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(5), None);
    assert!(!r.tree_ok);
    assert!(r.warnings[0].contains("skip from leaf 2 to 3"));
}

#[test]
fn allocation_never_reuses_a_handed_out_or_used_index() {
    let a = account(1);
    let mut v = Vault::default();
    v.ours(&a, USDC, 0, 0);
    v.ours(&a, USDC, 0, 1);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(5), None);
    let key = dep_key(&a.id, &USDC, D100);
    assert_eq!(allocate(&key, &r, &BTreeMap::new()), 2);
    assert_eq!(allocate(&key, &r, &BTreeMap::from([(key.clone(), 4)])), 4);
    assert_eq!(allocate(&dep_key(&a.id, &USDC, D1000), &r, &BTreeMap::new()), 0);
}

#[test]
fn the_result_carries_no_secret() {
    let a = account(1);
    let mut v = Vault::default();
    v.ours(&a, USDC, 0, 0);
    let r = recover(&[account(1)], &registry(), &v.view, &[], &params(5), None);
    let json = serde_json::to_string(&r).unwrap();
    let c = deposit_candidate(&a.root, &CFG, &a.id, &USDC, D100, 0);
    for secret in [c.master, c.secret] {
        assert!(!json.contains(&bridge_client::hexser::hex(&secret)));
    }
    let back: bridge_client::scan::Recovery = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
}
