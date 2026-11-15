//! lcbench zkcrypto guest (spike, EXECUTOR ONLY). One session:
//!
//! A. the attestation program's own `handle()` on the update's `ReceiveInput`, with the phase
//!    marks `lc::verify_update` records under the `cycle-marks` feature;
//! B. the signature check exactly as Helios does it (decompress + subgroup-check the aggregate
//!    and each non-signer, subtract; decompress the signature; Helios's extra `sig * r` check;
//!    hash to G2; two prepared G2 points; multi Miller loop; final exponentiation);
//! C. the same check without the redundant G2 check, keys from uncompressed witnesses (on curve,
//!    not the identity, recompressed equal to the committed bytes; no decompression and no
//!    per-key subgroup check);
//! D. committee binding: SSZ branches for the aggregate and the non-signers against the
//!    committee root, and the full committee root by direct SHA-256;
//! E. the 342/512 floor (170 non-signers, synthetic bits on the real committee): C's key path and
//!    D's branches, cost only;
//! F. SHA-256: patched sha2 0.9, the zkVM's `sha::Impl`, and a plain software control.

use std::io::Read;

use bls12_381::{
    G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt, Scalar,
    hash_to_curve::{ExpandMsgXmd, HashToCurve},
    multi_miller_loop,
};
use lcbench_common::{KEY_DEPTH, Keys, Report, SigCase, ZkInput};
use lee_core::program::ReceiveInput;
use risc0_zkvm::guest::env;
use risc0_zkvm::sha::{Impl, Sha256 as _};
use sha2::{Digest, Sha256};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

struct Clock {
    last: u64,
    r: Report,
}

impl Clock {
    fn mark(&mut self, label: &str) {
        let now = env::cycle_count();
        self.r.marks.push((label.to_string(), now - self.last));
        self.last = env::cycle_count();
    }
    fn verdict(&mut self, label: &str, ok: bool) {
        self.r.verdicts.push((label.to_string(), ok));
        self.last = env::cycle_count();
    }
}

fn committed_key(committee_ssz: &[u8], i: usize) -> [u8; 48] {
    committee_ssz[i * 48..i * 48 + 48].try_into().unwrap()
}

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

// --- B: Helios's method ---------------------------------------------------------------

fn g1_helios(b: &[u8; 48]) -> Option<G1Affine> {
    Option::from(G1Affine::from_compressed(b))
}

fn agg_helios(committee_ssz: &[u8], bits: &[bool]) -> Option<G1Affine> {
    // Helios `get_participating_aggregate_pubkey`, more-than-half branch.
    let mut acc = G1Projective::from(g1_helios(&committed_key(committee_ssz, 512))?);
    for (i, b) in bits.iter().enumerate() {
        if !b {
            acc -= G1Projective::from(g1_helios(&committed_key(committee_ssz, i))?);
        }
    }
    Some(G1Affine::from(acc))
}

// --- C: uncompressed witnesses ----------------------------------------------------------

/// The witness point, if it is on the curve, not the identity, and compresses to `committed`.
/// No subgroup check (see the spike report on why the committee's keys need none).
fn g1_witness(unc: &[u8; 96], committed: &[u8; 48]) -> Option<G1Affine> {
    let p: G1Affine = Option::from(G1Affine::from_uncompressed_unchecked(unc))?;
    let ok = bool::from(p.is_on_curve()) && !bool::from(p.is_identity()) && &p.to_compressed() == committed;
    ok.then_some(p)
}

/// Aggregate minus the non-signers, every key from its witness, checked against the committed
/// bytes in `committed(i)` (i = 512 is the aggregate). `bits` must be the update's bits: each
/// zero bit consumes the next witness, in order.
fn agg_witness(keys: &Keys, bits: &[bool], committed: impl Fn(usize) -> [u8; 48]) -> Option<G1Affine> {
    let agg = g1_witness(&keys.agg_uncompressed, &committed(512))?;
    let mut absent = keys.absent.iter();
    let mut acc = G1Projective::identity();
    for (i, b) in bits.iter().enumerate() {
        if !b {
            let w = absent.next()?;
            if usize::from(w.index) != i {
                return None;
            }
            acc += G1Projective::from(g1_witness(&w.uncompressed, &committed(i))?);
        }
    }
    if absent.next().is_some() {
        return None;
    }
    let signers = G1Affine::from(G1Projective::from(agg) - acc);
    (!bool::from(signers.is_identity())).then_some(signers)
}

// --- D: SSZ branches --------------------------------------------------------------------

