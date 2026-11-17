// SPDX-License-Identifier: MIT OR Apache-2.0
//! G4 guest: reads `u32le(len) | borsh(ReclaimInput)` with `read_slice`, runs
//! `reclaim_core::reclaim`, and commits the 288-byte `abi.encode(cfgId, depositRoot, finalState,
//! nullifier, token, denom, recipient, relayer, relayFee)` as the raw journal (`commit_slice`), so
//! `sha256(journal)` is the digest `BridgeVaultFailsafe.reclaim` checks. Any refusal panics: no
//! journal.

use risc0_zkvm::guest::env;

fn main() {
    let mut len = [0u8; 4];
    env::read_slice(&mut len);
    let n = u32::from_le_bytes(len) as usize;
    let mut buf = vec![0u8; n.next_multiple_of(4)];
    env::read_slice(&mut buf);
    let input: reclaim_core::ReclaimInput =
        borsh::from_slice(&buf[..n]).unwrap_or_else(|_| panic!("reclaim refused: input does not decode"));
    let journal = reclaim_core::reclaim(&input).unwrap_or_else(|e| panic!("reclaim refused: {e}"));
    env::commit_slice(&journal.abi_encode());
}
