// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed error codes (RFP-021 U5, U2; branch `public-update`): each failure mode refused on chain
//! (here the whole-transaction simulator) with its `RFP021-Enn` code, and the off-chain dry run
//! (`eth_attest::check`) reporting the same code for the same input. Table: `docs/ERROR-CODES.md`.
#![cfg(feature = "public-update")]

mod common;
use common::*;
use eth_attest::check::{self, Code};

/// The code a refusal text carries; panics if it carries none.
fn code(r: Result<(), String>) -> Code {
    let e = r.expect_err("expected a refusal");
    Code::parse(&e).unwrap_or_else(|| panic!("no code in: {e}"))
}

fn update_err<T>(r: Result<T, Code>) -> Code {
    r.err().expect("expected a refusal")
}

#[test]
fn every_code_has_a_distinct_number_and_text_and_parses_back() {
    for (i, c) in Code::ALL.iter().enumerate() {
        assert_eq!(usize::from(c.number()), i + 1);
        assert_eq!(Code::parse(c.prefix()), Some(*c));
        assert_eq!(Code::parse(&format!("Failed to execute program: Guest panicked: {}: x", c.prefix())), Some(*c));
    }
    assert_eq!(Code::parse("no code here"), None);
}

#[test]
fn e01_header_not_finalized() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    // An anchor the instance never recorded.
    let anchor = Anchor::BlockHash([0x42; 32]);
    let m = eth_attest_core::assert_anchor(&ATTEST, &n.id, n.cfg.evm_chain_id, &anchor);
    let input = ReceiveInput { receiver: m.to, from: Some(Actor::new(BOB, PROBE)), is_authorized: false, pre_state: ActorState::empty(), message: m.message.clone() };
    assert_eq!(code(w.deliver(input.from, input.receiver, input.message.clone(), None)), Code::HeaderNotFinalized);
    assert_eq!(check::call(eth_attest::handle, &input).err(), Some(Code::HeaderNotFinalized));
    // An update whose finalized header is altered (its finality proof fails).
    let mut t = fx("sepolia", "finality_update.ssz");
    t[496 + 16 + 5] ^= 1;
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(t.clone()))), Code::HeaderNotFinalized);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &LcUpdate::Finality(t))), Code::HeaderNotFinalized);
}

#[test]
fn e02_signature_invalid() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let mut t = fx("sepolia", "finality_update.ssz");
    t[1344 + 60] ^= 1;
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(t.clone()))), Code::SignatureInvalid);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &LcUpdate::Finality(t.clone()))), Code::SignatureInvalid);
    // The public path: the same code, on chain and off.
    let fin = fx("sepolia", "finality_update.ssz");
    let keys = eth_attest::public::key_witnesses(&cur, &eth_attest::public::finality_bits(&fin).unwrap()).unwrap();
    let m = Message::Instruction(Instruction::UpdatePublic { config: n.cfg.clone(), prev: prev.clone(), committee: cur.clone(), keys: keys.clone(), update: t.clone() });
    assert_eq!(code(w.tx(BOB, ATTEST, &m)), Code::SignatureInvalid);
    assert_eq!(update_err(check::finality_public(&ATTEST, &n.cfg, &prev, &cur, &keys, &t)), Code::SignatureInvalid);
}

#[test]
fn e03_handoff_not_verified() {
    // A store in 1410 that does not know 1411's committee, given an update signed in 1411.
    let mut w = World::default();
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let fin = LcUpdate::Finality(fx("sepolia", "finality_update.ssz"));
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(code(submit(&mut w, BOB, &n, fin.clone())), Code::HandoffNotVerified);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &fin)), Code::HandoffNotVerified);
    // A period update whose next committee no longer matches its branch (a key byte flipped).
    let u = fx("sepolia", "update_period_1410.ssz");
    let g = GenericUpdate::from(&Update::<S>::from_ssz_bytes(&u).unwrap());
    let key = g.next_sync_committee.as_ref().unwrap().as_ssz_bytes();
    let pos = u.windows(48).position(|x| x == &key[..48]).unwrap();
    let mut t = u.clone();
    t[pos + 47] ^= 1;
    let m = Message::Instruction(Instruction::PeriodBegin { config: n.cfg.clone(), prev: prev.clone(), update: t.clone() });
    assert_eq!(code(w.tx(BOB, ATTEST, &m)), Code::HandoffNotVerified);
    assert_eq!(update_err(check::period_begin(&ATTEST, &n.cfg, &prev, &t)), Code::HandoffNotVerified);
}

#[test]
fn e04_checkpoint_stale_or_invalid() {
    // A bootstrap that is not the configured checkpoint's.
    let mut w = World::default();
    let cfg = config("sepolia", "bootstrap_same_period");
    let other = fx("sepolia", "bootstrap_prev_period.ssz");
    let r = w.tx(ALICE, ATTEST, &Message::Instruction(Instruction::Init { config: cfg.clone(), bootstrap: other.clone() }));
    assert_eq!(code(r), Code::CheckpointStale);
    assert_eq!(update_err(check::bootstrap(&ATTEST, &cfg, &other)), Code::CheckpointStale);
}

