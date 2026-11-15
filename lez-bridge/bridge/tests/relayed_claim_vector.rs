// SPDX-License-Identifier: MIT OR Apache-2.0
//! The relayed public claim's golden vector (`modules/tools/make-relayed-claim-vector.py` (not published), an
//! independent Python implementation) against the program's own functions and borsh: the
//! commitment, leaf, Merkle path and root, nullifier, the three PDAs and the claim message bytes
//! that `lez-bridge claim --relay` signs and the `lez_bridge` core module hands the Names Wallet.
//! The module's own copy of these formulas is pinned to the same file
//! (`modules/lez_bridge/rust-lib/src/relayed.rs`).

use bridge_core::{
    Instruction, Message, claim_nullifier, config_address, deposit_commitment, deposit_leaf, merkle_path, merkle_root,
    nullifier_address, root_address,
};
use lee_core::account::AccountId;

const VECTOR: &str = include_str!("../../../modules/lez_bridge/rust-lib/testdata/relayed-claim-v1.json");

/// The string value of the first `"key": "…"` after `from` (the file is the generator's output;
/// no JSON crate in this crate's dependencies).
fn field<'a>(from: &'a str, key: &str) -> &'a str {
    let tag = format!("\"{key}\": \"");
    let at = from.find(&tag).unwrap_or_else(|| panic!("no {key}")) + tag.len();
    &from[at..at + from[at..].find('"').unwrap()]
}

fn number(from: &str, key: &str) -> u64 {
    let tag = format!("\"{key}\": ");
    let at = from.find(&tag).unwrap_or_else(|| panic!("no {key}")) + tag.len();
    from[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
}

fn list<'a>(from: &'a str, key: &str) -> Vec<&'a str> {
    let tag = format!("\"{key}\": [");
    let at = from.find(&tag).unwrap_or_else(|| panic!("no {key}")) + tag.len();
    let end = at + from[at..].find(']').unwrap();
    from[at..end].split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty()).collect()
}

fn hex(text: &str) -> Vec<u8> {
    let t = text.trim_start_matches("0x");
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).unwrap()).collect()
}

fn h32(text: &str) -> [u8; 32] {
    hex(text).try_into().unwrap()
}

fn id(text: &str) -> AccountId {
    text.parse().unwrap_or_else(|e| panic!("{text}: {e:?}"))
}

#[test]
fn the_relayed_claim_vector_matches_the_program() {
    let (inputs, expected) = VECTOR.split_at(VECTOR.find("\"expected\"").unwrap());
    let bridge = id(field(inputs, "bridge_program"));
    let cfg = h32(field(inputs, "cfg_id"));
    let token: [u8; 20] = hex(field(inputs, "eth_token")).try_into().unwrap();
    let denom: u128 = field(inputs, "denom").parse().unwrap();
    let denom_index = u32::try_from(number(inputs, "denom_index")).unwrap();
    let claimer = id(field(inputs, "claimer"));
    let k = h32(field(inputs, "claim_key"));
    let leaves: Vec<[u8; 32]> = list(inputs, "leaves").into_iter().map(h32).collect();
    let index = number(inputs, "leaf_index");

    let c = deposit_commitment(&k, &claimer, &token, denom, &cfg);
    assert_eq!(c, h32(field(expected, "commitment")));
    let leaf = deposit_leaf(&cfg, &token, denom, &c);
    assert_eq!(leaf, h32(field(expected, "leaf")));
    assert_eq!(leaves[usize::try_from(index).unwrap()], leaf);
    let path = merkle_path(&leaves, usize::try_from(index).unwrap());
    let want: Vec<[u8; 32]> = list(expected, "path").into_iter().map(h32).collect();
    assert_eq!(path, want);
    let root = merkle_root(&leaf, index, &path);
    assert_eq!(root, h32(field(expected, "root")));
    let nullifier = claim_nullifier(&k, &claimer, &token, denom);
    assert_eq!(nullifier, h32(field(expected, "nullifier")));
    assert_eq!(config_address(&bridge).to_string(), field(expected, "config_pda"));
    assert_eq!(root_address(&bridge, &root).to_string(), field(expected, "root_pda"));
    assert_eq!(nullifier_address(&bridge, &nullifier).to_string(), field(expected, "nullifier_pda"));

    // The message exactly as `claim` builds it (cli/src/main.rs, `Command::Claim`), public
    // claimer: no one-time recipient.
    let message = borsh::to_vec(&Message::Instruction(Instruction::Claim {
        eth_token: token,
        denom_index,
        denom,
        root,
        leaf_index: index,
        path,
        secret: k,
        cfg_id: cfg,
        recipient_opening: None,
    }))
    .unwrap();
    assert_eq!(message, hex(field(expected, "message")));
    assert_eq!(message.len() as u64, number(expected, "message_len"));
}
