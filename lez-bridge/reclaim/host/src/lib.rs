// SPDX-License-Identifier: MIT OR Apache-2.0
//! G4 host, EXECUTOR ONLY: builds a reclaim's guest input from what a depositor has (the deposit
//! master secret, re-derived from the wallet seed) and what is public (the vault's deposit leaves,
//! the final G2 state the outbound host saved, with the accumulator's leaves), runs the guest in
//! RISC Zero's in-process executor and checks it against the native statement, and writes the
//! cross-language fixture `vault/test/fixtures/ReclaimVector.sol`. Nothing here proves.

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

use anyhow::{Context, Result, anyhow, ensure};
use borsh::{BorshDeserialize, BorshSerialize};
use bridge_core::EthAddress;
use claim_acc::{AccState, Leaf, Tree};
use outbound_core::{finality::BlockRef, state::G2State, view::BridgeView};
use reclaim_core::{CfgPreimage, ReclaimInput, ReclaimJournal, sha256, u256};
use risc0_zkvm::{
    ExecutorEnv, ExecutorImpl, ExitCode, MaybePruned, ReceiptClaim,
    sha::{Digest, Digestible},
};
use serde_json::json;

/// The G4 guest's image id as the vault's `bytes32 reclaimImageId`.
#[must_use]
pub fn image_id() -> [u8; 32] {
    let d = Digest::from(RECLAIM_GUEST_ID);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// What `outbound_host` saves after an update (`next-state.bin`): the G2 state preimage, the
/// bridge view and the accumulator's leaves in index order. Same borsh layout as
/// `outbound_host::HostState` (checked in the tests against the outbound fixtures).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HostState {
    pub g2: G2State,
    pub view: BridgeView,
    pub leaves: Vec<Leaf>,
}

impl HostState {
    pub fn load(p: &std::path::Path) -> Result<Self> {
        Ok(borsh::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)?)
    }

    /// The accumulator rebuilt from the leaves; refuses leaves that don't give the state's root.
    pub fn tree(&self) -> Result<Tree> {
        let t = Tree::from_leaves(&self.leaves).map_err(|e| anyhow!("accumulator leaves: {e}"))?;
        ensure!(t.state() == self.g2.claim_acc, "the accumulator leaves do not give the state's claim_acc");
        Ok(t)
    }
}

/// The seal the vault's test `MockVerifier` accepts: `abi.encodePacked(imageId, journalDigest)`.
#[must_use]
pub fn mock_seal(image: &[u8; 32], journal: &[u8]) -> Vec<u8> {
    [image.as_slice(), &sha256(&[journal])].concat()
}

/// Why no input can be built.
#[derive(Debug, PartialEq, Eq)]
pub enum BuildError {
    /// The deposit's leaf is not among the vault's leaves (wrong secret, account, token or
    /// denomination, or never deposited).
    NotDeposited,
    /// The deposit's claim nullifier is in the final accumulator: it was claimed on LEZ.
    Claimed,
}

/// Builds the guest input for one deposit. `leaves` are the vault's deposit leaves in order; the
/// proof is made against the root of all of them (any root the vault reached is accepted).
#[allow(clippy::too_many_arguments)]
pub fn build_input(
    master: [u8; 32],
    dest: [u8; 32],
    token: EthAddress,
    denom: u128,
    cfg: CfgPreimage,
    leaves: &[[u8; 32]],
    state: &G2State,
    tree: &Tree,
    recipient: EthAddress,
    relayer: EthAddress,
    relay_fee: u128,
) -> std::result::Result<ReclaimInput, BuildError> {
    let cfg_id = cfg.cfg_id();
    let k = reclaim_core::claim_secret(&master);
    let c = bridge_core::deposit_commitment(&k, &lee_core::account::AccountId::new(dest), &token, denom, &cfg_id);
    let leaf = bridge_core::deposit_leaf(&cfg_id, &token, denom, &c);
    let index = leaves.iter().position(|l| *l == leaf).ok_or(BuildError::NotDeposited)?;
    let key = reclaim_core::acc_key(&cfg.program_id, &k, &dest, &token, denom);
    let non_membership = tree.non_membership(&key).ok_or(BuildError::Claimed)?;
    Ok(ReclaimInput {
        master,
        dest,
        token,
        denom,
        cfg,
        leaf_index: index as u64,
        path: bridge_core::merkle_path(leaves, index),
        state: state.clone(),
        non_membership,
        recipient,
        relayer,
        relay_fee,
    })
}

