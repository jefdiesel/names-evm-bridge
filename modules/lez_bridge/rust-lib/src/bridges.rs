// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bridge deployments as data (`modules/NAMES-SEND-BACK.md` §9): the built-in list
//! (`deployments.json`, v5 the default) plus the ones this install added from a bridge code
//! (`logosbr1…`, `bridge_code.rs`), kept in `<data>/bridges.json` (mode 600) as their codes.
//!
//! - A code is accepted only after both chains agree with it and with each other: Sepolia through
//!   this module's own RPC settings (never a URL from the code) and the bridge's Config on the
//!   code's LEZ endpoint (`verify`). The person then sees four words, the same the bridge page shows.
//! - Choosing a bridge (`bridge_use`) picks the deployment NEW receives and send-backs use. A
//!   stored deposit, send-back or claim job keeps the deployment it names: nothing in flight is
//!   ever re-pointed.
//! - An added bridge is stored under a name made from its cfgId, never from the code's text, and
//!   a code can't shadow a built-in one: a code for a built-in vault must equal it field for field.

use crate::bridge_code::BridgeCode;
use crate::config::{self, Deployment};
use crate::eth::{self, Words};
use crate::lez::{self, LezRpc};
use crate::util::{self, hex0x, now_ms};
use crate::Bridge;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Shown wherever a code is pasted.
pub const ADD_WARNING: &str = "A bridge code decides where your deposits go. Take it only from the bridge's own page.";
const FILE: &str = "bridges.json";
/// At most this many added bridges (the Names Wallet reads at most four bridges in all).
const MAX_ADDED: usize = 8;
/// At most this many remembered Ethereum addresses.
const MAX_RECIPIENTS: usize = 20;

/// `Public/<id>` for an account given with or without its prefix.
fn written_account(account: &str) -> String {
    let a = account.trim();
    if a.starts_with("Public/") || a.starts_with("Private/") { a.to_owned() } else { format!("Public/{a}") }
}

fn check(key: &str, ok: bool, text: impl Into<String>) -> Value {
    json!({ "key": key, "ok": ok, "text": text.into() })
}

fn short_addr(a: &[u8; 20]) -> String {
    let s = util::checksum_address(a);
    format!("{}…{}", &s[..6], &s[s.len() - 4..])
}

/// What two values are called in a mismatch line.
fn differ(what: &str, chain: impl std::fmt::Display, code: impl std::fmt::Display) -> String {
    format!("{what}: the chain says {chain}, the code says {code}")
}

impl Bridge {
    fn bridges_file(&self) -> PathBuf {
        self.data.join(FILE)
    }

    /// The whole file: `{added: [{id, code, added, label}], recipients: [{account, chain_id, address,
    /// last_used}]}` (public data only).
    fn bridges_store(&self) -> Value {
        util::read_json(&self.bridges_file()).filter(Value::is_object).unwrap_or_else(|| json!({}))
    }

    fn save_bridges_store(&self, v: &Value) -> Result<(), String> {
        util::private_dir(&self.data).map_err(|e| e.to_string())?;
        util::write_private(&self.bridges_file(), serde_json::to_vec_pretty(v).unwrap_or_default().as_slice()).map_err(|e| format!("cannot save the bridge list: {e}"))
    }

    /// The stored entries `{id, code, added, label}`; unreadable ones are left out.
    fn added_entries(&self) -> Vec<Value> {
        self.bridges_store()["added"].as_array().cloned().unwrap_or_default()
    }

    /// Every added bridge, decoded again from its stored code.
    pub(crate) fn added_codes(&self) -> Vec<BridgeCode> {
        self.added_entries().iter().filter_map(|e| BridgeCode::decode(e["code"].as_str()?).ok()).collect()
    }

    pub(crate) fn added_deployment(&self, name: &str) -> Option<Deployment> {
        self.added_codes().into_iter().find(|c| c.added_name() == name).map(|c| c.to_deployment())
    }

