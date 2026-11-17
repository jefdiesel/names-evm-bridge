//! The whole-transaction simulator and fixtures shared by every bridge integration test (moved
//! here from `flow.rs` and `attest_claim.rs`, unchanged in behaviour, so new test files reuse it
//! instead of copying it).
//!
//! A small simulator runs the token and bridge programs, the real RFP-021 attestation program,
//! and LEZ's own native token code (`lee_core::native_token::handle_message`), together,
//! delivering sync messages depth-first inside one transaction (any failure undoes the whole
//! transaction) and async messages afterwards, each as its own transaction. Which program id runs
//! which handler is a registry ([`World::programs`]), so one world can hold two bridge
//! deployments or two attestation programs.
//!
//! What it models from LEZ's executor (`lee/state_machine/core/src/execution_state.rs`):
//! - seed grants: a delivery whose sender's program derives the receiving account from one of
//!   the message's PDA seeds authorizes it (`authorize`);
//! - admission, for bridge PDAs only: a bridge actor of an account that holds no state yet,
//!   reached by anyone but the signer, must be reached with its seed (`admit`); LEZ would also
//!   accept unsigned admission evidence, which the CLI no longer sends for these PDAs;
//! - aliases: a transaction may carry openings for its signer, and a message to
//!   `signer.blinded(opening)` is handled by the signer's actor (`resolve`).
//!
//! What it does not model: private accounts (every actor state, including a private claimer's
//! own, sits in the same public map), sender presentation (LEZ shows a private sender under a
//! fresh blinded id; here `from` is always the canonical actor), promotion (an async message to a
//! private account lands inside the same proof in LEZ; here it runs after the transaction, with
//! the transaction's aliases still registered), publication rules for undeliverable async
//! messages, inherited authorizations, charged reverts, and fees. An async message that fails is
//! undone on its own: the transaction's synchronous part stays applied (`tx` then returns
//! `Err("async: ..")`), so an `Err` from [`World::tx`] does not by itself mean "nothing changed".
//!
//! The attested-path fixture writes an attestation instance's config and finalized anchors
//! straight into the simulator's state: that is the test-only way to stand up an instance at a
//! chosen anchor; the program has no such code path. The vault's Ethereum state is synthetic (a
//! state trie built with alloy-trie, so every proof is a real Merkle-Patricia proof).

#![allow(dead_code)]

pub use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use bridge_core::*;
pub use bridge_token_core::{Authority, BurnNotice, Message as Tok, TokenDefinition, TokenState, balance_of};
pub use eth_attest_core::{
    Anchor, AnchorRecord, Fork, InstanceConfig, MIN_SUPERMAJORITY, anchor_address, config_address as attest_config_address,
    inclusion::{AnchorWitness, StorageProof, keccak, mapping_slot, word},
    instance_id,
    testkit::{SynthAccount, SynthState, header_rlp},
};
pub use lee_core::{
    account::{AccountId, Actor, ActorState},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID as NATIVE, decode_balance, encode_balance},
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response},
};

pub const TOKEN: AccountId = AccountId::new([0x70; 32]);
pub const BRIDGE: AccountId = AccountId::new([0xB0; 32]);
/// A second, independent bridge deployment (B6).
pub const BRIDGE2: AccountId = AccountId::new([0xB2; 32]);
pub const ATTEST: AccountId = AccountId::new([0xE7; 32]);
/// A second attestation program (same code, another program id).
pub const ATTEST2: AccountId = AccountId::new([0xE8; 32]);
/// A program that accepts any message: what a claimer would point `AssertAnchor` at to cheat.
pub const YES: AccountId = AccountId::new([0x55; 32]);
pub const ADMIN: AccountId = AccountId::new([0xAD; 32]);
pub const FREEZER: AccountId = AccountId::new([0xF2; 32]);
pub const ATTESTOR: AccountId = AccountId::new([0xA7; 32]);
pub const ADMIN2: AccountId = AccountId::new([0xAE; 32]);
pub const FREEZER2: AccountId = AccountId::new([0xF3; 32]);
pub const ATTESTOR2: AccountId = AccountId::new([0xA8; 32]);
pub const ALICE: AccountId = AccountId::new([0xA1; 32]);
pub const BOB: AccountId = AccountId::new([0xB1; 32]);
pub const CAROL: AccountId = AccountId::new([0xC1; 32]);
pub const DAVE: AccountId = AccountId::new([0xD1; 32]);
pub const DEF: AccountId = AccountId::new([0xDE; 32]);
/// The second wrapped definition (DAI on `BRIDGE`, or USDC on `BRIDGE2`).
pub const DEF2: AccountId = AccountId::new([0xD2; 32]);
/// Anyone with native LEZ: tops up the bridge's float.
pub const FUNDER: AccountId = AccountId::new([0xF0; 32]);
pub const USDC: EthAddress = [0xAA; 20];
pub const DAI: EthAddress = [0xDA; 20];
pub const VAULT: EthAddress = [0x11; 20];
/// Another contract with the same mapping entry set (e.g. a second vault deployment).
pub const OTHER: EthAddress = [0x12; 20];
pub const VAULT2: EthAddress = [0x13; 20];
pub const EVM_CHAIN: u64 = 11_155_111;
/// The LEZ chain's (declared) Bedrock channel id.
pub const LEZ_CHAIN: [u8; 32] = [0x4C; 32];
/// A zone id as the retired `Init` took it from the sender.
pub const ZONE: [u8; 32] = [0x5A; 32];

