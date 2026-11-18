// SPDX-License-Identifier: MIT OR Apache-2.0
//! The seq-sig guests in RISC Zero's executor (never proves), on the recorded LEZ blocks of run
//! `public9` (`lez-bridge/outbound/fixtures/public9`, the batch `outbound/host/tests/outbound.rs`
//! and `vault/test/OutboundVector.t.sol` use). The signer keys are test keys (stand-ins).
//!
//! A refusal means the guest panicked: no journal, so no proof could exist.

use outbound_core::{finality::BlockRef, wire};
use outbound_host::{HostState, Prepared, parse_account, prepare};
use seqsig_core::{Checkpoint, Evidence, Sig, TAIL_LEN, Witness, check, journal_tail, public_key, sign, vectors};
use seqsig_host::{SEQSIG_EVIDENCE_GUEST_ELF, evidence_image, execute_update, run_guest};
use std::path::PathBuf;

const P9_BRIDGE: &str = "GR5ngdsJ1WC49YqShQ6JCAK4JmQ9QoRcVMobxTmXaV8u";
const P9_FIRST: u64 = 2893;
const P9_LAST: u64 = 2904;

fn block(id: u64) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../outbound/fixtures/public9").join(format!("block-{id}.bin"));
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn setup() -> (HostState, Prepared, Checkpoint) {
    let first_raw = block(P9_FIRST);
    let first = wire::parse_block(&first_raw).unwrap();
    let state = HostState::genesis(parse_account(P9_BRIDGE).unwrap(), BlockRef { id: P9_FIRST - 1, hash: first.header.prev_block_hash });
    let p = prepare(&state, (P9_FIRST..=P9_LAST).map(block).collect()).unwrap();
    let last_raw = block(P9_LAST);
    let last = wire::parse_block(&last_raw).unwrap();
    let cp = Checkpoint {
        cfg_id: p.out.journal.cfg_id,
        prev_id: state.g2.tip.id,
        prev_hash: state.g2.tip.hash,
        block_id: P9_LAST,
        block_hash: last.hash,
    };
    (state, p, cp)
}

/// Four set keys (sorted) and their secrets, plus an outsider's secret.
fn keys() -> (Vec<[u8; 32]>, Vec<[u8; 32]>, [u8; 32]) {
    let mut v: Vec<([u8; 32], [u8; 32])> = (1..=4u8).map(|i| (public_key(&[i; 32]), [i; 32])).collect();
    v.sort();
    (v.iter().map(|x| x.0).collect(), v.iter().map(|x| x.1).collect(), [9u8; 32])
}

fn witness(cp: &Checkpoint, t: u32, signers: &[usize]) -> Witness {
    let (pk, sk, _) = keys();
    Witness { keys: pk, threshold: t, ejected: 0, sigs: signers.iter().map(|&i| Sig { key_index: i as u32, sig: sign(&sk[i], cp) }).collect() }
}

