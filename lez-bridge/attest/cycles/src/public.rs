// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public update path (branch `public-update`, `DESIGN.md` §9.3a), EXECUTOR ONLY: every
//! program call one `UpdatePublic` transaction makes, run in the executor on the public guest
//! (`methods/attest_guest_public`, the ELF deployed on the local chain), summed and compared with
//! LEZ's 10M cap per transaction and per block.
//!
//!   attest_cycles public                       the recorded Sepolia and mainnet fixtures
//!   attest_cycles public-real PROGRAM SIGNER CONFIG_HEX DIR OUT_HEX
//!       a real update saved by `lez-bridge attest update --save-dir DIR` (update.ssz,
//!       prev.borsh.hex, the committee as <hash>.ssz) against the instance whose config
//!       `attest init --save-dir` wrote (config.borsh.hex), as PROGRAM (the deployed program id)
//!       and SIGNER (the submitting public account); writes the instruction's message as hex
//!       to OUT_HEX for `lez-bridge raw-call`. A saved PERIOD update (update.kind = period)
//!       gives two messages, OUT_HEX.begin and OUT_HEX.finish, and prints the job PDA; a finality
//!       update is signed by the committee Helios selects by the signature period (the next one
//!       for a rotation), read from DIR as <hash>.ssz.
//!
//! `public` also runs the committee handoff on the fixtures (period update 1410 Begin/Finish,
//! the rotation into 1411, period update 1411 Begin/Finish). Exit status 1 if a transaction's sum
//! reaches the cap.

use super::*;
use eth_attest_core::{JobState, KeyWitnesses, PeriodJob, job_address, job_id};

/// What the sequencer's whole transaction runs, summed (user cycles).
pub struct Tx {
    pub calls: Vec<(String, u64)>,
    pub next: LcSummary,
}

impl Tx {
    pub fn user(&self) -> u64 {
        self.calls.iter().map(|(_, c)| c).sum()
    }
}

fn exec_user(label: &str, elf: &[u8], input: &ReceiveInput) -> Result<(u64, Transition)> {
    let mut rows = Vec::new();
    let t = exec(&mut rows, label, elf, eth_attest::handle, input)?;
    Ok((rows[0].user, t))
}

/// Builds the `UpdatePublic` message for `update` against `prev` (key witnesses from the
/// committee and the update's bits), checks it natively, then runs the submitter's actor, the
/// store's `Advance` and each `RecordAnchor` in the executor.
pub fn run_tx(
    label: &str,
    program: AccountId,
    signer: AccountId,
    cfg: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    update: &[u8],
) -> Result<(Tx, Vec<u8>)> {
    let bits = eth_attest::public::finality_bits(update).map_err(|e| anyhow!(e))?;
    let keys: KeyWitnesses = eth_attest::public::key_witnesses(committee, &bits).map_err(|e| anyhow!(e))?;
    let signers = bits.iter().filter(|b| **b).count();
    println!("  {label}: {signers}/512 signers, {} key witnesses (aggregate + {} non-signers)", keys.absent.len() + 1, keys.absent.len());
    let v = eth_attest::public::verify_finality_public(&program, cfg, prev, committee, &keys, update)
        .map_err(|e| anyhow!("does not verify natively: {e}"))?;
    let message = msg(&Message::Instruction(Instruction::UpdatePublic {
        config: cfg.clone(),
        prev: prev.clone(),
        committee: committee.to_vec(),
        keys,
        update: update.to_vec(),
    }));
    println!("  message {} bytes (borsh)", message.len());
    let mut calls = Vec::new();
    let input = root_input(signer, program, message.clone());
    let (c, t) = exec_user(&format!("{label}: submitter's actor (UpdatePublic)"), ATTEST_GUEST_PUBLIC_ELF, &input)?;
    calls.push(("submitter's actor: verify the update, send Advance".to_owned(), c));
    let next = match t.response.sync_messages.first().map(|m| borsh::from_slice::<Message>(&m.message)) {
        Some(Ok(Message::Effect(Effect::Advance { next, .. }))) => next,
        _ => return Err(anyhow!("no Advance")),
    };
    ensure!(next == v.next, "the guest's Advance differs from the native verification");
    store_side(label, program, Actor::new(signer, program), cfg, prev, &next, &mut calls)?;
    Ok((Tx { calls, next }, message))
}