pub struct Run {
    pub journal: Vec<u8>,
    pub decoded: ReclaimJournal,
    pub user_cycles: u64,
    pub total_cycles: u64,
    pub segments: usize,
}

/// Executes the embedded G4 guest (never proves).
pub fn execute(input: &ReclaimInput) -> Result<Run> {
    execute_elf(RECLAIM_GUEST_ELF, image_id(), input)
}

/// Executes a G4 guest ELF; checks its exit, that its claim commits to `sha256(journal)` (what
/// risc0-ethereum's `verify(seal, imageId, journalDigest)` rebuilds) and that it equals the
/// native statement.
pub fn execute_elf(elf: &[u8], image: [u8; 32], input: &ReclaimInput) -> Result<Run> {
    let env = ExecutorEnv::builder().write_slice(&reclaim_core::encode_input(input)).build()?;
    let s = ExecutorImpl::from_elf(env, elf)?.run()?;
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&[&journal]))));
    ensure!(s.claim()?.digest() == want.digest(), "reclaim claim does not commit to sha256(journal)");
    let decoded = ReclaimJournal::abi_decode(&journal).ok_or_else(|| anyhow!("reclaim journal does not decode"))?;
    ensure!(decoded == reclaim_core::reclaim(input).map_err(|e| anyhow!("{e}"))?, "reclaim guest differs from native");
    Ok(Run { journal, decoded, user_cycles: s.user_cycles, total_cycles: s.total_cycles, segments: s.segments.len() })
}

/// Executes the guest on an input it must refuse; returns the panic text if it did (an error).
pub fn execute_refused(input: &ReclaimInput) -> Option<String> {
    let env = ExecutorEnv::builder().write_slice(&reclaim_core::encode_input(input)).build().ok()?;
    match ExecutorImpl::from_elf(env, RECLAIM_GUEST_ELF).and_then(|mut e| e.run()) {
        Ok(s) if s.exit_code == ExitCode::Halted(0) => None,
        Ok(s) => Some(format!("exit {:?}", s.exit_code)),
        Err(e) => Some(format!("{e:#}")),
    }
}

// ------------------------------------------------------------------------------- the scenario

// cfgId inputs: the ones `vault/test/Vectors.t.sol` and the G3 fixture pin, so Foundry deploys
// the failsafe vault with `deployCodeTo` at the same address on the same chain id.
pub const CHAIN_ID: u64 = 31337;
pub const VAULT: EthAddress = addr_lo(0xBB);
pub const LEZ_CHAIN_ID: [u8; 32] = word_lo(0x11);
pub const ZONE_ID: [u8; 32] = word_lo(0x22);
pub const PROGRAM_ID: [u8; 32] = word_lo(0x33);
pub const USDC: EthAddress = addr_lo(0xAA);
/// USDC denomination index 0 (100 USDC).
pub const DENOM: u128 = 100_000_000;
/// Deposits in the scenario; deposit [`CLAIMED`] is claimed on LEZ before the final state.
pub const DEPOSITS: u64 = 3;
pub const CLAIMED: usize = 1;

const fn addr_lo(b: u8) -> EthAddress {
    let mut a = [0u8; 20];
    a[19] = b;
    a
}

const fn word_lo(b: u8) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[31] = b;
    w
}

#[must_use]
pub fn tag20(tag: &str, i: u32) -> EthAddress {
    let h = sha256(&[tag.as_bytes(), &i.to_be_bytes()]);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h[..20]);
    a
}

#[must_use]
pub fn cfg() -> CfgPreimage {
    CfgPreimage { chain_id: CHAIN_ID, vault: VAULT, lez_chain_id: LEZ_CHAIN_ID, zone_id: ZONE_ID, program_id: PROGRAM_ID }
}

/// Three seed-derived deposits of 100 USDC for one LEZ account; deposit 1 claimed on LEZ (its key
/// inserted between two foreign keys); the final G2 state over that accumulator.
pub struct Scenario {
    pub dest: [u8; 32],
    pub masters: Vec<[u8; 32]>,
    pub commitments: Vec<[u8; 32]>,
    pub leaves: Vec<[u8; 32]>,
    pub tree: Tree,
    pub state: G2State,
}

