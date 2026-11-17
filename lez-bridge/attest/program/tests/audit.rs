//! Requirement-coverage tests for the RFP-021 attestation program and its inclusion library (audit
//! IDs B9, B10, B11 tie-in, B12, B13, B14, B16), on the real Sepolia and mainnet fixtures where the
//! requirement is about real data, and synthetic tries where it is about malformed input. The
//! simulator is `common/mod.rs` (as `instance.rs`).

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use alloy_primitives::{Bytes, keccak256};
use alloy_trie::{Nibbles, proof::ProofVerificationError};
use common::*;
use eth_attest_core::{
    Query,
    inclusion::{AnchorWitness as Witness, MAX_STATE_NODE_LEN, check_proof_nodes, header_roots, nibbles_of, receipt_logs, verify_account_proof},
};

/// Text a refusal must never carry: Rust runtime panics and library assertions, which say how the
/// code is written rather than why the input is wrong.
fn assert_clean(e: &str) {
    for marker in [
        "index out of bounds",
        "attempt to",
        "called `Option::unwrap()`",
        "called `Result::unwrap()`",
        "assertion failed",
        "Cannot extend",
        "capacity",
        "overflow",
        "range end index",
        "slice index",
    ] {
        assert!(!e.contains(marker), "an internal panic leaks through: {e}");
    }
}

/// Plants an instance whose config is `cfg` with the store a real bootstrap gave `real` (same
/// committees, same finalized header), re-keyed to `cfg`'s instance id. Test-only: it stands for
/// an instance created from a config that the bootstrap check does not look at.
fn plant_like(w: &mut World, real: &Net, cfg: InstanceConfig) -> Net {
    let id = instance_id(&ATTEST, &cfg);
    let mut store = w.store(&real.id);
    store.instance_id = id;
    w.states.insert((config_address(&ATTEST, &id), ATTEST), borsh::to_vec(&cfg).unwrap());
    w.states.insert((store_address(&ATTEST, &id), ATTEST), borsh::to_vec(&store).unwrap());
    Net { cfg, id, committees: real.committees.clone() }
}

// ---------------------------------------------------------------------------------------------
// B9 (RFP-021 Verification Security #2): the signing domain is the instance's own.

#[test]
fn an_instance_with_another_genesis_validators_root_refuses_the_real_update() {
    let mut w = World::default();
    let mut cfg = config("sepolia", "bootstrap_same_period");
    cfg.genesis_validators_root[0] ^= 1;
    // The bootstrap check does not involve the signing domain: the instance is created...
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg).unwrap();
    let before = w.states.clone();
    // ...but the real update, signed over Sepolia's domain, is refused by it.
    err_contains(
        submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))),
        "finality update rejected: invalid sync committee signature",
    );
    assert_eq!(w.states, before, "the store and every anchor are unchanged");
    assert_eq!(w.store(&n.id).updates, 0);
}

#[test]
fn an_instance_with_another_fork_version_refuses_the_real_update() {
    // The fork the update was signed in (Gloas, index 7) with another version.
    let mut w = World::default();
    let mut cfg = config("sepolia", "bootstrap_same_period");
    cfg.forks[7].version[3] ^= 1;
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg).unwrap();
    let before = w.states.clone();
    err_contains(
        submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))),
        "finality update rejected: invalid sync committee signature",
    );
    assert_eq!(w.states, before);
    // A version change in a fork long past (Altair) is irrelevant to this update's domain.
    let mut w = World::default();
    let mut cfg = config("sepolia", "bootstrap_same_period");
    cfg.forks[1].version[3] ^= 1;
    let n = init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg).unwrap();
    submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
}

#[test]
fn an_instance_with_another_fork_epoch_refuses_the_real_update() {
    // Gloas scheduled one epoch after the update's signature epoch: the signature slot falls in
    // Fulu under this config, so the domain uses Fulu's version.
    let sig_epoch = 11_563_489 / 32;
    let real_cfg = config("sepolia", "bootstrap_same_period");
    let mut cfg = real_cfg.clone();
    cfg.forks[7].epoch = sig_epoch + 1;
    let mut w = World::default();
    // The bootstrap (a Gloas header) is itself refused under this schedule: read as Fulu, its
    // execution branch is checked at the wrong generalized index...
    let init_err = init(&mut w, ALICE, "sepolia", "bootstrap_same_period", cfg.clone()).map(|_| ()).unwrap_err();
    assert_eq!(init_err, "RFP021-E04 checkpoint stale or invalid: bootstrap rejected: invalid execution payload proof");
    assert!(w.states.is_empty());
    // ...so stand the instance up with the real store under the altered config (test-only), and
    // submit the real update to it.
    let real = sepolia(&mut w);
    let n = plant_like(&mut w, &real, cfg);
    let before = w.states.clone();
    // Refused, by the same header-shape check before the signature is reached: on this Sepolia
    // fixture the attested header and the signature slot are in the same epoch, and moving Gloas
    // changes the header shape (the mainnet test below isolates the domain instead).
    let err = submit(&mut w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap_err();
    assert_eq!(err, "RFP021-E01 header not finalized: finality update rejected: invalid execution payload proof");
    assert_eq!(w.states, before);
    // The control: the same planting with the real config accepts the update.
    let mut w2 = World::default();
    let real2 = sepolia(&mut w2);
    let mut salted = real_cfg;
    salted.salt = [7; 32];
    let n2 = plant_like(&mut w2, &real2, salted);
    submit(&mut w2, BOB, &n2, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
}

#[test]
fn a_mainnet_instance_with_fulu_scheduled_later_refuses_the_real_update_by_its_signature() {
    // Mainnet: Fulu moved one epoch past the update's signature epoch. Electra and Fulu headers
    // have the same shape, so the bootstrap is accepted and only the signing domain differs.
    let sig_epoch = 15_639_330 / 32;
    let mut cfg = config("mainnet", "bootstrap_same_period");
    cfg.forks[6].epoch = sig_epoch + 1;
    let mut w = World::default();
    let n = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", cfg).unwrap();
    let before = w.states.clone();
    err_contains(
        submit(&mut w, BOB, &n, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))),
        "finality update rejected: invalid sync committee signature",
    );
    assert_eq!(w.states, before);
    // The control: the real schedule accepts it.
    let mut w = World::default();
    let n = init(&mut w, ALICE, "mainnet", "bootstrap_same_period", config("mainnet", "bootstrap_same_period")).unwrap();
    submit(&mut w, BOB, &n, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))).unwrap();
}

