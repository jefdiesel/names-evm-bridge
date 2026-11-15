//! The journal the vault's `update(journal, seal)` checks: `abi.encode(UpdateJournal)` with
//!
//! ```solidity
//! struct ClaimDelta { address token; uint32 denomIndex; uint64 count; }
//! struct Exit { address token; uint32 denomIndex; bytes32 commitment; }
//! struct UpdateJournal {
//!     bytes32 cfgId; bytes32 prevState; bytes32 newState;
//!     bytes32[] depositRootsUsed; ClaimDelta[] claims; Exit[] exits;
//! }
//! ```
//!
//! The vault computes `sha256(journal)` and passes it to the verifier as the journal digest, so
//! the guest commits exactly these bytes (`commit_slice`).
//!
//! **`depositRootsUsed`.** On the stand-in path: every root `AttestRoot` recorded in the batch.
//! A stand-in claim can only assert a recorded root PDA, so this covers every root any claim
//! used, and the vault refuses the update unless each is one of its deposit roots: a lying
//! attestor is caught on Ethereum. On the RFP-021 path nothing goes there: the root is never
//! published (only the anchor), and there is no Ethereum-side check G2 could add for it. That
//! path's soundness is the claimer's own proven verification of `isDepositRoot[root]` under an
//! anchor the attestation instance finalized, and on Ethereum `claims <= deposits` per (token,
//! denomination) holds whatever LEZ says, because the vault counts both.
//!
//! **Versions.** [`JournalVersion::V1`] is today's vault. [`JournalVersion::V2`] appends
//! `bytes32 claimAccRoot; uint64 claimAccCount; address[] woundDown;` for the planned failsafe
//! vault; nothing emits it yet.

use crate::view::{ClaimDelta, EthAddress, Exit};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalVersion {
    /// `BridgeVault` as deployed (6 fields).
    V1,
    /// The failsafe vault (planned): V1 plus the accumulator and the wound-down tokens.
    V2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateJournal {
    pub cfg_id: [u8; 32],
    pub prev_state: [u8; 32],
    pub new_state: [u8; 32],
    pub deposit_roots_used: Vec<[u8; 32]>,
    pub claims: Vec<ClaimDelta>,
    pub exits: Vec<Exit>,
    /// V2 only.
    pub claim_acc_root: [u8; 32],
    pub claim_acc_count: u64,
    pub wound_down: Vec<EthAddress>,
}

fn word_u(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

fn word_a(a: &EthAddress) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

impl UpdateJournal {
    /// `abi.encode(j)` for the struct type of `version`.
    #[must_use]
    pub fn abi_encode(&self, version: JournalVersion) -> Vec<u8> {
        let v2 = version == JournalVersion::V2;
        // Head of the tuple: static fields inline, dynamic ones as offsets from the tuple start.
        let head_words = if v2 { 9 } else { 6 };
        let mut tails: Vec<Vec<u8>> = Vec::new();
        let mut t = word_u(self.deposit_roots_used.len() as u64).to_vec();
        for r in &self.deposit_roots_used {
            t.extend_from_slice(r);
        }
        tails.push(t);
        let mut t = word_u(self.claims.len() as u64).to_vec();
        for c in &self.claims {
            t.extend_from_slice(&word_a(&c.token));
            t.extend_from_slice(&word_u(u64::from(c.denom_index)));
            t.extend_from_slice(&word_u(c.count));
        }
        tails.push(t);
        let mut t = word_u(self.exits.len() as u64).to_vec();
        for e in &self.exits {
            t.extend_from_slice(&word_a(&e.token));
            t.extend_from_slice(&word_u(u64::from(e.denom_index)));
            t.extend_from_slice(&e.commitment);
        }
        tails.push(t);
        if v2 {
            let mut t = word_u(self.wound_down.len() as u64).to_vec();
            for a in &self.wound_down {
                t.extend_from_slice(&word_a(a));
            }
            tails.push(t);
        }
        let mut offsets = Vec::new();
        let mut at = 32 * head_words as u64;
        for t in &tails {
            offsets.push(word_u(at));
            at += t.len() as u64;
        }
        let mut out = Vec::with_capacity(32 + at as usize);
        out.extend_from_slice(&word_u(32)); // abi.encode of one dynamic tuple: its offset first
        out.extend_from_slice(&self.cfg_id);
        out.extend_from_slice(&self.prev_state);
        out.extend_from_slice(&self.new_state);
        out.extend_from_slice(&offsets[0]);
        out.extend_from_slice(&offsets[1]);
        out.extend_from_slice(&offsets[2]);
        if v2 {
            out.extend_from_slice(&self.claim_acc_root);
            out.extend_from_slice(&word_u(self.claim_acc_count));
            out.extend_from_slice(&offsets[3]);
        }
        for t in &tails {
            out.extend_from_slice(t);
        }
        out
    }

    /// Strict inverse of [`Self::abi_encode`] for V1 (the canonical layout only).
    #[must_use]
    pub fn abi_decode_v1(b: &[u8]) -> Option<Self> {
        let word = |i: usize| -> Option<[u8; 32]> { b.get(i..i + 32)?.try_into().ok() };
        let uint = |i: usize| -> Option<u64> {
            let w = word(i)?;
            if w[..24].iter().any(|&x| x != 0) {
                return None;
            }
            Some(u64::from_be_bytes(w[24..].try_into().ok()?))
        };
        let addr = |i: usize| -> Option<EthAddress> {
            let w = word(i)?;
            if w[..12].iter().any(|&x| x != 0) {
                return None;
            }
            w[12..].try_into().ok()
        };
        if uint(0)? != 32 {
            return None;
        }
        let base = 32usize;
        let (o_r, o_c, o_e) = (uint(base + 96)? as usize, uint(base + 128)? as usize, uint(base + 160)? as usize);
        let nr = uint(base + o_r)? as usize;
        let roots = (0..nr).map(|i| word(base + o_r + 32 + 32 * i)).collect::<Option<Vec<_>>>()?;
        let nc = uint(base + o_c)? as usize;
        let claims = (0..nc)
            .map(|i| {
                let p = base + o_c + 32 + 96 * i;
                Some(ClaimDelta { token: addr(p)?, denom_index: u32::try_from(uint(p + 32)?).ok()?, count: uint(p + 64)? })
            })
            .collect::<Option<Vec<_>>>()?;
        let ne = uint(base + o_e)? as usize;
        let exits = (0..ne)
            .map(|i| {
                let p = base + o_e + 32 + 96 * i;
                Some(Exit { token: addr(p)?, denom_index: u32::try_from(uint(p + 32)?).ok()?, commitment: word(p + 64)? })
            })
            .collect::<Option<Vec<_>>>()?;
        let j = Self {
            cfg_id: word(base)?,
            prev_state: word(base + 32)?,
            new_state: word(base + 64)?,
            deposit_roots_used: roots,
            claims,
            exits,
            claim_acc_root: [0; 32],
            claim_acc_count: 0,
            wound_down: Vec::new(),
        };
        (j.abi_encode(JournalVersion::V1) == b).then_some(j)
    }
}
