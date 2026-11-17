//! attest_cycles: EXECUTOR ONLY. Runs the RFP-021 attestation guest, the reference consumer
//! guest and the bridge guest in RISC Zero's in-process executor on the same framed
//! `ReceiveInput`s LEZ feeds a program, and prints user cycles (LEZ's public execution gas).
//! Every guest result is checked against the same handler run natively. Nothing here proves.
//!
//!   attest_cycles            run every scenario
//!   attest_cycles public     the public update path only (src/public.rs; exit 1 at or over the cap)
//!   attest_cycles public-real ...   a real saved update (src/public.rs)
//!   attest_cycles sweep      participation sweep, synthetic committee (src/sweep.rs)
//!   attest_cycles stmt-log | stmt-account | consumer ...   statements on real data (src/stmt.rs)

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

mod public;
mod sweep;
mod stmt;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use bridge_core::{
    AttestSource, Config, Effect as BEffect, Instruction as BInstruction, Message as BMessage, RootSource, TokenEntry,
    VAULT_DEPOSIT_ROOT_SLOT, cfg_id, config_address as bridge_config_address, deposit_commitment, deposit_leaf, merkle_path,
    merkle_root, root_address,
};
use eth_attest_core::{
    Anchor, AnchorRecord, Effect, Fork, InstanceConfig, Instruction, LcSummary, LcUpdate, MIN_SUPERMAJORITY, Message,
    Query, anchor_address, committee_hash,
    inclusion::{
        AccountField, AccountFields, AccountProof, AnchorWitness, Predicate, StatementContext, StorageProof, keccak,
        mapping_slot, word,
    },
    instance_id, store_address,
    testkit::{SynthAccount, SynthState, header_rlp},
};
use eth_attest_probe::Probe;
use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec as S,
    types::{Bootstrap, GenericUpdate, Update},
};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::{ReceiveInput, Response, Transition},
};
use risc0_zkvm::{ExecutorEnv, ExecutorImpl, ExitCode};
use serde_json::Value;
use ssz::{Decode, Encode};

const ATTEST: AccountId = AccountId::new([0xE7; 32]);
const PROBE: AccountId = AccountId::new([0x9B; 32]);
const BRIDGE: AccountId = AccountId::new([0xB0; 32]);
const TOKEN: AccountId = AccountId::new([0x70; 32]);
const DEF: AccountId = AccountId::new([0xDE; 32]);
const ALICE: AccountId = AccountId::new([0xA1; 32]);
const BOB: AccountId = AccountId::new([0xB1; 32]);
const USDC: [u8; 20] = [0xAA; 20];
const VAULT: [u8; 20] = [0x11; 20];
const LEZ_CHAIN: [u8; 32] = [0x4C; 32];
const ZONE: [u8; 32] = [0x5A; 32];
const CAP: f64 = 10_000_000.0;

struct Row {
    label: String,
    user: u64,
    total: u64,
    segments: usize,
}

