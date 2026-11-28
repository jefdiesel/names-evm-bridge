//! outbound_host: EXECUTOR ONLY driver for G2-lite (and G3 release inputs for the round trip).
//!
//! - [`Store`]: LEZ blocks from a sequencer's JSON-RPC (`getBlock`, `getLastBlockId`, the methods
//!   `cli/` and `blocksearch.py` (run private4) use), cached as files.
//! - [`HostState`]: the G2 state preimage, the bridge view and the accumulator leaves, saved
//!   between updates.
//! - [`prepare`]: runs the statement natively (with the whole accumulator tree and a native
//!   receipt verifier) and produces the guest input.
//! - [`execute`]: runs a guest image in RISC Zero's in-process executor, with each private
//!   transaction's receipt added as an assumption.
//! - [`prove_update`] / [`prove_release`]: the OWNER-STARTED proving path (`outbound_host update
//!   --prove`, `release --prove`). In-process CPU prover only (`LocalProver`), refused unless
//!   [`prove_gate`] passes (no r0vm, no dev mode, no Bonsai, `RISC0_PROVER=local`,
//!   `RAYON_NUM_THREADS` 1..=4); succinct receipts, checked natively by [`check_receipt`] before
//!   they are written. Tests and agents never call them: only `public-demo/prove/prove-outbound-rc3.sh`.
//! - [`Guest`]: the guest image to run or prove, the one embedded at build time or a canonical
//!   ELF from a file whose computed image id must equal a pinned one (guest builds are not
//!   reproducible across directories, so a vault's image is proved from its own ELF).

