// SPDX-License-Identifier: MIT OR Apache-2.0
//! Participation sweep of the public update path, EXECUTOR ONLY (branch `public-update`).
//!
//!   attest_cycles sweep
//!
//! The recorded Sepolia finality update (`attest/fixtures/sepolia/finality_update.ssz`: attested and
//! finalized headers, finality branch, signature slot) is kept byte for byte; only the sync
//! committee, the participation bits and the aggregate signature are SYNTHETIC. The committee is
//! 512 keys from fixed public seeds (`SHA256("RFP021/synthetic-committee/v1" ‖ i)`; test keys,
//! nothing secret), and the store's committee commitment is that committee's hash: a finality
//! update carries no committee branch (only the bootstrap does), so nothing in the recorded
//! headers or branches changes. For each N the update is signed by exactly N members over the
//! signing root Helios itself computes (taken through the hook), checked natively by the program's
//! own `verify_finality_public`, then run in the executor on the public guest: submitter's actor,
//! store `Advance`, `RecordAnchor`, summed as one transaction.
//!
//! The two-step split is COMPOSED, not built: Begin = submitter's actor minus blst's final
//! exponentiation + `is_one` (3,581,137 cycles, measured with the same blst fork in
//! `attest/lcbench/results/run-20261114T175725Z.txt`, "blst P2: final exponentiation"); Finish =
//! that final exponentiation plus the store's `Advance` and `RecordAnchor`. The job account's own
//! write (Begin) and read (Finish) are not included.

use super::*;
use alloy_primitives::B256;
use blst::min_pk::{AggregatePublicKey, AggregateSignature, SecretKey};
use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec as S2,
    types::{BeaconBlockHeader, FinalityUpdate, Fork as HFork, Forks, LightClientHeader, LightClientHeaderBellatrix, LightClientStore, SyncCommittee},
    verify_generic_update_with,
};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
/// blst final exponentiation + is_one, same fork, measured 2026-11-14 (see the module docs).
const FINAL_EXP: u64 = 3_581_137;
/// Offsets in the Gloas finality update SSZ (1,448 bytes): sync_committee_bits, signature.
const BITS_AT: usize = 1280;
const SIG_AT: usize = 1344;

fn secret(i: u32) -> SecretKey {
    let ikm = eth_attest_core::sha256(&[b"RFP021/synthetic-committee/v1", &i.to_le_bytes()]);
    SecretKey::key_gen(&ikm, &[]).expect("32-byte ikm")
}

/// The signing root Helios computes for this update against this store (through the hook).
fn signing_root(cfg: &InstanceConfig, prev: &LcSummary, update: &[u8]) -> Result<[u8; 32]> {
    let k = |i: usize| HFork { epoch: cfg.forks[i].epoch, fork_version: cfg.forks[i].version.into() };
    let forks = Forks { genesis: k(0), altair: k(1), bellatrix: k(2), capella: k(3), deneb: k(4), electra: k(5), fulu: k(6), gloas: k(7) };
    let b = &prev.finalized.beacon;
    let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix {
        beacon: BeaconBlockHeader {
            slot: b.slot,
            proposer_index: b.proposer_index,
            parent_root: B256::from(b.parent_root),
            state_root: B256::from(b.state_root),
            body_root: B256::from(b.body_root),
        },
    });
    let store = LightClientStore::<S2> {
        finalized_header: header.clone(),
        current_sync_committee: SyncCommittee::<S2>::default(),
        next_sync_committee: None,
        optimistic_header: header,
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
        best_valid_update: None,
    };
    let u = FinalityUpdate::<S2>::from_ssz_bytes(update).map_err(|e| anyhow!("{e:?}"))?;
    let g = helios_consensus_core::types::GenericUpdate::from(&u);
    let mut root = None;
    verify_generic_update_with::<S2, _>(&g, g.signature_slot, &store, B256::from(cfg.genesis_validators_root), &forks, |_, _, r, _| {
        root = Some(r.0);
        Ok(true)
    })
    .map_err(|e| anyhow!("{e}"))?;
    root.context("no signing root")
}

