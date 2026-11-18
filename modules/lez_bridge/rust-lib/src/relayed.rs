// SPDX-License-Identifier: MIT OR Apache-2.0
//! "Claim a deposit" into a PUBLIC account of the Names Wallet, with no LEZ in it: the wallet
//! signs the claim; the bridge's relay sends it and pays the fee.
//!
//! 1. `relayed_claim_check`: the claim file the deposit page saved (pasted, or a path from the
//!    file picker) is checked as strictly as the CLI and the page check it (format, chain, vault,
//!    cfgId, token and amount, the commitment recomputed from the claim key). Then, reading only:
//!    the bridge's Config on LEZ (fail-closed decode), the deposit's leaf from the vault's
//!    `Deposit` logs (finalized blocks only; every rebuilt root checked against the vault's own),
//!    a root the bridge has recorded (its PDA), the nullifier unspent (its PDA), the relay's payer
//!    and caps (`bridge_relayInfo`) and the head fees. It builds the call the wallet will sign,
//!    exactly `lez-bridge claim --relay`'s transaction (golden vector
//!    `testdata/relayed-claim-v1.json`, also checked by the CLI and by `bridge_core`). Nothing
//!    is proposed, signed or sent: this is the dry run.
//! 2. `relayed_claim`: the wallet gates (connected, this public account given to this app, the
//!    bridge and token programs declared, LEZ 0.4 on the same chain), then
//!    `propose_relayed_call`. The person approves the wallet's own sheet in the Names app.
//! 3. `relayed_claim_status` moves the job on each call: the half-signed transaction from
//!    `request_status` goes to the relay (`bridge_relayClaim`, in a background thread); a relay
//!    whose payer had another claim in flight ("stale nonce") gets the SAME call re-signed with
//!    `resign_relayed_call` (no second approval; the wallet allows that only when nothing but the
//!    payer's nonce moved, 3 times within 10 minutes) and then a new proposal; then the block,
//!    the nullifier and the account's wrapped balance.
//!
//! The claim key (and the file's master secret) live only in this process's memory: the checked
//! claim's key field is overwritten when it is dropped; the kept call and the signed transaction,
//! which carry the key too, are freed but not overwritten. They are in no reply, no log and no file of
//! this module; the claim message that carries the key goes to the wallet, which keeps it in its
//! own 0600 store, and the half-signed transaction it gives back is held in memory until the
//! relay has it. Kept on disk (`wallet.json`, 0600): the job's public facts only.

use crate::config::Deployment;
use crate::eth::{self, DepositEvent};
use crate::lez::{self, LezRpc, Problem, Relayed};
use crate::util::{self, eth_address, format_amount, hex0x, now_ms, sha256, to_hex};
use crate::wallet;
use crate::Bridge;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const CLAIM_FILE_FORMAT: &str = "logos-bridge-claim";
/// The words every screen and the wallet's request use.
pub const WORDING: &str = "The wallet signs the claim; the bridge's relay sends it and pays the fee.";
pub const PRIVACY_LINE: &str = "A public claim publishes the claim secret and links the Sepolia deposit to this account.";
/// The "Not private" notice for a deposit into a public account (shown before the person
/// deposits; `config().notices.not_private` gives it with no account, before any request).
pub fn not_private_notice(account: Option<&str>) -> Value {
    let to = account.map_or_else(|| "this account".to_owned(), str::to_owned);
    json!({ "title": "Not private", "text": format!("{PRIVACY_LINE} Claiming into a public account publishes the deposit's claim secret on LEZ and links your Sepolia deposit to {to} for good. A private account keeps them apart (its claim needs a proof).") })
}

pub const ACCESS_REASON: &str = "To claim bridge deposits into your public account: the wallet signs each claim, the bridge's relay sends it and pays the fee.";
pub const DECLARE_REASON: &str = "The LEZ bridge (RFP-022) claims a Sepolia deposit by calling its bridge program, which mints through its token program. The bridge program is not in the Program Registry yet.";
/// Largest claim file read (the page's are about 1.5 KB).
const MAX_FILE: u64 = 64 * 1024;
/// Roots tried, newest first, when the newest finalized one isn't recorded yet.
const MAX_ROOT_TRIES: usize = 8;
/// Bytes a co-signed claim adds to its message on the wire (two witnesses, actors, nonces, fee):
/// a 1,175-byte claim message gave about 1.9 KB on rc1 and rc3 (relay.rs `MAX_TX_BYTES` note).
const WIRE_OVERHEAD: u64 = 900;
/// How long a submitted claim may stay out of a block before the chain is asked why.
const CONFIRM_PATIENCE_MS: u64 = 75_000;
/// A relay that is busy (HTTP 503) is asked again this often, this many times.
const BUSY_WAIT_MS: u64 = 10_000;
const MAX_BUSY: u32 = 30;
pub const TREE_DEPTH: usize = 32;
/// A wallet request lives 10 minutes (Names `REQUEST_LIFETIME`); a proposal that got no answer is
/// treated as possibly waiting there this long, with a minute's margin.
pub const UNCERTAIN_MS: u64 = 11 * 60 * 1000;
/// `relayed_claim`'s option that says the person looked in the Names app after a proposal got no
/// answer and wants it proposed again.
pub const AFTER_UNCERTAIN: &str = "after_uncertain";

// ── The claim file ─────────────────────────────────────────────────────

/// A refusal with a code the mini-app titles (`logic.mjs` `RELAY_TITLES`).
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

pub fn refuse<T>(code: &'static str, message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal { code, message: message.into() })
}

impl Refusal {
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

impl From<Problem> for Refusal {
    fn from(p: Problem) -> Self {
        Refusal { code: p.code, message: p.message }
    }
}

/// A checked claim file. The claim key and master secret are wiped on drop.
pub struct ClaimFile {
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub cfg_id: [u8; 32],
    pub token: [u8; 20],
    pub denom_index: u32,
    pub amount: u128,
    pub account: [u8; 32],
    pub commitment: [u8; 32],
    pub leaf_index: Option<u64>,
    pub deposit_tx: Option<String>,
    pub had_master: bool,
    key: [u8; 32],
}

impl Drop for ClaimFile {
    fn drop(&mut self) {
        // Volatile, so the wipe isn't optimised away.
        for b in self.key.iter_mut() {
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

impl ClaimFile {
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }
    pub fn account_text(&self) -> String {
        lez::b58(&self.account)
    }
}

fn hex_n<const N: usize>(v: &Value, what: &str) -> Result<[u8; N], Refusal> {
    let t = v.as_str().map(str::trim).unwrap_or_default();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    if t.len() != 2 * N || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return refuse("bad_file", format!("the claim file's {what} is not {N} bytes of hex"));
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&t[2 * i..2 * i + 2], 16).unwrap();
    }
    Ok(out)
}

fn whole(v: &Value, what: &str) -> Result<u128, Refusal> {
    let t = match v {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return refuse("bad_file", format!("the claim file has no {what}")),
    };
    t.parse::<u128>().or_else(|_| refuse("bad_file", format!("the claim file's {what} is not a whole number")))
}

/// The account a claim file names: `Public/<id>`, `Private/<id>` or a bare id (the page's
/// `parseLogosAccount`). A private account is refused here with where to go instead.
pub fn file_account(text: &str) -> Result<[u8; 32], Refusal> {
    let t: String = text.chars().filter(|c| !c.is_whitespace() && !matches!(c, '\u{200b}'..='\u{200d}' | '\u{feff}')).collect();
    let (label, bare) = match t.split_once('/') {
        Some((l, b)) => (l.to_ascii_lowercase(), b.to_owned()),
        None => (String::new(), t.clone()),
    };
    if label == "private" {
        return refuse(
            "private_account",
            "This deposit is for a private account. A private claim is proven by your Names Wallet: open it from Deposits & claims (Claim privately). This public claim can't take it.",
        );
    }
    if !(label.is_empty() || label == "public") {
        return refuse("bad_file", "the claim file's logosAccount is not a LEZ account");
    }
    util::lez_account(&bare).or_else(|_| refuse("bad_file", "the claim file's logosAccount is not a LEZ account (32 bytes of base58)"))
}

/// Reads a claim file given as its JSON text or as a path (a file picker gives `file://…`).
pub fn read_source(source: &str) -> Result<String, Refusal> {
    let s = source.trim();
    if s.starts_with('{') {
        return Ok(s.to_owned());
    }
    let path = s.strip_prefix("file://").unwrap_or(s);
    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return refuse("bad_file", "paste the claim file's text, or choose the file");
    }
    let meta = std::fs::metadata(path).or_else(|_| refuse("bad_file", format!("there is no file at {}", path.display())))?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return refuse("bad_file", "that is not a claim file (too large, or not a file)");
    }
    std::fs::read_to_string(path).or_else(|_| refuse("bad_file", "the claim file can't be read"))
}

