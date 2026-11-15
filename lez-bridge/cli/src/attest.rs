//! `lez-bridge attest ...`: the client for the RFP-021 attestation program (`eth_attest`) and its
//! reference consumer (`eth_attest_probe`).
//!
//! - `init`: create an instance from a trusted beacon checkpoint. The bootstrap is verified
//!   natively first (the same `eth_attest::lc::verify_bootstrap` the program runs), then sent as a
//!   PUBLIC transaction: about 9.0M user cycles, under LEZ's 10M cap, so it declares the cap as
//!   its gas limit.
//! - `update`: advance an instance by one finality or period update. The update is verified
//!   natively first (`eth_attest::lc::verify_update`); a failure stops here with Helios's reason.
//!   It costs 37–46M user cycles, over the 10M cap per transaction and per block, so it is sent
//!   only from a PRIVATE account of this wallet: the verification runs in that account's privately
//!   proved execution and only the `Advance` effect (plus the anchor records) runs publicly.
//!   `--dry-run` simulates it with the wallet (no proof) and prints those public effects.
//! - `show`: the instance's config and store, and whether an anchor is recorded.
//! - `prove-slot`: an `eth_getProof` storage statement through the probe, publicly (0.25–1.0M
//!   cycles), against an anchor the instance finalized.
//!
//! Every fetch is a plain public HTTP GET/POST with a generic user agent. Fetched material can be
//! saved (`--save-dir`) and replayed (`--load-dir`), so a proof started hours later uses exactly
//! what was checked.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::Subcommand;
use eth_attest_core::{
    Anchor, AnchorRecord, Fork, InstanceConfig, Instruction as AttestInstruction, LcSummary, LcUpdate, MIN_SUPERMAJORITY,
    Message as AttestMessage, SYNC_COMMITTEE_SIZE, anchor_address, committee_hash, config_address, instance_id,
    store_address,
    inclusion::{AnchorWitness, Predicate, StatementContext, keccak, verify_storage_slot},
};
use eth_attest_probe::Probe;
use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec as S,
    types::{Bootstrap, FinalityUpdate, GenericUpdate, Update},
};
use serde_json::{Value, json};
use ssz::{Decode as _, Encode as _};

use super::{
    AccountId, AccountIdentity, LeeTransaction, SequencerClient, WalletCore, eth, hex32, hex_bytes, identity,
    public_state, send_tx, signer, storage_proof_json, to_hex,
};

/// What `init` declares as its gas limit: LEZ's cap (`MAX_GAS_EXEC`). A bootstrap costs about
/// 9.0M user cycles (`attest/cycles/results/`), over the wallet's 2M default.
pub const INIT_GAS_LIMIT: u64 = 10_000_000;
const SLOTS_PER_PERIOD: u64 = 32 * 256;
const UA: &str = "Mozilla/5.0 (compatible; lc-fetch/0.1)";

#[derive(Subcommand)]
pub enum AttestCommand {
    /// The instance id and PDA addresses a config would get (nothing fetched or sent).
    Id {
        #[arg(long)]
        program: AccountId,
        #[arg(long, value_parser = ["sepolia", "mainnet"])]
        network: String,
        /// The checkpoint beacon block root (32 bytes of hex).
        #[arg(long)]
        checkpoint: String,
        #[arg(long, default_value_t = MIN_SUPERMAJORITY)]
        min_participants: u16,
        #[arg(long, default_value = ZERO32)]
        salt: String,
    },
    /// Create an instance from a trusted checkpoint; a public transaction signed and paid by
    /// `--from`.
    Init {
        #[arg(long)]
        program: AccountId,
        /// A public account of this wallet: signs and pays.
        #[arg(long)]
        from: AccountId,
        #[arg(long, value_parser = ["sepolia", "mainnet"])]
        network: String,
        /// The trusted checkpoint: a beacon block root (32 bytes of hex), or `finalized` for the
        /// beacon node's current finalized block (cross-check it with `--cross-check`).
        #[arg(long)]
        checkpoint: String,
        /// Beacon API for the network check, the checkpoint and the bootstrap (default per network).
        #[arg(long)]
        beacon: Option<String>,
        /// Further beacon APIs that must report the same checkpoint root (repeatable).
        #[arg(long)]
        cross_check: Vec<String>,
        /// The bootstrap's SSZ from a file instead of `--beacon`.
        #[arg(long)]
        bootstrap_file: Option<PathBuf>,
        /// Finality policy: fewest sync-committee signers an update may have (342..=512).
        #[arg(long, default_value_t = MIN_SUPERMAJORITY)]
        min_participants: u16,
        /// 32 bytes of hex; another salt is another instance.
        #[arg(long, default_value = ZERO32)]
        salt: String,
        /// Write the bootstrap, committee and ids here.
        #[arg(long)]
        save_dir: Option<PathBuf>,
    },
    /// Advance an instance by one update, from a PRIVATE account of this wallet (the update is
    /// verified in its privately proved execution). With `--dry-run`: simulate, no proof.
    Update {
        #[arg(long)]
        program: AccountId,
        /// The instance id (32 bytes of hex).
        #[arg(long)]
        instance: String,
        /// A private account of this wallet: runs the verification and pays the fee.
        #[arg(long)]
        from: AccountId,
        /// Beacon API for the update and committees (default per chain).
        #[arg(long)]
        beacon: Option<String>,
        /// `auto` picks a finality update, or the period update the store needs first.
        #[arg(long, value_parser = ["auto", "finality", "period"], default_value = "auto")]
        kind: String,
        /// The update's SSZ from a file (with `--kind finality` or `--kind period`).
        #[arg(long)]
        update_file: Option<PathBuf>,
        /// Replay what `--save-dir` wrote: update, kind and committees.
        #[arg(long, conflicts_with_all = ["update_file", "save_dir"])]
        load_dir: Option<PathBuf>,
        /// Save the update, its kind, the committee witness and the expected next store.
        #[arg(long)]
        save_dir: Option<PathBuf>,
        /// Committee SSZ cache, by commitment (default: `attest-committees` next to the wallet).
        #[arg(long)]
        committees: Option<PathBuf>,
    },
    /// The instance's config, store and anchors (read from the chain).
    Show {
        #[arg(long)]
        program: AccountId,
        #[arg(long)]
        instance: String,
        /// Report whether this execution block hash is recorded.
        #[arg(long)]
        anchor_block_hash: Option<String>,
        /// Report whether this execution state root is recorded.
        #[arg(long)]
        anchor_state_root: Option<String>,
    },
    /// Prove a storage slot through the reference consumer (public transaction from `--from`)
    /// against an anchor of the instance: by default its latest finalized execution block.
    ProveSlot {
        /// The deployed probe program.
        #[arg(long)]
        probe: AccountId,
        #[arg(long)]
        program: AccountId,
        #[arg(long)]
        instance: String,
        /// A public account of this wallet: signs and pays.
        #[arg(long)]
        from: AccountId,
        /// Ethereum JSON-RPC serving `eth_getProof` at the anchor block.
        #[arg(long)]
        eth_rpc: Option<String>,
        /// The contract (20 bytes of hex).
        #[arg(long)]
        contract: String,
        /// A raw storage slot (32 bytes of hex)...
        #[arg(long, conflicts_with_all = ["mapping_slot", "key"])]
        slot: Option<String>,
        /// ...or a `mapping(bytes32 => ..)` at this slot index, with `--key`.
        #[arg(long, requires = "key")]
        mapping_slot: Option<u64>,
        #[arg(long)]
        key: Option<String>,
        /// Predicate: the value equals this word (default: the proven value)...
        #[arg(long, conflicts_with = "ne")]
        eq: Option<String>,
        /// ...or differs from it.
        #[arg(long)]
        ne: Option<String>,
        /// Use this anchor (execution block hash) instead of the instance's latest.
        #[arg(long)]
        anchor_block_hash: Option<String>,
        /// Save the header and proof fetched.
        #[arg(long)]
        save_dir: Option<PathBuf>,
        /// Use the header and proof `--save-dir` wrote instead of fetching.
        #[arg(long, conflicts_with = "save_dir")]
        load_dir: Option<PathBuf>,
        /// Fetch, check natively and save; send nothing (the anchor need not be recorded yet).
        #[arg(long)]
        fetch_only: bool,
    },
}

