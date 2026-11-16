// SPDX-License-Identifier: MIT OR Apache-2.0
//! "Send back to Ethereum" (sendback.rs) end to end against a stand-in Names Wallet, a stand-in
//! bridge test network and a stand-in Sepolia: plan (the exit secret through `app_secret`), the
//! burn call, the Names Wallet's refusal today, the relay, the block, the update on Ethereum,
//! the witness, the seal checked by the verifier, the delay, the release code, the release.

use super::*;
use crate::lez::{LezRpc, Problem};
use crate::sendback::*;
use crate::util::{sha256, to_hex};
use crate::wallet::stand_in;
use crate::withdraw::{decode_release, WithdrawTo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const PAYER: &str = "1HHhGGKfxtpAL5Uno5QqowGy748EUkYS6QGLFgQMgNT";
const CHANNEL: &str = "2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374";
const DENOM: u128 = 10_000_000_000_000_000;
const BASE: u64 = 12_200_000;

fn bytes_json(b: &[u8]) -> Value {
    Value::from(b.iter().map(|x| json!(x)).collect::<Vec<_>>())
}

fn config_bytes(d: &config::Deployment, lez_burns: u64) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(util::hex32(&d.cfg_id).unwrap());
    b.extend(d.vault);
    b.extend(d.chain_id.to_le_bytes());
    b.extend(util::hex32(&format!("0x{CHANNEL}")).unwrap());
    b.extend([0u8; 32]);
    b.extend(util::lez_account(&d.token_program).unwrap());
    b.extend([7u8; 32]);
    b.push(0);
    b.extend([7u8; 32]);
    b.extend([7u8; 32]);
    b.extend([0, 0]);
    b.extend(1u32.to_le_bytes());
    b.extend([0u8; 20]);
    b.extend(util::lez_account(&d.tokens[0].definition).unwrap());
    b.push(18);
    b.extend(3u32.to_le_bytes());
    for x in [DENOM, DENOM * 10, DENOM * 100] {
        b.extend(x.to_le_bytes());
    }
    b.extend(3u32.to_le_bytes());
    b.extend([0u8; 24]);
    b.extend(3u32.to_le_bytes());
    b.extend(lez_burns.to_le_bytes());
    b.extend([0u8; 16]);
    b.extend([0u8; 32]);
    b.extend([1, 0, 0]);
    b.push(0);
    b.extend(25_000_000u128.to_le_bytes());
    b
}

fn holding(def: &str, v: u128) -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend(1u32.to_le_bytes());
    b.extend(util::lez_account(def).unwrap());
    b.extend(v.to_le_bytes());
    b
}

#[derive(Default)]
struct NetState {
    accounts: HashMap<String, Value>,
    tx_blocks: HashMap<String, u64>,
    relayed: Vec<String>,
    tip: u64,
    blocks: HashMap<u64, Vec<u8>>,
    burns: bool,
    /// Methods answered "429, slow down" this many more times.
    throttle: HashMap<String, u32>,
    asked: Vec<String>,
    /// The settlement service: whether it is available, what it was handed, the status it answers.
    settle_on: bool,
    settled: Vec<Value>,
    settle_status: Value,
}

struct Net(Mutex<NetState>);

