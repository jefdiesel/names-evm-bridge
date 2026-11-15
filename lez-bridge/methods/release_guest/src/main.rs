//! G3 guest: reads the fixed-layout `ReleaseInput` (`read_slice`, one read instead of risc0
//! serde's word per byte), runs `release_core::release`, and commits the 256-byte
//! `abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)` as the raw
//! journal (`commit_slice`, not the word-serialised `commit`), so `sha256(journal)` is exactly
//! the digest `BridgeVault.release` passes to the verifier. Any error panics: no journal.

use release_core::{INPUT_LEN, ReleaseInput};
use risc0_zkvm::guest::env;

fn main() {
    let mut buf = [0u8; INPUT_LEN];
    env::read_slice(&mut buf);
    let input = ReleaseInput::from_bytes(&buf);
    let journal = release_core::release(&input).unwrap_or_else(|e| panic!("release refused: {e}"));
    env::commit_slice(&journal.abi_encode());
}