/// The store's `Advance` from `from`, and each `RecordAnchor` it sends when the header moved.
fn store_side(label: &str, program: AccountId, from: Actor, cfg: &InstanceConfig, prev: &LcSummary, next: &LcSummary, calls: &mut Vec<(String, u64)>) -> Result<()> {
    let id = prev.instance_id;
    let advance = ReceiveInput {
        receiver: Actor::new(store_address(&program, &id), program),
        from: Some(from),
        is_authorized: false,
        pre_state: ActorState::from(msg(prev)),
        message: msg(&Message::Effect(Effect::Advance { prev: prev.clone(), next: next.clone() })),
    };
    let (c, _) = exec_user(&format!("{label}: store applies Advance"), ATTEST_GUEST_PUBLIC_ELF, &advance)?;
    calls.push(("store: Advance (+ sends RecordAnchor)".to_owned(), c));
    if next.finalized == prev.finalized {
        return Ok(());
    }
    for anchor in next.finalized.anchors() {
        let rec = AnchorRecord {
            instance_id: id,
            evm_chain_id: cfg.evm_chain_id,
            anchor,
            beacon_slot: next.finalized.beacon.slot,
            beacon_root: next.finalized.beacon_root,
            block_number: next.finalized.execution.block_number,
        };
        let record = ReceiveInput {
            receiver: Actor::new(anchor_address(&program, &id, &anchor), program),
            from: Some(Actor::new(store_address(&program, &id), program)),
            is_authorized: false,
            pre_state: ActorState::empty(),
            message: msg(&Message::Effect(Effect::RecordAnchor(rec))),
        };
        let (c, _) = exec_user(&format!("{label}: anchor PDA records an anchor"), ATTEST_GUEST_PUBLIC_ELF, &record)?;
        calls.push(("anchor PDA: RecordAnchor".to_owned(), c));
    }
    Ok(())
}

