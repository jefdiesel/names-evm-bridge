//! Guest A (consensus): Ethereum sync-committee light-client verification with
//! Helios consensus-core 0.12.0, unmodified, over SSZ inputs from the beacon API.
//!
//! bootstrap (checked against a trusted checkpoint root)
//!   -> N committee-period updates (verify_update + apply_update)
//!   -> one finality update (verify_finality_update + apply_finality_update)
//!   -> commit finalized header root, slot and execution commitment.
//!
//! Mode 1 additionally replays the BLS / SSZ steps of the finality update's signature
//! check directly on the same `bls12_381` crate (copied from Helios's private helpers,
//! MIT) so the cost can be broken down with `env::cycle_count()` deltas.
use alloy_primitives::{FixedBytes, B256};
use attest_common::{ConsensusOutput, ConsensusParams, Marks};
use helios_consensus_core::{
    apply_bootstrap, apply_finality_update, apply_update,
    consensus_spec::MainnetConsensusSpec as S,
    types::{
        Bootstrap, FinalityUpdate, Fork, Forks, GenericUpdate, LightClientHeader, LightClientStore,
        SyncCommittee, Update,
    },
    verify_bootstrap, verify_finality_update, verify_update,
};
use risc0_zkvm::guest::env;
use ssz::{Decode, Encode};
use tree_hash::TreeHash;

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