const ZERO32: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

// ---------------------------------------------------------------------------------------------
// Networks

struct Network {
    name: &'static str,
    chain_id: u64,
    genesis_validators_root: &'static str,
    genesis_time: u64,
    /// genesis, altair, bellatrix, capella, deneb, electra, fulu, gloas.
    forks: [(u64, &'static str); 8],
    beacon: &'static str,
}

/// From `/eth/v1/beacon/genesis` and `/eth/v1/config/fork_schedule` as fetched for
/// `attest/fixtures/{sepolia,mainnet}` (2026-11-12); `init` re-checks them against its beacon node.
const NETWORKS: [Network; 2] = [
    Network {
        name: "sepolia",
        chain_id: 11_155_111,
        genesis_validators_root: "0xd8ea171f3c94aea21ebc42a1ed61052acf3f9209c00e4efbaaddac09ed9b8078",
        genesis_time: 1_655_733_600,
        forks: [
            (0, "0x90000069"),
            (50, "0x90000070"),
            (100, "0x90000071"),
            (56_832, "0x90000072"),
            (132_608, "0x90000073"),
            (222_464, "0x90000074"),
            (272_640, "0x90000075"),
            (353_024, "0x90000076"),
        ],
        beacon: "https://lodestar-sepolia.chainsafe.io",
    },
    Network {
        name: "mainnet",
        chain_id: 1,
        genesis_validators_root: "0x4b363db94e286120d76eb905340fdd4e54bfe9f06bf33ff6cf5ad27f511bfe95",
        genesis_time: 1_606_824_023,
        forks: [
            (0, "0x00000000"),
            (74_240, "0x01000000"),
            (144_896, "0x02000000"),
            (194_048, "0x03000000"),
            (269_568, "0x04000000"),
            (364_032, "0x05000000"),
            (411_392, "0x06000000"),
            (u64::MAX, "0x07000000"),
        ],
        beacon: "https://lodestar-mainnet.chainsafe.io",
    },
];

fn network(name: &str) -> &'static Network {
    NETWORKS.iter().find(|n| n.name == name).expect("clap restricts the names")
}

fn network_by_chain(chain_id: u64) -> Option<&'static Network> {
    NETWORKS.iter().find(|n| n.chain_id == chain_id)
}

fn version(text: &str) -> [u8; 4] {
    let v = hex_bytes(text).expect("preset fork versions are hex");
    [v[0], v[1], v[2], v[3]]
}

fn base_config(net: &Network, checkpoint: [u8; 32], min_participants: u16, salt: [u8; 32]) -> Result<InstanceConfig> {
    ensure!(
        (MIN_SUPERMAJORITY..=SYNC_COMMITTEE_SIZE).contains(&min_participants),
        "--min-participants must be {MIN_SUPERMAJORITY}..={SYNC_COMMITTEE_SIZE} (a 2/3 supermajority of 512 or more)"
    );
    Ok(InstanceConfig {
        evm_chain_id: net.chain_id,
        genesis_validators_root: hex32(net.genesis_validators_root)?,
        genesis_time: net.genesis_time,
        seconds_per_slot: 12,
        forks: core::array::from_fn(|i| Fork { epoch: net.forks[i].0, version: version(net.forks[i].1) }),
        checkpoint,
        min_participants,
        salt,
    })
}

