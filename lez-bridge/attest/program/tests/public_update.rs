// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public update path (feature `public-update`, branch `public-update`, `src/public.rs`):
//! natively, through the same whole-transaction simulator as `instance.rs`.
//! - `UpdatePublic`: a finality update in one transaction, by the current or (rotation) the next
//!   committee;
//! - `PeriodBegin` / `PeriodFinish` / `CloseJob`: a period update in two transactions through a
//!   job PDA.
//! The zkVM cycle counts are pinned by `attest/cycles/tests/public_cycles.rs` (build host).
#![cfg(feature = "public-update")]

mod common;
use common::*;
use eth_attest::public::{fast, key_witnesses, sync_committee_root, update_bits};
use eth_attest_core::{JobState, KeyWitnesses, PeriodJob, job_address, job_id};
use helios_consensus_core::types::SyncCommittee;
use tree_hash::TreeHash;

/// Key witnesses for this update's bits against this committee.
fn witnesses(committee: &[u8], update: &[u8], period: bool) -> KeyWitnesses {
    key_witnesses(committee, &update_bits(update, period).unwrap()).unwrap()
}

fn public_msg(w: &World, n: &Net, committee: &[u8], update: &[u8], keys: Option<KeyWitnesses>) -> Message {
    let prev = w.store(&n.id);
    let keys = keys.unwrap_or_else(|| witnesses(committee, update, false));
    Message::Instruction(Instruction::UpdatePublic { config: n.cfg.clone(), prev, committee: committee.to_vec(), keys, update: update.to_vec() })
}

/// A finality update by the committee `which` (0 current, 1 next) of the store.
fn submit_public(w: &mut World, who: AccountId, n: &Net, which: usize, update: &[u8], keys: Option<KeyWitnesses>) -> Result<(), String> {
    let (cur, next) = n.committees.witness(&w.store(&n.id));
    let committee = if which == 0 { cur } else { next.expect("next committee known") };
    let m = public_msg(w, n, &committee, update, keys);
    w.tx(who, ATTEST, &m)
}

fn begin(w: &mut World, who: AccountId, n: &Net, update: &[u8]) -> Result<PeriodJob, String> {
    let prev = w.store(&n.id);
    w.tx(who, ATTEST, &Message::Instruction(Instruction::PeriodBegin { config: n.cfg.clone(), prev: prev.clone(), update: update.to_vec() }))?;
    let id = job_id(&prev, &eth_attest_core::sha256(&[update]));
    match borsh::from_slice::<JobState>(&w.state(job_address(&ATTEST, &id), ATTEST)) {
        Ok(JobState::Open(job)) => Ok(job),
        other => panic!("no open job after Begin: {other:?}"),
    }
}

fn finish(w: &mut World, who: AccountId, job: &PeriodJob, committee: &[u8], update: &[u8]) -> Result<(), String> {
    let keys = witnesses(committee, update, true);
    w.tx(who, ATTEST, &Message::Instruction(Instruction::PeriodFinish { job: job.clone(), committee: committee.to_vec(), keys }))
}

fn committee_of(n: &Net, h: &[u8; 32]) -> Vec<u8> {
    n.committees.0[h].clone()
}

// --- finality, one transaction ----------------------------------------------------------------

#[test]
fn the_public_path_advances_the_store_exactly_as_the_private_one() {
    let fin = fx("sepolia", "finality_update.ssz");
    let mut a = World::default();
    let na = sepolia(&mut a);
    submit(&mut a, BOB, &na, LcUpdate::Finality(fin.clone())).unwrap();
    let mut b = World::default();
    let nb = sepolia(&mut b);
    submit_public(&mut b, BOB, &nb, 0, &fin, None).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id), "same new store from both paths");
    assert_eq!(b.store(&nb.id).updates, 1);
    assert_eq!(b.attest_states(), a.attest_states(), "every account of the program the same");
    err_contains(submit_public(&mut b, CAROL, &nb, 0, &fin, None), "does not advance");
}

#[test]
fn the_mainnet_fixture_also_verifies() {
    let mut w = World::default();
    let n = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    submit_public(&mut w, BOB, &n, 0, &fx("mainnet", "finality_update.ssz"), None).unwrap();
    assert_eq!(w.store(&n.id).updates, 1);
}