    /// The built-in deployments that have a real LEZ endpoint, then the added ones.
    pub(crate) fn known_deployments(&self) -> Vec<Deployment> {
        let mut out: Vec<Deployment> = config::builtin_names().iter().filter_map(|n| config::builtin(n)).collect();
        out.extend(self.added_codes().iter().map(BridgeCode::to_deployment));
        out
    }

    fn save_added(&self, list: &[Value]) -> Result<(), String> {
        let mut v = self.bridges_store();
        v["added"] = json!(list);
        self.save_bridges_store(&v)
    }

    // ── Ethereum addresses sent to before (owner, 2026-11-16): offered first next time ──

    /// Keeps `recipient` as an address `account` sent back to on chain `chain_id` (after the burn
    /// was signed and handed to the relay). Public data; the newest first, at most [`MAX_RECIPIENTS`].
    pub(crate) fn remember_recipient(&self, account: &str, chain_id: u64, recipient: &[u8; 20]) -> Result<(), String> {
        let account = written_account(account);
        let address = util::checksum_address(recipient);
        let mut v = self.bridges_store();
        let mut list: Vec<Value> = v["recipients"].as_array().cloned().unwrap_or_default();
        list.retain(|x| !(x["account"] == account.as_str() && x["chain_id"].as_u64() == Some(chain_id) && x["address"] == address.as_str()));
        list.insert(0, json!({ "account": account, "chain_id": chain_id, "address": address, "last_used": now_ms() }));
        list.truncate(MAX_RECIPIENTS);
        v["recipients"] = json!(list);
        self.save_bridges_store(&v)
    }

    /// `send_back_recipients(account)`: the Ethereum addresses `account` sent back to before on this
    /// bridge's chain, newest first (`""`: every account's, for Settings). No chain read.
    pub(crate) fn send_back_recipients_now(&self, account: &str) -> Value {
        let chain = self.deployment().chain_id;
        let want = if account.trim().is_empty() { None } else { Some(written_account(account)) };
        let list: Vec<Value> = self.bridges_store()["recipients"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|x| x["chain_id"].as_u64() == Some(chain) && want.as_ref().is_none_or(|a| x["account"] == a.as_str()))
            .filter_map(|x| {
                let a = util::eth_address(x["address"].as_str()?).ok()?;
                let text = util::checksum_address(&a);
                Some(json!({ "account": x["account"], "chain_id": chain, "address": text, "short": format!("{}…{}", &text[..6], &text[text.len() - 4..]), "last_used": x["last_used"] }))
            })
            .collect();
        json!({ "recipients": list, "chain_id": chain })
    }

    /// `send_back_forget(account, address)`: removes a remembered address (every account's when
    /// `account` is "").
    pub(crate) fn send_back_forget_now(&mut self, account: &str, address: &str) -> Result<Value, String> {
        let a = util::checksum_address(&util::eth_address(address.trim())?);
        let who = if account.trim().is_empty() { None } else { Some(written_account(account)) };
        let mut v = self.bridges_store();
        let mut list: Vec<Value> = v["recipients"].as_array().cloned().unwrap_or_default();
        list.retain(|x| !(x["address"] == a.as_str() && who.as_ref().is_none_or(|w| x["account"] == w.as_str())));
        v["recipients"] = json!(list);
        self.save_bridges_store(&v)?;
        Ok(self.send_back_recipients_now(""))
    }

    /// How many things on this computer name deployment `d`: deposits, send-backs and claim jobs.
    fn items_on(&self, d: &Deployment) -> usize {
        let all = self.store().load();
        let deposits = all["deposits"].as_array().map_or(0, |l| l.iter().filter(|x| x["deployment"] == d.name.as_str()).count());
        let sends = all["send_backs"].as_array().map_or(0, |l| l.iter().filter(|x| x["deployment"] == d.name.as_str()).count());
        let jobs = self.wstate()["relayed_jobs"].as_array().map_or(0, |l| l.iter().filter(|j| j["meta"]["deployment"] == d.name.as_str() || j["meta"]["bridge"] == d.bridge_program.as_str()).count());
        deposits + sends + jobs
    }