/// The preset must agree with the beacon node: a wrong genesis validators root or fork version
/// makes every update fail its signature check.
async fn check_network(http: &reqwest::Client, beacon: &str, config: &InstanceConfig) -> Result<()> {
    let genesis = get_json(http, &format!("{beacon}/eth/v1/beacon/genesis")).await?;
    let g = &genesis["data"];
    let gvr = hex32(g["genesis_validators_root"].as_str().unwrap_or_default())?;
    let time: u64 = g["genesis_time"].as_str().unwrap_or_default().parse().context("genesis_time")?;
    ensure!(
        gvr == config.genesis_validators_root && time == config.genesis_time,
        "{beacon} reports genesis root 0x{} at {time}, not this network's",
        to_hex(&gvr)
    );
    let schedule = get_json(http, &format!("{beacon}/eth/v1/config/fork_schedule")).await?;
    let entries = schedule["data"].as_array().ok_or_else(|| anyhow!("{beacon}: no fork schedule"))?;
    for (i, fork) in config.forks.iter().enumerate() {
        let Some(e) = entries.get(i) else {
            ensure!(fork.epoch == u64::MAX, "{beacon} schedules no fork {i}, the preset has epoch {}", fork.epoch);
            continue;
        };
        let epoch: u64 = e["epoch"].as_str().unwrap_or_default().parse().context("fork epoch")?;
        let v = hex_bytes(e["current_version"].as_str().unwrap_or_default())?;
        ensure!(
            epoch == fork.epoch && v == fork.version,
            "{beacon} schedules fork {i} as 0x{} at epoch {epoch}; the preset has 0x{} at {}",
            to_hex(&v),
            to_hex(&fork.version),
            fork.epoch
        );
    }
    println!("network check: genesis root, genesis time and 8 fork versions agree with {beacon}");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// HTTP

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("http client")
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value> {
    let r = http.get(url).header("Accept", "application/json").send().await.with_context(|| format!("GET {url}"))?;
    let status = r.status();
    let body = r.bytes().await?;
    ensure!(status.is_success(), "GET {url}: {status}: {}", String::from_utf8_lossy(&body[..body.len().min(300)]));
    serde_json::from_slice(&body).with_context(|| format!("GET {url}: not JSON"))
}

async fn get_ssz(http: &reqwest::Client, url: &str) -> Result<(Vec<u8>, String)> {
    let r = http
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = r.status();
    let fork = r
        .headers()
        .get("eth-consensus-version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("?")
        .to_owned();
    let body = r.bytes().await?;
    ensure!(status.is_success(), "GET {url}: {status}: {}", String::from_utf8_lossy(&body[..body.len().min(300)]));
    Ok((body.to_vec(), fork))
}

async fn eth_rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let r = http.post(url).json_body(&body).send().await.with_context(|| format!("{method} at {url}"))?;
    let out: Value = serde_json::from_slice(&r.bytes().await?).with_context(|| format!("{method}: not JSON"))?;
    if let Some(e) = out.get("error") {
        bail!("{method} at {url}: {e}");
    }
    out.get("result").cloned().ok_or_else(|| anyhow!("{method} at {url}: no result"))
}

/// reqwest's `json` feature is not enabled; a JSON body by hand.
trait JsonBody {
    fn json_body(self, v: &Value) -> Self;
}

impl JsonBody for reqwest::RequestBuilder {
    fn json_body(self, v: &Value) -> Self {
        self.header("Content-Type", "application/json").body(v.to_string())
    }
}

/// The beacon block root and slot of a header id (`finalized`, a root, a slot).
async fn beacon_header(http: &reqwest::Client, beacon: &str, id: &str) -> Result<([u8; 32], u64)> {
    let h = get_json(http, &format!("{beacon}/eth/v1/beacon/headers/{id}")).await?;
    let d = &h["data"];
    let root = hex32(d["root"].as_str().unwrap_or_default())?;
    let slot: u64 = d["header"]["message"]["slot"].as_str().unwrap_or_default().parse().context("header slot")?;
    Ok((root, slot))
}

/// `/eth/v1/beacon/light_client/updates` SSZ: `[u64 LE len][4-byte fork digest][payload]` per
/// item, `len = 4 + payload`.
fn split_updates(b: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        ensure!(b.len() >= i + 12, "truncated updates response");
        let n = usize::try_from(u64::from_le_bytes(b[i..i + 8].try_into()?))?;
        ensure!(n >= 4 && b.len() >= i + 8 + n, "bad updates chunk length");
        out.push(b[i + 12..i + 8 + n].to_vec());
        i += 8 + n;
    }
    Ok(out)
}

async fn period_update(http: &reqwest::Client, beacon: &str, period: u64) -> Result<Vec<u8>> {
    let (body, _) = get_ssz(http, &format!("{beacon}/eth/v1/beacon/light_client/updates?start_period={period}&count=1")).await?;
    split_updates(&body)?.into_iter().next().ok_or_else(|| anyhow!("{beacon} served no update for period {period}"))
}

// ---------------------------------------------------------------------------------------------
// SSZ material

fn bootstrap_committee(b: &[u8]) -> Result<Vec<u8>> {
    let b = Bootstrap::<S>::from_ssz_bytes(b).map_err(|e| anyhow!("bootstrap does not decode: {e:?}"))?;
    Ok(b.current_sync_committee().as_ssz_bytes())
}

fn update_info(update: &LcUpdate) -> Result<GenericUpdate<S>> {
    Ok(match update {
        LcUpdate::Finality(b) => GenericUpdate::from(
            &FinalityUpdate::<S>::from_ssz_bytes(b).map_err(|e| anyhow!("finality update does not decode: {e:?}"))?,
        ),
        LcUpdate::Period(b) => {
            GenericUpdate::from(&Update::<S>::from_ssz_bytes(b).map_err(|e| anyhow!("update does not decode: {e:?}"))?)
        }
    })
}

fn describe_update(update: &LcUpdate) -> Result<String> {
    let g = update_info(update)?;
    let bits = g.sync_aggregate.sync_committee_bits.iter().filter(|b| *b).count();
    Ok(format!(
        "{} update: signature slot {} (period {}), attested slot {}, finalized slot {}, participation {bits}/512{}",
        match update {
            LcUpdate::Finality(_) => "finality",
            LcUpdate::Period(_) => "period",
        },
        g.signature_slot,
        g.signature_slot / SLOTS_PER_PERIOD,
        g.attested_header.beacon().slot,
        g.finalized_header.as_ref().map_or_else(|| "-".to_owned(), |h| h.beacon().slot.to_string()),
        if g.next_sync_committee.is_some() { ", carries the next committee" } else { "" },
    ))
}

/// Committee SSZ by commitment: the cache, else the bootstrap at the instance's checkpoint, else
/// the period updates around the store's period (each carries the next committee).
async fn committee(
    http: &reqwest::Client,
    beacon: &str,
    cache: &Path,
    config: &InstanceConfig,
    store: &LcSummary,
    want: [u8; 32],
) -> Result<Vec<u8>> {
    let file = cache.join(format!("{}.ssz", to_hex(&want)));
    if let Ok(b) = std::fs::read(&file)
        && committee_hash(&b) == want
    {
        return Ok(b);
    }
    let period = store.finalized.beacon.slot / SLOTS_PER_PERIOD;
    let mut tried = Vec::new();
    match get_ssz(http, &format!("{beacon}/eth/v1/beacon/light_client/bootstrap/0x{}", to_hex(&config.checkpoint))).await {
        Ok((b, _)) => {
            let c = bootstrap_committee(&b)?;
            if committee_hash(&c) == want {
                save_committee(cache, &c)?;
                return Ok(c);
            }
            tried.push("the checkpoint's bootstrap".to_owned());
        }
        Err(e) => tried.push(format!("the checkpoint's bootstrap ({e})")),
    }
    for p in [period.saturating_sub(1), period] {
        match period_update(http, beacon, p).await {
            Ok(u) => {
                if let Some(c) = update_info(&LcUpdate::Period(u))?.next_sync_committee {
                    let c = c.as_ssz_bytes();
                    if committee_hash(&c) == want {
                        save_committee(cache, &c)?;
                        return Ok(c);
                    }
                }
                tried.push(format!("the period-{p} update"));
            }
            Err(e) => tried.push(format!("the period-{p} update ({e})")),
        }
    }
    bail!("no committee with commitment 0x{} found (tried {})", to_hex(&want), tried.join(", "))
}

fn save_committee(cache: &Path, c: &[u8]) -> Result<()> {
    std::fs::create_dir_all(cache)?;
    std::fs::write(cache.join(format!("{}.ssz", to_hex(&committee_hash(c)))), c)?;
    Ok(())
}

