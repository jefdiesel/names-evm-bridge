// SPDX-License-Identifier: MIT OR Apache-2.0
//! RFP-021 reference consumers (branch `public-update`): a program that pays out (here: records)
//! a claim only for an Ethereum transfer it proves itself, under an anchor an attestation
//! instance finalized.
//!
//! One program, two kinds, each deployment configured once by [`Msg::Init`]:
//! - [`Kind::Erc20`]: an ERC-20 `Transfer(from, to, value)` log of `token` with `to` = `recipient`
//!   (the given contract). The claim gives the sender and the amount.
//! - [`Kind::Eth`]: a native ETH transfer to `contract`, proved through the event the contract
//!   emits when it receives ETH, `Deposit(address indexed dst, uint256 wad)` (WETH9, and the
//!   bridge vault's own deposits work the same way). **An event, not state:** a balance or a
//!   storage slot says how much the contract holds, not who sent it or which transfer it was; a
//!   log names the sender and the amount and has a fixed position (block, transaction, log index)
//!   that can be spent once. A plain ETH transfer to a contract that emits no event cannot be
//!   identified this way (the transaction itself would have to be proved and its sender
//!   recovered from the signature: not built).
//!
//! A claim ([`Msg::Claim`]) runs in the claimer's own actor: [`verify_receipt_log`] against the
//! anchored execution header (block-hash anchor + header RLP, the receipts trie), the log checked
//! against the kind, then three messages in the same transaction:
//! 1. `AssertAnchor` to the attestation program: the anchor is one the instance finalized, for
//!    this chain id (a mismatch is refused there);
//! 2. [`Effect::Spend`] to a nullifier PDA keyed on the deployment and the statement id: refused
//!    if it exists, so the same log can be claimed once;
//! 3. [`Query::AssertConfig`] to the deployment's config PDA: the claim used this deployment's
//!    configuration, not one the claimer made up.
//! The claimer's actor keeps the last claim ([`Claimed`]).

use borsh::{BorshDeserialize, BorshSerialize};
use eth_attest_core::{
    assert_anchor, sha256,
    inclusion::{AnchorWitness, ReceiptProof, StatementContext, verify_receipt_log},
};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};

/// keccak256("Transfer(address,address,uint256)").
pub const TRANSFER_TOPIC: [u8; 32] = hex32("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");
/// keccak256("Deposit(address,uint256)").
pub const DEPOSIT_TOPIC: [u8; 32] = hex32("e1fffcc4923d04b559f4d29a8bfc6cda04eb5b0d3c460751c2402c5c5cc9109c");

const fn nib(c: u8) -> u8 {
    if c >= b'a' { c - b'a' + 10 } else { c - b'0' }
}

const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nib(b[2 * i]) << 4) | nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Kind {
    Erc20 { token: [u8; 20], recipient: [u8; 20] },
    Eth { contract: [u8; 20] },
}

/// One deployment's configuration: which attestation instance and chain, and what it accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    pub attest: AccountId,
    pub instance_id: [u8; 32],
    pub evm_chain_id: u64,
    pub kind: Kind,
}

impl Config {
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        sha256(&[b"RFP021/consumer/config/v1", &borsh::to_vec(self).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: config serializes"))])
    }
}

/// What a claim proved: kept in the claimer's actor.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Claimed {
    pub statement_id: [u8; 32],
    pub sender: [u8; 20],
    pub amount: [u8; 32],
    pub recipient: [u8; 20],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Msg {
    /// From a transaction: create this deployment's config PDA (once).
    Init { config: Config },
    /// From a transaction: claim the transfer in log `log_index` of receipt `proof.tx_index`.
    Claim { config: Config, witness: AnchorWitness, proof: ReceiptProof, log_index: u64 },
    Effect(Effect),
    Query(Query),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    Create { config: Config },
    Spend { config_id: [u8; 32], statement_id: [u8; 32] },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Query {
    AssertConfig { config: Config },
}

#[must_use]
pub fn config_seed(config_id: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[b"/RFP021/consumer/config/v1", config_id]))
}

#[must_use]
pub fn nullifier_seed(config_id: &[u8; 32], statement_id: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[b"/RFP021/consumer/nullifier/v1", config_id, statement_id]))
}

#[must_use]
pub fn config_address(program: &AccountId, config: &Config) -> AccountId {
    AccountId::for_public_pda(program, &config_seed(&config.id()))
}

#[must_use]
pub fn nullifier_address(program: &AccountId, config: &Config, statement_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &nullifier_seed(&config.id(), statement_id))
}

fn to_pda(program: &AccountId, seed: PdaSeed, m: &Msg) -> SyncMessage {
    SyncMessage::new(Actor::new(AccountId::for_public_pda(program, &seed), *program), m).with_pda_seeds([seed])
}