/// Parses and checks a claim file against the deployment (page `verifyClaimFile`, CLI
/// `claim-own.sh` step 1): the denomination is checked later, against the bridge's own Config.
pub fn parse_claim_file(text: &str, d: &Deployment) -> Result<ClaimFile, Refusal> {
    let f: Value = serde_json::from_str(text).or_else(|_| refuse("bad_file", "that is not a claim file (not JSON)"))?;
    for key in ["format", "version", "chainId", "vault", "cfgId", "token", "denominationIndex", "amount", "logosAccount", "claimKey", "commitment"] {
        if f.get(key).is_none_or(Value::is_null) {
            return refuse("bad_file", format!("the claim file has no {key}"));
        }
    }
    if f["format"] != CLAIM_FILE_FORMAT || f["version"] != 1 {
        return refuse("bad_file", "that is not a bridge claim file (unknown format or version)");
    }
    let chain_id = u64::try_from(whole(&f["chainId"], "chainId")?).or_else(|_| refuse("bad_file", "the claim file's chainId is too large"))?;
    if chain_id != d.chain_id {
        return refuse("other_chain", format!("This deposit was made on chain {chain_id}; this bridge takes deposits from chain {} ({}).", d.chain_id, d.label));
    }
    let vault = eth_address(f["vault"].as_str().unwrap_or_default()).or_else(|_| refuse("bad_file", "the claim file's vault is not an address"))?;
    if vault != d.vault {
        return refuse("other_vault", format!("This deposit went to another vault ({}); this bridge claims deposits of {}.", util::checksum_address(&vault), util::checksum_address(&d.vault)));
    }
    let cfg_id = hex_n::<32>(&f["cfgId"], "cfgId")?;
    if hex0x(&cfg_id) != d.cfg_id.to_lowercase() {
        return refuse("cfg_mismatch", "This deposit was made for another bridge deployment (its cfgId differs); this bridge can't claim it.");
    }
    let token_text = f["token"]["address"].as_str().or_else(|| f["token"].as_str()).unwrap_or_default();
    let token = eth_address(token_text).or_else(|_| refuse("bad_file", "the claim file's token is not an address"))?;
    if !d.tokens.iter().any(|t| t.eth_token == token) {
        return refuse("token_not_registered", "This deposit's token is not one this bridge lists.");
    }
    let denom_index = u32::try_from(whole(&f["denominationIndex"], "denominationIndex")?).or_else(|_| refuse("bad_file", "the claim file's denominationIndex is too large"))?;
    let amount = whole(&f["amount"], "amount")?;
    let account = file_account(f["logosAccount"].as_str().unwrap_or_default())?;
    let key = hex_n::<32>(&f["claimKey"], "claimKey")?;
    let mut had_master = false;
    if let Some(ms) = f.get("masterSecret").filter(|v| !v.is_null()) {
        let ms = hex_n::<32>(ms, "masterSecret")?;
        if sha256(&[b"RFP022/ck", &ms]) != key {
            return refuse("bad_file", "the claim file's claimKey does not follow from its masterSecret: the file is damaged");
        }
        had_master = true;
    }
    let commitment = hex_n::<32>(&f["commitment"], "commitment")?;
    if wallet::deposit_commitment(&key, &account, &token, amount, &cfg_id) != commitment {
        return refuse("bad_file", "the claim file's commitment does not follow from its claim key, account, token, amount and cfgId: the file is damaged");
    }
    let leaf_index = match f["deposit"]["leafIndex"].clone() {
        Value::Null => None,
        v => Some(u64::try_from(whole(&v, "deposit leafIndex")?).or_else(|_| refuse("bad_file", "the claim file's leafIndex is too large"))?),
    };
    let deposit_tx = f["deposit"]["tx"].as_str().filter(|t| t.len() == 66 && t.starts_with("0x")).map(str::to_owned);
    Ok(ClaimFile { chain_id, vault, cfg_id, token, denom_index, amount, account, commitment, leaf_index, deposit_tx, had_master, key })
}

// ── The tree and the message (pinned by testdata/relayed-claim-v1.json) ─

pub fn zero_hashes() -> Vec<[u8; 32]> {
    let mut z = vec![[0u8; 32]];
    for h in 0..TREE_DEPTH {
        z.push(sha256(&[&z[h], &z[h]]));
    }
    z
}

/// The path of leaf `index` among `leaves` (the vault's tree, depth 32; `bridge_core::merkle_path`).
pub fn merkle_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    let zeros = zero_hashes();
    let mut layer = leaves.to_vec();
    let mut idx = index;
    let mut path = Vec::with_capacity(TREE_DEPTH);
    for zero in zeros.iter().take(TREE_DEPTH) {
        path.push(layer.get(idx ^ 1).copied().unwrap_or(*zero));
        layer = layer.chunks(2).map(|p| sha256(&[&p[0], p.get(1).unwrap_or(zero)])).collect();
        idx >>= 1;
    }
    path
}

pub fn merkle_root(leaf: &[u8; 32], index: u64, path: &[[u8; 32]]) -> [u8; 32] {
    let mut node = *leaf;
    for (h, sib) in path.iter().enumerate() {
        node = if (index >> h) & 1 == 1 { sha256(&[sib, &node]) } else { sha256(&[&node, sib]) };
    }
    node
}

/// The claim nullifier of the bridge since the 2026-11-14 security fixes:
/// `SHA256("RFP022/nfc" | k | dest | token | u256(denom))` (bound to the claimer, review H2).
pub fn claim_nullifier(k: &[u8; 32], dest: &[u8; 32], token: &[u8; 20], denom: u128) -> [u8; 32] {
    let mut d = [0u8; 32];
    d[16..].copy_from_slice(&denom.to_be_bytes());
    sha256(&[b"RFP022/nfc", k, dest, token, &d])
}

/// The claim nullifier of a bridge from before those fixes (the live v2 demo):
/// `SHA256("RFP022/nfc" | k)`. Used only for a config that [`lez::BridgeConfig::pre_fix`].
pub fn claim_nullifier_pre_fix(k: &[u8; 32]) -> [u8; 32] {
    sha256(&[b"RFP022/nfc", k])
}

/// borsh(bridge `Message::Instruction(Instruction::Claim{..})`), public claimer (no one-time
/// recipient): Message tag 0, Instruction tag 3, then the fields in order.
#[allow(clippy::too_many_arguments)]
pub fn claim_message(eth_token: &[u8; 20], denom_index: u32, denom: u128, root: &[u8; 32], leaf_index: u64, path: &[[u8; 32]], k: &[u8; 32], cfg_id: &[u8; 32]) -> Vec<u8> {
    let mut m = vec![0u8, 3];
    m.extend_from_slice(eth_token);
    m.extend_from_slice(&denom_index.to_le_bytes());
    m.extend_from_slice(&denom.to_le_bytes());
    m.extend_from_slice(root);
    m.extend_from_slice(&leaf_index.to_le_bytes());
    m.extend_from_slice(&(path.len() as u32).to_le_bytes());
    for p in path {
        m.extend_from_slice(p);
    }
    m.extend_from_slice(k);
    m.extend_from_slice(cfg_id);
    m.push(0);
    m
}

/// The call the wallet builds and signs (`propose_relayed_call`'s `call_json`): the claimer's
/// actor of the bridge is the root; the declared accounts are a public claim's, in
/// `ClaimPlan::mentions`' order; no entry is `new` (the relay refuses admission evidence: the
/// bridge creates the nullifier account by its own seed); the relay's payer pays.
#[allow(clippy::too_many_arguments)]
pub fn claim_call(bridge: &[u8; 32], token_program: &[u8; 32], definition: &[u8; 32], claimer: &[u8; 32], root: &[u8; 32], nullifier: &[u8; 32], message: &[u8], payer: &str, gas_limit: u64, max_fee: u128) -> Value {
    let b = lez::b58(bridge);
    let tp = lez::b58(token_program);
    // A seed alone: the wallet derives the address (its app-facing parser refuses a seed with an
    // address written beside it).
    let seeded = |seed: [u8; 32]| json!({ "seed": to_hex(&seed), "of": b, "program": b });
    json!({
        "program": b,
        "message": to_hex(message),
        "accounts": [
            seeded(lez::root_seed(root)),
            seeded(lez::nullifier_seed(nullifier)),
            seeded(lez::CONFIG_SEED),
            { "account": lez::b58(definition), "program": tp },
            { "account": lez::b58(claimer), "program": tp },
        ],
        "fee": { "payer": format!("Public/{}", payer.trim_start_matches("Public/")), "gas_limit": gas_limit, "tip": "0", "max_fee": max_fee.to_string() },
        "privacy": "bridge_public_claim",
    })
}

// ── State kept in memory ───────────────────────────────────────────────

/// A claim that passed the check: what proposing it needs (the key inside `file`).
pub struct Checked {
    pub file: ClaimFile,
    /// The deployment the claim was checked on (its own, not necessarily the current one) and its bridge.
    pub deployment: String,
    pub bridge: String,
    pub call: Value,
    pub summary: String,
    pub symbol: String,
    pub decimals: u8,
    pub definition: [u8; 32],
    pub token_program: [u8; 32],
    pub payer: String,
    pub nullifier: [u8; 32],
}

#[derive(Default)]
pub struct State {
    pub checks: HashMap<String, Checked>,
    /// Job id -> the half-signed transaction the wallet gave (in memory until the relay has it).
    pub signed: HashMap<String, String>,
    /// Tests: a recorded LEZ endpoint (posts then run in this thread).
    pub rpc_override: Option<Arc<dyn LezRpc + Send + Sync>>,
    /// The vault's `Deposit` events as last read, by vault: `(vault, depositCount, events)`. Read
    /// again only when `depositCount()` changes (`receive_status`).
    pub log_cache: Option<(String, u64, Vec<DepositEvent>)>,
    /// Whether the deployment's bridge is a pre-fix one (its claim nullifier formula), by
    /// deployment name.
    pub pre_fix: Option<(String, bool)>,
}

/// Relay answers that came back from the background thread, by job id (`None`: in flight).
static POSTS: Mutex<Option<HashMap<String, Option<Result<Relayed, Problem>>>>> = Mutex::new(None);

fn post_take(job: &str) -> Option<Option<Result<Relayed, Problem>>> {
    let mut g = POSTS.lock().ok()?;
    let map = g.get_or_insert_with(HashMap::new);
    match map.get(job) {
        Some(None) => Some(None),
        Some(Some(_)) => map.remove(job),
        None => None,
    }
}

fn post_put(job: &str, v: Option<Result<Relayed, Problem>>) {
    if let Ok(mut g) = POSTS.lock() {
        g.get_or_insert_with(HashMap::new).insert(job.to_owned(), v);
    }
}

fn check_id(commitment: &[u8; 32]) -> String {
    format!("rc-{}", &to_hex(commitment)[..16])
}

/// One leaf per finalized deposit, with the Config's denominations.
fn leaf_of(e: &DepositEvent, cfg: &lez::BridgeConfig) -> Result<[u8; 32], Refusal> {
    let t = cfg.tokens.iter().find(|t| t.eth_token == e.token).ok_or_else(|| Refusal {
        code: "leaves_mismatch",
        message: format!("the vault holds a deposit of {} at leaf {}, a token the bridge doesn't list; this module can't rebuild the vault's tree", eth::addr_hex(&e.token), e.leaf_index),
    })?;
    let denom = *t.denoms.get(e.denom_index as usize).ok_or_else(|| Refusal { code: "leaves_mismatch", message: format!("leaf {} names a denomination the bridge doesn't have", e.leaf_index) })?;
    Ok(crate::store::deposit_leaf(&cfg.cfg_id, &e.token, denom, &e.commitment))
}

/// The head fee state: `(base_fee_exec, base_fee_stor)`.
pub(crate) fn fee_state(rpc: &dyn LezRpc) -> Result<(u128, u128), Problem> {
    let v = rpc.call("getFeeState", json!([]))?;
    let n = |k: &str| v[k].as_u64().map(u128::from).or_else(|| v[k].as_str().and_then(|s| s.parse().ok()));
    match (n("base_fee_exec"), n("base_fee_stor")) {
        (Some(e), Some(s)) => Ok((e, s)),
        _ => Err(Problem { code: "unreachable", message: "getFeeState gave no base fees".to_owned(), retry_after: None }),
    }
}