use anyhow::{Context, Result, anyhow, bail, ensure};
use borsh::{BorshDeserialize, BorshSerialize};
use outbound_core::{
    finality::{BlockRef, KIND_TRUSTED_TIP, TrustedTip},
    journal::UpdateJournal,
    state::G2State,
    update::{GuestHeader, HostAcc, JOURNAL_VERSION, NoMeter, ReceiptVerifier, Stats, UpdateInput, UpdateOutput, encode_input, update},
    view::BridgeView,
    wire,
};
use risc0_zkvm::{
    ExecutorEnv, ExecutorImpl, ExitCode, InnerReceipt, LocalProver, MaybePruned, Prover as _, ProverOpts, Receipt, ReceiptClaim,
    sha::{Digest, Digestible as _},
};
use std::{
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

include!(concat!(env!("OUT_DIR"), "/methods.rs"));

/// Label printed on every round-trip output.
pub const LABEL: &str = "mock-verified: proofs executed, not proved; LEZ finality trusted for the test";

#[must_use]
pub fn digest_bytes(d: impl Into<Digest>) -> [u8; 32] {
    let d: Digest = d.into();
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// The test G2 image (TrustedTip), the vault's `updateImageId` in local tests.
#[must_use]
pub fn update_image_id_trusted_tip() -> [u8; 32] {
    digest_bytes(OUTBOUND_GUEST_TRUSTED_TIP_ID)
}

/// The failsafe vault's G2 image (M6): TrustedTip, committing the V2 journal.
#[must_use]
pub fn update_image_id_failsafe() -> [u8; 32] {
    digest_bytes(OUTBOUND_GUEST_FAILSAFE_ID)
}

/// The release G2 image (Logos Blockchain light client, not implemented).
#[must_use]
pub fn update_image_id_release() -> [u8; 32] {
    digest_bytes(OUTBOUND_GUEST_ID)
}

/// The G3 image, the vault's `releaseImageId`.
#[must_use]
pub fn release_image_id() -> [u8; 32] {
    digest_bytes(RELEASE_GUEST_ID)
}

#[must_use]
pub fn sha256(b: &[u8]) -> [u8; 32] {
    outbound_core::sha256(&[b])
}

/// The seal the vault's test `MockVerifier` accepts: `abi.encodePacked(imageId, journalDigest)`.
#[must_use]
pub fn mock_seal(image_id: &[u8; 32], journal: &[u8]) -> Vec<u8> {
    [image_id.as_slice(), &sha256(journal)].concat()
}

pub fn h0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

pub fn parse_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let v = hex::decode(s.trim().trim_start_matches("0x")).with_context(|| format!("hex: {s}"))?;
    v.try_into().map_err(|v: Vec<u8>| anyhow!("{s}: {} bytes, want {N}", v.len()))
}

/// An account id given as base58 (LEZ's display) or 0x-hex.
pub fn parse_account(s: &str) -> Result<[u8; 32]> {
    if s.starts_with("0x") {
        return parse_hex::<32>(s);
    }
    let id: lee_core::account::AccountId = s.parse().map_err(|e| anyhow!("account id {s}: {e:?}"))?;
    Ok(*id.value())
}

// ------------------------------------------------------------------------------- RPC and blocks

/// A minimal HTTP/1.0 JSON-RPC client (the sequencer is plain HTTP on localhost).
pub fn rpc(url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let rest = url.strip_prefix("http://").ok_or_else(|| anyhow!("only http:// URLs: {url}"))?;
    let (hostport, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let mut s = std::net::TcpStream::connect(hostport).with_context(|| format!("connect {hostport}"))?;
    write!(
        s,
        "POST {path} HTTP/1.0\r\nHost: {hostport}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp)?;
    let split = resp.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| anyhow!("no HTTP header end"))?;
    let head = String::from_utf8_lossy(&resp[..split]).to_lowercase();
    let mut payload = resp[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        let mut out = Vec::new();
        let mut cur = payload.as_slice();
        loop {
            let nl = cur.windows(2).position(|w| w == b"\r\n").ok_or_else(|| anyhow!("chunk"))?;
            let n = usize::from_str_radix(String::from_utf8_lossy(&cur[..nl]).trim(), 16)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&cur[nl + 2..nl + 2 + n]);
            cur = &cur[nl + 2 + n + 2..];
        }
        payload = out;
    }
    let v: serde_json::Value = serde_json::from_slice(&payload).with_context(|| format!("{method}: not JSON"))?;
    if let Some(e) = v.get("error") {
        bail!("{method}: {e}");
    }
    Ok(v["result"].clone())
}

pub fn last_block_id(url: &str) -> Result<u64> {
    rpc(url, "getLastBlockId", serde_json::json!([]))?.as_u64().ok_or_else(|| anyhow!("getLastBlockId: not a number"))
}

pub fn fetch_block(url: &str, id: u64) -> Result<Vec<u8>> {
    use base64::Engine as _;
    let r = rpc(url, "getBlock", serde_json::json!([id]))?;
    let s = r.as_str().ok_or_else(|| anyhow!("getBlock {id}: no block"))?;
    Ok(base64::engine::general_purpose::STANDARD.decode(s)?)
}

/// Blocks cached as `<dir>/block-<id>.bin`, fetched from `rpc` when missing.
pub struct Store {
    pub dir: PathBuf,
    pub rpc: Option<String>,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>, rpc: Option<String>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, rpc })
    }

    pub fn get(&self, id: u64) -> Result<Vec<u8>> {
        let p = self.dir.join(format!("block-{id}.bin"));
        if let Ok(b) = std::fs::read(&p) {
            return Ok(b);
        }
        let url = self.rpc.as_ref().ok_or_else(|| anyhow!("block {id} not cached in {} and no --rpc", self.dir.display()))?;
        let b = fetch_block(url, id)?;
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, &b)?;
        std::fs::rename(&tmp, &p)?;
        Ok(b)
    }

    pub fn range(&self, from: u64, to: u64) -> Result<Vec<Vec<u8>>> {
        (from..=to).map(|i| self.get(i)).collect()
    }

    /// `(id, recomputed hash)` of a block.
    pub fn block_ref(&self, id: u64) -> Result<BlockRef> {
        let raw = self.get(id)?;
        let b = wire::parse_block(&raw).map_err(|e| anyhow!("block {id}: {}", e.0))?;
        ensure!(b.header.block_id == id && b.header.hash == b.hash, "block {id}: bad id or stored hash");
        Ok(BlockRef { id, hash: b.hash })
    }
}

// ------------------------------------------------------------------------------- state

/// Everything the prover keeps between updates.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct HostState {
    pub g2: G2State,
    pub view: BridgeView,
    pub leaves: Vec<claim_acc::Leaf>,
}

impl HostState {
    #[must_use]
    pub fn genesis(bridge_program: [u8; 32], tip: BlockRef) -> Self {
        Self {
            g2: G2State::genesis(KIND_TRUSTED_TIP, bridge_program, tip),
            view: BridgeView::default(),
            leaves: claim_acc::Tree::new().leaves().to_vec(),
        }
    }

