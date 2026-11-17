//! Seed-derived positions and recovery (RFP-022 Functionality #9, Reliability #5): the CLI side
//! of `bridge_client` (spec in `client/src/lib.rs`).
//!
//! - `deposit-request`: derives the next deposit secret of an account and prints the commitment
//!   and the v1 deposit request (`logosdep1...`, the format of `web/deposit`) for the Ethereum
//!   side. Nothing secret is printed.
//! - `claim --key-index N` / `claim-attested --key-index N`: the deposit secret is derived here,
//!   never typed; leaves and leaf index come from the vault's logs when not given.
//! - `burn` without `--exit`: derives the exit secret, the release address (default) and `E`,
//!   refuses an `E` already burned, and records the index.
//! - `recover`: the scan, from the wallet's keys plus both chains; no local state needed.
//! - `positions`: the last scan's result, from the cache.
//! - `eth-key`: writes a derived release address's private key to a file (mode 600).
//!
//! The cache (`<wallet home>/bridge-client/<cfgId>.json`, or `$LEZ_BRIDGE_CACHE_DIR`) is only a
//! cache: delete it and `recover` rebuilds it.
//!
//! `--names-app APP` (global) switches every derivation here to the Names Wallet's `app_secret`
//! for module APP (`client/src/kdf.rs`, "Names app scheme"): with this wallet restored from the
//! Names Wallet's recovery phrase, `recover --names-app lez_bridge` finds what the bridge
//! mini-app deposited and burned. Only accounts of the phrase's key trees count (an imported key
//! isn't restored by the phrase, and the Names Wallet refuses it too). Its cache is a separate
//! file, `<cfgId>.names-<APP>.json`.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::Instant,
};

use anyhow::{Context as _, Result, anyhow, bail};
use bridge_client::{
    cache::ClientCache,
    chain::{self, EthLogSource, FetchPolicy, LezBlockSource, RawLog, RpcError, VaultView},
    hexser::hex,
    kdf::{self, AccountRoot},
    request::DepositRequest,
    scan::{self, ClaimState, Dest, ExitState, Recovery, ScanParams, TokenInfo, WalletAccount},
};
use bridge_core::{Config, EthAddress, config_address};
use lee_core::account::AccountId;
use serde_json::{Value, json};
use wallet::WalletCore;

use super::{SequencerClient, eth, out, public_state, to_hex};

/// `--names-app`: the module whose `app_secret` values the derivations reproduce.
static NAMES_APP: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_names_app(app: &str) -> Result<()> {
    if !kdf::names::valid_app(app) {
        bail!("--names-app {app:?} is not a module name (1-64 of A-Z a-z 0-9 _ - .)");
    }
    NAMES_APP.set(app.to_owned()).map_err(|_| anyhow!("--names-app given twice"))
}

fn names_app() -> Option<&'static str> {
    NAMES_APP.get().map(String::as_str)
}

// ---------------------------------------------------------------------------------------------
// Arguments

/// Where the chains are and how far back to read.
#[derive(clap::Args, Clone, Debug)]
pub struct ChainArgs {
    /// Ethereum JSON-RPC of the vault's chain (read only: `eth_getLogs`, block numbers, the
    /// vault's `depositCount()`/`depositRoot()`). It must serve logs back to the vault's creation
    /// block (PublicNode's Sepolia RPC no longer does). Several, comma-separated, are tried in
    /// order; a scan that finds fewer deposits than the vault counts is refused, never used.
    #[arg(long, env = "LEZ_BRIDGE_ETH_RPC")]
    pub eth_rpc: Option<String>,
    /// First Ethereum block to read vault logs from (the vault's deployment block).
    #[arg(long, default_value_t = 0)]
    pub eth_from_block: u64,
    /// First LEZ block to scan (the bridge's deployment block).
    #[arg(long, default_value_t = 1)]
    pub lez_from_block: u64,
    /// Largest `eth_getLogs` block range to ask for first (halved when the provider refuses).
    #[arg(long, default_value_t = 2_000)]
    pub max_range: u64,
}

#[derive(clap::Args, Debug)]
pub struct RecoverArgs {
    #[arg(long)]
    pub bridge: AccountId,
    #[command(flatten)]
    pub chain: ChainArgs,
    /// Stop an index sequence after this many unused indexes in a row.
    #[arg(long, default_value_t = 20)]
    pub gap: u64,
    /// Also scan this many not-yet-created accounts of each key tree, in the wallet's own
    /// creation order: accounts a restore did not regenerate (`restore-keys --depth` too small,
    /// or pruned as unused) whose only footprint is a deposit.
    #[arg(long, default_value_t = 5)]
    pub account_gap: u32,
    /// Ethereum addresses you chose as release destinations (repeat). Derived ones need nothing.
    #[arg(long = "eth-dest")]
    pub eth_dests: Vec<String>,
    /// More maxRelayFee values to try (besides 0, the 2% cap and every menu value ever set).
    #[arg(long = "max-relay-fee")]
    pub max_relay_fees: Vec<u128>,
    /// `scan` (default): read every LEZ block; the sequencer learns nothing about your
    /// positions. `point`: ask for each claim nullifier's account instead (faster, but the
    /// sequencer learns which nullifiers you watch) and skip exits, which need the block scan.
    #[arg(long, default_value = "scan", value_parser = ["scan", "point"])]
    pub lez_mode: String,
}

// ---------------------------------------------------------------------------------------------
// Keys, config, cache

/// The derivation root of one of this wallet's accounts: from a public account's signing key or
/// a private account's authorization secret key. `None` for accounts whose keys the wallet does
/// not hold (keycard, shared/group accounts), and under `--names-app` for accounts outside the
/// recovery phrase's key trees (as the Names Wallet's `account_secret` refuses them).
pub fn account_root(wallet: &WalletCore<SequencerClient>, id: AccountId) -> Option<(AccountRoot, bool)> {
    let kc = wallet.storage().key_chain();
    let root = |secret: &[u8; 32]| match names_app() {
        None => Some(AccountRoot::from_account_secret(secret)),
        Some(app) => AccountRoot::names_app(secret, app),
    };
    let from_phrase = |found: Option<bool>| names_app().is_none() || found == Some(true);
    if let Some(key) = kc.pub_account_signing_key(id) {
        if !from_phrase(kc.public_account_ids().find(|(a, _)| *a == id).map(|(_, index)| index.is_some())) {
            return None;
        }
        return root(key.value()).map(|r| (r, false));
    }
    let found = kc.private_account(id)?;
    if !from_phrase(kc.private_account_ids().find(|(a, _)| *a == id).map(|(_, index)| index.is_some())) {
        return None;
    }
    root(&found.key_chain.private_key_holder.authorization_secret_key.0).map(|r| (r, true))
}

pub fn root_or_bail(wallet: &WalletCore<SequencerClient>, id: AccountId) -> Result<AccountRoot> {
    account_root(wallet, id).map(|(r, _)| r).ok_or_else(|| match names_app() {
        Some(app) => anyhow!(
            "{id}: this wallet can't derive {app}'s values for it (not an account of this wallet's recovery phrase, or its key isn't held here)"
        ),
        None => anyhow!("{id}: this wallet does not hold its key (keycard or shared accounts can't derive bridge secrets)"),
    })
}

