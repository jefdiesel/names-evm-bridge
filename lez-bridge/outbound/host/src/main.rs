//! outbound_host: executes by default; proves only with `--prove` (owner-started, through
//! `public-demo/prove/prove-outbound-rc3.sh`; in-process CPU prover only, see `prove_gate`).
//!
//!   outbound_host ids
//!   outbound_host last    --rpc URL
//!   outbound_host genesis --rpc URL --dir BLOCKS --block N --bridge ID [--out STATE]
//!                         prints G2_STATE / INITIAL_G2_STATE: the vault's `initialG2State` for a
//!                         genesis at block N (N must come before the bridge's Initialize)
//!   outbound_host update  --rpc URL --dir BLOCKS --state STATE --to N --out DIR [--no-exec]
//!                         [--elf GUEST --image-id H] [--po2 P] [--prove]
//!   outbound_host release (--secret-file F | --secret K) --eth-dest A --token A --denom N
//!                         --max-relay-fee N --cfg-id H --leaves FILE --index I --relayer A --relay-fee N
//!                         [--elf GUEST --image-id H] [--po2 P] [--out DIR] [--prove]
//!   outbound_host verify  --receipt FILE --image-id H [--journal FILE]
//!   outbound_host scan    --rpc URL --dir BLOCKS --from A --to B
//!   outbound_host measure --rpc URL --dir BLOCKS --bridge ID --from S --at B1,B2,..
//!   outbound_host circuit-id --artifact FILE
//!
//! `update` runs G2 natively from STATE over blocks tip+1..N, then the TrustedTip guest in the
//! executor (unless --no-exec), checks both journals are equal, and writes to DIR: journal.hex,
//! seal.hex (the test MockVerifier's seal), next-state.bin/.json, report.json. STATE itself is
//! not changed: the caller promotes next-state.bin once the vault accepted the update.
//!
//! `--elf GUEST --image-id H`: run (and prove) that ELF instead of the embedded guest; its computed
//! image id must equal H (the vault's `updateImageId` / `releaseImageId`). `--image-id` alone pins
//! the embedded guest. `--prove` requires `--image-id`, executes first (journals must agree), then
//! proves to a succinct receipt, verifies it natively, and writes receipt.bin (borsh) and
//! proof.json to DIR. `verify` checks a receipt file natively against an image id (and a journal).

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use outbound_core::{finality::BlockRef, update::JOURNAL_VERSION, wire};
use outbound_host::*;
use std::{collections::HashMap, path::PathBuf};

struct Args(HashMap<String, String>, Vec<String>);