#[test]
fn tampered_updates_and_witnesses_are_refused_and_change_nothing() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let before = w.clone();
    let fin = fx("sepolia", "finality_update.ssz");
    let committee = n.committees.witness(&w.store(&n.id)).0;
    let good = witnesses(&committee, &fin, false);
    assert_eq!(good.absent.len(), 3, "509/512: three non-signers");
    let sig = "invalid sync committee signature";
    let mut t = fin.clone();
    t[1344 + 60] ^= 1;
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, Some(good.clone())), "rejected");
    let mut t = fin.clone();
    let byte = (0..64).find(|i| t[1280 + i] != 0).unwrap();
    let bit = (0..8).find(|b| t[1280 + byte] >> b & 1 == 1).unwrap();
    t[1280 + byte] ^= 1 << bit;
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, Some(good.clone())), sig);
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, None), sig);
    let mut t = fin.clone();
    t[496 + 16 + 5] ^= 1;
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, None), "invalid finality proof");
    // Missing, extra, reordered witnesses.
    let mut k = good.clone();
    k.absent.pop();
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    let mut k = good.clone();
    k.absent.push(k.absent[0]);
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    let mut k = good.clone();
    k.absent.swap(0, 1);
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    // Another member's key at a non-signer's index (in this fixture the three non-signers' slots
    // hold one validator's key, so take a signer's, which differs).
    let mut k = good.clone();
    let at = usize::from(k.absent[0].0);
    let signer = (0..512)
        .find(|i| k.absent.iter().all(|(j, _)| usize::from(*j) != *i) && committee[i * 48..i * 48 + 48] != committee[at * 48..at * 48 + 48])
        .unwrap();
    k.absent[0].1 = fast::uncompress(&committee[signer * 48..signer * 48 + 48]).unwrap();
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    let mut k = good.clone();
    k.aggregate = k.absent[0].1;
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    // The right point with flag bits set (compressed, infinity), an off-curve point, infinity.
    for flag in [0x80u8, 0x40, 0x20] {
        let mut k = good.clone();
        k.absent[0].1[0] |= flag;
        err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    }
    for flag in [0x80u8, 0x40, 0x20] {
        let mut k = good.clone();
        k.aggregate[0] |= flag;
        err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    }
    let mut k = good.clone();
    k.absent[0].1[95] ^= 1;
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    let mut k = good.clone();
    k.absent[0].1 = [0u8; 96];
    k.absent[0].1[0] = 0x40;
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    // An index of 512 or more.
    let mut k = good.clone();
    k.absent[2].0 = 600;
    err_contains(submit_public(&mut w, BOB, &n, 0, &fin, Some(k)), sig);
    // The infinity signature, and a signature that is not a G2 point.
    let mut t = fin.clone();
    t[1344..1440].fill(0);
    t[1344] = 0xC0;
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, None), "rejected");
    let mut t = fin.clone();
    t[1344..1440].fill(0x11);
    t[1344] = 0x91;
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, None), "rejected");
    // Participation below the instance's policy (342): clear 200 bits.
    let mut t = fin.clone();
    let mut cleared = 0;
    for i in 0..512 {
        if cleared < 200 && t[1280 + i / 8] >> (i % 8) & 1 == 1 {
            t[1280 + i / 8] &= !(1 << (i % 8));
            cleared += 1;
        }
    }
    err_contains(submit_public(&mut w, BOB, &n, 0, &t, None), "below the instance's policy");
    // A committee witness that is not the store's.
    let mut bad = committee.clone();
    bad[100] ^= 1;
    let m = public_msg(&w, &n, &bad, &fin, Some(good.clone()));
    err_contains(w.tx(BOB, ATTEST, &m), sig);
    assert_eq!(w.states, before.states, "refused updates leave every account as it was");
}

#[test]
fn only_the_submitters_own_signed_actor_runs_it() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let committee = n.committees.witness(&w.store(&n.id)).0;
    let m = public_msg(&w, &n, &committee, &fx("sepolia", "finality_update.ssz"), None);
    let to = Actor::new(store_address(&ATTEST, &n.id), ATTEST);
    err_contains(w.deliver(Some(Actor::new(BOB, ATTEST)), to, borsh::to_vec(&m).unwrap(), None), "sent by the transaction itself");
}

// --- the handoff, all public ---------------------------------------------------------------------

