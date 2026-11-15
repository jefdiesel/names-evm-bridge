//! G3 tests, EXECUTOR ONLY (no proving anywhere). Run on the test machine:
//!   cd lez-bridge/release && cargo test -j 4 -- --nocapture

use lee_core::account::AccountId;
use release_core::{ReleaseError, ReleaseInput, ReleaseJournal, TREE_DEPTH, sha256, u256};
use release_host::{
    BATCHES, CHAIN_ID, ETH, IncTree, LEZ_CHAIN_ID, PROGRAM_ID, Scenario, USDC, VAULT, ZONE_ID, as_u128, bad_cases, execute,
    execute_err, fixture, release_cases,
};

/// The vault's digest, built here word by word from the decoded fields (not via abi_encode).
fn vault_digest(j: &ReleaseJournal) -> [u8; 32] {
    let pad = |a: &[u8; 20]| {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(a);
        w
    };
    sha256(&[&j.cfg_id, &j.root, &j.nullifier, &pad(&j.token), &j.denom, &pad(&j.recipient), &pad(&j.relayer), &j.relay_fee])
}

#[test]
fn formulas_equal_bridge_core() {
    assert_eq!(release_core::EXIT_LEAF_TAG, bridge_core::EXIT_LEAF_TAG);
    assert_eq!(release_core::CFG_TAG, bridge_core::CFG_TAG);
    assert_eq!(release_core::EXIT_DOMAIN, bridge_core::EXIT_DOMAIN);
    assert_eq!(release_core::NFR_DOMAIN, bridge_core::NFR_DOMAIN);
    assert_eq!(release_core::TREE_DEPTH, bridge_core::TREE_DEPTH);
    for v in [0u128, 1, 255, 256, 100_000_000, u128::from(u64::MAX) + 1, u128::MAX] {
        assert_eq!(release_core::u256(v), bridge_core::u256(v));
    }
    let s = Scenario::build();
    assert_eq!(
        s.cfg_id,
        bridge_core::cfg_id(CHAIN_ID, &VAULT, &LEZ_CHAIN_ID, &ZONE_ID, &AccountId::new(PROGRAM_ID)),
        "cfgId"
    );
    for e in &s.exits {
        let c = bridge_core::exit_commitment(&e.secret, &e.eth_dest, &e.token, e.denom, e.max_relay_fee, &s.cfg_id);
        assert_eq!(e.commitment, c);
        assert_eq!(e.leaf, bridge_core::exit_leaf(&s.cfg_id, &e.token, e.denom, &c));
        assert_eq!(release_core::release_nullifier(&e.secret), bridge_core::release_nullifier(&e.secret));
    }
}

/// Several tokens and denominations in one tree; every exit proves against every recorded root
/// that contains it, with paths from bridge_core::merkle_path, and both root functions agree
/// with the vault's incremental algorithm.
#[test]
fn every_exit_against_every_recorded_root() {
    let s = Scenario::build();
    let mut kinds = std::collections::BTreeSet::new();
    for e in &s.exits {
        kinds.insert((e.token, e.denom_index));
    }
    assert_eq!(kinds.len(), 6, "2 tokens x 3 denominations in one tree");
    assert!(Scenario::batch_end(1) == 16 && Scenario::batch_end(2) == 17, "crosses the 2^4 boundary");
    let mut checked = 0;
    for b in 0..BATCHES.len() {
        let n = Scenario::batch_end(b);
        let leaves = s.leaves(n);
        for i in 0..n {
            let path = bridge_core::merkle_path(&leaves, i);
            let r = release_core::merkle_root(&leaves[i], i as u64, &path).unwrap();
            assert_eq!(r, bridge_core::merkle_root(&leaves[i], i as u64, &path));
            assert_eq!(r, s.roots[b], "exit {i} under batch {b}");
            let input = ReleaseInput {
                witness: s.witness_at(i, n),
                public: release_core::ReleasePublic { relayer: [7; 20], relay_fee: u256(0) },
            };
            assert_eq!(release_core::release(&input).unwrap().root, s.roots[b]);
            checked += 1;
        }
    }
    assert_eq!(checked, 5 + 16 + 17 + 20);
    // the vault's incremental algorithm against the full-layer one at every size, including
    // the empty tree
    let mut t = IncTree::default();
    assert_eq!(t.root, bridge_core::zero_hashes()[TREE_DEPTH]);
    for (i, e) in s.exits.iter().enumerate() {
        t.insert(e.leaf);
        let leaves = s.leaves(i + 1);
        assert_eq!(t.root, release_core::merkle_root(&e.leaf, i as u64, &bridge_core::merkle_path(&leaves, i)).unwrap());
    }
}