impl Args {
    fn parse(v: &[String]) -> Self {
        let mut m = HashMap::new();
        let mut flags = Vec::new();
        let mut i = 0;
        while i < v.len() {
            let k = v[i].trim_start_matches("--").to_string();
            if i + 1 < v.len() && !v[i + 1].starts_with("--") {
                m.insert(k, v[i + 1].clone());
                i += 2;
            } else {
                flags.push(k);
                i += 1;
            }
        }
        Self(m, flags)
    }
    fn get(&self, k: &str) -> Result<&str> {
        self.0.get(k).map(String::as_str).ok_or_else(|| anyhow!("missing --{k}"))
    }
    fn opt(&self, k: &str) -> Option<String> {
        self.0.get(k).cloned()
    }
    fn u64(&self, k: &str) -> Result<u64> {
        Ok(self.get(k)?.parse()?)
    }
    fn u128(&self, k: &str) -> Result<u128> {
        Ok(self.get(k)?.parse()?)
    }
    fn flag(&self, k: &str) -> bool {
        self.1.iter().any(|f| f == k)
    }
    fn store(&self) -> Result<Store> {
        Store::new(self.get("dir")?, self.opt("rpc"))
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = argv.split_first().ok_or_else(|| anyhow!("usage: see the crate docs"))?;
    let a = Args::parse(rest);
    match cmd.as_str() {
        "ids" => {
            println!("UPDATE_IMAGE_ID_TRUSTED_TIP={}", h0x(&update_image_id_trusted_tip()));
            println!("UPDATE_IMAGE_ID_RELEASE={}", h0x(&update_image_id_release()));
            println!("RELEASE_IMAGE_ID={}", h0x(&release_image_id()));
            println!("PRIVACY_CIRCUIT_ID={}", h0x(&outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID));
            // The 4-byte selector a Groth16 seal from this risc0 version carries (risc0-ethereum's
            // RiscZeroGroth16Verifier: the first bytes of the verifier parameters' digest).
            println!("GROTH16_SELECTOR={}", h0x(&groth16_selector()));
        }
        "last" => println!("{}", last_block_id(a.get("rpc")?)?),
        "circuit-id" => {
            let elf = std::fs::read(a.get("artifact")?)?;
            println!("{}", h0x(&digest_bytes(risc0_zkvm::compute_image_id(&elf)?)));
        }
        "genesis" => {
            let s = a.store()?;
            let tip = s.block_ref(a.u64("block")?)?;
            let st = HostState::genesis(parse_account(a.get("bridge")?)?, tip);
            if let Some(out) = a.opt("out") {
                st.save(&PathBuf::from(out))?;
            }
            println!("G2_STATE={}", h0x(&st.g2.commitment()));
            println!("INITIAL_G2_STATE={} (the vault's initialG2State for this genesis; never zero)", h0x(&st.g2.commitment()));
            println!("tip {} {}", tip.id, h0x(&tip.hash));
        }
        "verify" => cmd_verify(&a)?,
        "update" => cmd_update(&a)?,
        "release" => cmd_release(&a)?,
        "scan" => cmd_scan(&a)?,
        "measure" => cmd_measure(&a)?,
        "vector" => cmd_vector(&a)?,
        _ => bail!("unknown command {cmd}"),
    }
    Ok(())
}

/// The guest to use: `--elf FILE` (checked against `--image-id`), else the embedded one (checked
/// against `--image-id` if given). `--prove` needs a pin.
fn guest(a: &Args, embedded: Guest) -> Result<Guest> {
    let pin = a.opt("image-id").map(|h| parse_hex::<32>(&h)).transpose()?;
    if a.flag("prove") {
        ensure!(pin.is_some(), "--prove needs --image-id (the id the vault stores)");
    }
    match a.opt("elf") {
        Some(f) => Guest::from_file(&PathBuf::from(f), pin.ok_or_else(|| anyhow!("--elf needs --image-id"))?),
        None => Guest::embedded_pinned(embedded, pin),
    }
}

fn po2(a: &Args) -> Result<Option<u32>> {
    a.opt("po2").map(|v| v.parse::<u32>().map_err(Into::into)).transpose()
}

fn cmd_verify(a: &Args) -> Result<()> {
    let r = load_receipt(&PathBuf::from(a.get("receipt")?))?;
    let image = parse_hex::<32>(a.get("image-id")?)?;
    let want = a.opt("journal").map(|f| -> Result<Vec<u8>> {
        let t = std::fs::read_to_string(&f)?;
        Ok(hex::decode(t.trim().trim_start_matches("0x"))?)
    }).transpose()?;
    let kind = check_receipt(&r, image, want.as_deref())?;
    println!("VERIFIED: {kind} receipt for image {}; claim ok(image, sha256(journal)), no assumptions", h0x(&image));
    println!("JOURNAL_DIGEST={}", h0x(&sha256(&r.journal.bytes)));
    println!("JOURNAL={}", h0x(&r.journal.bytes));
    if let Ok(j) = decode_journal(&r.journal.bytes) {
        println!("update journal: cfgId {} prevState {} newState {} roots {} claims {} exits {}", h0x(&j.cfg_id), h0x(&j.prev_state), h0x(&j.new_state), j.deposit_roots_used.len(), j.claims.len(), j.exits.len());
    } else if let Some(j) = release_core::ReleaseJournal::abi_decode(&r.journal.bytes) {
        println!("release journal: cfgId {} root {} nullifier {}", h0x(&j.cfg_id), h0x(&j.root), h0x(&j.nullifier));
    }
    if want.is_some() {
        println!("journal equals the given file");
    }
    Ok(())
}

fn cmd_update(a: &Args) -> Result<()> {
    let g = guest(a, Guest::trusted_tip())?;
    let po2 = po2(a)?;
    let prove = a.flag("prove");
    ensure!(!(prove && a.flag("no-exec")), "--prove executes first: drop --no-exec");
    let s = a.store()?;
    let st = HostState::load(&PathBuf::from(a.get("state")?))?;
    let to = a.u64("to")?;
    ensure!(to > st.g2.tip.id, "--to {to} is not past the state's tip {}", st.g2.tip.id);
    let blocks = s.range(st.g2.tip.id + 1, to)?;
    let bytes: usize = blocks.iter().map(Vec::len).sum();
    let p = prepare(&st, blocks)?;
    let out = PathBuf::from(a.get("out")?);
    std::fs::create_dir_all(&out)?;
    let run = if a.flag("no-exec") { None } else { Some(execute_guest(&g, &p, po2)?) };
    let image = g.image;
    let seal = mock_seal(&image, &p.journal);
    std::fs::write(out.join("journal.hex"), h0x(&p.journal))?;
    std::fs::write(out.join("seal.hex"), h0x(&seal))?;
    p.next.save(&out.join("next-state.bin"))?;
    let j = &p.out.journal;
    let report = serde_json::json!({
        "label": LABEL,
        "blocks": [st.g2.tip.id + 1, to],
        "bytes": bytes,
        "prevState": h0x(&j.prev_state),
        "newState": h0x(&j.new_state),
        "cfgId": h0x(&j.cfg_id),
        "depositRootsUsed": j.deposit_roots_used.iter().map(|r| h0x(r)).collect::<Vec<_>>(),
        "claims": j.claims.iter().map(|c| serde_json::json!({"token": h0x(&c.token), "denomIndex": c.denom_index, "count": c.count})).collect::<Vec<_>>(),
        "exits": j.exits.iter().map(|e| serde_json::json!({"token": h0x(&e.token), "denomIndex": e.denom_index, "commitment": h0x(&e.commitment)})).collect::<Vec<_>>(),
        "nullifierPdas": p.out.batch.nullifiers.iter().map(|n| h0x(n)).collect::<Vec<_>>(),
        "privateReceiptsVerifiedNatively": p.receipts.len(),
        "journalBytes": p.journal.len(),
        "journalDigest": h0x(&sha256(&p.journal)),
        "updateImageId": h0x(&image),
        "guest": g.source,
        "segmentPo2": po2,
        "executor": run.as_ref().map(|r| serde_json::json!({
            "userCycles": r.user_cycles, "totalCycles": r.total_cycles, "segments": r.segments,
            "assumptions": r.assumptions, "stats": format!("{:?}", r.stats),
        })),
        "next": p.next.summary(),
    });
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&report)?)?;
    println!("{LABEL}");
    println!("JOURNAL={}", h0x(&p.journal));
    println!("SEAL={}", h0x(&seal));
    println!("PREV_STATE={}", h0x(&j.prev_state));
    println!("NEW_STATE={}", h0x(&j.new_state));
    println!("blocks {}..{to} ({bytes} bytes), roots {}, claims {:?}, exits {}, nullifiers {}, private receipts {}",
        st.g2.tip.id + 1, j.deposit_roots_used.len(),
        j.claims.iter().map(|c| (hex::encode(c.token), c.denom_index, c.count)).collect::<Vec<_>>(),
        j.exits.len(), p.out.batch.nullifiers.len(), p.receipts.len());
    if let Some(r) = run {
        println!("executor: {} user cycles, {} total, {} segments{}, {} assumptions; journal equals the native one",
            r.user_cycles, r.total_cycles, r.segments, po2.map(|n| format!(" (po2 {n})")).unwrap_or_default(), r.assumptions);
    }
    if prove {
        println!("PROVING (in-process CPU prover, succinct) {} from {}", h0x(&image), g.source);
        let pr = prove_update(&g, &p, po2)?;
        save_receipt(&pr.receipt, &out.join("receipt.bin"))?;
        std::fs::write(out.join("proof.json"), serde_json::to_string_pretty(&pr.report(&g))?)?;
        println!("{PROVED_LABEL}");
        println!("PROVED: {} receipt, {} user cycles, {} total, {} segments, {:.1} s; verified natively; {}",
            pr.kind, pr.user_cycles, pr.total_cycles, pr.segments, pr.secs, out.join("receipt.bin").display());
    }
    Ok(())
}