/// The deployment id the bridge derives at `Initialize` for the `setup()` inputs (stand-in).
pub fn cfg() -> [u8; 32] {
    cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &source_id(&RootSource::StandIn), &BRIDGE)
}

/// The deployment id of an RFP-021 config on `BRIDGE` with source `src`.
pub fn cfg_for_source(src: AttestSource) -> [u8; 32] {
    cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &source_id(&RootSource::Attest(src)), &BRIDGE)
}

/// [`cfg_for_source`] of the default instance (`instance_config(EVM_CHAIN)` at `ATTEST`), the one
/// [`attested_setup`] and the other attested fixtures use.
pub fn acfg() -> [u8; 32] {
    cfg_for_source(source(instance_id(&ATTEST, &instance_config(EVM_CHAIN))))
}

/// `Initialize` with `ADMIN` as admin; the header account (`BRIDGE`) signs it.
pub fn init_with(vault: EthAddress, freezer: AccountId, attestor: AccountId) -> Message {
    instr(Instruction::Initialize {
        admin: ADMIN,
        vault,
        evm_chain_id: EVM_CHAIN,
        lez_chain_id: LEZ_CHAIN,
        token_program: TOKEN,
        freezer,
        attestor,
    })
}

// ---------------------------------------------------------------------------------------------
// Simulator

/// Which handler a program id runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prog {
    Token,
    Bridge,
    Attest,
    /// Accepts anything ([`YES`]).
    Yes,
}

#[derive(Clone)]
pub struct World {
    pub states: BTreeMap<(AccountId, AccountId), Vec<u8>>,
    pub events: Vec<ProgramEvent>,
    /// The current transaction's aliases (`signer.blinded(opening)` -> signer).
    pub aliases: BTreeMap<AccountId, AccountId>,
    /// Every message the current transaction delivered: (from, envelope `to`, bytes, seeds). The
    /// envelope `to` is the address as sent (an alias stays an alias), as LEZ journals it. Async
    /// deliveries after the transaction are appended too.
    pub trace: Vec<(Option<Actor>, Actor, Vec<u8>, BTreeSet<PdaSeed>)>,
    /// The current transaction's events, with the (resolved) actor that emitted each.
    pub emitted: Vec<(Actor, ProgramEvent)>,
    /// Program id -> handler. Native is always LEZ's own code.
    pub programs: BTreeMap<AccountId, Prog>,
    /// Off (the default): each async message runs as its own transaction after this one, and a
    /// failing one is undone alone. On: a failing async message undoes the whole transaction, as
    /// LEZ does for an async message it promotes into the same transaction (one to an actor that
    /// runs publicly in it, or a credit the prover promotes: `execution_state.rs` 1372-1398 at
    /// v0.4.0-rc1). An unpromoted message waits in LEZ's mailbox instead, which is not modelled.
    pub atomic_asyncs: bool,
    /// Accounts this world treats as private for one thing only: their actors' messages are
    /// presented to the receiver under a blinded alias, as LEZ presents a private sender
    /// (`execution_state.rs` `sender`). Empty by default (every sender canonical).
    pub private: BTreeSet<AccountId>,
}

/// The presentation factor the simulator uses for a private sender's alias.
pub const PRESENTATION: [u8; 32] = [0x9E; 32];

