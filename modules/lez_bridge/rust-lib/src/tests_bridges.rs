// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bridge deployments as data (bridges.rs, bridge_code.rs) against a stand-in Sepolia and a
//! stand-in bridge test network: a new bridge's code is checked on both chains, added and chosen;
//! codes that disagree with a chain, or that pose as a built-in bridge, are refused and nothing is
//! kept; an added bridge in use or named by a stored deposit isn't removed; a deposit keeps its bridge.

use super::*;
use crate::bridge_code::BridgeCode;
use crate::lez::{LezRpc, Problem};
use crate::util::to_hex;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const VECTORS: &str = include_str!("../../../../web/deposit/test/vectors/bridge-code-v1.json");

fn vector(name: &str) -> BridgeCode {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    let c = v["valid"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap();
    BridgeCode::decode(c["code"].as_str().unwrap()).unwrap()
}

/// A sixth bridge: v5's design at another vault and program, read through another endpoint.
fn v6() -> BridgeCode {
    let mut c = vector("v5");
    c.vault = [0x66; 20];
    c.desk = [0x67; 20];
    c.gate = [0x68; 20];
    c.bridge_program = util::sha256(&[b"bridge v6 program"]);
    c.config_account = lez::pda(&c.bridge_program, &lez::CONFIG_SEED);
    c.cfg_id = crate::bridge_code::cfg_id_of(c.chain_id, &c.vault, &c.lez_chain_id, &c.zone_id, &c.bridge_program);
    c.endpoint = "https://lez.example.org/".into();
    c.label = "bridge v6".into();
    c.min_release_delay = 600;
    for (i, t) in c.tokens.iter_mut().enumerate() {
        t.definition = util::sha256(&[b"v6 definition", &[i as u8]]);
    }
    c.encode().unwrap();
    c
}

fn w(b: [u8; 32]) -> Vec<u8> {
    b.to_vec()
}

/// `tokenConfig(address)`'s answer: (registered, false, false, decimals, delay, denoms, fees, fees, 0).
fn token_config(decimals: u8, delay: u64, denoms: &[u128]) -> Vec<u8> {
    let n = denoms.len() as u64;
    let mut out = Vec::new();
    for x in [eth::word_u64(1), [0; 32], [0; 32], eth::word_u64(u64::from(decimals)), eth::word_u64(delay), eth::word_u64(9 * 32), eth::word_u64(9 * 32 + 32 * (1 + n)), eth::word_u64(9 * 32 + 64 * (1 + n)), [0; 32]] {
        out.extend(x);
    }
    for _ in 0..3 {
        out.extend(eth::word_u64(n));
        for d in denoms {
            let mut word = [0u8; 32];
            word[16..].copy_from_slice(&d.to_be_bytes());
            out.extend(word);
        }
    }
    out
}

#[derive(Default)]
struct EthAnswers {
    map: HashMap<(String, String), Vec<u8>>,
    seen: Vec<[u8; 20]>,
}

struct FakeEth(Arc<Mutex<EthAnswers>>);

fn key(to: &[u8; 20], data: &[u8]) -> (String, String) {
    (to_hex(to), to_hex(data))
}

impl EthAnswers {
    fn put(&mut self, to: &[u8; 20], sig: &str, args: &[[u8; 32]], answer: Vec<u8>) {
        self.map.insert(key(to, &eth::calldata(sig, args)), answer);
    }
    /// The vault, gate and desk of `c` as Sepolia would answer them.
    fn serve(&mut self, c: &BridgeCode) {
        self.put(&c.vault, "cfgId()", &[], w(c.cfg_id));
        self.put(&c.vault, "programId()", &[], w(c.bridge_program));
        self.put(&c.vault, "lezChainId()", &[], w(c.lez_chain_id));
        self.put(&c.vault, "zoneId()", &[], w(c.zone_id));
        self.put(&c.vault, "updateImageId()", &[], w(c.update_image));
        self.put(&c.vault, "releaseImageId()", &[], w(c.release_image));
        self.put(&c.vault, "minReleaseDelay()", &[], eth::word_u64(c.min_release_delay).to_vec());
        self.put(&c.vault, "verifier()", &[], eth::word_address(&c.gate).to_vec());
        self.put(&c.vault, "g2State()", &[], w(c.genesis_g2_state));
        self.put(&c.gate, "UPDATE_IMAGE_ID()", &[], w(c.update_image));
        self.put(&c.desk, "vault()", &[], eth::word_address(&c.vault).to_vec());
        for t in &c.tokens {
            self.put(&c.vault, "tokenConfig(address)", &[eth::word_address(&t.eth_token)], token_config(t.decimals, c.min_release_delay, &t.denoms));
        }
    }
}

impl eth::EthSource for FakeEth {
    fn describe(&self) -> String {
        "fake Sepolia".into()
    }
    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        let mut s = self.0.lock().unwrap();
        s.seen.push(*to);
        s.map.get(&key(to, data)).cloned().ok_or_else(|| "execution reverted".to_owned())
    }
    fn block(&self, _: &str) -> Result<eth::BlockInfo, String> {
        Ok(eth::BlockInfo { number: 12_200_000, hash: "0x00".into(), timestamp: 1_800_000_000 })
    }
    fn block_by_hash(&self, _: &str) -> Result<Option<eth::BlockInfo>, String> {
        Ok(None)
    }
    fn logs(&self, _: &[u8; 20], _: &[u8; 32], _: u64, _: u64) -> Result<Vec<Value>, String> {
        Ok(vec![])
    }
    fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
        Ok(0)
    }
    fn chain_id(&self) -> Result<u64, String> {
        Ok(11_155_111)
    }
}

