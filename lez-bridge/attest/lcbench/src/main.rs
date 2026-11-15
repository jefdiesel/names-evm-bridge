//! lcbench (spike, branch `lc-cycles`): EXECUTOR ONLY. Prepares the recorded fixtures for the
//! lcbench guests and prints their phase marks (user cycles). Nothing here proves.
//!
//!   lcbench            Sepolia finality update, mainnet finality update, Sepolia period update

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use eth_attest_core::{Fork, InstanceConfig, Instruction, LcSummary, LcUpdate, MIN_SUPERMAJORITY, Message, committee_hash};
use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec as S,
    types::{Bootstrap, FinalityUpdate, GenericUpdate, SyncCommittee, Update},
};
use lcbench_common::{KEY_DEPTH, KeyWitness, Keys, Report, SigCase, ZkInput};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::ReceiveInput,
};
use risc0_zkvm::{ExecutorEnv, ExecutorImpl, ExitCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

const ATTEST: AccountId = AccountId::new([0xE7; 32]);
const BOB: AccountId = AccountId::new([0xB1; 32]);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn fx(net: &str, f: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(net).join(f)).unwrap_or_else(|e| panic!("{net}/{f}: {e}"))
}

fn json(net: &str, f: &str) -> Value {
    serde_json::from_slice(&fx(net, f)).unwrap()
}

fn hx(s: &str) -> Vec<u8> {
    hex::decode(s.trim().trim_start_matches("0x")).unwrap()
}

fn h32(s: &str) -> [u8; 32] {
    let t = s.trim_start_matches("0x");
    let t = if t.len() % 2 == 1 { format!("0{t}") } else { t.to_string() };
    let v = hex::decode(t).unwrap();
    let mut o = [0u8; 32];
    o[32 - v.len()..].copy_from_slice(&v);
    o
}

/// The same instance config `attest/cycles` builds.
fn config(net: &str, boot: &str) -> InstanceConfig {
    let meta = json(net, "meta.json");
    let sched = json(net, "fork_schedule.json");
    let sched = sched["data"].as_array().unwrap();
    let forks: [Fork; 8] = core::array::from_fn(|i| {
        let v = hx(sched[i]["current_version"].as_str().unwrap());
        Fork { epoch: sched[i]["epoch"].as_str().unwrap().parse().unwrap(), version: [v[0], v[1], v[2], v[3]] }
    });
    InstanceConfig {
        evm_chain_id: meta["chain_id"].as_u64().unwrap(),
        genesis_validators_root: h32(meta["genesis_validators_root"].as_str().unwrap()),
        genesis_time: meta["genesis_time"].as_u64().unwrap(),
        seconds_per_slot: 12,
        forks,
        checkpoint: h32(meta[boot]["block_root"].as_str().unwrap()),
        min_participants: MIN_SUPERMAJORITY,
        salt: [0; 32],
    }
}

