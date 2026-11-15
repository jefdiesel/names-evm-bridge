// SPDX-License-Identifier: MIT OR Apache-2.0
//! The relayed public claim (relayed.rs): the golden vector shared with the CLI and bridge_core,
//! the claim file's checks, the dry run against a stand-in LEZ endpoint and Sepolia, and the
//! claim job against a stand-in Names Wallet (request, approval, relay, busy, block).

use super::*;
use crate::lez::{LezRpc, Problem};
use crate::relayed::*;
use crate::util::{sha256, to_hex};
use crate::wallet::stand_in;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const VECTOR: &str = include_str!("../testdata/relayed-claim-v1.json");

fn h32(v: &Value) -> [u8; 32] {
    util::hex32(v.as_str().unwrap()).unwrap()
}

fn id32(v: &Value) -> [u8; 32] {
    util::lez_account(v.as_str().unwrap().trim_start_matches("Public/")).unwrap()
}

#[test]
fn the_module_builds_the_golden_vector_s_call() {
    let v: Value = serde_json::from_str(VECTOR).unwrap();
    let (i, e) = (&v["inputs"], &v["expected"]);
    let (cfg, k) = (h32(&i["cfg_id"]), h32(&i["claim_key"]));
    let token = util::eth_address(i["eth_token"].as_str().unwrap()).unwrap();
    let denom: u128 = i["denom"].as_str().unwrap().parse().unwrap();
    let claimer = id32(&i["claimer"]);
    let leaves: Vec<[u8; 32]> = i["leaves"].as_array().unwrap().iter().map(h32).collect();
    let index = i["leaf_index"].as_u64().unwrap();
    let c = wallet::deposit_commitment(&k, &claimer, &token, denom, &cfg);
    assert_eq!(hex0x(&c), e["commitment"]);
    let leaf = store::deposit_leaf(&cfg, &token, denom, &c);
    assert_eq!(hex0x(&leaf), e["leaf"]);
    let path = merkle_path(&leaves, index as usize);
    assert_eq!(path.iter().map(|p| hex0x(p)).collect::<Vec<_>>(), e["path"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect::<Vec<_>>());
    let root = merkle_root(&leaf, index, &path);
    assert_eq!(hex0x(&root), e["root"]);
    let nf = claim_nullifier(&k, &claimer, &token, denom);
    assert_eq!(hex0x(&nf), e["nullifier"]);
    assert_ne!(nf, claim_nullifier_pre_fix(&k), "the vector is the fixed bridge's formula");
    let bridge = id32(&i["bridge_program"]);
    assert_eq!(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), e["config_pda"]);
    assert_eq!(lez::b58(&lez::pda(&bridge, &lez::root_seed(&root))), e["root_pda"]);
    assert_eq!(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&nf))), e["nullifier_pda"]);
    let message = claim_message(&token, i["denom_index"].as_u64().unwrap() as u32, denom, &root, index, &path, &k, &cfg);
    assert_eq!(message.len() as u64, e["message_len"].as_u64().unwrap());
    let call = claim_call(&bridge, &id32(&i["token_program"]), &id32(&i["definition"]), &claimer, &root, &nf, &message, i["payer"].as_str().unwrap(),
        i["gas_limit"].as_u64().unwrap(), i["max_fee"].as_str().unwrap().parse().unwrap());
    assert_eq!(call, e["call"], "the module's call is the golden vector's (the CLI test turns it into claim --relay's transaction)");
}

// ── A stand-in bridge test network ─────────────────────────────────────

struct Net {
    accounts: Mutex<HashMap<String, Value>>,
    calls: Mutex<Vec<String>>,
    relay: Mutex<Vec<Result<Value, Problem>>>,
    tx_blocks: Mutex<HashMap<String, u64>>,
    nonces: Mutex<HashMap<String, u64>>,
    fee: Mutex<(u64, u64)>,
}

impl LezRpc for Net {
    fn describe(&self) -> String {
        "stand-in".into()
    }
    fn call(&self, method: &str, params: Value) -> Result<Value, Problem> {
        self.calls.lock().unwrap().push(method.to_owned());
        match method {
            "getAccount" => Ok(self.accounts.lock().unwrap().get(params[0].as_str().unwrap()).cloned().unwrap_or(Value::Null)),
            "getFeeState" => {
                let (e, s) = *self.fee.lock().unwrap();
                Ok(json!({ "base_fee_exec": e, "base_fee_stor": s }))
            }
            "bridge_relayInfo" => Ok(json!({ "payer": PAYER, "bridge": BRIDGE, "max_gas": 250000, "max_fee": "2500000" })),
            "bridge_relayClaim" => {
                let mut q = self.relay.lock().unwrap();
                if q.is_empty() { Ok(json!({ "ok": false, "reason": "no answer queued", "retry": false })) } else { q.remove(0) }
            }
            "getTransaction" => Ok(match self.tx_blocks.lock().unwrap().get(params[0].as_str().unwrap()) {
                Some(b) => json!(["tx", b]),
                None => Value::Null,
            }),
            "getAccountsNonces" => {
                let n = self.nonces.lock().unwrap();
                Ok(Value::from(params[0].as_array().unwrap().iter().map(|a| json!(n.get(a.as_str().unwrap()).copied().unwrap_or(0))).collect::<Vec<_>>()))
            }
            "getChannelId" => Ok(json!(CHANNEL)),
            _ => Err(Problem { code: "refused", message: format!("not served: {method}"), retry_after: None }),
        }
    }
}

const BRIDGE: &str = "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7";
const TP: &str = "3gF78HRJNCfabkWFiszahVVacq181aU8SSGua8Cj9ZE4";
const DEF_ETH: &str = "AuyvsTwpPsbfisc8DCHkhpWLwQg7iG13n4VNBsf3UAwJ";
const PAYER: &str = "1HHhGGKfxtpAL5Uno5QqowGy748EUkYS6QGLFgQMgNT";
/// Blocks after the deployment's vault creation block (12,122,976).
const BASE: u64 = 12_200_000;
const CHANNEL: &str = "2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374";

fn bytes_json(b: &[u8]) -> Value {
    Value::from(b.iter().map(|x| json!(x)).collect::<Vec<_>>())
}

/// The Config's borsh bytes (the inverse of `lez::decode_config`).
fn config_bytes(d: &config::Deployment, mint_frozen: bool) -> Vec<u8> {
    config_bytes_on(d, mint_frozen, [0; 32])
}