    /// One row of the list: who it is, its vault, its four words (when it has a code).
    fn bridge_row(&self, d: &Deployment) -> Value {
        let code = BridgeCode::from_deployment(d).ok();
        let added = d.bridge["added"] == true;
        let items = self.items_on(d);
        let current = d.name == self.deployment().name;
        json!({
            "id": d.name, "label": if added { d.label.clone() } else { config::plain_label(&d.name) }, "kind": if added { "added" } else { "built in" },
            "current": current, "vault": util::checksum_address(&d.vault), "vault_short": short_addr(&d.vault), "chain_id": d.chain_id,
            "bridge_program": d.bridge_program,
            "words": code.as_ref().map(|c| crate::compare::words(&c.digest())), "words_text": code.as_ref().map(BridgeCode::words_text),
            "has_code": code.is_some(), "release": d.release, "release_note": d.release_note, "network": d.network,
            "endpoint_host": code.as_ref().map(BridgeCode::endpoint_host), "items": items, "removable": added && !current && items == 0,
        })
    }

    /// `bridges()`: the bridge new receives and send-backs use, and the list to choose from (the
    /// built-in ones with a code, the added ones). No chain read.
    pub(crate) fn bridges_now(&self) -> Value {
        let mut list: Vec<Value> = config::builtin_names()
            .iter()
            .filter_map(|n| config::builtin(n))
            .filter(|d| d.bridge.is_object() || d.name == self.deployment().name)
            .map(|d| self.bridge_row(&d))
            .collect();
        list.extend(self.added_codes().iter().map(|c| self.bridge_row(&c.to_deployment())));
        if !list.iter().any(|r| r["current"] == true) {
            list.push(self.bridge_row(self.deployment()));
        }
        let active_row = list.iter().find(|r| r["current"] == true).cloned();
        json!({ "active": self.deployment().name, "active_row": active_row, "list": list, "warning": ADD_WARNING,
                "older": "Bridges v3 and v2 stay readable for their deposits; they can't pay out on Ethereum." })
    }

    /// `bridge_code(id)`: the code of a listed bridge, to give to someone (built-in v5/v4, or added).
    pub(crate) fn bridge_code_now(&self, id: &str) -> Result<Value, String> {
        let d = if id.trim().is_empty() { self.deployment().clone() } else { self.deployment_named(id.trim()).ok_or_else(|| format!("there is no bridge \"{id}\" on this computer"))? };
        let c = BridgeCode::from_deployment(&d)?;
        Ok(json!({ "id": d.name, "code": c.encode().map_err(|e| e.message)?, "words": crate::compare::words(&c.digest()), "words_text": c.words_text() }))
    }

    /// The LEZ reader for a code's endpoint (tests: the stand-in).
    fn code_rpc(&self, c: &BridgeCode) -> Arc<dyn LezRpc + Send + Sync> {
        match &self.relayed.rpc_override {
            Some(r) => Arc::new(lez::Patient::new(r.clone())),
            None => Arc::new(lez::Patient::new(Arc::new(lez::Http::new(&c.endpoint)))),
        }
    }