// ---------------------------------------------------------------------------------------------
// B10: a tampered receipts root or log proof is refused with the exact error.

struct LogFixture {
    w: World,
    n: Net,
    receipts: Vec<Vec<u8>>,
    root: [u8; 32],
    proof: ReceiptProof,
}

fn sample_log(i: u8) -> Log {
    Log { address: [i; 20], topics: vec![[i; 32], [i + 1; 32]], data: vec![i; 40] }
}

fn log_fixture() -> LogFixture {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let receipts = vec![
        receipt(true, &[sample_log(1)]),
        receipt(true, &[sample_log(2), sample_log(3)]),
        receipt(false, &[]),
        receipt(true, &[]),
    ];
    let (root, proof) = receipts_trie(&receipts, 1);
    LogFixture { w, n, receipts, root, proof }
}

impl LogFixture {
    /// A header with `receipts_root`, finalized (planted) by the instance; its witness.
    fn finalized_header(&mut self, receipts_root: [u8; 32]) -> Witness {
        let header = header_rlp([0x77; 32], receipts_root, 99, 0);
        let anchor = Anchor::BlockHash(keccak256(&header).0);
        let rec = AnchorRecord { instance_id: self.n.id, evm_chain_id: self.n.cfg.evm_chain_id, anchor, beacon_slot: 1, beacon_root: [0; 32], block_number: None };
        self.w.states.insert((anchor_address(&ATTEST, &self.n.id, &anchor), ATTEST), borsh::to_vec(&rec).unwrap());
        Witness { anchor, header_rlp: Some(header) }
    }

    fn probe(&mut self, witness: &Witness, proof: ReceiptProof, log_index: u64, expect: Log) -> Result<(), String> {
        let m = Probe::Log { attest: ATTEST, ctx: ctx(&self.n), witness: witness.clone(), proof, log_index, expect };
        self.w.tx(CAROL, PROBE, &m)
    }
}

fn mismatch(path: Nibbles, got: &[u8], expected: &[u8]) -> String {
    ProofVerificationError::ValueMismatch { path, got: Some(Bytes::copy_from_slice(got)), expected: Some(Bytes::copy_from_slice(expected)) }
        .to_string()
}

/// `rlp(hash)`, how a parent refers to a hashed child (and how the root is matched).
fn word_rlp(h: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![0xa0];
    v.extend_from_slice(h);
    v
}

#[test]
fn a_tampered_receipts_root_is_refused_with_the_exact_error() {
    let mut f = log_fixture();
    let mut tampered = f.root;
    tampered[0] ^= 1;
    let witness = f.finalized_header(tampered);
    // The proof's first node hashes to the real root, not to the header's.
    let want = InclusionError::ReceiptProof(mismatch(Nibbles::new(), &f.proof.nodes[0], &word_rlp(&tampered)));
    assert_eq!(verify_receipt_log(&ctx(&f.n), &witness, &f.proof, 1), Err(want.clone()));
    let before = f.w.states.clone();
    // The consumer refuses with the class only: the exact error (with its trie path and node
    // bytes) is for native callers, never a program's panic.
    assert_eq!(f.probe(&witness, f.proof.clone(), 1, sample_log(3)), Err(want.class().refusal().to_owned()));
    assert_eq!(f.w.states, before);
    // The control: the untampered header verifies.
    let good = f.finalized_header(f.root);
    f.probe(&good, f.proof.clone(), 1, sample_log(3)).unwrap();
}

#[test]
fn a_tampered_log_proof_is_refused_with_the_exact_error() {
    let mut f = log_fixture();
    let witness = f.finalized_header(f.root);
    let key = Nibbles::unpack(alloy_rlp::encode(1u64));
    // A receipt with one log byte changed: the trie holds the real one at that key.
    let mut bad = f.proof.clone();
    let at = bad.receipt.len() - 3;
    bad.receipt[at] ^= 1;
    let want = InclusionError::ReceiptProof(mismatch(key, &f.receipts[1], &bad.receipt));
    assert_eq!(verify_receipt_log(&ctx(&f.n), &witness, &bad, 1), Err(want.clone()));
    assert_eq!(f.probe(&witness, bad, 1, sample_log(3)), Err(want.class().refusal().to_owned()));
    // A proof node with one byte changed: the root no longer matches it.
    let mut bad = f.proof.clone();
    bad.nodes[0][5] ^= 1;
    let want = InclusionError::ReceiptProof(mismatch(Nibbles::new(), &bad.nodes[0], &word_rlp(&f.root)));
    assert_eq!(verify_receipt_log(&ctx(&f.n), &witness, &bad, 1), Err(want.clone()));
    assert_eq!(f.probe(&witness, bad, 1, sample_log(3)), Err(want.class().refusal().to_owned()));
    // Another index's proof presented for index 1 (the existing case), now exactly.
    let mut bad = f.proof.clone();
    bad.receipt = f.receipts[0].clone();
    let want = InclusionError::ReceiptProof(mismatch(key, &f.receipts[1], &f.receipts[0]));
    assert_eq!(verify_receipt_log(&ctx(&f.n), &witness, &bad, 1), Err(want));
    // A state-root anchor: no receipts root at all.
    let ws = Witness { anchor: Anchor::StateRoot(f.root), header_rlp: None };
    assert_eq!(verify_receipt_log(&ctx(&f.n), &ws, &f.proof, 1), Err(InclusionError::NoReceiptsRoot));
}

// ---------------------------------------------------------------------------------------------
// B11 tie-in: the pinned statement id (attest/core/tests/statement_ids.rs) is what the real
// fixture statement produces through the program.

#[test]
fn the_real_fixture_statement_has_the_pinned_id() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    assert_eq!(n.id, h32("778b1890283b84f27e52842b0efad701f52aba50950824bfa39beae07c8a6347"));
    let m = sepolia_mapping();
    probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap();
    assert_eq!(w.state(CAROL, PROBE), h32("fe276c38ec744a62b452647483e326c4696de7179d949b687c16edd08f04c5b2").to_vec());
}

