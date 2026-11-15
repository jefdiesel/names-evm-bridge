//! Names Wallet mode, end to end inside the module: a stand-in `names` module (`wallet.rs`
//! `stand_in`) that keeps grants and requests, a stand-in CLI that speaks `--json` and
//! `--emit-call` (files in testdata/cli/), and Ethereum as recorded from Sepolia plus deposits
//! made in the test from the stand-in wallet's values.
//!
//! What is checked: the gate order (access, token source, secret grant), that the module asks
//! for values with the bridge's scopes, contexts and tags, that a deposit stores its index and
//! never its secret, that a claim is built by the CLI with the secret in a 0600 file and proposed
//! as a private call (never proved here), that a failed wallet check shows only "see the Names
//! app", the burn path (built only on the demo vault), and recovery by batches.

use super::*;
use crate::wallet::stand_in;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

const V: &str = "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC";
const ANCHOR: &str = "0x5e81a69dd5b2dfd18a00c0021f7954b7971421221897b3ec752a4002561ddd7d";
const CFG: &str = "0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11";
const USDC: &str = "0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e";
const TP: &str = "4VScJ7mtwaanQ5gcSZZw2iAMHYAbLrVmbYKwEGx2eSex";

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("the module answers JSON")
}
fn ok(text: &str) -> Value {
    let v = parsed(text);
    assert_eq!(v["success"], true, "{v}");
    v["value"].clone()
}
fn err(text: &str) -> String {
    let v = parsed(text);
    assert_eq!(v["success"], false, "{v}");
    v["error"].as_str().unwrap().to_owned()
}

/// The stand-in wallet's values: anything deterministic and distinct per input does (the real
/// formula is pinned in lez-bridge `client/tests/kdf_names.rs`).
fn value(scope: &str, ctx_hex: &str, tag: &str, i: u64) -> [u8; 32] {
    util::sha256(&[b"stand-in", scope.as_bytes(), ctx_hex.as_bytes(), tag.as_bytes(), &i.to_be_bytes()])
}

/// The stand-in Names Wallet's state.
#[derive(Default)]
struct Wallet {
    connected: bool,
    accounts: Vec<String>,
    sources: Vec<String>,
    secret_grants: BTreeSet<String>,
    status: BTreeMap<String, Value>,
    proposed: Vec<Value>,
}

fn install(w: Rc<RefCell<Wallet>>) {
    stand_in::reset();
    let x = w.clone();
    stand_in::handle("granted_accounts", move |_| {
        let w = x.borrow();
        json!({ "success": true, "value": { "connected": w.connected, "accounts": w.accounts.iter().map(|a| json!({ "account": a, "account_kind": if a.starts_with("Private/") { "private" } else { "public" } })).collect::<Vec<_>>() } })
    });
    stand_in::answer("request_access", json!({ "request": "acc-1", "kind": "connect", "state": "pending" }));
    let x = w.clone();
    stand_in::handle("token_sources", move |_| {
        json!({ "success": true, "value": x.borrow().sources.iter().map(|t| json!({ "token_program": t, "definitions": [], "app": "lez_bridge" })).collect::<Vec<_>>() })
    });
    let x = w.clone();
    stand_in::handle("propose_token_source", move |args| {
        x.borrow_mut().proposed.push(json!({ "token_source": args }));
        json!({ "success": true, "value": { "request": "ts-1", "kind": "token_source", "state": "pending", "token_program": args[0] } })
    });
    let x = w.clone();
    stand_in::handle("request_status", move |args| {
        let id = args[0].as_str().unwrap_or_default().to_owned();
        match x.borrow().status.get(&id) {
            Some(s) => json!({ "success": true, "value": s }),
            None => json!({ "success": false, "value": null, "error": "no such request" }),
        }
    });
    let x = w.clone();
    stand_in::handle("app_secret", move |args| {
        let w = x.borrow();
        let account = args[0].as_str().unwrap();
        assert!(w.accounts.iter().any(|a| a == account), "asked for an account the wallet didn't give: {account}");
        let scope = args[1].as_str().unwrap();
        if !w.secret_grants.contains(scope) {
            return json!({ "success": true, "value": { "granted": false, "request": { "request": format!("sec-{scope}"), "kind": "secret", "state": "pending", "scope": scope } } });
        }
        let (ctx, tag, first, count) = (args[2].as_str().unwrap(), args[3].as_str().unwrap(), args[4].as_u64().unwrap(), args[5].as_u64().unwrap());
        assert!(count <= 64 && !ctx.starts_with("0x"));
        let values: Vec<String> = (first..first + count).map(|i| util::to_hex(&value(scope, ctx, tag, i))).collect();
        json!({ "success": true, "value": { "granted": true, "account": account, "scope": scope, "tag": tag, "first": first, "values": values } })
    });
    let x = w.clone();
    stand_in::handle("propose_private_call", move |args| {
        let mut w = x.borrow_mut();
        let id = format!("pc-{}", w.proposed.iter().filter(|p| p.get("private_call").is_some()).count() + 1);
        w.proposed.push(json!({ "private_call": args, "id": id }));
        w.status.insert(id.clone(), json!({ "request": id, "kind": "private_call", "state": "pending", "check": "running" }));
        json!({ "success": true, "value": { "request": id, "kind": "private_call", "state": "pending", "check": "running" } })
    });
    let x = w;
    stand_in::handle("cancel_request", move |args| {
        let id = args[0].as_str().unwrap_or_default().to_owned();
        x.borrow_mut().status.insert(id.clone(), json!({ "request": id, "state": "cancelled" }));
        json!({ "success": true, "value": { "request": id, "state": "cancelled" } })
    });
}

/// Sepolia as recorded, plus deposits the test makes (leaf 5 onwards, under the anchor).
struct Vault {
    replay: Box<dyn EthSource + Send>,
    extra: Vec<Value>,
}

