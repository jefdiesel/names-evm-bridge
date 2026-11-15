//! The claim-nullifier accumulator: an indexed Merkle tree.
//!
//! Depth [`DEPTH`] = 32, SHA-256, the vault's zero hashes (`zeros[0] = 0x00..00`,
//! `zeros[h + 1] = SHA256(zeros[h] | zeros[h])`, as `Sha256IncrementalTree.sol`), and the vault's
//! path convention (bit `h` of the index set means the node is a right child).
//!
//! Each leaf is a link of a sorted list over the inserted keys:
//!
//! ```text
//! leaf = SHA256("RFP022/acc-leaf" | key (32) | nextKey (32) | u64be(nextIndex))
//! ```
//!
//! Keys compare as big-endian `uint256`. `nextKey = 0` means +infinity (the last link). Index 0
//! holds the sentinel `(0, 0, 0)`; key 0 can never be inserted. Leaves are appended in insertion
//! order, so the inserted key number `n` (from 1) sits at index `n`.
//!
//! - **Insert `x`** ([`verify_insert`]): show the low leaf `L` (`L.key < x` and `x < L.nextKey` or
//!   `L.nextKey = 0`) under the current root; repoint it to `(L.key, x, n)`; show slot `n` empty
//!   under the root after that; write `(x, L.nextKey, L.nextIndex)` there. Two path updates, four
//!   path walks. A key already present has no low leaf (its predecessor's `nextKey` is `x`
//!   itself), so a duplicate is refused.
//! - **Non-membership of `x`** ([`verify_non_membership`]): the low leaf plus one path.
//! - **Membership of `x`** ([`verify_membership`]): its own leaf plus one path.
//!
//! Why this structure (G2, DESIGN §4): an insert costs 4 x 32 + 3 SHA-256 calls, against
//! 2 x 256 for a 256-deep sparse Merkle tree keyed by the nullifier; a non-membership proof is
//! one leaf and 32 siblings; and the node hashing is the vault's, so a Solidity verifier for the
//! failsafe reclaim reuses it. The root depends on insertion order, which is harmless: G2 inserts
//! in LEZ order, one batch after another.

use borsh::{BorshDeserialize, BorshSerialize};
use risc0_zkvm::sha::{Impl, Sha256 as _};
use std::collections::BTreeMap;

/// Tree depth: 2^32 leaves, as the vault's trees.
pub const DEPTH: usize = 32;
/// Domain tag of a leaf hash.
pub const LEAF_TAG: &[u8] = b"RFP022/acc-leaf";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(Impl::hash_bytes(bytes).as_bytes());
    out
}

/// `SHA256(a | b)`, a Merkle node.
#[must_use]
pub fn node(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(a);
    buf[32..].copy_from_slice(b);
    sha256(&buf)
}

/// `zeros[h]`: the root of an empty subtree of height `h` (empty leaf = 32 zero bytes).
#[must_use]
pub fn zero_hashes() -> [[u8; 32]; DEPTH + 1] {
    let mut z = [[0u8; 32]; DEPTH + 1];
    for h in 0..DEPTH {
        z[h + 1] = node(&z[h], &z[h]);
    }
    z
}

/// One link of the sorted list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Leaf {
    pub key: [u8; 32],
    /// The next larger key, or 0 for none (+infinity).
    pub next_key: [u8; 32],
    /// Index of the leaf holding `next_key` (0 when there is none).
    pub next_index: u64,
}

impl Leaf {
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        let mut buf = [0u8; 15 + 32 + 32 + 8];
        buf[..15].copy_from_slice(LEAF_TAG);
        buf[15..47].copy_from_slice(&self.key);
        buf[47..79].copy_from_slice(&self.next_key);
        buf[79..].copy_from_slice(&self.next_index.to_be_bytes());
        sha256(&buf)
    }

    /// `key < x < next_key` (or no next key): `x` is not in the list and belongs right after this
    /// link. Byte arrays compare lexicographically, which is big-endian `uint256` order.
    #[must_use]
    pub fn covers(&self, x: &[u8; 32]) -> bool {
        self.key < *x && (self.next_key == [0; 32] || *x < self.next_key)
    }
}

