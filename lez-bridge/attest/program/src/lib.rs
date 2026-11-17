//! RFP-021 Ethereum state attestation program for LEZ 0.4.
//!
//! An Ethereum sync-committee light client (Helios consensus-core 0.12.0) whose tracked state
//! lives in LEZ accounts, any number of independent instances under one immutable program, and
//! a cheap public check consumers use to bind their own inclusion proofs to a finalized
//! Ethereum block. Account layout, messages and the consumer library are in
//! [`eth_attest_core`]; read its docs first.
//!
//! # Why verification runs in the submitter's actor
//!
//! One finality update costs about 36M user cycles (attest-spike), against LEZ's 10M cap per
//! public transaction *and* per block (`lez/programs/fee/core/src/market.rs`: `MAX_GAS_EXEC`;
//! `validity.rs`; `accumulate_exec_gas`). So the update cannot be verified in public
//! execution. It is verified instead inside the submitter's own actor of this program
//! ([`Instruction::Update`]), which the submitter runs as a private, locally proved execution:
//! a private transition is not metered ("A private transition is bounded only by what its prover
//! can prove", `lee/state_machine/src/privacy_preserving_transaction/circuit/mod.rs:237-242`).
//! Only the result crosses to public state, as one small [`Effect::Advance`] message to the
//! instance's store PDA, which the sequencer executes publicly (about 38K user cycles, plus
//! about 18K per anchor recorded; `attest/cycles`).
//!
//! # Why the public store can trust that effect
//!
//! The store PDA accepts [`Effect::Advance`] only when all of these hold:
//!
//! 1. **The sender is this program's code.** The runtime stamps every message with the sending
//!    actor `(account, program)`; [`handle`] refuses any [`Message::Effect`] unless
//!    `from.program_account_id` is this program (`ReceiveInput::from_own_program`). A private
//!    sender keeps its program id even when its account is blinded
//!    (`lee/state_machine/core/src/execution_state.rs:1407-1428`, `sender()`).
//! 2. **"This program's code" is really this program's image.** In public execution the sequencer
//!    runs the deployed image. In a private execution the circuit verifies a receipt of the image
//!    claimed for the program account (`lee/privacy_preserving_circuit/src/private_backend.rs`,
//!    `env::verify(image_id, transition)`), and that claim is anchored to chain state: a
//!    `Disclosed` claim must equal the account's live image
//!    (`lee/state_machine/src/validated_state_diff/mod.rs:624-647`), an `Undisclosed` one must be
//!    an *immutable* program header committed on chain (`lee/state_machine/core/src/circuit_io.rs:71-94`).
//!    **The program must therefore be deployed with an immutable header**; a mutable header lets
//!    whoever controls it swap the code (RFP-021 Verification Security #3).
//! 3. **That code emits `Advance` only after verifying.** Without the `public-update` feature the
//!    single place this program builds an `Advance` is [`instruct`] for [`Instruction::Update`],
//!    immediately after [`lc::verify_update`] succeeded for exactly the `(config, prev)` it puts in
//!    the message: the instance id is recomputed from the config the verifier used, the committee
//!    witness must hash to `prev`'s commitments, and the destination store is derived from
//!    `prev.instance_id`. With the feature, two more places (`public.rs`): `UpdatePublic` right
//!    after `verify_finality_public`, and a job PDA right after it checked the signature of the
//!    job `PeriodBegin` verified (every other check) for that `prev`.
//! 4. **The verifier's "previous store" is the real one.** The store PDA compares the `prev` in
//!    the message with its own state, field for field, and refuses on any difference ("the store
//!    changed since this update was verified"). Two racing submitters: the second fails cleanly.
//! 5. **It is progress.** The store re-checks that `next` advances `prev` (later finalized slot,
//!    or the next committee newly learned) and keeps instance id and chain id.
//!
//! The submitter's own account state, identity and signature play no part (`keep_state` on the
//! submitter's actor); any account may submit (Functionality #11, Verification Security #6).
//!
//! **What is proven, public vs private.** If the submitter runs `Update` publicly (only possible
//! once a BLS precompile or a cheaper verifier fits 10M), the sequencer executes the verification
//! itself. If privately, the sequencer verifies a succinct proof that this program's image, given
//! the update material and the committee witness, produced the `Advance` it then applies; the
//! material and witness stay off chain. In both cases what becomes public is the same: the
//! `Advance` message (the old and new summary: finalized beacon header, execution block hash and,
//! before Gloas, state root and block number, committee commitments), the anchors recorded, and
//! an `Advanced` event. The update's signature, bits and committee keys are not published by
//! LEZ in the private case; they are public on Ethereum anyway.
//!
//! The same pattern creates an instance: [`Instruction::Init`] verifies the bootstrap against the
//! config's checkpoint in the initialiser's actor, then [`Effect::Create`] writes the config PDA
//! (refused if it exists), which opens the store PDA and records the checkpoint's anchors.
//!
//! # What this does not do
//!
//! - It does not check the weak-subjectivity checkpoint against Ethereum: whoever relies on an
//!   instance must (Functionality #9 asks for an API on the Logos Ethereum core module; not built).
//! - Nothing replaces an instance that fell outside the weak-subjectivity period: a new instance
//!   is initialised from a fresh checkpoint and consumers move to it (Functionality #10).

