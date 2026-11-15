//! The deposit request a Logos wallet hands to an Ethereum wallet: format v1 of
//! `web/deposit/src/lib/request.js` (bech32m, BIP-350, human-readable part `logosdep`), ported
//! byte for byte and checked against its vectors (`tests/vectors/request-v1.json`, a copy of
//! `web/deposit/test/vectors/request-v1.json`).
//!
//! Payload: `version(1) | chainId(u64 BE) | vault(20) | token(20) | denomIndex(1) |
//! amountLen(1) | amount(amountLen, minimal BE) | commitment(32)`. It carries only public facts:
//! never the deposit secret, the index or the LEZ destination.

use bridge_core::EthAddress;

pub const HRP: &str = "logosdep";
pub const VERSION: u8 = 1;
/// Longest request string accepted.
pub const MAX_LENGTH: usize = 200;

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const BECH32M_CONST: u32 = 0x2bc8_30a3;
const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];

/// A v1 deposit request. `amount` is the denomination in the token's smallest unit as minimal
/// big-endian bytes (v1 allows up to 2^256 - 1; [`DepositRequest::amount_u128`] for LEZ use).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositRequest {
    pub chain_id: u64,
    pub vault: EthAddress,
    pub token: EthAddress,
    pub denom_index: u8,
    pub amount: Vec<u8>,
    pub commitment: [u8; 32],
}

/// Why a request string or its fields are refused (codes as in `request.js`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestError(pub &'static str);

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "deposit request: {}", self.0)
    }
}

impl DepositRequest {
    #[must_use]
    pub fn new(chain_id: u64, vault: EthAddress, token: EthAddress, denom_index: u8, amount: u128, commitment: [u8; 32]) -> Self {
        let bytes = amount.to_be_bytes();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
        Self { chain_id, vault, token, denom_index, amount: bytes[first..].to_vec(), commitment }
    }

    /// The amount, if it fits 128 bits.
    #[must_use]
    pub fn amount_u128(&self) -> Option<u128> {
        if self.amount.len() > 16 {
            return None;
        }
        Some(self.amount.iter().fold(0u128, |acc, b| (acc << 8) | u128::from(*b)))
    }

    /// # Errors
    /// A field outside v1's rules.
    pub fn check(&self) -> Result<(), RequestError> {
        if self.chain_id == 0 {
            return Err(RequestError("chain"));
        }
        if self.vault == [0; 20] {
            return Err(RequestError("vault"));
        }
        if self.amount.is_empty() || self.amount.len() > 32 || self.amount[0] == 0 {
            return Err(RequestError("amount"));
        }
        if self.commitment == [0; 32] {
            return Err(RequestError("commitment"));
        }
        Ok(())
    }

    /// The v1 payload.
    ///
    /// # Errors
    /// See [`DepositRequest::check`].
    pub fn to_bytes(&self) -> Result<Vec<u8>, RequestError> {
        self.check()?;
        let mut out = vec![VERSION];
        out.extend_from_slice(&self.chain_id.to_be_bytes());
        out.extend_from_slice(&self.vault);
        out.extend_from_slice(&self.token);
        out.push(self.denom_index);
        out.push(u8::try_from(self.amount.len()).map_err(|_| RequestError("amount"))?);
        out.extend_from_slice(&self.amount);
        out.extend_from_slice(&self.commitment);
        Ok(out)
    }

