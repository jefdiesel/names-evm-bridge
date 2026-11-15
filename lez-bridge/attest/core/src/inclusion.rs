//! Inclusion and predicates, run by a consumer inside its own execution.
//!
//! Every function here takes an [`AnchorWitness`] (an anchor plus, for a block-hash anchor, the
//! execution header RLP), derives the execution roots from it, and verifies a Merkle-Patricia
//! proof with `alloy-trie`. The roots are never taken from the caller: a state root is either
//! the anchor itself or read out of a header whose keccak is the anchor (Functionality #3).
//! None of this says the anchor is *finalized*; the consumer must also send
//! [`crate::assert_anchor`] in the same transaction, or the statement means nothing.
//!
//! Statements carry a deterministic identifier ([`StorageStatement::id`] and friends): the same
//! fact at the same anchor gives the same id, and any difference (chain, instance, account, slot,
//! predicate, anchor) gives another one (Functionality #6). A consumer that wants "once per fact,
//! whatever block it was proven at" keys its nullifier on [`StorageStatement::fact_id`] instead.

use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_rlp::{Decodable, Header};
use alloy_trie::{Nibbles, TrieAccount, proof::verify_proof};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{Anchor, sha256};

const STORAGE_STMT_TAG: &[u8] = b"RFP021/statement/storage/v1";
const STORAGE_FACT_TAG: &[u8] = b"RFP021/fact/storage/v1";
const ACCOUNT_STMT_TAG: &[u8] = b"RFP021/statement/account/v1";
const LOG_STMT_TAG: &[u8] = b"RFP021/statement/log/v1";

/// Why an inclusion claim was refused.
///
/// The `Display` is detailed (alloy-trie's own error, with the trie path, node bytes and expected
/// value) and is meant for native callers only: a CLI that runs the same check before it builds a
/// transaction may show it to its user, on the user's machine. **A program must never panic with
/// it**: the path is `keccak(slot)` of whatever the consumer proved, a witness value. Programs
/// panic with [`InclusionError::class`] ([`InclusionClass::refuse`]), a fixed text per class, or
/// map every error to a refusal of their own (the bridge does: `CLAIM_REFUSED`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InclusionError {
    /// A block-hash anchor came without its header, or a state-root anchor was given a header.
    HeaderWitness,
    HeaderHash,
    HeaderRlp(&'static str),
    AccountProof(String),
    StorageProof(String),
    ReceiptProof(String),
    Receipt(&'static str),
    NoSuchLog,
    /// The anchor gives no receipts root (a bare state root).
    NoReceiptsRoot,
    PredicateFails,
}

impl core::fmt::Display for InclusionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let class = self.class().as_str();
        match self {
            Self::HeaderWitness => write!(f, "{class}: a block-hash anchor needs its header, a state-root anchor none"),
            Self::HeaderHash => write!(f, "{class}: execution header does not hash to the anchor"),
            Self::HeaderRlp(w) => write!(f, "{class}: execution header: {w}"),
            Self::AccountProof(e) | Self::StorageProof(e) | Self::ReceiptProof(e) => write!(f, "{class}: {e}"),
            Self::Receipt(w) => write!(f, "{class}: receipt: {w}"),
            Self::NoSuchLog => write!(f, "{class}: no log at that index"),
            Self::NoReceiptsRoot => write!(f, "{class}: a state-root anchor has no receipts root"),
            Self::PredicateFails => write!(f, "{class}"),
        }
    }
}

/// The class of an [`InclusionError`], without its payload: what a program may say on chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InclusionClass {
    /// The anchor witness: header missing, extra, malformed, not hashing to the anchor, or no
    /// receipts root under a state-root anchor.
    Anchor,
    AccountProof,
    StorageProof,
    ReceiptProof,
    /// The receipt's own encoding, a failed transaction, or no log at the index.
    Receipt,
    Predicate,
}

