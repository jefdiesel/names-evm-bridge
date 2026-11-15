//! LEZ bridge core module (RFP-022 Usability #1): the private Ethereum <-> LEZ bridge for any
//! Logos app. The trait below is the module's contract; the builder derives the `.lidl` from it.
//!
//! Every method returns JSON text:
//!
//! ```json
//! { "success": true,  "value": <answer>, "error": null }
//! { "success": false, "value": null,     "error": "why, in words a person can act on" }
//! ```
//!
//! Every parameter is a `String` (amounts are decimal text: values reach 1e18 and more, which
//! JavaScript numbers can't hold). No method or parameter name is a C++ keyword (the code
//! generator does not escape them; a test checks).
//!
//! What runs where:
//! - This module reads and shapes: settings, the registry (the Ethereum vault and the LEZ
//!   bridge), deposit plans and requests, positions, privacy reports.
//! - **Names Wallet mode** (`config().wallet == "names"`, the default): the person's accounts
//!   and keys are in the Names Wallet (`wallet.rs`). Deposit and exit secrets come from its
//!   `app_secret` (asked by this module, so they are `lez_bridge`'s values); claims and burns are
//!   built by the CLI (`--emit-call`, nothing sent) and proposed to the wallet as private calls,
//!   which the person approves in the Names app and the wallet proves. This module never proves.
//! - **CLI wallet mode** (`cli`, developer setups): everything that touches a wallet or proves
//!   runs in the `lez-bridge` CLI, started by this module as a background job (`jobs.rs`)
//!   through one adapter (`cli.rs`, CLI-CONTRACT.md). Sends are dry runs in this build.
//!
//! Fixture mode (`LEZ_BRIDGE_FIXTURES=<dir>` or `set_fixtures`): every method but the settings
//! answers the reply recorded in `<dir>/<method>.json`, so the mini-app can be built and
//! rendered without a chain. The recorded replies come from this module against the real v2
//! demo (`modules/fixtures/`).

mod cli;
mod compare;
mod config;
mod eth;
mod jobs;
mod lez;
mod names_mode;
mod plan;
mod privacy;
mod qr;
mod receive;
mod relayed;
mod request;
mod store;
mod text;
mod util;
mod vault;
mod wallet;

use config::{Config, Deployment};
use eth::{EthSource, JsonRpc, LogosEthModule};
use jobs::{Jobs, Spec};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::collections::BTreeSet;
use store::{public_view, Store};
use util::{format_amount, hex0x, now_ms, units};

/// The calls this build answers beyond the original set (`config().features`).
/// `account_add`: asks the Names Wallet to add an account to this app's access when the wallet
/// lists `grant_add_account` (wallet::add_account).
pub const FEATURES: [&str; 14] = [
    "relayed_claim", "relayed_claim_check_with", "relayed_claim_build", "relayed_claim_submit", "account_hint",
    "receive", "deposit_code", "public_receive", "receive_status", "receives", "relayed_claim_check_deposit", "account_add", "build_id",
    "receive_request",
];

pub const MODULE_VERSION: &str = "0.1.0";

pub trait LezBridgeModule: Send + 'static {
    // ── Settings ────────────────────────────────────────────────────────

    /// The settings and what is ready: `{module_version, sequencer, eth_rpc, eth_proof_rpc,
    /// eth_source, deployment, cli, wallet_dir, proving, fixtures, ready: {cli, wallet, why}}`.
    fn config(&mut self) -> String;

    /// The Ethereum JSON-RPC used for chain `chain_id` (decimal text), e.g. `11155111`.
    fn set_eth_rpc(&mut self, chain_id: String, url: String) -> String;

    /// The Ethereum JSON-RPC that serves `eth_getProof` well behind the head (for claims).
    fn set_eth_proof_rpc(&mut self, chain_id: String, url: String) -> String;

    /// Where Ethereum reads come from: `rpc`, or `logos_module` (the Logos Ethereum module; not
    /// wired in this build, and says so).
    fn set_eth_source(&mut self, source: String) -> String;

    /// The LEZ sequencer (http/https). Loopback only while the CLI refuses anything else.
    fn set_sequencer(&mut self, url: String) -> String;

    /// The `lez-bridge` program and the wallet folder it uses (its `LEE_WALLET_HOME_DIR`).
    fn set_cli(&mut self, program: String, wallet_dir: String) -> String;

    /// The bridge deployment: a built-in name (`sepolia-demo-v2`), a path to a deployment JSON
    /// file, or the JSON itself.
    fn use_deployment(&mut self, source: String) -> String;

    /// Serve recorded replies from folder `dir` ("" turns fixture mode off).
    fn set_fixtures(&mut self, dir: String) -> String;

    // ── Reading the bridge ──────────────────────────────────────────────

    /// The registry on both sides: tokens with denominations, fees, the relay-fee menu, caps
    /// and current usage, cap weights, freeze state on Ethereum and LEZ, the release delay,
    /// the LEZ float and sponsor amount, and the attestation instance (latest anchor and age).
    fn registry(&mut self) -> String;

    /// A split of `amount` (human text, e.g. `1,250.5`) of `token` (symbol or address) into the
    /// vault's denominations, a staggered sending schedule, fees and a privacy report.
    fn plan_deposit(&mut self, token: String, amount: String) -> String;

    // ── Deposits and claims ─────────────────────────────────────────────

    /// A deposit request for one denomination (`denom_index`) to LEZ account `account`: the
    /// commitment, the vault call (and the ERC-20 approval), an EIP-681 link and its QR, and an
    /// export text. A fresh secret is kept on this computer only.
    fn make_deposit_request(&mut self, token: String, denom_index: String, account: String) -> String;

    /// The same for a PUBLIC LEZ account, which `make_deposit_request` refuses: claiming into a
    /// public account publishes the deposit's secret and links the two sides for good, so this
    /// needs `confirm_weaker` = "true". With the Names Wallet: a public account given to this app
    /// (`Public/<id>`), its secret from `app_secret` of that account; the reply adds `not_private`
    /// (the notice to show before the person deposits). Its claim is the relayed public claim.
    fn make_public_deposit_request(&mut self, token: String, denom_index: String, account: String, confirm_weaker: String) -> String;

    /// Adds a deposit made elsewhere: `{secret, account, token, denom_index}`. Until the CLI
    /// derives secrets from the seed, this is how a deposit from another install is claimed.
    fn import_deposit(&mut self, deposit_json: String) -> String;

    /// This computer's deposits, each with its stage and timeline (sent → finalized → anchor
    /// recorded → suggested claim time → claimed) and its anonymity set.
    fn deposits(&mut self) -> String;

    /// The claim of deposit `id`, checked by a dry run (nothing proven or sent): one-time
    /// recipient, sponsorship, fee, the local diagnosis if it would be refused, proving cost.
    fn claim_plan(&mut self, id: String) -> String;

    /// Starts the claim of deposit `id` as a job: `options_json` `{reveal_recipient,
    /// claim_early, confirm_weaker, dry_run}`. Any choice that weakens privacy needs
    /// `confirm_weaker: true`. Returns the job.
    fn claim(&mut self, id: String, options_json: String) -> String;

    // ── Jobs ────────────────────────────────────────────────────────────

    /// Every job, newest first.
    fn jobs(&mut self) -> String;
    /// One job: `{id, kind, state, phase, phases, elapsed_s, prove_estimate_s, peak_rss_mb,
    /// log_tail, result, error, can_pause, can_resume, can_cancel}`.
    fn job(&mut self, id: String) -> String;
    fn pause_job(&mut self, id: String) -> String;
    fn resume_job(&mut self, id: String) -> String;
    fn cancel_job(&mut self, id: String) -> String;

    // ── Holdings, redemption, exits ─────────────────────────────────────

    /// The wrapped tokens this wallet holds, per account (private first), from a cache the
    /// module refreshes in the background (`as_of`, `stale`, `refreshing`).
    fn holdings(&mut self) -> String;

    /// The wallet's accounts and their kind, to choose a claim target (private pre-selected).
    fn accounts(&mut self) -> String;

    /// A redemption plan: `amount` of `token` to Ethereum address `eth_to`, relay fee choice
    /// `relay_fee_index` ("0" none, "1".. the menu), split into burns, with warnings about
    /// the recipient and what the vault can release.
    fn plan_redeem(&mut self, token: String, amount: String, eth_to: String, relay_fee_index: String) -> String;

    /// Starts the burns of plan `plan_id` as a job (`options_json` as for `claim`).
    fn redeem(&mut self, plan_id: String, options_json: String) -> String;

    /// Burns this computer made, and what each waits for on Ethereum.
    fn exits(&mut self) -> String;

    /// Releasing an exit on Ethereum. Not possible yet: the outbound proof (G2) is not built.
    /// Answers that plainly, with what is missing.
    fn release(&mut self, exit_id: String, relayer_json: String) -> String;

    /// Finds this account's positions again (`gap_limit` deposits past the last one found).
    fn recover(&mut self, account: String, gap_limit: String) -> String;

    /// What is public and what each chosen override leaks: `choices_json` `{flow, <override>:
    /// true, anonymity_set}` (see privacy.rs). An empty choice is the private default.
    fn privacy_report(&mut self, choices_json: String) -> String;

    // ── Admin ───────────────────────────────────────────────────────────

    /// Roles on both sides, whether this wallet holds the LEZ admin or freezer key, the state an
    /// admin manages, and which operations this build offers.
    fn admin_status(&mut self) -> String;

    /// One admin operation: `{op, ...}` with op `freeze`, `unfreeze` (`mint`, `burn`, `token`),
    /// `set_sponsor` (`amount`), `fund_float` (`amount`, `from`), `register_token`
    /// (`eth_token`, `definition`, `decimals`, `denoms`), or an Ethereum-side op, which this
    /// build does not offer and says so.
    fn admin_action(&mut self, action_json: String) -> String;

    // ── The Names Wallet ────────────────────────────────────────────────

    /// Where the accounts are and what the Names Wallet has granted: `{mode, connected,
    /// accounts: [{account, kind}], token_source: {accepted, request}, needs}`. In `names` mode,
    /// once connected, this also proposes the bridged tokens as a token source (first run).
    fn wallet_status(&mut self) -> String;

    /// Asks the Names Wallet for access (`request_access`): the request to watch.
    fn wallet_connect(&mut self) -> String;

    /// How a Names Wallet request this module made stands (`request_status`, in general terms).
    fn wallet_request(&mut self, request_id: String) -> String;

    /// `names` (the Names Wallet) or `cli` (the lez-bridge CLI's own wallet folder).
    fn set_wallet_mode(&mut self, mode: String) -> String;

    // ── Claim a deposit into a public account (Names Wallet, relayed) ────
    // The wallet signs the claim; the bridge's relay sends it and pays the fee (relayed.rs).

    /// Checks a claim file the deposit page saved, given as its JSON text or as a file path
    /// (`file://…` too), and everything a relayed claim needs, reading only: the bridge on LEZ,
    /// the deposit's leaf on Sepolia (final), a recorded root, the nullifier unspent, the
    /// relay's payer and caps. Builds the call the wallet will sign. Nothing is proposed,
    /// signed or sent (the dry run). `{ok, id, deposit, network, relay, call, checks, wallet,
    /// privacy, wording, error: {code, message}}`; the claim key stays in memory under `id`.
    fn relayed_claim_check(&mut self, claim: String) -> String;

    /// `relayed_claim_check` with options (`options_json`): `{"wallet_hint": false}` skips the
    /// reply's `wallet` part (`{"skipped": true, "needs": null}`), so the check asks the Names
    /// Wallet nothing and never waits on it: for a caller that is the wallet (the Names app).
    /// `{}` is the same as `relayed_claim_check`. A separate method because a generated method
    /// takes a fixed number of arguments.
    fn relayed_claim_check_with(&mut self, claim: String, options_json: String) -> String;

    /// Proposes the checked claim `check_id` to the Names Wallet (`propose_relayed_call`), after
    /// its gates (connected, the account given to this app, the programs declared, LEZ 0.4):
    /// `{needs, request, message}` while the person has something to approve, else the job.
    /// Safe to call again after a timeout: a claim that has a running job gets that job back, and
    /// after a proposal that got no answer it answers `{needs: "uncertain", uncertain, message}`
    /// until `options_json` is `{"after_uncertain": true}` (the person looked in the Names app) or
    /// 11 minutes passed. A busy wallet is asked again (2, 4, 6, 8 s; 30 s at most).
    fn relayed_claim(&mut self, check_id: String, options_json: String) -> String;

    /// "Receive from Ethereum": where stored deposit `id` stands, for a 60 s poll: `{stage:
    /// requested|deposited|final|claimable|claimed, text, leaf, block, tx, confirmations,
    /// final_in_min, claimed_block, claim: relayed|private, reads: {sepolia, lez}}`. Claimable is
    /// a root the bridge recorded on LEZ (as the relayed check), claimed is the deposit's
    /// nullifier account; no wallet call. Sepolia logs are read again only when the vault's count
    /// moved.
    fn receive_status(&mut self, id: String) -> String;

    /// `relayed_claim_check` for stored deposit `id` (a public account's, made by the Names
    /// Wallet): its claim key from `app_secret` instead of a claim file. Same reply and `id`;
    /// `{needs: "secret", …}` while the person approves this app's values first. `options_json`
    /// as `relayed_claim_check_with` (`{"wallet_hint": false}`).
    fn relayed_claim_check_deposit(&mut self, id: String, options_json: String) -> String;

    /// This install's "Receive from Ethereum" deposits (made by the Names Wallet's secret, or
    /// found by `recover`), newest first: `{receives: [{id, symbol, denom, denom_text, decimals,
    /// amount_text, account, account_kind, commitment, created, key_index, claim, claimed, stage,
    /// text, leaf, block, tx, claimed_block, as_of}]}` with the stage `receive_status` last saw
    /// (`unknown` before its first call). No chain or wallet read.
    fn receives(&mut self) -> String;

    /// The deposit code of stored deposit `id` again (a receive made in an earlier session):
    /// `{id, request, link, request_url, request_qr, compare: {first, last, text, words,
    /// words_text}, commitment, symbol, denom, denom_text, amount_text, chain_id, vault, deployment,
    /// network, account, account_kind, claimed, stage, text, as_of}` with the stage `receive_status`
    /// last saw. Rebuilt from the stored deposit, the same bytes as the first reply; no wallet call,
    /// no secret, no chain read. Refused for an unknown id, or a deposit whose stored data can't
    /// rebuild its code (made by a build that kept no vault).
    fn receive_request(&mut self, id: String) -> String;

    /// For a host that signs the claim itself (the Names app, `modules/NAMES-EMBED.md` Part B): the
    /// call for checked claim `check_id`, exactly `propose_relayed_call`'s input: `{check, account,
    /// call: {program, message, accounts, fee: {payer, gas_limit, tip, max_fee}, privacy}, summary,
    /// network, deployment, bridge_program, token_program, watch, wording, privacy}`. Asks no
    /// wallet, sends nothing. The message carries the claim key (the claim reveals it).
    fn relayed_claim_build(&mut self, check_id: String) -> String;

    /// For that host: its half-signed `transaction` (hex) of checked claim `check_id`. Starts the
    /// claim job at the relay phase (or gives a job that says `needs: "sign_again"` its new
    /// signature); then `relayed_claim_status`. Returns the job.
    fn relayed_claim_submit(&mut self, check_id: String, transaction: String) -> String;

    /// Claim job `job_id`, moved on as far as it can go now: approved in the wallet, handed to
    /// the relay, in a block. `{id, state, phase, phases, note, tx, block, result, error}`.
    fn relayed_claim_status(&mut self, job_id: String) -> String;

    /// Every relayed claim job, newest first.
    fn relayed_claims(&mut self) -> String;

    /// Cancels claim job `job_id` while it waits for the person's approval.
    fn relayed_claim_cancel(&mut self, job_id: String) -> String;

    /// Forgets checked claim `check_id` (its claim key leaves memory).
    fn relayed_claim_forget(&mut self, check_id: String) -> String;

    /// Called once the module is wired to the host.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

