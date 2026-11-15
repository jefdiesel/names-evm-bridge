//! TEMPORARY: parsers for the `lez-bridge` CLI's human text output.
//!
//! The CLI prints text today; CLI-CONTRACT.md asks for `--json`. Until it lands, these turn the
//! text of the commands the module needs into the contract's JSON shapes. Each is pinned by a
//! test on output captured from the real binary (testdata/cli/, 2026-11-13). They are defensive:
//! an unknown line is skipped, a missing required line is an error naming it. Delete this file
//! when `--json` exists.

use serde_json::{json, Value};

fn after<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix).map(str::trim)
}

fn list(text: &str) -> Vec<String> {
    text.trim().trim_start_matches('[').trim_end_matches(']').split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

fn bool_word(word: Option<&str>) -> Option<bool> {
    match word? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Lines of the wallet's own chatter that come before a command's answer.
fn wallet_noise(line: &str) -> bool {
    line.starts_with("Latest block is ")
        || line.starts_with("Syncing to block ")
        || line.starts_with("Synced to block ")
        || line.starts_with("Transaction data")
        || line.starts_with("Stored")
}

/// `show --bridge B`.
pub fn parse_show(out: &str) -> Result<Value, String> {
    let mut v = json!({ "tokens": [] });
    for line in out.lines().map(str::trim).filter(|l| !wallet_noise(l)) {
        if let Some(rest) = after(line, "token ") {
            v["tokens"].as_array_mut().unwrap().push(parse_token_line(rest)?);
            continue;
        }
        if let Some(rest) = after(line, "root_source ") {
            v["root_source"] = if rest == "stand-in" {
                json!({ "kind": "stand-in" })
            } else {
                let w: Vec<&str> = rest.split_whitespace().collect();
                let get = |k: &str| w.iter().position(|x| *x == k).and_then(|i| w.get(i + 1)).map(|s| s.to_string());
                json!({ "kind": "rfp-021", "program": get("program"), "instance": get("instance"),
                        "slot": get("slot").and_then(|s| s.parse::<u64>().ok()) })
            };
            continue;
        }
        let Some((key, value)) = line.split_once(' ') else { continue };
        let value = value.trim();
        match key {
            "cfg_id" | "vault" | "lez_chain_id" | "zone_id" | "admin" | "freezer" | "attestor" => v[key] = Value::from(value),
            "chain" => v["evm_chain_id"] = Value::from(value.parse::<u64>().map_err(|_| "show: bad chain id")?),
            "mint_frozen" | "burn_frozen" => v[key] = Value::from(bool_word(Some(value)).ok_or("show: bad freeze flag")?),
            "sponsor_amount" | "float" => v[key] = Value::from(value),
            _ => {}
        }
    }
    for k in ["cfg_id", "vault", "evm_chain_id", "admin", "freezer", "mint_frozen", "burn_frozen", "sponsor_amount", "float", "root_source"] {
        if v.get(k).is_none() {
            return Err(format!("the CLI's `show` printed no `{k}` line (its output changed?)"));
        }
    }
    Ok(v)
}

/// `0x… def <B58> denoms [..] claims [..] burns [..] claimed_value N burned_value N registered B mint_frozen B burn_frozen B`
fn parse_token_line(rest: &str) -> Result<Value, String> {
    let bracket = |key: &str| -> Result<Vec<String>, String> {
        let start = rest.find(&format!("{key} [")).ok_or_else(|| format!("show: token line without {key}"))? + key.len() + 1;
        let end = rest[start..].find(']').ok_or("show: unclosed list")? + start + 1;
        Ok(list(&rest[start..end]))
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let word = |k: &str| words.iter().position(|x| *x == k).and_then(|i| words.get(i + 1)).copied();
    let numbers = |k: &str| -> Result<Vec<u64>, String> { bracket(k)?.iter().map(|s| s.parse::<u64>().map_err(|_| format!("show: bad {k}"))).collect() };
    Ok(json!({
        "eth_token": words.first().ok_or("show: empty token line")?,
        "definition": word("def").ok_or("show: token line without def")?,
        "denoms": bracket("denoms")?,
        "claims": numbers("claims")?,
        "burns": numbers("burns")?,
        "claimed_value": word("claimed_value").ok_or("show: no claimed_value")?,
        "burned_value": word("burned_value").ok_or("show: no burned_value")?,
        "registered": bool_word(word("registered")).ok_or("show: no registered")?,
        "mint_frozen": bool_word(word("mint_frozen")).ok_or("show: no mint_frozen")?,
        "burn_frozen": bool_word(word("burn_frozen")).ok_or("show: no burn_frozen")?,
    }))
}

/// `balances --token-program T`.
pub fn parse_balances(out: &str) -> Result<Value, String> {
    let mut holdings = Vec::new();
    let mut synced = Value::Null;
    for line in out.lines().map(str::trim) {
        if let Some(rest) = after(line, "Synced to block ") {
            synced = rest.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()).map_or(Value::Null, Value::from);
            continue;
        }
        let (kind, rest) = match (after(line, "Public/"), after(line, "Private/")) {
            (Some(r), _) => ("public", r),
            (_, Some(r)) => ("private", r),
            _ => continue,
        };
        let w: Vec<&str> = rest.split_whitespace().collect();
        if w.len() != 3 || !w[2].bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("balances: a line the module can't read: {line}"));
        }
        holdings.push(json!({ "account": w[0], "kind": kind, "definition": w[1], "balance": w[2] }));
    }
    Ok(json!({ "synced_to": synced, "holdings": holdings }))
}

