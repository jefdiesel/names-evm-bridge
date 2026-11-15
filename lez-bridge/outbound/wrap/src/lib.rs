// SPDX-License-Identifier: MIT OR Apache-2.0
//! outbound_wrap: the Groth16 wrap of a succinct RISC Zero receipt (an outbound update or a G3
//! release proved by `outbound_host … --prove`) into the seal RISC Zero's on-chain
//! `RiscZeroGroth16Verifier` 3.1.0 checks.
//!
//! - [`wrap`]: risc0-zkvm 3.1.0 `Prover::compress(ProverOpts::groth16(), receipt)`: `identity_p254`
//!   on the CPU, then risc0-groth16 3.0.6 `shrink_wrap`, which runs [`DOCKER_IMAGE`] (gnark). x86-64
//!   Linux with Docker only (RISC Zero: "only works on x86 architecture").
//! - [`check`]: the receipt verifies natively against the image id (Groth16 with the 3.1.0
//!   verifier parameters), is not a fake, and claims exactly `ok(image, sha256(journal))`.
//! - [`encode_seal`]: `selector ‖ seal`, the selector being the first 4 bytes of the receipt's
//!   verifier-parameters digest, as risc0-ethereum v3.1.0 `contracts/src/lib.rs` `encode_seal` does
//!   for a Groth16 receipt. For risc0 3.1.0 the selector is `0xe71e6620`.

use anyhow::{Context, Result, anyhow, bail, ensure};
use risc0_zkvm::{
    Groth16ReceiptVerifierParameters, InnerReceipt, LocalProver, MaybePruned, Prover as _, ProverOpts, Receipt, ReceiptClaim,
    sha::{Digest, Digestible as _, Impl, Sha256 as _},
};
use std::path::Path;

/// The Groth16 prover image risc0-groth16 3.0.6 runs (`src/prove/docker.rs`).
pub const DOCKER_IMAGE: &str = "risczero/risc0-groth16-prover:v2026-10-20.1";
/// RISC Zero's `RiscZeroGroth16Verifier` 3.1.0 on Sepolia (risc0-ethereum `contracts/deployment.toml`).
pub const VERIFIER_V3_1_SEPOLIA: &str = "0x8c915b5071206C6E328BC18529586c513408aB3b";
/// Its selector.
pub const SELECTOR_V3_1: [u8; 4] = [0xe7, 0x1e, 0x66, 0x20];
pub const SEPOLIA_RPC: &str = "https://sepolia.gateway.tenderly.co";

#[must_use]
pub fn h0x(b: &[u8]) -> String {
    format!("0x{}", hex::encode(b))
}

#[must_use]
pub fn sha256(b: &[u8]) -> [u8; 32] {
    let d = Impl::hash_bytes(b);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// The selector this risc0 version's Groth16 receipts carry.
#[must_use]
pub fn selector() -> [u8; 4] {
    let d = Groth16ReceiptVerifierParameters::default().digest();
    let b = d.as_bytes();
    [b[0], b[1], b[2], b[3]]
}

/// `selector ‖ seal` of a Groth16 receipt (risc0-ethereum v3.1.0 `encode_seal`, Groth16 arm).
pub fn encode_seal(r: &Receipt) -> Result<Vec<u8>> {
    let InnerReceipt::Groth16(g) = &r.inner else { bail!("not a Groth16 receipt") };
    Ok([&g.verifier_parameters.as_bytes()[..4], g.seal.as_slice()].concat())
}

/// Checks a receipt natively: not a fake, verifies against `image`, claim is `ok(image,
/// sha256(journal))` with no assumptions. Returns its kind.
pub fn check(r: &Receipt, image: [u8; 32]) -> Result<&'static str> {
    let kind = match &r.inner {
        InnerReceipt::Groth16(_) => "groth16",
        InnerReceipt::Succinct(_) => "succinct",
        InnerReceipt::Composite(_) => "composite",
        InnerReceipt::Fake(_) => bail!("a fake (dev-mode) receipt: refused"),
        _ => bail!("an unknown receipt kind: refused"),
    };
    r.verify(Digest::from_bytes(image)).map_err(|e| anyhow!("the receipt does not verify against image {}: {e}", h0x(&image)))?;
    let want = ReceiptClaim::ok(Digest::from_bytes(image), MaybePruned::Pruned(Digest::from(sha256(&r.journal.bytes))));
    ensure!(r.claim()?.digest() == want.digest(), "the receipt's claim is not ok(image, sha256(journal)) with no assumptions");
    Ok(kind)
}

/// Refuses dev mode and any remote or external prover.
pub fn gate(var: impl Fn(&str) -> Option<String>) -> Result<()> {
    for k in ["RISC0_DEV_MODE", "RISC0_SERVER_PATH", "BONSAI_API_URL", "BONSAI_API_KEY"] {
        ensure!(var(k).filter(|v| !v.is_empty()).is_none(), "{k} is set: refused");
    }
    Ok(())
}

/// The Groth16 wrap of a checked succinct receipt (x86-64 Linux with Docker). The result is
/// checked natively before it is returned.
pub fn wrap(succinct: &Receipt, image: [u8; 32]) -> Result<Receipt> {
    gate(|k| std::env::var(k).ok())?;
    ensure!(check(succinct, image)? == "succinct", "the input is not a succinct receipt");
    let opts = ProverOpts::groth16().with_dev_mode(false);
    let g = LocalProver::new("local").compress(&opts, succinct).context("Groth16 compress (identity_p254 + Docker shrink_wrap)")?;
    ensure!(check(&g, image)? == "groth16", "compress did not give a Groth16 receipt");
    ensure!(g.journal.bytes == succinct.journal.bytes, "the journal changed");
    Ok(g)
}

pub fn load(p: &Path) -> Result<Receipt> {
    borsh::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?).with_context(|| format!("{}: not a borsh receipt", p.display()))
}

pub fn save(r: &Receipt, p: &Path) -> Result<()> {
    std::fs::write(p, borsh::to_vec(r)?)?;
    Ok(())
}

/// The read-only on-chain check: `verify(seal, imageId, sha256(journal))` on the 3.1.0 verifier.
#[must_use]
pub fn verify_call(seal: &[u8], image: &[u8; 32], journal_digest: &[u8; 32]) -> String {
    format!(
        "cast call {VERIFIER_V3_1_SEPOLIA} \"verify(bytes,bytes32,bytes32)\" {} {} {} --rpc-url {SEPOLIA_RPC}\n",
        h0x(seal),
        h0x(image),
        h0x(journal_digest)
    )
}