fn default_cache() -> Result<PathBuf> {
    let storage = wallet::utils::fetch_persistent_storage_path()?;
    Ok(storage.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf).join("attest-committees"))
}

// ---------------------------------------------------------------------------------------------
// Reading the chain

async fn read_instance(wallet: &WalletCore<SequencerClient>, program: AccountId, id: [u8; 32]) -> Result<(InstanceConfig, LcSummary)> {
    let c = public_state(wallet, config_address(&program, &id), program).await?;
    ensure!(!c.is_empty(), "no instance 0x{} under program {program}", to_hex(&id));
    let config: InstanceConfig = borsh::from_slice(&c).context("config state")?;
    ensure!(instance_id(&program, &config) == id, "the config at the instance's address hashes to another id");
    let s = public_state(wallet, store_address(&program, &id), program).await?;
    let store: LcSummary = borsh::from_slice(&s).context("store state")?;
    Ok((config, store))
}

async fn anchor_record(wallet: &WalletCore<SequencerClient>, program: AccountId, id: &[u8; 32], anchor: &Anchor) -> Result<Option<AnchorRecord>> {
    let s = public_state(wallet, anchor_address(&program, id, anchor), program).await?;
    if s.is_empty() {
        return Ok(None);
    }
    Ok(Some(borsh::from_slice(&s).context("anchor state")?))
}

fn anchor_text(a: &Anchor) -> String {
    match a {
        Anchor::BlockHash(h) => format!("block hash 0x{}", to_hex(h)),
        Anchor::StateRoot(r) => format!("state root 0x{}", to_hex(r)),
    }
}

fn print_summary(prefix: &str, s: &LcSummary) {
    let b = &s.finalized.beacon;
    println!("{prefix}instance 0x{}  chain {}  updates {}", to_hex(&s.instance_id), s.evm_chain_id, s.updates);
    println!(
        "{prefix}finalized beacon slot {} (period {}, epoch {})  root 0x{}",
        b.slot,
        b.slot / SLOTS_PER_PERIOD,
        b.slot / 32,
        to_hex(&s.finalized.beacon_root)
    );
    println!("{prefix}  proposer {}  parent 0x{}  state 0x{}  body 0x{}", b.proposer_index, to_hex(&b.parent_root), to_hex(&b.state_root), to_hex(&b.body_root));
    let e = &s.finalized.execution;
    println!(
        "{prefix}execution block hash 0x{}  state root {}  number {}",
        to_hex(&e.block_hash),
        e.state_root.map_or_else(|| "- (Gloas header: block hash only)".to_owned(), |r| format!("0x{}", to_hex(&r))),
        e.block_number.map_or_else(|| "-".to_owned(), |n| n.to_string())
    );
    println!(
        "{prefix}current committee 0x{}  next committee {}",
        to_hex(&s.current_committee),
        s.next_committee.map_or_else(|| "unknown".to_owned(), |h| format!("0x{}", to_hex(&h)))
    );
}

// ---------------------------------------------------------------------------------------------
// Commands