pub async fn bridge_config(wallet: &WalletCore<SequencerClient>, bridge: AccountId) -> Result<Config> {
    borsh::from_slice(&public_state(wallet, config_address(&bridge), bridge).await?).context("no bridge config")
}

/// The public demo's Sepolia v2 vault (`vault/DEPLOYMENTS.md`) and its creation block.
const DEMO_VAULT_V2: (u64, &str, u64) = (11_155_111, "8c8b2664de28e5287158db4db65d448638871b74", 12_122_976);

/// Where to start reading vault logs: `--eth-from-block` when given; for the demo vault with the
/// default 0, its creation block (from block 0, a 2,000-block range would take about 6,000
/// `eth_getLogs` requests to reach it).
pub fn eth_from_block(vault: &EthAddress, evm_chain_id: u64, given: u64) -> u64 {
    let (chain_id, address, created) = DEMO_VAULT_V2;
    if given == 0 && evm_chain_id == chain_id && hex(vault) == address { created } else { given }
}

/// The vault's deposit leaves up to Ethereum's finalized block, rebuilt from its `Deposit` logs:
/// what a claim with a raw `--secret` and no `--leaves` uses. Logs above the finalized block are
/// left out (their root cannot have been attested yet). Read-only on both chains.
pub async fn finalized_leaves(wallet: &WalletCore<SequencerClient>, bridge: AccountId, chain: &ChainArgs) -> Result<Vec<[u8; 32]>> {
    if chain.eth_rpc.is_none() {
        bail!(
            "without --leaves the leaves are rebuilt from the vault's logs: give --eth-rpc (or LEZ_BRIDGE_ETH_RPC), \
             an Ethereum JSON-RPC of the vault's chain that serves eth_getLogs back to the vault's creation \
             (Sepolia: {SEPOLIA_HISTORY_RPC}; several, comma-separated, are tried in order)"
        );
    }
    let config = bridge_config(wallet, bridge).await?;
    let (mut cache, path) = load_cache(&config.cfg_id)?;
    let (_, finalized) = sync(wallet, &config, bridge, chain, &mut cache, &path, false, false).await?;
    let view = chain::decode_vault_logs(&cache.eth_logs);
    let leaves = scan::rebuild_leaves(&view.deposits, &registry(&config), &config.cfg_id).map_err(|e| anyhow!("vault logs: {e}"))?;
    println!("vault leaves: {} (Ethereum finalized block {})", leaves.len(), finalized.unwrap_or_default());
    Ok(leaves)
}

pub fn registry(c: &Config) -> Vec<TokenInfo> {
    c.tokens.iter().map(|t| TokenInfo { token: t.eth_token, denoms: t.denoms.clone() }).collect()
}

pub fn cache_path(cfg_id: &[u8; 32]) -> Result<PathBuf> {
    let dir = match std::env::var_os("LEZ_BRIDGE_CACHE_DIR") {
        Some(d) => PathBuf::from(d),
        None => wallet::utils::fetch_persistent_storage_path()?
            .parent()
            .ok_or_else(|| anyhow!("wallet storage has no directory"))?
            .join("bridge-client"),
    };
    Ok(match names_app() {
        None => dir.join(format!("{}.json", to_hex(cfg_id))),
        Some(app) => dir.join(format!("{}.names-{app}.json", to_hex(cfg_id))),
    })
}

/// The cache, or a new one. An unreadable cache is set aside (`.bad`) and rebuilt: it is only a
/// cache, though indexes it had handed out but the chain does not show yet are forgotten.
fn load_cache(cfg_id: &[u8; 32]) -> Result<(ClientCache, PathBuf)> {
    let path = cache_path(cfg_id)?;
    match ClientCache::load(&path, *cfg_id) {
        Ok(c) => Ok((c, path)),
        Err(e) => {
            let bad = path.with_extension("json.bad");
            eprintln!("warning: the cache is unreadable ({e}); moved to {} and rebuilding from the chains", bad.display());
            std::fs::rename(&path, &bad).ok();
            Ok((ClientCache::new(*cfg_id), path))
        }
    }
}

fn save(cache: &ClientCache, path: &std::path::Path) -> Result<()> {
    cache.save(path).map_err(|e| anyhow!(e))
}

// ---------------------------------------------------------------------------------------------
// Chain sources

pub struct HttpEth {
    http: reqwest::Client,
    url: String,
}

impl HttpEth {
    pub fn new(url: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build().context("http client")?,
            url: url.to_owned(),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let r = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| RpcError::Transient(format!("{method}: {e}")))?;
        let status = r.status();
        let bytes = r.bytes().await.map_err(|e| RpcError::Transient(format!("{method}: {e}")))?;
        let out: Value = serde_json::from_slice(&bytes).map_err(|_| {
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
            if status.is_server_error() || status.as_u16() == 429 {
                RpcError::Transient(format!("{method}: {status} {text}"))
            } else {
                RpcError::classify(&format!("{method}: {status} {text}"))
            }
        })?;
        if let Some(e) = out.get("error") {
            return Err(RpcError::classify(&format!("{method}: {e}")));
        }
        out.get("result").cloned().ok_or_else(|| RpcError::Fatal(format!("{method}: no result")))
    }

    async fn block_number(&self, tag: &str) -> Result<u64, RpcError> {
        let b = self.call("eth_getBlockByNumber", json!([tag, false])).await?;
        quantity(b.get("number").unwrap_or(&Value::Null)).ok_or_else(|| RpcError::Fatal(format!("no number for block {tag}")))
    }
}

fn quantity(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?.trim_start_matches("0x"), 16).ok()
}

fn hex_field<const N: usize>(v: &Value) -> Option<[u8; N]> {
    bridge_client::hexser::unhex(v.as_str()?).ok()?.try_into().ok()
}

impl EthLogSource for HttpEth {
    async fn latest(&self) -> Result<u64, RpcError> {
        let v = self.call("eth_blockNumber", json!([])).await?;
        quantity(&v).ok_or_else(|| RpcError::Fatal("eth_blockNumber".into()))
    }

    async fn finalized(&self) -> Result<u64, RpcError> {
        match self.block_number("finalized").await {
            Ok(n) => Ok(n),
            // A node without the tag (old or dev chains): 64 blocks of margin.
            Err(_) => Ok(self.latest().await?.saturating_sub(64)),
        }
    }

    async fn logs(&self, address: &EthAddress, topic0: &[[u8; 32]], from: u64, to: u64) -> Result<Vec<RawLog>, RpcError> {
        let topics: Vec<String> = topic0.iter().map(|t| format!("0x{}", hex(t))).collect();
        let filter = json!({
            "address": format!("0x{}", hex(address)),
            "topics": [topics],
            "fromBlock": format!("0x{from:x}"),
            "toBlock": format!("0x{to:x}"),
        });
        let v = self.call("eth_getLogs", json!([filter])).await?;
        let arr = v.as_array().ok_or_else(|| RpcError::Fatal("eth_getLogs: not an array".into()))?;
        arr.iter()
            .filter(|l| l.get("removed").and_then(Value::as_bool) != Some(true))
            .map(|l| {
                Some(RawLog {
                    block: quantity(l.get("blockNumber")?)?,
                    tx_hash: hex_field(l.get("transactionHash")?)?,
                    log_index: quantity(l.get("logIndex")?)?,
                    topics: l.get("topics")?.as_array()?.iter().map(hex_field).collect::<Option<_>>()?,
                    data: bridge_client::hexser::unhex(l.get("data")?.as_str()?).ok()?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| RpcError::Fatal("eth_getLogs: a log does not parse".into()))
    }

    async fn pause(&self, attempt: u32) {
        tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt))).await;
    }
}

