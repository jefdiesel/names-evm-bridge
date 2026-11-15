// SPDX-License-Identifier: MIT OR Apache-2.0
//! outbound_wrap (x86-64 Linux with Docker for `wrap`; `check`/`seal` anywhere):
//!
//!   outbound_wrap wrap  --receipt receipt.bin --image-id H --out DIR
//!       the succinct receipt -> Groth16 (risc0-zkvm 3.1.0 compress; Docker image in DOCKER_IMAGE),
//!       checked natively; writes DIR/groth16.bin (borsh receipt), seal.bin (260 bytes:
//!       selector ‖ seal), seal.hex, journal.hex, wrap.json, verify-call.txt
//!   outbound_wrap check --receipt FILE --image-id H     natively, any kind
//!   outbound_wrap seal  --receipt groth16.bin           prints selector ‖ seal

use anyhow::{Result, anyhow, ensure};
use outbound_wrap::*;
use std::{collections::HashMap, path::PathBuf};

fn args(v: &[String]) -> HashMap<String, String> {
    v.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].trim_start_matches("--").to_string(), c[1].clone())).collect()
}

fn hex32(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim().trim_start_matches("0x"))?;
    v.try_into().map_err(|_| anyhow!("{s}: not 32 bytes"))
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = argv.split_first().ok_or_else(|| anyhow!("usage: outbound_wrap wrap|check|seal …"))?;
    let a = args(rest);
    let get = |k: &str| a.get(k).cloned().ok_or_else(|| anyhow!("missing --{k}"));
    match cmd.as_str() {
        "check" => {
            let r = load(&PathBuf::from(get("receipt")?))?;
            let image = hex32(&get("image-id")?)?;
            let kind = check(&r, image)?;
            println!("VERIFIED: {kind} receipt for image {}; journal digest {}", h0x(&image), h0x(&sha256(&r.journal.bytes)));
        }
        "seal" => {
            let r = load(&PathBuf::from(get("receipt")?))?;
            println!("{}", h0x(&encode_seal(&r)?));
        }
        "wrap" => {
            let r = load(&PathBuf::from(get("receipt")?))?;
            let image = hex32(&get("image-id")?)?;
            let out = PathBuf::from(get("out")?);
            std::fs::create_dir_all(&out)?;
            ensure!(selector() == SELECTOR_V3_1, "this build's Groth16 selector {} is not 3.1.0's", h0x(&selector()));
            println!("input: {} receipt for image {} verified natively", check(&r, image)?, h0x(&image));
            println!("WRAPPING with {DOCKER_IMAGE} (identity_p254 on the CPU, then the Docker Groth16 prover)");
            let t = std::time::Instant::now();
            let g = wrap(&r, image)?;
            let secs = t.elapsed().as_secs_f64();
            let seal = encode_seal(&g)?;
            ensure!(seal.len() == 260 && seal[..4] == SELECTOR_V3_1, "seal is not selector 0xe71e6620 + 256 bytes");
            let jd = sha256(&g.journal.bytes);
            save(&g, &out.join("groth16.bin"))?;
            std::fs::write(out.join("seal.bin"), &seal)?;
            std::fs::write(out.join("seal.hex"), h0x(&seal))?;
            std::fs::write(out.join("journal.hex"), h0x(&g.journal.bytes))?;
            std::fs::write(out.join("verify-call.txt"), verify_call(&seal, &image, &jd))?;
            std::fs::write(
                out.join("wrap.json"),
                serde_json::to_string_pretty(&serde_json::json!({
                    "imageId": h0x(&image), "journalDigest": h0x(&jd), "selector": h0x(&SELECTOR_V3_1),
                    "verifier": VERIFIER_V3_1_SEPOLIA, "dockerImage": DOCKER_IMAGE, "wrapSeconds": secs,
                    "sealBytes": seal.len(), "label": "Groth16 receipt, verified natively (risc0-zkvm 3.1.0)",
                }))?,
            )?;
            println!("WRAPPED: groth16 receipt verified natively, {secs:.1} s; seal {} bytes; {}", seal.len(), out.join("seal.bin").display());
            println!("SEAL={}", h0x(&seal));
            println!("JOURNAL_DIGEST={}", h0x(&jd));
        }
        _ => anyhow::bail!("unknown command {cmd}"),
    }
    Ok(())
}
