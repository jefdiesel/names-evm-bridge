//! The light-client verification, as pure functions over (config, store, material) -> new store.
//!
//! These run in the *submitter's* (or initialiser's) own actor, never in the instance's
//! accounts, so they can be executed privately and proved by whoever submits. Each is Helios
//! consensus-core 0.12.0, unmodified, plus the checks Helios leaves to its caller:
//! - the committee witness must hash to the store's commitments;
//! - participation must meet the instance's policy (at least a 2/3 supermajority);
//! - the store must actually advance (Helios's `apply_*` silently skips an update that lacks a
//!   supermajority or is not newer).

use alloy_primitives::B256;
use eth_attest_core::{
    BeaconHeader, ExecutionAnchor, Finalized, InstanceConfig, LcSummary, LcUpdate, committee_hash,
    instance_id,
};
use helios_consensus_core::{
    apply_bootstrap, apply_finality_update, apply_update,
    consensus_spec::MainnetConsensusSpec as S,
    types::{
        BeaconBlockHeader, Bootstrap, FinalityUpdate, Fork, Forks, GenericUpdate, LightClientHeader,
        LightClientHeaderBellatrix, LightClientStore, SyncCommittee, Update,
    },
    verify_bootstrap as helios_verify_bootstrap, verify_finality_update, verify_update as helios_verify_update,
};
use lee_core::account::AccountId;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

/// The outcome of a verified update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub next: LcSummary,
    /// The LEZ block time (ms) from which the update may be applied: the start of its signature
    /// slot. Helios's `expected_current_slot >= signature_slot` check, enforced by the sequencer
    /// through the response's timestamp window instead of a clock read.
    pub not_before_ms: u64,
}

fn forks(c: &InstanceConfig) -> Forks {
    let k = |i: usize| Fork {
        epoch: c.forks[i].epoch,
        fork_version: c.forks[i].version.into(),
    };
    Forks {
        genesis: k(0),
        altair: k(1),
        bellatrix: k(2),
        capella: k(3),
        deneb: k(4),
        electra: k(5),
        fulu: k(6),
        gloas: k(7),
    }
}

fn to_helios(b: &BeaconHeader) -> BeaconBlockHeader {
    BeaconBlockHeader {
        slot: b.slot,
        proposer_index: b.proposer_index,
        parent_root: B256::from(b.parent_root),
        state_root: B256::from(b.state_root),
        body_root: B256::from(b.body_root),
    }
}

fn finalized_of(h: &LightClientHeader) -> Result<Finalized, String> {
    let b = h.beacon();
    let beacon = BeaconHeader {
        slot: b.slot,
        proposer_index: b.proposer_index,
        parent_root: b.parent_root.0,
        state_root: b.state_root.0,
        body_root: b.body_root.0,
    };
    let execution = match h {
        LightClientHeader::Gloas(g) => ExecutionAnchor {
            block_hash: g.execution_block_hash.0,
            state_root: None,
            block_number: None,
        },
        other => {
            let e = other
                .execution()
                .map_err(|()| "a pre-Capella header has no execution payload".to_owned())?;
            ExecutionAnchor {
                block_hash: e.block_hash().0,
                state_root: Some(e.state_root().0),
                block_number: Some(*e.block_number()),
            }
        }
    };
    Ok(Finalized {
        beacon,
        beacon_root: b.tree_hash_root().0,
        execution,
    })
}

fn decode_committee(ssz: &[u8]) -> Result<SyncCommittee<S>, String> {
    SyncCommittee::<S>::from_ssz_bytes(ssz).map_err(|e| format!("committee witness does not decode: {e:?}"))
}

/// Verifies a bootstrap against the config's checkpoint and returns the instance's first store.
///
/// # Errors
/// Bad config, undecodable bootstrap, or a bootstrap Helios rejects.
pub fn verify_bootstrap(program: &AccountId, config: &InstanceConfig, bootstrap: &[u8]) -> Result<LcSummary, String> {
    config.validate();
    let b = Bootstrap::<S>::from_ssz_bytes(bootstrap).map_err(|e| format!("bootstrap does not decode: {e:?}"))?;
    helios_verify_bootstrap(&b, B256::from(config.checkpoint), &forks(config)).map_err(|e| format!("bootstrap rejected: {e}"))?;
    let mut store = LightClientStore::<S>::default();
    apply_bootstrap(&mut store, &b);
    let finalized = finalized_of(&store.finalized_header)?;
    Ok(LcSummary {
        instance_id: instance_id(program, config),
        evm_chain_id: config.evm_chain_id,
        finalized,
        current_committee: committee_hash(&store.current_sync_committee.as_ssz_bytes()),
        next_committee: None,
        updates: 0,
    })
}