impl LezRpc for Net {
    fn describe(&self) -> String {
        "stand-in".into()
    }
    fn call(&self, method: &str, params: Value) -> Result<Value, Problem> {
        let mut s = self.0.lock().unwrap();
        s.asked.push(method.to_owned());
        if let Some(n) = s.throttle.get_mut(method).filter(|n| **n > 0) {
            *n -= 1;
            return Err(Problem { code: "rate_limited", message: "the bridge test network's endpoint asks us to slow down".into(), retry_after: Some(10) });
        }
        match method {
            "getAccount" => Ok(s.accounts.get(params[0].as_str().unwrap()).cloned().unwrap_or(Value::Null)),
            "bridge_relayInfo" => Ok(json!({ "payer": PAYER, "max_gas": 250000, "max_fee": "2500000", "burns": s.burns })),
            "getLastBlockId" => Ok(json!(s.tip)),
            "getFeeState" => Ok(json!({ "base_fee_exec": 8, "base_fee_stor": 1 })),
            "getBlockRange" => {
                let (a, b) = (params[0].as_u64().unwrap(), params[1].as_u64().unwrap());
                assert!(b >= a && b - a < 100, "at most 100 blocks a call");
                Ok(Value::from((a..=b).map(|i| s.blocks.get(&i).map_or_else(|| bytes_json(&[1, 2, 3]), |x| bytes_json(x))).collect::<Vec<_>>()))
            }
            "getBlock" => Ok(s.blocks.get(&params[0].as_u64().unwrap()).map_or_else(|| bytes_json(&[1, 2, 3]), |b| bytes_json(b))),
            "bridge_relayBurn" => {
                s.relayed.push(params[0].as_str().unwrap().to_owned());
                Ok(json!({ "ok": true, "tx_hash": "cd".repeat(32) }))
            }
            "getTransaction" => Ok(s.tx_blocks.get(params[0].as_str().unwrap()).map_or(Value::Null, |b| json!(["tx", b]))),
            "getChannelId" => Ok(json!(CHANNEL)),
            "bridge_settleInfo" => Ok(json!({ "available": s.settle_on, "reason": if s.settle_on { Value::Null } else { json!("not set up") }, "fee": "0", "vaults": [] })),
            "bridge_settle" => {
                s.settled.push(params[0].clone());
                Ok(json!({ "ok": true, "id": "0x".to_owned() + &"5a".repeat(32), "stage": "queued" }))
            }
            "bridge_settleStatus" => {
                assert_eq!(params[0], "0x".to_owned() + &"5a".repeat(32));
                if s.settle_status == "unserved" {
                    return Err(Problem { code: "refused", message: "method not allowed".into(), retry_after: None });
                }
                Ok(s.settle_status.clone())
            }
            _ => Err(Problem { code: "refused", message: format!("not served: {method}"), retry_after: None }),
        }
    }
}

#[derive(Default)]
struct EthState {
    logs: Vec<Value>,
    leaves: Vec<[u8; 32]>,
    releasable_at: u64,
    spent: bool,
    eth_burns: u64,
    verify_ok: bool,
    verified: Vec<Vec<u8>>,
}

struct FakeEth(Arc<Mutex<EthState>>);

const GATE: [u8; 20] = [0xe5; 20];
const IMAGE: [u8; 32] = [0x6b; 32];