    /// # Errors
    /// A malformed payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RequestError> {
        if bytes.is_empty() {
            return Err(RequestError("length"));
        }
        if bytes[0] != VERSION {
            return Err(RequestError("version"));
        }
        if bytes.len() < 51 {
            return Err(RequestError("length"));
        }
        let chain_id = u64::from_be_bytes(bytes[1..9].try_into().map_err(|_| RequestError("length"))?);
        let vault: EthAddress = bytes[9..29].try_into().map_err(|_| RequestError("length"))?;
        let token: EthAddress = bytes[29..49].try_into().map_err(|_| RequestError("length"))?;
        let denom_index = bytes[49];
        let len = usize::from(bytes[50]);
        if !(1..=32).contains(&len) {
            return Err(RequestError("amount"));
        }
        match bytes.len().cmp(&(51 + len + 32)) {
            core::cmp::Ordering::Less => return Err(RequestError("length")),
            core::cmp::Ordering::Greater => return Err(RequestError("trailing")),
            core::cmp::Ordering::Equal => {}
        }
        let amount = bytes[51..51 + len].to_vec();
        if amount[0] == 0 {
            return Err(RequestError("amount"));
        }
        let commitment: [u8; 32] = bytes[51 + len..].try_into().map_err(|_| RequestError("length"))?;
        let r = Self { chain_id, vault, token, denom_index, amount, commitment };
        r.check()?;
        Ok(r)
    }

    /// The one canonical string (lowercase bech32m).
    ///
    /// # Errors
    /// See [`DepositRequest::check`].
    pub fn encode(&self) -> Result<String, RequestError> {
        Ok(bech32m_encode(HRP, &self.to_bytes()?))
    }

    /// Strict decode: one case, no whitespace, valid checksum, hrp `logosdep`.
    ///
    /// # Errors
    /// Anything `request.js`'s `decodeRequest` refuses.
    pub fn decode(text: &str) -> Result<Self, RequestError> {
        let (hrp, bytes) = bech32m_decode(text)?;
        if hrp != HRP {
            return Err(RequestError("hrp"));
        }
        Self::from_bytes(&bytes)
    }

    /// The JSON form of the vectors (`chainId` and `amount` as decimal strings when they fit).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "version": VERSION,
            "chainId": self.chain_id.to_string(),
            "vault": format!("0x{}", hex(&self.vault)),
            "token": format!("0x{}", hex(&self.token)),
            "denomIndex": self.denom_index,
            "amount": self.amount_u128().map_or_else(|| format!("0x{}", hex(&self.amount)), |a| a.to_string()),
            "commitment": format!("0x{}", hex(&self.commitment)),
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn polymod(values: impl IntoIterator<Item = u8>) -> u32 {
    let mut chk: u32 = 1;
    for v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(v);
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
    b.iter().map(|c| c >> 5).chain([0]).chain(b.iter().map(|c| c & 31)).collect()
}

fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Result<Vec<u8>, RequestError> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let maxv: u32 = (1 << to) - 1;
    let mut out = Vec::new();
    for v in data {
        acc = (acc << from) | u32::from(*v);
        bits += from;
        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & maxv) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if pad {
        if bits > 0 {
            out.push(((acc << (to - bits)) & maxv) as u8);
        }
    } else if bits >= from || ((acc << (to - bits)) & maxv) != 0 {
        return Err(RequestError("padding"));
    }
    Ok(out)
}

/// bech32m-encode `bytes` under `hrp`, lowercase.
#[must_use]
pub fn bech32m_encode(hrp: &str, bytes: &[u8]) -> String {
    let data = convert_bits(bytes, 8, 5, true).unwrap_or_else(|_| unreachable!("padding on"));
    let m = polymod(hrp_expand(hrp).into_iter().chain(data.iter().copied()).chain([0; 6])) ^ BECH32M_CONST;
    let mut s = String::with_capacity(hrp.len() + 1 + data.len() + 6);
    s.push_str(hrp);
    s.push('1');
    for d in &data {
        s.push(CHARSET[usize::from(*d)] as char);
    }
    for i in 0..6 {
        s.push(CHARSET[((m >> (5 * (5 - i))) & 31) as usize] as char);
    }
    s
}

/// Strict bech32m decode to (hrp, bytes).
///
/// # Errors
/// Length, characters, mixed case, separator, checksum or padding.
pub fn bech32m_decode(text: &str) -> Result<(String, Vec<u8>), RequestError> {
    if text.len() > MAX_LENGTH {
        return Err(RequestError("length"));
    }
    if text.bytes().any(|c| !(33..=126).contains(&c)) {
        return Err(RequestError("charset"));
    }
    let lower = text.to_ascii_lowercase();
    if text != lower && text != text.to_ascii_uppercase() {
        return Err(RequestError("case"));
    }
    let sep = lower.rfind('1').ok_or(RequestError("format"))?;
    if sep < 1 || sep + 7 > lower.len() {
        return Err(RequestError("format"));
    }
    let hrp = &lower[..sep];
    let data: Vec<u8> = lower[sep + 1..]
        .bytes()
        .map(|c| CHARSET.iter().position(|x| *x == c).map(|p| p as u8).ok_or(RequestError("charset")))
        .collect::<Result<_, _>>()?;
    if polymod(hrp_expand(hrp).into_iter().chain(data.iter().copied())) != BECH32M_CONST {
        return Err(RequestError("checksum"));
    }
    let bytes = convert_bits(&data[..data.len() - 6], 5, 8, false)?;
    Ok((hrp.to_owned(), bytes))
}