impl InclusionClass {
    /// The class's fixed text. [`InclusionError`]'s `Display` starts with it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anchor => "anchor witness invalid",
            Self::AccountProof => "account proof invalid",
            Self::StorageProof => "storage proof invalid",
            Self::ReceiptProof => "receipt proof invalid",
            Self::Receipt => "receipt or log invalid",
            Self::Predicate => "the predicate does not hold",
        }
    }

    /// Panics with [`InclusionClass::as_str`] and nothing else: the program-side refusal.
    pub fn refuse(self) -> ! {
        panic!("{}", self.as_str())
    }
}

impl InclusionError {
    /// The payload-free class (see [`InclusionClass`]).
    #[must_use]
    pub const fn class(&self) -> InclusionClass {
        match self {
            Self::HeaderWitness | Self::HeaderHash | Self::HeaderRlp(_) | Self::NoReceiptsRoot => InclusionClass::Anchor,
            Self::AccountProof(_) => InclusionClass::AccountProof,
            Self::StorageProof(_) => InclusionClass::StorageProof,
            Self::ReceiptProof(_) => InclusionClass::ReceiptProof,
            Self::Receipt(_) | Self::NoSuchLog => InclusionClass::Receipt,
            Self::PredicateFails => InclusionClass::Predicate,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Proof-node pre-validation (audit B16-NYB)
//
// alloy-trie's `verify_proof` keeps the path it has walked in a 64-nibble `Nibbles`, and
// `Nibbles::extend` panics past 64 ("Cannot extend: resulting length would exceed maximum
// capacity"). Each node's own key is bounded by the decoder, but the *sum* along a proof is not:
// an extension with a full 64-nibble key whose (correctly hashed) child is a leaf with a
// non-empty key walks past 64 and panics inside the dependency. A guest cannot catch that, so
// every proof is checked here first: each node is decoded, and the walk `verify_proof` would do
// along `key` is replayed node by node, adding exactly what it adds (an extension's or leaf's
// key, one nibble for the branch child at the key's next nibble, and that child's own walk when
// it is in place; off-path children add nothing). The check refuses where that walk would pass 64
// nibbles, so it accepts every proof `verify_proof` can walk without panicking, and no other
// that reaches the overflow.

/// The most nodes a proof needs: one branch per key nibble (64) and a leaf.
pub const MAX_PROOF_NODES: usize = 65;
/// Largest account or storage trie node. A branch is at most 17 items of 33 bytes (~535 bytes);
/// an account leaf about 150. Generous on purpose.
pub const MAX_STATE_NODE_LEN: usize = 1024;
/// Largest receipt trie node: a leaf holds a whole receipt, which a block's gas limit bounds to a
/// few MB of log data.
pub const MAX_RECEIPT_NODE_LEN: usize = 1 << 24;
/// The capacity of alloy-trie's `Nibbles`.
const MAX_PATH_NIBBLES: usize = 64;

const MALFORMED_NODE: &str = "malformed proof node";
const PATH_TOO_LONG: &str = "proof path longer than 64 nibbles";

/// The nibbles of `bytes`, high nibble first (what `Nibbles::unpack` gives).
#[must_use]
pub fn nibbles_of(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|b| [b >> 4, b & 0x0f]).collect()
}

/// Checks the proof nodes for the key whose nibbles are `key` before `verify_proof` sees them
/// (see the section comment). The error is a fixed text; it never carries node bytes or a path.
///
/// # Errors
/// More than [`MAX_PROOF_NODES`] nodes, a node over `max_node_len` bytes, a node that is not a
/// single RLP list of a trie node, a node key over 64 nibbles, or nodes whose walk along `key`
/// would pass 64 nibbles.
pub fn check_proof_nodes(nodes: &[Vec<u8>], key: &[u8], max_node_len: usize) -> Result<(), &'static str> {
    if nodes.len() > MAX_PROOF_NODES {
        return Err("too many proof nodes");
    }
    if nodes.iter().any(|n| n.len() > max_node_len) {
        return Err("proof node too large");
    }
    // alloy-trie's empty-trie case: a proof whose first node is the empty string is not walked.
    if nodes.first().is_none_or(|n| n.as_slice() == [alloy_rlp::EMPTY_STRING_CODE]) {
        return Ok(());
    }
    let mut walked = 0usize;
    for node in nodes {
        let mut buf = node.as_slice();
        let (list, payload, _) = rlp_item(&mut buf)?;
        if !list || !buf.is_empty() {
            return Err(MALFORMED_NODE);
        }
        walk_node(payload, key, &mut walked)?;
    }
    Ok(())
}

/// One RLP item: (is a list, payload, encoded length including the header).
fn rlp_item<'a>(buf: &mut &'a [u8]) -> Result<(bool, &'a [u8], usize), &'static str> {
    let start = buf.len();
    let h = Header::decode(buf).map_err(|_| MALFORMED_NODE)?;
    // `Header::decode` leaves a single byte below 0x80 in place (it is its own payload) and has
    // checked that `payload_length` bytes follow.
    if buf.len() < h.payload_length {
        return Err(MALFORMED_NODE);
    }
    let (payload, rest) = buf.split_at(h.payload_length);
    *buf = rest;
    Ok((h.list, payload, start - rest.len()))
}

fn extend(walked: &mut usize, by: usize) -> Result<(), &'static str> {
    if *walked + by > MAX_PATH_NIBBLES {
        return Err(PATH_TOO_LONG);
    }
    *walked += by;
    Ok(())
}