impl eth::EthSource for FakeEth {
    fn describe(&self) -> String {
        "fake Sepolia".into()
    }
    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        let mut s = self.0.lock().unwrap();
        let sel = |sig: &str| util::selector(sig) == data[..4];
        if to == &GATE {
            assert!(sel("verify(bytes,bytes32,bytes32)"));
            s.verified.push(data.to_vec());
            return if s.verify_ok { Ok(vec![]) } else { Err("execution reverted: VerificationFailed".into()) };
        }
        Ok(if sel("exitCount()") {
            eth::word_u64(s.leaves.len() as u64).to_vec()
        } else if sel("exitRoot()") {
            root_of(&s.leaves).to_vec()
        } else if sel("counters(address,uint256)") {
            [eth::word_u64(1), eth::word_u64(1), eth::word_u64(s.eth_burns), eth::word_u64(0)].concat()
        } else if sel("releasableAt(address,bytes32)") {
            eth::word_u64(s.releasable_at).to_vec()
        } else if sel("releaseNullifierSpent(bytes32)") {
            eth::word_u64(u64::from(s.spent)).to_vec()
        } else if sel("verifier()") {
            eth::word_address(&GATE).to_vec()
        } else if sel("releaseImageId()") {
            IMAGE.to_vec()
        } else {
            return Err(format!("unexpected call {}", to_hex(&data[..4])));
        })
    }
    fn block(&self, _: &str) -> Result<eth::BlockInfo, String> {
        Ok(eth::BlockInfo { number: BASE + 500, hash: "0x00".into(), timestamp: 1_800_000_000 })
    }
    fn block_by_hash(&self, _: &str) -> Result<Option<eth::BlockInfo>, String> {
        Ok(None)
    }
    fn logs(&self, _: &[u8; 20], t: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
        let s = self.0.lock().unwrap();
        Ok(s.logs.iter().filter(|l| l["topics"][0].as_str() == Some(hex0x(t).as_str()))
            .filter(|l| { let b = u64::from_str_radix(l["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap(); b >= from && b <= to })
            .cloned().collect())
    }
    fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
        Ok(0)
    }
    fn chain_id(&self) -> Result<u64, String> {
        Ok(11_155_111)
    }
}

fn update_log(first: u64, all: &[[u8; 32]], new: &[[u8; 32]], block: u64) -> Value {
    let mut d = Vec::new();
    d.extend([0u8; 32]);
    d.extend([1u8; 32]);
    d.extend(root_of(all));
    d.extend(eth::word_u64(first));
    d.extend(eth::word_u64(0xa0));
    d.extend(eth::word_u64(new.len() as u64));
    for l in new {
        d.extend(l);
    }
    json!({ "topics": [hex0x(&update_topic())], "data": hex0x(&d), "blockNumber": format!("0x{block:x}"), "transactionHash": format!("0x{}", "ef".repeat(32)) })
}

fn value_at(i: u64) -> [u8; 32] {
    sha256(&[b"exit value", &i.to_be_bytes()])
}

struct World {
    b: Bridge,
    net: Arc<Net>,
    eth: Arc<Mutex<EthState>>,
    acct: String,
    code: String,
    recipient: [u8; 20],
}

fn world(name: &str, deployment: &str) -> World {
    stand_in::reset();
    let dir = std::env::temp_dir().join(format!("lez-bridge-sendback-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = config::builtin(deployment).unwrap();
    let acct = lez::b58(&sha256(&[b"burner", name.as_bytes()]));
    let bridge = util::lez_account(&d.bridge_program).unwrap();
    let mut accounts = HashMap::new();
    accounts.insert(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), json!({ "nonce": 0, "data": { "actor_states": { d.bridge_program.clone(): bytes_json(&config_bytes(&d, 0)) } } }));
    accounts.insert(acct.clone(), json!({ "nonce": 1, "data": { "actor_states": { d.token_program.clone(): bytes_json(&holding(&d.tokens[0].definition, DENOM * 2)) } } }));
    let net = Arc::new(Net(Mutex::new(NetState { accounts, tip: 100, ..Default::default() })));
    let eth = Arc::new(Mutex::new(EthState { verify_ok: true, ..Default::default() }));
    let mut cfg = Config::default();
    cfg.deployment = d.clone();
    let mut b = Bridge { data: dir.join("data"), cfg, fixtures: None, registry_cache: None, eth_override: Some(Box::new(FakeEth(eth.clone()))), relayed: Default::default() };
    b.relayed.rpc_override = Some(net.clone());
    b.registry_cache = Some((util::now_ms(), json!({ "tokens": [{ "symbol": "ETH", "denoms": [
        { "value": DENOM.to_string(), "relay_fee_menu": [{ "value": "50000000000000" }], "withdraw_fee": "0" }] }] })));
    let recipient = [0x11u8; 20];
    let code = WithdrawTo { chain_id: d.chain_id, vault: d.vault, recipient, fee_choice: 0 }.encode();
    stand_in::answer("granted_accounts", json!({ "connected": true, "accounts": [{ "account": format!("Public/{acct}"), "account_kind": "public" }] }));
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "zone": { "channel": CHANNEL } }));
    stand_in::handle("app_secret", |args| {
        let first = args[4].as_u64().unwrap();
        let n = args[5].as_u64().unwrap();
        assert_eq!(args[1], "RFP022/exit");
        let values: Vec<String> = (first..first + n).map(|i| hex0x(&value_at(i))).collect();
        json!({ "success": true, "value": { "granted": true, "first": first, "values": values }, "error": null })
    });
    World { b, net, eth, acct, code, recipient }
}

fn ok(v: &str) -> Value {
    let r: Value = serde_json::from_str(v).unwrap();
    assert_eq!(r["success"], true, "{r}");
    r["value"].clone()
}

