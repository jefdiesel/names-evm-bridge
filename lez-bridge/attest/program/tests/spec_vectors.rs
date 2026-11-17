// SPDX-License-Identifier: MIT OR Apache-2.0
//! The official consensus-specs light-client test vectors (ethereum/consensus-specs releases,
//! `consensus-spec-tests`, release v1.6.1, CC0-1.0), run against the program's verification path (branch
//! `public-update`). Not vendored: point `CONSENSUS_SPEC_TESTS` at an extracted release (the
//! directory holding `tests/`); without it this test only says it was skipped.
//!
//! Applicable: `light_client/sync` (minimal preset; the mainnet preset ships no sync cases). Each
//! case's bootstrap is verified against its trusted block root with Helios's `verify_bootstrap`
//! (as `Init` does), then every `process_update` runs at the step's own `current_slot` through
//! three paths:
//! 0. Helios's `verify_generic_update` (its own signature check) and `apply_update`, with the
//!    program's rule that an update must advance the store: the private path's composition;
//! 1. Helios's `verify_generic_update_with` (the vendored hook) with the program's blst check
//!    (aggregate minus non-signers from uncompressed key witnesses, `fast::verify`): `UpdatePublic`;
//! 2. the same hook with the check split at the pairing seam (`fast::miller`, the canonical
//!    576-byte value, `fast::final_is_one`): `FinalityBegin`/`FinalityFinish`;
//!
//! and the finalized header after the step must be the vector's (slot and root). Every update that
//! applied is checked again with one participation bit cleared, and every path must refuse it.
//!
//! What this is NOT: the program's instruction handlers, its weak-subjectivity rule, its
//! participation policy or the job PDAs; those run on real Sepolia data in the other tests and
//! records. The preset is minimal (32-member committee, `MinimalConsensusSpec`); the program's
//! images use the mainnet preset (512), the same code at another size.
//!
//! Counted as not applicable, with the reason: Altair and Bellatrix cases (no execution header in
//! their light-client headers; Helios and the program take Capella and later), the optimistic
//! header (the program keeps none), steps after a `force_update` (the program has none: an update
//! must verify), `update_ranking` (the program applies the first valid update and never ranks),
//! `single_merkle_proof` (its root is the hash of a whole `BeaconState`/`BeaconBlockBody`, types
//! neither Helios nor the program has; the same four branches, finality, current and next
//! committee and execution payload, are checked inside every bootstrap and update run here),
//! `data_collection` (a full node's side).
#![cfg(feature = "public-update")]

use std::path::{Path, PathBuf};

use alloy_primitives::B256;
use helios_consensus_core::{
    apply_bootstrap, apply_update,
    consensus_spec::MinimalConsensusSpec as M,
    types::{Bootstrap, Fork, Forks, GenericUpdate, LightClientStore, Update},
    verify_bootstrap, verify_generic_update, verify_generic_update_with,
};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

