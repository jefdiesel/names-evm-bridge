//! This install's deposits and redemption plans, in `<data>/positions.json` (0600).
//!
//! Each deposit keeps its secret `k` here and nowhere else: never in a reply, a job record, a
//! log or a fixture. Until the CLI derives secrets from the wallet seed (CLI-CONTRACT.md §5),
//! this file is what lets a deposit be claimed, so the UI says to keep it backed up.

use crate::util::{hex0x, hex32, keccak, now_ms, read_json, sha256, write_private};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Store {
    pub path: PathBuf,
}

/// Fields of a stored position never shown outside this module.
const SECRET_FIELDS: [&str; 2] = ["secret", "exit_secrets"];

impl Store {
    pub fn at(data: &Path) -> Store {
        Store { path: data.join("positions.json") }
    }

    pub fn load(&self) -> Value {
        let mut v = read_json(&self.path).unwrap_or_else(|| json!({}));
        for k in ["deposits", "redeems"] {
            if !v[k].is_array() {
                v[k] = json!([]);
            }
        }
        v
    }

    pub fn save(&self, v: &Value) -> Result<(), String> {
        write_private(&self.path, serde_json::to_vec_pretty(v).unwrap_or_default().as_slice()).map_err(|e| format!("cannot save {}: {e}", self.path.display()))
    }

    pub fn deposit(&self, id: &str) -> Result<Value, String> {
        self.load()["deposits"].as_array().unwrap().iter().find(|d| d["id"] == id).cloned().ok_or_else(|| format!("there is no deposit {id} on this computer"))
    }

    pub fn put_deposit(&self, d: Value) -> Result<(), String> {
        let mut all = self.load();
        let list = all["deposits"].as_array_mut().unwrap();
        match list.iter_mut().find(|x| x["id"] == d["id"]) {
            Some(x) => *x = d,
            None => list.push(d),
        }
        self.save(&all)
    }

    pub fn redeem(&self, id: &str) -> Result<Value, String> {
        self.load()["redeems"].as_array().unwrap().iter().find(|d| d["id"] == id).cloned().ok_or_else(|| format!("there is no redemption plan {id}"))
    }

    pub fn put_redeem(&self, r: Value) -> Result<(), String> {
        let mut all = self.load();
        let list = all["redeems"].as_array_mut().unwrap();
        match list.iter_mut().find(|x| x["id"] == r["id"]) {
            Some(x) => *x = r,
            None => list.push(r),
        }
        self.save(&all)
    }
}

/// A position as shown: every secret field removed.
pub fn public_view(v: &Value) -> Value {
    let mut out = v.clone();
    if let Some(o) = out.as_object_mut() {
        for k in SECRET_FIELDS {
            o.remove(k);
        }
        if let Some(pieces) = o.get_mut("pieces").and_then(Value::as_array_mut) {
            for p in pieces {
                if let Some(po) = p.as_object_mut() {
                    po.remove("secret");
                }
            }
        }
    }
    out
}

/// A short id for a commitment: `dep-` and its first 12 hex digits.
pub fn deposit_id(commitment: &str) -> String {
    format!("dep-{}", &commitment.trim_start_matches("0x")[..12].to_ascii_lowercase())
}

/// The vault's leaf for a deposit: `sha256(DEPOSIT_LEAF_TAG ‖ cfgId ‖ token ‖ uint256 denom ‖ C)`
/// (`BridgeVault.depositLeaf`, `abi.encodePacked`).
pub fn deposit_leaf(cfg_id: &[u8; 32], token: &[u8; 20], denom: u128, commitment: &[u8; 32]) -> [u8; 32] {
    let tag = keccak(b"RFP022/deposit-leaf/v1");
    let mut d = [0u8; 32];
    d[16..].copy_from_slice(&denom.to_be_bytes());
    sha256(&[&tag, cfg_id, token, &d, commitment])
}

/// The leaves file the CLI's claim reads: one hex leaf per line, in vault order.
pub fn leaves_text(cfg_id: &str, events: &[crate::eth::DepositEvent], denoms: &dyn Fn(&[u8; 20], u32) -> Option<u128>) -> Result<String, String> {
    let cfg = hex32(cfg_id)?;
    let mut out = String::new();
    for e in events {
        let denom = denoms(&e.token, e.denom_index).ok_or_else(|| format!("deposit {} names a denomination the registry does not list", e.leaf_index))?;
        out.push_str(&hex0x(&deposit_leaf(&cfg, &e.token, denom, &e.commitment)));
        out.push('\n');
    }
    Ok(out)
}

pub fn new_deposit(token: &str, symbol: &str, denom_index: u32, denom: u128, account: &str, account_kind: &str, commitment: &str, secret: &str, source: &str) -> Value {
    json!({
        "id": deposit_id(commitment),
        "eth_token": token,
        "symbol": symbol,
        "denom_index": denom_index,
        "denom": denom.to_string(),
        "account": account,
        "account_kind": account_kind,
        "commitment": commitment,
        "secret": secret,
        "created": now_ms(),
        "source": source,
        "claimed": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::parse_deposit_log;

    #[test]
    fn leaves_match_the_ones_the_real_claims_used() {
        // The v2 vault's five deposits (Sepolia logs) give exactly the leaves file the proved
        // claim of LEZ block 4998 used (run/sepolia2/attested/leaves.txt).
        let logs: Value = serde_json::from_str(include_str!("../testdata/eth/logs.json")).unwrap();
        let events: Vec<_> = logs["result"].as_array().unwrap().iter().map(|l| parse_deposit_log(l).unwrap()).collect();
        let usdc = crate::util::eth_address("0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e").unwrap();
        let denoms = |t: &[u8; 20], i: u32| -> Option<u128> {
            let list: [u128; 3] = if *t == usdc { [100_000_000, 1_000_000_000, 10_000_000_000] } else { [10u128.pow(16), 10u128.pow(17), 10u128.pow(18)] };
            list.get(i as usize).copied()
        };
        let text = leaves_text("0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11", &events, &denoms).unwrap();
        assert_eq!(text, include_str!("../testdata/eth/leaves.txt"));
    }

    #[test]
    fn secrets_never_leave_through_a_view() {
        let d = new_deposit("0xac", "dUSDC", 0, 100, "V", "private", "0xa50e0f44940eca30735460fa9b657d6ab948fc422b29b1293be0f7e5d6b7ad2e", "0x1234", "module");
        assert_eq!(d["id"], "dep-a50e0f44940e");
        let v = public_view(&d);
        assert!(v.get("secret").is_none());
        assert!(!v.to_string().contains("0x1234"));
        let r = json!({ "id": "r1", "pieces": [{ "secret": "0x99", "e": 1 }], "exit_secrets": ["0x98"] });
        assert!(!public_view(&r).to_string().contains("0x9"));
    }

    #[test]
    fn the_store_is_private_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("lez-bridge-store-{}-{}", std::process::id(), now_ms()));
        let s = Store::at(&dir);
        s.put_deposit(json!({ "id": "dep-1", "secret": "0xaa" })).unwrap();
        s.put_deposit(json!({ "id": "dep-1", "secret": "0xaa", "claimed": true })).unwrap();
        assert_eq!(s.load()["deposits"].as_array().unwrap().len(), 1);
        assert_eq!(s.deposit("dep-1").unwrap()["claimed"], true);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&s.path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
