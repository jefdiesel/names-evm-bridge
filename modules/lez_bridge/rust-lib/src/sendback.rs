// SPDX-License-Identifier: MIT OR Apache-2.0
//! "Send back to Ethereum" from a public Names Wallet account (`modules/NAMES-SEND-BACK.md`).
//!
//! The person picks one denomination; the bridge page gives a withdraw-to code for the Ethereum
//! wallet they connect there (`withdraw.rs`). This module then:
//! - derives the exit secret `k'` with the wallet's `app_secret` (scope `RFP022/exit`, the context
//!   binding the recipient and the max relay fee) and keeps only its index, never `k'`;
//! - builds the burn (`BurnWithNotice` to the bridge's config, memo `E`) as the relayed call the
//!   Names Wallet signs and the bridge's relay pays for (golden vector `testdata/public-burn-v1.json`);
//! - follows it: the burn's block on LEZ, the exit leaf in the vault's `StateUpdated` logs (the tree
//!   rebuilt and checked against `exitCount()`/`exitRoot()`), the release proof the person's computer
//!   makes (a helper this module starts; it never proves itself), the Groth16 seal (checked with a
//!   read-only `verify` on the vault's verifier), the one-hour delay, the release;
//! - gives the release code the page sends `release` with.
//!
//! No reply carries `k'`. It is in memory only while `E` is computed, and on a prover's stdin, or
//! (the stand-in `send_back_witness`) in a mode-600 file the owner hands to his own prover run.

use crate::eth::{self, Words};
use crate::jobs::Spec;
use crate::lez;
use crate::util::{self, format_amount, hex0x, hex32, now_ms, sha256, to_hex, units};
use crate::wallet::{self, Values};
use crate::withdraw::{self, ReleaseCode, WithdrawTo};
use crate::Bridge;
use serde_json::{json, Value};
use std::path::PathBuf;

/// keccak256("RFP022/exit-leaf/v1"): BridgeVault.EXIT_LEAF_TAG.
pub const EXIT_LEAF_TAG: [u8; 32] = [
    0x22, 0x01, 0x87, 0x42, 0xad, 0x60, 0x28, 0xfd, 0xa6, 0xcb, 0x07, 0x26, 0xaf, 0xaa, 0xe9, 0x0e, 0xa2, 0x0d, 0xff, 0x3c, 0xef, 0x6b, 0x1e, 0xc5, 0xb3,
    0xc1, 0x4f, 0x85, 0x6d, 0x76, 0xc5, 0x9c,
];
pub const PAGE_LINK: &str = "https://nameswallet.app/bridge/#withdraw";
/// What the wallet shows before an exit secret goes to the bridge's settlement service.
pub const SETTLE_CONSENT: &str = "This wallet has no proof helper, so the bridge's service will make the proof: it learns which burn pays which Ethereum address.";
pub const PRIVACY_LINE: &str = "A public burn publishes this account, the amount and an exit commitment; the Ethereum address stays hidden until the release.";
/// Measured: the release proof, 36K cycles, 20.7 s on an M4 CPU, under 1 GB (RUNBOOK §14).
pub const PROOF_S: u64 = 21;
pub const PROOF_PEAK_MB: u64 = 1000;
/// The relayed burn's transaction is about this many bytes besides its message (two signers).
const WIRE_OVERHEAD: u64 = 420;
/// LEZ blocks a status poll reads at most while it looks for the burn (15 s blocks: 40 is 10 min).
const SCAN_PER_POLL: u64 = 1000;
/// Blocks read at once before a burn is proposed or sent (30 range reads, about 12 h of chain).
const SCAN_MAX: u64 = 3000;

pub fn exit_leaf(cfg: &[u8; 32], token: &[u8; 20], denom: u128, e: &[u8; 32]) -> [u8; 32] {
    sha256(&[&EXIT_LEAF_TAG, cfg, token, &wallet::u256(denom), e])
}

/// borsh(token `Message::BurnWithNotice { definition, amount, notify: Actor(config PDA, bridge), memo: E }`):
/// variant 10, the definition, the amount (u128 LE), the actor (account, program), the memo.
pub fn burn_message(definition: &[u8; 32], amount: u128, bridge: &[u8; 32], e: &[u8; 32]) -> Vec<u8> {
    let mut m = vec![10u8];
    m.extend_from_slice(definition);
    m.extend_from_slice(&amount.to_le_bytes());
    m.extend_from_slice(&lez::pda(bridge, &lez::CONFIG_SEED));
    m.extend_from_slice(bridge);
    m.extend_from_slice(e);
    m
}

/// `propose_relayed_call`'s `call_json` for the burn: the burner's actor of the token program is
/// the root; the definition and the bridge's config (by seed) are the declared actors; the relay pays.
pub fn burn_call(token_program: &[u8; 32], definition: &[u8; 32], bridge: &[u8; 32], message: &[u8], payer: &str, gas_limit: u64, max_fee: u128) -> Value {
    let (tp, b) = (lez::b58(token_program), lez::b58(bridge));
    json!({
        "program": tp,
        "message": to_hex(message),
        "accounts": [
            { "account": lez::b58(definition), "program": tp },
            { "seed": to_hex(&lez::CONFIG_SEED), "of": b, "program": b },
        ],
        "fee": { "payer": format!("Public/{}", payer.trim_start_matches("Public/")), "gas_limit": gas_limit, "tip": "0", "max_fee": max_fee.to_string() },
        "privacy": "bridge_public_burn",
    })
}

/// The 256-byte journal `release` checks: `abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)`.
#[allow(clippy::too_many_arguments)]
pub fn release_journal(cfg: &[u8; 32], root: &[u8; 32], nf: &[u8; 32], token: &[u8; 20], denom: u128, recipient: &[u8; 20], relayer: &[u8; 20], fee: u128) -> Vec<u8> {
    [*cfg, *root, *nf, eth::word_address(token), wallet::u256(denom), eth::word_address(recipient), eth::word_address(relayer), wallet::u256(fee)].concat()
}

/// `verify(bytes seal, bytes32 imageId, bytes32 journalDigest)` call data.
pub fn verify_calldata(seal: &[u8], image: &[u8; 32], digest: &[u8; 32]) -> Vec<u8> {
    let mut d = util::selector("verify(bytes,bytes32,bytes32)").to_vec();
    d.extend(eth::word_u64(0x60));
    d.extend(image);
    d.extend(digest);
    d.extend(eth::word_u64(seal.len() as u64));
    d.extend(seal);
    d.resize(d.len() + (32 - seal.len() % 32) % 32, 0);
    d
}

/// One `StateUpdated(bytes32 prevState, bytes32 newState, bytes32 exitRoot, uint64 firstExitIndex, bytes32[] exitLeaves)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub exit_root: [u8; 32],
    pub first: u64,
    pub leaves: Vec<[u8; 32]>,
    pub block: u64,
    pub tx: String,
}

pub fn update_topic() -> [u8; 32] {
    util::keccak(b"StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])")
}

pub fn released_topic() -> [u8; 32] {
    util::keccak(b"Released(bytes32,address,uint32,address,address,uint256)")
}

pub fn parse_update(log: &Value) -> Result<Update, String> {
    let data = util::hex_bytes(log["data"].as_str().unwrap_or_default())?;
    let w = Words(&data);
    let off = w.u64(4)? as usize;
    if off % 32 != 0 {
        return Err("a StateUpdated log with a misaligned array".into());
    }
    let n = w.u64(off / 32)? as usize;
    if n > 4096 {
        return Err("a StateUpdated log with too many leaves".into());
    }
    let leaves = (0..n).map(|k| w.word(off / 32 + 1 + k)).collect::<Result<Vec<_>, _>>()?;
    let block = log["blockNumber"].as_str().and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()).ok_or("a log without a block number")?;
    Ok(Update { exit_root: w.word(2)?, first: w.u64(3)?, leaves, block, tx: log["transactionHash"].as_str().unwrap_or_default().to_owned() })
}

