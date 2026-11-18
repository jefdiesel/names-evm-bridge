// SPDX-License-Identifier: MIT OR Apache-2.0
//! G4, the RFP-022 failsafe reclaim proof: the statement the zkVM guest (`methods/reclaim_guest`)
//! runs, as plain Rust so the native tests run the same code.
//!
//! # Where it fits
//!
//! After a vulnerability is declared, the failsafe vault (`vault/src/failsafe/BridgeVaultFailsafe.sol`)
//! is wound down: no deposits, and the first G2 update after that fixes `finalState`, the G2 state
//! commitment whose claim-nullifier accumulator is the final list of deposits claimed on LEZ.
//! A depositor whose deposit is not in that list takes it back on Ethereum with this proof.
//!
//! # Statement
//!
//! Private witness: the deposit master secret `s`, the LEZ account `dest` the deposit was made
//! for, `token`, `denom`, the `cfgId` preimage (`chainId`, `vault`, `lezChainId`, `zoneId`,
//! `programId`), the deposit's leaf index and 32-sibling path in the vault's deposit tree, the
//! final G2 state's PREIMAGE (`outbound_core::state::G2State`), and a non-membership witness in
//! its accumulator. Public, chosen when the proof is made: `recipient`, `relayer`, `relayFee`.
//!
//! ```text
//! cfgId       = SHA256(CFG_TAG | u256(chainId) | vault | lezChainId | zoneId | programId)
//! k           = SHA256("RFP022/ck" | s)                       (the claim secret; s is never published)
//! C           = SHA256("RFP022/dep" | k | dest | token | u256(denom) | cfgId)
//! leaf        = SHA256(DEPOSIT_LEAF_TAG | cfgId | token | u256(denom) | C)
//! depositRoot = the depth-32 root from leaf, index and path
//! nf_c        = SHA256("RFP022/nfc" | k | dest | token | u256(denom))
//! key         = for_public_pda(programId, nullifier_seed(nf_c))   (the G2 accumulator key)
//! require state.bridge_program == programId
//! require key NOT in the indexed Merkle tree with root state.claim_acc.root
//! finalState  = SHA256("RFP022/g2-state/v1" | borsh(state))     (the vault's g2State form)
//! nf_rc       = SHA256("RFP022/nfrc" | s)
//! journal     = abi.encode(cfgId, depositRoot, finalState, nf_rc, token, denom, recipient, relayer, relayFee)
//! ```
//!
//! The vault's `reclaim` hashes exactly those 288 bytes (`sha256`, RISC Zero's journal digest)
//! and checks: `cfgId` is its own, `depositRoot` is one of its deposit roots, `finalState` is the
//! state it fixed, `nf_rc` is unspent. Every variable that builds `C` and the leaf also builds the
//! accumulator key, and the program id in the key is the one `cfgId` binds, so a proof can only
//! show "not claimed" for the deposit it also shows is in the tree.
//!
//! Why `s` and not `k`: a public claim publishes `k` (and `dest`). A claim refused or reverted at
//! wind-down would leave `k` public and its nullifier unspent; with `k` alone anyone could reclaim
//! that deposit to themselves. `s` appears in no transaction (`lez-bridge/client/src/kdf.rs`).
//! Deposits made with a raw `k` (no master secret) cannot be reclaimed.

use bridge_core::EthAddress;
use claim_acc::NonMembership;
use lee_core::account::AccountId;
use outbound_core::state::G2State;
use risc0_zkvm::sha::{Impl, Sha256 as _};

/// A Solidity `uint256`, 32 bytes big-endian.
pub type U256 = [u8; 32];

/// `k = SHA256("RFP022/ck" | s)` (`bridge_client::kdf::CK_DOMAIN`).
pub const CK_DOMAIN: &[u8] = b"RFP022/ck";
/// The reclaim nullifier `SHA256("RFP022/nfrc" | s)` (`bridge_client::kdf::NFRC_DOMAIN`).
pub const NFRC_DOMAIN: &[u8] = b"RFP022/nfrc";
/// Depth of the vault's deposit tree.
pub const TREE_DEPTH: usize = 32;
/// `abi.encode` of 9 static words.
pub const JOURNAL_LEN: usize = 9 * 32;

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

#[must_use]
pub fn claim_secret(master: &[u8; 32]) -> [u8; 32] {
    sha256(&[CK_DOMAIN, master])
}

#[must_use]
pub fn reclaim_nullifier(master: &[u8; 32]) -> [u8; 32] {
    sha256(&[NFRC_DOMAIN, master])
}

/// The accumulator key G2 inserts when this deposit is claimed: the claim-nullifier PDA's id.
#[must_use]
pub fn acc_key(program_id: &[u8; 32], k: &[u8; 32], dest: &[u8; 32], token: &EthAddress, denom: u128) -> [u8; 32] {
    let nf = bridge_core::claim_nullifier(k, &AccountId::new(*dest), token, denom);
    *bridge_core::nullifier_address(&AccountId::new(*program_id), &nf).value()
}

/// The `cfgId` preimage (`BridgeVault.computeCfgId`).
#[derive(Clone, Debug, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct CfgPreimage {
    pub chain_id: u64,
    pub vault: EthAddress,
    pub lez_chain_id: [u8; 32],
    pub zone_id: [u8; 32],
    pub program_id: [u8; 32],
}

impl CfgPreimage {
    #[must_use]
    pub fn cfg_id(&self) -> [u8; 32] {
        bridge_core::cfg_id(self.chain_id, &self.vault, &self.lez_chain_id, &self.zone_id, &AccountId::new(self.program_id))
    }
}