/// [`config_bytes`] of a bridge initialised for LEZ chain `lez_chain_id` (zero: a bridge from
/// before the 2026-11-14 security fixes, as the live v2 demo).
fn config_bytes_on(d: &config::Deployment, mint_frozen: bool, lez_chain_id: [u8; 32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(util::hex32(&d.cfg_id).unwrap());
    b.extend(d.vault);
    b.extend(d.chain_id.to_le_bytes());
    b.extend(lez_chain_id);
    b.extend([0u8; 32]); // zone_id
    b.extend(util::lez_account(TP).unwrap());
    b.extend([7u8; 32]); // admin
    b.push(0); // pending_admin
    b.extend([7u8; 32]);
    b.extend([7u8; 32]);
    b.push(u8::from(mint_frozen));
    b.push(0);
    b.extend(1u32.to_le_bytes());
    b.extend([0u8; 20]); // ETH
    b.extend(util::lez_account(DEF_ETH).unwrap());
    b.push(18);
    b.extend(3u32.to_le_bytes());
    for d in [10u128.pow(16), 10u128.pow(17), 10u128.pow(18)] {
        b.extend(d.to_le_bytes());
    }
    for _ in 0..2 {
        b.extend(3u32.to_le_bytes());
        b.extend([0u8; 24]);
    }
    b.extend([0u8; 32]);
    b.extend([1, 0, 0]);
    b.push(0); // stand-in
    b.extend(25_000_000u128.to_le_bytes());
    b
}

struct FakeEth {
    logs: Vec<Value>,
    latest: u64,
    finalized: u64,
}

impl eth::EthSource for FakeEth {
    fn describe(&self) -> String {
        "fake Sepolia".into()
    }
    fn call(&self, _to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        assert_eq!(&data[..4], &util::selector("depositCount()"));
        Ok(eth::word_u64(self.logs.len() as u64).to_vec())
    }
    fn block(&self, tag: &str) -> Result<eth::BlockInfo, String> {
        let n = if tag == "finalized" { self.finalized } else { self.latest };
        Ok(eth::BlockInfo { number: n, hash: format!("0x{:064x}", n), timestamp: 1_800_000_000 })
    }
    fn block_by_hash(&self, _: &str) -> Result<Option<eth::BlockInfo>, String> {
        Ok(None)
    }
    fn logs(&self, _a: &[u8; 20], _t: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
        Ok(self.logs.iter().filter(|l| { let b = u64::from_str_radix(l["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap(); b >= from && b <= to }).cloned().collect())
    }
    fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
        Ok(0)
    }
    fn chain_id(&self) -> Result<u64, String> {
        Ok(11_155_111)
    }
}

struct World {
    b: Bridge,
    net: Arc<Net>,
    claimer: String,
    k: [u8; 32],
    file: String,
    roots: Vec<[u8; 32]>,
    _dir: PathBuf,
}

fn log(c: &[u8; 32], leaf_index: u64, root: &[u8; 32], block: u64) -> Value {
    let mut data = Vec::new();
    data.extend(eth::word_u64(0));
    data.extend(c);
    data.extend(eth::word_u64(leaf_index));
    data.extend(root);
    json!({ "topics": [hex0x(&eth::deposit_topic()), hex0x(&[0u8; 32])], "data": hex0x(&data), "blockNumber": format!("0x{block:x}"), "transactionHash": format!("0x{:064x}", leaf_index + 1) })
}

/// A deposit of 0.01 ETH to a never-used public account, at leaf 2 of 4 (all final unless
/// `final_upto` says otherwise), with the root after leaf 3 recorded on LEZ.
fn world(name: &str, final_upto: u64) -> World {
    stand_in::reset();
    let dir = std::env::temp_dir().join(format!("lez-bridge-relayed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = config::builtin("bridge-test-network").unwrap();
    let cfg = util::hex32(&d.cfg_id).unwrap();
    let claimer_b = sha256(&[b"claimer", name.as_bytes()]);
    let claimer = lez::b58(&claimer_b);
    let k = sha256(&[b"k", name.as_bytes()]);
    let c = wallet::deposit_commitment(&k, &claimer_b, &[0; 20], 10u128.pow(16), &cfg);
    let others: Vec<[u8; 32]> = (0..3u8).map(|i| sha256(&[b"other", &[i]])).collect();
    let commitments = [others[0], others[1], c, others[2]];
    let leaves: Vec<[u8; 32]> = commitments.iter().map(|x| store::deposit_leaf(&cfg, &[0; 20], 10u128.pow(16), x)).collect();
    let mut roots = Vec::new();
    let mut logs = Vec::new();
    for j in 0..4 {
        let path = merkle_path(&leaves[..=j], j);
        let root = merkle_root(&leaves[j], j as u64, &path);
        roots.push(root);
        logs.push(log(&commitments[j], j as u64, &root, BASE + 100 + j as u64));
    }
    let bridge = util::lez_account(BRIDGE).unwrap();
    let mut accounts = HashMap::new();
    accounts.insert(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), json!({ "nonce": 0, "data": { "actor_states": { BRIDGE: bytes_json(&config_bytes(&d, false)) } } }));
    accounts.insert(lez::b58(&lez::pda(&bridge, &lez::root_seed(&roots[3]))), json!({ "nonce": 0, "data": { "actor_states": { BRIDGE: [1] } } }));
    let net = Arc::new(Net { accounts: Mutex::new(accounts), calls: Mutex::new(vec![]), relay: Mutex::new(vec![]), tx_blocks: Mutex::new(HashMap::new()), nonces: Mutex::new(HashMap::new()), fee: Mutex::new((8, 1)) });
    let mut cfgm = Config::default();
    cfgm.deployment = d.clone();
    let mut b = Bridge {
        data: dir.join("data"),
        cfg: cfgm,
        fixtures: None,
        registry_cache: None,
        eth_override: Some(Box::new(FakeEth { logs, latest: BASE + 110, finalized: BASE + final_upto })),
        relayed: Default::default(),
    };
    b.relayed.rpc_override = Some(net.clone());
    let file = json!({
        "format": "logos-bridge-claim", "version": 1, "warning": "…", "network": "Sepolia", "chainId": "11155111",
        "vault": "0x8c8b2664de28e5287158db4db65d448638871b74", "cfgId": d.cfg_id, "token": { "address": "0x0000000000000000000000000000000000000000", "symbol": "ETH", "decimals": 18 },
        "denominationIndex": 0, "amount": "10000000000000000", "amountText": "0.01 ETH", "logosAccount": format!("Public/{claimer}"),
        "claimKey": hex0x(&k), "commitment": hex0x(&c), "createdAt": "2026-11-14T00:00:00Z",
        "deposit": { "tx": format!("0x{:064x}", 3), "block": (BASE + 102).to_string(), "leafIndex": "2" },
    })
    .to_string();
    World { b, net, claimer, k, file, roots, _dir: dir }
}

/// Request states the stand-in wallet answers `request_status` with (default: pending).
fn requests() -> std::rc::Rc<std::cell::RefCell<HashMap<String, Value>>> {
    let map = std::rc::Rc::new(std::cell::RefCell::new(HashMap::<String, Value>::new()));
    let m = map.clone();
    stand_in::handle("request_status", move |args| {
        let v = m.borrow().get(args[0].as_str().unwrap_or_default()).cloned().unwrap_or(json!({ "state": "pending" }));
        json!({ "success": true, "value": v, "error": null })
    });
    map
}

fn wallet_connected(claimer: &str, give: bool) {
    let accounts = if give { json!([{ "account": format!("Public/{claimer}"), "account_kind": "public" }]) } else { json!([{ "account": "Public/6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f", "account_kind": "public" }]) };
    stand_in::answer("granted_accounts", json!({ "connected": true, "accounts": accounts }));
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "zone": { "channel": CHANNEL } }));
}

#[test]
fn the_dry_run_checks_everything_and_builds_the_call_without_sending() {
    let mut w = world("dry", 200);
    wallet_connected(&w.claimer, true);
    let v = w.b.relayed_check_now(&w.file);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["deposit"]["leaf_index"], 2);
    assert_eq!(v["deposit"]["amount_text"], "0.01 ETH");
    assert_eq!(v["root"], hex0x(&w.roots[3]), "the newest recorded root");
    assert_eq!(v["network"], "LEZ 0.4 · bridge test network");
    assert_eq!(v["relay"]["payer"], format!("Public/{PAYER}"));
    assert_eq!(v["privacy"], PRIVACY_LINE);
    assert_eq!(v["wallet"]["needs"], Value::Null);
    let shown = v.to_string();
    assert!(!shown.contains(&to_hex(&w.k)), "no claim key in the reply");
    assert!(!w.net.calls.lock().unwrap().iter().any(|m| m == "bridge_relayClaim"), "the dry run never posts to the relay");
    assert!(stand_in::asked().iter().all(|(m, _)| !m.starts_with("propose") && m != "declare_programs"), "nothing proposed");
    // The call it keeps is the golden vector's shape: no `new` entry, the relay's payer.
    let c = &w.b.relayed.checks[v["id"].as_str().unwrap()].call;
    assert!(c["accounts"].as_array().unwrap().iter().all(|a| a.get("new").is_none()));
    assert_eq!(c["privacy"], "bridge_public_claim");
    assert_eq!(c["fee"]["gas_limit"], 250000);
    // A file path works too, and nothing of the file is written by the module.
    let path = w._dir.join("claim.json");
    std::fs::write(&path, &w.file).unwrap();
    assert_eq!(w.b.relayed_check_now(&format!("file://{}", path.display()))["ok"], true);
}

#[test]
fn each_refusal_says_what_happened() {
    let code = |w: &mut World, src: &str| w.b.relayed_check_now(src)["error"]["code"].as_str().unwrap_or("none").to_owned();
    // Not final on Sepolia yet (finalized up to block 101; the deposit is in 102).
    let mut w = world("notfinal", 101);
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "not_final");
    // Root not recorded yet.
    let mut w = world("noroot", 200);
    let bridge = util::lez_account(BRIDGE).unwrap();
    w.net.accounts.lock().unwrap().remove(&lez::b58(&lez::pda(&bridge, &lez::root_seed(&w.roots[3]))));
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "root_not_attested");
    // An older recorded root that includes the deposit is used.
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::root_seed(&w.roots[2]))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    let v = w.b.relayed_check_now(&w.file);
    assert_eq!(v["root"], hex0x(&w.roots[2]), "{v}");
    // Already claimed.
    let mut w = world("claimed", 200);
    let nf = claim_nullifier_pre_fix(&w.k); // the fixture's bridge is pre-fix (zero chain id)
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&nf))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "already_claimed");
    // Minting paused.
    let mut w = world("frozen", 200);
    let d = config::builtin("bridge-test-network").unwrap();
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), json!({ "data": { "actor_states": { BRIDGE: bytes_json(&config_bytes(&d, true)) } } }));
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "minting_frozen");
    // Fees above the relay's cap.
    let mut w = world("fees", 200);
    *w.net.fee.lock().unwrap() = (20, 1);
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "relay_cap");
    // The file itself.
    let mut w = world("file", 200);
    let f: Value = serde_json::from_str(&w.file).unwrap();
    let with = |k: &str, v: Value| { let mut g = f.clone(); g[k] = v; g.to_string() };
    assert_eq!(code(&mut w, &with("commitment", json!(hex0x(&[9; 32])))), "bad_file");
    let private = with("logosAccount", json!(format!("Private/{}", w.claimer)));
    assert_eq!(code(&mut w, &private), "private_account");
    assert_eq!(code(&mut w, &with("vault", json!("0x0000000000000000000000000000000000000001"))), "other_vault");
    assert_eq!(code(&mut w, &with("chainId", json!("1"))), "other_chain");
    assert_eq!(code(&mut w, &with("masterSecret", json!(hex0x(&[1; 32])))), "bad_file");
    assert_eq!(code(&mut w, &with("format", json!("other"))), "bad_file");
    let mut g = f.clone();
    g["deposit"]["leafIndex"] = json!("1");
    assert_eq!(code(&mut w, &g.to_string()), "wrong_leaf");
    assert_eq!(code(&mut w, "not json"), "bad_file");
    assert_eq!(code(&mut w, "relative/path.json"), "bad_file");
    // A refusal never carries the key.
    let v = w.b.relayed_check_now(&with("commitment", json!(hex0x(&[9; 32]))));
    assert!(!v.to_string().contains(&to_hex(&w.k)));
}