impl Default for World {
    fn default() -> Self {
        Self {
            states: BTreeMap::new(),
            events: Vec::new(),
            aliases: BTreeMap::new(),
            trace: Vec::new(),
            emitted: Vec::new(),
            programs: BTreeMap::from([(TOKEN, Prog::Token), (BRIDGE, Prog::Bridge), (ATTEST, Prog::Attest), (YES, Prog::Yes)]),
            atomic_asyncs: false,
            private: BTreeSet::new(),
        }
    }
}

/// Whether `seeds` hold the seed under which the sender's program derives `account`.
pub fn seeded(from: Option<Actor>, seeds: &BTreeSet<PdaSeed>, account: AccountId) -> bool {
    from.is_some_and(|f| seeds.iter().any(|s| AccountId::for_public_pda(&f.program_account_id, s) == account))
}

pub fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "panic".to_owned())
}

impl World {
    pub fn state(&self, account: AccountId, program: AccountId) -> Vec<u8> {
        self.states.get(&(account, program)).cloned().unwrap_or_default()
    }

    pub fn exists(&self, account: AccountId) -> bool {
        self.states.keys().any(|(a, _)| *a == account)
    }

    pub fn deliver(
        &mut self,
        from: Option<Actor>,
        to: Actor,
        message: Vec<u8>,
        seeds: BTreeSet<PdaSeed>,
        signer: Option<AccountId>,
        asyncs: &mut VecDeque<(Actor, Actor, Vec<u8>)>,
    ) -> Result<(), String> {
        self.trace.push((from, to, message.clone(), seeds.clone()));
        let receiver = Actor::new(self.aliases.get(&to.account_id).copied().unwrap_or(to.account_id), to.program_account_id);
        let granted = seeded(from, &seeds, receiver.account_id);
        let prog = self.programs.get(&receiver.program_account_id).copied();
        if prog == Some(Prog::Bridge) && signer != Some(receiver.account_id) && !self.exists(receiver.account_id) && !granted {
            return Err(format!("unadmitted public actor {:?}", receiver.account_id));
        }
        let input = ReceiveInput {
            receiver,
            from,
            is_authorized: signer == Some(receiver.account_id) || granted,
            pre_state: ActorState::from(self.state(receiver.account_id, receiver.program_account_id)),
            message,
        };
        let response: Response = if receiver.program_account_id == NATIVE {
            native_token::handle_message(&input).map_err(|e| e.to_string())?.response
        } else {
            let handler: fn(&ReceiveInput) -> Response = match prog {
                Some(Prog::Token) => bridge_token_core::handle,
                Some(Prog::Bridge) => bridge_core::handle,
                Some(Prog::Attest) => eth_attest::handle,
                Some(Prog::Yes) => |_| Response::keep_state(),
                None => return Err(format!("no program at {:?}", receiver.program_account_id)),
            };
            catch_unwind(AssertUnwindSafe(|| handler(&input))).map_err(panic_text)?
        };
        if let Some(post) = response.post_state {
            self.states.insert((receiver.account_id, receiver.program_account_id), post.into_inner());
        }
        for ev in &response.events {
            self.emitted.push((receiver, ev.clone()));
        }
        self.events.extend(response.events);
        let from = if self.private.contains(&receiver.account_id) {
            Actor::new(receiver.account_id.blinded(&PRESENTATION), receiver.program_account_id)
        } else {
            receiver
        };
        for m in response.sync_messages {
            self.deliver(Some(from), m.to, m.message, m.pda_seeds, signer, asyncs)?;
        }
        for m in response.async_messages {
            asyncs.push_back((from, m.to, m.message));
        }
        Ok(())
    }

    /// One transaction: `signer` sends `message` to its own actor of `program`. On failure
    /// nothing changes. Then async messages are delivered, each as its own transaction.
    pub fn tx<M: borsh::BorshSerialize>(&mut self, signer: AccountId, program: AccountId, message: &M) -> Result<(), String> {
        self.tx_with_openings(signer, program, message, &[])
    }

