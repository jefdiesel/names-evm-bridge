//! The bridge's flows when the person's accounts are in the Names Wallet (`config().wallet ==
//! "names"`): deposit requests from `app_secret` values, claims and burns proposed as private
//! calls, recovery by batch scans. See `wallet.rs` for what this module may hold.
//!
//! Gates, in this order, before anything is proposed (BRIDGE-SDK.md §2, the wallet's refusal
//! rules): access (`request_access`), the bridged tokens accepted as a token source (without it
//! the wallet can't value a change to a token holding and refuses a burn or a second claim),
//! the secret grant for the scope (`app_secret`'s first answer), then the proposal.

use crate::cli;
use crate::config;
use crate::eth;
use crate::privacy;
use crate::store::{self, public_view};
use crate::util::{self, format_amount, hex0x, hex32, now_ms, units};
use crate::wallet::{self, Values};
use crate::Bridge;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A claim's call is built in a few seconds (the material is cached); a burn's also reads the
/// bridge's LEZ blocks once to refuse a commitment already burned (cached after).
const EMIT_PATIENCE: Duration = Duration::from_secs(25);
const BURN_EMIT_PATIENCE: Duration = Duration::from_secs(120);
/// How far past the last found index a recovery scan looks (in values; batches of 64).
const DEFAULT_GAP: u64 = 20;

/// What the CLI's `--emit-call` gave: the call (with the secret in its message: kept in memory
/// only) and the result event (public).
pub struct Emitted {
    pub call: Value,
    pub result: Value,
    pub plan: Value,
}

/// The CLI's NDJSON: the `plan` event, and the `result` or `error` event.
pub fn read_events(out: &str) -> (Value, Option<Value>, Option<Value>) {
    let mut plan = Value::Null;
    let (mut result, mut error) = (None, None);
    for line in out.lines().map(str::trim).filter(|l| l.starts_with('{')) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v["event"].as_str() {
            Some("plan") => plan = v,
            Some("result") => result = Some(v),
            Some("error") => error = Some(v),
            _ => {}
        }
    }
    (plan, result, error)
}

/// A fresh name in the module's private temporary folder.
fn temp_path(dir: &Path, what: &str) -> Result<PathBuf, String> {
    let r = util::random32()?;
    Ok(dir.join(format!("{what}-{}", &util::to_hex(&r)[..16])))
}

impl Bridge {
    pub(crate) fn names_mode(&self) -> bool {
        self.cfg.names_wallet()
    }

    fn wstate_path(&self) -> PathBuf {
        self.data.join("wallet.json")
    }

    /// `<data>/wallet.json` (0600): the requests this module made (token source, connect) and
    /// the next exit index per tuple. Nothing secret.
    pub(crate) fn wstate(&self) -> Value {
        let mut v = util::read_json(&self.wstate_path()).unwrap_or_else(|| json!({}));
        for k in ["jobs", "exit_next"] {
            if v[k].is_null() {
                v[k] = if k == "jobs" { json!([]) } else { json!({}) };
            }
        }
        v
    }

    pub(crate) fn put_wstate(&self, v: &Value) -> Result<(), String> {
        util::write_private(&self.wstate_path(), v.to_string().as_bytes()).map_err(|e| format!("cannot save the wallet state: {e}"))
    }

    /// The private accounts the Names Wallet gave this module (bare ids), and whether it is
    /// connected at all.
    pub(crate) fn granted(&self) -> Result<(bool, Vec<Value>), String> {
        self.granted_typed().map_err(|f| f.text())
    }