#[test]
fn a_send_back_goes_from_the_code_to_the_release() {
    let mut w = world("flow", "bridge-test-network-v4");
    let acct = format!("Public/{}", w.acct);
    let link = ok(&w.b.send_back_link());
    assert_eq!((link["available"].as_bool(), link["link"].as_str()), (Some(true), Some(PAGE_LINK)));
    let check = ok(&w.b.send_back_check(w.code.clone()));
    assert_eq!(check["ok"], true);
    assert_eq!(check["words_text"].as_str().unwrap().split(" \u{b7} ").count(), 4);

    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), acct.clone(), format!("https://nameswallet.app/bridge/#{}", w.code)));
    assert_eq!(plan["stage"], "planned", "{plan}");
    let id = plan["id"].as_str().unwrap().to_owned();
    let d = w.b.cfg.deployment.clone();
    let cfg = util::hex32(&d.cfg_id).unwrap();
    let e = wallet::exit_commitment(&value_at(0), &w.recipient, &[0; 20], DENOM, 0, &cfg);
    assert_eq!(plan["exit_commitment"], hex0x(&e), "index 0: nothing was recorded before");
    let stored = std::fs::read_to_string(w.b.data.join("positions.json")).unwrap() + &std::fs::read_to_string(w.b.data.join("wallet.json")).unwrap();
    assert!(!stored.contains(&to_hex(&value_at(0))), "the exit secret is never stored");
    assert!(!plan.to_string().contains(&to_hex(&value_at(0))));

    // A second plan to the same address takes the next index (never the same exit twice).
    let again = ok(&w.b.send_back_plan("ETH".into(), "0".into(), acct.clone(), w.code.clone()));
    assert_eq!(again["exit_commitment"], hex0x(&wallet::exit_commitment(&value_at(1), &w.recipient, &[0; 20], DENOM, 0, &cfg)));

    let built = ok(&w.b.send_back_build(id.clone()));
    assert_eq!(built["call"]["program"], d.token_program);
    assert_eq!(built["call"]["privacy"], "bridge_public_burn");
    assert_eq!(built["call"]["fee"]["payer"], format!("Public/{PAYER}"));
    assert_eq!(built["message_len"], 145);
    assert!(built["call"]["message"].as_str().unwrap().ends_with(&to_hex(&e)));

    // A Names Wallet without `relayed_bridge_burn` is never asked to sign a burn.
    assert_eq!(plan["burn"]["can_sign"], false);
    let burn = ok(&w.b.send_back_burn(id.clone(), "{}".into()));
    assert_eq!(burn["needs"], "names_update", "{burn}");
    assert!(stand_in::asked().iter().all(|(m, _)| m != "propose_relayed_call"));

    // The Names app signed it (Part B): the relay sends it.
    let s = ok(&w.b.send_back_submit(id.clone(), "0xabcd".into()));
    assert_eq!(s["stage"], "burning");
    assert_eq!(w.net.0.lock().unwrap().relayed, vec!["abcd".to_owned()]);
    w.net.0.lock().unwrap().tx_blocks.insert("cd".repeat(32), 10_361);
    let acct_state = json!({ "nonce": 2, "data": { "actor_states": { d.token_program.clone(): bytes_json(&holding(&d.tokens[0].definition, DENOM)) } } });
    w.net.0.lock().unwrap().accounts.insert(w.acct.clone(), acct_state);
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["burn_block"].as_u64()), (Some("burned"), Some(10_361)), "{s}");
    assert!(s["wait"]["what"].as_str().unwrap().contains("operator"));

    // The operator's update records it on Ethereum, after one other exit.
    let leaf = exit_leaf(&cfg, &[0; 20], DENOM, &e);
    {
        let mut x = w.eth.lock().unwrap();
        let other = [0x44u8; 32];
        x.logs.push(update_log(0, &[other], &[other], BASE + 10));
        x.logs.push(update_log(1, &[other, leaf], &[leaf], BASE + 20));
        x.leaves = vec![other, leaf];
        x.releasable_at = util::now_ms() / 1000 + 3600;
    }
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["leaf_index"].as_u64()), (Some("recorded"), Some(1)), "{s}");
    assert_eq!(s["root"], hex0x(&root_of(&w.eth.lock().unwrap().leaves)));

    // No prover set: the stand-in writes the witness for the owner's own prover run.
    let p = ok(&w.b.send_back_prove(id.clone()));
    assert_eq!(p["needs"], "prover");
    let wit = ok(&w.b.send_back_witness(id.clone()));
    assert!(!wit.to_string().contains(&to_hex(&value_at(0))), "paths only");
    let k = std::fs::read_to_string(wit["secret_file"].as_str().unwrap()).unwrap();
    assert_eq!(k, hex0x(&value_at(0)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(wit["secret_file"].as_str().unwrap()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(wit["command"].as_str().unwrap().contains("--relay-fee 0"));
    assert_eq!(std::fs::read_to_string(wit["leaves"].as_str().unwrap()).unwrap().lines().count(), 2);

    // The receipt arrives, then the seal: refused when the verifier refuses, kept when it accepts.
    let dir = std::path::PathBuf::from(wit["dir"].as_str().unwrap());
    std::fs::create_dir_all(dir.join("run/prove")).unwrap();
    std::fs::write(dir.join("run/prove/receipt.bin"), b"r").unwrap();
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "proved");
    let seal = hex0x(&[withdraw::SELECTOR.to_vec(), vec![9u8; 256]].concat());
    w.eth.lock().unwrap().verify_ok = false;
    let refused: Value = serde_json::from_str(&w.b.send_back_seal(id.clone(), seal.clone())).unwrap();
    assert_eq!(refused["success"], false);
    w.eth.lock().unwrap().verify_ok = true;
    let s = ok(&w.b.send_back_seal(id.clone(), seal.clone()));
    assert_eq!(s["stage"], "wrapped");
    let call = w.eth.lock().unwrap().verified.last().cloned().unwrap();
    let journal = release_journal(&cfg, &root_of(&w.eth.lock().unwrap().leaves), &wallet::release_nullifier(&value_at(0)), &[0; 20], DENOM, &w.recipient, &[0; 20], 0);
    assert_eq!(&call[4 + 32..4 + 64], &IMAGE);
    assert_eq!(&call[4 + 64..4 + 96], &sha256(&[&journal]), "the seal is checked for this exit's journal");
    assert!(s["wait"]["seconds"].as_u64().unwrap() > 3500);

    w.eth.lock().unwrap().releasable_at = 1;
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "releasable");
    let rc = ok(&w.b.send_back_release_code(id.clone()));
    let r = decode_release(rc["release_code"].as_str().unwrap()).unwrap();
    assert_eq!((r.recipient, r.relay_fee, r.denom_index, r.vault), (w.recipient, 0, 0, d.vault));
    assert_eq!(r.nullifier, wallet::release_nullifier(&value_at(0)));
    assert_eq!(hex0x(&r.seal), seal);

    w.eth.lock().unwrap().spent = true;
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "released");
    let list = ok(&w.b.send_backs());
    assert_eq!(list["send_backs"].as_array().unwrap().len(), 2);
}