/// A period update as two public transactions: Begin (submitter's actor + job PDA opens) and
/// Finish (submitter's actor + job PDA + store `Advance` + `RecordAnchor`s). `committee` is the
/// signing committee's SSZ. Returns both, and the two messages.
pub fn run_period(
    label: &str,
    program: AccountId,
    signer: AccountId,
    cfg: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    update: &[u8],
) -> Result<(Tx, Tx, Vec<u8>, Vec<u8>, PeriodJob)> {
    let job = eth_attest::public::verify_period_begin(&program, cfg, prev, update).map_err(|e| anyhow!("Begin does not verify natively: {e}"))?;
    let bits = eth_attest::public::expand_bits(&job.bits);
    let keys = eth_attest::public::key_witnesses(committee, &bits).map_err(|e| anyhow!(e))?;
    eth_attest::public::verify_period_finish(&job, committee, &keys).map_err(|e| anyhow!("Finish does not verify natively: {e}"))?;
    println!("  {label}: {}/512 signers, signed by the store's {} committee", bits.iter().filter(|b| **b).count(),
        if Some(job.committee) == prev.next_committee { "NEXT" } else { "current" });
    let id = job_id(&job.prev, &job.update_digest);
    let job_pda = Actor::new(job_address(&program, &id), program);
    // Begin
    let m_begin = msg(&Message::Instruction(Instruction::PeriodBegin { config: cfg.clone(), prev: prev.clone(), update: update.to_vec() }));
    let mut calls = Vec::new();
    let (c, t) = exec_user(&format!("{label}: Begin, submitter's actor"), ATTEST_GUEST_PUBLIC_ELF, &root_input(signer, program, m_begin.clone()))?;
    calls.push(("Begin: submitter's actor (every check but the signature)".to_owned(), c));
    match t.response.sync_messages.get(1).map(|m| borsh::from_slice::<Message>(&m.message)) {
        Some(Ok(Message::Effect(Effect::JobOpen(j)))) if j == job => {}
        _ => return Err(anyhow!("Begin did not open this job")),
    }
    let store_pda = Actor::new(store_address(&program, &prev.instance_id), program);
    let current = |from: Actor| ReceiveInput {
        receiver: store_pda,
        from: Some(from),
        is_authorized: false,
        pre_state: ActorState::from(msg(prev)),
        message: msg(&Message::Effect(Effect::AssertCurrent { prev: prev.clone() })),
    };
    let (c, _) = exec_user(&format!("{label}: Begin, store is current"), ATTEST_GUEST_PUBLIC_ELF, &current(Actor::new(signer, program)))?;
    calls.push(("Begin: store asserts it is the job's prev".to_owned(), c));
    let open = ReceiveInput { receiver: job_pda, from: Some(Actor::new(signer, program)), is_authorized: false, pre_state: ActorState::empty(), message: msg(&Message::Effect(Effect::JobOpen(job.clone()))) };
    let (c, _) = exec_user(&format!("{label}: Begin, job PDA opens"), ATTEST_GUEST_PUBLIC_ELF, &open)?;
    calls.push(("Begin: job PDA opens".to_owned(), c));
    let begin = Tx { calls, next: job.next.clone() };
    // Finish
    let keys_f = keys.clone();
    let m_finish = msg(&Message::Instruction(Instruction::PeriodFinish { job: job.clone(), committee: committee.to_vec(), keys }));
    let mut calls = Vec::new();
    let (c, _) = exec_user(&format!("{label}: Finish, submitter's actor"), ATTEST_GUEST_PUBLIC_ELF, &root_input(signer, program, m_finish.clone()))?;
    calls.push(("Finish: submitter's actor".to_owned(), c));
    let (c, _) = exec_user(&format!("{label}: Finish, store is current"), ATTEST_GUEST_PUBLIC_ELF, &current(Actor::new(signer, program)))?;
    calls.push(("Finish: store asserts it is the job's prev".to_owned(), c));
    let fin = ReceiveInput {
        receiver: job_pda,
        from: Some(Actor::new(signer, program)),
        is_authorized: false,
        pre_state: ActorState::from(msg(&JobState::Open(job.clone()))),
        message: msg(&Message::Effect(Effect::JobFinish { job: job.clone(), committee: committee.to_vec(), keys: keys_f })),
    };
    let (c, _) = exec_user(&format!("{label}: Finish, job PDA"), ATTEST_GUEST_PUBLIC_ELF, &fin)?;
    calls.push(("Finish: job PDA (the signature; done; sends Advance)".to_owned(), c));
    store_side(label, program, job_pda, cfg, prev, &job.next, &mut calls)?;
    let finish = Tx { calls, next: job.next.clone() };
    Ok((begin, finish, m_begin, m_finish, job))
}

