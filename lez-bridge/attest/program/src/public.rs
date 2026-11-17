//! The public update path (feature `public-update`, branch `public-update`; `DESIGN.md` §9.3a,
//! §9.3b): updates verified inside the program's own PUBLIC execution, cheaply enough for LEZ's 10M
//! gas cap per transaction.
//!
//! Every Helios check runs, unchanged, through the vendored Helios's `verify_generic_update_with`
//! (`attest/vendor/helios-consensus-core/SPIKE-PATCH.diff`, patch 1). Only two things are ours:
//! - the sync-committee signature check ([`fast::verify`]): blst, the signers' key as the
//!   committee's aggregate minus the non-signers, given uncompressed and bound byte for byte to
//!   the committee witness, which must hash to the store's commitment of the committee Helios
//!   selects by the signature period;
//! - the next committee's SSZ root, computed by direct SHA-256 calls instead of `tree_hash`
//!   (patch 2, `sync_committee_root`; equal to `tree_hash_root`, tested).
//!
//! Helios's store is rebuilt with stand-in committees (the keys reach the check only through the
//! hook): an empty one for the current committee, and for the next one either the update's own
//! next committee (when it hashes to `prev.next_committee`) or a second stand-in. After Helios's
//! `apply`, the stand-ins map back to the store's commitments, so a rotation is recognised exactly.
//!
//! - [`verify_finality_public`] (`UpdatePublic`): a finality update in ONE transaction, signed by
//!   the current committee or, once the store knows it, by the next one (the rotation).
//! - [`verify_period_begin`] (`PeriodBegin`): a period update's every check but the signature,
//!   giving a [`PeriodJob`]; [`verify_period_finish`] (`PeriodFinish`): the job's signature. The
//!   job PDA ([`apply_job`]) holds the job between the two transactions and sends the store its
//!   `Advance` only after the signature verified; the store applies it only if it still equals
//!   the job's `prev`. `CloseJob` empties a job once the store has moved past its `prev`.

use alloy_primitives::B256;
use eth_attest_core::{
    Effect, InstanceConfig, JobState, KeyWitnesses, LcSummary, Message, PeriodJob, committee_hash, instance_id,
    job_address, job_id, job_seed, store_address, store_seed,
};
use helios_consensus_core::{
    apply_generic_update,
    consensus_spec::MainnetConsensusSpec as S,
    types::{FinalityUpdate, GenericUpdate, LightClientHeader, LightClientHeaderBellatrix, LightClientStore, SyncCommittee, Update, bls::PublicKey},
    verify_generic_update_with,
};
use lee_core::{
    account::AccountId,
    program::{ReceiveInput, Response},
};
use ssz::{Decode, Encode};

use crate::lc::{Verified, check_participation, finalized_of, forks, to_helios};

/// What a Helios run decided about the signature, without checking it.
struct Signed {
    /// The commitment of the committee Helios selected.
    committee: [u8; 32],
    signing_root: [u8; 32],
    signature: [u8; 96],
    bits: [u8; 64],
}

/// A committee that stands in for the store's next one. Stand-ins are told apart by their
/// aggregate key alone (all zero bytes for the current one, all 0xFF for the next one): neither is
/// a valid compressed G1 point, so no real committee has either, and comparing 48 bytes costs far
/// less than comparing or cloning 513 keys.
fn next_stand_in() -> SyncCommittee<S> {
    let mut c = SyncCommittee::<S>::default();
    c.aggregate_pubkey = PublicKey::from_ssz_bytes(&[0xFF; 48]).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: stand-in key"));
    c
}

