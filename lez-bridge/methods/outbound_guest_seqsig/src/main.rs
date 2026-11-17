// SPDX-License-Identifier: MIT OR Apache-2.0
//! G2-lite guest with sequencer signatures (branch `seq-sig`).
//!
//! Input: the TrustedTip guest's input (`u32le(len) | outbound_core::update::encode_input`), then
//! `u32le(len) | borsh(seqsig_core::Witness)`.
//!
//! 1. Exactly what `outbound_guest_trusted_tip` does: `update::<TrustedTip>` (block linkage and
//!    hashes, extraction, the claim accumulator; private transactions on the sequencer's word,
//!    `SequencerTrusted`, as in that build); the batch must end at the block the finality witness
//!    names.
//! 2. Then the checkpoint `(journal cfgId, previous G2 tip, that end block)` must carry valid
//!    Ed25519 signatures from at least `threshold` distinct keys of the signer set that the
//!    witness's ejected mask does not exclude (`seqsig_core::check`; set keys canonical and of
//!    large order, `n/2 < threshold <= n`).
//! 3. Journal: `abi.encode(UpdateJournal)` (what `BridgeVault.update` takes) followed by
//!    `abi.encode(signerSetHash, threshold, ejectedMask, signerBitmap, prevId, prevHash, blockId,
//!    blockHash)` (`seqsig_core::journal_tail`, 256 bytes). The seq-sig gate checks the set hash,
//!    threshold and ejected mask against its own, keeps the bitmap and the checkpoint for the
//!    contest window, verifies the seal over sha256 of the whole journal, and hands the vault only
//!    the first part.
//!
//! Any refusal panics: no journal. Stdout (not the journal): borsh `(u64 signature-check cycles,
//! u32 signatures given, u32 distinct signers)` for the host's measurements.

use outbound_core::{
    finality::{BlockRef, TrustedTip},
    update::{JOURNAL_VERSION, Meter, SequencerTrusted, WitnessedAcc, decode_input, update},
};
use risc0_zkvm::guest::env;
use seqsig_core::{Checkpoint, Witness, check, journal_tail};
use std::io::Write as _;

struct Cycles;

impl Meter for Cycles {
    fn cycles(&self) -> u64 {
        env::cycle_count()
    }
}

fn read_blob() -> Vec<u8> {
    let mut len = [0u8; 4];
    env::read_slice(&mut len);
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    env::read_slice(&mut buf);
    buf
}

fn main() {
    let buf = read_blob();
    let (header, blocks) = decode_input(&buf).unwrap_or_else(|e| panic!("G2 refused: {e}"));
    let tip: BlockRef = borsh::from_slice(&header.finality).unwrap_or_else(|_| panic!("G2 refused: finality witness"));
    let prev_tip = header.prev.tip;
    let mut acc = WitnessedAcc::new(header.prev.claim_acc, header.inserts.clone());
    let input = outbound_core::update::UpdateInput {
        prev: header.prev,
        view: header.view,
        finality: header.finality,
        blocks,
    };
    let out = update::<TrustedTip>(&input, &mut acc, &mut SequencerTrusted, &Cycles).unwrap_or_else(|e| panic!("G2 refused: {e}"));
    assert!(acc.leftover() == 0, "G2 refused: unused accumulator witnesses");

    let w: Witness = borsh::from_slice(&read_blob()).unwrap_or_else(|_| panic!("seq-sig refused: witness does not decode"));
    let cp = Checkpoint {
        cfg_id: out.journal.cfg_id,
        prev_id: prev_tip.id,
        prev_hash: prev_tip.hash,
        block_id: tip.id,
        block_hash: tip.hash,
    };
    let c0 = env::cycle_count();
    let ok = check(&cp, &w).unwrap_or_else(|e| panic!("seq-sig refused: {e}"));
    let c1 = env::cycle_count();

    let mut journal = out.journal.abi_encode(JOURNAL_VERSION);
    journal.extend_from_slice(&journal_tail(&ok, &cp));
    env::commit_slice(&journal);
    let _ = env::stdout().write_all(&borsh::to_vec(&(c1 - c0, w.sigs.len() as u32, ok.signers)).unwrap_or_default());
}