/// A finality update split at the pairing seam: Begin (submitter's actor + store `AssertCurrent` +
/// job PDA opens) and Finish (submitter's actor + job PDA's final exponentiation + store `Advance`
/// + `RecordAnchor`s). Returns both and the two messages.
pub fn run_split(
    label: &str,
    elf: &[u8],
    program: AccountId,
    signer: AccountId,
    cfg: &InstanceConfig,
    prev: &LcSummary,
    committee: &[u8],
    update: &[u8],
) -> Result<(Tx, Tx, Vec<u8>, Vec<u8>)> {
    let bits = eth_attest::public::finality_bits(update).map_err(|e| anyhow!(e))?;
    let keys = eth_attest::public::key_witnesses(committee, &bits).map_err(|e| anyhow!(e))?;
    let job = eth_attest::public::verify_finality_begin(&program, cfg, prev, committee, &keys, update).map_err(|e| anyhow!("Begin: {e}"))?;
    eth_attest::public::verify_finality_finish(&job).map_err(|e| anyhow!("Finish: {e}"))?;
    let id = job_id(&job.prev, &job.update_digest);
    let job_pda = Actor::new(job_address(&program, &id), program);
    let store_pda = Actor::new(store_address(&program, &prev.instance_id), program);
    let m_begin = msg(&Message::Instruction(Instruction::FinalityBegin { config: cfg.clone(), prev: prev.clone(), committee: committee.to_vec(), keys, update: update.to_vec() }));
    let mut calls = Vec::new();
    let (c, _) = exec_user(&format!("{label}: Begin, submitter's actor"), elf, &root_input(signer, program, m_begin.clone()))?;
    calls.push(("Begin: submitter's actor (every check, the Miller loop)".to_owned(), c));
    let (c, _) = exec_user(&format!("{label}: Begin, store is current"), elf, &ReceiveInput {
        receiver: store_pda, from: Some(Actor::new(signer, program)), is_authorized: false,
        pre_state: ActorState::from(msg(prev)), message: msg(&Message::Effect(Effect::AssertCurrent { prev: prev.clone() })),
    })?;
    calls.push(("Begin: store asserts it is the job's prev".to_owned(), c));
    let (c, _) = exec_user(&format!("{label}: Begin, job PDA opens"), elf, &ReceiveInput {
        receiver: job_pda, from: Some(Actor::new(signer, program)), is_authorized: false,
        pre_state: ActorState::empty(), message: msg(&Message::Effect(Effect::JobOpen(job.clone()))),
    })?;
    calls.push(("Begin: job PDA opens (holds the Miller value)".to_owned(), c));
    let begin = Tx { calls, next: job.next.clone() };
    let m_finish = msg(&Message::Instruction(Instruction::FinalityFinish { job_id: id }));
    let mut calls = Vec::new();
    let (c, _) = exec_user(&format!("{label}: Finish, submitter's actor"), elf, &root_input(signer, program, m_finish.clone()))?;
    calls.push(("Finish: submitter's actor".to_owned(), c));
    let (c, _) = exec_user(&format!("{label}: Finish, job PDA"), elf, &ReceiveInput {
        receiver: job_pda, from: Some(Actor::new(signer, program)), is_authorized: false,
        pre_state: ActorState::from(msg(&JobState::Open(job.clone()))), message: msg(&Message::Effect(Effect::JobFinalize { job_id: id })),
    })?;
    calls.push(("Finish: job PDA (final exponentiation; done; sends Advance)".to_owned(), c));
    store_side(label, program, job_pda, cfg, prev, &job.next, &mut calls)?;
    let finish = Tx { calls, next: job.next };
    Ok((begin, finish, m_begin, m_finish))
}

pub fn report(label: &str, tx: &Tx) -> bool {
    let sum = tx.user();
    println!("\n### {label}\n| program call | user cycles |\n|---|---:|");
    for (l, c) in &tx.calls {
        println!("| {l} | {c} |");
    }
    let under = sum < CAP as u64;
    println!(
        "| **whole transaction** | **{sum}** ({:.2}% of the 10M cap; margin {}) |",
        sum as f64 / CAP * 100.0,
        if under { format!("{}", CAP as u64 - sum) } else { "NONE: OVER THE CAP".to_owned() }
    );
    println!(
        "  new store: finalized slot {}, block hash 0x{}, updates {}",
        tx.next.finalized.beacon.slot,
        hex::encode(tx.next.finalized.execution.block_hash),
        tx.next.updates
    );
    under
}

/// The recorded fixtures (Sepolia 509/512, mainnet 507/512), and refusals of tampered input.
pub fn fixtures_run() -> Result<bool> {
    println!("# public update path (UpdatePublic), recorded fixtures, guest attest_guest_public");
    println!(
        "  guest image id {} ({} bytes ELF)",
        risc0_zkvm::sha::Digest::from(ATTEST_GUEST_PUBLIC_ID),
        ATTEST_GUEST_PUBLIC_ELF.len()
    );
    let mut ok = true;
    for (net, label) in [("sepolia", "Sepolia finality update, same period (509/512)"), ("mainnet", "mainnet finality update, same period (507/512)")] {
        let cfg = config(net, "bootstrap_same_period");
        let boot = fx(net, "bootstrap_same_period.ssz");
        let genesis = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg, &boot).map_err(|e| anyhow!(e))?;
        let (tx, _) = run_tx(label, ATTEST, BOB, &cfg, &genesis, &boot_committee(&boot), &fx(net, "finality_update.ssz"))?;
        ok &= report(label, &tx);
    }
    // The committee handoff on the fixtures, all public (as tests/public_update.rs natively).
    let cfg = config("sepolia", "bootstrap_prev_period");
    let boot = fx("sepolia", "bootstrap_prev_period.ssz");
    let g0 = eth_attest::lc::verify_bootstrap(&ATTEST, &cfg, &boot).map_err(|e| anyhow!(e))?;
    let cur = boot_committee(&boot);
    let u1410 = fx("sepolia", "update_period_1410.ssz");
    let (b1, f1, _, _, _) = run_period("period update 1410", ATTEST, BOB, &cfg, &g0, &cur, &u1410)?;
    ok &= report("handoff 1: period update 1410, Begin", &b1);
    ok &= report("handoff 1: period update 1410, Finish", &f1);
    let nc = next_committee(&u1410);
    let (r, _) = run_tx("rotation into 1411", ATTEST, BOB, &cfg, &f1.next, &nc, &fx("sepolia", "finality_update.ssz"))?;
    ok &= report("handoff 1: finality update signed by the next committee (rotation into 1411)", &r);
    let u1411 = fx("sepolia", "update_period_1411.ssz");
    let (b2, f2, _, _, _) = run_period("period update 1411", ATTEST, BOB, &cfg, &r.next, &nc, &u1411)?;
    ok &= report("handoff 2: period update 1411, Begin", &b2);
    ok &= report("handoff 2: period update 1411, Finish", &f2);
    Ok(ok)
}