/// Helios's verify (signature through `check`) and apply on stand-in committees, mapped back to
/// commitments. `check` gets what Helios selected and returns whether the signature is valid.
fn run_helios(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    g: &GenericUpdate<S>,
    check: impl FnOnce(&Signed) -> bool,
) -> Result<(Verified, Signed), String> {
    if instance_id(program, config) != prev.instance_id {
        return Err("the config is not this store's instance".into());
    }
    check_participation(config, g)?;
    let deadline_ms = crate::lc::ws_check(config, prev, g.signature_slot)?;
    let next = match prev.next_committee {
        None => None,
        Some(h) => Some(match &g.next_sync_committee {
            Some(c) if committee_hash(&c.as_ssz_bytes()) == h => c.clone(),
            _ => next_stand_in(),
        }),
    };
    // What identifies each committee after Helios's apply (see `next_stand_in`).
    let current_tag = PublicKey::default();
    let next_tag = next.as_ref().map(|c| c.aggregate_pubkey.clone());
    let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix { beacon: to_helios(&prev.finalized.beacon) });
    let mut store = LightClientStore::<S> {
        finalized_header: header.clone(),
        current_sync_committee: SyncCommittee::<S>::default(),
        next_sync_committee: next,
        optimistic_header: header,
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
        best_valid_update: None,
    };
    let mut signed = None;
    let cur_ref = &store.current_sync_committee;
    let next_ref = store.next_sync_committee.as_ref();
    verify_generic_update_with::<S, _>(g, g.signature_slot, &store, B256::from(config.genesis_validators_root), &forks(config), |committee, bits, root, sig| {
        // Which committee signs is Helios's choice, by the signature period.
        let commitment = if core::ptr::eq(committee, cur_ref) {
            prev.current_committee
        } else if next_ref.is_some_and(|n| core::ptr::eq(committee, n)) {
            prev.next_committee.unwrap_or_else(|| unreachable!())
        } else {
            return Ok(false);
        };
        let (Ok(signature), Ok(bits)) = (<[u8; 96]>::try_from(sig.as_ssz_bytes()), <[u8; 64]>::try_from(bits.as_ssz_bytes())) else {
            return Ok(false);
        };
        let s = Signed { committee: commitment, signing_root: root.0, signature, bits };
        let ok = check(&s);
        signed = Some(s);
        Ok(ok)
    })
    .map_err(|e| format!("update rejected: {e}"))?;
    let signed = signed.ok_or("no signature check")?;
    apply_generic_update(&mut store, g);
    let current_hash = if store.current_sync_committee.aggregate_pubkey == current_tag {
        prev.current_committee
    } else if next_tag.as_ref() == Some(&store.current_sync_committee.aggregate_pubkey) {
        prev.next_committee.ok_or("rotated to an unknown committee")?
    } else {
        return Err("the current committee changed to an unknown one".into());
    };
    let next_hash = match &store.next_sync_committee {
        None => None,
        Some(c) if next_tag.as_ref() == Some(&c.aggregate_pubkey) => prev.next_committee,
        Some(c) if c.aggregate_pubkey == current_tag || c.aggregate_pubkey.as_ssz_bytes() == [0xFF; 48] => {
            return Err("a stand-in committee reached the store".into());
        }
        Some(c) => Some(committee_hash(&c.as_ssz_bytes())),
    };
    let finalized = if store.finalized_header.beacon().slot == prev.finalized.beacon.slot {
        prev.finalized
    } else {
        finalized_of(&store.finalized_header)?
    };
    let next_summary = LcSummary {
        instance_id: prev.instance_id,
        evm_chain_id: prev.evm_chain_id,
        finalized,
        current_committee: current_hash,
        next_committee: next_hash,
        updates: prev.updates.checked_add(1).ok_or("update count overflow")?,
    };
    if !prev.is_advanced_by(&next_summary) {
        return Err("update verified but does not advance the store (not newer, or no 2/3 supermajority)".into());
    }
    let start = config
        .genesis_time
        .checked_add(g.signature_slot.checked_mul(config.seconds_per_slot).ok_or("slot overflow")?)
        .ok_or("time overflow")?;
    let not_before_ms = start.checked_mul(1000).ok_or("time overflow")?;
    Ok((Verified { next: next_summary, not_before_ms, deadline_ms }, signed))
}