impl EthSource for Vault {
    fn describe(&self) -> String {
        "recorded Sepolia plus test deposits".into()
    }
    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        // The deposit desk (tests that set one): 0.3%, no vault fee.
        if data.len() == 4 + 64 && data[..4] == util::selector("quote(address,uint32)") {
            let i = data[4 + 63] as usize;
            let denom = [100_000_000u128, 1_000_000_000, 10_000_000_000][i.min(2)];
            let words = [denom, 0, denom * 30 / 10_000, denom + denom * 30 / 10_000];
            return Ok(words.iter().flat_map(|v| wallet::u256(*v)).collect());
        }
        if data == eth::calldata("feeBps()", &[]).as_slice() {
            return Ok(wallet::u256(30).to_vec());
        }
        if data == eth::calldata("depositCount()", &[]).as_slice() {
            let mut w = [0u8; 32];
            w[24..].copy_from_slice(&(5 + self.extra.len() as u64).to_be_bytes());
            return Ok(w.to_vec());
        }
        self.replay.call(to, data)
    }
    fn block(&self, tag: &str) -> Result<eth::BlockInfo, String> {
        self.replay.block(tag).or_else(|_| self.replay.block("0xb8fb75"))
    }
    fn block_by_hash(&self, hash: &str) -> Result<Option<eth::BlockInfo>, String> {
        self.replay.block_by_hash(hash)
    }
    fn logs(&self, a: &[u8; 20], t: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
        let mut l = self.replay.logs(a, t, from, to)?;
        l.extend(self.extra.iter().filter(|_| (from..=to).contains(&12_122_997)).cloned());
        Ok(l)
    }
    fn code_len(&self, a: &[u8; 20]) -> Result<usize, String> {
        self.replay.code_len(a)
    }
    fn chain_id(&self) -> Result<u64, String> {
        Ok(11_155_111)
    }
}

/// A `Deposit` log for commitment `c` at `leaf` (block 12,122,997, under the recorded anchor).
fn deposit_log(c: &[u8; 32], leaf: u64, denom_index: u64) -> Value {
    let mut data = Vec::new();
    data.extend_from_slice(&wallet::u256(u128::from(denom_index)));
    data.extend_from_slice(c);
    data.extend_from_slice(&wallet::u256(u128::from(leaf)));
    data.extend_from_slice(&util::sha256(&[b"root", &leaf.to_be_bytes()]));
    json!({
        "address": "0x8c8b2664de28e5287158db4db65d448638871b74", "blockNumber": "0xb8fb75", "transactionHash": format!("0x{}", "77".repeat(32)),
        "topics": [hex0x(&eth::deposit_topic()), format!("0x000000000000000000000000{}", &USDC[2..])], "data": hex0x(&data), "removed": false,
    })
}

/// A module in Names Wallet mode, with a stand-in CLI that speaks --json and --emit-call.
fn bridge(name: &str, extra: Vec<Value>) -> (Bridge, PathBuf) {
    let root = std::env::temp_dir().join(format!("lez-bridge-names-{name}-{}-{}", std::process::id(), now_ms()));
    let _ = std::fs::remove_dir_all(&root);
    let wallet = root.join("reader");
    std::fs::create_dir_all(&wallet).unwrap();
    std::fs::write(wallet.join("storage.json"), "{}").unwrap();
    let td = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cli");
    let script = format!(
        r#"#!/bin/sh
D={td}
echo "ARGV $*" >> "$TMPDIR/argv.log"
J=0
while [ "$1" = "--sequencer" ] || [ "$1" = "--dry-run" ] || [ "$1" = "--json" ]; do
  case "$1" in --sequencer) shift 2;; --json) J=1; shift;; *) shift;; esac
done
arg() {{ want=$1; shift; while [ $# -gt 0 ]; do [ "$1" = "$want" ] && {{ echo "$2"; return; }}; shift; done; }}
case "$1" in
  --help) echo "Usage: lez-bridge [--json] [--names-app APP]";;
  show) cat $D/show.json;;
  attest) cat $D/attest-show.json;;
  claim-attested)
    K=$(arg --secret-file "$@"); P=$(arg --emit-call "$@")
    echo "SECRET_FILE_MODE $(stat -f %Lp "$K") BYTES $(wc -c < "$K" | tr -d ' ')" >> "$TMPDIR/argv.log"
    echo '{{"event":"phase","phase":"fetch","at_ms":1}}'
    if [ "$(arg --index "$@")" = "6" ]; then
      echo '{{"event":"error","code":"already_claimed","message":"deposit already claimed: its nullifier is spent; nothing sent","sent":false,"local_only":true}}'; exit 1
    fi
    echo '{{"event":"plan","root":"0xbe","recipient":"B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT","recipient_kind":"one_time","sponsor":"20000000","float":"80000000"}}'
    cp $D/claim-call.json "$P"; chmod 600 "$P"
    echo "{{\"event\":\"result\",\"ok\":true,\"emitted\":\"$P\",\"message_bytes\":6595,\"fee_payer\":\"account\",\"recipient\":\"B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT\",\"recipient_kind\":\"one_time\",\"sponsored\":true,\"sponsor\":\"20000000\",\"root\":\"0xbe\",\"dry_run\":true}}"
    echo "private call written (human text, stderr)" >&2;;
  burn)
    P=$(arg --emit-call "$@")
    cp $D/burn-call.json "$P"
    echo "{{\"event\":\"result\",\"ok\":true,\"emitted\":\"$P\",\"message_bytes\":82,\"fee_payer\":\"wallet\",\"exit_commitment\":\"$(arg --exit "$@")\",\"dry_run\":true}}";;
  *) echo "Error: fake cli: unknown $1"; exit 2;;
esac
"#
    );
    let program = root.join("lez-bridge");
    std::fs::write(&program, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = Config::default();
    cfg.deployment = config::sepolia_demo_v2();
    assert!(cfg.names_wallet(), "Names Wallet mode is the default");
    cfg.cli = Some(program.display().to_string());
    cfg.wallet_dir = Some(wallet.display().to_string());
    cfg.tmp_dir = Some(root.join("tmp").display().to_string());
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    let replay = crate::tests::replay();
    let b = Bridge { data: root.join("data"), cfg, fixtures: None, registry_cache: None, eth_override: Some(Box::new(Vault { replay, extra })), relayed: Default::default() };
    (b, root)
}

fn connected(w: &Rc<RefCell<Wallet>>) {
    let mut w = w.borrow_mut();
    w.connected = true;
    w.accounts = vec![format!("Private/{V}"), "Public/6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f".into()];
}

fn asked(method: &str) -> Vec<Value> {
    stand_in::asked().into_iter().filter(|(m, _)| m == method).map(|(_, a)| a).collect()
}

fn ctx_hex(denom: u128) -> String {
    util::to_hex(&wallet::deposit_context(&util::hex32(CFG).unwrap(), &util::eth_address(USDC).unwrap(), denom))
}

/// What a deposit of index `i` (dUSDC 100) commits to, from the stand-in wallet's value.
fn commitment_of(i: u64) -> [u8; 32] {
    let k = value("RFP022/deposit", &ctx_hex(100_000_000), "RFP022/ck", i);
    wallet::deposit_commitment(&k, &util::lez_account(V).unwrap(), &util::eth_address(USDC).unwrap(), 100_000_000, &util::hex32(CFG).unwrap())
}