pub async fn run(wallet: &mut WalletCore<SequencerClient>, sequencer: &str, command: AttestCommand) -> Result<()> {
    match command {
        AttestCommand::Id { program, network: name, checkpoint, min_participants, salt } => {
            let config = base_config(network(&name), hex32(&checkpoint)?, min_participants, hex32(&salt)?)?;
            print_ids(&program, &config, None);
            Ok(())
        }
        AttestCommand::Init {
            program,
            from,
            network: name,
            checkpoint,
            beacon,
            cross_check,
            bootstrap_file,
            min_participants,
            salt,
            save_dir,
        } => {
            let net = network(&name);
            let beacon = beacon.unwrap_or_else(|| net.beacon.to_owned());
            let http = http()?;
            let mut config = base_config(net, [0; 32], min_participants, hex32(&salt)?)?;
            check_network(&http, &beacon, &config).await?;
            let (root, slot) = if checkpoint == "finalized" {
                beacon_header(&http, &beacon, "finalized").await?
            } else {
                let root = hex32(&checkpoint)?;
                let (r, slot) = beacon_header(&http, &beacon, &format!("0x{}", to_hex(&root))).await?;
                ensure!(r == root, "{beacon} returns header 0x{} for 0x{}", to_hex(&r), to_hex(&root));
                (root, slot)
            };
            println!("checkpoint 0x{} at slot {slot} (period {}, epoch {}), from {beacon}", to_hex(&root), slot / SLOTS_PER_PERIOD, slot / 32);
            for other in &cross_check {
                let (r, s) = beacon_header(&http, other, &format!("0x{}", to_hex(&root))).await?;
                ensure!(r == root && s == slot, "{other} disagrees: 0x{} at slot {s}", to_hex(&r));
                println!("  cross-checked with {other}");
            }
            config.checkpoint = root;
            let bootstrap = match bootstrap_file {
                Some(path) => std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                None => {
                    let (b, fork) = get_ssz(&http, &format!("{beacon}/eth/v1/beacon/light_client/bootstrap/0x{}", to_hex(&root))).await?;
                    println!("bootstrap: {} bytes, fork {fork}, from {beacon}", b.len());
                    b
                }
            };
            let genesis = eth_attest::lc::verify_bootstrap(&program, &config, &bootstrap)
                .map_err(|e| anyhow!("the bootstrap does not verify natively, nothing sent: {e}"))?;
            println!("bootstrap verified natively (Helios consensus-core, the program's own code):");
            print_summary("  ", &genesis);
            print_ids(&program, &config, Some(&genesis));
            let committee = bootstrap_committee(&bootstrap)?;
            save_committee(&default_cache()?, &committee)?;
            if let Some(dir) = &save_dir {
                std::fs::create_dir_all(dir)?;
                std::fs::write(dir.join("bootstrap.ssz"), &bootstrap)?;
                save_committee(dir, &committee)?;
                std::fs::write(dir.join("config.borsh.hex"), to_hex(&borsh::to_vec(&config)?))?;
            }
            let id = genesis.instance_id;
            if !public_state(wallet, config_address(&program, &id), program).await?.is_empty() {
                bail!("instance 0x{} already exists", to_hex(&id));
            }
            if identity(wallet, from).is_private() {
                bail!("init is public: give a public account of this wallet as --from");
            }
            let mut accounts = vec![
                signer(wallet, from, program),
                AccountIdentity::PublicNoSign(config_address(&program, &id)).select_program_actor_state(program),
                AccountIdentity::PublicNoSign(store_address(&program, &id)).select_program_actor_state(program),
            ];
            for a in genesis.finalized.anchors() {
                accounts.push(AccountIdentity::PublicNoSign(anchor_address(&program, &id, &a)).select_program_actor_state(program));
            }
            let message = borsh::to_vec(&AttestMessage::Instruction(AttestInstruction::Init { config, bootstrap }))?;
            println!("init message {} bytes; public transaction from {from}", message.len());
            let fees = FeeProbe::before(sequencer, from).await?;
            let Some((tx, block)) = send_tx(wallet, &[program], accounts, &message, None).await? else {
                return Ok(());
            };
            fees.report(&tx, block).await?;
            let (_, store) = read_instance(wallet, program, id).await?;
            println!("on chain:");
            print_summary("  ", &store);
            println!("INSTANCE=0x{}", to_hex(&id));
            Ok(())
        }
        AttestCommand::Update { program, instance, from, beacon, kind, update_file, load_dir, save_dir, committees } => {
            let id = hex32(&instance)?;
            let (config, prev) = read_instance(wallet, program, id).await?;
            let beacon = beacon
                .or_else(|| network_by_chain(config.evm_chain_id).map(|n| n.beacon.to_owned()))
                .ok_or_else(|| anyhow!("give --beacon for chain {}", config.evm_chain_id))?;
            println!("store now:");
            print_summary("  ", &prev);
            let http = http()?;
            let cache = match committees {
                Some(dir) => dir,
                None => default_cache()?,
            };
            let witness_dir = load_dir.clone().unwrap_or_else(|| cache.clone());
            let current = committee(&http, &beacon, &witness_dir, &config, &prev, prev.current_committee).await?;
            let next = match prev.next_committee {
                Some(h) => Some(committee(&http, &beacon, &witness_dir, &config, &prev, h).await?),
                None => None,
            };
            println!(
                "committee witness: current 0x{} ({} bytes){}",
                to_hex(&prev.current_committee),
                current.len(),
                next.as_ref().map_or_else(String::new, |n| format!(", next ({} bytes)", n.len()))
            );
            let update = match (&load_dir, update_file) {
                (Some(dir), _) => {
                    let k = std::fs::read_to_string(dir.join("update.kind")).context("update.kind")?;
                    let b = std::fs::read(dir.join("update.ssz")).context("update.ssz")?;
                    println!("update from {} (saved earlier)", dir.display());
                    if k.trim() == "period" { LcUpdate::Period(b) } else { LcUpdate::Finality(b) }
                }
                (None, Some(path)) => {
                    let b = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                    match kind.as_str() {
                        "period" => LcUpdate::Period(b),
                        "finality" => LcUpdate::Finality(b),
                        _ => bail!("with --update-file give --kind finality or --kind period"),
                    }
                }
                (None, None) => fetch_update(&http, &beacon, &kind, &prev).await?,
            };
            println!("{}", describe_update(&update)?);
            let verified = eth_attest::lc::verify_update(&program, &config, &prev, &current, next.as_deref(), &update)
                .map_err(|e| anyhow!("the update does not verify natively, nothing proven or sent: {e}"))?;
            let new = verified.next.clone();
            println!("verified natively (Helios consensus-core, the program's own code). Public effects of this update:");
            let advance = borsh::to_vec(&eth_attest_core::Effect::Advance { prev: prev.clone(), next: new.clone() })?;
            println!(
                "  1. Advance -> store {} ({} bytes: the old and new summary, both public)",
                store_address(&program, &id),
                advance.len()
            );
            print_summary("       next: ", &new);
            let mut anchors = Vec::new();
            if new.finalized != prev.finalized {
                for (i, a) in new.finalized.anchors().into_iter().enumerate() {
                    let exists = anchor_record(wallet, program, &id, &a).await?.is_some();
                    println!(
                        "  {}. RecordAnchor {} -> anchor PDA {}{}",
                        i + 2,
                        anchor_text(&a),
                        anchor_address(&program, &id, &a),
                        if exists { " (already recorded: kept)" } else { " (new)" }
                    );
                    anchors.push(a);
                }
            } else {
                println!("  (the finalized header is unchanged: no anchor recorded; the next committee is learned)");
            }
            println!(
                "  event Advanced {{ beacon_slot {}, block_hash 0x{}, next_committee_known {} }}",
                new.finalized.beacon.slot,
                to_hex(&new.finalized.execution.block_hash),
                new.next_committee.is_some()
            );
            let now_ms = u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis())?;
            println!(
                "  valid from LEZ block time {} ms (start of the signature slot; now {now_ms}: {})",
                verified.not_before_ms,
                if now_ms >= verified.not_before_ms { "ok" } else { "NOT YET" }
            );
            println!(
                "  private (never published by LEZ): the update ({} bytes), the committee witness, {from}",
                match &update {
                    LcUpdate::Finality(b) | LcUpdate::Period(b) => b.len(),
                }
            );
            println!("NEXT_SLOT={}", new.finalized.beacon.slot);
            println!("NEXT_BLOCK_HASH=0x{}", to_hex(&new.finalized.execution.block_hash));
            if let Some(dir) = &save_dir {
                std::fs::create_dir_all(dir)?;
                let (k, b) = match &update {
                    LcUpdate::Finality(b) => ("finality", b),
                    LcUpdate::Period(b) => ("period", b),
                };
                std::fs::write(dir.join("update.kind"), k)?;
                std::fs::write(dir.join("update.ssz"), b)?;
                save_committee(dir, &current)?;
                if let Some(n) = &next {
                    save_committee(dir, n)?;
                }
                std::fs::write(dir.join("prev.borsh.hex"), to_hex(&borsh::to_vec(&prev)?))?;
                std::fs::write(dir.join("next.borsh.hex"), to_hex(&borsh::to_vec(&new)?))?;
                println!("saved the update, its kind and the committee witness in {}", dir.display());
            }
            if !identity(wallet, from).is_private() {
                bail!(
                    "{from} is not a private account of this wallet. An update costs 37-46M user cycles, over \
                     LEZ's 10M cap per public transaction and per block, so it must run in a private account's \
                     proved execution (`lez-bridge wallet account new private`, fund it with `shield`)"
                );
            }
            let mut accounts = vec![
                signer(wallet, from, program),
                AccountIdentity::PublicNoSign(store_address(&program, &id)).select_program_actor_state(program),
            ];
            for a in &anchors {
                accounts.push(AccountIdentity::PublicNoSign(anchor_address(&program, &id, a)).select_program_actor_state(program));
            }
            let message = borsh::to_vec(&AttestMessage::Instruction(AttestInstruction::Update {
                config,
                prev,
                current_committee: current,
                next_committee: next,
                update,
            }))?;
            println!("update message {} bytes (private witness)", message.len());
            if send_tx(wallet, &[program], accounts, &message, Some(from)).await?.is_some() {
                let (_, store) = read_instance(wallet, program, id).await?;
                ensure!(store == new, "the store on chain is not the summary verified here");
                println!("store on chain now:");
                print_summary("  ", &store);
            }
            Ok(())
        }
        AttestCommand::Show { program, instance, anchor_block_hash, anchor_state_root } => {
            let id = hex32(&instance)?;
            let (config, store) = read_instance(wallet, program, id).await?;
            let net = network_by_chain(config.evm_chain_id).map_or("unknown network", |n| n.name);
            println!("program {program}");
            println!("config {} (write-once)", config_address(&program, &id));
            println!("  chain {} ({net})  genesis validators root 0x{}", config.evm_chain_id, to_hex(&config.genesis_validators_root));
            println!("  genesis time {}  seconds per slot {}", config.genesis_time, config.seconds_per_slot);
            let names = ["genesis", "altair", "bellatrix", "capella", "deneb", "electra", "fulu", "gloas"];
            for (n, f) in names.iter().zip(config.forks.iter()) {
                let epoch = if f.epoch == u64::MAX { "never".to_owned() } else { f.epoch.to_string() };
                println!("  fork {n:<9} 0x{} at epoch {epoch}", to_hex(&f.version));
            }
            println!("  checkpoint 0x{}  min participants {}  salt 0x{}", to_hex(&config.checkpoint), config.min_participants, to_hex(&config.salt));
            println!("store {}", store_address(&program, &id));
            print_summary("  ", &store);
            let anchor_json = |a: &Anchor, recorded: bool| {
                json!({
                    "kind": match a { Anchor::BlockHash(_) => "block_hash", Anchor::StateRoot(_) => "state_root" },
                    "value": format!("0x{}", to_hex(a.value())), "recorded": recorded,
                })
            };
            let mut doc = json!({
                "program": program.to_string(), "instance": format!("0x{}", to_hex(&store.instance_id)), "chain_id": config.evm_chain_id,
                "network": network_by_chain(config.evm_chain_id).map(|n| n.name), "updates": store.updates,
                "min_participants": config.min_participants, "finalized_slot": store.finalized.beacon.slot,
                "finalized_root": format!("0x{}", to_hex(&store.finalized.beacon_root)),
                "execution_block_number": store.finalized.execution.block_number, "latest_anchor": null, "asked_anchor": null,
            });
            for a in store.finalized.anchors() {
                let rec = anchor_record(wallet, program, &id, &a).await?;
                println!("latest anchor: {} at {} {}", anchor_text(&a), anchor_address(&program, &id, &a), if rec.is_some() { "(recorded)" } else { "(MISSING)" });
                let mut v = anchor_json(&a, rec.is_some());
                v["block_number"] = json!(rec.as_ref().and_then(|r| r.block_number));
                doc["latest_anchor"] = v;
            }
            let asked = match (anchor_block_hash, anchor_state_root) {
                (Some(h), _) => Some(Anchor::BlockHash(hex32(&h)?)),
                (None, Some(r)) => Some(Anchor::StateRoot(hex32(&r)?)),
                (None, None) => None,
            };
            if let Some(a) = asked {
                let rec = anchor_record(wallet, program, &id, &a).await?;
                let mut v = anchor_json(&a, rec.is_some());
                match rec {
                    Some(rec) => {
                        println!(
                            "anchor {}: RECORDED by beacon slot {} (root 0x{}), block number {}",
                            anchor_text(&a),
                            rec.beacon_slot,
                            to_hex(&rec.beacon_root),
                            rec.block_number.map_or_else(|| "-".to_owned(), |n| n.to_string())
                        );
                        v["beacon_slot"] = json!(rec.beacon_slot);
                        v["block_number"] = json!(rec.block_number);
                    }
                    None => println!("anchor {}: NOT recorded by this instance", anchor_text(&a)),
                }
                doc["asked_anchor"] = v;
            }
            super::out::doc(&doc);
            Ok(())
        }
        AttestCommand::ProveSlot {
            probe,
            program,
            instance,
            from,
            eth_rpc: rpc,
            contract,
            slot,
            mapping_slot,
            key,
            eq,
            ne,
            anchor_block_hash,
            save_dir,
            load_dir,
            fetch_only,
        } => {
            let id = hex32(&instance)?;
            let (config, store) = read_instance(wallet, program, id).await?;
            let address = eth(&contract)?;
            let slot = match (slot, mapping_slot, key) {
                (Some(s), _, _) => hex32(&s)?,
                (None, Some(index), Some(k)) => eth_attest_core::inclusion::mapping_slot(&hex32(&k)?, index),
                _ => bail!("give --slot, or --mapping-slot with --key"),
            };
            // The anchor: given, else the store's latest (a state root when the header has one:
            // no header witness needed).
            let anchor = match &anchor_block_hash {
                Some(h) => Anchor::BlockHash(hex32(h)?),
                None => *store.finalized.anchors().last().expect("a block hash at least"),
            };
            println!("anchor {} (instance's latest: {})", anchor_text(&anchor), anchor_block_hash.is_none());
            let (witness, proof_json) = match &load_dir {
                Some(dir) => {
                    let p: Value = serde_json::from_slice(&std::fs::read(dir.join("eth_getProof.json")).context("eth_getProof.json")?)?;
                    let header = match std::fs::read_to_string(dir.join("header.rlp.hex")) {
                        Ok(t) => Some(hex_bytes(&t)?),
                        Err(_) => None,
                    };
                    println!("header and proof from {} (fetched earlier)", dir.display());
                    (AnchorWitness { anchor, header_rlp: header }, p)
                }
                None => {
                    let rpc = rpc.ok_or_else(|| anyhow!("give --eth-rpc (or --load-dir)"))?;
                    let http = http()?;
                    let (header_rlp, number) = match anchor {
                        Anchor::BlockHash(hash) => {
                            let (rlp, number) = header_for(&http, &rpc, &hash).await?;
                            (Some(rlp), number)
                        }
                        Anchor::StateRoot(_) => (None, store.finalized.execution.block_number.ok_or_else(|| anyhow!("no block number for the state-root anchor"))?),
                    };
                    let p = eth_rpc(&http, &rpc, "eth_getProof", json!([format!("0x{}", to_hex(&address)), [format!("0x{}", to_hex(&slot))], format!("0x{number:x}")]))
                        .await
                        .context("eth_getProof (public nodes serve only about the last 128 blocks)")?;
                    println!("eth_getProof at block {number} from {rpc}");
                    if let Some(dir) = &save_dir {
                        std::fs::create_dir_all(dir)?;
                        std::fs::write(dir.join("eth_getProof.json"), serde_json::to_vec_pretty(&p)?)?;
                        if let Some(h) = &header_rlp {
                            std::fs::write(dir.join("header.rlp.hex"), format!("0x{}", to_hex(h)))?;
                        }
                        std::fs::write(dir.join("block_number"), number.to_string())?;
                        println!("saved the header and proof in {}", dir.display());
                    }
                    (AnchorWitness { anchor, header_rlp }, p)
                }
            };
            let proof = storage_proof_json(&proof_json, &address, &slot)?;
            let predicate = match (eq, ne) {
                (Some(v), _) => Predicate::Eq(word_arg(&v)?),
                (None, Some(v)) => Predicate::Ne(word_arg(&v)?),
                (None, None) => Predicate::Eq(proof.value),
            };
            let ctx = StatementContext { evm_chain_id: config.evm_chain_id, instance_id: id };
            let statement = verify_storage_slot(&ctx, &witness, &address, &slot, predicate, &proof)
                .map_err(|e| anyhow!("the statement does not verify natively, nothing sent: {e}"))?;
            println!(
                "statement verified natively: contract 0x{} slot 0x{} value 0x{} ({} account + {} storage nodes)",
                to_hex(&address),
                to_hex(&slot),
                to_hex(&proof.value),
                proof.account.nodes.len(),
                proof.nodes.len()
            );
            println!("STATEMENT_ID=0x{}", to_hex(&statement.id()));
            if fetch_only {
                return Ok(());
            }
            match anchor_record(wallet, program, &id, &anchor).await? {
                Some(rec) => println!("anchor recorded by the instance at beacon slot {}", rec.beacon_slot),
                None => bail!("the instance has not finalized {} (yet): nothing sent", anchor_text(&anchor)),
            }
            let accounts = vec![
                signer(wallet, from, probe),
                AccountIdentity::PublicNoSign(anchor_address(&program, &id, &anchor)).select_program_actor_state(program),
            ];
            let message = borsh::to_vec(&Probe::Storage { attest: program, ctx, witness, address, slot, predicate, proof })?;
            println!("probe message {} bytes; public transaction from {from}", message.len());
            let fees = FeeProbe::before(sequencer, from).await?;
            if let Some((tx, block)) = send_tx(wallet, &[probe, program], accounts, &message, None).await? {
                fees.report(&tx, block).await?;
                let stored = public_state(wallet, from, probe).await?;
                ensure!(stored == statement.id().to_vec(), "the probe's actor holds another statement id");
                println!("the probe actor ({from}, {probe}) holds statement id 0x{}", to_hex(&stored));
            }
            Ok(())
        }
    }
}