/// The participation bits as booleans (SSZ `Bitvector[512]`: bit i is byte i/8, bit i%8).
#[must_use]
pub fn expand_bits(b: &[u8; 64]) -> Vec<bool> {
    (0..512).map(|i| (b[i / 8] >> (i % 8)) & 1 == 1).collect()
}

fn sig_ok(committee: &[u8], keys: &KeyWitnesses, s: &Signed) -> bool {
    committee.len() == eth_attest_core::COMMITTEE_SSZ_LEN
        && committee_hash(committee) == s.committee
        && fast::verify(committee, keys, &expand_bits(&s.bits), &s.signing_root, &s.signature)
}

/// A finality update in one public transaction (`UpdatePublic`). `committee` is the SSZ of the
/// committee Helios selects by the signature period; it must hash to that commitment in `prev`.
///
/// # Errors
/// Any check fails (a wrong committee witness fails as an invalid signature).
pub fn verify_finality_public(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    keys: &KeyWitnesses,
    update: &[u8],
) -> Result<Verified, String> {
    let u = FinalityUpdate::<S>::from_ssz_bytes(update).map_err(|e| format!("finality update does not decode: {e:?}"))?;
    let g = GenericUpdate::from(&u);
    run_helios(program, config, prev, &g, |s| sig_ok(committee, keys, s)).map(|(v, _)| v)
}

/// Step 1 of a public period update: every check but the signature; the job it leads to.
///
/// # Errors
/// Any check fails.
pub fn verify_period_begin(program: &AccountId, config: &InstanceConfig, prev: &LcSummary, update: &[u8]) -> Result<PeriodJob, String> {
    let u = Update::<S>::from_ssz_bytes(update).map_err(|e| format!("update does not decode: {e:?}"))?;
    let g = GenericUpdate::from(&u);
    let (v, s) = run_helios(program, config, prev, &g, |_| true)?;
    Ok(PeriodJob {
        prev: prev.clone(),
        next: v.next,
        update_digest: eth_attest_core::sha256(&[update]),
        signing_root: s.signing_root,
        signature: s.signature,
        bits: s.bits,
        committee: s.committee,
        not_before_ms: v.not_before_ms,
        deadline_ms: v.deadline_ms,
        miller: None,
    })
}

/// Step 1 of a finality update split at the pairing seam: every check of
/// [`verify_finality_public`] up to and including the Miller loop; the job carries the Miller value.
///
/// # Errors
/// Any check fails (a refused witness or signature is `invalid sync committee signature`).
pub fn verify_finality_begin(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    keys: &KeyWitnesses,
    update: &[u8],
) -> Result<PeriodJob, String> {
    let u = FinalityUpdate::<S>::from_ssz_bytes(update).map_err(|e| format!("finality update does not decode: {e:?}"))?;
    let g = GenericUpdate::from(&u);
    let mut f = None;
    let (v, s) = run_helios(program, config, prev, &g, |s| {
        if committee.len() == eth_attest_core::COMMITTEE_SSZ_LEN && committee_hash(committee) == s.committee {
            f = fast::miller(committee, keys, &expand_bits(&s.bits), &s.signing_root, &s.signature);
        }
        f.is_some()
    })?;
    Ok(PeriodJob {
        prev: prev.clone(),
        next: v.next,
        update_digest: eth_attest_core::sha256(&[update]),
        signing_root: s.signing_root,
        signature: s.signature,
        bits: s.bits,
        committee: s.committee,
        not_before_ms: v.not_before_ms,
        deadline_ms: v.deadline_ms,
        miller: f.map(|m| m.to_vec()),
    })
}

