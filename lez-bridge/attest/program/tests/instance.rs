//! Whole-transaction tests for the RFP-021 attestation program, on real Sepolia (Gloas) and
//! mainnet (Fulu) light-client data from the attest-spike fixtures. The simulator and the
//! fixtures are in `common/mod.rs`. The reference consumer `eth_attest_probe` stands for any
//! program that consumes an attestation: it runs the inclusion library in its own actor and
//! sends `AssertAnchor`.

mod common;

use common::*;

// ---------------------------------------------------------------------------------------------
// Instance creation and updates

#[test]
fn init_writes_config_store_and_checkpoint_anchor() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let m = meta("sepolia");
    // The config is stored under its own hash.
    let stored: InstanceConfig = borsh::from_slice(&w.state(config_address(&ATTEST, &n.id), ATTEST)).unwrap();
    assert_eq!(stored, n.cfg);
    let s = w.store(&n.id);
    assert_eq!(s.instance_id, n.id);
    assert_eq!(s.evm_chain_id, 11_155_111);
    assert_eq!(s.finalized.beacon.slot, m["bootstrap_same_period"]["slot"].as_u64().unwrap());
    assert_eq!(s.finalized.beacon_root, n.cfg.checkpoint, "the store starts at the checkpoint");
    assert_eq!((s.next_committee, s.updates), (None, 0));
    assert_eq!(s.finalized.execution.state_root, None, "Gloas headers carry only the block hash");
    // The checkpoint's execution block is an anchor from the start.
    let rec = w.anchor(&n.id, &Anchor::BlockHash(s.finalized.execution.block_hash)).expect("checkpoint anchor");
    assert_eq!((rec.beacon_slot, rec.evm_chain_id), (s.finalized.beacon.slot, 11_155_111));
}

#[test]
fn a_real_finality_update_advances_the_store_permissionlessly() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let genesis = w.store(&n.id);
    // Bob has no relation to the instance.
    submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
    let s = w.store(&n.id);
    let fu = &meta("sepolia")["finality_update"];
    assert_eq!(s.finalized.beacon.slot, fu["finalized_slot"].as_u64().unwrap());
    assert_eq!(s.finalized.execution.block_hash, h32(fu["finalized_execution_block_hash"].as_str().unwrap()));
    assert_eq!(s.current_committee, genesis.current_committee);
    assert_eq!(s.updates, 1);
    assert!(w.anchor(&n.id, &Anchor::BlockHash(s.finalized.execution.block_hash)).is_some());
    // The checkpoint's anchor stays valid forever.
    assert!(w.anchor(&n.id, &Anchor::BlockHash(genesis.finalized.execution.block_hash)).is_some());
    let advanced: Vec<eth_attest_core::Advanced> = w.events.iter().filter_map(|e| borsh::from_slice(&e.data).ok()).collect();
    assert_eq!(advanced.last().unwrap().beacon_slot, s.finalized.beacon.slot);
}

#[test]
fn committee_handoff_period_update_then_finality_signed_by_the_next_committee() {
    let mut w = World::default();
    let mut n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let start = w.store(&n.id);
    // Without the handoff, the next period's finality update is refused.
    err_contains(
        submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))),
        "invalid sync committee period",
    );
    // The period-1410 update proves the period-1411 committee.
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    submit(&mut w, BOB, &n, LcUpdate::Period(u1410.clone())).unwrap();
    n.committees.from_period_update(&u1410);
    let s = w.store(&n.id);
    assert_eq!(s.finalized, start.finalized, "this update finalizes nothing newer");
    let next = s.next_committee.expect("next committee learned");
    // Now the finality update signed in 1411 rotates the committee.
    submit(&mut w, CAROL, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
    let s = w.store(&n.id);
    assert_eq!(s.current_committee, next, "rotated to the handed-off committee");
    assert_eq!(s.next_committee, None, "a finality update carries no next committee");
    assert_eq!(s.finalized.beacon.slot, 11_563_424);
    // An older-period update that still proves something new (the 1412 committee) is accepted;
    // replaying it is not.
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    submit(&mut w, BOB, &n, LcUpdate::Period(u1411.clone())).unwrap();
    assert!(w.store(&n.id).next_committee.is_some());
    assert_eq!(w.store(&n.id).finalized.beacon.slot, 11_563_424);
    n.committees.from_period_update(&u1411);
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Period(u1411)), "update not relevant");
    assert_eq!(w.store(&n.id).updates, 3);
}