#[must_use]
pub fn scenario() -> Scenario {
    let c = cfg();
    let cfg_id = c.cfg_id();
    let seed_secret = sha256(&[b"RFP022/g4-vector/account-secret"]);
    let root = bridge_client::kdf::AccountRoot::from_account_secret(&seed_secret);
    let dest = sha256(&[b"RFP022/g4-vector/lez-account"]);
    let acct = lee_core::account::AccountId::new(dest);
    let masters: Vec<[u8; 32]> =
        (0..DEPOSITS).map(|i| bridge_client::kdf::deposit_master(&root, &cfg_id, &acct, &USDC, DENOM, i)).collect();
    let commitments: Vec<[u8; 32]> = masters
        .iter()
        .map(|m| bridge_core::deposit_commitment(&reclaim_core::claim_secret(m), &acct, &USDC, DENOM, &cfg_id))
        .collect();
    let leaves = commitments.iter().map(|cm| bridge_core::deposit_leaf(&cfg_id, &USDC, DENOM, cm)).collect();
    let mut tree = Tree::new();
    let other = |i: u8| sha256(&[b"RFP022/g4-vector/other-claim", &[i]]);
    tree.insert(&other(0)).expect("insert");
    let claimed_k = reclaim_core::claim_secret(&masters[CLAIMED]);
    tree.insert(&reclaim_core::acc_key(&PROGRAM_ID, &claimed_k, &dest, &USDC, DENOM)).expect("insert");
    tree.insert(&other(1)).expect("insert");
    let state = G2State {
        finality_kind: 1,
        l1_state: [0; 32],
        bridge_program: PROGRAM_ID,
        tip: BlockRef { id: 4242, hash: sha256(&[b"RFP022/g4-vector/tip"]) },
        claim_acc: tree.state(),
        view_hash: BridgeView::default().hash(),
        exit_count: 0,
        wound_down: Vec::new(),
    };
    Scenario { dest, masters, commitments, leaves, tree, state }
}

pub fn recipient(i: u32) -> EthAddress {
    tag20("RFP022/g4-vector/recipient", i)
}

pub fn relayer() -> EthAddress {
    tag20("RFP022/g4-vector/relayer", 0)
}

/// The scenario's input for deposit `i`.
pub fn input_for(s: &Scenario, i: usize, recipient: EthAddress, relayer: EthAddress, fee: u128) -> std::result::Result<ReclaimInput, BuildError> {
    build_input(s.masters[i], s.dest, USDC, DENOM, cfg(), &s.leaves, &s.state, &s.tree, recipient, relayer, fee)
}

