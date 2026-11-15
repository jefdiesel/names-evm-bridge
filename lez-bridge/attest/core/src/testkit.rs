//! Client-side helpers for tests and cycle counts (feature `testkit`): build a synthetic
//! Ethereum state trie, execution header and receipts trie with real `alloy-trie` proofs. Nothing
//! here is reachable from a program; it only produces inputs.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_rlp::{Encodable, Header};
use alloy_trie::{EMPTY_ROOT_HASH, HashBuilder, KECCAK_EMPTY, Nibbles, TrieAccount, proof::ProofRetainer};

use crate::inclusion::{AccountFields, AccountProof, Log, ReceiptProof, StorageProof};

/// One account of a synthetic state.
#[derive(Debug, Clone)]
pub struct SynthAccount {
    pub address: [u8; 20],
    pub nonce: u64,
    pub balance: u128,
    /// (slot, value); zero values are left out of the trie, as Ethereum does.
    pub storage: Vec<([u8; 32], [u8; 32])>,
}

impl SynthAccount {
    #[must_use]
    pub fn new(address: [u8; 20]) -> Self {
        Self { address, nonce: 1, balance: 0, storage: Vec::new() }
    }

    #[must_use]
    pub fn with(mut self, slot: [u8; 32], value: [u8; 32]) -> Self {
        self.storage.push((slot, value));
        self
    }

    fn storage_entries(&self) -> Vec<(Nibbles, Vec<u8>)> {
        self.storage
            .iter()
            .filter(|(_, v)| *v != [0u8; 32])
            .map(|(k, v)| (Nibbles::unpack(keccak256(k)), alloy_rlp::encode(U256::from_be_bytes(*v))))
            .collect()
    }

    #[must_use]
    pub fn storage_root(&self) -> [u8; 32] {
        trie(self.storage_entries(), None).0
    }

    #[must_use]
    pub fn fields(&self) -> AccountFields {
        AccountFields {
            nonce: self.nonce,
            balance: U256::from(self.balance).to_be_bytes::<32>(),
            storage_root: self.storage_root(),
            code_hash: KECCAK_EMPTY.0,
        }
    }

    fn leaf(&self) -> Vec<u8> {
        let f = self.fields();
        alloy_rlp::encode(TrieAccount {
            nonce: f.nonce,
            balance: U256::from_be_bytes(f.balance),
            storage_root: B256::from(f.storage_root),
            code_hash: B256::from(f.code_hash),
        })
    }
}

/// Builds a trie from (key, leaf value) pairs and returns its root and, for `target`, the proof.
#[must_use]
pub fn trie(mut entries: Vec<(Nibbles, Vec<u8>)>, target: Option<Nibbles>) -> ([u8; 32], Vec<Vec<u8>>) {
    if entries.is_empty() {
        return (EMPTY_ROOT_HASH.0, Vec::new());
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let targets: Vec<Nibbles> = target.into_iter().collect();
    let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(targets.clone()));
    for (k, v) in &entries {
        hb.add_leaf(*k, v);
    }
    let root = hb.root();
    let nodes = match targets.first() {
        Some(t) => hb
            .take_proof_nodes()
            .matching_nodes_sorted(t)
            .into_iter()
            .map(|(_, n)| n.to_vec())
            .collect(),
        None => Vec::new(),
    };
    (root.0, nodes)
}

/// A synthetic Ethereum state.
#[derive(Debug, Clone, Default)]
pub struct SynthState {
    pub accounts: Vec<SynthAccount>,
}

impl SynthState {
    fn entries(&self) -> Vec<(Nibbles, Vec<u8>)> {
        self.accounts.iter().map(|a| (Nibbles::unpack(keccak256(a.address)), a.leaf())).collect()
    }

    fn account(&self, address: &[u8; 20]) -> &SynthAccount {
        self.accounts.iter().find(|a| &a.address == address).expect("no such synthetic account")
    }