fn word_address(w: &[u8; 32]) -> Option<[u8; 20]> {
    w[..12].iter().all(|b| *b == 0).then(|| w[12..].try_into().unwrap_or_else(|_| unreachable!()))
}

/// The (sender, amount, recipient) of a log the kind accepts.
///
/// # Errors
/// The log is not the kind's transfer.
pub fn accept(kind: &Kind, log: &eth_attest_core::inclusion::Log) -> Result<([u8; 20], [u8; 32], [u8; 20]), &'static str> {
    let amount: [u8; 32] = log.data.as_slice().try_into().map_err(|_| "RFP021-E06 predicate not satisfied: not the configured transfer")?;
    match kind {
        Kind::Erc20 { token, recipient } => {
            let ok = log.address == *token && log.topics.len() == 3 && log.topics[0] == TRANSFER_TOPIC;
            let from = ok.then(|| word_address(&log.topics[1])).flatten();
            let to = ok.then(|| word_address(&log.topics[2])).flatten();
            match (from, to) {
                (Some(from), Some(to)) if to == *recipient => Ok((from, amount, to)),
                _ => Err("RFP021-E06 predicate not satisfied: not the configured transfer"),
            }
        }
        Kind::Eth { contract } => {
            let ok = log.address == *contract && log.topics.len() == 2 && log.topics[0] == DEPOSIT_TOPIC;
            match ok.then(|| word_address(&log.topics[1])).flatten() {
                Some(from) if amount != [0u8; 32] => Ok((from, amount, *contract)),
                _ => Err("RFP021-E06 predicate not satisfied: not the configured transfer"),
            }
        }
    }
}

/// The guest's entry point.
#[must_use]
pub fn handle(input: &ReceiveInput) -> Response {
    let Ok(m) = borsh::from_slice::<Msg>(&input.message) else {
        panic!("RFP021-E08 malformed input: message must decode")
    };
    let program = input.receiver.program_account_id;
    let here = input.receiver.account_id;
    match m {
        Msg::Init { config } => {
            assert!(input.from.is_none() && input.is_authorized, "RFP021-E12 not permitted: the signer's own actor, from the transaction");
            Response::keep_state().send(to_pda(&program, config_seed(&config.id()), &Msg::Effect(Effect::Create { config })))
        }
        Msg::Claim { config, witness, proof, log_index } => {
            assert!(input.from.is_none() && input.is_authorized, "RFP021-E12 not permitted: the signer's own actor, from the transaction");
            let ctx = StatementContext { evm_chain_id: config.evm_chain_id, instance_id: config.instance_id };
            let s = verify_receipt_log(&ctx, &witness, &proof, log_index).unwrap_or_else(|e| e.class().refuse());
            let Ok((sender, amount, recipient)) = accept(&config.kind, &s.log) else {
                panic!("RFP021-E06 predicate not satisfied: not the configured transfer")
            };
            let statement_id = s.id();
            let claimed = Claimed { statement_id, sender, amount, recipient };
            Response::set_state(ActorState::from(borsh::to_vec(&claimed).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: claim serializes"))))
                .send(assert_anchor(&config.attest, &config.instance_id, config.evm_chain_id, &s.anchor))
                .send(to_pda(&program, nullifier_seed(&config.id(), &statement_id), &Msg::Effect(Effect::Spend { config_id: config.id(), statement_id })))
                .send(SyncMessage::new(Actor::new(config_address(&program, &config), program), &Msg::Query(Query::AssertConfig { config })))
        }
        Msg::Effect(e) => {
            assert!(input.from_own_program(), "RFP021-E12 not permitted: only this program may send an effect");
            match e {
                Effect::Create { config } => {
                    assert!(here == config_address(&program, &config), "RFP021-E12 not permitted: config goes to its PDA");
                    assert!(input.pre_state.is_empty(), "RFP021-E09 stale or replayed: this deployment exists");
                    Response::set_state(ActorState::from(borsh::to_vec(&config).unwrap_or_else(|_| panic!("RFP021-E08 malformed input: config serializes"))))
                }
                Effect::Spend { config_id, statement_id } => {
                    assert!(here == AccountId::for_public_pda(&program, &nullifier_seed(&config_id, &statement_id)), "RFP021-E12 not permitted: nullifier goes to its PDA");
                    assert!(input.pre_state.is_empty(), "RFP021-E09 stale or replayed: already claimed");
                    Response::set_state(ActorState::from(vec![1u8]))
                }
            }
        }
        Msg::Query(Query::AssertConfig { config }) => {
            assert!(here == config_address(&program, &config), "RFP021-E12 not permitted: wrong config account");
            let Ok(stored) = borsh::from_slice::<Config>(&input.pre_state) else {
                panic!("RFP021-E13 witness mismatch: no such deployment")
            };
            assert!(stored == config, "RFP021-E13 witness mismatch: not this deployment's config");
            Response::keep_state()
        }
    }
}
