// SPDX-License-Identifier: MIT OR Apache-2.0
//! LEZ reads over plain JSON-RPC, from this module's own process, with no LEZ library and no
//! CLI: the bridge Config, the root and nullifier PDAs, accounts' nonces and balances, a
//! transaction's block, and the claim relay's two methods (`bridge_relayInfo`,
//! `bridge_relayClaim`), which the bridge test network's RPC filter serves
//! (`public-demo/proxy/lez-rpc-proxy.py --relay`). Layouts: `public-demo/WALLET-READ.md` (not published).
//!
//! The Names Wallet path of a public claim needs nothing else from LEZ: the wallet builds and
//! signs the transaction (`propose_relayed_call`) and the relay sends it. Unlike the CLI, this
//! reads any http(s) endpoint, the public one included: it only reads, and the one write it can
//! make is handing the relay a transaction the wallet signed after the person approved it.

use crate::util::{sha256, to_hex};
use serde_json::{json, Value};
use std::time::Duration;

/// The native token program id (32 zero bytes).
pub const NATIVE: &str = "11111111111111111111111111111111";
/// `for_public_pda`'s prefix (LEZ v0.4.0-rc3 `lee/state_machine/core/src/program/mod.rs`).
const PDA_PREFIX: &[u8; 32] = b"/LEE/v0.2/AccountId/PDA/\0\0\0\0\0\0\0\0";
pub const CONFIG_SEED: [u8; 32] = *b"/RFP022/bridge/config/v1/000000/";
const ROOT_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/root/v1";
const NULLIFIER_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/claim-nullifier/v1";

pub fn b58(bytes: &[u8; 32]) -> String {
    bs58::encode(bytes).into_string()
}

/// A public PDA: `SHA256(prefix | program | seed)`.
pub fn pda(program: &[u8; 32], seed: &[u8; 32]) -> [u8; 32] {
    sha256(&[PDA_PREFIX, program, seed])
}

pub fn root_seed(root: &[u8; 32]) -> [u8; 32] {
    sha256(&[ROOT_SEED_DOMAIN, root])
}

pub fn nullifier_seed(nullifier: &[u8; 32]) -> [u8; 32] {
    sha256(&[NULLIFIER_SEED_DOMAIN, nullifier])
}

/// An RPC failure, with what it means for the person.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// `unreachable`, `rate_limited`, `relay_busy`, `relay_unavailable`, `refused`.
    pub code: &'static str,
    pub message: String,
    /// Seconds the endpoint asked to wait (HTTP `Retry-After`).
    pub retry_after: Option<u64>,
}

impl Problem {
    fn new(code: &'static str, message: String) -> Problem {
        Problem { code, message, retry_after: None }
    }
}

/// Where the LEZ reads go. `Http` in the module; tests use [`Rpc::Recorded`].
pub trait LezRpc {
    fn call(&self, method: &str, params: Value) -> Result<Value, Problem>;
    fn describe(&self) -> String;
}

pub struct Http {
    pub url: String,
    pub timeout: Duration,
}

impl Http {
    pub fn new(url: &str) -> Http {
        Http { url: url.to_owned(), timeout: Duration::from_secs(20) }
    }
}

impl LezRpc for Http {
    fn describe(&self) -> String {
        self.url.clone()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, Problem> {
        // The relay checks, co-signs and submits within one call (up to two minutes).
        let timeout = if method == "bridge_relayClaim" { Duration::from_secs(150) } else { self.timeout };
        let agent = ureq::AgentBuilder::new().timeout(timeout).user_agent("logos-lez-bridge-module/0.1").build();
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let reply = match agent.post(&self.url).set("Content-Type", "application/json").send_json(body) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let after = r.header("Retry-After").and_then(|s| s.trim().parse().ok());
                let text = r.into_string().unwrap_or_default();
                let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_owned)).unwrap_or_default();
                let mut p = match code {
                    429 => Problem::new("rate_limited", format!("the bridge test network's endpoint asks us to slow down ({said}); wait a little and try again")),
                    503 if said.contains("relay busy") => Problem::new("relay_busy", "the bridge's relay is busy with another claim".to_owned()),
                    502 if said.contains("relay") => Problem::new("relay_unavailable", "the bridge's relay is not answering right now; nothing was sent".to_owned()),
                    _ => Problem::new("unreachable", format!("{} answered HTTP {code} to {method}{}", self.url, if said.is_empty() { String::new() } else { format!(": {said}") })),
                };
                p.retry_after = after;
                return Err(p);
            }
            Err(e) => return Err(Problem::new("unreachable", format!("could not reach {} ({e})", self.url))),
        };
        let v: Value = reply.into_json().map_err(|e| Problem::new("unreachable", format!("{} answered something that isn't JSON: {e}", self.url)))?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            let said = err["message"].as_str().unwrap_or("error").to_owned();
            return Err(Problem::new("refused", format!("{} refused {method}: {said}", self.url)));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// What a read says when the endpoint kept answering "slow down" past the call's budget.
