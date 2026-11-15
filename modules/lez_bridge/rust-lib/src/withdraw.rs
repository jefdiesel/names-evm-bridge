// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two "Send back to Ethereum" codes (`modules/NAMES-SEND-BACK.md` §3.1, §3.2), the same
//! bytes as the bridge page's `web/deposit/src/lib/withdraw.js`; both are checked against
//! `web/deposit/test/vectors/withdraw-v1.json` (generator `modules/tools/make-withdraw-vectors.py` (not published)).
//!
//! Withdraw-to code (the page makes it from the connected Ethereum wallet; this module reads it):
//!   `version(1)=1 | chainId(8, BE) | vault(20) | recipient(20) | relayFeeChoice(1)`, bech32m "logoswd";
//!   its four words are the compare words of `SHA-256("logos-bridge/withdraw-to/v1" | payload)`.
//! Release code (this module makes it; the page sends `release`):
//!   `version | chainId(8) | vault(20) | root(32) | nullifier(32) | token(20) | denomIndex(4, BE)
//!   | recipient(20) | relayer(20) | relayFee(32) | seal(260)`, bech32m "logosrel".
//! Both hold public facts only.

use crate::request::bech32m;
use crate::util::sha256;

pub const WD_HRP: &str = "logoswd";
pub const REL_HRP: &str = "logosrel";
pub const TAG: &[u8] = b"logos-bridge/withdraw-to/v1";
pub const SELECTOR: [u8; 4] = [0xe7, 0x1e, 0x66, 0x20];
pub const MAX_FEE_CHOICE: u8 = 4;
pub const SEAL_LEN: usize = 260;
const WD_LEN: usize = 50;
const REL_LEN: usize = 1 + 8 + 20 + 32 + 32 + 20 + 4 + 20 + 20 + 32 + SEAL_LEN;
const CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const CONST: u32 = 0x2bc8_30a3;
const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];

/// A refusal with a stable code (the same codes as the page and the vectors).
#[derive(Debug, Clone, PartialEq)]
pub struct CodeError {
    pub code: &'static str,
    pub message: String,
}

fn err<T>(code: &'static str, message: impl Into<String>) -> Result<T, CodeError> {
    Err(CodeError { code, message: message.into() })
}

