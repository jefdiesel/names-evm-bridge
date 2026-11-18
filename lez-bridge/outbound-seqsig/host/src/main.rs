// SPDX-License-Identifier: MIT OR Apache-2.0
//! `seqsig`: the "sequencer signatures" route's tools (branch `seq-sig`). EXECUTOR ONLY: nothing
//! here proves. The signer keys are STAND-INS (keys of ours), not LEZ sequencer keys.
//!
//!   seqsig ids
//!   seqsig keygen  --out KEYFILE                    a stand-in signer key (32 random bytes, mode 600);
//!                                                   prints PUBKEY= (never the key)
//!   seqsig set     --keys PK,PK,.. --out SETFILE    the signer set (keys sorted); prints SET_HASH=
//!   seqsig attest  --key KEYFILE --rpc URL --dir BLOCKS --state STATE --cfg-id H --to N
//!                  --record FILE [--final-depth D] [--test-conflicting-hash H] --out CPFILE
//!                  the attestor: REPLAYS blocks tip+1..N itself (outbound_host::prepare: the hash
//!                  chain recomputed, every claim, burn, root and registry change extracted as the
//!                  bridge would), refuses unless the replay succeeds and its cfgId is --cfg-id, then
//!                  signs the checkpoint (cfgId, the state's LEZ tip, block N and its hash) with one
//!                  key and writes the replay's result next to the signature. FINALITY: this chain
//!                  has no Bedrock finality, so the rule is depth: block N must be at least D blocks
//!                  under the sequencer's head (default 20, the public demo updater's head-20).
//!                  RECORD: an append-only file of every checkpoint this key signed; a second, other
//!                  hash for a (cfgId, prevId, prevHash, blockId) it already signed is refused. It does NOT verify
//!                  privacy receipts (LEZ rc3 makes them with risc0 3.0.5, which this 3.1 host
//!                  refuses) or public execution: a production signer must. TEST ONLY:
//!                  --test-conflicting-hash signs another hash for block N after the replay, without
//!                  the record (a dishonest signer: equivocation, to exercise the gate's contest)
//!   seqsig update  --rpc URL --dir BLOCKS --state STATE --to N --set SETFILE --threshold T
//!                  [--ejected MASK] --sigs CP,CP,.. --out DIR [--expect-refusal] [--outsider-index I]
//!                  G2 natively (outbound_host::prepare, TrustedTip at block N), the signature check
//!                  natively, then the seq-sig guest in RISC Zero's executor; the TrustedTip guest on
//!                  the same batch for the cycle difference. Writes journal.hex (the guest's whole
//!                  journal), vault-journal.hex, seal.hex (the test MockVerifier's seal over the
//!                  whole journal), next-state.bin/.json, report.json. With --expect-refusal it
//!                  succeeds only if the guest refuses (no journal).
//!   seqsig evidence --checkpoints CP,CP,.. --set SETFILE --out DIR
//!                  the evidence guest in the executor over one checkpoint signed by one or more set
//!                  keys: journal.hex, seal.hex, report.json

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use outbound_core::finality::BlockRef;
use outbound_host::{HostState, Store, decode_journal, h0x, mock_seal, parse_hex, prepare, sha256};
use seqsig_core::{Checkpoint, Evidence, Sig, Witness};
use seqsig_host::{OUTBOUND_GUEST_SEQSIG_ELF, SEQSIG_EVIDENCE_GUEST_ELF, evidence_image, run_guest, update_image};
use std::{collections::HashMap, io::Read as _, path::Path};


const LABEL: &str = "mock-verified: proofs executed, not proved; signer keys are STAND-INS (ours), not LEZ sequencer keys";

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
    fn flag(&self, k: &str) -> bool {
        self.1.iter().any(|f| f == k)
    }
}

fn read_secret(p: &str) -> Result<[u8; 32]> {
    let s = std::fs::read_to_string(p).with_context(|| format!("read key file {p}"))?;
    parse_hex::<32>(s.trim())
}

