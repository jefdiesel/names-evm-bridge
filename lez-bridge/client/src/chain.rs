//! What the recovery scan reads from the two chains, behind two traits so tests can feed it
//! in-memory fakes: [`EthLogSource`] (Ethereum JSON-RPC: `eth_getLogs` and block numbers) and
//! [`LezBlockSource`] (the LEZ sequencer: blocks, and a point query the private mode avoids).
//!
//! Fetching is resumable and never duplicates work (RFP-022 Reliability #6): logs are fetched
//! in bounded block ranges; a range the provider refuses as too large is halved and retried; a
//! transient failure is retried with a pause; after the last retry the fetch stops and reports
//! the first block it did not finish, and every finished range has already been handed to the
//! caller's `progress` callback exactly once, which persists it. A rerun starts from there.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bridge_core::EthAddress;
use serde::{Deserialize, Serialize};

use crate::{hexser, kdf::keccak256};

/// One Ethereum log, as `eth_getLogs` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLog {
    pub block: u64,
    #[serde(with = "hexser::arr")]
    pub tx_hash: [u8; 32],
    pub log_index: u64,
    #[serde(with = "hexser::words")]
    pub topics: Vec<[u8; 32]>,
    #[serde(with = "hexser::bytes")]
    pub data: Vec<u8>,
}

/// One included, non-reverted LEZ transaction that mentions the bridge config, as borsh bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LezTx {
    pub block: u64,
    #[serde(with = "hexser::bytes")]
    pub bytes: Vec<u8>,
}

/// How a chain request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    /// The provider refuses this block range or result size: split the range.
    RangeTooLarge(String),
    /// Anything that may succeed on retry (timeouts, connection errors, rate limits, 5xx).
    Transient(String),
    /// Retrying will not help.
    Fatal(String),
}

impl core::fmt::Display for RpcError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RangeTooLarge(m) => write!(f, "range refused: {m}"),
            Self::Transient(m) => write!(f, "transient: {m}"),
            Self::Fatal(m) => write!(f, "{m}"),
        }
    }
}

/// What a provider says when it no longer serves old blocks' logs or receipts (PublicNode's
/// Sepolia RPC, 2026-11-14: `{"code":4444,"message":"pruned history unavailable"}`, on some
/// requests and not others).
const PRUNED: [&str; 3] = ["pruned", "history unavailable", "history is not available"];

/// Rate limits, checked before the range words: thirdweb says "Rate limit exceeded" with the same
/// code -32005 as its range refusal, OnFinality "Too Many Requests"; halving the range would not help.
const RATE_LIMITED: [&str; 3] = ["rate limit", "too many requests", "rate-limit"];

impl RpcError {
    /// Sorts a JSON-RPC error message: providers word the range and result limits differently
    /// (geth "query returned more than 10000 results", Alchemy "block range", Infura "-32005",
    /// "limit exceeded", "range too large"...). Rate limits are transient. Pruned history is
    /// fatal: retrying the same provider does not bring old logs back; the caller moves to
    /// another RPC or stops, saying so.
    #[must_use]
    pub fn classify(message: &str) -> Self {
        let m = message.to_ascii_lowercase();
        if PRUNED.iter().any(|w| m.contains(w)) {
            return Self::Fatal(format!(
                "{message} (this RPC has pruned old history: use one that serves eth_getLogs back to the vault's creation block)"
            ));
        }
        if RATE_LIMITED.iter().any(|w| m.contains(w)) {
            return Self::Transient(message.to_owned());
        }
        let range = ["range", "too many", "more than", "limit exceeded", "-32005", "10000 results", "response size", "too large"];
        if range.iter().any(|w| m.contains(w)) {
            Self::RangeTooLarge(message.to_owned())
        } else {
            Self::Transient(message.to_owned())
        }
    }
}

/// Ethereum logs.
#[allow(async_fn_in_trait)]
pub trait EthLogSource {
    /// The latest block number.
    async fn latest(&self) -> Result<u64, RpcError>;
    /// The latest finalized block number (logs at or below it never change).
    async fn finalized(&self) -> Result<u64, RpcError>;
    /// Logs of `address` whose first topic is any of `topic0`, in `from..=to`.
    async fn logs(&self, address: &EthAddress, topic0: &[[u8; 32]], from: u64, to: u64) -> Result<Vec<RawLog>, RpcError>;
    /// Wait before retry `attempt` (1-based). Tests return at once.
    async fn pause(&self, _attempt: u32) {}
}

