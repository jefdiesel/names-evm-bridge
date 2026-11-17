// SPDX-License-Identifier: MIT OR Apache-2.0
//! EXECUTOR ONLY (build host: the public guest needs RISC Zero's C toolchain; run with
//! `attest/cycles/build-public.sh test`, which is `cargo test --release` here). Not in CI.
//!
//! Pins the user cycles of every public transaction on the recorded fixtures (finality updates,
//! and the committee handoff: period Begin and Finish, the rotation) under LEZ's 10M cap per
//! transaction, and within 1% of the counts measured on 2026-11-17, so a change that makes the public path dearer
//! fails here before it fails on a chain.

use std::process::Command;

const CAP: u64 = 10_000_000;
/// Measured 2026-11-17 (`attest_cycles public`, branch public-update, image bf0dc638…, after the review fixes), in the
/// order `attest_cycles public` prints them: finality (Sepolia, mainnet), then the fixture handoff.
const MEASURED: [(&str, u64); 7] = [
    ("Sepolia finality", 9_256_285),
    ("mainnet finality", 9_537_290),
    ("period 1410 Begin", 3_194_395),
    ("period 1410 Finish", 9_319_158),
    ("rotation into 1411", 9_315_028),
    ("period 1411 Begin", 3_193_398),
    ("period 1411 Finish", 9_319_338),
];

#[test]
fn every_public_transaction_fits_and_matches_the_pinned_count() {
    let out = Command::new(env!("CARGO_BIN_EXE_attest_cycles")).arg("public").output().expect("runs attest_cycles");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "attest_cycles public failed (over the cap or refused):\n{text}");
    let sums: Vec<u64> = text
        .lines()
        .filter_map(|l| l.strip_prefix("| **whole transaction** | **"))
        .map(|l| l.split("**").next().and_then(|n| n.parse().ok()).expect("a cycle count"))
        .collect();
    assert_eq!(sums.len(), MEASURED.len(), "{text}");
    for ((net, measured), sum) in MEASURED.iter().zip(&sums) {
        assert!(*sum < CAP, "{net}: {sum} user cycles, at or over the 10M cap");
        assert!(sum.abs_diff(*measured) * 100 <= *measured, "{net}: {sum} user cycles, more than 1% from the pinned {measured}");
    }
}
