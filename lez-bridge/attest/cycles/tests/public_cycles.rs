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

/// The finality update split at the pairing seam (`attest_cycles split`, synthetic committee over
/// the recorded Sepolia update), measured 2026-11-17 (image 4b253ca9…): (signers, one
/// transaction, Begin, Finish).
const SPLIT: [(u64, u64, u64, u64); 5] = [
    (512, 9_224_671, 5_615_063, 3_780_389),
    (441, 10_000_427, 6_391_206, 3_780_389),
    (427, 10_158_667, 6_549_561, 3_780_389),
    (400, 10_452_659, 6_843_102, 3_780_389),
    (342, 11_095_486, 7_485_924, 3_780_389),
];

#[test]
fn the_split_fits_every_participation_down_to_the_floor() {
    let out = Command::new(env!("CARGO_BIN_EXE_attest_cycles")).arg("split").output().expect("runs attest_cycles split");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "attest_cycles split failed:\n{text}");
    let rows: Vec<Vec<u64>> = text
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| signers") && !l.starts_with("| **") && !l.starts_with("| program"))
        .map(|l| l.split('|').filter_map(|c| c.trim().parse().ok()).collect::<Vec<u64>>())
        .filter(|r| r.len() == 4)
        .collect();
    assert_eq!(rows.len(), SPLIT.len(), "{text}");
    for (r, (n, one, begin, finish)) in rows.iter().zip(SPLIT) {
        assert_eq!(r[0], n);
        assert!(r[2] < CAP && r[3] < CAP, "{n}/512: a step at or over the cap");
        for (got, pinned) in [(r[1], one), (r[2], begin), (r[3], finish)] {
            assert!(got.abs_diff(pinned) * 100 <= pinned, "{n}/512: {got} more than 1% from the pinned {pinned}");
        }
    }
}