impl HttpEth {
    pub async fn chain_id(&self) -> Result<u64, RpcError> {
        let v = self.call("eth_chainId", json!([])).await?;
        quantity(&v).ok_or_else(|| RpcError::Fatal("eth_chainId".into()))
    }

    /// `eth_call` of a no-argument view of `to` returning one word, at block `block`.
    pub async fn view_word(&self, to: &EthAddress, selector: [u8; 4], block: u64) -> Result<[u8; 32], RpcError> {
        let v = self
            .call("eth_call", json!([{"to": format!("0x{}", hex(to)), "data": format!("0x{}", hex(&selector))}, format!("0x{block:x}")]))
            .await?;
        hex_field(&v).ok_or_else(|| RpcError::Fatal(format!("eth_call 0x{}: not one word: {v}", hex(&selector))))
    }
}

pub struct WalletLez<'a> {
    pub wallet: &'a WalletCore<SequencerClient>,
}

impl LezBlockSource for WalletLez<'_> {
    async fn tip(&self) -> Result<u64, RpcError> {
        self.wallet.get_last_block_id().await.map_err(|e| RpcError::Transient(e.to_string()))
    }

    async fn block_txs(&self, id: u64) -> Result<Option<Vec<(Vec<u8>, bool)>>, RpcError> {
        let block = self.wallet.get_block(id).await.map_err(|e| RpcError::Transient(e.to_string()))?;
        Ok(block.map(|b| {
            b.body
                .transactions
                .iter()
                .enumerate()
                .map(|(i, tx)| (borsh::to_vec(tx).unwrap_or_default(), b.body.is_reverted(i)))
                .collect()
        }))
    }

    async fn account_exists(&self, account: &AccountId, program: &AccountId) -> Result<bool, RpcError> {
        public_state(self.wallet, *account, *program)
            .await
            .map(|s| !s.is_empty())
            .map_err(|e| RpcError::Transient(e.to_string()))
    }

    async fn pause(&self, attempt: u32) {
        tokio::time::sleep(std::time::Duration::from_millis(300 * u64::from(attempt))).await;
    }
}

/// A keyless Sepolia RPC that served `eth_getLogs` over the demo vault's whole life on 2026-11-14,
/// in one call (PublicNode had pruned it to about the last 10,000 blocks). Named in errors only:
/// `--eth-rpc` has no default.
pub const SEPOLIA_HISTORY_RPC: &str = "https://sepolia.gateway.tenderly.co";

/// `--eth-rpc` as an ordered list: one URL, or several separated by commas.
pub fn eth_rpc_list(arg: &str) -> Vec<&str> {
    arg.split(',').map(str::trim).filter(|u| !u.is_empty()).collect()
}

/// Why one RPC could not bring the vault logs up to date. `incomplete`: it answered, but the
/// logs read (cached plus fresh) are not all the vault's deposits.
struct EthFail {
    incomplete: bool,
    error: anyhow::Error,
}

impl EthFail {
    fn rpc(error: anyhow::Error) -> Self {
        Self { incomplete: false, error }
    }
}

/// One RPC's part of [`sync`]: the finalized logs into the cache (each finished range saved), the
/// unfinalized ones returned, then the completeness check: the deposits read up to the latest
/// block must be exactly the vault's `depositCount()` there, ending in its `depositRoot()`.
/// Returns the fresh logs and the finalized block.
#[allow(clippy::too_many_arguments)]
async fn sync_eth(
    url: &str,
    vault: &EthAddress,
    evm_chain_id: u64,
    chain: &ChainArgs,
    from_block: u64,
    cache: &mut ClientCache,
    path: &std::path::Path,
    quiet: bool,
) -> Result<(Vec<RawLog>, u64), EthFail> {
    let eth = HttpEth::new(url).map_err(EthFail::rpc)?;
    let rpc = |e: RpcError| EthFail::rpc(anyhow!("{e}"));
    let chain_id = eth.chain_id().await.map_err(rpc)?;
    if chain_id != evm_chain_id {
        return Err(EthFail::rpc(anyhow!("this RPC is chain {chain_id}, the bridge's vault is on {}", evm_chain_id)));
    }
    let fin = eth.finalized().await.map_err(rpc)?;
    let latest = eth.latest().await.map_err(rpc)?.max(fin);
    let start = cache.eth_start(from_block);
    let policy = FetchPolicy { max_range: chain.max_range, ..FetchPolicy::default() };
    let topics = chain::vault_topics();
    let mut requests = 0;
    if start <= fin {
        let result = chain::fetch_logs(&eth, vault, &topics, start, fin, policy, |next, logs| {
            cache.eth_logs.extend(logs);
            cache.eth_next_block = next;
            if let Err(e) = cache.save(path) {
                eprintln!("warning: cache not saved: {e}");
            }
        })
        .await;
        requests += result.map_err(|e| EthFail::rpc(anyhow!("logs: {e}")))?;
    }
    let mut fresh = Vec::new();
    if latest > fin {
        let from = fin.max(start.saturating_sub(1)) + 1;
        requests += chain::fetch_logs(&eth, vault, &topics, from, latest, policy, |_, logs| fresh.extend(logs))
            .await
            .map_err(|e| EthFail::rpc(anyhow!("logs above the finalized block: {e}")))?;
    }
    let count = eth.view_word(vault, chain::SEL_DEPOSIT_COUNT, latest).await.map_err(rpc)?;
    let count = u64::from_be_bytes(count[24..].try_into().unwrap_or_default());
    let root = eth.view_word(vault, chain::SEL_DEPOSIT_ROOT, latest).await.map_err(rpc)?;
    let mut all = cache.eth_logs.clone();
    all.extend(fresh.iter().cloned());
    let view = chain::decode_vault_logs(&all);
    chain::check_deposits_complete(&view.deposits, latest, count, &root)
        .map_err(|e| EthFail { incomplete: true, error: anyhow!("refused, the vault logs are incomplete: {e}") })?;
    if !quiet {
        eprintln!(
            "ethereum: vault logs to block {latest} (finalized {fin}) through {url}: {} cached + {} fresh, {requests} eth_getLogs request(s); \
             {count} deposit(s), as the vault counts",
            cache.eth_logs.len(),
            fresh.len()
        );
    }
    Ok((fresh, fin))
}

