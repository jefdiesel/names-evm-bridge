// SPDX-License-Identifier: MIT OR Apache-2.0
//! The CLI's risc0 is the one the LEZ v0.4.0-rc3 sequencer verifies with.
//!
//! The CLI makes the privacy receipts of private claims and burns; the sequencer accepts a
//! succinct receipt only if its recursion program is under the sequencer's control root. LEZ rc3
//! (its Cargo.lock at bafd07be, and the demo chain's sequencer binary) links
//! risc0-circuit-recursion 4.0.4; 4.1.0 has other control ids. No proving here:
//! - the control root this build links is LEZ rc3's;
//! - a privacy receipt that the public demo chain's rc3 sequencer accepted (the team's private
//!   claim in block 3069, 2026-11-14, `public-demo/prove/run1-20261114/`) passes this build's
//!   native integrity check, and fails it with one seal byte changed. With the 4.1.0 lock this test
//!   fails (checked 2026-11-15 on the build host, `VERSIONS.toml` lez_host).

use common::{block::Block, transaction::LeeTransaction};
use risc0_zkvm::{InnerReceipt, VerifierContext, sha::Digestible};

/// risc0-circuit-recursion 4.0.4 and 4.0.5 `ALLOWED_CONTROL_ROOT` (LEZ rc3's).
const LEZ_RC3_CONTROL_ROOT: &str = "a54dc85ac99f851c92d7c96d7318af41dbe7c0194edfcc37eb4d422a998c1f56";
/// LEZ rc3's privacy circuit (DESIGN.md "LEZ version").
const LEZ_RC3_PRIVACY_CIRCUIT_ID: &str = "631f0c96572a86ce63d1b9c4e98c8383cabb016b3c852017101452b992cf0c38";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn private_receipts(block: &[u8]) -> Vec<Vec<u8>> {
    let block: Block = borsh::from_slice(block).expect("an rc3 block");
    block
        .body
        .transactions
        .iter()
        .filter_map(|tx| match tx {
            LeeTransaction::PrivacyPreserving(tx) => Some(tx.witness_set().proof().clone().into_inner()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_cli_links_lez_rc3s_recursion_circuit() {
    let root = risc0_circuit_recursion::control_id::ALLOWED_CONTROL_ROOT;
    assert_eq!(hex(root.as_bytes()), LEZ_RC3_CONTROL_ROOT, "risc0-circuit-recursion is not LEZ rc3's (4.0.4)");
    let id = risc0_zkvm::sha::Digest::from(lee::PRIVACY_PRESERVING_CIRCUIT_ID);
    assert_eq!(hex(id.as_bytes()), LEZ_RC3_PRIVACY_CIRCUIT_ID);
}

#[test]
fn a_privacy_receipt_the_rc3_sequencer_accepted_verifies_here() {
    let raw = include_bytes!("fixtures/rc3-private-claim-block-3069.bin");
    let receipts = private_receipts(raw);
    assert_eq!(receipts.len(), 1, "block 3069 holds one private transaction");
    let inner: InnerReceipt = borsh::from_slice(&receipts[0]).expect("an InnerReceipt");
    let InnerReceipt::Succinct(succinct) = &inner else { panic!("LEZ rc3 makes succinct receipts") };
    // The receipt is of LEZ rc3's privacy circuit.
    let claim = inner.claim().expect("claim").as_value().expect("unpruned claim").clone();
    assert_eq!(hex(claim.pre.digest().as_bytes()), LEZ_RC3_PRIVACY_CIRCUIT_ID);
    // Seal and control id under this build's control root: what the sequencer checks.
    let ctx = VerifierContext::default();
    inner
        .verify_integrity_with_context(&ctx)
        .unwrap_or_else(|e| panic!("refused by this build's verifier (control id {}): {e}", hex(succinct.control_id.as_bytes())));
    // Control: one seal word changed is refused.
    let mut bad = succinct.clone();
    bad.seal[bad.seal.len() / 2] ^= 1;
    assert!(InnerReceipt::Succinct(bad).verify_integrity_with_context(&ctx).is_err());
}
