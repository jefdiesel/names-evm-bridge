//! lcbench blst guest (spike, EXECUTOR ONLY): the same signature check as lc_bench_zk path C
//! (keys from uncompressed witnesses, aggregate minus non-signers, no redundant G2 check), on
//! blst with RISC Zero's accelerated field arithmetic. Two pairing shapes: two Miller loops and
//! a product (as one would split a staged update), and one shared `blst_miller_loop_n`.

use std::io::Read;

use blst::{
    BLST_ERROR, blst_final_exp, blst_fp12, blst_fp12_conjugate, blst_fp12_is_one, blst_fp12_mul, blst_hash_to_g2,
    blst_miller_loop, blst_miller_loop_n, blst_p1, blst_p1_add_or_double, blst_p1_add_or_double_affine,
    blst_p1_affine, blst_p1_affine_compress, blst_p1_affine_generator, blst_p1_affine_is_inf, blst_p1_cneg,
    blst_p1_deserialize, blst_p1_from_affine, blst_p1_to_affine, blst_p2, blst_p2_affine, blst_p2_affine_in_g2,
    blst_p2_affine_is_inf, blst_p2_to_affine, blst_p2_uncompress,
};
use lcbench_common::{BlstInput, Keys, Report};
use risc0_zkvm::guest::env;

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