/// The Ethereum part of [`sync`], without a wallet: `--eth-rpc`'s RPCs in order until one brings
/// the vault logs up to date and passes the completeness check ([`sync_eth`]). A short set resets
/// the cache's Ethereum part (and, when the hole may have come from the cache, reads the same RPC
/// once more from scratch) before the next RPC. Returns the fresh logs and the finalized block.
async fn sync_eth_list(
    list: &str,
    vault: &EthAddress,
    evm_chain_id: u64,
    chain: &ChainArgs,
    cache: &mut ClientCache,
    path: &std::path::Path,
    quiet: bool,
) -> Result<(Vec<RawLog>, u64)> {
    let urls = eth_rpc_list(list);
    if urls.is_empty() {
        bail!("--eth-rpc is empty");
    }
    let from_block = eth_from_block(vault, evm_chain_id, chain.eth_from_block);
    let mut failures = Vec::new();
    let mut retried_from_scratch = false;
    let mut i = 0;
    while i < urls.len() {
        let url = urls[i];
        let resumed = cache.eth_next_block > from_block && cache.eth_from_block <= from_block;
        match sync_eth(url, vault, evm_chain_id, chain, from_block, cache, path, quiet).await {
            Ok(done) => return Ok(done),
            Err(EthFail { incomplete, error }) => {
                if incomplete {
                    // The cache may hold the hole (an earlier run, an earlier RPC): never resume past it.
                    cache.eth_logs.clear();
                    cache.eth_next_block = 0;
                    save(cache, path)?;
                }
                if !quiet {
                    eprintln!("ethereum: {url}: {error:#}");
                }
                failures.push(format!("{url}: {error:#}"));
                if incomplete && resumed && !retried_from_scratch {
                    retried_from_scratch = true;
                    if !quiet {
                        eprintln!("ethereum: the cache was reset; reading {url} again from block {from_block}");
                    }
                    continue;
                }
                if !quiet && i + 1 < urls.len() {
                    eprintln!("ethereum: trying the next RPC, {}", urls[i + 1]);
                }
            }
        }
        i += 1;
    }
    bail!(
        "no Ethereum RPC gave the vault's complete deposit history:\n  {}\n\
         Give --eth-rpc (or LEZ_BRIDGE_ETH_RPC) an RPC that serves eth_getLogs back to the vault's creation block, \
         e.g. {SEPOLIA_HISTORY_RPC} for Sepolia; a comma-separated list is tried in order",
        failures.join("\n  ")
    );
}

/// Brings the cache's chain data up to date. Returns the vault view (finalized logs from the
/// cache plus fresh unfinalized ones) and the Ethereum finalized block, or `None` without an
/// Ethereum RPC. Every finished range is saved, so an interrupted sync resumes. `--eth-rpc` may
/// list several RPCs (comma-separated): each is tried in order until one gives every deposit the
/// vault counts; a sync that finds fewer resets the cache's Ethereum part and never proceeds.
pub async fn sync(
    wallet: &WalletCore<SequencerClient>,
    config: &Config,
    bridge: AccountId,
    chain: &ChainArgs,
    cache: &mut ClientCache,
    path: &std::path::Path,
    lez_blocks: bool,
    quiet: bool,
) -> Result<(VaultView, Option<u64>)> {
    let started = Instant::now();
    let mut fresh: Vec<RawLog> = Vec::new();
    let mut finalized = None;
    if let Some(list) = &chain.eth_rpc {
        let (logs, fin) = sync_eth_list(list, &config.vault, config.evm_chain_id, chain, cache, path, quiet).await?;
        fresh = logs;
        finalized = Some(fin);
    }
    if lez_blocks {
        let lez = WalletLez { wallet };
        let tip = lez.tip().await.map_err(|e| anyhow!("LEZ sequencer: {e}"))?;
        let start = cache.lez_start(chain.lez_from_block);
        let t = Instant::now();
        let config_addr = *config_address(&bridge).value();
        if start <= tip {
            let r = chain::scan_lez(&lez, &config_addr, start, tip, 5, |next, txs| {
                cache.lez_txs.extend(txs);
                cache.lez_next_block = next;
                if next % 500 == 0
                    && let Err(e) = cache.save(path)
                {
                    eprintln!("warning: cache not saved: {e}");
                }
            })
            .await;
            save(cache, path)?;
            r.map_err(|e| anyhow!("LEZ blocks: {e}"))?;
        }
        if !quiet {
            let n = tip.saturating_sub(start) + u64::from(start <= tip);
            eprintln!(
                "lez: scanned blocks {start}..={tip} ({n} blocks in {:.1?}); {} bridge transaction(s) cached",
                t.elapsed(),
                cache.lez_txs.len()
            );
        }
    }
    save(cache, path)?;
    let mut all = cache.eth_logs.clone();
    all.extend(fresh);
    let view = chain::decode_vault_logs(&all);
    if !quiet {
        eprintln!("chain sync {:.1?}", started.elapsed());
    }
    Ok((view, finalized))
}

/// Every account the wallet can derive for, plus `account_gap` not-yet-created accounts of each
/// key tree (in memory only: the wallet file is not written).
pub fn wallet_accounts(wallet: &mut WalletCore<SequencerClient>, account_gap: u32) -> Vec<WalletAccount> {
    let kc = wallet.storage_mut().key_chain_mut();
    for _ in 0..account_gap {
        kc.generate_new_public_transaction_private_key(None);
        kc.generate_new_privacy_preserving_transaction_key_chain(None);
    }
    let kc = wallet.storage().key_chain();
    let mut ids: Vec<AccountId> = kc.public_account_ids().map(|(id, _)| id).collect();
    ids.extend(kc.private_account_ids().map(|(id, _)| id));
    ids.sort_unstable_by_key(|id| *id.value());
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| account_root(wallet, id).map(|(root, private)| WalletAccount { id, root, private }))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Commands