fn addr(s: &str) -> Result<[u8; 20]> {
    parse_hex::<20>(s)
}

fn cmd_release(a: &Args) -> Result<()> {
    let leaves: Vec<[u8; 32]> = std::fs::read_to_string(a.get("leaves")?)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse_hex::<32>)
        .collect::<Result<_>>()?;
    let index = a.u64("index")? as usize;
    ensure!(index < leaves.len(), "index {index} outside {} leaves", leaves.len());
    let g = guest(a, Guest::release())?;
    let secret = match a.opt("secret-file") {
        Some(f) => parse_hex::<32>(std::fs::read_to_string(&f).with_context(|| format!("read {f}"))?.trim())?,
        None => parse_hex::<32>(a.get("secret").map_err(|_| anyhow!("missing --secret-file (or --secret)"))?)?,
    };
    let input = release_core::ReleaseInput {
        witness: release_core::ReleaseWitness {
            secret,
            eth_dest: addr(a.get("eth-dest")?)?,
            token: addr(a.get("token")?)?,
            denom: release_core::u256(a.u128("denom")?),
            max_relay_fee: release_core::u256(a.u128("max-relay-fee")?),
            cfg_id: parse_hex::<32>(a.get("cfg-id")?)?,
            leaf_index: index as u64,
            path: exit_path(&leaves, index),
        },
        public: release_core::ReleasePublic {
            relayer: addr(a.get("relayer")?)?,
            relay_fee: release_core::u256(a.u128("relay-fee")?),
        },
    };
    let r = execute_release_guest(&g, &input)?;
    let seal = mock_seal(&g.image, &r.journal);
    println!("{LABEL}");
    println!("ROOT={}", h0x(&r.decoded.root));
    println!("NULLIFIER={}", h0x(&r.decoded.nullifier));
    println!("JOURNAL={}", h0x(&r.journal));
    println!("SEAL={}", h0x(&seal));
    println!("USER_CYCLES={}", r.user_cycles);
    let out = a.opt("out").map(PathBuf::from);
    if let Some(d) = &out {
        std::fs::create_dir_all(d)?;
        std::fs::write(d.join("journal.hex"), h0x(&r.journal))?;
    }
    if a.flag("prove") {
        let d = out.ok_or_else(|| anyhow!("--prove needs --out DIR"))?;
        println!("PROVING (in-process CPU prover, succinct) {} from {}", h0x(&g.image), g.source);
        let pr = prove_release(&g, &input, po2(a)?)?;
        save_receipt(&pr.receipt, &d.join("receipt.bin"))?;
        std::fs::write(d.join("proof.json"), serde_json::to_string_pretty(&pr.report(&g))?)?;
        println!("{PROVED_LABEL}");
        println!("PROVED: {} receipt, {} user cycles, {} segments, {:.1} s; verified natively; {}",
            pr.kind, pr.user_cycles, pr.segments, pr.secs, d.join("receipt.bin").display());
    }
    Ok(())
}

