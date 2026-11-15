//! The Names app scheme (`kdf.rs`, "Names app scheme"): the Names Wallet's `app_secret` with the
//! bridge's scopes, contexts and tags. Vectors from `tests/vectors/gen_names_vectors.py`
//! (Python `hmac`/`hashlib`, Foundry's `cast wallet address`); the first group is the Names
//! branch's own test vector (`app_secret.rs` `vectors`), with the same inputs.

use std::collections::HashSet;

use bridge_client::{
    hexser::unhex,
    kdf::{self, AccountRoot, Kind, Scheme, names},
    scan::{Dest, deposit_candidate, exit_candidate},
};
use bridge_core::{claim_nullifier, recipient_opening, release_nullifier};
use lee_core::account::AccountId;
use serde_json::Value;

fn bytes(v: &Value) -> Vec<u8> {
    unhex(v.as_str().unwrap()).unwrap()
}
fn b32(v: &Value) -> [u8; 32] {
    bytes(v).try_into().unwrap()
}
fn b20(v: &Value) -> [u8; 20] {
    bytes(v).try_into().unwrap()
}
fn num(v: &Value) -> u128 {
    v.as_str().unwrap().parse().unwrap()
}
fn hex(b: &[u8]) -> String {
    bridge_client::hexser::hex(b)
}

/// Same inputs as the Names branch's `app_secret.rs` test `vectors`, same outputs.
#[test]
fn the_names_branch_vectors_match() {
    let secret = [7_u8; 32];
    let account = [9_u8; 32];
    let m0 = names::master(&secret, "lez_bridge", "RFP022/deposit", &[0xab, 0xcd], &account, 0);
    assert_eq!(hex(&m0), "d3d73d74e8003e7236d01e9b4bfcd9db81d6ade99c2304c4527ef46f5ef4b716");
    let got: Vec<String> = names::values(&secret, "lez_bridge", "RFP022/deposit", &[0xab, 0xcd], "RFP022/ck", &account, 0, 2).iter().map(|v| hex(v)).collect();
    assert_eq!(
        got,
        [
            "8bdd25262f56570aa34cd58d46bd80fc6de18deedc75ac8fdc8994c500f19e59",
            "566f88b5c0b2ad570680236cdfb51278ba3b74e772be69443d4560036e2acdfb",
        ]
    );
    // The JSON file carries the same literals.
    let doc: Value = serde_json::from_str(include_str!("vectors/kdf-names-v1.json")).unwrap();
    let v = &doc["names_branch_vectors"];
    assert_eq!(b32(&v["master_0"]), m0);
    assert_eq!(v["values"].as_array().unwrap().iter().map(|x| hex(&b32(x))).collect::<Vec<_>>(), got);
}