pub async fn recover(wallet: &mut WalletCore<SequencerClient>, args: RecoverArgs) -> Result<()> {
    let started = Instant::now();
    let config = bridge_config(wallet, args.bridge).await?;
    let (mut cache, path) = load_cache(&config.cfg_id)?;
    if args.chain.eth_rpc.is_none() {
        bail!("recover needs --eth-rpc (or LEZ_BRIDGE_ETH_RPC): the deposits are on Ethereum");
    }
    let point = args.lez_mode == "point";
    let (view, finalized) = sync(wallet, &config, args.bridge, &args.chain, &mut cache, &path, !point, out::json()).await?;
    let accounts = wallet_accounts(wallet, args.account_gap);
    let eth_dests = args.eth_dests.iter().map(|a| eth(a)).collect::<Result<Vec<_>>>()?;
    let params = ScanParams {
        cfg_id: config.cfg_id,
        bridge: args.bridge,
        gap: args.gap,
        eth_dests,
        extra_fees: args.max_relay_fees.clone(),
        finalized_block: finalized,
        known_next: cache.next.clone(),
    };
    let reg = registry(&config);
    let t = Instant::now();
    let mut r = if point {
        // First pass finds the deposits; then one point query per found deposit's nullifier PDA.
        let first = scan::recover(&accounts, &reg, &view, &[], &params, Some(&HashMap::new()));
        let lez = WalletLez { wallet };
        let mut spent = HashMap::new();
        for d in &first.deposits {
            let pda = bridge_core::nullifier_address(&args.bridge, &d.claim_nullifier);
            if lez.account_exists(&pda, &args.bridge).await.map_err(|e| anyhow!("{e}"))? {
                spent.insert(*pda.value(), 0);
            }
        }
        let mut r = scan::recover(&accounts, &reg, &view, &[], &params, Some(&spent));
        r.exits.clear();
        r.warnings.push(format!(
            "point mode: {} nullifier account(s) queried by id (the sequencer saw which); exits were NOT scanned (they need --lez-mode scan)",
            first.deposits.len()
        ));
        r
    } else {
        scan::recover(&accounts, &reg, &view, &cache.lez_txs, &params, None)
    };
    let match_time = t.elapsed();
    for (k, v) in &r.next {
        let n = cache.next.entry(k.clone()).or_default();
        *n = (*n).max(*v);
    }
    r.warnings.extend(view_notes(&view));
    cache.last = Some(r.clone());
    save(&cache, &path)?;
    if out::json() {
        let mut v = serde_json::to_value(&r)?;
        v["accounts_scanned"] = json!(accounts.len());
        v["lez_mode"] = json!(args.lez_mode);
        v["scheme"] = json!(names_app().map_or_else(|| "bridge-v1".to_owned(), |a| format!("names-app:{a}")));
        out::doc(&v);
    } else {
        print_recovery(&r, &config);
        println!(
            "scanned {} account(s), {} candidate(s) in {match_time:.1?}; total {:.1?}; cache {}",
            accounts.len(),
            r.candidates,
            started.elapsed(),
            path.display()
        );
    }
    Ok(())
}

fn view_notes(view: &VaultView) -> Vec<String> {
    if view.undecoded > 0 { vec![format!("{} vault log(s) did not decode", view.undecoded)] } else { vec![] }
}

fn account_text(bytes: &[u8; 32]) -> String {
    AccountId::new(*bytes).to_string()
}

fn print_recovery(r: &Recovery, config: &Config) {
    let token_name = |t: &EthAddress| format!("0x{}", hex(t));
    println!("DEPOSITS ({} found, {} claimable)", r.deposits.len(), r.claimable().count());
    println!("{:<10} {:<46} {:>5} {:<44} {:>5} {:>18} {:>6} {:>9}  tx", "state", "account", "index", "token", "denom#", "denom", "leaf", "eth block");
    for d in &r.deposits {
        let state = match d.claim {
            ClaimState::Claimable => "CLAIMABLE".to_owned(),
            ClaimState::Claimed { claim_block } => format!("claimed@{claim_block}"),
        };
        println!(
            "{state:<10} {:<46} {:>5} {:<44} {:>5} {:>18} {:>6} {:>9}  0x{}",
            account_text(&d.account),
            d.key_index,
            token_name(&d.token),
            d.denom_index,
            d.denom,
            d.leaf_index,
            d.block,
            hex(&d.tx_hash)
        );
    }
    println!("EXITS ({} found, {} unreleased)", r.exits.len(), r.unreleased().count());
    println!("{:<14} {:<46} {:>5} {:<44} {:>18} {:<44} {:>14} {:>9}", "state", "account", "index", "token", "denom", "eth dest", "max relay fee", "lez block");
    for e in &r.exits {
        let state = match &e.exit {
            ExitState::Burned => "BURNED".to_owned(),
            ExitState::Unreleased { exit_index } => format!("IN-TREE#{exit_index}"),
            ExitState::Released { release_block, .. } => format!("released@{release_block}"),
        };
        println!(
            "{state:<14} {:<46} {:>5} {:<44} {:>18} 0x{}{} {:>14} {:>9}",
            account_text(&e.account),
            e.key_index,
            token_name(&e.token),
            e.denom,
            hex(&e.eth_dest),
            if e.dest_derived { "(derived)" } else { "" },
            e.max_relay_fee,
            e.burn_block
        );
    }
    for w in &r.warnings {
        println!("WARNING: {w}");
    }
    println!("tree check: {}", if r.tree_ok { "the deposit tree rebuilt from the logs matches every logged root" } else { "FAILED (see warnings)" });
    let _ = config;
}

pub async fn positions(wallet: &WalletCore<SequencerClient>, bridge: AccountId) -> Result<()> {
    let config = bridge_config(wallet, bridge).await?;
    let (cache, path) = load_cache(&config.cfg_id)?;
    let Some(r) = cache.last else {
        bail!("no scan in {} yet: run `lez-bridge recover`", path.display());
    };
    if out::json() {
        out::doc(&serde_json::to_value(&r)?);
    } else {
        print_recovery(&r, &config);
        println!("(from the cache {}; run `recover` to refresh)", path.display());
    }
    Ok(())
}

/// The next index for `key`: scans the chain first (vault logs, and LEZ blocks for exits), then
/// takes the larger of the chain's and the cache's next index.
async fn allocate(
    wallet: &WalletCore<SequencerClient>,
    config: &Config,
    bridge: AccountId,
    chain: &ChainArgs,
    account: AccountId,
    root: AccountRoot,
    key: &str,
    exits: bool,
    eth_dests: Vec<EthAddress>,
    fees: Vec<u128>,
) -> Result<(u64, ClientCache, PathBuf, VaultView, Vec<chain::LezTx>)> {
    let (mut cache, path) = load_cache(&config.cfg_id)?;
    if chain.eth_rpc.is_none() {
        bail!("give --eth-rpc (or LEZ_BRIDGE_ETH_RPC): the next index is chosen after scanning the chain, never from the local cache alone");
    }
    let (view, finalized) = sync(wallet, config, bridge, chain, &mut cache, &path, exits, true).await?;
    let params = ScanParams {
        cfg_id: config.cfg_id,
        bridge,
        gap: 20,
        eth_dests,
        extra_fees: fees,
        finalized_block: finalized,
        known_next: cache.next.clone(),
    };
    let me = [WalletAccount { id: account, root, private: false }];
    let txs = if exits { cache.lez_txs.clone() } else { Vec::new() };
    let r = scan::recover(&me, &registry(config), &view, &txs, &params, Some(&HashMap::new()));
    let j = scan::allocate(key, &r, &cache.next);
    Ok((j, cache, path, view, txs))
}

#[derive(clap::Args, Debug)]
pub struct DepositRequestArgs {
    #[arg(long)]
    pub bridge: AccountId,
    /// The LEZ account the deposit will be claimed into (it must be this wallet's).
    #[arg(long)]
    pub from: AccountId,
    /// The Ethereum token (0x0 for ETH).
    #[arg(long)]
    pub eth_token: String,
    #[arg(long)]
    pub denom_index: u32,
    /// Use this index instead of the next free one (re-showing a request already handed out).
    #[arg(long)]
    pub key_index: Option<u64>,
    #[command(flatten)]
    pub chain: ChainArgs,
}