#[test]
fn tampered_updates_are_refused_and_change_nothing() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let before = w.clone();
    let fin = fx("sepolia", "finality_update.ssz");
    let other = fx("sepolia", "update_period_1411.ssz");
    assert_eq!((fin.len(), other.len()), (1448, 26424), "Gloas SSZ sizes");
    // A valid signature from another update.
    let mut t = fin.clone();
    t[1344..1440].copy_from_slice(&other[26424 - 104..26424 - 8]);
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Finality(t)), "invalid sync committee signature");
    // One signature byte flipped.
    let mut t = fin.clone();
    t[1344 + 60] ^= 1;
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Finality(t)), "invalid sync committee signature");
    // One participation bit cleared: wrong signer set.
    let mut t = fin.clone();
    let byte = (0..64).find(|i| t[1280 + i] != 0).unwrap();
    let bit = (0..8).find(|b| t[1280 + byte] >> b & 1 == 1).unwrap();
    t[1280 + byte] ^= 1 << bit;
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Finality(t)), "invalid sync committee signature");
    // Finalized header or its execution block hash altered.
    let mut t = fin.clone();
    t[496 + 16 + 5] ^= 1;
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Finality(t)), "invalid finality proof");
    let mut t = fin.clone();
    t[496 + 112 + 3] ^= 1;
    err_contains(submit(&mut w, BOB, &n, LcUpdate::Finality(t)), "invalid execution payload proof");
    assert_eq!(w.states, before.states, "refused updates leave every account as it was");
}

#[test]
fn the_wrong_committee_witness_is_refused() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let mut m = update_msg(&w, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz")));
    // The period-1412 committee (from the 1411 update) in place of the store's current one.
    let u = Update::<S>::from_ssz_bytes(&fx("sepolia", "update_period_1411.ssz")).unwrap();
    let wrong = GenericUpdate::from(&u).next_sync_committee.unwrap().as_ssz_bytes();
    if let Message::Instruction(Instruction::Update { current_committee, .. }) = &mut m {
        *current_committee = wrong.clone();
    }
    err_contains(w.tx(BOB, ATTEST, &m), "committee witness does not match the store (current)");
    // A next committee the store does not have.
    let mut m = update_msg(&w, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz")));
    if let Message::Instruction(Instruction::Update { next_committee, .. }) = &mut m {
        *next_committee = Some(wrong);
    }
    err_contains(w.tx(BOB, ATTEST, &m), "committee witness does not match the store (next)");
}

#[test]
fn a_wrong_checkpoint_or_bootstrap_is_refused() {
    let mut w = World::default();
    let mut cfg = config("sepolia", "bootstrap_same_period");
    cfg.checkpoint[0] ^= 1;
    err_contains(init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg).map(|_| ()), "invalid header hash");
    // The prev-period bootstrap against the same-period checkpoint.
    let cfg = config("sepolia", "bootstrap_same_period");
    err_contains(init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", cfg.clone()).map(|_| ()), "invalid header hash");
    // A committee key altered in the bootstrap (Gloas: header 496 bytes, then the committee).
    let mut boot = fx("sepolia", "bootstrap_same_period.ssz");
    boot[496 + 48 * 5 + 20] ^= 1;
    let m = Message::Instruction(Instruction::Init { config: cfg.clone(), bootstrap: boot });
    err_contains(w.tx(ALICE, ATTEST, &m), "invalid current sync committee proof");
    assert!(w.states.is_empty(), "nothing was created");
    // A policy below a 2/3 supermajority is not a valid instance.
    let mut low = cfg;
    low.min_participants = 341;
    err_contains(init(&mut w, ALICE, "sepolia", "bootstrap_same_period", low).map(|_| ()), "2/3 supermajority");
}

#[test]
fn replayed_older_and_stale_updates_are_refused() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let genesis = w.store(&n.id);
    let fin = fx("sepolia", "finality_update.ssz");
    // Verified against the genesis store, but submitted after another submitter advanced it.
    let stale = update_msg(&w, &n, LcUpdate::Finality(fin.clone()));
    submit(&mut w, BOB, &n, LcUpdate::Finality(fin.clone())).unwrap();
    let advanced = w.store(&n.id);
    err_contains(w.tx(CAROL, ATTEST, &stale), "the store changed since this update was verified");
    // The same update again, against the current store: verified, but not progress.
    err_contains(submit(&mut w, CAROL, &n, LcUpdate::Finality(fin)), "does not advance the store");
    // An update from an older period.
    err_contains(
        submit(&mut w, CAROL, &n, LcUpdate::Period(fx("sepolia", "update_period_1410.ssz"))),
        "invalid sync committee period",
    );
    assert_eq!(w.store(&n.id), advanced);
    assert_ne!(advanced, genesis);
}