/// `attest show --program A --instance I [--anchor-block-hash H]`.
pub fn parse_attest_show(out: &str) -> Result<Value, String> {
    let mut v = json!({});
    for line in out.lines().map(str::trim) {
        if let Some(rest) = after(line, "program ") {
            v["program"] = Value::from(rest);
        } else if let Some(rest) = after(line, "instance ") {
            let w: Vec<&str> = rest.split_whitespace().collect();
            v["instance"] = Value::from(w.first().copied().unwrap_or_default());
            if let Some(i) = w.iter().position(|x| *x == "updates") {
                v["updates"] = Value::from(w.get(i + 1).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
            }
        } else if let Some(rest) = after(line, "chain ") {
            let w: Vec<&str> = rest.split_whitespace().collect();
            v["chain_id"] = Value::from(w.first().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
            v["network"] = Value::from(w.get(1).map(|s| s.trim_matches(|c| c == '(' || c == ')')).unwrap_or(""));
        } else if let Some(rest) = after(line, "checkpoint ") {
            let w: Vec<&str> = rest.split_whitespace().collect();
            if let Some(i) = w.iter().position(|x| *x == "participants") {
                v["min_participants"] = Value::from(w.get(i + 1).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
            }
        } else if let Some(rest) = after(line, "finalized beacon slot ") {
            v["finalized_slot"] = Value::from(rest.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
            if let Some(root) = rest.split("root ").nth(1) {
                v["finalized_root"] = Value::from(root.trim());
            }
        } else if let Some(rest) = after(line, "latest anchor: ") {
            v["latest_anchor"] = anchor_of(rest, rest.ends_with("(recorded)"));
        } else if let Some(rest) = after(line, "anchor ") {
            if let Some((what, verdict)) = rest.split_once(": ") {
                let recorded = verdict.starts_with("RECORDED");
                let mut a = anchor_of(what, recorded);
                if recorded {
                    let w: Vec<&str> = verdict.split_whitespace().collect();
                    if let Some(i) = w.iter().position(|x| *x == "slot") {
                        a["beacon_slot"] = Value::from(w.get(i + 1).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0));
                    }
                    if let Some(n) = verdict.rsplit("block number ").next().and_then(|n| n.trim().parse::<u64>().ok()) {
                        a["block_number"] = Value::from(n);
                    }
                }
                v["asked_anchor"] = a;
            }
        }
    }
    if v.get("instance").is_none() || v.get("latest_anchor").is_none() {
        return Err("the CLI's `attest show` printed no instance or anchor (its output changed?)".to_owned());
    }
    Ok(v)
}

fn anchor_of(text: &str, recorded: bool) -> Value {
    let w: Vec<&str> = text.split_whitespace().collect();
    let (kind, value) = match (w.first(), w.get(1), w.get(2)) {
        (Some(&"block"), Some(&"hash"), Some(v)) => ("block_hash", *v),
        (Some(&"state"), Some(&"root"), Some(v)) => ("state_root", *v),
        _ => ("unknown", ""),
    };
    json!({ "kind": kind, "value": value, "recorded": recorded, "block_number": null })
}

/// `cfg-id …`: `CFG_ID=0x…` and `PROGRAM_ID=0x…`.
pub fn parse_cfg_id(out: &str) -> Result<Value, String> {
    let get = |k: &str| out.lines().find_map(|l| l.trim().strip_prefix(k)).map(str::to_owned);
    Ok(json!({ "cfg_id": get("CFG_ID=").ok_or("cfg-id: no CFG_ID line")?, "program_id": get("PROGRAM_ID=").ok_or("cfg-id: no PROGRAM_ID line")? }))
}

/// `commitment …`: one `0x…` line.
pub fn parse_commitment(out: &str) -> Result<String, String> {
    out.lines()
        .map(str::trim)
        .find(|l| l.len() == 66 && l.starts_with("0x"))
        .map(str::to_owned)
        .ok_or_else(|| "commitment: no 0x… line".to_owned())
}

/// `exit-commitment …`: the commitment, then `release nullifier 0x…`.
pub fn parse_exit_commitment(out: &str) -> Result<Value, String> {
    let e = parse_commitment(out)?;
    let n = out.lines().find_map(|l| l.trim().strip_prefix("release nullifier ")).ok_or("exit-commitment: no release nullifier line")?;
    Ok(json!({ "exit_commitment": e, "release_nullifier": n.trim() }))
}

/// `definition …`: `name …`, `supply N`, `authority Some(B)`.
pub fn parse_definition(out: &str) -> Result<Value, String> {
    let get = |k: &str| out.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_owned());
    let authority = get("authority ").map(|a| a.trim_start_matches("Some(").trim_end_matches(')').to_owned());
    Ok(json!({ "name": get("name ").ok_or("definition: no name")?, "supply": get("supply ").ok_or("definition: no supply")?, "authority": authority }))
}

/// The user-facing code of an `Error: …` line (CLI-CONTRACT.md §1).
pub fn error_code(message: &str) -> &'static str {
    let m = message.trim_start_matches("Error: ");
    let table: [(&str, &str); 16] = [
        ("claim for another deployment", "cfg_mismatch"),
        ("token not registered", "token_not_registered"),
        ("minting is frozen on this bridge", "minting_frozen"),
        ("minting is frozen for this token", "minting_frozen_token"),
        ("bad denomination", "bad_denomination"),
        ("wrong amount", "wrong_amount"),
        ("is not this deposit", "wrong_leaf"),
        ("does not verify", "proof_invalid"),
        ("is not attested", "root_not_attested"),
        ("insufficient finality", "anchor_not_finalized"),
        ("deposit already claimed", "already_claimed"),
        ("the bridge float holds", "float_empty"),
        ("is not loopback", "not_loopback"),
        ("no wallet yet", "wallet_missing"),
        ("but reverted", "reverted"),
        ("burning is frozen", "burning_frozen"),
    ];
    table.iter().find(|(needle, _)| m.contains(needle)).map_or("cli_error", |(_, code)| code)
}

/// What a claim run printed: the plan lines, the dry run's simulation, or the outcome.
pub fn parse_claim(out: &str) -> Value {
    let mut v = json!({});
    let num = |s: &str| s.split_whitespace().next().and_then(|n| n.parse::<u64>().ok());
    for line in out.lines().filter(|l| !wallet_noise(l.trim())) {
        let t = line.trim();
        if let Some(r) = after(t, "root ") {
            v["root"] = Value::from(r);
        } else if let Some(r) = after(t, "recipient ") {
            let who = r.split_whitespace().next().unwrap_or_default();
            v["recipient"] = Value::from(who);
            v["recipient_kind"] = Value::from(if r.contains("one-time address") {
                "one_time"
            } else if r.contains("BY NAME") {
                "by_name"
            } else {
                "public"
            });
        } else if let Some(r) = after(t, "sponsor ") {
            v["sponsor"] = Value::from(r.split_whitespace().next().unwrap_or_default());
            if let Some(f) = r.split("(float ").nth(1) {
                v["float"] = Value::from(f.trim_end_matches(')'));
            }
        } else if t.starts_with("no sponsoring") {
            v["sponsor"] = Value::from("0");
            v["no_sponsor_reason"] = Value::from(t);
        } else if t.starts_with("pre-check passed") {
            v["precheck"] = Value::from(true);
        } else if let Some(r) = after(t, "public cycles:") {
            v["public_cycles"] = num(r).map_or(Value::Null, Value::from);
        } else if let Some(r) = after(t, "fee at head:") {
            v["fee_at_head"] = Value::from(r);
        } else if let Some(r) = after(t, "reserve:") {
            v["reserve"] = Value::from(r.split_whitespace().next().unwrap_or_default());
        } else if let Some(r) = after(t, "fee payer:") {
            v["fee_payer"] = Value::from(r);
        } else if let Some(r) = after(t, "dry run: simulated in ") {
            v["dry_run"] = Value::from(true);
            v["simulated_in"] = Value::from(r.split(';').next().unwrap_or_default());
        } else if let Some(r) = after(t, "proved and submitted in ") {
            v["proved_in_s"] = r.trim_end_matches('s').parse::<f64>().map_or(Value::Null, Value::from);
        } else if let Some(r) = after(t, "tx ") {
            v["tx"] = Value::from(r);
        } else if let Some(r) = after(t, "applied in block ") {
            v["block"] = num(r).map_or(Value::Null, Value::from);
        } else if let Some(r) = after(t, "Error: ") {
            v["error"] = json!({ "code": error_code(r), "message": r, "local_only": !r.contains("reverted") });
        } else if let Some(r) = after(t, "guard: ") {
            for kv in r.split_whitespace() {
                if let Some(mb) = kv.strip_prefix("peak_footprint=").and_then(|x| x.strip_suffix("MB")) {
                    v["peak_footprint_mb"] = mb.parse::<u64>().map_or(Value::Null, Value::from);
                }
            }
        }
    }
    v
}

/// The last working phase a log reached (`done` lines aside): a dry run ends in `simulate`.
pub fn work_phase_of(log: &str) -> &'static str {
    let without: String = log
        .lines()
        .filter(|l| !(l.trim().starts_with("applied in block ") || l.trim().starts_with("dry run: simulated in ")))
        .map(|l| format!("{l}\n"))
        .collect();
    phase_of(&without)
}