/// Replays `verify_proof`'s walk over the node whose list payload is `payload`, `walked` nibbles
/// into `key` (alloy-trie 0.9 `process_trie_node` and `process_branch`).
fn walk_node(mut payload: &[u8], key: &[u8], walked: &mut usize) -> Result<(), &'static str> {
    let mut items: [(bool, &[u8], usize); 17] = [(false, &[], 0); 17];
    let mut n = 0;
    while !payload.is_empty() {
        if n == items.len() {
            return Err(MALFORMED_NODE);
        }
        items[n] = rlp_item(&mut payload)?;
        n += 1;
    }
    match n {
        17 => {
            // Branch. A value is never set in these tries (alloy-trie refuses one too); every
            // child is absent, a hash, or a whole node encoded in place in under 32 bytes.
            let (value_list, value, _) = items[16];
            if value_list || !value.is_empty() {
                return Err(MALFORMED_NODE);
            }
            for &(list, child, len) in &items[..16] {
                if (list && len >= 32) || (!list && !(child.is_empty() || child.len() == 32)) {
                    return Err(MALFORMED_NODE);
                }
            }
            // Only the child at the key's next nibble is followed; past the key's end, none.
            let Some(&next) = key.get(*walked) else { return Ok(()) };
            let (list, child, _) = items[usize::from(next & 0x0f)];
            if !list && child.is_empty() {
                return Ok(());
            }
            extend(walked, 1)?;
            if list { walk_node(child, key, walked) } else { Ok(()) }
        }
        2 => {
            let (key_list, hp, _) = items[0];
            if key_list || hp.is_empty() {
                return Err(MALFORMED_NODE);
            }
            let flag = hp[0] >> 4;
            if flag > 3 {
                return Err(MALFORMED_NODE);
            }
            let nibbles = 2 * (hp.len() - 1) + usize::from(flag & 1);
            if nibbles > MAX_PATH_NIBBLES {
                // The same words as alloy-trie's decoder, which refuses this node on its own.
                return Err("trie node key too long");
            }
            extend(walked, nibbles)?;
            let (child_list, child, len) = items[1];
            match (flag >= 2, child_list) {
                // Leaf: its value is a string.
                (true, false) => Ok(()),
                // Extension to a hashed child, or to one in place (walked on).
                (false, false) if child.len() == 32 => Ok(()),
                (false, true) if len < 32 => walk_node(child, key, walked),
                _ => Err(MALFORMED_NODE),
            }
        }
        _ => Err(MALFORMED_NODE),
    }
}

/// Which chain and instance a statement is for. The consumer fills it from its own immutable
/// configuration; `assert_anchor` with the same values is what makes it true.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StatementContext {
    pub evm_chain_id: u64,
    pub instance_id: [u8; 32],
}

/// An anchor and what is needed to reach its roots.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AnchorWitness {
    pub anchor: Anchor,
    /// The RLP execution header, for [`Anchor::BlockHash`]; `None` for [`Anchor::StateRoot`].
    pub header_rlp: Option<Vec<u8>>,
}