/// The root reached from `leaf_hash` at `index` along `path` (sibling at each level, leaf
/// first). `None` unless `path` has [`DEPTH`] siblings and `index < 2^DEPTH`.
#[must_use]
pub fn merkle_root(leaf_hash: &[u8; 32], index: u64, path: &[[u8; 32]]) -> Option<[u8; 32]> {
    if path.len() != DEPTH || index >= 1u64 << DEPTH {
        return None;
    }
    let mut n = *leaf_hash;
    for (h, s) in path.iter().enumerate() {
        n = if (index >> h) & 1 == 1 { node(s, &n) } else { node(&n, s) };
    }
    Some(n)
}

/// What the accumulator commits to: its root and the number of inserted keys (the sentinel not
/// counted; the next free index is `count + 1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AccState {
    pub root: [u8; 32],
    pub count: u64,
}

impl AccState {
    /// The tree holding only the sentinel.
    #[must_use]
    pub fn empty() -> Self {
        let z = zero_hashes();
        let path: Vec<[u8; 32]> = z[..DEPTH].to_vec();
        Self {
            root: merkle_root(&Leaf::default().hash(), 0, &path).unwrap_or_else(|| unreachable!()),
            count: 0,
        }
    }
}

/// Proof that `key` can be inserted, made by whoever holds the whole tree ([`Tree::insert`]).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InsertWitness {
    pub low_index: u64,
    pub low: Leaf,
    /// Path of the low leaf under the root before the insert.
    pub low_path: Vec<[u8; 32]>,
    /// Path of the new slot `count + 1` under the root after the low leaf was repointed.
    pub new_path: Vec<[u8; 32]>,
}

/// Proof that `key` is not in the tree: its low leaf and that leaf's path.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct NonMembership {
    pub low_index: u64,
    pub low: Leaf,
    pub path: Vec<[u8; 32]>,
}

/// Proof that `key` is in the tree: its own leaf and path.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Membership {
    pub index: u64,
    pub leaf: Leaf,
    pub path: Vec<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccError {
    /// Key 0 is the sentinel's.
    ZeroKey,
    /// The low leaf is not under the root at that index (or the path is malformed).
    LowNotInTree,
    /// The low leaf does not cover the key: the key is present, or the leaf is the wrong one.
    NotCovered,
    /// The new slot is not empty under the updated root (or its path is malformed).
    SlotNotEmpty,
    /// 2^32 - 1 keys inserted.
    Full,
}

impl core::fmt::Display for AccError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ZeroKey => "key 0 is reserved for the sentinel",
            Self::LowNotInTree => "low leaf not in the tree",
            Self::NotCovered => "low leaf does not cover the key (already present?)",
            Self::SlotNotEmpty => "new slot not empty",
            Self::Full => "tree full",
        })
    }
}

/// Checks an insertion of `key` into `state` and returns the new state. Everything is checked
/// against `state.root`; nothing in the witness is trusted.
pub fn verify_insert(state: &AccState, key: &[u8; 32], w: &InsertWitness) -> Result<AccState, AccError> {
    if *key == [0; 32] {
        return Err(AccError::ZeroKey);
    }
    let n = state.count.checked_add(1).ok_or(AccError::Full)?;
    if n >= 1u64 << DEPTH {
        return Err(AccError::Full);
    }
    if merkle_root(&w.low.hash(), w.low_index, &w.low_path) != Some(state.root) {
        return Err(AccError::LowNotInTree);
    }
    if !w.low.covers(key) {
        return Err(AccError::NotCovered);
    }
    let repointed = Leaf { key: w.low.key, next_key: *key, next_index: n };
    let mid = merkle_root(&repointed.hash(), w.low_index, &w.low_path).ok_or(AccError::LowNotInTree)?;
    if merkle_root(&[0; 32], n, &w.new_path) != Some(mid) {
        return Err(AccError::SlotNotEmpty);
    }
    let new = Leaf { key: *key, next_key: w.low.next_key, next_index: w.low.next_index };
    let root = merkle_root(&new.hash(), n, &w.new_path).ok_or(AccError::SlotNotEmpty)?;
    Ok(AccState { root, count: n })
}

/// True iff `w` proves `key` absent from the tree with root `root`.
#[must_use]
pub fn verify_non_membership(root: &[u8; 32], key: &[u8; 32], w: &NonMembership) -> bool {
    *key != [0; 32] && w.low.covers(key) && merkle_root(&w.low.hash(), w.low_index, &w.path) == Some(*root)
}