/// Verifies one update against `prev` (with the committee witness) and returns the new store.
///
/// # Errors
/// Any check fails: wrong config or witness, undecodable update, participation below policy,
/// Helios rejects it, or it does not advance the store.
pub fn verify_update(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    current_committee: &[u8],
    next_committee: Option<&[u8]>,
    update: &LcUpdate,
) -> Result<Verified, String> {
    crate::mark!("lc: enter verify_update");
    if instance_id(program, config) != prev.instance_id {
        return Err("the config is not this store's instance".into());
    }
    crate::mark!("lc: instance id (sha256 of config)");
    if committee_hash(current_committee) != prev.current_committee {
        return Err("committee witness does not match the store (current)".into());
    }
    match (prev.next_committee, next_committee) {
        (None, None) => {}
        (Some(h), Some(c)) if committee_hash(c) == h => {}
        _ => return Err("committee witness does not match the store (next)".into()),
    }
    crate::mark!("lc: committee_hash of the 24,624-byte witness(es)");
    let current = decode_committee(current_committee)?;
    let next = next_committee.map(decode_committee).transpose()?;
    crate::mark!("lc: SSZ decode of the committee witness(es)");

    // Rebuild the Helios store from the summary. Helios's verify/apply read only the finalized
    // header's slot, the two committees, and (for the optimistic header and force_update, which
    // we never use) the remaining fields.
    let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix {
        beacon: to_helios(&prev.finalized.beacon),
    });
    let mut store = LightClientStore::<S> {
        finalized_header: header.clone(),
        current_sync_committee: current.clone(),
        next_sync_committee: next.clone(),
        optimistic_header: header,
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
        best_valid_update: None,
    };
    crate::mark!("lc: build the Helios store (committee clones)");

    let genesis_root = B256::from(config.genesis_validators_root);
    let forks = forks(config);
    let signature_slot = match update {
        LcUpdate::Finality(ssz) => {
            let u = FinalityUpdate::<S>::from_ssz_bytes(ssz).map_err(|e| format!("finality update does not decode: {e:?}"))?;
            crate::mark!("lc: SSZ decode of the update");
            check_participation(config, &GenericUpdate::from(&u))?;
            crate::mark!("lc: participation policy (GenericUpdate clone + popcount)");
            let slot = *u.signature_slot();
            helios_verify_finality(&u, slot, &store, genesis_root, &forks)?;
            crate::mark!("lc: Helios verify_finality_update");
            apply_finality_update(&mut store, &u);
            crate::mark!("lc: Helios apply_finality_update");
            slot
        }
        LcUpdate::Period(ssz) => {
            let u = Update::<S>::from_ssz_bytes(ssz).map_err(|e| format!("update does not decode: {e:?}"))?;
            crate::mark!("lc: SSZ decode of the update");
            check_participation(config, &GenericUpdate::from(&u))?;
            crate::mark!("lc: participation policy (GenericUpdate clone + popcount)");
            let slot = *u.signature_slot();
            helios_verify_update(&u, slot, &store, genesis_root, &forks).map_err(|e| format!("update rejected: {e}"))?;
            crate::mark!("lc: Helios verify_update");
            apply_update(&mut store, &u);
            crate::mark!("lc: Helios apply_update");
            slot
        }
    };

    // Map the new committees back to commitments without re-hashing the known ones.
    let current_hash = if store.current_sync_committee == current {
        prev.current_committee
    } else if next.as_ref() == Some(&store.current_sync_committee) {
        prev.next_committee.unwrap_or_else(|| unreachable!())
    } else {
        return Err("the current committee changed to an unknown one".into());
    };
    let next_hash = match &store.next_sync_committee {
        None => None,
        Some(c) if Some(c) == next.as_ref() => prev.next_committee,
        Some(c) => Some(committee_hash(&c.as_ssz_bytes())),
    };
    let finalized = if store.finalized_header.beacon().slot == prev.finalized.beacon.slot {
        prev.finalized
    } else {
        finalized_of(&store.finalized_header)?
    };
    let next_summary = LcSummary {
        instance_id: prev.instance_id,
        evm_chain_id: prev.evm_chain_id,
        finalized,
        current_committee: current_hash,
        next_committee: next_hash,
        updates: prev.updates.checked_add(1).expect("update count overflow"),
    };
    crate::mark!("lc: map committees back to commitments, new summary");
    if !prev.is_advanced_by(&next_summary) {
        return Err("update verified but does not advance the store (not newer, or no 2/3 supermajority)".into());
    }
    let start = config
        .genesis_time
        .checked_add(signature_slot.checked_mul(config.seconds_per_slot).ok_or("slot overflow")?)
        .ok_or("time overflow")?;
    Ok(Verified {
        next: next_summary,
        not_before_ms: start.checked_mul(1000).ok_or("time overflow")?,
    })
}

fn helios_verify_finality(
    u: &FinalityUpdate<S>,
    expected_current_slot: u64,
    store: &LightClientStore<S>,
    genesis_root: B256,
    forks: &Forks,
) -> Result<(), String> {
    verify_finality_update(u, expected_current_slot, store, genesis_root, forks).map_err(|e| format!("finality update rejected: {e}"))
}

fn check_participation(config: &InstanceConfig, u: &GenericUpdate<S>) -> Result<(), String> {
    let n = u.sync_aggregate.sync_committee_bits.iter().filter(|b| *b).count();
    if n < usize::from(config.min_participants) {
        return Err(format!(
            "participation {n} is below the instance's policy of {}",
            config.min_participants
        ));
    }
    Ok(())
}
