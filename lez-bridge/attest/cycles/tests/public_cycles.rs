// SPDX-License-Identifier: MIT OR Apache-2.0
//! EXECUTOR ONLY (build host: the public guest needs RISC Zero's C toolchain; run with
//! `attest/cycles/build-public.sh test`, which is `cargo test --release` here). Not in CI.
//!
//! Pins the user cycles of one whole `UpdatePublic` transaction (submitter's actor + store
//! `Advance` + `RecordAnchor`) on the recorded fixtures under LEZ's 10M cap per transaction, and
//! within 1% of the count measured on 2026-11-17, so a change that makes the public path dearer
//! fails here before it fails on a chain.

use std::process::Command;

const CAP: u64 = 10_000_000;
/// Measured 2026-11-17 (`attest_cycles public`, branch public-update): Sepolia, mainnet.
const MEASURED: [(&str, u64); 2] = [("Sepolia", SEPOLIA), ("mainnet", MAINNET)];
const SEPOLIA: u64 = 9_252_369;
const MAINNET: u64 = 9_530_886;

#[test]
fn a_public_finality_update_fits_one_transaction() {
    let out = Command::new(env!("CARGO_BIN_EXE_attest_cycles")).arg("public").output().expect("runs attest_cycles");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "attest_cycles public failed (over the cap or refused):\n{text}");
    let sums: Vec<u64> = text
        .lines()
        .filter_map(|l| l.strip_prefix("| **whole transaction** | **"))
        .map(|l| l.split("**").next().and_then(|n| n.parse().ok()).expect("a cycle count"))
        .collect();
    assert_eq!(sums.len(), 2, "{text}");
    for ((net, measured), sum) in MEASURED.iter().zip(&sums) {
        assert!(*sum < CAP, "{net}: {sum} user cycles, at or over the 10M cap");
        assert!(sum.abs_diff(*measured) * 100 <= *measured, "{net}: {sum} user cycles, more than 1% from the pinned {measured}");
    }
}
