// SPDX-License-Identifier: MIT OR Apache-2.0
//! G4 tests, executor only. The vault-side refusals (replay, redirect, unknown root, wrong final
//! state, before wind-down) are in vault/test/failsafe/; here: what the guest itself proves or
//! refuses, and that every field the vault pays from is bound into the journal.

use claim_acc::{NonMembership, Tree};
use reclaim_core::{ReclaimError, ReclaimJournal, reclaim};
use reclaim_host::*;

fn real_roots(s: &Scenario) -> Vec<[u8; 32]> {
    // every deposit root the scenario's vault reached (after 1, 2, 3 deposits)
    (1..=s.leaves.len())
        .map(|n| {
            let path = bridge_core::merkle_path(&s.leaves[..n], 0);
            claim_acc::merkle_root(&s.leaves[0], 0, &path).unwrap()
        })
        .collect()
}

#[test]
fn an_unclaimed_deposit_reclaims_and_the_guest_equals_native() {
    let s = scenario();
    let input = input_for(&s, 0, recipient(0), [0; 20], 0).expect("deposit 0 is unclaimed");
    let run = execute(&input).expect("guest runs");
    let j = &run.decoded;
    assert_eq!(j.cfg_id, cfg().cfg_id());
    assert_eq!(j.deposit_root, *real_roots(&s).last().unwrap());
    assert_eq!(j.final_state, s.state.commitment());
    assert_eq!(j.nullifier, reclaim_core::reclaim_nullifier(&s.masters[0]));
    assert_eq!(j.token, USDC);
    assert_eq!(j.denom, reclaim_core::u256(DENOM));
    assert_eq!(j.recipient, recipient(0));
    assert_eq!(run.journal.len(), 288);
    println!("G4 deposit 0: user cycles {}, total {}, segments {}", run.user_cycles, run.total_cycles, run.segments);
    // deposit 2 as well, through a relayer with a fee
    let input = input_for(&s, 2, recipient(2), relayer(), 1_000_000).expect("deposit 2 is unclaimed");
    let run = execute(&input).expect("guest runs");
    assert_eq!(run.decoded.relayer, relayer());
    assert_eq!(run.decoded.relay_fee, reclaim_core::u256(1_000_000));
}

#[test]
fn a_claimed_deposit_has_no_reclaim() {
    let s = scenario();
    // the host finds no non-membership witness
    assert_eq!(input_for(&s, CLAIMED, recipient(1), [0; 20], 0).unwrap_err(), BuildError::Claimed);
    // and no witness made by hand verifies: every leaf of the tree as the "low leaf"
    let mut forged = input_for(&s, 0, recipient(1), [0; 20], 0).unwrap();
    forged.master = s.masters[CLAIMED];
    forged.leaf_index = CLAIMED as u64;
    forged.path = bridge_core::merkle_path(&s.leaves, CLAIMED);
    for (i, l) in s.tree.leaves().iter().enumerate() {
        forged.non_membership = NonMembership { low_index: i as u64, low: *l, path: s.tree.path(i as u64) };
        assert_eq!(reclaim(&forged), Err(ReclaimError::Claimed), "low leaf {i}");
    }
    // the guest refuses too (no journal)
    let why = execute_refused(&forged).expect("the guest must refuse a claimed deposit");
    assert!(why.contains("reclaim refused"), "{why}");
}

#[test]
fn a_proof_against_another_accumulator_names_another_final_state() {
    // Before deposit 1's claim the tree did not hold its key: non-membership verifies there, but
    // the journal then names that older state, which is not the vault's finalState.
    let s = scenario();
    let mut early = Tree::new();
    early.insert(&reclaim_core::sha256(&[b"RFP022/g4-vector/other-claim", &[0u8]])).unwrap();
    let mut state = s.state.clone();
    state.claim_acc = early.state();
    let input = build_input(s.masters[CLAIMED], s.dest, USDC, DENOM, cfg(), &s.leaves, &state, &early, recipient(1), [0; 20], 0).unwrap();
    let j = reclaim(&input).unwrap();
    assert_ne!(j.final_state, s.state.commitment(), "an older accumulator gives another state commitment");
}