    /// Both chains against the code, and against each other. `Ok(checks)` when every read was
    /// made (some may have failed: a mismatch); `Err` when a chain couldn't be read at all.
    fn verify(&self, c: &BridgeCode) -> Result<Vec<Value>, String> {
        if c.chain_id != 11_155_111 {
            return Ok(vec![check("sepolia", false, format!("This app reads Sepolia (chain 11155111) only; this code is for chain {}.", c.chain_id))]);
        }
        let mut out = Vec::new();
        let eth = self.eth_for(c.chain_id).map_err(|e| format!("Sepolia can't be read: {e}"))?;
        let sepolia = |e: String| format!("Sepolia can't be read now ({e})");
        let got = eth.chain_id().map_err(sepolia)?;
        out.push(check("sepolia", got == c.chain_id, if got == c.chain_id { "Sepolia answers as chain 11155111.".to_owned() } else { format!("The Sepolia service answered as chain {got}.") }));
        // The vault: cfgId and its parts (the cfgId binds the LEZ side), the proof images, the delay.
        let call = |to: &[u8; 20], sig: &str, args: &[[u8; 32]]| eth.call(to, &eth::calldata(sig, args)).map_err(sepolia);
        let w32 = |to: &[u8; 20], sig: &str| -> Result<[u8; 32], String> { Words(&call(to, sig, &[])?).word(0) };
        let cfg = w32(&c.vault, "cfgId()").map_err(|e| format!("The vault {} can't be read on Sepolia ({e}). Nothing was added.", util::checksum_address(&c.vault)))?;
        let mut bad = Vec::new();
        if cfg != c.cfg_id {
            bad.push(differ("cfgId", hex0x(&cfg), hex0x(&c.cfg_id)));
        }
        for (sig, want, what) in [("programId()", c.bridge_program, "bridge program"), ("lezChainId()", c.lez_chain_id, "LEZ chain"), ("zoneId()", c.zone_id, "zone")] {
            let v = w32(&c.vault, sig)?;
            if v != want {
                bad.push(differ(what, hex0x(&v), hex0x(&want)));
            }
        }
        out.push(check("vault", bad.is_empty(), if bad.is_empty() { format!("The vault {} on Sepolia has this cfgId, bridge program, LEZ chain and zone.", short_addr(&c.vault)) } else { format!("The vault on Sepolia doesn't match the code: {}.", bad.join("; ")) }));
        let mut bad = Vec::new();
        for (sig, want, what) in [("updateImageId()", c.update_image, "update proof image"), ("releaseImageId()", c.release_image, "release proof image")] {
            let v = w32(&c.vault, sig)?;
            if v != want {
                bad.push(differ(what, hex0x(&v), hex0x(&want)));
            }
        }
        let delay = Words(&call(&c.vault, "minReleaseDelay()", &[])?).u64(0)?;
        if delay != c.min_release_delay {
            bad.push(differ("minimum release delay", delay, c.min_release_delay));
        }
        out.push(check("proofs", bad.is_empty(), if bad.is_empty() { "The vault's update and release proof images and its release delay are the code's.".to_owned() } else { format!("The vault doesn't match the code: {}.", bad.join("; ")) }));
        // The gate in front of the verifier (updates only from its submitter).
        let verifier = Words(&call(&c.vault, "verifier()", &[])?).address(0)?;
        if c.gate == [0; 20] {
            out.push(check("gate", true, format!("The code names no update gate; the vault's verifier is {}.", short_addr(&verifier))));
        } else {
            let gate_image = if verifier == c.gate { w32(&c.gate, "UPDATE_IMAGE_ID()").ok() } else { None };
            let ok = verifier == c.gate && gate_image == Some(c.update_image);
            out.push(check("gate", ok, if ok { format!("The vault's verifier is the code's update gate {}, for the same update image.", short_addr(&c.gate)) } else if verifier != c.gate { format!("The vault's verifier is {}, not the code's gate {}.", short_addr(&verifier), short_addr(&c.gate)) } else { "The gate checks another update image than the vault.".to_owned() }));
        }
        // The deposit desk, if any, must send into this vault.
        if c.desk == [0; 20] {
            out.push(check("desk", true, "The code names no deposit desk: deposits go straight to the vault."));
        } else {
            let into = Words(&call(&c.desk, "vault()", &[]).map_err(|e| format!("The deposit desk can't be read ({e}). Nothing was added."))?).address(0)?;
            out.push(check("desk", into == c.vault, if into == c.vault { format!("The deposit desk {} sends deposits into this vault.", short_addr(&c.desk)) } else { format!("The deposit desk {} sends deposits into {}, not this vault.", short_addr(&c.desk), short_addr(&into)) }));
        }
        // The tokens on Sepolia.
        let mut bad = Vec::new();
        for t in &c.tokens {
            let r = call(&c.vault, "tokenConfig(address)", &[eth::word_address(&t.eth_token)])?;
            let w = Words(&r);
            let denoms: Vec<String> = w.dec_array(5)?;
            let want: Vec<String> = t.denoms.iter().map(u128::to_string).collect();
            if !w.boolean(0)? {
                bad.push(format!("{} is not registered in the vault", t.symbol));
            } else if w.u64(3)? != u64::from(t.decimals) || denoms != want {
                bad.push(format!("{}'s decimals or amounts differ from the vault's", t.symbol));
            }
        }
        let symbols = c.tokens.iter().map(|t| t.symbol.as_str()).collect::<Vec<_>>().join(", ");
        out.push(check("tokens", bad.is_empty(), if bad.is_empty() { format!("The vault lists {symbols} with the code's amounts.") } else { format!("On Sepolia: {}.", bad.join("; ")) }));
        // The LEZ side: the bridge's Config on the code's endpoint.
        let rpc = self.code_rpc(c);
        let host = c.endpoint_host();
        let show = match lez::show_json(rpc.as_ref(), &c.bridge_program) {
            Ok(v) => v,
            Err(e) if e.contains("no bridge Config") => {
                out.push(check("lez", false, format!("There is no bridge {} on {host}.", lez::b58(&c.bridge_program))));
                return Ok(out);
            }
            Err(e) => return Err(format!("{host} can't be read now ({e}). Nothing was added; try again.")),
        };
        let s = |k: &str| show[k].as_str().unwrap_or_default().to_ascii_lowercase();
        let mut bad = Vec::new();
        for (k, want, what) in [
            ("cfg_id", hex0x(&c.cfg_id), "cfgId"),
            ("vault", hex0x(&c.vault), "vault"),
            ("lez_chain_id", hex0x(&c.lez_chain_id), "LEZ chain"),
            ("zone_id", hex0x(&c.zone_id), "zone"),
        ] {
            if s(k) != want {
                bad.push(differ(what, s(k), want));
            }
        }
        if show["evm_chain_id"].as_u64() != Some(c.chain_id) {
            bad.push(differ("Ethereum chain", &show["evm_chain_id"], c.chain_id));
        }
        if show["token_program"].as_str() != Some(lez::b58(&c.token_program).as_str()) {
            bad.push(differ("token program", show["token_program"].as_str().unwrap_or("?"), lez::b58(&c.token_program)));
        }
        let listed = show["tokens"].as_array().cloned().unwrap_or_default();
        for t in &c.tokens {
            let eth_token = format!("0x{}", util::to_hex(&t.eth_token));
            let found = listed.iter().find(|x| x["eth_token"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(&eth_token)));
            let want: Vec<Value> = t.denoms.iter().map(|d| json!(d.to_string())).collect();
            match found {
                None => bad.push(format!("the bridge doesn't list {}", t.symbol)),
                Some(x) if x["definition"] != lez::b58(&t.definition).as_str() => bad.push(format!("{} has another token definition there", t.symbol)),
                Some(x) if x["decimals"].as_u64() != Some(u64::from(t.decimals)) || x["denoms"].as_array() != Some(&want) || x["registered"] != true => {
                    bad.push(format!("{}'s decimals or amounts differ there", t.symbol))
                }
                Some(_) => {}
            }
        }
        out.push(check("lez", bad.is_empty(), if bad.is_empty() { format!("On {host}, the bridge's Config names this vault, cfgId, chain, token program and tokens.") } else { format!("On {host}, the bridge's Config doesn't match: {}.", bad.join("; ")) }));
        // The endpoint's own chain id, where it publishes one.
        match lez::chain_channel_id(rpc.as_ref()) {
            Ok(Some(id)) => out.push(check("lez_chain", id == c.lez_chain_id, if id == c.lez_chain_id { format!("{host} is the LEZ chain the vault is bound to.") } else { format!("{host} is another LEZ chain ({}) than the vault's.", hex0x(&id)) })),
            Ok(None) => out.push(check("lez_chain", true, format!("{host} publishes no chain id; the bridge's Config names the vault's."))),
            Err(e) => return Err(format!("{host} can't be read now ({e}). Nothing was added; try again.")),
        }
        // For information: has the vault taken updates since its starting state?
        if let Ok(g2) = w32(&c.vault, "g2State()") {
            out.push(check("updates", true, if g2 == c.genesis_g2_state { "No update on Ethereum yet: the vault is at the code's starting state." } else { "The vault has taken updates since its starting state." }));
        }
        Ok(out)
    }