    #[must_use]
    pub fn state_root(&self) -> [u8; 32] {
        trie(self.entries(), None).0
    }

    /// Proof for an account that exists.
    #[must_use]
    pub fn account_proof(&self, address: &[u8; 20]) -> AccountProof {
        let (_, nodes) = trie(self.entries(), Some(Nibbles::unpack(keccak256(address))));
        AccountProof { fields: self.account(address).fields(), nodes }
    }

    /// Proof for a slot of an existing account; `value` is the slot's real value (zero if absent).
    #[must_use]
    pub fn storage_proof(&self, address: &[u8; 20], slot: &[u8; 32]) -> StorageProof {
        let a = self.account(address);
        let (_, nodes) = trie(a.storage_entries(), Some(Nibbles::unpack(keccak256(slot))));
        let value = a.storage.iter().find(|(k, _)| k == slot).map_or([0u8; 32], |(_, v)| *v);
        StorageProof { account: self.account_proof(address), value, nodes }
    }
}

fn list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.concat();
    let mut out = Vec::new();
    Header { list: true, payload_length: payload.len() }.encode(&mut out);
    out.extend_from_slice(&payload);
    out
}

/// A minimal (pre-London-shaped, 15-field) execution header RLP with these roots. Only field
/// positions 3, 5 and 8 matter to the inclusion code; `salt` goes into `extraData` so distinct
/// headers can share roots.
#[must_use]
pub fn header_rlp(state_root: [u8; 32], receipts_root: [u8; 32], number: u64, salt: u8) -> Vec<u8> {
    let e = |x: &dyn Encodable| alloy_rlp::encode(x);
    list(&[
        e(&B256::repeat_byte(0x11)),
        e(&B256::repeat_byte(0x22)),
        e(&Address::repeat_byte(0x33)),
        e(&B256::from(state_root)),
        e(&B256::repeat_byte(0x44)),
        e(&B256::from(receipts_root)),
        e(&Bytes::from(vec![0u8; 256])),
        e(&0u64),
        e(&number),
        e(&30_000_000u64),
        e(&1_000_000u64),
        e(&1_700_000_000u64),
        e(&Bytes::from(vec![salt])),
        e(&B256::ZERO),
        e(&Bytes::from(vec![0u8; 8])),
    ])
}

/// An EIP-1559 (type 2) receipt: `0x02 ‖ rlp([status, cumulativeGasUsed, logsBloom, logs])`.
#[must_use]
pub fn receipt(success: bool, logs: &[Log]) -> Vec<u8> {
    let e = |x: &dyn Encodable| alloy_rlp::encode(x);
    let logs_rlp: Vec<Vec<u8>> = logs
        .iter()
        .map(|l| {
            let topics: Vec<B256> = l.topics.iter().map(|t| B256::from(*t)).collect();
            list(&[e(&Address::from(l.address)), e(&topics), e(&Bytes::from(l.data.clone()))])
        })
        .collect();
    let mut out = vec![0x02];
    out.extend(list(&[e(&u64::from(success)), e(&21_000u64), e(&Bytes::from(vec![0u8; 256])), list(&logs_rlp)]));
    out
}

/// The receipts trie of a block (keys `rlp(index)`, unhashed) and a proof for `index`.
#[must_use]
pub fn receipts_trie(receipts: &[Vec<u8>], index: u64) -> ([u8; 32], ReceiptProof) {
    let entries: Vec<(Nibbles, Vec<u8>)> = receipts
        .iter()
        .enumerate()
        .map(|(i, r)| (Nibbles::unpack(alloy_rlp::encode(i as u64)), r.clone()))
        .collect();
    let (root, nodes) = trie(entries, Some(Nibbles::unpack(alloy_rlp::encode(index))));
    let receipt = receipts[usize::try_from(index).expect("index fits")].clone();
    (root, ReceiptProof { tx_index: index, receipt, nodes })
}
