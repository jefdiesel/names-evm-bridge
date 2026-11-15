//! `0x`-hex serde for byte arrays, words and byte strings.

use serde::{Deserializer, Serializer};

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// # Errors
/// Odd length or a non-hex digit.
pub fn unhex(text: &str) -> Result<Vec<u8>, String> {
    let t = text.trim().trim_start_matches("0x");
    if t.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).map_err(|e| format!("bad hex: {e}"))).collect()
}

pub mod arr {
    use super::{Deserializer, Serializer, hex, unhex};
    use serde::{Deserialize as _, de::Error as _};

    pub fn serialize<S: Serializer, const N: usize>(v: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("0x{}", hex(v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        let t = String::deserialize(d)?;
        unhex(&t).map_err(D::Error::custom)?.try_into().map_err(|_| D::Error::custom("wrong length"))
    }
}

pub mod bytes {
    use super::{Deserializer, Serializer, hex, unhex};
    use serde::{Deserialize as _, de::Error as _};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("0x{}", hex(v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        unhex(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

pub mod words {
    use super::{Deserializer, Serializer, hex, unhex};
    use serde::{Deserialize as _, de::Error as _, ser::SerializeSeq as _};

    pub fn serialize<S: Serializer>(v: &[[u8; 32]], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for w in v {
            seq.serialize_element(&format!("0x{}", hex(w)))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<[u8; 32]>, D::Error> {
        Vec::<String>::deserialize(d)?
            .iter()
            .map(|t| unhex(t).map_err(D::Error::custom)?.try_into().map_err(|_| D::Error::custom("wrong length")))
            .collect()
    }
}

/// A `u128` as a decimal string (JSON numbers lose precision above 2^53 in most readers).
pub mod dec {
    use super::{Deserializer, Serializer};
    use serde::{Deserialize as _, de::Error as _};

    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        String::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}