    pub fn load(p: &Path) -> Result<Self> {
        Ok(borsh::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)?)
    }

    /// Writes the state (borsh) and a readable summary next to it (`.json`).
    pub fn save(&self, p: &Path) -> Result<()> {
        std::fs::write(p, borsh::to_vec(self)?)?;
        std::fs::write(p.with_extension("json"), serde_json::to_string_pretty(&self.summary())?)?;
        Ok(())
    }

    pub fn tree(&self) -> Result<claim_acc::Tree> {
        claim_acc::Tree::from_leaves(&self.leaves).map_err(|e| anyhow!("accumulator leaves: {e}"))
    }

    #[must_use]
    pub fn summary(&self) -> serde_json::Value {
        let g = &self.g2;
        serde_json::json!({
            "g2State": h0x(&g.commitment()),
            "finalityKind": g.finality_kind,
            "bridgeProgram": h0x(&g.bridge_program),
            "tipId": g.tip.id,
            "tipHash": h0x(&g.tip.hash),
            "claimAccRoot": h0x(&g.claim_acc.root),
            "claimAccCount": g.claim_acc.count,
            "exitCount": g.exit_count,
            "woundDown": g.wound_down.iter().map(|a| h0x(a)).collect::<Vec<_>>(),
            "viewHash": h0x(&g.view_hash),
            "cfgId": h0x(&self.view.cfg_id),
            "tokens": self.view.tokens.iter().map(|t| serde_json::json!({
                "ethToken": h0x(&t.eth_token),
                "definition": h0x(&t.definition),
                "denoms": t.denoms.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
                "claims": t.claims, "burns": t.burns,
                "claimedValue": t.claimed_value.to_string(), "burnedValue": t.burned_value.to_string(),
            })).collect::<Vec<_>>(),
        })
    }
}

// ------------------------------------------------------------------------------- receipts

/// Verifies each private transaction's receipt natively (real cryptography: the succinct
/// receipt's seal against `circuit` and this journal), and keeps the receipts for the executor's
/// assumptions. `Default` pins [`outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID`], the circuit the
/// guest pins; [`NativeVerifier::for_circuit`] exists only to check the LEZ rc1 fixtures natively.
pub struct NativeVerifier {
    pub circuit: [u8; 32],
    pub receipts: Vec<Receipt>,
    pub failures: usize,
}

impl Default for NativeVerifier {
    fn default() -> Self {
        Self::for_circuit(outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID)
    }
}

impl NativeVerifier {
    #[must_use]
    pub const fn for_circuit(circuit: [u8; 32]) -> Self {
        Self { circuit, receipts: Vec::new(), failures: 0 }
    }
}

impl ReceiptVerifier for NativeVerifier {
    fn verify(&mut self, journal: &[u8], proof: &[u8]) -> bool {
        let ok = borsh::from_slice::<InnerReceipt>(proof).ok().and_then(|inner| {
            let r = Receipt::new(inner, journal.to_vec());
            r.verify(Digest::from_bytes(self.circuit)).ok().map(|()| r)
        });
        match ok {
            Some(r) => {
                self.receipts.push(r);
                true
            }
            None => {
                self.failures += 1;
                false
            }
        }
    }
}

// ------------------------------------------------------------------------------- one update

pub struct Prepared {
    pub out: UpdateOutput,
    pub header: GuestHeader,
    pub blocks: Vec<Vec<u8>>,
    pub receipts: Vec<Receipt>,
    pub next: HostState,
    pub journal: Vec<u8>,
}

/// Runs the statement natively from `state` over `blocks` (TrustedTip at the last block) and
/// builds the guest input, with the trusted-tip guest's receipt policy
/// ([`outbound_core::update::SequencerTrusted`]: private receipts not checked, no assumptions).
pub fn prepare(state: &HostState, blocks: Vec<Vec<u8>>) -> Result<Prepared> {
    prepare_with_verifier(state, blocks, &mut outbound_core::update::SequencerTrusted)
}

