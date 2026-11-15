//! Reference consumer of the RFP-021 attestation program.
//!
//! The pattern any consumer follows, with nothing else around it: the transaction sends a
//! [`Probe`] to the signer's own actor of this program; that actor (normally a private,
//! locally proved execution) verifies one statement with [`eth_attest_core::inclusion`] and
//! sends `AssertAnchor` to the attestation instance it is told to use. A real consumer must take
//! the attestation program and instance from its own *immutable* configuration, never from the
//! message as this probe does (RFP-021 Verification Security #3); see the bridge's
//! `ClaimAttested` for that.
//!
//! The actor keeps the statement id as its state, so a test can read what was verified.

use borsh::{BorshDeserialize, BorshSerialize};
use eth_attest_core::{
    assert_anchor,
    inclusion::{
        AccountField, AccountProof, AnchorWitness, InclusionError, Log, Predicate, ReceiptProof, StatementContext,
        StorageProof, verify_account_field, verify_receipt_log, verify_storage_slot,
    },
};
use lee_core::{
    account::{AccountId, ActorState},
    program::{ReceiveInput, Response},
};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Probe {
    Storage {
        attest: AccountId,
        ctx: StatementContext,
        witness: AnchorWitness,
        address: [u8; 20],
        slot: [u8; 32],
        predicate: Predicate,
        proof: StorageProof,
    },
    Account {
        attest: AccountId,
        ctx: StatementContext,
        witness: AnchorWitness,
        address: [u8; 20],
        field: AccountField,
        predicate: Predicate,
        proof: AccountProof,
    },
    Log {
        attest: AccountId,
        ctx: StatementContext,
        witness: AnchorWitness,
        proof: ReceiptProof,
        log_index: u64,
        expect: Log,
    },
}

/// A refusal says only the error's class, never its `Display`: that carries the trie path
/// (`keccak(slot)` of what was proven) and node bytes, witness values a consumer must not
/// publish (see [`InclusionError`]).
fn fail(e: InclusionError) -> ! {
    e.class().refuse()
}

/// The guest's entry point.
#[must_use]
pub fn handle(input: &ReceiveInput) -> Response {
    assert!(input.from.is_none() && input.is_authorized, "the signer's own actor, from the transaction");
    let Ok(probe) = borsh::from_slice::<Probe>(&input.message) else {
        panic!("probe message")
    };
    match probe {
        Probe::Storage { attest, ctx, witness, address, slot, predicate, proof } => {
            let s = verify_storage_slot(&ctx, &witness, &address, &slot, predicate, &proof).unwrap_or_else(|e| fail(e));
            Response::set_state(ActorState::from(s.id().to_vec())).send(assert_anchor(&attest, &ctx.instance_id, ctx.evm_chain_id, &s.anchor))
        }
        Probe::Account { attest, ctx, witness, address, field, predicate, proof } => {
            let s = verify_account_field(&ctx, &witness, &address, field, predicate, &proof).unwrap_or_else(|e| fail(e));
            Response::set_state(ActorState::from(s.id().to_vec())).send(assert_anchor(&attest, &ctx.instance_id, ctx.evm_chain_id, &s.anchor))
        }
        Probe::Log { attest, ctx, witness, proof, log_index, expect } => {
            let s = verify_receipt_log(&ctx, &witness, &proof, log_index).unwrap_or_else(|e| fail(e));
            assert!(s.log == expect, "not the expected log");
            Response::set_state(ActorState::from(s.id().to_vec())).send(assert_anchor(&attest, &ctx.instance_id, ctx.evm_chain_id, &s.anchor))
        }
    }
}