// The builder injects the generated scaffold here: `install`, `RustModuleContext`, `context()`
// and `modules()`. Unit tests build without it. It is the builder's generated/provider_gen.rs with
// its instance lock taken by try_lock (build.rs, scaffold_patch.rs).
#[cfg(not(test))]
include!(concat!(env!("OUT_DIR"), "/provider_gen.rs"));

/// How many places the built scaffold takes its instance lock with try_lock (build.rs): 3 in a
/// module build, "none" in unit tests. Kept whole in the library, so a built file can be checked
/// with `grep -a lez-bridge-scaffold-try-lock=`; `config()` says it too.
pub const SCAFFOLD_LINE: &str = concat!("lez-bridge-scaffold-try-lock=", env!("LEZ_BRIDGE_SCAFFOLD_TRY_LOCK_SITES"));

/// The source commit this module was built from and when (build.rs: `build-id.txt`, written by
/// `modules/build-lgx.sh`): "unknown" only in unit tests; a module build refuses it. Kept whole
/// in the library so a built file can be checked with `grep -a lez-bridge-build-commit=`.
pub const BUILD_COMMIT_LINE: &str = concat!("lez-bridge-build-commit=", env!("LEZ_BRIDGE_BUILD_COMMIT"));
pub const BUILD_DATE: &str = env!("LEZ_BRIDGE_BUILD_DATE");

/// `config().build`: tells one module build from another (`modules/AGENTS.md`).
pub fn build_info() -> Value {
    json!({ "commit": BUILD_COMMIT_LINE.trim_start_matches("lez-bridge-build-commit="), "built": BUILD_DATE, "version": MODULE_VERSION })
}

/// What the generated dispatch answers when the module's one lock is held: a call that came in
/// through the nested event loop of a call waiting on another module (the names module). Before
/// any module code runs; so "busy" always means "not taken, ask again" (scaffold_patch.rs).
pub(crate) mod scaffold_busy {
    use serde_json::{json, Value};

    pub const BUSY: &str = "The LEZ Bridge is busy with another call; nothing was done. Ask again in a moment.";
    pub const STOPPED: &str = "The LEZ Bridge module stopped after an internal error; unload it and load it again (or restart Basecamp) to use it again.";

    /// `{success: false, busy: true, value: null, error}` as JSON text, as every answer is (every
    /// method of this module returns text).
    pub fn answer(_method: &str) -> Value {
        Value::String(json!({ "success": false, "busy": true, "value": null, "error": BUSY }).to_string())
    }

    /// The lock was left poisoned by a call that panicked: refused, never taken over.
    pub fn stopped(_method: &str) -> Value {
        Value::String(json!({ "success": false, "value": null, "error": STOPPED }).to_string())
    }
}

#[cfg(test)]
pub struct RustModuleContext;

fn reply(outcome: Result<Value, String>) -> String {
    match outcome {
        Ok(value) => json!({ "success": true, "value": value, "error": null }),
        Err(error) => json!({ "success": false, "value": null, "error": error }),
    }
    .to_string()
}

/// A shared LEZ endpoint (tests) as a boxed one.
struct SharedRpc(std::sync::Arc<dyn lez::LezRpc + Send + Sync>);

impl lez::LezRpc for SharedRpc {
    fn call(&self, method: &str, params: Value) -> Result<Value, lez::Problem> {
        self.0.call(method, params)
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
}

/// Seconds after which a cached registry read is read again.
const REGISTRY_FRESH_MS: u64 = 30_000;
const HOLDINGS_FRESH_MS: u64 = 60_000;
/// Data folders whose holdings are being read now (one read per folder at a time).
// Fully qualified: the generated provider code (included below) imports `Mutex` itself.
static HOLDINGS_REFRESHING: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());

fn refreshing(dir: &Path) -> bool {
    HOLDINGS_REFRESHING.lock().is_ok_and(|s| s.contains(dir))
}

/// Marks `dir` as being read; false if it already was.
fn start_refresh(dir: &Path) -> bool {
    HOLDINGS_REFRESHING.lock().is_ok_and(|mut s| s.insert(dir.to_path_buf()))
}

fn end_refresh(dir: &Path) {
    if let Ok(mut s) = HOLDINGS_REFRESHING.lock() {
        s.remove(dir);
    }
}

struct Bridge {
    data: PathBuf,
    cfg: Config,
    fixtures: Option<PathBuf>,
    registry_cache: Option<(u64, Value)>,
    /// Tests put a recorded Ethereum here.
    eth_override: Option<Box<dyn EthSource + Send>>,
    /// Checked claims (with their keys, in memory only) and signed transactions in flight.
    relayed: relayed::State,
}

impl Default for Bridge {
    fn default() -> Self {
        let data = config::data_dir();
        let cfg = Config::load(&data);
        let fixtures = std::env::var_os("LEZ_BRIDGE_FIXTURES")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| util::read_json(&data.join("fixtures.json")).and_then(|v| v["dir"].as_str().map(PathBuf::from)));
        let b = Bridge { data, cfg, fixtures, registry_cache: None, eth_override: None, relayed: Default::default() };
        b.runner().adopt();
        b
    }
}

impl Bridge {
    fn runner(&self) -> Jobs {
        Jobs { dir: jobs::jobs_dir(&self.data) }
    }

    /// Starts a job, unless the balances read (which syncs and saves the wallet) is running:
    /// two processes saving one wallet would undo each other.
    fn start_job(&self, spec: Spec) -> Result<Value, String> {
        if refreshing(&self.data) {
            return Err("the wallet is catching up with the chain to read your balances; try again in a few seconds".to_owned());
        }
        self.runner().start(spec)
    }

    fn store(&self) -> Store {
        Store::at(&self.data)
    }

    fn save(&self) -> Result<(), String> {
        self.cfg.save(&self.data)
    }