/// [`prepare`] with private receipts checked against `circuit` instead of the pinned one. Tests
/// only: the LEZ rc1 private fixtures were proved under rc1's circuit, which the guest no longer
/// pins, so their receipts satisfy the native statement only.
pub fn prepare_for_circuit(state: &HostState, blocks: Vec<Vec<u8>>, circuit: [u8; 32]) -> Result<Prepared> {
    let mut verifier = NativeVerifier::for_circuit(circuit);
    let mut p = prepare_with_verifier(state, blocks, &mut verifier)?;
    p.receipts = verifier.receipts;
    Ok(p)
}

/// [`prepare`] with any receipt verifier; `receipts` stays empty (nothing to pass as assumptions).
pub fn prepare_with_verifier(state: &HostState, blocks: Vec<Vec<u8>>, verifier: &mut impl ReceiptVerifier) -> Result<Prepared> {
    let last = blocks.last().ok_or_else(|| anyhow!("no blocks"))?;
    let tip = wire::parse_block(last).map_err(|e| anyhow!("last block: {}", e.0))?;
    let finality = borsh::to_vec(&BlockRef { id: tip.header.block_id, hash: tip.hash })?;
    let mut acc = HostAcc { tree: state.tree()?, witnesses: Vec::new() };
    let input = UpdateInput {
        prev: state.g2.clone(),
        view: state.view.clone(),
        finality: finality.clone(),
        blocks: blocks.iter().map(Vec::as_slice).collect(),
    };
    let out = update::<TrustedTip>(&input, &mut acc, verifier, &NoMeter).map_err(|e| anyhow!("G2 refused natively: {e}"))?;
    let header = GuestHeader { prev: state.g2.clone(), view: state.view.clone(), finality, inserts: acc.witnesses };
    let next = HostState { g2: out.next.clone(), view: out.view.clone(), leaves: acc.tree.leaves().to_vec() };
    let journal = out.journal.abi_encode(JOURNAL_VERSION);
    Ok(Prepared { out, header, blocks, receipts: Vec::new(), next, journal })
}

pub struct Run {
    pub journal: Vec<u8>,
    pub user_cycles: u64,
    pub total_cycles: u64,
    pub segments: usize,
    pub assumptions: usize,
    pub stats: Stats,
}

/// The executor environment of one G2 run: `u32le(len) | blob`, each private receipt as an
/// assumption, an optional segment size (default: RISC Zero's, 2^20) and stats on stdout.
fn update_env<'a>(blob: &[u8], receipts: &[Receipt], po2: Option<u32>, stdout: Option<&'a mut Vec<u8>>) -> Result<ExecutorEnv<'a>> {
    let mut b = ExecutorEnv::builder();
    for r in receipts {
        b.add_assumption(r.clone());
    }
    if let Some(n) = po2 {
        ensure!((15..=22).contains(&n), "segment po2 {n} outside 15..=22");
        b.segment_limit_po2(n);
    }
    b.write_slice(&(blob.len() as u32).to_le_bytes());
    b.write_slice(blob);
    if let Some(out) = stdout {
        b.stdout(out);
    }
    b.build()
}

/// Executes a G2 guest image on a prepared input (never proves). With no private transactions,
/// checks that the claim a proof would carry is `ReceiptClaim::ok(image, sha256(journal))`, what
/// risc0-ethereum's `verify(seal, imageId, journalDigest)` rebuilds. With some, the session's
/// claim still lists them as assumptions; a prover resolves them with the receipts added here.
pub fn execute(elf: &[u8], image: [u8; 32], header: &GuestHeader, blocks: &[Vec<u8>], receipts: &[Receipt]) -> Result<Run> {
    execute_po2(elf, image, header, blocks, receipts, None)
}

/// [`execute`] with a segment size (`po2`, the prover's segments of 2^po2 cycles).
pub fn execute_po2(elf: &[u8], image: [u8; 32], header: &GuestHeader, blocks: &[Vec<u8>], receipts: &[Receipt], po2: Option<u32>) -> Result<Run> {
    let blob = encode_input(header, blocks);
    let mut stdout = Vec::new();
    let s = {
        let env = update_env(&blob, receipts, po2, Some(&mut stdout))?;
        ExecutorImpl::from_elf(env, elf)?.run()?
    };
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let claim = s.claim()?;
    if receipts.is_empty() {
        let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&journal))));
        ensure!(claim.digest() == want.digest(), "receipt claim does not commit to sha256(journal)");
    }
    let stats = Stats::try_from_slice(&stdout).unwrap_or_default();
    Ok(Run {
        journal,
        user_cycles: s.user_cycles,
        total_cycles: s.total_cycles,
        segments: s.segments.len(),
        assumptions: receipts.len(),
        stats,
    })
}

