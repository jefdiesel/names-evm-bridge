//! KDF vectors (computed independently: `tests/vectors/gen_kdf_vectors.py`, Python `hmac` and
//! `hashlib`; release addresses by Foundry's `cast wallet address`) and independence checks.

use std::collections::HashSet;

use bridge_client::{
    hexser::unhex,
    kdf::{self, AccountRoot, Kind},
    scan::{Dest, deposit_candidate, exit_candidate},
};
use bridge_core::{claim_nullifier, recipient_opening, release_nullifier};
use lee_core::account::AccountId;
use serde_json::Value;

fn b32(v: &Value) -> [u8; 32] {
    unhex(v.as_str().unwrap()).unwrap().try_into().unwrap()
}
fn b20(v: &Value) -> [u8; 20] {
    unhex(v.as_str().unwrap()).unwrap().try_into().unwrap()
}
fn num(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn the_python_vectors_match() {
    let doc: Value = serde_json::from_str(include_str!("vectors/kdf-v1.json")).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for c in cases {
        let root = AccountRoot::from_account_secret(&b32(&c["account_secret"]));
        let (account, cfg, token) = (AccountId::new(b32(&c["account"])), b32(&c["cfg_id"]), b20(&c["token"]));
        let (denom, index, dest, fee) = (num(&c["denom"]), c["index"].as_u64().unwrap(), b20(&c["eth_dest"]), num(&c["max_relay_fee"]));
        assert_eq!(kdf::hmac_sha256(kdf::ROOT_SALT, &b32(&c["account_secret"])), b32(&c["prk"]), "{}", c["name"]);
        let s = kdf::deposit_master(&root, &cfg, &account, &token, denom, index);
        assert_eq!(s, b32(&c["deposit_master"]));
        let d = deposit_candidate(&root, &cfg, &account, &token, denom, index);
        assert_eq!(d.master, s);
        assert_eq!(d.secret, b32(&c["claim_secret"]));
        assert_eq!(d.commitment, b32(&c["commitment"]));
        assert_eq!(claim_nullifier(&d.secret, &account, &token, denom), b32(&c["claim_nullifier"]));
        assert_eq!(recipient_opening(&d.secret), b32(&c["recipient_opening"]));
        assert_eq!(kdf::reclaim_nullifier(&s), b32(&c["reclaim_nullifier"]));
        let e = exit_candidate(&root, &cfg, &account, &token, denom, &Dest::Address(dest), fee, index);
        assert_eq!(e.master, b32(&c["exit_master"]));
        assert_eq!(e.secret, b32(&c["exit_secret"]));
        assert_eq!(e.exit_commitment, b32(&c["exit_commitment"]));
        assert_eq!(e.release_nullifier, b32(&c["release_nullifier"]));
        assert_eq!(release_nullifier(&e.secret), e.release_nullifier);
        let rk = kdf::release_key(&root, &cfg, &account, &token, denom, fee, index);
        assert_eq!(*rk.secret(), b32(&c["release_key"]));
        assert_eq!(rk.address, b20(&c["release_address"]), "cast wallet address");
        // A derived destination is that address.
        let ed = exit_candidate(&root, &cfg, &account, &token, denom, &Dest::Derived, fee, index);
        assert_eq!(ed.eth_dest, rk.address);
    }
}

/// The first vector as literals, so a change to the JSON file cannot hide a change of the KDF.
#[test]
fn pinned_literals() {
    let root = AccountRoot::from_account_secret(&[0x11; 32]);
    let d = deposit_candidate(&root, &[0xC0; 32], &AccountId::new([0xA1; 32]), &[0xAA; 20], 100_000_000, 0);
    assert_eq!(bridge_client::hexser::hex(&d.secret), "6c88f9ac79b5d7cc0d8c6d6b32668c7ade420ebe5c73f8cdef7c403e640a25ec");
    assert_eq!(bridge_client::hexser::hex(&d.commitment), "863f393d13b71de9bd0cc2216972be2a95d864afe1e8f8c299178825d6cd7a91");
    let e = exit_candidate(&root, &[0xC0; 32], &AccountId::new([0xA1; 32]), &[0xAA; 20], 100_000_000, &Dest::Address([0xBE; 20]), 2_000_000, 0);
    assert_eq!(bridge_client::hexser::hex(&e.exit_commitment), "e297f4f8b46d6e7cd26789c4248d600fcf656d99017809847b16d27022ba8ccc");
    let rk = kdf::release_key(&root, &[0xC0; 32], &AccountId::new([0xA1; 32]), &[0xAA; 20], 100_000_000, 2_000_000, 0);
    assert_eq!(bridge_client::hexser::hex(&rk.address), "c30957aac77b39c697bd78038ccab2bb751a3416");
}

/// Changing any one input (account secret, cfg id, account, kind, index, token, denomination)
/// gives a secret unrelated to every other: all distinct, and no two share a byte prefix.
#[test]
fn every_input_changes_the_secret() {
    let mut seen = HashSet::new();
    let mut prefixes = HashSet::new();
    let mut n = 0;
    for secret in [[1u8; 32], [2u8; 32]] {
        let root = AccountRoot::from_account_secret(&secret);
        for cfg in [[0u8; 32], [1u8; 32]] {
            for account in [AccountId::new([3; 32]), AccountId::new([4; 32])] {
                for kind in [Kind::Dep, Kind::Exit, Kind::Release] {
                    for index in [0u64, 1, 1 << 33] {
                        for (token, denom) in [([5u8; 20], 100u128), ([6u8; 20], 100), ([5u8; 20], 1000)] {
                            let out = root.derive(&cfg, kind, &account, index, &kdf::deposit_ctx(&token, denom));
                            assert!(seen.insert(out), "a repeated secret");
                            prefixes.insert(out[..4].to_vec());
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(seen.len(), n);
    assert_eq!(prefixes.len(), n, "two secrets share a 4-byte prefix");
}

/// The exit secret binds its destination and fee: the same index with another address or fee
/// never shares a release nullifier.
#[test]
fn exit_secrets_bind_destination_and_fee() {
    let root = AccountRoot::from_account_secret(&[9; 32]);
    let a = AccountId::new([1; 32]);
    let mut nfs = HashSet::new();
    for dest in [Dest::Derived, Dest::Address([1; 20]), Dest::Address([2; 20])] {
        for fee in [0u128, 1, 2_000_000] {
            let e = exit_candidate(&root, &[0; 32], &a, &[7; 20], 100_000_000, &dest, fee, 0);
            assert!(nfs.insert(e.release_nullifier));
        }
    }
    // The derived destination changes with the index (a fresh address per exit).
    let d0 = exit_candidate(&root, &[0; 32], &a, &[7; 20], 100_000_000, &Dest::Derived, 0, 0).eth_dest;
    let d1 = exit_candidate(&root, &[0; 32], &a, &[7; 20], 100_000_000, &Dest::Derived, 0, 1).eth_dest;
    assert_ne!(d0, d1);
}

#[test]
fn the_fee_cap_is_the_vaults_bound() {
    for denom in [1u128, 99, 10_000, 100_000_000, 10u128.pow(18), u128::MAX] {
        let cap = kdf::max_relay_fee_cap(denom);
        // BridgeVault: `fee * 10_000 > denom * MAX_FEE_BPS` reverts. The cap passes, cap + 1 fails.
        let ok = |fee: u128| primitive_types_mul_le(fee, 10_000, denom, 200);
        assert!(ok(cap), "{denom}");
        assert!(!ok(cap + 1), "{denom}");
    }
}

/// `a * x <= b * y` without overflow (all operands below 2^128, multipliers small).
fn primitive_types_mul_le(a: u128, x: u128, b: u128, y: u128) -> bool {
    let wide = |v: u128, m: u128| -> (u128, u128) {
        let (lo, hi) = (v & u128::from(u64::MAX), v >> 64);
        let lo_m = lo * m;
        let hi_m = hi * m + (lo_m >> 64);
        (hi_m >> 64, (hi_m << 64) | (lo_m & u128::from(u64::MAX)))
    };
    wide(a, x) <= wide(b, y)
}