#[test]
fn a_bridge_that_cannot_release_and_a_code_for_another_vault_are_refused() {
    let mut w = world("refuse", "bridge-test-network-v3");
    let r: Value = serde_json::from_str(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone())).unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error"].as_str().unwrap().contains("destroy"), "{r}");
    let mut w = world("other", "bridge-test-network-v4");
    let other = WithdrawTo { chain_id: 11_155_111, vault: [0x26; 20], recipient: [1; 20], fee_choice: 0 }.encode();
    let c = ok(&w.b.send_back_check(other));
    assert_eq!(c["error"]["code"], "other_vault");
    let bad = ok(&w.b.send_back_check("logoswd1qqqq".into()));
    assert_eq!(bad["ok"], false);
}

#[test]
fn a_fresh_install_waits_while_lez_counts_burns_ethereum_has_not_recorded() {
    let mut w = world("fresh", "bridge-test-network-v4");
    let d = w.b.cfg.deployment.clone();
    let bridge = util::lez_account(&d.bridge_program).unwrap();
    w.net.0.lock().unwrap().accounts.insert(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), json!({ "nonce": 0, "data": { "actor_states": { d.bridge_program.clone(): bytes_json(&config_bytes(&d, 1)) } } }));
    let r: Value = serde_json::from_str(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone())).unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error"].as_str().unwrap().contains("next update"), "{r}");
    // Once Ethereum recorded it (one of this wallet's: index 0), the plan takes index 1.
    let cfg = util::hex32(&d.cfg_id).unwrap();
    let leaf = exit_leaf(&cfg, &[0; 20], DENOM, &wallet::exit_commitment(&value_at(0), &w.recipient, &[0; 20], DENOM, 0, &cfg));
    {
        let mut x = w.eth.lock().unwrap();
        x.eth_burns = 1;
        x.logs.push(update_log(0, &[leaf], &[leaf], BASE + 5));
        x.leaves = vec![leaf];
    }
    let p = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    assert_eq!(p["exit_commitment"], hex0x(&wallet::exit_commitment(&value_at(1), &w.recipient, &[0; 20], DENOM, 0, &cfg)));
}


#[test]
fn the_names_wallet_signs_the_burn_and_the_relay_sends_it() {
    let mut w = world("partA", "bridge-test-network-v4");
    let d = w.b.cfg.deployment.clone();
    stand_in::answer("get_config", json!({ "names_network": "lez-0.4", "zone": { "channel": CHANNEL }, "features": ["relayed_bridge_burn"] }));
    w.net.0.lock().unwrap().burns = true;
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    assert_eq!(plan["burn"]["can_sign"], true, "{plan}");
    let id = plan["id"].as_str().unwrap().to_owned();
    let mut st = w.b.wstate();
    st["relay_declared"] = json!({ "programs": [d.bridge_program, d.token_program], "request": null, "proven": true });
    w.b.put_wstate(&st).unwrap();
    stand_in::answer("propose_relayed_call", json!({ "request": "rel-burn-1", "state": "pending" }));
    let s = ok(&w.b.send_back_burn(id.clone(), "{}".into()));
    assert_eq!(s["stage"], "approve", "{s}");
    let proposed = stand_in::asked().into_iter().find(|(m, _)| m == "propose_relayed_call").unwrap().1;
    let call: Value = serde_json::from_str(proposed[1].as_str().unwrap()).unwrap();
    assert_eq!((call["program"].as_str(), call["privacy"].as_str()), (Some(d.token_program.as_str()), Some("bridge_public_burn")));
    // A second press proposes nothing new.
    ok(&w.b.send_back_burn(id.clone(), "{}".into()));
    assert_eq!(stand_in::count("propose_relayed_call"), 1);
    stand_in::answer("request_status", json!({ "state": "done", "transaction": "beef", "hash": "11".repeat(32) }));
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!(s["stage"], "burning", "{s}");
    assert_eq!(w.net.0.lock().unwrap().relayed, vec!["beef".to_owned()]);
}