/// The job phase a log has reached (CLI-CONTRACT.md §3), from the text lines that start each.
pub fn phase_of(log: &str) -> &'static str {
    let mut phase = "fetch";
    for line in log.lines().map(str::trim) {
        if line.starts_with("pre-check passed") {
            phase = "simulate";
        } else if line.starts_with("privacy-preserving: proving") {
            phase = "prove";
        } else if line.starts_with("proved and submitted in ") || line.starts_with("submitted in ") {
            phase = "submit";
        } else if line.starts_with("tx ") {
            phase = "confirm";
        } else if line.starts_with("applied in block ") || line.starts_with("dry run: simulated in ") {
            phase = "done";
        }
    }
    phase
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = include_str!("../testdata/cli/show.txt");
    const BALANCES: &str = include_str!("../testdata/cli/balances.txt");
    const ATTEST: &str = include_str!("../testdata/cli/attest-show.txt");
    const DRY1: &str = include_str!("../testdata/cli/claim-dry-leaf1.txt");
    const DRY0: &str = include_str!("../testdata/cli/claim-dry-leaf0-claimed.txt");
    const REVEAL: &str = include_str!("../testdata/cli/claim-dry-leaf1-reveal.txt");
    const WRONG: &str = include_str!("../testdata/cli/claim-dry-wrong-leaf.txt");
    const PROVED: &str = include_str!("../../../fixtures/source/out-private-claim.log");

    #[test]
    fn show_reads_the_v2_bridge() {
        let v = parse_show(SHOW).unwrap();
        assert_eq!(v["cfg_id"], "0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11");
        assert_eq!(v["evm_chain_id"], 11_155_111);
        assert_eq!(v["sponsor_amount"], "20000000");
        assert_eq!(v["float"], "80000000");
        assert_eq!(v["root_source"]["kind"], "rfp-021");
        assert_eq!(v["root_source"]["slot"], 83);
        let t = &v["tokens"][0];
        assert_eq!(t["denoms"], json!(["100000000", "1000000000", "10000000000"]));
        assert_eq!(t["claims"], json!([2, 0, 0]));
        assert_eq!(t["claimed_value"], "200000000");
        assert_eq!(v["tokens"][1]["eth_token"], "0x0000000000000000000000000000000000000000");
        assert_eq!(v["tokens"][1]["denoms"][2], "1000000000000000000");
    }

    #[test]
    fn show_fails_loudly_when_a_line_is_missing() {
        let cut: String = SHOW.lines().filter(|l| !l.starts_with("float")).map(|l| format!("{l}\n")).collect();
        assert!(parse_show(&cut).unwrap_err().contains("float"));
        assert!(parse_show("").is_err());
    }

    #[test]
    fn balances_skip_the_wallets_sync_lines() {
        let v = parse_balances(BALANCES).unwrap();
        assert_eq!(v["synced_to"], 5075);
        assert_eq!(v["holdings"].as_array().unwrap().len(), 2);
        assert_eq!(v["holdings"][1]["kind"], "private");
        assert_eq!(v["holdings"][1]["account"], "2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC");
        assert_eq!(v["holdings"][1]["balance"], "100000000");
        assert!(parse_balances("Public/x y\n").is_err());
    }

    #[test]
    fn attest_show_reads_the_instance_and_anchor() {
        let v = parse_attest_show(ATTEST).unwrap();
        assert_eq!(v["instance"], "0x09dd33204d65597cdb66e90f793a4fe3478cbe226a047a77e228b4dcf05ab4de");
        assert_eq!(v["chain_id"], 11_155_111);
        assert_eq!(v["network"], "sepolia");
        assert_eq!(v["updates"], 0);
        assert_eq!(v["min_participants"], 342);
        assert_eq!(v["finalized_slot"], 11_566_976);
        assert_eq!(v["latest_anchor"]["value"], "0x5e81a69dd5b2dfd18a00c0021f7954b7971421221897b3ec752a4002561ddd7d");
        assert_eq!(v["latest_anchor"]["recorded"], true);
        assert_eq!(v["asked_anchor"]["beacon_slot"], 11_566_976);
        assert!(v["asked_anchor"]["block_number"].is_null());
    }

    #[test]
    fn small_outputs_parse() {
        let c = parse_cfg_id(include_str!("../testdata/cli/cfg-id.txt")).unwrap();
        assert_eq!(c["program_id"], "0xfff8054169fcabf078493cfda8214d14f03a7294be509dfd9180d29e33fde910");
        let d = parse_definition(include_str!("../testdata/cli/definition-usdc.txt")).unwrap();
        assert_eq!(d["supply"], "200000000");
        assert_eq!(d["authority"], "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7");
        assert_eq!(d["name"], "Wrapped dUSDC (Sepolia demo v2)");
    }

    #[test]
    fn claim_dry_runs_read_as_plans() {
        let v = parse_claim(DRY1);
        assert_eq!(v["recipient_kind"], "one_time");
        assert_eq!(v["recipient"], "B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT");
        assert_eq!(v["sponsor"], "20000000");
        assert_eq!(v["float"], "80000000");
        assert_eq!(v["public_cycles"], 81_568);
        assert_eq!(v["dry_run"], true);
        assert_eq!(phase_of(DRY1), "done");
        assert_eq!(work_phase_of(DRY1), "simulate");
        let r = parse_claim(REVEAL);
        assert_eq!(r["recipient_kind"], "by_name");
        assert_eq!(r["sponsor"], "0");
        let claimed = parse_claim(DRY0);
        assert_eq!(claimed["error"]["code"], "already_claimed");
        assert_eq!(claimed["error"]["local_only"], true);
        assert_eq!(phase_of(DRY0), "fetch");
        assert_eq!(parse_claim(WRONG)["error"]["code"], "wrong_leaf");
    }

    #[test]
    fn the_real_proved_claim_reads_through_every_phase() {
        let v = parse_claim(PROVED);
        assert_eq!(v["block"], 4998);
        assert_eq!(v["proved_in_s"], 670.9);
        assert_eq!(v["peak_footprint_mb"], 9403);
        assert_eq!(phase_of(PROVED), "done");
        let until_prove: String = PROVED.lines().take(7).map(|l| format!("{l}\n")).collect();
        assert_eq!(phase_of(&until_prove), "prove");
    }

    #[test]
    fn every_error_text_has_a_code() {
        for (text, code) in [
            ("Error: claim for another deployment: the deposit's cfg id", "cfg_mismatch"),
            ("Error: minting is frozen for this token: retry", "minting_frozen_token"),
            ("Error: insufficient finality: the attestation instance has not finalized", "anchor_not_finalized"),
            ("Error: abc was included in block 9 but reverted", "reverted"),
            ("Error: something new", "cli_error"),
        ] {
            assert_eq!(error_code(text), code);
        }
    }
}