use borsh::BorshSerialize;
use eth_attest_core::{
    Advanced, AnchorRecord, Effect, InstanceConfig, Instruction, LcSummary, Message, Query, anchor_address,
    anchor_seed, config_address, config_seed, instance_id, store_address, store_seed,
};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response, SyncMessage},
};

pub mod check;
pub mod lc;
#[cfg(feature = "public-update")]
pub mod public;

/// Phase marks for the cycle-count spike (`attest/lcbench`). Compiled out unless the
/// `cycle-marks` feature is on, and records nothing outside the zkVM.
#[cfg(feature = "cycle-marks")]
pub mod marks {
    use std::sync::Mutex;

    /// `(label, absolute cycle count)` in the order reached.
    pub static MARKS: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

    pub fn mark(label: &'static str) {
        #[cfg(target_os = "zkvm")]
        {
            let c = risc0_zkvm::guest::env::cycle_count();
            MARKS.lock().unwrap_or_else(|e| e.into_inner()).push((label, c));
        }
        #[cfg(not(target_os = "zkvm"))]
        let _ = label;
    }
}

macro_rules! mark {
    ($l:literal) => {
        #[cfg(feature = "cycle-marks")]
        crate::marks::mark($l);
    };
}
pub(crate) use mark;

pub use eth_attest_core as core;

/// An effect to this program's PDA under `seed`, carrying the seed. LEZ admits a public account a
/// transaction reaches for the first time only if it already exists, the transaction carries
/// admission evidence for it, or the delivering program presents the seed that derives it
/// (`lee/state_machine/core/src/execution_state.rs` `admit`). The config, store and anchor PDAs
/// are new when `Create`, `Open` and `RecordAnchor` reach them, so every effect carries its seed
/// and no client has to attach (unsigned, strippable) evidence. A seeded delivery also
/// authorizes the PDA; no handler here reads `is_authorized` except [`instruct`], which also
/// requires `from.is_none()`, so a PDA still can never run an instruction.
fn effect(seed: PdaSeed, program: &AccountId, effect: Effect) -> SyncMessage {
    SyncMessage::new(Actor::new(AccountId::for_public_pda(program, &seed), *program), &Message::Effect(effect))
        .with_pda_seeds([seed])
}

fn event<T: BorshSerialize>(name: &str, data: &T) -> ProgramEvent {
    let selector = eth_attest_core::sha256(&[b"rfp021_attest::", name.as_bytes()]);
    ProgramEvent {
        selector: selector[..8].try_into().unwrap_or_else(|_| unreachable!()),
        data: borsh::to_vec(data).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: event serializes")),
    }
}