// ---------------------------------------------------------------------------------------------
// B12 (RFP-021 R#1): determinism.

/// Runs `message` from `signer` on two clones of `w`: the results, every post-state, every handler
/// response (borsh bytes) and every event are byte-for-byte equal. Returns the first clone.
fn replay<M: BorshSerialize>(w: &World, signer: AccountId, program: AccountId, message: &M) -> (World, Result<(), String>) {
    let fresh = || {
        let mut c = w.clone();
        c.responses.clear();
        c.events.clear();
        c
    };
    let (mut a, mut b) = (fresh(), fresh());
    let ra = a.tx(signer, program, message);
    let rb = b.tx(signer, program, message);
    assert_eq!(ra, rb, "same result");
    assert_eq!(a.states, b.states, "same post-states");
    assert_eq!(a.responses, b.responses, "same responses, byte for byte");
    assert_eq!(a.events, b.events, "same events");
    if ra.is_ok() {
        assert!(!a.responses.is_empty());
    }
    (a, ra)
}

#[test]
fn every_update_and_statement_replays_identically() {
    let w = World::default();
    // Init (bootstrap), Sepolia handoff sequence: period 1410, finality (rotation), period 1411.
    let cfg = config("sepolia", "bootstrap_prev_period");
    let boot = fx("sepolia", "bootstrap_prev_period.ssz");
    let (mut w, r) = replay(&w, ALICE, ATTEST, &Message::Instruction(Instruction::Init { config: cfg.clone(), bootstrap: boot.clone() }));
    r.unwrap();
    let mut n = Net { id: instance_id(&ATTEST, &cfg), cfg, committees: Committees::default() };
    n.committees.from_bootstrap(&boot);
    for (period, file) in [(true, "update_period_1410.ssz"), (false, "finality_update.ssz"), (true, "update_period_1411.ssz")] {
        let u = fx("sepolia", file);
        let m = update_msg(&w, &n, if period { LcUpdate::Period(u.clone()) } else { LcUpdate::Finality(u.clone()) });
        let (next, r) = replay(&w, BOB, ATTEST, &m);
        r.unwrap_or_else(|e| panic!("{file}: {e}"));
        w = next;
        if period {
            n.committees.from_period_update(&u);
        }
    }
    assert_eq!(w.store(&n.id).updates, 3);
    // A refused update is refused the same way every time.
    let (_, r) = replay(&w, BOB, ATTEST, &update_msg(&w, &n, LcUpdate::Period(fx("sepolia", "update_period_1411.ssz"))));
    assert!(r.unwrap_err().contains("update not relevant"));
    // Mainnet (Fulu, state-root anchors).
    let mcfg = config("mainnet", "bootstrap_same_period");
    let mboot = fx("mainnet", "bootstrap_same_period.ssz");
    let (mut w, r) = replay(&w, CAROL, ATTEST, &Message::Instruction(Instruction::Init { config: mcfg.clone(), bootstrap: mboot.clone() }));
    r.unwrap();
    let mut mn = Net { id: instance_id(&ATTEST, &mcfg), cfg: mcfg, committees: Committees::default() };
    mn.committees.from_bootstrap(&mboot);
    let (next, r) = replay(&w, BOB, ATTEST, &update_msg(&w, &mn, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))));
    r.unwrap();
    w = next;
    // Statements: storage, account field, log (the last against a planted finalized header).
    let m = sepolia_mapping();
    let storage = Probe::Storage { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, slot: m.slot, predicate: Predicate::Eq(word(1)), proof: m.proof.clone() };
    let (w2, r) = replay(&w, CAROL, PROBE, &storage);
    r.unwrap();
    assert_eq!(w2.state(CAROL, PROBE).len(), 32);
    let f = m.proof.account.fields;
    let account = Probe::Account { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, field: AccountField::StorageRoot, predicate: Predicate::Eq(f.storage_root), proof: m.proof.account.clone() };
    replay(&w, CAROL, PROBE, &account).1.unwrap();
    let mut lf = log_fixture();
    let witness = lf.finalized_header(lf.root);
    let log = Probe::Log { attest: ATTEST, ctx: ctx(&lf.n), witness, proof: lf.proof.clone(), log_index: 1, expect: sample_log(3) };
    replay(&lf.w, CAROL, PROBE, &log).1.unwrap();
    // A handler called twice on the same input gives the same response (no hidden state).
    let input = ReceiveInput {
        receiver: Actor::new(CAROL, PROBE),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: borsh::to_vec(&storage).unwrap(),
    };
    assert_eq!(probe(&input), probe(&input));
}

// ---------------------------------------------------------------------------------------------
// B13 (RFP-021 R#3): valid through rotation.