/// The root of the first `n` leaves of the exit tree.
pub fn root_of(leaves: &[[u8; 32]]) -> [u8; 32] {
    match leaves.len() {
        0 => crate::relayed::zero_hashes()[32],
        n => crate::relayed::merkle_root(&leaves[n - 1], (n - 1) as u64, &crate::relayed::merkle_path(leaves, n - 1)),
    }
}

/// Every exit leaf in order, from the updates, each batch's root checked; refused if the list is
/// short of `count` or doesn't give `root` (AGENTS.md: never a short set).
pub fn exit_tree(updates: &[Update], count: u64, root: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
    let mut all: Vec<[u8; 32]> = Vec::new();
    let mut ups = updates.to_vec();
    ups.sort_by_key(|u| (u.first, u.block));
    for u in &ups {
        if u.leaves.is_empty() {
            continue;
        }
        if u.first != all.len() as u64 {
            return Err(format!("the vault's update logs skip exit leaves (an update starts at {} after {}); choose another Ethereum RPC in Settings", u.first, all.len()));
        }
        all.extend(&u.leaves);
        if root_of(&all) != u.exit_root {
            return Err("the exit tree as rebuilt here does not give an update's own root; nothing was changed".into());
        }
    }
    if (all.len() as u64) != count || root_of(&all) != *root {
        return Err(format!("the Ethereum RPC returned {} of the vault's {count} exit leaves; choose another in Settings", all.len()));
    }
    Ok(all)
}

fn stage_text(stage: &str) -> &'static str {
    match stage {
        "planned" => "Ready to send back: approve the burn in your Names Wallet.",
        "approve" => "Waiting for you in the Names app.",
        "burning" => "Sent; waiting for its block on the bridge test network (about 15 s).",
        "burned" => "Burned. Waiting for Ethereum to record it: the bridge operator posts updates by hand today.",
        "recorded" => "Recorded on Ethereum. Make the release proof (about 20 s on this computer).",
        "proving" => "Making the release proof on this computer (about 20 s).",
        "proved" => "Proof made. Waiting for its Ethereum wrapper (about 3 min, by the bridge operator today).",
        "wrapped" => "Proof ready. Waiting for the vault's one-hour delay.",
        "releasable" => "Ready: copy the release code into the bridge page and press Release.",
        "released" => "Released on Ethereum.",
        _ => "",
    }
}

impl Bridge {
    fn sb_dir(&self, id: &str) -> PathBuf {
        self.data.join("send-back").join(id)
    }

    fn sb_all(&self) -> Vec<Value> {
        self.store().load()["send_backs"].as_array().cloned().unwrap_or_default()
    }

    fn sb_get(&self, id: &str) -> Result<Value, String> {
        self.sb_all().into_iter().find(|s| s["id"] == id).ok_or_else(|| format!("there is no send-back {id} on this computer"))
    }

    fn sb_put(&self, rec: &Value) -> Result<(), String> {
        let s = self.store();
        let mut all = s.load();
        if !all["send_backs"].is_array() {
            all["send_backs"] = json!([]);
        }
        let list = all["send_backs"].as_array_mut().unwrap();
        match list.iter_mut().find(|x| x["id"] == rec["id"]) {
            Some(x) => *x = rec.clone(),
            None => list.push(rec.clone()),
        }
        s.save(&all)
    }

    fn sb_ready(&self) -> Result<(), String> {
        let d = self.deployment();
        if d.release != "proof" {
            return Err(format!(
                "{} can't pay out on Ethereum, so a burn here would destroy the tokens: sending back is off for this bridge. {}",
                d.label, d.release_note
            ));
        }
        Ok(())
    }

    /// `send_back_link()`: the page link the wallet copies, and whether this bridge can send back.
    pub(crate) fn send_back_link_now(&self) -> Value {
        let d = self.deployment();
        let ready = self.sb_ready();
        json!({ "link": PAGE_LINK, "deployment": d.name, "vault": util::checksum_address(&d.vault), "chain_id": d.chain_id,
                "available": ready.is_ok(), "why": ready.err() })
    }

    fn sb_decode(&self, code: &str) -> Result<WithdrawTo, Value> {
        let d = self.deployment();
        let w = withdraw::decode_withdraw_to(&withdraw::from_input(code)).map_err(|e| json!({ "code": format!("bad_code:{}", e.code), "message": e.message }))?;
        if w.chain_id != d.chain_id {
            return Err(json!({ "code": "other_chain", "message": format!("This code is for chain {}, not {} (this bridge's).", w.chain_id, d.chain_id) }));
        }
        if w.vault != d.vault {
            return Err(json!({ "code": "other_vault", "message": "This code is for another bridge vault: open the bridge page from this wallet's link again." }));
        }
        Ok(w)
    }

    /// `send_back_check(code)`: what a withdraw-to code says, with the four words. No wallet call.
    pub(crate) fn send_back_check_now(&self, code: &str) -> Value {
        let w = match self.sb_decode(code) {
            Ok(w) => w,
            Err(e) => return json!({ "ok": false, "error": e }),
        };
        let mut warnings = Vec::new();
        if let Ok(eth) = self.eth() {
            if eth.code_len(&w.recipient).is_ok_and(|n| n > 0) {
                warnings.push("This address is a contract. It must be able to receive the token: the recipient is fixed when you send back.");
            }
        }
        let addr = util::checksum_address(&w.recipient);
        json!({ "ok": true, "recipient": addr, "recipient_short": format!("{}…{}", &addr[..6], &addr[addr.len() - 4..]),
                "words": crate::compare::words(&w.digest()), "words_text": crate::compare::words_text(&w.digest()),
                "fee_choice": w.fee_choice, "chain_id": w.chain_id, "vault": util::checksum_address(&w.vault), "warnings": warnings })
    }

    /// The vault's exit tree and its updates (logs since the vault's creation, checked complete).
    fn sb_updates(&self) -> Result<(Vec<Update>, Vec<[u8; 32]>), String> {
        let d = self.deployment().clone();
        let eth = self.eth()?;
        let latest = eth.block("latest")?.number;
        let count = Words(&eth.call(&d.vault, &eth::calldata("exitCount()", &[]))?).u64(0)?;
        let root = Words(&eth.call(&d.vault, &eth::calldata("exitRoot()", &[]))?).word(0)?;
        let mut ups = Vec::new();
        let topic = update_topic();
        let mut start = d.vault_create_block;
        let mut span: u64 = 45_000;
        while start <= latest {
            let end = (start + span - 1).min(latest);
            match eth.logs(&d.vault, &topic, start, end) {
                Ok(logs) => {
                    for l in logs {
                        ups.push(parse_update(&l)?);
                    }
                    start = end + 1;
                }
                Err(e) if span > 500 && !e.contains("429") => span = (span / 2).max(500),
                Err(e) => return Err(e),
            }
        }
        let leaves = exit_tree(&ups, count, &root)?;
        Ok((ups, leaves))
    }