pub async fn deposit_request(wallet: &WalletCore<SequencerClient>, a: DepositRequestArgs) -> Result<()> {
    let config = bridge_config(wallet, a.bridge).await?;
    let token = eth(&a.eth_token)?;
    let t = config.tokens.iter().find(|t| t.eth_token == token).ok_or_else(|| anyhow!("token not registered on this bridge"))?;
    let denom = *t.denoms.get(a.denom_index as usize).ok_or_else(|| anyhow!("bad denomination index; the token has {:?}", t.denoms))?;
    let root = root_or_bail(wallet, a.from)?;
    let key = scan::dep_key(&a.from, &token, denom);
    let (j, mut cache, path) = match a.key_index {
        Some(j) => {
            let (c, p) = load_cache(&config.cfg_id)?;
            (j, c, p)
        }
        None => {
            let (j, c, p, _, _) = allocate(wallet, &config, a.bridge, &a.chain, a.from, root_or_bail(wallet, a.from)?, &key, false, vec![], vec![]).await?;
            (j, c, p)
        }
    };
    let c = scan::deposit_candidate(&root, &config.cfg_id, &a.from, &token, denom, j);
    let request = DepositRequest::new(config.evm_chain_id, config.vault, token, u8::try_from(a.denom_index)?, denom, c.commitment);
    let text = request.encode().map_err(|e| anyhow!("{e}"))?;
    cache.handed_out(&key, j);
    save(&cache, &path)?;
    let doc = json!({
        "account": a.from.to_string(),
        "keyIndex": j,
        "commitment": format!("0x{}", hex(&c.commitment)),
        "request": request.to_json(),
        "requestString": text,
    });
    out::doc(&doc);
    println!("{}", serde_json::to_string_pretty(&doc)?);
    println!("COMMITMENT=0x{}", hex(&c.commitment));
    println!("KEY_INDEX={j}");
    println!("REQUEST={text}");
    Ok(())
}

/// What a derived claim needs: the claim secret, the vault's leaves and this deposit's leaf index.
pub async fn derived_claim(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    from: AccountId,
    token: EthAddress,
    denom: u128,
    key_index: u64,
    chain: &ChainArgs,
) -> Result<([u8; 32], Vec<[u8; 32]>, usize)> {
    let config = bridge_config(wallet, bridge).await?;
    let root = root_or_bail(wallet, from)?;
    let (mut cache, path) = load_cache(&config.cfg_id)?;
    let (view, _) = sync(wallet, &config, bridge, chain, &mut cache, &path, false, true).await?;
    let c = scan::deposit_candidate(&root, &config.cfg_id, &from, &token, denom, key_index);
    let leaves = scan::rebuild_leaves(&view.deposits, &registry(&config), &config.cfg_id).map_err(|e| anyhow!("vault logs: {e}"))?;
    let d = view
        .deposits
        .iter()
        .find(|d| d.commitment == c.commitment)
        .ok_or_else(|| anyhow!("no vault deposit for index {key_index} of {from} (token, denomination as given) in the logs read: deposit first, or give --eth-rpc"))?;
    if d.token != token || config.tokens.iter().find(|t| t.eth_token == token).and_then(|t| t.denoms.get(d.denom_index as usize)) != Some(&denom) {
        bail!("the vault holds this commitment under another token or denomination (a copied commitment): not claimable");
    }
    println!("derived deposit: index {key_index}, leaf {} of {}", d.leaf_index, leaves.len());
    Ok((c.secret, leaves, usize::try_from(d.leaf_index)?))
}

/// A derived burn's exit commitment: picks the next index (after scanning both chains), refuses
/// an `E` already burned, and returns `(E, key, index)` to record after the burn is applied.
#[allow(clippy::too_many_arguments)]
pub async fn derived_exit(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    from: AccountId,
    definition: AccountId,
    amount: u128,
    eth_dest: Option<&str>,
    max_relay_fee: Option<u128>,
    key_index: Option<u64>,
    chain: &ChainArgs,
) -> Result<([u8; 32], String, u64)> {
    let config = bridge_config(wallet, bridge).await?;
    let t = config.tokens.iter().find(|t| t.definition == definition).ok_or_else(|| anyhow!("{definition} is not a bridged token"))?;
    let di = t.denoms.iter().position(|d| *d == amount).ok_or_else(|| anyhow!("a burn is exactly one denomination: {:?}", t.denoms))?;
    let token = t.eth_token;
    let dest = match eth_dest {
        None | Some("derived") => Dest::Derived,
        Some(a) => {
            let a = eth(a)?;
            if a == [0; 20] || a == config.vault {
                bail!("refused: an exit to the zero address or to the vault itself can never be released");
            }
            Dest::Address(a)
        }
    };
    let cap = kdf::max_relay_fee_cap(amount);
    let fee = max_relay_fee.unwrap_or(cap);
    let root = root_or_bail(wallet, from)?;
    let key = scan::exit_key(&from, &token, amount, &dest, fee);
    let eth_dests = match dest {
        Dest::Address(a) => vec![a],
        Dest::Derived => vec![],
    };
    let (j, _cache, _path, view, txs) = allocate(wallet, &config, bridge, chain, from, root, &key, true, eth_dests, vec![fee]).await?;
    let allowed = scan::fee_candidates(&view, &token, di as u32, amount, &[]);
    if !allowed.contains(&fee) {
        bail!("maxRelayFee {fee} is not 0, the 2% cap {cap} or a value ever on the vault's relay-fee menu {allowed:?}: recovery could not find this exit");
    }
    let j = key_index.unwrap_or(j);
    let root = root_or_bail(wallet, from)?;
    let c = scan::exit_candidate(&root, &config.cfg_id, &from, &token, amount, &dest, fee, j);
    if txs.iter().any(|tx| chain::contains(&tx.bytes, &c.exit_commitment)) {
        bail!("refused: exit index {j} of this tuple was already burned (the same E twice could be released only once)");
    }
    println!(
        "derived exit: index {j}, destination 0x{}{}, maxRelayFee {fee}",
        hex(&c.eth_dest),
        if dest == Dest::Derived { " (seed-derived; export its key with `lez-bridge eth-key`)" } else { " (you must name it again at recovery: --eth-dest)" }
    );
    Ok((c.exit_commitment, key, j))
}

/// Before emitting a burn of an exit commitment made elsewhere (`burn --emit-call --exit E`):
/// refuses an `E` the bridge's LEZ transactions already carry (the same `E` twice can be
/// released only once). Reads LEZ blocks only (through the cache), never Ethereum.
pub async fn refuse_burned_exit(wallet: &WalletCore<SequencerClient>, bridge: AccountId, chain_args: &ChainArgs, e: &[u8; 32]) -> Result<()> {
    let config = bridge_config(wallet, bridge).await?;
    let (mut cache, path) = load_cache(&config.cfg_id)?;
    let lez_only = ChainArgs { eth_rpc: None, ..chain_args.clone() };
    sync(wallet, &config, bridge, &lez_only, &mut cache, &path, true, true).await?;
    if cache.lez_txs.iter().any(|tx| chain::contains(&tx.bytes, e)) {
        bail!("refused: this exit commitment was already burned (the same E twice could be released only once); nothing sent");
    }
    Ok(())
}

/// After a derived burn applied: remember the index.
pub fn record_exit(config_cfg: &[u8; 32], key: &str, j: u64) -> Result<()> {
    let (mut cache, path) = load_cache(config_cfg)?;
    cache.handed_out(key, j);
    save(&cache, &path)
}