fn bytes_json(b: &[u8]) -> Value {
    Value::from(b.iter().map(|x| json!(x)).collect::<Vec<_>>())
}

/// A bridge Config as the bridge program keeps it (lez.rs decode_config's layout).
fn config_bytes(c: &BridgeCode) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(c.cfg_id);
    b.extend(c.vault);
    b.extend(c.chain_id.to_le_bytes());
    b.extend(c.lez_chain_id);
    b.extend(c.zone_id);
    b.extend(c.token_program);
    b.extend([7u8; 32]);
    b.push(0);
    b.extend([7u8; 32]);
    b.extend([7u8; 32]);
    b.extend([0, 0]);
    b.extend((c.tokens.len() as u32).to_le_bytes());
    for t in &c.tokens {
        b.extend(t.eth_token);
        b.extend(t.definition);
        b.push(t.decimals);
        b.extend((t.denoms.len() as u32).to_le_bytes());
        for d in &t.denoms {
            b.extend(d.to_le_bytes());
        }
        for _ in 0..2 {
            b.extend((t.denoms.len() as u32).to_le_bytes());
            b.extend(vec![0u8; 8 * t.denoms.len()]);
        }
        b.extend([0u8; 32]);
        b.extend([1, 0, 0]);
    }
    b.push(0);
    b.extend(0u128.to_le_bytes());
    b
}

#[derive(Default)]
struct NetState {
    accounts: HashMap<String, Value>,
}

struct Net(Mutex<NetState>);

impl LezRpc for Net {
    fn describe(&self) -> String {
        "stand-in".into()
    }
    fn call(&self, method: &str, params: Value) -> Result<Value, Problem> {
        let s = self.0.lock().unwrap();
        match method {
            "getAccount" => Ok(s.accounts.get(params[0].as_str().unwrap()).cloned().unwrap_or(Value::Null)),
            _ => Err(Problem { code: "refused", message: format!("not served: {method}"), retry_after: None }),
        }
    }
}

impl NetState {
    fn serve(&mut self, c: &BridgeCode) {
        self.accounts.insert(lez::b58(&c.config_account), json!({ "nonce": 0, "balance": 0, "data": { "actor_states": { lez::b58(&c.bridge_program): bytes_json(&config_bytes(c)) } } }));
    }
}