/// Step 2 of the split: the final exponentiation of the job's Miller value.
///
/// # Errors
/// No Miller value in the job, or the pairing product is not one.
pub fn verify_finality_finish(job: &PeriodJob) -> Result<(), String> {
    let Some(m) = &job.miller else { return Err("not a finality job (no Miller value): finish it with PeriodFinish".into()) };
    let Ok(m) = <[u8; 576]>::try_from(m.as_slice()) else { return Err("the job's Miller value has the wrong length".into()) };
    if fast::final_is_one(&m) { Ok(()) } else { Err("update rejected: invalid sync committee signature (final exponentiation)".into()) }
}

/// Step 2: the job's signature, by the committee whose SSZ is `committee`.
///
/// # Errors
/// The committee is not the job's, or the signature does not verify.
pub fn verify_period_finish(job: &PeriodJob, committee: &[u8], keys: &KeyWitnesses) -> Result<(), String> {
    let s = Signed { committee: job.committee, signing_root: job.signing_root, signature: job.signature, bits: job.bits };
    if sig_ok(committee, keys, &s) { Ok(()) } else { Err("update rejected: invalid sync committee signature".into()) }
}

/// The key witnesses for `bits` against this committee SSZ (off-chain helper; the program checks
/// them, it never trusts them).
///
/// # Errors
/// The committee SSZ has the wrong length or a key does not decompress.
pub fn key_witnesses(committee_ssz: &[u8], bits: &[bool]) -> Result<KeyWitnesses, String> {
    // Any committee size (512 on every real network; the consensus-spec vectors' minimal preset
    // has 32): `bits.len()` keys, then the aggregate.
    let size = bits.len();
    if size == 0 || committee_ssz.len() != (size + 1) * 48 {
        return Err("committee SSZ or bits have the wrong length".into());
    }
    let unc = |i: usize| fast::uncompress(&committee_ssz[i * 48..i * 48 + 48]).ok_or_else(|| format!("committee key {i} does not decompress"));
    let mut absent = Vec::new();
    for (i, b) in bits.iter().enumerate() {
        if !b {
            absent.push((u16::try_from(i).map_err(|_| "index")?, unc(i)?));
        }
    }
    Ok(KeyWitnesses { aggregate: unc(size)?, absent })
}

/// The participation bits of a finality or period update (SSZ).
///
/// # Errors
/// The update does not decode as `kind`.
pub fn update_bits(update: &[u8], period: bool) -> Result<Vec<bool>, String> {
    let g = if period {
        GenericUpdate::from(&Update::<S>::from_ssz_bytes(update).map_err(|e| format!("update does not decode: {e:?}"))?)
    } else {
        GenericUpdate::from(&FinalityUpdate::<S>::from_ssz_bytes(update).map_err(|e| format!("finality update does not decode: {e:?}"))?)
    };
    Ok(g.sync_aggregate.sync_committee_bits.iter().collect())
}

/// The finality update's bits (kept for the earlier harness).
///
/// # Errors
/// The update does not decode.
pub fn finality_bits(update: &[u8]) -> Result<Vec<bool>, String> {
    update_bits(update, false)
}