    fn eth(&self) -> Result<Box<dyn EthSource + '_>, String> {
        if let Some(e) = &self.eth_override {
            return Ok(Box::new(Borrowed(e.as_ref())));
        }
        if self.cfg.eth_source == "logos_module" {
            return Ok(Box::new(LogosEthModule));
        }
        let url = self.cfg.rpc_for(self.cfg.deployment.chain_id).ok_or_else(|| format!("no Ethereum RPC is set for chain {}; set one in Settings", self.cfg.deployment.chain_id))?;
        let fallbacks = if self.cfg.deployment.chain_id == 11_155_111 { config::SEPOLIA_FALLBACK_RPCS.iter().map(|u| (*u).to_owned()).collect() } else { Vec::new() };
        Ok(Box::new(JsonRpc::new(url).with_fallbacks(fallbacks)))
    }

    fn cli(&self) -> Result<cli::Cli, String> {
        let program = self
            .cfg
            .cli
            .clone()
            .or_else(|| std::env::var("LEZ_BRIDGE_CLI").ok())
            .ok_or("the lez-bridge program is not set; choose it in Settings")?;
        let wallet = self
            .cfg
            .wallet_dir
            .clone()
            .or_else(|| std::env::var("LEE_WALLET_HOME_DIR").ok())
            // Names Wallet mode: the CLI only reads the chain and builds calls; it gets a wallet
            // folder of its own that holds no funds (made on first use).
            .or_else(|| self.cfg.names_wallet().then(|| self.data.join("reader-wallet").display().to_string()))
            .ok_or("the wallet folder is not set; choose it in Settings")?;
        let tmp = self.cfg.tmp_dir.clone().map(PathBuf::from).unwrap_or_else(|| self.data.join("tmp"));
        let _ = util::private_dir(&tmp);
        Ok(cli::Cli { program: program.into(), wallet_dir: wallet.into(), sequencer: self.cfg.sequencer.clone(), tmp_dir: tmp })
    }

    fn wallet_cli(&self) -> Result<cli::Cli, String> {
        if !config::is_loopback(&self.cfg.sequencer) {
            return Err(format!(
                "the lez-bridge CLI only talks to a sequencer on this computer today, and {} is not one (CLI-CONTRACT.md §0.2)",
                self.cfg.sequencer
            ));
        }
        let c = self.cli()?;
        if self.cfg.names_wallet() && self.cfg.wallet_dir.is_none() && !c.wallet_dir.join("storage.json").exists() && cli::is_executable(&c.program) {
            // The reader wallet: `wallet check-health` makes it (its phrase is never shown; it
            // holds nothing).
            let _ = util::private_dir(&c.wallet_dir);
            let _ = cli::run(c.command(&["wallet".into(), "check-health".into()]), cli::SYNC_PATIENCE);
        }
        c.check()?;
        Ok(c)
    }

    /// Every job: the CLI's and, in Names Wallet mode, the wallet's.
    fn all_jobs(&self) -> Vec<Value> {
        let mut list = self.runner().list();
        if self.names_mode() {
            list.extend(self.wallet_jobs());
            list.sort_by_key(|j| std::cmp::Reverse(j["started"].as_u64().unwrap_or(0)));
        }
        list
    }

    /// A recorded reply, in fixture mode.
    fn fixture(&self, method: &str) -> Option<String> {
        let dir = self.fixtures.as_ref()?;
        let text = std::fs::read_to_string(dir.join(format!("{method}.json")))
            .unwrap_or_else(|_| reply(Err(format!("fixture mode: no recorded reply for {method} in {}", dir.display()))));
        let mut v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "success": false, "value": null, "error": "bad fixture" }));
        v["fixture"] = Value::from(true);
        Some(v.to_string())
    }

    fn deployment(&self) -> &Deployment {
        &self.cfg.deployment
    }

    // ── Registry ────────────────────────────────────────────────────────

    fn registry_now(&mut self) -> Result<Value, String> {
        if let Some((at, v)) = &self.registry_cache {
            if now_ms().saturating_sub(*at) < REGISTRY_FRESH_MS {
                return Ok(v.clone());
            }
        }
        let d = self.deployment().clone();
        let mut problems = Vec::new();
        // The LEZ side: read natively from the deployment's endpoint (no CLI needed: the Names
        // Wallet mode in Basecamp sets none), else through the developer CLI.
        let native = !d.lez_rpc.is_empty() && (self.names_mode() || self.wallet_cli().is_err());
        let lez = if native {
            let rpc: Box<dyn lez::LezRpc> = match &self.relayed.rpc_override {
                Some(r) => Box::new(SharedRpc(r.clone())),
                None => Box::new(lez::Http::new(&d.lez_rpc)),
            };
            util::lez_account(&d.bridge_program)
                .and_then(|b| lez::show_json(rpc.as_ref(), &b))
                .map_err(|e| problems.push(format!("LEZ bridge ({}): {e}", d.network)))
                .ok()
        } else {
            self.wallet_cli().and_then(|c| c.show(&d.bridge_program)).map_err(|e| problems.push(format!("LEZ bridge: {e}"))).ok()
        };
        // A stand-in deployment has no attestation instance to read.
        let attest = if d.attest_program.is_empty() {
            None
        } else {
            self.wallet_cli()
                .and_then(|c| c.attest_show(&d.attest_program, &d.attest_instance, None))
                .map_err(|e| problems.push(format!("attestation: {e}")))
                .ok()
        };
        let eth = self.eth()?;
        let token_addrs: Vec<[u8; 20]> = d.tokens.iter().map(|t| t.eth_token).collect();
        let denoms_per: Vec<usize> = d
            .tokens
            .iter()
            .map(|t| {
                lez.as_ref()
                    .and_then(|l| l["tokens"].as_array()?.iter().find(|x| x["eth_token"].as_str() == Some(&eth::addr_hex(&t.eth_token)))?["denoms"].as_array().map(Vec::len))
                    .unwrap_or(3)
            })
            .collect();
        let vault = vault::read(eth.as_ref(), &d.vault, &token_addrs, &denoms_per).map_err(|e| problems.push(format!("Ethereum vault: {e}"))).ok();
        // The anchor's block, for its age: the attestation records a block hash only (Gloas).
        let anchor = attest.as_ref().and_then(|a| a["latest_anchor"]["value"].as_str().map(str::to_owned));
        let anchor_block = anchor.as_deref().and_then(|h| eth.block_by_hash(h).map_err(|e| problems.push(format!("anchor block: {e}"))).ok().flatten());
        let finalized = eth.block("finalized").map_err(|e| problems.push(format!("finalized block: {e}"))).ok();
        if lez.is_none() && vault.is_none() {
            return Err(problems.join("; "));
        }
        let lez_source = if native { format!("{} ({})", d.network, d.lez_rpc) } else { self.cfg.sequencer.clone() };
        let v = shape_registry(&d, lez.as_ref(), vault.as_ref(), attest.as_ref(), anchor_block.as_ref(), finalized.as_ref(), &problems, &eth.describe(), &lez_source);
        drop(eth);
        self.registry_cache = Some((now_ms(), v.clone()));
        Ok(v)
    }

    fn token_info(&mut self, token: &str) -> Result<(config::Token, Value), String> {
        let t = self.deployment().token(token)?.clone();
        let reg = self.registry_now()?;
        let row = reg["tokens"].as_array().and_then(|l| l.iter().find(|r| r["symbol"] == t.symbol.as_str()).cloned()).ok_or("the registry has no row for this token")?;
        Ok((t, row))
    }

    // ── Deposits ────────────────────────────────────────────────────────

    fn events(&self) -> Result<(Vec<eth::DepositEvent>, eth::BlockInfo, u64), String> {
        let eth = self.eth()?;
        let d = self.deployment();
        let latest = eth.block("latest")?;
        let finalized = eth.block("finalized")?;
        let count_raw = eth.call(&d.vault, &eth::calldata("depositCount()", &[]))?;
        let count = eth::Words(&count_raw).u64(0)?;
        let events = eth::deposit_events(eth.as_ref(), &d.vault, d.vault_create_block, latest.number, count)?;
        Ok((events, finalized, latest.number))
    }

    fn deposits_now(&mut self) -> Result<Value, String> {
        let list = self.store().load()["deposits"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return Ok(json!({ "deposits": [], "as_of": now_ms() }));
        }
        let (events, finalized, _) = self.events()?;
        let reg = self.registry_now().ok();
        let anchor_number = reg.as_ref().and_then(|r| r["attestation"]["anchor_block"].as_u64());
        let jobs = self.all_jobs();
        let mut out = Vec::new();
        for mut d in list {
            let decimals = self.deployment().token(d["symbol"].as_str().unwrap_or("")).map(|t| t.decimals).unwrap_or(18);
            d["decimals"] = Value::from(decimals);
            let ev = events.iter().find(|e| hex0x(&e.commitment).eq_ignore_ascii_case(d["commitment"].as_str().unwrap_or("")));
            if let Some(e) = ev {
                if d["deposit_time"].is_null() {
                    if let Ok(eth) = self.eth() {
                        if let Ok(b) = eth.block(&format!("0x{:x}", e.block)) {
                            d["deposit_time"] = Value::from(b.timestamp);
                            let _ = self.store().put_deposit(d.clone());
                        }
                    }
                }
            }
            out.push(deposit_status(&d, ev, &events, finalized.number, anchor_number, &jobs, now_ms() / 1000));
        }
        Ok(json!({ "deposits": out, "as_of": now_ms(), "finalized_block": finalized.number, "anchor_block": anchor_number }))
    }

    /// Writes the leaves under `anchor_number` and makes sure the storage proof and header for
    /// their root at the anchor are on disk. Returns (leaves file, material dir, root, anchor).
    fn claim_material(&mut self, d: &Value) -> Result<(PathBuf, PathBuf, String, String), String> {
        let reg = self.registry_now()?;
        let anchor = reg["attestation"]["anchor"].as_str().ok_or("the attestation instance has no recorded anchor yet")?.to_owned();
        let anchor_number = reg["attestation"]["anchor_block"].as_u64().ok_or("the anchor's Ethereum block is unknown (the Ethereum RPC did not find it)")?;
        let (events, _, _) = self.events()?;
        let under: Vec<eth::DepositEvent> = events.iter().filter(|e| e.block <= anchor_number).cloned().collect();
        let mine = under.iter().find(|e| hex0x(&e.commitment).eq_ignore_ascii_case(d["commitment"].as_str().unwrap_or("")));
        if mine.is_none() {
            return Err(if events.iter().any(|e| hex0x(&e.commitment).eq_ignore_ascii_case(d["commitment"].as_str().unwrap_or(""))) {
                "insufficient finality: this deposit is newer than the attestation's latest anchor; it becomes claimable after the next attestation update".to_owned()
            } else {
                "this deposit has not been seen in the vault yet".to_owned()
            });
        }
        let root = hex0x(&under.last().unwrap().root);
        let tokens = reg["tokens"].as_array().cloned().unwrap_or_default();
        let denoms = |t: &[u8; 20], i: u32| -> Option<u128> {
            let row = tokens.iter().find(|r| r["eth_token"].as_str() == Some(&eth::addr_hex(t)))?;
            units(row["denoms"][i as usize]["value"].as_str()?).ok()
        };
        let leaves = store::leaves_text(&self.deployment().cfg_id, &under, &denoms)?;
        let mdir = self.data.join("material");
        util::private_dir(&mdir).map_err(|e| e.to_string())?;
        let leaves_path = mdir.join(format!("leaves-{}.txt", &root[2..14]));
        util::write_private(&leaves_path, leaves.as_bytes()).map_err(|e| e.to_string())?;
        let dir = mdir.join(format!("{}-{}", &anchor[2..14], &root[2..14]));
        if !dir.join("eth_getProof.json").exists() {
            let rpc = self.cfg.proof_rpc_for(self.deployment().chain_id).ok_or("no Ethereum RPC for proofs is set")?.to_owned();
            self.wallet_cli()?.fetch_material(self.deployment(), &rpc, &root, &anchor, &dir).map_err(|e| {
                format!("could not fetch the storage proof at the anchor from {rpc} ({e}). Public endpoints often serve only recent blocks; set an Ethereum RPC for proofs that keeps state history")
            })?;
        }
        Ok((leaves_path, dir, root, anchor))
    }

    /// The CLI wallet mode's claim and what it reads on stdin: the deposit's secret goes through
    /// `--secret-file -` (stdin) when the CLI has it, never on argv; an older CLI gets `--secret`.
    fn claim_command(&self, d: &Value, leaves: &Path, dir: &Path, anchor: &str, index: u64, reveal: bool, dry_run: bool) -> Result<(std::process::Command, Option<Vec<u8>>), String> {
        let dep = self.deployment();
        let secret = d["secret"].as_str().ok_or("this deposit's secret is not on this computer")?;
        let mut a: Vec<String> = Vec::new();
        if dry_run {
            a.push("--dry-run".into());
        }
        for s in [
            "claim-attested", "--bridge", &dep.bridge_program, "--token-program", &dep.token_program, "--from", d["account"].as_str().unwrap_or_default(), "--eth-token",
            d["eth_token"].as_str().unwrap_or_default(), "--denom-index", &d["denom_index"].to_string(), "--denom", d["denom"].as_str().unwrap_or_default(), "--cfg-id",
            &dep.cfg_id, "--leaves", &leaves.display().to_string(), "--index", &index.to_string(), "--proof", &dir.join("eth_getProof.json").display().to_string(),
            "--anchor-block-hash", anchor, "--header-rlp", &dir.join("header.rlp.hex").display().to_string(),
        ] {
            a.push(s.to_owned());
        }
        if reveal {
            a.push("--reveal-recipient".into());
        }
        let cli = self.wallet_cli()?;
        if cli.json_mode() {
            a.extend(["--secret-file".to_owned(), "-".to_owned()]);
            return Ok((cli.command(&a), Some(secret.as_bytes().to_vec())));
        }
        // An older CLI without --secret-file: on argv, dry runs only (CLI-CONTRACT.md §0.4).
        a.extend(["--secret".to_owned(), secret.to_owned()]);
        Ok((cli.command(&a), None))
    }

    fn leaf_index(&self, d: &Value) -> Result<u64, String> {
        let (events, _, _) = self.events()?;
        events
            .iter()
            .find(|e| hex0x(&e.commitment).eq_ignore_ascii_case(d["commitment"].as_str().unwrap_or("")))
            .map(|e| e.leaf_index)
            .ok_or_else(|| "this deposit has not been seen in the vault yet".to_owned())
    }

    fn claim_plan_now(&mut self, id: &str) -> Result<Value, String> {
        let mut d = self.store().deposit(id)?;
        let (leaves, dir, root, anchor) = self.claim_material(&d)?;
        let index = self.leaf_index(&d)?;
        let (cmd, input) = self.claim_command(&d, &leaves, &dir, &anchor, index, false, true)?;
        let (code, out) = cli::run_input(cmd, input.as_deref(), std::time::Duration::from_secs(25))?;
        let parsed = text::parse_claim(&out);
        if parsed["error"]["code"] == "already_claimed" && d["claimed"] != true {
            d["claimed"] = Value::from(true);
            self.store().put_deposit(d.clone())?;
        }
        let status = self.deposits_now().ok().and_then(|v| v["deposits"].as_array()?.iter().find(|x| x["id"] == id).cloned()).unwrap_or(Value::Null);
        let set = status["anonymity_set"].as_u64();
        let early = status["claim_after"].as_u64().is_some_and(|t| now_ms() / 1000 < t);
        let private_target = d["account_kind"] != "public";
        let mut choices = json!({ "flow": "claim" });
        if let Some(n) = set {
            choices["anonymity_set"] = Value::from(n);
        }
        if !private_target {
            choices["public_account"] = Value::from(true);
        }
        if early {
            choices["claim_early"] = Value::from(true);
        }
        let reg = self.registry_now().ok();
        let sponsor = reg.as_ref().map(|r| r["lez"]["sponsor_amount"].clone()).unwrap_or(Value::Null);
        Ok(json!({
            "deposit": public_view(&status),
            "ok": code == Some(0) && parsed["error"].is_null(),
            "dry_run": parsed,
            "error": parsed["error"],
            "root": root,
            "anchor": anchor,
            "leaf_index": index,
            "recipient": parsed["recipient"],
            "recipient_kind": parsed["recipient_kind"],
            "sponsored": parsed["sponsor"].as_str().is_some_and(|s| s != "0"),
            "sponsor": sponsor,
            "claim_early": early,
            "privacy": privacy::report(&choices)?,
            "proving": proving_note(&self.cfg),
            "sends_enabled": self.cfg.proving_enabled,
        }))
    }

    fn claim_now(&mut self, id: &str, options: &Value) -> Result<Value, String> {
        let d = self.store().deposit(id)?;
        let reveal = options["reveal_recipient"] == true;
        let mut choices = json!({ "flow": "claim", "confirm_weaker": options["confirm_weaker"] == true });
        if reveal {
            choices["reveal_recipient"] = Value::from(true);
        }
        if options["claim_early"] == true {
            choices["claim_early"] = Value::from(true);
        }
        if d["account_kind"] == "public" {
            choices["public_account"] = Value::from(true);
        }
        privacy::require_confirmation(&choices)?;
        let (dry_run, forced) = self.send_mode(options)?;
        let (leaves, dir, _, anchor) = self.claim_material(&d)?;
        let index = self.leaf_index(&d)?;
        let (cmd, input) = self.claim_command(&d, &leaves, &dir, &anchor, index, reveal, dry_run)?;
        let mut job = self.start_job(Spec {
            kind: "claim".into(),
            subject: id.to_owned(),
            steps: vec![cmd],
            inputs: vec![input],
            dry_run,
            guarded: !dry_run,
            prove_estimate_s: if d["account_kind"] == "public" { 0 } else { config::CLAIM_PROOF_ESTIMATE_S },
            peak_estimate_mb: config::CLAIM_PROOF_PEAK_MB,
            meta: json!({ "deposit": id, "symbol": d["symbol"], "denom": d["denom"], "privacy": privacy::report(&choices)?["level"], "dry_run_forced": forced }),
        })?;
        if let Some(why) = forced {
            job["dry_run_forced"] = Value::from(why);
        }
        Ok(job)
    }

    /// Whether a send runs for real: only with sends turned on, and never in this build's
    /// default settings. Returns (dry_run, why it was forced).
    fn send_mode(&self, options: &Value) -> Result<(bool, Option<String>), String> {
        if !self.cfg.proving_enabled {
            return Ok((true, Some("Sends are off in this build: this ran as a dry run (simulated, nothing proven or sent). Proving needs the memory guard set up and a CLI that reads secrets from a file.".to_owned())));
        }
        if options["dry_run"] != false {
            return Ok((true, None));
        }
        Err("real sends need the CLI to read secrets from a file (CLI-CONTRACT.md §0.4) and run under the memory guard; this build does not start them".to_owned())
    }

    /// A deposit request (see `make_deposit_request`); `public_ok` allows a public account.
    fn deposit_request_now(&mut self, token: &str, denom_index: &str, account: &str, public_ok: bool) -> Result<Value, String> {
        let (t, row) = self.token_info(token)?;
        let i: usize = denom_index.trim().parse().map_err(|_| "denomination index must be a number")?;
        let denom_row = row["denoms"].get(i).ok_or_else(|| format!("{} has {} denominations", t.symbol, row["denoms"].as_array().map_or(0, Vec::len)))?.clone();
        let denom = units(denom_row["value"].as_str().unwrap_or(""))?;
        let fee = units(denom_row["deposit_fee"].as_str().unwrap_or("0")).unwrap_or(0);
        let acct = account.trim().trim_start_matches("Private/").trim_start_matches("Public/").to_owned();
        util::lez_account(&acct)?;
        let cli = self.wallet_cli()?;
        let kind = cli
            .accounts()
            .ok()
            .and_then(|a| a["accounts"].as_array()?.iter().find(|x| x["account"] == acct.as_str()).and_then(|x| x["kind"].as_str().map(str::to_owned)))
            .unwrap_or_else(|| "unknown".to_owned());
        if kind == "public" && !public_ok {
            return Err("this is a public account: claiming into it publishes the deposit's secret and links your Ethereum address to it for good. Use a private account, or make_public_deposit_request with confirm_weaker".to_owned());
        }
        let d = self.deployment().clone();
        let secret = hex0x(&util::random32()?);
        let c = cli.commitment(&secret, &acct, &eth::addr_hex(&t.eth_token), &denom.to_string(), &d.cfg_id)?;
        let stored = store::new_deposit(&eth::addr_hex(&t.eth_token), &t.symbol, i as u32, denom, &acct, &kind, &c, &secret, "module");
        self.store().put_deposit(stored.clone())?;
        self.deposit_reply(&t, i, denom, fee, &c, &stored, &kind)
    }

    /// The deposit request's reply: the vault call (and approval), the EIP-681 link and its QR,
    /// the export text and the privacy report.
    #[allow(clippy::too_many_arguments)]
    /// The deposit desk's quote for one denomination: `(vaultFee, bridgeFee, total, feeBps)`.
    fn desk_quote(&self, desk: &[u8; 20], token: &[u8; 20], i: usize) -> Result<(u128, u128, u128, u64), String> {
        let eth = self.eth()?;
        let q = eth.call(desk, &vault::desk_quote_calldata(token, i as u32))?;
        let w = eth::Words(&q);
        let (vault_fee, bridge_fee, total) = (units(&w.dec(1)?)?, units(&w.dec(2)?)?, units(&w.dec(3)?)?);
        let bps = eth::Words(&eth.call(desk, &eth::calldata("feeBps()", &[]))?).u64(0)?;
        Ok((vault_fee, bridge_fee, total, bps))
    }

    fn deposit_reply(&self, t: &config::Token, i: usize, denom: u128, fee: u128, c: &str, stored: &Value, kind: &str) -> Result<Value, String> {
        let d = self.deployment().clone();
        let cbytes = util::hex32(c)?;
        let is_eth = t.eth_token == [0; 20];
        let vault_cs = util::checksum_address(&d.vault);
        // The deposit page's code for this deposit (web/deposit/src/lib/request.js, same bytes).
        let code = request::encode(d.chain_id, &d.vault, &t.eth_token, u8::try_from(i).map_err(|_| "denomination index above 255")?, denom, &cbytes)?;
        let link = request::url(request::PAGE_URL, &code);
        // What `receive_request` rebuilds this code from later, kept with the deposit: the
        // deployment's chain and vault (the rest is stored already), and the code itself to check
        // the rebuild against.
        let mut stored = stored.clone();
        if let Some(o) = stored.as_object_mut() {
            o.insert("chain_id".into(), Value::from(d.chain_id));
            o.insert("vault".into(), Value::from(eth::addr_hex(&d.vault)));
            o.insert("deployment".into(), Value::from(d.name.clone()));
            o.insert("request".into(), Value::from(code.clone()));
        }
        self.store().put_deposit(stored.clone())?;
        let stored = &stored;
        // Through the deposit desk (the bridge fee) when the deployment has one; else straight to
        // the vault. A desk deployment gets no direct-to-vault link (it would skip the fee).
        let desk = if d.desk.is_empty() { None } else { Some(util::eth_address(&d.desk)?) };
        let quote = desk.map(|k| self.desk_quote(&k, &t.eth_token, i));
        let (to, call, total, spender) = match (&desk, &quote) {
            (Some(k), Some(Ok((_, _, total, _)))) => (*k, vault::desk_deposit_calldata(&t.eth_token, i as u32, &cbytes), *total, *k),
            _ => (d.vault, vault::deposit_calldata(&t.eth_token, i as u32, &cbytes), denom + fee, d.vault),
        };
        let mut steps = Vec::new();
        if desk.is_none() || matches!(quote, Some(Ok(_))) {
            let to_cs = util::checksum_address(&to);
            let via = if desk.is_some() { "the deposit desk" } else { "the vault" };
            if !is_eth {
                steps.push(json!({ "step": "approve", "to": util::checksum_address(&t.eth_token), "value": "0",
                    "data": hex0x(&vault::approve_calldata(&spender, total)), "text": format!("Approve {via} to take {} {}", format_amount(total, t.decimals), t.symbol) }));
            }
            steps.push(json!({ "step": "deposit", "to": to_cs, "value": if is_eth { total.to_string() } else { "0".to_owned() },
                "data": hex0x(&call), "text": format!("Deposit {} {} through {via}", format_amount(denom, t.decimals), t.symbol) }));
        }
        let eip681 = if desk.is_some() {
            None
        } else if is_eth {
            Some(format!("ethereum:{vault_cs}@{}/depositETH?uint32={i}&bytes32={c}&value={total}", d.chain_id))
        } else {
            Some(format!("ethereum:{vault_cs}@{}/deposit?address={}&uint32={i}&bytes32={c}", d.chain_id, util::checksum_address(&t.eth_token)))
        };
        let bridge_fee = match (&desk, &quote) {
            (Some(k), Some(Ok((vault_fee, bf, total, bps)))) => json!({
                "desk": util::checksum_address(k), "fee_bps": bps, "percent_text": format!("{}%", util::format_amount(u128::from(*bps), 2)),
                "amount": bf.to_string(), "amount_text": format_amount(*bf, t.decimals), "vault_fee": vault_fee.to_string(),
                "total": total.to_string(), "total_text": format_amount(*total, t.decimals),
                "text": format!("Bridge fee {} {} ({}% of {} {}); you send {} {} in all.", format_amount(*bf, t.decimals), t.symbol, util::format_amount(u128::from(*bps), 2), format_amount(denom, t.decimals), t.symbol, format_amount(*total, t.decimals), t.symbol),
            }),
            (Some(k), Some(Err(e))) => json!({ "desk": util::checksum_address(k), "error": format!("The bridge fee can't be read now ({e}); the deposit page reads it again before you send."), "total": null }),
            _ => Value::Null,
        };
        let fee_line = match bridge_fee["text"].as_str() {
            Some(text) => text.to_owned(),
            None => format!("deposit fee {}", format_amount(fee, t.decimals)),
        };
        let export = format!(
            "LEZ bridge deposit request\n\nNetwork: {} (chain id {})\nVault: {vault_cs}\nToken: {} ({})\nAmount: {} {} ({fee_line})\nCommitment: {c}\nDeposit code: {code}\nOpen: {link}\n\n{}\nThe commitment does not reveal which LEZ account receives the deposit. Keep this computer's bridge data backed up: the secret that claims this deposit is stored only here.\n",
            if d.chain_id == 11_155_111 { "Sepolia" } else { "Ethereum" },
            d.chain_id,
            t.symbol,
            if is_eth { "native ETH".to_owned() } else { util::checksum_address(&t.eth_token) },
            format_amount(denom, t.decimals),
            t.symbol,
            steps.iter().enumerate().map(|(n, s)| format!("Step {}: {}\n  to: {}\n  value (wei): {}\n  data: {}\n", n + 1, s["text"].as_str().unwrap(), s["to"].as_str().unwrap(), s["value"].as_str().unwrap(), s["data"].as_str().unwrap())).collect::<String>()
        );
        let mut choices = json!({ "flow": "deposit" });
        if kind == "public" {
            choices["public_account"] = Value::from(true);
        }
        Ok(json!({
            "deposit": public_view(stored),
            "symbol": t.symbol, "denom_text": format_amount(denom, t.decimals), "deposit_fee_text": format_amount(fee, t.decimals),
            "total": total.to_string(), "total_text": format_amount(total, t.decimals), "bridge_fee": bridge_fee,
            "vault": vault_cs, "chain_id": d.chain_id, "commitment": c, "account_kind": kind,
            "request": code, "request_url": link, "request_qr": qr::matrix(&link)?,
            "compare": compare::json(&cbytes),
            "steps": steps, "eip681": eip681, "qr": match &eip681 { Some(u) => qr::matrix(u)?, None => Value::Null }, "export_text": export,
            "send_in_basecamp": { "available": false, "reason": "Sending from Basecamp comes with the Logos Ethereum module. Until then, open the deposit code on the deposit page, or send these steps from any Ethereum wallet." },
            "privacy": privacy::report(&choices)?,
        }))
    }

    // ── Holdings ────────────────────────────────────────────────────────

    fn holdings_now(&mut self) -> Result<Value, String> {
        let path = self.data.join("holdings.json");
        let cached = util::read_json(&path);
        let age = cached.as_ref().and_then(|c| c["as_of"].as_u64()).map(|t| now_ms().saturating_sub(t));
        let busy = self.runner().busy();
        if age.map_or(true, |a| a > HOLDINGS_FRESH_MS) && busy.is_none() && start_refresh(&self.data) {
            let c = self.wallet_cli();
            let program = self.deployment().token_program.clone();
            let dep = self.deployment().clone();
            let p = path.clone();
            let dir = self.data.clone();
            let handle = std::thread::spawn(move || {
                let result = c.and_then(|c| c.balances(&program)).map(|b| shape_holdings(&dep, &b));
                let v = match result {
                    Ok(v) => json!({ "as_of": now_ms(), "value": v }),
                    Err(e) => json!({ "as_of": now_ms(), "error": e }),
                };
                let _ = util::write_private(&p, v.to_string().as_bytes());
                end_refresh(&dir);
            });
            if cached.is_none() {
                // First read: wait a little for it.
                // At most 8 s, well inside Basecamp's 20 s call limit; then "reading".
                for _ in 0..40 {
                    if handle.is_finished() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
        let now = util::read_json(&path);
        let refreshing = refreshing(&self.data);
        match now {
            Some(c) if c.get("value").is_some() => {
                let mut v = c["value"].clone();
                let age = now_ms().saturating_sub(c["as_of"].as_u64().unwrap_or(0));
                v["as_of"] = c["as_of"].clone();
                v["stale"] = Value::from(age > HOLDINGS_FRESH_MS);
                v["refreshing"] = Value::from(refreshing);
                if let Some(j) = busy {
                    v["note"] = Value::from(format!("the wallet is busy with job {}; balances refresh when it ends", j["id"].as_str().unwrap_or("")));
                }
                Ok(v)
            }
            Some(c) if c.get("error").is_some() && !refreshing => Err(c["error"].as_str().unwrap_or("could not read balances").to_owned()),
            _ => Ok(json!({ "reading": true, "message": "The wallet is catching up with the chain; balances appear in a moment." })),
        }
    }

    // ── Redemption ──────────────────────────────────────────────────────

    fn plan_redeem_now(&mut self, token: &str, amount: &str, eth_to: &str, relay_index: &str) -> Result<Value, String> {
        let (t, row) = self.token_info(token)?;
        let value = util::parse_amount(amount, t.decimals)?;
        let to = util::eth_address(eth_to)?;
        if util::bad_checksum(eth_to) {
            return Err("that address's capital letters don't match its checksum: it probably has a typo. Copy it again".to_owned());
        }
        let d = self.deployment().clone();
        if to == [0; 20] {
            return Err("the zero address can't receive a release".to_owned());
        }
        if to == d.vault {
            return Err("that is the bridge vault itself, which refuses to release to itself".to_owned());
        }
        let relay_index: usize = relay_index.trim().parse().map_err(|_| "relay fee choice must be 0 (none) or a menu number".to_owned())?;
        let denoms: Vec<u128> = row["denoms"].as_array().unwrap().iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect();
        let s = plan::split(value, &denoms)?;
        if s.pieces.is_empty() {
            return Err(format!("the smallest amount the bridge redeems is {} {}", format_amount(*denoms.iter().min().unwrap_or(&0), t.decimals), t.symbol));
        }
        let mut warnings: Vec<String> = Vec::new();
        if let Ok(eth) = self.eth() {
            match eth.code_len(&to) {
                Ok(n) if n > 0 => warnings.push(if t.eth_token == [0; 20] {
                    "This recipient is a contract. If it cannot receive ETH, the release fails and stays stuck: the recipient is fixed when you burn. Use an ordinary wallet address unless you are sure.".to_owned()
                } else {
                    format!("This recipient is a contract. Make sure it can hold {} and is not blocked by the token's issuer: the recipient is fixed when you burn.", t.symbol)
                }),
                Ok(_) => {}
                Err(e) => warnings.push(format!("Could not check whether the recipient is a contract ({e}).")),
            }
        }
        if t.eth_token != [0; 20] {
            warnings.push(format!("{}'s issuer can block addresses. A blocked recipient can't receive the release.", t.symbol));
        }
        let holdings = self.holdings_now()?;
        let mut accounts: Vec<(String, String, u128)> = holdings["accounts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .flat_map(|acct| {
                        acct["tokens"].as_array().into_iter().flatten().filter(|x| x["symbol"] == t.symbol.as_str()).map(move |x| {
                            (acct["account"].as_str().unwrap_or("").to_owned(), acct["kind"].as_str().unwrap_or("").to_owned(), units(x["balance"].as_str().unwrap_or("0")).unwrap_or(0))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        accounts.sort_by_key(|(_, kind, _)| if kind == "private" { 0 } else { 1 });
        let held: u128 = accounts.iter().map(|a| a.2).sum();
        if holdings.get("reading").is_none() && held < value - s.remainder {
            return Err(format!("you hold {} {} in this wallet, less than {}", format_amount(held, t.decimals), t.symbol, format_amount(value - s.remainder, t.decimals)));
        }
        let exits_possible = self.registry_now().map(|r| r["vault"]["exits_possible"] == true).unwrap_or(false);
        let cli = self.wallet_cli()?;
        let mut pieces = Vec::new();
        let mut any_public = false;
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
            for _ in 0..*count {
                let pos = accounts.iter().position(|a| a.2 >= denom).ok_or_else(|| {
                    format!("no single account holds {} {} to burn as one piece; move holdings together first", format_amount(denom, t.decimals), t.symbol)
                })?;
                accounts[pos].2 -= denom;
                let (from, kind) = (accounts[pos].0.clone(), accounts[pos].1.clone());
                any_public |= kind == "public";
                let secret = hex0x(&util::random32()?);
                let e = cli.exit_commitment(&secret, &eth::addr_hex(&to), &eth::addr_hex(&t.eth_token), &denom.to_string(), &relay_fee.to_string(), &d.cfg_id)?;
                pieces.push(json!({
                    "denom_index": i, "denom": denom.to_string(), "denom_text": format_amount(denom, t.decimals),
                    "from": from, "from_kind": kind, "relay_fee": relay_fee.to_string(), "relay_fee_text": format_amount(relay_fee, t.decimals),
                    "withdraw_fee": withdraw_fee.to_string(), "receive": (denom - withdraw_fee - relay_fee).to_string(),
                    "receive_text": format_amount(denom - withdraw_fee - relay_fee, t.decimals),
                    "exit_commitment": e["exit_commitment"], "secret": secret, "burned": false,
                }));
            }
        }
        let receive: u128 = pieces.iter().filter_map(|p| units(p["receive"].as_str()?).ok()).sum();
        let mut choices = json!({ "flow": "redeem" });
        if any_public {
            choices["public_account"] = Value::from(true);
        }
        if self_relay {
            choices["self_relay"] = Value::from(true);
        }
        if s.remainder > 0 {
            warnings.push(format!("{} {} is not a denomination and stays in your wallet.", format_amount(s.remainder, t.decimals), t.symbol));
        }
        let id = format!("rdm-{}", now_ms());
        let plan = json!({
            "id": id, "symbol": t.symbol, "eth_token": eth::addr_hex(&t.eth_token), "eth_to": util::checksum_address(&to), "pieces": pieces,
            "amount_text": format_amount(value - s.remainder, t.decimals), "receive_text": format_amount(receive, t.decimals), "relay_fee_index": relay_index,
            "warnings": warnings, "exits_possible": exits_possible,
            "blocked": if exits_possible { Value::Null } else { Value::from(format!("{} Burning here would destroy your wrapped tokens for good, so only a dry run is offered.", no_release_line(self.deployment()))) },
            "privacy": privacy::report(&choices)?, "created": now_ms(),
        });
        self.store().put_redeem(plan.clone())?;
        Ok(public_view(&plan))
    }

    fn redeem_now(&mut self, plan_id: &str, options: &Value) -> Result<Value, String> {
        let plan = self.store().redeem(plan_id)?;
        let mut choices = plan["privacy"]["weakened_by"].as_array().cloned().unwrap_or_default().iter().fold(json!({ "flow": "redeem" }), |mut c, o| {
            if let Some(k) = o["key"].as_str() {
                c[k] = Value::from(true);
            }
            c
        });
        choices["confirm_weaker"] = Value::from(options["confirm_weaker"] == true);
        privacy::require_confirmation(&choices)?;
        let (mut dry_run, mut forced) = self.send_mode(options)?;
        if plan["exits_possible"] != true && !dry_run {
            dry_run = true;
            forced = Some(plan["blocked"].as_str().unwrap_or("this vault cannot release").to_owned());
        }
        let d = self.deployment().clone();
        let cli = self.wallet_cli()?;
        let definition = d.token(plan["symbol"].as_str().unwrap_or(""))?.definition.clone();
        let steps: Vec<std::process::Command> = plan["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut a: Vec<String> = Vec::new();
                if dry_run {
                    a.push("--dry-run".into());
                }
                for s in [
                    "burn", "--bridge", &d.bridge_program, "--token-program", &d.token_program, "--from", p["from"].as_str().unwrap_or_default(), "--definition", &definition,
                    "--amount", p["denom"].as_str().unwrap_or_default(), "--exit", p["exit_commitment"].as_str().unwrap_or_default(),
                ] {
                    a.push(s.to_owned());
                }
                cli.command(&a)
            })
            .collect();
        let private = plan["pieces"].as_array().unwrap().iter().any(|p| p["from_kind"] == "private");
        let mut job = self.start_job(Spec {
            kind: "redeem".into(),
            subject: plan_id.to_owned(),
            steps,
            inputs: vec![],
            dry_run,
            guarded: !dry_run,
            prove_estimate_s: if private { config::BURN_PROOF_ESTIMATE_S } else { 0 },
            peak_estimate_mb: config::BURN_PROOF_PEAK_MB,
            meta: json!({ "plan": plan_id, "symbol": plan["symbol"], "amount_text": plan["amount_text"], "dry_run_forced": forced }),
        })?;
        if let Some(why) = forced {
            job["dry_run_forced"] = Value::from(why);
        }
        Ok(job)
    }

    fn exits_now(&self) -> Value {
        let all = self.store().load();
        let jobs = self.all_jobs();
        let mut exits = Vec::new();
        for r in all["redeems"].as_array().unwrap() {
            let job = jobs.iter().find(|j| j["subject"] == r["id"] && j["wallet"] != "names");
            for (k, p) in r["pieces"].as_array().unwrap().iter().enumerate() {
                // A Names Wallet plan proposes its pieces one by one, each its own wallet job.
                let wjob = jobs.iter().find(|j| j["wallet"] == "names" && j["subject"] == r["id"] && j["meta"]["piece"] == k as u64);
                let burned = job.is_some_and(|j| j["state"] == "done" && j["dry_run"] == false) || wjob.is_some_and(|j| j["state"] == "done");
                let stage = if burned {
                    "burned"
                } else if wjob.is_some_and(|j| j["state"] == "running") {
                    "proposed"
                } else if job.is_some_and(|j| j["dry_run"] == true && j["state"] == "done") {
                    "simulated"
                } else {
                    "planned"
                };
                exits.push(json!({
                    "id": format!("{}-{k}", r["id"].as_str().unwrap_or("")),
                    "plan": r["id"], "symbol": r["symbol"], "denom_text": p["denom_text"], "eth_to": r["eth_to"], "receive_text": p["receive_text"],
                    "relay_fee_text": p["relay_fee_text"], "exit_commitment": p["exit_commitment"], "job": wjob.map(|j| j["id"].clone()),
                    "stage": stage,
                    "release": release_unavailable(r["exits_possible"] == true, self.deployment()),
                }));
            }
        }
        json!({ "exits": exits })
    }

    // ── Admin ───────────────────────────────────────────────────────────

    fn admin_status_now(&mut self) -> Result<Value, String> {
        let reg = self.registry_now()?;
        let mine: Vec<String> = self
            .wallet_cli()
            .and_then(|c| c.accounts())
            .map(|a| a["accounts"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x["account"].as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let admin = reg["lez"]["admin"].as_str().unwrap_or("").to_owned();
        let freezer = reg["lez"]["freezer"].as_str().unwrap_or("").to_owned();
        let holds_admin = mine.contains(&admin);
        let holds_freezer = mine.contains(&freezer);
        let lez_op = |op: &str, title: &str, needs: bool| {
            json!({ "op": op, "title": title, "side": "lez", "available": needs,
                    "reason": if needs { Value::from("runs through the lez-bridge CLI as a dry run in this build") } else { Value::from("this wallet does not hold the key this needs") } })
        };
        let eth_op = |op: &str, title: &str| {
            json!({ "op": op, "title": title, "side": "ethereum", "available": false,
                    "reason": "not available in this build: Ethereum admin transactions go through the Logos Ethereum module, which is not wired yet" })
        };
        Ok(json!({
            "lez": { "admin": admin, "freezer": freezer, "this_wallet_is_admin": holds_admin, "this_wallet_is_freezer": holds_freezer,
                     "mint_frozen": reg["lez"]["mint_frozen"], "burn_frozen": reg["lez"]["burn_frozen"], "sponsor_amount": reg["lez"]["sponsor_amount"],
                     "float": reg["lez"]["float"], "claims_float_covers": reg["lez"]["claims_float_covers"], "root_source": reg["lez"]["root_source"] },
            "ethereum": { "admin": reg["vault"]["admin"], "freezer": reg["vault"]["freezer"], "fee_recipient": reg["vault"]["fee_recipient"],
                          "pending_admin": reg["vault"]["pending_admin"], "deposits_frozen": reg["vault"]["deposits_frozen"], "releases_frozen": reg["vault"]["releases_frozen"],
                          "min_release_delay_s": reg["vault"]["min_release_delay_s"], "global_cap": reg["vault"]["global_cap"], "exits_possible": reg["vault"]["exits_possible"] },
            "attestation": reg["attestation"],
            "tokens": reg["tokens"],
            "operations": [
                lez_op("freeze", "Freeze minting or burning (all tokens or one)", holds_freezer || holds_admin),
                lez_op("unfreeze", "Unfreeze minting or burning", holds_admin),
                lez_op("set_sponsor", "Set the sponsor amount per claim", holds_admin),
                lez_op("fund_float", "Top up the float (anyone, from a public account)", !mine.is_empty()),
                lez_op("register_token", "Register a token on LEZ", holds_admin),
                eth_op("eth_register_token", "Register or delist a token in the vault"),
                eth_op("eth_set_fees", "Deposit and withdraw fees"),
                eth_op("eth_set_caps", "Caps and cap window"),
                eth_op("eth_set_cap_weight", "Global cap weights"),
                eth_op("eth_set_relay_menu", "Relay-fee menu"),
                eth_op("eth_set_delay", "Release delay"),
                eth_op("eth_freeze", "Freeze deposits or releases on Ethereum"),
                eth_op("eth_roles", "Admin, freezer and fee recipient"),
            ],
            "updater": { "available": false, "reason": "The attestation update daemon is not built; a finality update took 7,074.7 s to prove on a 16 GB Apple M4 desktop." },
            "as_of": reg["as_of"],
        }))
    }

    fn admin_action_now(&mut self, a: &Value) -> Result<Value, String> {
        let op = a["op"].as_str().ok_or("give an op")?;
        if op.starts_with("eth_") {
            return Ok(json!({ "available": false, "op": op, "reason": "not available in this build: Ethereum admin transactions go through the Logos Ethereum module, which is not wired yet" }));
        }
        let status = self.admin_status_now()?;
        let d = self.deployment().clone();
        let admin = status["lez"]["admin"].as_str().unwrap_or("").to_owned();
        let freezer = status["lez"]["freezer"].as_str().unwrap_or("").to_owned();
        let token_arg = |a: &Value| -> Result<Vec<String>, String> {
            match a["token"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => Ok(vec!["--eth-token".into(), eth::addr_hex(&d.token(t)?.eth_token)]),
                None => Ok(vec![]),
            }
        };
        let sides = |a: &Value| -> Result<Vec<String>, String> {
            let mut v = Vec::new();
            if a["mint"] == true {
                v.push("--mint".to_owned());
            }
            if a["burn"] == true {
                v.push("--burn".to_owned());
            }
            if v.is_empty() {
                return Err("choose minting, burning or both".to_owned());
            }
            Ok(v)
        };
        let mut args: Vec<String> = match op {
            "freeze" => {
                let by = if status["lez"]["this_wallet_is_freezer"] == true { freezer } else { admin };
                let mut v = vec!["freeze".into(), "--bridge".into(), d.bridge_program.clone(), "--by".into(), by];
                v.extend(token_arg(a)?);
                v.extend(sides(a)?);
                v
            }
            "unfreeze" => {
                let mut v = vec!["unfreeze".into(), "--bridge".into(), d.bridge_program.clone(), "--admin".into(), admin];
                v.extend(token_arg(a)?);
                v.extend(sides(a)?);
                v
            }
            "set_sponsor" => vec!["set-sponsor".into(), "--bridge".into(), d.bridge_program.clone(), "--admin".into(), admin, "--amount".into(), units(a["amount"].as_str().unwrap_or(""))?.to_string()],
            "fund_float" => vec![
                "fund-float".into(), "--bridge".into(), d.bridge_program.clone(), "--from".into(), util::lez_account(a["from"].as_str().unwrap_or("")).map(|_| a["from"].as_str().unwrap().to_owned())?,
                "--amount".into(), units(a["amount"].as_str().unwrap_or(""))?.to_string(),
            ],
            "register_token" => {
                let denoms: Vec<String> = a["denoms"].as_array().ok_or("give denoms")?.iter().map(|x| units(x.as_str().unwrap_or("")).map(|n| n.to_string())).collect::<Result<_, _>>()?;
                vec![
                    "register".into(), "--bridge".into(), d.bridge_program.clone(), "--admin".into(), admin, "--eth-token".into(),
                    eth::addr_hex(&util::eth_address(a["eth_token"].as_str().unwrap_or(""))?), "--definition".into(), a["definition"].as_str().unwrap_or("").to_owned(),
                    "--decimals".into(), a["decimals"].as_u64().ok_or("give decimals")?.to_string(), "--denoms".into(), denoms.join(","),
                ]
            }
            _ => return Err(format!("unknown admin op \"{op}\"")),
        };
        let (dry_run, forced) = self.send_mode(a)?;
        if dry_run {
            args.insert(0, "--dry-run".into());
        }
        let cmd = self.wallet_cli()?.command(&args);
        let mut job = self.start_job(Spec {
            kind: format!("admin-{}", op.replace('_', "-")),
            subject: op.to_owned(),
            steps: vec![cmd],
            inputs: vec![],
            dry_run,
            guarded: false,
            prove_estimate_s: 0,
            peak_estimate_mb: 0,
            meta: json!({ "action": a, "dry_run_forced": forced }),
        })?;
        if let Some(why) = forced {
            job["dry_run_forced"] = Value::from(why);
        }
        Ok(job)
    }
}

/// An `EthSource` borrowed from the test override.
struct Borrowed<'a>(&'a (dyn EthSource + Send));

impl EthSource for Borrowed<'_> {
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn call(&self, to: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        self.0.call(to, data)
    }
    fn calls(&self, to: &[u8; 20], data: &[Vec<u8>]) -> Result<Vec<Result<Vec<u8>, String>>, String> {
        self.0.calls(to, data)
    }
    fn block(&self, tag: &str) -> Result<eth::BlockInfo, String> {
        self.0.block(tag)
    }
    fn block_by_hash(&self, hash: &str) -> Result<Option<eth::BlockInfo>, String> {
        self.0.block_by_hash(hash)
    }
    fn logs(&self, a: &[u8; 20], t: &[u8; 32], f: u64, to: u64) -> Result<Vec<Value>, String> {
        self.0.logs(a, t, f, to)
    }
    fn code_len(&self, a: &[u8; 20]) -> Result<usize, String> {
        self.0.code_len(a)
    }
    fn chain_id(&self) -> Result<u64, String> {
        self.0.chain_id()
    }
}

fn pct(used: &str, cap: &str) -> Value {
    match (units(used), units(cap)) {
        (Ok(u), Ok(c)) if c > 0 => Value::from(((u as f64 / c as f64) * 1000.0).round() / 10.0),
        _ => Value::Null,
    }
}

/// Global-cap units (USD with 6 decimals) as dollars.
fn usd(v: &Value) -> Value {
    v.as_str().and_then(|x| units(x).ok()).map_or(Value::Null, |n| Value::from(format!("${}", format_amount(n, 6))))
}

fn bps_text(fee: u128, denom: u128) -> String {
    if fee == 0 || denom == 0 {
        return "none".to_owned();
    }
    format!("{:.2}%", fee as f64 * 100.0 / denom as f64).replace(".00%", "%")
}

/// The registry reply from the three reads; any of them may be missing (`partial`).
#[allow(clippy::too_many_arguments)]
fn shape_registry(
    d: &Deployment,
    lez: Option<&Value>,
    vault: Option<&Value>,
    attest: Option<&Value>,
    anchor_block: Option<&eth::BlockInfo>,
    finalized: Option<&eth::BlockInfo>,
    problems: &[String],
    eth_source: &str,
    sequencer: &str,
) -> Value {
    let now_s = now_ms() / 1000;
    let mut tokens = Vec::new();
    for t in &d.tokens {
        let a = eth::addr_hex(&t.eth_token);
        let l = lez.and_then(|l| l["tokens"].as_array()?.iter().find(|x| x["eth_token"].as_str() == Some(&a)).cloned()).unwrap_or(Value::Null);
        let v = vault.and_then(|v| v["tokens"].as_array()?.iter().find(|x| x["eth_token"].as_str() == Some(&a)).cloned()).unwrap_or(Value::Null);
        let denoms: Vec<String> = v["denoms"]
            .as_array()
            .or(l["denoms"].as_array())
            .map(|x| x.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let denom_rows: Vec<Value> = denoms
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let n = units(s).unwrap_or(0);
                let dep_fee = units(v["deposit_fees"][i].as_str().unwrap_or("0")).unwrap_or(0);
                let wd_fee = units(v["withdraw_fees"][i].as_str().unwrap_or("0")).unwrap_or(0);
                let menu: Vec<Value> = v["relay_fee_menu"][i]
                    .as_array()
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| x.as_str())
                            .map(|x| json!({ "value": x, "text": format_amount(units(x).unwrap_or(0), t.decimals), "pct": bps_text(units(x).unwrap_or(0), n) }))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "index": i, "value": s, "text": format_amount(n, t.decimals),
                    "deposit_fee": dep_fee.to_string(), "deposit_fee_text": bps_text(dep_fee, n),
                    "withdraw_fee": wd_fee.to_string(), "withdraw_fee_text": bps_text(wd_fee, n),
                    "relay_fee_menu": menu,
                    "deposits": v["counters"][i]["deposits"], "lez_claims": l["claims"][i], "lez_burns": l["burns"][i],
                })
            })
            .collect();
        let cap = &v["cap"];
        tokens.push(json!({
            "symbol": t.symbol, "decimals": t.decimals, "eth_token": a, "definition": t.definition,
            "registered": v["registered"], "lez_registered": l["registered"],
            "denoms": denom_rows,
            "freeze": { "eth_deposits": v["deposits_frozen"], "eth_releases": v["releases_frozen"], "lez_mint": l["mint_frozen"], "lez_burn": l["burn_frozen"] },
            "caps": {
                "window_s": cap["window_s"],
                "deposit": { "cap": cap["deposit_cap"], "used": cap["deposit_used"], "cap_text": cap["deposit_cap"].as_str().map(|x| format_amount(units(x).unwrap_or(0), t.decimals)),
                             "used_text": cap["deposit_used"].as_str().map(|x| format_amount(units(x).unwrap_or(0), t.decimals)), "pct": pct(cap["deposit_used"].as_str().unwrap_or(""), cap["deposit_cap"].as_str().unwrap_or("")) },
                "release": { "cap": cap["release_cap"], "used": cap["release_used"], "cap_text": cap["release_cap"].as_str().map(|x| format_amount(units(x).unwrap_or(0), t.decimals)),
                             "used_text": cap["release_used"].as_str().map(|x| format_amount(units(x).unwrap_or(0), t.decimals)), "pct": pct(cap["release_used"].as_str().unwrap_or(""), cap["release_cap"].as_str().unwrap_or("")) },
            },
            "cap_weight": v["cap_weight"],
            "cap_weight_text": v["cap_weight"].as_str().and_then(|w| units(w).ok()).and_then(|w| w.checked_mul(10u128.pow(t.decimals as u32))).map(|x| format!("1 {} counts as ${} toward the global caps", t.symbol, format_amount(x / 1_000_000_000_000_000_000, 6))),
            "release_delay_s": v["release_delay_s"],
            "accrued_fees": v["accrued_fees"],
            "lez_claimed_value": l["claimed_value"], "lez_burned_value": l["burned_value"],
            "lez_claimed_text": l["claimed_value"].as_str().map(|x| format_amount(units(x).unwrap_or(0), t.decimals)),
        }));
    }
    let gc = vault.map(|v| v["global_cap"].clone()).unwrap_or(Value::Null);
    let sponsor = lez.and_then(|l| units(l["sponsor_amount"].as_str()?).ok());
    let float = lez.and_then(|l| units(l["float"].as_str()?).ok());
    let covers = match (sponsor, float) {
        (Some(s), Some(f)) if s > 0 => Value::from((f / s) as u64),
        _ => Value::Null,
    };
    json!({
        "deployment": { "name": d.name, "label": d.label, "notes": d.notes, "chain_id": d.chain_id, "vault": util::checksum_address(&d.vault), "bridge_program": d.bridge_program, "cfg_id": d.cfg_id },
        "tokens": tokens,
        "lez": lez.map(|l| json!({
            "admin": l["admin"], "freezer": l["freezer"], "mint_frozen": l["mint_frozen"], "burn_frozen": l["burn_frozen"],
            "sponsor_amount": l["sponsor_amount"], "float": l["float"], "claims_float_covers": covers, "root_source": l["root_source"],
            "sponsor_text": sponsor.map(|s| format_amount(s, 0)), "float_text": float.map(|f| format_amount(f, 0)),
            "cfg_id_matches": l["cfg_id"].as_str().is_some_and(|c| c.eq_ignore_ascii_case(&d.cfg_id)),
        })).unwrap_or(Value::Null),
        "vault": vault.map(|v| json!({
            "deposit_count": v["deposit_count"], "deposit_root": v["deposit_root"], "exit_count": v["exit_count"],
            "deposits_frozen": v["deposits_frozen"], "releases_frozen": v["releases_frozen"], "global_cap": gc,
            "global_cap_text": { "deposit_used": usd(&gc["deposit_used"]), "deposit_cap": usd(&gc["deposit_cap"]), "release_used": usd(&gc["release_used"]), "release_cap": usd(&gc["release_cap"]) },
            "global_cap_pct": { "deposit": pct(gc["deposit_used"].as_str().unwrap_or(""), gc["deposit_cap"].as_str().unwrap_or("")),
                                "release": pct(gc["release_used"].as_str().unwrap_or(""), gc["release_cap"].as_str().unwrap_or("")) },
            "min_release_delay_s": v["min_release_delay_s"], "admin": v["admin"], "freezer": v["freezer"], "fee_recipient": v["fee_recipient"],
            "pending_admin": v["pending_admin"], "exits_possible": v["exits_possible"] == true && d.release != "never", "release_kind": d.release, "update_image_id": v["update_image_id"],
            "release_note": if d.release_note.is_empty() { Value::Null } else { Value::from(d.release_note.clone()) },
            "release_detail": if d.release_detail.is_empty() { Value::Null } else { Value::from(d.release_detail.clone()) },
            "cfg_id_matches": v["cfg_id"].as_str().is_some_and(|c| c.eq_ignore_ascii_case(&d.cfg_id)),
        })).unwrap_or(Value::Null),
        "attestation": attest.map(|a| json!({
            "program": a["program"], "instance": a["instance"], "network": a["network"], "chain_id": a["chain_id"], "updates": a["updates"],
            "min_participants": a["min_participants"], "finalized_slot": a["finalized_slot"],
            "anchor": a["latest_anchor"]["value"], "anchor_recorded": a["latest_anchor"]["recorded"],
            "anchor_block": anchor_block.map(|b| b.number), "anchor_time": anchor_block.map(|b| b.timestamp),
            "anchor_age_s": anchor_block.map(|b| now_s.saturating_sub(b.timestamp)),
            "anchor_age_text": anchor_block.map(|b| util::duration_text(now_s.saturating_sub(b.timestamp))),
            "behind_finalized_blocks": match (anchor_block, finalized) { (Some(a), Some(f)) => Value::from(f.number.saturating_sub(a.number)), _ => Value::Null },
            "updater": "No update daemon runs; each finality update is proved by hand (about 2 h on a 16 GB Apple M4 desktop).",
        })).unwrap_or(Value::Null),
        "sources": { "ethereum": eth_source, "sequencer": sequencer, "lez_network": if config::is_loopback(sequencer) { "local sequencer".to_owned() } else if sequencer.starts_with(&d.network) { d.network.clone() } else { "remote sequencer".to_owned() } },
        "partial": !problems.is_empty(),
        "problems": problems,
        "as_of": now_ms(),
    })
}