#[test]
fn a_burn_sent_any_other_way_is_found_by_its_exit_commitment() {
    let mut w = world("scan", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "planned");
    {
        let mut n = w.net.0.lock().unwrap();
        n.tip = 103;
        n.blocks.insert(102, [vec![9u8; 500], e.to_vec(), vec![7u8; 40]].concat());
    }
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["burn_block"].as_u64()), (Some("burned"), Some(102)), "{s}");
}


#[test]
fn an_exit_already_burned_is_never_sent_again() {
    let mut w = world("twice", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    {
        let mut n = w.net.0.lock().unwrap();
        n.tip = 101;
        n.blocks.insert(101, e.to_vec());
    }
    let r: Value = serde_json::from_str(&w.b.send_back_submit(id.clone(), "0xabcd".into())).unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error"].as_str().unwrap().contains("burned already"), "{r}");
    assert!(w.net.0.lock().unwrap().relayed.is_empty(), "nothing handed to the relay");
    assert_eq!(ok(&w.b.send_backs())["send_backs"][0]["stage"], "burned");
}


#[test]
fn the_installed_prover_gets_the_secret_on_stdin_and_its_receipt_moves_the_send_back_on() {
    let mut w = world("prover", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    let cfg = util::hex32(&w.b.cfg.deployment.cfg_id).unwrap();
    let leaf = exit_leaf(&cfg, &[0; 20], DENOM, &e);
    {
        let mut n = w.net.0.lock().unwrap();
        n.tip = 101;
        n.blocks.insert(101, e.to_vec());
        let mut x = w.eth.lock().unwrap();
        x.logs.push(update_log(0, &[leaf], &[leaf], BASE + 5));
        x.leaves = vec![leaf];
        x.releasable_at = util::now_ms() / 1000 + 3600;
    }
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "recorded");
    let script = w.b.data.join("fake-prover.sh");
    std::fs::create_dir_all(&w.b.data).unwrap();
    std::fs::write(&script, "#!/bin/sh\nout=''\nfor a in \"$@\"; do [ \"$prev\" = --out ] && out=$a; prev=$a; done\ncat > \"$out/stdin.seen\"\necho \"$@\" > \"$out/argv.seen\"\nprintf r > \"$out/receipt.bin\"\necho 0xab > \"$out/journal.hex\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    ok(&w.b.set_release_prover(json!([script.display().to_string()]).to_string()));
    let p = ok(&w.b.send_back_prove(id.clone()));
    assert_eq!(p["stage"], "proving", "{p}");
    let mut stage = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        stage = ok(&w.b.send_back_status(id.clone()))["stage"].as_str().unwrap().to_owned();
        if stage == "proved" {
            break;
        }
    }
    assert_eq!(stage, "proved");
    let dir = w.b.data.join("send-back").join(&id);
    assert_eq!(std::fs::read_to_string(dir.join("stdin.seen")).unwrap(), hex0x(&value_at(0)), "the secret arrives on stdin");
    let argv = std::fs::read_to_string(dir.join("argv.seen")).unwrap();
    assert!(!argv.contains(&to_hex(&value_at(0))), "never on argv");
    assert!(argv.contains(&format!("--image-id {}", hex0x(&IMAGE))) && argv.contains("--prove") && argv.contains("--relay-fee 0"), "{argv}");
    let v = ok(&w.b.send_back_status(id.clone()));
    assert!(v["receipt"]["path"].as_str().unwrap().ends_with("receipt.bin"), "{v}");
}


#[test]
fn a_busy_endpoint_is_waited_out_and_a_scan_resumes_where_it_stopped() {
    let mut w = world("busy", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    {
        let mut n = w.net.0.lock().unwrap();
        n.tip = 350;
        n.blocks.insert(330, e.to_vec());
        // Two "slow down" answers within one read's patience: waited out (Retry-After 10 s each).
        n.throttle.insert("getLastBlockId".into(), 1);
        n.throttle.insert("getBlockRange".into(), 2);
    }
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["burn_block"].as_u64()), (Some("burned"), Some(330)), "{s}");
    assert_eq!(stand_in::slept(), vec![10_000, 10_000, 10_000]);
    let ranges = w.net.0.lock().unwrap().asked.iter().filter(|m| *m == "getBlockRange").count();
    assert_eq!(ranges, 2 + 3, "blocks 101..350 in ranges of 100, the first one asked three times");

    // Longer than a read's patience: a plain busy note, nothing lost, the next poll goes on.
    let mut w = world("busiest", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    w.net.0.lock().unwrap().tip = 350;
    w.net.0.lock().unwrap().throttle.insert("getBlockRange".into(), 3);
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["busy"].as_bool(), s["note"].as_str()), (Some("planned"), Some(true), Some(lez::BUSY_TEXT)), "{s}");
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    w.net.0.lock().unwrap().blocks.insert(330, e.to_vec());
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["burn_block"].as_u64()), (Some("burned"), Some(330)), "the next poll goes on: {s}");
}