/// True iff `w` proves `key` present in the tree with root `root`.
#[must_use]
pub fn verify_membership(root: &[u8; 32], key: &[u8; 32], w: &Membership) -> bool {
    *key != [0; 32] && w.leaf.key == *key && merkle_root(&w.leaf.hash(), w.index, &w.path) == Some(*root)
}

/// The whole tree, kept by the host (G2 prover, wallet, reclaim client) to make witnesses.
#[derive(Clone, Debug)]
pub struct Tree {
    leaves: Vec<Leaf>,
    /// `levels[h][i]`: node `i` at height `h`, for the nodes that exist (others are zeros[h]).
    levels: Vec<Vec<[u8; 32]>>,
    by_key: BTreeMap<[u8; 32], u64>,
    zeros: [[u8; 32]; DEPTH + 1],
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    /// The tree holding only the sentinel.
    #[must_use]
    pub fn new() -> Self {
        let mut t = Self {
            leaves: Vec::new(),
            levels: vec![Vec::new(); DEPTH + 1],
            by_key: BTreeMap::new(),
            zeros: zero_hashes(),
        };
        t.push(Leaf::default());
        t.by_key.insert([0; 32], 0);
        t
    }

    /// Rebuilds a tree from its leaves in index order (as saved by [`Self::leaves`]).
    pub fn from_leaves(leaves: &[Leaf]) -> Result<Self, AccError> {
        let mut t = Self::new();
        for l in leaves.iter().skip(1) {
            t.insert(&l.key)?;
        }
        if t.leaves != leaves {
            return Err(AccError::LowNotInTree);
        }
        Ok(t)
    }

    #[must_use]
    pub fn leaves(&self) -> &[Leaf] {
        &self.leaves
    }

    #[must_use]
    pub fn state(&self) -> AccState {
        AccState { root: self.node(DEPTH, 0), count: self.leaves.len() as u64 - 1 }
    }

    #[must_use]
    pub fn contains(&self, key: &[u8; 32]) -> bool {
        *key != [0; 32] && self.by_key.contains_key(key)
    }

    fn node(&self, h: usize, i: u64) -> [u8; 32] {
        self.levels[h].get(i as usize).copied().unwrap_or(self.zeros[h])
    }

    fn set(&mut self, index: u64, leaf: Leaf) {
        let mut i = index as usize;
        let mut v = leaf.hash();
        for h in 0..=DEPTH {
            let level = &mut self.levels[h];
            if level.len() <= i {
                level.resize(i + 1, self.zeros[h]);
            }
            level[i] = v;
            if h == DEPTH {
                break;
            }
            let sib = self.node(h, (i ^ 1) as u64);
            v = if i & 1 == 1 { node(&sib, &v) } else { node(&v, &sib) };
            i >>= 1;
        }
    }

    fn push(&mut self, leaf: Leaf) {
        let i = self.leaves.len() as u64;
        self.leaves.push(leaf);
        self.set(i, leaf);
    }

    /// The 32 siblings of leaf `index`.
    #[must_use]
    pub fn path(&self, index: u64) -> Vec<[u8; 32]> {
        (0..DEPTH).map(|h| self.node(h, (index >> h) ^ 1)).collect()
    }

    fn low_index(&self, key: &[u8; 32]) -> u64 {
        // the largest key below `key` (the sentinel 0 is always there)
        *self.by_key.range(..*key).next_back().map(|(_, i)| i).unwrap_or(&0)
    }

    /// Inserts `key` and returns the witness [`verify_insert`] checks against the state before.
    pub fn insert(&mut self, key: &[u8; 32]) -> Result<InsertWitness, AccError> {
        if *key == [0; 32] {
            return Err(AccError::ZeroKey);
        }
        if self.by_key.contains_key(key) {
            return Err(AccError::NotCovered);
        }
        let n = self.leaves.len() as u64;
        if n >= 1u64 << DEPTH {
            return Err(AccError::Full);
        }
        let low_index = self.low_index(key);
        let low = self.leaves[low_index as usize];
        let low_path = self.path(low_index);
        let repointed = Leaf { key: low.key, next_key: *key, next_index: n };
        self.leaves[low_index as usize] = repointed;
        self.set(low_index, repointed);
        let new_path = self.path(n);
        self.push(Leaf { key: *key, next_key: low.next_key, next_index: low.next_index });
        self.by_key.insert(*key, n);
        Ok(InsertWitness { low_index, low, low_path, new_path })
    }