#[test]
fn e05_e06_inclusion_proof_and_predicate() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap();
    // A tampered storage proof node.
    let mut bad = sepolia_mapping();
    let last = bad.proof.nodes.len() - 1;
    bad.proof.nodes[last][5] ^= 1;
    assert_eq!(code(probe_storage(&mut w, ctx(&n), &bad, Predicate::Eq(word(1)))), Code::InclusionProofInvalid);
    let e = verify_storage_slot(&ctx(&n), &bad.witness, &bad.address, &bad.slot, Predicate::Eq(word(1)), &bad.proof).unwrap_err();
    assert_eq!(check::code_of_inclusion(&e), Code::InclusionProofInvalid);
    // The right proof, a predicate that does not hold.
    assert_eq!(code(probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(2)))), Code::PredicateNotSatisfied);
    let e = verify_storage_slot(&ctx(&n), &m.witness, &m.address, &m.slot, Predicate::Eq(word(2)), &m.proof).unwrap_err();
    assert_eq!(check::code_of_inclusion(&e), Code::PredicateNotSatisfied);
}

#[test]
fn e07_chain_id_mismatch() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    let wrong = StatementContext { evm_chain_id: 1, instance_id: n.id };
    assert_eq!(code(probe_storage(&mut w, wrong, &m, Predicate::Eq(word(1)))), Code::ChainIdMismatch);
    let q = eth_attest_core::assert_anchor(&ATTEST, &n.id, 1, &m.witness.anchor);
    let input = ReceiveInput { receiver: q.to, from: Some(Actor::new(CAROL, PROBE)), is_authorized: false, pre_state: ActorState::from(w.state(q.to.account_id, ATTEST)), message: q.message };
    assert_eq!(check::call(eth_attest::handle, &input).err(), Some(Code::ChainIdMismatch));
}

#[test]
fn e08_malformed_input() {
    let mut w = World::default();
    let to = Actor::new(BOB, ATTEST);
    assert_eq!(code(w.deliver(None, to, vec![0xFF, 0x01], Some(BOB))), Code::MalformedInput);
    let input = ReceiveInput { receiver: to, from: None, is_authorized: true, pre_state: ActorState::empty(), message: vec![0xFF, 0x01] };
    assert_eq!(check::call(eth_attest::handle, &input).err(), Some(Code::MalformedInput));
    let n = sepolia(&mut w);
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    let junk = LcUpdate::Finality(vec![1, 2, 3]);
    assert_eq!(code(submit(&mut w, BOB, &n, junk.clone())), Code::MalformedInput);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &junk)), Code::MalformedInput);
}

#[test]
fn e09_e10_stale_replayed_and_not_newer() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    // The same instance again.
    let again = Message::Instruction(Instruction::Init { config: n.cfg.clone(), bootstrap: fx("sepolia", "bootstrap_same_period.ssz") });
    assert_eq!(code(w.tx(CAROL, ATTEST, &again)), Code::StaleOrReplayed);
    // An update verified against a store that has since moved.
    let fin = fx("sepolia", "finality_update.ssz");
    let stale = update_msg(&w, &n, LcUpdate::Finality(fin.clone()));
    submit(&mut w, BOB, &n, LcUpdate::Finality(fin.clone())).unwrap();
    assert_eq!(code(w.tx(CAROL, ATTEST, &stale)), Code::StaleOrReplayed);
    // The same update on the moved store: valid, nothing new.
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(fin.clone()))), Code::NotNewer);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &LcUpdate::Finality(fin))), Code::NotNewer);
}

#[test]
fn e11_participation_below_policy() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let mut t = fx("sepolia", "finality_update.ssz");
    let mut cleared = 0;
    for i in 0..512 {
        if cleared < 200 && t[1280 + i / 8] >> (i % 8) & 1 == 1 {
            t[1280 + i / 8] &= !(1 << (i % 8));
            cleared += 1;
        }
    }
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(t.clone()))), Code::ParticipationBelowPolicy);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &LcUpdate::Finality(t))), Code::ParticipationBelowPolicy);
}