impl Bridge {
    pub(crate) fn lez_rpc(&self) -> Result<Arc<dyn LezRpc + Send + Sync>, Refusal> {
        let d = self.deployment().clone();
        self.lez_rpc_for(&d)
    }

    /// The LEZ endpoint of a claim job: its own deployment's (kept in its meta), so choosing
    /// another bridge never moves a claim in flight; older jobs without one use the current one.
    fn job_rpc(&self, job: &Value) -> Result<Arc<dyn LezRpc + Send + Sync>, Refusal> {
        match job["meta"]["deployment"].as_str().and_then(|n| self.deployment_named(n)) {
            Some(d) => self.lez_rpc_for(&d),
            None => self.lez_rpc(),
        }
    }

    /// [`Self::lez_rpc`] of deployment `d` (a deposit or claim read on the deployment it is for).
    pub(crate) fn lez_rpc_for(&self, d: &Deployment) -> Result<Arc<dyn LezRpc + Send + Sync>, Refusal> {
        if let Some(r) = &self.relayed.rpc_override {
            return Ok(Arc::new(lez::Patient::new(r.clone())));
        }
        if d.lez_rpc.is_empty() || d.relay != "rpc" {
            return refuse("no_relay", format!("{} has no claim relay: a public claim from the Names Wallet needs one. Choose the bridge test network deployment in Settings.", d.label));
        }
        Ok(Arc::new(lez::Patient::new(Arc::new(lez::Http::new(&d.lez_rpc)))))
    }

    /// The Names Wallet's view of this claim's account: `{available, connected, given, network,
    /// channel, needs}`. Never fails: a missing wallet is reported, the dry run goes on.
    fn relay_wallet_view(&self, account: &str) -> Value {
        let written = format!("Public/{account}");
        let granted = match self.granted_typed() {
            Ok(g) => g,
            // Busy or slow is not "not installed": the screen keeps its Send and asks again.
            Err(f) => {
                let needs = match f {
                    wallet::Fail::Busy => "busy",
                    wallet::Fail::NoAnswer => "no_answer",
                    _ => "wallet",
                };
                return json!({ "available": false, "connected": false, "given": false, "problem": f.text(), "needs": needs });
            }
        };
        let (connected, accounts) = granted;
        let given = accounts.iter().any(|a| a["written"] == written.as_str() || (a["account"] == account && a["kind"] == "public"));
        let cfg = wallet::names_config().unwrap_or(Value::Null);
        let network = cfg["names_network"].as_str().map(str::to_owned);
        let channel = cfg["zone"]["channel"].as_str().map(str::to_owned);
        let needs = if !connected {
            Value::from("connect")
        } else if !given {
            Value::from("public_account")
        } else if network.as_deref().is_some_and(|n| n != "lez-0.4") {
            Value::from("network")
        } else {
            Value::Null
        };
        json!({ "available": true, "connected": connected, "given": given, "network": network, "channel": channel, "needs": needs })
    }

    /// Step 1: the dry run. Nothing is proposed, signed or sent.
    pub(crate) fn relayed_check_now(&mut self, source: &str) -> Value {
        self.relayed_check_opts(source, &Value::Null)
    }

    /// The check for a deposit the Names Wallet made the secret for (`make_public_deposit_request`,
    /// or found by `recover`): the claim key re-derived with `app_secret` instead of read from a
    /// claim file, then exactly `relayed_claim_check` (same reply, same `id`, so `relayed_claim`
    /// and `relayed_claim_build` work from it unchanged). `{needs: "secret", …}` while the
    /// person has to approve this app's values in the Names app first.
    pub(crate) fn relayed_check_deposit_now(&mut self, id: &str, options: &Value) -> Value {
        match self.deposit_claim_text(id) {
            Ok(Ok(text)) => self.relayed_check_opts(&text, options),
            Ok(Err(needs)) => needs,
            Err(r) => json!({ "ok": false, "error": r.to_json(), "wording": WORDING, "network": self.deployment().network }),
        }
    }

    /// A claim file's text for a stored wallet-made deposit, held in memory only for the check.
    fn deposit_claim_text(&self, id: &str) -> Result<Result<String, Value>, Refusal> {
        let row = self.store().deposit(id.trim()).map_err(|e| Refusal { code: "no_deposit", message: e })?;
        if row["derivation"] != "names-app" {
            return refuse("bad_file", "This deposit's secret isn't made by your Names Wallet: claim it from its claim file.");
        }
        if row["account_kind"] != "public" {
            return refuse("private_account", "This deposit is for a private account: it is claimed privately, with a proof your wallet makes (Deposits and claims).");
        }
        // The deployment the deposit was made on, never the current one by default.
        let d = self.deposit_deployment(&row).map_err(|e| Refusal { code: "unknown_deployment", message: e })?;
        let bad = |what: &str| Refusal { code: "bad_config", message: format!("this deposit's stored {what} can't be read") };
        let token = eth_address(row["eth_token"].as_str().unwrap_or_default()).map_err(|_| bad("token"))?;
        let t = d.tokens.iter().find(|t| t.eth_token == token).ok_or(Refusal { code: "token_not_registered", message: "This deposit's token is not one this bridge lists.".into() })?.clone();
        let denom = util::units(row["denom"].as_str().unwrap_or_default()).map_err(|_| bad("amount"))?;
        let index = row["key_index"].as_u64().ok_or_else(|| bad("key index"))?;
        let cfg = util::hex32(&d.cfg_id).map_err(|_| bad("cfgId"))?;
        let account = row["account"].as_str().unwrap_or_default().to_owned();
        let acct = util::lez_account(&account).map_err(|_| bad("account"))?;
        let written = format!("Public/{account}");
        let ctx = wallet::deposit_context(&cfg, &token, denom);
        let k = match wallet::app_secret(&written, wallet::SCOPE_DEPOSIT, &ctx, wallet::TAG_DEPOSIT, index, 1) {
            Ok(wallet::Values::Granted(v)) => v[0],
            Ok(wallet::Values::Pending(req)) => {
                return match wallet::needs("secret", &req, "Approve \"RFP022/deposit\" in the Names app: your wallet makes this deposit's claim secret from your account.") {
                    Ok(n) => Ok(Err(n)),
                    Err(e) => refuse("wallet_failed", e),
                };
            }
            Err(e) => return refuse("wallet_failed", e),
        };
        let c = wallet::deposit_commitment(&k, &acct, &token, denom, &cfg);
        if hex0x(&c) != row["commitment"].as_str().unwrap_or_default().to_lowercase() {
            return refuse("bad_file", "Your Names Wallet's value doesn't give this deposit's commitment (another wallet, or another account): nothing was checked.");
        }
        Ok(Ok(json!({
            "format": CLAIM_FILE_FORMAT, "version": 1, "chainId": d.chain_id.to_string(), "vault": hex0x(&d.vault), "cfgId": d.cfg_id,
            "token": { "address": hex0x(&token), "symbol": t.symbol, "decimals": t.decimals }, "denominationIndex": row["denom_index"],
            "amount": denom.to_string(), "logosAccount": written, "claimKey": hex0x(&k), "commitment": hex0x(&c),
        })
        .to_string()))
    }

    /// [`Self::relayed_check_now`] with options: `wallet_hint: false` asks the Names Wallet
    /// nothing (the reply's `wallet` says `skipped`).
    pub(crate) fn relayed_check_opts(&mut self, source: &str, options: &Value) -> Value {
        let hint = options["wallet_hint"] != false;
        match self.relayed_check_inner(source, hint) {
            Ok(v) => v,
            Err(r) => json!({ "ok": false, "error": r.to_json(), "wording": WORDING, "network": self.deployment().network }),
        }
    }