#[test]
fn direct_committee_root_equals_tree_hash() {
    for c in [
        Bootstrap::<S>::from_ssz_bytes(&fx("sepolia", "bootstrap_prev_period.ssz")).unwrap().current_sync_committee().clone(),
        GenericUpdate::from(&Update::<S>::from_ssz_bytes(&fx("sepolia", "update_period_1410.ssz")).unwrap()).next_sync_committee.unwrap(),
    ] {
        let c: SyncCommittee<S> = c;
        assert_eq!(sync_committee_root(&c), c.tree_hash_root());
    }
}

/// The private handoff of `instance.rs`, all public: period update 1410 (Begin, Finish), the
/// finality update signed by the next committee (rotation into 1411), then period update 1411
/// (the second handoff). Same stores as the private path, step by step.
#[test]
fn a_committee_handoff_through_the_public_path_matches_the_private_one() {
    let fin = fx("sepolia", "finality_update.ssz");
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    let cfg = config("sepolia", "bootstrap_prev_period");
    let mut a = World::default();
    let mut na = init(&mut a, ALICE, "sepolia", "bootstrap_prev_period", cfg.clone()).unwrap();
    let mut b = World::default();
    let mut nb = init(&mut b, ALICE, "sepolia", "bootstrap_prev_period", cfg).unwrap();

    // Period update 1410, signed by the current committee.
    submit(&mut a, BOB, &na, LcUpdate::Period(u1410.clone())).unwrap();
    let job = begin(&mut b, BOB, &nb, &u1410).unwrap();
    assert_eq!(job.committee, b.store(&nb.id).current_committee);
    assert_eq!(b.store(&nb.id).updates, 0, "Begin alone changes no store");
    let cur = committee_of(&nb, &job.committee);
    finish(&mut b, CAROL, &job, &cur, &u1410).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id), "next committee learned, as privately");
    na.committees.from_period_update(&u1410);
    nb.committees.from_period_update(&u1410);

    // Rotation: the finality update signed in 1411 by the next committee.
    // With the current committee as witness it is refused.
    err_contains(submit_public(&mut b, BOB, &nb, 0, &fin, None), "invalid sync committee signature");
    submit(&mut a, BOB, &na, LcUpdate::Finality(fin.clone())).unwrap();
    submit_public(&mut b, BOB, &nb, 1, &fin, None).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id), "rotated, as privately");
    assert_eq!(b.store(&nb.id).next_committee, None);

    // Second handoff: period update 1411 by the (new) current committee.
    submit(&mut a, BOB, &na, LcUpdate::Period(u1411.clone())).unwrap();
    let job = begin(&mut b, BOB, &nb, &u1411).unwrap();
    let cur = committee_of(&nb, &job.committee);
    finish(&mut b, BOB, &job, &cur, &u1411).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id));
    assert_eq!(b.store(&nb.id).updates, 3);
}