#[test]
fn the_status_asks_for_access_then_proposes_the_bridged_tokens_once() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    let (mut b, root) = bridge("status", vec![]);
    let s = ok(&b.wallet_status());
    assert_eq!((s["mode"].as_str(), s["connected"].as_bool(), s["needs"].as_str()), (Some("names"), Some(false), Some("connect")));
    let r = ok(&b.wallet_connect());
    assert_eq!(r["request"], "acc-1");
    assert_eq!(asked("request_access")[0][0], wallet::ACCESS_REASON);
    connected(&w);
    let s = ok(&b.wallet_status());
    assert_eq!(s["needs"], "token_source", "{s}");
    assert_eq!(s["token_source"]["request"]["request"], "ts-1");
    // The definitions carry the bridge config's decimals (show --json), not the deployment's.
    let p = asked("propose_token_source");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0][0], TP);
    let defs: Value = serde_json::from_str(p[0][1].as_str().unwrap()).unwrap();
    assert_eq!(defs[0]["definition"], "FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ");
    assert_eq!(defs[0]["decimals"], 6);
    assert_eq!(defs[1]["decimals"], 18);
    assert_eq!(defs[0]["origin"], "Sepolia 0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e");
    assert_eq!(defs[1]["origin"], "Sepolia native ETH");
    // Waiting: asked again, it is not proposed twice.
    w.borrow_mut().status.insert("ts-1".into(), json!({ "request": "ts-1", "state": "pending" }));
    assert_eq!(ok(&b.wallet_status())["needs"], "token_source");
    assert_eq!(asked("propose_token_source").len(), 1);
    w.borrow_mut().sources.push(TP.into());
    let s = ok(&b.wallet_status());
    assert!(s["needs"].is_null(), "{s}");
    assert_eq!(s["accounts"][0]["account"], V);
    // The wallet's accounts are the module's accounts.
    let a = ok(&b.accounts());
    assert_eq!(a["suggested"], V);
    assert!(ok(&b.holdings())["note"].as_str().unwrap().contains("Bridged tokens"));
    // A failed check reads as "see the Names app", in general terms only.
    w.borrow_mut().status.insert("x".into(), json!({ "request": "x", "kind": "private_call", "state": "pending", "check": "failed", "error": "V holds 3 dUSDC" }));
    let r = ok(&b.wallet_request("x".into()));
    assert!(r["error"].as_str().unwrap().contains("Open the Names app"));
    assert!(!r.to_string().contains("3 dUSDC"));
    assert!(err(&b.set_wallet_mode("other".into())).contains("names"));
    assert_eq!(ok(&b.set_wallet_mode("cli".into()))["wallet"], "cli");
    assert_eq!(ok(&b.wallet_status())["mode"], "cli");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_deposit_request_takes_the_wallets_value_and_keeps_only_its_index() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    // Index 0 is already deposited on Ethereum (e.g. from this phrase on another computer).
    let (mut b, root) = bridge("deposit", vec![deposit_log(&commitment_of(0), 5, 0)]);
    let first = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    assert_eq!(first["needs"], "secret", "{first}");
    assert_eq!(first["request"]["scope"], "RFP022/deposit");
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let r = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    // The values asked for: the bridge's scope, context and tag, a batch of 64 from 0.
    let a = asked("app_secret");
    assert_eq!(a.last().unwrap(), &json!([format!("Private/{V}"), "RFP022/deposit", ctx_hex(100_000_000), "RFP022/ck", 0, 64]));
    // Index 0 is used on Ethereum, so this is index 1.
    assert_eq!(r["derivation"]["index"], 1, "{r}");
    assert_eq!(r["commitment"], hex0x(&commitment_of(1)));
    let k1 = util::to_hex(&value("RFP022/deposit", &ctx_hex(100_000_000), "RFP022/ck", 1));
    let stored = b.store().load();
    assert_eq!(stored["deposits"][0]["key_index"], 1);
    assert!(stored["deposits"][0].get("secret").is_none(), "{stored}");
    for text in [r.to_string(), stored.to_string(), std::fs::read_to_string(root.join("tmp/argv.log")).unwrap_or_default()] {
        assert!(!text.contains(&k1), "the claim secret left the module");
    }
    assert!(r["export_text"].as_str().unwrap().contains("restoring the wallet from its recovery phrase restores it"));
    // The next one takes the next index; a public account or one the wallet didn't give is refused.
    assert_eq!(ok(&b.make_deposit_request("dUSDC".into(), "0".into(), "".into()))["derivation"]["index"], 2);
    assert!(err(&b.make_deposit_request("dUSDC".into(), "0".into(), "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f".into())).contains("isn't one of the private accounts"));
    // A public request for an account the wallet gave as private: the wallet is asked for it as a
    // public account first (Receive into a public account: a_public_account_receives_…).
    assert!(err(&b.make_public_deposit_request("dUSDC".into(), "0".into(), V.into(), "true".into())).contains("take this app's access back"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_claim_is_built_by_the_cli_and_proposed_never_proved_here() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    // The deposit of index 0 is leaf 5 on Ethereum (under the anchor).
    let (mut b, root) = bridge("claim", vec![deposit_log(&commitment_of(0), 5, 0)]);
    let rootword = hex0x(&util::sha256(&[b"root", &5u64.to_be_bytes()]));
    let dir = b.data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &rootword[2..14]));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("eth_getProof.json"), "{}").unwrap();
    std::fs::write(dir.join("header.rlp.hex"), "0x").unwrap();
    // Recovery finds it (index 0), and adds it to this computer's list.
    let found = ok(&b.recover(V.into(), "20".into()));
    assert_eq!(found["found"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["found"][0]["key_index"], 0);
    let id = found["found"][0]["id"].as_str().unwrap().to_owned();
    // Gate order: the token source first; nothing is built or proposed before.
    let gated = ok(&b.claim_plan(id.clone()));
    assert_eq!(gated["needs"]["needs"], "token_source", "{gated}");
    assert!(asked("propose_private_call").is_empty());
    w.borrow_mut().sources.push(TP.into());
    let plan = ok(&b.claim_plan(id.clone()));
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["wallet"], "names");
    assert_eq!(plan["recipient"], "B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT");
    assert_eq!(plan["call"]["message_bytes"], 6595);
    assert_eq!(plan["call"]["accounts"], 7);
    assert!(plan.get("message").is_none() && !plan.to_string().contains("abababab"), "the message (it holds k) stays out of replies");
    assert!(asked("propose_private_call").is_empty(), "planning proposes nothing");
    // The secret reached the CLI only in a 0600 file, now gone; argv never had it.
    let argv = std::fs::read_to_string(root.join("tmp/argv.log")).unwrap();
    assert!(argv.contains("SECRET_FILE_MODE 600 BYTES 66"), "{argv}");
    let k0 = util::to_hex(&value("RFP022/deposit", &ctx_hex(100_000_000), "RFP022/ck", 0));
    assert!(!argv.contains(&k0));
    let line = argv.lines().find(|l| l.contains("claim-attested")).unwrap();
    assert!(line.contains("--json claim-attested") && line.contains("--secret-file") && line.contains("--emit-call") && line.contains(&format!("--from {V}")), "{line}");
    let leftovers: Vec<_> = std::fs::read_dir(root.join("tmp")).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("k-") || n.starts_with("call-")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Claiming proposes exactly the call the CLI built, from V.
    let job = ok(&b.claim(id.clone(), json!({ "claim_early": true, "confirm_weaker": true }).to_string()));
    assert_eq!((job["state"].as_str(), job["phase"].as_str(), job["wallet"].as_str()), (Some("running"), Some("approve"), Some("names")), "{job}");
    let p = asked("propose_private_call");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0][0], format!("Private/{V}"));
    let sent: Value = serde_json::from_str(p[0][1].as_str().unwrap()).unwrap();
    let built: Value = serde_json::from_str(include_str!("../testdata/cli/claim-call.json")).unwrap();
    assert_eq!(sent, built);
    assert!(p[0][2].as_str().unwrap().starts_with("Claim 100 dUSDC from Sepolia"));
    let jid = job["id"].as_str().unwrap().to_owned();
    // Asked again while it is with the wallet: the same job, no second proposal.
    let again = ok(&b.claim(id.clone(), json!({ "claim_early": true, "confirm_weaker": true }).to_string()));
    assert_eq!(again["id"], jid.as_str());
    assert_eq!(asked("propose_private_call").len(), 1);
    // A busy wallet while polling: the job as last known, with a note, no wait in the module.
    stand_in::script("request_status", vec![stand_in::busy()]);
    let j = ok(&b.job(jid.clone()));
    assert_eq!(j["state"], "running", "{j}");
    assert!(j["note"].as_str().unwrap().contains("busy"), "{j}");
    assert!(stand_in::slept().is_empty());
    assert!(err(&b.pause_job(jid.clone())).contains("Names app"));
    // The person approves; the wallet proves; it lands.
    for (state, phase) in [("running", "prove"), ("running", "submit")] {
        w.borrow_mut().status.insert("pc-1".into(), json!({ "request": "pc-1", "state": state, "check": "ready", "phase": phase }));
        let j = ok(&b.job(jid.clone()));
        assert_eq!(j["phase"], phase);
        assert_eq!(j["can_cancel"], phase == "prove");
    }
    w.borrow_mut().status.insert("pc-1".into(), json!({ "request": "pc-1", "state": "done", "check": "ready", "tx": "f40c" }));
    let end = ok(&b.job(jid.clone()));
    assert_eq!((end["state"].as_str(), end["result"]["tx"].as_str()), (Some("done"), Some("f40c")));
    assert!(ok(&b.jobs())["jobs"].as_array().unwrap().iter().any(|j| j["id"] == jid.as_str()));
    let list = ok(&b.deposits());
    assert_eq!(list["deposits"][0]["stage"], "claimed", "{list}");
    // Nothing secret in the module's own files.
    for f in ["wallet.json", "positions.json"] {
        assert!(!std::fs::read_to_string(b.data.join(f)).unwrap().contains(&k0), "{f}");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_refused_check_or_claim_shows_no_details_and_cancel_goes_to_the_wallet() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    w.borrow_mut().sources.push(TP.into());
    let (mut b, root) = bridge("refused", vec![deposit_log(&commitment_of(0), 5, 0), deposit_log(&commitment_of(1), 6, 0)]);
    let rootword = hex0x(&util::sha256(&[b"root", &6u64.to_be_bytes()]));
    let dir = b.data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &rootword[2..14]));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("eth_getProof.json"), "{}").unwrap();
    std::fs::write(dir.join("header.rlp.hex"), "0x").unwrap();
    let found = ok(&b.recover(V.into(), "20".into()));
    assert_eq!(found["found"].as_array().unwrap().len(), 2, "{found}");
    // Leaf 6's claim: the CLI's local diagnosis refuses it before anything is built.
    let id6 = found["found"].as_array().unwrap().iter().find(|d| d["key_index"] == 1).unwrap()["id"].as_str().unwrap().to_owned();
    let plan = ok(&b.claim_plan(id6.clone()));
    assert_eq!((plan["ok"].as_bool(), plan["error"]["code"].as_str()), (Some(false), Some("already_claimed")), "{plan}");
    assert_eq!(b.store().deposit(&id6).unwrap()["claimed"], true);
    assert!(err(&b.claim(id6, json!({ "claim_early": true, "confirm_weaker": true }).to_string())).contains("already claimed"));
    assert!(asked("propose_private_call").is_empty());
    // Leaf 5: proposed; the wallet's check fails; then a second one is cancelled.
    let id5 = found["found"].as_array().unwrap().iter().find(|d| d["key_index"] == 0).unwrap()["id"].as_str().unwrap().to_owned();
    let job = ok(&b.claim(id5.clone(), json!({ "claim_early": true, "confirm_weaker": true }).to_string()));
    w.borrow_mut().status.insert("pc-1".into(), json!({ "request": "pc-1", "state": "pending", "check": "refused", "error": "It takes 5 dUSDC from your account" }));
    let j = ok(&b.job(job["id"].as_str().unwrap().into()));
    assert_eq!(j["state"], "failed");
    assert_eq!(j["error"]["code"], "wallet_check");
    assert!(j["error"]["message"].as_str().unwrap().contains("Open the Names app"));
    assert!(!j.to_string().contains("5 dUSDC"));
    let job2 = ok(&b.claim(id5, json!({ "claim_early": true, "confirm_weaker": true }).to_string()));
    let c = ok(&b.cancel_job(job2["id"].as_str().unwrap().into()));
    assert_eq!(c["state"], "cancelled", "{c}");
    assert_eq!(asked("cancel_request").len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_burn_is_built_from_the_wallets_exit_value_and_proposed_only_where_it_can_be_released() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().sources.push(TP.into());
    let (mut b, root) = bridge("redeem", vec![]);
    let to = "0x00000000000000000000000000000000000000be";
    let first = ok(&b.plan_redeem("dUSDC".into(), "100".into(), to.into(), "0".into()));
    assert_eq!(first["needs"], "secret");
    assert_eq!(first["request"]["scope"], "RFP022/exit");
    w.borrow_mut().secret_grants.insert("RFP022/exit".into());
    let plan = ok(&b.plan_redeem("dUSDC".into(), "100".into(), to.into(), "0".into()));
    assert_eq!(plan["pieces"].as_array().unwrap().len(), 1, "{plan}");
    let cfg = util::hex32(CFG).unwrap();
    let (usdc, dest) = (util::eth_address(USDC).unwrap(), util::eth_address(to).unwrap());
    let ctx = util::to_hex(&wallet::exit_context(&cfg, &usdc, 100_000_000, &dest, 0));
    let a = asked("app_secret");
    assert_eq!(a.last().unwrap(), &json!([format!("Private/{V}"), "RFP022/exit", ctx, "RFP022/ek", 0, 1]));
    let k = value("RFP022/exit", &ctx, "RFP022/ek", 0);
    assert_eq!(plan["pieces"][0]["exit_commitment"], hex0x(&wallet::exit_commitment(&k, &dest, &usdc, 100_000_000, 0, &cfg)));
    assert!(!plan.to_string().contains(&util::to_hex(&k)));
    // The demo vault can never release: the call is built (to show it) and not proposed.
    let built = ok(&b.redeem(plan["id"].as_str().unwrap().into(), json!({ "confirm_weaker": true }).to_string()));
    assert_eq!(built["proposed"], false, "{built}");
    assert_eq!(built["call"]["declared"]["debits"][0]["amount"], "100000000");
    assert!(asked("propose_private_call").is_empty());
    let argv = std::fs::read_to_string(root.join("tmp/argv.log")).unwrap();
    let line = argv.lines().find(|l| l.contains(" burn ")).unwrap();
    assert!(line.contains("--json burn") && line.contains("--exit 0x") && line.contains("--emit-call") && !line.contains("--secret"), "{line}");
    // Where exits can be released, the piece is proposed and its index is used up.
    if let Some((_, reg)) = b.registry_cache.as_mut() {
        reg["vault"]["exits_possible"] = Value::from(true);
    }
    let plan2 = ok(&b.plan_redeem("dUSDC".into(), "100".into(), to.into(), "0".into()));
    let job = ok(&b.redeem(plan2["id"].as_str().unwrap().into(), json!({ "confirm_weaker": true }).to_string()));
    assert_eq!(job["wallet"], "names", "{job}");
    let p = asked("propose_private_call");
    assert_eq!(p.len(), 1);
    assert!(p[0][2].as_str().unwrap().starts_with("Redeem 100 dUSDC"));
    let st = b.wstate();
    assert_eq!(st["exit_next"].as_object().unwrap().values().next().unwrap(), 1);
    // The next plan takes index 1.
    let plan3 = ok(&b.plan_redeem("dUSDC".into(), "100".into(), to.into(), "0".into()));
    assert_eq!(plan3["pieces"][0]["key_index"], 1);
    let exits = ok(&b.exits());
    assert!(exits["exits"].as_array().unwrap().iter().any(|e| e["stage"] == "proposed"), "{exits}");
    // A burn proposal that got no answer is never proposed again from that plan (it could burn
    // twice if the first one is approved).
    stand_in::script("propose_private_call", vec![Value::from(stand_in::NO_ANSWER)]);
    let e = err(&b.redeem(plan3["id"].as_str().unwrap().into(), json!({ "confirm_weaker": true }).to_string()));
    assert!(e.contains("won't propose this piece again"), "{e}");
    let e = err(&b.redeem(plan3["id"].as_str().unwrap().into(), json!({ "confirm_weaker": true }).to_string()));
    assert!(e.contains("proposed already"), "{e}");
    assert_eq!(asked("propose_private_call").len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn recovery_reads_batches_past_the_gap() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    // Indexes 0 and 60: the second is within the gap of the first batch's end, so a second
    // batch is read; nothing past it.
    let (mut b, root) = bridge("recover", vec![deposit_log(&commitment_of(0), 5, 0), deposit_log(&commitment_of(60), 6, 0)]);
    let r = ok(&b.recover(V.into(), "20".into()));
    let idx: Vec<u64> = r["found"].as_array().unwrap().iter().map(|d| d["key_index"].as_u64().unwrap()).collect();
    assert_eq!(idx, [0, 60], "{r}");
    let calls: HashMap<String, usize> = asked("app_secret").iter().fold(HashMap::new(), |mut m, a| {
        *m.entry(format!("{}:{}", a[2].as_str().unwrap(), a[4])).or_default() += 1;
        m
    });
    assert_eq!(calls.get(&format!("{}:64", ctx_hex(100_000_000))), Some(&1), "{calls:?}");
    assert!(!calls.contains_key(&format!("{}:128", ctx_hex(100_000_000))));
    assert_eq!(r["exits"]["available"], false);
    let _ = std::fs::remove_dir_all(root);
}

/// `app_secret` and `granted_accounts` answering busy: asked again, the same value, one deposit;
/// a wallet busy past the cap says so (not "install the Names app") and keeps nothing.
#[test]
fn a_busy_wallet_is_asked_again_for_secrets_and_accounts() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let (mut b, root) = bridge("busy", vec![]);
    stand_in::script("granted_accounts", vec![stand_in::busy()]);
    stand_in::script("app_secret", vec![stand_in::busy(), stand_in::busy()]);
    let r = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    assert_eq!(r["commitment"], hex0x(&commitment_of(0)), "{r}");
    assert_eq!(asked("app_secret").len(), 3);
    assert_eq!(b.store().load()["deposits"].as_array().unwrap().len(), 1);
    // Busy past the cap: the busy words, nothing stored.
    stand_in::handle("app_secret", |_| stand_in::busy());
    let e = err(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    assert!(e.contains("Names Wallet is busy; nothing was sent; try again") && !e.contains("install"), "{e}");
    assert_eq!(b.store().load()["deposits"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

/// A private claim proposal that got no answer: not proposed again while it may be waiting in the
/// Names app (approved there, the wallet proves and sends it itself).
#[test]
fn a_private_claim_that_got_no_answer_is_not_proposed_again_at_once() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    w.borrow_mut().sources.push(TP.into());
    let (mut b, root) = bridge("claim-noanswer", vec![deposit_log(&commitment_of(0), 5, 0)]);
    let rootword = hex0x(&util::sha256(&[b"root", &5u64.to_be_bytes()]));
    let dir = b.data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &rootword[2..14]));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("eth_getProof.json"), "{}").unwrap();
    std::fs::write(dir.join("header.rlp.hex"), "0x").unwrap();
    let found = ok(&b.recover(V.into(), "20".into()));
    let id = found["found"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(ok(&b.claim_plan(id.clone()))["ok"], true);
    let opts = json!({ "claim_early": true, "confirm_weaker": true }).to_string();
    stand_in::script("propose_private_call", vec![Value::from(stand_in::NO_ANSWER)]);
    let e = err(&b.claim(id.clone(), opts.clone()));
    assert!(e.contains("may be waiting in the Names app"), "{e}");
    let e = err(&b.claim(id.clone(), opts.clone()));
    assert!(e.contains("doesn't propose it again"), "{e}");
    assert_eq!(asked("propose_private_call").len(), 1);
    // The person looked: proposed, once.
    let job = ok(&b.claim(id.clone(), json!({ "claim_early": true, "confirm_weaker": true, "after_uncertain": true }).to_string()));
    assert_eq!(job["state"], "running", "{job}");
    assert_eq!(asked("propose_private_call").len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

// ── Receive from Ethereum (modules/NAMES-RECEIVE.md) ─────────────────────

/// The deposit request carries the page's own code (same bytes as web/deposit's request.js), its
/// link and QR, and the digits to compare; with no desk the steps go to the vault as before.
#[test]
fn a_deposit_request_carries_the_page_s_code_and_link() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let (mut b, root) = bridge("code", vec![]);
    let r = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    let c = r["commitment"].as_str().unwrap().to_owned();
    let d = b.deployment().clone();
    let code = crate::request::encode(d.chain_id, &d.vault, &util::eth_address(USDC).unwrap(), 0, 100_000_000, &util::hex32(&c).unwrap()).unwrap();
    assert_eq!(r["request"], code.as_str());
    assert!(code.starts_with("logosdep1") && code.len() == 155, "{code}");
    assert_eq!(r["request_url"], format!("https://nameswallet.app/bridge/#{code}"));
    assert_eq!(r["request_qr"]["text"], r["request_url"]);
    assert_eq!(r["compare"]["first"], &c[2..8]);
    assert_eq!(r["compare"]["last"], &c[c.len() - 6..]);
    let cb = util::hex32(&c).unwrap();
    assert_eq!(r["compare"]["words"], json!(crate::compare::words(&cb)));
    assert_eq!(r["compare"]["words_text"], r["compare"]["words"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect::<Vec<_>>().join(" \u{b7} "));
    assert!(r["eip681"].is_string() && r["bridge_fee"].is_null(), "no desk on this deployment: {r}");
    assert_eq!(r["steps"][1]["to"], util::checksum_address(&d.vault));
    assert!(r["export_text"].as_str().unwrap().contains(&code));
    let _ = std::fs::remove_dir_all(root);
}

/// An Ethereum that fails the test if anything reads it.
struct NoEth;
impl EthSource for NoEth {
    fn describe(&self) -> String {
        "no reads allowed".into()
    }
    fn call(&self, _: &[u8; 20], _: &[u8]) -> Result<Vec<u8>, String> {
        panic!("receive_request read Ethereum")
    }
    fn block(&self, _: &str) -> Result<eth::BlockInfo, String> {
        panic!("receive_request read Ethereum")
    }
    fn block_by_hash(&self, _: &str) -> Result<Option<eth::BlockInfo>, String> {
        panic!("receive_request read Ethereum")
    }
    fn logs(&self, _: &[u8; 20], _: &[u8; 32], _: u64, _: u64) -> Result<Vec<Value>, String> {
        panic!("receive_request read Ethereum")
    }
    fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
        panic!("receive_request read Ethereum")
    }
    fn chain_id(&self) -> Result<u64, String> {
        panic!("receive_request read Ethereum")
    }
}

/// A LEZ endpoint that fails the test if anything calls it.
struct NoLez;
impl lez::LezRpc for NoLez {
    fn call(&self, method: &str, _: Value) -> Result<Value, lez::Problem> {
        panic!("receive_request called LEZ {method}")
    }
    fn describe(&self) -> String {
        "no calls allowed".into()
    }
}

/// `receive_request(id)` gives a stored deposit's code again, the same bytes as the reply that
/// made it, with no wallet call, no CLI run and no chain read; unknown ids and deposits whose
/// stored data can't rebuild the code are refused, never guessed.
#[test]
fn receive_request_rebuilds_the_same_code_without_any_call() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let (mut b, root) = bridge("again", vec![]);
    let first = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    let id = first["deposit"]["id"].as_str().unwrap().to_owned();
    // From here on, any wallet, CLI or chain call fails the test.
    let asked_before = stand_in::asked().len();
    let argv = root.join("tmp").join("argv.log");
    let argv_before = std::fs::read_to_string(&argv).unwrap_or_default();
    b.eth_override = Some(Box::new(NoEth));
    b.relayed.rpc_override = Some(std::sync::Arc::new(NoLez));
    let again = ok(&b.receive_request(id.clone()));
    assert_eq!(again["request"], first["request"], "byte for byte the first code");
    assert_eq!((&again["link"], &again["request_url"]), (&first["request_url"], &first["request_url"]));
    assert_eq!(again["request_qr"], first["request_qr"]);
    assert_eq!(again["compare"], first["compare"]);
    assert_eq!(again["compare"]["words"].as_array().unwrap().len(), 4);
    assert_eq!((again["symbol"].as_str(), again["denom_text"].as_str(), again["amount_text"].as_str()), (Some("dUSDC"), first["denom_text"].as_str(), Some("100 dUSDC")), "{again}");
    assert_eq!((again["stage"].as_str(), again["claimed"].as_bool(), again["id"].as_str()), (Some("unknown"), Some(false), Some(id.as_str())));
    assert_eq!(again["vault"], util::checksum_address(&b.deployment().vault));
    assert!(again.get("secret").is_none() && !again.to_string().contains("key_index"));
    assert_eq!(stand_in::asked().len(), asked_before, "no Names call");
    assert_eq!(std::fs::read_to_string(&argv).unwrap_or_default(), argv_before, "no CLI run");
    // The stage as receive_status last saw it.
    let mut row = b.store().deposit(&id).unwrap();
    row["receive"] = json!({ "stage": "claimable", "text": "Ready to claim." });
    b.store().put_deposit(row.clone()).unwrap();
    let again = ok(&b.receive_request(id.clone()));
    assert_eq!((again["stage"].as_str(), again["text"].as_str()), (Some("claimable"), Some("Ready to claim.")));
    // Unknown id; empty id.
    assert!(err(&b.receive_request("dep-nope".into())).contains("there is no deposit dep-nope"));
    assert!(err(&b.receive_request(" ".into())).contains("give a deposit id"));
    // A deposit from a build that kept no vault: refused, not guessed from today's deployment.
    let mut old = row.clone();
    old["id"] = json!("dep-old");
    for k in ["vault", "chain_id", "deployment", "request"] {
        old.as_object_mut().unwrap().remove(k);
    }
    b.store().put_deposit(old).unwrap();
    let e = err(&b.receive_request("dep-old".into()));
    assert!(e.contains("older build") && e.contains("Nothing was guessed"), "{e}");
    // A kept code that no longer matches the stored details: refused.
    let mut bad = row.clone();
    bad["id"] = json!("dep-bad");
    bad["denom_index"] = json!(1);
    b.store().put_deposit(bad).unwrap();
    assert!(err(&b.receive_request("dep-bad".into())).contains("does not match"));
    let _ = std::fs::remove_dir_all(root);
}

/// The rebuilt code equals the page's own vectors (web/deposit/test/vectors/request-v1.json) for
/// every valid vector whose amount and chain fit a stored deposit.
#[test]
fn receive_request_rebuilds_the_page_s_request_vectors() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w);
    let (mut b, root) = bridge("again-vectors", vec![]);
    b.eth_override = Some(Box::new(NoEth));
    let doc: Value = serde_json::from_str(include_str!("../../../../web/deposit/test/vectors/request-v1.json")).unwrap();
    let mut n = 0;
    for (i, v) in doc["valid"].as_array().unwrap().iter().enumerate() {
        let r = &v["request"];
        let (Ok(chain), Ok(amount)) = (r["chainId"].as_str().unwrap().parse::<u64>(), r["amount"].as_str().unwrap().parse::<u128>()) else { continue };
        let id = format!("dep-vec-{i}");
        b.store().put_deposit(json!({ "id": id, "eth_token": r["token"], "symbol": "X", "denom_index": r["denomIndex"], "denom": amount.to_string(),
            "commitment": r["commitment"], "chain_id": chain, "vault": r["vault"], "deployment": "elsewhere" })).unwrap();
        let again = ok(&b.receive_request(id));
        assert_eq!(again["request"], v["string"], "{}", v["name"]);
        assert_eq!(again["link"], format!("https://nameswallet.app/bridge/#{}", v["string"].as_str().unwrap()));
        assert!(again["amount_text"].is_null(), "no known deployment: no amount in words");
        n += 1;
    }
    assert!(n >= 3, "{n} vectors fit");
    let _ = std::fs::remove_dir_all(root);
}

