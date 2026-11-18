//! Ethereum reads: the vault's registry, caps, fees, delay and `Deposit` events.
//!
//! RFP-022 wants the Ethereum side through the Logos Ethereum core module (with its verified
//! proxy). Another task evaluates those modules, so reads go through [`EthSource`], with two
//! implementations:
//! - [`JsonRpc`]: plain JSON-RPC over HTTPS from this module's own process. Core modules may
//!   make outbound connections (the Names module does; QML views may not), and the logosctl
//!   test calls a Sepolia endpoint through this module to show it.
//! - [`LogosEthModule`]: the seam for `eth_rpc_module` / `verified_proxy`. Not wired: it answers
//!   plainly that it is not available in this build, so choosing it never pretends.
//!
//! Every read is `eth_call`, `eth_getLogs`, `eth_getBlockBy*` or `eth_getCode`: nothing is signed
//! or sent from here.

use crate::util::{hex0x, hex_bytes, selector, to_hex};
use serde_json::{json, Value};
use std::time::Duration;

/// One `Deposit(address indexed token, uint32 denomIndex, bytes32 commitment, uint64 leafIndex, bytes32 root)`.
#[derive(Clone, Debug, PartialEq)]
pub struct DepositEvent {
    pub token: [u8; 20],
    pub denom_index: u32,
    pub commitment: [u8; 32],
    pub leaf_index: u64,
    pub root: [u8; 32],
    pub block: u64,
    pub tx: String,
}

#[derive(Clone, Debug)]
pub struct BlockInfo {
    pub number: u64,
    pub hash: String,
    pub timestamp: u64,
}

pub trait EthSource {
    /// What this source is, for the UI ("JSON-RPC https://…", "Logos Ethereum module").
    fn describe(&self) -> String;
    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String>;
    /// Several calls at once (one JSON-RPC batch where the endpoint allows it).
    fn calls(&self, to: &[u8; 20], data: &[Vec<u8>]) -> Result<Vec<Result<Vec<u8>, String>>, String> {
        Ok(data.iter().map(|d| self.call(to, d)).collect())
    }
    fn block(&self, tag: &str) -> Result<BlockInfo, String>;
    fn block_by_hash(&self, hash: &str) -> Result<Option<BlockInfo>, String>;
    fn logs(&self, address: &[u8; 20], topic0: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String>;
    fn code_len(&self, address: &[u8; 20]) -> Result<usize, String>;
    fn chain_id(&self) -> Result<u64, String>;
    /// What an address shows of its past: the transactions sent FROM it (`eth_getTransactionCount`
    /// at "latest") and what it holds now (`eth_getBalance`, wei). It can't tell an account that
    /// only ever received and was emptied by someone else from an unused one.
    fn history(&self, _address: &[u8; 20]) -> Result<(u64, u128), String> {
        Err("this Ethereum source can't read an account's history".to_owned())
    }
}

pub struct JsonRpc {
    pub url: String,
    pub timeout: Duration,
    /// Tried in order when `url` is rate-limited (HTTP 429), down, or can't serve the request
    /// (pruned logs, a block-range limit).
    pub fallbacks: Vec<String>,
}

/// Whether an endpoint's refusal is about the endpoint (try another), not about the request.
fn endpoint_limit(said: &str) -> bool {
    let s = said.to_ascii_lowercase();
    ["429", "too many", "rate", "pruned", "limit", "maximum allowed", "exceeded", "range", "timed out", "timeout", "connection", "dns", "502", "503", "504"].iter().any(|k| s.contains(k))
}

impl JsonRpc {
    pub fn new(url: &str) -> Self {
        JsonRpc { url: url.to_owned(), timeout: Duration::from_secs(12), fallbacks: Vec::new() }
    }

    pub fn with_fallbacks(mut self, list: Vec<String>) -> Self {
        self.fallbacks = list.into_iter().filter(|u| *u != self.url).collect();
        self
    }

    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new().timeout(self.timeout).user_agent("logos-lez-bridge-module/0.1").build()
    }

