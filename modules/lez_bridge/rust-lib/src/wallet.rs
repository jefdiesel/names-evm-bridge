//! The Names Wallet, through the `names` core module's Wallet Provider SDK (Names branch
//! `bridge-sdk`, `docs/wallet/PROVIDER-SDK.md` and `BRIDGE-SDK.md` §5).
//!
//! Every call goes from THIS module: the wallet keys an app's values and requests to the
//! calling module's name (`current_caller()`), so the values `lez_bridge` gets are the ones
//! `lez-bridge --names-app lez_bridge recover` reproduces from the recovery phrase. The mini-app
//! (QML) never calls `names` for the bridge.
//!
//! What this module holds: the claim secrets `k` and exit secrets `k'` the wallet gives it
//! (`app_secret`, tags `RFP022/ck` and `RFP022/ek`), only in memory and in the 0600 files a CLI
//! call reads, never in a reply, a job record, a log or `positions.json` (deposits keep their
//! index). It never holds a key, a master secret `s`, and never proves: claims and burns go to
//! the wallet as `propose_private_call`, which the person approves in the Names app.

use crate::util::{sha256, to_hex};
use serde_json::{json, Value};
use std::time::Duration;

pub const SCOPE_DEPOSIT: &str = "RFP022/deposit";
pub const SCOPE_EXIT: &str = "RFP022/exit";
pub const TAG_DEPOSIT: &str = "RFP022/ck";
pub const TAG_EXIT: &str = "RFP022/ek";
/// Values one `app_secret` call returns at most.
pub const BATCH: u64 = 64;
/// Shown to the person when the module asks for access.
pub const ACCESS_REASON: &str = "To make deposit requests you can recover from your recovery phrase, and to claim and redeem bridged tokens privately from your account";

// The names module answers `busy` at once when it is in another call (single mode with try_lock
// since Names af520ba, in every 0.17.0 build), so a call either runs at once or is turned away
// at once. These waits bound everything else, in particular a names replica that never connects
// (names reloaded in a running Basecamp, 2026-11-14): each Names call gives up after its wait with
// "no answer" instead of holding this module. The module's own dispatch answers calls that arrive
// meanwhile `busy` (scaffold_patch.rs), so the wait can run out and the module is never stuck.
/// Reads and proposals that answer at once.
const QUICK: Duration = Duration::from_secs(8);
/// `app_secret` runs the names CLI (about a second).
const SECRET: Duration = Duration::from_secs(20);
/// A relayed call's build or re-sign (the wallet gives its CLI 12 s; Basecamp IPC adds its own).
const BUILD: Duration = Duration::from_secs(45);

/// After a `busy` answer the call is asked again after these waits (ms), within [`BUSY_CAP_MS`].
pub const BUSY_BACKOFF_MS: [u64; 4] = [2_000, 4_000, 6_000, 8_000];
/// No new attempt starts later than this after the first (each busy answer itself takes up to
/// 15 s, so in practice two or three attempts).
pub const BUSY_CAP_MS: u64 = 30_000;

/// What the person reads when the wallet stayed busy past the cap.
pub const BUSY_TEXT: &str = "Your Names Wallet is busy; nothing was sent; try again.";
/// The call got no answer at all: it may have been taken.
pub const NO_ANSWER_TEXT: &str = "Your Names Wallet didn't answer in time (it may be busy, or the Names app isn't installed or open). Nothing was sent; try again.";
/// The `names` module isn't there (not installed, not loaded): nothing was taken.
pub const UNREACHABLE_TEXT: &str = "Your Names Wallet isn't reachable here (is the Names app installed and open?). Nothing was sent; try again.";

/// Why a call to `names` gave no value.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// `{success:false, busy:true}`: the call waited its turn for 15 s and was NOT taken (the
    /// reply comes from the generated dispatch before any wallet code runs). Safe to ask again.
    Busy,
    /// No answer at all (a timeout, or the connection dropped mid-call): the call MAY have been
    /// taken. A proposal must not be asked again blindly.
    NoAnswer,
    /// The module isn't there or couldn't dispatch the call: nothing was taken.
    Unreachable,
    /// The wallet ran the call and refused it.
    Refused { error: String, code: Option<String> },
}

impl Fail {
    pub fn text(&self) -> String {
        match self {
            Fail::Busy => BUSY_TEXT.to_owned(),
            Fail::NoAnswer => NO_ANSWER_TEXT.to_owned(),
            Fail::Unreachable => UNREACHABLE_TEXT.to_owned(),
            Fail::Refused { error, .. } => error.clone(),
        }
    }

    /// A few words for a poll's note (the poll itself asks again).
    pub fn short(&self) -> String {
        match self {
            Fail::Busy => "it is busy".to_owned(),
            Fail::NoAnswer => "it didn't answer in time".to_owned(),
            Fail::Unreachable => "it isn't reachable (is the Names app open?)".to_owned(),
            Fail::Refused { error, .. } => error.clone(),
        }
    }

    /// Busy, no answer or unreachable: the wallet said nothing about the call itself.
    pub fn transient(&self) -> bool {
        !matches!(self, Fail::Refused { .. })
    }
}

fn wait_for(method: &str) -> Duration {
    match method {
        "app_secret" => SECRET,
        // The wallet builds (and re-signs) a relayed call with its CLI, nonces read from the chain.
        "propose_relayed_call" | "resign_relayed_call" => BUILD,
        _ => QUICK,
    }
}