#[test]
fn the_relays_info_is_read_once_a_minute() {
    let mut w = world("relayinfo", "bridge-test-network-v4");
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    ok(&w.b.send_back_build(id.clone()));
    ok(&w.b.send_back_build(id.clone()));
    assert_eq!(w.net.0.lock().unwrap().asked.iter().filter(|m| *m == "bridge_relayInfo").count(), 1);
}


/// A send-back recorded on Ethereum (leaf 0), with no prover set.
fn recorded(name: &str, settle_on: bool) -> (World, String) {
    let mut w = world(name, "bridge-test-network-v4");
    w.net.0.lock().unwrap().settle_on = settle_on;
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    let id = plan["id"].as_str().unwrap().to_owned();
    let e = util::hex32(plan["exit_commitment"].as_str().unwrap()).unwrap();
    let cfg = util::hex32(&w.b.cfg.deployment.cfg_id).unwrap();
    let leaf = exit_leaf(&cfg, &[0; 20], DENOM, &e);
    {
        let mut n = w.net.0.lock().unwrap();
        n.tip = 101;
        n.blocks.insert(101, e.to_vec());
        let mut x = w.eth.lock().unwrap();
        x.logs.push(update_log(0, &[leaf], &[leaf], BASE + 5));
        x.leaves = vec![leaf];
        x.releasable_at = util::now_ms() / 1000 + 3600;
    }
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["stage"], "recorded");
    (w, id)
}

#[test]
fn without_a_helper_the_settlement_service_gets_the_exit_only_after_consent() {
    let (mut w, id) = recorded("settle-exit", false);
    // Not available: plain words, nothing handed over.
    let r: Value = serde_json::from_str(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into())).unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error"].as_str().unwrap().contains("not set up"), "{r}");
    let (mut w, id) = recorded("settle-exit-2", true);
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    assert_eq!((plan["settle_offer"]["available"].as_bool(), plan["settle_offer"]["consent_needed"].as_bool()), (Some(true), Some(true)), "{plan}");
    assert_eq!(plan["settle_offer"]["consent_text"], crate::sendback::SETTLE_CONSENT);
    assert_eq!(ok(&w.b.send_back_status(id.clone()))["settle_offer"]["available"], true);
    let n = ok(&w.b.send_back_settle(id.clone(), "{}".into()));
    assert_eq!(n["needs"], "consent", "{n}");
    assert!(w.net.0.lock().unwrap().settled.is_empty(), "no consent, nothing sent");
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "queued", "text": "Waiting its turn on the bridge's machine." });
    let s = ok(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into()));
    assert_eq!(s["settle"]["stage"], "queued", "{s}");
    let sent = w.net.0.lock().unwrap().settled[0].clone();
    assert_eq!(sent["exit"]["secret"], hex0x(&value_at(0)));
    assert_eq!((sent["exit"]["index"].as_u64(), sent["exit"]["leaves"].as_array().unwrap().len()), (Some(0), 1));
    assert_eq!(sent["vault"], eth::addr_hex(&w.b.cfg.deployment.vault));
    assert!(sent.get("receipt_hex").is_none());
    fn all_files(d: &std::path::Path, out: &mut String) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            if e.path().is_dir() { all_files(&e.path(), out) } else { out.push_str(&String::from_utf8_lossy(&std::fs::read(e.path()).unwrap_or_default())) }
        }
    }
    let mut stored = String::new();
    all_files(&w.b.data, &mut stored);
    assert!(stored.contains("settle_id"), "the record was read");
    assert!(!stored.contains(&to_hex(&value_at(0))), "the exit secret is never stored");
    // A second press hands nothing over again.
    ok(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into()));
    assert_eq!(w.net.0.lock().unwrap().settled.len(), 1);
    // send_back_status follows the service, and its release ends the send-back.
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "wrapping", "text": "Wrapping the proof for Ethereum (about 4 min).", "run": 77 });
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["settle"]["stage"].as_str(), s["settle_id"].as_str().map(|x| x.len())), (Some("wrapping"), Some(66)), "{s}");
    let seal = hex0x(&[withdraw::SELECTOR.to_vec(), vec![9u8; 256]].concat());
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "released", "text": "Released on Ethereum.", "seal": seal, "tx": "0xabc", "block": 9 });
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["released_tx"].as_str(), s["seal"].as_bool()), (Some("released"), Some("0xabc"), Some(true)), "{s}");
}