fn status(b: &mut Bridge, id: &str) -> Value {
    b.relayed_status_now(id).unwrap()
}

#[test]
fn the_claim_job_goes_from_approval_through_the_relay_to_a_block() {
    let mut w = world("job", 200);
    let check = w.b.relayed_check_now(&w.file);
    let cid = check["id"].as_str().unwrap().to_owned();

    // Not connected: the wallet is asked for access.
    stand_in::answer("granted_accounts", json!({ "connected": false, "accounts": [] }));
    stand_in::answer("request_access", json!({ "request": "acc-1", "state": "pending" }));
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(r["needs"], "connect", "{r}");
    // Connected, but without this account: the wallet can't add it to the access it gave (asking
    // makes no request), so an error that says what to do, never a needs with nothing to approve.
    wallet_connected(&w.claimer, false);
    let e = w.b.relayed_claim_now(&cid, &json!({})).unwrap_err();
    assert!(e.contains("take this app's access back") && e.contains(&w.claimer), "{e}");
    // The account given: the programs are declared first (both), once.
    wallet_connected(&w.claimer, true);
    stand_in::answer("declare_programs", json!({ "request": "decl-1", "state": "pending" }));
    let req = requests();
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(r["needs"], "programs");
    let declared = stand_in::asked().into_iter().find(|(m, _)| m == "declare_programs").unwrap().1;
    assert_eq!(declared[0].as_str().unwrap(), json!([BRIDGE, TP]).to_string());
    // Approved: the call is proposed with the relay's payer, the privacy key and the wording.
    req.borrow_mut().insert("decl-1".into(), json!({ "state": "approved" }));
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending", "kind": "relayed_call" }));
    let job = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(job["phase"], "approve", "{job}");
    let proposed = stand_in::asked().into_iter().find(|(m, _)| m == "propose_relayed_call").unwrap().1;
    assert_eq!(proposed[0], format!("Public/{}", w.claimer));
    let call: Value = serde_json::from_str(proposed[1].as_str().unwrap()).unwrap();
    assert_eq!(call["fee"]["payer"], format!("Public/{PAYER}"));
    assert!(proposed[2].as_str().unwrap().contains("the bridge's relay sends it and pays the fee"));
    let jid = job["id"].as_str().unwrap().to_owned();

    // Waiting for the person.
    assert!(status(&mut w.b, &jid)["note"].as_str().unwrap().contains("Names app"));
    // Approved and signed: the relay is "busy" (stale payer nonce), the wallet re-signs the
    // same call without a new approval, the relay takes it, it lands.
    req.borrow_mut().insert("rel-1".into(), json!({ "state": "done", "transaction": "aa11", "hash": "11".repeat(32) }));
    stand_in::answer("resign_relayed_call", json!({ "state": "done", "transaction": "bb22", "hash": "22".repeat(32), "resigns": 1 }));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": false, "reason": "stale nonce: rebuild the claim and send it again", "retry": true })));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) })));
    let s = status(&mut w.b, &jid);
    assert_eq!((s["phase"].as_str(), s["resigns"].as_u64()), (Some("relay"), Some(0)), "{s}");
    assert!(s["note"].as_str().unwrap().contains("busy"), "{s}");
    // Nothing is re-signed while the relay's other claim hasn't landed (its nonce unchanged).
    let s = status(&mut w.b, &jid);
    assert_eq!(s["resigns"], 0, "{s}");
    // It lands: the payer's nonce moves, the wallet re-signs, the relay takes the new one.
    w.net.nonces.lock().unwrap().insert(PAYER.to_owned(), 1);
    let s = status(&mut w.b, &jid);
    assert_eq!(s["resigns"], 1, "{s}");
    assert_eq!(s["phase"], "confirm", "{s}");
    assert_eq!(s["tx"], "cd".repeat(32));
    // In a block, the nullifier spent, the wrapped ETH in the account and no native LEZ.
    w.net.tx_blocks.lock().unwrap().insert("cd".repeat(32), 77);
    let bridge = util::lez_account(BRIDGE).unwrap();
    let nf = claim_nullifier_pre_fix(&w.k); // the fixture's bridge is pre-fix (zero chain id)
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&nf))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    let mut holding = vec![0u8, 1, 0, 0, 0];
    holding.extend(util::lez_account(DEF_ETH).unwrap());
    holding.extend(10u128.pow(16).to_le_bytes());
    w.net.accounts.lock().unwrap().insert(w.claimer.clone(), json!({ "nonce": 1, "data": { "actor_states": { TP: bytes_json(&holding) } } }));
    let s = status(&mut w.b, &jid);
    assert_eq!(s["state"], "done", "{s}");
    assert_eq!(s["block"], 77);
    assert_eq!(s["result"]["balance_text"], "0.01 ETH");
    assert_eq!(s["result"]["native"], "0");
    // The relay saw the re-signed transaction, never the first one twice.
    assert_eq!(w.net.calls.lock().unwrap().iter().filter(|m| *m == "bridge_relayClaim").count(), 2);
    // No key or signed transaction on disk, the key gone from memory once done.
    let kept = std::fs::read_to_string(w.b.data.join("wallet.json")).unwrap();
    assert!(!kept.contains(&to_hex(&w.k)) && !kept.contains("bb22") && !kept.contains("aa11"), "{kept}");
    assert!(w.b.relayed.checks.is_empty());
    // A repeat is refused before anything is proposed.
    assert_eq!(w.b.relayed_check_now(&w.file)["error"]["code"], "already_claimed");
}