/// One call, no retry: the raw answer, or why there is none.
#[cfg(not(test))]
fn call_raw(method: &str, args: &Value) -> Result<Value, Fail> {
    use logos_rust_sdk::{LogosError, LogosModuleSDK};
    match LogosModuleSDK::new().plugin("names").call_json_with_timeout(method, args, wait_for(method)) {
        Ok(v) => Ok(v),
        Err(LogosError::PluginCallFailed { message, .. }) => Err(transport_fail(&message)),
        // Refused before anything was sent (bad timeout, bad string, no client).
        Err(_) => Err(Fail::Unreachable),
    }
}

/// The SDK's error text for a failed `lp_invoke` carries the protocol's `{code, message, origin}`
/// (`logos_call_error.h`). Only codes that say the call never reached the wallet count as not
/// taken; a timeout, a dropped connection and anything unknown count as "may have been taken".
///
/// A provider that ran and rejected the call (the SDK's dispatch-rejection fold, e.g. a method an
/// older Names doesn't have) gives its plain message, not that JSON: a refusal, not taken.
pub fn transport_fail(message: &str) -> Fail {
    let code = serde_json::from_str::<Value>(message).ok().and_then(|v| v["code"].as_str().map(str::to_owned));
    let said = code.as_deref().unwrap_or(message);
    if ["object_unavailable", "call_failed", "Module not connected", "Module source unavailable", "Method not found"].iter().any(|c| said.contains(c)) {
        return Fail::Unreachable;
    }
    if code.is_none() && !message.trim().is_empty() && !message.starts_with("lp_invoke failed") && !message.contains("timeout") && !message.contains("timed out") {
        return Fail::Refused { error: message.to_owned(), code: None };
    }
    Fail::NoAnswer
}

/// The answer as a value: JSON text parsed (up to twice: Basecamp may serialise it again),
/// `busy` told apart from a refusal. `plain_ok`: an answer without a `success` field is the value
/// itself (`get_config` for another app).
fn decode(answer: Value, plain_ok: bool) -> Result<Value, Fail> {
    let mut answer = answer;
    for _ in 0..2 {
        match answer.as_str().map(serde_json::from_str::<Value>) {
            Some(Ok(parsed)) => answer = parsed,
            _ => break,
        }
    }
    if answer["busy"] == true {
        return Err(Fail::Busy);
    }
    if answer.get("success").is_none() && plain_ok && answer.is_object() {
        return Ok(answer);
    }
    if answer["success"] == true {
        Ok(answer["value"].clone())
    } else {
        Err(Fail::Refused {
            error: answer["error"].as_str().unwrap_or("the Names Wallet refused without saying why").to_owned(),
            code: answer["code"].as_str().map(str::to_owned),
        })
    }
}

/// Asks once; `busy` comes back as [`Fail::Busy`]. For calls a poll repeats anyway (the poll is
/// the retry, and the module doesn't sleep while the screen waits for it).
pub fn names_once(method: &str, args: Value) -> Result<Value, Fail> {
    names_inner(method, &args, false, false)
}

/// Asks, and asks again after a `busy` answer (2, 4, 6, 8 s, no new attempt after 30 s).
pub fn names_try(method: &str, args: Value) -> Result<Value, Fail> {
    names_inner(method, &args, true, false)
}

/// [`names_try`] with the reason as text: [`BUSY_TEXT`] after the cap.
pub fn names(method: &str, args: Value) -> Result<Value, String> {
    names_try(method, args).map_err(|f| f.text())
}

