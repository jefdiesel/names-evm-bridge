//! The finality seam: which LEZ blocks count as final.
//!
//! G2 in full checks that every LEZ block it reads was inscribed in the LEZ channel on the Logos
//! Blockchain and that the inscribing L1 block is final (k deep under Cryptarchia's rules) from a
//! checkpoint. That is [`LogosBlockchainLightClient`], which is NOT implemented: it needs PoL
//! verification against ledger roots no header carries (`research/lez-and-l1-provability.md`).
//! Until then the only working source is [`TrustedTip`], compiled in only with the `trusted-tip`
//! feature (tests and the local round trip).
//!
//! Which source produced an update is bound twice: the source's [`FinalitySource::KIND`] is in
//! the G2 state preimage (so the vault's `initialG2State` and every later `g2State` commit to it),
//! and the guest image differs per source (the vault's immutable `updateImageId` names one).

use borsh::{BorshDeserialize, BorshSerialize};

/// `G2State::finality_kind` of [`TrustedTip`].
pub const KIND_TRUSTED_TIP: u8 = 1;
/// `G2State::finality_kind` of [`LogosBlockchainLightClient`].
pub const KIND_LOGOS_LIGHT_CLIENT: u8 = 2;

/// A LEZ block as the finality source sees it: id and recomputed hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockRef {
    pub id: u64,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinalityError {
    /// The witness does not decode.
    Witness,
    /// TrustedTip: the batch does not end at the tip the host named.
    NotTheTrustedTip,
    /// The Logos Blockchain light client does not exist yet.
    NotImplemented,
}

impl core::fmt::Display for FinalityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Witness => "witness does not decode",
            Self::NotTheTrustedTip => "the batch does not end at the trusted tip",
            Self::NotImplemented => "the Logos Blockchain light client is not implemented",
        })
    }
}

/// Decides that a batch of linked LEZ blocks is final, and carries an L1 state across updates.
pub trait FinalitySource {
    /// Stored in the G2 state; an update by another source cannot follow this one's state.
    const KIND: u8;
    /// `prev_l1`: the source's state in the previous G2 state (32 bytes). `blocks`: the batch,
    /// already linked to the previous LEZ tip. `witness`: the source's own input, borsh.
    /// Returns the new L1 state.
    fn finalize(prev_l1: &[u8; 32], blocks: &[BlockRef], witness: &[u8]) -> Result<[u8; 32], FinalityError>;
}

/// TEST ONLY: the host names a block (id, hash) and the guest accepts the batch as final if it
/// ends there. No cryptography: "final" means "the host said so". Labelled in every output of
/// the round trip ("LEZ finality trusted for the test"). Its L1 state stays as it was (zero).
#[cfg(feature = "trusted-tip")]
pub struct TrustedTip;

#[cfg(feature = "trusted-tip")]
impl FinalitySource for TrustedTip {
    const KIND: u8 = KIND_TRUSTED_TIP;

    fn finalize(prev_l1: &[u8; 32], blocks: &[BlockRef], witness: &[u8]) -> Result<[u8; 32], FinalityError> {
        let tip: BlockRef = borsh::from_slice(witness).map_err(|_| FinalityError::Witness)?;
        if blocks.last() != Some(&tip) {
            return Err(FinalityError::NotTheTrustedTip);
        }
        Ok(*prev_l1)
    }
}

/// The interface the Logos Blockchain light client will implement. Not implemented: always
/// refuses, so the release guest built with it can produce no journal today.
///
/// Its L1 state (`G2State::l1_state`) will commit to: the last finalized L1 block id and slot,
/// the epoch-nonce and stake-distribution state needed for the next PoL checks (or a ledger root
/// once a header carries one), and the LEZ channel's last inscription `MsgId`.
pub struct LogosBlockchainLightClient;

/// What the light client's witness will carry (designed, not implemented).
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
pub struct LightClientWitness {
    /// L1 headers from the previous finalized L1 block to the new tip, each with its leader
    /// proof (Groth16 BN254), leader key and Ed25519 signature (Blake2b ids).
    pub headers: Vec<Vec<u8>>,
    /// The ledger roots the PoL proofs are checked against, with their authentication (a header
    /// commitment under a future LIP, or a Mantle replay from the checkpoint).
    pub ledger_roots: Vec<Vec<u8>>,
    /// For each LEZ block of the batch: the channel inscription carrying its borsh bytes, the
    /// inscription's `parent MsgId`, and its inclusion proof under an L1 `body_root`.
    pub inscriptions: Vec<Vec<u8>>,
    /// k: how deep under the new tip an inscription must be to count as final.
    pub k: u64,
}

impl FinalitySource for LogosBlockchainLightClient {
    const KIND: u8 = KIND_LOGOS_LIGHT_CLIENT;

    /// To do, in this order: verify the header chain from `prev_l1` (PoL, leader signature, ids,
    /// epoch transitions, density rule); find the k-deep tip; check that the batch's blocks are
    /// exactly the channel's inscriptions after the previous `MsgId`, in order, each inscribed
    /// in a block at least k deep, with `blake2b`/Merkle inclusion under that block's body root.
    fn finalize(_prev_l1: &[u8; 32], _blocks: &[BlockRef], _witness: &[u8]) -> Result<[u8; 32], FinalityError> {
        Err(FinalityError::NotImplemented)
    }
}