#[test]
fn past_what_the_wallet_re_signs_the_person_is_asked_again_and_relay_refusals_are_named() {
    let mut w = world("again", 200);
    wallet_connected(&w.claimer, true);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    let mut st = w.b.wstate();
    st["relay_declared"] = json!({ "programs": [BRIDGE, TP], "request": "decl-1" });
    w.b.put_wstate(&st).unwrap();
    let req = requests();
    req.borrow_mut().insert("decl-1".into(), json!({ "state": "approved" }));
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    req.borrow_mut().insert("rel-1".into(), json!({ "state": "done", "transaction": "aa11", "hash": "11".repeat(32) }));
    stand_in::refuse_with_code("resign_relayed_call", "it was signed again 3 times already: the wallet won't sign it again without the person. Propose it again (propose_relayed_call).", "ask_again");
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-2", "state": "pending" }));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": false, "reason": "stale nonce: rebuild the claim and send it again", "retry": true })));
    let s = status(&mut w.b, &jid);
    assert_eq!(s["phase"], "relay", "{s}");
    w.net.nonces.lock().unwrap().insert(PAYER.to_owned(), 1);
    let s = status(&mut w.b, &jid);
    assert_eq!(s["phase"], "approve", "{s}");
    assert_eq!(s["request"], "rel-2");
    assert_eq!(s["proposals"], 2);
    assert!(s["note"].as_str().unwrap().contains("Names app"), "{s}");
    // The relay busy (HTTP 503): the same transaction is asked again later, nothing re-signed.
    req.borrow_mut().insert("rel-2".into(), json!({ "state": "done", "transaction": "cc33", "hash": "33".repeat(32) }));
    w.net.relay.lock().unwrap().push(Err(Problem { code: "relay_busy", message: "the bridge's relay is busy with another claim".into(), retry_after: Some(10) }));
    let s = status(&mut w.b, &jid);
    assert_eq!(s["phase"], "relay", "{s}");
    assert!(s["note"].as_str().unwrap().contains("Asking again in 10 s"));
    // Named refusals.
    for (reason, code) in [
        ("the relay's daily cap is reached; try tomorrow (UTC) or pay the fee yourself", "relay_cap"),
        ("deposit already claimed: its nullifier is spent; nothing sent", "already_claimed"),
        ("this claim was already relayed (tx ab); pay its fee yourself if it did not apply", "relay_once"),
        ("the relay is out of funds; try later or pay the fee yourself", "relay_funds"),
        ("the claim does not apply in simulation: x", "relay_refused"),
    ] {
        let mut job = json!({ "id": "x", "request": "r", "check": "none", "state": "running", "phase": "relay" });
        w.b.relayed_refused_for_test(&mut job, reason);
        assert_eq!(job["error"]["code"], code, "{reason}");
    }
}

impl Bridge {
    fn relayed_refused_for_test(&mut self, job: &mut Value, reason: &str) {
        let mut j = job.clone();
        crate::relayed::test_refused(self, &mut j, reason);
        *job = j;
    }
}

/// The hand-made render fixtures (modules/fixtures/v2-demo/relayed_*) have the shapes the module
/// really answers: the same keys as a real check and a real job view.
#[test]
fn the_render_fixtures_have_the_module_s_shapes() {
    let fx = |name: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(format!("{}/../../fixtures/v2-demo/{name}.json", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap() };
    let keys = |v: &Value| -> Vec<String> { let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect(); k.sort(); k };
    let mut w = world("fixtures", 200);
    wallet_connected(&w.claimer, true);
    let real = w.b.relayed_check_now(&w.file);
    assert_eq!(keys(&fx("relayed_claim_check")["value"]), keys(&real));
    assert_eq!(keys(&fx("relayed_claim_check")["value"]["deposit"]), keys(&real["deposit"]));
    assert_eq!(keys(&fx("relayed_claim_check")["value"]["relay"]), keys(&real["relay"]));
    assert_eq!(keys(&fx("relayed_claim_check")["value"]["wallet"]), keys(&real["wallet"]));
    let job = crate::relayed::job_view(&json!({ "id": "x", "phase": "approve", "state": "running", "meta": {} }), 0);
    for name in ["relayed_claim_status__approve", "relayed_claim_status__relay", "relayed_claim_status__done", "relayed_claim_status__failed"] {
        assert_eq!(keys(&fx(name)["value"]), keys(&job), "{name}");
    }
    let refused = w.b.relayed_check_now("{}");
    assert_eq!(keys(&fx("relayed_claim_check__not_final")["value"]), keys(&refused));
}

/// e2e rehearsal (modules/tests/e2e-names-relayed.sh, build host only): the module's real check
/// against a throwaway chain's endpoint and a local Ethereum node, no Names Wallet. Writes the call
/// the wallet would get to `$E2E_OUT/call.json` (0600; it carries the claim key; the script deletes
/// it) and the public view to `$E2E_OUT/check.json`.
#[test]
#[ignore]
fn e2e_check_from_env() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    stand_in::reset();
    stand_in::answer("granted_accounts", json!({ "connected": false, "accounts": [] }));
    stand_in::answer("get_config", json!({}));
    let d = config::Deployment::from_json(&util::read_json(std::path::Path::new(&var("E2E_DEPLOYMENT"))).unwrap()).unwrap();
    let out = PathBuf::from(var("E2E_OUT"));
    let mut cfg = Config::default();
    cfg.eth_rpc = vec![(d.chain_id, var("E2E_ETH_RPC"))];
    cfg.deployment = d;
    let mut b = Bridge { data: out.join("module-data"), cfg, fixtures: None, registry_cache: None, eth_override: None, relayed: Default::default() };
    let v = b.relayed_check_now(&var("E2E_CLAIM"));
    std::fs::write(out.join("check.json"), serde_json::to_string_pretty(&v).unwrap()).unwrap();
    if let Some(c) = v["id"].as_str().and_then(|id| b.relayed.checks.get(id)) {
        util::write_private(&out.join("call.json"), c.call.to_string().as_bytes()).unwrap();
    }
    println!("CHECK {}", v["ok"]);
    // Reads only: what the relay would be asked to pay with, on the deployment's endpoint. Never
    // bridge_relayClaim (this test has no route to it: no wallet, no job).
    if std::env::var("E2E_PUBLIC_READS").is_ok() {
        let rpc = lez::Http::new(&b.cfg.deployment.lez_rpc);
        let info = lez::relay_info(&rpc).unwrap();
        let fees = rpc.call("getFeeState", json!([])).unwrap();
        let exec = u128::from(fees["base_fee_exec"].as_u64().unwrap());
        let stor = u128::from(fees["base_fee_stor"].as_u64().unwrap());
        let reserve = u128::from(info.max_gas) * exec + 2075 * stor;
        let cfg = lez::bridge_config(&rpc, &util::lez_account(&b.cfg.deployment.bridge_program).unwrap()).unwrap().unwrap();
        println!("RELAY payer {} max_gas {} max_fee {}; head fees exec {exec} stor {stor}; reserve at the gas cap {reserve} (fits: {})", info.payer, info.max_gas, info.max_fee, reserve <= info.max_fee);
        println!("CONFIG token program {} tokens {} root source {} mint frozen {}", lez::b58(&cfg.token_program), cfg.tokens.len(), cfg.root_source, cfg.mint_frozen);
        println!("TREE {}", crate::relayed::vault_tree_report(&mut b).map_err(|r| r.message).unwrap());
        assert!(!b.relayed.checks.is_empty() || v["ok"] != true);
    }
}

/// The wallet's `landed_or_used` re-sign answer: the chain is asked (the nullifier), never a loop.
#[test]
fn a_re_sign_refused_as_landed_or_used_asks_the_chain() {
    let mut w = world("landed", 200);
    wallet_connected(&w.claimer, true);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    let mut st = w.b.wstate();
    st["relay_declared"] = json!({ "programs": [BRIDGE, TP], "request": "decl-1" });
    w.b.put_wstate(&st).unwrap();
    let req = requests();
    req.borrow_mut().insert("decl-1".into(), json!({ "state": "approved" }));
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    req.borrow_mut().insert("rel-1".into(), json!({ "state": "done", "transaction": "aa11", "hash": "11".repeat(32) }));
    // Names 0.17 (54be22c): the reply carries `code`; matched first.
    stand_in::refuse_with_code("resign_relayed_call", "your account was used since the approval", "landed_or_used");
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": false, "reason": "stale nonce: rebuild the claim and send it again", "retry": true })));
    assert_eq!(status(&mut w.b, &jid)["phase"], "relay");
    w.net.nonces.lock().unwrap().insert(PAYER.to_owned(), 1);
    // Still unclaimed: a plain failure, no new proposal.
    let s = status(&mut w.b, &jid);
    assert_eq!(s["error"]["code"], "landed_or_used", "{s}");
    assert_eq!(stand_in::asked().iter().filter(|(m, _)| m == "propose_relayed_call").count(), 1);
}