fn print_ids(program: &AccountId, config: &InstanceConfig, genesis: Option<&LcSummary>) {
    let id = instance_id(program, config);
    println!("instance id 0x{}", to_hex(&id));
    println!("  config PDA {}", config_address(program, &id));
    println!("  store PDA  {}", store_address(program, &id));
    if let Some(g) = genesis {
        for a in g.finalized.anchors() {
            println!("  anchor PDA {} for {}", anchor_address(program, &id, &a), anchor_text(&a));
        }
    }
}

fn word_arg(text: &str) -> Result<[u8; 32]> {
    let t = text.trim().trim_start_matches("0x");
    ensure!(t.len() <= 64, "more than 32 bytes: {text}");
    hex32(&format!("{t:0>64}"))
}

/// Picks the update the store needs: a finality update signed in the store's period (or the next
/// one, once the next committee is known); otherwise the store's own period update first, which
/// proves the next committee.
async fn fetch_update(http: &reqwest::Client, beacon: &str, kind: &str, prev: &LcSummary) -> Result<LcUpdate> {
    let store_period = prev.finalized.beacon.slot / SLOTS_PER_PERIOD;
    if kind != "period" {
        let (b, fork) = get_ssz(http, &format!("{beacon}/eth/v1/beacon/light_client/finality_update")).await?;
        let update = LcUpdate::Finality(b);
        let g = update_info(&update)?;
        let sig_period = g.signature_slot / SLOTS_PER_PERIOD;
        let fin = g.finalized_header.as_ref().map_or(0, |h| h.beacon().slot);
        println!("finality update from {beacon} ({fork}): signature period {sig_period}, finalized slot {fin}; store period {store_period}");
        let usable = sig_period == store_period || (sig_period == store_period + 1 && prev.next_committee.is_some());
        if kind == "finality" || usable {
            ensure!(
                fin > prev.finalized.beacon.slot,
                "the beacon node's finality update finalizes slot {fin}, not after the store's {}: wait for the next \
                 epoch (6.4 min) and retry",
                prev.finalized.beacon.slot
            );
            return Ok(update);
        }
        println!("signed in period {sig_period}: the store (period {store_period}) needs the period-{store_period} update first");
    }
    let u = period_update(http, beacon, store_period).await?;
    Ok(LcUpdate::Period(u))
}