fn refused(p: &Prepared, w: &Witness) -> String {
    match execute_update(p, w) {
        Ok(_) => panic!("the guest committed a journal"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn three_of_four_accepted_and_the_tail_is_committed() {
    let (_, p, cp) = setup();
    let w = witness(&cp, 3, &[0, 1, 3]);
    let run = execute_update(&p, &w).unwrap();
    let n = run.journal.len();
    assert_eq!(&run.journal[..n - TAIL_LEN], &p.journal[..], "the vault part is the TrustedTip journal");
    let ok = check(&cp, &w).unwrap();
    assert_eq!(ok.bitmap, 0b1011);
    assert_eq!(run.journal[n - TAIL_LEN..], journal_tail(&ok, &cp));
    let (cycles, given, signers): (u64, u32, u32) = borsh::from_slice(&run.stdout).unwrap();
    assert_eq!((given, signers), (3, 3));
    eprintln!("3 of 4: {} user cycles, {cycles} in the signature check", run.user_cycles);
    let four = execute_update(&p, &witness(&cp, 3, &[0, 1, 2, 3])).unwrap();
    let (c4, _, _): (u64, u32, u32) = borsh::from_slice(&four.stdout).unwrap();
    eprintln!("4 of 4: {} user cycles, {c4} in the signature check; one more signature: {} user cycles", four.user_cycles, four.user_cycles - run.user_cycles);
}

#[test]
fn two_of_four_refused() {
    let (_, p, cp) = setup();
    let e = refused(&p, &witness(&cp, 3, &[0, 2]));
    assert!(e.contains("2 distinct valid signers, threshold 3"), "{e}");
}

#[test]
fn a_key_signing_twice_counts_once() {
    let (_, p, cp) = setup();
    let e = refused(&p, &witness(&cp, 3, &[0, 1, 1]));
    assert!(e.contains("2 distinct valid signers, threshold 3"), "{e}");
}

#[test]
fn a_key_outside_the_set_refused() {
    let (_, p, cp) = setup();
    let (_, _, outsider) = keys();
    let mut w = witness(&cp, 3, &[0, 1]);
    w.sigs.push(Sig { key_index: 2, sig: sign(&outsider, &cp) });
    assert!(refused(&p, &w).contains("signature for key 2 does not verify"));
    w.sigs.pop();
    w.sigs.push(Sig { key_index: 4, sig: sign(&outsider, &cp) });
    assert!(refused(&p, &w).contains("not in the set"));
}

/// Signatures over another vault's cfgId, another history (previous tip) or another end block are
/// refused: the guest builds the checkpoint from its own input.
#[test]
fn signatures_for_another_cfg_history_or_block_refused() {
    let (_, p, cp) = setup();
    for other in [
        Checkpoint { cfg_id: [0xee; 32], ..cp },
        Checkpoint { prev_id: cp.prev_id - 1, ..cp },
        Checkpoint { prev_hash: [0xee; 32], ..cp },
        Checkpoint { block_id: cp.block_id - 1, ..cp },
        Checkpoint { block_hash: [0xee; 32], ..cp },
    ] {
        let e = refused(&p, &witness(&other, 3, &[0, 1, 2]));
        assert!(e.contains("does not verify"), "{e}");
    }
}

/// An ejected key's signature no longer counts, and the mask is committed.
#[test]
fn ejected_key_refused_and_mask_committed() {
    let (_, p, cp) = setup();
    let mut w = witness(&cp, 3, &[0, 1, 2]);
    w.ejected = 0b0010;
    assert!(refused(&p, &w).contains("key 1 is ejected"));
    let mut w = witness(&cp, 3, &[0, 2, 3]);
    w.ejected = 0b0010;
    let run = execute_update(&p, &w).unwrap();
    let n = run.journal.len();
    assert_eq!(run.journal[n - TAIL_LEN + 95], 0b0010, "ejected mask word");
    assert_eq!(run.journal[n - TAIL_LEN + 127], 0b1101, "signer bitmap word");
}

/// Set keys that are of small order, or a non-canonical encoding, and a non-canonical S: the
/// guest (RISC Zero's curve25519-dalek fork) refuses each.
#[test]
fn bad_keys_and_non_canonical_s_refused_in_the_guest() {
    let (_, p, cp) = setup();
    for bad in [vectors::SMALL_ORDER_IDENTITY, vectors::SMALL_ORDER_8, vectors::non_canonical_key().unwrap()] {
        let mut w = witness(&cp, 3, &[0, 1, 2]);
        w.keys[3] = bad;
        w.keys.sort();
        // indexes moved; whatever they are now, the set itself is refused first
        let e = refused(&p, &w);
        assert!(e.contains("not a canonical point encoding of large order"), "{e}");
    }
    let mut w = witness(&cp, 3, &[0, 1, 2]);
    w.sigs[2].sig = vectors::non_canonical_s(&w.sigs[2].sig);
    let e = refused(&p, &w);
    assert!(e.contains("does not verify"), "{e}");
}

#[test]
fn evidence_guest() {
    let (_, _, cp) = setup();
    let (pk, sk, outsider) = keys();
    let conflict = Checkpoint { block_hash: [0x42; 32], ..cp };
    let e = Evidence { checkpoint: conflict, keys: pk.clone(), sigs: vec![Sig { key_index: 3, sig: sign(&sk[3], &conflict) }] };
    let run = run_guest(SEQSIG_EVIDENCE_GUEST_ELF, evidence_image(), &[&borsh::to_vec(&e).unwrap()]).unwrap();
    assert_eq!(run.journal, seqsig_core::evidence_journal(&e).unwrap());
    eprintln!("evidence: {} user cycles", run.user_cycles);
    let bad = Evidence { sigs: vec![Sig { key_index: 3, sig: sign(&outsider, &conflict) }], ..e };
    assert!(run_guest(SEQSIG_EVIDENCE_GUEST_ELF, evidence_image(), &[&borsh::to_vec(&bad).unwrap()]).is_err());
}

/// The attestor's record refuses a second, different hash for a checkpoint key it already signed.
#[test]
fn attestor_record_refuses_a_second_hash() {
    let (_, _, cp) = setup();
    let path = std::env::temp_dir().join(format!("seqsig-record-test-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&path);
    assert!(seqsig_host::record_and_check(&path, &cp).unwrap());
    assert!(!seqsig_host::record_and_check(&path, &cp).unwrap(), "same hash again: fine");
    let other = Checkpoint { block_hash: [0xee; 32], ..cp };
    let e = seqsig_host::record_and_check(&path, &other).unwrap_err();
    assert!(format!("{e}").contains("refusing to sign"), "{e}");
    // another previous tip or another block id is another key
    assert!(seqsig_host::record_and_check(&path, &Checkpoint { prev_id: cp.prev_id + 1, ..other }).unwrap());
    assert!(seqsig_host::record_and_check(&path, &Checkpoint { block_id: cp.block_id + 1, ..other }).unwrap());
    let _ = std::fs::remove_file(&path);
}