    /// The deployment a claim file is for: the current one, else the built-in or added one whose
    /// chain and vault the file names (a v3 file after v4 became the default). Unknown: the current one, whose
    /// checks then say which vault the file is for.
    fn claim_file_deployment(&self, text: &str) -> Deployment {
        let cur = self.deployment().clone();
        let Ok(f) = serde_json::from_str::<Value>(text) else { return cur };
        let Ok(vault) = eth_address(f["vault"].as_str().unwrap_or_default()) else { return cur };
        let chain = f["chainId"].as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| f["chainId"].as_u64());
        if cur.vault == vault {
            return cur;
        }
        self.known_deployments().into_iter().find(|d| d.vault == vault && chain.is_none_or(|c| c == d.chain_id)).unwrap_or(cur)
    }

    fn relayed_check_inner(&mut self, source: &str, wallet_hint: bool) -> Result<Value, Refusal> {
        let text = read_source(source)?;
        let d = self.claim_file_deployment(&text);
        let file = parse_claim_file(&text, &d)?;
        drop(text);
        let rpc = self.lez_rpc_for(&d)?;
        let bridge = util::lez_account(&d.bridge_program).or_else(|_| refuse("bad_config", "the deployment's bridge program is not an account id"))?;
        let mut checks: Vec<Value> = vec![json!({ "key": "file", "ok": true, "text": "Claim file checked: its commitment follows from its claim key, account, token and amount." })];

        // The bridge, as LEZ holds it.
        let cfg = match lez::bridge_config(rpc.as_ref(), &bridge) {
            Ok(Some(c)) => c,
            Ok(None) => return refuse("no_bridge", format!("There is no bridge {} on {} (the network may have been reset).", short(&d.bridge_program), d.network)),
            Err(e) if e.contains("reach") || e.contains("HTTP") || e.contains("slow down") => return refuse("unreachable", format!("{} can't be reached now: {e}", d.network)),
            Err(e) => return refuse("bridge_unreadable", format!("This module can't read the bridge's settings: {e}")),
        };
        if cfg.cfg_id != file.cfg_id || cfg.vault != d.vault || cfg.evm_chain_id != d.chain_id {
            return refuse("cfg_mismatch", format!("The bridge on {} is bound to another vault or deployment than this app's settings; update the deployment.", d.network));
        }
        if lez::b58(&cfg.token_program) != d.token_program {
            return refuse("bad_config", format!("The bridge mints through token program {}, not the {} this app's settings name; update the deployment.", short(&lez::b58(&cfg.token_program)), short(&d.token_program)));
        }
        if cfg.root_source != 0 {
            return refuse("bad_config", "This bridge takes roots from an Ethereum attestation; the relay takes claims under operator-recorded roots only.");
        }
        // A fixed bridge names the chain it was initialised for; a program can't check it, this
        // module can (security review 2026-11-14, M1). A pre-fix bridge (zero) names none.
        if !cfg.pre_fix() {
            match lez::chain_channel_id(rpc.as_ref()) {
                Ok(Some(ch)) if ch == cfg.lez_chain_id => {}
                Ok(_) => return refuse("wrong_chain", format!("The bridge on {} was initialised for another LEZ chain than the one this app reaches: a copy, not the deployment. Nothing was proposed.", d.network)),
                Err(e) => return refuse("bridge_unreadable", format!("This module can't read the chain's identity: {e}")),
            }
        }
        let token = cfg.tokens.iter().find(|t| t.eth_token == file.token && t.registered).ok_or(Refusal { code: "token_not_registered", message: "The bridge doesn't list this deposit's token.".into() })?.clone();
        if cfg.mint_frozen {
            return refuse("minting_frozen", "Claims are paused on this bridge (all tokens): try again once the admin unfreezes it. Nothing was sent.");
        }
        if token.mint_frozen {
            return refuse("minting_frozen_token", "Claims of this token are paused: try again once the admin unfreezes it. Nothing was sent.");
        }
        match token.denoms.get(file.denom_index as usize) {
            None => return refuse("bad_denomination", "The claim file names a denomination this token doesn't have."),
            Some(&x) if x != file.amount => return refuse("wrong_amount", "The claim file's amount is not its denomination's: the file is damaged or for another bridge."),
            Some(_) => {}
        }
        let sym = d.tokens.iter().find(|t| t.eth_token == file.token).map_or_else(|| "token".to_owned(), |t| t.symbol.clone());
        let amount_text = format!("{} {sym}", format_amount(file.amount, token.decimals));
        checks.push(json!({ "key": "bridge", "ok": true, "text": format!("Bridge read on {}: {sym} claims are open.", d.network) }));

        // The deposit on Ethereum, finalized, and its leaf.
        let (events, finalized, _latest) = self.events_for(&d).map_err(|e| Refusal { code: "eth_unreachable", message: format!("Sepolia can't be read now: {e}") })?;
        let Some(ev) = events.iter().find(|e| e.commitment == file.commitment).cloned() else {
            return refuse("not_in_vault", "The vault has no deposit with this claim file's commitment. If you just deposited, wait for the transaction to be mined; otherwise this file is for another deposit.");
        };
        if let Some(i) = file.leaf_index.filter(|i| *i != ev.leaf_index) {
            return refuse("wrong_leaf", format!("The claim file says leaf {i}, but the vault holds this deposit at leaf {}.", ev.leaf_index));
        }
        if ev.block > finalized.number {
            let mins = (ev.block - finalized.number) * 12 / 60 + 1;
            return refuse("not_final", format!("This deposit is not final on Sepolia yet (block {}, final up to {}). That takes about 15 minutes after the deposit; try again in about {mins} minute(s).", ev.block, finalized.number));
        }
        let fin: Vec<&DepositEvent> = events.iter().filter(|e| e.block <= finalized.number).collect();
        let leaves: Vec<[u8; 32]> = fin.iter().map(|e| leaf_of(e, &cfg)).collect::<Result<_, _>>()?;
        let index = ev.leaf_index as usize;
        let mine = crate::store::deposit_leaf(&cfg.cfg_id, &file.token, file.amount, &file.commitment);
        if leaves.get(index) != Some(&mine) {
            return refuse("leaves_mismatch", "The vault's leaf for this deposit is not the one this claim file gives.");
        }
        checks.push(json!({ "key": "deposit", "ok": true, "text": format!("Deposit found on Sepolia: leaf {index} of {}, final.", leaves.len()) }));

        // A root that includes it, recorded on LEZ (newest first, as the CLI takes the newest).
        let mut chosen = None;
        let mut tried = 0;
        for j in (index..leaves.len()).rev() {
            let path = merkle_path(&leaves[..=j], index);
            let root = merkle_root(&mine, index as u64, &path);
            if root != fin[j].root {
                return refuse("leaves_mismatch", format!("The vault's tree as rebuilt here does not give the vault's own root at leaf {j}; nothing was proposed. Try another Ethereum RPC in Settings."));
            }
            if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::root_seed(&root))? {
                chosen = Some((root, path, j));
                break;
            }
            tried += 1;
            if tried >= MAX_ROOT_TRIES {
                break;
            }
        }
        let Some((root, path, at)) = chosen else {
            return refuse("root_not_attested", format!("{} hasn't recorded a vault root that includes this deposit yet. The bridge records final Sepolia roots about once a minute: try again shortly.", d.network));
        };
        checks.push(json!({ "key": "root", "ok": true, "text": format!("Vault root recorded on {} (the root after leaf {at}).", d.network) }));

        let nullifier = if cfg.pre_fix() {
            claim_nullifier_pre_fix(file.key())
        } else {
            claim_nullifier(file.key(), &file.account, &file.token, file.amount)
        };
        if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nullifier))? {
            return refuse("already_claimed", "This deposit was claimed already: its claim nullifier is spent. Nothing was sent.");
        }
        checks.push(json!({ "key": "unclaimed", "ok": true, "text": "Not claimed yet." }));

        // The relay and the fee it pays.
        let info = lez::relay_info(rpc.as_ref())?;
        if let Some(other) = info.bridge.as_deref().filter(|b| *b != d.bridge_program) {
            let cur = self.deployment();
            if d.name != cur.name && other == cur.bridge_program {
                // A deposit on an older deployment (v3) after the relay moved to the current one (v4).
                let old = crate::config::plain_label(&d.name);
                return refuse(
                    "relay_other_bridge",
                    format!(
                        "This deposit is on the old vault ({old}). The bridge's relay now serves {} only, so it won't pay this claim's fee: a claim on {old} pays its own fee, which the Names Wallet can't send yet. Nothing was sent; the deposit stays claimable on {old}.",
                        crate::config::plain_label(&cur.name)
                    ),
                );
            }
            return refuse("relay_refused", "The relay serves another bridge than this one.");
        }
        let claimer = file.account_text();
        if info.payer == claimer {
            return refuse("wrong_account", "This account is the relay's own payer; a relayed claim needs another account.");
        }
        let message = claim_message(&file.token, file.denom_index, file.amount, &root, ev.leaf_index, &path, file.key(), &file.cfg_id);
        let (exec, stor) = fee_state(rpc.as_ref())?;
        let wire = message.len() as u64 + WIRE_OVERHEAD;
        let reserve = u128::from(info.max_gas) * exec + u128::from(wire) * stor;
        if reserve > info.max_fee {
            return refuse("relay_cap", format!("The network's fees are above what the relay pays per claim right now (about {reserve} needed, the relay pays at most {}); try again later.", info.max_fee));
        }
        let call = claim_call(&bridge, &cfg.token_program, &token.definition, &file.account, &root, &nullifier, &message, &info.payer, info.max_gas, info.max_fee);
        checks.push(json!({ "key": "relay", "ok": true, "text": format!("The bridge's relay pays the fee (at most {}).", util::format_amount(info.max_fee, 0)) }));

        let id = check_id(&file.commitment);
        let wallet_view = if wallet_hint {
            self.relay_wallet_view(&claimer)
        } else {
            json!({ "skipped": true, "available": null, "connected": null, "given": null, "network": null, "channel": null, "needs": null })
        };
        let summary = format!("Claim {amount_text} from Sepolia (leaf {}) into Public/{}. {WORDING}", ev.leaf_index, short(&claimer));
        let message_sha256 = to_hex(&sha256(&[&message]));
        let view = json!({
            "ok": true, "id": id, "error": null, "network": d.network, "wording": WORDING, "privacy": PRIVACY_LINE,
            "deposit": {
                "amount_text": amount_text, "symbol": sym, "amount": file.amount.to_string(), "decimals": token.decimals,
                "account": format!("Public/{claimer}"), "leaf_index": ev.leaf_index, "leaves": leaves.len(),
                "deposit_tx": file.deposit_tx.clone().or_else(|| Some(ev.tx.clone()).filter(|t| !t.is_empty())), "block": ev.block, "finalized_block": finalized.number,
            },
            "root": hex0x(&root), "root_after_leaf": at,
            "relay": { "payer": format!("Public/{}", info.payer), "max_gas": info.max_gas, "max_fee": info.max_fee.to_string(), "reserve_at_head": reserve.to_string() },
            "call": { "program": d.bridge_program, "token_program": d.token_program, "accounts": 5, "message_len": message.len(), "message_sha256": message_sha256,
                      "gas_limit": info.max_gas, "max_fee": info.max_fee.to_string(), "payer": format!("Public/{}", info.payer) },
            "checks": checks, "wallet": wallet_view, "dry_run": true,
            "approval": "Your Names app shows the call your wallet built and asks you to approve it. Your wallet signs it and sends nothing; the bridge's relay sends it and pays the fee.",
        });
        self.relayed.checks.insert(
            id,
            Checked { file, deployment: d.name.clone(), bridge: d.bridge_program.clone(), call, summary, symbol: sym, decimals: token.decimals, definition: token.definition, token_program: cfg.token_program, payer: info.payer, nullifier },
        );
        Ok(view)
    }

    /// The gates before proposing. `Some(needs)` when the person has something to do first.
    pub(crate) fn relayed_gates(&self, account: &str, bridge: &str, token_program: &str, options: &Value) -> Result<Option<Value>, String> {
        let v = self.relay_wallet_view(account);
        if v["available"] != true {
            return Err(v["problem"].as_str().unwrap_or(wallet::UNREACHABLE_TEXT).to_owned());
        }
        match v["needs"].as_str() {
            Some("connect") => {
                let r = wallet::request_access(ACCESS_REASON, Some(&format!("Public/{account}"))).map_err(|f| f.text())?;
                let want = format!("Public/{account}");
                if r["state"] == "approved" && r["accounts"].as_array().is_some_and(|l| l.iter().any(|a| a.as_str() == Some(want.as_str()))) {
                    // Given meanwhile: on to the next gate.
                } else {
                    return Ok(Some(wallet::needs("connect", &r, &format!("Connect your Names Wallet: approve this app in the Names app and give it your public account Public/{account}."))?));
                }
            }
            Some("public_account") => {
                // Names with `grant_add_account` asks the person to add it; without it, asking
                // makes no request: take the access back (wallet::add_account says which).
                let written = format!("Public/{account}");
                if let Some(r) = wallet::add_account(ACCESS_REASON, &written)? {
                    return Ok(Some(wallet::needs("public_account", &r, &format!("Approve adding {written} to this app's access in the Names app, then this claim goes on."))?));
                }
                // Given meanwhile: on to the next gate.
            }
            Some("network") => {
                return Err("Switch your Names Wallet to LEZ 0.4 (the bridge test network) in the Names app's settings, then try again. Nothing was sent.".to_owned());
            }
            _ => {}
        }
        // The wallet's LEZ 0.4 must be this bridge's chain.
        if let (Some(theirs), Ok(rpc)) = (v["channel"].as_str(), self.lez_rpc()) {
            if let Ok(ours) = rpc.call("getChannelId", json!([])) {
                if ours.as_str().is_some_and(|o| !o.eq_ignore_ascii_case(theirs)) {
                    return Err(format!("Your Names Wallet's LEZ 0.4 is another chain than this bridge's ({}): point the wallet at the bridge test network, then try again.", self.deployment().lez_rpc));
                }
            }
        }
        // The programs the wallet will sign for, declared once (both, together).
        let want = json!([bridge, token_program]);
        let mut st = self.wstate();
        let decl = st["relay_declared"].clone();
        if decl["programs"] == want {
            // Declared by a request this module never got the id of (its declare_programs got no
            // answer), and the person said to go on: the proposal itself tells (the wallet
            // refuses a call to an undeclared program before anyone sees it).
            if decl["proven"] == true || (decl["unknown"] == true && options[AFTER_UNCERTAIN] == true) {
                return Ok(None);
            }
            if let Some(rid) = decl["request"].as_str() {
                // Busy or no answer: say so; never declare a second time because a read failed.
                let r = match wallet::names_try("request_status", json!([rid])) {
                    Ok(r) => r,
                    Err(f) if f.transient() => return Err(f.text()),
                    Err(_) => Value::Null,
                };
                match r["state"].as_str() {
                    Some("approved") | Some("done") => return Ok(None),
                    Some("pending") => return Ok(Some(wallet::needs("programs", &r, &Self::declare_words())?)),
                    _ => {}
                }
            }
        }
        if let Some(n) = self.uncertain_needs("declare", options) {
            return Ok(Some(n));
        }
        match wallet::names_try("declare_programs", json!([want.to_string(), DECLARE_REASON])) {
            Ok(r) => {
                st["relay_declared"] = json!({ "programs": want, "request": r["request"] });
                self.put_wstate(&st)?;
                Ok(Some(wallet::needs("programs", &r, &Self::declare_words())?))
            }
            Err(wallet::Fail::NoAnswer) => {
                // It may be waiting in the Names app: no second request until the person looked.
                st["relay_declared"] = json!({ "programs": want, "request": null, "unknown": true });
                self.put_wstate(&st)?;
                self.mark_uncertain("declare");
                Ok(self.uncertain_needs("declare", &Value::Null))
            }
            Err(f) => Err(f.text()),
        }
    }

    /// A proposal (`propose`: the claim; `declare`: the programs) that got no answer at all may be
    /// waiting in the Names app. Its id is unknown and the wallet lists an app's requests to no
    /// app, so it can't be read back: it is remembered here for the request's lifetime.
    fn mark_uncertain(&self, key: &str) {
        let mut st = self.wstate();
        if !st["relay_uncertain"].is_object() {
            st["relay_uncertain"] = json!({});
        }
        st["relay_uncertain"][key] = json!(now_ms());
        let _ = self.put_wstate(&st);
    }

    fn clear_uncertain(&self, key: &str) {
        let mut st = self.wstate();
        if let Some(m) = st["relay_uncertain"].as_object_mut() {
            if m.remove(key).is_some() {
                let _ = self.put_wstate(&st);
            }
        }
    }

    /// While a proposal that got no answer may still be waiting in the Names app: what the person
    /// is told instead of a second proposal. None once it has lapsed, or when the person said to go
    /// on (`after_uncertain`), which clears it.
    fn uncertain_needs(&self, key: &str, options: &Value) -> Option<Value> {
        let at = self.wstate()["relay_uncertain"][key].as_u64()?;
        if now_ms().saturating_sub(at) > UNCERTAIN_MS || options[AFTER_UNCERTAIN] == true {
            self.clear_uncertain(key);
            return None;
        }
        let message = if key == "declare" {
            "Your Names Wallet didn't answer in time when this app asked it to approve the bridge's two programs, so that request may be waiting in the Names app. If it is there, approve it; then press Continue. Nothing was sent."
        } else {
            "Your Names Wallet didn't answer in time when this app proposed the claim, so it may be waiting in the Names app. This app can't follow that request: if the Names app shows a claim for this deposit, reject it there, then press Propose again (or wait 10 minutes and it lapses). Nothing was signed or sent."
        };
        Some(json!({ "needs": "uncertain", "uncertain": key, "request": null, "message": message, "since": at, "lapses_at": at + UNCERTAIN_MS }))
    }

    fn declare_words() -> String {
        "Approve, once, the two programs this app asks your wallet to sign claims for (the bridge and its token program) in the Names app. The bridge program is not in the Program Registry yet, so the Names app asks you to tick that you accept that before you approve.".to_owned()
    }

    /// Step 2: gates, then the proposal. Returns `{needs…}` or the job.
    pub(crate) fn relayed_claim_now(&mut self, id: &str, options: &Value) -> Result<Value, String> {
        // Asked again for a claim that already has a job (the screen gave up waiting while this
        // module went on, then "Try again"): that job, never a second proposal.
        let running = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().rev().find(|j| j["check"] == id && j["state"] == "running").cloned());
        if let Some(job) = running {
            return self.relayed_status_now(job["id"].as_str().unwrap_or_default());
        }
        let (account, bridge, tp, call, summary, meta) = {
            let c = self.relayed.checks.get(id).ok_or("Check the claim file first (this app keeps the claim secret only in memory, so give the file again after a restart).")?;
            if self.deployment_named(&c.deployment).is_none() {
                return Err("the bridge deployment changed since this claim was checked; check the claim file again".to_owned());
            }
            (
                c.file.account_text(),
                c.bridge.clone(),
                lez::b58(&c.token_program),
                c.call.to_string(),
                c.summary.clone(),
                Self::job_meta(c),
            )
        };
        if let Some(n) = self.relayed_gates(&account, &bridge, &tp, options)? {
            return Ok(n);
        }
        let ukey = format!("propose:{id}");
        if let Some(n) = self.uncertain_needs(&ukey, options) {
            return Ok(n);
        }
        let r = match wallet::names_try("propose_relayed_call", json!([format!("Public/{account}"), call, summary])) {
            Ok(r) => r,
            Err(wallet::Fail::NoAnswer) => {
                // Perhaps taken: remembered, so "Try again" can't propose it a second time blindly.
                self.mark_uncertain(&ukey);
                return Ok(self.uncertain_needs(&ukey, &Value::Null).unwrap_or(Value::Null));
            }
            Err(wallet::Fail::Refused { error, .. }) if error.contains("declare_programs") || error.contains("not a program this app declared") => {
                // The declaration is gone (taken back in Settings) or was never approved: declare
                // again (one request; the person approves it, then this claim goes on).
                let mut st = self.wstate();
                st["relay_declared"] = Value::Null;
                self.put_wstate(&st)?;
                return match self.relayed_gates(&account, &bridge, &tp, options)? {
                    Some(n) => Ok(n),
                    None => Err(format!("The Names Wallet refused the claim: {error}. Nothing was sent.")),
                };
            }
            Err(f) => return Err(f.text()),
        };
        let rid = r["request"].as_str().ok_or("the Names Wallet gave no request id")?.to_owned();
        let mut st = self.wstate();
        if st["relay_declared"]["unknown"] == true {
            // The proposal passed the wallet's declared-programs check: the declaration holds.
            st["relay_declared"] = json!({ "programs": st["relay_declared"]["programs"].clone(), "request": null, "proven": true });
            self.put_wstate(&st)?;
            self.clear_uncertain("declare");
        }
        let jid = self.push_job(id, &account, meta, Some(rid), "approve", "bridge_app")?;
        self.relayed_status_now(&jid)
    }

    /// A new claim job, kept in `wallet.json` (public facts only). `host`: who gets the wallet's
    /// signature, this module (`bridge_app`: it asks Names itself) or the Names app (`names_app`:
    /// it signs, then hands the transaction over with `relayed_claim_submit`).
    fn push_job(&self, check: &str, account: &str, meta: Value, request: Option<String>, phase: &str, host: &str) -> Result<String, String> {
        let now = now_ms();
        let jid = format!("rc-job-{now}-{}", &to_hex(&util::random32()?)[..6]);
        let job = json!({
            "id": jid, "kind": "relayed_claim", "check": check, "request": request, "account": format!("Public/{account}"),
            "meta": meta, "network": self.deployment().network, "started": now, "finished": null, "state": "running", "phase": phase, "host": host,
            "proposals": u64::from(request.is_some()), "resigns": 0, "busy": 0, "tx": null, "block": null, "error": null, "result": null, "note": null,
        });
        let mut st = self.wstate();
        if st["relayed_jobs"].is_null() {
            st["relayed_jobs"] = json!([]);
        }
        st["relayed_jobs"].as_array_mut().unwrap().push(job);
        self.put_wstate(&st)?;
        Ok(jid)
    }

    /// The facts a job keeps about a checked claim (public: amounts, ids, the nullifier).
    fn job_meta(c: &Checked) -> Value {
        json!({ "symbol": c.symbol, "decimals": c.decimals, "amount_text": format!("{} {}", format_amount(c.file.amount, c.decimals), c.symbol),
                "definition": lez::b58(&c.definition), "token_program": lez::b58(&c.token_program), "nullifier": hex0x(&c.nullifier), "payer": c.payer, "bridge": c.bridge, "deployment": c.deployment })
    }

    // ── Hosted by the Names app (modules/NAMES-EMBED.md, Part B) ────────

    /// The call the wallet signs, for a checked claim: exactly `propose_relayed_call`'s input
    /// (`account`, and `call` = {program, message, accounts, fee, privacy}). Asks no wallet,
    /// sends nothing, writes nothing. The claim key is in the message (the bridge's `Claim`
    /// carries it as `secret`): the same bytes today's proposal gives the wallet, and what the
    /// claim publishes when it lands. Never the file's master secret.
    pub(crate) fn relayed_build_now(&self, id: &str) -> Result<Value, String> {
        let c = self.relayed.checks.get(id).ok_or("Check the claim file first (this app keeps the claim only in memory, so give the file again after a restart).")?;
        let Some(d) = self.deployment_named(&c.deployment) else {
            return Err("the bridge deployment changed since this claim was checked; check the claim file again".to_owned());
        };
        Ok(json!({
            "check": id, "account": format!("Public/{}", c.file.account_text()), "call": c.call, "summary": c.summary,
            "network": d.network, "deployment": d.name, "bridge_program": d.bridge_program, "token_program": lez::b58(&c.token_program),
            "watch": { "nullifier": hex0x(&c.nullifier), "definition": lez::b58(&c.definition), "decimals": c.decimals, "symbol": c.symbol },
            "wording": WORDING, "privacy": PRIVACY_LINE,
        }))
    }

    /// The Names app's half-signed claim (hex), for the relay. Starts the job at `relay`, or gives
    /// a job that waits for a new signature (`needs: "sign_again"`) its new one. Then
    /// `relayed_claim_status`.
    pub(crate) fn relayed_submit_now(&mut self, id: &str, transaction: &str) -> Result<Value, String> {
        let tx = transaction.trim().trim_start_matches("0x").to_owned();
        if tx.is_empty() || tx.len() % 2 != 0 || !tx.bytes().all(|b| b.is_ascii_hexdigit()) || tx.len() > 2 * 64 * 1024 {
            return Err("give the signed transaction as hex, as the Names Wallet returned it".to_owned());
        }
        let running = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().rev().find(|j| j["check"] == id && j["state"] == "running").cloned());
        if let Some(mut job) = running {
            let jid = job["id"].as_str().unwrap_or_default().to_owned();
            if job["sign_again"] == true {
                // The relay's payer moved on (or this module restarted): the same claim, signed again.
                self.relayed.signed.insert(jid.clone(), tx);
                job["sign_again"] = Value::Null;
                job["awaiting_payer"] = json!(false);
                job["busy_since"] = Value::Null;
                job["resigns"] = json!(job["resigns"].as_u64().unwrap_or(0) + 1);
                job["phase"] = json!("relay");
                job["note"] = Value::Null;
                self.note_payer_nonce(&mut job);
                self.put_job(&job)?;
            }
            // Otherwise a repeat (the screen stopped waiting): the job as it stands.
            return self.relayed_status_now(&jid);
        }
        let (account, meta) = {
            let c = self.relayed.checks.get(id).ok_or("Check the claim file first (this app keeps the claim only in memory, so give the file again after a restart).")?;
            if self.deployment_named(&c.deployment).is_none() {
                return Err("the bridge deployment changed since this claim was checked; check the claim file again".to_owned());
            }
            (c.file.account_text(), Self::job_meta(c))
        };
        let jid = self.push_job(id, &account, meta, None, "relay", "names_app")?;
        self.relayed.signed.insert(jid.clone(), tx);
        let mut job = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().find(|j| j["id"] == jid.as_str()).cloned()).ok_or("the job was not kept")?;
        job["signed_at"] = json!(now_ms());
        self.note_payer_nonce(&mut job);
        self.put_job(&job)?;
        self.relayed_status_now(&jid)
    }

    fn put_job(&self, job: &Value) -> Result<(), String> {
        let mut st = self.wstate();
        if let Some(list) = st["relayed_jobs"].as_array_mut() {
            if let Some(j) = list.iter_mut().find(|j| j["id"] == job["id"]) {
                *j = job.clone();
            }
        }
        self.put_wstate(&st)
    }

    pub(crate) fn relayed_jobs(&self) -> Vec<Value> {
        let mut v: Vec<Value> = self.wstate()["relayed_jobs"].as_array().cloned().unwrap_or_default();
        v.reverse();
        v.iter().map(|j| job_view(j, now_ms())).collect()
    }

    /// Step 3: moves the job on, as far as it can go now.
    pub(crate) fn relayed_status_now(&mut self, id: &str) -> Result<Value, String> {
        let mut job = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().find(|j| j["id"] == id).cloned()).ok_or_else(|| format!("there is no claim job {id}"))?;
        if job["state"] == "running" {
            let before = job.clone();
            self.advance(&mut job);
            if job != before {
                self.put_job(&job)?;
            }
        }
        Ok(job_view(&job, now_ms()))
    }

    fn fail(job: &mut Value, code: &str, message: &str) {
        job["state"] = json!("failed");
        job["error"] = json!({ "code": code, "message": message });
        job["finished"] = json!(now_ms());
        job["note"] = Value::Null;
    }

    fn advance(&mut self, job: &mut Value) {
        let id = job["id"].as_str().unwrap_or_default().to_owned();
        for _ in 0..4 {
            let phase = job["phase"].as_str().unwrap_or_default().to_owned();
            match phase.as_str() {
                "approve" => {
                    let rid = job["request"].as_str().unwrap_or_default().to_owned();
                    // A poll: asked once, the next poll is the retry (no sleeping in the module).
                    let r = match wallet::names_once("request_status", json!([rid])) {
                        Ok(r) => r,
                        Err(wallet::Fail::Busy) => {
                            job["note"] = json!("Your Names Wallet is busy; asking again shortly. Nothing was sent.");
                            return;
                        }
                        Err(f) => {
                            // Silent past the request's lifetime: it has lapsed in the wallet too.
                            let since = job["proposed_at"].as_u64().or_else(|| job["started"].as_u64()).unwrap_or(0);
                            if now_ms().saturating_sub(since) > UNCERTAIN_MS {
                                return Self::fail(job, "wallet_no_answer", "Your Names Wallet hasn't answered for longer than a request lives (10 minutes): nothing was signed or sent. If the Names app still shows the claim, reject it there, then check the claim file again.");
                            }
                            job["note"] = json!(format!("Asking your Names Wallet how the request stands: {}. Asking again shortly.", f.short()));
                            return;
                        }
                    };
                    match r["state"].as_str() {
                        Some("pending") => {
                            job["note"] = json!("Waiting for you in the Names app: it shows the call your wallet built. Approve it there.");
                            return;
                        }
                        Some("approved") | Some("running") => {
                            job["note"] = json!("Approved; your wallet is signing it.");
                            return;
                        }
                        Some("done") => match r["transaction"].as_str() {
                            Some(tx) => {
                                self.relayed.signed.insert(id.clone(), tx.to_owned());
                                job["hash"] = r["hash"].clone();
                                job["signed_at"] = json!(now_ms());
                                self.note_payer_nonce(job);
                                job["phase"] = json!("relay");
                                job["note"] = Value::Null;
                            }
                            None => return Self::fail(job, "wallet_failed", "The Names Wallet ended the request without a signed transaction; nothing was sent."),
                        },
                        Some("rejected") => return Self::cancelled(job, &id, "You rejected it in the Names app; nothing was signed or sent."),
                        Some("expired") => return Self::cancelled(job, &id, "Nobody answered it in the Names app within 10 minutes; nothing was signed or sent."),
                        Some("cancelled") => return Self::cancelled(job, &id, "Cancelled; nothing was signed or sent."),
                        _ => return Self::fail(job, "wallet_failed", &format!("The Names Wallet says: {}. Nothing was sent.", r["error"].as_str().unwrap_or("it failed"))),
                    }
                }
                "relay" => {
                    // Go on when it moved to another phase (confirm, or approve again).
                    if !self.relay_step(job, &id) && (job["phase"] == "relay" || job["state"] != "running") {
                        return;
                    }
                }
                "confirm" => {
                    self.confirm_step(job, &id);
                    return;
                }
                _ => return,
            }
        }
    }

    fn cancelled(job: &mut Value, _id: &str, message: &str) {
        job["state"] = json!("cancelled");
        job["error"] = json!({ "code": "wallet_rejected", "message": message });
        job["finished"] = json!(now_ms());
        job["note"] = Value::Null;
    }

    fn note_payer_nonce(&self, job: &mut Value) {
        let Ok(rpc) = self.job_rpc(job) else { return };
        let payer = job["meta"]["payer"].as_str().unwrap_or_default().to_owned();
        let acct = job["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
        if let Ok(n) = lez::nonces(rpc.as_ref(), &[acct, payer]) {
            job["nonce_account"] = json!(n[0].to_string());
            job["nonce_payer"] = json!(n[1].to_string());
        }
    }

    /// Hands the signed transaction to the relay (background thread) and reads its answer.
    /// True when the job moved on to `confirm`.
    fn relay_step(&mut self, job: &mut Value, id: &str) -> bool {
        if job["next_try"].as_u64().is_some_and(|t| now_ms() < t) {
            return false;
        }
        if job["sign_again"] == true {
            return false;
        }
        if job["awaiting_payer"] == true && !self.payer_moved(job, id) {
            return false;
        }
        let outcome = match post_take(id) {
            Some(None) => {
                job["note"] = json!("The bridge's relay is checking the claim, then sends it and pays the fee.");
                return false;
            }
            Some(Some(o)) => o,
            None => {
                let Some(tx) = self.relayed.signed.get(id).cloned() else {
                    if job["host"] == "names_app" {
                        // Hosted by the Names app: only it can give the signature again.
                        job["sign_again"] = json!(true);
                        job["note"] = json!("This module restarted and no longer holds the signed claim: sign it again in the Names app.");
                        return false;
                    }
                    // The module restarted: the wallet still holds the transaction it signed.
                    match wallet::names_once("request_status", json!([job["request"]])) {
                        Ok(r) => match r["transaction"].as_str() {
                            Some(tx) => {
                                self.relayed.signed.insert(id.to_owned(), tx.to_owned());
                                return false;
                            }
                            None => {
                                Self::fail(job, "wallet_failed", "The signed claim is no longer available from the Names Wallet; check the claim file again.");
                                return false;
                            }
                        },
                        // Busy or no answer: asked again at the next poll, the job kept.
                        Err(f) if f.transient() => {
                            job["note"] = json!(format!("Getting the signed claim back from your Names Wallet: {}. Asking again shortly.", f.short()));
                            return false;
                        }
                        Err(f) => {
                            Self::fail(job, "wallet_failed", &format!("The signed claim is no longer available from the Names Wallet ({}); check the claim file again.", f.text()));
                            return false;
                        }
                    }
                };
                job["posted_at"] = json!(now_ms());
                job["note"] = json!("The bridge's relay is checking the claim, then sends it and pays the fee.");
                let rpc = match self.job_rpc(job) {
                    Ok(r) => r,
                    Err(r) => {
                        Self::fail(job, r.code, &r.message);
                        return false;
                    }
                };
                if self.relayed.rpc_override.is_some() {
                    // Tests: in this thread.
                    post_put(id, Some(lez::relay_claim(rpc.as_ref(), &tx)));
                    return self.relay_step(job, id);
                }
                post_put(id, None);
                let key = id.to_owned();
                std::thread::spawn(move || {
                    let o = lez::relay_claim(rpc.as_ref(), &tx);
                    post_put(&key, Some(o));
                });
                return false;
            }
        };
        match outcome {
            Ok(Relayed::Submitted(hash)) => {
                self.relayed.signed.remove(id);
                job["tx"] = json!(hash);
                job["submitted_at"] = json!(now_ms());
                job["phase"] = json!("confirm");
                job["note"] = json!("Sent by the relay; waiting for a block.");
                true
            }
            Ok(Relayed::Refused { reason, retry }) => {
                self.relay_refused(job, id, &reason, retry);
                false
            }
            Err(p) if matches!(p.code, "relay_busy" | "rate_limited" | "unreachable" | "relay_unavailable") => {
                let busy = job["busy"].as_u64().unwrap_or(0) as u32 + 1;
                job["busy"] = json!(busy);
                if busy > MAX_BUSY {
                    Self::fail(job, p.code, &format!("{} Tried {MAX_BUSY} times; nothing was sent. Try again later.", p.message));
                    return false;
                }
                let wait = p.retry_after.map_or(BUSY_WAIT_MS, |s| s.max(1) * 1000);
                job["next_try"] = json!(now_ms() + wait);
                job["note"] = json!(format!("{}. Asking again in {} s (the same signed claim; nothing new to approve).", p.message.trim_end_matches('.'), wait / 1000));
                false
            }
            Err(p) => {
                Self::fail(job, "relay_refused", &format!("The relay refused it: {}. Nothing was sent.", p.message));
                false
            }
        }
    }

    /// A refusal from the relay. "Stale nonce" (its payer had another claim in flight) is the
    /// "busy" case: the wallet re-signs the same call (`resign_relayed_call`), or, past what it
    /// allows, the person is asked again.
    fn relay_refused(&mut self, job: &mut Value, id: &str, reason: &str, retry: bool) {
        let r = reason.to_lowercase();
        if r.contains("already claimed") || r.contains("nullifier is spent") {
            return Self::fail(job, "already_claimed", "This deposit was claimed already; nothing was sent.");
        }
        if r.contains("already relayed") {
            return Self::fail(job, "relay_once", &format!("The relay sends each claim once and has sent this one before ({reason}). If it never landed, the relay's operator can release it."));
        }
        if r.contains("daily cap") || r.contains("too many") {
            return Self::fail(job, "relay_cap", &format!("The relay's limit is reached: {reason}. Nothing was sent; try again tomorrow (UTC)."));
        }
        if r.contains("out of funds") {
            return Self::fail(job, "relay_funds", "The relay is out of funds for now; nothing was sent. Try again later.");
        }
        if r.contains("not attested") {
            return Self::fail(job, "root_not_attested", "The bridge hasn't recorded the root this claim uses; nothing was sent. Check the claim again in a minute.");
        }
        if r.contains("nonce") {
            // "Busy": the relay's payer had another claim in flight. Wait for the chain to show
            // its nonce moved, then the wallet signs the same call again for the new nonce.
            let _ = retry;
            return Self::await_payer(job, reason);
        }
        Self::fail(job, "relay_refused", &format!("The relay refused the claim: {reason}. Nothing was sent."))
    }

    fn await_payer(job: &mut Value, why: &str) {
        job["phase"] = json!("relay");
        job["awaiting_payer"] = json!(true);
        if job["busy_since"].is_null() {
            job["busy_since"] = json!(now_ms());
        }
        job["busy_reason"] = json!(why.chars().take(200).collect::<String>());
        job["note"] = json!("The bridge's relay is busy: its account has another claim in flight. When that lands, your wallet signs this same claim again for the relay's new nonce (no new approval) and it goes out.");
    }

    /// While the relay's payer has another claim in flight: re-sign once the chain shows its
    /// nonce moved. True when the job may post again now.
    fn payer_moved(&mut self, job: &mut Value, id: &str) -> bool {
        let Ok(rpc) = self.job_rpc(job) else { return false };
        let payer = job["meta"]["payer"].as_str().unwrap_or_default().to_owned();
        let acct = job["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
        let Ok(n) = lez::nonces(rpc.as_ref(), &[acct, payer]) else { return false };
        let before_a: u128 = job["nonce_account"].as_str().and_then(|s| s.parse().ok()).unwrap_or(n[0]);
        let before_p: u128 = job["nonce_payer"].as_str().and_then(|s| s.parse().ok()).unwrap_or(n[1]);
        if n[0] != before_a {
            // The account was used meanwhile: perhaps this very claim landed.
            let bridge = util::lez_account(job["meta"]["bridge"].as_str().unwrap_or(&self.deployment().bridge_program)).unwrap_or([0; 32]);
            let nullifier = util::hex32(job["meta"]["nullifier"].as_str().unwrap_or_default()).unwrap_or([0; 32]);
            if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nullifier)).unwrap_or(false) {
                Self::fail(job, "already_claimed", "This deposit was claimed meanwhile (perhaps by this same claim through another route). Check the account's holdings.");
                return false;
            }
        }
        let waited = now_ms().saturating_sub(job["busy_since"].as_u64().unwrap_or(0));
        if n[1] > before_p || waited > 180_000 {
            job["awaiting_payer"] = json!(false);
            let why = job["busy_reason"].as_str().unwrap_or("busy").to_owned();
            self.ask_again_or_resign(job, id, &why);
            // The wallet busy or silent leaves it awaiting: nothing is posted on the old signature.
            return job["phase"] == "relay" && job["state"] == "running" && job["resign_waits"] != true && job["sign_again"] != true;
        }
        false
    }

    /// The payer's nonce moved: re-sign without the person if the wallet allows it, else
    /// propose again (a new approval).
    fn ask_again_or_resign(&mut self, job: &mut Value, id: &str, why: &str) {
        if job["host"] == "names_app" {
            // The Names app signed it, so the Names app signs it again (same call, the relay's new
            // nonce): `relayed_claim_status` says `sign_again` until `relayed_claim_submit` brings it.
            self.relayed.signed.remove(id);
            job["sign_again"] = json!(true);
            job["awaiting_payer"] = json!(false);
            job["phase"] = json!("relay");
            job["note"] = json!(format!("The bridge's relay was busy with another claim ({why}): your Names Wallet signs this same claim again for the relay's new nonce, then it goes out."));
            return;
        }
        let rid = job["request"].as_str().unwrap_or_default().to_owned();
        job["resign_waits"] = Value::Null;
        // Asked once: this runs inside a poll, and the next poll asks again.
        let answer = match wallet::names_once("resign_relayed_call", json!([rid])) {
            // Busy, no answer or not reachable says nothing about the call: never "ask again" (a
            // new proposal) on that. A re-sign is safe to ask twice (the wallet answers
            // `unchanged` or signs the same call again), so the next poll simply asks again.
            Err(f) if f.transient() => {
                job["awaiting_payer"] = json!(true);
                job["resign_waits"] = json!(true);
                job["note"] = json!(format!("The relay's account is free again; asking your Names Wallet to sign the same claim again: {}. Asking again shortly.", f.short()));
                return;
            }
            other => other,
        };
        // The wallet's `code` first (Names 0.17: unchanged, ask_again, landed_or_used, refused),
        // its words second (older builds give no code).
        let code = match &answer {
            Err(wallet::Fail::Refused { code: Some(c), .. }) => c.clone(),
            Err(wallet::Fail::Refused { error: said, code: None }) if said.contains("landed_or_used") || said.contains("may have landed") || said.contains("your account was used") => "landed_or_used".to_owned(),
            Err(wallet::Fail::Refused { error: said, code: None }) if said.contains("hasn't moved") => "unchanged".to_owned(),
            Err(_) => "ask_again".to_owned(),
            Ok(_) => String::new(),
        };
        let answer = answer.map_err(|f| f.text());
        match answer {
            Ok(r) if r["transaction"].is_string() => {
                self.relayed.signed.insert(id.to_owned(), r["transaction"].as_str().unwrap().to_owned());
                job["resigns"] = json!(job["resigns"].as_u64().unwrap_or(0) + 1);
                job["hash"] = r["hash"].clone();
                job["awaiting_payer"] = json!(false);
                job["busy_since"] = Value::Null;
                self.note_payer_nonce(job);
                job["phase"] = json!("relay");
                job["note"] = json!(format!("The relay was busy with another claim ({why}); your wallet signed the same claim again for its new nonce, no new approval needed."));
            }
            Err(_) if code == "landed_or_used" => {
                // The account was used since the approval: this claim may have landed. Ask the
                // chain (the nullifier), never re-propose in a loop.
                job["phase"] = json!("confirm");
                job["landed_check"] = json!(true);
                job["note"] = json!("Your account was used since you approved: checking whether this claim already landed.");
            }
            Err(said) if code == "unchanged" => {
                // The transaction already given is still good: the relay's other claim hasn't
                // landed yet. Wait for it.
                Self::await_payer(job, &said);
            }
            refused => {
                let said = refused.err().unwrap_or_else(|| "no transaction".to_owned());
                // A new proposal: the person approves again (the wallet's rule, SIGNING.md §6).
                let Some(c) = self.relayed.checks.get(job["check"].as_str().unwrap_or_default()) else {
                    return Self::fail(job, "busy", &format!("The relay was busy and the wallet won't sign again without you ({said}). Check the claim file again to propose it anew."));
                };
                let (account, call, summary) = (c.file.account_text(), c.call.to_string(), c.summary.clone());
                match wallet::names_try("propose_relayed_call", json!([format!("Public/{account}"), call, summary])) {
                    Ok(r) if r["request"].is_string() => {
                        job["request"] = r["request"].clone();
                        job["proposed_at"] = json!(now_ms());
                        job["proposals"] = json!(job["proposals"].as_u64().unwrap_or(1) + 1);
                        job["phase"] = json!("approve");
                        job["note"] = json!(format!("The relay was busy with another claim and your wallet won't sign it again without you ({said}). Approve it once more in the Names app."));
                    }
                    Err(wallet::Fail::NoAnswer) => {
                        // Perhaps taken, id unknown: no blind second proposal for this claim.
                        let check = job["check"].as_str().unwrap_or_default().to_owned();
                        self.mark_uncertain(&format!("propose:{check}"));
                        Self::fail(job, "wallet_no_answer", "The relay was busy and your Names Wallet didn't answer when asked to approve this claim once more, so that request may be waiting in the Names app. This app can't follow it: reject it there if you see it, then check the claim file again. Nothing was sent.");
                    }
                    Ok(_) | Err(_) => Self::fail(job, "busy", &format!("The relay was busy and a new request couldn't be made ({said}). Nothing was sent.")),
                }
            }
        }
    }

    fn confirm_step(&mut self, job: &mut Value, id: &str) {
        let Ok(rpc) = self.job_rpc(job) else { return };
        let hash = job["tx"].as_str().unwrap_or_default().to_owned();
        // The bridge the claim was checked on (kept in the job; older jobs: the current one).
        let bridge = util::lez_account(job["meta"]["bridge"].as_str().unwrap_or(&self.deployment().bridge_program)).unwrap_or([0; 32]);
        let nullifier = util::hex32(job["meta"]["nullifier"].as_str().unwrap_or_default()).unwrap_or([0; 32]);
        if job["landed_check"] == true && job["tx"].is_null() {
            // The wallet says the account was used since the approval (`landed_or_used`).
            if lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nullifier)).unwrap_or(false) {
                let acct = job["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
                let a = lez::account(rpc.as_ref(), &acct).ok().flatten();
                let def = util::lez_account(job["meta"]["definition"].as_str().unwrap_or_default()).unwrap_or([0; 32]);
                let bal = lez::token_balance(a.as_ref(), job["meta"]["token_program"].as_str().unwrap_or_default(), &def).unwrap_or(0);
                let decimals = job["meta"]["decimals"].as_u64().unwrap_or(18) as u8;
                job["state"] = json!("done");
                job["phase"] = json!("done");
                job["finished"] = json!(now_ms());
                job["note"] = json!("This deposit is claimed (its nullifier is spent); the transaction that did it isn't known here.");
                job["result"] = json!({ "tx": null, "block": null, "ok": true, "balance": bal.to_string(),
                    "balance_text": format!("{} {}", format_amount(bal, decimals), job["meta"]["symbol"].as_str().unwrap_or("")),
                    "native": lez::native_balance(a.as_ref()).to_string(), "wallet_note": "Your Names Wallet's LEZ 0.4 view (the bridge test network) shows this balance." });
                return;
            }
            return Self::fail(job, "landed_or_used", "Your account was used since you approved this claim, and the deposit is still unclaimed: check your account's recent activity, then check the claim file again to propose it anew. Nothing more was sent.");
        }
        match lez::tx_block(rpc.as_ref(), &hash) {
            Ok(Some(block)) => {
                let spent = lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nullifier)).unwrap_or(false);
                if !spent {
                    return Self::fail(job, "reverted", &format!("The claim was included in block {block} but did not apply (reverted): minting may have been paused in the same block. The relay paid its fee."));
                }
                let acct = job["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
                let a = lez::account(rpc.as_ref(), &acct).ok().flatten();
                let def = util::lez_account(job["meta"]["definition"].as_str().unwrap_or_default()).unwrap_or([0; 32]);
                let tp = job["meta"]["token_program"].as_str().unwrap_or_default();
                let decimals = job["meta"]["decimals"].as_u64().unwrap_or(18) as u8;
                let bal = lez::token_balance(a.as_ref(), tp, &def).unwrap_or(0);
                let native = lez::native_balance(a.as_ref());
                job["block"] = json!(block);
                job["state"] = json!("done");
                job["phase"] = json!("done");
                job["finished"] = json!(now_ms());
                job["note"] = Value::Null;
                job["result"] = json!({
                    "tx": hash, "block": block, "ok": true,
                    "balance": bal.to_string(), "balance_text": format!("{} {}", format_amount(bal, decimals), job["meta"]["symbol"].as_str().unwrap_or("")),
                    "native": native.to_string(),
                    "wallet_note": "Your Names Wallet's LEZ 0.4 view (the bridge test network) shows this balance.",
                });
                self.relayed.checks.remove(job["check"].as_str().unwrap_or_default());
            }
            Ok(None) => {
                let waited = now_ms().saturating_sub(job["submitted_at"].as_u64().unwrap_or(0));
                if waited < CONFIRM_PATIENCE_MS {
                    job["note"] = json!("Sent by the relay; waiting for a block (about 15 s each).");
                    return;
                }
                // Out of a block for long: ask the chain why (busy = the payer's nonce moved).
                let spent = lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::nullifier_seed(&nullifier)).unwrap_or(false);
                if spent {
                    return Self::fail(job, "already_claimed", "This deposit was claimed by another transaction meanwhile.");
                }
                let payer = job["meta"]["payer"].as_str().unwrap_or_default().to_owned();
                let acct = job["account"].as_str().unwrap_or_default().trim_start_matches("Public/").to_owned();
                match lez::nonces(rpc.as_ref(), &[acct, payer]) {
                    Ok(n) => {
                        let before_a: u128 = job["nonce_account"].as_str().and_then(|s| s.parse().ok()).unwrap_or(n[0]);
                        let before_p: u128 = job["nonce_payer"].as_str().and_then(|s| s.parse().ok()).unwrap_or(n[1]);
                        if n[0] == before_a && n[1] > before_p {
                            job["dropped"] = json!(hash);
                            return self.ask_again_or_resign(job, id, "the network dropped it: the relay's account was used by another claim first");
                        }
                        job["note"] = json!(format!("Sent by the relay {} s ago and not in a block yet; still waiting.", waited / 1000));
                    }
                    Err(p) => job["note"] = json!(format!("Waiting for a block; {} can't be read now ({}).", self.deployment().network, p.message)),
                }
            }
            Err(p) => job["note"] = json!(format!("Waiting for a block; {} can't be read now ({}).", self.deployment().network, p.message)),
        }
    }

    pub(crate) fn relayed_cancel_now(&mut self, id: &str) -> Result<Value, String> {
        let mut job = self.wstate()["relayed_jobs"].as_array().and_then(|l| l.iter().find(|j| j["id"] == id).cloned()).ok_or_else(|| format!("there is no claim job {id}"))?;
        if job["state"] != "running" || job["phase"] != "approve" {
            return Err("only a claim still waiting for your approval can be cancelled; once signed it is the relay's to send".to_owned());
        }
        match wallet::names_try("cancel_request", json!([job["request"]])) {
            // Busy or silent: the request may still be waiting, so the job isn't marked cancelled.
            Err(wallet::Fail::Busy) => return Err("Your Names Wallet is busy; nothing was cancelled; try again.".to_owned()),
            // Silent or gone: this app stops following the request (it can't take it back). If it
            // is approved later, the wallet signs and sends nothing, and nobody posts it.
            Err(f) if f.transient() => {
                Self::cancelled(&mut job, id, "Your Names Wallet didn't answer, so this app stopped following the claim; nothing was sent. If the Names app still shows it, reject it there.");
                self.relayed.checks.remove(job["check"].as_str().unwrap_or_default());
                self.put_job(&job)?;
                return Ok(job_view(&job, now_ms()));
            }
            // Refused (it was answered or lapsed meanwhile) or done: the job ends here as before.
            _ => {}
        }
        Self::cancelled(&mut job, id, "Cancelled; nothing was signed or sent.");
        self.relayed.checks.remove(job["check"].as_str().unwrap_or_default());
        self.put_job(&job)?;
        Ok(job_view(&job, now_ms()))
    }

    /// Forgets a checked claim (its key leaves memory).
    pub(crate) fn relayed_forget(&mut self, id: &str) -> Value {
        json!({ "forgotten": self.relayed.checks.remove(id).is_some() })
    }
}