fn mk_forks(f: &[(u64, [u8; 4]); 8]) -> Forks {
    let k = |i: usize| Fork { epoch: f[i].0, fork_version: FixedBytes::from(f[i].1) };
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

fn main() {
    let p: ConsensusParams = env::read();
    let boot = env::read_frame();
    let ups: Vec<Vec<u8>> = (0..p.n_updates).map(|_| env::read_frame()).collect();
    let fin = env::read_frame();
    let forks = mk_forks(&p.forks);
    let genesis_root = B256::from(p.genesis_validators_root);
    let checkpoint = B256::from(p.checkpoint);
    let mut c = Clock::new();

    // --- bootstrap: trusted checkpoint -> store ---
    let bootstrap =
        Bootstrap::<S>::from_ssz_bytes(&boot).unwrap_or_else(|e| panic!("bootstrap decode: {e:?}"));
    c.mark("bootstrap: SSZ decode");
    verify_bootstrap(&bootstrap, checkpoint, &forks)
        .unwrap_or_else(|e| panic!("bootstrap rejected: {e}"));
    c.mark("bootstrap: verify_bootstrap (committee root + branch + header root)");
    let mut store = LightClientStore::<S>::default();
    apply_bootstrap(&mut store, &bootstrap);
    drop(bootstrap);
    c.mark("bootstrap: apply_bootstrap");

    // --- committee-period updates (carry next_sync_committee) ---
    for (i, u) in ups.iter().enumerate() {
        let upd = Update::<S>::from_ssz_bytes(u).unwrap_or_else(|e| panic!("update {i} decode: {e:?}"));
        c.mark(&format!("update[{i}]: SSZ decode"));
        verify_update(&upd, p.expected_current_slot, &store, genesis_root, &forks)
            .unwrap_or_else(|e| panic!("update {i} rejected: {e}"));
        c.mark(&format!("update[{i}]: verify_update"));
        let before = (store.finalized_header.beacon().slot, store.next_sync_committee.is_some());
        apply_update(&mut store, &upd);
        let after = (store.finalized_header.beacon().slot, store.next_sync_committee.is_some());
        // Helios's verify_* does not enforce the 2/3 supermajority; apply_* silently skips
        // an update without it. A guest must not commit an unchanged store as progress.
        assert!(after != before, "update {i} verified but not applied (no 2/3 supermajority or not newer)");
        c.mark(&format!("update[{i}]: apply_update"));
    }

    // --- finality update ---
    let fu = FinalityUpdate::<S>::from_ssz_bytes(&fin)
        .unwrap_or_else(|e| panic!("finality update decode: {e:?}"));
    c.mark("finality: SSZ decode");
    verify_finality_update(&fu, p.expected_current_slot, &store, genesis_root, &forks)
        .unwrap_or_else(|e| panic!("finality update rejected: {e}"));
    c.mark("finality: verify_finality_update");
    let want = fu.finalized_header().beacon().slot;
    apply_finality_update(&mut store, &fu);
    assert_eq!(
        store.finalized_header.beacon().slot,
        want,
        "finality update verified but not applied (no 2/3 supermajority?)"
    );
    c.mark("finality: apply_finality_update");

    if p.mode == 1 {
        breakdown(&fu, &store.current_sync_committee, genesis_root, &forks, &mut c);
    }

    // --- outputs ---
    let finalized_header_root: B256 = store.finalized_header.beacon().tree_hash_root();
    let (execution_kind, execution_commitment) = match &store.finalized_header {
        LightClientHeader::Gloas(h) => (0u8, h.execution_block_hash),
        h => (1u8, *h.execution().expect("pre-Capella header has no execution payload").state_root()),
    };
    c.mark("output: finalized header root + execution commitment");
    // Committee roots (what a LEZ account would persist instead of 512 keys) cost about as
    // much as one committee hash_tree_root each, so they are only computed in mode 1 and
    // reported separately; mode 0 is verification only.
    let (current_sync_committee_root, next_sync_committee_root) = if p.mode == 1 {
        let cur: B256 = store.current_sync_committee.tree_hash_root();
        let next: B256 = store.next_sync_committee.as_ref().map(|n| n.tree_hash_root()).unwrap_or_default();
        c.mark("output: committee roots (bookkeeping, not verification)");
        (cur, next)
    } else {
        (B256::ZERO, B256::ZERO)
    };

    env::commit(&ConsensusOutput {
        genesis_validators_root: p.genesis_validators_root,
        checkpoint: p.checkpoint,
        finalized_header_root: finalized_header_root.0,
        finalized_slot: store.finalized_header.beacon().slot,
        execution_kind,
        execution_commitment: execution_commitment.0,
        current_sync_committee_root: current_sync_committee_root.0,
        next_sync_committee_root: next_sync_committee_root.0,
        marks: c.marks,
    });
}

// ---------------------------------------------------------------------------------------
// Breakdown replay. Mirrors helios consensus-core 0.12.0 src/types/bls.rs, src/utils.rs
// (get_participating_aggregate_pubkey, calculate_fork_version, compute_committee_sign_root)
// step by step on the same crate versions. Cost measurement only; the verdict that counts
// is the unmodified Helios call above.
// ---------------------------------------------------------------------------------------
use bls12_381::{
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop, G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt, Scalar,
};
use sha2::{Digest, Sha256};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

fn g1(b: &[u8; 48]) -> G1Affine {
    Option::from(G1Affine::from_compressed(b)).expect("invalid G1")
}

/// Helios: if more than half participate, start from the committee aggregate and subtract
/// the non-participants; otherwise add the participants.
fn aggregate(agg: &[u8; 48], keys: &[[u8; 48]], participates: impl Fn(usize) -> bool) -> G1Affine {
    let n = keys.len();
    let count = (0..n).filter(|&i| participates(i)).count();
    if count > n / 2 {
        let mut acc = G1Projective::from(g1(agg));
        for i in 0..n {
            if !participates(i) {
                acc -= G1Projective::from(g1(&keys[i]));
            }
        }
        G1Affine::from(acc)
    } else {
        let mut acc = G1Projective::identity();
        for i in 0..n {
            if participates(i) {
                acc += G1Projective::from(g1(&keys[i]));
            }
        }
        G1Affine::from(acc)
    }
}

fn fork_version(forks: &Forks, slot: u64) -> [u8; 4] {
    let epoch = slot / 32;
    let f = if epoch >= forks.gloas.epoch {
        &forks.gloas
    } else if epoch >= forks.fulu.epoch {
        &forks.fulu
    } else if epoch >= forks.electra.epoch {
        &forks.electra
    } else if epoch >= forks.deneb.epoch {
        &forks.deneb
    } else if epoch >= forks.capella.epoch {
        &forks.capella
    } else if epoch >= forks.bellatrix.epoch {
        &forks.bellatrix
    } else if epoch >= forks.altair.epoch {
        &forks.altair
    } else {
        &forks.genesis
    };
    f.fork_version.0
}

/// First on-curve G2 point from a deterministic x sequence; almost surely outside the
/// prime-order subgroup (the G2 cofactor is huge). Used only to test Helios's check.
fn off_subgroup_g2() -> G2Affine {
    let mut ctr = 0u32;
    loop {
        let h: [u8; 32] = Sha256::digest(&ctr.to_le_bytes()).into();
        let mut b = [0u8; 96];
        b[0] = 0x80; // compressed flag; x.c1 top byte 0 so x.c1 < p
        b[1..33].copy_from_slice(&h);
        b[49..81].copy_from_slice(&h); // x.c0, top byte 0
        ctr += 1;
        if let Some(p) = Option::<G2Affine>::from(G2Affine::from_compressed_unchecked(&b)) {
            return p;
        }
    }
}

fn sha256_pair(a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

fn breakdown(
    fu: &FinalityUpdate<S>,
    committee: &SyncCommittee<S>,
    genesis_root: B256,
    forks: &Forks,
    c: &mut Clock,
) {
    let g = GenericUpdate::from(fu);
    c.mark("bd: GenericUpdate::from (clone; Helios does this inside verify and apply)");
    let keys: Vec<[u8; 48]> =
        committee.pubkeys.iter().map(|k| k.as_ssz_bytes().try_into().unwrap()).collect();
    let agg_key: [u8; 48] = committee.aggregate_pubkey.as_ssz_bytes().try_into().unwrap();
    c.mark("bd: (setup) copy 512 pubkey byte strings");

    let bits = &g.sync_aggregate.sync_committee_bits;
    let nonpart = bits.iter().filter(|b| !*b).count();
    let agg = aggregate(&agg_key, &keys, |i| bits.get(i).unwrap());
    c.mark(&format!("bd: aggregate pubkey, real bits ({nonpart} non-participants): decompress agg + subtract"));

    for k in keys.iter().take(16) {
        core::hint::black_box(g1(k));
    }
    c.mark("bd: 16 x G1 decompress incl. subgroup check (divide by 16 for per-key)");

    // Where the per-key cost goes (alternatives, not what Helios does): committee keys are
    // already authenticated by the committee's SSZ root, so a design could skip the
    // per-key subgroup check, or carry uncompressed points as a witness.
    for k in keys.iter().take(16) {
        core::hint::black_box(Option::<G1Affine>::from(G1Affine::from_compressed_unchecked(k)).unwrap());
    }
    c.mark("bd: (alt) 16 x G1 decompress WITHOUT subgroup check (sqrt only)");
    let unc: Vec<[u8; 96]> = keys.iter().take(16).map(|k| g1(k).to_uncompressed()).collect();
    c.mark("bd: (setup) 16 uncompressed witnesses");
    for u in &unc {
        let p = Option::<G1Affine>::from(G1Affine::from_uncompressed_unchecked(u)).unwrap();
        assert!(bool::from(p.is_on_curve()));
        core::hint::black_box(p);
    }
    c.mark("bd: (alt) 16 x G1 from uncompressed witness + on-curve check");

    let worst = core::hint::black_box(aggregate(&agg_key, &keys, |i| i >= 170));
    let _ = worst;
    c.mark("bd: aggregate pubkey, worst passing case (342/512 sign, 170 subtracted; cost only)");

    let header_root = g.attested_header.beacon().tree_hash_root();
    let mut version32 = [0u8; 32];
    version32[..4].copy_from_slice(&fork_version(forks, g.signature_slot.saturating_sub(1)));
    let fork_data_root = sha256_pair(&version32, genesis_root.as_slice());
    let mut domain = [0u8; 32];
    domain[..4].copy_from_slice(&[7, 0, 0, 0]);
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    let signing_root = sha256_pair(header_root.as_slice(), &domain);
    c.mark("bd: attested header hash_tree_root + fork data / domain / signing root");

    let sig_bytes: [u8; 96] =
        g.sync_aggregate.sync_committee_signature.as_ssz_bytes().try_into().unwrap();
    let sig: G2Affine =
        Option::from(G2Affine::from_compressed(&sig_bytes)).expect("signature not a G2 point");
    c.mark("bd: signature G2 decompress incl. subgroup check");

    // Helios subgroup_check_g2: sig * r == identity. Redundant after from_compressed.
    let r = Scalar::from_raw([
        0xffff_ffff_0000_0001,
        0x53bd_a402_fffe_5bfe,
        0x3339_d808_09a1_d805,
        0x73ed_a753_299d_7d48,
    ]);
    let in_subgroup: bool = (sig * r).is_identity().into();
    assert!(in_subgroup);
    c.mark("bd: Helios extra G2 subgroup check (sig * r)");
    {
        let q = off_subgroup_g2();
        let tf: bool = q.is_torsion_free().into();
        let id: bool = (q * r).is_identity().into();
        let rejected = bool::from(G2Affine::from_compressed(&q.to_compressed()).is_none());
        c.mark(&format!(
            "bd: probe (in zkVM): off-subgroup G2 point: torsion_free={tf}, Helios check (q*r==O)={id}, from_compressed rejects={rejected}"
        ));
    }

    let h = G2Affine::from(<G2Projective as HashToCurve<ExpandMsgXmd<Sha256>>>::hash_to_curve(
        signing_root,
        DST,
    ));
    c.mark("bd: hash_to_curve to G2 (expand_message_xmd SHA-256 + SSWU + clear cofactor)");

    let neg_g1 = G1Affine::from(-G1Projective::generator());
    let sp = G2Prepared::from(sig);
    let hp = G2Prepared::from(h);
    c.mark("bd: G2Prepared x2 (line coefficients)");
    let ml = multi_miller_loop(&[(&neg_g1, &sp), (&agg, &hp)]);
    c.mark("bd: multi Miller loop, 2 pairs");
    let ok = ml.final_exponentiation() == Gt::identity();
    c.mark("bd: final exponentiation + compare");
    assert!(ok, "breakdown replay: pairing check failed");

    let want: B256 = committee.tree_hash_root();
    c.mark("bd: SyncCommittee hash_tree_root via tree_hash 0.10 (512 keys + aggregate)");

    // Same root with direct SHA-256 calls (is the SHA accelerator on, and how much is
    // tree_hash overhead?). pubkey root = sha256(pk[0..32] || pk[32..48] ++ 16 zero bytes).
    let leaf = |k: &[u8; 48]| {
        let mut b = [0u8; 64];
        b[..48].copy_from_slice(k);
        let o: [u8; 32] = Sha256::digest(&b).into();
        o
    };
    let mut layer: Vec<[u8; 32]> = keys.iter().map(leaf).collect();
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|p| sha256_pair(&p[0], &p[1])).collect();
    }
    let direct = sha256_pair(&layer[0], &leaf(&agg_key));
    assert_eq!(B256::from(direct), want, "direct committee root mismatch");
    c.mark("bd: (alt) same SyncCommittee root with direct sha2 calls (1,025 sha256 of 64 bytes)");

    let blk = [7u8; 64];
    for _ in 0..1024 {
        core::hint::black_box(Sha256::digest(core::hint::black_box(&blk)));
    }
    c.mark("bd: 1024 x sha256(64 bytes), sha2 0.9 risczero (2 compressions each)");
}