/// A deposit's stage, timeline and anonymity set.
fn deposit_status(d: &Value, ev: Option<&eth::DepositEvent>, events: &[eth::DepositEvent], finalized: u64, anchor: Option<u64>, jobs: &[Value], now_s: u64) -> Value {
    let mut v = public_view(d);
    let claim_job = jobs.iter().find(|j| j["subject"] == d["id"] && j["kind"] == "claim");
    let claimed = d["claimed"] == true || claim_job.is_some_and(|j| j["state"] == "done" && j["dry_run"] == false);
    v["denom_text"] = Value::from(d["denom"].as_str().and_then(|x| units(x).ok()).map(|n| format_amount(n, d["decimals"].as_u64().unwrap_or(18) as u8)).unwrap_or_default());
    let Some(e) = ev else {
        v["stage"] = Value::from("requested");
        v["stage_text"] = Value::from("Waiting for the deposit on Ethereum");
        v["timeline"] = json!([{ "step": "sent", "done": false, "label": "Send the deposit on Ethereum" }]);
        return v;
    };
    let is_final = e.block <= finalized;
    let anchored = anchor.is_some_and(|a| e.block <= a);
    let deposit_time = d["deposit_time"].as_u64();
    let claim_after = deposit_time.map(|t| t + plan::CLAIM_WAIT_S);
    let same = |x: &&eth::DepositEvent| x.token == e.token && x.denom_index == e.denom_index;
    let set = events.iter().filter(|x| same(x) && anchor.is_some_and(|a| x.block <= a)).count() as u64;
    let after = events.iter().filter(|x| same(x) && x.leaf_index > e.leaf_index).count() as u64;
    let stage = if claimed {
        "claimed"
    } else if anchored && claim_after.is_some_and(|t| now_s >= t) {
        "claimable"
    } else if anchored {
        "anchored"
    } else if is_final {
        "finalized"
    } else {
        "included"
    };
    v["stage"] = Value::from(stage);
    v["stage_text"] = Value::from(match stage {
        "claimed" => "Claimed",
        "claimable" => "Ready to claim",
        "anchored" => "Claimable now; waiting is more private",
        "finalized" => "Final on Ethereum; waiting for the attestation to pass it",
        _ => "Included on Ethereum; waiting for finality (about 13 min)",
    });
    v["leaf_index"] = Value::from(e.leaf_index);
    v["block"] = Value::from(e.block);
    v["tx"] = Value::from(e.tx.clone());
    v["anonymity_set"] = Value::from(set);
    v["deposits_after"] = Value::from(after);
    v["claim_after"] = claim_after.map_or(Value::Null, Value::from);
    v["claim_job"] = claim_job.map_or(Value::Null, |j| j["id"].clone());
    v["timeline"] = json!([
        { "step": "sent", "done": true, "label": "Included on Ethereum", "detail": format!("block {}", e.block), "at": deposit_time },
        { "step": "finalized", "done": is_final, "label": "Final on Ethereum", "detail": if is_final { "past the finalized checkpoint".to_owned() } else { "about 13 minutes after inclusion".to_owned() } },
        { "step": "anchored", "done": anchored, "label": "Anchor recorded on LEZ", "detail": if anchored { "the attestation instance has a finalized anchor past this deposit".to_owned() } else { "waits for the next attestation update".to_owned() } },
        { "step": "suggested", "done": claim_after.is_some_and(|t| now_s >= t), "label": "Suggested claim time", "at": claim_after,
          "detail": format!("{} of this amount under the anchor; {after} arrived after yours", util::count(set, "deposit", "deposits")) },
        { "step": "claimed", "done": claimed, "label": "Claimed on LEZ" },
    ]);
    v
}