pub const BUSY_TEXT: &str = "The bridge's endpoint is busy; trying again shortly.";

/// The deployment's endpoint, patient with its rate limits: a read answered HTTP 429 ("slow down",
/// the RPC filter's per-client buckets or the edge's rule) is asked again after the endpoint's
/// `Retry-After` (1 to 10 s), until `budget_ms` is spent; then it answers `rate_limited` with
/// [`BUSY_TEXT`]. A 429 is never "taken", so asking again is safe. The relay's two sends
/// (`bridge_relayClaim`, `bridge_relayBurn`) are passed through: their callers retry on their own
/// schedule (the claim job, the send-back's next poll).
pub struct Patient {
    pub inner: std::sync::Arc<dyn LezRpc + Send + Sync>,
    pub budget_ms: u64,
}

/// The time a call may spend waiting out 429s.
pub const PATIENCE_MS: u64 = 20_000;

impl Patient {
    pub fn new(inner: std::sync::Arc<dyn LezRpc + Send + Sync>) -> Patient {
        Patient { inner, budget_ms: PATIENCE_MS }
    }
}

impl LezRpc for Patient {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, Problem> {
        let started = crate::wallet::clock::now_ms();
        loop {
            match self.inner.call(method, params.clone()) {
                Err(p) if p.code == "rate_limited" && !matches!(method, "bridge_relayClaim" | "bridge_relayBurn") => {
                    let wait = p.retry_after.unwrap_or(2).clamp(1, 10) * 1000;
                    if crate::wallet::clock::now_ms().saturating_sub(started) + wait > self.budget_ms {
                        return Err(Problem { code: "rate_limited", message: BUSY_TEXT.to_owned(), retry_after: p.retry_after });
                    }
                    crate::wallet::clock::sleep_ms(wait);
                }
                other => return other,
            }
        }
    }
}

/// The blocks `from..=to` (`getBlockRange`, at most `MAX_RANGE` a call), searched for `needle`:
/// the first block holding it, and the last block read. On a failed read: the block read last
/// before it (so a scan resumes there) and the problem.
pub const MAX_RANGE: u64 = 100;

pub fn scan_blocks(rpc: &dyn LezRpc, needle: &[u8; 32], from: u64, to: u64) -> (Option<u64>, u64, Option<Problem>) {
    let mut last = from.saturating_sub(1);
    let mut a = from;
    while a <= to {
        let b = to.min(a + MAX_RANGE - 1);
        let got = match rpc.call("getBlockRange", json!([a, b])) {
            Ok(v) => v,
            Err(p) => return (None, last, Some(p)),
        };
        let Some(list) = got.as_array() else {
            return (None, last, Some(Problem::new("unreachable", "getBlockRange gave no list".to_owned())));
        };
        for (k, blk) in list.iter().enumerate() {
            let raw = state_bytes(blk).unwrap_or_default();
            if raw.windows(32).any(|w| w == needle) {
                return (Some(a + k as u64), a + k as u64, None);
            }
        }
        last = b;
        a = b + 1;
    }
    (None, last, None)
}

/// Bytes of a JSON actor state: an array of numbers (getAccount) or base64 text.
pub fn state_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Array(a) => a.iter().map(|x| x.as_u64().filter(|n| *n < 256).map(|n| n as u8)).collect(),
        Value::String(s) => base64(s),
        Value::Null => Some(Vec::new()),
        _ => None,
    }
}