#[test]
fn an_update_for_another_instance_or_chain_is_refused() {
    let mut w = World::default();
    let a = sepolia(&mut w);
    // A second Sepolia instance, same checkpoint, different salt.
    let mut cfg_b = a.cfg.clone();
    cfg_b.salt = [1; 32];
    let b = init(&mut w, BOB, "sepolia", "bootstrap_same_period", cfg_b).unwrap();
    assert_ne!(a.id, b.id);
    let mainnet = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    // B's config with A's store.
    let mut m = update_msg(&w, &a, LcUpdate::Finality(fx("sepolia", "finality_update.ssz")));
    if let Message::Instruction(Instruction::Update { config, .. }) = &mut m {
        *config = b.cfg.clone();
    }
    err_contains(w.tx(CAROL, ATTEST, &m), "the config is not this store's instance");
    // A mainnet update on the Sepolia instance and the other way round.
    assert!(submit(&mut w, CAROL, &a, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))).is_err());
    assert!(submit(&mut w, CAROL, &mainnet, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).is_err());
    assert_eq!(w.store(&a.id).updates + w.store(&b.id).updates + w.store(&mainnet.id).updates, 0);
    // Advancing A leaves B and mainnet exactly as they were.
    let (sb, sm) = (w.store(&b.id), w.store(&mainnet.id));
    submit(&mut w, CAROL, &a, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
    assert_eq!((w.store(&b.id), w.store(&mainnet.id)), (sb, sm));
    assert!(w.anchor(&b.id, &Anchor::BlockHash(w.store(&a.id).finalized.execution.block_hash)).is_none());
}

#[test]
fn participation_below_the_instance_policy_is_refused() {
    let mut w = World::default();
    let mut cfg = config("sepolia", "bootstrap_same_period");
    cfg.min_participants = 510; // the fixture has 509 signers
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg).unwrap();
    err_contains(
        submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))),
        "participation 509 is below the instance's policy of 510",
    );
}

#[test]
fn an_update_cannot_be_applied_before_its_signature_slot() {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let Message::Instruction(i) = update_msg(&w, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))) else {
        unreachable!()
    };
    let input = ReceiveInput {
        receiver: Actor::new(BOB, ATTEST),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: Vec::new(),
    };
    let r = eth_attest::instruct(&input, i);
    // genesis_time + signature_slot * 12 s, in ms.
    let start = (1_655_733_600u64 + 11_563_489 * 12) * 1000;
    assert!(r.validity.timestamps.is_valid_for(start));
    assert!(!r.validity.timestamps.is_valid_for(start - 1));
}