#[test]
fn early_anchors_stay_valid_through_a_committee_rotation() {
    let mut w = World::default();
    let mut n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let checkpoint = Anchor::BlockHash(w.store(&n.id).finalized.execution.block_hash);
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    submit(&mut w, BOB, &n, LcUpdate::Period(u1410.clone())).unwrap();
    n.committees.from_period_update(&u1410);
    submit(&mut w, CAROL, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
    // The first anchor an update finalized: the block the real mapping proof is for.
    let m = sepolia_mapping();
    let first_update_anchor = m.witness.anchor;
    assert!(w.anchor(&n.id, &first_update_anchor).is_some());
    let rotated = w.store(&n.id).current_committee;
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    submit(&mut w, BOB, &n, LcUpdate::Period(u1411)).unwrap();
    let s = w.store(&n.id);
    assert_eq!((s.current_committee, s.updates), (rotated, 3));
    assert!(s.next_committee.is_some(), "the store has moved on to the next period's committee");
    let attest_before = w.attest_states();
    // The real statement against that anchor still verifies after the rotation and the later
    // period update, and AssertAnchor accepts it.
    probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap();
    // The earliest anchor of all, the checkpoint's (no inclusion proof exists for that block in
    // the fixtures, so only its AssertAnchor is checked), is still asserted.
    let q = Message::Query(Query::AssertAnchor { instance_id: n.id, evm_chain_id: 11_155_111, anchor: checkpoint });
    w.deliver(Some(Actor::new(CAROL, PROBE)), Actor::new(anchor_address(&ATTEST, &n.id, &checkpoint), ATTEST), borsh::to_vec(&q).unwrap(), None)
        .unwrap();
    assert_eq!(w.attest_states(), attest_before, "asserting changes nothing");
}

// ---------------------------------------------------------------------------------------------
// B14 (RFP-021 R#11): independent consumers.

#[test]
fn two_consumers_asserting_one_anchor_are_independent_of_order() {
    let mut w0 = World::default();
    let n = sepolia_finalized(&mut w0);
    let m = sepolia_mapping();
    let attest0 = w0.attest_states();
    let carol = Probe::Storage { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, slot: m.slot, predicate: Predicate::Eq(word(1)), proof: m.proof.clone() };
    let f = m.proof.account.fields;
    // Another consumer program, another statement under the same anchor.
    let alice = Probe::Account { attest: ATTEST, ctx: ctx(&n), witness: m.witness.clone(), address: m.address, field: AccountField::CodeHash, predicate: Predicate::Eq(f.code_hash), proof: m.proof.account.clone() };
    // One simulated block, both orders.
    let mut ab = w0.clone();
    ab.tx(CAROL, PROBE, &carol).unwrap();
    ab.tx(ALICE, PROBE2, &alice).unwrap();
    let mut ba = w0.clone();
    ba.tx(ALICE, PROBE2, &alice).unwrap();
    ba.tx(CAROL, PROBE, &carol).unwrap();
    assert_eq!(ab.states, ba.states, "the order of consumers does not matter");
    assert_eq!(ab.attest_states(), attest0, "neither consumer changed the instance");
    assert_eq!(ba.attest_states(), attest0);
    // Each consumer holds exactly its own result; the instance holds nothing about either.
    assert_eq!(ab.state(CAROL, PROBE).len(), 32);
    assert_eq!(ab.state(ALICE, PROBE2).len(), 32);
    assert_ne!(ab.state(CAROL, PROBE), ab.state(ALICE, PROBE2));
    // The same consumer statement twice, or by a third party, changes nothing either.
    ab.tx(BOB, PROBE, &carol).unwrap();
    assert_eq!(ab.attest_states(), attest0);
}

// ---------------------------------------------------------------------------------------------
// B16: malformed and oversized inputs are refused with clean errors.

#[test]
fn truncated_or_oversized_ssz_updates_are_refused_cleanly() {
    let mut w = World::default();
    let mut n = init(&mut w, ALICE, "sepolia", "bootstrap_prev_period", config("sepolia", "bootstrap_prev_period")).unwrap();
    let fin = fx("sepolia", "finality_update.ssz");
    let per = fx("sepolia", "update_period_1410.ssz");
    let variants = |b: &[u8]| -> Vec<(String, Vec<u8>)> {
        let mut doubled = b.to_vec();
        doubled.extend_from_slice(b);
        let mut big = b.to_vec();
        big.extend(std::iter::repeat_n(0xFF, 1 << 20));
        vec![
            ("empty".into(), Vec::new()),
            ("one byte".into(), b[..1].to_vec()),
            ("half".into(), b[..b.len() / 2].to_vec()),
            ("one byte short".into(), b[..b.len() - 1].to_vec()),
            ("one byte long".into(), [b, &[0][..]].concat()),
            ("doubled".into(), doubled),
            ("1 MiB long".into(), big),
        ]
    };
    let before = w.states.clone();
    for (what, u) in variants(&fin) {
        let err = submit(&mut w, BOB, &n, LcUpdate::Finality(u)).unwrap_err();
        assert_clean(&err);
        assert_eq!(err, "RFP021-E08 malformed input: finality update does not decode: NoMatchingVariant", "finality {what}");
    }
    for (what, u) in variants(&per) {
        let err = submit(&mut w, BOB, &n, LcUpdate::Period(u)).unwrap_err();
        assert_clean(&err);
        assert_eq!(err, "RFP021-E08 malformed input: update does not decode: NoMatchingVariant", "period {what}");
    }
    // A period update presented as a finality update and the other way round.
    let err = submit(&mut w, BOB, &n, LcUpdate::Finality(per.clone())).unwrap_err();
    assert_clean(&err);
    let err = submit(&mut w, BOB, &n, LcUpdate::Period(fin.clone())).unwrap_err();
    assert_clean(&err);
    // Truncated or oversized committee witnesses.
    for cut in [0, 48, 24_623, 24_625] {
        let mut m = update_msg(&w, &n, LcUpdate::Period(per.clone()));
        if let Message::Instruction(Instruction::Update { current_committee, .. }) = &mut m {
            current_committee.resize(cut, 0);
        }
        let err = w.tx(BOB, ATTEST, &m).unwrap_err();
        assert_eq!(err, "RFP021-E13 witness mismatch: committee witness does not match the store (current)");
    }
    // Truncated or oversized bootstraps.
    let boot = fx("sepolia", "bootstrap_prev_period.ssz");
    let mut cfg2 = n.cfg.clone();
    cfg2.salt = [3; 32];
    for (what, b) in variants(&boot) {
        let m = Message::Instruction(Instruction::Init { config: cfg2.clone(), bootstrap: b });
        let err = w.tx(CAROL, ATTEST, &m).unwrap_err();
        assert_clean(&err);
        assert_eq!(err, "RFP021-E08 malformed input: bootstrap does not decode: NoMatchingVariant", "bootstrap {what}");
    }
    assert_eq!(w.states, before, "nothing was created or advanced");
    // The intact update still goes through.
    submit(&mut w, BOB, &n, LcUpdate::Period(per.clone())).unwrap();
    n.committees.from_period_update(&per);
}

/// A real-shaped header RLP and its broken variants; each must give `HeaderRlp`.
#[test]
fn malformed_header_rlp_is_refused_cleanly() {
    let good = header_rlp([0x77; 32], [0x88; 32], 99, 0);
    assert!(header_roots(&good).is_ok());
    let list = |items: &[Vec<u8>]| {
        let payload: Vec<u8> = items.concat();
        let mut out = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut out);
        out.extend(payload);
        out
    };
    let mut huge_len = vec![0xfb, 0xff, 0xff, 0xff, 0xff];
    huge_len.extend_from_slice(&good[3..]);
    let four_fields = list(&vec![alloy_rlp::encode([1u8; 32]); 4]);
    let wrong_root_type = list(&[
        alloy_rlp::encode([1u8; 32]),
        alloy_rlp::encode([1u8; 32]),
        alloy_rlp::encode([1u8; 20]),
        alloy_rlp::encode(7u64), // a number where the state root goes
        alloy_rlp::encode([1u8; 32]),
        alloy_rlp::encode([1u8; 32]),
    ]);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("one byte", good[..1].to_vec()),
        ("truncated", good[..good.len() - 1].to_vec()),
        ("one byte long", [&good[..], &[0x80]].concat()),
        ("doubled", [&good[..], &good[..]].concat()),
        ("a string, not a list", alloy_rlp::encode(good.as_slice())),
        ("huge declared length", huge_len),
        ("four fields", four_fields),
        ("number as state root", wrong_root_type),
        ("1 MiB of 0xff", vec![0xff; 1 << 20]),
    ];
    for (what, rlp) in cases {
        let r = catch_unwind(AssertUnwindSafe(|| header_roots(&rlp)));
        let r = r.unwrap_or_else(|e| panic!("{what}: header_roots panicked: {}", panic_text(e)));
        assert!(matches!(r, Err(InclusionError::HeaderRlp(_))), "{what}: {r:?}");
        // Through a witness whose anchor is the bad header's hash (so the hash check passes).
        let witness = Witness { anchor: Anchor::BlockHash(keccak256(&rlp).0), header_rlp: Some(rlp.clone()) };
        assert_eq!(witness.roots().map(|_| ()), r.clone().map(|_| ()), "{what}");
        assert_clean(&r.unwrap_err().to_string());
    }
    // The same refusal reaches a consumer as a clean error (before AssertAnchor runs).
    let mut w = World::default();
    let n = sepolia(&mut w);
    let mut m = sepolia_mapping();
    let bad = good[..good.len() - 1].to_vec();
    m.witness = Witness { anchor: Anchor::BlockHash(keccak256(&bad).0), header_rlp: Some(bad) };
    let err = probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap_err();
    // The consumer refuses with the class only, never InclusionError's Display.
    assert_eq!(err, "RFP021-E05 inclusion proof invalid: anchor witness invalid");
    assert_clean(&err);
}