/// LEZ blocks and accounts.
#[allow(async_fn_in_trait)]
pub trait LezBlockSource {
    async fn tip(&self) -> Result<u64, RpcError>;
    /// Every transaction of block `id` as (borsh bytes, reverted); `None` if there is no such block.
    async fn block_txs(&self, id: u64) -> Result<Option<Vec<(Vec<u8>, bool)>>, RpcError>;
    /// Whether `account` holds state under `program`. A point query: the sequencer learns which
    /// account the client asked about (see the crate docs, "What the scan reveals").
    async fn account_exists(&self, account: &lee_core::account::AccountId, program: &lee_core::account::AccountId) -> Result<bool, RpcError>;
    async fn pause(&self, _attempt: u32) {}
}

/// Range splitting and retry policy.
#[derive(Clone, Copy, Debug)]
pub struct FetchPolicy {
    /// First range size, in blocks.
    pub max_range: u64,
    /// Retries of one request after a transient failure.
    pub retries: u32,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self { max_range: 2_000, retries: 5 }
    }
}

/// A fetch stopped before `next_block`; everything below it was handed to `progress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupted {
    pub next_block: u64,
    pub cause: RpcError,
}

impl core::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "stopped at block {} ({}); run again to resume from there", self.next_block, self.cause)
    }
}

/// Fetches the logs of `from..=to` in bounded ranges. After each finished range,
/// `progress(next_block, logs_of_that_range)` (the caller appends and persists). Returns the
/// number of requests made.
///
/// # Errors
/// [`Interrupted`] after `policy.retries` transient failures in a row, a range refused at size
/// 1, or a fatal error.
pub async fn fetch_logs<S: EthLogSource>(
    src: &S,
    address: &EthAddress,
    topic0: &[[u8; 32]],
    from: u64,
    to: u64,
    policy: FetchPolicy,
    mut progress: impl FnMut(u64, Vec<RawLog>),
) -> Result<u64, Interrupted> {
    let mut start = from;
    let mut size = policy.max_range.max(1);
    let mut requests = 0u64;
    let mut attempt = 0u32;
    while start <= to {
        let end = start.saturating_add(size - 1).min(to);
        requests += 1;
        match src.logs(address, topic0, start, end).await {
            Ok(mut logs) => {
                logs.retain(|l| (start..=end).contains(&l.block));
                logs.sort_by_key(|l| (l.block, l.log_index));
                attempt = 0;
                progress(end + 1, logs);
                if end == u64::MAX {
                    break;
                }
                start = end + 1;
            }
            Err(RpcError::RangeTooLarge(m)) => {
                if size == 1 {
                    return Err(Interrupted { next_block: start, cause: RpcError::RangeTooLarge(m) });
                }
                size = (size / 2).max(1);
            }
            Err(RpcError::Transient(m)) => {
                attempt += 1;
                if attempt > policy.retries {
                    return Err(Interrupted { next_block: start, cause: RpcError::Transient(m) });
                }
                src.pause(attempt).await;
            }
            Err(fatal @ RpcError::Fatal(_)) => return Err(Interrupted { next_block: start, cause: fatal }),
        }
    }
    Ok(requests)
}

/// Reads blocks `from..=to` and keeps every non-reverted transaction whose bytes contain
/// `config` (the bridge config account: every claim and burn names it). After each block,
/// `progress(next_block, kept)`.
///
/// # Errors
/// [`Interrupted`] as for [`fetch_logs`]; a block the sequencer does not have is fatal.
pub async fn scan_lez<S: LezBlockSource>(
    src: &S,
    config: &[u8; 32],
    from: u64,
    to: u64,
    retries: u32,
    mut progress: impl FnMut(u64, Vec<LezTx>),
) -> Result<(), Interrupted> {
    let mut id = from;
    let mut attempt = 0u32;
    while id <= to {
        match src.block_txs(id).await {
            Ok(Some(txs)) => {
                attempt = 0;
                let kept = txs
                    .into_iter()
                    .filter(|(bytes, reverted)| !reverted && contains(bytes, config))
                    .map(|(bytes, _)| LezTx { block: id, bytes })
                    .collect();
                progress(id + 1, kept);
                id += 1;
            }
            Ok(None) => return Err(Interrupted { next_block: id, cause: RpcError::Fatal(format!("the sequencer has no block {id}")) }),
            Err(RpcError::Fatal(m)) => return Err(Interrupted { next_block: id, cause: RpcError::Fatal(m) }),
            Err(e) => {
                attempt += 1;
                if attempt > retries {
                    return Err(Interrupted { next_block: id, cause: e });
                }
                src.pause(attempt).await;
            }
        }
    }
    Ok(())
}

