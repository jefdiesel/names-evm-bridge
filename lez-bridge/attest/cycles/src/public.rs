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
//!       to OUT_HEX for `lez-bridge raw-call`.
//!
//! Exit status 1 if a transaction's sum reaches the cap.

use super::*;
use eth_attest_core::KeyWitnesses;

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
    let bits = eth_attest::lc::finality_bits(update).map_err(|e| anyhow!(e))?;
    let keys: KeyWitnesses = eth_attest::lc::key_witnesses(committee, &bits).map_err(|e| anyhow!(e))?;
    let signers = bits.iter().filter(|b| **b).count();
    println!("  {label}: {signers}/512 signers, {} key witnesses (aggregate + {} non-signers)", keys.absent.len() + 1, keys.absent.len());
    let v = eth_attest::lc::verify_finality_public(&program, cfg, prev, committee, &keys, update)
        .map_err(|e| anyhow!("does not verify natively: {e}"))?;
    let message = msg(&Message::Instruction(Instruction::UpdatePublic {
        config: cfg.clone(),
        prev: prev.clone(),
        current_committee: committee.to_vec(),
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
    let id = prev.instance_id;
    let advance = ReceiveInput {
        receiver: Actor::new(store_address(&program, &id), program),
        from: Some(Actor::new(signer, program)),
        is_authorized: false,
        pre_state: ActorState::from(msg(prev)),
        message: msg(&Message::Effect(Effect::Advance { prev: prev.clone(), next: next.clone() })),
    };
    let (c, _) = exec_user(&format!("{label}: store applies Advance"), ATTEST_GUEST_PUBLIC_ELF, &advance)?;
    calls.push(("store: Advance (+ sends RecordAnchor)".to_owned(), c));
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
    Ok((Tx { calls, next }, message))
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
    ensure!(std::fs::read_to_string(dir.join("update.kind"))?.trim() == "finality", "the public path takes a finality update");
    let update = std::fs::read(dir.join("update.ssz"))?;
    let committee = std::fs::read(dir.join(format!("{}.ssz", hex::encode(prev.current_committee))))?;
    ensure!(committee_hash(&committee) == prev.current_committee, "committee file does not match the store");
    ensure!(instance_id(&program, &cfg) == prev.instance_id, "the config is not this instance's (program id?)");
    println!("# public update path, REAL update from {}", dir.display());
    println!("  store now: finalized slot {}, updates {}", prev.finalized.beacon.slot, prev.updates);
    let (tx, message) = run_tx("real update", program, signer, &cfg, &prev, &committee, &update)?;
    let under = report("real update, as one public transaction", &tx);
    std::fs::write(out, hex::encode(&message))?;
    println!("message written to {out} ({} bytes, {} hex chars)", message.len(), message.len() * 2);
    Ok(under)
}
