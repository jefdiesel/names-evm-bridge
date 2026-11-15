//! Module-level tests: the JSON each method returns, shaped from real data. The CLI is a
//! stand-in that prints output captured from the real binary (testdata/cli/); Ethereum is a
//! replay of answers recorded from Sepolia for the v2 vault (testdata/eth/replay.json, written
//! by the ignored `record_sepolia_replay` test against the live RPC).

use super::*;
use std::collections::HashMap;

const C_V100B: &str = "0x188c6f3a5a409e9ac4a5c69fb55213593b4b1774eadca84ede91ec44e300bacd";
const ANCHOR: &str = "0x5e81a69dd5b2dfd18a00c0021f7954b7971421221897b3ec752a4002561ddd7d";
const ROOT: &str = "0xbe1e080d85207b6b7832392daf38ce65347cabe7902bed16e3251ee03a21d3e8";

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("the module answers JSON")
}

fn ok(text: &str) -> Value {
    let v = parsed(text);
    assert_eq!(v["success"], true, "{v}");
    v["value"].clone()
}

fn err(text: &str) -> String {
    let v = parsed(text);
    assert_eq!(v["success"], false, "{v}");
    v["error"].as_str().unwrap().to_owned()
}

/// Ethereum as recorded.
struct Replay(HashMap<String, Value>);

impl Replay {
    fn load() -> Replay {
        let v: Value = serde_json::from_str(include_str!("../testdata/eth/replay.json")).expect("replay.json");
        Replay(v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
    fn get(&self, k: &str) -> Result<&Value, String> {
        self.0.get(k).ok_or_else(|| format!("not recorded: {k}"))
    }
}

fn block_of(v: &Value) -> eth::BlockInfo {
    eth::BlockInfo { number: v["number"].as_u64().unwrap(), hash: v["hash"].as_str().unwrap().to_owned(), timestamp: v["timestamp"].as_u64().unwrap() }
}

impl EthSource for Replay {
    fn describe(&self) -> String {
        "replay of Sepolia answers".into()
    }
    fn call(&self, _: &[u8; 20], data: &[u8]) -> Result<Vec<u8>, String> {
        util::hex_bytes(self.get(&format!("call:{}", hex0x(data)))?.as_str().unwrap())
    }
    fn block(&self, tag: &str) -> Result<eth::BlockInfo, String> {
        let key = format!("block:{tag}");
        match self.0.get(&key) {
            Some(b) => Ok(block_of(b)),
            // A deposit block's time: from the recorded logs' blocks.
            None => Err(format!("not recorded: {key}")),
        }
    }
    fn block_by_hash(&self, hash: &str) -> Result<Option<eth::BlockInfo>, String> {
        Ok(self.0.get(&format!("hash:{}", hash.to_ascii_lowercase())).filter(|b| !b.is_null()).map(block_of))
    }
    fn logs(&self, _: &[u8; 20], _: &[u8; 32], from: u64, to: u64) -> Result<Vec<Value>, String> {
        Ok(self.get("logs")?.as_array().unwrap().iter().filter(|l| (from..=to).contains(&u64::from_str_radix(l["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap())).cloned().collect())
    }
    fn code_len(&self, a: &[u8; 20]) -> Result<usize, String> {
        Ok(self.get(&format!("code:{}", eth::addr_hex(a)))?.as_u64().unwrap() as usize)
    }
    fn chain_id(&self) -> Result<u64, String> {
        Ok(11_155_111)
    }
}

/// The recorded Sepolia answers, for other test modules.
pub(crate) fn replay() -> Box<dyn EthSource + Send> {
    Box::new(Replay::load())
}

/// A module with its own data folder, a stand-in CLI and a wallet folder (CLI wallet mode: the
/// Names Wallet mode's tests are in `tests_names.rs`).
fn bridge(name: &str) -> (Bridge, PathBuf) {
    let root = std::env::temp_dir().join(format!("lez-bridge-mod-{name}-{}-{}", std::process::id(), now_ms()));
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let wallet = root.join("wallet");
    std::fs::create_dir_all(&wallet).unwrap();
    std::fs::write(wallet.join("storage.json"), "{}").unwrap();
    let td = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cli");
    let script = format!(
        r#"#!/bin/sh
D={td}
echo "ARGV $*" >> "$TMPDIR/argv.log"
while [ "$1" = "--sequencer" ] || [ "$1" = "--dry-run" ]; do if [ "$1" = "--sequencer" ]; then shift 2; else shift; fi; done
case "$1" in
  --help) echo "Usage: lez-bridge";;
  show) cat $D/show.txt;;
  balances) cat $D/balances.txt;;
  attest) cat $D/attest-show.txt;;
  wallet) cat $D/accounts.txt;;
  commitment) echo {C_V100B};;
  exit-commitment) echo 0x0000000000000000000000000000000000000000000000000000000000000007; echo "release nullifier 0x0000000000000000000000000000000000000000000000000000000000000008";;
  claim-attested) case "$*" in *--reveal-recipient*) cat $D/claim-dry-leaf1-reveal.txt;; *) sleep 0.3; cat $D/claim-dry-leaf1.txt;; esac;;
  freeze|unfreeze|set-sponsor|fund-float|register|burn) echo "dry run: simulated in 1.0ms; nothing proven or sent";;
  *) echo "Error: fake cli: unknown $1"; exit 2;;