/// Dry-run evidence on a live deployment (tests only, reads only): every finalized deposit's leaf
/// rebuilt with the Config's denominations, each prefix root equal to the vault's own, and
/// which of the newest roots the bridge has recorded.
#[cfg(test)]
pub fn vault_tree_report(b: &mut Bridge) -> Result<Value, Refusal> {
    let d = b.deployment().clone();
    let rpc = b.lez_rpc()?;
    let bridge = util::lez_account(&d.bridge_program).unwrap();
    let cfg = lez::bridge_config(rpc.as_ref(), &bridge).map_err(|e| Refusal { code: "bridge_unreadable", message: e })?.ok_or(Refusal { code: "no_bridge", message: "no bridge".into() })?;
    let (events, finalized, latest) = b.events().map_err(|e| Refusal { code: "eth_unreachable", message: e })?;
    let fin: Vec<&DepositEvent> = events.iter().filter(|e| e.block <= finalized.number).collect();
    let leaves: Vec<[u8; 32]> = fin.iter().map(|e| leaf_of(e, &cfg)).collect::<Result<_, _>>()?;
    let mut matched = 0;
    for j in 0..leaves.len() {
        let path = merkle_path(&leaves[..=j], j);
        if merkle_root(&leaves[j], j as u64, &path) == fin[j].root {
            matched += 1;
        }
    }
    let mut recorded = Vec::new();
    for j in (0..leaves.len()).rev().take(MAX_ROOT_TRIES) {
        recorded.push(json!({ "after_leaf": j, "recorded": lez::bridge_pda_used(rpc.as_ref(), &bridge, &lez::root_seed(&fin[j].root))? }));
    }
    Ok(json!({ "deposits": events.len(), "finalized_deposits": fin.len(), "roots_matching_the_vault": matched, "eth_finalized": finalized.number, "eth_latest": latest,
               "newest_roots": recorded, "token_program": lez::b58(&cfg.token_program), "root_source": cfg.root_source }))
}

