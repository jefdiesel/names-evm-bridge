// SPDX-License-Identifier: MIT OR Apache-2.0
//! "Receive from Ethereum" (modules/NAMES-RECEIVE.md): where a deposit the wallet made the
//! secret for stands, cheaply enough for a 60 s poll, and its claim checked without a file.
//!
//! `receive_status(id)` reads, per call:
//! - Sepolia: `latest`, `finalized` and the vault's `depositCount()` (3 calls). The vault's
//!   `Deposit` logs are read again only when the count changed since the last call (cached in
//!   memory).
//! - LEZ: once per session the bridge's Config (its nullifier formula). Then, for a final deposit,
//!   the recorded-root accounts of the newest final roots that include its leaf, newest first, up
//!   to 8 (usually 1), and the deposit's nullifier account (1).
//!
//! Claimability is the relayed check's test (a root the bridge recorded on LEZ), not the RFP-021
//! attestation anchor that `deposits()` shows. No wallet call: "claimed" comes from the
//! nullifiers kept with the deposit when it was made or recovered.

use crate::eth::{self, DepositEvent};
use crate::lez;
use crate::util::{self, format_amount, hex0x, hex32, now_ms};
use crate::Bridge;
use serde_json::{json, Value};

/// Final roots tried, newest first, for a recorded one (as the relayed check).
const ROOT_TRIES: usize = 8;

impl Bridge {
    /// The vault's deposit events, read again only when `depositCount()` moved. Returns
    /// (events, latest block, finalized block, Sepolia calls made).
    fn receive_events(&mut self) -> Result<(Vec<DepositEvent>, u64, u64, u32), String> {
        let d = self.deployment().clone();
        let vault = hex0x(&d.vault);
        let cached = self.relayed.log_cache.clone().filter(|(v, _, _)| *v == vault);
        let (events, latest, finalized, calls, count) = {
            let eth = self.eth()?;
            let latest = eth.block("latest")?;
            let finalized = eth.block("finalized")?;
            let count = eth::Words(&eth.call(&d.vault, &eth::calldata("depositCount()", &[]))?).u64(0)?;
            match cached {
                Some((_, n, ev)) if n == count => (ev, latest.number, finalized.number, 3, count),
                _ => (eth::deposit_events(eth.as_ref(), &d.vault, d.vault_create_block, latest.number, count)?, latest.number, finalized.number, 4, count),
            }
        };
        self.relayed.log_cache = Some((vault, count, events.clone()));
        Ok((events, latest, finalized, calls))
    }

    /// Where a stored deposit stands: `requested` (not in the vault yet), `deposited` (N
    /// confirmations, final in about M minutes), `final` (waiting for the bridge to record a root
    /// that includes it), `claimable`, `claimed`.
    pub(crate) fn receive_status_now(&mut self, id: &str) -> Result<Value, String> {
        let v = self.receive_status_read(id)?;
        // Kept with the deposit, so `receives()` lists the last known stage without a chain read.
        let mut row = self.store().deposit(id)?;
        row["receive"] = json!({ "stage": v["stage"], "text": v["text"], "leaf": v["leaf"], "block": v["block"], "tx": v["tx"],
                                 "claimed_block": v["claimed_block"], "as_of": v["as_of"] });
        if v["stage"] == "claimed" {
            row["claimed"] = json!(true);
        }
        self.store().put_deposit(row)?;
        Ok(v)
    }

    /// This install's deposits whose secret the Names Wallet makes ("Receive from Ethereum"),
    /// newest first, with the stage `receive_status` last saw: no chain or wallet read.
    pub(crate) fn receives_now(&self) -> Value {
        let mut list: Vec<Value> = self.store().load()["deposits"].as_array().cloned().unwrap_or_default().into_iter().filter(|d| d["derivation"] == "names-app").collect();
        list.sort_by_key(|d| std::cmp::Reverse(d["created"].as_u64().unwrap_or(0)));
        let d = self.deployment().clone();
        let rows: Vec<Value> = list
            .iter()
            .map(|r| {
                let decimals = d.token(r["symbol"].as_str().unwrap_or_default()).map(|t| t.decimals).unwrap_or(18);
                let amount = r["denom"].as_str().and_then(|x| util::units(x).ok()).unwrap_or(0);
                let last = &r["receive"];
                let claimed = r["claimed"] == true || last["stage"] == "claimed";
                json!({
                    "id": r["id"], "symbol": r["symbol"], "denom": r["denom"], "denom_text": format_amount(amount, decimals), "decimals": decimals,
                    "amount_text": format!("{} {}", format_amount(amount, decimals), r["symbol"].as_str().unwrap_or("")),
                    "account": r["account"], "account_kind": r["account_kind"], "commitment": r["commitment"], "created": r["created"], "key_index": r["key_index"],
                    "compare": r["commitment"].as_str().and_then(|c| hex32(c).ok()).map_or(Value::Null, |c| crate::compare::json(&c)),
                    "claim": if r["account_kind"] == "public" { "relayed" } else { "private" },
                    "claimed": claimed,
                    "stage": if claimed { Value::from("claimed") } else { last.get("stage").cloned().filter(|s| !s.is_null()).unwrap_or(Value::from("unknown")) },
                    "text": last["text"], "leaf": last["leaf"], "block": last["block"], "tx": last["tx"], "claimed_block": last["claimed_block"], "as_of": last["as_of"],
                })
            })
            .collect();
        json!({ "receives": rows, "network": d.network, "deployment": d.name })
    }

