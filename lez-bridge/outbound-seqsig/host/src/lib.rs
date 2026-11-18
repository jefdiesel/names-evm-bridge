// SPDX-License-Identifier: MIT OR Apache-2.0
//! The seq-sig guests, embedded at build time, their executor runs, and (branch `v7-seqsig`) the
//! OWNER-STARTED proving path: [`prove_guest`], the in-process CPU prover only, refused unless
//! `outbound_host::prove_gate_env` passes (no r0vm, no dev mode, `RISC0_PROVER=local`,
//! `RAYON_NUM_THREADS` 1..=4); a succinct receipt checked natively before it is returned. Tests and
//! agents never call it: only `public-demo/prove/prove-seqsig-rc3.sh` (through `seqsig update
//! --prove` / `seqsig evidence --prove`).

use anyhow::{Context as _, Result, ensure};
use outbound_host::{Guest, Prepared, Proved, check_receipt, digest_bytes, prove_gate_env, sha256};
use risc0_zkvm::{
    ExecutorEnv, ExecutorImpl, ExitCode, LocalProver, MaybePruned, Prover as _, ProverOpts, ReceiptClaim,
    sha::{Digest, Digestible as _},
};
use seqsig_core::{Checkpoint, Witness};
use std::{io::Write as _, path::Path};

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

/// The seq-sig G2 update image (a vault's `updateImageId` behind a SeqSigGate).
#[must_use]
pub fn update_image() -> [u8; 32] {
    digest_bytes(OUTBOUND_GUEST_SEQSIG_ID)
}

/// The contest evidence image (the gate's `EVIDENCE_IMAGE_ID`): signatures of set keys over one
/// checkpoint; equivocation when those keys also signed the pending update.
#[must_use]
pub fn evidence_image() -> [u8; 32] {
    digest_bytes(SEQSIG_EVIDENCE_GUEST_ID)
}

pub struct GuestRun {
    pub journal: Vec<u8>,
    pub user_cycles: u64,
    pub total_cycles: u64,
    pub segments: usize,
    pub stdout: Vec<u8>,
}

/// The embedded seq-sig update guest.
pub fn update_guest() -> Guest {
    Guest { elf: OUTBOUND_GUEST_SEQSIG_ELF.to_vec(), image: update_image(), source: "embedded".into() }
}

/// The embedded evidence guest.
pub fn evidence_guest() -> Guest {
    Guest { elf: SEQSIG_EVIDENCE_GUEST_ELF.to_vec(), image: evidence_image(), source: "embedded".into() }
}

fn guest_env<'a>(blobs: &[&[u8]], po2: Option<u32>, stdout: &'a mut Vec<u8>) -> Result<ExecutorEnv<'a>> {
    let mut b = ExecutorEnv::builder();
    if let Some(n) = po2 {
        ensure!((15..=22).contains(&n), "segment po2 {n} outside 15..=22");
        b.segment_limit_po2(n);
    }
    for x in blobs {
        b.write_slice(&(x.len() as u32).to_le_bytes());
        b.write_slice(x);
    }
    b.stdout(stdout);
    b.build()
}

/// Executes a guest with length-prefixed blobs (never proves); checks the claim a proof would
/// carry is `ReceiptClaim::ok(image, sha256(journal))`. A guest that refuses (panics) is an error.
pub fn run_guest(elf: &[u8], image: [u8; 32], blobs: &[&[u8]]) -> Result<GuestRun> {
    run_guest_po2(elf, image, blobs, None)
}

/// [`run_guest`] with a segment size (2^po2 cycles, the prover's segments).
pub fn run_guest_po2(elf: &[u8], image: [u8; 32], blobs: &[&[u8]], po2: Option<u32>) -> Result<GuestRun> {
    let mut stdout = Vec::new();
    let s = {
        let env = guest_env(blobs, po2, &mut stdout)?;
        ExecutorImpl::from_elf(env, elf)?.run()?
    };
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&journal))));
    ensure!(s.claim()?.digest() == want.digest(), "claim does not commit to sha256(journal)");
    Ok(GuestRun { journal, user_cycles: s.user_cycles, total_cycles: s.total_cycles, segments: s.segments.len(), stdout })
}

/// The seq-sig update guest (embedded) on a prepared G2 input (`outbound_host::prepare`) and a witness.
pub fn execute_update(p: &Prepared, w: &Witness) -> Result<GuestRun> {
    execute_update_with(&update_guest(), p, w, None)
}

/// The two input blobs of the seq-sig update guest.
pub fn update_blobs(p: &Prepared, w: &Witness) -> Result<(Vec<u8>, Vec<u8>)> {
    Ok((outbound_core::update::encode_input(&p.header, &p.blocks), borsh::to_vec(w)?))
}

/// A seq-sig update guest `g` (embedded, or a canonical ELF from `Guest::from_file`) in the executor.
pub fn execute_update_with(g: &Guest, p: &Prepared, w: &Witness, po2: Option<u32>) -> Result<GuestRun> {
    let (a, b) = update_blobs(p, w)?;
    run_guest_po2(&g.elf, g.image, &[&a, &b], po2)
}

/// OWNER-STARTED: proves guest `g` on `blobs` (succinct receipt, in-process CPU prover), checks it
/// natively against `g.image` and, with `journal`, that its journal is those bytes.
pub fn prove_guest(g: &Guest, blobs: &[&[u8]], po2: Option<u32>, journal: Option<&[u8]>) -> Result<Proved> {
    prove_gate_env()?;
    let mut stdout = Vec::new();
    let env = guest_env(blobs, po2, &mut stdout)?;
    let opts = ProverOpts::succinct().with_dev_mode(false);
    let t = std::time::Instant::now();
    let info = LocalProver::new("local").prove_with_opts(env, &g.elf, &opts)?;
    let secs = t.elapsed().as_secs_f64();
    let kind = check_receipt(&info.receipt, g.image, journal)?;
    Ok(Proved { kind, user_cycles: info.stats.user_cycles, total_cycles: info.stats.total_cycles, segments: info.stats.segments, secs, receipt: info.receipt })
}

/// The attestor's append-only signing record, one line per checkpoint it signed:
/// `cfgId prevId prevHash blockId blockHash` (0x-hex and decimal). Before it signs, the attestor
/// calls this: a checkpoint with the same (cfgId, prevId, prevHash, blockId) and ANOTHER block hash
/// is refused (signing it would be equivocation); the same hash again returns `Ok(false)` (already
/// recorded); a new key is appended and flushed to disk before the signature exists (`Ok(true)`).
pub fn record_and_check(path: &Path, cp: &Checkpoint) -> Result<bool> {
    let key = format!("{} {} {} {}", outbound_host::h0x(&cp.cfg_id), cp.prev_id, outbound_host::h0x(&cp.prev_hash), cp.block_id);
    let hash = outbound_host::h0x(&cp.block_hash);
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let Some((k, h)) = line.rsplit_once(' ') else { continue };
            if k == key {
                ensure!(h == hash, "refusing to sign: this key already signed block {} after ({}, {}) with hash {h}, not {hash}", cp.block_id, cp.prev_id, outbound_host::h0x(&cp.prev_hash));
                return Ok(false);
            }
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).with_context(|| format!("signing record {}", path.display()))?;
    writeln!(f, "{key} {hash}")?;
    f.sync_all()?;
    Ok(true)
}