/// An account state's bytes. Panic texts in this program are fixed: no `assert_eq!`, no
/// `expect` on a `Result`, so a refusal never prints a value (account ids, chain ids, decoder
/// errors). `bridge/tests/panic_texts.rs` checks the source.
fn state<T: BorshSerialize>(value: &T) -> ActorState {
    ActorState::from(borsh::to_vec(value).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: state serializes")))
}

/// The guest's entry point.
#[must_use]
pub fn handle(input: &ReceiveInput) -> Response {
    let Ok(message) = borsh::from_slice::<Message>(&input.message) else {
        panic!("RFP021-E08 malformed input: message must decode")
    };
    match message {
        Message::Instruction(i) => instruct(input, i),
        Message::Effect(e) => {
            assert!(input.from_own_program(), "RFP021-E12 not permitted: only this program may send an effect");
            apply(input, e)
        }
        Message::Query(q) => query(input, &q),
    }
}

/// Runs an instruction in the signer's own actor: all heavy verification happens here.
#[must_use]
pub fn instruct(input: &ReceiveInput, instruction: Instruction) -> Response {
    assert!(input.from.is_none(), "RFP021-E12 not permitted: an instruction is sent by the transaction itself");
    // Any account may submit; it only has to be the signer's own (so a PDA of this program can
    // never be made to run an instruction).
    assert!(input.is_authorized, "RFP021-E12 not permitted: the submitter's account must sign");
    let program = input.receiver.program_account_id;
    match instruction {
        Instruction::Init { config, bootstrap } => {
            let genesis = lc::verify_bootstrap(&program, &config, &bootstrap).unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = config_seed(&genesis.instance_id);
            Response::keep_state().send(effect(seed, &program, Effect::Create { config, genesis }))
        }
        Instruction::Update {
            config,
            prev,
            current_committee,
            next_committee,
            update,
        } => {
            let v = lc::verify_update(&program, &config, &prev, &current_committee, next_committee.as_deref(), &update)
                .unwrap_or_else(|e| {
                let e = eth_attest_core::codes::coded(&e);
                panic!("{e}")
            });
            let seed = store_seed(&prev.instance_id);
            Response::keep_state()
                .send(effect(seed, &program, Effect::Advance { prev, next: v.next }))
                .try_timestamp_window(v.not_before_ms..v.deadline_ms)
                .unwrap_or_else(|_| panic!("RFP021-E04 checkpoint stale or invalid: weak-subjectivity bound passed"))
        }
        // The public path (branch public-update, DESIGN.md §9.3a/§9.3b): src/public.rs.
        #[cfg(feature = "public-update")]
        i => public::instruct(&program, i),
        #[cfg(not(feature = "public-update"))]
        _ => panic!("RFP021-E12 not permitted: this build of the program has no public update path"),
    }
}

fn record_anchors(program: &AccountId, s: &LcSummary, mut r: Response) -> Response {
    for anchor in s.finalized.anchors() {
        let rec = AnchorRecord {
            instance_id: s.instance_id,
            evm_chain_id: s.evm_chain_id,
            anchor,
            beacon_slot: s.finalized.beacon.slot,
            beacon_root: s.finalized.beacon_root,
            block_number: s.finalized.execution.block_number,
        };
        r = r.send(effect(anchor_seed(&s.instance_id, &anchor), program, Effect::RecordAnchor(rec)));
    }
    r
}

fn sender_is(input: &ReceiveInput, account: &AccountId) -> bool {
    input.from.is_some_and(|f| f.account_id == *account)
}

fn apply(input: &ReceiveInput, e: Effect) -> Response {
    let program = input.receiver.program_account_id;
    let here = input.receiver.account_id;
    match e {
        Effect::Create { config, genesis } => {
            let id = instance_id(&program, &config);
            assert!(here == config_address(&program, &id), "RFP021-E12 not permitted: create goes to the instance's config");
            assert!(input.pre_state.is_empty(), "RFP021-E09 stale or replayed: this instance already exists");
            config.validate();
            assert!(genesis.instance_id == id, "RFP021-E13 witness mismatch: store for another instance");
            assert!(genesis.evm_chain_id == config.evm_chain_id, "RFP021-E07 chain id mismatch: store for another chain");
            assert!(genesis.updates == 0, "RFP021-E08 malformed input: a new store has no updates");
            Response::set_state(state(&config)).send(effect(store_seed(&id), &program, Effect::Open { genesis }))
        }
        Effect::Open { genesis } => {
            assert!(
                sender_is(input, &config_address(&program, &genesis.instance_id)),
                "RFP021-E12 not permitted: only the instance's config opens its store"
            );
            assert!(here == store_address(&program, &genesis.instance_id), "RFP021-E12 not permitted: open goes to the instance's store");
            assert!(input.pre_state.is_empty(), "RFP021-E09 stale or replayed: the store is already open");
            let r = Response::set_state(state(&genesis));
            record_anchors(&program, &genesis, r)
        }
        Effect::Advance { prev, next } => {
            assert!(here == store_address(&program, &prev.instance_id), "RFP021-E12 not permitted: advance goes to the instance's store");
            assert!(!input.pre_state.is_empty(), "RFP021-E13 witness mismatch: no such instance");
            let Ok(current) = borsh::from_slice::<LcSummary>(&input.pre_state) else {
                panic!("RFP021-E08 malformed input: store state does not decode")
            };
            assert!(current == prev, "RFP021-E09 stale or replayed: the store changed since this update was verified");
            assert!(next.instance_id == prev.instance_id, "RFP021-E13 witness mismatch: advance for another instance");
            assert!(next.evm_chain_id == prev.evm_chain_id, "RFP021-E07 chain id mismatch: advance for another chain");
            assert!(prev.is_advanced_by(&next), "RFP021-E10 not newer: the update does not advance the store");
            let mut r = Response::set_state(state(&next)).event(event(
                "Advanced",
                &Advanced {
                    instance_id: next.instance_id,
                    beacon_slot: next.finalized.beacon.slot,
                    block_hash: next.finalized.execution.block_hash,
                    next_committee_known: next.next_committee.is_some(),
                },
            ));
            if next.finalized != prev.finalized {
                r = record_anchors(&program, &next, r);
            }
            r
        }
        Effect::RecordAnchor(rec) => {
            assert!(
                sender_is(input, &store_address(&program, &rec.instance_id)),
                "RFP021-E12 not permitted: only the instance's store records anchors"
            );
            assert!(here == anchor_address(&program, &rec.instance_id, &rec.anchor), "RFP021-E12 not permitted: anchor record goes to its PDA");
            if input.pre_state.is_empty() {
                Response::set_state(state(&rec))
            } else {
                // On Gloas several finalized beacon blocks can carry the same last-delivered
                // execution block; the first record stands.
                Response::keep_state()
            }
        }
        #[cfg(feature = "public-update")]
        e @ (Effect::JobOpen(_) | Effect::JobFinish { .. } | Effect::JobFinalize { .. } | Effect::JobClose { .. } | Effect::AssertStale { .. } | Effect::AssertCurrent { .. }) => {
            public::apply_job(input, e)
        }
        #[cfg(not(feature = "public-update"))]
        Effect::JobOpen(_) | Effect::JobFinish { .. } | Effect::JobFinalize { .. } | Effect::JobClose { .. } | Effect::AssertStale { .. } | Effect::AssertCurrent { .. } => {
            panic!("RFP021-E12 not permitted: this build of the program has no public update path")
        }
    }
}

fn query(input: &ReceiveInput, q: &Query) -> Response {
    let program = input.receiver.program_account_id;
    let here = input.receiver.account_id;
    match q {
        Query::AssertAnchor {
            instance_id,
            evm_chain_id,
            anchor,
        } => {
            // Fixed texts: the anchor, instance and chain are public in the query itself, and
            // the same for every claim made under this anchor.
            assert!(here == anchor_address(&program, instance_id, anchor), "RFP021-E12 not permitted: wrong anchor account");
            assert!(!input.pre_state.is_empty(), "RFP021-E01 header not finalized: anchor not finalized by this instance");
            let Ok(rec) = borsh::from_slice::<AnchorRecord>(&input.pre_state) else {
                panic!("RFP021-E08 malformed input: anchor state does not decode")
            };
            assert!(
                rec.instance_id == *instance_id && rec.anchor == *anchor,
                "RFP021-E01 header not finalized: anchor not finalized by this instance"
            );
            assert!(rec.evm_chain_id == *evm_chain_id, "RFP021-E07 chain id mismatch: anchor is for another chain");
        }
        Query::AssertInstance {
            instance_id: id,
            evm_chain_id,
        } => {
            assert!(here == config_address(&program, id), "RFP021-E12 not permitted: wrong instance account");
            assert!(!input.pre_state.is_empty(), "RFP021-E13 witness mismatch: no such instance");
            let Ok(config) = borsh::from_slice::<InstanceConfig>(&input.pre_state) else {
                panic!("RFP021-E08 malformed input: config state does not decode")
            };
            assert!(instance_id(&program, &config) == *id, "RFP021-E13 witness mismatch: no such instance");
            assert!(config.evm_chain_id == *evm_chain_id, "RFP021-E07 chain id mismatch: instance is for another chain");
        }
    }
    Response::keep_state()
}