/// The test G2 guest on a prepared update; its journal must equal the native one.
pub fn execute_trusted_tip(p: &Prepared) -> Result<Run> {
    let run = execute(OUTBOUND_GUEST_TRUSTED_TIP_ELF, update_image_id_trusted_tip(), &p.header, &p.blocks, &p.receipts)?;
    ensure!(run.journal == p.journal, "the guest's journal differs from the native run's");
    Ok(run)
}

/// [`execute_trusted_tip`] with a given guest (embedded or a canonical ELF) and segment size.
pub fn execute_guest(g: &Guest, p: &Prepared, po2: Option<u32>) -> Result<Run> {
    let run = execute_po2(&g.elf, g.image, &p.header, &p.blocks, &p.receipts, po2)?;
    ensure!(run.journal == p.journal, "the guest's journal differs from the native run's");
    Ok(run)
}

/// Decodes a V1 journal (as the vault would).
pub fn decode_journal(b: &[u8]) -> Result<UpdateJournal> {
    UpdateJournal::abi_decode_v1(b).ok_or_else(|| anyhow!("not a canonical abi.encode(UpdateJournal)"))
}

// ------------------------------------------------------------------------------- G3 release

pub struct ReleaseRun {
    pub journal: Vec<u8>,
    pub decoded: release_core::ReleaseJournal,
    pub user_cycles: u64,
}

/// Executes the G3 guest (never proves) and checks its claim commits to sha256(journal).
pub fn execute_release(input: &release_core::ReleaseInput) -> Result<ReleaseRun> {
    execute_release_guest(&Guest::release(), input)
}

/// [`execute_release`] with a given release guest (embedded or a canonical ELF).
pub fn execute_release_guest(g: &Guest, input: &release_core::ReleaseInput) -> Result<ReleaseRun> {
    let env = ExecutorEnv::builder().write_slice(&input.to_bytes()).build()?;
    let s = ExecutorImpl::from_elf(env, &g.elf)?.run()?;
    ensure!(s.exit_code == ExitCode::Halted(0), "exit {:?}", s.exit_code);
    let journal = s.journal.as_ref().context("no journal")?.bytes.clone();
    let want = ReceiptClaim::ok(Digest::from_bytes(g.image), MaybePruned::Pruned(Digest::from(sha256(&journal))));
    ensure!(s.claim()?.digest() == want.digest(), "release claim does not commit to sha256(journal)");
    let decoded = release_core::ReleaseJournal::abi_decode(&journal).ok_or_else(|| anyhow!("release journal"))?;
    ensure!(decoded == release_core::release(input).map_err(|e| anyhow!("{e}"))?, "release guest differs from native");
    Ok(ReleaseRun { journal, decoded, user_cycles: s.user_cycles })
}

/// `(leaf index, 32-sibling path)` in the vault's exit tree from its leaves in order.
#[must_use]
pub fn exit_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    bridge_core::merkle_path(leaves, index)
}

// ------------------------------------------------------------------------------- guests

/// A guest image to execute or prove: its ELF and its image id (computed, never taken on trust).
#[derive(Clone)]
pub struct Guest {
    pub elf: Vec<u8>,
    pub image: [u8; 32],
    /// Where the ELF came from (`embedded` or the file), for reports.
    pub source: String,
}

impl Guest {
    /// The TrustedTip G2 guest embedded at build time.
    #[must_use]
    pub fn trusted_tip() -> Self {
        Self { elf: OUTBOUND_GUEST_TRUSTED_TIP_ELF.to_vec(), image: update_image_id_trusted_tip(), source: "embedded".into() }
    }

    /// The failsafe vault's G2 guest (TrustedTip, V2 journal) embedded at build time.
    #[must_use]
    pub fn failsafe() -> Self {
        Self { elf: OUTBOUND_GUEST_FAILSAFE_ELF.to_vec(), image: update_image_id_failsafe(), source: "embedded".into() }
    }

    /// The G3 release guest embedded at build time.
    #[must_use]
    pub fn release() -> Self {
        Self { elf: RELEASE_GUEST_ELF.to_vec(), image: release_image_id(), source: "embedded".into() }
    }

