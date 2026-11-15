// SPDX-License-Identifier: MIT OR Apache-2.0
//! The deposit page's deposit request, format v1 (`web/deposit/src/lib/request.js`, spec in
//! `web/deposit/README.md` "Deposit request format, v1"): the code a wallet hands an Ethereum
//! wallet so it can deposit behind a commitment without ever seeing the secret or the account.
//!
//! payload = version(1)=1 | chainId(8, big-endian) | vault(20) | token(20; zero = ETH) |
//!           denomIndex(1) | amountLen(1) | amount(minimal big-endian) | commitment(32)
//! string  = bech32m("logosdep", payload), lowercase
//!
//! Encoding only: the module makes codes, the page reads them. The test below checks every valid
//! vector of the page's own `test/vectors/request-v1.json`, so the two cannot drift.

pub const HRP: &str = "logosdep";
pub const VERSION: u8 = 1;
/// The deposit page; a code goes in the fragment (never the query).
pub const PAGE_URL: &str = "https://nameswallet.app/bridge/";

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const BECH32M_CONST: u32 = 0x2bc8_30a3;
const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];

fn polymod(values: &[u8]) -> u32 {
    let mut chk: u32 = 1;
    for &v in values {
        let top = chk >> 25;
        chk = ((chk & 0x1ff_ffff) << 5) ^ u32::from(v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &str) -> Vec<u8> {
    let b = hrp.as_bytes();
    let mut out: Vec<u8> = b.iter().map(|c| c >> 5).collect();
    out.push(0);
    out.extend(b.iter().map(|c| c & 31));
    out
}

fn to_words(bytes: &[u8]) -> Vec<u8> {
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 31) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 31) as u8);
    }
    out
}

/// bech32m-encode `bytes` under `hrp`, lowercase.
pub fn bech32m(hrp: &str, bytes: &[u8]) -> String {
    let data = to_words(bytes);
    let mut check_in = hrp_expand(hrp);
    check_in.extend(&data);
    check_in.extend([0u8; 6]);
    let m = polymod(&check_in) ^ BECH32M_CONST;
    let mut s = String::with_capacity(hrp.len() + 1 + data.len() + 6);
    s.push_str(hrp);
    s.push('1');
    for d in data.iter().copied().chain((0..6).map(|i| ((m >> (5 * (5 - i))) & 31) as u8)) {
        s.push(CHARSET[d as usize] as char);
    }
    s
}

/// The v1 payload. `amount` is big-endian (leading zeros are dropped; it must not be zero).
pub fn payload(chain_id: u64, vault: &[u8; 20], token: &[u8; 20], denom_index: u8, amount: &[u8], commitment: &[u8; 32]) -> Result<Vec<u8>, String> {
    let amt: Vec<u8> = amount.iter().copied().skip_while(|b| *b == 0).collect();
    if amt.is_empty() || amt.len() > 32 {
        return Err("a deposit request's amount is 1 to 2^256-1".to_owned());
    }
    if chain_id == 0 || vault == &[0; 20] || commitment == &[0; 32] {
        return Err("a deposit request needs a chain id, a vault and a commitment".to_owned());
    }
    let mut out = vec![VERSION];
    out.extend(chain_id.to_be_bytes());
    out.extend(vault);
    out.extend(token);
    out.push(denom_index);
    out.push(amt.len() as u8);
    out.extend(&amt);
    out.extend(commitment);
    Ok(out)
}

/// The request string (`logosdep1…`).
pub fn encode(chain_id: u64, vault: &[u8; 20], token: &[u8; 20], denom_index: u8, amount: u128, commitment: &[u8; 32]) -> Result<String, String> {
    Ok(bech32m(HRP, &payload(chain_id, vault, token, denom_index, &amount.to_be_bytes(), commitment)?))
}

/// The page link for a request: `<page>#<request>`.
pub fn url(page: &str, request: &str) -> String {
    format!("{}#{request}", page.split('#').next().unwrap_or(page))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{eth_address, hex32, to_hex};
    use serde_json::Value;

    /// The page's own vectors (web/deposit/test/vectors/request-v1.json).
    const VECTORS: &str = include_str!("../../../../web/deposit/test/vectors/request-v1.json");

    /// Decimal text to big-endian bytes (the vectors reach 2^256-1).
    fn dec_bytes(s: &str) -> Vec<u8> {
        let mut out = vec![0u8];
        for ch in s.bytes() {
            let mut carry = u32::from(ch - b'0');
            for b in out.iter_mut().rev() {
                let v = u32::from(*b) * 10 + carry;
                *b = (v & 0xff) as u8;
                carry = v >> 8;
            }
            while carry > 0 {
                out.insert(0, (carry & 0xff) as u8);
                carry >>= 8;
            }
        }
        out
    }

    #[test]
    fn every_valid_page_vector_encodes_to_the_same_bytes_and_string() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(v["hrp"], HRP);
        let valid = v["valid"].as_array().unwrap();
        assert!(valid.len() >= 5);
        for case in valid {
            let r = &case["request"];
            let p = payload(
                r["chainId"].as_str().unwrap().parse().unwrap(),
                &eth_address(r["vault"].as_str().unwrap()).unwrap(),
                &eth_address(r["token"].as_str().unwrap()).unwrap(),
                r["denomIndex"].as_u64().unwrap() as u8,
                &dec_bytes(r["amount"].as_str().unwrap()),
                &hex32(r["commitment"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(to_hex(&p), case["payloadHex"].as_str().unwrap(), "{}", case["name"]);
            let s = bech32m(HRP, &p);
            assert_eq!(s, case["string"].as_str().unwrap(), "{}", case["name"]);
            assert_eq!(s.len() as u64, case["length"].as_u64().unwrap());
        }
        // The u128 front door gives the same string as the bytes path.
        let r = &valid[0]["request"];
        let s = encode(11_155_111, &eth_address(r["vault"].as_str().unwrap()).unwrap(), &[0; 20], 0, 10u128.pow(16), &hex32(r["commitment"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(s, valid[0]["string"].as_str().unwrap());
        assert_eq!(url(PAGE_URL, &s), format!("https://nameswallet.app/bridge/#{s}"));
        assert!(encode(11_155_111, &[1; 20], &[0; 20], 0, 0, &[2; 32]).is_err(), "zero amount");
    }
}