    fn request_at(&self, url: &str, method: &str, params: &Value) -> Result<Value, String> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let reply: Value = self
            .agent()
            .post(url)
            .send_json(body)
            .map_err(|e| rpc_problem(url, &e.to_string()))?
            .into_json()
            .map_err(|e| format!("{url} answered something that isn't JSON: {e}"))?;
        if let Some(err) = reply.get("error").filter(|e| !e.is_null()) {
            return Err(format!("{url} refused {method}: {}", err.get("message").and_then(Value::as_str).unwrap_or("error")));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    /// One request: the endpoint first, a short backoff when it rate-limits, then each fallback.
    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut last = String::new();
        for url in std::iter::once(&self.url).chain(self.fallbacks.iter()) {
            for attempt in 0..2u64 {
                match self.request_at(url, method, &params) {
                    Ok(v) => return Ok(v),
                    Err(e) if e.contains("429") || e.to_ascii_lowercase().contains("too many") => {
                        last = e;
                        if attempt == 0 {
                            std::thread::sleep(Duration::from_millis(1500));
                        }
                    }
                    Err(e) if endpoint_limit(&e) => {
                        last = e;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Err(if last.contains("429") || last.to_ascii_lowercase().contains("too many") {
            format!("Sepolia's RPC is busy (rate limited, HTTP 429); retrying shortly. ({last})")
        } else {
            last
        })
    }
}

fn rpc_problem(url: &str, why: &str) -> String {
    if why.contains("timed out") || why.contains("Timeout") {
        format!("the Ethereum RPC {url} did not answer in time; try again, or choose another in Settings")
    } else if why.contains("Dns") || why.contains("resolve") || why.contains("Connection") || why.contains("connect") {
        format!("could not reach the Ethereum RPC {url} ({why}); check the address in Settings and your connection")
    } else {
        format!("the Ethereum RPC {url} failed: {why}")
    }
}

fn quantity(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.trim_start_matches("0x"), 16).ok()
}

fn block_info(b: &Value) -> Option<BlockInfo> {
    Some(BlockInfo { number: quantity(&b["number"])?, hash: b["hash"].as_str()?.to_owned(), timestamp: quantity(&b["timestamp"])? })
}

impl EthSource for JsonRpc {
    fn describe(&self) -> String {
        format!("JSON-RPC {}", self.url)
    }

    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        let r = self.request("eth_call", json!([{ "to": hex0x(to), "data": hex0x(data) }, "latest"]))?;
        hex_bytes(r.as_str().unwrap_or_default())
    }

    fn calls(&self, to: &[u8; 20], data: &[Vec<u8>]) -> Result<Vec<Result<Vec<u8>, String>>, String> {
        let batch: Vec<Value> = data
            .iter()
            .enumerate()
            .map(|(i, d)| json!({ "jsonrpc": "2.0", "id": i, "method": "eth_call", "params": [{ "to": hex0x(to), "data": hex0x(d) }, "latest"] }))
            .collect();
        let reply = self.agent().post(&self.url).send_json(Value::Array(batch)).map_err(|e| rpc_problem(&self.url, &e.to_string()));
        let parsed: Option<Vec<Value>> = reply.ok().and_then(|r| r.into_json::<Value>().ok()).and_then(|v| v.as_array().cloned());
        match parsed {
            Some(items) if items.len() == data.len() => {
                let mut out: Vec<Result<Vec<u8>, String>> = vec![Err("missing".into()); data.len()];
                for item in items {
                    let Some(i) = item["id"].as_u64().map(|i| i as usize).filter(|i| *i < data.len()) else { continue };
                    out[i] = match item.get("error").filter(|e| !e.is_null()) {
                        Some(e) => Err(e["message"].as_str().unwrap_or("reverted").to_owned()),
                        None => hex_bytes(item["result"].as_str().unwrap_or_default()),
                    };
                }
                Ok(out)
            }
            // Some endpoints refuse batches: one call at a time.
            _ => Ok(data.iter().map(|d| self.call(to, d)).collect()),
        }
    }

    fn block(&self, tag: &str) -> Result<BlockInfo, String> {
        let b = self.request("eth_getBlockByNumber", json!([tag, false]))?;
        block_info(&b).ok_or_else(|| format!("{} has no {tag} block", self.url))
    }

    fn block_by_hash(&self, hash: &str) -> Result<Option<BlockInfo>, String> {
        let b = self.request("eth_getBlockByHash", json!([hash, false]))?;
        Ok(if b.is_null() { None } else { block_info(&b) })
    }

    fn logs(&self, address: &[u8; 20], topic0: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
        let r = self.request(
            "eth_getLogs",
            json!([{ "address": hex0x(address), "topics": [hex0x(topic0)], "fromBlock": format!("0x{from:x}"), "toBlock": format!("0x{to:x}") }]),
        )?;
        r.as_array().cloned().ok_or_else(|| format!("{} answered eth_getLogs without a list", self.url))
    }

    fn code_len(&self, address: &[u8; 20]) -> Result<usize, String> {
        let r = self.request("eth_getCode", json!([hex0x(address), "latest"]))?;
        Ok(hex_bytes(r.as_str().unwrap_or("0x"))?.len())
    }

    fn chain_id(&self) -> Result<u64, String> {
        quantity(&self.request("eth_chainId", json!([]))?).ok_or_else(|| "no chain id".to_owned())
    }

    fn history(&self, address: &[u8; 20]) -> Result<(u64, u128), String> {
        let sent = quantity(&self.request("eth_getTransactionCount", json!([hex0x(address), "latest"]))?).ok_or("no transaction count")?;
        let b = self.request("eth_getBalance", json!([hex0x(address), "latest"]))?;
        let balance = u128::from_str_radix(b.as_str().unwrap_or("").trim_start_matches("0x"), 16).map_err(|_| "no balance".to_owned())?;
        Ok((sent, balance))
    }
}

/// The Logos Ethereum core module (`eth_rpc_module` with `verified_proxy`), which the spec asks
/// for. Its API has not been evaluated yet (separate task), so this source answers plainly.
pub struct LogosEthModule;

pub const LOGOS_ETH_UNAVAILABLE: &str = "the Logos Ethereum module is not wired into this build yet (its API is being evaluated separately); choose \"JSON-RPC\" as the Ethereum source in Settings";

impl EthSource for LogosEthModule {
    fn describe(&self) -> String {
        "Logos Ethereum module (not wired in this build)".to_owned()
    }
    fn call(&self, _: &[u8; 20], _: &[u8]) -> Result<Vec<u8>, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
    fn block(&self, _: &str) -> Result<BlockInfo, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
    fn block_by_hash(&self, _: &str) -> Result<Option<BlockInfo>, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
    fn logs(&self, _: &[u8; 20], _: &[u8; 32], _: u64, _: u64) -> Result<Vec<Value>, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
    fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
    fn chain_id(&self) -> Result<u64, String> {
        Err(LOGOS_ETH_UNAVAILABLE.to_owned())
    }
}

// ── ABI ─────────────────────────────────────────────────────────────────

pub fn word_address(a: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

pub fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// Call data: selector of `signature`, then the static words.
pub fn calldata(signature: &str, words: &[[u8; 32]]) -> Vec<u8> {
    let mut out = selector(signature).to_vec();
    for w in words {
        out.extend_from_slice(w);
    }
    out
}

pub struct Words<'a>(pub &'a [u8]);

impl Words<'_> {
    pub fn word(&self, i: usize) -> Result<[u8; 32], String> {
        self.0.get(i * 32..i * 32 + 32).map(|s| s.try_into().unwrap()).ok_or_else(|| "short ABI answer".to_owned())
    }
    pub fn boolean(&self, i: usize) -> Result<bool, String> {
        Ok(self.word(i)?[31] != 0)
    }
    pub fn u64(&self, i: usize) -> Result<u64, String> {
        let w = self.word(i)?;
        if w[..24].iter().any(|b| *b != 0) {
            return Err("ABI value above u64".to_owned());
        }
        Ok(u64::from_be_bytes(w[24..].try_into().unwrap()))
    }
    /// A uint256 as decimal text (exact, whatever its size).
    pub fn dec(&self, i: usize) -> Result<String, String> {
        Ok(u256_dec(&self.word(i)?))
    }
    pub fn address(&self, i: usize) -> Result<[u8; 20], String> {
        Ok(self.word(i)?[12..].try_into().unwrap())
    }
    /// A dynamic `uint256[]` whose offset is in head word `i`.
    pub fn dec_array(&self, i: usize) -> Result<Vec<String>, String> {
        let offset = self.u64(i)? as usize;
        if offset % 32 != 0 {
            return Err("misaligned ABI array".to_owned());
        }
        let at = offset / 32;
        let len = self.u64(at)? as usize;
        if len > 64 {
            return Err("ABI array too long".to_owned());
        }
        (0..len).map(|k| self.dec(at + 1 + k)).collect()
    }
}

/// A 256-bit big-endian number as decimal text.
pub fn u256_dec(word: &[u8; 32]) -> String {
    let mut n = *word;
    if n.iter().all(|b| *b == 0) {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while n.iter().any(|b| *b != 0) {
        let mut rem = 0u32;
        for b in n.iter_mut() {
            let cur = (rem << 8) | *b as u32;
            *b = (cur / 10) as u8;
            rem = cur % 10;
        }
        digits.push(b'0' + rem as u8);
    }
    digits.reverse();
    String::from_utf8(digits).unwrap()
}

/// The `Deposit` event's topic.
pub fn deposit_topic() -> [u8; 32] {
    crate::util::keccak(b"Deposit(address,uint32,bytes32,uint64,bytes32)")
}

/// A `Deposit` log as the RPC returns it.
pub fn parse_deposit_log(log: &Value) -> Result<DepositEvent, String> {
    let topics = log["topics"].as_array().ok_or("a log without topics")?;
    let token_word = crate::util::hex32(topics.get(1).and_then(Value::as_str).ok_or("a Deposit log without its token topic")?)?;
    let data = hex_bytes(log["data"].as_str().unwrap_or_default())?;
    let w = Words(&data);
    Ok(DepositEvent {
        token: token_word[12..].try_into().unwrap(),
        denom_index: w.u64(0)? as u32,
        commitment: w.word(1)?,
        leaf_index: w.u64(2)?,
        root: w.word(3)?,
        block: quantity(&log["blockNumber"]).ok_or("a log without a block number")?,
        tx: log["transactionHash"].as_str().unwrap_or_default().to_owned(),
    })
}

/// Every `Deposit` of `vault` since `from`, in leaf order, read in spans the endpoint accepts.
/// Fewer events than `expected` (the vault's `depositCount()`) is an error, never "no deposits".
pub fn deposit_events(eth: &dyn EthSource, vault: &[u8; 20], from: u64, to: u64, expected: u64) -> Result<Vec<DepositEvent>, String> {
    // A span the endpoint takes: halved on a block-range refusal (some public RPCs allow 1,000
    // blocks per call), down to 500.
    let mut span: u64 = 45_000;
    let topic = deposit_topic();
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = (start + span - 1).min(to);
        match eth.logs(vault, &topic, start, end) {
            Ok(logs) => {
                for log in logs {
                    out.push(parse_deposit_log(&log)?);
                }
                start = end + 1;
            }
            Err(e) if span > 500 && endpoint_limit(&e) && !e.contains("429") => span = (span / 2).max(500),
            Err(e) => return Err(e),
        }
    }
    out.sort_by_key(|e| e.leaf_index);
    out.dedup_by_key(|e| e.leaf_index);
    if (out.len() as u64) < expected || out.iter().enumerate().any(|(i, e)| e.leaf_index != i as u64) {
        return Err(format!(
            "{} returned {} of the vault's {expected} deposit events (logs from block {from}); its log history may be cut short: choose another Ethereum RPC in Settings",
            eth.describe(),
            out.len()
        ));
    }
    Ok(out)
}

pub fn addr_hex(a: &[u8; 20]) -> String {
    format!("0x{}", to_hex(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that refuses log spans over 1,000 blocks, as thirdweb's public Sepolia RPC does.
    struct Narrow(Vec<Value>);
    impl EthSource for Narrow {
        fn describe(&self) -> String {
            "narrow".into()
        }
        fn call(&self, _: &[u8; 20], _: &[u8]) -> Result<Vec<u8>, String> {
            Err("no".into())
        }
        fn block(&self, _: &str) -> Result<BlockInfo, String> {
            Err("no".into())
        }
        fn block_by_hash(&self, _: &str) -> Result<Option<BlockInfo>, String> {
            Ok(None)
        }
        fn logs(&self, _: &[u8; 20], _: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
            if to - from + 1 > 1000 {
                return Err("x refused eth_getLogs: Maximum allowed number of requested blocks is 1000".into());
            }
            Ok(self.0.iter().filter(|l| quantity(&l["blockNumber"]).is_some_and(|b| b >= from && b <= to)).cloned().collect())
        }
        fn code_len(&self, _: &[u8; 20]) -> Result<usize, String> {
            Ok(0)
        }
        fn chain_id(&self) -> Result<u64, String> {
            Ok(1)
        }
    }

    #[test]
    fn a_narrow_endpoint_gets_smaller_log_spans_and_a_busy_one_is_said_calmly() {
        let logs: Value = serde_json::from_str(include_str!("../testdata/eth/logs.json")).unwrap();
        let list = logs["result"].as_array().unwrap().clone();
        let first = quantity(&list[0]["blockNumber"]).unwrap();
        let last = quantity(&list[list.len() - 1]["blockNumber"]).unwrap();
        let got = deposit_events(&Narrow(list.clone()), &[0; 20], first - 20_000, last + 5, list.len() as u64).unwrap();
        assert_eq!(got.len(), list.len());
        assert!(endpoint_limit("https://x refused eth_getLogs: pruned history unavailable"));
        assert!(endpoint_limit("the Ethereum RPC https://x failed: https://x: status code 429"));
        assert!(!endpoint_limit("https://x refused eth_call: execution reverted"));
    }

    #[test]
    fn u256_decimal_is_exact() {
        let mut w = [0u8; 32];
        assert_eq!(u256_dec(&w), "0");
        w[31] = 0xff;
        assert_eq!(u256_dec(&w), "255");
        let max = [0xffu8; 32];
        assert_eq!(u256_dec(&max), "115792089237316195423570985008687907853269984665640564039457584007913129639935");
    }

    #[test]
    fn the_deposit_topic_is_the_vaults() {
        // topics[0] of the v2 vault's Deposit logs on Sepolia (recorded in testdata/eth/logs.json).
        let logs: Value = serde_json::from_str(include_str!("../testdata/eth/logs.json")).unwrap();
        let first = &logs["result"][0];
        assert_eq!(first["topics"][0].as_str().unwrap(), hex0x(&deposit_topic()));
        let e = parse_deposit_log(first).unwrap();
        assert_eq!(e.leaf_index, 0);
        assert_eq!(e.block, 12_122_993);
        assert_eq!(hex0x(&e.commitment), "0xa50e0f44940eca30735460fa9b657d6ab948fc422b29b1293be0f7e5d6b7ad2e");
    }
}