fn base64(s: &str) -> Option<Vec<u8>> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = A.iter().position(|a| *a == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `getAccount`: `None` for an account the chain has never seen.
pub fn account(rpc: &dyn LezRpc, id: &str) -> Result<Option<Value>, Problem> {
    let r = rpc.call("getAccount", json!([id]))?;
    Ok(if r.is_null() { None } else { Some(r) })
}

/// The actor state of `program` in an account (empty when absent).
pub fn actor_state(account: &Value, program: &str) -> Option<Vec<u8>> {
    state_bytes(&account["data"]["actor_states"][program])
}

pub fn le_u128(b: &[u8]) -> u128 {
    let mut w = [0u8; 16];
    let n = b.len().min(16);
    w[..n].copy_from_slice(&b[..n]);
    u128::from_le_bytes(w)
}

/// The native balance of an account read with `getAccount` (`None`: never seen = 0).
pub fn native_balance(account: Option<&Value>) -> u128 {
    account.and_then(|a| actor_state(a, NATIVE)).map_or(0, |b| le_u128(&b))
}

/// A token holding: `TokenState::Holdings` (tag 0, u32 n, n × (definition 32, u128)).
pub fn token_balance(account: Option<&Value>, token_program: &str, definition: &[u8; 32]) -> Result<u128, String> {
    let Some(b) = account.and_then(|a| actor_state(a, token_program)) else { return Ok(0) };
    if b.is_empty() {
        return Ok(0);
    }
    let mut r = Reader::new(&b);
    if r.u8()? != 0 {
        return Ok(0);
    }
    let n = r.u32()?;
    for _ in 0..n {
        let d = r.b32()?;
        let v = r.u128()?;
        if &d == definition {
            return Ok(v);
        }
    }
    Ok(0)
}

pub fn nonces(rpc: &dyn LezRpc, ids: &[String]) -> Result<Vec<u128>, Problem> {
    let r = rpc.call("getAccountsNonces", json!([ids]))?;
    let list = r.as_array().ok_or_else(|| Problem::new("unreachable", "getAccountsNonces gave no list".to_owned()))?;
    list.iter()
        .map(|n| {
            let n = if let Some(a) = n.as_array() { a.first().cloned().unwrap_or(Value::Null) } else { n.clone() };
            n.as_u64().map(u128::from).or_else(|| n.as_str().and_then(|s| s.parse().ok())).ok_or_else(|| Problem::new("unreachable", "a nonce that isn't a number".to_owned()))
        })
        .collect()
}

/// `getTransaction`: the block that holds it, or `None` while it is in no block.
pub fn tx_block(rpc: &dyn LezRpc, hash: &str) -> Result<Option<u64>, Problem> {
    let r = rpc.call("getTransaction", json!([hash]))?;
    Ok(r.as_array().and_then(|a| a.get(1)).and_then(Value::as_u64))
}

/// The relay's payer and caps (`bridge_relayInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct RelayInfo {
    pub payer: String,
    pub bridge: Option<String>,
    pub max_gas: u64,
    pub max_fee: u128,
}

pub fn relay_info(rpc: &dyn LezRpc) -> Result<RelayInfo, Problem> {
    parse_relay_info(&rpc.call("bridge_relayInfo", json!([]))?)
}

/// [`relay_info`] of an answer already read.
pub fn parse_relay_info(v: &Value) -> Result<RelayInfo, Problem> {
    let payer = v["payer"].as_str().filter(|p| crate::util::lez_account(p).is_ok()).ok_or_else(|| Problem::new("relay_unavailable", "the relay names no payer account".to_owned()))?;
    let max_gas = v["max_gas"].as_u64().filter(|g| *g > 0).ok_or_else(|| Problem::new("relay_unavailable", "the relay gives no gas cap".to_owned()))?;
    let max_fee = v["max_fee"].as_str().and_then(|s| s.parse().ok()).or_else(|| v["max_fee"].as_u64().map(u128::from)).filter(|f| *f > 0)
        .ok_or_else(|| Problem::new("relay_unavailable", "the relay gives no fee cap".to_owned()))?;
    Ok(RelayInfo { payer: payer.to_owned(), bridge: v["bridge"].as_str().map(str::to_owned), max_gas, max_fee })
}