#[test]
fn guest_matches_native_and_vault_digest() {
    let s = Scenario::build();
    println!("image id 0x{}", hex::encode(release_host::image_id()));
    for c in release_cases(&s) {
        let input = s.input(&c);
        let run = execute(&input).unwrap();
        let native = release_core::release(&input).unwrap();
        assert_eq!(run.journal, native.abi_encode().to_vec(), "{}", c.label);
        assert_eq!(run.journal.len(), 256);
        let j = ReleaseJournal::abi_decode(&run.journal).unwrap();
        let e = &s.exits[c.exit];
        assert_eq!(j.cfg_id, s.cfg_id);
        assert_eq!(j.root, s.roots[c.batch]);
        assert_eq!(j.nullifier, bridge_core::release_nullifier(&e.secret));
        assert_eq!(j.token, e.token);
        assert_eq!(as_u128(&j.denom), e.denom);
        assert_eq!(j.recipient, e.eth_dest);
        assert_eq!(j.relayer, c.relayer);
        assert_eq!(as_u128(&j.relay_fee), c.relay_fee);
        assert_eq!(vault_digest(&j), sha256(&[&run.journal]));
        println!(
            "  {}: user={} total={} segments={}",
            c.label, run.user_cycles, run.total_cycles, run.segments
        );
        assert_eq!(run.segments, 1);
    }
}

/// Wrong k, path, index, ethDest, token, denom or cfgId: the guest runs, but its journal names a
/// root the vault never recorded, so `release` reverts `UnknownExitRoot` when submitted with that
/// root, and fails verification when submitted with any recorded root (the digest differs).
#[test]
fn tampered_inputs_give_journals_the_vault_rejects() {
    let s = Scenario::build();
    let mut cases: Vec<(String, ReleaseInput)> = bad_cases(&s).into_iter().map(|b| (b.label.to_string(), b.input)).collect();
    let mut wrong_token = s.input(&release_cases(&s)[2]);
    assert_eq!(wrong_token.witness.token, USDC);
    wrong_token.witness.token = ETH;
    cases.push(("different token than committed".into(), wrong_token));
    for (label, input) in &cases {
        let run = execute(input).unwrap_or_else(|e| panic!("{label}: {e:#}"));
        let j = ReleaseJournal::abi_decode(&run.journal).unwrap();
        assert!(!s.roots.contains(&j.root), "{label}: root is recorded");
        for r in &s.roots {
            let mut sub = j.clone();
            sub.root = *r;
            assert_ne!(vault_digest(&sub), sha256(&[&run.journal]), "{label}");
        }
        println!("  {label}: executes, root 0x{}.. never recorded -> UnknownExitRoot / bad seal", hex::encode(&j.root[..6]));
    }
    assert_eq!(cases.len(), 8);
}

#[test]
fn fee_above_ceiling_and_malformed_inputs_panic() {
    let s = Scenario::build();
    let cases = release_cases(&s);
    // exit 3: ceiling 2% of 1 ETH; one wei more
    let mut over = s.input(&cases[0]);
    over.public.relay_fee = u256(s.exits[3].max_relay_fee + 1);
    assert_eq!(release_core::release(&over), Err(ReleaseError::RelayFeeAboveMax));
    println!("  {}", execute_err(&over, "relay fee above the committed maxRelayFee").unwrap());
    // exit 16: ceiling 0; any fee
    let mut over0 = s.input(&cases[2]);
    over0.public.relay_fee = u256(1);
    execute_err(&over0, "relay fee above").unwrap();
    // fee in the high word only
    let mut high = s.input(&cases[1]);
    high.public.relay_fee = [0; 32];
    high.public.relay_fee[0] = 1;
    execute_err(&high, "relay fee above").unwrap();

    // The guest's fixed input layout always carries 32 siblings; the native statement refuses
    // any other length.
    let mut short = s.input(&cases[0]);
    short.witness.path.pop();
    assert_eq!(release_core::release(&short), Err(ReleaseError::PathLength(31)));
    let mut long = s.input(&cases[0]);
    long.witness.path.push([0; 32]);
    assert_eq!(release_core::release(&long), Err(ReleaseError::PathLength(33)));
    let mut far = s.input(&cases[0]);
    far.witness.leaf_index = 1 << 32;
    println!("  {}", execute_err(&far, "outside the depth-32 tree").unwrap());
}

/// The relayer and fee are in the journal: the same witness with another relayer or fee is a
/// different digest, so a seal for one can't be submitted with the other.
#[test]
fn relayer_and_fee_are_bound() {
    let s = Scenario::build();
    let c = &release_cases(&s)[1];
    let a = s.input(c);
    let mut b = a.clone();
    b.public.relayer[0] ^= 1;
    let mut f = a.clone();
    f.public.relay_fee = u256(c.relay_fee + 1);
    let ra = execute(&a).unwrap();
    let rb = execute(&b).unwrap();
    let rf = execute(&f).unwrap();
    let d = |r: &release_host::Run| sha256(&[&r.journal]);
    assert_ne!(d(&ra), d(&rb));
    assert_ne!(d(&ra), d(&rf));
    // same exit, same nullifier: only one of them can ever be released
    let n = |r: &release_host::Run| ReleaseJournal::abi_decode(&r.journal).unwrap().nullifier;
    assert_eq!(n(&ra), n(&rb));
    assert_eq!(n(&ra), n(&rf));
}

#[test]
fn fixture_on_disk_is_current() {
    let f = fixture().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
    let json = std::fs::read_to_string(dir.join("release_vector.json")).expect("run release_host --write ../fixtures first");
    let sol = std::fs::read_to_string(dir.join("ReleaseVector.sol")).unwrap();
    assert!(json == f.json, "release_vector.json is stale: regenerate with release_host --write");
    assert!(sol == f.sol, "ReleaseVector.sol is stale: regenerate with release_host --write");
    assert_eq!(f.positives.len(), 5);
    assert_eq!(f.negatives.len(), 7);
}