/// The execution roots an anchor gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecRoots {
    pub state_root: [u8; 32],
    pub receipts_root: Option<[u8; 32]>,
    pub block_number: Option<u64>,
}

impl AnchorWitness {
    /// # Errors
    /// The header is missing, extra, malformed, or does not hash to the anchor.
    pub fn roots(&self) -> Result<ExecRoots, InclusionError> {
        match (&self.anchor, &self.header_rlp) {
            (Anchor::StateRoot(root), None) => Ok(ExecRoots {
                state_root: *root,
                receipts_root: None,
                block_number: None,
            }),
            (Anchor::BlockHash(hash), Some(rlp)) => {
                if keccak256(rlp).0 != *hash {
                    return Err(InclusionError::HeaderHash);
                }
                header_roots(rlp)
            }
            _ => Err(InclusionError::HeaderWitness),
        }
    }
}

/// State root (field 3), receipts root (field 5) and number (field 8) of an RLP execution header.
/// The field order is fixed since Frontier; later forks only append fields.
///
/// # Errors
/// Malformed RLP.
pub fn header_roots(rlp: &[u8]) -> Result<ExecRoots, InclusionError> {
    let mut buf = rlp;
    let h = Header::decode(&mut buf).map_err(|_| InclusionError::HeaderRlp("bad RLP"))?;
    if !h.list || buf.len() != h.payload_length {
        return Err(InclusionError::HeaderRlp("not a single RLP list"));
    }
    let mut state_root = [0u8; 32];
    let mut receipts_root = [0u8; 32];
    let mut number = 0u64;
    for i in 0..=8 {
        match i {
            3 => state_root = B256::decode(&mut buf).map_err(|_| InclusionError::HeaderRlp("state root"))?.0,
            5 => receipts_root = B256::decode(&mut buf).map_err(|_| InclusionError::HeaderRlp("receipts root"))?.0,
            8 => number = u64::decode(&mut buf).map_err(|_| InclusionError::HeaderRlp("number"))?,
            _ => skip_item(&mut buf).map_err(|()| InclusionError::HeaderRlp("field"))?,
        }
    }
    Ok(ExecRoots {
        state_root,
        receipts_root: Some(receipts_root),
        block_number: Some(number),
    })
}

fn skip_item(buf: &mut &[u8]) -> Result<(), ()> {
    let h = Header::decode(buf).map_err(|_| ())?;
    if buf.len() < h.payload_length {
        return Err(());
    }
    *buf = &buf[h.payload_length..];
    Ok(())
}

/// Keccak-256 (the backend the inclusion checks use).
#[must_use]
pub fn keccak(bytes: &[u8]) -> [u8; 32] {
    keccak256(bytes).0
}

/// The storage slot of a Solidity `mapping(K => V)` entry with a 32-byte key at slot index
/// `index`: `keccak256(abi.encode(key, uint256(index)))`.
#[must_use]
pub fn mapping_slot(key: &[u8; 32], index: u64) -> [u8; 32] {
    let mut pre = [0u8; 64];
    pre[..32].copy_from_slice(key);
    pre[32..].copy_from_slice(&U256::from(index).to_be_bytes::<32>());
    keccak256(pre).0
}

// ---------------------------------------------------------------------------------------------
// Predicates

/// A predicate over a 32-byte attested value (big-endian `uint256`, or a 32-byte field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Predicate {
    Eq([u8; 32]),
    Ne([u8; 32]),
}

impl Predicate {
    #[must_use]
    pub fn holds(&self, value: &[u8; 32]) -> bool {
        match self {
            Self::Eq(v) => v == value,
            Self::Ne(v) => v != value,
        }
    }

    const fn encode(&self) -> (u8, &[u8; 32]) {
        match self {
            Self::Eq(v) => (1, v),
            Self::Ne(v) => (2, v),
        }
    }
}

/// A `uint256` from a small integer, big-endian.
#[must_use]
pub fn word(v: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

// ---------------------------------------------------------------------------------------------
// Accounts and storage

/// The four account fields, as in the state trie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountFields {
    pub nonce: u64,
    pub balance: [u8; 32],
    pub storage_root: [u8; 32],
    pub code_hash: [u8; 32],
}