#[test]
fn truncated_or_oversized_proof_node_lists_are_refused_cleanly() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    let c = ctx(&n);
    let run = |p: &StorageProof| {
        catch_unwind(AssertUnwindSafe(|| verify_storage_slot(&c, &m.witness, &m.address, &m.slot, Predicate::Eq(word(1)), p)))
            .unwrap_or_else(|e| panic!("verify_storage_slot panicked: {}", panic_text(e)))
    };
    assert!(run(&m.proof).is_ok());
    type Nodes = Vec<Vec<u8>>;
    let edits: Vec<(&str, Box<dyn Fn(&mut Nodes)>)> = vec![
        ("empty list", Box::new(|n: &mut Nodes| n.clear())),
        ("last node dropped", Box::new(|n: &mut Nodes| {
            n.pop();
        })),
        ("first node dropped", Box::new(|n: &mut Nodes| {
            n.remove(0);
        })),
        ("garbage node appended", Box::new(|n: &mut Nodes| n.push(vec![0xde, 0xad, 0xbe, 0xef]))),
        ("last node one byte short", Box::new(|n: &mut Nodes| {
            n.last_mut().unwrap().pop();
        })),
        ("last node one byte long", Box::new(|n: &mut Nodes| n.last_mut().unwrap().push(0))),
        ("an empty node", Box::new(|n: &mut Nodes| n.insert(1, Vec::new()))),
        ("a 1 MiB node", Box::new(|n: &mut Nodes| n.insert(1, vec![0xc0; 1 << 20]))),
        ("10,000 copies of the root node", Box::new(|n: &mut Nodes| {
            let r = n[0].clone();
            n.splice(0..0, std::iter::repeat_n(r, 10_000));
        })),
    ];
    for (what, edit) in &edits {
        let mut p = m.proof.clone();
        edit(&mut p.account.nodes);
        let r = run(&p);
        assert!(matches!(r, Err(InclusionError::AccountProof(_))), "account nodes, {what}: {r:?}");
        let mut p = m.proof.clone();
        edit(&mut p.nodes);
        let r = run(&p);
        assert!(matches!(r, Err(InclusionError::StorageProof(_))), "storage nodes, {what}: {r:?}");
    }
    // Receipt proofs.
    let mut lf = log_fixture();
    let witness = lf.finalized_header(lf.root);
    for (what, edit) in &edits {
        let mut p = lf.proof.clone();
        edit(&mut p.nodes);
        let r = catch_unwind(AssertUnwindSafe(|| verify_receipt_log(&ctx(&lf.n), &witness, &p, 1)))
            .unwrap_or_else(|e| panic!("receipt nodes, {what}: panicked: {}", panic_text(e)));
        assert!(matches!(r, Err(InclusionError::ReceiptProof(_))), "receipt nodes, {what}: {r:?}");
    }
    // Malformed receipt bytes, straight to the receipt decoder.
    for (what, bytes) in [
        ("empty", Vec::new()),
        ("type byte only", vec![0x02]),
        ("truncated", lf.receipts[1][..lf.receipts[1].len() - 1].to_vec()),
        ("one byte long", [&lf.receipts[1][..], &[0]].concat()),
        ("garbage", vec![0x02, 0xff, 0xff]),
    ] {
        let r = catch_unwind(AssertUnwindSafe(|| receipt_logs(&bytes))).unwrap_or_else(|e| panic!("receipt {what}: {}", panic_text(e)));
        assert!(matches!(r, Err(InclusionError::Receipt(_))), "receipt {what}: {r:?}");
    }
}

