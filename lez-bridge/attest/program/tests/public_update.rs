// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public update path (`Instruction::UpdatePublic`, feature `public-update`, branch
//! `public-update`): natively, through the same whole-transaction simulator as `instance.rs`.
//! Its cycle count in the zkVM is pinned by `attest/cycles/tests/public_cycles.rs` (build host).
#![cfg(feature = "public-update")]

mod common;
use common::*;
use eth_attest_core::KeyWitnesses;

fn witnesses(committee: &[u8], update: &[u8]) -> KeyWitnesses {
    let bits = eth_attest::lc::finality_bits(update).unwrap();
    eth_attest::lc::key_witnesses(committee, &bits).unwrap()
}

fn public_msg(w: &World, n: &Net, update: &[u8], keys: Option<KeyWitnesses>) -> Message {
    let prev = w.store(&n.id);
    let (current_committee, _) = n.committees.witness(&prev);
    let keys = keys.unwrap_or_else(|| witnesses(&current_committee, update));
    Message::Instruction(Instruction::UpdatePublic { config: n.cfg.clone(), prev, current_committee, keys, update: update.to_vec() })
}

fn submit_public(w: &mut World, who: AccountId, n: &Net, update: &[u8], keys: Option<KeyWitnesses>) -> Result<(), String> {
    let m = public_msg(w, n, update, keys);
    w.tx(who, ATTEST, &m)
}

#[test]
fn the_public_path_advances_the_store_exactly_as_the_private_one() {
    let fin = fx("sepolia", "finality_update.ssz");
    let mut a = World::default();
    let na = sepolia(&mut a);
    submit(&mut a, BOB, &na, LcUpdate::Finality(fin.clone())).unwrap();
    let mut b = World::default();
    let nb = sepolia(&mut b);
    submit_public(&mut b, BOB, &nb, &fin, None).unwrap();
    assert_eq!(b.store(&nb.id), a.store(&na.id), "same new store from both paths");
    let fu = &meta("sepolia")["finality_update"];
    let s = b.store(&nb.id);
    assert_eq!(s.finalized.beacon.slot, fu["finalized_slot"].as_u64().unwrap());
    assert_eq!(s.updates, 1);
    let anchor = Anchor::BlockHash(s.finalized.execution.block_hash);
    assert!(b.anchor(&nb.id, &anchor).is_some(), "the new anchor is recorded");
    assert_eq!(b.attest_states(), a.attest_states(), "every account of the program the same");
    // Replaying it is refused: the store moved on (prev no longer matches the instruction's).
    err_contains(submit_public(&mut b, CAROL, &nb, &fin, None), "does not advance");
}

#[test]
fn the_mainnet_fixture_also_verifies() {
    let mut w = World::default();
    let n = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    submit_public(&mut w, BOB, &n, &fx("mainnet", "finality_update.ssz"), None).unwrap();
    assert_eq!(w.store(&n.id).updates, 1);
}

#[test]
fn tampered_updates_and_witnesses_are_refused_and_change_nothing() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let before = w.clone();
    let fin = fx("sepolia", "finality_update.ssz");
    let committee = n.committees.witness(&w.store(&n.id)).0;
    let good = witnesses(&committee, &fin);
    assert_eq!(good.absent.len(), 3, "509/512: three non-signers");
    // One signature byte flipped.
    let mut t = fin.clone();
    t[1344 + 60] ^= 1;
    err_contains(submit_public(&mut w, BOB, &n, &t, Some(good.clone())), "rejected");
    // One participation bit cleared: the witnesses no longer match the bits.
    let mut t = fin.clone();
    let byte = (0..64).find(|i| t[1280 + i] != 0).unwrap();
    let bit = (0..8).find(|b| t[1280 + byte] >> b & 1 == 1).unwrap();
    t[1280 + byte] ^= 1 << bit;
    err_contains(submit_public(&mut w, BOB, &n, &t, Some(good.clone())), "invalid sync committee signature");
    // ... and with witnesses rebuilt for the changed bits, the signature does not match.
    err_contains(submit_public(&mut w, BOB, &n, &t, None), "invalid sync committee signature");
    // Finalized header altered.
    let mut t = fin.clone();
    t[496 + 16 + 5] ^= 1;
    err_contains(submit_public(&mut w, BOB, &n, &t, None), "invalid finality proof");
    // A missing, an extra, a reordered and a wrong key witness.
    let mut k = good.clone();
    k.absent.pop();
    err_contains(submit_public(&mut w, BOB, &n, &fin, Some(k)), "invalid sync committee signature");
    let mut k = good.clone();
    k.absent.push(k.absent[0]);
    err_contains(submit_public(&mut w, BOB, &n, &fin, Some(k)), "invalid sync committee signature");
    let mut k = good.clone();
    k.absent.swap(0, 1);
    err_contains(submit_public(&mut w, BOB, &n, &fin, Some(k)), "invalid sync committee signature");
    // Another member's key at a non-signer's index. (A sync committee can hold one validator more
    // than once: in this fixture the three non-signers' slots hold the same key, so swapping two
    // of their keys would be the same witness; a signer's key is a different one.)
    let mut k = good.clone();
    let at = usize::from(k.absent[0].0);
    let signer = (0..512)
        .find(|i| k.absent.iter().all(|(j, _)| usize::from(*j) != *i) && committee[i * 48..i * 48 + 48] != committee[at * 48..at * 48 + 48])
        .unwrap();
    k.absent[0].1 = eth_attest::lc::fast::uncompress(&committee[signer * 48..signer * 48 + 48]).unwrap();
    err_contains(submit_public(&mut w, BOB, &n, &fin, Some(k)), "invalid sync committee signature");
    // The aggregate replaced by a member's key.
    let mut k = good.clone();
    k.aggregate = k.absent[0].1;
    err_contains(submit_public(&mut w, BOB, &n, &fin, Some(k)), "invalid sync committee signature");
    // A committee witness that is not the store's.
    let mut m = public_msg(&w, &n, &fin, Some(good.clone()));
    if let Message::Instruction(Instruction::UpdatePublic { current_committee, .. }) = &mut m {
        current_committee[100] ^= 1;
    }
    err_contains(w.tx(BOB, ATTEST, &m), "committee witness does not match the store");
    assert_eq!(w.states, before.states, "refused updates leave every account as it was");
}

#[test]
fn rotations_and_next_committee_stores_go_the_private_way() {
    // A store a period behind: the update is signed outside its period (Helios: invalid period).
    let mut w = World::default();
    let mut n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let fin = fx("sepolia", "finality_update.ssz");
    err_contains(submit_public(&mut w, BOB, &n, &fin, None), "invalid sync committee period");
    // Once the store knows its next committee (period update, private path), the public path refuses.
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    submit(&mut w, BOB, &n, LcUpdate::Period(u1410.clone())).unwrap();
    n.committees.from_period_update(&u1410);
    err_contains(submit_public(&mut w, BOB, &n, &fin, None), "next committee is unknown");
}

#[test]
fn only_the_submitters_own_signed_actor_runs_it() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let m = public_msg(&w, &n, &fx("sepolia", "finality_update.ssz"), None);
    // Delivered to the store PDA as an instruction from another actor: refused.
    let to = Actor::new(store_address(&ATTEST, &n.id), ATTEST);
    err_contains(w.deliver(Some(Actor::new(BOB, ATTEST)), to, borsh::to_vec(&m).unwrap(), None), "sent by the transaction itself");
}