#[derive(clap::Args, Debug)]
pub struct EthKeyArgs {
    #[arg(long)]
    pub bridge: AccountId,
    #[arg(long)]
    pub from: AccountId,
    #[arg(long)]
    pub eth_token: String,
    #[arg(long)]
    pub denom: u128,
    /// Default: the 2% cap, as `burn` uses.
    #[arg(long)]
    pub max_relay_fee: Option<u128>,
    #[arg(long)]
    pub key_index: u64,
    /// File to write the private key to (created with mode 600; never printed).
    #[arg(long)]
    pub out: PathBuf,
}

pub async fn eth_key(wallet: &WalletCore<SequencerClient>, a: EthKeyArgs) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let config = bridge_config(wallet, a.bridge).await?;
    let token = eth(&a.eth_token)?;
    let fee = a.max_relay_fee.unwrap_or_else(|| kdf::max_relay_fee_cap(a.denom));
    let root = root_or_bail(wallet, a.from)?;
    let k = kdf::release_key(&root, &config.cfg_id, &a.from, &token, a.denom, fee, a.key_index);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&a.out)
        .with_context(|| format!("{} (must not exist)", a.out.display()))?;
    writeln!(f, "0x{}", hex(k.secret()))?;
    println!("release address 0x{}: key written to {}", hex(&k.address), a.out.display());
    out::doc(&json!({ "address": format!("0x{}", hex(&k.address)), "key_file": a.out.display().to_string() }));
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct ExitSecretArgs {
    #[arg(long)]
    pub bridge: AccountId,
    /// The account that burned (its key derives the exit).
    #[arg(long)]
    pub from: AccountId,
    #[arg(long)]
    pub eth_token: String,
    #[arg(long)]
    pub denom: u128,
    /// The release address the burn used: `derived` (the default, as `burn`) or the address given
    /// to `burn --eth-dest`.
    #[arg(long)]
    pub eth_dest: Option<String>,
    /// Default: the 2% cap, as `burn` uses.
    #[arg(long)]
    pub max_relay_fee: Option<u128>,
    /// The exit's index (`burn` printed it: "derived exit: index N").
    #[arg(long)]
    pub key_index: u64,
    /// File to write the exit secret to (created with mode 600; must not exist; never printed).
    #[arg(long)]
    pub out: PathBuf,
}

/// `deposit-secret`: one seed-derived deposit, named as `deposit-request` made it.
#[derive(clap::Args, Debug)]
pub struct DepositSecretArgs {
    #[arg(long)]
    pub bridge: AccountId,
    /// The account the deposit was requested for (its key derives the deposit).
    #[arg(long)]
    pub from: AccountId,
    #[arg(long)]
    pub eth_token: String,
    #[arg(long)]
    pub denom: u128,
    /// The deposit's index (`deposit-request` printed it as KEY_INDEX; `recover` lists it).
    #[arg(long)]
    pub key_index: u64,
    /// File to write the deposit MASTER secret `s` to (created with mode 600; must not exist;
    /// never printed).
    #[arg(long)]
    pub out: PathBuf,
}

/// The MASTER secret `s` of a seed-derived deposit, the failsafe reclaim proof's witness
/// (`reclaim_host reclaim --master-file`), written to a new mode-600 file. `s` appears in no
/// transaction (a claim publishes only `k = SHA256("RFP022/ck" | s)`). Prints only public values:
/// the commitment `C` (compare it with the vault's Deposit event) and the reclaim nullifier.
pub async fn deposit_secret(wallet: &WalletCore<SequencerClient>, a: DepositSecretArgs) -> Result<()> {
    let config = bridge_config(wallet, a.bridge).await?;
    let token = eth(&a.eth_token)?;
    let root = root_or_bail(wallet, a.from)?;
    let c = scan::deposit_candidate(&root, &config.cfg_id, &a.from, &token, a.denom, a.key_index);
    write_secret_file(&a.out, &c.master)?;
    let nf = kdf::reclaim_nullifier(&c.master);
    println!(
        "deposit index {}: C 0x{}, reclaim nullifier 0x{}; master secret written to {}",
        a.key_index,
        hex(&c.commitment),
        hex(&nf),
        a.out.display()
    );
    out::doc(&json!({
        "key_index": a.key_index,
        "commitment": format!("0x{}", hex(&c.commitment)),
        "reclaim_nullifier": format!("0x{}", hex(&nf)),
        "secret_file": a.out.display().to_string(),
    }));
    Ok(())
}

/// The exit secret `k` of a seed-derived exit, the G3 release proof's witness
/// (`prove-release-rc3.sh --secret-file`), written to a new mode-600 file. Prints only public
/// values: the exit commitment `E` (compare it with the burn's), the release address, the fee cap.
pub async fn exit_secret(wallet: &WalletCore<SequencerClient>, a: ExitSecretArgs) -> Result<()> {
    let config = bridge_config(wallet, a.bridge).await?;
    let token = eth(&a.eth_token)?;
    let fee = a.max_relay_fee.unwrap_or_else(|| kdf::max_relay_fee_cap(a.denom));
    let dest = match a.eth_dest.as_deref() {
        None | Some("derived") => Dest::Derived,
        Some(x) => Dest::Address(eth(x)?),
    };
    let root = root_or_bail(wallet, a.from)?;
    let c = scan::exit_candidate(&root, &config.cfg_id, &a.from, &token, a.denom, &dest, fee, a.key_index);
    write_secret_file(&a.out, &c.secret)?;
    println!(
        "exit index {}: E 0x{}, release address 0x{}, maxRelayFee {fee}; secret written to {}",
        a.key_index,
        hex(&c.exit_commitment),
        hex(&c.eth_dest),
        a.out.display()
    );
    out::doc(&json!({
        "key_index": a.key_index,
        "exit_commitment": format!("0x{}", hex(&c.exit_commitment)),
        "eth_dest": format!("0x{}", hex(&c.eth_dest)),
        "max_relay_fee": fee.to_string(),
        "secret_file": a.out.display().to_string(),
    }));
    Ok(())
}

/// `0x<hex>` and a newline into a NEW file (mode 600; an existing file is refused, never overwritten).
pub fn write_secret_file(path: &std::path::Path, secret: &[u8; 32]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("{} (must not exist)", path.display()))?;
    writeln!(f, "0x{}", hex(secret))?;
    Ok(())
}