fn polymod(values: &[u8]) -> u32 {
    let mut chk: u32 = 1;
    for v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(*v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &str) -> Vec<u8> {
    let mut v: Vec<u8> = hrp.bytes().map(|c| c >> 5).collect();
    v.push(0);
    v.extend(hrp.bytes().map(|c| c & 31));
    v
}

/// Strict bech32m decode: one case, no spaces, a valid checksum, zero padding bits.
pub fn bech32m_decode(text: &str, max: usize) -> Result<(String, Vec<u8>), CodeError> {
    if text.len() > max {
        return err("length", "The code is too long.");
    }
    let lower = text.to_ascii_lowercase();
    if text != lower && text != text.to_ascii_uppercase() {
        return err("case", "The code mixes upper and lower case letters.");
    }
    let sep = lower.rfind('1').filter(|s| *s >= 1 && s + 7 <= lower.len()).ok_or(CodeError { code: "format", message: "This is not a code from the bridge.".into() })?;
    let hrp = &lower[..sep];
    let mut data = Vec::new();
    for c in lower[sep + 1..].bytes() {
        data.push(CHARSET.iter().position(|x| *x == c).ok_or(CodeError { code: "charset", message: format!("The code contains \"{}\", which no code can contain.", c as char) })? as u8);
    }
    let mut all = hrp_expand(hrp);
    all.extend(&data);
    if polymod(&all) != CONST {
        return err("checksum", "The code's checksum does not match: it was mistyped or cut short. Copy it again.");
    }
    let words = &data[..data.len() - 6];
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for w in words {
        acc = (acc << 5) | u32::from(*w);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 5 || acc != 0 {
        return err("padding", "The code has invalid padding bits.");
    }
    Ok((hrp.to_owned(), out))
}

/// What a person pasted: whitespace a chat app adds removed, a link's fragment taken.
pub fn from_input(text: &str) -> String {
    let s: String = text.chars().filter(|c| !c.is_whitespace() && !matches!(*c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}')).collect();
    match s.rfind('#') {
        Some(i) => s[i + 1..].to_owned(),
        None => s,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawTo {
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub recipient: [u8; 20],
    pub fee_choice: u8,
}

impl WithdrawTo {
    pub fn payload(&self) -> Vec<u8> {
        let mut p = vec![1u8];
        p.extend(self.chain_id.to_be_bytes());
        p.extend(self.vault);
        p.extend(self.recipient);
        p.push(self.fee_choice);
        p
    }

    pub fn encode(&self) -> String {
        bech32m(WD_HRP, &self.payload())
    }

    /// The 32 bytes the four compare words are taken over.
    pub fn digest(&self) -> [u8; 32] {
        sha256(&[TAG, &self.payload()])
    }
}

pub fn decode_withdraw_to(text: &str) -> Result<WithdrawTo, CodeError> {
    let (hrp, b) = bech32m_decode(text, 120)?;
    if hrp != WD_HRP {
        return err("hrp", format!("This is a \"{hrp}\" code, not a withdraw-to code."));
    }
    if b.first() != Some(&1) {
        return err("version", "This code is of a version this wallet does not read.");
    }
    if b.len() != WD_LEN {
        return err("length", "The code has the wrong length.");
    }
    let w = WithdrawTo {
        chain_id: u64::from_be_bytes(b[1..9].try_into().unwrap()),
        vault: b[9..29].try_into().unwrap(),
        recipient: b[29..49].try_into().unwrap(),
        fee_choice: b[49],
    };
    if w.chain_id == 0 {
        return err("chain", "The code names no chain.");
    }
    if w.vault == [0; 20] {
        return err("vault", "The code names no vault.");
    }
    if w.recipient == [0; 20] || w.recipient == w.vault {
        return err("recipient", "The zero address and the vault itself can't receive a release.");
    }
    if w.fee_choice > MAX_FEE_CHOICE {
        return err("fee-choice", "The relay-fee choice must be 0 to 4.");
    }
    Ok(w)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseCode {
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub root: [u8; 32],
    pub nullifier: [u8; 32],
    pub token: [u8; 20],
    pub denom_index: u32,
    pub recipient: [u8; 20],
    pub relayer: [u8; 20],
    pub relay_fee: u128,
    pub seal: Vec<u8>,
}

impl ReleaseCode {
    pub fn payload(&self) -> Vec<u8> {
        let mut p = vec![1u8];
        p.extend(self.chain_id.to_be_bytes());
        p.extend(self.vault);
        p.extend(self.root);
        p.extend(self.nullifier);
        p.extend(self.token);
        p.extend(self.denom_index.to_be_bytes());
        p.extend(self.recipient);
        p.extend(self.relayer);
        p.extend(crate::wallet::u256(self.relay_fee));
        p.extend(&self.seal);
        p
    }

    pub fn encode(&self) -> Result<String, CodeError> {
        if self.seal.len() != SEAL_LEN || self.seal[..4] != SELECTOR {
            return err("seal", "The proof is not a 260-byte seal for RISC Zero's Groth16 verifier 3.1.0.");
        }
        Ok(bech32m(REL_HRP, &self.payload()))
    }
}

pub fn decode_release(text: &str) -> Result<ReleaseCode, CodeError> {
    let (hrp, b) = bech32m_decode(text, 800)?;
    if hrp != REL_HRP {
        return err("hrp", format!("This is a \"{hrp}\" code, not a release code."));
    }
    if b.first() != Some(&1) {
        return err("version", "This code is of a version this wallet does not read.");
    }
    if b.len() != REL_LEN {
        return err("length", "The release code has the wrong length.");
    }
    let fee = &b[157..189];
    if fee[..16].iter().any(|x| *x != 0) {
        return err("field", "The relay fee is too large.");
    }
    let r = ReleaseCode {
        chain_id: u64::from_be_bytes(b[1..9].try_into().unwrap()),
        vault: b[9..29].try_into().unwrap(),
        root: b[29..61].try_into().unwrap(),
        nullifier: b[61..93].try_into().unwrap(),
        token: b[93..113].try_into().unwrap(),
        denom_index: u32::from_be_bytes(b[113..117].try_into().unwrap()),
        recipient: b[117..137].try_into().unwrap(),
        relayer: b[137..157].try_into().unwrap(),
        relay_fee: u128::from_be_bytes(fee[16..].try_into().unwrap()),
        seal: b[189..].to_vec(),
    };
    if r.chain_id == 0 {
        return err("chain", "The code names no chain.");
    }
    if r.vault == [0; 20] {
        return err("vault", "The code names no vault.");
    }
    if r.recipient == [0; 20] || r.recipient == r.vault {
        return err("recipient", "This release has no usable recipient.");
    }
    if r.relay_fee != 0 && (r.relayer == [0; 20] || r.relayer == r.vault) {
        return err("relayer", "A relay fee needs a relayer.");
    }
    if r.seal[..4] != SELECTOR {
        return err("seal", "The proof is not for RISC Zero's Groth16 verifier 3.1.0.");
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{eth_address, hex32, hex_bytes, to_hex};
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../../../web/deposit/test/vectors/withdraw-v1.json");

    #[test]
    fn the_withdraw_to_vectors_read_and_write_the_same_bytes_and_words() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["withdrawTo"]["valid"].as_array().unwrap() {
            let f = &c["fields"];
            let w = decode_withdraw_to(c["string"].as_str().unwrap()).unwrap();
            assert_eq!(w.chain_id.to_string(), f["chainId"].as_str().unwrap());
            assert_eq!(w.vault, eth_address(f["vault"].as_str().unwrap()).unwrap());
            assert_eq!(w.recipient, eth_address(f["recipient"].as_str().unwrap()).unwrap());
            assert_eq!(u64::from(w.fee_choice), f["feeChoice"].as_u64().unwrap());
            assert_eq!(to_hex(&w.payload()), c["payloadHex"].as_str().unwrap());
            assert_eq!(w.encode(), c["string"].as_str().unwrap());
            assert_eq!(w.digest(), hex32(c["digest"].as_str().unwrap()).unwrap());
            assert_eq!(crate::compare::words_text(&w.digest()), c["words_text"].as_str().unwrap(), "{}", c["name"]);
            assert_eq!(decode_withdraw_to(&c["string"].as_str().unwrap().to_ascii_uppercase()).unwrap(), w);
        }
        for c in v["withdrawTo"]["invalid"].as_array().unwrap() {
            let e = decode_withdraw_to(c["string"].as_str().unwrap()).unwrap_err();
            assert_eq!(e.code, c["code"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn the_release_vectors_round_trip_and_bad_ones_are_refused() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["release"]["valid"].as_array().unwrap() {
            let r = decode_release(c["string"].as_str().unwrap()).unwrap();
            let f = &c["fields"];
            assert_eq!(r.root, hex32(f["root"].as_str().unwrap()).unwrap());
            assert_eq!(r.nullifier, hex32(f["nullifier"].as_str().unwrap()).unwrap());
            assert_eq!(r.denom_index as u64, f["denomIndex"].as_u64().unwrap());
            assert_eq!(r.relay_fee.to_string(), f["relayFee"].as_str().unwrap());
            assert_eq!(r.seal, hex_bytes(f["seal"].as_str().unwrap()).unwrap());
            assert_eq!(r.encode().unwrap(), c["string"].as_str().unwrap());
        }
        for c in v["release"]["invalid"].as_array().unwrap() {
            let e = decode_release(c["string"].as_str().unwrap()).unwrap_err();
            assert_eq!(e.code, c["code"].as_str().unwrap(), "{}", c["name"]);
        }
    }

    #[test]
    fn links_and_chat_whitespace_are_read() {
        assert_eq!(from_input(" https://nameswallet.app/bridge/#logoswd1abc\n"), "logoswd1abc");
        assert_eq!(from_input("logoswd1a b\u{200b}c"), "logoswd1abc");
    }
}