/// The holdings reply from the CLI's balances, per account, private first.
fn shape_holdings(d: &Deployment, b: &Value) -> Value {
    let mut accounts: Vec<Value> = Vec::new();
    let mut totals: Vec<(String, u8, u128)> = d.tokens.iter().map(|t| (t.symbol.clone(), t.decimals, 0u128)).collect();
    for h in b["holdings"].as_array().cloned().unwrap_or_default() {
        let Some(t) = d.tokens.iter().find(|t| h["definition"].as_str() == Some(&t.definition)) else { continue };
        let bal = units(h["balance"].as_str().unwrap_or("0")).unwrap_or(0);
        if let Some(x) = totals.iter_mut().find(|x| x.0 == t.symbol) {
            x.2 += bal;
        }
        let entry = json!({ "symbol": t.symbol, "balance": bal.to_string(), "balance_text": format_amount(bal, t.decimals) });
        let acct = h["account"].as_str().unwrap_or("").to_owned();
        match accounts.iter_mut().find(|a| a["account"] == acct.as_str()) {
            Some(a) => a["tokens"].as_array_mut().unwrap().push(entry),
            None => accounts.push(json!({ "account": acct, "kind": h["kind"], "tokens": [entry] })),
        }
    }
    accounts.sort_by_key(|a| if a["kind"] == "private" { 0 } else { 1 });
    json!({
        "accounts": accounts,
        "totals": totals.iter().map(|(s, dec, n)| json!({ "symbol": s, "balance": n.to_string(), "balance_text": format_amount(*n, *dec) })).collect::<Vec<_>>(),
        "synced_to": b["synced_to"],
    })
}

