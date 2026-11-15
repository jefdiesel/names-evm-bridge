//! Simulator and fixtures shared by the attestation program's integration tests (moved here from
//! `instance.rs`, unchanged in behaviour).
//!
//! A small simulator delivers sync messages depth-first inside one transaction (any panic undoes
//! the whole transaction), as in `bridge/tests/common`. The reference consumer `eth_attest_probe`
//! stands for any program that consumes an attestation: it runs the inclusion library in its own
//! actor and sends `AssertAnchor`. It runs at two program ids ([`PROBE`], [`PROBE2`]) so two
//! independent consumers can share one world.

#![allow(dead_code, unused_imports)]

pub use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use borsh::BorshSerialize;
pub use eth_attest_core::{
    Anchor, AnchorRecord, Effect, Fork, InstanceConfig, Instruction, LcSummary, LcUpdate, MIN_SUPERMAJORITY, Message,
    anchor_address, committee_hash, config_address, instance_id,
    inclusion::{
        AccountField, AccountFields, AccountProof, AnchorWitness, InclusionError, Log, Predicate, ReceiptProof,
        StatementContext, StorageProof, StorageStatement, mapping_slot, verify_receipt_log, verify_storage_slot, word,
    },
    store_address,
    testkit::{SynthAccount, SynthState, header_rlp, receipt, receipts_trie},
};
pub use eth_attest_probe::{Probe, handle as probe};
pub use helios_consensus_core::{
    consensus_spec::MainnetConsensusSpec as S,
    types::{Bootstrap, GenericUpdate, Update},
};
pub use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::{ProgramEvent, ReceiveInput, Response},
};
pub use serde_json::Value;
pub use ssz::{Decode, Encode};

pub const ATTEST: AccountId = AccountId::new([0xE7; 32]);
pub const PROBE: AccountId = AccountId::new([0x9B; 32]);
/// A second consumer program (the same probe code at another program id).
pub const PROBE2: AccountId = AccountId::new([0x9C; 32]);
pub const ALICE: AccountId = AccountId::new([0xA1; 32]);
pub const BOB: AccountId = AccountId::new([0xB1; 32]);
pub const CAROL: AccountId = AccountId::new([0xC1; 32]);

// ---------------------------------------------------------------------------------------------
// Simulator

#[derive(Clone, Default)]
pub struct World {
    pub states: BTreeMap<(AccountId, AccountId), Vec<u8>>,
    pub events: Vec<ProgramEvent>,
    /// Every handler response of every delivery so far, borsh-encoded, with the receiving actor
    /// (for byte-for-byte determinism checks).
    pub responses: Vec<(Actor, Vec<u8>)>,
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

    pub fn deliver(&mut self, from: Option<Actor>, to: Actor, message: Vec<u8>, signer: Option<AccountId>) -> Result<(), String> {
        let input = ReceiveInput {
            receiver: to,
            from,
            is_authorized: signer == Some(to.account_id),
            pre_state: ActorState::from(self.state(to.account_id, to.program_account_id)),
            message,
        };
        let handler: fn(&ReceiveInput) -> Response = if to.program_account_id == ATTEST {
            eth_attest::handle
        } else if to.program_account_id == PROBE || to.program_account_id == PROBE2 {
            probe
        } else {
            return Err(format!("no program at {:?}", to.program_account_id));
        };
        let response = catch_unwind(AssertUnwindSafe(|| handler(&input))).map_err(panic_text)?;
        self.responses.push((to, borsh::to_vec(&response).unwrap()));
        if let Some(post) = response.post_state {
            self.states.insert((to.account_id, to.program_account_id), post.into_inner());
        }
        self.events.extend(response.events);
        for m in response.sync_messages {
            // LEZ admission (`execution_state.rs` `admit`): a public account the transaction
            // reaches for the first time must be seeded by the sending program (no test here
            // attaches evidence). Checked for this program's effects, which create every PDA.
            let is_effect = m.to.program_account_id == ATTEST
                && matches!(borsh::from_slice::<Message>(&m.message), Ok(Message::Effect(_)));
            let exists = self.states.keys().any(|(account, _)| *account == m.to.account_id);
            let seeded = m
                .pda_seeds
                .iter()
                .any(|seed| AccountId::for_public_pda(&to.program_account_id, seed) == m.to.account_id);
            if is_effect && !exists && !seeded {
                return Err(format!("unadmitted public account {:?}", m.to.account_id));
            }
            self.deliver(Some(to), m.to, m.message, signer)?;
        }
        assert!(response.async_messages.is_empty(), "no async messages here");
        Ok(())
    }