    /// [`World::tx`] whose witness carries `openings` for the signer, as a private claimer's
    /// wallet does: messages to `signer.blinded(o)` reach the signer's actors.
    pub fn tx_with_openings<M: borsh::BorshSerialize>(
        &mut self,
        signer: AccountId,
        program: AccountId,
        message: &M,
        openings: &[[u8; 32]],
    ) -> Result<(), String> {
        let snapshot = self.clone();
        self.trace.clear();
        self.emitted.clear();
        self.aliases = openings.iter().map(|o| (signer.blinded(o), signer)).collect();
        let mut asyncs = VecDeque::new();
        let to = Actor::new(signer, program);
        if let Err(e) = self.deliver(None, to, borsh::to_vec(message).unwrap(), BTreeSet::new(), Some(signer), &mut asyncs) {
            let trace = std::mem::take(&mut self.trace);
            let emitted = std::mem::take(&mut self.emitted);
            *self = snapshot;
            self.trace = trace;
            self.emitted = emitted;
            return Err(e);
        }
        while let Some((from, to, msg)) = asyncs.pop_front() {
            let snap = self.clone();
            let mut more = VecDeque::new();
            if let Err(e) = self.deliver(Some(from), to, msg, BTreeSet::new(), None, &mut more) {
                if self.atomic_asyncs {
                    let trace = std::mem::take(&mut self.trace);
                    *self = snapshot;
                    self.trace = trace;
                } else {
                    *self = snap;
                }
                self.aliases.clear();
                return Err(format!("async: {e}"));
            }
            asyncs.extend(more);
        }
        self.aliases.clear();
        Ok(())
    }

    pub fn native(&self, who: AccountId) -> u128 {
        decode_balance(&self.state(who, NATIVE)).unwrap()
    }

    /// The bridge's float: the config account's native balance.
    pub fn float(&self) -> u128 {
        self.float_of(BRIDGE)
    }

    pub fn float_of(&self, program: AccountId) -> u128 {
        self.native(config_address(&program))
    }

    pub fn wrapped(&self, who: AccountId) -> u128 {
        self.bal(who, DEF)
    }

    /// `who`'s balance of the wrapped definition `def`.
    pub fn bal(&self, who: AccountId, def: AccountId) -> u128 {
        balance_of(&self.state(who, TOKEN), &def)
    }

    pub fn config(&self) -> Config {
        self.config_of(BRIDGE)
    }

    pub fn config_of(&self, program: AccountId) -> Config {
        borsh::from_slice(&self.state(config_address(&program), program)).unwrap()
    }

    pub fn supply(&self) -> u128 {
        self.supply_of(DEF)
    }

    pub fn supply_of(&self, def: AccountId) -> u128 {
        match TokenState::decode(&self.state(def, TOKEN)) {
            Some(TokenState::Definition(TokenDefinition::Fungible { total_supply, .. })) => total_supply,
            _ => panic!("no definition"),
        }
    }

    /// The sum of every account's balance of `def` under the token program.
    pub fn holdings_sum(&self, def: AccountId) -> u128 {
        self.states
            .iter()
            .filter(|((_, p), _)| *p == TOKEN)
            .map(|(_, s)| balance_of(s, &def))
            .sum()
    }
}

pub fn instr(i: Instruction) -> Message {
    Message::Instruction(i)
}

pub fn freeze(eth_token: Option<EthAddress>, mint: bool, burn: bool) -> Message {
    instr(Instruction::Admin(AdminOp::Freeze { eth_token, mint, burn }))
}

pub fn unfreeze(eth_token: Option<EthAddress>, mint: bool, burn: bool) -> Message {
    instr(Instruction::Admin(AdminOp::Unfreeze { eth_token, mint, burn }))
}

pub fn err_contains(r: Result<(), String>, want: &str) {
    let e = r.expect_err("expected a refusal");
    assert!(e.contains(want), "wanted '{want}', got: {e}");
}

/// A refusal whose text is exactly `want`.
#[track_caller]
pub fn err_is(r: Result<(), String>, want: &str) {
    let e = r.expect_err("expected a refusal");
    assert_eq!(e, want);
}

// ---------------------------------------------------------------------------------------------
// Stand-in fixtures (from flow.rs)