fn write_private(p: &Path, s: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).with_context(|| format!("create {}", p.display()))?;
    std::io::Write::write_all(&mut f, s.as_bytes())?;
    Ok(())
}

fn load_set(p: &str) -> Result<Vec<[u8; 32]>> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
    let keys = v["keys"].as_array().ok_or_else(|| anyhow!("{p}: no keys"))?;
    keys.iter().map(|k| parse_hex::<32>(k.as_str().unwrap_or_default())).collect()
}

fn cp_json(cp: &Checkpoint, key: &[u8; 32], sig: &[u8; 64]) -> serde_json::Value {
    serde_json::json!({
        "cfgId": h0x(&cp.cfg_id), "prevId": cp.prev_id, "prevHash": h0x(&cp.prev_hash),
        "blockId": cp.block_id, "blockHash": h0x(&cp.block_hash),
        "message": h0x(&cp.message()), "key": h0x(key), "sig": h0x(sig),
    })
}

fn load_cp(p: &str) -> Result<(Checkpoint, [u8; 32], [u8; 64])> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).with_context(|| format!("read {p}"))?)?;
    let s = |k: &str| v[k].as_str().map(str::to_string).ok_or_else(|| anyhow!("{p}: no {k}"));
    let n = |k: &str| v[k].as_u64().ok_or_else(|| anyhow!("{p}: no {k}"));
    let cp = Checkpoint {
        cfg_id: parse_hex(&s("cfgId")?)?,
        prev_id: n("prevId")?,
        prev_hash: parse_hex(&s("prevHash")?)?,
        block_id: n("blockId")?,
        block_hash: parse_hex(&s("blockHash")?)?,
    };
    Ok((cp, parse_hex(&s("key")?)?, parse_hex(&s("sig")?)?))
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = argv.split_first().ok_or_else(|| anyhow!("usage: see the crate docs"))?;
    let a = Args::parse(rest);
    match cmd.as_str() {
        "ids" => {
            println!("SEQSIG_UPDATE_IMAGE_ID={}", h0x(&update_image()));
            println!("SEQSIG_EVIDENCE_IMAGE_ID={}", h0x(&evidence_image()));
            println!("TRUSTED_TIP_IMAGE_ID={}", h0x(&outbound_host::update_image_id_trusted_tip()));
            println!("RELEASE_IMAGE_ID={}", h0x(&outbound_host::release_image_id()));
        }
        "keygen" => {
            let mut sk = [0u8; 32];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut sk)?;
            write_private(Path::new(a.get("out")?), &h0x(&sk))?;
            println!("PUBKEY={}", h0x(&seqsig_core::public_key(&sk)));
        }
        "pubkey" => println!("PUBKEY={}", h0x(&seqsig_core::public_key(&read_secret(a.get("key")?)?))),
        "set" => {
            let mut keys = a.get("keys")?.split(',').map(parse_hex::<32>).collect::<Result<Vec<_>>>()?;
            keys.sort();
            let h = seqsig_core::set_hash(&keys).map_err(|e| anyhow!("{e}"))?;
            let v = serde_json::json!({ "keys": keys.iter().map(|k| h0x(k)).collect::<Vec<_>>(), "setHash": h0x(&h) });
            std::fs::write(a.get("out")?, serde_json::to_string_pretty(&v)?)?;
            println!("SET_HASH={}", h0x(&h));
            println!("SIGNERS={}", keys.len());
        }
        "attest" => {
            let sk = read_secret(a.get("key")?)?;
            let state = HostState::load(Path::new(a.get("state")?))?;
            let store = Store::new(a.get("dir")?, a.opt("rpc"))?;
            let to = a.u64("to")?;
            let cfg_id: [u8; 32] = parse_hex(a.get("cfg-id")?)?;
            ensure!(to > state.g2.tip.id, "--to {to} is not after the state's tip {}", state.g2.tip.id);
            // Replay the range before signing: linkage, hashes, extraction (natively).
            let p = prepare(&state, store.range(state.g2.tip.id + 1, to)?).context("replay refused: not signing")?;
            ensure!(p.out.journal.cfg_id == cfg_id, "replay: the bridge's cfgId is {}, not --cfg-id; not signing", h0x(&p.out.journal.cfg_id));
            let BlockRef { id, hash } = store.block_ref(to)?;
            ensure!(p.next.g2.tip == BlockRef { id, hash }, "replay ended at another block; not signing");
            let depth: u64 = a.opt("final-depth").map(|d| d.parse()).transpose()?.unwrap_or(20);
            let head = outbound_host::last_block_id(a.get("rpc")?)?;
            ensure!(to + depth <= head, "block {to} is not final by this attestor's rule (head {head}, depth {depth}); not signing");
            let block_hash = match a.opt("test-conflicting-hash") {
                Some(h) => parse_hex::<32>(&h)?,
                None => hash,
            };
            let cp = Checkpoint { cfg_id, prev_id: state.g2.tip.id, prev_hash: state.g2.tip.hash, block_id: id, block_hash };
            if block_hash == hash {
                let new = seqsig_host::record_and_check(Path::new(a.get("record")?), &cp)?;
                if !new {
                    println!("note: this checkpoint is already in the signing record (same hash); signing it again");
                }
            } else {
                println!("TEST: --test-conflicting-hash: signing another hash for block {id} WITHOUT the record (a dishonest signer)");
            }
            let sig = seqsig_core::sign(&sk, &cp);
            let pk = seqsig_core::public_key(&sk);
            let j = decode_journal(&p.journal)?;
            let mut v = cp_json(&cp, &pk, &sig);
            v["replay"] = serde_json::json!({
                "blocks": [state.g2.tip.id + 1, to], "newState": h0x(&j.new_state), "claims": j.claims.len(), "exits": j.exits.len(),
                "depositRootsUsed": j.deposit_roots_used.len(),
                "notChecked": "privacy receipts (taken on the sequencer's word) and public execution",
            });
            std::fs::write(a.get("out")?, serde_json::to_string_pretty(&v)?)?;
            println!("replayed blocks {}..{to} ({} claim deltas, {} exits; receipts NOT checked); signed checkpoint: block {id} hash {} (prev {} {}), key {}{}",
                state.g2.tip.id + 1, j.claims.len(), j.exits.len(), h0x(&block_hash), cp.prev_id, h0x(&cp.prev_hash), h0x(&pk),
                if block_hash == hash { "" } else { "  [TEST: NOT the chain's hash: an equivocating signature]" });
        }
        "update" => update_cmd(&a)?,
        "evidence" => {
            let keys = load_set(a.get("set")?)?;
            let mut cp0 = None;
            let mut sigs = Vec::new();
            for f in a.get("checkpoints")?.split(',').filter(|s| !s.is_empty()) {
                let (cp, key, sig) = load_cp(f)?;
                ensure!(cp0.is_none_or(|c| c == cp), "{f}: another checkpoint than the first file's");
                cp0 = Some(cp);
                let idx = keys.iter().position(|k| *k == key).ok_or_else(|| anyhow!("{f}: key not in the set"))?;
                sigs.push(Sig { key_index: idx as u32, sig });
            }
            let cp = cp0.ok_or_else(|| anyhow!("no checkpoints"))?;
            let e = Evidence { checkpoint: cp, keys, sigs };
            let native = seqsig_core::evidence_journal(&e).map_err(|err| anyhow!("evidence refused natively: {err}"))?;
            let run = run_guest(SEQSIG_EVIDENCE_GUEST_ELF, evidence_image(), &[&borsh::to_vec(&e)?])?;
            ensure!(run.journal == native, "evidence guest journal differs from the native one");
            let out = Path::new(a.get("out")?);
            std::fs::create_dir_all(out)?;
            std::fs::write(out.join("journal.hex"), h0x(&run.journal))?;
            std::fs::write(out.join("seal.hex"), h0x(&mock_seal(&evidence_image(), &run.journal)))?;
            let check_cycles = u64::from_le_bytes(run.stdout.get(..8).and_then(|s| s.try_into().ok()).unwrap_or_default());
            let bitmap = e.sigs.iter().fold(0u64, |m, s| m | 1u64 << s.key_index);
            let r = serde_json::json!({
                "label": LABEL, "imageId": h0x(&evidence_image()), "journalDigest": h0x(&sha256(&run.journal)),
                "blockId": cp.block_id, "blockHash": h0x(&cp.block_hash), "prevId": cp.prev_id, "prevHash": h0x(&cp.prev_hash),
                "signerBitmap": bitmap, "signatures": e.sigs.len(),
                "userCycles": run.user_cycles, "totalCycles": run.total_cycles, "segments": run.segments, "signatureCheckCycles": check_cycles,
            });
            std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&r)?)?;
            println!("EVIDENCE_JOURNAL={}", h0x(&run.journal));
            println!("evidence guest: {} signature(s), signer bitmap {bitmap:#b}, {} user cycles ({} in the signature check), {} segment(s)  [{LABEL}]",
                e.sigs.len(), run.user_cycles, check_cycles, run.segments);
        }
        _ => bail!("unknown command {cmd}"),
    }
    Ok(())
}

