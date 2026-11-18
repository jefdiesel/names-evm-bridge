//! The module's settings, kept in `<data>/config.json` (0600), and the known deployments.

use crate::util::{eth_address, lez_account, read_json, write_private};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The local LEZ 0.4 sequencer the v2 demo runs on (the test machine's `seq2`).
pub const DEFAULT_SEQUENCER: &str = "http://127.0.0.1:3250";
/// Sepolia: reads and logs over the vault's whole life. PublicNode pruned `eth_getLogs` to about
/// the last 10,000 blocks by 2026-11-14 ("pruned history unavailable" for the vault's first
/// deposits); Tenderly's public gateway answered the vault's whole range in one call that day.
pub const DEFAULT_SEPOLIA_RPC: &str = "https://sepolia.gateway.tenderly.co";
/// Tried after the chosen Sepolia RPC when it rate-limits (Tenderly answers HTTP 429 to bursts)
/// or can't serve a read: PublicNode (blocks and calls; its logs are pruned) and thirdweb (logs
/// in spans of 1,000 blocks; `deposit_events` shrinks its span to match).
pub const SEPOLIA_FALLBACK_RPCS: [&str; 3] = ["https://sepolia.gateway.tenderly.co", "https://ethereum-sepolia-rpc.publicnode.com", "https://sepolia.rpc.thirdweb.com"];
/// Sepolia `eth_getProof` at blocks well behind the head (vault/DEPLOYMENTS.md: only thirdweb served it).
pub const DEFAULT_SEPOLIA_PROOF_RPC: &str = "https://sepolia.rpc.thirdweb.com";

/// Proving costs measured on the Apple M4 (16 GB), 4 threads, CPU prover:
/// private attested claim v2 670.9 s at 9,403 MB peak footprint (LEZ block 4998).
pub const CLAIM_PROOF_ESTIMATE_S: u64 = 671;
pub const CLAIM_PROOF_PEAK_MB: u64 = 9_403;
/// Private stand-in burn (run/private5, block 884): 601-771 s, 9,485-9,550 MB.
pub const BURN_PROOF_ESTIMATE_S: u64 = 690;
pub const BURN_PROOF_PEAK_MB: u64 = 9_550;
pub const MIN_RAM_GB: u64 = 16;
/// The fee cap a burn (or an unsponsored claim) declares to the Names Wallet (`declared.max_fee`):
/// the wallet refuses a call whose fee can reach more. The v2 claim's fee was at most 9,300,135
/// (BRIDGE-SDK.md §6), so this leaves room for twice that.
pub const DEFAULT_MAX_FEE: u128 = 20_000_000;

/// Where the person's accounts and keys are.
/// - `names`: the Names Wallet (the `names` module). This module asks it for `app_secret`
///   values and proposes private calls; it never holds a key or proves.
/// - `cli`: the `lez-bridge` CLI's own wallet folder (developer setups; secrets made here).
pub const WALLET_MODES: [&str; 2] = ["names", "cli"];

#[derive(Clone, Debug)]
pub struct Token {
    pub eth_token: [u8; 20],
    pub symbol: String,
    pub decimals: u8,
    pub definition: String,
    /// The denominations, when the deployment lists them (needed for its bridge code); empty
    /// otherwise (the registry reads them from the chains).
    pub denoms: Vec<u128>,
}

#[derive(Clone, Debug)]
pub struct Deployment {
    pub name: String,
    pub label: String,
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub vault_create_block: u64,
    pub cfg_id: String,
    pub bridge_program: String,
    pub token_program: String,
    pub attest_program: String,
    pub attest_instance: String,
    pub probe_program: String,
    pub tokens: Vec<Token>,
    pub notes: Vec<String>,
    /// How the LEZ side is named beside every action (site rule: "LEZ 0.4 · bridge test network").
    pub network: String,
    /// The LEZ JSON-RPC endpoint the Names Wallet claim reads (any http(s); it only reads).
    pub lez_rpc: String,
    /// The claim relay: `rpc` (the `lez_rpc` endpoint's `bridge_relayInfo`/`bridge_relayClaim`)
    /// or empty (no relay: no public claim from the Names Wallet).
    pub relay: String,
    /// The Sepolia deposit desk in front of the vault (the bridge fee), or empty.
    pub desk: String,
    /// How this vault releases: `never` (it can't pay out: a placeholder outbound image, or v3's
    /// outbound state deployed at zero), `operator` (an operator-attested verifier: the operator's
    /// attestor signs releases; no deployment uses it today), `proof` (the outbound zk proof, G2).
    pub release: String,
    /// For a `never` vault: the one line the apps show for why nothing is paid out, and the
    /// detail behind it (empty: the generic placeholder-image words).
    pub release_note: String,
    pub release_detail: String,
    /// The bridge code's other fields (`bridge_code.rs`): `{label, gate, lez_chain_id, zone_id,
    /// config_account, min_release_delay, genesis_lez_block, genesis_g2_state, update_image,
    /// release_image, settle}`; `added: true` for a deployment added from a code. Null for a
    /// deployment without a code (v3, v2).
    pub bridge: Value,
}

/// The deployments this module knows, from ONE file (`rust-lib/deployments.json`): the bridge and
/// token program ids, the LEZ endpoint and relay, the Sepolia vault and desk. A new bridge
/// instance is a new entry there, never a constant here.
pub const DEPLOYMENTS: &str = include_str!("../deployments.json");

/// A built-in deployment by name.
pub fn builtin(name: &str) -> Option<Deployment> {
    let all: Value = serde_json::from_str(DEPLOYMENTS).ok()?;
    all["deployments"].as_array()?.iter().find(|d| d["name"] == name).and_then(|d| Deployment::from_json(d).ok())
}

/// A deployment's short name for people: "bridge v4", "bridge v3", "bridge v2" (the built-ins), else
/// its own name.
pub fn plain_label(name: &str) -> String {
    match name {
        "bridge-test-network" => "bridge v2".to_owned(),
        n => n.strip_prefix("bridge-test-network-").map_or_else(|| n.to_owned(), |v| format!("bridge {v}")),
    }
}

/// The names of the built-in deployments.
pub fn builtin_names() -> Vec<String> {
    let all: Value = serde_json::from_str(DEPLOYMENTS).unwrap_or_default();
    all["deployments"].as_array().map(|l| l.iter().filter_map(|d| d["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// The deployment a fresh install uses (`default` in deployments.json).
pub fn default_deployment() -> Deployment {
    let all: Value = serde_json::from_str(DEPLOYMENTS).expect("deployments.json is JSON");
    builtin(all["default"].as_str().unwrap_or_default()).expect("deployments.json names a default it lists")
}

/// The Sepolia demo v2 pairing on the old local sequencer (kept for the CLI-wallet flows' tests).
pub fn sepolia_demo_v2() -> Deployment {
    builtin("sepolia-demo-v2").expect("deployments.json lists sepolia-demo-v2")
}

impl Deployment {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "label": self.label,
            "chain_id": self.chain_id,
            "vault": crate::util::checksum_address(&self.vault),
            "vault_create_block": self.vault_create_block,
            "cfg_id": self.cfg_id,
            "bridge_program": self.bridge_program,
            "token_program": self.token_program,
            "attest_program": self.attest_program,
            "attest_instance": self.attest_instance,
            "probe_program": self.probe_program,
            "network": self.network,
            "lez_rpc": self.lez_rpc,
            "relay": self.relay,
            "desk": self.desk,
            "release": self.release,
            "release_note": self.release_note,
            "release_detail": self.release_detail,
            "tokens": self.tokens.iter().map(|t| {
                let mut x = json!({ "eth_token": crate::eth::addr_hex(&t.eth_token), "symbol": t.symbol, "decimals": t.decimals, "definition": t.definition });
                if !t.denoms.is_empty() {
                    x["denoms"] = json!(t.denoms.iter().map(|d| d.to_string()).collect::<Vec<_>>());
                }
                x
            }).collect::<Vec<_>>(),
            "notes": self.notes,
            "bridge": self.bridge,
        })
    }

    pub fn from_json(v: &Value) -> Result<Deployment, String> {
        let s = |k: &str| v[k].as_str().map(str::to_owned).ok_or_else(|| format!("the deployment has no \"{k}\""));
        let n = |k: &str| v[k].as_u64().ok_or_else(|| format!("the deployment has no number \"{k}\""));
        let tokens = v["tokens"]
            .as_array()
            .ok_or("the deployment lists no tokens")?
            .iter()
            .map(|t| {
                Ok(Token {
                    eth_token: eth_address(t["eth_token"].as_str().unwrap_or_default())?,
                    symbol: t["symbol"].as_str().unwrap_or("?").chars().take(16).collect(),
                    decimals: t["decimals"].as_u64().filter(|d| *d <= 36).ok_or("a token without decimals")? as u8,
                    definition: t["definition"].as_str().unwrap_or_default().to_owned(),
                    denoms: match t["denoms"].as_array() {
                        Some(l) => l.iter().map(|x| x.as_str().ok_or("a denomination is text").and_then(|x| x.parse::<u128>().map_err(|_| "a denomination is a number"))).collect::<Result<Vec<_>, _>>()?,
                        None => Vec::new(),
                    },
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let d = Deployment {
            name: s("name")?,
            label: v["label"].as_str().unwrap_or("custom deployment").to_owned(),
            chain_id: n("chain_id")?,
            vault: eth_address(&s("vault")?)?,
            vault_create_block: n("vault_create_block")?,
            cfg_id: s("cfg_id")?,
            bridge_program: s("bridge_program")?,
            token_program: s("token_program")?,
            attest_program: s("attest_program")?,
            attest_instance: s("attest_instance")?,
            probe_program: v["probe_program"].as_str().unwrap_or_default().to_owned(),
            tokens,
            notes: v["notes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
            network: v["network"].as_str().unwrap_or("LEZ").chars().take(60).collect(),
            lez_rpc: v["lez_rpc"].as_str().unwrap_or_default().trim().to_owned(),
            relay: v["relay"].as_str().unwrap_or_default().trim().to_owned(),
            desk: v["desk"].as_str().unwrap_or_default().trim().to_owned(),
            release: v["release"].as_str().unwrap_or("proof").trim().to_owned(),
            release_note: v["release_note"].as_str().unwrap_or_default().trim().to_owned(),
            release_detail: v["release_detail"].as_str().unwrap_or_default().trim().to_owned(),
            bridge: if v["bridge"].is_object() { v["bridge"].clone() } else { Value::Null },
        };
        for id in [&d.bridge_program, &d.token_program] {
            lez_account(id)?;
        }
        crate::util::hex32(&d.cfg_id)?;
        // A stand-in deployment (roots recorded by an attestor) has no attestation instance.
        if !d.attest_program.is_empty() || !d.attest_instance.is_empty() {
            lez_account(&d.attest_program)?;
            crate::util::hex32(&d.attest_instance)?;
        }
        if !d.lez_rpc.is_empty() {
            check_url(&d.lez_rpc, "LEZ endpoint")?;
        }
        if !matches!(d.relay.as_str(), "" | "rpc") {
            return Err("the deployment's relay is \"rpc\" (the LEZ endpoint's relay methods) or empty".to_owned());
        }
        if !d.desk.is_empty() {
            eth_address(&d.desk)?;
        }
        if !matches!(d.release.as_str(), "never" | "operator" | "proof") {
            return Err("the deployment's release is never, operator or proof".to_owned());
        }
        Ok(d)
    }

    pub fn token(&self, text: &str) -> Result<&Token, String> {
        let t = text.trim();
        self.tokens
            .iter()
            .find(|k| k.symbol.eq_ignore_ascii_case(t) || eth_address(t).is_ok_and(|a| a == k.eth_token))
            .ok_or_else(|| format!("{t} is not a token of this bridge ({})", self.tokens.iter().map(|k| k.symbol.as_str()).collect::<Vec<_>>().join(", ")))
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub sequencer: String,
    /// Ethereum JSON-RPC per chain id, as text keys.
    pub eth_rpc: Vec<(u64, String)>,
    /// An endpoint that serves `eth_getProof` well behind the head.
    pub eth_proof_rpc: Vec<(u64, String)>,
    /// "rpc" or "logos_module".
    pub eth_source: String,
    pub deployment: Deployment,
    pub cli: Option<String>,
    pub wallet_dir: Option<String>,
    /// Proving is off until the person turns it on, with the guard that keeps the machine safe.
    pub proving_enabled: bool,
    pub prover_wrapper: Vec<String>,
    pub tmp_dir: Option<String>,
    /// `names` (default) or `cli` (see [`WALLET_MODES`]).
    pub wallet: String,
    /// The release prover's argv (the helper, and any wrapper in front of it); empty: none set.
    pub release_prover: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sequencer: DEFAULT_SEQUENCER.into(),
            eth_rpc: vec![(11_155_111, DEFAULT_SEPOLIA_RPC.into())],
            eth_proof_rpc: vec![(11_155_111, DEFAULT_SEPOLIA_PROOF_RPC.into())],
            eth_source: "rpc".into(),
            deployment: default_deployment(),
            cli: None,
            wallet_dir: None,
            proving_enabled: false,
            prover_wrapper: Vec::new(),
            tmp_dir: None,
            wallet: "names".into(),
            release_prover: Vec::new(),
        }
    }
}

impl Config {
    pub fn rpc_for(&self, chain_id: u64) -> Option<&str> {
        self.eth_rpc.iter().find(|(c, _)| *c == chain_id).map(|(_, u)| u.as_str())
    }

    pub fn proof_rpc_for(&self, chain_id: u64) -> Option<&str> {
        self.eth_proof_rpc.iter().find(|(c, _)| *c == chain_id).map(|(_, u)| u.as_str()).or_else(|| self.rpc_for(chain_id))
    }

    pub fn to_json(&self) -> Value {
        let rpc = |list: &Vec<(u64, String)>| Value::Object(list.iter().map(|(c, u)| (c.to_string(), Value::from(u.clone()))).collect());
        json!({
            "sequencer": self.sequencer,
            "eth_rpc": rpc(&self.eth_rpc),
            "eth_proof_rpc": rpc(&self.eth_proof_rpc),
            "eth_source": self.eth_source,
            "deployment": self.deployment.to_json(),
            "cli": self.cli,
            "wallet_dir": self.wallet_dir,
            "proving": { "enabled": self.proving_enabled, "wrapper": self.prover_wrapper },
            "tmp_dir": self.tmp_dir,
            "wallet": self.wallet,
            "release_prover": self.release_prover,
        })
    }

    pub fn from_json(v: &Value) -> Config {
        let mut c = Config::default();
        if let Some(s) = v["sequencer"].as_str() {
            c.sequencer = s.to_owned();
        }
        let rpc = |x: &Value| -> Vec<(u64, String)> {
            x.as_object()
                .map(|o| o.iter().filter_map(|(k, u)| Some((k.parse().ok()?, u.as_str()?.to_owned()))).collect())
                .unwrap_or_default()
        };
        let r = rpc(&v["eth_rpc"]);
        if !r.is_empty() {
            c.eth_rpc = r;
        }
        let r = rpc(&v["eth_proof_rpc"]);
        if !r.is_empty() {
            c.eth_proof_rpc = r;
        }
        if let Some(s) = v["eth_source"].as_str().filter(|s| matches!(*s, "rpc" | "logos_module")) {
            c.eth_source = s.to_owned();
        }
        if let Ok(d) = Deployment::from_json(&v["deployment"]) {
            // A built-in deployment is taken from this build's deployments.json by its name, not
            // from the copy an older build saved (which may lack the desk, the relay or a fixed
            // program id); a custom one is kept as saved. Matters most when the app that once
            // saved settings is no longer used (the bridge hosted inside the Names app).
            c.deployment = builtin(&d.name).unwrap_or(d);
        }
        c.cli = v["cli"].as_str().map(str::to_owned);
        c.wallet_dir = v["wallet_dir"].as_str().map(str::to_owned);
        c.proving_enabled = v["proving"]["enabled"].as_bool().unwrap_or(false);
        c.prover_wrapper = v["proving"]["wrapper"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        c.tmp_dir = v["tmp_dir"].as_str().map(str::to_owned);
        c.release_prover = v["release_prover"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        if let Some(w) = v["wallet"].as_str().filter(|w| WALLET_MODES.contains(w)) {
            c.wallet = w.to_owned();
        }
        c
    }

    pub fn names_wallet(&self) -> bool {
        self.wallet == "names"
    }

    pub fn load(dir: &Path) -> Config {
        read_json(&dir.join("config.json")).map(|v| Config::from_json(&v)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        write_private(&dir.join("config.json"), self.to_json().to_string().as_bytes()).map_err(|e| format!("cannot save the settings: {e}"))
    }
}

/// Where the module keeps its files: `LEZ_BRIDGE_MODULE_HOME`, else `~/.config/logos-lez-bridge`.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LEZ_BRIDGE_MODULE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".config").join("logos-lez-bridge")
}

/// A sequencer URL: http(s), with a host. Loopback is required for anything that opens a
/// wallet, because the CLI refuses any other address today (CLI-CONTRACT.md §0.2).
pub fn check_url(url: &str, what: &str) -> Result<String, String> {
    let u = url.trim();
    let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("https://")).ok_or_else(|| format!("the {what} must start with http:// or https://"))?;
    if rest.is_empty() || rest.starts_with('/') || rest.contains(char::is_whitespace) {
        return Err(format!("the {what} needs a host, like http://127.0.0.1:3250"));
    }
    Ok(u.trim_end_matches('/').to_owned())
}

pub fn is_loopback(url: &str) -> bool {
    let host = url.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or("");
    matches!(host, "127.0.0.1" | "localhost") || url.contains("://[::1]")
}

#[cfg(test)]
mod tests {

    /// Hosted inside the Names app the bridge app may never run: no settings file means v3 (the
    /// default), and a built-in deployment an older build saved is read from this build.
    #[test]
    fn no_settings_means_the_default_and_a_saved_builtin_is_refreshed() {
        let none = Config::from_json(&serde_json::json!({}));
        assert_eq!(none.deployment.name, default_deployment().name);
        let mut stale = builtin("bridge-test-network-v3").unwrap().to_json();
        stale["desk"] = serde_json::json!("");
        stale["relay"] = serde_json::json!("");
        let c = Config::from_json(&serde_json::json!({ "deployment": stale }));
        assert_eq!(c.deployment.desk, builtin("bridge-test-network-v3").unwrap().desk, "the saved copy's empty desk is not used");
        let mut custom = builtin("bridge-test-network-v3").unwrap().to_json();
        custom["name"] = serde_json::json!("my-local-chain");
        assert_eq!(Config::from_json(&serde_json::json!({ "deployment": custom })).deployment.name, "my-local-chain");
    }

    use super::*;

    #[test]
    fn every_built_in_deployment_loads_and_the_default_is_the_bridge_test_network() {
        let names = builtin_names();
        assert!(names.len() >= 2);
        for n in &names {
            let d = builtin(n).unwrap_or_else(|| panic!("{n} does not load"));
            assert_eq!(Deployment::from_json(&d.to_json()).unwrap().relay, d.relay);
        }
        // The default is v3, or v4 once public-demo/v4/apply-v4-entries.py --default made it so.
        let d = default_deployment();
        assert!(matches!(d.name.as_str(), "bridge-test-network-v3" | "bridge-test-network-v4" | "bridge-test-network-v5"), "{}", d.name);
        let d = builtin("bridge-test-network-v3").unwrap();
        // v3 can't pay out (its outbound state was deployed at zero, vault/DEPLOYMENTS.md): it
        // says so in words without "never", and no built-in deployment claims operator releases.
        assert_eq!(d.release, "never");
        assert!(d.release_note.starts_with("This test vault was set up with a wrong starting value") && d.release_detail.starts_with("Why: "));
        assert!(!d.release_note.to_lowercase().contains("never") && !d.release_note.to_lowercase().contains("ever"));
        assert_eq!(Deployment::from_json(&d.to_json()).unwrap().release_note, d.release_note);
        assert_eq!(builtin("bridge-test-network").unwrap().release, "never");
        for n in &names {
            let b = builtin(n).unwrap();
            assert_ne!(b.release, "operator", "{n}");
            assert!(!b.notes.iter().any(|x| x.to_lowercase().contains("operator-attested")), "{n}");
        }
        assert_eq!(d.network, "LEZ 0.4 · bridge test network");
        assert_eq!(d.relay, "rpc");
        assert!(d.lez_rpc.starts_with("https://"));
        assert!(!d.network.to_lowercase().contains("official"));
        assert!(Deployment::from_json(&json!({ "relay": "https://evil.example" })).is_err());
    }

    /// v4 (public-demo/V4-PLAN.md step C1), only once the apply script wrote it: releases by proof, said
    /// without "never", no placeholder left, and its own vault, bridge and cfgId.
    #[test]
    fn v4_when_listed_releases_by_proof_and_holds_no_placeholder() {
        assert!(!DEPLOYMENTS.contains("<<V4:"), "deployments.json holds a v4 placeholder");
        let Some(d) = builtin("bridge-test-network-v4") else { return };
        assert_eq!(d.release, "proof");
        // The default's words until v5 is the default (public-demo/v5/apply-v5-entries.py --default), then an earlier vault's.
        let want = if default_deployment().name == "bridge-test-network-v4" {
            "Withdrawals back to Ethereum are checked by proof on Ethereum. The first proofs are being set up; until then a burn waits."
        } else {
            "An earlier test vault: withdrawals work, with a one-hour wait."
        };
        assert_eq!(d.release_note, want);
        assert!(d.release_detail.contains("operator's word until a finality proof"));
        assert!(!d.release_note.to_lowercase().contains("never"));
        let v3 = builtin("bridge-test-network-v3").unwrap();
        assert_ne!(d.vault, v3.vault);
        assert_ne!(d.bridge_program, v3.bridge_program);
        assert_ne!(d.cfg_id, v3.cfg_id);
        assert_eq!(d.token_program, v3.token_program, "v4 reuses v3's token program");
    }

    /// v5 (public-demo/V5-PLAN.md step C1), only once the apply script wrote it: v4's design with no
    /// release delay, its own vault, bridge and cfgId, the same token program.
    #[test]
    fn v5_when_listed_releases_by_proof_and_holds_no_placeholder() {
        assert!(!DEPLOYMENTS.contains("<<V5:"), "deployments.json holds a v5 placeholder");
        let Some(d) = builtin("bridge-test-network-v5") else { return };
        assert_eq!(d.release, "proof");
        assert_eq!(d.release_note, "Withdrawals back to Ethereum are checked by proof on Ethereum.");
        assert!(d.release_detail.contains("operator's word until a finality proof") && !d.release_detail.contains("delay"));
        let v4 = builtin("bridge-test-network-v4").unwrap();
        assert_ne!(d.vault, v4.vault);
        assert_ne!(d.bridge_program, v4.bridge_program);
        assert_ne!(d.cfg_id, v4.cfg_id);
        assert_eq!(d.token_program, v4.token_program, "v5 reuses the v3/v4 token program");
    }

    #[test]
    fn the_built_in_deployment_survives_a_round_trip() {
        let d = sepolia_demo_v2();
        let back = Deployment::from_json(&d.to_json()).unwrap();
        assert_eq!(back.vault, d.vault);
        assert_eq!(back.tokens.len(), 2);
        assert_eq!(back.token("dusdc").unwrap().decimals, 6);
        assert_eq!(back.token("0x0000000000000000000000000000000000000000").unwrap().symbol, "ETH");
        assert!(back.token("DAI").is_err());
    }

    #[test]
    fn urls_are_checked() {
        assert_eq!(check_url("http://127.0.0.1:3250/", "sequencer").unwrap(), "http://127.0.0.1:3250");
        assert!(check_url("127.0.0.1:3250", "sequencer").is_err());
        assert!(check_url("http://", "sequencer").is_err());
        assert!(is_loopback("http://127.0.0.1:3250"));
        assert!(is_loopback("http://localhost:3250"));
        assert!(!is_loopback("https://testnet.lez.logos.co"));
    }

    #[test]
    fn settings_round_trip_and_proving_stays_off_by_default() {
        let c = Config::default();
        assert!(!c.proving_enabled);
        let back = Config::from_json(&c.to_json());
        assert_eq!(back.sequencer, DEFAULT_SEQUENCER);
        assert_eq!(back.rpc_for(11_155_111), Some(DEFAULT_SEPOLIA_RPC));
    }
}
