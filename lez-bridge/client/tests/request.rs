//! The deposit request format v1 against the web page's own vectors
//! (`vectors/request-v1.json`, copied from `web/deposit/test/vectors/`).

use bridge_client::{hexser::unhex, request::DepositRequest};
use serde_json::Value;

fn parse_amount(text: &str) -> Vec<u8> {
    // Decimal string to minimal big-endian bytes (up to 2^256 - 1).
    let mut bytes: Vec<u8> = vec![0];
    for ch in text.bytes() {
        let mut carry = u32::from(ch - b'0');
        for b in bytes.iter_mut().rev() {
            let v = u32::from(*b) * 10 + carry;
            *b = (v & 0xff) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            bytes.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

fn from_json(r: &Value) -> DepositRequest {
    DepositRequest {
        chain_id: r["chainId"].as_str().unwrap().parse().unwrap(),
        vault: unhex(r["vault"].as_str().unwrap()).unwrap().try_into().unwrap(),
        token: unhex(r["token"].as_str().unwrap()).unwrap().try_into().unwrap(),
        denom_index: u8::try_from(r["denomIndex"].as_u64().unwrap()).unwrap(),
        amount: parse_amount(r["amount"].as_str().unwrap()),
        commitment: unhex(r["commitment"].as_str().unwrap()).unwrap().try_into().unwrap(),
    }
}

#[test]
fn valid_vectors_encode_and_decode() {
    let doc: Value = serde_json::from_str(include_str!("vectors/request-v1.json")).unwrap();
    let valid = doc["valid"].as_array().unwrap();
    assert!(valid.len() >= 5);
    for v in valid {
        let r = from_json(&v["request"]);
        assert_eq!(bridge_client::hexser::hex(&r.to_bytes().unwrap()), v["payloadHex"].as_str().unwrap(), "{}", v["name"]);
        let s = r.encode().unwrap();
        assert_eq!(s, v["string"].as_str().unwrap(), "{}", v["name"]);
        assert_eq!(s.len() as u64, v["length"].as_u64().unwrap());
        assert_eq!(DepositRequest::decode(&s).unwrap(), r);
        assert_eq!(DepositRequest::decode(&s.to_ascii_uppercase()).unwrap(), r);
    }
}

#[test]
fn invalid_vectors_are_refused() {
    let doc: Value = serde_json::from_str(include_str!("vectors/request-v1.json")).unwrap();
    let invalid = doc["invalid"].as_array().unwrap();
    assert!(invalid.len() >= 10);
    for v in invalid {
        let Some(input) = v["input"].as_str() else { continue };
        assert!(DepositRequest::decode(input).is_err(), "accepted: {}", v["name"]);
    }
}

#[test]
fn a_u128_request_round_trips() {
    let r = DepositRequest::new(31337, [0x11; 20], [0xAA; 20], 1, 1_000_000_000, [7; 32]);
    assert_eq!(r.amount_u128(), Some(1_000_000_000));
    let s = r.encode().unwrap();
    assert!(s.starts_with("logosdep1"));
    assert_eq!(DepositRequest::decode(&s).unwrap(), r);
    assert_eq!(r.to_json()["amount"], "1000000000");
}