/// The `sequencer_stake` config account's JSON for a chain whose channel id is `channel`.
fn stake_config(channel: [u8; 32]) -> (String, Value) {
    let program = sha256(&[b"/LEE-BuiltinProgram/v1/AccountId", b"sequencer_stake"]);
    let id = lez::b58(&lez::pda(&program, b"/LEZ/v0.3/MinSequencerStake/0000"));
    let mut b = vec![1u8]; // channel_params: Some
    b.extend(149u128.to_le_bytes());
    b.extend(10u32.to_le_bytes());
    b.extend(20u32.to_le_bytes());
    b.extend(100u64.to_le_bytes());
    b.extend(1_000_000u64.to_le_bytes());
    b.push(1); // channel_id: Some
    b.extend(channel);
    b.extend(0u32.to_le_bytes()); // entries: empty
    (id, json!({ "data": { "actor_states": { lez::b58(&program): bytes_json(&b) } } }))
}

/// A bridge from after the 2026-11-14 security fixes (non-zero chain id): the module checks the
/// chain's channel id against the bridge's, and uses the nullifier bound to the claimer.
#[test]
fn a_fixed_bridge_is_checked_against_the_chain_and_uses_the_bound_nullifier() {
    let code = |w: &mut World, src: &str| w.b.relayed_check_now(src)["error"]["code"].as_str().unwrap_or("none").to_owned();
    let channel = [0xC4u8; 32];
    let bridge = util::lez_account(BRIDGE).unwrap();
    let fixed = |w: &mut World| {
        let d = w.b.deployment().clone();
        w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), json!({ "nonce": 0, "data": { "actor_states": { BRIDGE: bytes_json(&config_bytes_on(&d, false, channel)) } } }));
    };
    // The chain names this bridge's channel: the check passes, with the bound nullifier.
    let mut w = world("fixed-ok", 200);
    fixed(&mut w);
    let (id, acct) = stake_config(channel);
    w.net.accounts.lock().unwrap().insert(id, acct);
    let v = w.b.relayed_check_now(&w.file);
    assert_eq!(v["ok"], true, "{v}");
    let c = &w.b.relayed.checks[v["id"].as_str().unwrap()];
    let claimer = sha256(&[b"claimer", b"fixed-ok"]);
    assert_eq!(c.nullifier, claim_nullifier(&w.k, &claimer, &[0; 20], 10u128.pow(16)));
    assert_ne!(c.nullifier, claim_nullifier_pre_fix(&w.k));
    // Its nullifier spent: already claimed. The old formula's PDA spent says nothing.
    let mut w = world("fixed-old", 200);
    fixed(&mut w);
    let (id, acct) = stake_config(channel);
    w.net.accounts.lock().unwrap().insert(id, acct);
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&claim_nullifier_pre_fix(&w.k)))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "none");
    // Another chain (a copy at the same program id): refused.
    let mut w = world("fixed-copy", 200);
    fixed(&mut w);
    let (id, acct) = stake_config([0xC5; 32]);
    w.net.accounts.lock().unwrap().insert(id, acct);
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "wrong_chain");
    // A chain that records no channel id: refused too.
    let mut w = world("fixed-none", 200);
    fixed(&mut w);
    assert_eq!({ let f = w.file.clone(); code(&mut w, &f) }, "wrong_chain");
}

#[test]
fn the_channel_id_decoder_reads_both_shapes() {
    let (_, acct) = stake_config([9; 32]);
    let program = sha256(&[b"/LEE-BuiltinProgram/v1/AccountId", b"sequencer_stake"]);
    let bytes = lez::actor_state(&acct, &lez::b58(&program)).unwrap();
    assert_eq!(lez::decode_channel_id(&bytes).unwrap(), Some([9; 32]));
    assert_eq!(lez::decode_channel_id(&[0, 0]).unwrap(), None);
    assert_eq!(lez::decode_channel_id(&[0, 1]).map(|_| ()).is_err(), true);
}

// ── A busy Names Wallet, and one that doesn't answer ─────────────────────
//
// `busy:true` means the call was NOT taken (the generated dispatch answers it before any wallet
// code runs): asked again, safely. No answer at all means it MAY have been taken: a proposal is
// never asked again blindly.

/// Checked, connected, the account given; the programs declared and approved unless `declared`
/// is false. Returns the check id and the request states.
fn ready(w: &mut World, declared: bool) -> (String, std::rc::Rc<std::cell::RefCell<HashMap<String, Value>>>) {
    wallet_connected(&w.claimer, true);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    let req = requests();
    if declared {
        let mut st = w.b.wstate();
        st["relay_declared"] = json!({ "programs": [BRIDGE, TP], "request": "decl-1" });
        w.b.put_wstate(&st).unwrap();
        req.borrow_mut().insert("decl-1".into(), json!({ "state": "approved" }));
    }
    (cid, req)
}

fn jobs(b: &Bridge) -> usize {
    b.wstate()["relayed_jobs"].as_array().map_or(0, Vec::len)
}

#[test]
fn a_busy_wallet_is_asked_again_and_each_request_is_made_once() {
    let mut w = world("busy-ok", 200);
    let (cid, req) = ready(&mut w, false);
    // declare_programs: busy, then taken.
    stand_in::script("declare_programs", vec![stand_in::busy()]);
    stand_in::answer("declare_programs", json!({ "request": "decl-1", "state": "pending" }));
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(r["needs"], "programs", "{r}");
    assert_eq!(stand_in::count("declare_programs"), 2);
    assert_eq!(w.b.wstate()["relay_declared"]["request"], "decl-1");
    // propose_relayed_call: busy twice, then taken: one request, one job.
    req.borrow_mut().insert("decl-1".into(), json!({ "state": "approved" }));
    stand_in::script("propose_relayed_call", vec![stand_in::busy(), stand_in::busy()]);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let job = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!((job["phase"].as_str(), job["request"].as_str()), (Some("approve"), Some("rel-1")), "{job}");
    assert_eq!(stand_in::count("propose_relayed_call"), 3);
    assert_eq!(jobs(&w.b), 1);
    assert_eq!(stand_in::slept(), vec![2_000, 2_000, 4_000]);
}

