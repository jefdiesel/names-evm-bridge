//! G3 host, EXECUTOR ONLY: the test scenario (a vault exit tree built batch by batch, as
//! `BridgeVault.update` builds it), a guest runner on RISC Zero's in-process executor, and the
//! writer of the cross-language fixture that `vault/test/ReleaseVector.t.sol` replays.
//! Nothing here proves.

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

use anyhow::{Context, Result, anyhow, ensure};
use release_core::{
    EthAddress, ReleaseInput, ReleaseJournal, ReleasePublic, ReleaseWitness, TREE_DEPTH, U256, exit_commitment, exit_leaf,
    sha256, u256,
};
use risc0_zkvm::{
    ExecutorEnv, ExecutorImpl, ExitCode, MaybePruned, ReceiptClaim,
    sha::{Digest, Digestible},
};
use serde_json::{Value, json};

// cfgId inputs: the ones `vault/test/Vectors.t.sol` pins, so the Foundry side deploys the vault
// with `deployCodeTo` at the same address on the same chain id and gets the same cfgId.
pub const CHAIN_ID: u64 = 31337;
pub const VAULT: EthAddress = addr_lo(0xBB);
pub const LEZ_CHAIN_ID: [u8; 32] = word_lo(0x11);
pub const ZONE_ID: [u8; 32] = word_lo(0x22);
pub const PROGRAM_ID: [u8; 32] = word_lo(0x33);
/// `Vectors.t.sol`: `assertEq(v.cfgId(), 0x5da9...cfbc)`.
pub const CFG_ID_PINNED: &str = "5da920fc38f4f4750c2ed8cc5eb8771f3547ecb4b99a8b7eefbf767731b5cfbc";
/// USDC stand-in: `address(0xAA)`, etched with `TestToken` (6 decimals) as in `Vectors.t.sol`.
pub const USDC: EthAddress = addr_lo(0xAA);
pub const ETH: EthAddress = [0; 20];
/// Exits per `update()`: ends at 5, 16, 17, 20 leaves. The second batch fills the tree to
/// exactly 2^4 leaves, the third crosses into the next height-4 subtree.
pub const BATCHES: [usize; 4] = [5, 11, 1, 3];

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

fn tag20(tag: &str, i: u32) -> EthAddress {
    let h = sha256(&[tag.as_bytes(), &i.to_be_bytes()]);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h[..20]);
    a
}

pub fn relayer_a() -> EthAddress {
    tag20("RFP022/g3-vector/relayer", 0)
}

pub fn relayer_b() -> EthAddress {
    tag20("RFP022/g3-vector/relayer", 1)
}

#[derive(Clone, Debug)]
pub struct TokenSpec {
    pub name: &'static str,
    pub addr: EthAddress,
    pub decimals: u8,
    pub denoms: Vec<u128>,
    pub withdraw_fees: Vec<u128>,
}

pub fn tokens() -> Vec<TokenSpec> {
    vec![
        TokenSpec {
            name: "USDC",
            addr: USDC,
            decimals: 6,
            denoms: vec![100_000_000, 1_000_000_000, 10_000_000_000],
            withdraw_fees: vec![1_000_000, 10_000_000, 100_000_000],
        },
        TokenSpec {
            name: "ETH",
            addr: ETH,
            decimals: 18,
            denoms: vec![100_000_000_000_000_000, 1_000_000_000_000_000_000, 10_000_000_000_000_000_000],
            withdraw_fees: vec![1_000_000_000_000_000, 10_000_000_000_000_000, 100_000_000_000_000_000],
        },
    ]
}

#[derive(Clone, Debug)]
pub struct ExitSpec {
    pub token: EthAddress,
    pub denom_index: u32,
    pub denom: u128,
    pub secret: [u8; 32],
    pub eth_dest: EthAddress,
    pub max_relay_fee: u128,
    pub commitment: [u8; 32],
    pub leaf: [u8; 32],
}

/// The vault's `Sha256IncrementalTree.insert`, line for line (deposit-contract algorithm), as an
/// independent root computation next to the full-layer `merkle_path`/`merkle_root`.
pub struct IncTree {
    filled: [[u8; 32]; TREE_DEPTH],
    next: u64,
    pub root: [u8; 32],
}