/// An `eth_getProof` account proof.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountProof {
    pub fields: AccountFields,
    pub nodes: Vec<Vec<u8>>,
}

/// An `eth_getProof` account proof plus one storage proof. `value` is the slot's actual value
/// (zero for an absent slot, proven by exclusion).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StorageProof {
    pub account: AccountProof,
    pub value: [u8; 32],
    pub nodes: Vec<Vec<u8>>,
}

fn bytes(nodes: &[Vec<u8>]) -> Vec<Bytes> {
    nodes.iter().map(|n| Bytes::copy_from_slice(n)).collect()
}

/// Checks the account proof under `state_root`.
///
/// # Errors
/// The proof does not show exactly these fields for `address`.
pub fn verify_account_proof(state_root: &[u8; 32], address: &[u8; 20], proof: &AccountProof) -> Result<(), InclusionError> {
    let key = keccak256(address);
    check_proof_nodes(&proof.nodes, &nibbles_of(key.as_slice()), MAX_STATE_NODE_LEN).map_err(|e| InclusionError::AccountProof(e.into()))?;
    let f = &proof.fields;
    let account = TrieAccount {
        nonce: f.nonce,
        balance: U256::from_be_bytes(f.balance),
        storage_root: B256::from(f.storage_root),
        code_hash: B256::from(f.code_hash),
    };
    verify_proof(
        B256::from(*state_root),
        Nibbles::unpack(key),
        Some(alloy_rlp::encode(&account)),
        &bytes(&proof.nodes),
    )
    .map_err(|e| InclusionError::AccountProof(format!("{e}")))
}

/// "Slot `slot` of contract `address` satisfies `predicate` at `anchor`."
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StorageStatement {
    pub context: StatementContext,
    pub address: [u8; 20],
    pub slot: [u8; 32],
    pub predicate: Predicate,
    pub anchor: Anchor,
}

impl StorageStatement {
    /// `SHA256(tag ‖ u64be(chain) ‖ instance ‖ address ‖ slot ‖ pred ‖ operand ‖ kind ‖ anchor)`.
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        let (p, operand) = self.predicate.encode();
        sha256(&[
            STORAGE_STMT_TAG,
            &self.context.evm_chain_id.to_be_bytes(),
            &self.context.instance_id,
            &self.address,
            &self.slot,
            &[p],
            operand,
            &[self.anchor.kind()],
            self.anchor.value(),
        ])
    }

    /// The same without the anchor and instance: one id per (chain, contract, slot, predicate),
    /// whichever finalized block proved it.
    #[must_use]
    pub fn fact_id(&self) -> [u8; 32] {
        let (p, operand) = self.predicate.encode();
        sha256(&[
            STORAGE_FACT_TAG,
            &self.context.evm_chain_id.to_be_bytes(),
            &self.address,
            &self.slot,
            &[p],
            operand,
        ])
    }
}

/// Verifies that storage slot `slot` of `address` holds `proof.value` under the anchor's state
/// root and that `predicate` holds of it.
///
/// # Errors
/// Any proof step fails, or the predicate does not hold.
pub fn verify_storage_slot(
    context: &StatementContext,
    witness: &AnchorWitness,
    address: &[u8; 20],
    slot: &[u8; 32],
    predicate: Predicate,
    proof: &StorageProof,
) -> Result<StorageStatement, InclusionError> {
    let roots = witness.roots()?;
    verify_account_proof(&roots.state_root, address, &proof.account)?;
    let key = keccak256(slot);
    check_proof_nodes(&proof.nodes, &nibbles_of(key.as_slice()), MAX_STATE_NODE_LEN).map_err(|e| InclusionError::StorageProof(e.into()))?;
    let value = U256::from_be_bytes(proof.value);
    let expected = if value.is_zero() { None } else { Some(alloy_rlp::encode(value)) };
    verify_proof(
        B256::from(proof.account.fields.storage_root),
        Nibbles::unpack(key),
        expected,
        &bytes(&proof.nodes),
    )
    .map_err(|e| InclusionError::StorageProof(format!("{e}")))?;
    if !predicate.holds(&proof.value) {
        return Err(InclusionError::PredicateFails);
    }
    Ok(StorageStatement {
        context: *context,
        address: *address,
        slot: *slot,
        predicate,
        anchor: witness.anchor,
    })
}