    /// A canonical guest ELF from a file. Its image id is computed here and must equal `pin` (the
    /// id the vault stores), or nothing runs.
    pub fn from_file(path: &Path, pin: [u8; 32]) -> Result<Self> {
        let elf = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let image = digest_bytes(risc0_zkvm::compute_image_id(&elf).with_context(|| format!("{}: not a guest ELF", path.display()))?);
        ensure!(image == pin, "{}: image id {} is not the pinned {}", path.display(), h0x(&image), h0x(&pin));
        Ok(Self { elf, image, source: path.display().to_string() })
    }

    /// The embedded guest, which must have the pinned id when one is given.
    pub fn embedded_pinned(g: Self, pin: Option<[u8; 32]>) -> Result<Self> {
        if let Some(p) = pin {
            ensure!(g.image == p, "the embedded guest's image id {} is not the pinned {}; pass the canonical ELF with --elf", h0x(&g.image), h0x(&p));
        }
        Ok(g)
    }
}

// ------------------------------------------------------------------------------- proving

/// The 4-byte selector of this risc0 version's Groth16 verifier parameters (what a Groth16 seal
/// for the RiscZeroVerifierRouter starts with).
#[must_use]
pub fn groth16_selector() -> [u8; 4] {
    let d = digest_bytes(risc0_zkvm::Groth16ReceiptVerifierParameters::default().digest());
    [d[0], d[1], d[2], d[3]]
}

/// Label of every proved output.
pub const PROVED_LABEL: &str = "proved: succinct receipt, RISC Zero in-process CPU prover, verified natively; LEZ finality trusted (TrustedTip); not checked on chain";

/// Where an `r0vm` could be picked up: each `PATH` entry, `~/.risc0/bin`, `~/.risc0/extensions/*`.
#[must_use]
pub fn r0vm_candidates(path: Option<&str>, home: Option<&str>) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).map(|d| d.join("r0vm")).collect()).unwrap_or_default();
    if let Some(h) = home {
        let r = Path::new(h).join(".risc0");
        v.push(r.join("bin/r0vm"));
        if let Ok(rd) = std::fs::read_dir(r.join("extensions")) {
            v.extend(rd.flatten().map(|e| e.path().join("r0vm")));
        }
    }
    v
}

/// The proving rules, checked before any proof: the in-process CPU prover and nothing else.
/// `var` reads the environment; `r0vm` lists places an external prover could come from.
pub fn prove_gate(var: impl Fn(&str) -> Option<String>, r0vm: &[PathBuf]) -> Result<()> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    for k in ["RISC0_DEV_MODE", "RISC0_SERVER_PATH", "BONSAI_API_URL", "BONSAI_API_KEY"] {
        ensure!(set(k).is_none(), "{k} is set: refused (dev mode gives fake receipts; the others select another prover)");
    }
    ensure!(set("RISC0_PROVER").as_deref() == Some("local"), "RISC0_PROVER must be `local` (the in-process CPU prover)");
    let t: usize = set("RAYON_NUM_THREADS").and_then(|v| v.parse().ok()).unwrap_or(0);
    ensure!((1..=4).contains(&t), "RAYON_NUM_THREADS must be 1..=4");
    if let Some(p) = r0vm.iter().find(|p| p.exists()) {
        bail!("an r0vm exists at {}: refused (it must stay disabled)", p.display());
    }
    Ok(())
}

/// [`prove_gate`] on this process's environment.
pub fn prove_gate_env() -> Result<()> {
    let path = std::env::var("PATH").ok();
    let home = std::env::var("HOME").ok();
    prove_gate(|k| std::env::var(k).ok(), &r0vm_candidates(path.as_deref(), home.as_deref()))
}

