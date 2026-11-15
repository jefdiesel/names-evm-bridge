//! G2-lite guest. Reads the input (`outbound_core::update::encode_input`) with one `read_slice`,
//! runs `outbound_core::update::update` with the finality source this build names, verifies the
//! privacy-circuit receipt of every private transaction that reaches the bridge with
//! `env::verify` (resolved by the prover as a composition assumption), and commits
//! `abi.encode(UpdateJournal)` as the raw journal (`commit_slice`), so `sha256(journal)` is the
//! digest `BridgeVault.update` checks. Any refusal panics: no journal.
//!
//! Built twice: `outbound_guest` (release, the Logos Blockchain light client, which is not
//! implemented, so it refuses every batch today) and `outbound_guest_trusted_tip` (the
//! `trusted-tip` feature: the host names the final block; with `unchecked-private`, on in that
//! build only, private transactions are extracted without their privacy receipt, see
//! `outbound_core::update::SequencerTrusted`). Different code, different image ids.

#[cfg(all(feature = "unchecked-private", not(feature = "trusted-tip")))]
compile_error!("unchecked-private belongs to the trusted-tip build only: the light-client build checks every privacy receipt");

#[allow(unused_imports)]
use outbound_core::{
    PRIVACY_PRESERVING_CIRCUIT_ID,
    update::{JOURNAL_VERSION, Meter, ReceiptVerifier, WitnessedAcc, decode_input, update},
};
use risc0_zkvm::{guest::env, sha::Digest};
use std::io::Write as _;

#[cfg(feature = "trusted-tip")]
type Finality = outbound_core::finality::TrustedTip;
#[cfg(not(feature = "trusted-tip"))]
type Finality = outbound_core::finality::LogosBlockchainLightClient;

#[derive(Default)]
#[allow(dead_code)]
struct Zk;

#[cfg(feature = "unchecked-private")]
type Receipts = outbound_core::update::SequencerTrusted;
#[cfg(not(feature = "unchecked-private"))]
type Receipts = Zk;

impl ReceiptVerifier for Zk {
    fn verify(&mut self, journal: &[u8], _proof: &[u8]) -> bool {
        // Adds the assumption ReceiptClaim::ok(circuit, sha256(journal)); a proof of this guest
        // is valid only once a receipt for it resolves the assumption.
        env::verify(Digest::from_bytes(PRIVACY_PRESERVING_CIRCUIT_ID), journal).is_ok()
    }
}

struct Cycles;

impl Meter for Cycles {
    fn cycles(&self) -> u64 {
        env::cycle_count()
    }
}

fn main() {
    let mut len = [0u8; 4];
    env::read_slice(&mut len);
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    env::read_slice(&mut buf);
    let (header, blocks) = decode_input(&buf).unwrap_or_else(|e| panic!("G2 refused: {e}"));
    let mut acc = WitnessedAcc::new(header.prev.claim_acc, header.inserts.clone());
    let input = outbound_core::update::UpdateInput {
        prev: header.prev,
        view: header.view,
        finality: header.finality,
        blocks,
    };
    let mut receipts = Receipts::default();
    let out = update::<Finality>(&input, &mut acc, &mut receipts, &Cycles).unwrap_or_else(|e| panic!("G2 refused: {e}"));
    assert!(acc.leftover() == 0, "G2 refused: unused accumulator witnesses");
    env::commit_slice(&out.journal.abi_encode(JOURNAL_VERSION));
    // Profile for the host's measurements: stdout, not the journal.
    let mut s = out.stats;
    s.per_block.truncate(64);
    let _ = env::stdout().write_all(&borsh::to_vec(&s).unwrap_or_default());
}
