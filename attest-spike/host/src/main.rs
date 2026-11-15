//! attest-spike host: EXECUTOR ONLY. Runs the guests in RISC Zero's in-process executor
//! and reports cycle counts. There is no prover call anywhere in this program.
//!
//!   host <fixtures-dir>            run everything (native checks, guests, negative tests)
use std::{path::Path, time::Instant};

use alloy_primitives::{keccak256, Bytes, B256, U256};
use anyhow::{anyhow, bail, ensure, Context, Result};
use attest_common::{ConsensusOutput, ConsensusParams, InclusionOutput, InclusionParams};
use helios_consensus_core::{
    apply_bootstrap, apply_finality_update, apply_update,
    consensus_spec::MainnetConsensusSpec as S,
    types::{Bootstrap, FinalityUpdate, Fork, Forks, LightClientHeader, LightClientStore, Update},
    verify_bootstrap, verify_finality_update, verify_update,
};
use methods::{CONSENSUS_GUEST_ELF, INCLUSION_GUEST_ELF, INCLUSION_GUEST_SW_ELF};
use risc0_zkvm::{ExecutorEnv, ExecutorImpl, Session};
use serde_json::Value;
use ssz::Decode;
use tree_hash::TreeHash;

fn h32(s: &str) -> [u8; 32] {
    let t = s.trim_start_matches("0x");
    let t = if t.len() % 2 == 1 { format!("0{t}") } else { t.to_string() };
    let v = hex::decode(t).unwrap();
    let mut o = [0u8; 32];
    o[32 - v.len()..].copy_from_slice(&v);
    o
}
fn hx(s: &str) -> Vec<u8> {
    hex::decode(s.trim().trim_start_matches("0x")).unwrap()
}
fn u64x(s: &str) -> u64 {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap()
}
fn rd(dir: &Path, f: &str) -> Result<Vec<u8>> {
    std::fs::read(dir.join(f)).with_context(|| format!("read {f}"))
}
fn rj(dir: &Path, f: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&rd(dir, f)?)?)
}

struct Net {
    dir: std::path::PathBuf,
    meta: Value,
    forks_raw: [(u64, [u8; 4]); 8],
    genesis_root: [u8; 32],
}

impl Net {
    fn load(dir: &Path) -> Result<Self> {
        let meta = rj(dir, "meta.json")?;
        let fs = rj(dir, "fork_schedule.json")?;
        let sched = fs["data"].as_array().unwrap();
        ensure!(sched.len() >= 8, "fork schedule shorter than genesis..gloas");
        let mut forks_raw = [(0u64, [0u8; 4]); 8];
        for (i, f) in sched.iter().take(8).enumerate() {
            let v = hx(f["current_version"].as_str().unwrap());
            forks_raw[i] = (f["epoch"].as_str().unwrap().parse()?, [v[0], v[1], v[2], v[3]]);
        }
        let genesis_root = h32(meta["genesis_validators_root"].as_str().unwrap());
        Ok(Self { dir: dir.to_path_buf(), meta, forks_raw, genesis_root })
    }
    fn forks(&self) -> Forks {
        let k = |i: usize| Fork {
            epoch: self.forks_raw[i].0,
            fork_version: self.forks_raw[i].1.into(),
        };
        Forks {
            genesis: k(0),
            altair: k(1),
            bellatrix: k(2),
            capella: k(3),
            deneb: k(4),
            electra: k(5),
            fulu: k(6),
            gloas: k(7),
        }
    }
    fn period(&self) -> u64 {
        self.meta["finality_update"]["signature_period"].as_u64().unwrap()
    }
}

/// One guest-A scenario: bootstrap + period updates + finality update.
#[derive(Clone)]
struct Scenario {
    name: String,
    boot: Vec<u8>,
    checkpoint: [u8; 32],
    updates: Vec<Vec<u8>>,
    fin: Vec<u8>,
    expected_current_slot: u64,
}

fn scenario(net: &Net, name: &str, boot: &str, updates: &[String]) -> Result<Scenario> {
    let bmeta = &net.meta[if boot == "bootstrap_prev_period" { "bootstrap_prev_period" } else { "bootstrap_same_period" }];
    let fin = rd(&net.dir, "finality_update.ssz")?;
    Ok(Scenario {
        name: name.into(),
        boot: rd(&net.dir, &format!("{boot}.ssz"))?,
        checkpoint: h32(bmeta["block_root"].as_str().unwrap()),
        updates: updates.iter().map(|u| rd(&net.dir, u)).collect::<Result<_>>()?,
        fin,
        expected_current_slot: net.meta["finality_update"]["signature_slot"].as_u64().unwrap(),
    })
}