#[test]
fn effects_only_come_from_the_program_and_config_never_changes() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let cfg_bytes = w.state(config_address(&ATTEST, &n.id), ATTEST);
    let s = w.store(&n.id);
    // Creating the same instance again is refused, by anyone.
    for who in [ALICE, BOB] {
        err_contains(
            init(&mut w, who, "sepolia", "bootstrap_same_period", n.cfg.clone()).map(|_| ()),
            "this instance already exists",
        );
    }
    // Forged effects: from the transaction itself, and from another program.
    let mut fake = s.clone();
    fake.finalized.beacon.slot += 1_000;
    fake.finalized.execution.block_hash = [0xEE; 32];
    let forged = Message::Effect(Effect::Advance { prev: s.clone(), next: fake.clone() });
    err_contains(w.tx(store_address(&ATTEST, &n.id), ATTEST, &forged), "only this program may send an effect");
    err_contains(
        w.deliver(Some(Actor::new(CAROL, PROBE)), Actor::new(store_address(&ATTEST, &n.id), ATTEST), borsh::to_vec(&forged).unwrap(), None),
        "only this program may send an effect",
    );
    let create = Message::Effect(Effect::Create { config: n.cfg.clone(), genesis: s.clone() });
    err_contains(w.tx(ALICE, ATTEST, &create), "only this program may send an effect");
    // The store's own anchors can't be recorded by a submitter's actor either.
    let rec = AnchorRecord {
        instance_id: n.id,
        evm_chain_id: n.cfg.evm_chain_id,
        anchor: Anchor::BlockHash([0xEE; 32]),
        beacon_slot: 1,
        beacon_root: [0; 32],
        block_number: None,
    };
    err_contains(
        w.deliver(
            Some(Actor::new(CAROL, ATTEST)),
            Actor::new(anchor_address(&ATTEST, &n.id, &rec.anchor), ATTEST),
            borsh::to_vec(&Message::Effect(Effect::RecordAnchor(rec))).unwrap(),
            None,
        ),
        "only the instance's store records anchors",
    );
    // A new config (here: another salt) is another instance; the first is untouched.
    let mut other = n.cfg.clone();
    other.salt = [9; 32];
    init(&mut w, CAROL, "sepolia", "bootstrap_same_period", other).unwrap();
    assert_eq!(w.state(config_address(&ATTEST, &n.id), ATTEST), cfg_bytes);
    assert_eq!(w.store(&n.id), s);
}

#[test]
fn mainnet_fulu_instance_records_state_root_anchors() {
    let mut w = World::default();
    let n = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    let s0 = w.store(&n.id);
    assert!(s0.finalized.execution.state_root.is_some(), "Fulu headers carry the state root");
    submit(&mut w, BOB, &n, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))).unwrap();
    let s = w.store(&n.id);
    let fu = &meta("mainnet")["finality_update"];
    assert_eq!(s.finalized.beacon.slot, fu["finalized_slot"].as_u64().unwrap());
    assert_eq!(s.finalized.execution.block_hash, h32(fu["finalized_execution_block_hash"].as_str().unwrap()));
    let root = s.finalized.execution.state_root.unwrap();
    let rec = w.anchor(&n.id, &Anchor::StateRoot(root)).expect("state-root anchor");
    assert_eq!((rec.evm_chain_id, rec.block_number), (1, s.finalized.execution.block_number));
    assert!(w.anchor(&n.id, &Anchor::BlockHash(s.finalized.execution.block_hash)).is_some());
}

// ---------------------------------------------------------------------------------------------
// Consuming: statements inside the consumer, AssertAnchor in public

#[test]
fn the_real_sepolia_mapping_entry_is_attested() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap();
    // The statement id is deterministic and is what the consumer saw.
    let st = StorageStatement { context: ctx(&n), address: m.address, slot: m.slot, predicate: Predicate::Eq(word(1)), anchor: m.witness.anchor };
    assert_eq!(w.state(CAROL, PROBE), st.id().to_vec());
    // Inequality: the entry is not 0.
    probe_storage(&mut w, ctx(&n), &m, Predicate::Ne(word(0))).unwrap();
    // Distinct facts give distinct ids.
    let mut other = st;
    other.predicate = Predicate::Ne(word(0));
    assert_ne!(other.id(), st.id());
    other = st;
    other.context.evm_chain_id = 1;
    assert_ne!(other.id(), st.id());
    assert_eq!(other.fact_id() == st.fact_id(), false);
}