    /// The decoded code, its chain checks and what it is to this install. `Err` (as JSON): a code
    /// that can't be read, or a chain that can't be read.
    fn check_code(&self, code: &str) -> Result<(BridgeCode, Value), Value> {
        let c = BridgeCode::decode(code).map_err(|e| json!({ "code": e.code, "message": e.message }))?;
        // A built-in vault: the code must be that deployment, field for field.
        let builtin = config::builtin_names().into_iter().filter_map(|n| config::builtin(&n)).find(|d| d.vault == c.vault && d.chain_id == c.chain_id);
        if let Some(b) = &builtin {
            match BridgeCode::from_deployment(b) {
                Ok(bc) if bc == c => {}
                _ => {
                    return Err(json!({ "code": "not_builtin", "message": format!("This code names the vault of {}, but its other details differ from this app's {}. Refused.", config::plain_label(&b.name), config::plain_label(&b.name)) }));
                }
            }
        } else {
            // An added bridge may not pose as a built-in one: no built-in name inside its name, and
            // not "built in" ("bridge v5 (built in)" would read like the real one in the list).
            let low = c.label.to_ascii_lowercase();
            if low.contains("built") || config::builtin_names().iter().any(|n| low.contains(&config::plain_label(n).to_ascii_lowercase())) {
                return Err(json!({ "code": "label", "message": format!("This code calls itself \"{}\", which reads like a bridge this app has built in. Refused.", c.label) }));
            }
        }
        let other = self.added_codes().into_iter().find(|a| a.cfg_id == c.cfg_id && *a != c);
        if other.is_some() {
            return Err(json!({ "code": "added_differs", "message": "A bridge with this vault is added already, with other details. Remove that one first if this code replaces it." }));
        }
        if self.added_codes().iter().any(|a| a.cfg_id != c.cfg_id && a.label.eq_ignore_ascii_case(&c.label)) {
            return Err(json!({ "code": "label", "message": format!("Another added bridge is called \"{}\" already. Refused.", c.label) }));
        }
        let checks = match self.verify(&c) {
            Ok(x) => x,
            Err(e) => return Err(json!({ "code": "unreachable", "message": e })),
        };
        let id = builtin.as_ref().map_or_else(|| c.added_name(), |b| b.name.clone());
        let error = checks
            .iter()
            .find(|x| x["ok"] != true)
            .map(|f| json!({ "code": "mismatch", "message": format!("{} Refused: nothing was added.", f["text"].as_str().unwrap_or_default()) }));
        let d = builtin.clone().unwrap_or_else(|| c.to_deployment());
        let v = json!({
            "ok": error.is_none(), "id": id, "builtin": builtin.is_some(),
            "added": builtin.is_none() && self.added_codes().iter().any(|a| *a == c), "current": id == self.deployment().name,
            "label": if builtin.is_some() { config::plain_label(&id) } else { c.label.clone() },
            "vault": util::checksum_address(&c.vault), "vault_short": short_addr(&c.vault), "chain_id": c.chain_id,
            "bridge_program": lez::b58(&c.bridge_program), "endpoint_host": c.endpoint_host(), "tokens": c.tokens.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>(),
            "release_note": d.release_note, "words": crate::compare::words(&c.digest()), "words_text": c.words_text(),
            "checks": checks, "warning": ADD_WARNING, "error": error,
        });
        Ok((c, v))
    }