/// An execution header's RLP for `hash`: `debug_getRawHeader` if served, else rebuilt from
/// `eth_getBlockByHash`; either way it must hash to `hash`.
async fn header_for(http: &reqwest::Client, rpc: &str, hash: &[u8; 32]) -> Result<(Vec<u8>, u64)> {
    let b = eth_rpc(http, rpc, "eth_getBlockByHash", json!([format!("0x{}", to_hex(hash)), false])).await?;
    ensure!(!b.is_null(), "{rpc} does not know block 0x{}", to_hex(hash));
    let number = u64::from_str_radix(b["number"].as_str().unwrap_or_default().trim_start_matches("0x"), 16).context("block number")?;
    if let Ok(raw) = eth_rpc(http, rpc, "debug_getRawHeader", json!([format!("0x{number:x}")])).await
        && let Some(text) = raw.as_str()
        && let Ok(rlp) = hex_bytes(text)
        && keccak(&rlp) == *hash
    {
        println!("header RLP from debug_getRawHeader ({} bytes), hashes to the anchor", rlp.len());
        return Ok((rlp, number));
    }
    let rlp = rebuild_header(&b)?;
    ensure!(
        keccak(&rlp) == *hash,
        "the header rebuilt from eth_getBlockByHash hashes to 0x{}, not 0x{} (a field this client does not know?)",
        to_hex(&keccak(&rlp)),
        to_hex(hash)
    );
    println!("header RLP rebuilt from eth_getBlockByHash ({} bytes), hashes to the anchor", rlp.len());
    Ok((rlp, number))
}

fn rlp_bytes(x: &[u8]) -> Vec<u8> {
    if x.len() == 1 && x[0] < 0x80 {
        return x.to_vec();
    }
    let mut out = rlp_len(x.len(), 0x80);
    out.extend_from_slice(x);
    out
}

fn rlp_len(len: usize, offset: u8) -> Vec<u8> {
    if len <= 55 {
        return vec![offset + u8::try_from(len).expect("<= 55")];
    }
    let be = len.to_be_bytes();
    let skip = be.iter().take_while(|b| **b == 0).count();
    let mut out = vec![offset + 55 + u8::try_from(be.len() - skip).expect("small")];
    out.extend_from_slice(&be[skip..]);
    out
}