fn update_cmd(a: &Args) -> Result<()> {
    let state = HostState::load(Path::new(a.get("state")?))?;
    let store = Store::new(a.get("dir")?, a.opt("rpc"))?;
    let to = a.u64("to")?;
    let from = state.g2.tip.id + 1;
    ensure!(to >= from, "--to {to} is not after the state's tip {}", state.g2.tip.id);
    let keys = load_set(a.get("set")?)?;
    let threshold: u32 = a.get("threshold")?.parse()?;
    let mut sigs = Vec::new();
    let mut cps = Vec::new();
    for f in a.get("sigs")?.split(',').filter(|s| !s.is_empty()) {
        let (cp, key, sig) = load_cp(f)?;
        let idx = match keys.iter().position(|k| *k == key) {
            Some(i) => i,
            // Test only: present a signature by a key outside the set under a set member's index.
            None => match a.opt("outsider-index") {
                Some(i) => {
                    println!("note: {f}: key {} is NOT in the set; presented as key {i} (--outsider-index)", h0x(&key));
                    i.parse()?
                }
                None => bail!("{f}: key {} is not in the set", h0x(&key)),
            },
        };
        sigs.push(Sig { key_index: idx as u32, sig });
        cps.push(cp);
    }
    let ejected: u64 = match a.opt("ejected") {
        Some(m) => m.trim().parse()?,
        None => 0,
    };
    let w = Witness { keys, threshold, ejected, sigs };
    let p = prepare(&state, store.range(from, to)?)?;
    let tip = store.block_ref(to)?;
    let cp = Checkpoint {
        cfg_id: p.out.journal.cfg_id,
        prev_id: state.g2.tip.id,
        prev_hash: state.g2.tip.hash,
        block_id: tip.id,
        block_hash: tip.hash,
    };
    for (i, c) in cps.iter().enumerate() {
        if *c != cp {
            println!("note: signature {i} is over another checkpoint (block {} hash {}), the guest will refuse it", c.block_id, h0x(&c.block_hash));
        }
    }
    let native = seqsig_core::check(&cp, &w);
    println!("checkpoint: cfgId {} prev {} {} -> block {} {}", h0x(&cp.cfg_id), cp.prev_id, h0x(&cp.prev_hash), cp.block_id, h0x(&cp.block_hash));
    println!("native signature check: {}", match &native { Ok(x) => format!("ACCEPTED, {} distinct signers (bitmap {:#b}), threshold {}, ejected mask {:#b}", x.signers, x.bitmap, x.threshold, x.ejected), Err(e) => format!("REFUSED: {e}") });
    let out = Path::new(a.get("out")?);
    std::fs::create_dir_all(out)?;
    let res = seqsig_host::execute_update(&p, &w);
    if a.flag("expect-refusal") {
        let msg = match res {
            Ok(_) => bail!("the seq-sig guest produced a journal; a refusal was expected"),
            Err(e) => format!("{e:#}"),
        };
        ensure!(native.is_err(), "native check accepted but the guest refused: {msg}");
        let r = serde_json::json!({ "label": LABEL, "refused": true, "guestError": msg, "nativeError": native.err().map(|e| e.to_string()),
            "threshold": threshold, "ejectedMask": ejected, "signaturesGiven": w.sigs.len(), "blockId": cp.block_id });
        std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&r)?)?;
        println!("REFUSED by the seq-sig guest (no journal): {msg}");
        return Ok(());
    }
    let run = res.context("the seq-sig guest refused")?;
    let ok = native.map_err(|e| anyhow!("native check refused but the guest accepted: {e}"))?;
    let n = run.journal.len();
    let t = seqsig_core::TAIL_LEN;
    ensure!(n > t && run.journal[..n - t] == p.journal[..], "the guest's journal does not start with the native vault journal");
    ensure!(run.journal[n - t..] == seqsig_core::journal_tail(&ok, &cp), "journal tail");
    let tt = outbound_host::execute_trusted_tip(&p)?;
    let (sig_cycles, given, signers): (u64, u32, u32) = borsh::from_slice(&run.stdout).unwrap_or_default();
    let j = decode_journal(&p.journal)?;
    std::fs::write(out.join("journal.hex"), h0x(&run.journal))?;
    std::fs::write(out.join("vault-journal.hex"), h0x(&p.journal))?;
    std::fs::write(out.join("seal.hex"), h0x(&mock_seal(&update_image(), &run.journal)))?;
    p.next.save(&out.join("next-state.bin"))?;
    let r = serde_json::json!({
        "label": LABEL,
        "imageId": h0x(&update_image()),
        "journalDigest": h0x(&sha256(&run.journal)),
        "cfgId": h0x(&j.cfg_id), "prevState": h0x(&j.prev_state), "newState": h0x(&j.new_state),
        "fromBlock": from, "toBlock": to, "blockHash": h0x(&tip.hash), "prevTipId": cp.prev_id, "prevTipHash": h0x(&cp.prev_hash),
        "signerSetHash": h0x(&ok.set_hash), "threshold": ok.threshold, "ejectedMask": ok.ejected, "signerBitmap": ok.bitmap,
        "signaturesGiven": given, "distinctSigners": signers,
        "seqsigUserCycles": run.user_cycles, "seqsigTotalCycles": run.total_cycles, "seqsigSegments": run.segments,
        "signatureCheckCycles": sig_cycles,
        "trustedTipUserCycles": tt.user_cycles,
        "exits": j.exits.iter().map(|e| serde_json::json!({ "token": h0x(&e.token), "denomIndex": e.denom_index, "commitment": h0x(&e.commitment) })).collect::<Vec<_>>(),
        "claims": j.claims.iter().map(|c| serde_json::json!({ "token": h0x(&c.token), "denomIndex": c.denom_index, "count": c.count })).collect::<Vec<_>>(),
    });
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&r)?)?;
    println!("seq-sig guest: {} user cycles ({} segment(s)); TrustedTip guest on the same batch: {}; difference {}; inside the signature check {} for {} signature(s), {} distinct signers",
        run.user_cycles, run.segments, tt.user_cycles, run.user_cycles as i64 - tt.user_cycles as i64, sig_cycles, given, signers);
    println!("JOURNAL_DIGEST={}", h0x(&sha256(&run.journal)));
    println!("NEW_STATE={}", h0x(&j.new_state));
    println!("[{LABEL}]");
    Ok(())
}