impl Default for IncTree {
    fn default() -> Self {
        Self { filled: [[0; 32]; TREE_DEPTH], next: 0, root: bridge_core::zero_hashes()[TREE_DEPTH] }
    }
}

impl IncTree {
    pub fn insert(&mut self, leaf: [u8; 32]) {
        let zeros = bridge_core::zero_hashes();
        let index = self.next;
        let mut size = index + 1;
        let mut node = leaf;
        for h in 0..TREE_DEPTH {
            if size & 1 == 1 {
                self.filled[h] = node;
                break;
            }
            node = sha256(&[&self.filled[h], &node]);
            size >>= 1;
        }
        size = index + 1;
        node = [0; 32];
        for (h, zero) in zeros.iter().enumerate().take(TREE_DEPTH) {
            node = if size & 1 == 1 { sha256(&[&self.filled[h], &node]) } else { sha256(&[&node, zero]) };
            size >>= 1;
        }
        self.root = node;
        self.next = index + 1;
    }
}

pub struct Scenario {
    pub cfg_id: [u8; 32],
    pub tokens: Vec<TokenSpec>,
    pub exits: Vec<ExitSpec>,
    /// Root after each batch: the only exit roots the vault records (`exitRootTime`).
    pub roots: Vec<[u8; 32]>,
}

impl Scenario {
    pub fn build() -> Self {
        let cfg_id = release_core::cfg_id(CHAIN_ID, &VAULT, &LEZ_CHAIN_ID, &ZONE_ID, &PROGRAM_ID);
        let tokens = tokens();
        let n: usize = BATCHES.iter().sum();
        let exits: Vec<ExitSpec> = (0..n as u32)
            .map(|i| {
                let t = &tokens[(i % 2) as usize];
                let denom_index = (i / 2) % 3;
                let denom = t.denoms[denom_index as usize];
                // fee ceilings 0, 1%, 0.5%, 2% of the denomination
                let max_relay_fee = match i % 4 {
                    0 => 0,
                    1 => denom / 100,
                    2 => denom / 200,
                    _ => denom / 50,
                };
                let secret = sha256(&[b"RFP022/g3-vector/k", &i.to_be_bytes()]);
                let eth_dest = tag20("RFP022/g3-vector/dest", i);
                let commitment = exit_commitment(&secret, &eth_dest, &t.addr, &u256(denom), &u256(max_relay_fee), &cfg_id);
                let leaf = exit_leaf(&cfg_id, &t.addr, &u256(denom), &commitment);
                ExitSpec { token: t.addr, denom_index, denom, secret, eth_dest, max_relay_fee, commitment, leaf }
            })
            .collect();
        let mut tree = IncTree::default();
        let mut roots = Vec::new();
        let mut at = 0;
        for size in BATCHES {
            for e in &exits[at..at + size] {
                tree.insert(e.leaf);
            }
            at += size;
            roots.push(tree.root);
        }
        Self { cfg_id, tokens, exits, roots }
    }

    /// Number of leaves in the tree after batch `b`.
    pub fn batch_end(b: usize) -> usize {
        BATCHES[..=b].iter().sum()
    }

    pub fn leaves(&self, n: usize) -> Vec<[u8; 32]> {
        self.exits[..n].iter().map(|e| e.leaf).collect()
    }

    /// The witness for exit `i` against the tree holding the first `n` leaves.
    pub fn witness_at(&self, i: usize, n: usize) -> ReleaseWitness {
        let e = &self.exits[i];
        ReleaseWitness {
            secret: e.secret,
            eth_dest: e.eth_dest,
            token: e.token,
            denom: u256(e.denom),
            max_relay_fee: u256(e.max_relay_fee),
            cfg_id: self.cfg_id,
            leaf_index: i as u64,
            path: bridge_core::merkle_path(&self.leaves(n), i),
        }
    }