/// Checks the wire mirror on real blocks: strict decode, byte-identical re-encode, the parser,
/// stored hash = recomputed hash, linkage.
fn cmd_scan(a: &Args) -> Result<()> {
    let s = a.store()?;
    let (from, to) = (a.u64("from")?, a.u64("to")?);
    let mut prev: Option<BlockRef> = None;
    let (mut txs, mut private, mut reverted, mut bytes, mut max) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for id in from..=to {
        let raw = s.get(id)?;
        let whole: wire::Block = borsh::from_slice(&raw).map_err(|e| anyhow!("block {id}: whole decode: {e}"))?;
        ensure!(borsh::to_vec(&whole)? == raw, "block {id}: re-encode differs");
        let p = wire::parse_block(&raw).map_err(|e| anyhow!("block {id}: {}", e.0))?;
        ensure!(p.header.block_id == id, "block {id}: id {}", p.header.block_id);
        ensure!(p.header.hash == p.hash, "block {id}: stored hash differs from recomputed");
        if let Some(pr) = prev {
            ensure!(p.header.prev_block_hash == pr.hash, "block {id}: prev hash breaks the chain");
        }
        prev = Some(BlockRef { id, hash: p.hash });
        txs += p.txs.len();
        private += p.txs.iter().filter(|t| matches!(t, wire::ParsedTx::Private { .. })).count();
        reverted += p.reverted.len();
        bytes += raw.len();
        max = max.max(raw.len());
    }
    println!("blocks {from}..={to}: all decode strictly, re-encode byte-identical, hash = stored hash, linked");
    println!("{txs} transactions ({private} private, {reverted} reverted), {bytes} bytes, largest block {max} bytes");
    Ok(())
}