/// What the relay answered to a half-signed claim.
#[derive(Clone, Debug, PartialEq)]
pub enum Relayed {
    Submitted(String),
    Refused { reason: String, retry: bool },
}

/// Hands the relay a transaction the Names Wallet signed (`bridge_relayClaim [hex, false]`). The
/// only write this module makes, and only from a claim job after the person approved it.
pub fn relay_claim(rpc: &dyn LezRpc, tx_hex: &str) -> Result<Relayed, Problem> {
    let v = rpc.call("bridge_relayClaim", json!([tx_hex, false]))?;
    if v["ok"] == true {
        let h = v["tx_hash"].as_str().filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
        return h.map(|h| Relayed::Submitted(h.to_owned())).ok_or_else(|| Problem::new("relay_unavailable", "the relay said ok but gave no transaction hash".to_owned()));
    }
    Ok(Relayed::Refused { reason: v["reason"].as_str().unwrap_or("refused without a reason").chars().take(400).collect(), retry: v["retry"] == true })
}

// ── The bridge Config (borsh, fail-closed) ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct TokenEntry {
    pub eth_token: [u8; 20],
    pub definition: [u8; 32],
    pub decimals: u8,
    pub denoms: Vec<u128>,
    pub registered: bool,
    pub mint_frozen: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BridgeConfig {
    pub cfg_id: [u8; 32],
    pub vault: [u8; 20],
    pub evm_chain_id: u64,
    pub token_program: [u8; 32],
    pub admin: [u8; 32],
    pub freezer: [u8; 32],
    pub attestor: [u8; 32],
    pub mint_frozen: bool,
    pub tokens: Vec<TokenEntry>,
    /// 0: stand-in roots (an attestor records them); 1: an RFP-021 attestation.
    pub root_source: u8,
    /// The LEZ chain the bridge was initialised for (its Bedrock channel id). Zero on a bridge
    /// from before the 2026-11-14 security fixes (the live v2 demo), whose `Init` took any value;
    /// the fixed bridge's `Initialize` refuses zero.
    pub lez_chain_id: [u8; 32],
}

impl BridgeConfig {
    /// A bridge from before the 2026-11-14 security fixes: its claim nullifier is
    /// `SHA256("RFP022/nfc" | k)`. The fixed bridge refuses a zero chain id at init, so a zero one
    /// marks the old program.
    #[must_use]
    pub fn pre_fix(&self) -> bool {
        self.lez_chain_id == [0; 32]
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("the bridge's state is shorter than its layout")?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("a flag in the bridge's state is neither 0 nor 1".to_owned()),
        }
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn u128(&mut self) -> Result<u128, String> {
        Ok(u128::from_le_bytes(self.take(16)?.try_into().unwrap()))
    }
    fn b32(&mut self) -> Result<[u8; 32], String> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn b20(&mut self) -> Result<[u8; 20], String> {
        Ok(self.take(20)?.try_into().unwrap())
    }
    fn count(&mut self, max: u32) -> Result<u32, String> {
        let n = self.u32()?;
        if n > max {
            return Err("a list in the bridge's state is longer than the program allows".to_owned());
        }
        Ok(n)
    }
}