    /// `send_back_plan(token, denom_index, account, code)`.
    pub(crate) fn send_back_plan_now(&mut self, token: &str, denom_index: &str, account: &str, code: &str) -> Result<Value, String> {
        self.sb_ready()?;
        let w = match self.sb_decode(code) {
            Ok(w) => w,
            Err(e) => return Ok(json!({ "ok": false, "error": e })),
        };
        if !account.trim().starts_with("Public/") {
            return Err("sending back works from a public account today (Public/<id>); private accounts come later".into());
        }
        let (t, row) = self.token_info(token)?;
        let i: usize = denom_index.trim().parse().map_err(|_| "the denomination index must be a number")?;
        let drow = row["denoms"].get(i).cloned().ok_or_else(|| format!("{} has no denomination {i}", t.symbol))?;
        let denom = units(drow["value"].as_str().unwrap_or(""))?;
        let menu: Vec<u128> = drow["relay_fee_menu"].as_array().map(|m| m.iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect()).unwrap_or_default();
        let fee = match w.fee_choice {
            0 => 0,
            n => *menu.get(n as usize - 1).ok_or_else(|| format!("the vault's relay-fee menu for this amount has {} choice(s); this code asks for choice {n}", menu.len()))?,
        };
        let withdraw_fee = units(drow["withdraw_fee"].as_str().unwrap_or("0")).unwrap_or(0);
        let (acct, written, _) = match self.receive_account(account)? {
            Ok(x) => x,
            Err(needs) => return Ok(needs),
        };
        let d = self.deployment().clone();
        let cfg = hex32(&d.cfg_id)?;
        let definition = util::lez_account(&t.definition)?;
        let rpc = self.lez_rpc().map_err(|r| r.message)?;
        let bridge = util::lez_account(&d.bridge_program)?;
        let show = lez::show_json(rpc.as_ref(), &bridge)?;
        let entry = show["tokens"].as_array().and_then(|l| l.iter().find(|x| x["definition"] == t.definition.as_str()).cloned()).ok_or("the bridge doesn't list this token")?;
        if show["burn_frozen"] == true || entry["burn_frozen"] == true {
            return Err("sending back is paused on this bridge (its burns are frozen); nothing was done".into());
        }
        let lez_from = rpc.call("getLastBlockId", json!([])).map_err(|p| p.message)?.as_u64().ok_or("the bridge test network gave no block height")?;
        let held = lez::token_balance(lez::account(rpc.as_ref(), &acct).map_err(|p| p.message)?.as_ref(), &d.token_program, &definition)?;
        if held < denom {
            return Err(format!("{written} holds {} {}, less than {}", format_amount(held, t.decimals), t.symbol, format_amount(denom, t.decimals)));
        }
        // The index: next unused for (account, context), kept before anything is built and never reused.
        let key = format!("sendback/{written}/{}/{denom}/{}/{fee}", eth::addr_hex(&t.eth_token), eth::addr_hex(&w.recipient));
        let mut st = self.wstate();
        let ctx = wallet::exit_context(&cfg, &t.eth_token, denom, &w.recipient, fee);
        let index = match st["exit_next"][&key].as_u64() {
            Some(n) => n,
            None => {
                // No record here (a new install or a restore): find the used indexes from Ethereum,
                // and refuse while LEZ counts burns of this amount that Ethereum hasn't recorded.
                let lez_burns = entry["burns"].get(i).and_then(Value::as_u64).unwrap_or(0);
                let eth = self.eth()?;
                let c = eth.call(&d.vault, &eth::calldata("counters(address,uint256)", &[eth::word_address(&t.eth_token), eth::word_u64(i as u64)]))?;
                let eth_burns = Words(&c).u64(2)?;
                if lez_burns > eth_burns {
                    return Err("This wallet has no record of earlier send-backs to this address, and the bridge test network has burns Ethereum hasn't recorded yet. Wait for the next update on Ethereum (the operator posts it), then try again: that way an exit is never made twice.".into());
                }
                let values = match wallet::app_secret(&written, wallet::SCOPE_EXIT, &ctx, wallet::TAG_EXIT, 0, wallet::BATCH)? {
                    Values::Pending(req) => return Self::secret_needs_exit(&req),
                    Values::Granted(v) => v,
                };
                let (_, leaves) = self.sb_updates()?;
                let used = values.iter().rposition(|k| leaves.contains(&exit_leaf(&cfg, &t.eth_token, denom, &wallet::exit_commitment(k, &w.recipient, &t.eth_token, denom, fee, &cfg))));
                used.map_or(0, |u| u as u64 + 1)
            }
        };
        let k = match wallet::app_secret(&written, wallet::SCOPE_EXIT, &ctx, wallet::TAG_EXIT, index, 1)? {
            Values::Pending(req) => return Self::secret_needs_exit(&req),
            Values::Granted(v) => v[0],
        };
        let e = wallet::exit_commitment(&k, &w.recipient, &t.eth_token, denom, fee, &cfg);
        let nf = wallet::release_nullifier(&k);
        st["exit_next"][&key] = json!(index + 1);
        self.put_wstate(&st)?;
        let id = format!("sb-{}", &to_hex(&e)[..16]);
        let addr = util::checksum_address(&w.recipient);
        let rec = json!({
            "id": id, "created": now_ms(), "deployment": d.name, "account": written, "symbol": t.symbol, "decimals": t.decimals,
            "eth_token": eth::addr_hex(&t.eth_token), "definition": t.definition, "denom_index": i, "denom": denom.to_string(),
            "recipient": addr, "fee_choice": w.fee_choice, "max_relay_fee": fee.to_string(), "withdraw_fee": withdraw_fee.to_string(),
            "key_index": index, "index_key": key, "exit_commitment": hex0x(&e), "release_nullifier": hex0x(&nf),
            "exit_leaf": hex0x(&exit_leaf(&cfg, &t.eth_token, denom, &e)), "balance_at_plan": held.to_string(), "scanned_to": lez_from,
            "words_text": crate::compare::words_text(&w.digest()), "stage": "planned",
        });
        self.sb_put(&rec)?;
        let mut v = self.sb_view(&rec);
        v["burn"] = self.burn_signable();
        v["settle_offer"] = self.settle_offer();
        Ok(v)
    }

    /// The settlement service as this send-back would use it: `{available, reason, consent_needed,
    /// consent_text}` (consent is needed only without a local prover: the secret would leave).
    fn settle_offer(&self) -> Value {
        let info = self.settle_info_raw();
        let local = !self.cfg.release_prover.is_empty();
        match info {
            Ok(v) => json!({ "available": v["available"] == true, "reason": v["reason"], "consent_needed": !local, "consent_text": if local { Value::Null } else { json!(SETTLE_CONSENT) },
                             "prove_first": local }),
            Err(e) => json!({ "available": false, "reason": e, "consent_needed": !local, "consent_text": if local { Value::Null } else { json!(SETTLE_CONSENT) } }),
        }
    }