/// A proof node whose path runs past 64 nibbles: an extension with a full 64-nibble key whose
/// child is a leaf with a non-empty key. Every node hashes correctly to the next, so the proof
/// reaches path handling inside `alloy-trie`.
fn overlong_path_proof() -> ([u8; 32], Vec<Vec<u8>>) {
    let rlp_list = |items: &[Vec<u8>]| {
        let payload: Vec<u8> = items.concat();
        let mut out = Vec::new();
        alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut out);
        out.extend(payload);
        out
    };
    let bytes = |b: &[u8]| alloy_rlp::encode(Bytes::copy_from_slice(b));
    // Leaf: even-length key flag 0x20, one key byte (two nibbles), a 40-byte value.
    let leaf = rlp_list(&[bytes(&[0x20, 0x12]), bytes(&[0x01; 40])]);
    // Extension: even-length key flag 0x00, 32 key bytes (64 nibbles), child = rlp(keccak(leaf)).
    let mut ext_key = vec![0x00];
    ext_key.extend_from_slice(&[0xAB; 32]);
    let ext = rlp_list(&[bytes(&ext_key), word_rlp(&keccak256(&leaf).0)]);
    (keccak256(&ext).0, vec![ext, leaf])
}

#[test]
fn an_overlong_trie_path_is_refused_with_a_clean_error_where_it_is_checked() {
    // The decoder's own bound (a single node key over 64 nibbles) gives a clean RLP error.
    let mut key = vec![0x20];
    key.extend_from_slice(&[0xAB; 33]);
    let payload = [alloy_rlp::encode(Bytes::from(key)), alloy_rlp::encode(Bytes::from(vec![1u8; 40]))].concat();
    let mut node = Vec::new();
    alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut node);
    node.extend(payload);
    let root = keccak256(&node).0;
    let proof = AccountProof { fields: sepolia_mapping().proof.account.fields, nodes: vec![node] };
    let r = catch_unwind(AssertUnwindSafe(|| verify_account_proof(&root, &[0x11; 20], &proof))).expect("no panic");
    assert!(matches!(&r, Err(InclusionError::AccountProof(e)) if e.contains("trie node key too long")), "{r:?}");
}

/// B16-NYB: every node hashes correctly to the next, and alloy-trie's `verify_proof` used to
/// panic inside nybbles ("Cannot extend: resulting length would exceed maximum capacity").
/// The inclusion library now checks the nodes first and refuses the proof cleanly.
#[test]
fn a_proof_path_past_64_nibbles_is_refused_without_a_panic() {
    let (root, nodes) = overlong_path_proof();
    let proof = AccountProof { fields: sepolia_mapping().proof.account.fields, nodes };
    let r = catch_unwind(AssertUnwindSafe(|| verify_account_proof(&root, &[0x11; 20], &proof)));
    let r = r.unwrap_or_else(|e| panic!("verify_account_proof panicked: {}", panic_text(e)));
    assert_eq!(r, Err(InclusionError::AccountProof("proof path longer than 64 nibbles".into())));
    // Through a consumer: the refusal text must be a clean, payload-free class, not the library panic.
    let mut w = World::default();
    let n = sepolia(&mut w);
    let mut m = sepolia_mapping();
    m.witness = Witness { anchor: Anchor::StateRoot(root), header_rlp: None };
    m.proof.account = proof;
    let err = probe_storage(&mut w, ctx(&n), &m, Predicate::Eq(word(1))).unwrap_err();
    assert_eq!(err, "RFP021-E05 inclusion proof invalid: account proof invalid");
    assert_clean(&err);
    // The control: the same nodes fed to alloy-trie directly still panic, so the check above is
    // what stops it.
    let raw = catch_unwind(AssertUnwindSafe(|| {
        alloy_trie::proof::verify_proof(
            root.into(),
            Nibbles::unpack(keccak256([0x11u8; 20])),
            None,
            &m.proof.account.nodes.iter().map(|n| Bytes::copy_from_slice(n)).collect::<Vec<_>>(),
        )
    }));
    assert!(panic_text(raw.expect_err("alloy-trie alone panics")).contains("Cannot extend"));
}

// ---------------------------------------------------------------------------------------------
// B16-NYB fuzz: no proof, however malformed, panics anywhere in the inclusion library.

/// A panic payload's text, by reference.
fn payload_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default()
}

/// splitmix64: a seeded, dependency-free generator, so every run sees the same cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }

    fn word(&mut self) -> [u8; 32] {
        self.bytes(32).try_into().unwrap()
    }
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.concat();
    let mut out = Vec::new();
    alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut out);
    out.extend(payload);
    out
}

fn rlp_bytes(b: &[u8]) -> Vec<u8> {
    alloy_rlp::encode(Bytes::copy_from_slice(b))
}

/// Hex-prefix encoding of a node key (leaf or extension).
fn hex_prefix(nibbles: &[u8], leaf: bool) -> Vec<u8> {
    let flag = if leaf { 2 } else { 0 };
    let (mut out, rest) = if nibbles.len() % 2 == 1 { (vec![((flag + 1) << 4) | nibbles[0]], &nibbles[1..]) } else { (vec![flag << 4], nibbles) };
    out.extend(rest.chunks(2).map(|c| (c[0] << 4) | c[1]));
    out
}