/// RLP of an execution header from `eth_getBlockByHash` JSON, the field order the attest-spike
/// verified on Sepolia (through Amsterdam: ..., parentBeaconBlockRoot, requestsHash,
/// blockAccessListHash, slotNumber). A field absent from the JSON is absent from the header.
fn rebuild_header(b: &Value) -> Result<Vec<u8>> {
    let data = |k: &str| -> Result<Vec<u8>> { Ok(rlp_bytes(&hex_bytes(b[k].as_str().ok_or_else(|| anyhow!("header has no {k}"))?)?)) };
    let int = |k: &str| -> Result<Vec<u8>> {
        let t = b[k].as_str().ok_or_else(|| anyhow!("header has no {k}"))?.trim_start_matches("0x");
        let t = t.trim_start_matches('0');
        let padded = if t.len() % 2 == 1 { format!("0{t}") } else { t.to_owned() };
        Ok(rlp_bytes(&hex_bytes(&padded)?))
    };
    let mut fields = Vec::new();
    for k in ["parentHash", "sha3Uncles", "miner", "stateRoot", "transactionsRoot", "receiptsRoot", "logsBloom"] {
        fields.push(data(k)?);
    }
    for k in ["difficulty", "number", "gasLimit", "gasUsed", "timestamp"] {
        fields.push(int(k)?);
    }
    for k in ["extraData", "mixHash", "nonce"] {
        fields.push(data(k)?);
    }
    fields.push(int("baseFeePerGas")?);
    for (k, is_int) in [
        ("withdrawalsRoot", false),
        ("blobGasUsed", true),
        ("excessBlobGas", true),
        ("parentBeaconBlockRoot", false),
        ("requestsHash", false),
        ("blockAccessListHash", false),
        ("slotNumber", true),
    ] {
        if b.get(k).is_some_and(|v| !v.is_null()) {
            fields.push(if is_int { int(k)? } else { data(k)? });
        }
    }
    let payload: Vec<u8> = fields.concat();
    let mut out = rlp_len(payload.len(), 0xc0);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// The fee a public transaction was charged: the payer's native balance across it (blocks carry
/// no receipts on 0.4), with the head fee quote around it to turn the fee back into gas.
struct FeeProbe {
    client: SequencerClient,
    payer: AccountId,
    balance: u128,
    quote: sequencer_service_rpc::FeeStateQuote,
}

impl FeeProbe {
    async fn before(sequencer: &str, payer: AccountId) -> Result<Self> {
        use sequencer_service_rpc::RpcClient as _;
        let client = sequencer_service_rpc::SequencerClientBuilder::default().build(sequencer)?;
        let balance = client.get_account_balance(payer).await.map_err(|e| anyhow!("getAccountBalance: {e}"))?;
        let quote = client.get_fee_state().await.map_err(|e| anyhow!("getFeeState: {e}"))?;
        Ok(Self { client, payer, balance, quote })
    }

    async fn report(&self, tx: &LeeTransaction, block: u64) -> Result<()> {
        use sequencer_service_rpc::RpcClient as _;
        let after = self.client.get_account_balance(self.payer).await.map_err(|e| anyhow!("getAccountBalance: {e}"))?;
        let quote = self.client.get_fee_state().await.map_err(|e| anyhow!("getFeeState: {e}"))?;
        let fee = self.balance.saturating_sub(after);
        let bytes = u128::try_from(borsh::to_vec(tx)?.len())?;
        let (exec, stor) = (u128::from(self.quote.base_fee_exec), u128::from(self.quote.base_fee_stor));
        let gas = fee.saturating_sub(bytes * stor) / exec.max(1);
        println!(
            "charged: fee {fee} (payer balance {} -> {after}); wire bytes {bytes}; base fees exec {} / stor {} at height {} \
             (after: {} / {} at {}); so execution gas = (fee - bytes x stor) / exec = {gas} in block {block}",
            self.balance,
            self.quote.base_fee_exec,
            self.quote.base_fee_stor,
            self.quote.height,
            quote.base_fee_exec,
            quote.base_fee_stor,
            quote.height,
        );
        if (self.quote.base_fee_exec, self.quote.base_fee_stor) != (quote.base_fee_exec, quote.base_fee_stor) {
            println!(
                "  (a block charges at its opening fee state, the quote before; the head moved after it. If another \
                 block came in between, the figure is approximate; the dry run's metered cycles are exact)"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rebuilt header of the real Sepolia fixture block hashes to its block hash.
    #[test]
    fn rebuilds_the_fixture_header() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../attest/fixtures/sepolia");
        let b: Value = serde_json::from_slice(&std::fs::read(format!("{dir}/el_block_header.json")).unwrap()).unwrap();
        let rlp = rebuild_header(&b).unwrap();
        let want = hex_bytes(std::fs::read_to_string(format!("{dir}/el_block_header.rlp.hex")).unwrap().trim()).unwrap();
        assert_eq!(rlp, want);
        assert_eq!(keccak(&rlp), hex32(b["hash"].as_str().unwrap()).unwrap());
    }

    #[test]
    fn presets_build_valid_configs_and_ids_are_stable() {
        for net in &NETWORKS {
            let c = base_config(net, [7; 32], MIN_SUPERMAJORITY, [0; 32]).unwrap();
            c.validate();
            assert_eq!(instance_id(&AccountId::new([1; 32]), &c), instance_id(&AccountId::new([1; 32]), &c.clone()));
        }
        assert!(base_config(&NETWORKS[0], [0; 32], 341, [0; 32]).is_err());
    }

    /// The preset matches the fixture's fork schedule (first 8 entries) and genesis.
    #[test]
    fn sepolia_preset_matches_the_fixture() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../attest/fixtures/sepolia");
        let meta: Value = serde_json::from_slice(&std::fs::read(format!("{dir}/meta.json")).unwrap()).unwrap();
        let c = base_config(network("sepolia"), [0; 32], MIN_SUPERMAJORITY, [0; 32]).unwrap();
        assert_eq!(c.genesis_validators_root, hex32(meta["genesis_validators_root"].as_str().unwrap()).unwrap());
        assert_eq!(c.genesis_time, meta["genesis_time"].as_u64().unwrap());
        for (i, f) in meta["fork_schedule"].as_array().unwrap().iter().take(8).enumerate() {
            assert_eq!(c.forks[i].epoch, f["epoch"].as_str().unwrap().parse::<u64>().unwrap());
            assert_eq!(c.forks[i].version.to_vec(), hex_bytes(f["current_version"].as_str().unwrap()).unwrap());
        }
    }
}
