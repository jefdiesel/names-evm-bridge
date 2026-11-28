//! The G2 state the vault stores as `g2State` (a commitment) and the next update must start from.

use crate::{finality::BlockRef, view::{BridgeView, EthAddress}};
use borsh::{BorshDeserialize, BorshSerialize};
use claim_acc::AccState;

/// Domain tag of the state commitment.
pub const STATE_TAG: &[u8] = b"RFP022/g2-state/v1";

/// The preimage of `g2State`. The vault stores only [`G2State::commitment`]; the prover keeps
/// this (and the view and the accumulator's leaves) to make the next update.
///
/// No cfg id here: the vault's `initialG2State` is a constructor argument, and its `cfgId`
/// depends on the vault's own address. The journal carries the cfg id the bridge derived at init
/// (from this state's `bridge_program`), and the vault checks it equals its own.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct G2State {
    /// Which finality source made this state (`finality::KIND_*`).
    pub finality_kind: u8,
    /// The finality source's own state (zero for `TrustedTip`).
    pub l1_state: [u8; 32],
    /// The bridge program whose transactions count (the vault's `programId`).
    pub bridge_program: [u8; 32],
    /// The last LEZ block read.
    pub tip: BlockRef,
    /// The claim-nullifier accumulator after every claim up to `tip`.
    pub claim_acc: AccState,
    /// `BridgeView::hash` of the bridge config as of `tip`.
    pub view_hash: [u8; 32],
    /// Exits recorded so far (the vault's `exitCount` once every update is applied).
    pub exit_count: u64,
    /// Tokens wound down by the failsafe: `bridge_core::GLOBAL` once G2 has read the bridge's
    /// wind-down (M6), kept in every later state; empty before.
    pub wound_down: Vec<EthAddress>,
}

impl G2State {
    /// The state before any update: at LEZ block `tip`, an empty accumulator, an uninitialised
    /// bridge view. `tip` must come before the bridge's `Init`.
    #[must_use]
    pub fn genesis(finality_kind: u8, bridge_program: [u8; 32], tip: BlockRef) -> Self {
        Self {
            finality_kind,
            l1_state: [0; 32],
            bridge_program,
            tip,
            claim_acc: AccState::empty(),
            view_hash: BridgeView::default().hash(),
            exit_count: 0,
            wound_down: Vec::new(),
        }
    }

    /// `SHA256(STATE_TAG | borsh(self))`: the vault's `g2State`.
    #[must_use]
    pub fn commitment(&self) -> [u8; 32] {
        crate::sha256(&[STATE_TAG, &borsh::to_vec(self).unwrap_or_default()])
    }
}
