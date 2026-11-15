//! lcbench whole-update guest (spike, EXECUTOR ONLY). Each row is one complete verification of
//! the same finality update: instance id, committee binding, Helios's checks (vendored Helios,
//! `verify_generic_update_with`), the signature check, `apply_finality_update`, the new summary.
//! Only the signature check and the committee binding differ between rows.
//!
//! Spike limits: finality updates within the store's period only (no next committee, no
//! rotation); the branch-bound row reads the committee root from `prev.current_committee`.

use std::io::Read;

use alloy_primitives::B256;
use eth_attest_core::{
    BeaconHeader, ExecutionAnchor, Finalized, InstanceConfig, Instruction, LcSummary, LcUpdate, Message, committee_hash,
    instance_id,
};
use helios_consensus_core::{
    apply_finality_update,
    consensus_spec::MainnetConsensusSpec as S,
    types::{
        BeaconBlockHeader, FinalityUpdate, Fork, Forks, GenericUpdate, LightClientHeader, LightClientHeaderBellatrix,
        LightClientStore, SyncCommittee,
    },
    verify_finality_update, verify_generic_update_with,
};
use lcbench_common::{KEY_DEPTH, Keys, Report, ZkInput};
use lee_core::{account::AccountId, program::ReceiveInput};
use risc0_zkvm::guest::env;
use sha2::{Digest, Sha256};
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

#[derive(Clone, Copy, PartialEq)]
enum Path {
    /// Helios's own signature check (unpatched behaviour), committee by flat hash: today.
    Helios,
    /// zkcrypto, key witnesses, no redundant G2 check, committee by flat hash.
    Zk,
    /// blst, key witnesses, committee by flat hash (no on-chain change).
    BlstFlat,
    /// blst, key witnesses bound by SSZ branches to the committee root; no committee witness.
    BlstBranches,
}

// --- copied from attest/program/src/lc.rs (same crate family, MIT OR Apache-2.0) -------------

fn forks(c: &InstanceConfig) -> Forks {
    let k = |i: usize| Fork { epoch: c.forks[i].epoch, fork_version: c.forks[i].version.into() };
    Forks { genesis: k(0), altair: k(1), bellatrix: k(2), capella: k(3), deneb: k(4), electra: k(5), fulu: k(6), gloas: k(7) }
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
        LightClientHeader::Gloas(g) => ExecutionAnchor { block_hash: g.execution_block_hash.0, state_root: None, block_number: None },
        other => {
            let e = other.execution().map_err(|()| "a pre-Capella header has no execution payload".to_owned())?;
            ExecutionAnchor { block_hash: e.block_hash().0, state_root: Some(e.state_root().0), block_number: Some(*e.block_number()) }
        }
    };
    Ok(Finalized { beacon, beacon_root: b.tree_hash_root().0, execution })
}

// --- binding -------------------------------------------------------------------------------

