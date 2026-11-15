//! G3, the RFP-022 release proof: the statement the zkVM guest (`methods/release_guest`) runs, as
//! plain Rust so native tests run exactly the same code.
//!
//! # Statement
//!
//! Private witness: the exit secret `k`, `ethDest`, `token`, `denom`, `maxRelayFee`, `cfgId`, the
//! leaf index and the 32-sibling Merkle path in the vault's exit tree. Public, chosen by whoever
//! asks for the proof: `relayer` and `relayFee`. The guest computes
//!
//! ```text
//! E    = SHA256("RFP022/exit" | k | ethDest | token | u256(denom) | u256(maxRelayFee) | cfgId)
//! leaf = SHA256(EXIT_LEAF_TAG | cfgId | token | u256(denom) | E)           (BridgeVault.exitLeaf)
//! root = the depth-32 root from leaf, index and path                     (Sha256IncrementalTree)
//! nf   = SHA256("RFP022/nfr" | k)
//! ```
//!
//! refuses (panics in the guest) if `relayFee > maxRelayFee`, and commits as its journal the 256
//! bytes
//!
//! ```text
//! abi.encode(cfgId, root, nf, token, denom, ethDest, relayer, relayFee)
//! ```
//!
//! which are exactly the bytes `BridgeVault.release` hashes:
//! `sha256(abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee))`. RISC
//! Zero's journal digest is the SHA-256 of the journal bytes, so the vault's `digest` is the
//! journal digest `IRiscZeroVerifier.verify(seal, releaseImageId, digest)` checks.
//!
//! The guest takes no root as input: a wrong secret, path, index or any wrong field of `E` gives
//! a journal with a root the vault never recorded (`UnknownExitRoot`), or, if the submitter
//! passes a real root instead, a digest that doesn't match the seal (verification fails). Only
//! a fee above the committed ceiling, a malformed path or an index outside the tree make the
//! guest itself fail.
//!
//! Amounts are full `uint256` here. The LEZ side (`bridge_core`) holds amounts as `u128` and
//! writes them as `u256(u128)`, so every `E` a LEZ burn can produce has a denomination and fee
//! ceiling below 2^128; the encodings agree on that range (checked in `release/host`).

use risc0_zkvm::sha::{Impl, Sha256 as _};

/// An Ethereum address.
pub type EthAddress = [u8; 20];
/// A Solidity `uint256`, 32 bytes big-endian.
pub type U256 = [u8; 32];

/// `keccak256("RFP022/exit-leaf/v1")`, `BridgeVault.EXIT_LEAF_TAG` (same as `bridge_core`).
pub const EXIT_LEAF_TAG: [u8; 32] = [
    0x22, 0x01, 0x87, 0x42, 0xad, 0x60, 0x28, 0xfd, 0xa6, 0xcb, 0x07, 0x26, 0xaf, 0xaa, 0xe9, 0x0e, 0xa2, 0x0d, 0xff,
    0x3c, 0xef, 0x6b, 0x1e, 0xc5, 0xb3, 0xc1, 0x4f, 0x85, 0x6d, 0x76, 0xc5, 0x9c,
];
/// `keccak256("RFP022/cfg/v1")`, `BridgeVault.CFG_TAG`.
pub const CFG_TAG: [u8; 32] = [
    0x26, 0xe9, 0x62, 0x30, 0x2b, 0x1d, 0xc9, 0x41, 0x7c, 0x46, 0x6f, 0xf5, 0x6e, 0xf2, 0x06, 0xa5, 0x78, 0xcd, 0x05,
    0x4a, 0x0a, 0x09, 0x91, 0x72, 0xcf, 0xf8, 0xb8, 0x04, 0x60, 0xab, 0x5a, 0x45,
];
pub const EXIT_DOMAIN: &[u8] = b"RFP022/exit";
pub const NFR_DOMAIN: &[u8] = b"RFP022/nfr";
/// Depth of the vault's exit tree (`Sha256IncrementalTree.DEPTH`).
pub const TREE_DEPTH: usize = 32;
/// `abi.encode` of 8 static words.
pub const JOURNAL_LEN: usize = 8 * 32;

#[must_use]
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let bytes: Vec<u8> = parts.concat();
    let mut out = [0u8; 32];
    out.copy_from_slice(Impl::hash_bytes(&bytes).as_bytes());
    out
}

/// A u128 as Solidity's 32-byte big-endian `uint256`.
#[must_use]
pub fn u256(v: u128) -> U256 {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

/// `SHA256(CFG_TAG | u256(chainId) | vault | lezChainId | zoneId | programId)`, as
/// `BridgeVault.computeCfgId`.
#[must_use]
pub fn cfg_id(chain_id: u64, vault: &EthAddress, lez_chain_id: &[u8; 32], zone_id: &[u8; 32], program_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[&CFG_TAG, &u256(u128::from(chain_id)), vault, lez_chain_id, zone_id, program_id])
}