/// A deployment with a deposit desk: the steps go through the desk with its quoted fee, and there
/// is no direct-to-vault link (it would skip the fee).
#[test]
fn a_desk_deployment_deposits_through_the_desk_with_its_fee() {
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let (mut b, root) = bridge("desk", vec![]);
    let desk = "0x6b7F38D05751b1ed432a0A0d377A63cFB8c4DBc5";
    b.cfg.deployment.desk = desk.into();
    let r = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), V.into()));
    assert!(r["eip681"].is_null() && r["qr"].is_null(), "{r}");
    assert_eq!(r["bridge_fee"]["amount"], "300000");
    assert_eq!(r["bridge_fee"]["total"], "100300000");
    assert_eq!(r["bridge_fee"]["percent_text"], "0.3%");
    assert_eq!(r["bridge_fee"]["text"], "Bridge fee 0.3 dUSDC (0.3% of 100 dUSDC); you send 100.3 dUSDC in all.");
    assert_eq!(r["total"], "100300000");
    // Approve the desk for the total, then depositWithFee on the desk (no LEZ key).
    let approve = r["steps"][0]["data"].as_str().unwrap();
    assert!(approve.starts_with("0x095ea7b3") && approve.contains(&desk[2..].to_lowercase()), "{approve}");
    assert_eq!(r["steps"][1]["to"], desk);
    assert!(r["steps"][1]["data"].as_str().unwrap().starts_with("0xa93bf283"));
    assert!(r["request"].as_str().unwrap().starts_with("logosdep1"), "the page's code is the same either way");
    let _ = std::fs::remove_dir_all(root);
}