fn sha256_pair(a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

fn key_leaf(k: &[u8; 48]) -> [u8; 32] {
    let mut b = [0u8; 64];
    b[..48].copy_from_slice(k);
    Sha256::digest(&b).into()
}

fn branches_ok(keys: &Keys, root: &[u8; 32]) -> bool {
    if &sha256_pair(&keys.pubkeys_root, &key_leaf(&keys.agg_compressed)) != root {
        return false;
    }
    keys.absent.iter().all(|w| {
        let mut node = key_leaf(&w.compressed);
        let i = usize::from(w.index);
        for (l, sib) in w.branch.iter().enumerate().take(KEY_DEPTH) {
            node = if (i >> l) & 1 == 1 { sha256_pair(sib, &node) } else { sha256_pair(&node, sib) };
        }
        node == keys.pubkeys_root
    })
}

// --- zkcrypto from witnesses ---------------------------------------------------------------

mod zk {
    use bls12_381::{
        G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt,
        hash_to_curve::{ExpandMsgXmd, HashToCurve},
        multi_miller_loop,
    };
    use lcbench_common::Keys;
    use sha2::Sha256;

    fn g1(unc: &[u8; 96], committed: &[u8; 48]) -> Option<G1Affine> {
        let p: G1Affine = Option::from(G1Affine::from_uncompressed_unchecked(unc))?;
        (bool::from(p.is_on_curve()) && !bool::from(p.is_identity()) && &p.to_compressed() == committed).then_some(p)
    }

    pub fn verify(keys: &Keys, bits: &[bool], committed: impl Fn(usize) -> [u8; 48], msg: &[u8; 32], sig: &[u8; 96]) -> bool {
        let Some(agg) = g1(&keys.agg_uncompressed, &committed(512)) else { return false };
        let mut absent = keys.absent.iter();
        let mut acc = G1Projective::identity();
        for (i, b) in bits.iter().enumerate() {
            if !b {
                let Some(w) = absent.next() else { return false };
                let Some(p) = (usize::from(w.index) == i).then(|| g1(&w.uncompressed, &committed(i))).flatten() else { return false };
                acc += G1Projective::from(p);
            }
        }
        if absent.next().is_some() {
            return false;
        }
        let pk = G1Affine::from(G1Projective::from(agg) - acc);
        if bool::from(pk.is_identity()) {
            return false;
        }
        let Some(s) = Option::<G2Affine>::from(G2Affine::from_compressed(sig)) else { return false };
        let h = G2Affine::from(<G2Projective as HashToCurve<ExpandMsgXmd<Sha256>>>::hash_to_curve(msg, super::DST));
        let neg_g1 = G1Affine::from(-G1Projective::generator());
        multi_miller_loop(&[(&neg_g1, &G2Prepared::from(s)), (&pk, &G2Prepared::from(h))]).final_exponentiation() == Gt::identity()
    }
}

// --- blst from witnesses -------------------------------------------------------------------

mod bl {
    use blst::{
        BLST_ERROR, blst_final_exp, blst_fp12, blst_fp12_is_one, blst_hash_to_g2, blst_miller_loop_n, blst_p1,
        blst_p1_add_or_double, blst_p1_add_or_double_affine, blst_p1_affine, blst_p1_affine_compress,
        blst_p1_affine_generator, blst_p1_affine_is_inf, blst_p1_cneg, blst_p1_deserialize, blst_p1_from_affine,
        blst_p1_to_affine, blst_p2, blst_p2_affine, blst_p2_affine_in_g2, blst_p2_affine_is_inf, blst_p2_to_affine,
        blst_p2_uncompress,
    };
    use lcbench_common::Keys;

    fn g1(unc: &[u8; 96], committed: &[u8; 48]) -> Option<blst_p1_affine> {
        let mut p = blst_p1_affine::default();
        if unsafe { blst_p1_deserialize(&mut p, unc.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
            return None;
        }
        let mut c = [0u8; 48];
        unsafe { blst_p1_affine_compress(c.as_mut_ptr(), &p) };
        (!unsafe { blst_p1_affine_is_inf(&p) } && &c == committed).then_some(p)
    }

    pub fn verify(keys: &Keys, bits: &[bool], committed: impl Fn(usize) -> [u8; 48], msg: &[u8; 32], sig: &[u8; 96]) -> bool {
        let Some(agg) = g1(&keys.agg_uncompressed, &committed(512)) else { return false };
        let mut absent = keys.absent.iter();
        let mut neg = blst_p1::default();
        for (i, b) in bits.iter().enumerate() {
            if !b {
                let Some(w) = absent.next() else { return false };
                let Some(p) = (usize::from(w.index) == i).then(|| g1(&w.uncompressed, &committed(i))).flatten() else { return false };
                let a = neg;
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
            blst_hash_to_g2(&mut hp, msg.as_ptr(), msg.len(), super::DST.as_ptr(), super::DST.len(), core::ptr::null(), 0);
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

// --- the whole update ------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn verify(
    path: Path,
    program: &AccountId,
    config: &InstanceConfig,
    prev: &LcSummary,
    committee_ssz: Option<&[u8]>,
    fin: &[u8],
    keys: &Keys,
) -> Result<LcSummary, String> {
    if instance_id(program, config) != prev.instance_id {
        return Err("the config is not this store's instance".into());
    }
    if prev.next_committee.is_some() {
        return Err("spike: in-period finality updates only".into());
    }
    let current = match (path, committee_ssz) {
        (Path::BlstBranches, None) => {
            if !branches_ok(keys, &prev.current_committee) {
                return Err("key witnesses are not in the committee".into());
            }
            SyncCommittee::<S>::default()
        }
        (Path::BlstBranches, Some(_)) | (_, None) => return Err("wrong binding".into()),
        (_, Some(ssz)) => {
            if committee_hash(ssz) != prev.current_committee {
                return Err("committee witness does not match the store".into());
            }
            SyncCommittee::<S>::from_ssz_bytes(ssz).map_err(|e| format!("committee: {e:?}"))?
        }
    };
    let header = LightClientHeader::Bellatrix(LightClientHeaderBellatrix { beacon: to_helios(&prev.finalized.beacon) });
    let mut store = LightClientStore::<S> {
        finalized_header: header.clone(),
        current_sync_committee: current,
        next_sync_committee: None,
        optimistic_header: header,
        previous_max_active_participants: 0,
        current_max_active_participants: 0,
        best_valid_update: None,
    };
    let u = FinalityUpdate::<S>::from_ssz_bytes(fin).map_err(|e| format!("finality update: {e:?}"))?;
    let g = GenericUpdate::from(&u);
    let n = g.sync_aggregate.sync_committee_bits.iter().filter(|b| *b).count();
    if n < usize::from(config.min_participants) {
        return Err("participation below policy".into());
    }
    let slot = g.signature_slot;
    let genesis = B256::from(config.genesis_validators_root);
    let fks = forks(config);
    if path == Path::Helios {
        verify_finality_update(&u, slot, &store, genesis, &fks).map_err(|e| format!("rejected: {e}"))?;
    } else {
        let cur = &store.current_sync_committee;
        verify_generic_update_with::<S, _>(&g, slot, &store, genesis, &fks, |committee, bits, signing_root, sig| {
            if !core::ptr::eq(committee, cur) {
                return Ok(false); // spike: signed by the next committee
            }
            let bits: Vec<bool> = bits.iter().collect();
            let sig: [u8; 96] = sig.as_ssz_bytes().try_into().map_err(|_| eyre_like())?;
            let msg = signing_root.0;
            Ok(match path {
                Path::Zk => zk::verify(keys, &bits, |i| ssz_key(committee_ssz.unwrap_or_default(), i), &msg, &sig),
                Path::BlstFlat => bl::verify(keys, &bits, |i| ssz_key(committee_ssz.unwrap_or_default(), i), &msg, &sig),
                Path::BlstBranches => bl::verify(keys, &bits, |i| witness_key(keys, i), &msg, &sig),
                Path::Helios => unreachable!(),
            })
        })
        .map_err(|e| format!("rejected: {e}"))?;
    }
    apply_finality_update(&mut store, &u);
    if store.next_sync_committee.is_some() {
        return Err("spike: no next committee".into());
    }
    let finalized = if store.finalized_header.beacon().slot == prev.finalized.beacon.slot {
        prev.finalized
    } else {
        finalized_of(&store.finalized_header)?
    };
    let next = LcSummary {
        instance_id: prev.instance_id,
        evm_chain_id: prev.evm_chain_id,
        finalized,
        current_committee: prev.current_committee,
        next_committee: None,
        updates: prev.updates + 1,
    };
    if !prev.is_advanced_by(&next) {
        return Err("does not advance the store".into());
    }
    Ok(next)
}

fn eyre_like() -> std::fmt::Error {
    std::fmt::Error
}

fn ssz_key(ssz: &[u8], i: usize) -> [u8; 48] {
    ssz.get(i * 48..i * 48 + 48).and_then(|s| s.try_into().ok()).unwrap_or([0xFF; 48])
}

fn witness_key(keys: &Keys, i: usize) -> [u8; 48] {
    if i == 512 {
        return keys.agg_compressed;
    }
    keys.absent.iter().find(|w| usize::from(w.index) == i).map_or([0xFF; 48], |w| w.compressed)
}

fn main() {
    let mut buf = Vec::new();
    env::stdin().read_to_end(&mut buf).unwrap();
    let inp: ZkInput = borsh::from_slice(&buf).unwrap();
    let mut r = Report { marks: Vec::new(), verdicts: Vec::new() };
    let mut last = env::cycle_count();
    let mut mark = |r: &mut Report, label: &str| {
        let now = env::cycle_count();
        r.marks.push((label.to_string(), now - last));
        last = env::cycle_count();
    };
    mark(&mut r, "setup: read + decode the bench input");
    let ri: ReceiveInput = borsh::from_slice(&inp.receive_input).unwrap();
    mark(&mut r, "setup: decode the ReceiveInput");
    let Ok(Message::Instruction(Instruction::Update { config, prev, current_committee, update: LcUpdate::Finality(fin), .. })) =
        borsh::from_slice::<Message>(&ri.message)
    else {
        panic!("not a finality Update");
    };
    let program = ri.receiver.program_account_id;
    mark(&mut r, "borsh-decode the Message (as handle() does)");

    let mut prev_b = prev.clone();
    prev_b.current_committee = inp.sig.keys.committee_root;
    let rows = [
        (Path::Helios, "whole: Helios signature path (today), committee by flat hash", &prev, Some(&current_committee[..])),
        (Path::Zk, "whole: zkcrypto, key witnesses, no extra G2 check, committee by flat hash", &prev, Some(&current_committee[..])),
        (Path::BlstFlat, "whole: blst, key witnesses, committee by flat hash (no on-chain change)", &prev, Some(&current_committee[..])),
        (Path::BlstBranches, "whole: blst, key witnesses by SSZ branch, committee root stored (no committee witness)", &prev_b, None),
    ];
    let mut first: Option<LcSummary> = None;
    for (path, label, p, ssz) in rows {
        let t0 = env::cycle_count();
        let out = verify(path, &program, &config, p, ssz, &fin, &inp.sig.keys);
        let t1 = env::cycle_count();
        r.marks.push((label.to_string(), t1 - t0));
        match out {
            Ok(next) => {
                let same = first.as_ref().is_none_or(|f| f.finalized == next.finalized);
                first.get_or_insert(next);
                r.verdicts.push((format!("{label}: verified, same finalized header"), same));
            }
            Err(e) => r.verdicts.push((format!("{label}: {e}"), false)),
        }
    }
    // Negative: one flipped signature byte must fail on the blst branch path.
    let mut bad = fin.clone();
    let u = FinalityUpdate::<S>::from_ssz_bytes(&fin).unwrap();
    let sig = GenericUpdate::from(&u).sync_aggregate.sync_committee_signature.as_ssz_bytes();
    if let Some(pos) = bad.windows(96).position(|w| w == &sig[..]) {
        bad[pos + 95] ^= 1;
    }
    let rejected = verify(Path::BlstBranches, &program, &config, &prev_b, None, &bad, &inp.sig.keys).is_err();
    r.verdicts.push(("negative: a flipped signature byte is rejected (blst, branches)".into(), rejected));
    env::commit_slice(borsh::to_vec(&r).unwrap().as_slice());
}