    /// The deposit code of stored deposit `id` again (a receive made in an earlier session):
    /// rebuilt from what this computer kept with the deposit (chain, vault, token, denomination,
    /// commitment), byte for byte the code `make_deposit_request` gave. No wallet call, no secret,
    /// no chain read. Refused, with the reason, when the stored data cannot rebuild that code
    /// (a deposit from a build that kept no vault, or a code that no longer matches): never a
    /// guessed code.
    pub(crate) fn receive_request_now(&self, id: &str) -> Result<Value, String> {
        if id.is_empty() {
            return Err("give a deposit id (from receives())".to_owned());
        }
        let row = self.store().deposit(id)?;
        let cant = |why: &str| format!("The deposit code of {id} can't be shown again: {why}. Nothing was guessed; the code is in the reply that first made it (its export text), or make a new deposit request.");
        let commitment = row["commitment"].as_str().and_then(|c| hex32(c).ok()).ok_or_else(|| cant("its commitment is not stored"))?;
        let token = row["eth_token"].as_str().and_then(|t| util::eth_address(t).ok()).ok_or_else(|| cant("its token is not stored"))?;
        let denom_index = row["denom_index"].as_u64().and_then(|i| u8::try_from(i).ok()).ok_or_else(|| cant("its denomination index is not stored"))?;
        let denom = row["denom"].as_str().and_then(|x| util::units(x).ok()).filter(|d| *d > 0).ok_or_else(|| cant("its amount is not stored"))?;
        let (Some(chain_id), Some(vault)) = (row["chain_id"].as_u64(), row["vault"].as_str().and_then(|v| util::eth_address(v).ok())) else {
            return Err(cant("it was made by an older build of this module, which did not keep the vault it was for"));
        };
        let code = crate::request::encode(chain_id, &vault, &token, denom_index, denom, &commitment)?;
        if row["request"].as_str().is_some_and(|orig| orig != code) {
            return Err(cant("the code kept with it does not match its stored details"));
        }
        let link = crate::request::url(crate::request::PAGE_URL, &code);
        // The amount in words from the deployment the deposit was made on (this one, or a built-in
        // one by name); none when that deployment doesn't list the token.
        let name = row["deployment"].as_str().unwrap_or_default();
        let dep = if self.deployment().name == name { Some(self.deployment().clone()) } else { crate::config::builtin(name) };
        let t = dep.as_ref().and_then(|d| d.tokens.iter().find(|k| k.eth_token == token && d.vault == vault));
        let symbol = row["symbol"].as_str().unwrap_or_default();
        let (denom_text, amount_text) = match t {
            Some(t) => (Value::from(format_amount(denom, t.decimals)), Value::from(format!("{} {symbol}", format_amount(denom, t.decimals)))),
            None => (Value::Null, Value::Null),
        };
        let last = &row["receive"];
        let claimed = row["claimed"] == true || last["stage"] == "claimed";
        Ok(json!({
            "id": id, "request": code, "link": link, "request_url": link, "request_qr": crate::qr::matrix(&link)?,
            "compare": crate::compare::json(&commitment), "commitment": hex0x(&commitment),
            "symbol": symbol, "denom": denom.to_string(), "denom_text": denom_text, "amount_text": amount_text, "denom_index": denom_index,
            "chain_id": chain_id, "vault": util::checksum_address(&vault), "deployment": name,
            "network": dep.as_ref().map(|d| d.network.clone()),
            "account": row["account"], "account_kind": row["account_kind"],
            "claimed": claimed,
            "stage": if claimed { Value::from("claimed") } else { last.get("stage").cloned().filter(|s| !s.is_null()).unwrap_or(Value::from("unknown")) },
            "text": last["text"], "as_of": last["as_of"],
        }))
    }

