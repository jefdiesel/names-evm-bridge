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

/// The public update path (`Instruction::UpdatePublic`, feature `public-update`): one finality
/// update, verified cheaply enough for a public transaction (`DESIGN.md` §9.3a).
///
/// Every Helios check runs, unchanged, through the vendored Helios's
/// `verify_generic_update_with` (`attest/vendor/helios-consensus-core`, `SPIKE-PATCH.diff`); only
/// the sync-committee signature check is ours ([`fast::verify`]):
/// - the signers' key is the committee's aggregate key minus the non-signers' keys, all given
///   uncompressed in `keys`; each must be on the curve, not infinity, and compress to the
///   committed bytes at its position in `current_committee`, which must hash to the store's
///   commitment. No key is decompressed; no per-key G1 subgroup check (committee keys passed a
///   proof-of-possession check at deposit; `DESIGN.md` §9.3a, to be confirmed against the
///   specs);
/// - the signature is decompressed and subgroup-checked once (Helios's extra `sig * r` check is
///   not repeated), hashed to G2, and the pairing checked with RISC Zero's blst fork in the guest.
///
/// Helios's store is rebuilt with an empty committee (the keys reach the check only through the
/// hook), so the committee SSZ is hashed but never decoded. Refused: a store that knows its next
/// committee, an update signed outside the store's period (Helios: invalid period), an update
/// finalized in the next period (does not advance), and any rotation.
///
/// # Errors
/// Any check fails.
#[cfg(feature = "public-update")]
pub fn verify_finality_public(
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    current_committee: &[u8],
    keys: &eth_attest_core::KeyWitnesses,
    update: &[u8],
) -> Result<Verified, String> {
    use helios_consensus_core::{apply_generic_update, verify_generic_update_with};
    if instance_id(program, config) != prev.instance_id {
        return Err("the config is not this store's instance".into());
    }
    if prev.next_committee.is_some() {
        return Err("the public path takes only a store whose next committee is unknown".into());
    }
    if current_committee.len() != eth_attest_core::COMMITTEE_SSZ_LEN || committee_hash(current_committee) != prev.current_committee {
        return Err("committee witness does not match the store (current)".into());
    }
    let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix {
        beacon: to_helios(&prev.finalized.beacon),
    });
    let mut store = LightClientStore::<S> {
        finalized_header: header.clone(),
        current_sync_committee: SyncCommittee::<S>::default(),
        next_sync_committee: None,
        optimistic_header: header,
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
        best_valid_update: None,
    };
    let u = FinalityUpdate::<S>::from_ssz_bytes(update).map_err(|e| format!("finality update does not decode: {e:?}"))?;
    let g = GenericUpdate::from(&u);
    check_participation(config, &g)?;
    let signature_slot = g.signature_slot;
    let current = &store.current_sync_committee;
    verify_generic_update_with::<S, _>(&g, signature_slot, &store, B256::from(config.genesis_validators_root), &forks(config), |committee, bits, signing_root, sig| {
        // With no next committee, Helios picks the current one; anything else is refused.
        if !core::ptr::eq(committee, current) {
            return Ok(false);
        }
        let bits: Vec<bool> = bits.iter().collect();
        let Ok(sig) = <[u8; 96]>::try_from(sig.as_ssz_bytes()) else { return Ok(false) };
        Ok(fast::verify(current_committee, keys, &bits, &signing_root.0, &sig))
    })
    .map_err(|e| format!("finality update rejected: {e}"))?;
    apply_generic_update(&mut store, &g);
    if store.next_sync_committee.is_some() {
        return Err("the public path does not learn a committee".into());
    }
    let finalized = if store.finalized_header.beacon().slot == prev.finalized.beacon.slot {
        prev.finalized
    } else {
        finalized_of(&store.finalized_header)?
    };
    let next_summary = LcSummary {
        instance_id: prev.instance_id,
        evm_chain_id: prev.evm_chain_id,
        finalized,
        current_committee: prev.current_committee,
        next_committee: None,
        updates: prev.updates.checked_add(1).ok_or("update count overflow")?,
    };
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

/// The key witnesses [`verify_finality_public`] needs for `bits` against this committee SSZ:
/// the aggregate key and each non-signer, uncompressed. Off-chain helper (the submitter builds
/// them); the program checks them, it never trusts them.
///
/// # Errors
/// The committee SSZ has the wrong length or a key does not decompress.
#[cfg(feature = "public-update")]
pub fn key_witnesses(committee_ssz: &[u8], bits: &[bool]) -> Result<eth_attest_core::KeyWitnesses, String> {
    if committee_ssz.len() != eth_attest_core::COMMITTEE_SSZ_LEN || bits.len() != 512 {
        return Err("committee SSZ or bits have the wrong length".into());
    }
    let unc = |i: usize| fast::uncompress(&committee_ssz[i * 48..i * 48 + 48]).ok_or_else(|| format!("committee key {i} does not decompress"));
    let mut absent = Vec::new();
    for (i, b) in bits.iter().enumerate() {
        if !b {
            absent.push((u16::try_from(i).map_err(|_| "index")?, unc(i)?));
        }
    }
    Ok(eth_attest_core::KeyWitnesses { aggregate: unc(512)?, absent })
}

/// The participation bits of a finality update (SSZ), for [`key_witnesses`].
///
/// # Errors
/// The update does not decode.
#[cfg(feature = "public-update")]
pub fn finality_bits(update: &[u8]) -> Result<Vec<bool>, String> {
    let u = FinalityUpdate::<S>::from_ssz_bytes(update).map_err(|e| format!("finality update does not decode: {e:?}"))?;
    Ok(GenericUpdate::from(&u).sync_aggregate.sync_committee_bits.iter().collect())
}