/// One account field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AccountField {
    Nonce,
    Balance,
    StorageRoot,
    CodeHash,
}

impl AccountField {
    fn of(self, f: &AccountFields) -> [u8; 32] {
        match self {
            Self::Nonce => word(u128::from(f.nonce)),
            Self::Balance => f.balance,
            Self::StorageRoot => f.storage_root,
            Self::CodeHash => f.code_hash,
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::Nonce => 1,
            Self::Balance => 2,
            Self::StorageRoot => 3,
            Self::CodeHash => 4,
        }
    }
}

/// "Field `field` of account `address` satisfies `predicate` at `anchor`."
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccountStatement {
    pub context: StatementContext,
    pub address: [u8; 20],
    pub field: AccountField,
    pub predicate: Predicate,
    pub anchor: Anchor,
}

impl AccountStatement {
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        let (p, operand) = self.predicate.encode();
        sha256(&[
            ACCOUNT_STMT_TAG,
            &self.context.evm_chain_id.to_be_bytes(),
            &self.context.instance_id,
            &self.address,
            &[self.field.tag()],
            &[p],
            operand,
            &[self.anchor.kind()],
            self.anchor.value(),
        ])
    }
}

/// Verifies an account field under the anchor's state root.
///
/// # Errors
/// The proof fails or the predicate does not hold.
pub fn verify_account_field(
    context: &StatementContext,
    witness: &AnchorWitness,
    address: &[u8; 20],
    field: AccountField,
    predicate: Predicate,
    proof: &AccountProof,
) -> Result<AccountStatement, InclusionError> {
    let roots = witness.roots()?;
    verify_account_proof(&roots.state_root, address, proof)?;
    if !predicate.holds(&field.of(&proof.fields)) {
        return Err(InclusionError::PredicateFails);
    }
    Ok(AccountStatement {
        context: *context,
        address: *address,
        field,
        predicate,
        anchor: witness.anchor,
    })
}

// ---------------------------------------------------------------------------------------------
// Receipt logs

/// One event log.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Log {
    pub address: [u8; 20],
    pub topics: Vec<[u8; 32]>,
    pub data: Vec<u8>,
}

/// A receipt as stored in the receipts trie (typed: `type ‖ rlp`, legacy: `rlp`) and its proof.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ReceiptProof {
    pub tx_index: u64,
    pub receipt: Vec<u8>,
    pub nodes: Vec<Vec<u8>>,
}

/// "Log `log_index` of transaction `tx_index` in the anchored block is `log`."
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct LogStatement {
    pub context: StatementContext,
    pub anchor: Anchor,
    pub tx_index: u64,
    pub log_index: u64,
    pub log: Log,
}

impl LogStatement {
    /// Includes the position, so two identical logs at different positions differ.
    #[must_use]
    pub fn id(&self) -> [u8; 32] {
        let topics: Vec<u8> = self.log.topics.concat();
        sha256(&[
            LOG_STMT_TAG,
            &self.context.evm_chain_id.to_be_bytes(),
            &self.context.instance_id,
            &[self.anchor.kind()],
            self.anchor.value(),
            &self.tx_index.to_be_bytes(),
            &self.log_index.to_be_bytes(),
            &self.log.address,
            &(self.log.topics.len() as u64).to_be_bytes(),
            &topics,
            &sha256(&[&self.log.data]),
        ])
    }
}