    /// Non-membership witness for an absent key; `None` if present or zero.
    #[must_use]
    pub fn non_membership(&self, key: &[u8; 32]) -> Option<NonMembership> {
        if *key == [0; 32] || self.by_key.contains_key(key) {
            return None;
        }
        let low_index = self.low_index(key);
        Some(NonMembership { low_index, low: self.leaves[low_index as usize], path: self.path(low_index) })
    }

    /// Membership witness for a present key.
    #[must_use]
    pub fn membership(&self, key: &[u8; 32]) -> Option<Membership> {
        let index = *self.by_key.get(key)?;
        if index == 0 {
            return None;
        }
        Some(Membership { index, leaf: self.leaves[index as usize], path: self.path(index) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> [u8; 32] {
        let mut o = [0u8; 32];
        for i in 0..32 {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    }

    fn key(b: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[31] = b;
        k
    }

    /// xorshift64*, so the property tests need no dependency and replay identically.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn key(&mut self) -> [u8; 32] {
            let mut k = [0u8; 32];
            for c in k.chunks_mut(8) {
                c.copy_from_slice(&self.next().to_be_bytes());
            }
            k
        }
    }

    /// Pinned vectors, computed independently with Python's hashlib (see the README of G2 in
    /// DESIGN): the zero hashes match the vault's, the sentinel leaf, the empty root, and the
    /// roots after inserting keys 5, 2, 9 (as 32-byte big-endian integers) in that order.
    #[test]
    fn pinned_vectors() {
        let z = zero_hashes();
        assert_eq!(z[1], h("f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"));
        assert_eq!(z[32], h("c6f67e02e6e4e1bdefb994c6098953f34636ba2b6ca20a4721d2b26a886722ff"));
        assert_eq!(Leaf::default().hash(), h(PIN_SENTINEL));
        assert_eq!(AccState::empty().root, h(PIN_EMPTY_ROOT));
        let mut t = Tree::new();
        assert_eq!(t.state(), AccState::empty());
        let mut roots = Vec::new();
        for k in [5u8, 2, 9] {
            t.insert(&key(k)).unwrap();
            roots.push(t.state().root);
        }
        assert_eq!(roots, [h(PIN_ROOT_5), h(PIN_ROOT_52), h(PIN_ROOT_529)]);
        // the sorted list: 0 -> 2 -> 5 -> 9 -> end, at indices 0, 2, 1, 3
        let l = t.leaves();
        assert_eq!(l[0], Leaf { key: [0; 32], next_key: key(2), next_index: 2 });
        assert_eq!(l[1], Leaf { key: key(5), next_key: key(9), next_index: 3 });
        assert_eq!(l[2], Leaf { key: key(2), next_key: key(5), next_index: 1 });
        assert_eq!(l[3], Leaf { key: key(9), next_key: [0; 32], next_index: 0 });
    }

    pub const PIN_SENTINEL: &str = "9c1ef29db24855da341f9ef71b2928fa50b6a614a139ade741d2030155ecbe41";
    pub const PIN_EMPTY_ROOT: &str = "f729f971d6f0f373c215df2fc06c1a08b54eafd2dc4a29edb9d60a4b0be00bf3";
    pub const PIN_ROOT_5: &str = "9f8665930b79f9b485292cfd11328b03f620e50a8791b92961ef445a0d1ffb8a";
    pub const PIN_ROOT_52: &str = "189a1c29cfa7718ed8dcea7da27eb6c55f18bcb895d5f0c7ee7dedaa564bbd63";
    pub const PIN_ROOT_529: &str = "a29780d54179df6c9fc871f710898fdbb09ddedfbc9df36e1dbdfe6cdad6711a";

    #[test]
    fn witnesses_verify_and_track_the_tree() {
        let mut t = Tree::new();
        let mut st = AccState::empty();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut keys = Vec::new();
        for _ in 0..300 {
            let k = rng.key();
            let w = t.insert(&k).unwrap();
            st = verify_insert(&st, &k, &w).unwrap();
            assert_eq!(st, t.state());
            keys.push(k);
        }
        // every inserted key: membership verifies, no non-membership witness exists or verifies
        for k in &keys {
            let m = t.membership(k).unwrap();
            assert!(verify_membership(&st.root, k, &m));
            assert!(t.non_membership(k).is_none());
            // the low leaf of k's predecessor does not cover k
            let pred = t.leaves()[t.low_index(k) as usize];
            let fake = NonMembership { low_index: t.low_index(k), low: pred, path: t.path(t.low_index(k)) };
            assert!(!verify_non_membership(&st.root, k, &fake));
            // nor does any other leaf
            for (i, l) in t.leaves().iter().enumerate().step_by(37) {
                let w = NonMembership { low_index: i as u64, low: *l, path: t.path(i as u64) };
                assert!(!verify_non_membership(&st.root, k, &w));
            }
        }
        // fresh keys: non-membership verifies, membership does not
        for _ in 0..300 {
            let k = rng.key();
            let w = t.non_membership(&k).unwrap();
            assert!(verify_non_membership(&st.root, &k, &w));
            assert!(t.membership(&k).is_none());
        }
        // the sorted list is complete and ordered
        let mut sorted = keys.clone();
        sorted.sort();
        let mut walk = Vec::new();
        let mut i = t.leaves()[0].next_index;
        while i != 0 {
            walk.push(t.leaves()[i as usize].key);
            i = t.leaves()[i as usize].next_index;
        }
        assert_eq!(walk, sorted);
        // a tree rebuilt from its leaves is the same tree
        let r = Tree::from_leaves(t.leaves()).unwrap();
        assert_eq!(r.state(), st);
    }

    #[test]
    fn duplicates_and_bad_witnesses_are_refused() {
        let mut t = Tree::new();
        let st0 = t.state();
        let w5 = t.insert(&key(5)).unwrap();
        let st1 = verify_insert(&st0, &key(5), &w5).unwrap();
        // the same key again: the tree refuses, and no witness from the tree verifies
        assert_eq!(t.insert(&key(5)), Err(AccError::NotCovered));
        for (i, l) in t.leaves().iter().enumerate() {
            let w = InsertWitness { low_index: i as u64, low: *l, low_path: t.path(i as u64), new_path: t.path(2) };
            assert!(verify_insert(&st1, &key(5), &w).is_err());
        }
        // a witness for one key does not insert another
        let mut t2 = t.clone();
        let w7 = t2.insert(&key(7)).unwrap();
        // (even 8, which has the same low leaf: the new slot's path holds the repointed low leaf,
        // which names 7)
        assert_eq!(verify_insert(&st1, &key(8), &w7), Err(AccError::SlotNotEmpty));
        assert!(verify_insert(&st1, &key(7), &w7).is_ok());
        assert_eq!(verify_insert(&st1, &key(3), &w7), Err(AccError::NotCovered));
        // replaying an old witness against a newer root fails
        assert_eq!(verify_insert(&st1, &key(5), &w5), Err(AccError::LowNotInTree));
        // tampering with any part fails
        let mut bad = w7.clone();
        bad.low.next_index ^= 1;
        assert!(verify_insert(&st1, &key(7), &bad).is_err());
        let mut bad = w7.clone();
        bad.low_path[3][0] ^= 1;
        assert!(verify_insert(&st1, &key(7), &bad).is_err());
        let mut bad = w7.clone();
        bad.new_path[0] = [1; 32];
        assert_eq!(verify_insert(&st1, &key(7), &bad), Err(AccError::SlotNotEmpty));
        let mut bad = w7;
        bad.low_path.pop();
        assert!(verify_insert(&st1, &key(7), &bad).is_err());
        // key 0 never
        assert_eq!(verify_insert(&st1, &[0; 32], &w5), Err(AccError::ZeroKey));
        assert!(t.non_membership(&[0; 32]).is_none());
    }

    #[test]
    fn insertion_order_changes_the_root_not_the_set() {
        let ks = [key(3), key(1), key(2)];
        let mut a = Tree::new();
        let mut b = Tree::new();
        for k in &ks {
            a.insert(k).unwrap();
        }
        for k in ks.iter().rev() {
            b.insert(k).unwrap();
        }
        assert_ne!(a.state().root, b.state().root);
        assert_eq!(a.state().count, b.state().count);
        for k in &ks {
            assert!(a.contains(k) && b.contains(k));
        }
    }
}