/// Executes `elf` on `input`, checks it halted with the natively computed response.
fn exec(rows: &mut Vec<Row>, label: &str, elf: &[u8], native: fn(&ReceiveInput) -> Response, input: &ReceiveInput) -> Result<Transition> {
    let payload = borsh::to_vec(input)?;
    let mut b = ExecutorEnv::builder();
    b.write_slice(&lee_core::to_frame(&payload));
    let env = b.build()?;
    let s = ExecutorImpl::from_elf(env, elf)?.run().with_context(|| format!("{label}: executor"))?;
    ensure!(s.exit_code == ExitCode::Halted(0), "{label}: exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?;
    let t: Transition = borsh::from_slice(lee_core::from_frame(&journal.bytes).context("bad journal frame")?)?;
    ensure!(t.input == *input, "{label}: guest saw another input");
    ensure!(t.response == native(input), "{label}: guest and native responses differ");
    println!(
        "  {label}: user={} total={} segments={} ({:.2}x the 10M cap)",
        s.user_cycles,
        s.total_cycles,
        s.segments.len(),
        s.user_cycles as f64 / CAP
    );
    rows.push(Row { label: label.into(), user: s.user_cycles, total: s.total_cycles, segments: s.segments.len() });
    Ok(t)
}

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

fn boot_committee(b: &[u8]) -> Vec<u8> {
    Bootstrap::<S>::from_ssz_bytes(b).unwrap().current_sync_committee().as_ssz_bytes()
}

fn next_committee(u: &[u8]) -> Vec<u8> {
    GenericUpdate::from(&Update::<S>::from_ssz_bytes(u).unwrap()).next_sync_committee.unwrap().as_ssz_bytes()
}

fn root_input(signer: AccountId, program: AccountId, message: Vec<u8>) -> ReceiveInput {
    ReceiveInput { receiver: Actor::new(signer, program), from: None, is_authorized: true, pre_state: ActorState::empty(), message }
}

fn msg<M: borsh::BorshSerialize>(m: &M) -> Vec<u8> {
    borsh::to_vec(m).unwrap()
}

fn update_input(cfg: &InstanceConfig, prev: &LcSummary, current: &[u8], next: Option<&[u8]>, u: LcUpdate) -> ReceiveInput {
    assert_eq!(committee_hash(current), prev.current_committee);
    root_input(
        BOB,
        ATTEST,
        msg(&Message::Instruction(Instruction::Update {
            config: cfg.clone(),
            prev: prev.clone(),
            current_committee: current.to_vec(),
            next_committee: next.map(<[u8]>::to_vec),
            update: u,
        })),
    )
}

fn next_of(t: &Transition) -> Result<LcSummary> {
    let m = t.response.sync_messages.first().context("no effect")?;
    match borsh::from_slice::<Message>(&m.message)? {
        Message::Effect(Effect::Advance { next, .. }) => Ok(next),
        _ => Err(anyhow!("not an Advance")),
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("public") => std::process::exit(if public::fixtures_run()? { 0 } else { 1 }),
        Some("public-real") => std::process::exit(if public::real_run(&args[1..])? { 0 } else { 1 }),
        Some("sweep") => std::process::exit(if sweep::run()? { 0 } else { 1 }),
        Some("split") => std::process::exit(if sweep::split()? { 0 } else { 1 }),
        Some("synth-chain") => std::process::exit(if sweep::synth_chain(&args[1..])? { 0 } else { 1 }),
        Some("stmt-log" | "stmt-account" | "consumer") => std::process::exit(if stmt::run(&args)? { 0 } else { 1 }),
        _ => {}
    }
    let mut rows = Vec::new();
    let r = &mut rows;
    let attest = eth_attest::handle;

    println!("# attestation program: submitter's actor (private part when proved) and public parts");
    // Sepolia (Gloas), instance at the same-period checkpoint.
    let cfg = config("sepolia", "bootstrap_same_period");
    let id = instance_id(&ATTEST, &cfg);
    let boot = fx("sepolia", "bootstrap_same_period.ssz");
    let init = root_input(ALICE, ATTEST, msg(&Message::Instruction(Instruction::Init { config: cfg.clone(), bootstrap: boot.clone() })));
    exec(r, "Init: verify bootstrap at the checkpoint (Sepolia, Gloas)", ATTEST_GUEST_ELF, attest, &init)?;
    let genesis = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg, &boot).map_err(|e| anyhow!(e))?;
    let fin = fx("sepolia", "finality_update.ssz");
    let t = exec(
        r,
        "Update: finality update, same period (Sepolia, 509/512)",
        ATTEST_GUEST_ELF,
        attest,
        &update_input(&cfg, &genesis, &boot_committee(&boot), None, LcUpdate::Finality(fin.clone())),
    )?;
    let next = next_of(&t)?;

    // Public parts of an update.
    let advance = ReceiveInput {
        receiver: Actor::new(store_address(&ATTEST, &id), ATTEST),
        from: Some(Actor::new(BOB, ATTEST)),
        is_authorized: false,
        pre_state: ActorState::from(msg(&genesis)),
        message: msg(&Message::Effect(Effect::Advance { prev: genesis.clone(), next: next.clone() })),
    };
    exec(r, "public: store applies Advance (+ sends RecordAnchor)", ATTEST_GUEST_ELF, attest, &advance)?;
    println!(
        "  (Advance message: {} bytes borsh; store state {} bytes; Gloas summaries carry no state root/number)",
        advance.message.len(),
        advance.pre_state.len()
    );
    let anchor = Anchor::BlockHash(next.finalized.execution.block_hash);
    let rec = AnchorRecord {
        instance_id: id,
        evm_chain_id: cfg.evm_chain_id,
        anchor,
        beacon_slot: next.finalized.beacon.slot,
        beacon_root: next.finalized.beacon_root,
        block_number: None,
    };
    let record = ReceiveInput {
        receiver: Actor::new(anchor_address(&ATTEST, &id, &anchor), ATTEST),
        from: Some(Actor::new(store_address(&ATTEST, &id), ATTEST)),
        is_authorized: false,
        pre_state: ActorState::empty(),
        message: msg(&Message::Effect(Effect::RecordAnchor(rec.clone()))),
    };
    exec(r, "public: anchor PDA records an anchor", ATTEST_GUEST_ELF, attest, &record)?;
    let assert_anchor = ReceiveInput {
        receiver: Actor::new(anchor_address(&ATTEST, &id, &anchor), ATTEST),
        from: Some(Actor::new(ALICE, BRIDGE)),
        is_authorized: false,
        pre_state: ActorState::from(msg(&rec)),
        message: msg(&Message::Query(Query::AssertAnchor { instance_id: id, evm_chain_id: cfg.evm_chain_id, anchor })),
    };
    exec(r, "public: AssertAnchor (what a consumer pays publicly)", ATTEST_GUEST_ELF, attest, &assert_anchor)?;

    // Sepolia committee handoff.
    let cfg_h = config("sepolia", "bootstrap_prev_period");
    let boot_h = fx("sepolia", "bootstrap_prev_period.ssz");
    let g_h = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg_h, &boot_h).map_err(|e| anyhow!(e))?;
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let t = exec(
        r,
        "Update: period update proving the next committee (Sepolia 1410)",
        ATTEST_GUEST_ELF,
        attest,
        &update_input(&cfg_h, &g_h, &boot_committee(&boot_h), None, LcUpdate::Period(u1410.clone())),
    )?;
    let h1 = next_of(&t)?;
    let nc = next_committee(&u1410);
    exec(
        r,
        "Update: finality update signed by the next committee, rotates (Sepolia 1411)",
        ATTEST_GUEST_ELF,
        attest,
        &update_input(&cfg_h, &h1, &boot_committee(&boot_h), Some(&nc), LcUpdate::Finality(fin.clone())),
    )?;

    // Mainnet (Fulu).
    let cfg_m = config("mainnet", "bootstrap_same_period");
    let boot_m = fx("mainnet", "bootstrap_same_period.ssz");
    let init_m = root_input(ALICE, ATTEST, msg(&Message::Instruction(Instruction::Init { config: cfg_m.clone(), bootstrap: boot_m.clone() })));
    exec(r, "Init: verify bootstrap (mainnet, Fulu)", ATTEST_GUEST_ELF, attest, &init_m)?;
    let g_m = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg_m, &boot_m).map_err(|e| anyhow!(e))?;
    exec(
        r,
        "Update: finality update, same period (mainnet, 507/512)",
        ATTEST_GUEST_ELF,
        attest,
        &update_input(&cfg_m, &g_m, &boot_committee(&boot_m), None, LcUpdate::Finality(fx("mainnet", "finality_update.ssz"))),
    )?;

    println!("# consumer: statements in the consumer's own actor (reference consumer guest)");
    let st = &json("sepolia", "meta.json")["storage"];
    let pf = json("sepolia", "eth_getProof.json");
    let sp = &pf["storageProof"][0];
    let nodes = |v: &Value| v.as_array().unwrap().iter().map(|x| hx(x.as_str().unwrap())).collect::<Vec<_>>();
    let account = AccountProof {
        fields: AccountFields {
            nonce: u64::from_str_radix(pf["nonce"].as_str().unwrap().trim_start_matches("0x"), 16)?,
            balance: h32(pf["balance"].as_str().unwrap()),
            storage_root: h32(pf["storageHash"].as_str().unwrap()),
            code_hash: h32(pf["codeHash"].as_str().unwrap()),
        },
        nodes: nodes(&pf["accountProof"]),
    };
    let header = hx(std::str::from_utf8(&fx("sepolia", "el_block_header.rlp.hex"))?);
    let witness = AnchorWitness { anchor: Anchor::BlockHash(h32(st["block_hash"].as_str().unwrap())), header_rlp: Some(header) };
    let address: [u8; 20] = hx(pf["address"].as_str().unwrap()).try_into().unwrap();
    let ctx = StatementContext { evm_chain_id: cfg.evm_chain_id, instance_id: id };
    let probe = eth_attest_probe::handle;
    let storage = Probe::Storage {
        attest: ATTEST,
        ctx,
        witness: witness.clone(),
        address,
        slot: mapping_slot(&h32(st["mapping_key"].as_str().unwrap()), st["mapping_slot_index"].as_u64().unwrap()),
        predicate: Predicate::Eq(word(1)),
        proof: StorageProof { account: account.clone(), value: h32(sp["value"].as_str().unwrap()), nodes: nodes(&sp["proof"]) },
    };
    println!(
        "  (real Sepolia proof: {} account nodes, {} storage nodes, header {} bytes)",
        account.nodes.len(),
        sp["proof"].as_array().unwrap().len(),
        witness.header_rlp.as_ref().unwrap().len()
    );
    exec(r, "statement: storage slot, real Sepolia mapping entry (header + account + storage proof)", ATTEST_PROBE_GUEST_ELF, probe, &root_input(ALICE, PROBE, msg(&storage)))?;
    let acct = Probe::Account {
        attest: ATTEST,
        ctx,
        witness,
        address,
        field: AccountField::StorageRoot,
        predicate: Predicate::Eq(account.fields.storage_root),
        proof: account,
    };
    exec(r, "statement: account field, real Sepolia account (header + account proof)", ATTEST_PROBE_GUEST_ELF, probe, &root_input(ALICE, PROBE, msg(&acct)))?;

    println!("# bridge claim: stand-in vs RFP-021 path (claimer's actor = private part; config = public)");
    bridge(r)?;

    println!("\n| scenario | user cycles | total cycles | segments | user / 10M |\n|---|---:|---:|---:|---:|");
    for row in &rows {
        println!("| {} | {} | {} | {} | {:.3} |", row.label, row.user, row.total, row.segments, row.user as f64 / CAP);
    }
    Ok(())
}