/// The witness point if it is a canonical on-curve encoding (blst checks both), not the point at
/// infinity, and recompresses to the committed bytes. No subgroup check.
fn g1_witness(unc: &[u8; 96], committed: &[u8; 48]) -> Option<blst_p1_affine> {
    let mut p = blst_p1_affine::default();
    if unsafe { blst_p1_deserialize(&mut p, unc.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
        return None;
    }
    let mut c = [0u8; 48];
    unsafe { blst_p1_affine_compress(c.as_mut_ptr(), &p) };
    (!unsafe { blst_p1_affine_is_inf(&p) } && &c == committed).then_some(p)
}

/// Aggregate minus the non-signers named by `bits`, each from its witness, in index order.
fn signers_key(keys: &Keys, bits: &[bool]) -> Option<blst_p1_affine> {
    let agg = g1_witness(&keys.agg_uncompressed, &keys.agg_compressed)?;
    let mut absent = keys.absent.iter();
    let mut neg = blst_p1::default(); // infinity
    for (i, b) in bits.iter().enumerate() {
        if !b {
            let w = absent.next()?;
            if usize::from(w.index) != i {
                return None;
            }
            let p = g1_witness(&w.uncompressed, &w.compressed)?;
            let a = neg;
            unsafe { blst_p1_add_or_double_affine(&mut neg, &a, &p) };
        }
    }
    if absent.next().is_some() {
        return None;
    }
    let mut acc = blst_p1::default();
    let mut out = blst_p1_affine::default();
    unsafe {
        blst_p1_cneg(&mut neg, true);
        blst_p1_from_affine(&mut acc, &agg);
        let a = acc;
        blst_p1_add_or_double(&mut acc, &a, &neg);
        blst_p1_to_affine(&mut out, &acc);
    }
    (!unsafe { blst_p1_affine_is_inf(&out) }).then_some(out)
}

/// `signature_to_point` + `KeyValidate`: decompress, in G2, not infinity.
fn signature(sig: &[u8; 96]) -> Option<blst_p2_affine> {
    let mut s = blst_p2_affine::default();
    if unsafe { blst_p2_uncompress(&mut s, sig.as_ptr()) } != BLST_ERROR::BLST_SUCCESS {
        return None;
    }
    (unsafe { blst_p2_affine_in_g2(&s) && !blst_p2_affine_is_inf(&s) }).then_some(s)
}

fn hash_to_g2(msg: &[u8; 32]) -> blst_p2_affine {
    let mut p = blst_p2::default();
    let mut a = blst_p2_affine::default();
    unsafe {
        blst_hash_to_g2(&mut p, msg.as_ptr(), msg.len(), DST.as_ptr(), DST.len(), core::ptr::null(), 0);
        blst_p2_to_affine(&mut a, &p);
    }
    a
}

fn neg_g1() -> blst_p1_affine {
    let mut p = blst_p1::default();
    let mut a = blst_p1_affine::default();
    unsafe {
        blst_p1_from_affine(&mut p, blst_p1_affine_generator());
        blst_p1_cneg(&mut p, true);
        blst_p1_to_affine(&mut a, &p);
    }
    a
}

fn main() {
    let mut buf = Vec::new();
    env::stdin().read_to_end(&mut buf).unwrap();
    let mut c = Clock { last: env::cycle_count(), r: Report { marks: Vec::new(), verdicts: Vec::new() } };
    let inp: BlstInput = borsh::from_slice(&buf).unwrap();
    c.mark("setup: read + decode the bench input");
    let s = &inp.sig;
    let k = s.keys.absent.len();

    let pk = signers_key(&s.keys, &s.bits);
    c.mark(&format!("blst: signing key from witnesses: aggregate + {k} non-signers (on curve, recompress = committed), subtract"));
    let sig = signature(&s.signature);
    c.mark("blst: signature decompress + G2 subgroup check + not infinity");
    let hm = hash_to_g2(&s.signing_root);
    c.mark("blst: hash_to_curve G2 (blst_hash_to_g2, same DST)");
    let (Some(pk), Some(sig)) = (pk, sig) else {
        c.verdict("blst: key and signature decode", false);
        env::commit_slice(borsh::to_vec(&c.r).unwrap().as_slice());
        return;
    };

    // Pairing, shape 1: two Miller loops, then the product (a staged update's natural seam).
    let mut a = blst_fp12::default();
    let mut b = blst_fp12::default();
    let mut f = blst_fp12::default();
    unsafe {
        blst_miller_loop(&mut a, &hm, &pk);
        blst_miller_loop(&mut b, &sig, blst_p1_affine_generator());
        blst_fp12_conjugate(&mut b);
        blst_fp12_mul(&mut f, &a, &b);
    }
    c.mark("blst P1: two Miller loops + conjugate + product");
    let mut e = blst_fp12::default();
    let ok1 = unsafe {
        blst_final_exp(&mut e, &f);
        blst_fp12_is_one(&e)
    };
    c.mark("blst P1: final exponentiation + is_one");
    c.verdict("blst P1: real signature verifies", ok1);

    // Pairing, shape 2: one shared Miller loop over both pairs (e(pk, H(m)) * e(-g1, sig)).
    let ng = neg_g1();
    c.mark("blst P2: negate the G1 generator");
    let qs: [*const blst_p2_affine; 2] = [&hm, &sig];
    let ps: [*const blst_p1_affine; 2] = [&pk, &ng];
    let mut f2 = blst_fp12::default();
    unsafe { blst_miller_loop_n(&mut f2, qs.as_ptr(), ps.as_ptr(), 2) };
    c.mark("blst P2: blst_miller_loop_n, 2 pairs");
    let mut e2 = blst_fp12::default();
    let ok2 = unsafe {
        blst_final_exp(&mut e2, &f2);
        blst_fp12_is_one(&e2)
    };
    c.mark("blst P2: final exponentiation + is_one");
    c.verdict("blst P2: real signature verifies", ok2);

    // A wrong message must fail (the check is not vacuous).
    let mut wrong = s.signing_root;
    wrong[0] ^= 1;
    let hw = hash_to_g2(&wrong);
    let qs: [*const blst_p2_affine; 2] = [&hw, &sig];
    let mut f3 = blst_fp12::default();
    let mut e3 = blst_fp12::default();
    let bad = unsafe {
        blst_miller_loop_n(&mut f3, qs.as_ptr(), ps.as_ptr(), 2);
        blst_final_exp(&mut e3, &f3);
        blst_fp12_is_one(&e3)
    };
    c.verdict("blst: a wrong signing root is rejected", !bad);
    c.mark("blst: (negative test, not part of a path)");

    // The two-thirds floor: 170 non-signers on the same committee (cost only).
    let bits: Vec<bool> = {
        let mut b = vec![true; 512];
        for w in &inp.floor.absent {
            b[usize::from(w.index)] = false;
        }
        b
    };
    let n = inp.floor.absent.len();
    let fk = signers_key(&inp.floor, &bits);
    c.mark(&format!("blst E: floor 342/512: signing key from witnesses, {n} non-signers"));
    c.verdict("blst E: floor key path accepted the witnesses", fk.is_some());

    env::commit_slice(borsh::to_vec(&c.r).unwrap().as_slice());
}