/// A public account given to this app: the request is made from that account's values, kept as
/// public, with the "Not private" notice in the reply; it needs the confirmation; recovery finds it.
#[test]
fn a_public_account_receives_with_the_not_private_notice() {
    const P: &str = "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f";
    let w = Rc::new(RefCell::new(Wallet::default()));
    install(w.clone());
    connected(&w);
    w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
    let (mut b, root) = bridge("public", vec![]);
    assert!(err(&b.make_public_deposit_request("dUSDC".into(), "0".into(), format!("Public/{P}"), "false".into())).contains("confirm"));
    let r = ok(&b.make_public_deposit_request("dUSDC".into(), "0".into(), format!("Public/{P}"), "true".into()));
    assert_eq!(r["account_kind"], "public", "{r}");
    assert_eq!(asked("app_secret").last().unwrap()[0], format!("Public/{P}"));
    let k = value("RFP022/deposit", &ctx_hex(100_000_000), "RFP022/ck", 0);
    let c = wallet::deposit_commitment(&k, &util::lez_account(P).unwrap(), &util::eth_address(USDC).unwrap(), 100_000_000, &util::hex32(CFG).unwrap());
    assert_eq!(r["commitment"], hex0x(&c));
    assert_eq!(r["compare"]["words"], json!(crate::compare::words(&c)));
    assert_eq!(r["compare"]["words_text"], crate::compare::words_text(&c));
    assert_eq!(r["derivation"]["account"], format!("Public/{P}"));
    assert!(r["not_private"]["text"].as_str().unwrap().starts_with(crate::relayed::PRIVACY_LINE));
    let stored = b.store().load();
    assert_eq!((stored["deposits"][0]["account"].as_str(), stored["deposits"][0]["account_kind"].as_str()), (Some(P), Some("public")));
    assert!(stored["deposits"][0].get("secret").is_none());
    assert!(!r.to_string().contains(&util::to_hex(&k)), "no key in the reply");
    // A public account the wallet didn't give, the app already connected: an error with what to
    // do (the wallet makes no request for it), never a needs without a request.
    let other = "Public/2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC";
    let e = err(&b.make_public_deposit_request("dUSDC".into(), "0".into(), other.into(), "true".into()));
    assert!(e.contains("take this app's access back") && e.contains(other), "{e}");
    // Recovery from the public account's values finds the deposit once it is in the vault.
    let (mut b2, root2) = bridge("public-recover", vec![deposit_log(&c, 5, 0)]);
    let found = ok(&b2.recover(format!("Public/{P}"), "20".into()));
    assert_eq!(found["found"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["found"][0]["account_kind"], "public");
    let _ = (std::fs::remove_dir_all(root), std::fs::remove_dir_all(root2));
}

/// Every `needs` carries a request the wallet made; anything else is an error with a message
/// (2026-11-14 on a test machine: "needs public_account" with no sheet behind it).
#[test]
fn a_needs_always_carries_a_request_the_wallet_made() {
    const P: &str = "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f";
    let pending = json!({ "success": true, "value": { "request": "acc-9", "kind": "connect", "state": "pending" } });
    let fresh = || {
        let w = Rc::new(RefCell::new(Wallet::default()));
        install(w.clone());
        w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
        let (b, root) = bridge("needs", vec![]);
        (w, b, root)
    };
    let ask = |b: &mut Bridge| b.make_public_deposit_request("dUSDC".into(), "0".into(), format!("Public/{P}"), "true".into());
    // Not connected: the wallet makes a connect request (through request_access_with when listed).
    let (_w, mut b, root) = fresh();
    stand_in::answer("get_config", json!({ "features": ["request_access_with"] }));
    stand_in::answer("request_access_with", pending["value"].clone());
    let n = ok(&ask(&mut b));
    assert_eq!((n["needs"].as_str(), n["request"]["request"].as_str()), (Some("connect"), Some("acc-9")), "{n}");
    // request_access_with refused or not dispatched: the plain request_access, still a real request.
    for answer in [json!({ "success": false, "value": null, "error": "unknown option" }), Value::from(stand_in::UNREACHABLE)] {
        let (_w, mut b, root2) = fresh();
        stand_in::answer("get_config", json!({ "features": ["request_access_with"] }));
        stand_in::script("request_access_with", vec![answer]);
        let n = ok(&ask(&mut b));
        assert_eq!(n["request"]["request"], "acc-1", "{n}");
        let _ = std::fs::remove_dir_all(root2);
    }
    // Busy past the cap, or no answer: an error, no needs.
    let (_w, mut b, root3) = fresh();
    stand_in::handle("request_access", |_| stand_in::busy());
    assert!(err(&ask(&mut b)).contains("busy; nothing was sent"));
    let (_w, mut b, root4) = fresh();
    stand_in::script("request_access", vec![Value::from(stand_in::NO_ANSWER)]);
    assert!(err(&ask(&mut b)).contains("didn't answer"));
    // The wallet answers "approved" with no request: with the account, on; without it, an error.
    let (w, mut b, root5) = fresh();
    stand_in::answer("request_access", json!({ "request": null, "kind": "connect", "state": "approved", "accounts": [format!("Public/{P}")] }));
    // granted_accounts said "not connected", then request_access said approved with the account
    // (a race): the request goes on, no needs.
    w.borrow_mut().accounts = vec![format!("Public/{P}")];
    let r = ok(&ask(&mut b));
    assert!(r.get("needs").is_none() && r["commitment"].is_string(), "{r}");
    let (_w, mut b, root6) = fresh();
    stand_in::answer("request_access", json!({ "request": null, "kind": "connect", "state": "approved", "accounts": ["Private/x"] }));
    assert!(err(&ask(&mut b)).contains("made no request"));
    assert!(wallet::needs("connect", &json!({ "request": null }), "m").is_err());
    assert!(wallet::needs("connect", &json!({ "request": "" }), "m").is_err());
    assert!(wallet::needs("connect", &Value::Null, "m").is_err());
    for r in [root, root3, root4, root5, root6] {
        let _ = std::fs::remove_dir_all(r);
    }
}

/// Names with `grant_add_account`: an app that has access asks for one more public account with
/// request_access_with; each of the wallet's four answers is followed as it is meant.
#[test]
fn with_grant_add_account_the_wallet_is_asked_to_add_the_account() {
    const P2: &str = "Public/2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC";
    let setup = |name: &str| {
        let w = Rc::new(RefCell::new(Wallet::default()));
        install(w.clone());
        connected(&w);
        w.borrow_mut().secret_grants.insert("RFP022/deposit".into());
        stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "features": ["request_access_with", "grant_add_account"] }));
        let (b, root) = bridge(name, vec![]);
        (w, b, root)
    };
    let ask = |b: &mut Bridge| b.make_public_deposit_request("dUSDC".into(), "0".into(), P2.into(), "true".into());
    // 1. A real pending request: needs public_account with its id; the hint names the account.
    let (_w, mut b, r1) = setup("add-pending");
    stand_in::answer("request_access_with", json!({ "request": "add-1", "kind": "connect", "state": "pending", "account_hint": P2 }));
    let n = ok(&ask(&mut b));
    assert_eq!((n["needs"].as_str(), n["request"]["request"].as_str()), (Some("public_account"), Some("add-1")), "{n}");
    let a = asked("request_access_with");
    assert_eq!(a[0][1], json!({ "account_hint": P2 }).to_string());
    assert!(asked("request_access").is_empty(), "no fallback to request_access (it makes no request for an app that has access)");
    // 2. Not now: the screen polls wallet_request and shows the wallet's own words.
    let (w, mut b, r2) = setup("add-rejected");
    let words = format!("{P2} wasn't added (the person chose Not now); the app still sees the accounts it had");
    w.borrow_mut().status.insert("add-1".into(), json!({ "request": "add-1", "kind": "connect", "state": "rejected", "error": words }));
    let s = ok(&b.wallet_request("add-1".into()));
    assert_eq!((s["state"].as_str(), s["error"].as_str()), (Some("rejected"), Some(words.as_str())));
    // 3. An account the wallet doesn't hold: the wallet's error, not our revoke-and-retry text.
    let (_w, mut b, r3) = setup("add-not-held");
    let held = format!("the Names wallet doesn't hold {P2}, so it can't be added to this app's access");
    stand_in::refuse("request_access_with", &held);
    let e = err(&ask(&mut b));
    assert_eq!(e, held);
    // 4. Already given (approved, no request): carry on to the deposit request.
    let (w, mut b, r4) = setup("add-given");
    stand_in::script("granted_accounts", vec![json!({ "success": true, "value": { "connected": true, "accounts": [{ "account": format!("Private/{V}"), "account_kind": "private" }] } })]);
    w.borrow_mut().accounts.push(P2.into());
    stand_in::answer("request_access_with", json!({ "request": null, "kind": "connect", "state": "approved", "accounts": [P2] }));
    let r = ok(&ask(&mut b));
    assert!(r.get("needs").is_none() && r["commitment"].is_string(), "{r}");
    // A wallet that can't add an account: without request_access_with (not dispatched), or one
    // that answers "approved" and made nothing (the account still not in the grant): the
    // revoke-and-connect-again words. (Names lists no features to other apps, so nothing is
    // probed: the answer tells.)
    let (_w, mut b, r5) = setup("add-none");
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4" }));
    stand_in::script("request_access_with", vec![Value::from(stand_in::UNREACHABLE)]);
    let e = err(&ask(&mut b));
    assert!(e.contains("take this app's access back") && e.contains(P2), "{e}");
    let (_w, mut b, r6) = setup("add-old");
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4" }));
    stand_in::answer("request_access_with", json!({ "request": null, "kind": "connect", "state": "approved", "accounts": [format!("Private/{V}")] }));
    let e = err(&ask(&mut b));
    assert!(e.contains("take this app's access back") && e.contains(P2), "{e}");
    let _ = std::fs::remove_dir_all(r6);
    for r in [r1, r2, r3, r4, r5] {
        let _ = std::fs::remove_dir_all(r);
    }
}