/// Synthetic vault state, sized so proofs have realistic depth: `n_accounts` accounts and a vault
/// with `n_roots` recorded deposit roots.
fn bridge(r: &mut Vec<Row>) -> Result<()> {
    const N_ACCOUNTS: u32 = 200_000;
    const N_ROOTS: u32 = 4_000;
    let chain = 11_155_111u64;
    let cfg = cfg_id(chain, &VAULT, &LEZ_CHAIN, &ZONE, &BRIDGE);
    let secret = [7u8; 32];
    let denom = 100_000_000u128;
    let leaves: Vec<[u8; 32]> = (0..3u8)
        .map(|i| {
            let k = if i == 0 { secret } else { [i; 32] };
            deposit_leaf(&cfg, &USDC, denom, &deposit_commitment(&k, &ALICE, &USDC, denom, &cfg))
        })
        .collect();
    let path = merkle_path(&leaves, 0);
    let root = merkle_root(&leaves[0], 0, &path);
    let mut vault = SynthAccount::new(VAULT).with(mapping_slot(&root, VAULT_DEPOSIT_ROOT_SLOT), word(1));
    for i in 0..N_ROOTS {
        vault = vault.with(mapping_slot(&keccak(&i.to_be_bytes()), VAULT_DEPOSIT_ROOT_SLOT), word(1));
    }
    let mut accounts = vec![vault];
    for i in 0..N_ACCOUNTS {
        let mut a = [0u8; 20];
        a[..4].copy_from_slice(&i.to_be_bytes());
        a[19] = 0xEE;
        accounts.push(SynthAccount::new(a));
    }
    let state = SynthState { accounts };
    let state_root = state.state_root();
    let proof = state.storage_proof(&VAULT, &mapping_slot(&root, VAULT_DEPOSIT_ROOT_SLOT));
    println!(
        "  (synthetic state: {} accounts, vault with {} roots; proof {} account nodes, {} storage nodes)",
        N_ACCOUNTS + 1,
        N_ROOTS + 1,
        proof.account.nodes.len(),
        proof.nodes.len()
    );
    let attest_cfg = InstanceConfig {
        evm_chain_id: chain,
        genesis_validators_root: [0xD8; 32],
        genesis_time: 0,
        seconds_per_slot: 12,
        forks: [Fork { epoch: 0, version: [0; 4] }; 8],
        checkpoint: [0; 32],
        min_participants: MIN_SUPERMAJORITY,
        salt: [0; 32],
    };
    let source = AttestSource { attest_program: ATTEST, attest_instance: instance_id(&ATTEST, &attest_cfg), deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT };
    let bridge = bridge_core::handle;

    let stand_in = BMessage::Instruction(BInstruction::Claim {
        eth_token: USDC,
        denom_index: 0,
        denom,
        root,
        leaf_index: 0,
        path: path.clone(),
        secret,
        cfg_id: cfg,
        recipient_opening: None,
    });
    exec(r, "bridge claim, stand-in: claimer's actor (Merkle path)", BRIDGE_GUEST_ELF, bridge, &root_input(ALICE, BRIDGE, msg(&stand_in)))?;
    let attested = |anchor: AnchorWitness| {
        BMessage::Instruction(BInstruction::ClaimAttested {
            eth_token: USDC,
            denom_index: 0,
            denom,
            root,
            leaf_index: 0,
            path: path.clone(),
            secret,
            cfg_id: cfg,
            source,
            vault: VAULT,
            evm_chain_id: chain,
            anchor,
            proof: proof.clone(),
            recipient_opening: None,
        })
    };
    let m = attested(AnchorWitness { anchor: Anchor::StateRoot(state_root), header_rlp: None });
    exec(r, "bridge claim, RFP-021: claimer's actor (Merkle path + storage proof, state-root anchor)", BRIDGE_GUEST_ELF, bridge, &root_input(ALICE, BRIDGE, msg(&m)))?;
    let header = header_rlp(state_root, [0; 32], 12_000_000, 0);
    let m = attested(AnchorWitness { anchor: Anchor::BlockHash(keccak(&header)), header_rlp: Some(header) });
    exec(r, "bridge claim, RFP-021: claimer's actor (same, block-hash anchor + header)", BRIDGE_GUEST_ELF, bridge, &root_input(ALICE, BRIDGE, msg(&m)))?;

    // Public parts that differ: AssertRoot vs AssertAnchor (measured above), and the config.
    let assert_root = ReceiveInput {
        receiver: Actor::new(root_address(&BRIDGE, &root), BRIDGE),
        from: Some(Actor::new(ALICE, BRIDGE)),
        is_authorized: false,
        pre_state: ActorState::from(vec![1u8]),
        message: msg(&BMessage::Effect(BEffect::AssertRoot)),
    };
    exec(r, "public: bridge root PDA AssertRoot (stand-in)", BRIDGE_GUEST_ELF, bridge, &assert_root)?;
    let config = |root_source| Config {
        cfg_id: cfg,
        vault: VAULT,
        evm_chain_id: chain,
        lez_chain_id: LEZ_CHAIN,
        zone_id: ZONE,
        token_program: TOKEN,
        admin: BOB,
        pending_admin: None,
        freezer: BOB,
        attestor: BOB,
        mint_frozen: false,
        burn_frozen: false,
        tokens: vec![TokenEntry {
            eth_token: USDC,
            definition: DEF,
            decimals: 6,
            denoms: vec![denom, 10 * denom],
            claims: vec![0, 0],
            burns: vec![0, 0],
            claimed_value: 0,
            burned_value: 0,
            registered: true,
            mint_frozen: false,
            burn_frozen: false,
        }],
        root_source,
        sponsor_amount: 0,
    };
    let claim_cfg = |root_source, e: BEffect| ReceiveInput {
        receiver: Actor::new(bridge_config_address(&BRIDGE), BRIDGE),
        from: Some(Actor::new(ALICE, BRIDGE)),
        is_authorized: false,
        pre_state: ActorState::from(msg(&config(root_source))),
        message: msg(&BMessage::Effect(e)),
    };
    exec(
        r,
        "public: bridge config Claim (stand-in)",
        BRIDGE_GUEST_ELF,
        bridge,
        &claim_cfg(RootSource::StandIn, BEffect::Claim { eth_token: USDC, denom_index: 0, denom, to: ALICE, cfg_id: cfg, one_time: false }),
    )?;
    exec(
        r,
        "public: bridge config ClaimAttested (RFP-021)",
        BRIDGE_GUEST_ELF,
        bridge,
        &claim_cfg(
            RootSource::Attest(source),
            BEffect::ClaimAttested { eth_token: USDC, denom_index: 0, denom, to: ALICE, cfg_id: cfg, source, vault: VAULT, evm_chain_id: chain, one_time: false },
        ),
    )?;
    Ok(())
}