/// The guest's whole input (borsh, read as `u32le(len) | bytes`).
#[derive(Clone, Debug, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct ReclaimInput {
    // ---- private
    pub master: [u8; 32],
    pub dest: [u8; 32],
    pub token: EthAddress,
    pub denom: u128,
    pub cfg: CfgPreimage,
    pub leaf_index: u64,
    pub path: Vec<[u8; 32]>,
    /// The preimage of the vault's `finalState`.
    pub state: G2State,
    pub non_membership: NonMembership,
    // ---- public (bound into the journal)
    pub recipient: EthAddress,
    pub relayer: EthAddress,
    pub relay_fee: u128,
}

/// The guest's public output, in `BridgeVaultFailsafe.reclaim`'s digest order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimJournal {
    pub cfg_id: [u8; 32],
    pub deposit_root: [u8; 32],
    pub final_state: [u8; 32],
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

impl ReclaimJournal {
    /// `abi.encode(cfgId, depositRoot, finalState, nullifier, token, denom, recipient, relayer,
    /// relayFee)`: nine 32-byte words, addresses left-padded with 12 zero bytes.
    #[must_use]
    pub fn abi_encode(&self) -> [u8; JOURNAL_LEN] {
        let words: [[u8; 32]; 9] = [
            self.cfg_id,
            self.deposit_root,
            self.final_state,
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

    /// Strict inverse of [`Self::abi_encode`].
    #[must_use]
    pub fn abi_decode(b: &[u8]) -> Option<Self> {
        if b.len() != JOURNAL_LEN {
            return None;
        }
        let w = |i: usize| -> [u8; 32] { b[32 * i..32 * (i + 1)].try_into().unwrap_or([0; 32]) };
        Some(Self {
            cfg_id: w(0),
            deposit_root: w(1),
            final_state: w(2),
            nullifier: w(3),
            token: word_addr(&b[128..160])?,
            denom: w(5),
            recipient: word_addr(&b[192..224])?,
            relayer: word_addr(&b[224..256])?,
            relay_fee: w(8),
        })
    }

    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        sha256(&[&self.abi_encode()])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclaimError {
    /// The deposit path is not 32 siblings or the index is outside the tree.
    BadPath,
    /// The final state belongs to another bridge program than the one `cfgId` binds.
    WrongProgram,
    /// The deposit's claim nullifier is in the final accumulator (it was claimed on LEZ), or the
    /// witness is not a valid non-membership witness under the state's accumulator root.
    Claimed,
}

impl core::fmt::Display for ReclaimError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::BadPath => "deposit path is not 32 siblings inside the tree",
            Self::WrongProgram => "the final state is another bridge program's",
            Self::Claimed => "not shown unclaimed: the claim nullifier is in the final accumulator (or the witness is bad)",
        })
    }
}

/// The whole G4 statement.
pub fn reclaim(i: &ReclaimInput) -> Result<ReclaimJournal, ReclaimError> {
    if i.state.bridge_program != i.cfg.program_id {
        return Err(ReclaimError::WrongProgram);
    }
    let cfg_id = i.cfg.cfg_id();
    let k = claim_secret(&i.master);
    let dest = AccountId::new(i.dest);
    let c = bridge_core::deposit_commitment(&k, &dest, &i.token, i.denom, &cfg_id);
    let leaf = bridge_core::deposit_leaf(&cfg_id, &i.token, i.denom, &c);
    let deposit_root = claim_acc::merkle_root(&leaf, i.leaf_index, &i.path).ok_or(ReclaimError::BadPath)?;
    let key = acc_key(&i.cfg.program_id, &k, &i.dest, &i.token, i.denom);
    if !claim_acc::verify_non_membership(&i.state.claim_acc.root, &key, &i.non_membership) {
        return Err(ReclaimError::Claimed);
    }
    Ok(ReclaimJournal {
        cfg_id,
        deposit_root,
        final_state: i.state.commitment(),
        nullifier: reclaim_nullifier(&i.master),
        token: i.token,
        denom: u256(i.denom),
        recipient: i.recipient,
        relayer: i.relayer,
        relay_fee: u256(i.relay_fee),
    })
}

/// `u32le(len) | borsh(input)`, zero-padded to a multiple of 4 (the guest reads it with two
/// `read_slice` calls).
#[must_use]
pub fn encode_input(i: &ReclaimInput) -> Vec<u8> {
    let b = borsh::to_vec(i).unwrap_or_default();
    let mut out = Vec::with_capacity(b.len() + 8);
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(&b);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_roundtrip_and_padding() {
        let j = ReclaimJournal {
            cfg_id: [1; 32],
            deposit_root: [2; 32],
            final_state: [3; 32],
            nullifier: [4; 32],
            token: [5; 20],
            denom: u256(6),
            recipient: [7; 20],
            relayer: [8; 20],
            relay_fee: u256(9),
        };
        let b = j.abi_encode();
        assert_eq!(b.len(), 288);
        assert_eq!(&b[128..140], &[0u8; 12]);
        assert_eq!(&b[140..160], &[5u8; 20]);
        assert_eq!(ReclaimJournal::abi_decode(&b), Some(j));
        let mut bad = b;
        bad[200] = 1;
        assert_eq!(ReclaimJournal::abi_decode(&bad), None);
        assert_eq!(ReclaimJournal::abi_decode(&b[..287]), None);
    }
}