fn names_inner(method: &str, args: &Value, retry: bool, plain_ok: bool) -> Result<Value, Fail> {
    let start = clock::now_ms();
    let mut attempt = 0usize;
    loop {
        #[cfg(not(test))]
        let raw = call_raw(method, args);
        #[cfg(test)]
        let raw = stand_in::raw(method, args);
        let out = raw.and_then(|a| decode(a, plain_ok));
        match out {
            Err(Fail::Busy) if retry => {
                let wait = BUSY_BACKOFF_MS[attempt.min(BUSY_BACKOFF_MS.len() - 1)];
                if clock::now_ms().saturating_sub(start) + wait > BUSY_CAP_MS {
                    return Err(Fail::Busy);
                }
                clock::sleep_ms(wait);
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Time, real outside tests; a counter the tests read in them.
mod clock {
    #[cfg(not(test))]
    pub fn now_ms() -> u64 {
        crate::util::now_ms()
    }
    #[cfg(not(test))]
    pub fn sleep_ms(ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    #[cfg(test)]
    pub fn now_ms() -> u64 {
        super::stand_in::CLOCK.with(|c| *c.borrow())
    }
    #[cfg(test)]
    pub fn sleep_ms(ms: u64) {
        super::stand_in::SLEPT.with(|s| s.borrow_mut().push(ms));
        super::stand_in::CLOCK.with(|c| *c.borrow_mut() += ms);
    }
}

/// Unit tests: a stand-in `names` module that answers what a test set, and keeps what was asked.
#[cfg(test)]
pub mod stand_in {
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    /// Scripted: no answer at all (a timeout).
    pub const NO_ANSWER: &str = "__stand_in_no_answer__";
    /// Scripted: the module isn't there.
    pub const UNREACHABLE: &str = "__stand_in_unreachable__";

    thread_local! {
        /// Method -> answer (the module's `{success, value, error}`, as an object).
        pub static ANSWERS: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
        /// Method -> a function of the arguments (overrides ANSWERS).
        pub static HANDLERS: RefCell<HashMap<String, Box<dyn Fn(&Value) -> Value>>> = RefCell::new(HashMap::new());
        /// Method -> answers given first, in order (override HANDLERS and ANSWERS).
        pub static SCRIPT: RefCell<HashMap<String, VecDeque<Value>>> = RefCell::new(HashMap::new());
        pub static ASKED: RefCell<Vec<(String, Value)>> = const { RefCell::new(Vec::new()) };
        /// The fake clock (ms), the waits slept, and how long each call takes.
        pub static CLOCK: RefCell<u64> = const { RefCell::new(0) };
        pub static SLEPT: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
        pub static CALL_MS: RefCell<u64> = const { RefCell::new(0) };
    }

    pub fn raw(method: &str, args: &Value) -> Result<Value, super::Fail> {
        ASKED.with(|a| a.borrow_mut().push((method.to_owned(), args.clone())));
        let _ = super::wait_for(method);
        let took = CALL_MS.with(|c| *c.borrow());
        CLOCK.with(|c| *c.borrow_mut() += took);
        let scripted = SCRIPT.with(|s| s.borrow_mut().get_mut(method).and_then(VecDeque::pop_front));
        let answer = scripted
            .or_else(|| HANDLERS.with(|h| h.borrow().get(method).map(|f| f(args))))
            .or_else(|| ANSWERS.with(|a| a.borrow().get(method).cloned()))
            .ok_or_else(|| super::Fail::Refused { error: format!("stand-in names: nothing set for {method}: Method not found"), code: None })?;
        match answer.as_str() {
            Some(NO_ANSWER) => Err(super::Fail::NoAnswer),
            Some(UNREACHABLE) => Err(super::Fail::Unreachable),
            _ => Ok(answer),
        }
    }

    pub fn ask(method: &str, args: Value) -> Result<Value, String> {
        super::names(method, args)
    }

    /// The busy envelope, as the Names module sends it (JSON text).
    pub fn busy() -> Value {
        Value::from(json!({ "success": false, "busy": true, "value": null, "error": "The wallet is busy; still working." }).to_string())
    }

    /// `method` answers these first, in order.
    pub fn script(method: &str, answers: Vec<Value>) {
        SCRIPT.with(|s| s.borrow_mut().entry(method.to_owned()).or_default().extend(answers));
    }

    /// A refusal, as a value for [`script`].
    pub fn refuse_json(error: &str) -> Value {
        json!({ "success": false, "value": null, "error": error })
    }

    /// The answer `answer()` would set, as a value for [`script`].
    pub fn ok(value: Value) -> Value {
        json!({ "success": true, "value": value, "error": null })
    }

    pub fn call_ms(ms: u64) {
        CALL_MS.with(|c| *c.borrow_mut() = ms);
    }

    pub fn slept() -> Vec<u64> {
        SLEPT.with(|s| s.borrow().clone())
    }

    pub fn count(method: &str) -> usize {
        ASKED.with(|a| a.borrow().iter().filter(|(m, _)| m == method).count())
    }

    pub fn answer(method: &str, value: Value) {
        ANSWERS.with(|a| a.borrow_mut().insert(method.to_owned(), json!({ "success": true, "value": value, "error": null })));
    }

    pub fn refuse_with_code(method: &str, error: &str, code: &str) {
        ANSWERS.with(|a| a.borrow_mut().insert(method.to_owned(), json!({ "success": false, "value": null, "error": error, "code": code })));
    }

    #[allow(dead_code)]
    pub fn refuse(method: &str, error: &str) {
        ANSWERS.with(|a| a.borrow_mut().insert(method.to_owned(), json!({ "success": false, "value": null, "error": error })));
    }

    pub fn handle(method: &str, f: impl Fn(&Value) -> Value + 'static) {
        HANDLERS.with(|h| h.borrow_mut().insert(method.to_owned(), Box::new(f)));
    }

    pub fn asked() -> Vec<(String, Value)> {
        ASKED.with(|a| a.borrow().clone())
    }

    pub fn reset() {
        ANSWERS.with(|a| a.borrow_mut().clear());
        HANDLERS.with(|h| h.borrow_mut().clear());
        SCRIPT.with(|s| s.borrow_mut().clear());
        ASKED.with(|a| a.borrow_mut().clear());
        CLOCK.with(|c| *c.borrow_mut() = 0);
        SLEPT.with(|s| s.borrow_mut().clear());
        CALL_MS.with(|c| *c.borrow_mut() = 0);
        super::forget_features();
    }
}

/// The `names` method that takes options (`{"account_hint": "Public/<id>"}`), and the feature
/// name its `get_config` lists when it has it (Names after 5cb5e47). The hint only pre-ticks the
/// account in the wallet's access sheet.
pub const ACCESS_WITH: &str = "request_access_with";

thread_local! {
    /// The `features` the wallet's `get_config` listed, read once per session (this module runs
    /// one call at a time on one thread). Not kept when the read failed: asked again next time.
    static FEATURES: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// Whether the wallet lists `feature` in `get_config`'s `features` (the plain object, or the
/// `{success, value}` envelope Names answers other apps with, also as JSON text encoded twice).
/// Only a list that names the feature is kept for the session; a missing `features`, a list
/// without it or a failed read is "no" now and read again next time.
///
/// Names 0.17 (to 3434018 at least) gives other apps no `features` at all (its `public_config`),
/// so nothing that must work today depends on this: `request_access` and `add_account` ask
/// `request_access_with` directly and read the answer.
pub fn names_has(feature: &str) -> bool {
    if let Some(f) = FEATURES.with(|c| c.borrow().clone()) {
        if f.iter().any(|x| x == feature) {
            return true;
        }
    }
    let Ok(cfg) = names_config() else { return false };
    let list: Vec<String> = cfg["features"].as_array().or_else(|| cfg["value"]["features"].as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let has = list.iter().any(|x| x == feature);
    if has {
        FEATURES.with(|c| *c.borrow_mut() = Some(list));
    }
    has
}

/// Forgets the probed features (tests; and the stand-in's reset).
pub fn forget_features() {
    FEATURES.with(|c| *c.borrow_mut() = None);
}

/// The connect request, with the account to pre-tick when the wallet can take it:
/// `request_access_with(reason, {"account_hint": account})` on a wallet whose `get_config`
/// lists it, else `request_access(reason)` exactly as before. A refusal or a call not dispatched
/// is asked again the plain way (safe: the wallet gives back a connect request already waiting);
/// no answer at all is never asked again the other way (it may have been taken).
pub fn request_access(reason: &str, account: Option<&str>) -> Result<Value, Fail> {
    // Asked directly: Names doesn't list its features to other apps (names_has), and a wallet
    // without the method refuses it or doesn't dispatch it, which falls back below.
    if let Some(account) = account {
        match names_try(ACCESS_WITH, json!([reason, json!({ "account_hint": account }).to_string()])) {
            Ok(v) => return Ok(v),
            Err(Fail::Refused { .. } | Fail::Unreachable) => {}
            Err(f) => return Err(f),
        }
    }
    names_try("request_access", json!([reason]))
}

/// Names' feature (in `get_config().features`) that lets an app with access ask for one more
/// account: `request_access_with(reason, {account_hint})` then makes a request offering only that
/// account (Approve adds it and keeps the others).
pub const GRANT_ADD: &str = "grant_add_account";

/// An account the wallet hasn't given this app although it has access. With `grant_add_account`:
/// `Ok(Some(request))` for the wallet's request (state pending), `Ok(None)` when the wallet says
/// it is given already (approved, no request), `Err(the wallet's own words)` when it refuses (an
/// account the wallet doesn't hold), busy or no answer. Without the feature: the
/// revoke-and-connect-again error ([`account_not_given`]). No fallback to `request_access` here:
/// it makes no request for an app that has access.
pub fn add_account(reason: &str, written: &str) -> Result<Option<Value>, String> {
    // Asked directly, whatever `features` says (Names gives other apps none): the answer tells.
    let r = match names_try(ACCESS_WITH, json!([reason, json!({ "account_hint": written }).to_string()])) {
        Ok(r) => r,
        // A wallet without request_access_with (no such method, or not dispatched): it can't add.
        Err(Fail::Unreachable) => return Err(account_not_given(written)),
        Err(Fail::Refused { error, .. }) if is_unknown_method(&error) => return Err(account_not_given(written)),
        // Its own refusal (an account it doesn't hold, a bad hint): its words.
        Err(Fail::Refused { error, .. }) => return Err(error),
        Err(f) => return Err(f.text()),
    };
    if r["request"].as_str().is_some_and(|id| !id.trim().is_empty()) {
        return Ok(Some(r));
    }
    // "Approved" with no request: given already (a wallet with grant_add_account), or a wallet
    // that can't add one and made nothing. The grant itself tells which.
    if r["state"] == "approved" {
        let g = names_try("granted_accounts", json!([])).map_err(|f| f.text())?;
        if g["accounts"].as_array().is_some_and(|l| l.iter().any(|a| a["account"] == written)) {
            return Ok(None);
        }
        return Err(account_not_given(written));
    }
    Err(no_request_text("public_account"))
}

/// A refusal that says the method doesn't exist (an older Names), not a refusal of the call.
fn is_unknown_method(error: &str) -> bool {
    let e = error.to_lowercase();
    ["method not found", "unknown method", "no such method", "not a method", "unknown_method"].iter().any(|m| e.contains(m))
}

/// `get_config` for another app answers the public part of the configuration as plain JSON, not
/// in a `{success, value}` envelope (Names `public_config`): take either. Asked again when busy.
pub fn names_config() -> Result<Value, String> {
    names_inner("get_config", &json!([]), true, true).map_err(|f| f.text())
}

/// `Private/<id>` for a bare id or a written account.
pub fn private_written(account: &str) -> String {
    format!("Private/{}", account.trim().trim_start_matches("Private/").trim_start_matches("Public/"))
}

/// What an `app_secret` call gave.
pub enum Values {
    /// The person approved this (account, scope): the values, `first` onwards.
    Granted(Vec<[u8; 32]>),
    /// Waiting for the person in the Names app: the request.
    Pending(Value),
}

/// `app_secret(account, scope, context, tag, first, count)`; `count` at most [`BATCH`].
pub fn app_secret(account: &str, scope: &str, context: &[u8], tag: &str, first: u64, count: u64) -> Result<Values, String> {
    // A public account is asked as written (`Public/<id>`: its values come from its signing key);
    // anything else is a private account.
    let written = if account.trim().starts_with("Public/") { account.trim().to_owned() } else { private_written(account) };
    let v = names("app_secret", json!([written, scope, to_hex(context), tag, first, count.min(BATCH)]))?;
    if v["granted"] != true {
        return Ok(Values::Pending(v["request"].clone()));
    }
    let values: Vec<[u8; 32]> = v["values"]
        .as_array()
        .ok_or("the Names Wallet gave no values")?
        .iter()
        .map(|x| crate::util::hex32(x.as_str().unwrap_or_default()))
        .collect::<Result<_, _>>()?;
    if values.len() as u64 != count.min(BATCH) || v["first"].as_u64() != Some(first) {
        return Err("the Names Wallet gave other values than asked for".to_owned());
    }
    Ok(Values::Granted(values))
}

/// `context` of a deposit's values: `cfgId(32) | token(20) | u256(denom)` (lez-bridge
/// `client/src/kdf.rs`, "Names app scheme"; pinned by its `kdf-names-v1.json`).
pub fn deposit_context(cfg_id: &[u8; 32], token: &[u8; 20], denom: u128) -> Vec<u8> {
    [cfg_id.as_slice(), token, &u256(denom)].concat()
}

/// `context` of an exit's values: `cfgId | token | u256(denom) | ethDest(20) | u256(maxRelayFee)`.
pub fn exit_context(cfg_id: &[u8; 32], token: &[u8; 20], denom: u128, eth_dest: &[u8; 20], max_relay_fee: u128) -> Vec<u8> {
    [cfg_id.as_slice(), token, &u256(denom), eth_dest, &u256(max_relay_fee)].concat()
}

pub fn u256(v: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

/// The deposit commitment `C = SHA256("RFP022/dep" | k | account | token | u256(denom) | cfgId)`
/// (`bridge_core::deposit_commitment`).
pub fn deposit_commitment(k: &[u8; 32], account: &[u8; 32], token: &[u8; 20], denom: u128, cfg_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[b"RFP022/dep", k, account, token, &u256(denom), cfg_id])
}

/// The exit commitment `E = SHA256("RFP022/exit" | k' | ethDest | token | u256(denom) |
/// u256(maxRelayFee) | cfgId)` (`bridge_core::exit_commitment`).
pub fn exit_commitment(k: &[u8; 32], eth_dest: &[u8; 20], token: &[u8; 20], denom: u128, max_relay_fee: u128, cfg_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[b"RFP022/exit", k, eth_dest, token, &u256(denom), &u256(max_relay_fee), cfg_id])
}

/// The release nullifier `SHA256("RFP022/nfr" | k')`.
pub fn release_nullifier(k: &[u8; 32]) -> [u8; 32] {
    sha256(&[b"RFP022/nfr", k])
}

/// The phases a wallet job shows, in order: the person's approval first, then the wallet's.
pub const PHASES: [&str; 6] = ["approve", "simulate", "prove", "submit", "confirm", "done"];

/// A wallet request (`request_status`) as one of this module's jobs. The wallet's own check
/// result and errors stay in the Names app: a failed or refused check shows only "see the
/// Names app".
pub fn job_view(record: &Value, status: Option<&Value>, now: u64) -> Value {
    let mut job = record.clone();
    let s = status.cloned().unwrap_or(Value::Null);
    let state_now = s["state"].as_str().unwrap_or(record["state"].as_str().unwrap_or("pending"));
    let check = s["check"].as_str().unwrap_or("");
    let check_failed = matches!(check, "failed" | "refused");
    let phase = match state_now {
        "pending" => "approve",
        "approved" | "running" => match s["phase"].as_str() {
            Some("plan") | Some("simulate") | None => "simulate",
            Some(p) => p,
        },
        "done" => "done",
        _ => record["phase"].as_str().unwrap_or("approve"),
    };
    let state = match state_now {
        "done" => "done",
        "failed" => "failed",
        "rejected" | "expired" | "cancelled" => "cancelled",
        _ if check_failed => "failed",
        _ => "running",
    };
    // The UI's error shape: `{code, message}` (codes titled in the mini-app's logic.mjs).
    let error = if check_failed {
        json!({ "code": "wallet_check", "message": "The Names Wallet's check stopped this call. Open the Names app to see why." })
    } else {
        match state_now {
            "failed" => json!({ "code": "wallet_failed", "message": format!("The Names Wallet says: {}. Open the Names app for the details.", s["error"].as_str().unwrap_or("it failed")) }),
            "rejected" => json!({ "code": "wallet_rejected", "message": "You rejected it in the Names app; nothing was signed or sent." }),
            "expired" => json!({ "code": "wallet_rejected", "message": "Nobody answered it in the Names app within 10 minutes; nothing was signed or sent." }),
            "cancelled" => json!({ "code": "wallet_rejected", "message": "Cancelled; nothing was signed or sent." }),
            _ => Value::Null,
        }
    };
    let idx = PHASES.iter().position(|p| *p == phase).unwrap_or(0);
    job["state"] = Value::from(state);
    job["phase"] = Value::from(phase);
    // The phases reached so far, as the CLI jobs list them.
    job["phases"] = Value::from(PHASES.iter().take(idx + 1).map(|p| json!({ "phase": p })).collect::<Vec<_>>());
    job["log_tail"] = Value::from("");
    job["peak_rss_mb"] = Value::from(0);
    job["peak_estimate_mb"] = Value::from(crate::config::CLAIM_PROOF_PEAK_MB);
    job["phase_elapsed_s"] = Value::from(record["prove_since"].as_u64().map_or(0, |t| now.saturating_sub(t) / 1000));
    job["wallet_request"] = json!({ "state": state_now, "check": s["check"], "phase": s["phase"] });
    job["error"] = error;
    job["result"] = if state == "done" { json!({ "tx": s["tx"], "ok": true }) } else { Value::Null };
    job["elapsed_s"] = Value::from(now.saturating_sub(record["started"].as_u64().unwrap_or(now)) / 1000);
    job["can_pause"] = Value::from(false);
    job["can_resume"] = Value::from(false);
    // The wallet stops a cancel once the call is being sent.
    job["can_cancel"] = Value::from(state == "running" && !matches!(phase, "submit" | "confirm" | "done"));
    job["dry_run"] = Value::from(false);
    job["wallet"] = Value::from("names");
    job["note"] = if state != "running" { Value::Null } else { Value::from(match phase {
        "approve" if check == "ready" => "Waiting for you in the Names app: it shows what this call does, simulated by your wallet.",
        "approve" => "Your Names Wallet is checking the call; then the Names app asks you to approve it.",
        "prove" => "Your wallet is proving it on this computer (about 11 minutes, about 9.5 GB of memory).",
        "done" => "Included in a block.",
        _ => "Your wallet is running it.",
    }) };
    if state != "running" {
        job["finished"] = record["finished"].clone();
        if job["finished"].is_null() {
            job["finished"] = Value::from(now);
        }
    }
    job
}

/// The token-source definitions the bridge proposes (`propose_token_source`): from the bridge
/// config the CLI read (`show --json`), so the decimals are the bridge's own.
pub fn token_definitions(show: &Value, deployment: &crate::config::Deployment) -> Result<Value, String> {
    let chain = if deployment.chain_id == 11_155_111 { "Sepolia".to_owned() } else { format!("chain {}", deployment.chain_id) };
    let mut out = Vec::new();
    for t in show["tokens"].as_array().ok_or("the bridge config lists no tokens")? {
        let definition = t["definition"].as_str().ok_or("a token without a definition")?;
        let decimals = t["decimals"].as_u64().ok_or("the CLI's bridge config has no decimals (needs a lez-bridge with --json)")?;
        let eth = t["eth_token"].as_str().unwrap_or_default();
        let symbol = deployment.tokens.iter().find(|d| d.definition == definition).map_or_else(|| "?".to_owned(), |d| d.symbol.clone());
        let origin = if eth.trim_start_matches("0x").chars().all(|c| c == '0') {
            format!("{chain} native ETH")
        } else {
            format!("{chain} {}", crate::util::eth_address(eth).map(|a| crate::util::checksum_address(&a)).unwrap_or_else(|_| eth.to_owned()))
        };
        out.push(json!({ "definition": definition, "name": format!("{symbol} (bridged)"), "symbol": symbol, "decimals": decimals, "origin": origin }));
    }
    if out.is_empty() {
        return Err("the bridge has no registered tokens".to_owned());
    }
    Ok(Value::from(out))
}

/// The `needs` reply: what the person must do in the Names app before this can go on.
/// What the person must approve in the Names app first: `{needs, request, message}`, only for a
/// request the wallet really made (its `request` id). Without one there is no sheet to approve, so
/// it is an error saying what to do instead, never a `needs` the screen would wait on for ever
/// (2026-11-14: the wallet answers `request_access` of an app that already has access with
/// `{request: null, state: "approved"}` and makes no request).
pub fn needs(what: &str, request: &Value, message: &str) -> Result<Value, String> {
    match request["request"].as_str().map(str::trim) {
        Some(id) if !id.is_empty() => Ok(json!({ "needs": what, "request": request, "message": message })),
        _ => Err(no_request_text(what)),
    }
}

/// Why there is nothing to approve although the module asked the wallet for something.
pub fn no_request_text(what: &str) -> String {
    match what {
        "connect" | "public_account" | "private_account" => "Your Names Wallet made no request for this app to approve: it may already have given this app access, but not to the account this needs. In the Names app, take this app's access back (Settings, Connected apps), then try again and give it that account. Nothing was sent.".to_owned(),
        _ => "Your Names Wallet made no request to approve; nothing was asked or sent. Open the Names app, then try again.".to_owned(),
    }
}

/// The error for an account the wallet didn't give this app although it already has access: the
/// wallet can't add an account to an existing grant, so the person takes the access back and
/// connects again with it.
pub fn account_not_given(written: &str) -> String {
    format!("This app has access to your Names Wallet, but not to {written}. The wallet can't add an account to that access: in the Names app, take this app's access back (Settings, Connected apps), then try again and give it {written}. If {written} isn't in your wallet, this deposit is for someone else's account. Nothing was sent.")
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex0x;

    /// `busy:true`, as Names sends it (JSON text), once more encoded by Basecamp, or as an object:
    /// always busy, never a refusal.
    #[test]
    fn busy_is_told_apart_from_a_refusal_however_it_is_encoded() {
        let envelope = json!({ "success": false, "busy": true, "value": null, "error": "The wallet is busy; still working." });
        let once = Value::from(envelope.to_string());
        let twice = Value::from(once.to_string());
        for a in [envelope.clone(), once, twice] {
            assert_eq!(decode(a.clone(), false), Err(Fail::Busy), "{a}");
            assert_eq!(decode(a, true), Err(Fail::Busy));
        }
        let refused = json!({ "success": false, "value": null, "error": "no", "code": "refused" });
        assert_eq!(decode(refused, false), Err(Fail::Refused { error: "no".into(), code: Some("refused".into()) }));
        assert_eq!(decode(Value::from(json!({ "names_network": "lez-0.4" }).to_string()), true).unwrap()["names_network"], "lez-0.4");
        // Transport errors: only "never reached the wallet" counts as not taken.
        assert_eq!(transport_fail(r#"{"code":"timeout","message":"call to 'names.propose_relayed_call' timed out after 90000ms","origin":"names"}"#), Fail::NoAnswer);
        assert_eq!(transport_fail(r#"{"code":"transport_error","message":"x","origin":"names"}"#), Fail::NoAnswer);
        assert_eq!(transport_fail(r#"{"code":"object_unavailable","message":"x","origin":"names"}"#), Fail::Unreachable);
        assert_eq!(transport_fail("lp_invoke failed with code -3"), Fail::NoAnswer, "unknown: may have been taken");
        assert_eq!(transport_fail("unknown method 'resign_relayed_call'"), Fail::Refused { error: "unknown method 'resign_relayed_call'".into(), code: None }, "a dispatch rejection ran and refused");
        assert!(Fail::Unreachable.text().contains("isn't reachable") && !Fail::Unreachable.text().contains("busy"));
        assert!(Fail::Busy.transient() && Fail::NoAnswer.transient() && !Fail::Refused { error: String::new(), code: None }.transient());
        assert_eq!(Fail::Busy.text(), "Your Names Wallet is busy; nothing was sent; try again.");
    }

    /// The access hint goes to request_access_with directly (Names lists no features to other
    /// apps); a wallet without it falls back to request_access; no answer never falls back.
    #[test]
    fn the_account_hint_is_asked_directly_and_falls_back_when_the_method_is_missing() {
        let pending = json!({ "request": "acc-1", "state": "pending" });
        // A wallet with the method: one call, with the hint; no get_config read needed.
        stand_in::reset();
        stand_in::answer("request_access_with", pending.clone());
        assert_eq!(request_access("why", Some("Public/abc")).unwrap()["request"], "acc-1");
        assert_eq!(stand_in::asked(), vec![("request_access_with".to_owned(), json!(["why", "{\"account_hint\":\"Public/abc\"}"]))]);
        // Without it (not dispatched, or refused as an unknown method): today's call.
        for missing in [Value::from(stand_in::UNREACHABLE), stand_in::refuse_json("Method not found: request_access_with")] {
            stand_in::reset();
            stand_in::script("request_access_with", vec![missing]);
            stand_in::answer("request_access", pending.clone());
            assert_eq!(request_access("why", Some("Public/abc")).unwrap()["request"], "acc-1");
            assert_eq!(stand_in::asked().last().unwrap(), &("request_access".to_owned(), json!(["why"])));
        }
        // No account to hint: today's call only.
        stand_in::reset();
        stand_in::answer("request_access", pending);
        request_access("why", None).unwrap();
        assert_eq!(stand_in::asked(), vec![("request_access".to_owned(), json!(["why"]))]);
        // No answer at all: never asked again the other way.
        stand_in::reset();
        stand_in::script("request_access_with", vec![Value::from(stand_in::NO_ANSWER)]);
        assert_eq!(request_access("why", Some("Public/abc")), Err(Fail::NoAnswer));
        assert_eq!(stand_in::count("request_access"), 0);
    }

    /// Names answers another app's get_config in the envelope, as JSON text (and Basecamp may
    /// encode it again); `features` is read from `value` then. Only a positive read is kept.
    #[test]
    fn features_are_read_through_the_envelope_and_a_negative_read_is_not_kept() {
        stand_in::reset();
        // As Names 3434018 answers another app: {success, value: public_config, error}, no features.
        let public = json!({ "sequencer": "https://lez.nameswallet.app/", "names_sequencer": null, "registry": "x", "token_program": null,
                             "zone": { "id": null, "channel": "c", "home": true }, "module_version": "0.17.0", "names_network": "lez-0.4" });
        stand_in::answer("get_config", public.clone());
        assert!(!names_has(GRANT_ADD));
        // A later wallet that lists them, encoded twice: read again (the "no" was not kept), found.
        let mut with = public;
        with["features"] = json!(["request_access_with", "grant_add_account"]);
        let envelope = json!({ "success": true, "value": with, "error": null }).to_string();
        stand_in::script("get_config", vec![Value::from(Value::from(envelope).to_string())]);
        assert!(names_has(GRANT_ADD));
        let reads = stand_in::count("get_config");
        assert!(names_has(ACCESS_WITH));
        assert_eq!(stand_in::count("get_config"), reads, "a positive read is kept for the session");
    }

    /// Busy, busy, then an answer: the same call asked three times after 2 and 4 s, one value.
    #[test]
    fn a_busy_wallet_is_asked_again_after_short_waits() {
        stand_in::reset();
        stand_in::script("request_access", vec![stand_in::busy(), stand_in::busy()]);
        stand_in::answer("request_access", json!({ "request": "acc-1", "state": "pending" }));
        assert_eq!(names("request_access", json!(["why"])).unwrap()["request"], "acc-1");
        assert_eq!(stand_in::count("request_access"), 3);
        assert_eq!(stand_in::slept(), vec![2_000, 4_000]);
        // `get_config` (plain JSON for another app) and the coded call take the same path.
        stand_in::script("get_config", vec![stand_in::busy()]);
        stand_in::answer("get_config", json!({ "names_network": "lez-0.4" }));
        assert_eq!(names_config().unwrap()["names_network"], "lez-0.4");
        stand_in::script("resign_relayed_call", vec![stand_in::busy()]);
        stand_in::refuse_with_code("resign_relayed_call", "hasn't moved", "unchanged");
        assert_eq!(names_try("resign_relayed_call", json!(["r"])), Err(Fail::Refused { error: "hasn't moved".into(), code: Some("unchanged".into()) }));
        // A poll asks once.
        stand_in::script("request_status", vec![stand_in::busy()]);
        assert_eq!(names_once("request_status", json!(["r"])), Err(Fail::Busy));
    }

    /// Busy past the cap: "busy; nothing was sent; try again", no attempt started after 30 s.
    #[test]
    fn a_wallet_busy_past_the_cap_says_busy_not_missing() {
        stand_in::reset();
        stand_in::handle("declare_programs", |_| stand_in::busy());
        assert_eq!(names("declare_programs", json!(["[]", "why"])).unwrap_err(), BUSY_TEXT);
        assert_eq!(stand_in::slept(), vec![2_000, 4_000, 6_000, 8_000, 8_000]);
        assert!(stand_in::slept().iter().sum::<u64>() <= BUSY_CAP_MS);
        // Each busy answer takes 15 s in the real module: two attempts, then the message.
        stand_in::reset();
        stand_in::call_ms(15_000);
        stand_in::handle("declare_programs", |_| stand_in::busy());
        assert_eq!(names("declare_programs", json!(["[]", "why"])).unwrap_err(), BUSY_TEXT);
        assert_eq!(stand_in::count("declare_programs"), 2);
        // Not reachable is not "busy", and nothing is asked twice.
        stand_in::reset();
        stand_in::script("granted_accounts", vec![Value::from(stand_in::UNREACHABLE)]);
        let e = names("granted_accounts", json!([])).unwrap_err();
        assert!(e.contains("isn't reachable") && !e.contains("busy"), "{e}");
        assert_eq!(stand_in::count("granted_accounts"), 1);
        // No answer at all is never retried here (it may have been taken).
        stand_in::script("propose_relayed_call", vec![Value::from(stand_in::NO_ANSWER)]);
        assert_eq!(names_try("propose_relayed_call", json!([])), Err(Fail::NoAnswer));
        assert_eq!(stand_in::count("propose_relayed_call"), 1);
        assert!(stand_in::slept().is_empty());
    }

    /// The commitment formulas against lez-bridge `client/tests/vectors/kdf-names-v1.json`
    /// (Python, independent): case "all bytes 0x11..".
    #[test]
    fn formulas_match_the_client_vectors() {
        let k = crate::util::hex32("0x0a1f4d6e9a790065ea8bf49cef246799abcdcb14805b24748b2267483d6268bc").unwrap();
        let c = deposit_commitment(&k, &[0xA1; 32], &[0xAA; 20], 100_000_000, &[0xC0; 32]);
        assert_eq!(hex0x(&c), "0x027f057a1f83a6f4270e4aa16ddcd62c2476f33d77ed700bb8a7cc2104f9b8a5");
        let ctx = deposit_context(&[0xC0; 32], &[0xAA; 20], 100_000_000);
        assert_eq!(to_hex(&ctx), "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0000000000000000000000000000000000000000000000000000000005f5e100");
        let k2 = crate::util::hex32("0x7efacc826dc0703ed13346aa7daa60ba3e034419abc4d165949507ca58a82693").unwrap();
        let e = exit_commitment(&k2, &[0xBE; 20], &[0xAA; 20], 100_000_000, 2_000_000, &[0xC0; 32]);
        assert_eq!(hex0x(&e), "0x1dd2fce4b5c653e03deca4c53a2fc34087cb1f3f7b53a128abaa33a64b3decf3");
        assert_eq!(hex0x(&release_nullifier(&k2)), "0xd4e7b8dc19cb2d6d1abae112fa164853bd55ff5ad9d8ed84d2cc93bff6cc51a3");
        assert_eq!(exit_context(&[0xC0; 32], &[0xAA; 20], 100_000_000, &[0xBE; 20], 2_000_000).len(), 136);
    }

    #[test]
    fn a_failed_check_says_only_see_the_names_app() {
        let rec = json!({ "id": "wr-1", "kind": "claim", "started": 1000, "state": "running", "phase": "approve" });
        for check in ["failed", "refused"] {
            let st = json!({ "state": "pending", "check": check, "error": "V holds 3 dUSDC; the wallet's diagnosis" });
            let j = job_view(&rec, Some(&st), 5000);
            assert_eq!(j["state"], "failed");
            assert_eq!(j["error"]["code"], "wallet_check");
            let shown = j.to_string();
            assert!(shown.contains("Open the Names app"), "{shown}");
            assert!(!shown.contains("diagnosis") && !shown.contains("3 dUSDC"), "{shown}");
        }
        let ready = job_view(&rec, Some(&json!({ "state": "pending", "check": "ready" })), 5000);
        assert_eq!((ready["state"].as_str(), ready["phase"].as_str()), (Some("running"), Some("approve")));
        assert_eq!(ready["can_cancel"], true);
        assert_eq!(ready["can_pause"], false);
        let proving = job_view(&rec, Some(&json!({ "state": "running", "check": "ready", "phase": "prove" })), 5000);
        assert_eq!(proving["phase"], "prove");
        // Reached phases only, in order, as the CLI's jobs list them.
        assert_eq!(proving["phases"], json!([{ "phase": "approve" }, { "phase": "simulate" }, { "phase": "prove" }]));
        assert_eq!(proving["can_cancel"], true);
        let sending = job_view(&rec, Some(&json!({ "state": "running", "check": "ready", "phase": "submit" })), 5000);
        assert_eq!(sending["can_cancel"], false);
        let done = job_view(&rec, Some(&json!({ "state": "done", "tx": "abc" })), 5000);
        assert_eq!((done["state"].as_str(), done["result"]["tx"].as_str()), (Some("done"), Some("abc")));
        let rejected = job_view(&rec, Some(&json!({ "state": "rejected" })), 5000);
        assert_eq!(rejected["state"], "cancelled");
    }
}