/// The job PDA's and the store's side of the period path. `e` is one of `JobOpen`, `JobFinish`,
/// `JobClose`, `AssertStale`, `AssertCurrent`, already checked to come from this program.
pub fn apply_job(input: &ReceiveInput, e: Effect) -> Response {
    let program = input.receiver.program_account_id;
    let here = input.receiver.account_id;
    match e {
        Effect::JobOpen(job) => {
            assert!(here == job_address(&program, &job_id(&job.prev, &job.update_digest)), "RFP021-E12 not permitted: job goes to its PDA");
            assert!(input.pre_state.is_empty(), "RFP021-E09 stale or replayed: this job exists");
            Response::set_state(crate::state(&JobState::Open(job)))
        }
        Effect::JobFinish { job, committee, keys } => {
            let id = job_id(&job.prev, &job.update_digest);
            assert!(here == job_address(&program, &id), "RFP021-E12 not permitted: job goes to its PDA");
            let Ok(JobState::Open(open)) = borsh::from_slice::<JobState>(&input.pre_state) else {
                panic!("RFP021-E09 stale or replayed: no open job here")
            };
            assert!(open == job, "RFP021-E09 stale or replayed: not this job");
            verify_period_finish(&job, &committee, &keys).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = store_seed(&job.prev.instance_id);
            Response::set_state(crate::state(&JobState::Done))
                .send(crate::effect(seed, &program, Effect::Advance { prev: job.prev, next: job.next }))
        }
        Effect::JobFinalize { job_id: id } => {
            assert!(here == job_address(&program, &id), "RFP021-E12 not permitted: job goes to its PDA");
            let Ok(JobState::Open(job)) = borsh::from_slice::<JobState>(&input.pre_state) else {
                panic!("RFP021-E09 stale or replayed: no open job here")
            };
            verify_finality_finish(&job).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = store_seed(&job.prev.instance_id);
            let (from, until) = (job.not_before_ms, job.deadline_ms);
            Response::set_state(crate::state(&JobState::Done))
                .send(crate::effect(seed, &program, Effect::Advance { prev: job.prev, next: job.next }))
                .try_timestamp_window(from..until)
                .unwrap_or_else(|_| panic!("RFP021-E04 checkpoint stale or invalid: weak-subjectivity bound passed"))
        }
        Effect::JobClose { job_id: id } => {
            assert!(here == job_address(&program, &id), "RFP021-E12 not permitted: job goes to its PDA");
            assert!(!input.pre_state.is_empty(), "RFP021-E09 stale or replayed: no job here");
            let prev = match borsh::from_slice::<JobState>(&input.pre_state) {
                Ok(JobState::Open(job)) => Some(job.prev),
                Ok(JobState::Done) => None,
                Err(_) => panic!("RFP021-E08 malformed input: job state does not decode"),
            };
            let r = Response::set_state(lee_core::account::ActorState::empty());
            match prev {
                // An open job closes only if the store has moved past its prev.
                Some(prev) => {
                    let seed = store_seed(&prev.instance_id);
                    r.send(crate::effect(seed, &program, Effect::AssertStale { prev }))
                }
                None => r,
            }
        }
        Effect::AssertStale { prev } => {
            assert!(here == store_address(&program, &prev.instance_id), "RFP021-E12 not permitted: goes to the instance's store");
            if !input.pre_state.is_empty() {
                let Ok(current) = borsh::from_slice::<LcSummary>(&input.pre_state) else {
                    panic!("RFP021-E08 malformed input: store state does not decode")
                };
                assert!(current != prev, "RFP021-E09 stale or replayed: the job is on the current store: it can still finish");
            }
            Response::keep_state()
        }
        Effect::AssertCurrent { prev } => {
            assert!(here == store_address(&program, &prev.instance_id), "RFP021-E12 not permitted: goes to the instance's store");
            assert!(!input.pre_state.is_empty(), "RFP021-E13 witness mismatch: no such instance");
            let Ok(current) = borsh::from_slice::<LcSummary>(&input.pre_state) else {
                panic!("RFP021-E08 malformed input: store state does not decode")
            };
            assert!(current == prev, "RFP021-E09 stale or replayed: the store changed since this update was verified");
            Response::keep_state()
        }
        _ => panic!("RFP021-E12 not permitted: not a job effect"),
    }
}