/// A bridge with USDC (100 and 1000) registered and its wrapped definition minted by the bridge.
pub fn setup() -> World {
    let mut w = World::default();
    w.tx(
        DEF,
        TOKEN,
        &Tok::NewFungibleDefinition {
            name: "Wrapped USDC".into(),
            total_supply: 0,
            mint_authority: Some(Authority::Program(BRIDGE)),
            supply_to: ADMIN,
        },
    )
    .unwrap();
    w.tx(BRIDGE, BRIDGE, &init_with(VAULT, FREEZER, ATTESTOR)).unwrap();
    w.tx(
        ADMIN,
        BRIDGE,
        &instr(Instruction::Admin(AdminOp::RegisterToken {
            eth_token: USDC,
            definition: DEF,
            decimals: 6,
            denoms: vec![100_000_000, 1_000_000_000],
        })),
    )
    .unwrap();
    w
}

/// Deposits as the vault would record them: returns (leaves, root) for (secret, dest, denom) list.
pub fn vault(deposits: &[([u8; 32], AccountId, u128)]) -> (Vec<[u8; 32]>, [u8; 32]) {
    let leaves: Vec<[u8; 32]> = deposits
        .iter()
        .map(|(k, dest, d)| deposit_leaf(&cfg(), &USDC, *d, &deposit_commitment(k, dest, &USDC, *d, &cfg())))
        .collect();
    let path = merkle_path(&leaves, 0);
    let root = merkle_root(&leaves[0], 0, &path);
    (leaves, root)
}

pub fn claim(k: [u8; 32], idx: usize, leaves: &[[u8; 32]], root: [u8; 32], denom_index: u32, denom: u128) -> Message {
    claim_to(k, idx, leaves, root, denom_index, denom, None)
}

/// [`claim`] with a one-time recipient opening.
pub fn claim_to(
    k: [u8; 32],
    idx: usize,
    leaves: &[[u8; 32]],
    root: [u8; 32],
    denom_index: u32,
    denom: u128,
    recipient_opening: Option<[u8; 32]>,
) -> Message {
    instr(Instruction::Claim {
        eth_token: USDC,
        denom_index,
        denom,
        root,
        leaf_index: idx as u64,
        path: merkle_path(leaves, idx),
        secret: k,
        cfg_id: cfg(),
        recipient_opening,
    })
}

/// A burn notice delivered straight to the bridge config as if from the token program, so the
/// bridge's own bound is what refuses it (the token program would refuse an unbacked burn first).
pub fn notice(w: &mut World, amount: u128) -> Result<(), String> {
    let snapshot = w.clone();
    let notice = BurnNotice { definition: DEF, amount, memo: [7; 32] };
    let mut asyncs = VecDeque::new();
    let r = w.deliver(
        Some(Actor::new(DEF, TOKEN)),
        Actor::new(config_address(&BRIDGE), BRIDGE),
        borsh::to_vec(&notice).unwrap(),
        BTreeSet::new(),
        None,
        &mut asyncs,
    );
    if r.is_err() {
        *w = snapshot;
    }
    r
}

/// Burn one 100 of USDC for exit from `who` through the token program.
pub fn burn(w: &mut World, who: AccountId) -> Result<(), String> {
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    w.tx(who, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: 100_000_000, notify, memo: [4; 32] })
}

/// `setup()` with an attested root holding three 100 deposits to Alice; the first is claimed.
/// Returns the world, the unclaimed deposits' secrets, the leaves and the root.
pub fn claimed_one() -> (World, [[u8; 32]; 2], Vec<[u8; 32]>, [u8; 32]) {
    let mut w = setup();
    let ks = [[1u8; 32], [2u8; 32], [3u8; 32]];
    let (leaves, root) = vault(&[(ks[0], ALICE, 100_000_000), (ks[1], ALICE, 100_000_000), (ks[2], ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(ks[0], 0, &leaves, root, 0, 100_000_000)).unwrap();
    (w, [ks[1], ks[2]], leaves, root)
}

pub fn set_sponsor(w: &mut World, amount: u128) {
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount }))).unwrap();
}

/// Anyone tops up the float with a plain native transfer to the config account.
pub fn fund_float(w: &mut World, amount: u128) {
    fund_float_of(w, BRIDGE, amount);
}

pub fn fund_float_of(w: &mut World, program: AccountId, amount: u128) {
    let have = w.native(FUNDER);
    w.states.insert((FUNDER, NATIVE), encode_balance(have + amount).into_inner());
    w.tx(FUNDER, NATIVE, &native_token::Message::Transfer { to: config_address(&program), amount }).unwrap();
}