pub fn run() -> Result<bool> {
    println!("# participation sweep, public update path, SYNTHETIC committee over the recorded Sepolia finality update");
    let cfg = config("sepolia", "bootstrap_same_period");
    let boot = fx("sepolia", "bootstrap_same_period.ssz");
    let genesis = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg, &boot).map_err(|e| anyhow!(e))?;
    let fin = fx("sepolia", "finality_update.ssz");
    let sks: Vec<SecretKey> = (0..512).map(secret).collect();
    let pks: Vec<_> = sks.iter().map(SecretKey::sk_to_pk).collect();
    let refs: Vec<_> = pks.iter().collect();
    let agg = AggregatePublicKey::aggregate(&refs, false).map_err(|e| anyhow!("{e:?}"))?.to_public_key();
    let mut committee = Vec::with_capacity(513 * 48);
    for pk in &pks {
        committee.extend_from_slice(&pk.compress());
    }
    committee.extend_from_slice(&agg.compress());
    let prev = LcSummary { current_committee: committee_hash(&committee), ..genesis };
    let msg = signing_root(&cfg, &prev, &fin)?;
    println!("  signing root 0x{} (Helios, through the hook); committee hash 0x{}", hex::encode(msg), hex::encode(prev.current_committee));
    let mut rows = Vec::new();
    for n in [512usize, 506, 480, 469, 450, 445, 443, 442, 441, 440, 430, 427, 420, 400, 342] {
        let absent = 512 - n;
        let off: Vec<usize> = (0..absent).map(|k| k * 512 / absent.max(1)).collect();
        let mut bits = [0xFFu8; 64];
        for &i in &off {
            bits[i / 8] &= !(1 << (i % 8));
        }
        let sigs: Vec<_> = (0..512).filter(|i| !off.contains(i)).map(|i| sks[i].sign(&msg, DST, &[])).collect();
        let srefs: Vec<_> = sigs.iter().collect();
        let sig = AggregateSignature::aggregate(&srefs, false).map_err(|e| anyhow!("{e:?}"))?.to_signature().compress();
        let mut upd = fin.clone();
        upd[BITS_AT..SIG_AT].copy_from_slice(&bits);
        upd[SIG_AT..SIG_AT + 96].copy_from_slice(&sig);
        let (tx, _) = public::run_tx(&format!("N={n}"), ATTEST, BOB, &cfg, &prev, &committee, &upd)?;
        rows.push((n, tx.calls[0].1, tx.user(), tx.user() - tx.calls[0].1));
    }
    println!("\n### Participation sweep (synthetic committee; recorded Sepolia headers and branches)");
    println!("| signers N/512 | submitter's actor | whole transaction | of 10M | fits | Begin (composed) | Finish (composed) |");
    println!("|---:|---:|---:|---:|---|---:|---:|");
    let mut lowest = None;
    for (n, actor, whole, rest) in &rows {
        let fits = *whole < CAP as u64;
        if fits {
            lowest = Some(*n);
        }
        println!(
            "| {n} | {actor} | {whole} | {:.2}% | {} | {} | {} |",
            *whole as f64 / CAP * 100.0,
            if fits { "yes" } else { "NO" },
            actor - FINAL_EXP,
            FINAL_EXP + rest
        );
    }
    println!("lowest N in this sweep that fits one transaction: {}", lowest.map_or("none".into(), |n| n.to_string()));
    println!("(Begin = submitter's actor - {FINAL_EXP} final exponentiation; Finish = {FINAL_EXP} + Advance + RecordAnchor; job account write/read not included)");
    Ok(true)
}

/// The synthetic committee, its store and the update signed by `n` of its members (evenly spread
/// non-signers), over the recorded Sepolia finality update. Program `program` (instance ids depend
/// on it), config salt `salt`.
pub struct Synthetic {
    pub cfg: InstanceConfig,
    pub prev: LcSummary,
    pub committee: Vec<u8>,
    sks: Vec<SecretKey>,
    msg: [u8; 32],
    fin: Vec<u8>,
}

impl Synthetic {
    pub fn new(program: AccountId, salt: [u8; 32]) -> Result<Self> {
        let cfg = InstanceConfig { salt, ..config("sepolia", "bootstrap_same_period") };
        let boot = fx("sepolia", "bootstrap_same_period.ssz");
        let genesis = eth_attest::lc::verify_bootstrap(&program, &cfg, &boot).map_err(|e| anyhow!(e))?;
        let fin = fx("sepolia", "finality_update.ssz");
        let sks: Vec<SecretKey> = (0..512).map(secret).collect();
        let pks: Vec<_> = sks.iter().map(SecretKey::sk_to_pk).collect();
        let refs: Vec<_> = pks.iter().collect();
        let agg = AggregatePublicKey::aggregate(&refs, false).map_err(|e| anyhow!("{e:?}"))?.to_public_key();
        let mut committee: Vec<u8> = pks.iter().flat_map(|p| p.compress()).collect();
        committee.extend_from_slice(&agg.compress());
        let prev = LcSummary { current_committee: committee_hash(&committee), ..genesis };
        let msg = signing_root(&cfg, &prev, &fin)?;
        Ok(Self { cfg, prev, committee, sks, msg, fin })
    }

    pub fn update(&self, n: usize) -> Result<Vec<u8>> {
        let absent = 512 - n;
        let off: Vec<usize> = (0..absent).map(|k| k * 512 / absent.max(1)).collect();
        let mut bits = [0xFFu8; 64];
        for &i in &off {
            bits[i / 8] &= !(1 << (i % 8));
        }
        let sigs: Vec<_> = (0..512).filter(|i| !off.contains(i)).map(|i| self.sks[i].sign(&self.msg, DST, &[])).collect();
        let srefs: Vec<_> = sigs.iter().collect();
        let sig = AggregateSignature::aggregate(&srefs, false).map_err(|e| anyhow!("{e:?}"))?.to_signature().compress();
        let mut upd = self.fin.clone();
        upd[BITS_AT..SIG_AT].copy_from_slice(&bits);
        upd[SIG_AT..SIG_AT + 96].copy_from_slice(&sig);
        Ok(upd)
    }
}