/// The exit commitment `E` a burn carries.
#[must_use]
pub fn exit_commitment(
    secret: &[u8; 32],
    eth_dest: &EthAddress,
    token: &EthAddress,
    denom: &U256,
    max_relay_fee: &U256,
    cfg_id: &[u8; 32],
) -> [u8; 32] {
    sha256(&[EXIT_DOMAIN, secret, eth_dest, token, denom, max_relay_fee, cfg_id])
}

/// `BridgeVault.exitLeaf`: `sha256(abi.encodePacked(EXIT_LEAF_TAG, cfgId, token, denom, E))`.
#[must_use]
pub fn exit_leaf(cfg_id: &[u8; 32], token: &EthAddress, denom: &U256, commitment: &[u8; 32]) -> [u8; 32] {
    sha256(&[&EXIT_LEAF_TAG, cfg_id, token, denom, commitment])
}

/// The release nullifier `SHA256("RFP022/nfr" | k)`.
#[must_use]
pub fn release_nullifier(secret: &[u8; 32]) -> [u8; 32] {
    sha256(&[NFR_DOMAIN, secret])
}

/// The root reached from `leaf` at `index` along `path` (sibling at each level, leaf first),
/// with the vault's convention: bit `h` of `index` set means the node is a right child.
pub fn merkle_root(leaf: &[u8; 32], index: u64, path: &[[u8; 32]]) -> Result<[u8; 32], ReleaseError> {
    if path.len() != TREE_DEPTH {
        return Err(ReleaseError::PathLength(path.len()));
    }
    if index >= 1u64 << TREE_DEPTH {
        return Err(ReleaseError::IndexOutOfRange(index));
    }
    let mut node = *leaf;
    for (h, sibling) in path.iter().enumerate() {
        node = if (index >> h) & 1 == 1 { sha256(&[sibling, &node]) } else { sha256(&[&node, sibling]) };
    }
    Ok(node)
}

/// What the prover knows and never reveals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseWitness {
    pub secret: [u8; 32],
    pub eth_dest: EthAddress,
    pub token: EthAddress,
    pub denom: U256,
    pub max_relay_fee: U256,
    pub cfg_id: [u8; 32],
    pub leaf_index: u64,
    pub path: Vec<[u8; 32]>,
}

/// Chosen when the proof is made, bound into the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleasePublic {
    pub relayer: EthAddress,
    pub relay_fee: U256,
}

/// The guest's whole input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInput {
    pub witness: ReleaseWitness,
    pub public: ReleasePublic,
}

/// Size of the guest input: `k | ethDest | token | denom | maxRelayFee | cfgId | u64be(index) |
/// path[0..32] | relayer | relayFee`.
pub const INPUT_LEN: usize = 32 + 20 + 20 + 32 + 32 + 32 + 8 + 32 * TREE_DEPTH + 20 + 32;

impl ReleaseInput {
    /// The fixed layout the guest reads. Panics unless the path has [`TREE_DEPTH`] siblings
    /// (the layout has no room for any other length).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let w = &self.witness;
        assert_eq!(w.path.len(), TREE_DEPTH, "a Merkle path has {TREE_DEPTH} siblings");
        let mut out = Vec::with_capacity(INPUT_LEN);
        out.extend_from_slice(&w.secret);
        out.extend_from_slice(&w.eth_dest);
        out.extend_from_slice(&w.token);
        out.extend_from_slice(&w.denom);
        out.extend_from_slice(&w.max_relay_fee);
        out.extend_from_slice(&w.cfg_id);
        out.extend_from_slice(&w.leaf_index.to_be_bytes());
        for p in &w.path {
            out.extend_from_slice(p);
        }
        out.extend_from_slice(&self.public.relayer);
        out.extend_from_slice(&self.public.relay_fee);
        out
    }

    #[must_use]
    pub fn from_bytes(b: &[u8; INPUT_LEN]) -> Self {
        let mut at = 0;
        let mut take = |n: usize| {
            let s = &b[at..at + n];
            at += n;
            s
        };
        let a32 = |s: &[u8]| -> [u8; 32] { s.try_into().unwrap_or([0; 32]) };
        let a20 = |s: &[u8]| -> [u8; 20] { s.try_into().unwrap_or([0; 20]) };
        let secret = a32(take(32));
        let eth_dest = a20(take(20));
        let token = a20(take(20));
        let denom = a32(take(32));
        let max_relay_fee = a32(take(32));
        let cfg_id = a32(take(32));
        let leaf_index = u64::from_be_bytes(take(8).try_into().unwrap_or([0; 8]));
        let path = (0..TREE_DEPTH).map(|_| a32(take(32))).collect();
        let relayer = a20(take(20));
        let relay_fee = a32(take(32));
        Self {
            witness: ReleaseWitness { secret, eth_dest, token, denom, max_relay_fee, cfg_id, leaf_index, path },
            public: ReleasePublic { relayer, relay_fee },
        }
    }
}

/// The guest's public output, in `BridgeVault.release`'s digest order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseJournal {
    pub cfg_id: [u8; 32],
    pub root: [u8; 32],
    pub nullifier: [u8; 32],
    pub token: EthAddress,
    pub denom: U256,
    pub recipient: EthAddress,
    pub relayer: EthAddress,
    pub relay_fee: U256,
}

fn addr_word(a: &EthAddress) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