#[test]
fn period_steps_refuse_tampering_replays_and_stale_jobs() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let prev = w.store(&n.id);
    let begin_msg = |u: &[u8]| Message::Instruction(Instruction::PeriodBegin { config: n.cfg.clone(), prev: prev.clone(), update: u.to_vec() });

    // Tampered next-committee branch (the last 32-byte node before the finalized header area).
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1410).unwrap());
    let node = g.next_sync_committee_branch.as_ref().unwrap()[0];
    let pos = u1410.windows(32).position(|x| x == node.as_slice()).unwrap();
    let mut t = u1410.clone();
    t[pos] ^= 1;
    err_contains(w.tx(BOB, ATTEST, &begin_msg(&t)), "rejected");
    // A tampered next committee key (the branch no longer proves it).
    let key = g.next_sync_committee.as_ref().unwrap().as_ssz_bytes();
    let pos = u1410.windows(48).position(|x| x == &key[..48]).unwrap();
    let mut t = u1410.clone();
    t[pos + 47] ^= 1;
    err_contains(w.tx(BOB, ATTEST, &begin_msg(&t)), "rejected");
    // Wrong period: 1411's update to a store in 1410 that does not know 1411's committee.
    err_contains(w.tx(BOB, ATTEST, &begin_msg(&u1411)), "invalid sync committee period");

    // A job with a forged signature opens (Begin does not check it) but never finishes.
    let mut forged = u1410.clone();
    let sig = g.sync_aggregate.sync_committee_signature.as_ssz_bytes();
    let spos = forged.windows(96).position(|x| x == &sig[..]).unwrap();
    forged[spos + 60] ^= 1;
    let bad = begin(&mut w, BOB, &n, &forged).unwrap();
    let cur = committee_of(&n, &bad.committee);
    err_contains(finish(&mut w, BOB, &bad, &cur, &forged), "rejected");

    // The honest job: replayed Begin refused; Finish with the wrong committee or an altered job refused.
    let job = begin(&mut w, BOB, &n, &u1410).unwrap();
    err_contains(w.tx(CAROL, ATTEST, &begin_msg(&u1410)), "this job exists");
    let mut other = cur.clone();
    other[10] ^= 1;
    err_contains(finish(&mut w, BOB, &job, &other, &u1410), "invalid sync committee signature");
    let mut altered = job.clone();
    altered.next.updates += 7;
    err_contains(finish(&mut w, BOB, &altered, &cur, &u1410), "not this job");
    // A stranger cannot close a job on the current store.
    let id = job_id(&job.prev, &job.update_digest);
    err_contains(w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: id })), "can still finish");

    // The store moves (the same update, privately); the job is now stale.
    submit(&mut w, BOB, &n, LcUpdate::Period(u1410.clone())).unwrap();
    let moved = w.store(&n.id);
    err_contains(finish(&mut w, BOB, &job, &cur, &u1410), "the store changed since this update was verified");
    assert_eq!(w.store(&n.id), moved);
    // Anyone may close it, and the forged one; then they are gone.
    w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: id })).unwrap();
    assert!(w.state(job_address(&ATTEST, &id), ATTEST).is_empty());
    let bad_id = job_id(&bad.prev, &bad.update_digest);
    w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: bad_id })).unwrap();
    assert!(w.state(job_address(&ATTEST, &bad_id), ATTEST).is_empty());
    err_contains(w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: id })), "no job here");
}

#[test]
fn a_finished_job_cannot_finish_again() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let job = begin(&mut w, BOB, &n, &u1410).unwrap();
    let cur = committee_of(&n, &job.committee);
    finish(&mut w, BOB, &job, &cur, &u1410).unwrap();
    // The store moved, so a second Finish stops at the store check, before the signature.
    err_contains(finish(&mut w, CAROL, &job, &cur, &u1410), "the store changed since this update was verified");
    let id = job_id(&job.prev, &job.update_digest);
    assert_eq!(borsh::from_slice::<JobState>(&w.state(job_address(&ATTEST, &id), ATTEST)).unwrap(), JobState::Done);
    // A finished job may be closed by anyone.
    w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: id })).unwrap();
}

// --- review 2026-11-17 ---------------------------------------------------------------------------

fn set_signature_slot(update: &[u8], slot: u64) -> Vec<u8> {
    let mut t = update.to_vec();
    let n = t.len();
    t[n - 8..].copy_from_slice(&slot.to_le_bytes());
    t
}

#[test]
fn a_store_that_knows_next_takes_period_updates_by_the_next_committee_and_refuses_bad_ones() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    let cfg = config("sepolia", "bootstrap_prev_period");
    let (mut a, mut b) = (World::default(), World::default());
    let mut na = init(&mut a, ALICE, "sepolia", "bootstrap_prev_period", cfg.clone()).unwrap();
    let mut nb = init(&mut b, ALICE, "sepolia", "bootstrap_prev_period", cfg).unwrap();
    submit(&mut a, BOB, &na, LcUpdate::Period(u1410.clone())).unwrap();
    let job = begin(&mut b, BOB, &nb, &u1410).unwrap();
    finish(&mut b, BOB, &job, &committee_of(&nb, &job.committee), &u1410).unwrap();
    na.committees.from_period_update(&u1410);
    nb.committees.from_period_update(&u1410);
    // The same period's update again (its next committee matches the known one, so the update's
    // own committee stands in): it verifies but advances nothing, refused as on the private path.
    err_contains(submit(&mut a, BOB, &na, LcUpdate::Period(u1410.clone())), "does not advance");
    err_contains(begin(&mut b, BOB, &nb, &u1410).map(drop), "does not advance");
    // Its next committee tampered (no longer the known one, nor proved by the branch): refused.
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1410).unwrap());
    let key = g.next_sync_committee.as_ref().unwrap().as_ssz_bytes();
    let pos = u1410.windows(48).position(|x| x == &key[..48]).unwrap();
    let mut t = u1410.clone();
    t[pos + 47] ^= 1;
    err_contains(begin(&mut b, BOB, &nb, &t).map(drop), "rejected");
    // Period update 1411, signed in 1411 by the store's NEXT committee: the job names that
    // committee, and Finish rotates exactly as the private path does.
    submit(&mut a, BOB, &na, LcUpdate::Period(u1411.clone())).unwrap();
    let job = begin(&mut b, BOB, &nb, &u1411).unwrap();
    assert_eq!(Some(job.committee), b.store(&nb.id).next_committee, "signed by the next committee");
    let cur = committee_of(&nb, &b.store(&nb.id).current_committee);
    err_contains(finish(&mut b, BOB, &job, &cur, &u1411), "invalid sync committee signature");
    finish(&mut b, BOB, &job, &committee_of(&nb, &job.committee), &u1411).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id));
}