esac
"#
    );
    let program = root.join("lez-bridge");
    std::fs::write(&program, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = Config::default();
    cfg.deployment = config::sepolia_demo_v2();
    cfg.wallet = "cli".into();
    cfg.cli = Some(program.display().to_string());
    cfg.wallet_dir = Some(wallet.display().to_string());
    cfg.tmp_dir = Some(root.join("tmp").display().to_string());
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    let b = Bridge { data: data.clone(), cfg, fixtures: None, registry_cache: None, eth_override: Some(Box::new(Replay::load())), relayed: Default::default() };
    (b, root)
}

/// The claim material the real proved claim used for this root and anchor.
fn place_material(b: &Bridge) {
    let dir = b.data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &ROOT[2..14]));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("eth_getProof.json"), "{}").unwrap();
    std::fs::write(dir.join("header.rlp.hex"), "0x").unwrap();
}

#[test]
fn no_method_or_parameter_is_a_cpp_keyword() {
    const KEYWORDS: &[&str] = &[
        "alignas", "alignof", "and", "and_eq", "asm", "auto", "bitand", "bitor", "bool", "break", "case", "catch", "char", "class", "compl", "concept", "const",
        "consteval", "constexpr", "constinit", "const_cast", "continue", "co_await", "co_return", "co_yield", "decltype", "default", "delete", "do", "double",
        "dynamic_cast", "else", "enum", "explicit", "export", "extern", "false", "float", "for", "friend", "goto", "if", "import", "inline", "int", "long", "module",
        "mutable", "namespace", "new", "noexcept", "not", "not_eq", "nullptr", "operator", "or", "or_eq", "private", "protected", "public", "register",
        "reinterpret_cast", "requires", "return", "short", "signed", "sizeof", "static", "static_assert", "static_cast", "struct", "switch", "template", "this",
        "thread_local", "throw", "true", "try", "typedef", "typeid", "typename", "union", "unsigned", "using", "virtual", "void", "volatile", "wchar_t", "while",
        "xor", "xor_eq", "signals", "slots", "emit",
    ];
    let src = include_str!("lib.rs");
    let start = src.find("pub trait LezBridgeModule").unwrap();
    let end = start + src[start..].find("\n}\n").unwrap();
    let mut names = 0;
    for line in src[start..end].lines().map(str::trim).filter(|l| l.starts_with("fn ")) {
        let name = &line[3..line.find('(').unwrap()];
        assert!(!KEYWORDS.contains(&name), "method {name} is a C++ keyword");
        let params = &line[line.find('(').unwrap() + 1..line.find(')').unwrap()];
        for p in params.split(',').map(str::trim).filter(|p| p.contains(':')) {
            let pname = p.split(':').next().unwrap().trim().trim_start_matches('_');
            assert!(!KEYWORDS.contains(&pname), "{name}: parameter {pname} is a C++ keyword");
            if name != "on_context_ready" {
                assert!(p.ends_with("String"), "{name}: every parameter is a String ({p})");
            }
        }
        names += 1;
    }
    assert!(names >= 30, "found {names} methods");
}