fn h(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn sol_addr(a: &EthAddress) -> String {
    format!("address(bytes20(hex\"{}\"))", hex::encode(a))
}

pub struct Fixture {
    pub json: String,
    pub sol: String,
    pub cycles: Vec<(String, Run)>,
}

/// The cross-language fixture: the scenario's public values and the guest's journals for the two
/// unclaimed deposits (deposit 0 to recipient 0 with no fee, deposit 2 to recipient 2 through
/// the relayer for `fee`).
pub fn fixture(fee: u128) -> Result<Fixture> {
    let s = scenario();
    let mut runs = Vec::new();
    for (i, f) in [(0usize, 0u128), (2, fee)] {
        let rel = if f == 0 { [0u8; 20] } else { relayer() };
        let input = input_for(&s, i, recipient(i as u32), rel, f).map_err(|e| anyhow!("deposit {i}: {e:?}"))?;
        runs.push((format!("deposit{i}"), execute(&input)?));
    }
    let cfg_id = cfg().cfg_id();
    let final_state = s.state.commitment();
    let rv: Vec<serde_json::Value> = runs
        .iter()
        .map(|(l, r)| {
            let j = &r.decoded;
            json!({"label": l, "depositRoot": h(&j.deposit_root), "nullifier": h(&j.nullifier), "recipient": h(&j.recipient),
                   "relayer": h(&j.relayer), "relayFee": u128::from_be_bytes(j.relay_fee[16..].try_into().unwrap_or([0; 16])).to_string(),
                   "journal": h(&r.journal), "journalDigest": h(&j.digest()), "userCycles": r.user_cycles})
        })
        .collect();
    let json = serde_json::to_string_pretty(&json!({
        "generator": "lez-bridge/reclaim/host (reclaim_host vector --write)",
        "chainId": CHAIN_ID, "vault": h(&VAULT), "lezChainId": h(&LEZ_CHAIN_ID), "zoneId": h(&ZONE_ID), "programId": h(&PROGRAM_ID),
        "cfgId": h(&cfg_id), "usdc": h(&USDC), "denom": DENOM.to_string(),
        "commitments": s.commitments.iter().map(|c| h(c)).collect::<Vec<_>>(),
        "claimed": CLAIMED, "claimAccRoot": h(&s.state.claim_acc.root), "claimAccCount": s.state.claim_acc.count,
        "finalState": h(&final_state), "reclaims": rv,
    }))?;
    let mut sol = String::new();
    sol.push_str("// SPDX-License-Identifier: MIT OR Apache-2.0\n");
    sol.push_str("// GENERATED by lez-bridge/reclaim/host (`reclaim_host vector --write`). Do not edit by hand.\n");
    sol.push_str("// Journals were produced by the G4 guest in RISC Zero's executor; see reclaim_vector.json.\n");
    sol.push_str("pragma solidity ^0.8.24;\n\nlibrary ReclaimVector {\n");
    sol.push_str("    struct ReclaimV {\n        bytes32 depositRoot;\n        bytes32 nullifier;\n        address recipient;\n        address relayer;\n        uint256 relayFee;\n        bytes journal;\n        bytes32 journalDigest;\n    }\n\n");
    sol.push_str(&format!("    uint256 internal constant CHAIN_ID = {CHAIN_ID};\n"));
    sol.push_str(&format!("    address internal constant VAULT = {};\n", sol_addr(&VAULT)));
    sol.push_str(&format!("    bytes32 internal constant LEZ_CHAIN_ID = {};\n", h(&LEZ_CHAIN_ID)));
    sol.push_str(&format!("    bytes32 internal constant ZONE_ID = {};\n", h(&ZONE_ID)));
    sol.push_str(&format!("    bytes32 internal constant PROGRAM_ID = {};\n", h(&PROGRAM_ID)));
    sol.push_str(&format!("    bytes32 internal constant CFG_ID = {};\n", h(&cfg_id)));
    sol.push_str(&format!("    address internal constant USDC = {};\n", sol_addr(&USDC)));
    sol.push_str(&format!("    uint256 internal constant DENOM = {DENOM};\n"));
    sol.push_str(&format!("    uint256 internal constant CLAIMED = {CLAIMED};\n"));
    // No image id here: a guest build embeds its directory, so the id differs between checkouts;
    // the journals do not depend on it (the Foundry test uses its own constant).
    sol.push_str(&format!("    bytes32 internal constant FINAL_STATE = {};\n\n", h(&final_state)));
    sol.push_str(&format!("    function commitments() internal pure returns (bytes32[] memory c) {{\n        c = new bytes32[]({});\n", s.commitments.len()));
    for (i, c) in s.commitments.iter().enumerate() {
        sol.push_str(&format!("        c[{i}] = {};\n", h(c)));
    }
    sol.push_str("    }\n\n");
    sol.push_str(&format!("    function reclaims() internal pure returns (ReclaimV[] memory r) {{\n        r = new ReclaimV[]({});\n", runs.len()));
    for (i, (_, r)) in runs.iter().enumerate() {
        let j = &r.decoded;
        let fee = u128::from_be_bytes(j.relay_fee[16..].try_into().unwrap_or([0; 16]));
        sol.push_str(&format!(
            "        r[{i}] = ReclaimV({}, {}, {}, {}, {fee}, hex\"{}\", {});\n",
            h(&j.deposit_root),
            h(&j.nullifier),
            sol_addr(&j.recipient),
            sol_addr(&j.relayer),
            hex::encode(&r.journal),
            h(&j.digest())
        ));
    }
    sol.push_str("    }\n}\n");
    Ok(Fixture { json, sol, cycles: runs })
}

/// `AccState` of an empty accumulator (for tests).
#[must_use]
pub fn empty_acc() -> AccState {
    AccState::empty()
}

/// `u256(v)` re-exported for the CLI.
#[must_use]
pub fn word(v: u128) -> [u8; 32] {
    u256(v)
}