struct World {
    b: Bridge,
    eth: Arc<Mutex<EthAnswers>>,
    net: Arc<Net>,
}

fn world(name: &str) -> World {
    let dir = std::env::temp_dir().join(format!("lez-bridge-bridges-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let eth = Arc::new(Mutex::new(EthAnswers::default()));
    let net = Arc::new(Net(Mutex::new(NetState::default())));
    let mut b = Bridge { data: dir.join("data"), cfg: Config::default(), fixtures: None, registry_cache: None, eth_override: Some(Box::new(FakeEth(eth.clone()))), relayed: Default::default() };
    b.relayed.rpc_override = Some(net.clone());
    World { b, eth, net }
}

fn ok(v: &str) -> Value {
    let r: Value = serde_json::from_str(v).unwrap();
    assert_eq!(r["success"], true, "{r}");
    r["value"].clone()
}

fn served(name: &str, c: &BridgeCode) -> World {
    let w = world(name);
    w.eth.lock().unwrap().serve(c);
    w.net.0.lock().unwrap().serve(c);
    w
}

#[test]
fn a_new_bridge_is_checked_on_both_chains_added_and_chosen_for_new_receives_only() {
    let c = v6();
    let code = c.encode().unwrap();
    let mut w = served("add", &c);
    assert_eq!(w.b.cfg.deployment.name, "bridge-test-network-v5", "a fresh install needs no setup");

    let list = ok(&w.b.bridges());
    assert_eq!(list["active"], "bridge-test-network-v5");
    let ids: Vec<&str> = list["list"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["bridge-test-network-v5", "bridge-test-network-v4"], "the built-in bridges with a code");
    assert_eq!(list["active_row"]["kind"], "built in");
    assert_eq!(list["active_row"]["words_text"], vector("v5").words_text(), "the page's words for v5");

    let check = ok(&w.b.bridge_check(code.clone()));
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!((check["builtin"].as_bool(), check["added"].as_bool()), (Some(false), Some(false)));
    assert_eq!(check["words_text"], c.words_text());
    assert_eq!(check["label"], "bridge v6");
    assert_eq!(check["endpoint_host"], "lez.example.org");
    let keys: Vec<&str> = check["checks"].as_array().unwrap().iter().map(|x| x["key"].as_str().unwrap()).collect();
    for k in ["sepolia", "vault", "proofs", "gate", "desk", "tokens", "lez", "lez_chain", "updates"] {
        assert!(keys.contains(&k), "{k} in {keys:?}");
    }
    assert!(check["checks"].as_array().unwrap().iter().all(|x| x["ok"] == true));
    assert!(!w.b.data.join("bridges.json").exists(), "a check keeps nothing");

    // Added and chosen: new receives use it; nothing else moves.
    let added = ok(&w.b.bridge_add(code.clone(), r#"{"use":true}"#.into()));
    assert_eq!((added["ok"].as_bool(), added["added"].as_bool(), added["current"].as_bool()), (Some(true), Some(true), Some(true)), "{added}");
    let id = c.added_name();
    assert_eq!(added["id"], id);
    assert_eq!(w.b.cfg.deployment.name, id);
    assert_eq!(w.b.cfg.deployment.lez_rpc, "https://lez.example.org/");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(w.b.data.join("bridges.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let list = ok(&w.b.bridges());
    let row = list["list"].as_array().unwrap().iter().find(|r| r["id"] == id.as_str()).unwrap().clone();
    assert_eq!((row["kind"].as_str(), row["current"].as_bool(), row["label"].as_str()), (Some("added"), Some(true), Some("bridge v6")));
    assert_eq!(row["words_text"], c.words_text());
    assert_eq!(ok(&w.b.bridge_code(id.clone()))["code"], code, "its code given back");
    // A second add of the same code adds nothing more.
    ok(&w.b.bridge_add(code.clone(), "{}".into()));
    assert_eq!(w.b.added_codes().len(), 1);
    // After a restart the choice and the list are read back.
    let again = Bridge { data: w.b.data.clone(), cfg: Config::load(&w.b.data), fixtures: None, registry_cache: None, eth_override: None, relayed: Default::default() };
    assert_eq!(again.cfg.deployment.name, id);
    assert_eq!(again.deployment_named(&id).unwrap().vault, c.vault);

    // A deposit stored on v5 is still read on v5.
    let v5 = config::builtin("bridge-test-network-v5").unwrap();
    let row = json!({ "id": "dep-1", "deployment": v5.name, "vault": util::checksum_address(&v5.vault), "chain_id": v5.chain_id });
    assert_eq!(w.b.deposit_deployment(&row).unwrap().vault, v5.vault);

    // Not removed while chosen; not removed while a deposit names it; removed otherwise.
    let r: Value = serde_json::from_str(&w.b.bridge_remove(id.clone())).unwrap();
    assert!(r["error"].as_str().unwrap().contains("Choose another one first"), "{r}");
    ok(&w.b.bridge_use(v5.name.clone()));
    let store = w.b.store();
    let mut all = store.load();
    all["deposits"] = json!([{ "id": "dep-6", "deployment": id }]);
    store.save(&all).unwrap();
    let r: Value = serde_json::from_str(&w.b.bridge_remove(id.clone())).unwrap();
    assert!(r["error"].as_str().unwrap().contains("1 thing"), "{r}");
    all["deposits"] = json!([]);
    store.save(&all).unwrap();
    let left = ok(&w.b.bridge_remove(id.clone()));
    assert!(left["list"].as_array().unwrap().iter().all(|r| r["id"] != id.as_str()));
    assert!(w.b.deployment_named(&id).is_none());
    let r: Value = serde_json::from_str(&w.b.bridge_remove("bridge-test-network-v4".into())).unwrap();
    assert!(r["error"].as_str().unwrap().contains("built-in"));
}

#[test]
fn a_code_that_disagrees_with_a_chain_is_refused_and_nothing_is_kept() {
    let c = v6();
    let code = c.encode().unwrap();
    let refused = |w: &mut World, want: &str| {
        let r = ok(&w.b.bridge_add(code.clone(), r#"{"use":true}"#.into()));
        assert_eq!(r["ok"], false, "{r}");
        assert_eq!(r["error"]["code"], "mismatch", "{r}");
        assert!(r["error"]["message"].as_str().unwrap().contains(want), "{want}: {r}");
        assert!(!w.b.data.join("bridges.json").exists(), "nothing kept");
        assert_eq!(w.b.cfg.deployment.name, "bridge-test-network-v5", "nothing chosen");
    };
    // The desk sends deposits into another vault.
    let mut w = served("desk", &c);
    w.eth.lock().unwrap().put(&c.desk, "vault()", &[], eth::word_address(&[0x99; 20]).to_vec());
    refused(&mut w, "deposit desk");
    // The vault on Sepolia has another cfgId (another LEZ chain or program behind it).
    let mut w = served("cfg", &c);
    w.eth.lock().unwrap().put(&c.vault, "cfgId()", &[], vec![0x11; 32]);
    refused(&mut w, "cfgId");
    // Another release delay than the code says.
    let mut w = served("delay", &c);
    w.eth.lock().unwrap().put(&c.vault, "minReleaseDelay()", &[], eth::word_u64(0).to_vec());
    refused(&mut w, "release delay");
    // The vault's verifier is not the code's gate.
    let mut w = served("gate", &c);
    w.eth.lock().unwrap().put(&c.vault, "verifier()", &[], eth::word_address(&[0x77; 20]).to_vec());
    refused(&mut w, "gate");
    // The LEZ Config lists other amounts.
    let mut w = served("lez", &c);
    let mut other = c.clone();
    other.tokens[0].denoms = vec![1, 2, 3];
    w.net.0.lock().unwrap().serve(&other);
    refused(&mut w, "decimals or amounts");
    // The LEZ Config is for another vault.
    let mut w = served("lezvault", &c);
    let mut other = c.clone();
    other.vault = [0x55; 20];
    w.net.0.lock().unwrap().serve(&other);
    refused(&mut w, "vault");
    // No bridge on that endpoint.
    let mut w = served("nobridge", &c);
    w.net.0.lock().unwrap().accounts.clear();
    refused(&mut w, "There is no bridge");

    // A code that can't be read, or that poses as another bridge, is refused before any read.
    let mut w = served("bad", &c);
    let mut forged = c.payload();
    forged[10] ^= 1; // the vault's address: its cfgId no longer follows
    let r = ok(&w.b.bridge_check(crate::request::bech32m(crate::bridge_code::HRP, &forged)));
    assert_eq!(r["error"]["code"], "cfg_id", "{r}");
    for label in ["Bridge V4", "bridge v5 (built in)", "new bridge v5", "Built-in"] {
        let mut posing = c.clone();
        posing.label = label.into();
        let r = ok(&w.b.bridge_check(posing.encode().unwrap()));
        assert_eq!(r["error"]["code"], "label", "{label}: {r}");
    }
    let r = ok(&w.b.bridge_check("logoswd1qqqq".into()));
    assert_eq!(r["ok"], false);
    assert!(w.eth.lock().unwrap().seen.is_empty(), "nothing was read for codes refused offline");
    assert!(r["warning"].as_str().unwrap().contains("bridge's own page"));
}

#[test]
fn a_code_for_a_built_in_bridge_must_be_that_bridge() {
    let v4 = vector("v4");
    let mut w = served("builtin", &v4);
    let r = ok(&w.b.bridge_check(v4.encode().unwrap()));
    assert_eq!((r["ok"].as_bool(), r["builtin"].as_bool(), r["id"].as_str()), (Some(true), Some(true), Some("bridge-test-network-v4")), "{r}");
    assert_eq!(r["label"], "bridge v4");
    let r = ok(&w.b.bridge_add(v4.encode().unwrap(), r#"{"use":true}"#.into()));
    assert_eq!(r["ok"], true);
    assert!(w.b.added_codes().is_empty(), "a built-in bridge is never stored again");
    assert_eq!(w.b.cfg.deployment.name, "bridge-test-network-v4");
    // v4's vault with another endpoint (or anything else changed) is not v4.
    let mut other = v4.clone();
    other.endpoint = "https://evil.example/".into();
    let r = ok(&w.b.bridge_check(other.encode().unwrap()));
    assert_eq!(r["error"]["code"], "not_builtin", "{r}");
}

#[test]
fn an_added_bridge_s_claim_file_and_deposit_are_read_on_it() {
    let c = v6();
    let mut w = served("claimfile", &c);
    ok(&w.b.bridge_add(c.encode().unwrap(), "{}".into()));
    assert_eq!(w.b.cfg.deployment.name, "bridge-test-network-v5", "added, not chosen");
    let known: Vec<String> = w.b.known_deployments().iter().map(|d| d.name.clone()).collect();
    assert!(known.contains(&c.added_name()));
    let row = json!({ "id": "dep-6", "deployment": c.added_name(), "vault": util::checksum_address(&c.vault), "chain_id": 11_155_111 });
    let d = w.b.deposit_deployment(&row).unwrap();
    assert_eq!((d.vault, d.lez_rpc.as_str()), (c.vault, "https://lez.example.org/"));
    // A deposit kept with another vault than the bridge it names is not guessed.
    let row = json!({ "id": "dep-7", "deployment": c.added_name(), "vault": util::checksum_address(&[1; 20]), "chain_id": 11_155_111 });
    assert!(w.b.deposit_deployment(&row).is_err());
}
