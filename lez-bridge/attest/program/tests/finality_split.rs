// SPDX-License-Identifier: MIT OR Apache-2.0
//! The finality update split at the pairing seam (branch `public-update`): `FinalityBegin` (every
//! check up to and including the Miller loop; a job holds the Miller value) and `FinalityFinish`
//! (the final exponentiation and the store write). For participation too low for one transaction.
//! SYNTHETIC sync committee (test keys from fixed public seeds) over the recorded Sepolia finality
//! update: only the committee, the bits and the signature are synthetic; the store's commitment is
//! that committee's (planted, as `instance.rs` plants anchors).
#![cfg(feature = "public-update")]

mod common;
use blst::min_pk::{AggregatePublicKey, AggregateSignature, SecretKey};
use common::*;
use eth_attest::check::{self, Code};
use eth_attest_core::{JobState, KeyWitnesses, LcSummary, job_address, job_id};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

struct Synth {
    w: World,
    n: Net,
    committee: Vec<u8>,
    sks: Vec<SecretKey>,
    msg: [u8; 32],
}

fn synth() -> Synth {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let sks: Vec<SecretKey> = (0..512u32)
        .map(|i| SecretKey::key_gen(&eth_attest_core::sha256(&[b"RFP021/synthetic-committee/v1", &i.to_le_bytes()]), &[]).unwrap())
        .collect();
    let pks: Vec<_> = sks.iter().map(SecretKey::sk_to_pk).collect();
    let refs: Vec<_> = pks.iter().collect();
    let agg = AggregatePublicKey::aggregate(&refs, false).unwrap().to_public_key();
    let mut committee: Vec<u8> = pks.iter().flat_map(|p| p.compress()).collect();
    committee.extend_from_slice(&agg.compress());
    // Plant the store with the synthetic committee's commitment.
    let store = LcSummary { current_committee: committee_hash(&committee), ..w.store(&n.id) };
    w.states.insert((store_address(&ATTEST, &n.id), ATTEST), borsh::to_vec(&store).unwrap());
    // The signing root, through the real update's own job (Begin computes it before the signature).
    let fin = fx("sepolia", "finality_update.ssz");
    let keys = eth_attest::public::key_witnesses(&committee, &[true; 512]).unwrap();
    let msg = {
        // A 512/512 job built natively; its signing root does not depend on the signature.
        let t = signed(&fin, &sks, &[0u8; 32], 512).0;
        match check::finality_begin(&ATTEST, &n.cfg, &store, &committee, &keys, &t) {
            Ok(j) => j.signing_root,
            Err(_) => panic!("cannot read the signing root"),
        }
    };
    Synth { w, n, committee, sks, msg }
}

/// The update signed over `msg` by the first `n` keys (the rest absent), and its bits.
fn signed(update: &[u8], sks: &[SecretKey], msg: &[u8; 32], n: usize) -> (Vec<u8>, Vec<bool>) {
    let bits: Vec<bool> = (0..512).map(|i| i < n).collect();
    let sigs: Vec<_> = sks.iter().take(n).map(|k| k.sign(msg, DST, &[])).collect();
    let refs: Vec<_> = sigs.iter().collect();
    let sig = AggregateSignature::aggregate(&refs, false).unwrap().to_signature().compress();
    let mut t = update.to_vec();
    let mut b = [0u8; 64];
    for (i, on) in bits.iter().enumerate() {
        if *on {
            b[i / 8] |= 1 << (i % 8);
        }
    }
    t[1280..1344].copy_from_slice(&b);
    t[1344..1440].copy_from_slice(&sig);
    (t, bits)
}

fn begin(s: &mut Synth, update: &[u8], keys: KeyWitnesses) -> Result<eth_attest_core::PeriodJob, String> {
    let prev = s.w.store(&s.n.id);
    let m = Message::Instruction(Instruction::FinalityBegin { config: s.n.cfg.clone(), prev: prev.clone(), committee: s.committee.clone(), keys, update: update.to_vec() });
    s.w.tx(BOB, ATTEST, &m)?;
    let id = job_id(&prev, &eth_attest_core::sha256(&[update]));
    match borsh::from_slice::<JobState>(&s.w.state(job_address(&ATTEST, &id), ATTEST)) {
        Ok(JobState::Open(j)) => Ok(j),
        other => panic!("no open job: {other:?}"),
    }
}