/// Decodes the Config bytes (layout in WALLET-READ.md §3): every byte consumed, or an error.
pub fn decode_config(b: &[u8]) -> Result<BridgeConfig, String> {
    let mut r = Reader::new(b);
    let cfg_id = r.b32()?;
    let vault = r.b20()?;
    let evm_chain_id = r.u64()?;
    let lez_chain_id = r.b32()?;
    r.b32()?; // zone_id
    let token_program = r.b32()?;
    let admin = r.b32()?;
    if r.bool()? {
        r.b32()?; // pending_admin
    }
    let freezer = r.b32()?;
    let attestor = r.b32()?;
    let mint_frozen = r.bool()?;
    r.bool()?; // burn_frozen
    let mut tokens = Vec::new();
    for _ in 0..r.count(32)? {
        let eth_token = r.b20()?;
        let definition = r.b32()?;
        let decimals = r.u8()?;
        let mut denoms = Vec::new();
        for _ in 0..r.count(8)? {
            denoms.push(r.u128()?);
        }
        for _ in 0..r.count(8)? {
            r.u64()?; // claims
        }
        for _ in 0..r.count(8)? {
            r.u64()?; // burns
        }
        r.u128()?; // claimed_value
        r.u128()?; // burned_value
        let registered = r.bool()?;
        let token_frozen = r.bool()?;
        r.bool()?; // burn_frozen
        tokens.push(TokenEntry { eth_token, definition, decimals, denoms, registered, mint_frozen: token_frozen });
    }
    let root_source = r.u8()?;
    match root_source {
        0 => {}
        1 => {
            r.b32()?;
            r.b32()?;
            r.u64()?;
        }
        _ => return Err("the bridge's root source is of a kind this module doesn't know".to_owned()),
    }
    r.u128()?; // sponsor_amount
    if r.at != b.len() {
        return Err(format!("the bridge's state has {} bytes this module can't read; it may be a newer bridge", b.len() - r.at));
    }
    Ok(BridgeConfig { cfg_id, vault, evm_chain_id, token_program, admin, freezer, attestor, mint_frozen, tokens, root_source, lez_chain_id })
}

/// The bridge Config as the CLI's `show --json` gives it (CLI-CONTRACT §2.1), read natively, for
/// the Overview and the registry when no `lez-bridge` CLI is set (the Names Wallet mode in
/// Basecamp): every field `shape_registry` reads, the float (the Config account's native balance)
/// included. Fail-closed: every byte read.
pub fn show_json(rpc: &dyn LezRpc, bridge: &[u8; 32]) -> Result<Value, String> {
    let id = b58(&pda(bridge, &CONFIG_SEED));
    let acct = account(rpc, &id).map_err(|p| p.message)?.ok_or("there is no bridge Config on this network (it may have been reset)")?;
    let b = actor_state(&acct, &b58(bridge)).ok_or("the bridge's state isn't readable")?;
    let mut r = Reader::new(&b);
    let cfg_id = r.b32()?;
    let vault = r.b20()?;
    let evm_chain_id = r.u64()?;
    let lez_chain_id = r.b32()?;
    let zone_id = r.b32()?;
    let token_program = r.b32()?;
    let admin = r.b32()?;
    let pending_admin = if r.bool()? { Some(b58(&r.b32()?)) } else { None };
    let freezer = r.b32()?;
    let attestor = r.b32()?;
    let mint_frozen = r.bool()?;
    let burn_frozen = r.bool()?;
    let mut tokens = Vec::new();
    for _ in 0..r.count(32)? {
        let eth_token = r.b20()?;
        let definition = r.b32()?;
        let decimals = r.u8()?;
        let denoms: Vec<String> = (0..r.count(8)?).map(|_| r.u128().map(|v| v.to_string())).collect::<Result<_, _>>()?;
        let claims: Vec<u64> = (0..r.count(8)?).map(|_| r.u64()).collect::<Result<_, _>>()?;
        let burns: Vec<u64> = (0..r.count(8)?).map(|_| r.u64()).collect::<Result<_, _>>()?;
        let claimed = r.u128()?;
        let burned = r.u128()?;
        let registered = r.bool()?;
        let t_mint = r.bool()?;
        let t_burn = r.bool()?;
        tokens.push(json!({ "eth_token": format!("0x{}", to_hex(&eth_token)), "definition": b58(&definition), "decimals": decimals, "denoms": denoms,
            "claims": claims, "burns": burns, "claimed_value": claimed.to_string(), "burned_value": burned.to_string(),
            "registered": registered, "mint_frozen": t_mint, "burn_frozen": t_burn }));
    }
    let root_source = match r.u8()? {
        0 => json!({ "kind": "stand-in" }),
        1 => {
            let program = r.b32()?;
            let instance = r.b32()?;
            let slot = r.u64()?;
            json!({ "kind": "rfp-021", "program": b58(&program), "instance": hex32_text(&instance), "slot": slot })
        }
        _ => return Err("the bridge's root source is of a kind this module doesn't know".to_owned()),
    };
    let sponsor = r.u128()?;
    if r.at != b.len() {
        return Err(format!("the bridge's state has {} bytes this module can't read; it may be a newer bridge", b.len() - r.at));
    }
    Ok(json!({
        "cfg_id": hex32_text(&cfg_id), "vault": format!("0x{}", to_hex(&vault)), "evm_chain_id": evm_chain_id,
        "lez_chain_id": hex32_text(&lez_chain_id), "zone_id": hex32_text(&zone_id), "token_program": b58(&token_program),
        "admin": b58(&admin), "pending_admin": pending_admin, "freezer": b58(&freezer), "attestor": b58(&attestor),
        "mint_frozen": mint_frozen, "burn_frozen": burn_frozen, "root_source": root_source,
        "sponsor_amount": sponsor.to_string(), "float": native_balance(Some(&acct)).to_string(), "tokens": tokens,
    }))
}