/// `vector --out FILE`: the cross-language fixture `vault/test/fixtures/OutboundVector.sol`. Two
/// G2 updates over the committed public9 blocks (2893..2899, 2900..2904), each executed by the
/// TrustedTip guest (journal = native journal), for `vault/test/OutboundVector.t.sol` to feed to
/// a real `BridgeVault.update`.
fn cmd_vector(a: &Args) -> Result<()> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/public9");
    let s = Store::new(&dir, None)?;
    let raw = s.get(2893)?;
    let first = wire::parse_block(&raw).map_err(|e| anyhow!("{}", e.0))?;
    let g = HostState::genesis(parse_account("GR5ngdsJ1WC49YqShQ6JCAK4JmQ9QoRcVMobxTmXaV8u")?, BlockRef { id: 2892, hash: first.header.prev_block_hash });
    let p1 = prepare(&g, s.range(2893, 2899)?)?;
    let r1 = execute_trusted_tip(&p1)?;
    let p2 = prepare(&p1.next, s.range(2900, 2904)?)?;
    let r2 = execute_trusted_tip(&p2)?;
    let e = &p2.out.journal.exits[0];
    let sol = format!(
        "// SPDX-License-Identifier: MIT\n// GENERATED by `outbound_host vector` (lez-bridge/outbound/host) from the real LEZ blocks\n\
         // 2893..2904 of run public9 (lez-bridge/outbound/fixtures/public9). Journals executed by the G2-lite\n\
         // TrustedTip guest in RISC Zero's executor ({} and {} user cycles); not proved. Do not edit.\n\
         pragma solidity ^0.8.24;\n\nlibrary OutboundVector {{\n\
         \x20   bytes32 constant UPDATE_IMAGE_ID = {};\n\
         \x20   bytes32 constant GENESIS_STATE = {};\n\
         \x20   bytes32 constant STATE_1 = {};\n\
         \x20   bytes32 constant STATE_2 = {};\n\
         \x20   bytes32 constant EXIT_COMMITMENT = {};\n\
         \x20   bytes constant JOURNAL_1 = hex\"{}\";\n\
         \x20   bytes constant JOURNAL_2 = hex\"{}\";\n}}\n",
        r1.user_cycles, r2.user_cycles,
        h0x(&update_image_id_trusted_tip()), h0x(&g.g2.commitment()), h0x(&p1.next.g2.commitment()), h0x(&p2.next.g2.commitment()),
        h0x(&e.commitment), hex::encode(&p1.journal), hex::encode(&p2.journal),
    );
    std::fs::write(a.get("out")?, sol)?;
    println!("wrote {}", a.get("out")?);
    Ok(())
}

/// Replays natively from block `--from` (genesis state there) to just before each `--at` block,
/// then executes the TrustedTip guest on that one block. Prints user cycles per block kind.
fn cmd_measure(a: &Args) -> Result<()> {
    let s = a.store()?;
    let bridge = parse_account(a.get("bridge")?)?;
    let mut st = HostState::genesis(bridge, s.block_ref(a.u64("from")?)?);
    let mut at: Vec<u64> = a.get("at")?.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>()?;
    at.sort_unstable();
    println!("| block | bytes | txs | private verified | inserts | user cycles | parse+hash | extract | verify | acc | total cycles | segments |");
    println!("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for b in at {
        // Native catch-up to b - 1. A batch must end with the bridge initialised, so before
        // init, the state simply moves its tip (nothing of ours has happened yet).
        if b - 1 > st.g2.tip.id {
            let blocks = s.range(st.g2.tip.id + 1, b - 1)?;
            match prepare(&st, blocks) {
                Ok(p) => st = p.next,
                Err(e) if e.to_string().contains("not initialised") => {
                    st = HostState::genesis(bridge, s.block_ref(b - 1)?);
                }
                Err(e) => return Err(e),
            }
        }
        let raw = s.get(b)?;
        let p = prepare(&st, vec![raw.clone()])?;
        let r = execute_trusted_tip(&p)?;
        let ntx = wire::parse_block(&raw).map(|x| x.txs.len()).unwrap_or(0);
        println!(
            "| {b} | {} | {ntx} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            raw.len(), r.stats.private_verified, r.stats.inserts, r.user_cycles, r.stats.parse_hash, r.stats.extract,
            r.stats.verify, r.stats.acc, r.total_cycles, r.segments
        );
        st = p.next;
    }
    let _ = JOURNAL_VERSION;
    Ok(())
}