/// One random mutation of a proof's node list.
fn mutate(rng: &mut Rng, nodes: &mut Vec<Vec<u8>>) {
    let n = nodes.len();
    match rng.below(10) {
        0 if n > 0 => {
            let i = rng.below(n);
            let keep = rng.below(nodes[i].len());
            nodes[i].truncate(keep);
        }
        1 if n > 0 => {
            nodes.remove(rng.below(n));
        }
        2 if n > 1 => {
            let (i, j) = (rng.below(n), rng.below(n));
            nodes.swap(i, j);
        }
        3 if n > 0 => {
            let i = rng.below(n);
            let copy = nodes[i].clone();
            nodes.insert(rng.below(n + 1), copy);
        }
        4 => {
            let len = rng.below(200);
            let junk = rng.bytes(len);
            nodes.insert(rng.below(n + 1), junk);
        }
        5 if n > 0 => {
            let i = rng.below(n);
            if !nodes[i].is_empty() {
                let at = rng.below(nodes[i].len());
                nodes[i][at] ^= 1 << rng.below(8);
            }
        }
        6 => {
            // A leaf with an oversized (65..=70 nibble) key.
            let len = 65 + rng.below(6);
            let nibs: Vec<u8> = (0..len).map(|_| rng.below(16) as u8).collect();
            let node = rlp_list(&[rlp_bytes(&hex_prefix(&nibs, true)), rlp_bytes(&[1; 40])]);
            nodes.insert(rng.below(n + 1), node);
        }
        7 if n > 0 => {
            // Many copies of one node.
            let i = rng.below(n);
            let copy = nodes[i].clone();
            for _ in 0..(1 + rng.below(80)) {
                nodes.insert(i, copy.clone());
            }
        }
        8 if n > 0 => {
            let i = rng.below(n);
            let len = 1 + rng.below(40);
            let extra = rng.bytes(len);
            nodes[i].extend(extra);
        }
        _ => {
            let len = rng.below(3);
            let extra = rng.bytes(len);
            nodes.push(extra);
        }
    }
}

/// Mutated real fixture proofs (the Sepolia account and storage proof, and a receipt proof):
/// truncated, dropped, swapped, duplicated, garbage, bit-flipped, oversized keys, extra nodes.
/// Never a panic; the untouched proof is Ok; any change is a clean error of the right kind.
#[test]
fn seeded_mutations_of_real_proofs_never_panic() {
    let mut w = World::default();
    let n = sepolia_finalized(&mut w);
    let m = sepolia_mapping();
    let c = ctx(&n);
    let run = |p: &StorageProof| {
        catch_unwind(AssertUnwindSafe(|| verify_storage_slot(&c, &m.witness, &m.address, &m.slot, Predicate::Eq(word(1)), p)))
            .unwrap_or_else(|e| panic!("verify_storage_slot panicked: {}", panic_text(e)))
    };
    let mut lf = log_fixture();
    let witness = lf.finalized_header(lf.root);
    let lc = ctx(&lf.n);
    let run_log = |p: &ReceiptProof| {
        catch_unwind(AssertUnwindSafe(|| verify_receipt_log(&lc, &witness, p, 1)))
            .unwrap_or_else(|e| panic!("verify_receipt_log panicked: {}", panic_text(e)))
    };
    assert!(run(&m.proof).is_ok() && run_log(&lf.proof).is_ok(), "the untouched proofs verify");
    let mut rng = Rng(0x0B16_4E7B);
    let mut changed = 0;
    for case in 0..300 {
        let rounds = 1 + rng.below(3);
        // Account nodes.
        let mut p = m.proof.clone();
        for _ in 0..rounds {
            mutate(&mut rng, &mut p.account.nodes);
        }
        let r = run(&p);
        if p.account.nodes == m.proof.account.nodes {
            assert!(r.is_ok(), "case {case}: {r:?}");
        } else {
            changed += 1;
            assert!(matches!(&r, Err(InclusionError::AccountProof(_))), "case {case} (account): {r:?}");
            assert_clean(&r.unwrap_err().to_string());
        }
        // Storage nodes.
        let mut p = m.proof.clone();
        for _ in 0..rounds {
            mutate(&mut rng, &mut p.nodes);
        }
        let r = run(&p);
        if p.nodes == m.proof.nodes {
            assert!(r.is_ok(), "case {case}: {r:?}");
        } else {
            assert!(matches!(&r, Err(InclusionError::StorageProof(_))), "case {case} (storage): {r:?}");
            assert_clean(&r.unwrap_err().to_string());
        }
        // Receipt nodes.
        let mut p = lf.proof.clone();
        for _ in 0..rounds {
            mutate(&mut rng, &mut p.nodes);
        }
        let r = run_log(&p);
        if p.nodes == lf.proof.nodes {
            assert!(r.is_ok(), "case {case}: {r:?}");
        } else {
            assert!(matches!(&r, Err(InclusionError::ReceiptProof(_))), "case {case} (receipt): {r:?}");
            assert_clean(&r.unwrap_err().to_string());
        }
    }
    assert!(changed > 250, "the mutations change the proof ({changed})");
}

/// A valid storage-style proof through a branch that holds an in-place sibling leaf with a long
/// key: [extension (20 nibbles), branch, leaf (43 nibbles)]. The walk is 20 + 1 + 43 = 64; a
/// bound that also counted the sibling's 43 nibbles (an earlier draft of the node check) refused
/// it, which would refuse real claims against large contracts with small values.
#[test]
fn an_in_place_sibling_off_the_path_does_not_count() {
    use alloy_trie::{HashBuilder, proof::ProofRetainer};
    let mut a = [0xAB; 32];
    let mut b = [0xAB; 32];
    a[10] = 0x31;
    b[10] = 0x72;
    let (ka, kb) = (Nibbles::unpack(a), Nibbles::unpack(b));
    let value_b = vec![0x5A; 33];
    let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([kb.clone()]));
    hb.add_leaf(ka, &[0x01]);
    hb.add_leaf(kb.clone(), &value_b);
    let root = hb.root();
    let nodes: Vec<Bytes> = hb.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, n)| n).collect();
    assert_eq!(nodes.len(), 3, "extension, branch, leaf");
    // 14 empty children, A's 25-byte leaf in place, B's hash, the empty value: 73 bytes of
    // payload (83 if A were hashed instead).
    assert_eq!(nodes[1].len(), 75, "the branch holds A's leaf in place");
    alloy_trie::proof::verify_proof(root, kb.clone(), Some(value_b), &nodes).expect("alloy-trie accepts it");
    let raw: Vec<Vec<u8>> = nodes.iter().map(|n| n.to_vec()).collect();
    assert_eq!(check_proof_nodes(&raw, &nibbles_of(&b), MAX_STATE_NODE_LEN), Ok(()));
}