/// The bridge's Config, read from its PDA.
pub fn bridge_config(rpc: &dyn LezRpc, bridge: &[u8; 32]) -> Result<Option<BridgeConfig>, String> {
    let id = b58(&pda(bridge, &CONFIG_SEED));
    let acct = account(rpc, &id).map_err(|p| p.message)?;
    let Some(acct) = acct else { return Ok(None) };
    let bytes = actor_state(&acct, &b58(bridge)).ok_or("the bridge's state isn't readable")?;
    if bytes.is_empty() {
        return Ok(None);
    }
    decode_config(&bytes).map(Some)
}

/// The chain's Bedrock channel id, from the `sequencer_stake` program's config actor (LEZ
/// v0.4.0-rc3 `lez/programs/sequencer_stake/core`: program id = the builtin name
/// "sequencer_stake", config PDA seed `/LEZ/v0.3/MinSequencerStake/0000`, state
/// `{ channel_params: Option<ChannelParams>, channel_id: Option<[u8; 32]>, .. }`). `None` if the
/// chain records none. No LEZ program can read this; a client can (security review 2026-11-14, M1).
pub fn chain_channel_id(rpc: &dyn LezRpc) -> Result<Option<[u8; 32]>, String> {
    let program = sha256(&[b"/LEE-BuiltinProgram/v1/AccountId", b"sequencer_stake"]);
    let id = b58(&pda(&program, b"/LEZ/v0.3/MinSequencerStake/0000"));
    let Some(acct) = account(rpc, &id).map_err(|p| p.message)? else { return Ok(None) };
    let Some(bytes) = actor_state(&acct, &b58(&program)) else { return Ok(None) };
    decode_channel_id(&bytes)
}

/// The `channel_id` of a `SequencerStakeConfig` (the fields before it only; the entries after it
/// are not read).
pub fn decode_channel_id(b: &[u8]) -> Result<Option<[u8; 32]>, String> {
    let mut r = Reader::new(b);
    if r.bool()? {
        // ChannelParams: minimum_sequencer_stake u128, posting_timeframe u32, posting_timeout u32,
        // exit_delay u64, max_block_size u64.
        r.u128()?;
        r.u32()?;
        r.u32()?;
        r.u64()?;
        r.u64()?;
    }
    if r.bool()? { Ok(Some(r.b32()?)) } else { Ok(None) }
}

/// Whether a PDA of the bridge holds state (an attested root, a spent nullifier).
pub fn bridge_pda_used(rpc: &dyn LezRpc, bridge: &[u8; 32], seed: &[u8; 32]) -> Result<bool, Problem> {
    let id = b58(&pda(bridge, seed));
    Ok(account(rpc, &id)?.is_some_and(|a| actor_state(&a, &b58(bridge)).is_some_and(|s| !s.is_empty())))
}