fn finish(s: &mut Synth, job: &eth_attest_core::PeriodJob) -> Result<(), String> {
    let id = job_id(&job.prev, &job.update_digest);
    s.w.tx(CAROL, ATTEST, &Message::Instruction(Instruction::FinalityFinish { job_id: id }))
}

#[test]
fn the_split_takes_low_participation_down_to_the_policy_floor() {
    let fin = fx("sepolia", "finality_update.ssz");
    for n in [427usize, 342] {
        let mut s = synth();
        let (t, bits) = signed(&fin, &s.sks, &s.msg, n);
        let keys = eth_attest::public::key_witnesses(&s.committee, &bits).unwrap();
        let prev = s.w.store(&s.n.id);
        let job = begin(&mut s, &t, keys.clone()).unwrap();
        assert_eq!(job.miller.as_ref().map(Vec::len), Some(576));
        assert_eq!(s.w.store(&s.n.id), prev, "Begin changes no store");
        finish(&mut s, &job).unwrap();
        let after = s.w.store(&s.n.id);
        assert_eq!(after.updates, 1, "{n}/512: advanced");
        assert_eq!(after.finalized.beacon.slot, meta("sepolia")["finality_update"]["finalized_slot"].as_u64().unwrap());
        // The same result as the one-transaction path's verification of the same input.
        let v = check::finality_public(&ATTEST, &s.n.cfg, &prev, &s.committee, &keys, &t).unwrap();
        assert_eq!(after, v.next);
        // Finished once: a second Finish finds no open job.
        assert_eq!(Code::parse(&finish(&mut s, &job).unwrap_err()), Some(Code::StaleOrReplayed));
    }
}

#[test]
fn the_split_refuses_a_bad_signature_at_finish_and_low_participation_at_begin() {
    let fin = fx("sepolia", "finality_update.ssz");
    let mut s = synth();
    // Signed over another message: Begin's Miller loop runs, Finish's final exponentiation refuses.
    let (t, bits) = signed(&fin, &s.sks, &[7u8; 32], 400);
    let keys = eth_attest::public::key_witnesses(&s.committee, &bits).unwrap();
    let job = begin(&mut s, &t, keys).unwrap();
    assert_eq!(Code::parse(&finish(&mut s, &job).unwrap_err()), Some(Code::SignatureInvalid));
    assert_eq!(check::finality_finish(&job), Err(Code::SignatureInvalid));
    assert_eq!(s.w.store(&s.n.id).updates, 0);
    // 341 signers, one under the policy: refused at Begin.
    let (t, bits) = signed(&fin, &s.sks, &s.msg, 341);
    let keys = eth_attest::public::key_witnesses(&s.committee, &bits).unwrap();
    let prev = s.w.store(&s.n.id);
    let m = Message::Instruction(Instruction::FinalityBegin { config: s.n.cfg.clone(), prev: prev.clone(), committee: s.committee.clone(), keys: keys.clone(), update: t.clone() });
    assert_eq!(Code::parse(&s.w.tx(BOB, ATTEST, &m).unwrap_err()), Some(Code::ParticipationBelowPolicy));
    assert_eq!(check::finality_begin(&ATTEST, &s.n.cfg, &prev, &s.committee, &keys, &t).err(), Some(Code::ParticipationBelowPolicy));
    // A wrong key witness is refused at Begin (no Miller value).
    let (t, bits) = signed(&fin, &s.sks, &s.msg, 400);
    let mut keys = eth_attest::public::key_witnesses(&s.committee, &bits).unwrap();
    keys.absent[0].1[95] ^= 1;
    let m = Message::Instruction(Instruction::FinalityBegin { config: s.n.cfg.clone(), prev, committee: s.committee.clone(), keys, update: t });
    assert_eq!(Code::parse(&s.w.tx(BOB, ATTEST, &m).unwrap_err()), Some(Code::SignatureInvalid));
    // A period job (no Miller value) cannot be finished as a finality job.
    let period = eth_attest_core::PeriodJob { miller: None, ..job };
    assert_eq!(check::finality_finish(&period), Err(Code::MalformedInput));
}