#[test]
fn a_wallet_busy_past_the_cap_says_so_makes_no_job_and_try_again_is_safe() {
    let mut w = world("busy-cap", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::handle("propose_relayed_call", |_| stand_in::busy());
    let e = w.b.relayed_claim_now(&cid, &json!({})).unwrap_err();
    assert_eq!(e, wallet::BUSY_TEXT);
    assert_eq!(jobs(&w.b), 0, "nothing proposed, nothing kept");
    assert!(w.b.wstate()["relay_uncertain"].is_null(), "busy is not 'may have been taken'");
    // Try again: busy meant not taken, so it is simply proposed.
    stand_in::script("propose_relayed_call", vec![stand_in::ok(json!({ "request": "rel-1", "state": "pending" }))]);
    assert_eq!(w.b.relayed_claim_now(&cid, &json!({})).unwrap()["request"], "rel-1");
    assert_eq!(jobs(&w.b), 1);
}

#[test]
fn a_proposal_that_got_no_answer_is_not_proposed_again_until_the_person_looked() {
    let mut w = world("noanswer", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::script("propose_relayed_call", vec![Value::from(stand_in::NO_ANSWER)]);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-2", "state": "pending" }));
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!((r["needs"].as_str(), r["uncertain"].as_str()), (Some("uncertain"), Some(format!("propose:{cid}").as_str())), "{r}");
    assert!(r["message"].as_str().unwrap().contains("reject it there"));
    // "Try again": no second proposal.
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(r["needs"], "uncertain");
    assert_eq!(stand_in::count("propose_relayed_call"), 1);
    assert_eq!(jobs(&w.b), 0);
    // The person looked ("Propose again"): proposed, once.
    let job = w.b.relayed_claim_now(&cid, &json!({ "after_uncertain": true })).unwrap();
    assert_eq!(job["request"], "rel-2", "{job}");
    assert_eq!(stand_in::count("propose_relayed_call"), 2);
    assert!(w.b.wstate()["relay_uncertain"].as_object().unwrap().is_empty());
    // A marker past the request's lifetime lapses by itself.
    let mut w = world("noanswer-lapse", 200);
    let (cid, _req) = ready(&mut w, true);
    let mut st = w.b.wstate();
    st["relay_uncertain"] = json!({ format!("propose:{cid}"): util::now_ms() - UNCERTAIN_MS - 1 });
    w.b.put_wstate(&st).unwrap();
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-3", "state": "pending" }));
    assert_eq!(w.b.relayed_claim_now(&cid, &json!({})).unwrap()["request"], "rel-3");
}

#[test]
fn try_again_after_the_screen_gave_up_gets_the_claim_s_job_not_a_second_proposal() {
    let mut w = world("again-job", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let first = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    let again = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!(first["id"], again["id"]);
    assert_eq!(stand_in::count("propose_relayed_call"), 1);
    assert_eq!(jobs(&w.b), 1);
}

#[test]
fn a_declaration_that_got_no_answer_waits_for_the_person_then_the_proposal_tells() {
    let mut w = world("decl-noanswer", 200);
    let (cid, _req) = ready(&mut w, false);
    stand_in::script("declare_programs", vec![Value::from(stand_in::NO_ANSWER)]);
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!((r["needs"].as_str(), r["uncertain"].as_str()), (Some("uncertain"), Some("declare")), "{r}");
    assert_eq!(w.b.relayed_claim_now(&cid, &json!({})).unwrap()["needs"], "uncertain");
    assert_eq!(stand_in::count("declare_programs"), 1, "no second declaration while it may be waiting");
    // "Continue" without having approved it: the wallet refuses the proposal (undeclared), so the
    // programs are declared anew, once.
    stand_in::script("propose_relayed_call", vec![stand_in::refuse_json("JECKBk… is not a program this app declared; ask with declare_programs first")]);
    stand_in::answer("declare_programs", json!({ "request": "decl-2", "state": "pending" }));
    let r = w.b.relayed_claim_now(&cid, &json!({ "after_uncertain": true })).unwrap();
    assert_eq!(r["needs"], "programs", "{r}");
    assert_eq!(stand_in::count("declare_programs"), 2);
    assert_eq!(w.b.wstate()["relay_declared"]["request"], "decl-2");
    // Or it was approved there: the proposal goes through and the declaration counts as proven.
    let mut w = world("decl-noanswer-ok", 200);
    let (cid, _req) = ready(&mut w, false);
    stand_in::script("declare_programs", vec![Value::from(stand_in::NO_ANSWER)]);
    w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let job = w.b.relayed_claim_now(&cid, &json!({ "after_uncertain": true })).unwrap();
    assert_eq!(job["request"], "rel-1", "{job}");
    assert_eq!(w.b.wstate()["relay_declared"]["proven"], true);
    assert_eq!(stand_in::count("declare_programs"), 1);
}

#[test]
fn a_status_read_that_fails_never_declares_a_second_time() {
    let mut w = world("decl-read", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::script("request_status", vec![Value::from(stand_in::NO_ANSWER)]);
    let e = w.b.relayed_claim_now(&cid, &json!({})).unwrap_err();
    assert_eq!(e, wallet::NO_ANSWER_TEXT);
    assert_eq!(stand_in::count("declare_programs"), 0);
    assert_eq!(stand_in::count("propose_relayed_call"), 0);
}

#[test]
fn polls_keep_the_job_while_the_wallet_is_busy() {
    let mut w = world("busy-poll", 200);
    let (cid, req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    // Waiting for approval: a busy read is a note, asked once per poll.
    stand_in::script("request_status", vec![stand_in::busy()]);
    let before = stand_in::count("request_status");
    let s = status(&mut w.b, &jid);
    assert_eq!((s["state"].as_str(), s["phase"].as_str()), (Some("running"), Some("approve")), "{s}");
    assert!(s["note"].as_str().unwrap().contains("busy"), "{s}");
    assert_eq!(stand_in::count("request_status") - before, 1);
    assert!(stand_in::slept().is_empty(), "a poll never sleeps in the module");
    // After a module restart (the signed claim no longer in memory): busy keeps the job.
    let mut st = w.b.wstate();
    st["relayed_jobs"][0]["phase"] = json!("relay");
    w.b.put_wstate(&st).unwrap();
    stand_in::script("request_status", vec![stand_in::busy()]);
    let s = status(&mut w.b, &jid);
    assert_eq!((s["state"].as_str(), s["phase"].as_str()), (Some("running"), Some("relay")), "{s}");
    // Then the wallet answers: the same signed claim goes to the relay.
    req.borrow_mut().insert("rel-1".into(), json!({ "state": "done", "transaction": "aa11", "hash": "11".repeat(32) }));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) })));
    status(&mut w.b, &jid);
    let s = status(&mut w.b, &jid);
    assert_eq!(s["phase"], "confirm", "{s}");
}

#[test]
fn a_busy_re_sign_is_asked_again_never_turned_into_a_new_proposal() {
    let mut w = world("busy-resign", 200);
    let (cid, req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    req.borrow_mut().insert("rel-1".into(), json!({ "state": "done", "transaction": "aa11", "hash": "11".repeat(32) }));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": false, "reason": "stale nonce: rebuild the claim and send it again", "retry": true })));
    assert_eq!(status(&mut w.b, &jid)["phase"], "relay");
    w.net.nonces.lock().unwrap().insert(PAYER.to_owned(), 1);
    // The payer's nonce moved; the wallet is busy (and then silent): no new proposal, nothing posted.
    stand_in::script("resign_relayed_call", vec![stand_in::busy(), Value::from(stand_in::NO_ANSWER)]);
    for _ in 0..2 {
        let s = status(&mut w.b, &jid);
        assert_eq!((s["state"].as_str(), s["phase"].as_str(), s["resigns"].as_u64()), (Some("running"), Some("relay"), Some(0)), "{s}");
    }
    assert_eq!(stand_in::count("propose_relayed_call"), 1);
    assert_eq!(w.net.calls.lock().unwrap().iter().filter(|m| *m == "bridge_relayClaim").count(), 1);
    // It answers: re-signed, and the relay takes the new one.
    stand_in::answer("resign_relayed_call", json!({ "state": "done", "transaction": "bb22", "hash": "22".repeat(32) }));
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) })));
    let s = status(&mut w.b, &jid);
    assert_eq!((s["resigns"].as_u64(), s["phase"].as_str()), (Some(1), Some("confirm")), "{s}");
}

#[test]
fn cancel_while_the_wallet_is_busy_cancels_nothing() {
    let mut w = world("busy-cancel", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    stand_in::handle("cancel_request", |_| stand_in::busy());
    let e = w.b.relayed_cancel_now(&jid).unwrap_err();
    assert!(e.contains("busy; nothing was cancelled"), "{e}");
    assert_eq!(status(&mut w.b, &jid)["state"], "running");
    stand_in::script("cancel_request", vec![stand_in::ok(json!({ "state": "cancelled" }))]);
    assert_eq!(w.b.relayed_cancel_now(&jid).unwrap()["state"], "cancelled");
}

#[test]
fn a_busy_wallet_during_the_check_is_not_called_missing() {
    let mut w = world("busy-check", 200);
    stand_in::handle("granted_accounts", |_| stand_in::busy());
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "zone": { "channel": CHANNEL } }));
    let v = w.b.relayed_check_now(&w.file);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["wallet"]["needs"], "busy", "{v}");
    let cid = v["id"].as_str().unwrap().to_owned();
    assert_eq!(w.b.relayed_claim_now(&cid, &json!({})).unwrap_err(), wallet::BUSY_TEXT);
    let mut w = world("gone-check", 200);
    stand_in::script("granted_accounts", vec![Value::from(stand_in::UNREACHABLE)]);
    stand_in::answer("get_config", json!({}));
    assert_eq!(w.b.relayed_check_now(&w.file)["wallet"]["needs"], "wallet");
}