/// Hash-linked random chains of branch, extension and leaf nodes along a real key, some valid,
/// many walking past 64 nibbles. Random byte mutations die at alloy-trie's first hash check;
/// these reach its path handling. Differential: whenever `check_proof_nodes` accepts, raw
/// alloy-trie does not panic, and the library's answer is alloy-trie's; it never refuses a proof
/// alloy-trie accepts; the library itself never panics. The control counts show the generator does reach the old panic and valid proofs.
#[test]
fn structured_random_proofs_never_panic_and_the_node_check_is_sound() {
    use alloy_primitives::{B256, U256};
    let fields = sepolia_mapping().proof.account.fields;
    let account = alloy_rlp::encode(&alloy_trie::TrieAccount {
        nonce: fields.nonce,
        balance: U256::from_be_bytes(fields.balance),
        storage_root: B256::from(fields.storage_root),
        code_hash: B256::from(fields.code_hash),
    });
    let mut rng = Rng(0x5EED_0064);
    let (mut raw_panics, mut valid, mut checked_ok) = (0, 0, 0);
    for case in 0..600 {
        let address: [u8; 20] = rng.bytes(20).try_into().unwrap();
        let key_nibbles: Vec<u8> = keccak256(address).0.iter().flat_map(|b| [b >> 4, b & 15]).collect();
        // Top-down plan: branches (one nibble) and extensions (several), then a leaf.
        #[derive(Clone, Copy)]
        enum Step {
            Branch,
            Ext,
        }
        // One case in four strays from the key somewhere (an exclusion or a mismatch).
        let stray = rng.below(4) == 0;
        let nibble = |rng: &mut Rng, pos: usize| if pos < 64 && !(stray && rng.below(16) == 0) { key_nibbles[pos] } else { rng.below(16) as u8 };
        let mut plan = Vec::new();
        let mut pos = 0;
        while pos < 64 && plan.len() < 70 && rng.below(10) >= 2 {
            if rng.below(2) == 0 {
                plan.push((Step::Branch, vec![nibble(&mut rng, pos)]));
                pos += 1;
            } else {
                let k = 1 + rng.below((64 - pos).min(40) + 3);
                let nibs: Vec<u8> = (0..k).map(|i| nibble(&mut rng, pos + i)).collect();
                plan.push((Step::Ext, nibs));
                pos += k;
            }
        }
        let rest = 64usize.saturating_sub(pos);
        let leaf_len = if rng.below(3) == 0 { rng.below(67) } else { rest };
        let leaf_nibs: Vec<u8> = (0..leaf_len).map(|i| nibble(&mut rng, pos + i)).collect();
        let value_len = 1 + rng.below(60);
        let value = if rng.below(4) == 0 { rng.bytes(value_len) } else { account.clone() };
        // Bottom-up: hash-link each node to its parent, or embed it when it is short.
        let mut current = rlp_list(&[rlp_bytes(&hex_prefix(&leaf_nibs, true)), rlp_bytes(&value)]);
        let mut below: Vec<Vec<u8>> = Vec::new();
        for (step, nibs) in plan.iter().rev() {
            let child = if current.len() < 32 && rng.below(2) == 0 {
                current.clone()
            } else {
                below.insert(0, current.clone());
                rlp_bytes(&keccak256(&current).0)
            };
            current = match step {
                Step::Branch => {
                    // Off-path children: absent, hashed, or a short leaf in place (which the walk
                    // never follows, so it must not count).
                    let mut items: Vec<Vec<u8>> = (0..16)
                        .map(|_| match rng.below(4) {
                            0 => rlp_bytes(&rng.word()),
                            1 => {
                                let len = rng.below(53);
                                let nibs: Vec<u8> = (0..len).map(|_| rng.below(16) as u8).collect();
                                rlp_list(&[rlp_bytes(&hex_prefix(&nibs, true)), rlp_bytes(&[1])])
                            }
                            _ => rlp_bytes(&[]),
                        })
                        .collect();
                    items[nibs[0] as usize] = child;
                    items.push(rlp_bytes(&[]));
                    rlp_list(&items)
                }
                Step::Ext => rlp_list(&[rlp_bytes(&hex_prefix(nibs, false)), child]),
            };
        }
        let root = keccak256(&current).0;
        let mut nodes = vec![current];
        nodes.extend(below);
        let ours = catch_unwind(AssertUnwindSafe(|| check_proof_nodes(&nodes, &key_nibbles, MAX_STATE_NODE_LEN)))
            .unwrap_or_else(|e| panic!("case {case}: check_proof_nodes panicked: {}", panic_text(e)));
        let bytes: Vec<Bytes> = nodes.iter().map(|n| Bytes::copy_from_slice(n)).collect();
        let raw = catch_unwind(AssertUnwindSafe(|| {
            alloy_trie::proof::verify_proof(B256::from(root), Nibbles::unpack(keccak256(address)), Some(account.clone()), &bytes)
        }));
        let proof = AccountProof { fields, nodes: nodes.clone() };
        let lib = catch_unwind(AssertUnwindSafe(|| verify_account_proof(&root, &address, &proof)))
            .unwrap_or_else(|e| panic!("case {case}: verify_account_proof panicked: {}", panic_text(e)));
        match (&ours, &raw) {
            (Ok(()), Err(e)) => panic!("case {case}: the node check accepted a proof alloy-trie panics on: {}", payload_text(e)),
            (Ok(()), Ok(raw)) => {
                checked_ok += 1;
                assert_eq!(lib.is_ok(), raw.is_ok(), "case {case}: {lib:?} vs {raw:?}");
            }
            (Err(_), Err(_)) => raw_panics += 1,
            // Completeness: never refuse a proof alloy-trie accepts.
            (Err(e), Ok(Ok(()))) => panic!("case {case}: the node check refused a valid proof: {e}"),
            (Err(_), Ok(Err(_))) => {}
        }
        match &lib {
            Ok(()) => valid += 1,
            Err(InclusionError::AccountProof(e)) => assert_clean(e),
            Err(e) => panic!("case {case}: {e:?}"),
        }
    }
    eprintln!("structured fuzz: {raw_panics} reach alloy-trie's panic (all refused first), {valid} valid, {checked_ok} pass the node check");
    assert!(raw_panics >= 20, "the generator reaches alloy-trie's overflow panic ({raw_panics})");
    assert!(valid >= 20, "the generator builds valid proofs too ({valid})");
    assert!(checked_ok >= valid, "{checked_ok} {valid}");
}