fn proving_note(cfg: &Config) -> Value {
    json!({
        "estimate_s": config::CLAIM_PROOF_ESTIMATE_S,
        "estimate_text": "about 11 minutes",
        "peak_mb": config::CLAIM_PROOF_PEAK_MB,
        "threads": 4,
        "min_ram_gb": config::MIN_RAM_GB,
        "measured_on": "Apple M4 (16 GB), 4 threads, CPU prover: 670.9 s, 9,403 MB peak (LEZ block 4998)",
        "hardware_note": "A private claim is proved on this computer. Measured on a 16 GB Apple M4 desktop: about 11 minutes and 9.4 GB of memory at peak. Smaller computers are untested; close other large apps first, and keep Basecamp open until it finishes.",
        "enabled": cfg.proving_enabled,
    })
}

/// The first words of a redeem's `blocked` line for a vault that can't pay out: the deployment's
/// own `release_note` (v3: its outbound state deployed at zero), else the placeholder-image words.
fn no_release_line(d: &Deployment) -> String {
    if d.release == "never" && !d.release_note.is_empty() {
        d.release_note.clone()
    } else {
        "This vault can never release: its outbound proof image is a placeholder (demo deployment), and the outbound proof (G2) is not built.".to_owned()
    }
}

/// What the apps say about releasing on this deployment's vault (`release`, exits, the registry).
fn release_unavailable(exits_possible: bool, d: &Deployment) -> Value {
    if d.release == "never" && !d.release_note.is_empty() {
        return json!({
            "available": false,
            "reason": d.release_note,
            "vault_note": d.release_note,
            "detail": d.release_detail,
            "what_happens_next": "A burn on this vault can't be paid out on Ethereum: withdrawals come with the next vault.",
        });
    }
    if d.release == "operator" && exits_possible {
        return json!({
            "available": false,
            "reason": "This vault releases on the bridge operator's attestation: the operator's attestor key signs each release (an operator-attested demo; no zk proof is checked on Ethereum). This app can't release by itself, and no signing service runs yet: ask the operator.",
            "vault_note": "Operator-attested demo vault: not trustless. Releases happen within the vault's caps and after its release delay.",
            "what_happens_next": "Once the operator signs, the vault records the exit; after the release delay, anyone (you or a relayer, paid the fee you chose) releases it to the recipient you fixed when burning.",
        });
    }
    json!({
        "available": false,
        "reason": "Releasing on Ethereum needs the outbound proof (G2), which proves LEZ burns to the vault. It is not built yet, so no exit can be released today.",
        "vault_note": if exits_possible { Value::Null } else { Value::from("This deployment's vault is a demo whose outbound image is a placeholder: it can never release, even after G2 exists. A new vault version is needed.") },
        "what_happens_next": "Once G2 runs, the vault records exit roots; after the release delay, anyone (you or a relayer, paid the fee you chose) proves your exit and the vault pays the recipient you fixed when burning.",
    })
}