#[test]
fn a_silent_wallet_never_holds_a_claim_job_for_good() {
    let mut w = world("silent", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    // Cancel while the wallet doesn't answer: this app stops following it (it can't take it back).
    stand_in::script("cancel_request", vec![Value::from(stand_in::NO_ANSWER)]);
    let c = w.b.relayed_cancel_now(&jid).unwrap();
    assert_eq!(c["state"], "cancelled", "{c}");
    assert!(c["error"]["message"].as_str().unwrap().contains("reject it there"), "{c}");
    // Silent past a request's lifetime while waiting for approval: the job ends, nothing sent.
    let mut w = world("silent-lapse", 200);
    let (cid, _req) = ready(&mut w, true);
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-1", "state": "pending" }));
    let jid = w.b.relayed_claim_now(&cid, &json!({})).unwrap()["id"].as_str().unwrap().to_owned();
    let mut st = w.b.wstate();
    st["relayed_jobs"][0]["started"] = json!(util::now_ms() - UNCERTAIN_MS - 1);
    w.b.put_wstate(&st).unwrap();
    stand_in::script("request_status", vec![Value::from(stand_in::NO_ANSWER)]);
    let s = status(&mut w.b, &jid);
    assert_eq!((s["state"].as_str(), s["error"]["code"].as_str()), (Some("failed"), Some("wallet_no_answer")), "{s}");
}

// ── Hosted by the Names app: build, then submit (modules/NAMES-EMBED.md, Part B) ──

#[test]
fn the_names_app_builds_signs_and_submits_and_this_module_asks_no_wallet() {
    let mut w = world("embed", 200);
    wallet_connected(&w.claimer, true);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    stand_in::reset();
    let before = std::fs::read_to_string(w.b.data.join("wallet.json")).unwrap_or_default();
    // Build: exactly propose_relayed_call's input, the call the check built; no wallet, no state.
    let built = w.b.relayed_build_now(&cid).unwrap();
    assert_eq!(built["account"], format!("Public/{}", w.claimer));
    assert_eq!(built["call"], w.b.relayed.checks[&cid].call);
    assert_eq!(built["call"]["program"], BRIDGE);
    assert_eq!(built["call"]["fee"]["payer"], format!("Public/{PAYER}"));
    assert_eq!(built["watch"]["nullifier"], hex0x(&w.b.relayed.checks[&cid].nullifier));
    // The key is only inside the message (the claim's `secret`), once.
    let k = to_hex(&w.k);
    assert_eq!(built.to_string().matches(&k).count(), 1);
    assert!(built["call"]["message"].as_str().unwrap().contains(&k));
    assert!(stand_in::asked().is_empty(), "build asks no wallet");
    assert_eq!(std::fs::read_to_string(w.b.data.join("wallet.json")).unwrap_or_default(), before, "build writes nothing");
    // Submit: a job at the relay phase; the relay takes it; no wallet asked.
    assert!(w.b.relayed_submit_now(&cid, "not hex").is_err());
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) })));
    let job = w.b.relayed_submit_now(&cid, "0xaa11").unwrap();
    assert_eq!((job["phase"].as_str(), job["host"].as_str(), job["request"].is_null()), (Some("confirm"), Some("names_app"), true), "{job}");
    let jid = job["id"].as_str().unwrap().to_owned();
    // A repeat submit: the same job, nothing sent again.
    assert_eq!(w.b.relayed_submit_now(&cid, "0xaa11").unwrap()["id"], jid.as_str());
    assert_eq!(w.net.calls.lock().unwrap().iter().filter(|m| *m == "bridge_relayClaim").count(), 1);
    assert!(stand_in::asked().is_empty(), "submit and status ask no wallet");
    let kept = std::fs::read_to_string(w.b.data.join("wallet.json")).unwrap();
    assert!(!kept.contains("aa11") && !kept.contains(&k));
    // Today's flow is untouched: its jobs are the bridge app's.
    assert!(w.b.relayed_jobs().iter().all(|j| j["host"] == "names_app"));
}

#[test]
fn a_busy_relay_asks_the_names_app_to_sign_again_never_the_wallet() {
    let mut w = world("embed-busy", 200);
    wallet_connected(&w.claimer, true);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    stand_in::reset();
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": false, "reason": "stale nonce: rebuild the claim and send it again", "retry": true })));
    let job = w.b.relayed_submit_now(&cid, "aa11").unwrap();
    let jid = job["id"].as_str().unwrap().to_owned();
    assert_eq!((job["phase"].as_str(), job["needs"].is_null()), (Some("relay"), true), "{job}");
    // The relay's payer moves on: the Names app is asked to sign the same claim again.
    w.net.nonces.lock().unwrap().insert(PAYER.to_owned(), 1);
    let s = status(&mut w.b, &jid);
    assert_eq!((s["state"].as_str(), s["needs"].as_str()), (Some("running"), Some("sign_again")), "{s}");
    assert!(stand_in::asked().is_empty(), "no names call from a hosted job");
    status(&mut w.b, &jid);
    assert_eq!(w.net.calls.lock().unwrap().iter().filter(|m| *m == "bridge_relayClaim").count(), 1, "nothing posted while it waits");
    // The same build, signed again, submitted: it goes out.
    assert_eq!(w.b.relayed_build_now(&cid).unwrap()["call"], w.b.relayed.checks[&cid].call);
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) })));
    let s = w.b.relayed_submit_now(&cid, "bb22").unwrap();
    assert_eq!((s["phase"].as_str(), s["resigns"].as_u64(), s["needs"].is_null()), (Some("confirm"), Some(1), true), "{s}");
    assert_eq!(w.net.calls.lock().unwrap().iter().filter(|m| *m == "bridge_relayClaim").count(), 2);
    // After a module restart the signature is gone: sign again (no wallet call); a submit takes
    // it even before the file is checked again.
    w.b.relayed.signed.clear();
    w.b.relayed.checks.clear();
    let mut st = w.b.wstate();
    st["relayed_jobs"][0]["phase"] = json!("relay");
    w.b.put_wstate(&st).unwrap();
    assert_eq!(status(&mut w.b, &jid)["needs"], "sign_again");
    w.net.relay.lock().unwrap().push(Ok(json!({ "ok": true, "tx_hash": "ef".repeat(32) })));
    assert_eq!(w.b.relayed_submit_now(&cid, "cc33").unwrap()["phase"], "confirm");
    assert!(stand_in::asked().is_empty());
}

/// A caller that is the wallet checks without the `wallet` hint: the check asks Names nothing, so
/// a busy or stuck wallet never slows it; the default still asks.
#[test]
fn a_check_without_the_wallet_hint_asks_names_nothing() {
    let mut w = world("nohint", 200);
    stand_in::handle("granted_accounts", |_| stand_in::busy());
    stand_in::handle("get_config", |_| stand_in::busy());
    let v = w.b.relayed_check_opts(&w.file, &json!({ "wallet_hint": false }));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!((v["wallet"]["skipped"].as_bool(), v["wallet"]["needs"].is_null()), (Some(true), true), "{v}");
    assert!(stand_in::asked().is_empty(), "no names call");
    assert!(stand_in::slept().is_empty(), "no wait");
    let checks: Vec<&str> = v["checks"].as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap()).collect();
    assert_eq!(checks, ["file", "bridge", "deposit", "root", "unclaimed", "relay"]);
    // The default (and `{}`) still asks, for the bridge app's own screen.
    let v = w.b.relayed_check_opts(&w.file, &json!({}));
    assert_eq!(v["wallet"]["needs"], "busy");
    assert!(stand_in::count("granted_accounts") > 0);
}

// ── Receive from Ethereum: a wallet-made deposit (modules/NAMES-RECEIVE.md) ──

/// The world's deposit as `make_public_deposit_request` stores it: no secret, its index and
/// nullifiers. Returns its id.
fn stored_receive(w: &mut World) -> String {
    let d = w.b.deployment().clone();
    let cfg = util::hex32(&d.cfg_id).unwrap();
    let claimer = util::lez_account(&w.claimer).unwrap();
    let c = wallet::deposit_commitment(&w.k, &claimer, &[0; 20], 10u128.pow(16), &cfg);
    let mut row = store::new_deposit(&eth::addr_hex(&[0; 20]), "ETH", 0, 10u128.pow(16), &w.claimer, "public", &hex0x(&c), "", "wallet");
    let o = row.as_object_mut().unwrap();
    o.remove("secret");
    o.insert("key_index".into(), json!(0));
    o.insert("derivation".into(), json!("names-app"));
    o.insert("nullifier".into(), json!(hex0x(&claim_nullifier(&w.k, &claimer, &[0; 20], 10u128.pow(16)))));
    o.insert("nullifier_pre_fix".into(), json!(hex0x(&claim_nullifier_pre_fix(&w.k))));
    w.b.store().put_deposit(row).unwrap();
    store::deposit_id(&hex0x(&c))
}