fn snappy(path: &Path) -> Vec<u8> {
    let raw = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    snap::raw::Decoder::new().decompress_vec(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn yaml(path: &Path) -> serde_yaml::Value {
    serde_yaml::from_slice(&std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn b256(v: &serde_yaml::Value) -> B256 {
    let s = v.as_str().expect("hex string");
    B256::from_slice(&hex::decode(s.trim_start_matches("0x")).unwrap())
}

fn forks(cfg: &serde_yaml::Value) -> Forks {
    let version = |k: &str| -> [u8; 4] {
        match &cfg[k] {
            serde_yaml::Value::String(s) => hex::decode(s.trim_start_matches("0x")).unwrap().try_into().unwrap(),
            serde_yaml::Value::Number(n) => (n.as_u64().unwrap() as u32).to_be_bytes(),
            _ => [0xff; 4],
        }
    };
    let epoch = |k: &str| cfg[k].as_u64().unwrap_or(u64::MAX);
    let f = |e: &str, v: &str| Fork { epoch: epoch(e), fork_version: version(v).into() };
    Forks {
        genesis: Fork { epoch: 0, fork_version: version("GENESIS_FORK_VERSION").into() },
        altair: f("ALTAIR_FORK_EPOCH", "ALTAIR_FORK_VERSION"),
        bellatrix: f("BELLATRIX_FORK_EPOCH", "BELLATRIX_FORK_VERSION"),
        capella: f("CAPELLA_FORK_EPOCH", "CAPELLA_FORK_VERSION"),
        deneb: f("DENEB_FORK_EPOCH", "DENEB_FORK_VERSION"),
        electra: f("ELECTRA_FORK_EPOCH", "ELECTRA_FORK_VERSION"),
        fulu: f("FULU_FORK_EPOCH", "FULU_FORK_VERSION"),
        gloas: f("GLOAS_FORK_EPOCH", "GLOAS_FORK_VERSION"),
    }
}

#[derive(Default, Debug)]
struct Tally {
    cases: usize,
    /// `process_update` steps run (each through all three paths).
    steps: usize,
    steps_pass: usize,
    steps_fail: Vec<String>,
    optimistic_na: usize,
    after_force_na: usize,
    upgrade_noop: usize,
    case_na: Vec<String>,
    /// Checks where the update verified and advanced the store (the rest: the vector expects the
    /// store unchanged and the program refused or ignored the update).
    applied: usize,
    /// Applied updates re-checked with one participation bit cleared: refused (must equal the
    /// number re-checked).
    tamper_checked: usize,
    tamper_refused: usize,
}

/// Whether `g` verifies against `store` at the vector's `current_slot` through `path` (0: Helios's
/// own signature check, as the private path; 1: the program's blst check with key witnesses, as
/// `UpdatePublic`; 2: the same split at the pairing seam, `fast::miller` then `fast::final_is_one`
/// across the canonical 576-byte value, as `FinalityBegin`/`FinalityFinish`).
fn verifies(path: u8, store: &LightClientStore<M>, g: &GenericUpdate<M>, slot: u64, gvr: B256, forks: &Forks) -> bool {
    match path {
        0 => verify_generic_update(g, slot, store, gvr, forks).is_ok(),
        _ => verify_generic_update_with::<M, _>(g, slot, store, gvr, forks, |committee, bits, root, sig| {
            let committee = committee.as_ssz_bytes();
            let bits: Vec<bool> = bits.iter().collect();
            let Ok(keys) = eth_attest::public::key_witnesses(&committee, &bits) else { return Ok(false) };
            let Ok(sig) = <[u8; 96]>::try_from(sig.as_ssz_bytes()) else { return Ok(false) };
            Ok(eth_attest::public::fast::verify(&committee, &keys, &bits, &root.0, &sig))
        })
        .is_ok(),
        _ => verify_generic_update_with::<M, _>(g, slot, store, gvr, forks, |committee, bits, root, sig| {
            let committee = committee.as_ssz_bytes();
            let bits: Vec<bool> = bits.iter().collect();
            let Ok(keys) = eth_attest::public::key_witnesses(&committee, &bits) else { return Ok(false) };
            let Ok(sig) = <[u8; 96]>::try_from(sig.as_ssz_bytes()) else { return Ok(false) };
            let Some(f) = eth_attest::public::fast::miller(&committee, &keys, &bits, &root.0, &sig) else { return Ok(false) };
            Ok(eth_attest::public::fast::final_is_one(&f))
        })
        .is_ok(),
    }
}

/// One `process_update` through `path`: the store after it, and whether it applied (the program
/// refuses an update that does not advance: the store is then unchanged).
fn step(path: u8, store: &LightClientStore<M>, u: &Update<M>, slot: u64, gvr: B256, forks: &Forks) -> (LightClientStore<M>, bool) {
    if !verifies(path, store, &GenericUpdate::from(u), slot, gvr, forks) {
        return (store.clone(), false);
    }
    let mut next = store.clone();
    apply_update(&mut next, u);
    let advanced = next.finalized_header.beacon().slot > store.finalized_header.beacon().slot
        || (store.next_sync_committee.is_none() && next.next_sync_committee.is_some());
    if advanced { (next, true) } else { (store.clone(), false) }
}

/// The same update with its first signer's participation bit cleared: the aggregate key no
/// longer matches the signature, so every path must refuse it.
fn tampered(u: &Update<M>) -> GenericUpdate<M> {
    let mut g = GenericUpdate::from(u);
    let bits = &mut g.sync_aggregate.sync_committee_bits;
    let i = (0..bits.len()).find(|&i| bits.get(i).unwrap_or(false)).expect("an update with a signer");
    bits.set(i, false).unwrap();
    g
}

fn run_case(dir: &Path, t: &mut Tally) {
    let name = dir.strip_prefix(dir.ancestors().nth(5).unwrap()).unwrap().display().to_string();
    let cfg = yaml(&dir.join("config.yaml"));
    let meta = yaml(&dir.join("meta.yaml"));
    let steps = yaml(&dir.join("steps.yaml"));
    let forks = forks(&cfg);
    let gvr = b256(&meta["genesis_validators_root"]);
    let fork = dir.ancestors().nth(4).unwrap().file_name().unwrap().to_string_lossy().to_string();
    let Ok(boot) = Bootstrap::<M>::from_ssz_bytes(&snappy(&dir.join("bootstrap.ssz_snappy"))) else {
        // Altair and Bellatrix light-client headers carry no execution payload header; Helios
        // (and so the program) takes Capella and later only. Sepolia and mainnet are past Electra.
        if fork == "altair" || fork == "bellatrix" {
            t.case_na.push(format!("{name}: pre-Capella bootstrap"));
        } else {
            t.steps_fail.push(format!("{name}: bootstrap does not decode"));
        }
        return;
    };
    if verify_bootstrap(&boot, b256(&meta["trusted_block_root"]), &forks).is_err() {
        t.steps_fail.push(format!("{name}: bootstrap refused"));
        return;
    }
    t.cases += 1;
    let mut base = LightClientStore::<M>::default();
    apply_bootstrap(&mut base, &boot);
    let mut stores = [base.clone(), base.clone(), base];
    let mut forced = false;
    for (i, st) in steps.as_sequence().unwrap().iter().enumerate() {
        if st.get("force_update").is_some() {
            forced = true;
            continue;
        }
        if st.get("upgrade_store").is_some() {
            t.upgrade_noop += 1;
            continue;
        }
        let Some(pu) = st.get("process_update") else { continue };
        if forced {
            t.after_force_na += 1;
            continue;
        }
        let file = pu["update"].as_str().unwrap().trim();
        let Ok(u) = Update::<M>::from_ssz_bytes(&snappy(&dir.join(format!("{file}.ssz_snappy")))) else {
            t.steps_fail.push(format!("{name} step {i}: update does not decode"));
            continue;
        };
        let now = pu["current_slot"].as_u64().expect("current_slot");
        t.steps += 1;
        let want_slot = pu["checks"]["finalized_header"]["slot"].as_u64().unwrap();
        let want_root = b256(&pu["checks"]["finalized_header"]["beacon_root"]);
        for (path, store) in stores.iter_mut().enumerate() {
            let p = u8::try_from(path).unwrap();
            let before = store.clone();
            let (next, applied) = step(p, store, &u, now, gvr, &forks);
            *store = next;
            t.applied += usize::from(applied);
            if applied {
                t.tamper_checked += 1;
                t.tamper_refused += usize::from(!verifies(p, &before, &tampered(&u), now, gvr, &forks));
            }
            let got = store.finalized_header.beacon();
            if got.slot == want_slot && got.tree_hash_root() == want_root {
                t.steps_pass += 1;
            } else {
                t.steps_fail.push(format!("{name} step {i} path {path}: finalized slot {} (want {want_slot})", got.slot));
            }
        }
        if pu["checks"].get("optimistic_header").is_some() {
            t.optimistic_na += 1;
        }
    }
}

#[test]
fn consensus_spec_light_client_vectors() {
    let Ok(root) = std::env::var("CONSENSUS_SPEC_TESTS") else {
        eprintln!("CONSENSUS_SPEC_TESTS not set: the consensus-specs light-client vectors were not run");
        return;
    };
    let root = PathBuf::from(root);
    let mut t = Tally::default();
    let mut ranking = 0;
    let mut merkle = 0;
    let mut collection = 0;
    // The minimal preset only: the mainnet preset ships no sync or ranking cases.
    for preset in ["minimal"] {
        let p = root.join("tests").join(preset);
        let Ok(forks) = std::fs::read_dir(&p) else { continue };
        let mut forks: Vec<_> = forks.flatten().map(|e| e.path()).collect();
        forks.sort();
        for fork in forks {
            let lc = fork.join("light_client");
            let cases = |kind: &str| -> Vec<PathBuf> {
                let Ok(rd) = std::fs::read_dir(lc.join(kind).join("pyspec_tests")) else { return Vec::new() };
                let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                v.sort();
                v
            };
            for case in cases("sync") {
                run_case(&case, &mut t);
            }
            ranking += cases("update_ranking").len();
            // single_merkle_proof/<BeaconState|BeaconBlockBody>/<case>
            if let Ok(rd) = std::fs::read_dir(lc.join("single_merkle_proof")) {
                for ty in rd.flatten() {
                    merkle += std::fs::read_dir(ty.path()).map(|r| r.count()).unwrap_or(0);
                }
            }
            collection += cases("data_collection").len();
        }
    }
    println!("consensus-specs light-client vectors ({}):", root.display());
    println!("  sync cases run: {}; process_update steps: {} (each through 3 paths: {} path-checks passed; {} path-checks applied an update, the rest expected the store unchanged); failed: {}", t.cases, t.steps, t.steps_pass, t.applied, t.steps_fail.len());
    println!("  not applicable: {} optimistic-header checks, {} updates after a force_update, {} sync cases ({:?})", t.optimistic_na, t.after_force_na, t.case_na.len(), t.case_na);
    println!("  applied updates re-checked with one participation bit cleared: {} refused of {}", t.tamper_refused, t.tamper_checked);
    println!("  upgrade_store steps (no-ops for Helios's fork-tagged headers): {}", t.upgrade_noop);
    println!("  not applicable: update_ranking {ranking}, single_merkle_proof {merkle}, data_collection {collection} cases");
    for f in &t.steps_fail {
        println!("  FAIL {f}");
    }
    assert!(t.cases > 0, "no sync case found under {}", root.display());
    assert!(t.steps_fail.is_empty(), "{} failures", t.steps_fail.len());
    assert!(t.tamper_checked > 0 && t.tamper_refused == t.tamper_checked, "a tampered update verified");
}