    pub fn input(&self, c: &ReleaseCase) -> ReleaseInput {
        ReleaseInput {
            witness: self.witness_at(c.exit, Self::batch_end(c.batch)),
            public: ReleasePublic { relayer: c.relayer, relay_fee: u256(c.relay_fee) },
        }
    }

    pub fn token(&self, addr: &EthAddress) -> &TokenSpec {
        self.tokens.iter().find(|t| t.addr == *addr).expect("known token")
    }
}

#[derive(Clone, Debug)]
pub struct ReleaseCase {
    pub label: &'static str,
    pub exit: usize,
    /// Prove against the root recorded after this batch.
    pub batch: usize,
    pub relayer: EthAddress,
    pub relay_fee: u128,
}

/// The positive vectors: several tokens and denominations, roots old and new, both sides of the
/// 2^4 boundary, fee at the ceiling, below it, and zero (self-submission).
pub fn release_cases(s: &Scenario) -> Vec<ReleaseCase> {
    vec![
        ReleaseCase {
            label: "exit 3 (1 ETH) under the oldest root (5 leaves), fee == maxRelayFee",
            exit: 3,
            batch: 0,
            relayer: relayer_a(),
            relay_fee: s.exits[3].max_relay_fee,
        },
        ReleaseCase {
            label: "exit 15 (1 ETH), last leaf of a full 2^4 tree (16 leaves), fee < max",
            exit: 15,
            batch: 1,
            relayer: relayer_a(),
            relay_fee: s.exits[15].max_relay_fee / 2,
        },
        ReleaseCase {
            label: "exit 16 (10000 USDC), first leaf past the 2^4 boundary (17 leaves), self-submit to own address, fee 0",
            exit: 16,
            batch: 2,
            relayer: s.exits[16].eth_dest,
            relay_fee: 0,
        },
        ReleaseCase {
            label: "exit 0 (100 USDC) under the newest root (20 leaves), relayer 0, fee 0",
            exit: 0,
            batch: 3,
            relayer: [0; 20],
            relay_fee: 0,
        },
        ReleaseCase {
            label: "exit 19 (0.1 ETH), last leaf under the newest root, fee < max",
            exit: 19,
            batch: 3,
            relayer: relayer_b(),
            relay_fee: s.exits[19].max_relay_fee / 2,
        },
    ]
}

/// Tampered inputs that still execute: each gives a journal whose root the vault never recorded.
/// The base is release case 2 (exit 16 under the 17-leaf root).
pub struct BadCase {
    pub label: &'static str,
    pub input: ReleaseInput,
    /// The denomination index the submitter would pass (the journal's denom must be registered).
    pub denom_index: u32,
}

pub fn bad_cases(s: &Scenario) -> Vec<BadCase> {
    let base = s.input(&release_cases(s)[2]);
    let mut out = Vec::new();
    let mut push = |label, f: &dyn Fn(&mut ReleaseWitness), denom_index| {
        let mut input = base.clone();
        f(&mut input.witness);
        out.push(BadCase { label, input, denom_index });
    };
    push("wrong secret k", &|w| w.secret[0] ^= 1, 2);
    push("different ethDest than committed", &|w| w.eth_dest[19] ^= 1, 2);
    push("different denom than committed (1000 USDC)", &|w| w.denom = u256(1_000_000_000), 1);
    push("different cfgId than committed", &|w| w.cfg_id[0] ^= 1, 2);
    push("one path sibling changed", &|w| w.path[3][0] ^= 1, 2);
    push("wrong leaf index (17, path of 16)", &|w| w.leaf_index = 17, 2);
    out.push(BadCase {
        label: "exit 2 under a mid-batch root (3 leaves; the vault records only batch ends)",
        input: ReleaseInput { witness: s.witness_at(2, 3), public: base.public.clone() },
        denom_index: s.exits[2].denom_index,
    });
    out
}

pub struct Run {
    pub journal: Vec<u8>,
    pub user_cycles: u64,
    pub total_cycles: u64,
    pub segments: usize,
}