#[test]
fn receive_status_follows_a_deposit_from_the_vault_to_claimable_and_claimed_without_the_wallet() {
    let mut w = world("recv", 200);
    let id = stored_receive(&mut w);
    stand_in::reset();
    let s = w.b.receive_status_now(&id).unwrap();
    assert_eq!((s["stage"].as_str(), s["leaf"].as_u64(), s["claim"].as_str()), (Some("claimable"), Some(2), Some("relayed")), "{s}");
    assert!(s["text"].as_str().unwrap().starts_with("Arrived: 0.01 ETH is ready to claim"));
    assert_eq!(s["reads"], json!({ "sepolia": 4, "lez": 3 }), "first poll: logs read, the Config once, one root, the nullifier");
    let s = w.b.receive_status_now(&id).unwrap();
    assert_eq!(s["reads"], json!({ "sepolia": 3, "lez": 2 }), "next polls: no log read while the count is the same; the Config cached");
    // The bridge hasn't recorded a root with the leaf yet: final, waiting.
    let bridge = util::lez_account(BRIDGE).unwrap();
    let root_pda = lez::b58(&lez::pda(&bridge, &lez::root_seed(&w.roots[3])));
    let saved = w.net.accounts.lock().unwrap().remove(&root_pda).unwrap();
    assert_eq!(w.b.receive_status_now(&id).unwrap()["stage"], "final");
    w.net.accounts.lock().unwrap().insert(root_pda, saved);
    // Claimed (the pre-fix fixture bridge: its nullifier formula).
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&claim_nullifier_pre_fix(&w.k)))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    let s = w.b.receive_status_now(&id).unwrap();
    assert_eq!((s["stage"].as_str(), s["text"].as_str()), (Some("claimed"), Some("Claimed on LEZ 0.4 · bridge test network.")), "{s}");
    assert!(stand_in::asked().is_empty(), "receive_status asks the wallet nothing");
    // Not final yet: confirmations and minutes.
    let mut w = world("recv-early", 50);
    let id = stored_receive(&mut w);
    let s = w.b.receive_status_now(&id).unwrap();
    assert_eq!((s["stage"].as_str(), s["confirmations"].as_u64(), s["final_in_min"].as_u64()), (Some("deposited"), Some(9), Some(13)), "{s}");
    assert_eq!(s["reads"]["lez"], 0);
    // Not in the vault at all.
    let mut row = store::new_deposit("0x0000000000000000000000000000000000000000", "ETH", 0, 10u128.pow(16), &w.claimer, "public", &hex0x(&[5; 32]), "", "wallet");
    row.as_object_mut().unwrap().remove("secret");
    w.b.store().put_deposit(row).unwrap();
    assert_eq!(w.b.receive_status_now(&store::deposit_id(&hex0x(&[5; 32]))).unwrap()["stage"], "requested");
}

#[test]
fn a_wallet_made_deposit_is_checked_from_app_secret_with_the_same_reply_as_its_claim_file() {
    let mut w = world("check-dep", 200);
    wallet_connected(&w.claimer, true);
    let from_file = w.b.relayed_check_now(&w.file);
    let id = stored_receive(&mut w);
    let k = to_hex(&w.k);
    // The person hasn't approved this app's values yet: the Names sheet first.
    stand_in::answer("app_secret", json!({ "granted": false, "request": { "request": "sec-1", "kind": "secret", "state": "pending" } }));
    let n = w.b.relayed_check_deposit_now(&id, &json!({}));
    assert_eq!(n["needs"], "secret", "{n}");
    // Granted: k for the deposit's index, asked of the public account; the same check, same id.
    let claimer = w.claimer.clone();
    stand_in::handle("app_secret", move |args| {
        assert_eq!(args[0], format!("Public/{claimer}"));
        assert_eq!((args[1].as_str(), args[3].as_str(), args[4].as_u64(), args[5].as_u64()), (Some("RFP022/deposit"), Some("RFP022/ck"), Some(0), Some(1)));
        json!({ "success": true, "value": { "granted": true, "account": args[0], "scope": "RFP022/deposit", "tag": "RFP022/ck", "first": 0, "values": [k.clone()] } })
    });
    let v = w.b.relayed_check_deposit_now(&id, &json!({ "wallet_hint": false }));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["id"], from_file["id"]);
    assert_eq!(v["checks"], from_file["checks"]);
    assert!(!v.to_string().contains(&to_hex(&w.k)), "no claim key in the reply");
    assert!(w.b.relayed.checks.contains_key(v["id"].as_str().unwrap()), "relayed_claim and relayed_claim_build work from its id");
    // A private deposit is claimed privately; an unknown id is refused.
    let mut row = w.b.store().deposit(&id).unwrap();
    row["account_kind"] = json!("private");
    w.b.store().put_deposit(row).unwrap();
    assert_eq!(w.b.relayed_check_deposit_now(&id, &json!({}))["error"]["code"], "private_account");
    assert_eq!(w.b.relayed_check_deposit_now("dep-nothing", &json!({}))["error"]["code"], "no_deposit");
}

/// The receives list: the wallet-made deposits with the stage receive_status last saw, no read;
/// config names the features and the "Not private" words before any request.
#[test]
fn receives_lists_the_last_known_stage_and_config_names_the_features() {
    let mut w = world("receives", 200);
    let id = stored_receive(&mut w);
    let l = w.b.receives_now();
    assert_eq!((l["receives"][0]["id"].as_str(), l["receives"][0]["stage"].as_str(), l["receives"][0]["amount_text"].as_str()), (Some(id.as_str()), Some("unknown"), Some("0.01 ETH")), "{l}");
    // Each row and each status carry the compare object of the stored commitment (no extra read).
    let c = util::hex32(l["receives"][0]["commitment"].as_str().unwrap()).unwrap();
    assert_eq!(l["receives"][0]["compare"], crate::compare::json(&c));
    assert_eq!(l["receives"][0]["compare"]["words"].as_array().unwrap().len(), 4);
    let st = w.b.receive_status_now(&id).unwrap();
    assert_eq!(st["compare"], crate::compare::json(&c));
    let l = w.b.receives_now();
    assert_eq!((l["receives"][0]["stage"].as_str(), l["receives"][0]["claim"].as_str(), l["receives"][0]["leaf"].as_u64()), (Some("claimable"), Some("relayed"), Some(2)), "{l}");
    let bridge = util::lez_account(BRIDGE).unwrap();
    w.net.accounts.lock().unwrap().insert(lez::b58(&lez::pda(&bridge, &lez::nullifier_seed(&claim_nullifier_pre_fix(&w.k)))), json!({ "data": { "actor_states": { BRIDGE: [1] } } }));
    w.b.receive_status_now(&id).unwrap();
    let l = w.b.receives_now();
    assert_eq!((l["receives"][0]["stage"].as_str(), l["receives"][0]["claimed"].as_bool()), (Some("claimed"), Some(true)));
    assert_eq!(w.b.store().deposit(&id).unwrap()["claimed"], true);
    let c: Value = serde_json::from_str(&w.b.config()).unwrap();
    let f = c["value"]["features"].as_array().unwrap();
    for name in ["receive", "receive_status", "receives", "relayed_claim_check_deposit", "relayed_claim_build", "account_add", "receive_request"] {
        assert!(f.iter().any(|x| x == name), "{name}");
    }
    assert!(c["value"]["notices"]["not_private"]["text"].as_str().unwrap().starts_with(PRIVACY_LINE));
}

/// The claim's gate with `grant_add_account`: an app with access but not this account gets the
/// wallet's "add this account" request, not the revoke-and-retry error.
#[test]
fn a_claim_into_an_account_not_given_asks_the_wallet_to_add_it_when_it_can() {
    let mut w = world("grant-add", 200);
    let cid = w.b.relayed_check_now(&w.file)["id"].as_str().unwrap().to_owned();
    wallet_connected(&w.claimer, false);
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "zone": { "channel": CHANNEL }, "features": ["request_access_with", "grant_add_account"] }));
    stand_in::answer("request_access_with", json!({ "request": "add-7", "kind": "connect", "state": "pending" }));
    let r = w.b.relayed_claim_now(&cid, &json!({})).unwrap();
    assert_eq!((r["needs"].as_str(), r["request"]["request"].as_str()), (Some("public_account"), Some("add-7")), "{r}");
    assert_eq!(stand_in::count("request_access"), 0);
}