#[cfg(test)]
pub fn test_refused(b: &mut Bridge, job: &mut Value, reason: &str) {
    b.relay_refused(job, "x", reason, false);
}

/// The phases a relayed claim shows, in order.
pub const PHASES: [&str; 4] = ["approve", "relay", "confirm", "done"];

/// A job as the mini-app shows it: phases reached, state, plain note, result or error. Never
/// the transaction or a secret.
pub fn job_view(job: &Value, now: u64) -> Value {
    let mut v = json!({});
    for k in ["id", "kind", "state", "phase", "account", "network", "tx", "block", "error", "result", "note", "proposals", "resigns", "started", "finished", "request", "check", "host"] {
        v[k] = job[k].clone();
    }
    v["amount_text"] = job["meta"]["amount_text"].clone();
    v["symbol"] = job["meta"]["symbol"].clone();
    let phase = job["phase"].as_str().unwrap_or("approve");
    let idx = PHASES.iter().position(|p| *p == phase).unwrap_or(0);
    v["phases"] = json!(PHASES.iter().enumerate().map(|(i, p)| json!({ "phase": p, "reached": i <= idx })).collect::<Vec<_>>());
    v["elapsed_s"] = json!(now.saturating_sub(job["started"].as_u64().unwrap_or(now)) / 1000);
    v["can_cancel"] = json!(job["state"] == "running" && phase == "approve");
    // Hosted by the Names app: it signs the same claim again (`relayed_claim_build` gives the same
    // call) and hands it over with `relayed_claim_submit`.
    v["needs"] = if job["state"] == "running" && job["sign_again"] == true { json!("sign_again") } else { Value::Null };
    v["wording"] = json!(WORDING);
    v
}

fn short(id: &str) -> String {
    let id = id.trim_start_matches("Public/");
    if id.len() <= 12 {
        id.to_owned()
    } else {
        format!("{}…{}", &id[..6], &id[id.len() - 4..])
    }
}