/// Verifies receipt `proof.tx_index` under the anchored header's receipts root and returns log
/// `log_index` of it. Needs a block-hash anchor (a state root has no receipts root). Only
/// successful receipts are accepted: a reverted transaction's logs are empty anyway.
///
/// # Errors
/// The proof or the receipt encoding is bad, the receipt failed, or there is no such log.
pub fn verify_receipt_log(
    context: &StatementContext,
    witness: &AnchorWitness,
    proof: &ReceiptProof,
    log_index: u64,
) -> Result<LogStatement, InclusionError> {
    let receipts_root = witness.roots()?.receipts_root.ok_or(InclusionError::NoReceiptsRoot)?;
    let key = alloy_rlp::encode(proof.tx_index);
    check_proof_nodes(&proof.nodes, &nibbles_of(&key), MAX_RECEIPT_NODE_LEN).map_err(|e| InclusionError::ReceiptProof(e.into()))?;
    verify_proof(
        B256::from(receipts_root),
        Nibbles::unpack(&key),
        Some(proof.receipt.clone()),
        &bytes(&proof.nodes),
    )
    .map_err(|e| InclusionError::ReceiptProof(format!("{e}")))?;
    let logs = receipt_logs(&proof.receipt)?;
    let log = usize::try_from(log_index)
        .ok()
        .and_then(|i| logs.into_iter().nth(i))
        .ok_or(InclusionError::NoSuchLog)?;
    Ok(LogStatement {
        context: *context,
        anchor: witness.anchor,
        tx_index: proof.tx_index,
        log_index,
        log,
    })
}

/// The logs of a successful receipt (`[status, cumulativeGasUsed, logsBloom, logs]`).
///
/// # Errors
/// Bad encoding or a failed (status 0) receipt.
pub fn receipt_logs(receipt: &[u8]) -> Result<Vec<Log>, InclusionError> {
    let first = *receipt.first().ok_or(InclusionError::Receipt("empty"))?;
    // EIP-2718: a typed receipt starts with its type byte (< 0x7f); a legacy one with a list.
    let mut buf = if first < 0x7f { &receipt[1..] } else { receipt };
    let h = Header::decode(&mut buf).map_err(|_| InclusionError::Receipt("bad RLP"))?;
    if !h.list || buf.len() != h.payload_length {
        return Err(InclusionError::Receipt("not a single RLP list"));
    }
    let status = Header::decode(&mut buf).map_err(|_| InclusionError::Receipt("status"))?;
    if status.list || status.payload_length > buf.len() {
        return Err(InclusionError::Receipt("status"));
    }
    let status_bytes = &buf[..status.payload_length];
    // Post-Byzantium status is 0 (empty string) or 1; a 32-byte post-state root is pre-Byzantium.
    if status.payload_length != 32 && status_bytes != [1] {
        return Err(InclusionError::Receipt("the transaction failed"));
    }
    buf = &buf[status.payload_length..];
    skip_item(&mut buf).map_err(|()| InclusionError::Receipt("cumulative gas"))?;
    skip_item(&mut buf).map_err(|()| InclusionError::Receipt("bloom"))?;
    let logs_h = Header::decode(&mut buf).map_err(|_| InclusionError::Receipt("logs"))?;
    if !logs_h.list || logs_h.payload_length > buf.len() {
        return Err(InclusionError::Receipt("logs"));
    }
    let mut logs_buf = &buf[..logs_h.payload_length];
    let mut out = Vec::new();
    while !logs_buf.is_empty() {
        let lh = Header::decode(&mut logs_buf).map_err(|_| InclusionError::Receipt("log"))?;
        if !lh.list || lh.payload_length > logs_buf.len() {
            return Err(InclusionError::Receipt("log"));
        }
        let mut l = &logs_buf[..lh.payload_length];
        logs_buf = &logs_buf[lh.payload_length..];
        let address: [u8; 20] = alloy_primitives::Address::decode(&mut l)
            .map_err(|_| InclusionError::Receipt("log address"))?
            .0
            .0;
        let th = Header::decode(&mut l).map_err(|_| InclusionError::Receipt("topics"))?;
        if !th.list || th.payload_length > l.len() {
            return Err(InclusionError::Receipt("topics"));
        }
        let mut tb = &l[..th.payload_length];
        l = &l[th.payload_length..];
        let mut topics = Vec::new();
        while !tb.is_empty() {
            topics.push(B256::decode(&mut tb).map_err(|_| InclusionError::Receipt("topic"))?.0);
        }
        let data = Bytes::decode(&mut l).map_err(|_| InclusionError::Receipt("log data"))?;
        if !l.is_empty() {
            return Err(InclusionError::Receipt("log has extra fields"));
        }
        out.push(Log { address, topics, data: data.to_vec() });
    }
    Ok(out)
}
