//! What the Ethereum vault says: per-token config, caps and usage, relay-fee menus, delay,
//! freezes, roles, counters and the deposit tree. All reads (`eth_call`).

use crate::eth::{addr_hex, calldata, word_address, word_u64, EthSource, Words};
use crate::util::{checksum_address, hex0x, keccak};
use serde_json::{json, Value};

/// `updateImageId` of the Sepolia demo vaults: a placeholder, so no exit can ever be released.
pub fn demo_update_image() -> [u8; 32] {
    keccak(b"RFP022/demo/no-update-image")
}

/// The calls one registry read makes, in a fixed order: the globals, then per token.
pub fn registry_calls(tokens: &[[u8; 20]], denoms_per_token: &[usize]) -> Vec<(String, Vec<u8>)> {
    let mut calls: Vec<(String, Vec<u8>)> = [
        "depositCount()",
        "depositRoot()",
        "globalDepositsFrozen()",
        "globalReleasesFrozen()",
        "globalCapState()",
        "minReleaseDelay()",
        "cfgId()",
        "admin()",
        "freezer()",
        "feeRecipient()",
        "pendingAdmin()",
        "updateImageId()",
        "exitCount()",
    ]
    .iter()
    .map(|s| (s.to_string(), calldata(s, &[])))
    .collect();
    for (t, n) in tokens.iter().zip(denoms_per_token) {
        let a = word_address(t);
        for s in ["tokenConfig(address)", "capState(address)", "releaseDelayState(address)", "capWeight(address)"] {
            calls.push((format!("{s}@{}", addr_hex(t)), calldata(s, &[a])));
        }
        for i in 0..*n {
            calls.push((format!("relayFeeMenu(address,uint32)@{}#{i}", addr_hex(t)), calldata("relayFeeMenu(address,uint32)", &[a, word_u64(i as u64)])));
            calls.push((format!("counters(address,uint256)@{}#{i}", addr_hex(t)), calldata("counters(address,uint256)", &[a, word_u64(i as u64)])));
        }
    }
    calls
}

/// The vault's state for the registry view. `tokens` are the bridge's registered tokens and how
/// many denominations each has (from the LEZ config), so the relay-fee menus can be read.
pub fn read(eth: &dyn EthSource, vault: &[u8; 20], tokens: &[[u8; 20]], denoms_per_token: &[usize]) -> Result<Value, String> {
    let calls = registry_calls(tokens, denoms_per_token);
    let data: Vec<Vec<u8>> = calls.iter().map(|(_, d)| d.clone()).collect();
    let answers = eth.calls(vault, &data)?;
    let get = |key: &str| -> Result<Vec<u8>, String> {
        let i = calls.iter().position(|(k, _)| k == key).ok_or_else(|| format!("no call {key}"))?;
        answers[i].clone().map_err(|e| format!("the vault's {}: {e}", key.split('@').next().unwrap_or(key)))
    };
    let g = |key: &str| get(key);
    let count = Words(&g("depositCount()")?).u64(0)?;
    let global_cap = g("globalCapState()")?;
    let gc = Words(&global_cap);
    let update_image = Words(&g("updateImageId()")?).word(0)?;
    let mut out_tokens = Vec::new();
    for (t, n) in tokens.iter().zip(denoms_per_token) {
        let a = addr_hex(t);
        let cfg = get(&format!("tokenConfig(address)@{a}"))?;
        let c = Words(&cfg);
        let cap = get(&format!("capState(address)@{a}"))?;
        let cs = Words(&cap);
        let delay = get(&format!("releaseDelayState(address)@{a}"))?;
        let ds = Words(&delay);
        let weight = Words(&get(&format!("capWeight(address)@{a}"))?).dec(0)?;
        let mut menus = Vec::new();
        let mut counters = Vec::new();
        for i in 0..*n {
            let m = get(&format!("relayFeeMenu(address,uint32)@{a}#{i}"))?;
            // A bare `uint256[]` return: head word 0 is the offset.
            menus.push(Value::from(Words(&m).dec_array(0)?));
            let k = get(&format!("counters(address,uint256)@{a}#{i}"))?;
            let kw = Words(&k);
            counters.push(json!({ "deposits": kw.u64(0)?, "claims": kw.u64(1)?, "burns": kw.u64(2)?, "releases": kw.u64(3)? }));
        }
        out_tokens.push(json!({
            "eth_token": a,
            "registered": c.boolean(0)?,
            "deposits_frozen": c.boolean(1)?,
            "releases_frozen": c.boolean(2)?,
            "decimals": c.u64(3)?,
            "release_delay_s": c.u64(4)?,
            "denoms": c.dec_array(5)?,
            "deposit_fees": c.dec_array(6)?,
            "withdraw_fees": c.dec_array(7)?,
            "accrued_fees": c.dec(8)?,
            "cap": { "window_s": cs.u64(0)?, "deposit_cap": cs.dec(1)?, "release_cap": cs.dec(2)?, "deposit_used": cs.dec(3)?, "release_used": cs.dec(4)? },
            "delay": { "delay_s": ds.u64(0)?, "changed_at": ds.u64(1)?, "steps": ds.u64(2)? },
            "cap_weight": weight,
            "relay_fee_menu": menus,
            "counters": counters,
        }));
    }
    Ok(json!({
        "vault": addr_hex(vault),
        "deposit_count": count,
        "deposit_root": hex0x(&Words(&g("depositRoot()")?).word(0)?),
        "exit_count": Words(&g("exitCount()")?).u64(0)?,
        "deposits_frozen": Words(&g("globalDepositsFrozen()")?).boolean(0)?,
        "releases_frozen": Words(&g("globalReleasesFrozen()")?).boolean(0)?,
        "global_cap": { "window_s": gc.u64(0)?, "deposit_cap": gc.dec(1)?, "release_cap": gc.dec(2)?, "deposit_used": gc.dec(3)?, "release_used": gc.dec(4)? },
        "min_release_delay_s": Words(&g("minReleaseDelay()")?).u64(0)?,
        "cfg_id": hex0x(&Words(&g("cfgId()")?).word(0)?),
        "admin": checksum_address(&Words(&g("admin()")?).address(0)?),
        "freezer": checksum_address(&Words(&g("freezer()")?).address(0)?),
        "fee_recipient": checksum_address(&Words(&g("feeRecipient()")?).address(0)?),
        "pending_admin": checksum_address(&Words(&g("pendingAdmin()")?).address(0)?),
        "update_image_id": hex0x(&update_image),
        "exits_possible": update_image != demo_update_image(),
        "tokens": out_tokens,
    }))
}