/// Image id of the G3 guest as the vault's `releaseImageId` (bytes32).
pub fn image_id() -> [u8; 32] {
    let d = Digest::from(RELEASE_GUEST_ID);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// Executes the guest (never proves). Requires a clean halt, and that the receipt claim a proof
/// would carry is `ReceiptClaim::ok(imageId, journalDigest)` with `journalDigest =
/// sha256(journal)`: what risc0-ethereum's `verify(seal, imageId, journalDigest)` rebuilds.
pub fn execute(input: &ReleaseInput) -> Result<Run> {
    let env = ExecutorEnv::builder().write_slice(&input.to_bytes()).build()?;
    let s = ExecutorImpl::from_elf(env, RELEASE_GUEST_ELF)?.run()?;
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let digest = Digest::from(sha256(&[&journal]));
    let want = ReceiptClaim::ok(RELEASE_GUEST_ID, MaybePruned::Pruned(digest));
    ensure!(s.claim()?.digest() == want.digest(), "receipt claim does not commit to sha256(journal)");
    Ok(Run { journal, user_cycles: s.user_cycles, total_cycles: s.total_cycles, segments: s.segments.len() })
}

/// Executes and expects the guest to fail with a message containing `want`.
pub fn execute_err(input: &ReleaseInput, want: &str) -> Result<String> {
    match execute(input) {
        Ok(r) => Err(anyhow!("guest accepted, journal {}", hex::encode(r.journal))),
        Err(e) => {
            let msg = format!("{e:#}");
            ensure!(msg.contains(want), "failed for another reason: {msg}");
            Ok(msg.lines().next().unwrap_or_default().to_string())
        }
    }
}

fn h(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

fn sol_addr(a: &EthAddress) -> String {
    format!("address(bytes20(hex\"{}\"))", hex::encode(a))
}

fn sol_b32(b: &[u8; 32]) -> String {
    format!("bytes32(hex\"{}\")", hex::encode(b))
}

/// One generated vector (positive or tampered), with the guest's journal.
pub struct Vector {
    pub label: String,
    pub input: ReleaseInput,
    pub denom_index: u32,
    pub batch: Option<usize>,
    pub journal: Vec<u8>,
    pub run: Run,
}

pub struct Fixture {
    pub json: String,
    pub sol: String,
    pub positives: Vec<Vector>,
    pub negatives: Vec<Vector>,
}

fn decoded(v: &Vector) -> Result<ReleaseJournal> {
    ReleaseJournal::abi_decode(&v.journal).ok_or_else(|| anyhow!("{}: journal is not 256 well-formed bytes", v.label))
}

/// Runs every vector in the executor (journals in the fixture come from the guest, not from the
/// native code) and renders the JSON and the Solidity library.
pub fn fixture() -> Result<Fixture> {
    let s = Scenario::build();
    ensure!(hex::encode(s.cfg_id) == CFG_ID_PINNED, "cfgId differs from Vectors.t.sol");
    let mut positives = Vec::new();
    for c in release_cases(&s) {
        let input = s.input(&c);
        let run = execute(&input).with_context(|| c.label)?;
        let native = release_core::release(&input).map_err(|e| anyhow!("{e}"))?.abi_encode();
        ensure!(run.journal == native, "{}: guest and native journals differ", c.label);
        ensure!(release_core::release(&input).unwrap().root == s.roots[c.batch], "{}: not the batch root", c.label);
        positives.push(Vector {
            label: c.label.into(),
            denom_index: s.exits[c.exit].denom_index,
            batch: Some(c.batch),
            journal: run.journal.clone(),
            input,
            run,
        });
    }
    let mut negatives = Vec::new();
    for b in bad_cases(&s) {
        let run = execute(&b.input).with_context(|| b.label)?;
        let j = ReleaseJournal::abi_decode(&run.journal).context("journal")?;
        ensure!(!s.roots.contains(&j.root), "{}: tampered journal hit a recorded root", b.label);
        negatives.push(Vector { label: b.label.into(), denom_index: b.denom_index, batch: None, journal: run.journal.clone(), input: b.input, run });
    }

    // ---- JSON
    let tok = |t: &TokenSpec| {
        json!({
            "name": t.name, "address": h(&t.addr), "decimals": t.decimals,
            "denoms": t.denoms.iter().map(u128::to_string).collect::<Vec<_>>(),
            "withdrawFees": t.withdraw_fees.iter().map(u128::to_string).collect::<Vec<_>>(),
        })
    };
    let exits: Vec<Value> = s
        .exits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "index": i, "token": h(&e.token), "denomIndex": e.denom_index, "denom": e.denom.to_string(),
                "k": h(&e.secret), "ethDest": h(&e.eth_dest), "maxRelayFee": e.max_relay_fee.to_string(),
                "E": h(&e.commitment), "leaf": h(&e.leaf),
            })
        })
        .collect();
    let vec_json = |v: &Vector| -> Result<Value> {
        let j = decoded(v)?;
        let w = &v.input.witness;
        Ok(json!({
            "label": v.label,
            "private": {
                "k": h(&w.secret), "ethDest": h(&w.eth_dest), "token": h(&w.token), "denom": h(&w.denom),
                "maxRelayFee": h(&w.max_relay_fee), "cfgId": h(&w.cfg_id), "leafIndex": w.leaf_index,
                "path": w.path.iter().map(|p| h(p)).collect::<Vec<_>>(),
            },
            "public": { "relayer": h(&v.input.public.relayer), "relayFee": h(&v.input.public.relay_fee) },
            "denomIndex": v.denom_index,
            "batch": v.batch,
            "out": {
                "cfgId": h(&j.cfg_id), "root": h(&j.root), "nullifier": h(&j.nullifier), "token": h(&j.token),
                "denom": h(&j.denom), "recipient": h(&j.recipient), "relayer": h(&j.relayer), "relayFee": h(&j.relay_fee),
            },
            "journal": h(&v.journal),
            "journalDigest": h(&sha256(&[&v.journal])),
        }))
    };
    let doc = json!({
        "about": "G3 release-proof vectors. Journals come from the guest run in RISC Zero's executor (never proved). \
                  Regenerate: release_host --write <dir> (lez-bridge/release). Replayed by vault/test/ReleaseVector.t.sol.",
        "cfg": {
            "chainId": CHAIN_ID, "vault": h(&VAULT), "lezChainId": h(&LEZ_CHAIN_ID), "zoneId": h(&ZONE_ID),
            "programId": h(&PROGRAM_ID), "cfgId": h(&s.cfg_id),
        },
        "tokens": s.tokens.iter().map(tok).collect::<Vec<_>>(),
        "batchSizes": BATCHES,
        "batchRoots": s.roots.iter().map(|r| h(r)).collect::<Vec<_>>(),
        "exits": exits,
        "releases": positives.iter().map(vec_json).collect::<Result<Vec<_>>>()?,
        "rejected": negatives.iter().map(vec_json).collect::<Result<Vec<_>>>()?,
    });
    let json = serde_json::to_string_pretty(&doc)? + "\n";

    // ---- Solidity
    let mut sol = String::new();
    let p = |sol: &mut String, l: &str| {
        sol.push_str(l);
        sol.push('\n');
    };
    p(&mut sol, "// SPDX-License-Identifier: MIT");
    p(&mut sol, "// GENERATED by lez-bridge/release/host (`release_host --write`). Do not edit by hand.");
    p(&mut sol, "// Journals were produced by the G3 guest in RISC Zero's executor; see release_vector.json.");
    p(&mut sol, "pragma solidity ^0.8.24;");
    p(&mut sol, "");
    p(&mut sol, "library ReleaseVector {");
    p(&mut sol, "    struct ExitV {\n        address token;\n        uint32 denomIndex;\n        uint256 denom;\n        bytes32 commitment;\n        bytes32 leaf;\n    }");
    p(&mut sol, "");
    p(&mut sol, "    struct ReleaseV {\n        string label;\n        bytes32 root;\n        bytes32 nullifier;\n        address token;\n        uint32 denomIndex;\n        uint256 denom;\n        address recipient;\n        address relayer;\n        uint256 relayFee;\n        bytes journal;\n        bytes32 journalDigest;\n    }");
    p(&mut sol, "");
    p(&mut sol, &format!("    uint256 internal constant CHAIN_ID = {CHAIN_ID};"));
    p(&mut sol, "    address internal constant VAULT = address(0xBB);");
    p(&mut sol, "    address internal constant USDC = address(0xAA);");
    p(&mut sol, "    bytes32 internal constant LEZ_CHAIN_ID = bytes32(uint256(0x11));");
    p(&mut sol, "    bytes32 internal constant ZONE_ID = bytes32(uint256(0x22));");
    p(&mut sol, "    bytes32 internal constant PROGRAM_ID = bytes32(uint256(0x33));");
    p(&mut sol, &format!("    bytes32 internal constant CFG_ID = 0x{};", hex::encode(s.cfg_id)));
    p(&mut sol, "");
    for t in &s.tokens {
        let arr = |name: &str, xs: &[u128]| {
            let mut f = format!("    function {}{}() internal pure returns (uint256[] memory a) {{\n        a = new uint256[]({});\n", t.name.to_lowercase(), name, xs.len());
            for (i, x) in xs.iter().enumerate() {
                f.push_str(&format!("        a[{i}] = {x};\n"));
            }
            f.push_str("    }\n");
            f
        };
        sol.push_str(&arr("Denoms", &t.denoms));
        sol.push_str(&arr("WithdrawFees", &t.withdraw_fees));
    }
    p(&mut sol, &format!("    function batchSizes() internal pure returns (uint256[] memory a) {{\n        a = new uint256[]({});", BATCHES.len()));
    for (i, b) in BATCHES.iter().enumerate() {
        p(&mut sol, &format!("        a[{i}] = {b};"));
    }
    p(&mut sol, "    }");
    p(&mut sol, &format!("    function batchRoots() internal pure returns (bytes32[] memory a) {{\n        a = new bytes32[]({});", s.roots.len()));
    for (i, r) in s.roots.iter().enumerate() {
        p(&mut sol, &format!("        a[{i}] = {};", sol_b32(r)));
    }
    p(&mut sol, "    }");
    p(&mut sol, &format!("    function exits() internal pure returns (ExitV[] memory a) {{\n        a = new ExitV[]({});", s.exits.len()));
    for (i, e) in s.exits.iter().enumerate() {
        p(
            &mut sol,
            &format!(
                "        a[{i}] = ExitV({}, {}, {}, {}, {});",
                sol_addr(&e.token),
                e.denom_index,
                e.denom,
                sol_b32(&e.commitment),
                sol_b32(&e.leaf)
            ),
        );
    }
    p(&mut sol, "    }");
    let vecs = |sol: &mut String, name: &str, vs: &[Vector]| -> Result<()> {
        p(sol, &format!("    function {name}() internal pure returns (ReleaseV[] memory a) {{\n        a = new ReleaseV[]({});", vs.len()));
        for (i, v) in vs.iter().enumerate() {
            let j = decoded(v)?;
            p(
                sol,
                &format!(
                    "        a[{i}] = ReleaseV(\n            \"{}\",\n            {},\n            {},\n            {},\n            {},\n            {},\n            {},\n            {},\n            {},\n            hex\"{}\",\n            {}\n        );",
                    v.label,
                    sol_b32(&j.root),
                    sol_b32(&j.nullifier),
                    sol_addr(&j.token),
                    v.denom_index,
                    sol_b32(&j.denom).replace("bytes32(", "uint256(bytes32(") + ")",
                    sol_addr(&j.recipient),
                    sol_addr(&j.relayer),
                    sol_b32(&j.relay_fee).replace("bytes32(", "uint256(bytes32(") + ")",
                    hex::encode(&v.journal),
                    sol_b32(&sha256(&[&v.journal])),
                ),
            );
        }
        p(sol, "    }");
        Ok(())
    };
    vecs(&mut sol, "releases", &positives)?;
    vecs(&mut sol, "rejected", &negatives)?;
    p(&mut sol, "}");

    Ok(Fixture { json, sol, positives, negatives })
}

/// `U256` of a vector field as u128 for printing (all test amounts fit).
pub fn as_u128(x: &U256) -> u128 {
    u128::from_be_bytes(x[16..].try_into().unwrap_or([0; 16]))
}