#[test]
fn the_registry_reads_both_sides_of_the_v2_demo() {
    let (mut b, root) = bridge("registry");
    let r = ok(&b.registry());
    let usdc = &r["tokens"][0];
    assert_eq!(usdc["symbol"], "dUSDC");
    let texts: Vec<&str> = usdc["denoms"].as_array().unwrap().iter().map(|d| d["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["100", "1,000", "10,000"]);
    // Relay-fee menu: 0.5% and 1% of each denomination (vault/DEPLOYMENTS.md).
    let menu: Vec<&str> = usdc["denoms"][0]["relay_fee_menu"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert_eq!(menu, ["0.5", "1"]);
    assert_eq!(usdc["denoms"][0]["relay_fee_menu"][0]["pct"], "0.50%");
    assert_eq!(usdc["denoms"][0]["deposit_fee_text"], "none");
    assert_eq!(usdc["caps"]["deposit"]["cap_text"], "50,000");
    assert_eq!(usdc["denoms"][0]["lez_claims"], 2);
    assert_eq!(usdc["denoms"][0]["deposits"], 3);
    assert_eq!(usdc["release_delay_s"], 86_400);
    assert_eq!(r["tokens"][1]["denoms"][0]["text"], "0.01");
    assert_eq!(r["lez"]["float"], "80000000");
    assert_eq!(r["lez"]["claims_float_covers"], 4);
    assert_eq!(r["lez"]["cfg_id_matches"], true);
    assert_eq!(r["vault"]["cfg_id_matches"], true);
    assert_eq!(r["vault"]["exits_possible"], false);
    assert_eq!(r["vault"]["deposit_count"], 5);
    assert_eq!(r["attestation"]["anchor"], ANCHOR);
    assert_eq!(r["attestation"]["anchor_block"], 12_123_027);
    assert_eq!(r["sources"]["lez_network"], "local sequencer");
    assert_eq!(r["partial"], false, "{}", r["problems"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_deposit_plan_uses_the_vaults_denominations() {
    let (mut b, root) = bridge("plan");
    let p = ok(&b.plan_deposit("dUSDC".into(), "1,250".into()));
    assert_eq!(p["bridged_text"], "1,200");
    assert_eq!(p["remainder_text"], "50");
    assert_eq!(p["pieces"].as_array().unwrap().len(), 3);
    assert_eq!(p["privacy"]["level"], "private");
    assert!(p["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Only 1 deposit of 1,000")));
    assert!(err(&b.plan_deposit("DAI".into(), "1".into())).contains("not a token"));
    assert!(err(&b.plan_deposit("ETH".into(), "0.001".into())).contains("smallest amount the bridge moves is 0.01 ETH"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_deposit_request_keeps_its_secret_here() {
    let (mut b, root) = bridge("request");
    let r = ok(&b.make_deposit_request("dUSDC".into(), "0".into(), "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC".into()));
    let secret = b.store().load()["deposits"][0]["secret"].as_str().unwrap().to_owned();
    assert_eq!(secret.len(), 66);
    let text = r.to_string();
    assert!(!text.contains(&secret[2..]), "the reply carries the secret");
    assert_eq!(r["commitment"], C_V100B);
    assert_eq!(r["account_kind"], "private");
    // The compare digits and words (words worked out with python3 hashlib, compare.rs's formula).
    assert_eq!((r["compare"]["first"].as_str(), r["compare"]["last"].as_str()), (Some("188c6f"), Some("00bacd")));
    assert_eq!(r["compare"]["words"], json!(["erode", "spare", "helmet", "welcome"]));
    assert_eq!(r["compare"]["words_text"], "erode \u{b7} spare \u{b7} helmet \u{b7} welcome");
    assert_eq!(r["steps"].as_array().unwrap().len(), 2, "approve, then deposit");
    assert!(r["steps"][1]["data"].as_str().unwrap().starts_with(&hex0x(&util::selector("deposit(address,uint32,bytes32)"))));
    assert!(r["eip681"].as_str().unwrap().starts_with("ethereum:0x8C8b2664De28e5287158db4DB65D448638871b74@11155111/deposit?"));
    assert!(r["qr"]["size"].as_u64().unwrap() >= 21);
    assert!(r["export_text"].as_str().unwrap().contains("Keep this computer's bridge data backed up"));
    assert_eq!(r["privacy"]["level"], "private");
    // A public account is refused by the plain method and needs the explicit one, confirmed.
    let p2 = "6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f";
    assert!(err(&b.make_deposit_request("dUSDC".into(), "0".into(), p2.into())).contains("public account"));
    assert!(err(&b.make_public_deposit_request("dUSDC".into(), "0".into(), p2.into(), "false".into())).contains("confirm_weaker"));
    let p = ok(&b.make_public_deposit_request("dUSDC".into(), "0".into(), p2.into(), "true".into()));
    assert_eq!(p["privacy"]["level"], "exposed");
    // The secret is only in the 0600 store, and never in a log.
    let argv = std::fs::read_to_string(root.join("tmp/argv.log")).unwrap();
    assert!(argv.contains("--sequencer http://127.0.0.1:3250 commitment"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(b.store().path).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(err(&b.make_deposit_request("dUSDC".into(), "7".into(), "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC".into())).contains("3 denominations"));
    let _ = std::fs::remove_dir_all(root);
}

fn wait_job(b: &mut Bridge, id: &str) -> Value {
    for _ in 0..200 {
        let j = ok(&b.job(id.into()));
        if !matches!(j["state"].as_str(), Some("running" | "paused")) {
            return j;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("job {id} did not end");
}

#[test]
fn a_claim_is_planned_by_a_dry_run_and_runs_as_a_forced_dry_run() {
    let (mut b, root) = bridge("claim");
    place_material(&b);
    let d = ok(&b.import_deposit(json!({ "secret": format!("0x{}", "ab".repeat(32)), "account": "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC", "token": "dUSDC", "denom_index": 0 }).to_string()));
    let id = d["id"].as_str().unwrap().to_owned();
    assert_eq!(id, "dep-188c6f3a5a40");
    assert!(d.get("secret").is_none());
    let list = ok(&b.deposits());
    let mine = &list["deposits"][0];
    assert_eq!(mine["leaf_index"], 1);
    assert_eq!(mine["block"], 12_122_994);
    assert_eq!(mine["anonymity_set"], 3);
    assert!(matches!(mine["stage"].as_str(), Some("anchored" | "claimable")), "{mine}");
    assert_eq!(mine["timeline"].as_array().unwrap().len(), 5);
    assert!(!list.to_string().contains(&"ab".repeat(32)));

    let plan = ok(&b.claim_plan(id.clone()));
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["recipient_kind"], "one_time");
    assert_eq!(plan["recipient"], "B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT");
    assert_eq!(plan["sponsored"], true);
    assert_eq!(plan["root"], ROOT);
    assert_eq!(plan["proving"]["min_ram_gb"], 16);
    // Before the suggested time, claiming now is the timing override, named as such.
    assert_eq!(plan["privacy"]["level"], if plan["claim_early"] == true { "reduced" } else { "private" });
    assert!(plan["privacy"]["notes"][0].as_str().unwrap().contains("Only 3 deposits"));
    // The leaves written for the claim are the vault's own.
    let leaves = std::fs::read_to_string(b.data.join("material").join(format!("leaves-{}.txt", &ROOT[2..14]))).unwrap();
    assert_eq!(leaves, include_str!("../testdata/eth/leaves.txt"));

    // Revealing the recipient weakens privacy: refused without confirmation, named.
    let refused = err(&b.claim(id.clone(), r#"{"reveal_recipient":true}"#.into()));
    assert!(refused.contains("confirm_weaker") && refused.contains("by name"), "{refused}");
    let early = plan["claim_early"] == true;
    let job = ok(&b.claim(id.clone(), json!({ "claim_early": early, "confirm_weaker": early }).to_string()));
    assert_eq!(job["dry_run"], true);
    assert!(job["dry_run_forced"].as_str().unwrap().contains("dry run"));
    // One wallet job at a time.
    assert!(err(&b.claim(id.clone(), json!({ "claim_early": early, "confirm_weaker": early }).to_string())).contains("busy"));
    let end = wait_job(&mut b, job["id"].as_str().unwrap());
    assert_eq!(end["state"], "done", "{end}");
    assert_eq!(end["result"]["recipient"], "B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT");
    let argv = std::fs::read_to_string(root.join("tmp/argv.log")).unwrap();
    let claim_line = argv.lines().filter(|l| l.contains("claim-attested")).last().unwrap();
    assert!(claim_line.starts_with("ARGV --sequencer http://127.0.0.1:3250 --dry-run claim-attested"), "{claim_line}");
    // A dry run does not mark the deposit claimed.
    assert_eq!(ok(&b.deposits())["deposits"][0]["stage"].as_str().map(|s| s != "claimed"), Some(true));
    let confirmed = ok(&b.claim(id, json!({ "reveal_recipient": true, "claim_early": early, "confirm_weaker": true }).to_string()));
    let end = wait_job(&mut b, confirmed["id"].as_str().unwrap());
    assert_eq!(end["result"]["recipient_kind"], "by_name");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn holdings_are_read_in_the_background_private_first() {
    let (mut b, root) = bridge("holdings");
    let h = ok(&b.holdings());
    assert_eq!(h["accounts"][0]["kind"], "private");
    assert_eq!(h["accounts"][0]["tokens"][0]["balance_text"], "100");
    assert_eq!(h["totals"][0]["balance_text"], "200");
    assert_eq!(h["stale"], false);
    let a = ok(&b.accounts());
    assert_eq!(a["accounts"][0]["kind"], "private");
    assert_eq!(a["suggested"], a["accounts"][0]["account"]);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_redemption_plan_checks_the_recipient_and_says_the_vault_cannot_release() {
    let (mut b, root) = bridge("redeem");
    assert!(err(&b.plan_redeem("dUSDC".into(), "100".into(), "0x8C8b2664De28e5287158db4DB65D448638871b74".into(), "1".into())).contains("vault itself"));
    assert!(err(&b.plan_redeem("dUSDC".into(), "100".into(), "0x0000000000000000000000000000000000000000".into(), "1".into())).contains("zero address"));
    assert!(err(&b.plan_redeem("dUSDC".into(), "100".into(), "0x37a46E7334Dd061E0C7E2298247EE9278d0d321f".into(), "1".into())).contains("checksum"));
    assert!(err(&b.plan_redeem("dUSDC".into(), "1,000".into(), "0x37a46E7334Dd061E0C7E2298247EE9278d0d321F".into(), "1".into())).contains("you hold 200 dUSDC"));
    let p = ok(&b.plan_redeem("dUSDC".into(), "200".into(), "0x37a46E7334Dd061E0C7E2298247EE9278d0d321F".into(), "1".into()));
    assert_eq!(p["pieces"].as_array().unwrap().len(), 2);
    assert_eq!(p["pieces"][0]["from_kind"], "private");
    assert_eq!(p["pieces"][0]["relay_fee_text"], "0.5");
    assert_eq!(p["pieces"][0]["receive_text"], "99.5");
    assert_eq!(p["exits_possible"], false);
    assert!(p["blocked"].as_str().unwrap().contains("never release"));
    // One piece burns from the public account: that is named and needs confirmation.
    assert_eq!(p["privacy"]["level"], "exposed");
    assert!(!p.to_string().contains("\"secret\""));
    let stored = b.store().redeem(p["id"].as_str().unwrap()).unwrap();
    assert!(stored["pieces"][0]["secret"].as_str().is_some());
    assert!(err(&b.redeem(p["id"].as_str().unwrap().into(), "{}".into())).contains("confirm_weaker"));
    let job = ok(&b.redeem(p["id"].as_str().unwrap().into(), r#"{"confirm_weaker":true}"#.into()));
    let end = wait_job(&mut b, job["id"].as_str().unwrap());
    assert_eq!(end["state"], "done");
    assert_eq!(end["steps"], 2);
    let exits = ok(&b.exits());
    assert_eq!(exits["exits"][0]["stage"], "simulated");
    assert_eq!(exits["exits"][0]["release"]["available"], false);
    let rel = ok(&b.release(exits["exits"][0]["id"].as_str().unwrap().into(), "{}".into()));
    assert_eq!(rel["available"], false);
    assert!(rel["reason"].as_str().unwrap().contains("not built"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn admin_operations_run_as_dry_runs_and_ethereum_ones_say_not_available() {
    let (mut b, root) = bridge("admin");
    let s = ok(&b.admin_status());
    // The wallet copy holds F, the v2 admin and freezer.
    assert_eq!(s["lez"]["this_wallet_is_admin"], true);
    let ops = s["operations"].as_array().unwrap();
    assert!(ops.iter().any(|o| o["op"] == "eth_set_caps" && o["available"] == false));
    assert!(ops.iter().any(|o| o["op"] == "freeze" && o["available"] == true));
    let e = ok(&b.admin_action(r#"{"op":"eth_set_caps"}"#.into()));
    assert_eq!(e["available"], false);
    assert!(err(&b.admin_action(r#"{"op":"freeze"}"#.into())).contains("minting, burning or both"));
    let j = ok(&b.admin_action(r#"{"op":"freeze","mint":true,"token":"dUSDC"}"#.into()));
    assert_eq!(j["dry_run"], true);
    wait_job(&mut b, j["id"].as_str().unwrap());
    let argv = std::fs::read_to_string(root.join("tmp/argv.log")).unwrap();
    assert!(argv.contains("--dry-run freeze --bridge JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7 --by 6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV --eth-token 0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e --mint"), "{argv}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_are_checked_and_a_remote_sequencer_keeps_wallet_reads_off() {
    let (mut b, root) = bridge("settings");
    assert!(err(&b.set_sequencer("127.0.0.1:3250".into())).contains("http"));
    let s = ok(&b.set_sequencer("https://testnet.example.org".into()));
    assert_eq!(s["loopback"], false);
    assert!(err(&b.accounts()).contains("only talks to a sequencer on this computer"));
    ok(&b.set_sequencer("http://127.0.0.1:3250".into()));
    assert!(err(&b.set_eth_rpc("sepolia".into(), "https://x".into())).contains("number"));
    assert!(err(&b.set_eth_source("infura".into())).contains("rpc or logos_module"));
    let c = ok(&b.config());
    assert_eq!(c["proving"]["enabled"], false);
    assert_eq!(c["ready"]["cli"], true);
    assert_eq!(c["lez_network"], "local sequencer");
    assert!(err(&b.use_deployment("{\"name\":1}".into())).contains("deployment"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_logos_ethereum_source_says_it_is_not_wired() {
    let (mut b, root) = bridge("ethsource");
    b.eth_override = None;
    ok(&b.set_eth_source("logos_module".into()));
    let r = ok(&b.registry());
    assert_eq!(r["partial"], true);
    assert!(r["problems"].to_string().contains("not wired"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn fixture_mode_serves_recorded_replies() {
    let (mut b, root) = bridge("fixtures");
    let dir = root.join("fx");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registry.json"), r#"{"success":true,"value":{"recorded":1},"error":null}"#).unwrap();
    ok(&b.set_fixtures(dir.display().to_string()));
    let r = parsed(&b.registry());
    assert_eq!(r["fixture"], true);
    assert_eq!(r["value"]["recorded"], 1);
    assert!(parsed(&b.deposits())["error"].as_str().unwrap().contains("no recorded reply"));
    // The privacy report and settings are never recorded.
    assert_eq!(ok(&b.privacy_report(r#"{"flow":"claim"}"#.into()))["level"], "private");
    ok(&b.set_fixtures(String::new()));
    assert!(parsed(&b.registry()).get("fixture").is_none());
    let _ = std::fs::remove_dir_all(root);
}

/// Records the Sepolia answers the tests replay. Run on a machine with network:
/// `cargo test record_sepolia_replay -- --ignored`.
#[test]
#[ignore]
fn record_sepolia_replay() {
    let d = config::sepolia_demo_v2();
    let rpc = JsonRpc::new(config::DEFAULT_SEPOLIA_RPC);
    let mut out = serde_json::Map::new();
    let tokens: Vec<[u8; 20]> = d.tokens.iter().map(|t| t.eth_token).collect();
    let mut calls: Vec<Vec<u8>> = vault::registry_calls(&tokens, &[3, 3]).into_iter().map(|(_, c)| c).collect();
    calls.push(eth::calldata("depositCount()", &[]));
    for c in calls {
        let r = rpc.call(&d.vault, &c).expect("eth_call");
        out.insert(format!("call:{}", hex0x(&c)), Value::from(hex0x(&r)));
    }
    let blk = |b: &eth::BlockInfo| json!({ "number": b.number, "hash": b.hash, "timestamp": b.timestamp });
    let latest = rpc.block("latest").unwrap();
    out.insert("block:latest".into(), blk(&latest));
    out.insert("block:finalized".into(), blk(&rpc.block("finalized").unwrap()));
    let a = rpc.block_by_hash(ANCHOR).unwrap().expect("anchor block");
    out.insert(format!("hash:{ANCHOR}"), blk(&a));
    let logs = rpc.logs(&d.vault, &eth::deposit_topic(), d.vault_create_block, latest.number).unwrap();
    for l in &logs {
        let n = u64::from_str_radix(l["blockNumber"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
        out.insert(format!("block:0x{n:x}"), blk(&rpc.block(&format!("0x{n:x}")).unwrap()));
    }
    out.insert("logs".into(), Value::Array(logs));
    for a in ["0x37a46e7334dd061e0c7e2298247ee9278d0d321f", "0x8c8b2664de28e5287158db4db65d448638871b74"] {
        out.insert(format!("code:{a}"), Value::from(rpc.code_len(&util::eth_address(a).unwrap()).unwrap()));
    }
    out.insert("recorded_at".into(), Value::from(now_ms()));
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/eth/replay.json"), serde_json::to_string_pretty(&Value::Object(out)).unwrap()).unwrap();
}

/// Records the mini-app's fixtures: every reply comes from this module's own code against the
/// real local sequencer (read-only calls and dry runs through the real `lez-bridge` binary, on a
/// copy of the v2 demo wallet) and the real Sepolia vault (read-only RPC). Nothing is proven or
/// sent. Driven by environment variables (see modules/tools/record-fixtures.sh (not published)).
#[test]
#[ignore]
fn record_v2_fixtures() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let out = PathBuf::from(var("FIXTURE_OUT"));
    let data = PathBuf::from(var("FIXTURE_DATA"));
    let _ = std::fs::remove_dir_all(&data);
    std::fs::create_dir_all(&out).unwrap();
    let mut cfg = Config::default();
    cfg.deployment = config::sepolia_demo_v2();
    cfg.wallet = "cli".into();
    cfg.cli = Some(var("LEZ_BRIDGE_CLI"));
    cfg.wallet_dir = Some(var("FIXTURE_WALLET"));
    cfg.tmp_dir = Some(var("FIXTURE_TMP"));
    let mut b = Bridge { data: data.clone(), cfg, fixtures: None, registry_cache: None, eth_override: None, relayed: Default::default() };
    // The frozen storage proof and header of the v2 claims (same anchor and root).
    let mdir = data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &ROOT[2..14]));
    std::fs::create_dir_all(&mdir).unwrap();
    for f in ["eth_getProof.json", "header.rlp.hex", "block_number"] {
        std::fs::copy(PathBuf::from(var("MATERIAL_DIR")).join(f), mdir.join(f)).unwrap();
    }
    let secrets: HashMap<String, String> = std::fs::read_to_string(var("SECRETS_ENV"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())))
        .collect();
    let save = |name: &str, reply: &str| {
        let v = parsed(reply);
        std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&v).unwrap()).unwrap();
        println!("{name}: success {}", v["success"]);
        v
    };
    let v2 = "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC";
    save("config", &b.config());
    save("registry", &b.registry());
    save("accounts", &b.accounts());
    let mut h = parsed(&b.holdings());
    for _ in 0..60 {
        if h["value"].get("reading").is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        h = parsed(&b.holdings());
    }
    save("holdings", &h.to_string());
    save("plan_deposit", &b.plan_deposit("dUSDC".into(), "1,250".into()));
    for (key, token, idx) in [("K_V100A", "dUSDC", 0), ("K_V100B", "dUSDC", 0), ("K_V1000", "dUSDC", 1), ("K_VETH", "ETH", 0)] {
        let r = parsed(&b.import_deposit(json!({ "secret": secrets[key], "account": v2, "token": token, "denom_index": idx }).to_string()));
        assert_eq!(r["success"], true, "{r}");
    }
    save("make_deposit_request", &b.make_deposit_request("dUSDC".into(), "0".into(), v2.into()));
    let deps = parsed(&b.deposits());
    let id_of = |leaf: u64| deps["value"]["deposits"].as_array().unwrap().iter().find(|d| d["leaf_index"] == leaf).unwrap()["id"].as_str().unwrap().to_owned();
    save("claim_plan", &b.claim_plan(id_of(1)));
    save("claim_plan__claimed", &b.claim_plan(id_of(0)));
    // After the dry runs: leaf 0 is now known to be claimed.
    save("deposits", &b.deposits());
    let job = save("claim", &b.claim(id_of(1), "{}".into()));
    let jid = job["value"]["id"].as_str().unwrap().to_owned();
    let mut j = parsed(&b.job(jid.clone()));
    for _ in 0..120 {
        if !matches!(j["value"]["state"].as_str(), Some("running")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        j = parsed(&b.job(jid.clone()));
    }
    save("job", &j.to_string());
    let plan = save("plan_redeem", &b.plan_redeem("dUSDC".into(), "100".into(), "0x37a46E7334Dd061E0C7E2298247EE9278d0d321F".into(), "1".into()));
    let pid = plan["value"]["id"].as_str().unwrap().to_owned();
    let rj = save("redeem", &b.redeem(pid, r#"{"confirm_weaker":true}"#.into()));
    let rid = rj["value"]["id"].as_str().unwrap_or_default().to_owned();
    for _ in 0..120 {
        if parsed(&b.job(rid.clone()))["value"]["state"] != "running" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    save("job__redeem", &b.job(rid));
    save("jobs", &b.jobs());
    let exits = save("exits", &b.exits());
    let eid = exits["value"]["exits"][0]["id"].as_str().unwrap_or("none").to_owned();
    save("release", &b.release(eid, "{}".into()));
    save("recover", &b.recover(v2.into(), "20".into()));
    save("admin_status", &b.admin_status());
    for (name, choices) in [
        ("deposit", json!({ "flow": "deposit" })),
        ("deposit__public_account", json!({ "flow": "deposit", "public_account": true })),
        ("claim", json!({ "flow": "claim" })),
        ("claim__reveal_recipient", json!({ "flow": "claim", "reveal_recipient": true })),
        ("claim__claim_early", json!({ "flow": "claim", "claim_early": true })),
        ("claim__claim_early__reveal_recipient", json!({ "flow": "claim", "claim_early": true, "reveal_recipient": true })),
        ("redeem", json!({ "flow": "redeem" })),
        ("redeem__self_relay", json!({ "flow": "redeem", "self_relay": true })),
    ] {
        save(&format!("privacy_report__{name}"), &b.privacy_report(choices.to_string()));
    }
    // No secret in any recorded file.
    for entry in std::fs::read_dir(&out).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap().to_ascii_lowercase();
        for (k, v) in &secrets {
            if k.starts_with("K_") {
                assert!(!text.contains(v.trim_start_matches("0x").to_ascii_lowercase().as_str()), "a fixture holds secret {k}");
            }
        }
    }
}

/// The cancel path and the timings, against the REAL binary on the real local chain (dry runs
/// only, wallet copy): a claim job is started through a wrapper that waits before running the
/// real CLI, cancelled, and checked: state, process group, wallet unchanged. Driven by the
/// same variables as `record_v2_fixtures` (modules/tools/real-cancel.sh (not published)).
#[test]
#[ignore]
fn real_cli_cancel_and_timings() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let data = PathBuf::from(var("FIXTURE_DATA"));
    let _ = std::fs::remove_dir_all(&data);
    let tmp = PathBuf::from(var("FIXTURE_TMP"));
    std::fs::create_dir_all(&tmp).unwrap();
    let wrapper = tmp.join("lez-bridge-slow");
    std::fs::write(&wrapper, format!("#!/bin/sh\n[ -e \"$TMPDIR/slow\" ] && sleep 30\nexec {} \"$@\"\n", var("LEZ_BRIDGE_CLI"))).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = Config::default();
    cfg.deployment = config::sepolia_demo_v2();
    cfg.wallet = "cli".into();
    cfg.cli = Some(wrapper.display().to_string());
    cfg.wallet_dir = Some(var("FIXTURE_WALLET"));
    cfg.tmp_dir = Some(tmp.display().to_string());
    let mut b = Bridge { data: data.clone(), cfg, fixtures: None, registry_cache: None, eth_override: None, relayed: Default::default() };
    let mdir = data.join("material").join(format!("{}-{}", &ANCHOR[2..14], &ROOT[2..14]));
    std::fs::create_dir_all(&mdir).unwrap();
    for f in ["eth_getProof.json", "header.rlp.hex", "block_number"] {
        std::fs::copy(PathBuf::from(var("MATERIAL_DIR")).join(f), mdir.join(f)).unwrap();
    }
    let secrets: HashMap<String, String> = std::fs::read_to_string(var("SECRETS_ENV"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())))
        .collect();
    let timed = |name: &str, f: &mut dyn FnMut() -> String| {
        let t = std::time::Instant::now();
        let r = f();
        println!("TIMING {name}: {:.1} s", t.elapsed().as_secs_f64());
        r
    };
    ok(&timed("registry (cold)", &mut || b.registry()));
    b.registry_cache = None;
    ok(&timed("registry (cold again)", &mut || b.registry()));
    let d = ok(&b.import_deposit(json!({ "secret": secrets["K_V100B"], "account": "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC", "token": "dUSDC", "denom_index": 0 }).to_string()));
    let id = d["id"].as_str().unwrap().to_owned();
    ok(&timed("deposits", &mut || b.deposits()));
    let plan = ok(&timed("claim_plan", &mut || b.claim_plan(id.clone())));
    let h = ok(&timed("holdings (first read, waits up to 12 s)", &mut || b.holdings()));
    println!("holdings first answer: reading {}", h.get("reading").is_some());
    // Let the background balances read finish before the job (they would refuse each other).
    for _ in 0..120 {
        if !refreshing(&data) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let wallet = PathBuf::from(var("FIXTURE_WALLET"));
    let sum = || {
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&wallet)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        files.sort();
        util::sha256(&files.iter().flat_map(|(n, b)| [n.as_bytes(), b.as_slice()]).collect::<Vec<_>>())
    };
    let before = sum();
    std::fs::write(tmp.join("slow"), "").unwrap();
    let early = plan["claim_early"] == true;
    let job = ok(&b.claim(id.clone(), json!({ "claim_early": early, "confirm_weaker": early }).to_string()));
    let jid = job["id"].as_str().unwrap().to_owned();
    std::thread::sleep(std::time::Duration::from_secs(2));
    let pgid = util::read_json(&data.join("jobs").join(format!("{jid}.json"))).unwrap()["pgid"].as_u64().unwrap();
    assert!(!jobs::group_members(pgid).is_empty(), "the job runs");
    println!("cancel: {}", ok(&b.cancel_job(jid.clone())));
    let mut state = Value::Null;
    for _ in 0..40 {
        state = ok(&b.job(jid.clone()));
        if state["state"] == "cancelled" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    assert_eq!(state["state"], "cancelled", "{state}");
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(jobs::group_members(pgid).is_empty(), "processes left in group {pgid}");
    assert_eq!(sum(), before, "the wallet copy changed");
    let _ = std::fs::remove_file(tmp.join("slow"));
    let k = secrets["K_V100B"].trim_start_matches("0x").to_ascii_lowercase();
    for e in std::fs::read_dir(data.join("jobs")).unwrap() {
        assert!(!std::fs::read_to_string(e.unwrap().path()).unwrap().to_ascii_lowercase().contains(&k), "a job file holds the secret");
    }
    println!("CANCEL OK: job {jid} cancelled, group {pgid} empty, wallet unchanged");
}

/// v4 releases by proof: once listed (public-demo/v4/apply-v4-entries.py), the release words are its own
/// and say neither that it never pays out nor that the outbound proof is not built.
#[test]
fn the_v4_vault_says_it_releases_by_proof() {
    let Some(d) = config::builtin("bridge-test-network-v4") else { return };
    assert_eq!(d.release, "proof");
    let r = release_unavailable(true, &d);
    assert_eq!(r["available"], false);
    assert_eq!(r["reason"], d.release_note.as_str());
    assert_eq!(r["detail"], d.release_detail.as_str());
    for text in [r["reason"].as_str().unwrap(), r["what_happens_next"].as_str().unwrap()] {
        let t = text.to_lowercase();
        assert!(!t.contains("never") && !t.contains("not built") && !t.contains("can't pay"), "{text}");
    }
    assert!(r["what_happens_next"].as_str().unwrap().contains("proved update"));
}

/// The v3 vault can't pay out (its outbound state was deployed at zero, vault/DEPLOYMENTS.md): the
/// release, exits and redeem words say so from deployments.json, without "never"/"ever" and
/// without the operator-attested claim; v2 keeps its placeholder-image words.
#[test]
fn the_v3_vault_says_it_cannot_pay_out_and_claims_no_operator_release() {
    let d = config::builtin("bridge-test-network-v3").unwrap();
    assert_eq!((d.name.as_str(), d.release.as_str()), ("bridge-test-network-v3", "never"));
    let r = release_unavailable(true, &d);
    assert_eq!(r["available"], false);
    assert_eq!(r["reason"], d.release_note.as_str());
    assert_eq!(r["vault_note"], d.release_note.as_str());
    assert_eq!(r["detail"], d.release_detail.as_str());
    assert_eq!(no_release_line(&d), d.release_note);
    for text in [r["reason"].as_str().unwrap(), r["what_happens_next"].as_str().unwrap(), r["detail"].as_str().unwrap()] {
        let t = text.to_lowercase();
        assert!(!t.contains("ever") && !t.contains("operator") && !t.contains("attest"), "{text}");
    }
    let v2 = config::builtin("bridge-test-network").unwrap();
    assert!(release_unavailable(false, &v2)["vault_note"].as_str().unwrap().contains("placeholder"));
    assert!(no_release_line(&v2).contains("placeholder"));
    // config().notices.release carries the same words for the default deployment.
    let (mut b, root) = bridge("v3-words");
    b.cfg.deployment = d.clone();
    let c: Value = serde_json::from_str(&b.config()).unwrap();
    assert_eq!(c["value"]["notices"]["release"]["text"], d.release_note.as_str(), "{c}");
    assert_eq!(c["value"]["notices"]["release"]["detail"], d.release_detail.as_str());
    b.cfg.deployment = v2;
    let c: Value = serde_json::from_str(&b.config()).unwrap();
    assert!(c["value"]["notices"]["release"].is_null());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn deployments_have_plain_labels() {
    assert_eq!(config::plain_label("bridge-test-network"), "bridge v2");
    assert_eq!(config::plain_label("bridge-test-network-v3"), "bridge v3");
    assert_eq!(config::plain_label("bridge-test-network-v4"), "bridge v4");
    assert_eq!(config::plain_label("my-local-chain"), "my-local-chain");
}