    /// `bridge_check(code)`: what a pasted code is, checked on both chains; nothing stored.
    pub(crate) fn bridge_check_now(&self, code: &str) -> Value {
        match self.check_code(code) {
            Ok((_, v)) => v,
            Err(e) => json!({ "ok": false, "error": e, "warning": ADD_WARNING }),
        }
    }

    /// `bridge_add(code, options)`: checks the code again on both chains (never trusting an earlier
    /// check), then keeps it; `{"use": true}` also makes it the bridge for new receives.
    pub(crate) fn bridge_add_now(&mut self, code: &str, options: &Value) -> Result<Value, String> {
        let (c, mut v) = match self.check_code(code) {
            Ok(x) => x,
            Err(e) => return Ok(json!({ "ok": false, "error": e, "warning": ADD_WARNING })),
        };
        if v["ok"] != true {
            return Ok(v);
        }
        let id = v["id"].as_str().unwrap_or_default().to_owned();
        if v["builtin"] != true && v["added"] != true {
            let mut list = self.added_entries();
            if list.len() >= MAX_ADDED {
                return Err(format!("This app keeps at most {MAX_ADDED} added bridges; remove one first."));
            }
            list.push(json!({ "id": id, "code": c.encode().map_err(|e| e.message)?, "added": now_ms(), "label": c.label }));
            self.save_added(&list)?;
            v["added"] = json!(true);
        }
        if options["use"] == true {
            self.bridge_use_now(&id)?;
            v["current"] = json!(true);
        }
        v["bridges"] = self.bridges_now();
        Ok(v)
    }