#[cfg(test)]
mod exit_secret_file_tests {
    use super::write_secret_file;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn the_secret_file_is_new_mode_600_and_holds_the_hex_line() {
        let dir = std::env::temp_dir().join(format!("exit-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("k");
        write_secret_file(&p, &[0xab; 32]).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("0x{}\n", "ab".repeat(32)));
        // never overwritten
        assert!(write_secret_file(&p, &[0xcd; 32]).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("0x{}\n", "ab".repeat(32)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// `--eth-rpc` lists against fake Sepolia RPCs on loopback (synthetic deposits): pruned history
/// moves to the next RPC, a silently short answer is refused and resets the cache, a hole already
/// in the cache is read again from scratch. Exercises `sync_eth_list` end to end over HTTP.
#[cfg(test)]
mod rpc_fallback_tests {
    use std::{
        io::{BufRead as _, BufReader, Read as _, Write as _},
        net::TcpListener,
    };

    use bridge_client::chain::{DepositEvent, encode_deposit_log};
    use serde_json::{Value, json};

    use super::*;

    const VAULT: [u8; 20] = [0x5A; 20];
    const CHAIN: u64 = 31_337;
    const CREATED: u64 = 1_000;
    const FIN: u64 = 19_930;
    const HEAD: u64 = 20_000;
    const KEPT: u64 = 10_000;

    fn deposits() -> Vec<DepositEvent> {
        [1_017u64, 1_018, 1_019, 1_020, 1_021, 13_996, 16_826, 19_990]
            .iter()
            .enumerate()
            .map(|(i, b)| DepositEvent {
                token: [0; 20],
                denom_index: 0,
                commitment: [i as u8 + 1; 32],
                leaf_index: i as u64,
                root: [0xA0 + i as u8; 32],
                block: *b,
                tx_hash: [i as u8 + 0x10; 32],
            })
            .collect()
    }

    fn word(n: u64) -> String {
        format!("0x{n:064x}")
    }

    /// One JSON-RPC answer. `mode`: "full", "pruned" (refuses ranges below KEPT) or "drops"
    /// (answers without the logs below KEPT).
    fn answer(mode: &str, req: &Value) -> Value {
        let p = &req["params"];
        let num = |v: &Value| u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
        let result = match req["method"].as_str().unwrap() {
            "eth_chainId" => json!(format!("0x{CHAIN:x}")),
            "eth_blockNumber" => json!(format!("0x{HEAD:x}")),
            "eth_getBlockByNumber" => json!({"number": format!("0x{FIN:x}")}),
            "eth_call" => {
                let at = num(&p[1]);
                let n = deposits().iter().filter(|d| d.block <= at).count();
                if p[0]["data"] == "0x2dfdf0b5" {
                    json!(word(n as u64))
                } else {
                    json!(format!("0x{}", hex(&deposits()[n - 1].root)))
                }
            }
            "eth_getLogs" => {
                let (lo, hi) = (num(&p[0]["fromBlock"]), num(&p[0]["toBlock"]));
                if mode == "pruned" && lo < KEPT {
                    return json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": 4444, "message": "pruned history unavailable"}});
                }
                let floor = if mode == "drops" { KEPT } else { 0 };
                let logs: Vec<Value> = deposits()
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| (lo..=hi).contains(&d.block) && d.block >= floor)
                    .map(|(i, d)| {
                        let l = encode_deposit_log(d, i as u64);
                        json!({
                            "blockNumber": format!("0x{:x}", l.block),
                            "transactionHash": format!("0x{}", hex(&l.tx_hash)),
                            "logIndex": format!("0x{:x}", l.log_index),
                            "topics": l.topics.iter().map(|t| format!("0x{}", hex(t))).collect::<Vec<_>>(),
                            "data": format!("0x{}", hex(&l.data)),
                            "removed": false,
                        })
                    })
                    .collect();
                json!(logs)
            }
            m => panic!("unexpected {m}"),
        };
        json!({"jsonrpc": "2.0", "id": req["id"], "result": result})
    }

    /// A loopback HTTP/1.1 JSON-RPC server (keep-alive) in a thread; returns its URL.
    fn serve(mode: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut out = stream;
                    loop {
                        let mut len = 0usize;
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        loop {
                            line.clear();
                            reader.read_line(&mut line).unwrap();
                            if line == "\r\n" || line.is_empty() {
                                break;
                            }
                            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                                len = v.trim().parse().unwrap();
                            }
                        }
                        let mut body = vec![0; len];
                        reader.read_exact(&mut body).unwrap();
                        let req: Value = serde_json::from_slice(&body).unwrap();
                        let resp = answer(mode, &req).to_string();
                        write!(out, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{resp}", resp.len()).unwrap();
                    }
                });
            }
        });
        url
    }

    fn chain_args(list: &str) -> ChainArgs {
        ChainArgs { eth_rpc: Some(list.to_owned()), eth_from_block: CREATED, lez_from_block: 1, max_range: 2_000 }
    }

    fn cache_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lez-bridge-rpc-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("cache.json")
    }

    fn run(list: &str, cache: &mut ClientCache, name: &str) -> Result<(Vec<RawLog>, u64)> {
        let path = cache_file(name);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(sync_eth_list(list, &VAULT, CHAIN, &chain_args(list), cache, &path, true))
    }

    fn all_deposits(cache: &ClientCache, fresh: &[RawLog]) -> Vec<DepositEvent> {
        let mut logs = cache.eth_logs.clone();
        logs.extend(fresh.iter().cloned());
        chain::decode_vault_logs(&logs).deposits
    }

    #[test]
    fn pruned_then_full_uses_the_second_rpc() {
        let list = format!("{}, {}", serve("pruned"), serve("full"));
        let mut cache = ClientCache::new([1; 32]);
        let (fresh, fin) = run(&list, &mut cache, "a").unwrap();
        assert_eq!(fin, FIN);
        assert_eq!(all_deposits(&cache, &fresh), deposits(), "every deposit, cached and fresh");
        assert_eq!(fresh.len(), 1, "the deposit above the finalized block is fresh, not cached");
    }

    #[test]
    fn a_silently_short_answer_is_refused_and_resets_the_cache() {
        let drops = serve("drops");
        let mut cache = ClientCache::new([2; 32]);
        let err = format!("{:#}", run(&drops, &mut cache, "b").unwrap_err());
        assert!(err.contains("refused, the vault logs are incomplete") && err.contains("skip from leaf 0 to leaf 5"), "{err}");
        assert!(err.contains(&drops) && err.contains(SEPOLIA_HISTORY_RPC), "names the RPC and suggests another: {err}");
        assert_eq!((cache.eth_next_block, cache.eth_logs.len()), (0, 0), "nothing short is kept");
        // With a full RPC after it, the same list succeeds.
        let list = format!("{drops},{}", serve("full"));
        let (fresh, _) = run(&list, &mut cache, "b").unwrap();
        assert_eq!(all_deposits(&cache, &fresh), deposits());
    }

    #[test]
    fn a_hole_in_the_cache_is_read_again_from_scratch() {
        // An earlier run through a provider that dropped the first five deposits, saved to FIN.
        let mut cache = ClientCache::new([3; 32]);
        cache.eth_start(CREATED);
        cache.eth_logs = deposits()[5..7].iter().enumerate().map(|(i, d)| encode_deposit_log(d, i as u64)).collect();
        cache.eth_next_block = FIN + 1;
        let (fresh, _) = run(&serve("full"), &mut cache, "c").unwrap();
        assert_eq!(all_deposits(&cache, &fresh), deposits(), "the full RPC refilled the cache from the creation block");
    }

    #[test]
    fn the_list_is_split_on_commas() {
        assert_eq!(eth_rpc_list(" a ,b,, c "), vec!["a", "b", "c"]);
        assert!(eth_rpc_list(" , ").is_empty());
    }
}