/// Same steps as the guest, natively, with the same Helios code. Returns the finalized slot.
fn native_consensus(net: &Net, sc: &Scenario) -> Result<(u64, B256)> {
    let forks = net.forks();
    let g = B256::from(net.genesis_root);
    let b = Bootstrap::<S>::from_ssz_bytes(&sc.boot).map_err(|e| anyhow!("{e:?}"))?;
    ensure!(matches!(b.header(), LightClientHeader::Gloas(_)) == (net.meta["network"] == "sepolia"));
    verify_bootstrap(&b, B256::from(sc.checkpoint), &forks).map_err(|e| anyhow!("{e}"))?;
    let mut store = LightClientStore::<S>::default();
    apply_bootstrap(&mut store, &b);
    for u in &sc.updates {
        let u = Update::<S>::from_ssz_bytes(u).map_err(|e| anyhow!("{e:?}"))?;
        verify_update(&u, sc.expected_current_slot, &store, g, &forks).map_err(|e| anyhow!("{e}"))?;
        apply_update(&mut store, &u);
    }
    let f = FinalityUpdate::<S>::from_ssz_bytes(&sc.fin).map_err(|e| anyhow!("{e:?}"))?;
    // SSZ and the JSON the beacon node served must describe the same update.
    let fj: FinalityUpdate<S> =
        serde_json::from_value(rj(&net.dir, "finality_update.json")?["data"].clone())?;
    ensure!(
        fj.finalized_header().beacon().tree_hash_root() == f.finalized_header().beacon().tree_hash_root()
            && fj.signature_slot() == f.signature_slot(),
        "finality_update.json and .ssz disagree"
    );
    verify_finality_update(&f, sc.expected_current_slot, &store, g, &forks).map_err(|e| anyhow!("{e}"))?;
    apply_finality_update(&mut store, &f);
    ensure!(store.finalized_header.beacon().slot == f.finalized_header().beacon().slot, "not applied");
    let exec = match &store.finalized_header {
        LightClientHeader::Gloas(h) => h.execution_block_hash,
        h => *h.execution().unwrap().state_root(),
    };
    Ok((store.finalized_header.beacon().slot, exec))
}

fn consensus_env(net: &Net, sc: &Scenario, mode: u32) -> Result<ExecutorEnv<'static>> {
    let p = ConsensusParams {
        mode,
        genesis_validators_root: net.genesis_root,
        checkpoint: sc.checkpoint,
        forks: net.forks_raw,
        expected_current_slot: sc.expected_current_slot,
        n_updates: sc.updates.len() as u32,
    };
    let mut b = ExecutorEnv::builder();
    b.write(&p)?;
    b.write_frame(&sc.boot);
    for u in &sc.updates {
        b.write_frame(u);
    }
    b.write_frame(&sc.fin);
    b.build()
}

fn execute(env: ExecutorEnv<'_>, elf: &[u8]) -> Result<(Session, f64)> {
    let t = Instant::now();
    let s = ExecutorImpl::from_elf(env, elf)?.run()?;
    Ok((s, t.elapsed().as_secs_f64()))
}

fn print_session(label: &str, s: &Session, secs: f64) {
    println!(
        "  SESSION {label}: user_cycles={} paging_cycles={} reserved_cycles={} total_cycles={} segments={} (segment limit 2^20) exec={secs:.1}s",
        s.user_cycles, s.paging_cycles, s.reserved_cycles, s.total_cycles, s.segments.len()
    );
}

fn print_marks(marks: &[(String, u64)]) {
    for (l, c) in marks {
        println!("    {c:>12}  {l}");
    }
    println!("    {:>12}  (sum of marks)", marks.iter().map(|m| m.1).sum::<u64>());
}

fn run_consensus(net: &Net, sc: &Scenario, mode: u32) -> Result<ConsensusOutput> {
    let (s, secs) = execute(consensus_env(net, sc, mode)?, CONSENSUS_GUEST_ELF)?;
    let out: ConsensusOutput = s.journal.as_ref().unwrap().decode()?;
    println!(
        "== guest A [{}]{}  finalized slot {} root 0x{} exec({}) 0x{}",
        sc.name,
        if mode == 1 { " +breakdown" } else { "" },
        out.finalized_slot,
        hex::encode(out.finalized_header_root),
        if out.execution_kind == 0 { "block_hash" } else { "state_root" },
        hex::encode(out.execution_commitment)
    );
    print_session(&sc.name, &s, secs);
    print_marks(&out.marks);
    Ok(out)
}