/// The instructions of the public path, run in the submitter's own signed actor.
pub fn instruct(program: &AccountId, i: eth_attest_core::Instruction) -> Response {
    use eth_attest_core::Instruction as I;
    match i {
        I::UpdatePublic { config, prev, committee, keys, update } => {
            let v = verify_finality_public(program, &config, &prev, &committee, &keys, &update).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = store_seed(&prev.instance_id);
            Response::keep_state()
                .send(crate::effect(seed, program, Effect::Advance { prev, next: v.next }))
                .try_timestamp_window(v.not_before_ms..v.deadline_ms)
                .unwrap_or_else(|_| panic!("RFP021-E04 checkpoint stale or invalid: weak-subjectivity bound passed"))
        }
        I::PeriodBegin { config, prev, update } => {
            let job = verify_period_begin(program, &config, &prev, &update).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = job_seed(&job_id(&job.prev, &job.update_digest));
            // The store first: a job opens only on a real instance's current state.
            Response::keep_state()
                .send(crate::effect(store_seed(&prev.instance_id), program, Effect::AssertCurrent { prev }))
                .send(crate::effect(seed, program, Effect::JobOpen(job)))
        }
        I::PeriodFinish { job, committee, keys } => {
            // The store first (cheap), then the job PDA checks the signature: a Finish against a
            // moved store stops before the expensive part.
            let seed = job_seed(&job_id(&job.prev, &job.update_digest));
            let (from, until) = (job.not_before_ms, job.deadline_ms);
            Response::keep_state()
                .send(crate::effect(store_seed(&job.prev.instance_id), program, Effect::AssertCurrent { prev: job.prev.clone() }))
                .send(crate::effect(seed, program, Effect::JobFinish { job, committee, keys }))
                .try_timestamp_window(from..until)
                .unwrap_or_else(|_| panic!("RFP021-E04 checkpoint stale or invalid: weak-subjectivity bound passed"))
        }
        I::FinalityBegin { config, prev, committee, keys, update } => {
            let job = verify_finality_begin(program, &config, &prev, &committee, &keys, &update).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = job_seed(&job_id(&job.prev, &job.update_digest));
            Response::keep_state()
                .send(crate::effect(store_seed(&prev.instance_id), program, Effect::AssertCurrent { prev }))
                .send(crate::effect(seed, program, Effect::JobOpen(job)))
        }
        I::FinalityFinish { job_id } => Response::keep_state().send(crate::effect(job_seed(&job_id), program, Effect::JobFinalize { job_id })),
        #[cfg(feature = "synthetic-init")]
        I::InitSynthetic { config, genesis } => {
            // TEST ONLY (feature `synthetic-init`, a separate test image): no bootstrap.
            let seed = eth_attest_core::config_seed(&genesis.instance_id);
            Response::keep_state().send(crate::effect(seed, program, Effect::Create { config, genesis }))
        }
        I::CloseJob { job_id } => Response::keep_state().send(crate::effect(job_seed(&job_id), program, Effect::JobClose { job_id })),
        _ => panic!("RFP021-E12 not permitted: not a public-path instruction"),
    }
}

/// The next committee's root: Helios's `is_next_committee_proof_valid` uses the vendored patch 2,
/// `helios_consensus_core::sync_committee_root` (direct SHA-256); re-exported for tests.
pub use helios_consensus_core::sync_committee_root;


/// The signature check of the public path, with blst (RISC Zero's accelerated fork in the guest).
/// From the lcbench spike's `lc_bench_fast` guest (`bl::verify`), keys read from the committee SSZ.
pub mod fast {
    use blst::{
        BLST_ERROR, blst_bendian_from_fp, blst_final_exp, blst_fp_from_bendian, blst_fp12, blst_fp12_is_one, blst_hash_to_g2, blst_miller_loop_n, blst_p1,
        blst_p1_add_or_double, blst_p1_add_or_double_affine, blst_p1_affine, blst_p1_affine_compress,
        blst_p1_affine_generator, blst_p1_affine_is_inf, blst_p1_affine_serialize, blst_p1_cneg, blst_p1_deserialize,
        blst_p1_from_affine, blst_p1_to_affine, blst_p1_uncompress, blst_p2, blst_p2_affine, blst_p2_affine_in_g2,
        blst_p2_affine_is_inf, blst_p2_to_affine, blst_p2_uncompress,
    };
    use eth_attest_core::KeyWitnesses;