#[test]
fn a_signature_slot_in_the_next_period_selects_the_next_committee() {
    // Update 1410 is attested and signed in 1410; with its signature slot moved into 1411 (same
    // fork, so the same signing root), Helios selects the next committee: a store that does not
    // know it refuses the period, and one that knows it checks the signature against it (fails).
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1410).unwrap());
    let next_period_slot = (g.signature_slot / 8192 + 1) * 8192 + 1;
    let moved = set_signature_slot(&u1410, next_period_slot);
    let mut w = World::default();
    let mut n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    err_contains(begin(&mut w, BOB, &n, &moved).map(drop), "invalid sync committee period");
    let job = begin(&mut w, BOB, &n, &u1410).unwrap();
    finish(&mut w, BOB, &job, &committee_of(&n, &job.committee), &u1410).unwrap();
    n.committees.from_period_update(&u1410);
    // Attested in 1410, signature slot re-dated into 1411: with 1411's committee known, Helios
    // selects it; the update proves nothing new for this store, so Begin refuses it.
    assert!(begin(&mut w, BOB, &n, &moved).is_err());
    // The store now knows 1411's committee. Update 1411 is signed by it; move a CURRENT-committee
    // signature (update 1410's, re-dated into 1411) and Begin names the next committee, so Finish
    // checks against it and fails with either witness.
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    let g11 = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1411).unwrap());
    let sig10 = g.sync_aggregate.sync_committee_signature.as_ssz_bytes();
    let sig11 = g11.sync_aggregate.sync_committee_signature.as_ssz_bytes();
    let pos = u1411.windows(96).position(|x| x == &sig11[..]).unwrap();
    let mut forged = u1411.clone();
    forged[pos..pos + 96].copy_from_slice(&sig10);
    let job = begin(&mut w, BOB, &n, &forged).unwrap();
    assert_eq!(Some(job.committee), w.store(&n.id).next_committee);
    err_contains(finish(&mut w, BOB, &job, &committee_of(&n, &job.committee), &forged), "invalid sync committee signature");
    let cur = committee_of(&n, &w.store(&n.id).current_committee);
    err_contains(finish(&mut w, BOB, &job, &cur, &forged), "invalid sync committee signature");
}

#[test]
fn two_jobs_on_one_store_the_first_to_finish_wins_and_the_other_closes() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let mut other = u1410.clone();
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1410).unwrap());
    let sig = g.sync_aggregate.sync_committee_signature.as_ssz_bytes();
    let spos = other.windows(96).position(|x| x == &sig[..]).unwrap();
    other[spos + 60] ^= 1;
    let j2 = begin(&mut w, CAROL, &n, &other).unwrap();
    let j1 = begin(&mut w, BOB, &n, &u1410).unwrap();
    finish(&mut w, BOB, &j1, &committee_of(&n, &j1.committee), &u1410).unwrap();
    let id2 = job_id(&j2.prev, &j2.update_digest);
    err_contains(finish(&mut w, CAROL, &j2, &committee_of(&n, &j2.committee), &other), "the store changed since this update was verified");
    w.tx(ALICE, ATTEST, &Message::Instruction(Instruction::CloseJob { job_id: id2 })).unwrap();
    assert!(w.state(job_address(&ATTEST, &id2), ATTEST).is_empty());
}