/// Run a tampered input and require a guest panic whose message contains `want`.
fn expect_reject(label: &str, env: ExecutorEnv<'_>, elf: &[u8], want: &str, bad: &mut u32) {
    match execute(env, elf) {
        Ok((s, _)) => {
            *bad += 1;
            println!("  NEG {label}: ACCEPTED (exit {:?}) <-- FAILURE", s.exit_code);
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let first = msg.lines().next().unwrap_or("");
            if msg.contains(want) {
                println!("  NEG {label}: rejected as expected: {first}");
            } else {
                *bad += 1;
                println!("  NEG {label}: rejected for the WRONG reason (wanted '{want}'): {first}");
            }
        }
    }
}

// ---- guest B ----

#[derive(Clone)]
struct Incl {
    params: InclusionParams,
    header: Vec<u8>,
    account_proof: Vec<Vec<u8>>,
    storage_proof: Vec<Vec<u8>>,
}

fn inclusion_input(net: &Net, block_hash: [u8; 32]) -> Result<Incl> {
    let st = &net.meta["storage"];
    let pf = rj(&net.dir, "eth_getProof.json")?;
    let header = hx(std::str::from_utf8(&rd(&net.dir, "el_block_header.rlp.hex")?)?);
    let sp = &pf["storageProof"][0];
    let mut address = [0u8; 20];
    address.copy_from_slice(&hx(pf["address"].as_str().unwrap()));
    let mapping_key = h32(st["mapping_key"].as_str().unwrap());
    let n = st["mapping_slot_index"].as_u64().unwrap();
    // Host-side derivation of the slot key, checked against the key the proof was fetched for.
    let mut pre = [0u8; 64];
    pre[..32].copy_from_slice(&mapping_key);
    pre[32..].copy_from_slice(&U256::from(n).to_be_bytes::<32>());
    ensure!(keccak256(pre) == B256::from(h32(sp["key"].as_str().unwrap())), "slot key derivation mismatch");
    let params = InclusionParams {
        chain_id: net.meta["chain_id"].as_u64().unwrap(),
        block_hash,
        address,
        mapping_key,
        mapping_slot_index: n,
        nonce: u64x(pf["nonce"].as_str().unwrap()),
        balance: h32(pf["balance"].as_str().unwrap()),
        storage_root: h32(pf["storageHash"].as_str().unwrap()),
        code_hash: h32(pf["codeHash"].as_str().unwrap()),
        value: h32(sp["value"].as_str().unwrap()),
        n_account_nodes: pf["accountProof"].as_array().unwrap().len() as u32,
        n_storage_nodes: sp["proof"].as_array().unwrap().len() as u32,
    };
    let nodes = |v: &Value| v.as_array().unwrap().iter().map(|x| hx(x.as_str().unwrap())).collect();
    Ok(Incl { params, header, account_proof: nodes(&pf["accountProof"]), storage_proof: nodes(&sp["proof"]) })
}

/// Native alloy-trie check against the state root from the RPC's header JSON (independent of
/// the guest's RLP parsing).
fn native_inclusion(i: &Incl, state_root: B256) -> Result<()> {
    use alloy_trie::{proof::verify_proof, Nibbles, TrieAccount};
    ensure!(keccak256(&i.header) == B256::from(i.params.block_hash), "header hash");
    let p = &i.params;
    let acct = TrieAccount {
        nonce: p.nonce,
        balance: U256::from_be_bytes(p.balance),
        storage_root: B256::from(p.storage_root),
        code_hash: B256::from(p.code_hash),
    };
    let ap: Vec<Bytes> = i.account_proof.iter().cloned().map(Bytes::from).collect();
    let sp: Vec<Bytes> = i.storage_proof.iter().cloned().map(Bytes::from).collect();
    verify_proof(state_root, Nibbles::unpack(keccak256(p.address)), Some(alloy_rlp::encode(&acct)), &ap)
        .map_err(|e| anyhow!("account: {e}"))?;
    let mut pre = [0u8; 64];
    pre[..32].copy_from_slice(&p.mapping_key);
    pre[32..].copy_from_slice(&U256::from(p.mapping_slot_index).to_be_bytes::<32>());
    verify_proof(
        acct.storage_root,
        Nibbles::unpack(keccak256(keccak256(pre))),
        Some(alloy_rlp::encode(U256::from_be_bytes(p.value))),
        &sp,
    )
    .map_err(|e| anyhow!("storage: {e}"))?;
    Ok(())
}

fn inclusion_env(i: &Incl) -> Result<ExecutorEnv<'static>> {
    let mut b = ExecutorEnv::builder();
    b.write(&i.params)?;
    b.write_frame(&i.header);
    for n in &i.account_proof {
        b.write_frame(n);
    }
    for n in &i.storage_proof {
        b.write_frame(n);
    }
    b.build()
}