#[test]
fn a_statement_is_refused_for_a_wrong_value_account_or_anchor() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    // Wrong value: claimed 2 (the proof says 1), or a predicate that does not hold.
    let mut t = sepolia_mapping();
    t.proof.value = word(2);
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(2))), "storage proof invalid");
    err_contains(probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(2))), "the predicate does not hold");
    err_contains(probe_storage(&mut w, ctx(&n), &m, Predicate::Ne(word(1))), "the predicate does not hold");
    // Another slot (an absent entry claimed true).
    let mut t = sepolia_mapping();
    t.slot[31] ^= 1;
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "storage proof invalid");
    // Wrong account.
    let mut t = sepolia_mapping();
    t.address[0] ^= 1;
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "account proof invalid");
    // Wrong anchor: the header no longer hashes to it.
    let mut t = sepolia_mapping();
    t.witness.anchor = Anchor::BlockHash(w.store(&n.id).finalized.execution.block_hash.map(|b| b ^ 1));
    // The probe refuses with the class only (a program never prints InclusionError's Display).
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "anchor witness invalid");
    assert_eq!(verify_storage_slot(&ctx(&n), &t.witness, &t.address, &t.slot, Predicate::Eq(word(1)), &t.proof).unwrap_err(), InclusionError::HeaderHash);
    // A header altered (e.g. a tampered state root) no longer hashes to the anchor either.
    let mut t = sepolia_mapping();
    let h = t.witness.header_rlp.as_mut().unwrap();
    h[100] ^= 1;
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "anchor witness invalid");
    assert_eq!(verify_storage_slot(&ctx(&n), &t.witness, &t.address, &t.slot, Predicate::Eq(word(1)), &t.proof).unwrap_err(), InclusionError::HeaderHash);
    // A bare state root as anchor: the instance (Gloas) never finalized one.
    let state_root = h32(json("sepolia", "el_block_header.json")["stateRoot"].as_str().unwrap());
    let mut t = sepolia_mapping();
    t.witness = AnchorWitness { anchor: Anchor::StateRoot(state_root), header_rlp: None };
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "anchor not finalized by this instance");
    assert!(w.state(CAROL, PROBE).is_empty(), "no refused statement left a trace");
}

#[test]
fn an_anchor_the_instance_never_finalized_is_refused() {
    let mut w = World::default();
    // The instance at its checkpoint only: the fixture block is not finalized by it yet.
    let n = sepolia(&mut w);
    let m = sepolia_mapping();
    err_contains(probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))), "anchor not finalized by this instance");
    // Verification Security #1: a fabricated chain. A self-consistent header and state in which
    // the entry is true pass every local check, and are refused only because no instance
    // finalized that header.
    let fake = SynthState { accounts: vec![SynthAccount::new(m.address).with(m.slot, word(1))] };
    let header = header_rlp(fake.state_root(), [0; 32], 12_119_519, 0);
    let anchor = Anchor::BlockHash(alloy_primitives::keccak256(&header).0);
    let t = MappingFixture {
        witness: AnchorWitness { anchor, header_rlp: Some(header) },
        address: m.address,
        slot: m.slot,
        proof: fake.storage_proof(&m.address, &m.slot),
    };
    let local = verify_storage_slot(&ctx(&n), &t.witness, &t.address, &t.slot, Predicate::Eq(word(1)), &t.proof);
    assert!(local.is_ok(), "the fabricated proof is internally consistent");
    err_contains(probe_storage(&mut w, ctx(&n), &t, Predicate::Eq(word(1))), "anchor not finalized by this instance");
}

#[test]
fn a_consumer_bound_to_another_instance_or_chain_refuses_the_statement() {
    let mut w = World::default();
    let sep = sepolia_finalized(&mut w);
    let main = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    submit(&mut w, BOB, &main, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))).unwrap();
    let m = sepolia_mapping();
    probe_storage(&mut w, ctx(&sep), &m, Predicate::Eq(word(1))).unwrap();
    // Same contract address, same proof, consumer bound to the mainnet instance: refused.
    err_contains(probe_storage(&mut w, ctx(&main), &m, Predicate::Eq(word(1))), "anchor not finalized by this instance");
    // Bound to the Sepolia instance but claiming chain id 1: refused.
    let wrong_chain = StatementContext { evm_chain_id: 1, instance_id: sep.id };
    err_contains(probe_storage(&mut w, wrong_chain, &m, Predicate::Eq(word(1))), "anchor is for another chain");
    // A second Sepolia instance that never advanced: refused too.
    let mut cfg = sep.cfg.clone();
    cfg.salt = [2; 32];
    let sep2 = init(&mut w, CAROL, "sepolia", "bootstrap_same_period", cfg).unwrap();
    err_contains(probe_storage(&mut w, ctx(&sep2), &m, Predicate::Eq(word(1))), "anchor not finalized by this instance");
}