/// Checks a receipt natively, as a verifier of the vault's kind would: real cryptography against
/// `image` (`Receipt::verify`), not a fake, and its claim is exactly `ReceiptClaim::ok(image,
/// sha256(journal))` with no assumption left (what `verify(seal, imageId, sha256(journal))`
/// rebuilds); with `journal`, the receipt's journal must be those bytes. Returns the kind.
pub fn check_receipt(r: &Receipt, image: [u8; 32], journal: Option<&[u8]>) -> Result<&'static str> {
    let kind = match &r.inner {
        InnerReceipt::Succinct(_) => "succinct",
        InnerReceipt::Groth16(_) => "groth16",
        InnerReceipt::Composite(_) => "composite",
        InnerReceipt::Fake(_) => bail!("a fake (dev-mode) receipt: refused"),
        _ => bail!("an unknown receipt kind: refused"),
    };
    r.verify(Digest::from_bytes(image)).map_err(|e| anyhow!("the receipt does not verify against image {}: {e}", h0x(&image)))?;
    let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&r.journal.bytes))));
    ensure!(r.claim()?.digest() == want.digest(), "the receipt's claim is not ok(image, sha256(journal)) with no assumptions");
    if let Some(j) = journal {
        ensure!(r.journal.bytes == j, "the receipt's journal is not the expected journal");
    }
    Ok(kind)
}

/// `receipt.bin`: borsh of the receipt.
pub fn save_receipt(r: &Receipt, p: &Path) -> Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, borsh::to_vec(r)?)?;
    std::fs::rename(&tmp, p)?;
    Ok(())
}

pub fn load_receipt(p: &Path) -> Result<Receipt> {
    borsh::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?).with_context(|| format!("{}: not a borsh receipt", p.display()))
}

pub struct Proved {
    pub receipt: Receipt,
    pub kind: &'static str,
    pub user_cycles: u64,
    pub total_cycles: u64,
    pub segments: usize,
    pub secs: f64,
}

impl Proved {
    #[must_use]
    pub fn report(&self, g: &Guest) -> serde_json::Value {
        serde_json::json!({
            "label": PROVED_LABEL,
            "receiptKind": self.kind,
            "imageId": h0x(&g.image),
            "guest": g.source,
            "journalDigest": h0x(&sha256(&self.receipt.journal.bytes)),
            "claimDigest": h0x(&digest_bytes(self.receipt.claim().map(|c| c.digest()).unwrap_or_default())),
            "userCycles": self.user_cycles, "totalCycles": self.total_cycles, "segments": self.segments,
            "proveSeconds": self.secs,
            "receiptBytes": borsh::to_vec(&self.receipt).map(|b| b.len()).unwrap_or(0),
        })
    }
}

fn prove_env(env: ExecutorEnv<'_>, g: &Guest) -> Result<Proved> {
    prove_gate_env()?;
    let opts = ProverOpts::succinct().with_dev_mode(false);
    let t = std::time::Instant::now();
    let info = LocalProver::new("local").prove_with_opts(env, &g.elf, &opts)?;
    let secs = t.elapsed().as_secs_f64();
    let kind = check_receipt(&info.receipt, g.image, None)?;
    Ok(Proved {
        kind,
        user_cycles: info.stats.user_cycles,
        total_cycles: info.stats.total_cycles,
        segments: info.stats.segments,
        secs,
        receipt: info.receipt,
    })
}

/// OWNER-STARTED: proves a prepared G2 update with `g` (succinct, in-process CPU prover), each
/// private receipt resolved as an assumption, and checks the receipt natively and that its journal
/// is the native journal.
pub fn prove_update(g: &Guest, p: &Prepared, po2: Option<u32>) -> Result<Proved> {
    prove_gate_env()?;
    let blob = encode_input(&p.header, &p.blocks);
    // The guest writes its cycle profile (borsh) to stdout: keep it out of the caller's log.
    let mut profile = Vec::new();
    let env = update_env(&blob, &p.receipts, po2, Some(&mut profile))?;
    let r = prove_env(env, g)?;
    ensure!(r.receipt.journal.bytes == p.journal, "the proved journal differs from the native one");
    Ok(r)
}

/// OWNER-STARTED: proves a G3 release with `g`, and checks the receipt natively and that its
/// journal is the native statement's.
pub fn prove_release(g: &Guest, input: &release_core::ReleaseInput, po2: Option<u32>) -> Result<Proved> {
    prove_gate_env()?;
    let mut b = ExecutorEnv::builder();
    if let Some(n) = po2 {
        ensure!((15..=22).contains(&n), "segment po2 {n} outside 15..=22");
        b.segment_limit_po2(n);
    }
    b.write_slice(&input.to_bytes());
    let r = prove_env(b.build()?, g)?;
    let native = release_core::release(input).map_err(|e| anyhow!("{e}"))?.abi_encode();
    ensure!(r.receipt.journal.bytes == native, "the proved release journal differs from the native one");
    Ok(r)
}
