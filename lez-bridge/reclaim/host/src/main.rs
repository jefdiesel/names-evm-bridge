// SPDX-License-Identifier: MIT OR Apache-2.0
//! reclaim_host: EXECUTOR ONLY (never proves).
//!
//!   reclaim_host ids
//!       RECLAIM_IMAGE_ID=0x… (the failsafe vault's reclaimImageId)
//!   reclaim_host vector [--write <dir>]
//!       run the fixture's reclaims in the executor, print cycles; write reclaim_vector.json and
//!       ReclaimVector.sol (copy the .sol to vault/test/fixtures/)
//!   reclaim_host reclaim --master-file F --dest D --token T --denom N --chain-id C --vault V
//!       --lez-chain-id X --zone-id Z --program-id P --leaves FILE --state FILE --recipient R
//!       [--relayer L --relay-fee F] [--elf E --image-id H]
//!       one reclaim: finds the deposit among the vault's leaves, checks it is not in the final
//!       accumulator, runs the guest, prints DEPOSIT_ROOT, FINAL_STATE, NULLIFIER, JOURNAL, SEAL
//!       (the MockVerifier's), USER_CYCLES, TOTAL_CYCLES, SEGMENTS. Exit 3 with "REFUSED: claimed"
//!       if the deposit was claimed, 4 with "REFUSED: not deposited" if no leaf matches. The master
//!       secret is read from a file and never printed.

use anyhow::{Context, Result, anyhow, bail};
use reclaim_core::CfgPreimage;
use reclaim_host::{BuildError, HostState, build_input, execute, execute_elf, fixture, image_id, mock_seal};
use std::collections::HashMap;

fn hex32(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim().trim_start_matches("0x")).with_context(|| format!("hex: {s}"))?;
    v.try_into().map_err(|_| anyhow!("want 32 bytes: {s}"))
}

fn addr(s: &str) -> Result<[u8; 20]> {
    let v = hex::decode(s.trim().trim_start_matches("0x")).with_context(|| format!("hex: {s}"))?;
    v.try_into().map_err(|_| anyhow!("want 20 bytes: {s}"))
}

/// An account id given as base58 (LEZ's display) or 0x-hex.
fn account(s: &str) -> Result<[u8; 32]> {
    if s.starts_with("0x") {
        return hex32(s);
    }
    let id: lee_core::account::AccountId = s.parse().map_err(|e| anyhow!("account id {s}: {e:?}"))?;
    Ok(*id.value())
}

fn h(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn flags(args: &[String]) -> Result<HashMap<String, String>> {
    let mut m = HashMap::new();
    let mut it = args.iter();
    while let Some(k) = it.next() {
        let k = k.strip_prefix("--").ok_or_else(|| anyhow!("unexpected argument {k}"))?;
        let v = it.next().ok_or_else(|| anyhow!("--{k} needs a value"))?;
        m.insert(k.to_string(), v.clone());
    }
    Ok(m)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ids") => println!("RECLAIM_IMAGE_ID={}", h(&image_id())),
        Some("vector") => {
            let f = fixture(1_000_000)?;
            println!("G4 reclaim guest image id: {}", h(&image_id()));
            println!("ELF size: {} bytes", reclaim_host::RECLAIM_GUEST_ELF.len());
            println!("| vector | user cycles | total cycles | segments |");
            println!("|---|---:|---:|---:|");
            for (l, r) in &f.cycles {
                println!("| {l} | {} | {} | {} |", r.user_cycles, r.total_cycles, r.segments);
            }
            if let Some(i) = args.iter().position(|a| a == "--write") {
                let dir = std::path::PathBuf::from(args.get(i + 1).ok_or_else(|| anyhow!("--write <dir>"))?);
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join("reclaim_vector.json"), &f.json)?;
                std::fs::write(dir.join("ReclaimVector.sol"), &f.sol)?;
                println!("wrote {}/reclaim_vector.json and ReclaimVector.sol", dir.display());
            }
        }
        Some("reclaim") => reclaim(&flags(&args[1..])?)?,
        _ => bail!("usage: reclaim_host ids | vector [--write <dir>] | reclaim --master-file F … (see the source header)"),
    }
    Ok(())
}

fn reclaim(f: &HashMap<String, String>) -> Result<()> {
    let get = |k: &str| f.get(k).map(String::as_str).ok_or_else(|| anyhow!("--{k} is required"));
    let master = hex32(&std::fs::read_to_string(get("master-file")?).context("read the master secret file")?)?;
    let dest = account(get("dest")?)?;
    let token = addr(get("token")?)?;
    let denom: u128 = get("denom")?.parse()?;
    let cfg = CfgPreimage {
        chain_id: get("chain-id")?.parse()?,
        vault: addr(get("vault")?)?,
        lez_chain_id: hex32(get("lez-chain-id")?)?,
        zone_id: hex32(get("zone-id")?)?,
        program_id: account(get("program-id")?)?,
    };
    let leaves: Vec<[u8; 32]> = std::fs::read_to_string(get("leaves")?)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(hex32)
        .collect::<Result<_>>()?;
    let state = HostState::load(std::path::Path::new(get("state")?))?;
    let tree = state.tree()?;
    let recipient = addr(get("recipient")?)?;
    let relayer = f.get("relayer").map(|s| addr(s)).transpose()?.unwrap_or([0; 20]);
    let fee: u128 = f.get("relay-fee").map(|s| s.parse()).transpose()?.unwrap_or(0);
    let input = match build_input(master, dest, token, denom, cfg, &leaves, &state.g2, &tree, recipient, relayer, fee) {
        Ok(i) => i,
        Err(BuildError::Claimed) => {
            println!("REFUSED: claimed (the deposit's claim nullifier is in the final accumulator; no reclaim proof exists)");
            std::process::exit(3);
        }
        Err(BuildError::NotDeposited) => {
            println!("REFUSED: not deposited (no vault leaf for this secret, account, token and denomination)");
            std::process::exit(4);
        }
    };
    println!("deposit leaf {} of {}; final state tip {} accumulator count {}", input.leaf_index, leaves.len(), state.g2.tip.id, state.g2.claim_acc.count);
    let (image, run) = match (f.get("elf"), f.get("image-id")) {
        (Some(e), Some(id)) => {
            let image = hex32(id)?;
            (image, execute_elf(&std::fs::read(e)?, image, &input)?)
        }
        (None, None) => (image_id(), execute(&input)?),
        _ => bail!("--elf and --image-id go together"),
    };
    let j = &run.decoded;
    println!("CFG_ID={}", h(&j.cfg_id));
    println!("DEPOSIT_ROOT={}", h(&j.deposit_root));
    println!("FINAL_STATE={}", h(&j.final_state));
    println!("NULLIFIER={}", h(&j.nullifier));
    println!("JOURNAL={}", h(&run.journal));
    println!("SEAL={}", h(&mock_seal(&image, &run.journal)));
    println!("USER_CYCLES={}", run.user_cycles);
    println!("TOTAL_CYCLES={}", run.total_cycles);
    println!("SEGMENTS={}", run.segments);
    Ok(())
}
