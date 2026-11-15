// SPDX-License-Identifier: MIT OR Apache-2.0
//! Four words for comparing a deposit's commitment by eye: the wallet and the deposit page both
//! show them, and the person checks they match (beside the commitment's first and last 6 hex
//! digits, `compare.first` / `compare.last`).
//!
//! h       = SHA-256( ASCII "logos-bridge/compare/v1" | the 32 commitment bytes )
//! indexes = the first 44 bits of h, big-endian, as four 11-bit numbers
//!           (bits 0-10, 11-21, 22-32, 33-43)
//! words   = the BIP-39 English word at each index (index 0 = "abandon")
//! text    = the words joined with " · " (space, U+00B7, space)
//!
//! The same derivation is in the deposit page (`web/deposit/src/lib/compare-words.js`); both are
//! checked against `web/deposit/test/vectors/compare-words-v1.json`, whose generator
//! (`modules/tools/make-compare-words-vectors.py` (not published)) uses neither.
//!
//! The word list, `data/bip39-english.txt`, is the BIP-39 English list unchanged (MIT, Pavol
//! Rusnak; `data/bip39-english.LICENSE`), embedded at build.

use sha2::{Digest, Sha256};

/// The domain tag hashed in front of the commitment.
pub const TAG: &str = "logos-bridge/compare/v1";
/// Between the words in `words_text`.
pub const SEPARATOR: &str = " \u{b7} ";
/// The BIP-39 English word list: 2048 lines, each ending in "\n".
const LIST: &str = include_str!("../data/bip39-english.txt");

/// The four 11-bit word indexes of a commitment.
pub fn indexes(commitment: &[u8; 32]) -> [u16; 4] {
    let mut s = Sha256::new();
    s.update(TAG.as_bytes());
    s.update(commitment);
    let h = s.finalize();
    // The first 48 bits of h as a number; its top 44 bits are the four indexes.
    let top = u64::from_be_bytes([0, 0, h[0], h[1], h[2], h[3], h[4], h[5]]) >> 4;
    [33u32, 22, 11, 0].map(|shift| ((top >> shift) & 0x7ff) as u16)
}

/// The BIP-39 English word at `index` (0..2048).
pub fn word(index: u16) -> &'static str {
    LIST.lines().nth(usize::from(index)).expect("the BIP-39 list has 2048 words")
}

/// The four words of a commitment.
pub fn words(commitment: &[u8; 32]) -> [&'static str; 4] {
    indexes(commitment).map(word)
}

/// The words joined for display: `cream · laugh · load · visual`.
pub fn words_text(commitment: &[u8; 32]) -> String {
    words(commitment).join(SEPARATOR)
}

/// The `compare` object of a reply: `{first, last, text, words, words_text}` (the commitment's
/// first and last 6 hex digits, the sentence the wallet shows, and the four words).
pub fn json(commitment: &[u8; 32]) -> serde_json::Value {
    let hex = crate::util::to_hex(commitment);
    let (first, last) = (&hex[..6], &hex[hex.len() - 6..]);
    serde_json::json!({
        "first": first, "last": last,
        "text": format!("Compare with the deposit page: its commitment starts {first} and ends {last}."),
        "words": words(commitment), "words_text": words_text(commitment),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex32;
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    /// The shared vectors, also read by the deposit page's test.
    const VECTORS: &str = include_str!("../../../../web/deposit/test/vectors/compare-words-v1.json");
    const LIST_SHA256: &str = "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda";

    #[test]
    fn the_embedded_list_is_the_canonical_bip39_english_list() {
        assert_eq!(crate::util::to_hex(&Sha256::digest(LIST.as_bytes())), LIST_SHA256);
        assert_eq!(LIST.lines().count(), 2048);
        assert!(LIST.lines().all(|w| (3..=8).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase())));
        assert_eq!((word(0), word(1), word(2047)), ("abandon", "ability", "zoo"));
    }

    #[test]
    fn every_shared_vector_gives_the_same_indexes_words_and_text() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(v["tag"], TAG);
        assert_eq!(v["separator"], SEPARATOR);
        assert_eq!(v["wordlist"]["sha256"], LIST_SHA256);
        let cases = v["vectors"].as_array().unwrap();
        assert!(cases.len() >= 8);
        let mut names = Vec::new();
        for case in cases {
            let name = case["name"].as_str().unwrap();
            names.push(name);
            let c = hex32(case["commitment"].as_str().unwrap()).unwrap();
            let want: Vec<u16> = case["indexes"].as_array().unwrap().iter().map(|i| u16::try_from(i.as_u64().unwrap()).unwrap()).collect();
            assert_eq!(indexes(&c).to_vec(), want, "{name}");
            let want: Vec<&str> = case["words"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
            assert_eq!(words(&c).to_vec(), want, "{name}");
            assert_eq!(words_text(&c), case["words_text"].as_str().unwrap(), "{name}");
        }
        assert!(names.contains(&"all zeros") && names.contains(&"all ones (ff)"));
    }
}
