//! release_host: EXECUTOR ONLY. Runs every G3 vector in RISC Zero's in-process executor, prints
//! user cycles, and with `--write <dir>` writes `release_vector.json` and `ReleaseVector.sol`
//! (copy the .sol to vault/test/fixtures/). Never proves.
//!
//!   release_host                 print image id and cycles
//!   release_host --write <dir>   also write the fixture

use anyhow::Result;
use release_host::{fixture, image_id};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let f = fixture()?;
    println!("G3 release guest image id (as bytes32): 0x{}", hex::encode(image_id()));
    println!("ELF size: {} bytes", release_host::RELEASE_GUEST_ELF.len());
    println!("| vector | user cycles | total cycles | segments |");
    println!("|---|---:|---:|---:|");
    for v in f.positives.iter().chain(&f.negatives) {
        println!("| {} | {} | {} | {} |", v.label, v.run.user_cycles, v.run.total_cycles, v.run.segments);
    }
    if let Some(i) = args.iter().position(|a| a == "--write") {
        let dir = std::path::PathBuf::from(args.get(i + 1).expect("--write <dir>"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("release_vector.json"), &f.json)?;
        std::fs::write(dir.join("ReleaseVector.sol"), &f.sol)?;
        println!("wrote {}/release_vector.json and ReleaseVector.sol", dir.display());
    }
    Ok(())
}