/// `attest_cycles split`: the finality update split at the pairing seam, measured on the
/// synthetic committee (executor), against the one-transaction path at the same participation.
pub fn split() -> Result<bool> {
    println!("# finality update split at the pairing seam (FinalityBegin / FinalityFinish), SYNTHETIC committee");
    let s = Synthetic::new(ATTEST, [0; 32])?;
    let mut ok = true;
    println!("\n| signers N/512 | one transaction | Begin | Finish |\n|---:|---:|---:|---:|");
    let mut rows = Vec::new();
    for n in [512usize, 441, 427, 400, 342] {
        let upd = s.update(n)?;
        let (one, _) = public::run_tx(&format!("one tx N={n}"), ATTEST, BOB, &s.cfg, &s.prev, &s.committee, &upd)?;
        let (b, f, _, _) = public::run_split(&format!("split N={n}"), ATTEST_GUEST_PUBLIC_ELF, ATTEST, BOB, &s.cfg, &s.prev, &s.committee, &upd)?;
        ok &= b.user() < CAP as u64 && f.user() < CAP as u64;
        rows.push((n, one.user(), b, f));
    }
    for (n, one, b, f) in &rows {
        println!("| {n} | {one} | {} | {} |", b.user(), f.user());
    }
    for (n, _, b, f) in &rows {
        public::report(&format!("split N={n}, Begin"), b);
        public::report(&format!("split N={n}, Finish"), f);
    }
    Ok(ok)
}

/// `attest_cycles synth-chain PROGRAM SIGNER OUT`: the messages for a run on a chain with the
/// SYNTHETIC test image (`attest_guest_synthetic`, `InitSynthetic`): per N in 427 and 342, an
/// instance (salt = N) created with the synthetic committee, the split's Begin and Finish; and a
/// Begin at 341 signers (refused). Prints the accounts each transaction touches.
pub fn synth_chain(args: &[String]) -> Result<bool> {
    let [program, signer, out] = args else { return Err(anyhow!("synth-chain PROGRAM SIGNER OUT")) };
    let program: AccountId = program.parse().map_err(|_| anyhow!("program"))?;
    let signer: AccountId = signer.parse().map_err(|_| anyhow!("signer"))?;
    let out = Path::new(out);
    std::fs::create_dir_all(out)?;
    for n in [427usize, 342] {
        let s = Synthetic::new(program, [u8::try_from(n % 256)?; 32])?;
        let id = s.prev.instance_id;
        let init = msg(&Message::Instruction(Instruction::InitSynthetic { config: s.cfg.clone(), genesis: s.prev.clone() }));
        std::fs::write(out.join(format!("n{n}-init.hex")), hex::encode(&init))?;
        let anchors: Vec<String> = s.prev.finalized.anchors().iter().map(|a| anchor_address(&program, &id, a).to_string()).collect();
        println!("N={n}: instance 0x{} config {} store {} genesis anchor {}", hex::encode(id), eth_attest_core::config_address(&program, &id), store_address(&program, &id), anchors.join(","));
        let upd = s.update(n)?;
        let (b, f, mb, mf) = public::run_split(&format!("chain N={n}"), ATTEST_GUEST_SYNTHETIC_ELF, program, signer, &s.cfg, &s.prev, &s.committee, &upd)?;
        std::fs::write(out.join(format!("n{n}-begin.hex")), hex::encode(&mb))?;
        std::fs::write(out.join(format!("n{n}-finish.hex")), hex::encode(&mf))?;
        let job = eth_attest::public::verify_finality_begin(&program, &s.cfg, &s.prev, &s.committee, &eth_attest::public::key_witnesses(&s.committee, &eth_attest::public::finality_bits(&upd).map_err(|e| anyhow!(e))?).map_err(|e| anyhow!(e))?, &upd).map_err(|e| anyhow!(e))?;
        let jid = eth_attest_core::job_id(&job.prev, &job.update_digest);
        let new_anchors: Vec<String> = job.next.finalized.anchors().iter().map(|a| anchor_address(&program, &id, a).to_string()).collect();
        println!("N={n}: Begin {} cycles, Finish {} cycles; job 0x{} PDA {}; new anchor {}", b.user(), f.user(), hex::encode(jid), eth_attest_core::job_address(&program, &jid), new_anchors.join(","));
        if n == 342 {
            let low = s.update(341)?;
            let bits = eth_attest::public::finality_bits(&low).map_err(|e| anyhow!(e))?;
            let keys = eth_attest::public::key_witnesses(&s.committee, &bits).map_err(|e| anyhow!(e))?;
            let m = msg(&Message::Instruction(Instruction::FinalityBegin { config: s.cfg.clone(), prev: s.prev.clone(), committee: s.committee.clone(), keys, update: low }));
            std::fs::write(out.join("n341-begin.hex"), hex::encode(&m))?;
            let low_job = eth_attest_core::job_id(&s.prev, &eth_attest_core::sha256(&[&s.update(341)?]));
            println!("N=341: Begin message written (its job PDA would be {})", eth_attest_core::job_address(&program, &low_job));
        }
    }
    Ok(true)
}