fn sha256_pair(a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

fn key_leaf(k: &[u8]) -> [u8; 32] {
    let mut b = [0u8; 64];
    b[..48].copy_from_slice(k);
    Sha256::digest(&b).into()
}

/// Helios `compute_committee_sign_root` with `calculate_fork_version(signature_slot - 1)`.
fn signing_root(cfg: &InstanceConfig, g: &GenericUpdate<S>) -> [u8; 32] {
    let epoch = g.signature_slot.saturating_sub(1) / 32;
    let version = cfg.forks.iter().rev().find(|f| epoch >= f.epoch).map_or(cfg.forks[0].version, |f| f.version);
    let mut v32 = [0u8; 32];
    v32[..4].copy_from_slice(&version);
    let fork_data_root = sha256_pair(&v32, &cfg.genesis_validators_root);
    let mut domain = [0u8; 32];
    domain[..4].copy_from_slice(&[7, 0, 0, 0]);
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    sha256_pair(g.attested_header.beacon().tree_hash_root().as_slice(), &domain)
}

fn uncompressed(k: &[u8; 48]) -> [u8; 96] {
    Option::<bls12_381::G1Affine>::from(bls12_381::G1Affine::from_compressed(k)).expect("committee key decompresses").to_uncompressed()
}

/// The witness for an aggregate-minus-`absent` check against this committee.
fn keys(committee_ssz: &[u8], absent: &[usize]) -> Keys {
    let key = |i: usize| -> [u8; 48] { committee_ssz[i * 48..i * 48 + 48].try_into().unwrap() };
    let mut layers: Vec<Vec<[u8; 32]>> = vec![(0..512).map(|i| key_leaf(&key(i))).collect()];
    while layers.last().unwrap().len() > 1 {
        let l = layers.last().unwrap();
        layers.push(l.chunks(2).map(|p| sha256_pair(&p[0], &p[1])).collect());
    }
    let pubkeys_root = layers[KEY_DEPTH][0];
    let committee_root = sha256_pair(&pubkeys_root, &key_leaf(&key(512)));
    let sc = SyncCommittee::<S>::from_ssz_bytes(committee_ssz).unwrap();
    assert_eq!(committee_root, sc.tree_hash_root().0, "branch tree disagrees with tree_hash");
    Keys {
        agg_compressed: key(512),
        agg_uncompressed: uncompressed(&key(512)),
        absent: absent
            .iter()
            .map(|&i| KeyWitness {
                index: u16::try_from(i).unwrap(),
                compressed: key(i),
                uncompressed: uncompressed(&key(i)),
                branch: core::array::from_fn(|l| layers[l][(i >> l) ^ 1]),
            })
            .collect(),
        pubkeys_root,
        committee_root,
    }
}

fn update_input(cfg: &InstanceConfig, prev: &LcSummary, current: &[u8], u: LcUpdate) -> ReceiveInput {
    assert_eq!(committee_hash(current), prev.current_committee);
    ReceiveInput {
        receiver: Actor::new(BOB, ATTEST),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: borsh::to_vec(&Message::Instruction(Instruction::Update {
            config: cfg.clone(),
            prev: prev.clone(),
            current_committee: current.to_vec(),
            next_committee: None,
            update: u,
        }))
        .unwrap(),
    }
}

fn zk_case(net: &str, boot_name: &str, update: LcUpdate) -> Result<ZkInput> {
    let cfg = config(net, boot_name);
    let boot = fx(net, &format!("{boot_name}.ssz"));
    let genesis = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg, &boot).map_err(|e| anyhow!(e))?;
    let committee_ssz = Bootstrap::<S>::from_ssz_bytes(&boot).unwrap().current_sync_committee().as_ssz_bytes();
    let g: GenericUpdate<S> = match &update {
        LcUpdate::Finality(b) => GenericUpdate::from(&FinalityUpdate::<S>::from_ssz_bytes(b).unwrap()),
        LcUpdate::Period(b) => GenericUpdate::from(&Update::<S>::from_ssz_bytes(b).unwrap()),
    };
    let bits: Vec<bool> = g.sync_aggregate.sync_committee_bits.iter().collect();
    let absent: Vec<usize> = (0..512).filter(|&i| !bits[i]).collect();
    let sig = SigCase {
        keys: keys(&committee_ssz, &absent),
        bits,
        signing_root: signing_root(&cfg, &g),
        signature: g.sync_aggregate.sync_committee_signature.as_ssz_bytes().try_into().unwrap(),
    };
    let floor_absent: Vec<usize> = (0..170).map(|i| i * 3).collect();
    let receive_input = borsh::to_vec(&update_input(&cfg, &genesis, &committee_ssz, update))?;
    Ok(ZkInput { receive_input, floor: keys(&committee_ssz, &floor_absent), committee_ssz, sig })
}

fn run(label: &str, elf: &[u8], input: &[u8]) -> Result<()> {
    let mut b = ExecutorEnv::builder();
    b.write_slice(input);
    let env = b.build()?;
    let s = ExecutorImpl::from_elf(env, elf)?.run().with_context(|| format!("{label}: executor"))?;
    ensure!(s.exit_code == ExitCode::Halted(0), "{label}: exit {:?}", s.exit_code);
    let r: Report = borsh::from_slice(&s.journal.as_ref().context("no journal")?.bytes)?;
    println!("## {label}: session user={} total={} segments={}", s.user_cycles, s.total_cycles, s.segments.len());
    println!("| step | user cycles |\n|---|---:|");
    for (l, c) in &r.marks {
        println!("| {l} | {c} |");
    }
    let mut ok = true;
    for (l, v) in &r.verdicts {
        println!("  verdict: {l}: {}", if *v { "ok" } else { "FAILED" });
        ok &= *v;
    }
    ensure!(ok, "{label}: a verdict failed");
    Ok(())
}

fn main() -> Result<()> {
    let only_blst = std::env::args().any(|a| a == "blst" || a == "fast");
    #[cfg(feature = "blst")]
    let only_fast = std::env::args().any(|a| a == "fast");
    let cases = [
        ("Sepolia finality update, same period (509/512)", zk_case("sepolia", "bootstrap_same_period", LcUpdate::Finality(fx("sepolia", "finality_update.ssz")))?),
        ("mainnet finality update, same period (507/512)", zk_case("mainnet", "bootstrap_same_period", LcUpdate::Finality(fx("mainnet", "finality_update.ssz")))?),
        ("Sepolia period update 1410 (proves the next committee)", zk_case("sepolia", "bootstrap_prev_period", LcUpdate::Period(fx("sepolia", "update_period_1410.ssz")))?),
    ];
    for (label, zk) in &cases {
        if !only_blst {
            run(&format!("zkcrypto: {label}"), LC_BENCH_ZK_ELF, &borsh::to_vec(zk)?)?;
        }
        #[cfg(feature = "blst")]
        {
            if !only_fast {
                let b = lcbench_common::BlstInput { sig: zk.sig.clone(), floor: zk.floor.clone() };
                run(&format!("blst: {label}"), LC_BENCH_BLST_ELF, &borsh::to_vec(&b)?)?;
            }
            if label.contains("finality") {
                run(&format!("whole update (vendored Helios hook): {label}"), LC_BENCH_FAST_ELF, &borsh::to_vec(zk)?)?;
            }
        }
    }
    Ok(())
}