#[test]
fn with_a_local_receipt_only_the_receipt_goes_and_a_failure_leaves_the_fallbacks() {
    let (mut w, id) = recorded("settle-receipt", true);
    let dir = w.b.data.join("send-back").join(&id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("receipt.bin"), [7u8; 40]).unwrap();
    std::fs::write(dir.join("journal.hex"), format!("0x{}\n", "cd".repeat(256))).unwrap();
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "queued" });
    ok(&w.b.send_back_settle(id.clone(), "{}".into()));
    let sent = w.net.0.lock().unwrap().settled[0].clone();
    assert_eq!((sent["receipt_hex"].as_str(), sent["journal_hex"].as_str()), (Some("07".repeat(40).as_str()), Some("cd".repeat(256).as_str())));
    assert!(sent.get("exit").is_none(), "no secret when a receipt is here");
    assert!(stand_in::asked().iter().filter(|(m, _)| m == "app_secret").count() <= 2, "no exit secret asked for the settlement");
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "failed", "error": "The Ethereum wrapper failed (run 5)." });
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!((s["stage"].as_str(), s["settle"]["error"].as_str()), (Some("proved"), Some("The Ethereum wrapper failed (run 5).")), "{s}");
    // After a failure it can be handed over again.
    ok(&w.b.send_back_settle(id.clone(), "{}".into()));
    assert_eq!(w.net.0.lock().unwrap().settled.len(), 2);
}


#[test]
fn a_settlement_service_gone_or_reset_never_strands_a_send_back() {
    let (mut w, id) = recorded("settle-gone", true);
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "wrapping" });
    ok(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into()));
    // Rolled back: the method is no longer served. The chain reads go on; Sepolia's release ends it.
    w.net.0.lock().unwrap().settle_status = json!("unserved");
    w.eth.lock().unwrap().spent = true;
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!(s["stage"], "released", "{s}");
    assert!(s["settle_note"].as_str().unwrap().contains("can't be asked"), "{s}");

    // Reset: the service answers "unknown"; it can be handed over again.
    let (mut w, id) = recorded("settle-reset", true);
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "queued" });
    ok(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into()));
    w.net.0.lock().unwrap().settle_status = json!({ "ok": false, "code": "unknown", "reason": "no such settlement" });
    let s = ok(&w.b.send_back_status(id.clone()));
    assert_eq!(s["settle"]["stage"], "failed", "{s}");
    assert_eq!(s["settle_offer"]["available"], true);
    w.net.0.lock().unwrap().settle_status = json!({ "ok": true, "stage": "queued" });
    ok(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into()));
    assert_eq!(w.net.0.lock().unwrap().settled.len(), 2);
}

#[test]
fn a_wallet_with_the_helper_never_sends_its_secret() {
    let (mut w, id) = recorded("settle-helper", true);
    ok(&w.b.set_release_prover(json!(["/bin/sh"]).to_string()));
    let plan = ok(&w.b.send_back_plan("ETH".into(), "0".into(), format!("Public/{}", w.acct), w.code.clone()));
    assert_eq!((plan["settle_offer"]["consent_needed"].as_bool(), plan["settle_offer"]["prove_first"].as_bool()), (Some(false), Some(true)));
    let r: Value = serde_json::from_str(&w.b.send_back_settle(id.clone(), "{\"consent\":true}".into())).unwrap();
    assert_eq!(r["success"], false);
    assert!(r["error"].as_str().unwrap().contains("make the proof here first"), "{r}");
    assert!(w.net.0.lock().unwrap().settled.is_empty());
}