#[test]
fn a_job_opens_only_on_a_real_instance_s_current_state() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    // A made-up instance (another salt: another instance id, no store): refused at Begin, so no
    // job exists that nobody could close.
    let mut cfg = n.cfg.clone();
    cfg.salt = [9; 32];
    let mut prev = w.store(&n.id);
    prev.instance_id = instance_id(&ATTEST, &cfg);
    let m = Message::Instruction(Instruction::PeriodBegin { config: cfg, prev, update: u1410.clone() });
    err_contains(w.tx(BOB, ATTEST, &m), "no such instance");
    // A real instance but a prev that is not its state: refused.
    let mut prev = w.store(&n.id);
    prev.updates = 5;
    let m = Message::Instruction(Instruction::PeriodBegin { config: n.cfg.clone(), prev, update: u1410 });
    err_contains(w.tx(BOB, ATTEST, &m), "the store changed since this update was verified");
    // JobClose goes only to the job's own PDA, and an empty store counts as moved.
    let fake = Actor::new(store_address(&ATTEST, &n.id), ATTEST);
    let e = Message::Effect(eth_attest_core::Effect::JobClose { job_id: [7; 32] });
    err_contains(w.deliver(Some(Actor::new(BOB, ATTEST)), fake, borsh::to_vec(&e).unwrap(), None), "job goes to its PDA");
}

#[test]
fn finish_carries_the_signature_slot_time_window() {
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let job = begin(&mut w, BOB, &n, &u1410).unwrap();
    let committee = committee_of(&n, &job.committee);
    let keys = witnesses(&committee, &u1410, true);
    let input = ReceiveInput {
        receiver: Actor::new(BOB, ATTEST),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: borsh::to_vec(&Message::Instruction(Instruction::PeriodFinish { job: job.clone(), committee, keys })).unwrap(),
    };
    let r = eth_attest::handle(&input);
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u1410).unwrap());
    let start = (n.cfg.genesis_time + g.signature_slot * n.cfg.seconds_per_slot) * 1000;
    assert_eq!(job.not_before_ms, start);
    assert_eq!(r.validity.timestamps.start(), Some(start), "Finish is valid only from the signature slot");
    assert_eq!(r.validity.timestamps.end(), Some(job.deadline_ms), "... and only until the weak-subjectivity deadline");
    assert_eq!(Some(job.deadline_ms), eth_attest_core::ws_deadline_ms(&n.cfg, &job.prev));
}

/// An SSZ `hash_tree_root(SyncCommittee)` written from the spec, independent of Helios and of
/// patch 2 (pubkey = Bytes48 -> two chunks; Vector[Bytes48, 512] -> merkleize; container of two).
fn spec_root(ssz: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let h = |a: &[u8], b: &[u8]| -> [u8; 32] { Sha256::new().chain_update(a).chain_update(b).finalize().into() };
    let key_root = |k: &[u8]| h(&k[..32], &[&k[32..48], &[0u8; 16][..]].concat());
    let mut layer: Vec<[u8; 32]> = (0..512).map(|i| key_root(&ssz[i * 48..i * 48 + 48])).collect();
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|p| h(&p[0], &p[1])).collect();
    }
    h(&layer[0], &key_root(&ssz[512 * 48..]))
}

#[test]
fn patch_2_matches_tree_hash_and_an_independent_spec_root_on_every_fixture_and_random_committees() {
    let mut committees: Vec<Vec<u8>> = Vec::new();
    for (net, f) in [("sepolia", "bootstrap_prev_period.ssz"), ("sepolia", "bootstrap_same_period.ssz"), ("mainnet", "bootstrap_same_period.ssz")] {
        committees.push(Bootstrap::<S>::from_ssz_bytes(&fx(net, f)).unwrap().current_sync_committee().as_ssz_bytes());
    }
    for f in ["update_period_1410.ssz", "update_period_1411.ssz"] {
        committees.push(GenericUpdate::from(&Update::<S>::from_ssz_bytes(&fx("sepolia", f)).unwrap()).next_sync_committee.unwrap().as_ssz_bytes());
    }
    // Random committees (any bytes: the root does not need curve points), from a fixed seed.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..8 {
        let mut c = vec![0u8; 513 * 48];
        for b in &mut c {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        committees.push(c);
    }
    assert_eq!(committees.len(), 13);
    for ssz in &committees {
        let c = SyncCommittee::<S>::from_ssz_bytes(ssz).unwrap();
        let direct = sync_committee_root(&c).0;
        assert_eq!(direct, c.tree_hash_root().0, "patch 2 vs tree_hash");
        assert_eq!(direct, spec_root(ssz), "patch 2 vs the spec, written here");
    }
}