fn run_inclusion(label: &str, i: &Incl, elf: &[u8]) -> Result<InclusionOutput> {
    let (s, secs) = execute(inclusion_env(i)?, elf)?;
    let out: InclusionOutput = s.journal.as_ref().unwrap().decode()?;
    println!(
        "== guest B [{label}] chain {} block {} state_root 0x{} slot 0x{} value 0x{}",
        out.chain_id,
        out.block_number,
        hex::encode(out.state_root),
        hex::encode(out.slot_key),
        hex::encode(out.value)
    );
    print_session(label, &s, secs);
    print_marks(&out.marks);
    Ok(out)
}

/// Helios's `subgroup_check_g2` computes `sig * Scalar::from_raw(r)`. Upstream `from_raw`
/// reduces mod r, so natively the scalar is 0 and the check always passes. Probe it on an
/// on-curve G2 point outside the subgroup with the upstream crate the host links.
fn native_subgroup_probe() {
    use bls12_381::{G2Affine, Scalar};
    use sha2::{Digest, Sha256};
    let r = Scalar::from_raw([
        0xffff_ffff_0000_0001,
        0x53bd_a402_fffe_5bfe,
        0x3339_d808_09a1_d805,
        0x73ed_a753_299d_7d48,
    ]);
    let mut ctr = 0u32;
    let q = loop {
        let h: [u8; 32] = Sha256::digest(&ctr.to_le_bytes()).into();
        let mut b = [0u8; 96];
        b[0] = 0x80;
        b[1..33].copy_from_slice(&h);
        b[49..81].copy_from_slice(&h);
        ctr += 1;
        if let Some(p) = Option::<G2Affine>::from(G2Affine::from_compressed_unchecked(&b)) {
            break p;
        }
    };
    let tf: bool = q.is_torsion_free().into();
    let id: bool = (q * r).is_identity().into();
    let rejected = bool::from(G2Affine::from_compressed(&q.to_compressed()).is_none());
    println!(
        "  native probe (upstream bls12_381): off-subgroup G2 point: torsion_free={tf}, Helios check (q*r==O)={id}, from_compressed rejects={rejected}, from_raw(r)==0: {}",
        r == Scalar::zero()
    );
}

