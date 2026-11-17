// SPDX-License-Identifier: MIT OR Apache-2.0
//! The seq-sig guests, embedded at build time, and their executor runs (never proves).

use anyhow::{Context as _, Result, ensure};
use outbound_host::{Prepared, digest_bytes, sha256};
use risc0_zkvm::{
    ExecutorEnv, ExecutorImpl, ExitCode, MaybePruned, ReceiptClaim,
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

/// Executes a guest with length-prefixed blobs (never proves); checks the claim a proof would
/// carry is `ReceiptClaim::ok(image, sha256(journal))`. A guest that refuses (panics) is an error.
pub fn run_guest(elf: &[u8], image: [u8; 32], blobs: &[&[u8]]) -> Result<GuestRun> {
    let mut stdout = Vec::new();
    let s = {
        let mut b = ExecutorEnv::builder();
        for x in blobs {
            b.write_slice(&(x.len() as u32).to_le_bytes());
            b.write_slice(x);
        }
        b.stdout(&mut stdout);
        let env = b.build()?;
        ExecutorImpl::from_elf(env, elf)?.run()?
    };
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&journal))));
    ensure!(s.claim()?.digest() == want.digest(), "claim does not commit to sha256(journal)");
    Ok(GuestRun { journal, user_cycles: s.user_cycles, total_cycles: s.total_cycles, segments: s.segments.len(), stdout })
}

/// The seq-sig update guest on a prepared G2 input (`outbound_host::prepare`) and a witness.
pub fn execute_update(p: &Prepared, w: &Witness) -> Result<GuestRun> {
    let blob = outbound_core::update::encode_input(&p.header, &p.blocks);
    run_guest(OUTBOUND_GUEST_SEQSIG_ELF, update_image(), &[&blob, &borsh::to_vec(w)?])
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