/// A real update saved by the CLI (see the module docs).
pub fn real_run(args: &[String]) -> Result<bool> {
    let [program, signer, config_hex, dir, out] = args else {
        return Err(anyhow!("public-real PROGRAM SIGNER CONFIG_HEX DIR OUT_HEX"));
    };
    let program: AccountId = program.parse().map_err(|_| anyhow!("bad program id"))?;
    let signer: AccountId = signer.parse().map_err(|_| anyhow!("bad signer"))?;
    let cfg: InstanceConfig = borsh::from_slice(&hx(&std::fs::read_to_string(config_hex)?))?;
    let dir = Path::new(dir);
    let prev: LcSummary = borsh::from_slice(&hx(&std::fs::read_to_string(dir.join("prev.borsh.hex"))?))?;
    let period = std::fs::read_to_string(dir.join("update.kind"))?.trim() == "period";
    let update = std::fs::read(dir.join("update.ssz"))?;
    ensure!(instance_id(&program, &cfg) == prev.instance_id, "the config is not this instance's (program id?)");
    // The signing committee, as Helios selects it: the current one if the signature is in the
    // store's period, else the next one.
    let sig_slot = u64::from_le_bytes(update[update.len() - 8..].try_into()?);
    let which = if sig_slot / 8192 == prev.finalized.beacon.slot / 8192 { prev.current_committee } else { prev.next_committee.context("signed in the next period, but the store has no next committee")? };
    let committee = std::fs::read(dir.join(format!("{}.ssz", hex::encode(which))))?;
    ensure!(committee_hash(&committee) == which, "committee file does not match the store");
    println!("# public update path, REAL {} update from {}", if period { "PERIOD" } else { "finality" }, dir.display());
    println!(
        "  store now: finalized slot {} (period {}), updates {}, next committee {}; signature slot {sig_slot} (period {}): signed by the {} committee",
        prev.finalized.beacon.slot, prev.finalized.beacon.slot / 8192, prev.updates,
        if prev.next_committee.is_some() { "known" } else { "unknown" }, sig_slot / 8192,
        if which == prev.current_committee { "current" } else { "NEXT" }
    );
    if period {
        let (b, f, mb, mf, job) = run_period("real period update", program, signer, &cfg, &prev, &committee, &update)?;
        let ok = report("real period update, Begin", &b) & report("real period update, Finish", &f);
        std::fs::write(format!("{out}.begin"), hex::encode(&mb))?;
        std::fs::write(format!("{out}.finish"), hex::encode(&mf))?;
        let id = job_id(&job.prev, &job.update_digest);
        println!("JOB_ID=0x{}\nJOB_PDA={}", hex::encode(id), job_address(&program, &id));
        println!("messages written to {out}.begin ({} bytes) and {out}.finish ({} bytes)", mb.len(), mf.len());
        return Ok(ok);
    }
    let (tx, message) = run_tx("real update", program, signer, &cfg, &prev, &committee, &update)?;
    let under = report("real update, as one public transaction", &tx);
    std::fs::write(out, hex::encode(&message))?;
    println!("message written to {out} ({} bytes, {} hex chars)", message.len(), message.len() * 2);
    Ok(under)
}