/// The signature check of the public path, with blst (RISC Zero's accelerated fork in the guest).
/// From the lcbench spike's `lc_bench_fast` guest (`bl::verify`), keys read from the committee SSZ.
#[cfg(feature = "public-update")]
pub mod fast {
    use blst::{
        BLST_ERROR, blst_final_exp, blst_fp12, blst_fp12_is_one, blst_hash_to_g2, blst_miller_loop_n, blst_p1,
        blst_p1_add_or_double, blst_p1_add_or_double_affine, blst_p1_affine, blst_p1_affine_compress,
        blst_p1_affine_generator, blst_p1_affine_is_inf, blst_p1_affine_serialize, blst_p1_cneg, blst_p1_deserialize,
        blst_p1_from_affine, blst_p1_to_affine, blst_p1_uncompress, blst_p2, blst_p2_affine, blst_p2_affine_in_g2,
        blst_p2_affine_is_inf, blst_p2_to_affine, blst_p2_uncompress,
    };
    use eth_attest_core::KeyWitnesses;

    /// The Ethereum BLS signature ciphersuite (proof of possession).
    pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

    /// Off-chain only: a compressed key, uncompressed (on the curve and in G1).
    #[must_use]
    pub fn uncompress(compressed: &[u8]) -> Option<[u8; 96]> {
        if compressed.len() != 48 {
            return None;
        }
        let mut p = blst_p1_affine::default();
        // SAFETY: 48 readable bytes, as checked above; blst writes only `p`.
        if unsafe { blst_p1_uncompress(&mut p, compressed.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
            return None;
        }
        let mut out = [0u8; 96];
        // SAFETY: `out` has the 96 bytes an uncompressed G1 point takes.
        unsafe { blst_p1_affine_serialize(out.as_mut_ptr(), &p) };
        Some(out)
    }

    /// An uncompressed key that is on the curve, not infinity, and compresses to `committed`.
    fn g1(unc: &[u8; 96], committed: &[u8]) -> Option<blst_p1_affine> {
        let mut p = blst_p1_affine::default();
        // SAFETY: `unc` is 96 readable bytes; blst writes only `p`.
        if unsafe { blst_p1_deserialize(&mut p, unc.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
            return None;
        }
        let mut c = [0u8; 48];
        // SAFETY: `c` has the 48 bytes a compressed G1 point takes.
        unsafe { blst_p1_affine_compress(c.as_mut_ptr(), &p) };
        // SAFETY: reads `p` only.
        (!unsafe { blst_p1_affine_is_inf(&p) } && c[..] == *committed).then_some(p)
    }

    /// Whether `sig` is the committee's signature of `msg` by exactly the signers in `bits`:
    /// e(-g1, sig) · e(aggregate − Σ non-signers, H(msg)) = 1.
    #[must_use]
    pub fn verify(committee: &[u8], keys: &KeyWitnesses, bits: &[bool], msg: &[u8; 32], sig: &[u8; 96]) -> bool {
        if committee.len() != 513 * 48 || bits.len() != 512 {
            return false;
        }
        let committed = |i: usize| &committee[i * 48..i * 48 + 48];
        let Some(agg) = g1(&keys.aggregate, committed(512)) else { return false };
        let mut absent = keys.absent.iter();
        let mut neg = blst_p1::default();
        for (i, b) in bits.iter().enumerate() {
            if !b {
                let Some((index, key)) = absent.next() else { return false };
                if usize::from(*index) != i {
                    return false;
                }
                let Some(p) = g1(key, committed(i)) else { return false };
                let a = neg;
                // SAFETY: valid points; blst writes only `neg`.
                unsafe { blst_p1_add_or_double_affine(&mut neg, &a, &p) };
            }
        }
        if absent.next().is_some() {
            return false;
        }
        let mut acc = blst_p1::default();
        let mut pk = blst_p1_affine::default();
        let mut ng = blst_p1::default();
        let mut ng_a = blst_p1_affine::default();
        let mut s = blst_p2_affine::default();
        let mut hp = blst_p2::default();
        let mut hm = blst_p2_affine::default();
        let mut f = blst_fp12::default();
        let mut e = blst_fp12::default();
        // SAFETY: every pointer is to a live local of the type blst expects; `sig` is 96 bytes,
        // `msg` 32, `DST` its own length; the two arrays passed to the Miller loop hold 2 each.
        unsafe {
            blst_p1_cneg(&mut neg, true);
            blst_p1_from_affine(&mut acc, &agg);
            let a = acc;
            blst_p1_add_or_double(&mut acc, &a, &neg);
            blst_p1_to_affine(&mut pk, &acc);
            if blst_p1_affine_is_inf(&pk) {
                return false;
            }
            if blst_p2_uncompress(&mut s, sig.as_ptr()) != BLST_ERROR::BLST_SUCCESS || !blst_p2_affine_in_g2(&s) || blst_p2_affine_is_inf(&s) {
                return false;
            }
            blst_hash_to_g2(&mut hp, msg.as_ptr(), msg.len(), DST.as_ptr(), DST.len(), core::ptr::null(), 0);
            blst_p2_to_affine(&mut hm, &hp);
            blst_p1_from_affine(&mut ng, blst_p1_affine_generator());
            blst_p1_cneg(&mut ng, true);
            blst_p1_to_affine(&mut ng_a, &ng);
            let qs: [*const blst_p2_affine; 2] = [&hm, &s];
            let ps: [*const blst_p1_affine; 2] = [&pk, &ng_a];
            blst_miller_loop_n(&mut f, qs.as_ptr(), ps.as_ptr(), 2);
            blst_final_exp(&mut e, &f);
            blst_fp12_is_one(&e)
        }
    }
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