    /// [`Self::granted`], with why the wallet gave no answer (busy is not "not installed").
    pub(crate) fn granted_typed(&self) -> Result<(bool, Vec<Value>), wallet::Fail> {
        let g = wallet::names_try("granted_accounts", json!([]))?;
        let accounts = g["accounts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                let written = a["account"].as_str().unwrap_or_default();
                json!({ "account": written.trim_start_matches("Private/").trim_start_matches("Public/"), "kind": a["account_kind"], "written": written })
            })
            .collect();
        Ok((g["connected"] == true, accounts))
    }

    fn granted_private(&self) -> Result<Vec<String>, String> {
        Ok(self.granted()?.1.into_iter().filter(|a| a["kind"] == "private").filter_map(|a| a["account"].as_str().map(str::to_owned)).collect())
    }

    /// The private account to use: `asked` if the wallet gave it, else the first it gave.
    fn names_account(&self, asked: &str) -> Result<String, String> {
        let mine = self.granted_private()?;
        let a = asked.trim().trim_start_matches("Private/").trim_start_matches("Public/");
        if a.is_empty() {
            return mine.into_iter().next().ok_or_else(|| "the Names Wallet hasn't given this app a private account yet: connect it (Settings) and pick a private account".to_owned());
        }
        if mine.iter().any(|m| m == a) {
            Ok(a.to_owned())
        } else {
            Err(format!("Private/{a} isn't one of the private accounts the Names Wallet gave this app; connect it again and pick it"))
        }
    }

    /// The bridge config the CLI reads (`show --json`), for the token source's decimals.
    fn show_json(&self) -> Result<Value, String> {
        let c = self.wallet_cli()?;
        if !c.json_mode() {
            return Err("this needs a lez-bridge CLI that speaks --json (CLI-CONTRACT.md)".to_owned());
        }
        c.show(&self.deployment().bridge_program)
    }

    /// Whether the bridged tokens are a token source the person accepted; proposes them once
    /// (first run) when not. Returns `(accepted, the request to watch)`.
    pub(crate) fn token_source(&self, propose: bool) -> Result<(bool, Value), String> {
        let tp = self.deployment().token_program.clone();
        let sources = wallet::names("token_sources", json!([]))?;
        if sources.as_array().is_some_and(|l| l.iter().any(|s| s["token_program"] == tp.as_str())) {
            return Ok((true, Value::Null));
        }
        let mut st = self.wstate();
        if let Some(id) = st["token_source_request"].as_str() {
            if let Ok(r) = wallet::names("request_status", json!([id])) {
                if matches!(r["state"].as_str(), Some("pending")) {
                    return Ok((false, r));
                }
                if matches!(r["state"].as_str(), Some("approved") | Some("done")) {
                    return Ok((true, Value::Null));
                }
            }
        }
        if !propose {
            return Ok((false, Value::Null));
        }
        let defs = wallet::token_definitions(&self.show_json()?, self.deployment())?;
        let r = wallet::names("propose_token_source", json!([tp, defs.to_string()]))?;
        st["token_source_request"] = r["request"].clone();
        self.put_wstate(&st)?;
        Ok((false, r))
    }

    /// The gates before a proposal: access, then (for claims and burns) the token source.
    /// `Some(needs)` when the person has something to approve first.
    fn wallet_gate(&self, token_source: bool) -> Result<Option<Value>, String> {
        let (connected, accounts) = self.granted()?;
        if !connected {
            let r = wallet::names("request_access", json!([wallet::ACCESS_REASON]))?;
            return Ok(Some(wallet::needs("connect", &r, "Connect your Names Wallet: approve this app in the Names app and give it a private account.")?));
        }
        if !accounts.iter().any(|a| a["kind"] == "private") {
            // The wallet can't add an account to an existing access: nothing to approve, so an error.
            return Err("Your Names Wallet gave this app no private account. In the Names app, take this app's access back (Settings, Connected apps) and connect again with a private account. Nothing was sent.".to_owned());
        }
        if token_source {
            let (ok, req) = self.token_source(true)?;
            if !ok {
                return Ok(Some(wallet::needs(
                    "token_source",
                    &req,
                    "Approve the bridged tokens in the Names app first: your wallet needs to know them to show and check what a claim or a burn changes.",
                )?));
            }
        }
        Ok(None)
    }

    fn secret_needs(req: &Value, scope: &str) -> Result<Value, String> {
        wallet::needs(
            "secret",
            req,
            &format!("Approve \"{scope}\" in the Names app: your wallet makes this app's deposit and exit secrets from your account, so they come back when you restore the wallet from its recovery phrase."),
        )
    }

    /// The claim secret of a stored deposit: its own (imported, or made in CLI mode) or the
    /// wallet's value for its index.
    fn deposit_secret(&self, d: &Value) -> Result<Result<[u8; 32], Value>, String> {
        if let Some(s) = d["secret"].as_str() {
            return Ok(Ok(hex32(s)?));
        }
        let index = d["key_index"].as_u64().ok_or("this deposit has neither a secret nor a wallet index")?;
        let (cfg, token, denom) = (hex32(&self.deployment().cfg_id)?, util::eth_address(d["eth_token"].as_str().unwrap_or_default())?, units(d["denom"].as_str().unwrap_or("0"))?);
        let account = d["account"].as_str().unwrap_or_default();
        match wallet::app_secret(account, wallet::SCOPE_DEPOSIT, &wallet::deposit_context(&cfg, &token, denom), wallet::TAG_DEPOSIT, index, 1)? {
            Values::Pending(req) => Ok(Err(Self::secret_needs(&req, wallet::SCOPE_DEPOSIT)?)),
            Values::Granted(v) => {
                let k = v[0];
                let c = wallet::deposit_commitment(&k, &util::lez_account(account)?, &token, denom, &cfg);
                if !hex0x(&c).eq_ignore_ascii_case(d["commitment"].as_str().unwrap_or_default()) {
                    return Err("the Names Wallet gives another secret for this deposit than the one it was made with (another account or another wallet?); nothing was proposed".to_owned());
                }
                Ok(Ok(k))
            }
        }
    }

    /// Runs `args` with the secret in a 0600 file (never on argv) and `--emit-call` to another
    /// new file; reads the call and removes both files whatever happens.
    fn emit(&self, mut args: Vec<String>, secret: Option<&[u8; 32]>, patience: Duration) -> Result<Result<Emitted, Value>, String> {
        let c = self.wallet_cli()?;
        if !c.json_mode() {
            return Err("the Names Wallet path needs a lez-bridge CLI that speaks --json and --emit-call (CLI-CONTRACT.md)".to_owned());
        }
        util::private_dir(&c.tmp_dir).map_err(|e| e.to_string())?;
        let call_path = temp_path(&c.tmp_dir, "call")?;
        let secret_path = match secret {
            Some(k) => {
                let p = temp_path(&c.tmp_dir, "k")?;
                util::write_private(&p, hex0x(k).as_bytes()).map_err(|e| e.to_string())?;
                args.extend(["--secret-file".to_owned(), p.display().to_string()]);
                Some(p)
            }
            None => None,
        };
        args.extend(["--emit-call".to_owned(), call_path.display().to_string()]);
        let mut a = vec!["--json".to_owned()];
        a.extend(args);
        let ran = cli::run(c.command(&a), patience);
        if let Some(p) = &secret_path {
            let _ = std::fs::remove_file(p);
        }
        let call = std::fs::read_to_string(&call_path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let _ = std::fs::remove_file(&call_path);
        let (code, out) = ran?;
        let (plan, result, error) = read_events(&out);
        if let Some(e) = error {
            return Ok(Err(json!({ "code": e["code"], "message": e["message"], "local_only": e["local_only"] })));
        }
        match (code, result, call) {
            (Some(0), Some(result), Some(call)) => Ok(Ok(Emitted { call, result, plan })),
            _ => Err(cli::cli_failure(&out)),
        }
    }

    /// The claim of a stored deposit as a private call (built by the CLI; nothing proposed).
    fn names_claim_call(&mut self, d: &Value, reveal: bool) -> Result<Result<Emitted, Value>, String> {
        if let Some(n) = self.wallet_gate(true)? {
            return Ok(Err(n));
        }
        let k = match self.deposit_secret(d)? {
            Ok(k) => k,
            Err(n) => return Ok(Err(n)),
        };
        let (leaves, dir, _, anchor) = self.claim_material(d)?;
        let index = self.leaf_index(d)?;
        let dep = self.deployment().clone();
        let mut a: Vec<String> = [
            "claim-attested", "--bridge", &dep.bridge_program, "--token-program", &dep.token_program, "--from", d["account"].as_str().unwrap_or_default(),
            "--eth-token", d["eth_token"].as_str().unwrap_or_default(), "--denom-index", &d["denom_index"].to_string(), "--denom",
            d["denom"].as_str().unwrap_or_default(), "--cfg-id", &dep.cfg_id, "--leaves", &leaves.display().to_string(), "--index", &index.to_string(),
            "--proof", &dir.join("eth_getProof.json").display().to_string(), "--anchor-block-hash", &anchor, "--header-rlp",
            &dir.join("header.rlp.hex").display().to_string(),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        if reveal {
            a.push("--reveal-recipient".into());
        }
        match self.emit(a, Some(&k), EMIT_PATIENCE)? {
            Ok(e) => Ok(Ok(e)),
            Err(error) => Ok(Err(json!({ "error": error }))),
        }
    }

    // ── Deposits ────────────────────────────────────────────────────────

    /// A deposit request whose secret is the wallet's `RFP022/deposit` value for the next free
    /// index of (account, token, denomination). Stores the index, never the secret.
    pub(crate) fn names_deposit_request(&mut self, token: &str, denom_index: &str, account: &str) -> Result<Value, String> {
        let (t, row) = self.token_info(token)?;
        let i: usize = denom_index.trim().parse().map_err(|_| "denomination index must be a number")?;
        let denom_row = row["denoms"].get(i).ok_or_else(|| format!("{} has {} denominations", t.symbol, row["denoms"].as_array().map_or(0, Vec::len)))?.clone();
        let denom = units(denom_row["value"].as_str().unwrap_or(""))?;
        let fee = units(denom_row["deposit_fee"].as_str().unwrap_or("0")).unwrap_or(0);
        let (acct, written, kind) = match self.receive_account(account)? {
            Ok(x) => x,
            Err(needs) => return Ok(needs),
        };
        let v = util::lez_account(&acct)?;
        let d = self.deployment().clone();
        let cfg = hex32(&d.cfg_id)?;
        let ctx = wallet::deposit_context(&cfg, &t.eth_token, denom);
        let (events, _, _) = self.events()?;
        // Past every index this computer handed out and every index the vault shows used.
        let local_next = self.store().load()["deposits"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["account"] == acct.as_str() && x["eth_token"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(&eth::addr_hex(&t.eth_token))) && x["denom_index"] == i as u64)
            .filter_map(|x| x["key_index"].as_u64().map(|k| k + 1))
            .max()
            .unwrap_or(0);
        let mut first = 0;
        let mut used: Option<u64> = None;
        let mut batch: Vec<[u8; 32]>;
        loop {
            batch = match wallet::app_secret(&written, wallet::SCOPE_DEPOSIT, &ctx, wallet::TAG_DEPOSIT, first, wallet::BATCH)? {
                Values::Pending(req) => return Self::secret_needs(&req, wallet::SCOPE_DEPOSIT),
                Values::Granted(b) => b,
            };
            for (o, k) in batch.iter().enumerate() {
                let c = wallet::deposit_commitment(k, &v, &t.eth_token, denom, &cfg);
                if events.iter().any(|e| e.commitment == c) {
                    used = Some(first + o as u64);
                }
            }
            // Another batch only while used indexes reach its end (a gap of 20 ends the scan).
            if used.is_some_and(|u| u + DEFAULT_GAP >= first + wallet::BATCH) {
                first += wallet::BATCH;
                continue;
            }
            break;
        }
        let next = used.map_or(0, |u| u + 1).max(local_next);
        let k = if next >= first && next < first + wallet::BATCH {
            batch[(next - first) as usize]
        } else {
            match wallet::app_secret(&written, wallet::SCOPE_DEPOSIT, &ctx, wallet::TAG_DEPOSIT, next, 1)? {
                Values::Pending(req) => return Self::secret_needs(&req, wallet::SCOPE_DEPOSIT),
                Values::Granted(b) => b[0],
            }
        };
        let c = hex0x(&wallet::deposit_commitment(&k, &v, &t.eth_token, denom, &cfg));
        let mut stored = store::new_deposit(&eth::addr_hex(&t.eth_token), &t.symbol, i as u32, denom, &acct, kind, &c, "", "wallet");
        if let Some(o) = stored.as_object_mut() {
            o.remove("secret");
            o.insert("key_index".into(), Value::from(next));
            o.insert("derivation".into(), Value::from("names-app"));
            // The claim nullifiers (one-way images of k; public once claimed): `receive_status`
            // tells "claimed" from them without asking the wallet again.
            o.insert("nullifier".into(), Value::from(hex0x(&crate::relayed::claim_nullifier(&k, &v, &t.eth_token, denom))));
            o.insert("nullifier_pre_fix".into(), Value::from(hex0x(&crate::relayed::claim_nullifier_pre_fix(&k))));
        }
        self.store().put_deposit(stored.clone())?;
        let mut reply = self.deposit_reply(&t, i, denom, fee, &c, &stored, kind)?;
        if kind == "public" {
            // Shown before the person deposits: a public claim is not private.
            reply["not_private"] = crate::relayed::not_private_notice(Some(&written));
        }
        reply["derivation"] = json!({ "scheme": "names-app", "app": "lez_bridge", "scope": wallet::SCOPE_DEPOSIT, "account": written, "index": next });
        if let Some(text) = reply["export_text"].as_str() {
            reply["export_text"] = Value::from(text.replace(
                "Keep this computer's bridge data backed up: the secret that claims this deposit is stored only here.",
                "The secret that claims this deposit comes from your Names Wallet: restoring the wallet from its recovery phrase restores it.",
            ));
        }
        Ok(reply)
    }

    /// The account a wallet-made deposit is for: `Public/<id>` (a public account given to this app:
    /// its claim is the relayed public claim) or a private account (as before). `Err(needs)` while
    /// the person has something to approve in the Names app first.
    pub(crate) fn receive_account(&self, asked: &str) -> Result<Result<(String, String, &'static str), Value>, String> {
        let asked = asked.trim();
        if let Some(id) = asked.strip_prefix("Public/") {
            util::lez_account(id)?;
            let written = format!("Public/{id}");
            let (connected, accounts) = self.granted()?;
            if connected && !accounts.iter().any(|a| a["account"] == id && a["kind"] == "public") {
                // Names with `grant_add_account` asks the person to add it (one sheet); without it,
                // asking makes no request (the wallet answers "approved"), so: take access back.
                return match wallet::add_account(crate::relayed::ACCESS_REASON, &written)? {
                    Some(r) => Ok(Err(wallet::needs("public_account", &r, &format!("Approve adding {written} to this app's access in the Names app: deposits to it are claimed from your wallet, the bridge's relay pays the fee."))?)),
                    None => Ok(Ok((id.to_owned(), written, "public"))),
                };
            }
            if !connected {
                let r = wallet::request_access(crate::relayed::ACCESS_REASON, Some(&written)).map_err(|f| f.text())?;
                // Already given meanwhile (approved without a request): go on.
                if r["state"] == "approved" && r["accounts"].as_array().is_some_and(|l| l.iter().any(|a| a == written.as_str())) {
                    return Ok(Ok((id.to_owned(), written, "public")));
                }
                return Ok(Err(wallet::needs("connect", &r, &format!("Approve this app in the Names app and give it your public account {written}: deposits to it are claimed from your wallet, the bridge's relay pays the fee."))?));
            }
            return Ok(Ok((id.to_owned(), written, "public")));
        }
        if let Some(n) = self.wallet_gate(false)? {
            return Ok(Err(n));
        }
        let acct = self.names_account(asked)?;
        let written = format!("Private/{acct}");
        Ok(Ok((acct, written, "private")))
    }

    // ── Claims ──────────────────────────────────────────────────────────

    pub(crate) fn names_claim_plan(&mut self, id: &str) -> Result<Value, String> {
        let d = self.store().deposit(id)?;
        let built = self.names_claim_call(&d, false)?;
        let status = self.deposits_now().ok().and_then(|v| v["deposits"].as_array()?.iter().find(|x| x["id"] == id).cloned()).unwrap_or(Value::Null);
        let early = status["claim_after"].as_u64().is_some_and(|t| now_ms() / 1000 < t);
        let mut choices = json!({ "flow": "claim" });
        if let Some(n) = status["anonymity_set"].as_u64() {
            choices["anonymity_set"] = Value::from(n);
        }
        if early {
            choices["claim_early"] = Value::from(true);
        }
        let mut v = json!({
            "deposit": public_view(&status), "wallet": "names", "claim_early": early, "privacy": privacy::report(&choices)?,
            "proving": crate::proving_note(&self.cfg), "sends_enabled": true,
            "approval": "Your Names app shows what this claim does, simulated by your wallet, and asks you to approve. Your wallet proves it on this computer after you approve; this app never proves.",
        });
        match built {
            Ok(e) => {
                v["ok"] = Value::from(true);
                v["error"] = Value::Null;
                v["recipient"] = e.result["recipient"].clone();
                v["recipient_kind"] = e.result["recipient_kind"].clone();
                v["sponsored"] = e.result["sponsored"].clone();
                v["sponsor"] = e.result["sponsor"].clone();
                v["root"] = e.result["root"].clone();
                v["call"] = json!({ "programs": e.call["programs"], "accounts": e.call["accounts"].as_array().map_or(0, Vec::len),
                                    "message_bytes": e.result["message_bytes"], "declared": e.call["declared"], "fee_payer": e.call["fee_payer"] });
                let _ = e.plan;
            }
            Err(n) if n.get("needs").is_some() => {
                v["ok"] = Value::from(false);
                v["needs"] = n;
            }
            Err(n) => {
                let error = n["error"].clone();
                if error["code"] == "already_claimed" && d["claimed"] != true {
                    let mut dd = d.clone();
                    dd["claimed"] = Value::from(true);
                    self.store().put_deposit(dd)?;
                }
                v["ok"] = Value::from(false);
                v["error"] = error;
            }
        }
        Ok(v)
    }

    pub(crate) fn names_claim(&mut self, id: &str, options: &Value) -> Result<Value, String> {
        let d = self.store().deposit(id)?;
        let reveal = options["reveal_recipient"] == true;
        let mut choices = json!({ "flow": "claim", "confirm_weaker": options["confirm_weaker"] == true });
        if reveal {
            choices["reveal_recipient"] = Value::from(true);
        }
        if options["claim_early"] == true {
            choices["claim_early"] = Value::from(true);
        }
        privacy::require_confirmation(&choices)?;
        // Asked again for a deposit whose claim is already with the wallet: that job.
        let running = self.wstate()["jobs"].as_array().and_then(|l| l.iter().rev().find(|j| j["kind"] == "claim" && j["subject"] == id && j["state"] == "running").cloned());
        if let Some(j) = running {
            return self.wallet_job(j["id"].as_str().unwrap_or_default());
        }
        // A proposal that got no answer may be waiting in the Names app (approved, the wallet
        // proves and sends it itself): not proposed again while it may still be there.
        let ukey = format!("claim:{id}");
        if let Some(at) = self.wstate()["private_uncertain"][&ukey].as_u64() {
            if now_ms().saturating_sub(at) <= crate::relayed::UNCERTAIN_MS && options[crate::relayed::AFTER_UNCERTAIN] != true {
                return Err(Self::claim_uncertain_words());
            }
        }
        let e = match self.names_claim_call(&d, reveal)? {
            Ok(e) => e,
            Err(n) if n.get("needs").is_some() => return Ok(n),
            Err(n) => return Err(n["error"]["message"].as_str().unwrap_or("the claim was refused before anything was proposed").to_owned()),
        };
        let decimals = self.deployment().token(d["symbol"].as_str().unwrap_or("")).map(|t| t.decimals).unwrap_or(18);
        let summary = format!(
            "Claim {} {} from {} (deposit {id})",
            format_amount(units(d["denom"].as_str().unwrap_or("0")).unwrap_or(0), decimals),
            d["symbol"].as_str().unwrap_or(""),
            if self.deployment().chain_id == 11_155_111 { "Sepolia" } else { "Ethereum" }
        );
        let r = match wallet::names_try("propose_private_call", json!([wallet::private_written(d["account"].as_str().unwrap_or_default()), e.call.to_string(), summary])) {
            Ok(r) => r,
            Err(wallet::Fail::NoAnswer) => {
                let mut st = self.wstate();
                st["private_uncertain"][&ukey] = Value::from(now_ms());
                self.put_wstate(&st)?;
                return Err(Self::claim_uncertain_words());
            }
            Err(f) => return Err(f.text()),
        };
        drop(e);
        let mut st = self.wstate();
        if let Some(m) = st["private_uncertain"].as_object_mut() {
            if m.remove(&ukey).is_some() {
                self.put_wstate(&st)?;
            }
        }
        self.new_wallet_job("claim", id, &r, json!({ "deposit": id, "symbol": d["symbol"], "denom": d["denom"], "privacy": privacy::report(&choices)?["level"] }))
    }

    fn claim_uncertain_words() -> String {
        "Your Names Wallet didn't answer in time, so this claim may be waiting in the Names app. If it is there, approve or reject it there (approved, the wallet proves and sends it itself, and this deposit then shows as claimed). This app doesn't propose it again for 11 minutes. Nothing was sent from here.".to_owned()
    }

    // ── Wallet jobs ─────────────────────────────────────────────────────

    fn new_wallet_job(&self, kind: &str, subject: &str, request: &Value, meta: Value) -> Result<Value, String> {
        let rid = request["request"].as_str().ok_or("the Names Wallet gave no request id")?.to_owned();
        let now = now_ms();
        let record = json!({
            "id": format!("wr-{now}"), "kind": kind, "subject": subject, "request": rid, "started": now, "finished": null,
            "state": "running", "phase": "approve", "meta": meta, "prove_estimate_s": config::CLAIM_PROOF_ESTIMATE_S,
        });
        let mut st = self.wstate();
        st["jobs"].as_array_mut().unwrap().push(record.clone());
        self.put_wstate(&st)?;
        Ok(wallet::job_view(&record, Some(request), now))
    }

    /// One wallet job, its state asked of the wallet while it runs (and kept once it ended).
    pub(crate) fn wallet_job(&self, id: &str) -> Result<Value, String> {
        let mut st = self.wstate();
        let list = st["jobs"].as_array_mut().unwrap();
        let rec = list.iter_mut().find(|j| j["id"] == id).ok_or_else(|| format!("there is no job {id}"))?;
        if rec["state"] != "running" {
            return Ok(wallet::job_view(rec, rec.get("last_status"), now_ms()));
        }
        // A poll: asked once. Busy or silent, the job is shown as last known, with a note.
        let status = match wallet::names_once("request_status", json!([rec["request"]])) {
            Ok(s) => s,
            Err(f) if f.transient() => {
                let mut view = wallet::job_view(rec, rec.get("last_status"), now_ms());
                view["note"] = Value::from(format!("Asking your Names Wallet how this stands: {}. Asking again shortly.", f.short()));
                return Ok(view);
            }
            Err(f) => return Err(f.text()),
        };
        // The proof's clock starts when the wallet first says it proves.
        if status["phase"] == "prove" && rec["prove_since"].is_null() {
            rec["prove_since"] = Value::from(now_ms());
        }
        let view = wallet::job_view(rec, Some(&status), now_ms());
        rec["last_status"] = json!({ "state": status["state"], "check": status["check"], "phase": status["phase"], "tx": status["tx"], "error": status["error"] });
        if view["state"] != "running" {
            rec["state"] = view["state"].clone();
            rec["phase"] = view["phase"].clone();
            rec["finished"] = view["finished"].clone();
        }
        self.put_wstate(&st)?;
        Ok(view)
    }

    pub(crate) fn wallet_jobs(&self) -> Vec<Value> {
        let ids: Vec<String> = self.wstate()["jobs"].as_array().cloned().unwrap_or_default().iter().filter_map(|j| j["id"].as_str().map(str::to_owned)).collect();
        let mut out: Vec<Value> = ids.iter().filter_map(|id| self.wallet_job(id).ok()).collect();
        out.reverse();
        out
    }

    pub(crate) fn cancel_wallet_job(&self, id: &str) -> Result<Value, String> {
        let st = self.wstate();
        let rec = st["jobs"].as_array().unwrap().iter().find(|j| j["id"] == id).cloned().ok_or_else(|| format!("there is no job {id}"))?;
        let view = self.wallet_job(id)?;
        if view["can_cancel"] != true {
            return Err("it is being sent (or has ended): the wallet can't take it back now".to_owned());
        }
        wallet::names("cancel_request", json!([rec["request"]]))?;
        self.wallet_job(id)
    }

    // ── Redemption ──────────────────────────────────────────────────────

    /// Exit commitments from the wallet's `RFP022/exit` values; pieces burn from the private
    /// account the wallet gave this app (the wallet checks its balance when it simulates).
    pub(crate) fn names_plan_redeem(&mut self, token: &str, amount: &str, eth_to: &str, relay_index: &str) -> Result<Value, String> {
        let (t, row) = self.token_info(token)?;
        let value = util::parse_amount(amount, t.decimals)?;
        let to = util::eth_address(eth_to)?;
        if util::bad_checksum(eth_to) {
            return Err("that address's capital letters don't match its checksum: it probably has a typo. Copy it again".to_owned());
        }
        let d = self.deployment().clone();
        if to == [0; 20] || to == d.vault {
            return Err("the zero address and the vault itself can't receive a release".to_owned());
        }
        let relay_index: usize = relay_index.trim().parse().map_err(|_| "relay fee choice must be 0 (none) or a menu number".to_owned())?;
        let denoms: Vec<u128> = row["denoms"].as_array().unwrap().iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect();
        let s = crate::plan::split(value, &denoms)?;
        if s.pieces.is_empty() {
            return Err(format!("the smallest amount the bridge redeems is {} {}", format_amount(*denoms.iter().min().unwrap_or(&0), t.decimals), t.symbol));
        }
        if let Some(n) = self.wallet_gate(true)? {
            return Ok(n);
        }
        let acct = self.names_account("")?;
        let cfg = hex32(&d.cfg_id)?;
        let mut warnings: Vec<String> = vec!["Your Names Wallet checks that your account holds each piece when it simulates the burn.".to_owned()];
        if let Ok(eth) = self.eth() {
            if let Ok(n) = eth.code_len(&to) {
                if n > 0 {
                    warnings.push("This recipient is a contract. Make sure it can receive the token: the recipient is fixed when you burn.".to_owned());
                }
            }
        }
        let exits_possible = self.registry_now().map(|r| r["vault"]["exits_possible"] == true).unwrap_or(false);
        let st = self.wstate();
        let mut next_of: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        let mut pieces = Vec::new();
        let mut self_relay = false;
        for (i, count) in &s.pieces {
            let denom = denoms[*i];
            let menu: Vec<u128> = row["denoms"][*i]["relay_fee_menu"].as_array().map(|m| m.iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect()).unwrap_or_default();
            let relay_fee = if relay_index == 0 {
                self_relay = true;
                0
            } else {
                *menu.get(relay_index - 1).ok_or_else(|| format!("the relay-fee menu for {} {} has {} choice(s)", format_amount(denom, t.decimals), t.symbol, menu.len()))?
            };
            let withdraw_fee = units(row["denoms"][*i]["withdraw_fee"].as_str().unwrap_or("0")).unwrap_or(0);
            let key = format!("exit/{acct}/{}/{denom}/{}/{relay_fee}", eth::addr_hex(&t.eth_token), eth::addr_hex(&to));
            let start = *next_of.entry(key.clone()).or_insert_with(|| st["exit_next"][&key].as_u64().unwrap_or(0));
            let ctx = wallet::exit_context(&cfg, &t.eth_token, denom, &to, relay_fee);
            let values = match wallet::app_secret(&acct, wallet::SCOPE_EXIT, &ctx, wallet::TAG_EXIT, start, *count as u64)? {
                Values::Pending(req) => return Self::secret_needs(&req, wallet::SCOPE_EXIT),
                Values::Granted(v) => v,
            };
            for (o, k) in values.iter().enumerate() {
                let e = wallet::exit_commitment(k, &to, &t.eth_token, denom, relay_fee, &cfg);
                pieces.push(json!({
                    "denom_index": i, "denom": denom.to_string(), "denom_text": format_amount(denom, t.decimals),
                    "from": acct, "from_kind": "private", "relay_fee": relay_fee.to_string(), "relay_fee_text": format_amount(relay_fee, t.decimals),
                    "withdraw_fee": withdraw_fee.to_string(), "receive": (denom - withdraw_fee - relay_fee).to_string(),
                    "receive_text": format_amount(denom - withdraw_fee - relay_fee, t.decimals), "exit_commitment": hex0x(&e),
                    "release_nullifier": hex0x(&wallet::release_nullifier(k)), "key_index": start + o as u64, "index_key": key, "burned": false, "request": null,
                }));
            }
            next_of.insert(key, start + *count as u64);
        }
        let receive: u128 = pieces.iter().filter_map(|p| units(p["receive"].as_str()?).ok()).sum();
        let mut choices = json!({ "flow": "redeem" });
        if self_relay {
            choices["self_relay"] = Value::from(true);
        }
        if s.remainder > 0 {
            warnings.push(format!("{} {} is not a denomination and stays in your wallet.", format_amount(s.remainder, t.decimals), t.symbol));
        }
        let plan = json!({
            "id": format!("rdm-{}", now_ms()), "symbol": t.symbol, "eth_token": eth::addr_hex(&t.eth_token), "eth_to": util::checksum_address(&to), "pieces": pieces,
            "amount_text": format_amount(value - s.remainder, t.decimals), "receive_text": format_amount(receive, t.decimals), "relay_fee_index": relay_index,
            "warnings": warnings, "exits_possible": exits_possible, "wallet": "names",
            "blocked": if exits_possible { Value::Null } else { Value::from(format!("{} A burn approved in your wallet would destroy your wrapped tokens for good, so this app only builds the call and does not propose it.", crate::no_release_line(self.deployment()))) },
            "privacy": privacy::report(&choices)?, "created": now_ms(),
        });
        self.store().put_redeem(plan.clone())?;
        Ok(public_view(&plan))
    }

    /// Proposes the plan's next piece as a private call (one at a time: the wallet keeps at most
    /// 4 waiting requests per app). On a vault that can never release, builds it only.
    pub(crate) fn names_redeem(&mut self, plan_id: &str, options: &Value) -> Result<Value, String> {
        let mut plan = self.store().redeem(plan_id)?;
        let mut choices = plan["privacy"]["weakened_by"].as_array().cloned().unwrap_or_default().iter().fold(json!({ "flow": "redeem" }), |mut c, o| {
            if let Some(k) = o["key"].as_str() {
                c[k] = Value::from(true);
            }
            c
        });
        choices["confirm_weaker"] = Value::from(options["confirm_weaker"] == true);
        privacy::require_confirmation(&choices)?;
        if let Some(n) = self.wallet_gate(true)? {
            return Ok(n);
        }
        let pos = plan["pieces"].as_array().unwrap().iter().position(|p| p["request"].is_null()).ok_or("every piece of this plan was proposed already")?;
        let p = plan["pieces"][pos].clone();
        let d = self.deployment().clone();
        let definition = d.token(plan["symbol"].as_str().unwrap_or(""))?.definition.clone();
        let a: Vec<String> = [
            "burn", "--bridge", &d.bridge_program, "--token-program", &d.token_program, "--from", p["from"].as_str().unwrap_or_default(), "--definition", &definition,
            "--amount", p["denom"].as_str().unwrap_or_default(), "--exit", p["exit_commitment"].as_str().unwrap_or_default(), "--max-fee", &config::DEFAULT_MAX_FEE.to_string(),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let e = match self.emit(a, None, BURN_EMIT_PATIENCE)? {
            Ok(e) => e,
            Err(error) => return Err(error["message"].as_str().unwrap_or("the burn was refused before anything was proposed").to_owned()),
        };
        let call_summary = json!({ "programs": e.call["programs"], "accounts": e.call["accounts"].as_array().map_or(0, Vec::len),
                                   "message_bytes": e.result["message_bytes"], "declared": e.call["declared"], "fee_payer": e.call["fee_payer"] });
        if plan["exits_possible"] != true {
            return Ok(json!({ "proposed": false, "plan": plan_id, "piece": pos, "call": call_summary, "blocked": plan["blocked"] }));
        }
        let summary = format!("Redeem {} {} to {} (piece {} of {})", p["denom_text"].as_str().unwrap_or(""), plan["symbol"].as_str().unwrap_or(""), plan["eth_to"].as_str().unwrap_or(""), pos + 1, plan["pieces"].as_array().map_or(0, Vec::len));
        let r = match wallet::names_try("propose_private_call", json!([wallet::private_written(p["from"].as_str().unwrap_or_default()), e.call.to_string(), summary])) {
            Ok(r) => r,
            Err(wallet::Fail::NoAnswer) => {
                // It may be waiting in the Names app, and approved it burns: this piece is never
                // proposed again from this plan (a second burn would burn twice).
                plan["pieces"][pos]["request"] = Value::from("unanswered");
                plan["pieces"][pos]["unanswered_at"] = Value::from(now_ms());
                self.store().put_redeem(plan.clone())?;
                return Err("Your Names Wallet didn't answer in time, so this burn may be waiting in the Names app: approve or reject it there. This app can't follow it and won't propose this piece again (that could burn twice); if it never went out, make a new redemption plan for it.".to_owned());
            }
            Err(f) => return Err(f.text()),
        };
        plan["pieces"][pos]["request"] = r["request"].clone();
        self.store().put_redeem(plan.clone())?;
        let mut st = self.wstate();
        let key = p["index_key"].as_str().unwrap_or_default().to_owned();
        let next = p["key_index"].as_u64().unwrap_or(0) + 1;
        if st["exit_next"][&key].as_u64().unwrap_or(0) < next {
            st["exit_next"][&key] = Value::from(next);
            self.put_wstate(&st)?;
        }
        let mut job = self.new_wallet_job("redeem", plan_id, &r, json!({ "plan": plan_id, "piece": pos, "symbol": plan["symbol"], "amount_text": p["denom_text"] }))?;
        job["call"] = call_summary;
        Ok(job)
    }

    // ── Recovery ────────────────────────────────────────────────────────

    /// Deposits of `account` found again from the wallet's values alone: every registered token
    /// and denomination, batches of 64 `RFP022/deposit` values matched against the vault's
    /// `Deposit` events, until `gap` unused indexes follow the last used one. Found deposits are
    /// added to this computer's list (index only).
    pub(crate) fn names_recover(&mut self, account: &str, gap: u64) -> Result<Value, String> {
        let (acct, written, kind) = match self.receive_account(account)? {
            Ok(x) => x,
            Err(needs) => return Ok(needs),
        };
        let v = util::lez_account(&acct)?;
        let d = self.deployment().clone();
        let cfg = hex32(&d.cfg_id)?;
        let (events, _, _) = self.events()?;
        let reg = self.registry_now()?;
        let mut found = Vec::new();
        let mut calls = 0u64;
        let mut scanned = Vec::new();
        for t in &d.tokens {
            let Some(row) = reg["tokens"].as_array().and_then(|l| l.iter().find(|r| r["symbol"] == t.symbol.as_str()).cloned()) else { continue };
            let denoms: Vec<u128> = row["denoms"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect();
            for (di, denom) in denoms.iter().enumerate() {
                let ctx = wallet::deposit_context(&cfg, &t.eth_token, *denom);
                let (mut first, mut last): (u64, Option<u64>) = (0, None);
                loop {
                    let batch = match wallet::app_secret(&written, wallet::SCOPE_DEPOSIT, &ctx, wallet::TAG_DEPOSIT, first, wallet::BATCH)? {
                        Values::Pending(req) => return Self::secret_needs(&req, wallet::SCOPE_DEPOSIT),
                        Values::Granted(b) => b,
                    };
                    calls += 1;
                    for (o, k) in batch.iter().enumerate() {
                        let c = wallet::deposit_commitment(k, &v, &t.eth_token, *denom, &cfg);
                        if let Some(e) = events.iter().find(|e| e.commitment == c && e.token == t.eth_token && e.denom_index == di as u32) {
                            let index = first + o as u64;
                            last = Some(index);
                            let nf = (hex0x(&crate::relayed::claim_nullifier(k, &v, &t.eth_token, *denom)), hex0x(&crate::relayed::claim_nullifier_pre_fix(k)));
                            found.push((t.clone(), di, *denom, index, hex0x(&c), e.clone(), nf));
                        }
                    }
                    if last.is_some_and(|l| l + gap >= first + wallet::BATCH) {
                        first += wallet::BATCH;
                        continue;
                    }
                    break;
                }
                scanned.push(json!({ "symbol": t.symbol, "denom": denom.to_string(), "to_index": first + wallet::BATCH }));
            }
        }
        let store = self.store();
        let mut out = Vec::new();
        for (t, di, denom, index, c, e, nf) in found {
            let id = store::deposit_id(&c);
            let mut rec = store.deposit(&id).unwrap_or_else(|_| {
                let mut x = store::new_deposit(&eth::addr_hex(&t.eth_token), &t.symbol, di as u32, denom, &acct, kind, &c, "", "recovered");
                if let Some(o) = x.as_object_mut() {
                    o.remove("secret");
                }
                x
            });
            rec["key_index"] = Value::from(index);
            rec["derivation"] = Value::from("names-app");
            rec["nullifier"] = Value::from(nf.0);
            rec["nullifier_pre_fix"] = Value::from(nf.1);
            // Found in this deployment's vault: what `receive_request` rebuilds the code from (kept
            // only where the deposit has none, e.g. one made by an older build).
            if rec.get("vault").is_none() {
                rec["chain_id"] = Value::from(d.chain_id);
                rec["vault"] = Value::from(eth::addr_hex(&d.vault));
                rec["deployment"] = Value::from(d.name.clone());
            }
            store.put_deposit(rec.clone())?;
            let mut shown = public_view(&rec);
            shown["leaf_index"] = Value::from(e.leaf_index);
            shown["block"] = Value::from(e.block);
            out.push(shown);
        }
        Ok(json!({
            "account": acct, "gap_limit": gap, "scheme": "names-app:lez_bridge", "found": out, "scanned": scanned, "wallet_calls": calls,
            "seed_recovery": { "available": true, "reason": "Deposits are found again from your Names Wallet's values (restored with its recovery phrase) and the vault's public events; whether each is claimed shows when you open its claim." },
            "exits": { "available": false, "reason": "Burns are found by the lez-bridge program from your recovery phrase (lez-bridge --names-app lez_bridge recover --eth-dest 0x…): finding them needs the LEZ blocks, which this module does not scan yet." },
        }))
    }

    // ── Status ──────────────────────────────────────────────────────────

    pub(crate) fn names_status(&self) -> Result<Value, String> {
        let (connected, accounts) = self.granted()?;
        let mut v = json!({ "mode": "names", "connected": connected, "accounts": accounts, "token_source": { "accepted": false, "request": null } });
        if connected {
            match self.token_source(true) {
                Ok((accepted, req)) => v["token_source"] = json!({ "accepted": accepted, "request": req }),
                Err(e) => v["token_source"]["problem"] = Value::from(e),
            }
        }
        v["needs"] = if !connected {
            Value::from("connect")
        } else if !accounts.iter().any(|a| a["kind"] == "private") {
            Value::from("private_account")
        } else if v["token_source"]["accepted"] != true {
            Value::from("token_source")
        } else {
            Value::Null
        };
        Ok(v)
    }
}