    /// One transaction: `signer` sends `message` to its own actor of `program`; on failure
    /// nothing changes.
    pub fn tx<M: BorshSerialize>(&mut self, signer: AccountId, program: AccountId, message: &M) -> Result<(), String> {
        let snapshot = self.clone();
        let r = self.deliver(None, Actor::new(signer, program), borsh::to_vec(message).unwrap(), Some(signer));
        if r.is_err() {
            *self = snapshot;
        }
        r
    }

    pub fn store(&self, id: &[u8; 32]) -> LcSummary {
        borsh::from_slice(&self.state(store_address(&ATTEST, id), ATTEST)).expect("store exists")
    }

    pub fn anchor(&self, id: &[u8; 32], anchor: &Anchor) -> Option<AnchorRecord> {
        let s = self.state(anchor_address(&ATTEST, id, anchor), ATTEST);
        (!s.is_empty()).then(|| borsh::from_slice(&s).unwrap())
    }

    /// Every state of the attestation program (config, store and anchor PDAs of all instances).
    pub fn attest_states(&self) -> BTreeMap<(AccountId, AccountId), Vec<u8>> {
        self.states.iter().filter(|((_, p), _)| *p == ATTEST).map(|(k, v)| (*k, v.clone())).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures

pub fn fx(net: &str, f: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../fixtures/{net}/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{net}/{f}: {e}"))
}

pub fn json(net: &str, f: &str) -> Value {
    serde_json::from_slice(&fx(net, f)).unwrap()
}

pub fn hx(s: &str) -> Vec<u8> {
    hex::decode(s.trim().trim_start_matches("0x")).unwrap()
}

/// Left-padded 32 bytes from a hex quantity or hash.
pub fn h32(s: &str) -> [u8; 32] {
    let t = s.trim_start_matches("0x");
    let t = if t.len() % 2 == 1 { format!("0{t}") } else { t.to_string() };
    let v = hex::decode(t).unwrap();
    let mut o = [0u8; 32];
    o[32 - v.len()..].copy_from_slice(&v);
    o
}

pub fn h20(s: &str) -> [u8; 20] {
    hx(s).try_into().unwrap()
}

/// The instance configuration for a fixture network, with the checkpoint of `boot`.
pub fn config(net: &str, boot: &str) -> InstanceConfig {
    let meta = json(net, "meta.json");
    let sched = json(net, "fork_schedule.json");
    let sched = sched["data"].as_array().unwrap();
    let forks: [Fork; 8] = core::array::from_fn(|i| {
        let v = hx(sched[i]["current_version"].as_str().unwrap());
        Fork {
            epoch: sched[i]["epoch"].as_str().unwrap().parse().unwrap(),
            version: [v[0], v[1], v[2], v[3]],
        }
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

/// What a submitter keeps: committee SSZ by commitment (from bootstraps and period updates, all
/// public beacon-node data).
#[derive(Default, Clone)]
pub struct Committees(pub HashMap<[u8; 32], Vec<u8>>);

impl Committees {
    pub fn add(&mut self, ssz: Vec<u8>) {
        self.0.insert(committee_hash(&ssz), ssz);
    }

    pub fn from_bootstrap(&mut self, b: &[u8]) {
        self.add(Bootstrap::<S>::from_ssz_bytes(b).unwrap().current_sync_committee().as_ssz_bytes());
    }

    pub fn from_period_update(&mut self, u: &[u8]) {
        let u = Update::<S>::from_ssz_bytes(u).unwrap();
        self.add(GenericUpdate::from(&u).next_sync_committee.unwrap().as_ssz_bytes());
    }

    pub fn witness(&self, s: &LcSummary) -> (Vec<u8>, Option<Vec<u8>>) {
        (
            self.0[&s.current_committee].clone(),
            s.next_committee.map(|h| self.0[&h].clone()),
        )
    }
}

pub struct Net {
    pub cfg: InstanceConfig,
    pub id: [u8; 32],
    pub committees: Committees,
}

pub fn init(w: &mut World, who: AccountId, net: &str, boot: &str, cfg: InstanceConfig) -> Result<Net, String> {
    let bootstrap = fx(net, &format!("{boot}.ssz"));
    let id = instance_id(&ATTEST, &cfg);
    w.tx(who, ATTEST, &Message::Instruction(Instruction::Init { config: cfg.clone(), bootstrap: bootstrap.clone() }))?;
    let mut committees = Committees::default();
    committees.from_bootstrap(&bootstrap);
    Ok(Net { cfg, id, committees })
}

pub fn update_msg(w: &World, n: &Net, update: LcUpdate) -> Message {
    let prev = w.store(&n.id);
    let (current_committee, next_committee) = n.committees.witness(&prev);
    Message::Instruction(Instruction::Update { config: n.cfg.clone(), prev, current_committee, next_committee, update })
}

pub fn submit(w: &mut World, who: AccountId, n: &Net, update: LcUpdate) -> Result<(), String> {
    let m = update_msg(w, n, update);
    w.tx(who, ATTEST, &m)
}

pub fn sepolia(w: &mut World) -> Net {
    init(w, ALICE, "sepolia", "bootstrap_same_period", config("sepolia", "bootstrap_same_period")).unwrap()
}

pub fn sepolia_finalized(w: &mut World) -> Net {
    let n = sepolia(w);
    submit(w, BOB, &n, LcUpdate::Finality(fx("sepolia", "finality_update.ssz"))).unwrap();
    n
}

pub fn meta(net: &str) -> Value {
    json(net, "meta.json")
}

/// The real Sepolia mapping entry (OP L1CrossDomainMessenger successfulMessages[h] = true).
pub struct MappingFixture {
    pub witness: AnchorWitness,
    pub address: [u8; 20],
    pub slot: [u8; 32],
    pub proof: StorageProof,
}

pub fn sepolia_mapping() -> MappingFixture {
    let st = &meta("sepolia")["storage"];
    let pf = json("sepolia", "eth_getProof.json");
    let sp = &pf["storageProof"][0];
    let nodes = |v: &Value| v.as_array().unwrap().iter().map(|x| hx(x.as_str().unwrap())).collect::<Vec<_>>();
    let slot = mapping_slot(&h32(st["mapping_key"].as_str().unwrap()), st["mapping_slot_index"].as_u64().unwrap());
    assert_eq!(slot, h32(sp["key"].as_str().unwrap()), "slot derivation matches the key the proof was fetched for");
    let header = hx(std::str::from_utf8(&fx("sepolia", "el_block_header.rlp.hex")).unwrap());
    MappingFixture {
        witness: AnchorWitness { anchor: Anchor::BlockHash(h32(st["block_hash"].as_str().unwrap())), header_rlp: Some(header) },
        address: h20(pf["address"].as_str().unwrap()),
        slot,
        proof: StorageProof {
            account: AccountProof {
                fields: AccountFields {
                    nonce: u64::from_str_radix(pf["nonce"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap(),
                    balance: h32(pf["balance"].as_str().unwrap()),
                    storage_root: h32(pf["storageHash"].as_str().unwrap()),
                    code_hash: h32(pf["codeHash"].as_str().unwrap()),
                },
                nodes: nodes(&pf["accountProof"]),
            },
            value: h32(sp["value"].as_str().unwrap()),
            nodes: nodes(&sp["proof"]),
        },
    }
}

pub fn ctx(n: &Net) -> StatementContext {
    StatementContext { evm_chain_id: n.cfg.evm_chain_id, instance_id: n.id }
}

pub fn probe_storage(w: &mut World, ctx: StatementContext, m: &MappingFixture, predicate: Predicate) -> Result<(), String> {
    w.tx(
        CAROL,
        PROBE,
        &Probe::Storage {
            attest: ATTEST,
            ctx,
            witness: m.witness.clone(),
            address: m.address,
            slot: m.slot,
            predicate,
            proof: m.proof.clone(),
        },
    )
}

pub fn err_contains(r: Result<(), String>, want: &str) {
    let e = r.expect_err("expected a refusal");
    assert!(e.contains(want), "wanted '{want}', got: {e}");
}