/// `setup()` with an attested root holding Alice's 100 (k1) and 1000 (k2) deposits.
pub fn two_deposits() -> (World, [[u8; 32]; 2], Vec<[u8; 32]>, [u8; 32]) {
    let mut w = setup();
    let ks = [[1u8; 32], [2u8; 32]];
    let (leaves, root) = vault(&[(ks[0], ALICE, 100_000_000), (ks[1], ALICE, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    (w, ks, leaves, root)
}

/// The config's claim effect in the current transaction's trace.
pub fn claim_effect(w: &World) -> Effect {
    w.trace
        .iter()
        .find_map(|(_, to, m, _)| match borsh::from_slice::<Message>(m) {
            Ok(Message::Effect(e @ Effect::Claim { .. })) if to.account_id == config_address(&BRIDGE) => Some(e),
            _ => None,
        })
        .expect("a claim effect")
}

/// Every message of the last transaction past the signer's own instruction, and every event:
/// none is addressed to `who` and none carries its 32 bytes.
pub fn assert_never_named(w: &World, who: AccountId) {
    assert!(w.trace[0].0.is_none(), "the trace starts with the transaction's own instruction");
    for (_, to, m, seeds) in &w.trace[1..] {
        assert_ne!(to.account_id, who, "a message is addressed to the real account");
        assert!(!m.windows(32).any(|x| x == who.value()), "a message carries the real account");
        assert!(seeds.iter().all(|s| s.as_bytes() != who.value()));
    }
    for e in &w.events {
        assert!(!e.data.windows(32).any(|x| x == who.value()), "an event carries the real account");
    }
}

// ---------------------------------------------------------------------------------------------
// Attested-path fixtures (from attest_claim.rs)

/// A Sepolia-like instance config; only its hash and chain id matter here.
pub fn instance_config(chain: u64) -> InstanceConfig {
    InstanceConfig {
        evm_chain_id: chain,
        genesis_validators_root: [0xD8; 32],
        genesis_time: 1_655_733_600,
        seconds_per_slot: 12,
        forks: [Fork { epoch: 0, version: [0x90, 0, 0, 0x69] }; 8],
        checkpoint: [0xC0; 32],
        min_participants: MIN_SUPERMAJORITY,
        salt: [0; 32],
    }
}

/// Test-only: an attestation instance exists with `config` (written into the simulator).
pub fn plant_instance(w: &mut World, config: &InstanceConfig) -> [u8; 32] {
    plant_instance_at(w, ATTEST, config)
}

pub fn plant_instance_at(w: &mut World, attest: AccountId, config: &InstanceConfig) -> [u8; 32] {
    let id = instance_id(&attest, config);
    w.states.insert((attest_config_address(&attest, &id), attest), borsh::to_vec(config).unwrap());
    id
}

/// Test-only: the instance finalized `anchor` (as its store would have recorded it).
pub fn plant_anchor(w: &mut World, id: &[u8; 32], chain: u64, anchor: Anchor) {
    plant_anchor_at(w, ATTEST, id, chain, anchor);
}

pub fn plant_anchor_at(w: &mut World, attest: AccountId, id: &[u8; 32], chain: u64, anchor: Anchor) {
    let rec = AnchorRecord { instance_id: *id, evm_chain_id: chain, anchor, beacon_slot: 7, beacon_root: [0; 32], block_number: None };
    w.states.insert((anchor_address(&attest, id, &anchor), attest), borsh::to_vec(&rec).unwrap());
}

pub fn source(id: [u8; 32]) -> AttestSource {
    AttestSource { attest_program: ATTEST, attest_instance: id, deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT }
}

/// `InitializeAttested` with `ADMIN` as admin; the header account (`BRIDGE`) signs it.
pub fn init_attested(src: AttestSource, chain: u64) -> Message {
    instr(Instruction::InitializeAttested {
        admin: ADMIN,
        vault: VAULT,
        evm_chain_id: chain,
        lez_chain_id: LEZ_CHAIN,
        token_program: TOKEN,
        freezer: FREEZER,
        source: src,
    })
}

/// Deposits as the vault records them under the default attested deployment ([`acfg`]); returns
/// the leaves and the root after the last one.
pub fn deposits(ds: &[([u8; 32], AccountId, u128)]) -> (Vec<[u8; 32]>, [u8; 32]) {
    let leaves: Vec<[u8; 32]> = ds
        .iter()
        .map(|(k, dest, d)| deposit_leaf(&acfg(), &USDC, *d, &deposit_commitment(k, dest, &USDC, *d, &acfg())))
        .collect();
    let root = merkle_root(&leaves[0], 0, &merkle_path(&leaves, 0));
    (leaves, root)
}

pub fn is_deposit_root(root: &[u8; 32]) -> [u8; 32] {
    mapping_slot(root, VAULT_DEPOSIT_ROOT_SLOT)
}

pub struct Fixture {
    pub w: World,
    pub id: [u8; 32],
    /// Alice's 100 and Bob's 1000 under `root`, which the vault marked true.
    pub keys: [[u8; 32]; 2],
    pub leaves: Vec<[u8; 32]>,
    pub root: [u8; 32],
    pub state: SynthState,
    pub state_anchor: Anchor,
    pub header: Vec<u8>,
    pub block_anchor: Anchor,
}

/// The vault's Ethereum state: `isDepositRoot` true for an older root and `root`, other storage,
/// another contract with the same entry, a few EOAs. Both a state-root and a block-hash anchor
/// of it are finalized by the instance.
pub fn attested_setup() -> Fixture {
    let mut w = World::default();
    let id = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    w.tx(
        DEF,
        TOKEN,
        &Tok::NewFungibleDefinition { name: "Wrapped USDC".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN },
    )
    .unwrap();
    w.tx(BRIDGE, BRIDGE, &init_attested(source(id), EVM_CHAIN)).unwrap();
    w.tx(
        ADMIN,
        BRIDGE,
        &instr(Instruction::Admin(AdminOp::RegisterToken {
            eth_token: USDC,
            definition: DEF,
            decimals: 6,
            denoms: vec![100_000_000, 1_000_000_000],
        })),
    )
    .unwrap();
    let keys = [[1u8; 32], [2u8; 32]];
    let (leaves, root) = deposits(&[(keys[0], ALICE, 100_000_000), (keys[1], BOB, 1_000_000_000)]);
    let (_, older) = deposits(&[(keys[0], ALICE, 100_000_000)]);
    let state = SynthState {
        accounts: vec![
            SynthAccount::new(VAULT)
                .with(is_deposit_root(&older), word(1))
                .with(is_deposit_root(&root), word(1))
                .with(word(0), word(0xAD))
                .with(word(4), word(7)),
            SynthAccount::new(OTHER).with(is_deposit_root(&root), word(1)),
            SynthAccount::new([0x21; 20]),
            SynthAccount::new([0x22; 20]),
        ],
    };
    let state_anchor = Anchor::StateRoot(state.state_root());
    plant_anchor(&mut w, &id, EVM_CHAIN, state_anchor);
    let header = header_rlp(state.state_root(), [0; 32], 12_000_000, 0);
    let block_anchor = Anchor::BlockHash(keccak(&header));
    plant_anchor(&mut w, &id, EVM_CHAIN, block_anchor);
    Fixture { w, id, keys, leaves, root, state, state_anchor, header, block_anchor }
}

impl Fixture {
    /// Alice's (index 0) or Bob's (index 1) honest claim against the state-root anchor.
    pub fn claim(&self, who: usize) -> Instruction {
        let denom = [100_000_000, 1_000_000_000][who];
        Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: who as u32,
            denom,
            root: self.root,
            leaf_index: who as u64,
            path: merkle_path(&self.leaves, who),
            secret: self.keys[who],
            cfg_id: acfg(),
            source: source(self.id),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor: AnchorWitness { anchor: self.state_anchor, header_rlp: None },
            proof: self.state.storage_proof(&VAULT, &is_deposit_root(&self.root)),
            recipient_opening: None,
        }
    }

    /// [`Fixture::claim`] to the claimer's default one-time address.
    pub fn claim_one_time(&self, who: usize) -> (Instruction, [u8; 32]) {
        let o = recipient_opening(&self.keys[who]);
        let mut i = self.claim(who);
        if let Instruction::ClaimAttested { recipient_opening, .. } = &mut i {
            *recipient_opening = Some(o);
        }
        (i, o)
    }

    /// [`Fixture::claim`] against the block-hash anchor (header supplied).
    pub fn claim_by_block_hash(&self, who: usize) -> Instruction {
        let (header, block_anchor) = (self.header.clone(), self.block_anchor);
        with(self.claim(who), |x| *x.anchor = AnchorWitness { anchor: block_anchor, header_rlp: Some(header) })
    }

    pub fn tx(&mut self, who: AccountId, i: Instruction) -> Result<(), String> {
        self.w.tx(who, BRIDGE, &instr(i))
    }
}

/// Edits one field of a `ClaimAttested`.
pub fn with(mut i: Instruction, f: impl FnOnce(&mut AttestedFields)) -> Instruction {
    if let Instruction::ClaimAttested { root, source, vault, evm_chain_id, anchor, proof, .. } = &mut i {
        f(&mut AttestedFields { root, source, vault, evm_chain_id, anchor, proof });
    }
    i
}

pub struct AttestedFields<'a> {
    pub root: &'a mut [u8; 32],
    pub source: &'a mut AttestSource,
    pub vault: &'a mut EthAddress,
    pub evm_chain_id: &'a mut u64,
    pub anchor: &'a mut AnchorWitness,
    pub proof: &'a mut StorageProof,
}

// ---------------------------------------------------------------------------------------------
// Generic helpers for tests that use several deployments, tokens or hash-derived secrets.

/// A hash-derived 32-byte test secret (no zero or repeated-byte patterns that would match
/// padding by accident).
pub fn secret(tag: &str, i: u64) -> [u8; 32] {
    sha256(&[b"test-secret/", tag.as_bytes(), &i.to_be_bytes()])
}

/// The vault leaf of a deposit under deployment `cfg`.
pub fn leaf_for(cfg: &[u8; 32], k: &[u8; 32], dest: &AccountId, token: &EthAddress, denom: u128) -> [u8; 32] {
    deposit_leaf(cfg, token, denom, &deposit_commitment(k, dest, token, denom, cfg))
}

/// The vault's root over `leaves` (all of them).
pub fn root_of(leaves: &[[u8; 32]]) -> [u8; 32] {
    merkle_root(&leaves[0], 0, &merkle_path(leaves, 0))
}

/// A stand-in claim of leaf `idx` of `leaves` under deployment `cfg`.
#[allow(clippy::too_many_arguments)]
pub fn claim_msg(
    cfg: [u8; 32],
    token: EthAddress,
    k: [u8; 32],
    idx: usize,
    leaves: &[[u8; 32]],
    denom_index: u32,
    denom: u128,
    recipient_opening: Option<[u8; 32]>,
) -> Message {
    instr(Instruction::Claim {
        eth_token: token,
        denom_index,
        denom,
        root: root_of(leaves),
        leaf_index: idx as u64,
        path: merkle_path(leaves, idx),
        secret: k,
        cfg_id: cfg,
        recipient_opening,
    })
}

/// What the claimer's actor refuses in `i` for `signer`, diagnosed natively as the CLI does
/// (`check_deposit_in_root`, then `check_attested_root`): `None` if the witness proves a
/// deposit under its root (and the vault proof holds). On chain every fault is `CLAIM_REFUSED`.
pub fn witness_fault(signer: AccountId, i: &Instruction) -> Option<ClaimFault> {
    match i {
        Instruction::Claim { eth_token, denom, root, leaf_index, path, secret, cfg_id, .. } => {
            check_deposit_in_root(secret, &signer, eth_token, *denom, cfg_id, root, *leaf_index, path).err()
        }
        Instruction::ClaimAttested {
            eth_token,
            denom,
            root,
            leaf_index,
            path,
            secret,
            cfg_id,
            source,
            vault,
            evm_chain_id,
            anchor,
            proof,
            ..
        } => check_deposit_in_root(secret, &signer, eth_token, *denom, cfg_id, root, *leaf_index, path)
            .err()
            .or_else(|| check_attested_root(source, vault, *evm_chain_id, root, anchor, proof).err()),
        _ => None,
    }
}

/// [`witness_fault`] of a [`Message::Instruction`].
pub fn message_fault(signer: AccountId, m: &Message) -> Option<ClaimFault> {
    match m {
        Message::Instruction(i) => witness_fault(signer, i),
        Message::Effect(_) => None,
    }
}

/// The [`InclusionClass`](eth_attest_core::inclusion::InclusionClass) of an attested claim's
/// native fault, if it is an inclusion fault.
pub fn inclusion_class(signer: AccountId, i: &Instruction) -> Option<eth_attest_core::inclusion::InclusionClass> {
    match witness_fault(signer, i) {
        Some(ClaimFault::Inclusion(e)) => Some(e.class()),
        _ => None,
    }
}

/// Lowercase hex.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