#[test]
fn the_python_vectors_match() {
    let doc: Value = serde_json::from_str(include_str!("vectors/kdf-names-v1.json")).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for c in cases {
        let secret = b32(&c["account_secret"]);
        let root = AccountRoot::names_app(&secret, c["app"].as_str().unwrap()).unwrap();
        assert_eq!(root.scheme(), &Scheme::NamesApp { app: "lez_bridge".into() });
        let (account, cfg, token) = (AccountId::new(b32(&c["account"])), b32(&c["cfg_id"]), b20(&c["token"]));
        let (denom, index, dest, fee) = (num(&c["denom"]), c["index"].as_u64().unwrap(), b20(&c["eth_dest"]), num(&c["max_relay_fee"]));
        // Contexts: cfgId(32) | token(20) | u256(denom) [| ethDest(20) | u256(fee)].
        assert_eq!(names::deposit_context(&cfg, &token, denom), bytes(&c["deposit_context"]));
        assert_eq!(names::deposit_context(&cfg, &token, denom).len(), 84);
        assert_eq!(names::exit_context(&cfg, &token, denom, &dest, fee), bytes(&c["exit_context"]));
        assert_eq!(names::release_context(&cfg, &token, denom, fee), bytes(&c["release_context"]));
        // Deposits: the master is the wallet's `m`, the claim secret is the value with tag RFP022/ck.
        let s = kdf::deposit_master(&root, &cfg, &account, &token, denom, index);
        assert_eq!(s, b32(&c["deposit_master"]), "{}", c["name"]);
        let d = deposit_candidate(&root, &cfg, &account, &token, denom, index);
        assert_eq!(d.secret, b32(&c["claim_secret"]));
        assert_eq!(d.commitment, b32(&c["commitment"]));
        assert_eq!(claim_nullifier(&d.secret, &account, &token, denom), b32(&c["claim_nullifier"]));
        assert_eq!(recipient_opening(&d.secret), b32(&c["recipient_opening"]));
        assert_eq!(kdf::reclaim_nullifier(&s), b32(&c["reclaim_nullifier"]));
        // What the module gets from `app_secret` is exactly k.
        let asked = names::values(&secret, "lez_bridge", names::DEPOSIT_SCOPE, &names::deposit_context(&cfg, &token, denom), names::DEPOSIT_TAG, account.value(), index, 1);
        assert_eq!(asked[0], d.secret);
        // Exits.
        let e = exit_candidate(&root, &cfg, &account, &token, denom, &Dest::Address(dest), fee, index);
        assert_eq!(e.master, b32(&c["exit_master"]));
        assert_eq!(e.secret, b32(&c["exit_secret"]));
        assert_eq!(e.exit_commitment, b32(&c["exit_commitment"]));
        assert_eq!(e.release_nullifier, b32(&c["release_nullifier"]));
        assert_eq!(release_nullifier(&e.secret), e.release_nullifier);
        let asked = names::values(&secret, "lez_bridge", names::EXIT_SCOPE, &names::exit_context(&cfg, &token, denom, &dest, fee), names::EXIT_TAG, account.value(), index, 1);
        assert_eq!(asked[0], e.secret);
        // Release-address keys: the value with tag RFP022/release-key is the key.
        let rk = kdf::release_key(&root, &cfg, &account, &token, denom, fee, index);
        assert_eq!(*rk.secret(), b32(&c["release_key"]));
        assert_eq!(rk.address, b20(&c["release_address"]), "cast wallet address");
        let asked = names::values(&secret, "lez_bridge", names::RELEASE_SCOPE, &names::release_context(&cfg, &token, denom, fee), names::RELEASE_TAG, account.value(), index, 1);
        assert_eq!(kdf::release_key_from_value(&asked[0]).address, rk.address);
        let ed = exit_candidate(&root, &cfg, &account, &token, denom, &Dest::Derived, fee, index);
        assert_eq!(ed.eth_dest, rk.address);
    }
}

/// The two schemes never agree, another app gets other values, and an invalid app name is refused.
#[test]
fn schemes_and_apps_are_separated() {
    let secret = [0x33; 32];
    let account = AccountId::new([0x44; 32]);
    let v1 = AccountRoot::from_account_secret(&secret);
    let ours = AccountRoot::names_app(&secret, "lez_bridge").unwrap();
    let theirs = AccountRoot::names_app(&secret, "other_app").unwrap();
    let mut seen = HashSet::new();
    for root in [&v1, &ours, &theirs] {
        for kind in [Kind::Dep, Kind::Exit, Kind::Release] {
            for index in [0_u64, 1] {
                assert!(seen.insert(root.derive(&[1; 32], kind, &account, index, &kdf::deposit_ctx(&[2; 20], 100))));
            }
        }
    }
    for bad in ["", "evil app", "naïve", &"a".repeat(65)] {
        assert!(AccountRoot::names_app(&secret, bad).is_none(), "{bad:?}");
    }
    assert_eq!(names::scope_of(Kind::Dep), "RFP022/deposit");
    assert_eq!(names::tag_of(Kind::Exit), "RFP022/ek");
    // A batch is the same as one index at a time (the module asks 64 at once).
    let ctx = names::deposit_context(&[1; 32], &[2; 20], 100);
    let batch = names::values(&secret, "lez_bridge", names::DEPOSIT_SCOPE, &ctx, names::DEPOSIT_TAG, account.value(), 5, 4);
    for (o, v) in batch.iter().enumerate() {
        assert_eq!(*v, names::values(&secret, "lez_bridge", names::DEPOSIT_SCOPE, &ctx, names::DEPOSIT_TAG, account.value(), 5 + o as u64, 1)[0]);
    }
}