fn main() -> Result<()> {
    let root = std::env::args().nth(1).unwrap_or_else(|| "fixtures".into());
    let only = std::env::args().nth(2).unwrap_or_default();
    let root = Path::new(&root);
    let mut bad = 0u32;

    // ---------------- Sepolia (Gloas) ----------------
    let sep = Net::load(&root.join("sepolia"))?;
    let p = sep.period();
    let same = scenario(&sep, "sepolia same-period finality", "bootstrap_same_period", &[])?;
    let hand = scenario(
        &sep,
        "sepolia committee handoff",
        "bootstrap_prev_period",
        &[format!("update_period_{}.ssz", p - 1)],
    )?;
    println!("# native Helios 0.12.0 (host) on the same inputs");
    native_subgroup_probe();
    for sc in [&same, &hand] {
        let (slot, exec) = native_consensus(&sep, sc)?;
        println!("  native [{}]: OK finalized slot {slot} exec 0x{}", sc.name, hex::encode(exec));
    }

    let out_same = run_consensus(&sep, &same, 0)?;
    if only != "fast" {
        run_consensus(&sep, &hand, 0)?;
        run_consensus(&sep, &same, 1)?;
    }

    // guest B on the block hash guest A committed
    ensure!(out_same.execution_kind == 0, "expected a Gloas block-hash commitment on Sepolia");
    let st = &sep.meta["storage"];
    ensure!(
        B256::from(out_same.execution_commitment) == B256::from(h32(st["block_hash"].as_str().unwrap())),
        "eth_getProof fixture is not for the block guest A finalized"
    );
    let blk = rj(&sep.dir, "el_block_header.json")?;
    let incl = inclusion_input(&sep, out_same.execution_commitment)?;
    native_inclusion(&incl, B256::from(h32(blk["stateRoot"].as_str().unwrap())))?;
    println!("  native alloy-trie: account + storage proof OK");
    let ob = run_inclusion("sepolia mapping(bytes32=>bool) entry, keccak accelerated", &incl, INCLUSION_GUEST_ELF)?;
    ensure!(B256::from(ob.state_root) == B256::from(h32(blk["stateRoot"].as_str().unwrap())));
    if only != "fast" {
        run_inclusion("same, software keccak (no accelerator)", &incl, INCLUSION_GUEST_SW_ELF)?;
    }

    // ---------------- negative tests ----------------
    println!("# negative tests (each must panic in the executor for the stated reason)");
    let fin = &same.fin;
    let upd_cur = rd(&sep.dir, &format!("update_period_{p}.ssz"))?;
    // Gloas FinalityUpdate SSZ layout (all fixed size): attested_header 0..496,
    // finalized_header 496..992, finality_branch 992..1280, sync_committee_bits 1280..1344,
    // signature 1344..1440, signature_slot 1440..1448. Gloas Update: signature at len-104..len-8.
    ensure!(fin.len() == 1448 && upd_cur.len() == 26424, "unexpected Gloas SSZ sizes");
    let mut t = same.clone();
    t.fin[1344..1440].copy_from_slice(&upd_cur[26424 - 104..26424 - 8]);
    expect_reject("A: valid G2 signature from another update (wrong message)", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid sync committee signature", &mut bad);
    let mut t = same.clone();
    t.fin[1344 + 60] ^= 0x01;
    expect_reject("A: signature byte flipped", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid sync committee signature", &mut bad);
    let mut t = same.clone();
    let byte = (0..64).find(|i| t.fin[1280 + i] != 0).unwrap();
    let bit = (0..8).find(|b| t.fin[1280 + byte] >> b & 1 == 1).unwrap();
    t.fin[1280 + byte] ^= 1 << bit;
    expect_reject("A: one participation bit cleared (wrong signer set)", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid sync committee signature", &mut bad);
    let mut t = same.clone();
    t.boot[496 + 48 * 5 + 20] ^= 0x01;
    expect_reject("A: bootstrap committee pubkey altered (wrong committee)", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid current sync committee proof", &mut bad);
    let mut t = same.clone();
    t.checkpoint[0] ^= 0x01;
    expect_reject("A: wrong trusted checkpoint root", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid header hash", &mut bad);
    let mut t = same.clone();
    t.fin[496 + 16 + 5] ^= 0x01; // finalized beacon header parent_root byte
    expect_reject("A: finalized header altered", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid finality proof", &mut bad);
    let mut t = same.clone();
    t.fin[496 + 112 + 3] ^= 0x01; // finalized header execution_block_hash byte
    expect_reject("A: finalized execution block hash altered", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid execution payload proof", &mut bad);
    let mut t = hand.clone();
    t.updates.clear();
    expect_reject("A: next-period finality update without the handoff update", consensus_env(&sep, &t, 0)?, CONSENSUS_GUEST_ELF, "invalid sync committee period", &mut bad);

    let mut t = incl.clone();
    let last = t.storage_proof.len() - 1;
    let l = t.storage_proof[last].len();
    t.storage_proof[last][l - 1] ^= 0x01;
    expect_reject("B: storage proof leaf byte flipped", inclusion_env(&t)?, INCLUSION_GUEST_ELF, "storage proof invalid", &mut bad);
    let mut t = incl.clone();
    t.params.value = [0u8; 32];
    t.params.value[31] = 2;
    expect_reject("B: claimed value 2 instead of 1", inclusion_env(&t)?, INCLUSION_GUEST_ELF, "storage proof invalid", &mut bad);
    let mut t = incl.clone();
    t.params.mapping_key[31] ^= 0x01;
    expect_reject("B: different mapping key (absent entry claimed true)", inclusion_env(&t)?, INCLUSION_GUEST_ELF, "storage proof invalid", &mut bad);
    let mut t = incl.clone();
    t.params.storage_root[0] ^= 0x01;
    expect_reject("B: account storage_root altered", inclusion_env(&t)?, INCLUSION_GUEST_ELF, "account proof invalid", &mut bad);
    let mut t = incl.clone();
    t.header[100] ^= 0x01;
    expect_reject("B: execution header altered", inclusion_env(&t)?, INCLUSION_GUEST_ELF, "does not hash to the finalized block hash", &mut bad);

    // ---------------- mainnet (Fulu), same-period only ----------------
    if root.join("mainnet/meta.json").exists() && only != "fast" {
        let mn = Net::load(&root.join("mainnet"))?;
        let sc = scenario(&mn, "mainnet same-period finality (Fulu)", "bootstrap_same_period", &[])?;
        let (slot, exec) = native_consensus(&mn, &sc)?;
        println!("  native [{}]: OK finalized slot {slot} state_root 0x{}", sc.name, hex::encode(exec));
        run_consensus(&mn, &sc, 0)?;
        run_consensus(&mn, &sc, 1)?;
    }

    if bad > 0 {
        bail!("{bad} negative test(s) misbehaved");
    }
    println!("# all negative tests rejected for the expected reason");
    Ok(())
}