#[test]
fn account_fields_are_attested() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    let f = m.proof.account.fields;
    let account = |w: &mut World, field, predicate| {
        w.tx(
            CAROL,
            PROBE,
            &Probe::Account { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, field, predicate, proof: m.proof.account.clone() },
        )
    };
    account(&mut w, AccountField::StorageRoot, Predicate::Eq(f.storage_root)).unwrap();
    account(&mut w, AccountField::CodeHash, Predicate::Eq(f.code_hash)).unwrap();
    account(&mut w, AccountField::Nonce, Predicate::Eq(word(u128::from(f.nonce)))).unwrap();
    account(&mut w, AccountField::Balance, Predicate::Eq(f.balance)).unwrap();
    err_contains(account(&mut w, AccountField::Nonce, Predicate::Ne(word(u128::from(f.nonce)))), "the predicate does not hold");
    // Claimed fields that differ from the proven account.
    let mut bad = m.proof.account.clone();
    bad.fields.nonce += 1;
    err_contains(
        w.tx(CAROL, PROBE, &Probe::Account { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, field: AccountField::Nonce, predicate: Predicate::Eq(word(u128::from(bad.fields.nonce))), proof: bad }),
        "account proof invalid",
    );
}

#[test]
fn receipt_logs_are_attested_against_a_finalized_header() {
    // Synthetic receipts trie and header; the anchor record is written straight into the
    // simulator (test only) to stand for an instance that finalized this header.
    let mut w = World::default();
    let n = sepolia(&mut w);
    let log = |i: u8| Log { address: [i; 20], topics: vec![[i; 32], [i + 1; 32]], data: vec![i; 40] };
    let receipts = vec![receipt(true, &[log(1)]), receipt(true, &[log(2), log(3)]), receipt(false, &[]), receipt(true, &[])];
    let (root, proof) = receipts_trie(&receipts, 1);
    let header = header_rlp([0x77; 32], root, 99, 0);
    let anchor = Anchor::BlockHash(alloy_primitives::keccak256(&header).0);
    w.states.insert(
        (anchor_address(&ATTEST, &n.id, &anchor), ATTEST),
        borsh::to_vec(&AnchorRecord { instance_id: n.id, evm_chain_id: n.cfg.evm_chain_id, anchor, beacon_slot: 1, beacon_root: [0; 32], block_number: None }).unwrap(),
    );
    let witness = AnchorWitness { anchor, header_rlp: Some(header) };
    let probe_log = |w: &mut World, proof: ReceiptProof, log_index: u64, expect: Log| {
        w.tx(CAROL, PROBE, &Probe::Log { attest: ATTEST, ctx: ctx(&n), witness: witness.clone(), proof, log_index, expect })
    };
    probe_log(&mut w, proof.clone(), 1, log(3)).unwrap();
    err_contains(probe_log(&mut w, proof.clone(), 0, log(3)), "not the expected log");
    err_contains(probe_log(&mut w, proof.clone(), 2, log(3)), "receipt or log invalid");
    assert_eq!(verify_receipt_log(&ctx(&n), &witness, &proof, 2).unwrap_err(), InclusionError::NoSuchLog);
    // The receipt bytes of another index under this index's proof.
    let mut bad = proof.clone();
    bad.receipt = receipts[0].clone();
    err_contains(probe_log(&mut w, bad, 0, log(1)), "receipt proof invalid");
    // A failed transaction's receipt is refused.
    let (_, failed) = receipts_trie(&receipts, 2);
    err_contains(probe_log(&mut w, failed.clone(), 0, log(1)), "receipt or log invalid");
    assert_eq!(verify_receipt_log(&ctx(&n), &witness, &failed, 0).unwrap_err(), InclusionError::Receipt("the transaction failed"));
    // A state-root anchor has no receipts root.
    let ws = AnchorWitness { anchor: Anchor::StateRoot([0x77; 32]), header_rlp: None };
    let r = verify_receipt_log(&ctx(&n), &ws, &proof, 1);
    assert_eq!(r, Err(InclusionError::NoReceiptsRoot));
    assert_eq!(InclusionError::NoReceiptsRoot.to_string(), "anchor witness invalid: a state-root anchor has no receipts root");
}