fn options(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("options are not JSON: {e}"))?;
    if !v.is_object() {
        return Err("options must be a JSON object".to_owned());
    }
    Ok(v)
}

impl LezBridgeModule for Bridge {
    fn config(&mut self) -> String {
        let cli = self.cli();
        let (cli_ok, why) = match cli.as_ref().map_err(|e| e.clone()).and_then(|c| c.check()) {
            Ok(()) => (true, Value::Null),
            Err(e) => (false, Value::from(e)),
        };
        let mut v = self.cfg.to_json();
        v["module_version"] = Value::from(MODULE_VERSION);
        v["data_dir"] = Value::from(self.data.display().to_string());
        v["fixtures"] = self.fixtures.as_ref().map_or(Value::Null, |f| Value::from(f.display().to_string()));
        v["ready"] = json!({ "cli": cli_ok, "why": why, "json_cli": cli.ok().filter(|_| cli_ok).map(|c| c.json_mode()) });
        // What this module can do, for a host that must tell an older one apart ("Method not
        // found" is the other way to learn it), and the words a host shows before it asks.
        v["features"] = json!(FEATURES);
        v["scaffold"] = Value::from(SCAFFOLD_LINE);
        v["build"] = build_info();
        // `release`: the words for a vault that can't pay out (v3), or null.
        let d = &self.cfg.deployment;
        let release_notice = if d.release_note.is_empty() { Value::Null } else { json!({ "text": d.release_note, "detail": d.release_detail }) };
        v["notices"] = json!({ "not_private": relayed::not_private_notice(None), "release": release_notice });
        // The deployment names its LEZ network ("LEZ 0.4 · bridge test network"); a local
        // developer sequencer is said as such.
        let d = &self.cfg.deployment;
        v["lez_network"] = Value::from(if !d.lez_rpc.is_empty() && !config::is_loopback(&d.lez_rpc) {
            d.network.clone()
        } else if config::is_loopback(&self.cfg.sequencer) {
            "local sequencer".to_owned()
        } else {
            "remote sequencer".to_owned()
        });
        reply(Ok(v))
    }

    fn set_eth_rpc(&mut self, chain_id: String, url: String) -> String {
        reply((|| {
            let c: u64 = chain_id.trim().parse().map_err(|_| "chain id must be a number, e.g. 11155111")?;
            let u = config::check_url(&url, "Ethereum RPC")?;
            self.cfg.eth_rpc.retain(|(x, _)| *x != c);
            self.cfg.eth_rpc.push((c, u.clone()));
            self.registry_cache = None;
            self.save()?;
            // Say at once whether it answers, and for which chain.
            let check = JsonRpc::new(&u).chain_id();
            Ok(json!({ "chain_id": c, "url": u, "answers": check.is_ok(), "chain_matches": check.as_ref().ok().map(|x| *x == c), "problem": check.err() }))
        })())
    }

    fn set_eth_proof_rpc(&mut self, chain_id: String, url: String) -> String {
        reply((|| {
            let c: u64 = chain_id.trim().parse().map_err(|_| "chain id must be a number")?;
            let u = config::check_url(&url, "Ethereum RPC")?;
            self.cfg.eth_proof_rpc.retain(|(x, _)| *x != c);
            self.cfg.eth_proof_rpc.push((c, u.clone()));
            self.save()?;
            Ok(json!({ "chain_id": c, "url": u }))
        })())
    }

    fn set_eth_source(&mut self, source: String) -> String {
        reply((|| {
            let s = source.trim();
            if !matches!(s, "rpc" | "logos_module") {
                return Err("the Ethereum source is rpc or logos_module".to_owned());
            }
            self.cfg.eth_source = s.to_owned();
            self.registry_cache = None;
            self.save()?;
            Ok(json!({ "eth_source": s, "note": if s == "logos_module" { Value::from(eth::LOGOS_ETH_UNAVAILABLE) } else { Value::Null } }))
        })())
    }

    fn set_sequencer(&mut self, url: String) -> String {
        reply((|| {
            let u = config::check_url(&url, "sequencer")?;
            self.cfg.sequencer = u.clone();
            self.registry_cache = None;
            let _ = std::fs::remove_file(self.data.join("holdings.json"));
            self.save()?;
            Ok(json!({ "sequencer": u, "loopback": config::is_loopback(&u),
                       "note": if config::is_loopback(&u) { Value::Null } else { Value::from("reads that need the wallet stay off until the CLI accepts a non-local sequencer") } }))
        })())
    }

    fn set_cli(&mut self, program: String, wallet_dir: String) -> String {
        reply((|| {
            let p = PathBuf::from(program.trim());
            let w = PathBuf::from(wallet_dir.trim());
            if !p.is_absolute() || !w.is_absolute() {
                return Err("give absolute paths for the program and the wallet folder".to_owned());
            }
            if !cli::is_executable(&p) {
                return Err(format!("there is no program at {}", p.display()));
            }
            if !w.join("storage.json").exists() {
                return Err(format!("{} holds no LEZ wallet (no storage.json)", w.display()));
            }
            self.cfg.cli = Some(p.display().to_string());
            self.cfg.wallet_dir = Some(w.display().to_string());
            self.registry_cache = None;
            let _ = std::fs::remove_file(self.data.join("holdings.json"));
            self.save()?;
            Ok(json!({ "cli": self.cfg.cli, "wallet_dir": self.cfg.wallet_dir }))
        })())
    }

    fn use_deployment(&mut self, source: String) -> String {
        reply((|| {
            let s = source.trim();
            let d = if let Some(d) = config::builtin(s) {
                d
            } else if s.starts_with('{') {
                Deployment::from_json(&serde_json::from_str(s).map_err(|e| format!("not JSON: {e}"))?)?
            } else {
                Deployment::from_json(&util::read_json(Path::new(s)).ok_or_else(|| format!("{s} is not a readable deployment JSON file"))?)?
            };
            self.cfg.deployment = d;
            self.registry_cache = None;
            self.save()?;
            Ok(self.cfg.deployment.to_json())
        })())
    }

    fn set_fixtures(&mut self, dir: String) -> String {
        reply((|| {
            let d = dir.trim();
            let path = self.data.join("fixtures.json");
            if d.is_empty() {
                self.fixtures = None;
                let _ = std::fs::remove_file(&path);
                return Ok(json!({ "fixtures": null }));
            }
            let p = PathBuf::from(d);
            if !p.join("registry.json").exists() {
                return Err(format!("{d} holds no recorded replies (registry.json)"));
            }
            util::write_private(&path, json!({ "dir": d }).to_string().as_bytes()).map_err(|e| e.to_string())?;
            self.fixtures = Some(p);
            Ok(json!({ "fixtures": d }))
        })())
    }

    fn registry(&mut self) -> String {
        if let Some(f) = self.fixture("registry") {
            return f;
        }
        reply(self.registry_now())
    }

    fn plan_deposit(&mut self, token: String, amount: String) -> String {
        if let Some(f) = self.fixture("plan_deposit") {
            return f;
        }
        reply((|| {
            let (t, row) = self.token_info(&token)?;
            let value = util::parse_amount(&amount, t.decimals)?;
            let denoms: Vec<u128> = row["denoms"].as_array().unwrap().iter().filter_map(|x| units(x["value"].as_str()?).ok()).collect();
            let fees: Vec<u128> = row["denoms"].as_array().unwrap().iter().map(|x| units(x["deposit_fee"].as_str().unwrap_or("0")).unwrap_or(0)).collect();
            let sets: Vec<u64> = row["denoms"].as_array().unwrap().iter().map(|x| x["deposits"].as_u64().unwrap_or(0)).collect();
            let mut p = plan::deposit_plan(&t.symbol, t.decimals, value, &denoms, &fees, &sets, &plan::random_jitter(plan::MAX_PIECES as usize))?;
            let frozen = row["freeze"]["eth_deposits"] == true || self.registry_now()?["vault"]["deposits_frozen"] == true;
            if frozen {
                p["blocked"] = Value::from("Deposits of this token are frozen on Ethereum right now; the vault would refuse them.");
            }
            p["privacy"] = privacy::report(&json!({ "flow": "deposit" }))?;
            Ok(p)
        })())
    }