#[test]
fn a_wrong_secret_or_a_deposit_not_in_the_tree_gives_no_known_root() {
    let s = scenario();
    let roots = real_roots(&s);
    // the host refuses: no leaf matches
    let wrong = reclaim_core::sha256(&[b"not the master"]);
    assert_eq!(
        build_input(wrong, s.dest, USDC, DENOM, cfg(), &s.leaves, &s.state, &s.tree, recipient(0), [0; 20], 0).unwrap_err(),
        BuildError::NotDeposited
    );
    // forced through with deposit 0's path: the guest's root is not one the vault ever had
    let mut forced = input_for(&s, 0, recipient(0), [0; 20], 0).unwrap();
    forced.master = wrong;
    let key = reclaim_core::acc_key(&PROGRAM_ID, &reclaim_core::claim_secret(&wrong), &s.dest, &USDC, DENOM);
    forced.non_membership = s.tree.non_membership(&key).unwrap();
    let j = reclaim(&forced).unwrap();
    assert!(!roots.contains(&j.deposit_root), "wrong secret: root must be unknown to the vault");
    // another account, another token or another denomination with the right secret: also unknown
    for (dest, token, denom) in [([9u8; 32], USDC, DENOM), (s.dest, [0u8; 20], DENOM), (s.dest, USDC, 1_000_000_000)] {
        let mut f = input_for(&s, 0, recipient(0), [0; 20], 0).unwrap();
        f.dest = dest;
        f.token = token;
        f.denom = denom;
        let k = reclaim_core::claim_secret(&f.master);
        f.non_membership = s.tree.non_membership(&reclaim_core::acc_key(&PROGRAM_ID, &k, &dest, &token, denom)).unwrap();
        let j = reclaim(&f).unwrap();
        assert!(!roots.contains(&j.deposit_root));
    }
    // a malformed path is refused
    let mut bad = input_for(&s, 0, recipient(0), [0; 20], 0).unwrap();
    bad.path.pop();
    assert_eq!(reclaim(&bad), Err(ReclaimError::BadPath));
}

#[test]
fn the_state_must_be_the_cfg_programs() {
    let s = scenario();
    let mut i = input_for(&s, 0, recipient(0), [0; 20], 0).unwrap();
    i.state.bridge_program = [0x44; 32];
    assert_eq!(reclaim(&i), Err(ReclaimError::WrongProgram));
    // and the key is the cfg program's: under another program id the deposit is not in the tree
    let mut j = input_for(&s, 0, recipient(0), [0; 20], 0).unwrap();
    j.cfg.program_id = [0x44; 32];
    j.state.bridge_program = [0x44; 32];
    // the old witness was for the key under PROGRAM_ID: under 0x44 the key differs
    assert_eq!(reclaim(&j), Err(ReclaimError::Claimed));
    let k = reclaim_core::claim_secret(&s.masters[0]);
    j.non_membership = s.tree.non_membership(&reclaim_core::acc_key(&[0x44; 32], &k, &s.dest, &USDC, DENOM)).unwrap();
    let out = reclaim(&j).unwrap();
    assert_ne!(out.cfg_id, cfg().cfg_id(), "another program is another cfgId, refused by the vault");
}

#[test]
fn payee_relayer_fee_and_amount_are_bound() {
    // A relayer holding a journal and seal can only submit them as made: each field it could
    // want to change (recipient, relayer, fee) is a journal word, so another value is another
    // digest, which the seal does not cover. The amount is the denomination inside C.
    let s = scenario();
    let base = reclaim(&input_for(&s, 2, recipient(2), relayer(), 1_000_000).unwrap()).unwrap();
    let changed: Vec<ReclaimJournal> = vec![
        reclaim(&input_for(&s, 2, recipient(9), relayer(), 1_000_000).unwrap()).unwrap(),
        reclaim(&input_for(&s, 2, recipient(2), recipient(8), 1_000_000).unwrap()).unwrap(),
        reclaim(&input_for(&s, 2, recipient(2), relayer(), 2_000_000).unwrap()).unwrap(),
    ];
    for c in &changed {
        assert_ne!(c.digest(), base.digest());
        assert_eq!(c.nullifier, base.nullifier, "same deposit, same reclaim nullifier: one payout");
    }
}

#[test]
fn formulas_agree_with_the_client_kdf() {
    for i in 0u8..16 {
        let m = reclaim_core::sha256(&[b"master", &[i]]);
        assert_eq!(reclaim_core::claim_secret(&m), bridge_client::kdf::claim_secret_of(&m));
        assert_eq!(reclaim_core::reclaim_nullifier(&m), bridge_client::kdf::reclaim_nullifier(&m));
    }
    // the scenario's deposits are the client's seed-derived ones
    let s = scenario();
    let acct = lee_core::account::AccountId::new(s.dest);
    let root = bridge_client::kdf::AccountRoot::from_account_secret(&reclaim_core::sha256(&[b"RFP022/g4-vector/account-secret"]));
    for (i, c) in s.commitments.iter().enumerate() {
        let cand = bridge_client::scan::deposit_candidate(&root, &cfg().cfg_id(), &acct, &USDC, DENOM, i as u64);
        assert_eq!(cand.commitment, *c);
        assert_eq!(cand.master, s.masters[i]);
    }
}

#[test]
fn fixture_is_current() {
    // vault/test/fixtures/ReclaimVector.sol must be what the generator writes today
    let f = fixture(1_000_000).unwrap();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../vault/test/fixtures/ReclaimVector.sol");
    let on_disk = std::fs::read_to_string(path).unwrap_or_default();
    assert_eq!(on_disk, f.sol, "regenerate: reclaim_host vector --write <dir>, copy ReclaimVector.sol to vault/test/fixtures/");
}