/// The witnessed keys' committed bytes are in the committee whose SSZ root is `committee_root`.
fn branches_ok(keys: &Keys) -> bool {
    if sha256_pair(&keys.pubkeys_root, &key_leaf(&keys.agg_compressed)) != keys.committee_root {
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

/// Branch binding for the keys path: the committed bytes come from the branch-checked witness.
fn committed_from_witness(keys: &Keys) -> impl Fn(usize) -> [u8; 48] + '_ {
    move |i| {
        if i == 512 {
            keys.agg_compressed
        } else {
            keys.absent.iter().find(|w| usize::from(w.index) == i).map_or([0xFF; 48], |w| w.compressed)
        }
    }
}

// --- pairing pieces ---------------------------------------------------------------------

fn hash_to_g2(msg: &[u8; 32]) -> G2Affine {
    G2Affine::from(<G2Projective as HashToCurve<ExpandMsgXmd<Sha256>>>::hash_to_curve(msg, DST))
}

fn pairing_ok(pk: &G1Affine, h: G2Affine, sig: G2Affine, c: &mut Clock, tag: &str) -> bool {
    let neg_g1 = G1Affine::from(-G1Projective::generator());
    let sp = G2Prepared::from(sig);
    let hp = G2Prepared::from(h);
    c.mark(&format!("{tag}: G2Prepared x2 (line coefficients)"));
    let ml = multi_miller_loop(&[(&neg_g1, &sp), (pk, &hp)]);
    c.mark(&format!("{tag}: multi Miller loop, 2 pairs"));
    let ok = ml.final_exponentiation() == Gt::identity();
    c.mark(&format!("{tag}: final exponentiation + compare"));
    ok
}

// --- F: software SHA-256 control ----------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
    0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
    0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

fn sw_compress(s: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *s;
    for i in 0..64 {
        let t1 = h
            .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
            .wrapping_add((e & f) ^ (!e & g))
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (x, y) in s.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *x = x.wrapping_add(y);
    }
}

fn sw_sha256_64(m: &[u8; 64]) -> [u8; 32] {
    let mut s = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    sw_compress(&mut s, m);
    let mut pad = [0u8; 64];
    pad[0] = 0x80;
    pad[56..].copy_from_slice(&512u64.to_be_bytes());
    sw_compress(&mut s, &pad);
    let mut o = [0u8; 32];
    for (i, x) in s.iter().enumerate() {
        o[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    o
}

// --- main -------------------------------------------------------------------------------

fn sig_paths(inp: &ZkInput, s: &SigCase, c: &mut Clock) {
    let committee = &inp.committee_ssz;
    let k = s.bits.iter().filter(|b| !**b).count();

    // B. Helios's method.
    let agg_b = agg_helios(committee, &s.bits);
    c.mark(&format!("B: signing key, Helios: decompress+subgroup-check aggregate and {k} non-signers, subtract"));
    let sig: Option<G2Affine> = Option::from(G2Affine::from_compressed(&s.signature));
    c.mark("B: signature G2 decompress incl. subgroup check");
    let r = Scalar::from_raw([0xffff_ffff_0000_0001, 0x53bd_a402_fffe_5bfe, 0x3339_d808_09a1_d805, 0x73ed_a753_299d_7d48]);
    let extra = sig.is_some_and(|q| bool::from((q * r).is_identity()));
    c.mark("B: Helios's extra G2 subgroup check (sig * r)");
    let h = hash_to_g2(&s.signing_root);
    c.mark("B: hash_to_curve G2 (XMD:SHA-256, SSWU, cofactor clearing)");
    let ok_b = match (agg_b, sig) {
        (Some(pk), Some(q)) if extra && !bool::from(pk.is_identity()) => pairing_ok(&pk, h, q, c, "B"),
        _ => false,
    };
    c.verdict("B: Helios method verifies the real signature", ok_b);

    // C. No extra check; keys from witnesses checked against the committee witness bytes.
    let agg_c = agg_witness(&s.keys, &s.bits, |i| committed_key(committee, i));
    c.mark(&format!("C: signing key from witnesses: aggregate + {k} non-signers (on curve, recompress = committed), subtract"));
    let sig: Option<G2Affine> = Option::from(G2Affine::from_compressed(&s.signature));
    c.mark("C: signature G2 decompress incl. subgroup check");
    let h = hash_to_g2(&s.signing_root);
    c.mark("C: hash_to_curve G2");
    let ok_c = match (agg_c, sig) {
        (Some(pk), Some(q)) => pairing_ok(&pk, h, q, c, "C"),
        _ => false,
    };
    c.verdict("C: witness path verifies the real signature", ok_c);
    c.verdict("C: witness key equals Helios's key", agg_b.is_some() && agg_b == agg_c);

    // D. Binding by branches (instead of hashing the 24,624-byte committee witness).
    let ok = branches_ok(&s.keys);
    c.mark(&format!("D: SSZ branches: aggregate leaf + {k} non-signer branches (depth 9) against the committee root"));
    c.verdict("D: branches verify against the committee root", ok);
    let agg_d = agg_witness(&s.keys, &s.bits, committed_from_witness(&s.keys));
    c.mark(&format!("D: signing key from branch-bound witnesses ({k} non-signers)"));
    c.verdict("D: branch-bound key equals Helios's key", agg_b.is_some() && agg_b == agg_d);
}

fn main() {
    let mut buf = Vec::new();
    env::stdin().read_to_end(&mut buf).unwrap();
    let mut c = Clock { last: env::cycle_count(), r: Report { marks: Vec::new(), verdicts: Vec::new() } };
    let inp: ZkInput = borsh::from_slice(&buf).unwrap();
    c.mark("setup: read + decode the bench input");

    // A. The program's own path.
    let ri: ReceiveInput = borsh::from_slice(&inp.receive_input).unwrap();
    c.mark("setup: decode the ReceiveInput");
    let t0 = env::cycle_count();
    let resp = eth_attest::handle(&ri);
    let t1 = env::cycle_count();
    let lc = eth_attest::marks::MARKS.lock().unwrap().clone();
    let mut last = t0;
    let mut sum = 0;
    for (label, at) in &lc {
        let label = if *label == "lc: enter verify_update" { "handle: borsh-decode the Message (committee + update), dispatch" } else { label };
        c.r.marks.push((format!("A: {label}"), at - last));
        sum += at - last;
        last = *at;
    }
    c.r.marks.push(("A: rest (progress check, Advance message, response)".into(), t1 - last));
    sum += t1 - last;
    c.r.marks.push(("A = TOTAL handle() (sum of the A rows)".into(), sum));
    c.verdict("A: handle() returned an Advance", !resp.sync_messages.is_empty());
    c.last = env::cycle_count();

    sig_paths(&inp, &inp.sig, &mut c);

    // D2. Full committee root, direct SHA-256 (what Init / a period update needs once).
    let mut layer: Vec<[u8; 32]> = (0..512).map(|i| key_leaf(&committed_key(&inp.committee_ssz, i))).collect();
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|p| sha256_pair(&p[0], &p[1])).collect();
    }
    let root = sha256_pair(&layer[0], &key_leaf(&committed_key(&inp.committee_ssz, 512)));
    c.mark("D2: whole SyncCommittee hash_tree_root, direct sha2 (1,025 sha256 of 64 bytes)");
    c.verdict("D2: direct root equals the host's tree_hash root", root == inp.sig.keys.committee_root);

    // E. The two-thirds floor: 170 non-signers.
    let bits: Vec<bool> = {
        let mut b = vec![true; 512];
        for w in &inp.floor.absent {
            b[usize::from(w.index)] = false;
        }
        b
    };
    let n = inp.floor.absent.len();
    let pk = agg_witness(&inp.floor, &bits, |i| committed_key(&inp.committee_ssz, i));
    c.mark(&format!("E: floor 342/512: signing key from witnesses, {n} non-signers"));
    c.verdict("E: floor key path accepted the witnesses", pk.is_some());
    let ok = branches_ok(&inp.floor);
    c.mark(&format!("E: floor 342/512: {n} SSZ branches against the committee root"));
    c.verdict("E: floor branches verify", ok);

    // F. SHA-256 paths, 1024 messages of 64 bytes each (two compressions each).
    let blk = [7u8; 64];
    for _ in 0..1024 {
        core::hint::black_box(Sha256::digest(core::hint::black_box(&blk)));
    }
    c.mark("F: 1024 x sha256(64 B), sha2 0.9 (risczero patch)");
    for _ in 0..1024 {
        core::hint::black_box(Impl::hash_bytes(core::hint::black_box(&blk)));
    }
    c.mark("F: 1024 x sha256(64 B), risc0_zkvm::sha::Impl");
    let mut x = [0u8; 32];
    for _ in 0..1024 {
        x = sw_sha256_64(core::hint::black_box(&blk));
    }
    c.mark("F: 1024 x sha256(64 B), plain software (control)");
    c.verdict("F: software control matches sha2", x[..] == Sha256::digest(&blk)[..]);

    env::commit_slice(borsh::to_vec(&c.r).unwrap().as_slice());
}
