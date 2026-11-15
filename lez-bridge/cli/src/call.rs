//! `--emit-call`: the Names Wallet `propose_private_call` JSON of a claim or a burn (Names
//! `docs/wallet/PROVIDER-SDK.md`, "Private calls"), written instead of sending. The bridge
//! mini-app's core module hands it to the wallet, which simulates, shows the person what it
//! does, and proves it only after they approve; nothing here proves or sends.
//!
//! The accounts are `ClaimPlan::mentions`' in the same order (the root actor, the claimer's own
//! actor of the first program, is implicit). PDAs are named by seed; each seed is computed from
//! the programs' public formulas and checked here against the program crates' own addresses.
//!
//! The file holds the claim secret `k` inside `message` (a claim instruction carries it), so it
//! is created new with mode 600 and only its path and size are printed.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use bridge_core::sha256;
use lee_core::{account::AccountId, program::PdaSeed};
use serde_json::{Value, json};

/// The bridge's config PDA seed (`bridge/src/lib.rs` `CONFIG_SEED`).
pub const CONFIG_SEED: [u8; 32] = *b"/RFP022/bridge/config/v1/000000/";
const ROOT_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/root/v1";
const NULLIFIER_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/claim-nullifier/v1";
const ANCHOR_SEED_DOMAIN: &[u8] = b"/RFP021/attest/anchor/v1";
/// `propose_private_call`'s message limit (PROVIDER-SDK.md, Limits).
pub const MAX_MESSAGE: usize = 16_384;
/// The fee cap declared when none is given and no sponsorship bounds it.
pub const DEFAULT_MAX_FEE: u128 = 20_000_000;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A PDA of `of` by seed, checked against the program crate's address for it.
pub fn seed_entry(seed: [u8; 32], of: &AccountId, program: Option<&str>, expect: &AccountId) -> Result<Value> {
    if AccountId::for_public_pda(of, &PdaSeed::new(seed)) != *expect {
        bail!("internal: the seed {} does not give the address {expect}", hex(&seed));
    }
    let mut v = json!({ "seed": hex(&seed), "of": of.to_string() });
    if let Some(p) = program {
        v["program"] = Value::from(p);
    }
    Ok(v)
}

pub fn config_entry(bridge: &AccountId, program: Option<&str>) -> Result<Value> {
    seed_entry(CONFIG_SEED, bridge, program, &bridge_core::config_address(bridge))
}

pub fn nullifier_entry(bridge: &AccountId, nullifier: &[u8; 32]) -> Result<Value> {
    seed_entry(sha256(&[NULLIFIER_SEED_DOMAIN, nullifier]), bridge, None, &bridge_core::nullifier_address(bridge, nullifier))
}

pub fn root_entry(bridge: &AccountId, root: &[u8; 32]) -> Result<Value> {
    seed_entry(sha256(&[ROOT_SEED_DOMAIN, root]), bridge, None, &bridge_core::root_address(bridge, root))
}

pub fn anchor_entry(attest_program: &AccountId, instance: &[u8; 32], anchor: &eth_attest_core::Anchor) -> Result<Value> {
    let seed = sha256(&[ANCHOR_SEED_DOMAIN, instance, &[anchor.kind()], anchor.value()]);
    let p = attest_program.to_string();
    seed_entry(seed, attest_program, Some(&p), &eth_attest_core::anchor_address(attest_program, instance, anchor))
}

/// The claimer's own actor of `program` (`native` for its LEZ balance).
#[must_use]
pub fn own(program: &str, openings: bool) -> Value {
    let mut v = json!({ "actor": "account", "program": program });
    if openings {
        v["openings"] = Value::from(true);
    }
    v
}

/// A public account's actor of `program`, never signed.
#[must_use]
pub fn public(account: &AccountId, program: &AccountId) -> Value {
    json!({ "account": account.to_string(), "program": program.to_string() })
}

/// The whole call.
#[allow(clippy::too_many_arguments)]
pub fn call(
    programs: &[AccountId],
    accounts: Vec<Value>,
    message: &[u8],
    openings: &[[u8; 32]],
    fee_payer: &str,
    credits: Vec<Value>,
    debits: Vec<Value>,
    max_fee: u128,
) -> Result<Value> {
    if message.len() > MAX_MESSAGE {
        bail!("the message is {} bytes, more than the {MAX_MESSAGE} a private call may carry", message.len());
    }
    let mut declared = json!({ "max_fee": max_fee.to_string() });
    if !credits.is_empty() {
        declared["credits"] = Value::from(credits);
    }
    if !debits.is_empty() {
        declared["debits"] = Value::from(debits);
    }
    Ok(json!({
        "programs": programs.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "accounts": accounts,
        "message": hex(message),
        "openings": openings.iter().map(|o| hex(o)).collect::<Vec<_>>(),
        "fee_payer": fee_payer,
        "declared": declared,
    }))
}

/// `{asset, amount}` of `declared`.
#[must_use]
pub fn amount(asset: &AccountId, amount: u128) -> Value {
    json!({ "asset": asset.to_string(), "amount": amount.to_string() })
}

/// Writes the call to a NEW file, mode 600 (it carries the claim secret).
pub fn write(path: &Path, call: &Value) -> Result<()> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("{} (must not exist yet)", path.display()))?;
    f.write_all(serde_json::to_string(call)?.as_bytes())?;
    Ok(())
}

/// A secret from a file: 64 hex characters (an optional `0x`, surrounding space ignored), in a
/// file only its owner can read (`-` reads standard input instead). The file is left as it is.
pub fn read_secret(path: &Path) -> Result<[u8; 32]> {
    use std::io::Read as _;
    let mut text = String::new();
    if path == Path::new("-") {
        std::io::stdin().read_to_string(&mut text).context("reading the secret from standard input")?;
    } else {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::metadata(path).with_context(|| format!("{}", path.display()))?;
        if meta.mode() & 0o077 != 0 {
            bail!("{} can be read by other users (mode {:o}); make it 600", path.display(), meta.mode() & 0o777);
        }
        std::fs::File::open(path)?.read_to_string(&mut text)?;
    }
    let t = text.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the secret file must hold 64 hex characters");
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&t[2 * i..2 * i + 2], 16)?;
    }
    Ok(out)
}