#[test]
fn e12_e13_not_permitted_and_witness_mismatch() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    // An effect from an account that is not this program.
    let prev = w.store(&n.id);
    let e = Message::Effect(eth_attest_core::Effect::Advance { prev: prev.clone(), next: prev.clone() });
    let to = Actor::new(store_address(&ATTEST, &n.id), ATTEST);
    assert_eq!(code(w.deliver(Some(Actor::new(BOB, PROBE)), to, borsh::to_vec(&e).unwrap(), None)), Code::NotPermitted);
    let input = ReceiveInput { receiver: to, from: Some(Actor::new(BOB, PROBE)), is_authorized: false, pre_state: ActorState::from(w.state(to.account_id, ATTEST)), message: borsh::to_vec(&e).unwrap() };
    assert_eq!(check::call(eth_attest::handle, &input).err(), Some(Code::NotPermitted));
    // A committee witness that is not the store's.
    let (mut cur, _) = n.committees.witness(&prev);
    cur[100] ^= 1;
    let fin = LcUpdate::Finality(fx("sepolia", "finality_update.ssz"));
    let m = Message::Instruction(Instruction::Update { config: n.cfg.clone(), prev: prev.clone(), current_committee: cur.clone(), next_committee: None, update: fin.clone() });
    assert_eq!(code(w.tx(BOB, ATTEST, &m)), Code::WitnessMismatch);
    assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &fin)), Code::WitnessMismatch);
}

// --- the weak-subjectivity rule (F10) ------------------------------------------------------------

fn with_signature_slot(update: &[u8], slot: u64) -> Vec<u8> {
    let mut t = update.to_vec();
    let n = t.len();
    t[n - 8..].copy_from_slice(&slot.to_le_bytes());
    t
}

#[test]
fn e04_an_instance_past_the_weak_subjectivity_bound_refuses_updates() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    assert_eq!(eth_attest_core::ws_epochs(n.cfg.evm_chain_id), 1_348, "Sepolia: Gloas's guide, lower value");
    assert_eq!(eth_attest_core::ws_epochs(1), 3_532, "mainnet: Electra's guide");
    let bound = prev.finalized.beacon.slot + 1_348 * 32;
    let fin = fx("sepolia", "finality_update.ssz");
    // Signed at the bound or later (a clock far ahead): refused with E04, before any other check,
    // on the private path, the public path and the dry run alike.
    for slot in [bound, bound + 1, bound + 1_000_000] {
        let t = with_signature_slot(&fin, slot);
        assert_eq!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(t.clone()))), Code::CheckpointStale, "slot {slot}");
        assert_eq!(update_err(check::update(&ATTEST, &n.cfg, &prev, &cur, None, &LcUpdate::Finality(t.clone()))), Code::CheckpointStale);
        let keys = eth_attest::public::key_witnesses(&cur, &eth_attest::public::finality_bits(&t).unwrap()).unwrap();
        assert_eq!(update_err(check::finality_public(&ATTEST, &n.cfg, &prev, &cur, &keys, &t)), Code::CheckpointStale);
        let m = Message::Instruction(Instruction::UpdatePublic { config: n.cfg.clone(), prev: prev.clone(), committee: cur.clone(), keys, update: t });
        assert_eq!(code(w.tx(BOB, ATTEST, &m)), Code::CheckpointStale);
    }
    // One slot under the bound: not E04 (here the update is then refused for its period).
    let t = with_signature_slot(&fin, bound - 1);
    assert_ne!(code(submit(&mut w, BOB, &n, LcUpdate::Finality(t))), Code::CheckpointStale);
    // And the real update applies.
    submit(&mut w, BOB, &n, LcUpdate::Finality(fin)).unwrap();
}

#[test]
fn the_clock_ends_every_update_s_window_at_the_store_s_deadline() {
    // The sequencer applies an update only while the block time is inside the response's window
    // (LEZ intersects every message's window into the transaction's and checks it per block):
    // its end is the store's weak-subjectivity deadline, so past it no update applies even when
    // its signature slot is old.
    let mut w = World::default();
    let n = sepolia(&mut w);
    let prev = w.store(&n.id);
    let (cur, _) = n.committees.witness(&prev);
    let deadline = eth_attest_core::ws_deadline_ms(&n.cfg, &prev).unwrap();
    assert_eq!(deadline, (n.cfg.genesis_time + (prev.finalized.beacon.slot + 1_348 * 32) * 12) * 1000);
    let fin = fx("sepolia", "finality_update.ssz");
    let root = |m: &Message| ReceiveInput { receiver: Actor::new(BOB, ATTEST), from: None, is_authorized: true, pre_state: ActorState::empty(), message: borsh::to_vec(m).unwrap() };
    let private = eth_attest::handle(&root(&update_msg(&w, &n, LcUpdate::Finality(fin.clone()))));
    assert_eq!(private.validity.timestamps.end(), Some(deadline));
    let keys = eth_attest::public::key_witnesses(&cur, &eth_attest::public::finality_bits(&fin).unwrap()).unwrap();
    let public = eth_attest::handle(&root(&Message::Instruction(Instruction::UpdatePublic { config: n.cfg.clone(), prev: prev.clone(), committee: cur, keys, update: fin })));
    assert_eq!(public.validity.timestamps.end(), Some(deadline));
    assert!(public.validity.timestamps.start() < public.validity.timestamps.end());
}