pub fn hex32_text(b: &[u8; 32]) -> String {
    format!("0x{}", to_hex(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_address_is_the_documented_one() {
        let bridge: [u8; 32] = bs58::decode("JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7").into_vec().unwrap().try_into().unwrap();
        assert_eq!(b58(&pda(&bridge, &CONFIG_SEED)), "HugSoWM7Hp1eXMqknfCewWNMoraweLG3HkezfNGtqKf3");
    }

    #[test]
    fn the_rc1_config_from_wallet_read_decodes_exactly() {
        // The real Config bytes printed in public-demo/WALLET-READ.md (not published) §3 (rc1, block ~512).
        let raw: Vec<u8> = serde_json::from_str(include_str!("../testdata/lez/config-rc1.json")).unwrap();
        let c = decode_config(&raw).unwrap();
        assert_eq!(hex32_text(&c.cfg_id), "0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11");
        assert_eq!(c.evm_chain_id, 11_155_111);
        assert_eq!(b58(&c.token_program), "FiooSkDaMZWNq4bFFpVXnDm2d2uoTB5S3cZ7kWhfUW4K");
        assert_eq!(c.tokens.len(), 2);
        assert_eq!(c.tokens[0].decimals, 6);
        assert_eq!(c.tokens[0].denoms, vec![100_000_000, 1_000_000_000, 10_000_000_000]);
        assert_eq!(c.root_source, 0);
        assert!(!c.mint_frozen);
        // One byte more or less is refused, never guessed.
        assert!(decode_config(&raw[..raw.len() - 1]).is_err());
        let mut long = raw.clone();
        long.push(0);
        assert!(decode_config(&long).is_err());
    }

    #[test]
    fn the_config_reads_as_the_cli_shows_it() {
        struct One(Value);
        impl LezRpc for One {
            fn call(&self, _m: &str, _p: Value) -> Result<Value, Problem> {
                Ok(self.0.clone())
            }
            fn describe(&self) -> String {
                "one".into()
            }
        }
        let raw: Vec<u8> = serde_json::from_str(include_str!("../testdata/lez/config-rc1.json")).unwrap();
        let acct = json!({ "nonce": 0, "data": { "actor_states": { "11111111111111111111111111111111": [0, 232, 118, 72, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7": raw } } });
        let bridge: [u8; 32] = bs58::decode("JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7").into_vec().unwrap().try_into().unwrap();
        let v = show_json(&One(acct), &bridge).unwrap();
        assert_eq!(v["cfg_id"], "0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11");
        assert_eq!(v["token_program"], "FiooSkDaMZWNq4bFFpVXnDm2d2uoTB5S3cZ7kWhfUW4K");
        assert_eq!(v["tokens"][0]["denoms"], json!(["100000000", "1000000000", "10000000000"]));
        assert_eq!(v["tokens"][0]["claimed_value"], "100000000");
        assert_eq!(v["root_source"]["kind"], "stand-in");
        assert_eq!(v["sponsor_amount"], "25000000");
        assert_eq!(v["float"], "100000000000");
        assert!(show_json(&One(Value::Null), &bridge).is_err());
    }

    #[test]
    fn balances_read_as_documented() {
        // WALLET-READ.md §4: a holder with 23,786,858 native and 0.1 wrapped ETH.
        let acct: Value = serde_json::from_str(r#"{"nonce":1,"data":{"actor_states":{"11111111111111111111111111111111":[106,245,106,1,0,0,0,0,0,0,0,0,0,0,0,0],"FiooSkDaMZWNq4bFFpVXnDm2d2uoTB5S3cZ7kWhfUW4K":[0,1,0,0,0,187,27,44,140,135,242,183,237,110,240,65,47,140,189,131,45,189,53,184,40,21,242,161,235,148,233,111,27,62,7,29,242,0,0,138,93,120,69,99,1,0,0,0,0,0,0,0,0]}}}"#).unwrap();
        assert_eq!(native_balance(Some(&acct)), 23_786_858);
        let def: [u8; 32] = bs58::decode("DbPGo6FeRLp7NaUWxQGC1WXpJkSQe1jbAqYVMQkCuuPw").into_vec().unwrap().try_into().unwrap();
        assert_eq!(token_balance(Some(&acct), "FiooSkDaMZWNq4bFFpVXnDm2d2uoTB5S3cZ7kWhfUW4K", &def).unwrap(), 100_000_000_000_000_000);
        assert_eq!(token_balance(None, "x", &def).unwrap(), 0);
        assert_eq!(native_balance(None), 0);
        assert_eq!(base64("AQID").unwrap(), vec![1, 2, 3]);
    }
}
