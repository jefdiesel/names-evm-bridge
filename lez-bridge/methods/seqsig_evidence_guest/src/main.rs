// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contest evidence for the seq-sig gate (branch `seq-sig`). Input `u32le(len) |
//! borsh(seqsig_core::Evidence)`: a checkpoint, the signer set and one or more Ed25519 signatures
//! over it. Refuses (panics) unless the set is well formed and every signature verifies
//! (`verify_strict`). Journal (224 bytes): `abi.encode(cfgId, signerSetHash, signerBitmap, prevId,
//! prevHash, blockId, blockHash)`. The gate compares it with a pending update: same cfgId, set,
//! previous tip and block id, another hash. Signers that also signed the update equivocated (they
//! are ejected and the update cancelled); a quorum of active signers halts the gate. Stdout: u64le
//! cycles of the check.

use risc0_zkvm::guest::env;
use seqsig_core::{Evidence, evidence_journal};
use std::io::Write as _;

fn main() {
    let mut len = [0u8; 4];
    env::read_slice(&mut len);
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    env::read_slice(&mut buf);
    let e: Evidence = borsh::from_slice(&buf).unwrap_or_else(|_| panic!("evidence refused: does not decode"));
    let c0 = env::cycle_count();
    let j = evidence_journal(&e).unwrap_or_else(|err| panic!("evidence refused: {err}"));
    let c1 = env::cycle_count();
    env::commit_slice(&j);
    let _ = env::stdout().write_all(&(c1 - c0).to_le_bytes());
}