/// Call data for a deposit: `deposit(token, denomIndex, C)`, or `depositETH(denomIndex, C)` for ETH.
pub fn deposit_calldata(token: &[u8; 20], denom_index: u32, commitment: &[u8; 32]) -> Vec<u8> {
    if token == &[0u8; 20] {
        calldata("depositETH(uint32,bytes32)", &[word_u64(denom_index as u64), *commitment])
    } else {
        calldata("deposit(address,uint32,bytes32)", &[word_address(token), word_u64(denom_index as u64), *commitment])
    }
}

/// Call data for a deposit through the deposit desk (`vault/src/desk/DepositDesk.sol`, the bridge
/// fee): `depositWithFee(token, denomIndex, C, lezPublicKey)`, or `depositETHWithFee(denomIndex, C,
/// lezPublicKey)` for ETH. `lezPublicKey` is always zero, as the page sends it (a public claim's fee
/// is paid by the relay; the desk names no account).
pub fn desk_deposit_calldata(token: &[u8; 20], denom_index: u32, commitment: &[u8; 32]) -> Vec<u8> {
    if token == &[0u8; 20] {
        calldata("depositETHWithFee(uint32,bytes32,bytes32)", &[word_u64(denom_index as u64), *commitment, [0; 32]])
    } else {
        calldata("depositWithFee(address,uint32,bytes32,bytes32)", &[word_address(token), word_u64(denom_index as u64), *commitment, [0; 32]])
    }
}

/// `quote(token, denomIndex)` on the desk: answers `(denom, vaultFee, bridgeFee, total)`.
pub fn desk_quote_calldata(token: &[u8; 20], denom_index: u32) -> Vec<u8> {
    calldata("quote(address,uint32)", &[word_address(token), word_u64(denom_index as u64)])
}

/// `approve(vault, amount)` on an ERC-20.
pub fn approve_calldata(vault: &[u8; 20], amount: u128) -> Vec<u8> {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&amount.to_be_bytes());
    calldata("approve(address,uint256)", &[word_address(vault), w])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::to_hex;

    #[test]
    fn deposit_calldata_has_the_vaults_selectors() {
        let c = [7u8; 32];
        let eth = deposit_calldata(&[0; 20], 2, &c);
        assert_eq!(to_hex(&eth[..4]), to_hex(&crate::util::selector("depositETH(uint32,bytes32)")));
        assert_eq!(eth.len(), 4 + 64);
        let usdc = crate::util::eth_address("0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e").unwrap();
        let erc = deposit_calldata(&usdc, 0, &c);
        assert_eq!(erc.len(), 4 + 96);
        assert_eq!(&erc[4 + 12..4 + 32], &usdc);
    }

    /// The desk's selectors, as vault/test/DepositDesk.t.sol pins them.
    #[test]
    fn desk_calldata_has_the_desks_selectors() {
        let c = [7u8; 32];
        let usdc = crate::util::eth_address("0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e").unwrap();
        let eth = desk_deposit_calldata(&[0; 20], 2, &c);
        assert_eq!(to_hex(&eth[..4]), "03321b04");
        assert_eq!(eth.len(), 4 + 96);
        assert_eq!(&eth[4 + 64..], &[0u8; 32], "no LEZ key: always zero");
        let erc = desk_deposit_calldata(&usdc, 0, &c);
        assert_eq!(to_hex(&erc[..4]), "a93bf283");
        assert_eq!(erc.len(), 4 + 128);
        assert_eq!(to_hex(&desk_quote_calldata(&usdc, 1)[..4]), "ea48dc6c");
    }
}