/// Whether `haystack` contains the 32 bytes `needle`.
#[must_use]
pub fn contains(haystack: &[u8], needle: &[u8; 32]) -> bool {
    haystack.windows(32).any(|w| w == needle)
}

/// For each wanted 32-byte value found in a transaction: the blocks it appears in (one entry per
/// transaction, in chain order).
#[must_use]
pub fn find_in_txs(txs: &[LezTx], wanted: &HashSet<[u8; 32]>) -> HashMap<[u8; 32], Vec<u64>> {
    let mut found: HashMap<[u8; 32], Vec<u64>> = HashMap::new();
    if wanted.is_empty() {
        return found;
    }
    for tx in txs {
        let mut seen = BTreeSet::new();
        for w in tx.bytes.windows(32) {
            let w: [u8; 32] = w.try_into().unwrap_or_else(|_| unreachable!());
            if wanted.contains(&w) && seen.insert(w) {
                found.entry(w).or_default().push(tx.block);
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------------------------
// Vault events

pub const DEPOSIT_SIG: &str = "Deposit(address,uint32,bytes32,uint64,bytes32)";
pub const STATE_UPDATED_SIG: &str = "StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])";
pub const RELEASED_SIG: &str = "Released(bytes32,address,uint32,address,address,uint256)";
pub const RELAY_FEE_MENU_SET_SIG: &str = "RelayFeeMenuSet(address,uint256[][])";

/// The four event topics the scan reads, in one `eth_getLogs` filter.
#[must_use]
pub fn vault_topics() -> [[u8; 32]; 4] {
    [DEPOSIT_SIG, STATE_UPDATED_SIG, RELEASED_SIG, RELAY_FEE_MENU_SET_SIG].map(|s| keccak256(s.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepositEvent {
    #[serde(with = "hexser::arr")]
    pub token: EthAddress,
    pub denom_index: u32,
    #[serde(with = "hexser::arr")]
    pub commitment: [u8; 32],
    pub leaf_index: u64,
    #[serde(with = "hexser::arr")]
    pub root: [u8; 32],
    pub block: u64,
    #[serde(with = "hexser::arr")]
    pub tx_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitLeafEvent {
    #[serde(with = "hexser::arr")]
    pub leaf: [u8; 32],
    pub exit_index: u64,
    #[serde(with = "hexser::arr")]
    pub exit_root: [u8; 32],
    pub block: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasedEvent {
    #[serde(with = "hexser::arr")]
    pub nullifier: [u8; 32],
    #[serde(with = "hexser::arr")]
    pub token: EthAddress,
    pub denom_index: u32,
    #[serde(with = "hexser::arr")]
    pub recipient: EthAddress,
    pub block: u64,
    #[serde(with = "hexser::arr")]
    pub tx_hash: [u8; 32],
}

/// Everything public the scan collected from the vault.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VaultView {
    /// In leaf order.
    pub deposits: Vec<DepositEvent>,
    pub exit_leaves: Vec<ExitLeafEvent>,
    pub released: BTreeMap<[u8; 32], ReleasedEvent>,
    /// Every non-zero relay fee ever on a menu, per (token, denomination index).
    pub menu_fees: BTreeMap<(EthAddress, u32), BTreeSet<u128>>,
    /// Logs the decoder did not understand (counted, not fatal).
    pub undecoded: usize,
}

fn word(data: &[u8], i: usize) -> Option<[u8; 32]> {
    data.get(32 * i..32 * i + 32)?.try_into().ok()
}

fn word_u64(w: &[u8; 32]) -> Option<u64> {
    w[..24].iter().all(|b| *b == 0).then(|| u64::from_be_bytes(w[24..].try_into().unwrap_or_default()))
}

fn word_u128(w: &[u8; 32]) -> Option<u128> {
    w[..16].iter().all(|b| *b == 0).then(|| u128::from_be_bytes(w[16..].try_into().unwrap_or_default()))
}

fn word_addr(w: &[u8; 32]) -> Option<EthAddress> {
    w[..12].iter().all(|b| *b == 0).then(|| w[12..].try_into().unwrap_or_default())
}

fn usize_at(data: &[u8], byte_offset: usize) -> Option<usize> {
    let w: [u8; 32] = data.get(byte_offset..byte_offset + 32)?.try_into().ok()?;
    usize::try_from(word_u64(&w)?).ok()
}

/// Decodes the vault logs (any order) into a [`VaultView`].
#[must_use]
pub fn decode_vault_logs(logs: &[RawLog]) -> VaultView {
    let [dep, upd, rel, menu] = vault_topics();
    let mut v = VaultView::default();
    let mut logs: Vec<&RawLog> = logs.iter().collect();
    logs.sort_by_key(|l| (l.block, l.log_index));
    for l in logs {
        let ok = match l.topics.first() {
            Some(t) if *t == dep => decode_deposit(l).map(|d| v.deposits.push(d)),
            Some(t) if *t == upd => decode_update(l).map(|e| v.exit_leaves.extend(e)),
            Some(t) if *t == rel => decode_released(l).map(|r| {
                v.released.insert(r.nullifier, r);
            }),
            Some(t) if *t == menu => decode_menu(l).map(|(token, menus)| {
                for (i, fees) in menus.into_iter().enumerate() {
                    if let Ok(i) = u32::try_from(i) {
                        v.menu_fees.entry((token, i)).or_default().extend(fees);
                    }
                }
            }),
            _ => None,
        };
        if ok.is_none() {
            v.undecoded += 1;
        }
    }
    v.deposits.sort_by_key(|d| d.leaf_index);
    v.exit_leaves.sort_by_key(|e| e.exit_index);
    v
}

/// `depositCount()` and `depositRoot()`: the vault's own count and root, read at the block the
/// logs were read to, to check the logs are complete.
pub const SEL_DEPOSIT_COUNT: [u8; 4] = [0x2d, 0xfd, 0xf0, 0xb5];
pub const SEL_DEPOSIT_ROOT: [u8; 4] = [0x89, 0x55, 0xfe, 0x8a];

/// Whether the `Deposit` logs read up to `block` are all of them: leaf indexes `0..n` in order,
/// `n` equal to the vault's `depositCount()` at `block`, and the last logged root equal to its
/// `depositRoot()` there. An RPC that silently leaves logs out (pruned history, a partial answer)
/// fails here instead of yielding a short tree. `deposits` in leaf order, as [`VaultView`] holds
/// them; deposits above `block` are ignored.
///
/// # Errors
/// What is missing or different, in words.
pub fn check_deposits_complete(deposits: &[DepositEvent], block: u64, count: u64, root: &[u8; 32]) -> Result<(), String> {
    let read: Vec<&DepositEvent> = deposits.iter().filter(|d| d.block <= block).collect();
    for (i, d) in read.iter().enumerate() {
        if d.leaf_index != i as u64 {
            return Err(format!(
                "the logs read skip from leaf {i} to leaf {}: the RPC left deposit logs out (pruned history?)",
                d.leaf_index
            ));
        }
    }
    let n = read.len() as u64;
    if n != count {
        return Err(format!(
            "the logs read hold {n} deposit(s) but the vault's depositCount() at block {block} is {count}: {}",
            if n < count { "the RPC left deposit logs out (pruned history or a partial answer)" } else { "the RPC returned logs the vault does not count" }
        ));
    }
    if let Some(last) = read.last()
        && last.root != *root
    {
        return Err(format!(
            "the last deposit log's root 0x{} is not the vault's depositRoot() 0x{} at block {block}",
            hexser::hex(&last.root),
            hexser::hex(root)
        ));
    }
    Ok(())
}

fn decode_deposit(l: &RawLog) -> Option<DepositEvent> {
    let token = word_addr(l.topics.get(1)?)?;
    Some(DepositEvent {
        token,
        denom_index: u32::try_from(word_u64(&word(&l.data, 0)?)?).ok()?,
        commitment: word(&l.data, 1)?,
        leaf_index: word_u64(&word(&l.data, 2)?)?,
        root: word(&l.data, 3)?,
        block: l.block,
        tx_hash: l.tx_hash,
    })
}

fn decode_update(l: &RawLog) -> Option<Vec<ExitLeafEvent>> {
    let root = word(&l.data, 2)?;
    let first = word_u64(&word(&l.data, 3)?)?;
    let offset = usize_at(&l.data, 4 * 32)?;
    let n = usize_at(&l.data, offset)?;
    (0..n)
        .map(|i| {
            let leaf: [u8; 32] = l.data.get(offset + 32 + 32 * i..offset + 64 + 32 * i)?.try_into().ok()?;
            Some(ExitLeafEvent { leaf, exit_index: first + i as u64, exit_root: root, block: l.block })
        })
        .collect()
}

fn decode_released(l: &RawLog) -> Option<ReleasedEvent> {
    Some(ReleasedEvent {
        nullifier: *l.topics.get(1)?,
        token: word_addr(l.topics.get(2)?)?,
        denom_index: u32::try_from(word_u64(&word(&l.data, 0)?)?).ok()?,
        recipient: word_addr(&word(&l.data, 1)?)?,
        block: l.block,
        tx_hash: l.tx_hash,
    })
}

fn decode_menu(l: &RawLog) -> Option<(EthAddress, Vec<Vec<u128>>)> {
    let token = word_addr(l.topics.get(1)?)?;
    let outer = usize_at(&l.data, 0)?;
    let n = usize_at(&l.data, outer)?;
    let base = outer + 32;
    let menus = (0..n)
        .map(|i| {
            let inner = base + usize_at(&l.data, base + 32 * i)?;
            let m = usize_at(&l.data, inner)?;
            (0..m)
                .map(|j| word_u128(&l.data.get(inner + 32 + 32 * j..inner + 64 + 32 * j)?.try_into().ok()?))
                .collect::<Option<Vec<u128>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    Some((token, menus))
}

// ---------------------------------------------------------------------------------------------
// ABI encoding for tests and the e2e helper (the same layouts the vault emits).

/// A `Deposit` log as the vault emits it.
#[must_use]
pub fn encode_deposit_log(e: &DepositEvent, log_index: u64) -> RawLog {
    let mut token = [0u8; 32];
    token[12..].copy_from_slice(&e.token);
    RawLog {
        block: e.block,
        tx_hash: e.tx_hash,
        log_index,
        topics: vec![vault_topics()[0], token],
        data: [u64_word(u64::from(e.denom_index)), e.commitment, u64_word(e.leaf_index), e.root].concat(),
    }
}

/// A `StateUpdated` log appending `leaves` from `first`.
#[must_use]
pub fn encode_update_log(block: u64, log_index: u64, root: [u8; 32], first: u64, leaves: &[[u8; 32]]) -> RawLog {
    let mut data = [[0u8; 32], [0u8; 32], root, u64_word(first), u64_word(5 * 32), u64_word(leaves.len() as u64)].concat();
    for l in leaves {
        data.extend_from_slice(l);
    }
    RawLog { block, tx_hash: [0xEE; 32], log_index, topics: vec![vault_topics()[1]], data }
}

/// A `Released` log.
#[must_use]
pub fn encode_released_log(block: u64, log_index: u64, nullifier: [u8; 32], token: EthAddress, denom_index: u32, recipient: EthAddress) -> RawLog {
    let addr = |a: &EthAddress| {
        let mut w = [0u8; 32];
        w[12..].copy_from_slice(a);
        w
    };
    RawLog {
        block,
        tx_hash: [0xDD; 32],
        log_index,
        topics: vec![vault_topics()[2], nullifier, addr(&token)],
        data: [u64_word(u64::from(denom_index)), addr(&recipient), [0u8; 32], [0u8; 32]].concat(),
    }
}

/// A `RelayFeeMenuSet` log.
#[must_use]
pub fn encode_menu_log(block: u64, log_index: u64, token: EthAddress, menus: &[Vec<u128>]) -> RawLog {
    let mut t = [0u8; 32];
    t[12..].copy_from_slice(&token);
    let mut heads = Vec::new();
    let mut tails = Vec::new();
    let mut offset = 32 * menus.len();
    for m in menus {
        heads.push(u64_word(offset as u64));
        let mut inner = vec![u64_word(m.len() as u64)];
        inner.extend(m.iter().map(|f| bridge_core::u256(*f)));
        offset += 32 * inner.len();
        tails.extend(inner);
    }
    let data = [vec![u64_word(32), u64_word(menus.len() as u64)], heads, tails].concat().concat();
    RawLog { block, tx_hash: [0xCC; 32], log_index, topics: vec![vault_topics()[3], t], data }
}

fn u64_word(v: u64) -> [u8; 32] {
    bridge_core::u256(u128::from(v))
}