    fn make_deposit_request(&mut self, token: String, denom_index: String, account: String) -> String {
        if let Some(f) = self.fixture("make_deposit_request") {
            return f;
        }
        if self.names_mode() {
            return reply(self.names_deposit_request(&token, &denom_index, &account));
        }
        reply(self.deposit_request_now(&token, &denom_index, &account, false))
    }

    fn make_public_deposit_request(&mut self, token: String, denom_index: String, account: String, confirm_weaker: String) -> String {
        if let Some(f) = self.fixture("make_deposit_request") {
            return f;
        }
        if confirm_weaker.trim() != "true" {
            return reply(Err(privacy::require_confirmation(&json!({ "flow": "deposit", "public_account": true })).unwrap_err()));
        }
        if self.names_mode() {
            // A public account the Names Wallet gave this app: its values come from that account's
            // key, its claim is the relayed public claim (no proof).
            let written = format!("Public/{}", account.trim().trim_start_matches("Public/").trim_start_matches("Private/"));
            return reply(self.names_deposit_request(&token, &denom_index, &written));
        }
        reply(self.deposit_request_now(&token, &denom_index, &account, true))
    }


    fn import_deposit(&mut self, deposit_json: String) -> String {
        reply((|| {
            let v = options(&deposit_json)?;
            let secret = v["secret"].as_str().ok_or("give the deposit's secret")?;
            util::hex32(secret)?;
            let (t, row) = self.token_info(v["token"].as_str().ok_or("give the token")?)?;
            let i = v["denom_index"].as_u64().or_else(|| v["denom_index"].as_str().and_then(|s| s.parse().ok())).ok_or("give denom_index")? as usize;
            let denom = units(row["denoms"][i]["value"].as_str().ok_or("no such denomination")?)?;
            let acct = v["account"].as_str().ok_or("give the LEZ account")?.trim_start_matches("Private/").trim_start_matches("Public/").to_owned();
            util::lez_account(&acct)?;
            let cli = self.wallet_cli()?;
            let kind = if self.names_mode() {
                if self.granted().map(|(_, a)| a.iter().any(|x| x["account"] == acct.as_str() && x["kind"] == "private")).unwrap_or(false) { "private".to_owned() } else { "unknown".to_owned() }
            } else {
                cli.accounts()
                    .ok()
                    .and_then(|a| a["accounts"].as_array()?.iter().find(|x| x["account"] == acct.as_str()).and_then(|x| x["kind"].as_str().map(str::to_owned)))
                    .unwrap_or_else(|| "unknown".to_owned())
            };
            let c = cli.commitment(secret, &acct, &eth::addr_hex(&t.eth_token), &denom.to_string(), &self.deployment().cfg_id)?;
            let d = store::new_deposit(&eth::addr_hex(&t.eth_token), &t.symbol, i as u32, denom, &acct, &kind, &c, secret, "imported");
            self.store().put_deposit(d.clone())?;
            Ok(public_view(&d))
        })())
    }

    fn deposits(&mut self) -> String {
        if let Some(f) = self.fixture("deposits") {
            return f;
        }
        reply(self.deposits_now())
    }

    fn claim_plan(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("claim_plan") {
            return f;
        }
        if self.names_mode() {
            return reply(self.names_claim_plan(id.trim()));
        }
        reply(self.claim_plan_now(id.trim()))
    }

    fn claim(&mut self, id: String, options_json: String) -> String {
        if let Some(f) = self.fixture("claim") {
            return f;
        }
        if self.names_mode() {
            return reply(options(&options_json).and_then(|o| self.names_claim(id.trim(), &o)));
        }
        reply(options(&options_json).and_then(|o| self.claim_now(id.trim(), &o)))
    }

    fn jobs(&mut self) -> String {
        if let Some(f) = self.fixture("jobs") {
            return f;
        }
        reply(Ok(json!({ "jobs": self.all_jobs() })))
    }

    fn job(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("job") {
            return f;
        }
        if id.trim().starts_with("wr-") {
            return reply(self.wallet_job(id.trim()));
        }
        reply(self.runner().state(id.trim()))
    }

    fn pause_job(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("pause_job") {
            return f;
        }
        if id.trim().starts_with("wr-") {
            return reply(Err("your Names Wallet runs this; pausing it is up to the Names app".to_owned()));
        }
        reply(self.runner().pause(id.trim()))
    }

    fn resume_job(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("resume_job") {
            return f;
        }
        if id.trim().starts_with("wr-") {
            return reply(Err("your Names Wallet runs this; resuming it is up to the Names app".to_owned()));
        }
        reply(self.runner().resume(id.trim()))
    }

    fn cancel_job(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("cancel_job") {
            return f;
        }
        if id.trim().starts_with("wr-") {
            return reply(self.cancel_wallet_job(id.trim()));
        }
        reply(self.runner().cancel(id.trim()))
    }

    fn holdings(&mut self) -> String {
        if let Some(f) = self.fixture("holdings") {
            return f;
        }
        if self.names_mode() {
            return reply(Ok(json!({
                "accounts": [], "totals": [], "wallet": "names",
                "note": "Your bridged tokens are in your Names Wallet: the Names app lists them under Bridged tokens once you approve them as a token source.",
            })));
        }
        reply(self.holdings_now())
    }

    fn accounts(&mut self) -> String {
        if let Some(f) = self.fixture("accounts") {
            return f;
        }
        if self.names_mode() {
            return reply((|| {
                let (connected, list) = self.granted()?;
                let list: Vec<Value> = list.into_iter().map(|a| json!({ "account": a["account"], "kind": a["kind"], "path": "", "label": null })).collect();
                Ok(json!({ "accounts": list, "connected": connected, "wallet": "names",
                           "suggested": list.iter().find(|x| x["kind"] == "private").map(|x| x["account"].clone()) }))
            })());
        }
        reply((|| {
            let a = self.wallet_cli()?.accounts()?;
            let mut list = a["accounts"].as_array().cloned().unwrap_or_default();
            // Private first: the private path is the pre-selected one (Usability #6).
            list.sort_by_key(|x| if x["kind"] == "private" { 0 } else { 1 });
            Ok(json!({ "accounts": list, "suggested": list.iter().find(|x| x["kind"] == "private").map(|x| x["account"].clone()) }))
        })())
    }

    fn plan_redeem(&mut self, token: String, amount: String, eth_to: String, relay_fee_index: String) -> String {
        if let Some(f) = self.fixture("plan_redeem") {
            return f;
        }
        if self.names_mode() {
            return reply(self.names_plan_redeem(&token, &amount, &eth_to, &relay_fee_index));
        }
        reply(self.plan_redeem_now(&token, &amount, &eth_to, &relay_fee_index))
    }

    fn redeem(&mut self, plan_id: String, options_json: String) -> String {
        if let Some(f) = self.fixture("redeem") {
            return f;
        }
        if self.names_mode() {
            return reply(options(&options_json).and_then(|o| self.names_redeem(plan_id.trim(), &o)));
        }
        reply(options(&options_json).and_then(|o| self.redeem_now(plan_id.trim(), &o)))
    }

    fn exits(&mut self) -> String {
        if let Some(f) = self.fixture("exits") {
            return f;
        }
        reply(Ok(self.exits_now()))
    }

    fn release(&mut self, exit_id: String, relayer_json: String) -> String {
        if let Some(f) = self.fixture("release") {
            return f;
        }
        let possible = self.registry_cache.as_ref().map(|(_, r)| r["vault"]["exits_possible"] == true).unwrap_or(false);
        let mut v = release_unavailable(possible, &self.cfg.deployment);
        v["exit_id"] = Value::from(exit_id.trim());
        v["relayer"] = options(&relayer_json).unwrap_or(Value::Null);
        reply(Ok(v))
    }

    fn recover(&mut self, account: String, gap_limit: String) -> String {
        if let Some(f) = self.fixture("recover") {
            return f;
        }
        reply((|| {
            let acct = account.trim().trim_start_matches("Private/").trim_start_matches("Public/").to_owned();
            let gap: u64 = gap_limit.trim().parse().map_err(|_| "gap limit must be a number, e.g. 20")?;
            if !(1..=1000).contains(&gap) {
                return Err("gap limit must be between 1 and 1000".to_owned());
            }
            if self.names_mode() {
                return self.names_recover(account.trim(), gap);
            }
            util::lez_account(&acct)?;
            let deposits = self.deposits_now()?;
            let found: Vec<Value> = deposits["deposits"].as_array().cloned().unwrap_or_default().into_iter().filter(|d| d["account"] == acct.as_str()).collect();
            Ok(json!({
                "account": acct,
                "seed_recovery": { "available": false, "reason": "Recovering from the wallet's seed needs the CLI to derive deposit and exit secrets from it (CLI-CONTRACT.md §2.5, §5). Until then, only positions whose secrets are on this computer can be found." },
                "gap_limit": gap,
                "found_on_this_computer": found,
                "scanned": "the vault's Deposit events, matched against the secrets stored on this computer",
            }))
        })())
    }

    fn privacy_report(&mut self, choices_json: String) -> String {
        reply(options(&choices_json).and_then(|c| privacy::report(&c)))
    }

    fn admin_status(&mut self) -> String {
        if let Some(f) = self.fixture("admin_status") {
            return f;
        }
        reply(self.admin_status_now())
    }

    fn admin_action(&mut self, action_json: String) -> String {
        if let Some(f) = self.fixture("admin_action") {
            return f;
        }
        reply(options(&action_json).and_then(|a| self.admin_action_now(&a)))
    }

    fn wallet_status(&mut self) -> String {
        if let Some(f) = self.fixture("wallet_status") {
            return f;
        }
        if !self.names_mode() {
            return reply(Ok(json!({ "mode": "cli", "wallet_dir": self.cfg.wallet_dir, "needs": null })));
        }
        reply(self.names_status())
    }

    fn wallet_connect(&mut self) -> String {
        if let Some(f) = self.fixture("wallet_connect") {
            return f;
        }
        reply(wallet::names("request_access", json!([wallet::ACCESS_REASON])))
    }

    fn wallet_request(&mut self, request_id: String) -> String {
        if let Some(f) = self.fixture("wallet_request") {
            return f;
        }
        reply((|| {
            // Polled by the screen: asked once, the next poll is the retry.
            let r = wallet::names_once("request_status", json!([request_id.trim()])).map_err(|f| f.text())?;
            let failed_check = matches!(r["check"].as_str(), Some("failed" | "refused"));
            Ok(json!({
                "request": r["request"], "kind": r["kind"], "state": r["state"], "check": r["check"], "phase": r["phase"], "tx": r["tx"],
                "accounts": r["accounts"],
                "error": if failed_check { Value::from("The Names Wallet's check stopped this call. Open the Names app to see why.") } else { r["error"].clone() },
            }))
        })())
    }

    fn relayed_claim_check(&mut self, claim: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_check") {
            return f;
        }
        reply(Ok(self.relayed_check_now(&claim)))
    }

    fn relayed_claim_check_with(&mut self, claim: String, options_json: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_check") {
            return f;
        }
        reply(options(&options_json).map(|o| self.relayed_check_opts(&claim, &o)))
    }

    fn relayed_claim(&mut self, check_id: String, options_json: String) -> String {
        if let Some(f) = self.fixture("relayed_claim") {
            return f;
        }
        reply(options(&options_json).and_then(|o| self.relayed_claim_now(check_id.trim(), &o)))
    }

    fn receive_status(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("receive_status") {
            return f;
        }
        reply(self.receive_status_now(id.trim()))
    }

    fn receives(&mut self) -> String {
        if let Some(f) = self.fixture("receives") {
            return f;
        }
        reply(Ok(self.receives_now()))
    }

    fn receive_request(&mut self, id: String) -> String {
        if let Some(f) = self.fixture("receive_request") {
            return f;
        }
        reply(self.receive_request_now(id.trim()))
    }

    fn relayed_claim_check_deposit(&mut self, id: String, options_json: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_check") {
            return f;
        }
        reply(options(&options_json).map(|o| self.relayed_check_deposit_now(id.trim(), &o)))
    }

    fn relayed_claim_build(&mut self, check_id: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_build") {
            return f;
        }
        reply(self.relayed_build_now(check_id.trim()))
    }

    fn relayed_claim_submit(&mut self, check_id: String, transaction: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_submit") {
            return f;
        }
        reply(self.relayed_submit_now(check_id.trim(), &transaction))
    }

    fn relayed_claim_status(&mut self, job_id: String) -> String {
        if let Some(f) = self.fixture("relayed_claim_status") {
            return f;
        }
        reply(self.relayed_status_now(job_id.trim()))
    }

    fn relayed_claims(&mut self) -> String {
        if let Some(f) = self.fixture("relayed_claims") {
            return f;
        }
        reply(Ok(Value::from(self.relayed_jobs())))
    }

    fn relayed_claim_cancel(&mut self, job_id: String) -> String {
        reply(self.relayed_cancel_now(job_id.trim()))
    }

    fn relayed_claim_forget(&mut self, check_id: String) -> String {
        reply(Ok(self.relayed_forget(check_id.trim())))
    }

    fn set_wallet_mode(&mut self, mode: String) -> String {
        reply((|| {
            let m = mode.trim();
            if !config::WALLET_MODES.contains(&m) {
                return Err("the wallet is \"names\" (the Names Wallet) or \"cli\" (the lez-bridge CLI's own wallet folder)".to_owned());
            }
            self.cfg.wallet = m.to_owned();
            self.registry_cache = None;
            self.save()?;
            Ok(json!({ "wallet": m }))
        })())
    }
}

#[cfg(not(test))]
#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Bridge>();
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_names;
#[cfg(test)]
mod tests_relayed;
#[cfg(test)]
mod tests_scaffold;