    /// `bridge_use(id)`: the bridge NEW receives and send-backs use. Nothing stored moves.
    pub(crate) fn bridge_use_now(&mut self, id: &str) -> Result<Value, String> {
        let id = id.trim();
        let d = config::builtin(id).or_else(|| self.added_deployment(id)).ok_or_else(|| format!("there is no bridge \"{id}\" on this computer"))?;
        self.cfg.deployment = d;
        self.registry_cache = None;
        self.save()?;
        Ok(self.bridges_now())
    }

    /// `bridge_remove(id)`: forgets an added bridge, only when it isn't chosen and nothing on this
    /// computer names it (a deposit, a send-back or a claim on it stays readable).
    pub(crate) fn bridge_remove_now(&mut self, id: &str) -> Result<Value, String> {
        let id = id.trim();
        if config::builtin(id).is_some() {
            return Err("A built-in bridge can't be removed.".into());
        }
        let d = self.added_deployment(id).ok_or_else(|| format!("there is no added bridge \"{id}\" on this computer"))?;
        if d.name == self.deployment().name {
            return Err("This is the bridge new receives use. Choose another one first, then remove it.".into());
        }
        let n = self.items_on(&d);
        if n > 0 {
            return Err(format!("{} on this computer {} made on this bridge, so it stays: removing it would leave {} unreadable.", util::count(n as u64, "thing", "things"), if n == 1 { "was" } else { "were" }, if n == 1 { "it" } else { "them" }));
        }
        let list: Vec<Value> = self.added_entries().into_iter().filter(|e| e["id"] != id).collect();
        self.save_added(&list)?;
        Ok(self.bridges_now())
    }
}