    fn receive_status_read(&mut self, id: &str) -> Result<Value, String> {
        let row = self.store().deposit(id)?;
        let commitment = hex32(row["commitment"].as_str().unwrap_or_default())?;
        let d = self.deployment().clone();
        let decimals = d.token(row["symbol"].as_str().unwrap_or_default()).map(|t| t.decimals).unwrap_or(18);
        let amount = row["denom"].as_str().and_then(|x| util::units(x).ok()).unwrap_or(0);
        let kind = row["account_kind"].as_str().unwrap_or("private").to_owned();
        let mut v = json!({
            "id": id, "commitment": hex0x(&commitment), "compare": crate::compare::json(&commitment), "account": row["account"], "account_kind": kind,
            "amount_text": format!("{} {}", format_amount(amount, decimals), row["symbol"].as_str().unwrap_or("")),
            "network": d.network, "claim": if kind == "public" { "relayed" } else { "private" },
            "leaf": null, "block": null, "tx": null, "confirmations": null, "final_in_min": null, "claimed_block": null, "as_of": now_ms(),
        });
        let (events, latest, finalized, eth_calls) = self.receive_events()?;
        let mut lez_reads = 0u32;
        let Some(ev) = events.iter().find(|e| e.commitment == commitment).cloned() else {
            v["stage"] = json!("requested");
            v["text"] = json!("Not deposited yet: open the deposit code on the deposit page.");
            v["reads"] = json!({ "sepolia": eth_calls, "lez": lez_reads });
            return Ok(v);
        };
        v["leaf"] = json!(ev.leaf_index);
        v["block"] = json!(ev.block);
        if !ev.tx.is_empty() {
            v["tx"] = json!(ev.tx);
        }
        if ev.block > finalized {
            let conf = latest.saturating_sub(ev.block) + 1;
            // Sepolia's finalized block moves an epoch (32 blocks, 6.4 min) at a time: the deposit is
            // final at the jump that passes it, so count whole epochs (an upper bound), not blocks
            // (which reads "1 min" for up to an epoch; seen on the public run, 2026-11-14).
            let mins = (ev.block - finalized).div_ceil(32) * 32 * 12 / 60 + 1;
            v["stage"] = json!("deposited");
            v["confirmations"] = json!(conf);
            v["final_in_min"] = json!(mins);
            v["text"] = json!(format!("Deposited: {conf} confirmations; final on Sepolia in about {mins} min."));
            v["reads"] = json!({ "sepolia": eth_calls, "lez": lez_reads });
            return Ok(v);
        }
        let rpc = self.lez_rpc().map_err(|r| r.message)?;
        let bridge = util::lez_account(&d.bridge_program)?;
        // Claimed? From the nullifier kept with the deposit (the bridge's formula, read once).
        let pre_fix = match self.relayed.pre_fix.clone().filter(|(n, _)| *n == d.name) {
            Some((_, p)) => p,
            None => {
                lez_reads += 1;
                let p = lez::bridge_config(rpc.as_ref(), &bridge)?.ok_or_else(|| format!("There is no bridge on {} (the network may have been reset).", d.network))?.pre_fix();
                self.relayed.pre_fix = Some((d.name.clone(), p));
                p
            }
        };
        let nf = row[if pre_fix { "nullifier_pre_fix" } else { "nullifier" }].as_str().and_then(|x| hex32(x).ok());
        if let Some(nf) = nf {
            lez_reads += 1;
            if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nf)).map_err(|p| p.message)? {
                // The block is known when this module sent the claim.
                let job = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().rev().find(|j| j["meta"]["nullifier"] == hex0x(&nf).as_str() && j["state"] == "done").cloned());
                v["stage"] = json!("claimed");
                v["claimed_block"] = job.as_ref().map_or(Value::Null, |j| j["block"].clone());
                v["text"] = json!(match job.as_ref().and_then(|j| j["block"].as_u64()) {
                    Some(b) => format!("Claimed in block {b} on {}.", d.network),
                    None => format!("Claimed on {}.", d.network),
                });
                v["reads"] = json!({ "sepolia": eth_calls, "lez": lez_reads });
                return Ok(v);
            }
        }
        // A root the bridge recorded that includes the leaf: the newest final ones first.
        let mut recorded = false;
        for e in events.iter().rev().filter(|e| e.leaf_index >= ev.leaf_index && e.block <= finalized).take(ROOT_TRIES) {
            lez_reads += 1;
            if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::root_seed(&e.root)).map_err(|p| p.message)? {
                recorded = true;
                break;
            }
        }
        if !recorded {
            v["stage"] = json!("final");
            v["text"] = json!(format!("Final on Sepolia; waiting for the bridge to record it on {} (about a minute).", d.network));
        } else {
            v["stage"] = json!("claimable");
            v["text"] = json!(if kind == "public" {
                format!("Arrived: {} is ready to claim (the bridge's relay pays the fee).", v["amount_text"].as_str().unwrap_or(""))
            } else {
                format!("Arrived: {} is ready to claim privately (your wallet makes a proof).", v["amount_text"].as_str().unwrap_or(""))
            });
            if nf.is_none() {
                v["note"] = json!("This deposit was stored before its nullifier was kept: whether it was claimed elsewhere shows when you check its claim.");
            }
        }
        v["reads"] = json!({ "sepolia": eth_calls, "lez": lez_reads });
        Ok(v)
    }
}
