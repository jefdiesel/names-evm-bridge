// Guest B (inclusion): storage-slot attestation under a finalized execution block.
//
// Input: the finalized execution block hash committed by guest A, the execution header RLP,
// eth_getProof account + storage proofs, and a Solidity `mapping(bytes32 => bool)` entry
// (key, mapping slot index N).
//
// 1. keccak256(header_rlp) == block_hash, read state_root (field 3) and number (field 8).
//    On Gloas the light-client header carries only the execution block hash, so this step is
//    what binds the state root to the finalized header; pre-Gloas the LC header has the
//    state root directly and this step can be skipped.
// 2. slot_key = keccak256(abi.encode(key, N)) = keccak256(key ++ uint256(N) big-endian).
// 3. Account proof: keccak256(address) -> rlp(TrieAccount) under state_root (alloy-trie).
// 4. Storage proof: keccak256(slot_key) -> rlp(value) under the account's storage_root.
// Any failure panics; the journal is only written on success.
use alloy_primitives::{keccak256, Bytes, B256, U256};
use alloy_rlp::{Decodable, Header};
use alloy_trie::{proof::verify_proof, Nibbles, TrieAccount};
use attest_common::{InclusionOutput, InclusionParams, Marks};
use risc0_zkvm::guest::env;

struct Clock {
    last: u64,
    marks: Marks,
}

impl Clock {
    fn new() -> Self {
        Self { last: env::cycle_count(), marks: Vec::new() }
    }
    fn mark(&mut self, label: &str) {
        let now = env::cycle_count();
        self.marks.push((label.to_string(), now - self.last));
        self.last = env::cycle_count();
    }
}

/// Returns (state_root, number) from an RLP execution header. Field order is fixed since
/// Frontier: parentHash, ommersHash, beneficiary, stateRoot, txRoot, receiptsRoot, bloom,
/// difficulty, number, ...; later forks only append fields.
fn header_fields(rlp: &[u8]) -> (B256, u64) {
    let mut buf = rlp;
    let h = Header::decode(&mut buf).expect("header: bad RLP");
    assert!(h.list, "header: not an RLP list");
    assert_eq!(buf.len(), h.payload_length, "header: trailing bytes");
    let mut state_root = B256::ZERO;
    let mut number = 0u64;
    for i in 0..=8 {
        match i {
            3 => state_root = B256::decode(&mut buf).expect("header: state_root"),
            8 => number = u64::decode(&mut buf).expect("header: number"),
            _ => {
                let f = Header::decode(&mut buf).expect("header: field");
                buf = &buf[f.payload_length..];
            }
        }
    }
    (state_root, number)
}

fn main() {
    let p: InclusionParams = env::read();
    let header_rlp = env::read_frame();
    let account_proof: Vec<Bytes> =
        (0..p.n_account_nodes).map(|_| Bytes::from(env::read_frame())).collect();
    let storage_proof: Vec<Bytes> =
        (0..p.n_storage_nodes).map(|_| Bytes::from(env::read_frame())).collect();
    let mut c = Clock::new();

    // 1. bind the state root to the finalized execution block hash
    let block_hash = B256::from(p.block_hash);
    assert_eq!(keccak256(&header_rlp), block_hash, "execution header does not hash to the finalized block hash");
    let (state_root, block_number) = header_fields(&header_rlp);
    c.mark("header: keccak256(rlp) == finalized block hash, decode state_root");

    // 2. Solidity mapping slot: keccak256(abi.encode(bytes32 key, uint256 N))
    let mut pre = [0u8; 64];
    pre[..32].copy_from_slice(&p.mapping_key);
    pre[32..].copy_from_slice(&U256::from(p.mapping_slot_index).to_be_bytes::<32>());
    let slot_key = keccak256(pre);
    c.mark("slot key: keccak256(abi.encode(key, N))");

    // 3. account proof under the state root
    let account = TrieAccount {
        nonce: p.nonce,
        balance: U256::from_be_bytes(p.balance),
        storage_root: B256::from(p.storage_root),
        code_hash: B256::from(p.code_hash),
    };
    let account_key = Nibbles::unpack(keccak256(p.address));
    verify_proof(state_root, account_key, Some(alloy_rlp::encode(&account)), &account_proof)
        .unwrap_or_else(|e| panic!("account proof invalid: {e}"));
    c.mark(&format!("account proof: {} nodes (alloy-trie verify_proof)", account_proof.len()));

    // 4. storage proof under the account's storage root
    let value = U256::from_be_bytes(p.value);
    let expected = if value.is_zero() { None } else { Some(alloy_rlp::encode(value)) };
    verify_proof(account.storage_root, Nibbles::unpack(keccak256(slot_key)), expected, &storage_proof)
        .unwrap_or_else(|e| panic!("storage proof invalid: {e}"));
    c.mark(&format!("storage proof: {} nodes (alloy-trie verify_proof)", storage_proof.len()));

    env::commit(&InclusionOutput {
        chain_id: p.chain_id,
        block_hash: p.block_hash,
        block_number,
        state_root: state_root.0,
        address: p.address,
        slot_key: slot_key.0,
        value: p.value,
        marks: c.marks,
    });
}