fn word_addr(w: &[u8]) -> Option<EthAddress> {
    if w[..12].iter().any(|&b| b != 0) {
        return None;
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&w[12..32]);
    Some(a)
}

impl ReleaseJournal {
    /// `abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)`: eight
    /// 32-byte words, addresses left-padded with 12 zero bytes.
    #[must_use]
    pub fn abi_encode(&self) -> [u8; JOURNAL_LEN] {
        let words: [[u8; 32]; 8] = [
            self.cfg_id,
            self.root,
            self.nullifier,
            addr_word(&self.token),
            self.denom,
            addr_word(&self.recipient),
            addr_word(&self.relayer),
            self.relay_fee,
        ];
        let mut out = [0u8; JOURNAL_LEN];
        for (i, w) in words.iter().enumerate() {
            out[32 * i..32 * (i + 1)].copy_from_slice(w);
        }
        out
    }

    /// Strict inverse of [`Self::abi_encode`]: exactly 256 bytes, address padding zero.
    #[must_use]
    pub fn abi_decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != JOURNAL_LEN {
            return None;
        }
        let w = |i: usize| -> [u8; 32] { bytes[32 * i..32 * (i + 1)].try_into().unwrap_or([0; 32]) };
        Some(Self {
            cfg_id: w(0),
            root: w(1),
            nullifier: w(2),
            token: word_addr(&bytes[96..128])?,
            denom: w(4),
            recipient: word_addr(&bytes[160..192])?,
            relayer: word_addr(&bytes[192..224])?,
            relay_fee: w(7),
        })
    }

    /// `sha256(journal)`: the vault's release digest and RISC Zero's journal digest.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        sha256(&[&self.abi_encode()])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseError {
    PathLength(usize),
    IndexOutOfRange(u64),
    RelayFeeAboveMax,
}

impl core::fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PathLength(n) => write!(f, "a Merkle path has {TREE_DEPTH} siblings, got {n}"),
            Self::IndexOutOfRange(i) => write!(f, "leaf index {i} outside the depth-{TREE_DEPTH} tree"),
            Self::RelayFeeAboveMax => write!(f, "relay fee above the committed maxRelayFee"),
        }
    }
}

/// The whole G3 statement. Every field of the journal comes from one variable that also went
/// into `E` (or is the public relayer / fee), so the recipient, token, denomination and config
/// the vault pays out are the ones the exit committed to.
pub fn release(input: &ReleaseInput) -> Result<ReleaseJournal, ReleaseError> {
    let w = &input.witness;
    let p = &input.public;
    // Big-endian 32-byte words compare as unsigned integers lexicographically.
    if p.relay_fee > w.max_relay_fee {
        return Err(ReleaseError::RelayFeeAboveMax);
    }
    let e = exit_commitment(&w.secret, &w.eth_dest, &w.token, &w.denom, &w.max_relay_fee, &w.cfg_id);
    let leaf = exit_leaf(&w.cfg_id, &w.token, &w.denom, &e);
    let root = merkle_root(&leaf, w.leaf_index, &w.path)?;
    Ok(ReleaseJournal {
        cfg_id: w.cfg_id,
        root,
        nullifier: release_nullifier(&w.secret),
        token: w.token,
        denom: w.denom,
        recipient: w.eth_dest,
        relayer: p.relayer,
        relay_fee: p.relay_fee,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_roundtrip_and_padding() {
        let j = ReleaseJournal {
            cfg_id: [1; 32],
            root: [2; 32],
            nullifier: [3; 32],
            token: [4; 20],
            denom: u256(5),
            recipient: [6; 20],
            relayer: [7; 20],
            relay_fee: u256(8),
        };
        let b = j.abi_encode();
        assert_eq!(b.len(), 256);
        assert_eq!(&b[96..108], &[0u8; 12]);
        assert_eq!(&b[108..128], &[4u8; 20]);
        assert_eq!(ReleaseJournal::abi_decode(&b), Some(j));
        let mut bad = b;
        bad[100] = 1;
        assert_eq!(ReleaseJournal::abi_decode(&bad), None);
        assert_eq!(ReleaseJournal::abi_decode(&b[..255]), None);
    }

    #[test]
    fn input_roundtrip() {
        let i = ReleaseInput {
            witness: ReleaseWitness {
                secret: [1; 32],
                eth_dest: [2; 20],
                token: [3; 20],
                denom: u256(4),
                max_relay_fee: u256(5),
                cfg_id: [6; 32],
                leaf_index: 0x0102_0304,
                path: (0..TREE_DEPTH as u8).map(|h| [h; 32]).collect(),
            },
            public: ReleasePublic { relayer: [7; 20], relay_fee: u256(8) },
        };
        let b = i.to_bytes();
        assert_eq!(b.len(), INPUT_LEN);
        assert_eq!(INPUT_LEN % 4, 0);
        assert_eq!(ReleaseInput::from_bytes(b.as_slice().try_into().unwrap()), i);
    }

    #[test]
    fn fee_compare_is_numeric() {
        assert!(u256(255) < u256(256));
        assert!(u256(u128::MAX) < [0x01; 32]);
    }
}