    /// The Ethereum BLS signature ciphersuite (proof of possession).
    pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

    /// Off-chain only: a compressed key, uncompressed (on the curve and in G1).
    #[must_use]
    pub fn uncompress(compressed: &[u8]) -> Option<[u8; 96]> {
        if compressed.len() != 48 {
            return None;
        }
        let mut p = blst_p1_affine::default();
        // SAFETY: 48 readable bytes, as checked above; blst writes only `p`.
        if unsafe { blst_p1_uncompress(&mut p, compressed.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
            return None;
        }
        let mut out = [0u8; 96];
        // SAFETY: `out` has the 96 bytes an uncompressed G1 point takes.
        unsafe { blst_p1_affine_serialize(out.as_mut_ptr(), &p) };
        Some(out)
    }

    /// An uncompressed key that is on the curve, not infinity, and compresses to `committed`.
    fn g1(unc: &[u8; 96], committed: &[u8]) -> Option<blst_p1_affine> {
        // Uncompressed form only: the compression, infinity and sort flag bits must be clear
        // (blst would otherwise read the first 48 bytes as a compressed point).
        if unc[0] & 0xe0 != 0 {
            return None;
        }
        let mut p = blst_p1_affine::default();
        // SAFETY: `unc` is 96 readable bytes; blst writes only `p`.
        if unsafe { blst_p1_deserialize(&mut p, unc.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
            return None;
        }
        let mut c = [0u8; 48];
        // SAFETY: `c` has the 48 bytes a compressed G1 point takes.
        unsafe { blst_p1_affine_compress(c.as_mut_ptr(), &p) };
        // SAFETY: reads `p` only.
        (!unsafe { blst_p1_affine_is_inf(&p) } && c[..] == *committed).then_some(p)
    }

    /// Whether `sig` is the committee's signature of `msg` by exactly the signers in `bits`:
    /// e(-g1, sig) · e(aggregate − Σ non-signers, H(msg)) = 1.
    #[must_use]
    pub fn verify(committee: &[u8], keys: &KeyWitnesses, bits: &[bool], msg: &[u8; 32], sig: &[u8; 96]) -> bool {
        miller_raw(committee, keys, bits, msg, sig).is_some_and(|f| {
            let mut e = blst_fp12::default();
            // SAFETY: live locals of the types blst expects.
            unsafe {
                blst_final_exp(&mut e, &f);
                blst_fp12_is_one(&e)
            }
        })
    }

    /// The canonical bytes of an Fp12 element: its 12 Fp coefficients, each 48 bytes big endian
    /// (`blst_bendian_from_fp`), in blst's field order. The same on every target, so a value the
    /// guest wrote is read back the same way.
    fn fp12_bytes(f: &blst_fp12) -> [u8; 576] {
        let mut out = [0u8; 576];
        let mut k = 0;
        for f6 in &f.fp6 {
            for f2 in &f6.fp2 {
                for fp in &f2.fp {
                    // SAFETY: 48 writable bytes at `out[k..]`.
                    unsafe { blst_bendian_from_fp(out[k..k + 48].as_mut_ptr(), fp) };
                    k += 48;
                }
            }
        }
        out
    }

    fn fp12_from(bytes: &[u8; 576]) -> blst_fp12 {
        let mut f = blst_fp12::default();
        let mut k = 0;
        for f6 in &mut f.fp6 {
            for f2 in &mut f6.fp2 {
                for fp in &mut f2.fp {
                    // SAFETY: 48 readable bytes at `bytes[k..]`; blst writes only `fp`.
                    unsafe { blst_fp_from_bendian(fp, bytes[k..k + 48].as_ptr()) };
                    k += 48;
                }
            }
        }
        f
    }

    /// The pairing-seam split's second half: the final exponentiation of a Miller-loop value and
    /// the identity check (blst's `final_exp` + `fp12_is_one`).
    #[must_use]
    pub fn final_is_one(f: &[u8; 576]) -> bool {
        let f = fp12_from(f);
        let mut e = blst_fp12::default();
        // SAFETY: live locals of the types blst expects.
        unsafe {
            blst_final_exp(&mut e, &f);
            blst_fp12_is_one(&e)
        }
    }

    /// The pairing-seam split's first half: every check of [`verify`] up to and including the
    /// shared Miller loop of e(pk, H(msg)) · e(-g1, sig); `None` if a witness, the signature or
    /// the signers' key is refused before the loop.
    #[must_use]
    pub fn miller(committee: &[u8], keys: &KeyWitnesses, bits: &[bool], msg: &[u8; 32], sig: &[u8; 96]) -> Option<[u8; 576]> {
        miller_raw(committee, keys, bits, msg, sig).map(|f| fp12_bytes(&f))
    }

    fn miller_raw(committee: &[u8], keys: &KeyWitnesses, bits: &[bool], msg: &[u8; 32], sig: &[u8; 96]) -> Option<blst_fp12> {
        // `bits.len()` keys then the aggregate (512 on every real network; the program's callers
        // pass a 512-bit vector, the consensus-spec minimal vectors 32).
        let size = bits.len();
        if size == 0 || committee.len() != (size + 1) * 48 {
            return None;
        }
        let committed = |i: usize| &committee[i * 48..i * 48 + 48];
        let Some(agg) = g1(&keys.aggregate, committed(size)) else { return None };
        let mut absent = keys.absent.iter();
        let mut neg = blst_p1::default();
        for (i, b) in bits.iter().enumerate() {
            if !b {
                let Some((index, key)) = absent.next() else { return None };
                if usize::from(*index) != i {
                    return None;
                }
                let Some(p) = g1(key, committed(i)) else { return None };
                let a = neg;
                // SAFETY: valid points; blst writes only `neg`.
                unsafe { blst_p1_add_or_double_affine(&mut neg, &a, &p) };
            }
        }
        if absent.next().is_some() {
            return None;
        }
        let mut acc = blst_p1::default();
        let mut pk = blst_p1_affine::default();
        let mut ng = blst_p1::default();
        let mut ng_a = blst_p1_affine::default();
        let mut s = blst_p2_affine::default();
        let mut hp = blst_p2::default();
        let mut hm = blst_p2_affine::default();
        let mut f = blst_fp12::default();
        // SAFETY: every pointer is to a live local of the type blst expects; `sig` is 96 bytes,
        // `msg` 32, `DST` its own length; the two arrays passed to the Miller loop hold 2 each.
        unsafe {
            blst_p1_cneg(&mut neg, true);
            blst_p1_from_affine(&mut acc, &agg);
            let a = acc;
            blst_p1_add_or_double(&mut acc, &a, &neg);
            blst_p1_to_affine(&mut pk, &acc);
            if blst_p1_affine_is_inf(&pk) {
                return None;
            }
            if blst_p2_uncompress(&mut s, sig.as_ptr()) != BLST_ERROR::BLST_SUCCESS || !blst_p2_affine_in_g2(&s) || blst_p2_affine_is_inf(&s) {
                return None;
            }
            blst_hash_to_g2(&mut hp, msg.as_ptr(), msg.len(), DST.as_ptr(), DST.len(), core::ptr::null(), 0);
            blst_p2_to_affine(&mut hm, &hp);
            blst_p1_from_affine(&mut ng, blst_p1_affine_generator());
            blst_p1_cneg(&mut ng, true);
            blst_p1_to_affine(&mut ng_a, &ng);
            let qs: [*const blst_p2_affine; 2] = [&hm, &s];
            let ps: [*const blst_p1_affine; 2] = [&pk, &ng_a];
            blst_miller_loop_n(&mut f, qs.as_ptr(), ps.as_ptr(), 2);
        }
        Some(f)
    }
}
