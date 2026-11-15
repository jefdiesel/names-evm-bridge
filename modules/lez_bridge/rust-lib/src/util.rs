//! Small shared pieces: hex, hashes, amounts as decimal text, private files.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use tiny_keccak::{Hasher, Keccak};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn hex0x(bytes: &[u8]) -> String {
    format!("0x{}", to_hex(bytes))
}

pub fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    let t = text.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    if t.len() % 2 != 0 {
        return Err(format!("odd-length hex: {text}"));
    }
    (0..t.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&t[i..i + 2], 16).map_err(|_| format!("not hex: {text}")))
        .collect()
}

pub fn hex32(text: &str) -> Result<[u8; 32], String> {
    hex_bytes(text)?.try_into().map_err(|_| format!("expected 32 bytes of hex: {text}"))
}

pub fn keccak(data: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// The 4-byte selector of a Solidity signature such as `depositCount()`.
pub fn selector(signature: &str) -> [u8; 4] {
    let h = keccak(signature.as_bytes());
    [h[0], h[1], h[2], h[3]]
}

/// An Ethereum address from text (0x + 40 hex), lower-cased bytes.
pub fn eth_address(text: &str) -> Result<[u8; 20], String> {
    let t = text.trim();
    if !(t.starts_with("0x") || t.starts_with("0X")) || t.len() != 42 {
        return Err(format!("{t} is not an Ethereum address (0x and 40 hex digits)"));
    }
    hex_bytes(t)?.try_into().map_err(|_| format!("{t} is not an Ethereum address"))
}

/// EIP-55 mixed-case spelling of an address.
pub fn checksum_address(address: &[u8; 20]) -> String {
    let lower = to_hex(address);
    let hash = keccak(lower.as_bytes());
    let mut out = String::from("0x");
    for (i, c) in lower.chars().enumerate() {
        let nibble = (hash[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0xf;
        if c.is_ascii_alphabetic() && nibble >= 8 {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether mixed-case text fails its EIP-55 checksum (all-lower or all-upper text has none).
pub fn bad_checksum(text: &str) -> bool {
    let body = text.trim().trim_start_matches("0x").trim_start_matches("0X");
    let mixed = body.chars().any(|c| c.is_ascii_lowercase()) && body.chars().any(|c| c.is_ascii_uppercase());
    match eth_address(text) {
        Ok(a) => mixed && checksum_address(&a) != format!("0x{body}"),
        Err(_) => true,
    }
}

/// A LEZ account id: base58 of 32 bytes.
pub fn lez_account(text: &str) -> Result<[u8; 32], String> {
    let t = text.trim().trim_start_matches("Private/").trim_start_matches("Public/");
    let bytes = bs58::decode(t).into_vec().map_err(|_| format!("{t} is not a LEZ account id (base58)"))?;
    bytes.try_into().map_err(|_| format!("{t} is not a LEZ account id (32 bytes)"))
}

/// A whole number of base units, from decimal text (`"100000000"`).
pub fn units(text: &str) -> Result<u128, String> {
    let t = text.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("not a whole number: {text}"));
    }
    t.parse::<u128>().map_err(|_| format!("too large: {text}"))
}

/// A human amount (`"1,300.5"`, `"0.01"`) in base units of a token with `decimals`.
pub fn parse_amount(text: &str, decimals: u8) -> Result<u128, String> {
    let t: String = text.trim().chars().filter(|c| *c != ',' && *c != '_' && *c != ' ').collect();
    if t.is_empty() {
        return Err("enter an amount".to_owned());
    }
    let (whole, frac) = t.split_once('.').unwrap_or((&t, ""));
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) || (whole.is_empty() && frac.is_empty()) {
        return Err(format!("\"{text}\" is not an amount"));
    }
    if frac.len() > decimals as usize {
        return Err(format!("this token has {decimals} decimals; \"{text}\" has more"));
    }
    let scale = 10u128.checked_pow(decimals as u32).ok_or("too many decimals")?;
    let w: u128 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| format!("\"{text}\" is too large"))? };
    let mut f: u128 = if frac.is_empty() { 0 } else { frac.parse().map_err(|_| "bad fraction".to_owned())? };
    f *= 10u128.pow((decimals as usize - frac.len()) as u32);
    w.checked_mul(scale).and_then(|x| x.checked_add(f)).ok_or_else(|| format!("\"{text}\" is too large"))
}

/// Base units as a human amount: `format_amount(100000000, 6)` = `"100"`, with thousands separators.
pub fn format_amount(value: u128, decimals: u8) -> String {
    let scale = 10u128.pow(decimals as u32);
    let whole = value / scale;
    let frac = value % scale;
    let mut w = group(&whole.to_string());
    if frac > 0 {
        let f = format!("{frac:0width$}", width = decimals as usize);
        w.push('.');
        w.push_str(f.trim_end_matches('0'));
    }
    w
}

fn group(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A span of seconds as people say it: "40 s", "11 min", "3 h", "2 days".
pub fn duration_text(secs: u64) -> String {
    match secs {
        0..=89 => format!("{} s", secs.max(1)),
        90..=5399 => format!("{} min", (secs + 30) / 60),
        5400..=172_799 => format!("{} h", (secs + 1800) / 3600),
        _ => format!("{} days", (secs + 43_200) / 86_400),
    }
}

/// `n` and a noun, singular or plural: "1 deposit", "3 deposits".
pub fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Writes `bytes` to `path` with mode 0600, through a temporary file and a rename.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        private_dir(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
}

pub fn random32() -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    getrandom::getrandom(&mut out).map_err(|e| format!("no randomness from the OS: {e}"))?;
    Ok(out)
}

pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_round_trip_with_decimals() {
        assert_eq!(parse_amount("100", 6).unwrap(), 100_000_000);
        assert_eq!(parse_amount("1,300.5", 6).unwrap(), 1_300_500_000);
        assert_eq!(parse_amount("0.01", 18).unwrap(), 10_000_000_000_000_000);
        assert!(parse_amount("0.0000001", 6).is_err());
        assert!(parse_amount("1e6", 6).is_err());
        assert!(parse_amount("", 6).is_err());
        assert_eq!(format_amount(1_300_500_000, 6), "1,300.5");
        assert_eq!(format_amount(10_000_000_000_000_000, 18), "0.01");
        assert_eq!(format_amount(0, 18), "0");
    }

    #[test]
    fn selectors_and_checksums_match_known_values() {
        // ERC-20 transfer(address,uint256) is a9059cbb everywhere.
        assert_eq!(to_hex(&selector("transfer(address,uint256)")), "a9059cbb");
        // EIP-55's own test vector.
        let a = eth_address("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap();
        assert_eq!(checksum_address(&a), "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed");
        assert!(!bad_checksum("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"));
        assert!(bad_checksum("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD"));
        assert!(!bad_checksum("0x8c8b2664de28e5287158db4db65d448638871b74"));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_text(40), "40 s");
        assert_eq!(duration_text(671), "11 min");
        assert_eq!(duration_text(86_400), "24 h");
        assert_eq!(duration_text(3 * 86_400), "3 days");
    }
}