    /// `bridge_settleInfo`, read at most once a minute per module data folder.
    fn settle_info_raw(&self) -> Result<Value, String> {
        static CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, Value)>>> = std::sync::Mutex::new(None);
        let key = format!("{}|{}", self.data.display(), self.deployment().name);
        let now = now_ms();
        if let Some((at, v)) = CACHE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&key).cloned())) {
            if now.saturating_sub(at) < 60_000 {
                return Ok(v);
            }
        }
        let v = self.lez_rpc().map_err(|r| r.message)?.call("bridge_settleInfo", json!([])).map_err(|p| p.message)?;
        if let Ok(mut g) = CACHE.lock() {
            g.get_or_insert_with(Default::default).insert(key, (now, v.clone()));
        }
        Ok(v)
    }

    /// `send_back_settle(id, options)`: hands the send-back to the bridge's settlement service, which
    /// proves (when needed), wraps and releases it; `send_back_status` then follows it. With a local
    /// receipt only the receipt goes (no secret). Without one the exit secret goes, and only with
    /// `{"consent": true}` after the person read [`SETTLE_CONSENT`] (else `needs: "consent"`).
    pub(crate) fn send_back_settle_now(&mut self, id: &str, options: &Value) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        if r["settle_id"].is_string() && r["settle_failed"] != true {
            return self.send_back_status_now(id);
        }
        if r["root"].is_null() {
            return Err("Ethereum hasn't recorded this burn yet: the bridge's update records it first.".into());
        }
        let info = self.settle_info_raw()?;
        if info["available"] != true {
            return Err(info["reason"].as_str().unwrap_or("The bridge's settlement service is not available now.").to_owned());
        }
        let d = self.deployment().clone();
        let vault = eth::addr_hex(&d.vault);
        let receipt = self.sb_receipt(&r);
        let body = if let Some(p) = receipt["path"].as_str() {
            let rb = std::fs::read(p).map_err(|e| format!("cannot read the receipt: {e}"))?;
            let jh = std::fs::read_to_string(receipt["journal"].as_str().unwrap_or_default()).map_err(|e| format!("cannot read the journal: {e}"))?;
            json!({ "vault": vault, "receipt_hex": to_hex(&rb), "journal_hex": jh.trim().trim_start_matches("0x") })
        } else {
            if !self.cfg.release_prover.is_empty() {
                // A wallet with the helper sends only its receipt: the secret never leaves.
                return Err("This computer has the proof helper: make the proof here first (\"Make the proof\"), then finish on Ethereum.".into());
            }
            if options["consent"] != true {
                return Ok(json!({ "needs": "consent", "request": null, "message": SETTLE_CONSENT }));
            }
            let (k, inputs, leaves) = match self.sb_witness(&r)? {
                Ok(x) => x,
                Err(needs) => return Ok(needs),
            };
            json!({ "vault": vault, "exit": {
                "secret": hex0x(&k), "eth_dest": inputs["eth_dest"], "token": inputs["token"], "denom": inputs["denom"],
                "max_relay_fee": inputs["max_relay_fee"], "cfg_id": inputs["cfg_id"], "index": inputs["index"],
                "leaves": leaves.iter().map(|l| hex0x(l)).collect::<Vec<_>>() } })
        };
        let v = self.lez_rpc().map_err(|e| e.message)?.call("bridge_settle", json!([body])).map_err(|p| p.message)?;
        if v["ok"] != true {
            return Err(format!("{} Nothing was handed over.", v["reason"].as_str().unwrap_or("The settlement service refused it.")));
        }
        r["settle_id"] = v["id"].clone();
        r["settle_failed"] = Value::Null;
        r["settle"] = json!({ "stage": v["stage"], "text": null });
        r["settle_mode"] = json!(if receipt.is_null() { "exit" } else { "receipt" });
        self.sb_put(&r)?;
        self.send_back_status_now(id)
    }

    /// Whether a burn can be signed and paid for now: the Names Wallet lists `relayed_bridge_burn`
    /// in its public config's features, and the bridge's relay says it serves burns.
    fn burn_signable(&self) -> Value {
        let names = wallet::names_config().map(|c| c["features"].as_array().is_some_and(|f| f.iter().any(|x| x == "relayed_bridge_burn")));
        let relay = self.relay_info_raw().map(|v| v["burns"] == true);
        let why = match (&names, &relay) {
            (Ok(true), Ok(true)) => Value::Null,
            (Ok(false), _) => json!("Your Names Wallet can't sign a burn yet: update the Names app to a version that can send back to Ethereum."),
            (Err(e), _) => json!(format!("Your Names Wallet can't be asked now ({e}).")),
            (_, Ok(false)) => json!("The bridge's relay doesn't pay for burns yet; try again later."),
            (_, Err(e)) => json!(format!("The bridge's relay can't be reached now ({e}).")),
        };
        json!({ "can_sign": why.is_null(), "why": why })
    }

    /// The relay's `bridge_relayInfo`, read at most once a minute per module data folder (the
    /// endpoint counts it in the relay's own small bucket).
    fn relay_info_raw(&self) -> Result<Value, String> {
        static CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, Value)>>> = std::sync::Mutex::new(None);
        let key = format!("{}|{}", self.data.display(), self.deployment().name);
        let now = now_ms();
        if let Some((at, v)) = CACHE.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&key).cloned())) {
            if now.saturating_sub(at) < 60_000 {
                return Ok(v);
            }
        }
        let v = self.lez_rpc().map_err(|r| r.message)?.call("bridge_relayInfo", json!([])).map_err(|p| p.message)?;
        if let Ok(mut g) = CACHE.lock() {
            g.get_or_insert_with(Default::default).insert(key, (now, v.clone()));
        }
        Ok(v)
    }

    fn secret_needs_exit(req: &Value) -> Result<Value, String> {
        wallet::needs("secret", req, "Approve \"RFP022/exit\" in the Names app: your wallet makes this app's exit secrets from your account, so a send-back can be released after you restore the wallet.")
    }

    /// What every reply shows of a send-back (public facts only).
    fn sb_view(&self, r: &Value) -> Value {
        let dec = r["decimals"].as_u64().unwrap_or(18) as u8;
        let n = |k: &str| units(r[k].as_str().unwrap_or("0")).unwrap_or(0);
        let receive = n("denom").saturating_sub(n("withdraw_fee")).saturating_sub(n("max_relay_fee"));
        let sym = r["symbol"].as_str().unwrap_or("");
        let stage = r["stage"].as_str().unwrap_or("planned");
        json!({
            "id": r["id"], "stage": stage, "text": stage_text(stage), "account": r["account"], "symbol": sym,
            "amount_text": format!("{} {sym}", format_amount(n("denom"), dec)), "receive_text": format!("{} {sym}", format_amount(receive, dec)),
            "relay_fee_text": format!("{} {sym}", format_amount(n("max_relay_fee"), dec)), "recipient": r["recipient"], "words_text": r["words_text"],
            "exit_commitment": r["exit_commitment"], "burn_tx": r["burn_tx"], "burn_block": r["burn_block"], "leaf_index": r["leaf_index"],
            "root": r["root"], "releasable_at": r["releasable_at"], "seal": r["seal"].is_string(), "released_tx": r["released_tx"],
            "fee_payer": "The bridge's relay pays the LEZ fee: your account needs no LEZ.", "privacy": PRIVACY_LINE, "created": r["created"],
            "prover_set": !self.cfg.release_prover.is_empty(), "prove_error": r["prove_error"], "receipt": self.sb_receipt(r),
            "settle_id": r["settle_id"], "settle": r["settle"], "settle_note": r["settle_note"],
        })
    }

    /// The release receipt this computer holds for a send-back (the wrap's input; no secret in it),
    /// with the release digest, or null.
    fn sb_receipt(&self, r: &Value) -> Value {
        let dir = self.sb_dir(r["id"].as_str().unwrap_or_default());
        for d in [dir.clone(), dir.join("run/prove")] {
            if d.join("receipt.bin").exists() {
                let digest = std::fs::read_to_string(d.join("journal.sha256")).ok().map(|s| s.trim().to_owned());
                return json!({ "path": d.join("receipt.bin").display().to_string(), "journal": d.join("journal.hex").display().to_string(), "digest": digest });
            }
        }
        Value::Null
    }

    /// `send_back_build(id)`: the relayed call the Names Wallet signs (pure apart from the relay's caps).
    pub(crate) fn send_back_build_now(&self, id: &str) -> Result<Value, String> {
        self.sb_ready()?;
        let r = self.sb_get(id)?;
        let d = self.deployment();
        if r["deployment"] != d.name.as_str() {
            return Err("the bridge deployment changed since this send-back was planned".into());
        }
        let rpc = self.lez_rpc().map_err(|e| e.message)?;
        let info = lez::parse_relay_info(&self.relay_info_raw()?).map_err(|p| p.message)?;
        let (bridge, tp, def) = (util::lez_account(&d.bridge_program)?, util::lez_account(&d.token_program)?, util::lez_account(r["definition"].as_str().unwrap_or_default())?);
        let e = hex32(r["exit_commitment"].as_str().unwrap_or_default())?;
        let message = burn_message(&def, units(r["denom"].as_str().unwrap_or("0"))?, &bridge, &e);
        // The relay's own caps, as for a claim: the gas limit is its max_gas, and the reserve at
        // today's base fees must fit under its max fee.
        let (exec, stor) = crate::relayed::fee_state(rpc.as_ref()).map_err(|p| p.message)?;
        let reserve = u128::from(info.max_gas) * exec + u128::from(message.len() as u64 + WIRE_OVERHEAD) * stor;
        if reserve > info.max_fee {
            return Err(format!("The network's fees are above what the relay pays per burn right now (about {reserve} needed, at most {}); try again later.", info.max_fee));
        }
        let call = burn_call(&tp, &def, &bridge, &message, &info.payer, info.max_gas, info.max_fee);
        let v = self.sb_view(&r);
        let summary = format!("Send back {} to {} on Ethereum (exit {}…). The bridge's relay sends it and pays the fee.", v["amount_text"].as_str().unwrap_or(""), r["recipient"].as_str().unwrap_or(""), &r["exit_commitment"].as_str().unwrap_or("")[..10]);
        Ok(json!({ "id": id, "account": r["account"], "call": call, "summary": summary, "network": d.network, "deployment": d.name,
                   "message_len": message.len(), "wire_estimate": message.len() as u64 + WIRE_OVERHEAD, "privacy": PRIVACY_LINE }))
    }

    /// `send_back_burn(id)`: proposes the burn to the Names Wallet (`propose_relayed_call`).
    pub(crate) fn send_back_burn_now(&mut self, id: &str, options: &Value) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        if r["burn_tx"].is_string() || r["request"].is_string() {
            return self.send_back_status_now(id);
        }
        let can = self.burn_signable();
        if can["can_sign"] != true {
            return Ok(json!({ "needs": "names_update", "request": null, "message": can["why"] }));
        }
        // Never a second burn of this exit: the chain is read up to its tip first.
        if let Some(b) = self.sb_seen(&mut r)? {
            self.sb_put(&r)?;
            return Err(format!("This exit was burned already (block {b}): nothing was proposed."));
        }
        let built = self.send_back_build_now(id)?;
        let account = r["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
        let d = self.deployment().clone();
        if let Some(n) = self.relayed_gates(&account, &d.bridge_program, &d.token_program, options)? {
            return Ok(n);
        }
        match wallet::names_try("propose_relayed_call", json!([r["account"], built["call"].to_string(), built["summary"]])) {
            Ok(req) => {
                r["request"] = req["request"].clone();
                r["stage"] = json!("approve");
                self.sb_put(&r)?;
                self.send_back_status_now(id)
            }
            // Names' own words for a Config it couldn't read: a wait, not a refusal.
            Err(wallet::Fail::Refused { error, .. }) if error.contains("couldn't read the Config") => {
                Err(format!("{error}. Nothing was signed or sent; try again in a moment."))
            }
            Err(wallet::Fail::Refused { error, .. }) => Err(format!("Your Names Wallet refused the burn: {error}. Nothing was signed or sent.")),
            Err(wallet::Fail::NoAnswer) => {
                r["request"] = json!("unanswered");
                r["stage"] = json!("approve");
                self.sb_put(&r)?;
                Err("Your Names Wallet didn't answer in time, so the burn may be waiting in the Names app: approve or reject it there. This app won't propose it again (that could burn twice).".into())
            }
            Err(f) => Err(f.text()),
        }
    }

    /// `send_back_submit(id, tx)`: hands the burn the Names app signed to the relay.
    pub(crate) fn send_back_submit_now(&mut self, id: &str, transaction: &str) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        if r["burn_tx"].is_string() {
            return self.send_back_status_now(id);
        }
        let tx = transaction.trim().trim_start_matches("0x");
        if tx.is_empty() || tx.len() % 2 != 0 || !tx.bytes().all(|b| b.is_ascii_hexdigit()) || tx.len() > 2 * 16 * 1024 {
            return Err("give the signed transaction as hex, as the Names Wallet returned it".into());
        }
        if let Some(b) = self.sb_seen(&mut r)? {
            self.sb_put(&r)?;
            return Err(format!("This exit was burned already (block {b}): nothing was sent."));
        }
        let rpc = self.lez_rpc().map_err(|e| e.message)?;
        let v = rpc.call("bridge_relayBurn", json!([tx, false])).map_err(|p| p.message)?;
        if v["ok"] != true {
            return Err(format!("The bridge's relay refused the burn: {}. Nothing was sent.", v["reason"].as_str().unwrap_or("no reason given")));
        }
        let h = v["tx_hash"].as_str().filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).ok_or("the relay said ok but gave no transaction hash")?;
        r["burn_tx"] = json!(h);
        r["stage"] = json!("burning");
        self.sb_put(&r)?;
        self.send_back_status_now(id)
    }

    /// Reads the LEZ blocks not read yet for this exit's `E`, up to the tip (at most
    /// [`SCAN_MAX`] blocks; past that it refuses rather than guess). The block it is in, if any.
    fn sb_seen(&self, r: &mut Value) -> Result<Option<u64>, String> {
        let rpc = self.lez_rpc().map_err(|e| e.message)?;
        let tip = rpc.call("getLastBlockId", json!([])).map_err(|p| p.message)?.as_u64().unwrap_or(0);
        let from = r["scanned_to"].as_u64().unwrap_or(tip) + 1;
        if tip + 1 > from + SCAN_MAX {
            return Err("This app hasn't read the bridge test network's newest blocks for this send-back yet; open it again in a minute, then send.".into());
        }
        let e = hex32(r["exit_commitment"].as_str().unwrap_or_default())?;
        let (found, last, problem) = lez::scan_blocks(rpc.as_ref(), &e, from, tip);
        r["scanned_to"] = json!(last);
        if let Some(b) = found {
            r["burn_block"] = json!(b);
            r["stage"] = json!("burned");
            return Ok(Some(b));
        }
        match problem {
            Some(p) => Err(p.message),
            None => Ok(None),
        }
    }

    pub(crate) fn send_backs_now(&self) -> Value {
        let mut l: Vec<Value> = self.sb_all().iter().map(|r| self.sb_view(r)).collect();
        l.reverse();
        json!({ "send_backs": l })
    }

    /// `send_back_status(id)`: moves the record on as far as the chains say. For a 60 s poll.
    pub(crate) fn send_back_status_now(&mut self, id: &str) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        let before = r.clone();
        let mut reads = json!({ "lez": 0, "sepolia": 0 });
        let res = self.sb_advance(&mut r, &mut reads);
        if r != before {
            self.sb_put(&r)?;
        }
        let mut v = self.sb_view(&r);
        v["reads"] = reads;
        v["wait"] = self.sb_wait(&r);
        // The settlement service's offer, once the burn is recorded and no settlement runs.
        if matches!(r["stage"].as_str(), Some("recorded" | "proved" | "wrapped" | "releasable")) && (r["settle_id"].is_null() || r["settle_failed"] == true) {
            v["settle_offer"] = self.settle_offer();
        }
        if let Err(e) = res {
            // A busy endpoint or a failed read is a wait, never the end of the send-back: the next
            // poll goes on from where this one stopped.
            v["busy"] = json!(e == lez::BUSY_TEXT);
            v["note"] = json!(e);
        }
        Ok(v)
    }

    fn sb_wait(&self, r: &Value) -> Value {
        let now = now_ms() / 1000;
        match r["stage"].as_str().unwrap_or("") {
            "burning" => json!({ "what": "its block on the bridge test network", "seconds": 15 }),
            "burned" => json!({ "what": "the next update on Ethereum (the bridge operator, by hand today)", "seconds": null }),
            "recorded" | "proving" | "proved" | "wrapped" => {
                let at = r["releasable_at"].as_u64().unwrap_or(0);
                json!({ "what": "the vault's one-hour delay", "until": at, "seconds": at.saturating_sub(now) })
            }
            _ => Value::Null,
        }
    }

    fn sb_advance(&mut self, r: &mut Value, reads: &mut Value) -> Result<(), String> {
        let bump = |reads: &mut Value, k: &str, n: u64| reads[k] = json!(reads[k].as_u64().unwrap_or(0) + n);
        // Part A: the wallet's request, then the relay.
        if r["stage"] == "approve" {
            let Some(rid) = r["request"].as_str().filter(|x| *x != "unanswered").map(str::to_owned) else { return Ok(()) };
            let s = wallet::names_once("request_status", json!([rid])).map_err(|f| f.text())?;
            match s["state"].as_str() {
                Some("done") => {
                    let tx = s["transaction"].as_str().ok_or("the Names Wallet signed but gave no transaction")?.to_owned();
                    let id = r["id"].as_str().unwrap_or_default().to_owned();
                    self.sb_put(r)?;
                    self.send_back_submit_now(&id, &tx)?;
                    *r = self.sb_get(&id)?;
                }
                Some("rejected") | Some("expired") | Some("cancelled") => {
                    r["request"] = Value::Null;
                    r["stage"] = json!("planned");
                }
                _ => return Ok(()),
            }
        }
        // The settlement service, when it has this send-back: its stage, its seal, its release.
        if let Some(sid) = r["settle_id"].as_str().map(str::to_owned) {
            if r["stage"] != "released" && r["settle_failed"] != true {
                bump(reads, "lez", 1);
                // Never fatal: a stopped or rolled-back service must not stop the chain reads below
                // (the release seen on Sepolia, the manual fallbacks).
                let v = self.lez_rpc().map_err(|e| e.message).and_then(|rpc| rpc.call("bridge_settleStatus", json!([sid])).map_err(|p| p.message));
                let v = match v {
                    Ok(v) => v,
                    Err(e) => {
                        r["settle_note"] = json!(format!("The settlement service can't be asked now ({e})."));
                        json!({ "ok": null })
                    }
                };
                if v["ok"] == false && v["code"] == "unknown" {
                    // The service lost it (a reset): it can be handed over again.
                    r["settle_failed"] = json!(true);
                    r["settle"] = json!({ "stage": "failed", "error": "The settlement service no longer has this send-back; finish it again." });
                }
                if v["ok"] == true {
                    r["settle_note"] = Value::Null;
                    r["settle"] = json!({ "stage": v["stage"], "text": v["text"], "releasable_at": v["releasable_at"], "run": v["run"] });
                    if let Some(seal) = v["seal"].as_str() {
                        r["seal"] = json!(seal);
                    }
                    match v["stage"].as_str() {
                        Some("released") => {
                            r["released_tx"] = v["tx"].clone();
                            r["stage"] = json!("released");
                            return Ok(());
                        }
                        Some("failed") => {
                            r["settle_failed"] = json!(true);
                            r["settle"]["error"] = v["error"].clone();
                        }
                        _ => {}
                    }
                }
            }
        }
        let d = self.deployment().clone();
        if matches!(r["stage"].as_str(), Some("planned") | Some("approve") | Some("burning")) {
            // Found by its exit commitment in the bridge test network's blocks, whoever sent it.
            let rpc = self.lez_rpc().map_err(|e| e.message)?;
            let e = hex32(r["exit_commitment"].as_str().unwrap_or_default())?;
            bump(reads, "lez", 1);
            let tip = rpc.call("getLastBlockId", json!([])).map_err(|p| p.message)?.as_u64().unwrap_or(0);
            let from = r["scanned_to"].as_u64().unwrap_or(tip) + 1;
            let to = tip.min(from + SCAN_PER_POLL - 1);
            if from <= to {
                bump(reads, "lez", (to - from) / lez::MAX_RANGE + 1);
                // Resumes where it stopped: `scanned_to` is kept even when a read fails.
                let (found, last, problem) = lez::scan_blocks(rpc.as_ref(), &e, from, to);
                r["scanned_to"] = json!(last);
                if let Some(b) = found {
                    r["burn_block"] = json!(b);
                    r["stage"] = json!("burned");
                } else if let Some(p) = problem {
                    return Err(p.message);
                }
            }
        }
        if r["stage"] == "burning" {
            let rpc = self.lez_rpc().map_err(|e| e.message)?;
            bump(reads, "lez", 1);
            if let Some(b) = lez::tx_block(rpc.as_ref(), r["burn_tx"].as_str().unwrap_or_default()).map_err(|p| p.message)? {
                // In a block. Whether it applied is settled by the vault's record of the exit.
                r["burn_block"] = json!(b);
                r["stage"] = json!("burned");
            }
        }
        if r["stage"] == "burned" {
            bump(reads, "sepolia", 4);
            let (ups, _) = self.sb_updates()?;
            let leaf = hex32(r["exit_leaf"].as_str().unwrap_or_default())?;
            if let Some(u) = ups.iter().find(|u| u.leaves.contains(&leaf)) {
                let pos = u.leaves.iter().position(|l| *l == leaf).unwrap() as u64;
                r["leaf_index"] = json!(u.first + pos);
                r["root"] = json!(hex0x(&u.exit_root));
                r["root_leaves"] = json!(u.first + u.leaves.len() as u64);
                r["update_tx"] = json!(u.tx);
                let eth = self.eth()?;
                let token = util::eth_address(r["eth_token"].as_str().unwrap_or_default())?;
                let at = Words(&eth.call(&d.vault, &eth::calldata("releasableAt(address,bytes32)", &[eth::word_address(&token), u.exit_root]))?).u64(0)?;
                r["releasable_at"] = json!(at);
                r["stage"] = json!("recorded");
            }
        }
        let dir = self.sb_dir(r["id"].as_str().unwrap_or_default());
        if matches!(r["stage"].as_str(), Some("recorded") | Some("proving")) {
            if dir.join("receipt.bin").exists() || dir.join("run/prove/receipt.bin").exists() {
                r["stage"] = json!("proved");
                // The proof is made: the exit secret's file (the stand-in's) is not needed any more.
                let _ = std::fs::remove_file(dir.join("k"));
            } else if let Some(j) = r["prove_job"].as_str() {
                let s = self.runner().state(j).unwrap_or(Value::Null);
                r["stage"] = json!(if s["state"] == "running" { "proving" } else { "recorded" });
                if s["state"] == "failed" {
                    r["prove_error"] = s["error"].clone();
                    r["prove_job"] = Value::Null;
                }
            }
        }
        if matches!(r["stage"].as_str(), Some("recorded") | Some("proving") | Some("proved") | Some("wrapped")) {
            // Read again until it has passed: a lowered delay brings it forward (never later).
            let now = now_ms() / 1000;
            if r["releasable_at"].as_u64().is_none_or(|a| now < a) {
                let eth = self.eth()?;
                let token = util::eth_address(r["eth_token"].as_str().unwrap_or_default())?;
                let root = hex32(r["root"].as_str().unwrap_or_default())?;
                bump(reads, "sepolia", 1);
                r["releasable_at"] = json!(Words(&eth.call(&d.vault, &eth::calldata("releasableAt(address,bytes32)", &[eth::word_address(&token), root]))?).u64(0)?);
            }
        }
        if matches!(r["stage"].as_str(), Some("proved") | Some("wrapped") | Some("releasable")) {
            let now = now_ms() / 1000;
            if r["seal"].is_string() {
                r["stage"] = json!(if r["releasable_at"].as_u64().is_some_and(|a| now >= a) { "releasable" } else { "wrapped" });
            }
        }
        if matches!(r["stage"].as_str(), Some("recorded") | Some("proving") | Some("proved") | Some("wrapped") | Some("releasable")) {
            let eth = self.eth()?;
            let nf = hex32(r["release_nullifier"].as_str().unwrap_or_default())?;
            bump(reads, "sepolia", 1);
            if Words(&eth.call(&d.vault, &eth::calldata("releaseNullifierSpent(bytes32)", &[nf]))?).boolean(0)? {
                r["stage"] = json!("released");
                let latest = eth.block("latest")?.number;
                let from = latest.saturating_sub(40_000).max(d.vault_create_block);
                if let Ok(logs) = eth.logs(&d.vault, &released_topic(), from, latest) {
                    if let Some(l) = logs.iter().find(|l| l["topics"][1].as_str().is_some_and(|t| t.eq_ignore_ascii_case(&hex0x(&nf)))) {
                        r["released_tx"] = l["transactionHash"].clone();
                    }
                }
            }
        }
        Ok(())
    }

    /// The release witness, for a prover: the exit secret (from the wallet, by index), the public
    /// inputs and the leaves up to the root that recorded the exit.
    fn sb_witness(&mut self, r: &Value) -> Result<Result<([u8; 32], Value, Vec<[u8; 32]>), Value>, String> {
        if r["root"].is_null() {
            return Err("Ethereum hasn't recorded this send-back yet: the release proof can be made once it has".into());
        }
        let d = self.deployment().clone();
        let cfg = hex32(&d.cfg_id)?;
        let token = util::eth_address(r["eth_token"].as_str().unwrap_or_default())?;
        let recipient = util::eth_address(r["recipient"].as_str().unwrap_or_default())?;
        let denom = units(r["denom"].as_str().unwrap_or("0"))?;
        let fee = units(r["max_relay_fee"].as_str().unwrap_or("0"))?;
        let ctx = wallet::exit_context(&cfg, &token, denom, &recipient, fee);
        let k = match wallet::app_secret(r["account"].as_str().unwrap_or_default(), wallet::SCOPE_EXIT, &ctx, wallet::TAG_EXIT, r["key_index"].as_u64().unwrap_or(0), 1)? {
            Values::Pending(req) => return Ok(Err(Self::secret_needs_exit(&req)?)),
            Values::Granted(v) => v[0],
        };
        if hex0x(&wallet::exit_commitment(&k, &recipient, &token, denom, fee, &cfg)) != r["exit_commitment"].as_str().unwrap_or_default() {
            return Err("the Names Wallet gives another exit secret than this send-back was made with (another account or wallet?)".into());
        }
        let (_, leaves) = self.sb_updates()?;
        let n = r["root_leaves"].as_u64().unwrap_or(0) as usize;
        let leaves = leaves.get(..n).ok_or("the vault's exit tree is shorter than when this send-back was recorded")?.to_vec();
        if hex0x(&root_of(&leaves)) != r["root"].as_str().unwrap_or_default() {
            return Err("the exit leaves read now don't give the recorded root".into());
        }
        // Self-release: no relay fee, no relayer (the person sends `release` from the page).
        let inputs = json!({
            "eth_dest": r["recipient"], "token": r["eth_token"], "denom": r["denom"], "denom_index": r["denom_index"], "max_relay_fee": r["max_relay_fee"],
            "cfg_id": d.cfg_id, "index": r["leaf_index"], "relayer": eth::addr_hex(&[0; 20]), "relay_fee": "0", "vault": eth::addr_hex(&d.vault), "root": r["root"],
            "release_nullifier": r["release_nullifier"], "leaves": "leaves.txt",
        });
        Ok(Ok((k, inputs, leaves)))
    }

    fn release_args(inputs: &Value, leaves: &std::path::Path, out: &std::path::Path, image: &str) -> Vec<String> {
        let s = |k: &str| inputs[k].as_str().map(str::to_owned).unwrap_or_else(|| inputs[k].to_string());
        vec![
            "release".into(), "--secret-file".into(), "/dev/stdin".into(), "--eth-dest".into(), s("eth_dest"), "--token".into(), s("token"), "--denom".into(), s("denom"),
            "--max-relay-fee".into(), s("max_relay_fee"), "--cfg-id".into(), s("cfg_id"), "--leaves".into(), leaves.display().to_string(), "--index".into(), s("index"),
            "--relayer".into(), s("relayer"), "--relay-fee".into(), s("relay_fee"), "--out".into(), out.display().to_string(), "--image-id".into(), image.to_owned(), "--prove".into(),
        ]
    }

    /// `send_back_witness(id)`: today's stand-in for the helper: the files a prover run reads.
    pub(crate) fn send_back_witness_now(&mut self, id: &str) -> Result<Value, String> {
        let r = self.sb_get(id)?;
        let (k, inputs, leaves) = match self.sb_witness(&r)? {
            Ok(x) => x,
            Err(needs) => return Ok(needs),
        };
        let dir = self.sb_dir(id);
        util::private_dir(&dir).map_err(|e| e.to_string())?;
        util::write_private(&dir.join("k"), hex0x(&k).as_bytes()).map_err(|e| e.to_string())?;
        util::write_private(&dir.join("leaves.txt"), leaves.iter().map(|l| hex0x(l) + "\n").collect::<String>().as_bytes()).map_err(|e| e.to_string())?;
        util::write_private(&dir.join("inputs.json"), serde_json::to_vec_pretty(&inputs).unwrap_or_default().as_slice()).map_err(|e| e.to_string())?;
        let s = |k: &str| inputs[k].as_str().map(str::to_owned).unwrap_or_else(|| inputs[k].to_string());
        let command = format!(
            "prove-release-rc3.sh --out {} --secret-file {} --eth-dest {} --token {} --denom {} --denom-index {} --max-relay-fee {} --leaves {} --index {} --relayer {} --relay-fee 0 --vault {}",
            dir.join("run").display(), dir.join("k").display(), s("eth_dest"), s("token"), s("denom"), s("denom_index"), s("max_relay_fee"),
            dir.join("leaves.txt").display(), s("index"), s("relayer"), s("vault")
        );
        Ok(json!({ "id": id, "dir": dir.display().to_string(), "secret_file": dir.join("k").display().to_string(), "inputs": dir.join("inputs.json").display().to_string(),
                   "leaves": dir.join("leaves.txt").display().to_string(), "receipt": dir.join("run/prove/receipt.bin").display().to_string(), "command": command,
                   "note": "The file k holds this exit's secret (mode 600). It can only release to the recipient fixed in the burn; it shows which burn pays which address. Delete it after the proof." }))
    }

    /// `send_back_prove(id)`: starts the release prover (the helper) when one is set.
    pub(crate) fn send_back_prove_now(&mut self, id: &str) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        if self.cfg.release_prover.is_empty() {
            return Ok(json!({ "needs": "prover", "request": null,
                "message": "This computer has no release prover set up yet. Until the helper ships with the bridge module, use \"Write the proof inputs\" and run the proof with the bridge operator's script." }));
        }
        let (k, inputs, leaves) = match self.sb_witness(&r)? {
            Ok(x) => x,
            Err(needs) => return Ok(needs),
        };
        let dir = self.sb_dir(id);
        util::private_dir(&dir).map_err(|e| e.to_string())?;
        util::write_private(&dir.join("leaves.txt"), leaves.iter().map(|l| hex0x(l) + "\n").collect::<String>().as_bytes()).map_err(|e| e.to_string())?;
        let mut argv = self.cfg.release_prover.clone();
        let image = {
            let eth = self.eth()?;
            Words(&eth.call(&self.deployment().vault, &eth::calldata("releaseImageId()", &[]))?).word(0)?
        };
        argv.extend(Self::release_args(&inputs, &dir.join("leaves.txt"), &dir, &hex0x(&image)));
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).env("RISC0_PROVER", "local").env("RAYON_NUM_THREADS", "4");
        let job = self.start_job(Spec {
            kind: "release_proof".into(), subject: id.to_owned(), steps: vec![cmd], dry_run: false, guarded: false,
            prove_estimate_s: PROOF_S, peak_estimate_mb: PROOF_PEAK_MB, meta: json!({ "send_back": id }), inputs: vec![Some(hex0x(&k).into_bytes())],
        })?;
        r["prove_job"] = job["id"].clone();
        r["stage"] = json!("proving");
        self.sb_put(&r)?;
        Ok(json!({ "id": id, "job": job, "stage": "proving", "text": stage_text("proving") }))
    }

    /// `send_back_seal(id, seal)`: attaches the Groth16 seal once Sepolia's verifier accepts it.
    pub(crate) fn send_back_seal_now(&mut self, id: &str, seal: &str) -> Result<Value, String> {
        let mut r = self.sb_get(id)?;
        if r["root"].is_null() {
            return Err("Ethereum hasn't recorded this send-back yet".into());
        }
        let seal = util::hex_bytes(seal.trim())?;
        if seal.len() != withdraw::SEAL_LEN || seal[..4] != withdraw::SELECTOR {
            return Err("that is not a 260-byte seal for RISC Zero's Groth16 verifier 3.1.0 (it starts 0xe71e6620)".into());
        }
        let d = self.deployment().clone();
        let journal = self.sb_journal(&r)?;
        let digest = sha256(&[&journal]);
        {
            let eth = self.eth()?;
            let gate = Words(&eth.call(&d.vault, &eth::calldata("verifier()", &[]))?).address(0)?;
            let image = Words(&eth.call(&d.vault, &eth::calldata("releaseImageId()", &[]))?).word(0)?;
            if let Err(e) = eth.call(&gate, &verify_calldata(&seal, &image, &digest)) {
                return Err(format!("Sepolia's verifier refused this proof for this send-back ({e}); nothing was kept"));
            }
        }
        r["seal"] = json!(hex0x(&seal));
        self.sb_put(&r)?;
        self.send_back_status_now(id)
    }

    fn sb_journal(&self, r: &Value) -> Result<Vec<u8>, String> {
        let d = self.deployment();
        Ok(release_journal(
            &hex32(&d.cfg_id)?,
            &hex32(r["root"].as_str().unwrap_or_default())?,
            &hex32(r["release_nullifier"].as_str().unwrap_or_default())?,
            &util::eth_address(r["eth_token"].as_str().unwrap_or_default())?,
            units(r["denom"].as_str().unwrap_or("0"))?,
            &util::eth_address(r["recipient"].as_str().unwrap_or_default())?,
            &[0; 20],
            0,
        ))
    }

    /// `send_back_release_code(id)`: the code the bridge page sends `release` with.
    pub(crate) fn send_back_release_code_now(&self, id: &str) -> Result<Value, String> {
        let r = self.sb_get(id)?;
        let seal = r["seal"].as_str().ok_or("there is no proof for this send-back yet")?;
        let d = self.deployment();
        let code = ReleaseCode {
            chain_id: d.chain_id,
            vault: d.vault,
            root: hex32(r["root"].as_str().unwrap_or_default())?,
            nullifier: hex32(r["release_nullifier"].as_str().unwrap_or_default())?,
            token: util::eth_address(r["eth_token"].as_str().unwrap_or_default())?,
            denom_index: r["denom_index"].as_u64().unwrap_or(0) as u32,
            recipient: util::eth_address(r["recipient"].as_str().unwrap_or_default())?,
            relayer: [0; 20],
            relay_fee: 0,
            seal: util::hex_bytes(seal)?,
        }
        .encode()
        .map_err(|e| e.message)?;
        let mut v = self.sb_view(&r);
        v["release_code"] = json!(code);
        v["release_link"] = json!(format!("https://nameswallet.app/bridge/#{code}"));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR: &str = include_str!("../testdata/public-burn-v1.json");

    #[test]
    fn the_module_builds_the_golden_vector_s_burn() {
        let v: Value = serde_json::from_str(VECTOR).unwrap();
        let (i, e) = (&v["inputs"], &v["expected"]);
        let id = |x: &Value| util::lez_account(x.as_str().unwrap().trim_start_matches("Public/")).unwrap();
        let h = |x: &Value| hex32(x.as_str().unwrap()).unwrap();
        let (cfg, k) = (h(&i["cfg_id"]), h(&i["exit_secret"]));
        let dest = util::eth_address(i["eth_dest"].as_str().unwrap()).unwrap();
        let token = util::eth_address(i["eth_token"].as_str().unwrap()).unwrap();
        let denom: u128 = i["denom"].as_str().unwrap().parse().unwrap();
        let fee: u128 = i["max_relay_fee"].as_str().unwrap().parse().unwrap();
        let ex = wallet::exit_commitment(&k, &dest, &token, denom, fee, &cfg);
        assert_eq!(hex0x(&ex), e["exit_commitment"]);
        assert_eq!(hex0x(&wallet::release_nullifier(&k)), e["release_nullifier"]);
        assert_eq!(hex0x(&exit_leaf(&cfg, &token, denom, &ex)), e["exit_leaf"]);
        let bridge = id(&i["bridge_program"]);
        assert_eq!(lez::b58(&lez::pda(&bridge, &lez::CONFIG_SEED)), e["config_pda"]);
        let m = burn_message(&id(&i["definition"]), denom, &bridge, &ex);
        assert_eq!(m.len() as u64, e["message_len"].as_u64().unwrap());
        let call = burn_call(&id(&i["token_program"]), &id(&i["definition"]), &bridge, &m, i["payer"].as_str().unwrap(), i["gas_limit"].as_u64().unwrap(), i["max_fee"].as_str().unwrap().parse().unwrap());
        assert_eq!(call, e["call"]);
    }

    fn log(first: u64, root: [u8; 32], leaves: &[[u8; 32]], block: u64) -> Value {
        let mut d = Vec::new();
        d.extend([0u8; 32]);
        d.extend([1u8; 32]);
        d.extend(root);
        d.extend(eth::word_u64(first));
        d.extend(eth::word_u64(0xa0));
        d.extend(eth::word_u64(leaves.len() as u64));
        for l in leaves {
            d.extend(l);
        }
        json!({ "data": hex0x(&d), "blockNumber": format!("0x{block:x}"), "transactionHash": "0xab" })
    }

    #[test]
    fn the_exit_tree_is_rebuilt_from_the_update_logs_and_a_short_set_is_refused() {
        let l: Vec<[u8; 32]> = (1..=5u8).map(|x| [x; 32]).collect();
        let a = parse_update(&log(0, root_of(&l[..2]), &l[..2], 10)).unwrap();
        let b = parse_update(&log(2, root_of(&l), &l[2..], 20)).unwrap();
        assert_eq!(b.first, 2);
        assert_eq!(b.leaves.len(), 3);
        assert_eq!(exit_tree(&[b.clone(), a.clone()], 5, &root_of(&l)).unwrap(), l);
        assert!(exit_tree(&[b.clone()], 5, &root_of(&l)).unwrap_err().contains("skip"));
        assert!(exit_tree(&[a.clone()], 5, &root_of(&l)).is_err(), "fewer leaves than exitCount()");
        let mut bad = b.clone();
        bad.exit_root = [9; 32];
        assert!(exit_tree(&[a, bad], 5, &root_of(&l)).is_err());
        assert_eq!(root_of(&[]), crate::relayed::zero_hashes()[32]);
    }

    #[test]
    fn the_release_digest_and_verify_call_are_the_vault_s() {
        let j = release_journal(&[1; 32], &[2; 32], &[3; 32], &[4; 20], 5, &[6; 20], &[0; 20], 0);
        assert_eq!(j.len(), 256);
        assert_eq!(&j[96 + 12..128], &[4u8; 20]);
        let seal = [withdraw::SELECTOR.to_vec(), vec![7u8; 256]].concat();
        let c = verify_calldata(&seal, &[8; 32], &[9; 32]);
        assert_eq!(&c[..4], &util::selector("verify(bytes,bytes32,bytes32)"));
        assert_eq!(c.len(), 4 + 32 * 4 + 288);
        assert_eq!(Words(&c[4..]).u64(3).unwrap(), 260);
    }

    #[test]
    fn the_topics_are_the_vault_s_events() {
        assert_eq!(update_topic(), util::keccak(b"StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])"));
        assert_eq!(EXIT_LEAF_TAG, util::keccak(b"RFP022/exit-leaf/v1"));
    }
}
