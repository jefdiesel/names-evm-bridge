//! G2-lite tests. Natively and in RISC Zero's EXECUTOR only (no proving anywhere). Run on the test machine:
//!   cd lez-bridge/outbound && cargo test --release -j 4 -- --nocapture
//!
//! Fixtures (`outbound/fixtures/`) are real blocks of the local LEZ 0.4 sequencer seq2:
//! - `public9/block-2893..2904.bin`: run `public9` (stand-in, public account): init 2895,
//!   register 2896, AttestRoot 2899, claims 2900 and 2901, burn 2903, a reverted odd burn 2904.
//! - `private/block-782.bin`, `block-884.bin`: run `private5` (stand-in, private account V): a
//!   private claim and a private burn, each with its privacy-circuit receipt.
//! - `private/block-4998.bin`: run `sepolia2` (RFP-021 attested): a private attested claim.
//!
//! All of these are LEZ v0.4.0-rc1 blocks, made by the bridge program as it was before the
//! 2026-11-14 security fixes. G2 now derives a public claim's nullifier key with the new formula
//! (`SHA256("RFP022/nfc" | k | dest | token | u256(denom))`), so for these old public claims the
//! accumulator keys it computes are not the nullifier PDAs the old program wrote (G2 does not
//! cross-check the key against the PDA the transaction touched); the counts, exits, roots and
//! state chaining these tests check are unaffected, and `vault/test/fixtures/OutboundVector.sol`
//! was regenerated from them with the new keys. The block and transaction wire format is the same in
//! v0.4.0-rc3 (checked by `rc3/` blocks of an rc3 chain), but the privacy circuit changed, so the
//! private fixtures' receipts would verify only under rc1's circuit id, never under the circuit the
//! rc3 guest pins. Since the host moved to risc0-zkvm 3.1.0 (advisory GHSA-g6qg-9j8p-7j87), these
//! receipts, made with risc0 3.0.x like every LEZ rc1/rc3 receipt, do not verify at all: extraction
//! tests read them unchecked (`extract_unchecked`), and the tests of the receipt check itself wait,
//! ignored, for a privacy receipt made with risc0 3.1.

use borsh::BorshDeserialize as _;
use bridge_core::{claim_nullifier, nullifier_address};
use lee_core::{RootCall, account::{AccountId, Actor}};
use outbound_core::{
    G2Error,
    finality::{BlockRef, KIND_LOGOS_LIGHT_CLIENT, KIND_TRUSTED_TIP, TrustedTip},
    journal::{JournalVersion, UpdateJournal},
    state::G2State,
    update::{AcceptAll, GuestHeader, HostAcc, NoMeter, UpdateInput, update},
    view::{At, BatchOut, BridgeView, ClaimDelta, Exit, Extractor, TokenView},
    wire,
};
use outbound_host::*;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn block(dir: &str, id: u64) -> Vec<u8> {
    std::fs::read(fixtures().join(dir).join(format!("block-{id}.bin"))).unwrap_or_else(|e| panic!("fixture {dir}/{id}: {e}"))
}

fn acct(s: &str) -> [u8; 32] {
    parse_account(s).unwrap()
}

fn h32(s: &str) -> [u8; 32] {
    parse_hex::<32>(s).unwrap()
}

fn h20(s: &str) -> [u8; 20] {
    parse_hex::<20>(s).unwrap()
}

// public9 (run/public9/ids.env, run/public9.e2e.log)
const P9_BRIDGE: &str = "GR5ngdsJ1WC49YqShQ6JCAK4JmQ9QoRcVMobxTmXaV8u";
const P9_TOKEN_PROGRAM: &str = "Hb8C7Fp5P79L3hR2tavkH1wP1L1BFYUmYEHwDJtgquDr";
const P9_DEF: &str = "56HxckTSfsa8848R42xApcMMaRfnXhFnTVn4EhCKPvV9";
const P9_USDC: &str = "0xcbeaf3bde82155f56486fb5a1072cb8baaf547cc";
const P9_CFG: &str = "0x08897d4523bbf36bacbd0b7403e7e0b1c04efa0b3bdd1ad9450661ba6371c580";
const P9_ROOT: &str = "0x05bbe0c005ad2872e35a5407727820315daadad54c8ff0a89eccc157775d7fe3";
const P9_EXIT: &str = "0x406141a0a5d6cb32d85e59d4f4f623056f040368de64910a3c8851206a25fcea";
const P9_FIRST: u64 = 2893;
const P9_LAST: u64 = 2904;

// rc3 (public-demo/rc3/e2e-rc3.sh (not published), 2026-11-13): bridge JECKB… on a LEZ v0.4.0-rc3 test chain.
// Blocks 34 and 38 carry v2 leaf 3's claim secret in clear, as every public claim does: it was
// already published on the rc1 chains (seq2, and the public demo chain in block 385), and the deposit
// commitment binds the team account P2, so it claims nothing for anyone else.
const RC3_FIRST: u64 = 32;
const RC3_LAST: u64 = 38;

// secfix (lez-bridge/scripts/e2e-security.sh, 2026-11-14): the FIXED bridge and token programs (after
// the 2026-11-14 security fixes) on a throwaway LEZ rc3 chain with an anvil vault. Blocks 1021-1028:
// AttestRoot 1021, the attacker's claim of its own deposit made with the victim's secret 1022, the
// victim's claim 1023 (applies: review H2 fixed), a repeat reverted 1024, a fee transfer 1025,
// AttestRoot 1026, a relayed claim 1027, a burn 1028. The public claims carry throwaway secrets of
// deposits on a deleted local anvil.
const SF_BRIDGE: &str = "GUmAgP76RQsKZr2xqPkMDgFFT5wpcdJWGi5PRMipAGUe";
const SF_TOKEN_PROGRAM: &str = "6XK69yrwCp6BB5KdarF5tGneH3e2t6SbmQ37AYYTyejc";
const SF_DEF: &str = "6HMN9dpZYhztzBSasz3dDj6dUfD8RpURe3F6ouZDLYFM";
const SF_USDC: &str = "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512";
const SF_FIRST: u64 = 1021;
const SF_LAST: u64 = 1028;

/// The rc3 blocks chain to each other, and the reverted claim in 38 is marked reverted.
#[test]
fn rc3_blocks_link_and_the_reverted_claim_is_marked() {
    let parsed: Vec<_> = (RC3_FIRST..=RC3_LAST).map(|i| block("rc3", i)).collect();
    for w in parsed.windows(2) {
        let (a, b) = (wire::parse_block(&w[0]).unwrap(), wire::parse_block(&w[1]).unwrap());
        assert_eq!(b.header.prev_block_hash, a.hash);
        assert_eq!(b.header.block_id, a.header.block_id + 1);
    }
    let last = wire::Block::try_from_slice(&parsed[parsed.len() - 1]).unwrap();
    assert_eq!(last.body.reverted.len(), 1, "block 38 holds the reverted repeat claim");
}

fn p9_blocks(from: u64, to: u64) -> Vec<Vec<u8>> {
    (from..=to).map(|i| block("public9", i)).collect()
}

/// The G2 genesis state just before block 2893 (the tip hash is 2893's parent hash).
fn p9_genesis() -> HostState {
    let b = block("public9", P9_FIRST);
    let p = wire::parse_block(&b).unwrap();
    HostState::genesis(acct(P9_BRIDGE), BlockRef { id: P9_FIRST - 1, hash: p.header.prev_block_hash })
}

fn err(r: Result<Prepared, String>) -> String {
    match r {
        Ok(_) => panic!("accepted"),
        Err(e) => e,
    }
}

fn native(st: &HostState, blocks: &[Vec<u8>]) -> Result<Prepared, String> {
    prepare(st, blocks.to_vec()).map_err(|e| e.to_string())
}

/// [`native`] for the rc1 private fixtures: receipts checked against LEZ rc1's circuit. Under
/// risc0 3.1 their (3.0.x) receipts never verify; only the ignored tests use it.
fn native_rc1(st: &HostState, blocks: &[Vec<u8>]) -> Result<Prepared, String> {
    prepare_for_circuit(st, blocks.to_vec(), outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1).map_err(|e| e.to_string())
}

/// The statement with private receipts accepted unchecked (`AcceptAll`): extraction only.
fn extract_unchecked(st: &HostState, blocks: &[Vec<u8>]) -> Result<Prepared, String> {
    prepare_with_verifier(st, blocks.to_vec(), &mut AcceptAll).map_err(|e| e.to_string())
}

/// The first private transaction of a fixture block: its receipt journal frame and proof bytes.
fn private_tx(id: u64) -> (Vec<u8>, Vec<u8>) {
    let raw = block("private", id);
    let p = wire::parse_block(&raw).unwrap();
    p.txs
        .iter()
        .find_map(|t| match t {
            wire::ParsedTx::Private { journal_payload, proof, .. } => Some((wire::journal_frame(journal_payload), proof.to_vec())),
            wire::ParsedTx::Public { .. } => None,
        })
        .unwrap()
}

#[test]
fn every_fixture_block_decodes_strictly_and_hashes_to_its_stored_hash() {
    let mut all: Vec<(String, Vec<u8>)> = (P9_FIRST..=P9_LAST).map(|i| (format!("public9/{i}"), block("public9", i))).collect();
    for i in [782, 884, 4998] {
        all.push((format!("private/{i}"), block("private", i)));
    }
    // LEZ v0.4.0-rc3 blocks (an rc3 test chain, public-demo/rc3 (not published)): stand-in AttestRoot 32, a fee
    // transfer 33, a public claim 34, a public burn 35, a reverted claim 38. Same wire format.
    for i in RC3_FIRST..=RC3_LAST {
        all.push((format!("rc3/{i}"), block("rc3", i)));
    }
    for i in SF_FIRST..=SF_LAST {
        all.push((format!("secfix/{i}"), block("secfix", i)));
    }
    for (name, raw) in &all {
        let whole = wire::Block::try_from_slice(raw).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&borsh::to_vec(&whole).unwrap(), raw, "{name}: re-encode");
        let p = wire::parse_block(raw).unwrap_or_else(|e| panic!("{name}: {}", e.0));
        assert_eq!(p.header.hash, p.hash, "{name}: hash");
        let mut longer = raw.clone();
        longer.push(0);
        assert!(wire::parse_block(&longer).is_err(), "{name}: trailing byte accepted");
    }
}

#[test]
fn public9_known_answer_natively_and_in_the_executor() {
    let st = p9_genesis();
    let p = native(&st, &p9_blocks(P9_FIRST, P9_LAST)).unwrap();
    let j = &p.out.journal;
    assert_eq!(j.cfg_id, h32(P9_CFG), "cfg id derived from the bridge's Init");
    assert_eq!(j.prev_state, st.g2.commitment());
    assert_eq!(j.deposit_roots_used, vec![h32(P9_ROOT)], "the stand-in root recorded in 2899");
    assert_eq!(
        j.claims,
        vec![
            ClaimDelta { token: h20(P9_USDC), denom_index: 0, count: 1 },
            ClaimDelta { token: h20(P9_USDC), denom_index: 1, count: 1 }
        ]
    );
    assert_eq!(j.exits, vec![Exit { token: h20(P9_USDC), denom_index: 0, commitment: h32(P9_EXIT) }], "2904's odd burn reverted");
    // the bridge's own counters (`lez-bridge show`, run/public9.e2e.log line 199)
    let t = &p.next.view.tokens[0];
    assert_eq!((t.claims.clone(), t.burns.clone(), t.claimed_value, t.burned_value), (vec![1, 1], vec![1, 0], 1_100_000_000, 100_000_000));
    assert_eq!(t.definition, acct(P9_DEF));
    assert_eq!(p.next.view.token_program, acct(P9_TOKEN_PROGRAM));
    assert!(p.next.view.stand_in);
    // two nullifier PDAs, each the bridge's PDA of SHA256("RFP022/nfc" | k) for the published k
    assert_eq!(p.out.batch.nullifiers.len(), 2);
    assert_eq!(p.next.g2.claim_acc.count, 2);
    let tree = p.next.tree().unwrap();
    for n in &p.out.batch.nullifiers {
        assert!(tree.contains(n));
    }
    assert_eq!(p.next.g2.tip.id, P9_LAST);
    assert_eq!(p.next.g2.exit_count, 1);
    assert_eq!(p.next.g2.finality_kind, KIND_TRUSTED_TIP);
    let mut d = UpdateJournal::abi_decode_v1(&p.journal).unwrap();
    assert_eq!((d.claim_acc_root, d.claim_acc_count), ([0; 32], 0), "V1 carries no accumulator fields");
    d.claim_acc_root = j.claim_acc_root;
    d.claim_acc_count = j.claim_acc_count;
    assert_eq!(&d, j);
    // the guest, in the executor, commits exactly the native journal
    let run = execute_trusted_tip(&p).unwrap();
    assert_eq!(run.journal, p.journal);
    assert_eq!(run.stats.blocks, 12);
    assert_eq!(run.stats.inserts, 2);
    println!("public9 2893..2904: {} user cycles, {} segments; {:?}", run.user_cycles, run.segments, run.stats);
}

#[test]
fn split_batches_chain_to_the_same_state() {
    let st = p9_genesis();
    let whole = native(&st, &p9_blocks(P9_FIRST, P9_LAST)).unwrap();
    let a = native(&st, &p9_blocks(P9_FIRST, 2899)).unwrap();
    let b = native(&a.next, &p9_blocks(2900, P9_LAST)).unwrap();
    assert_eq!(b.out.journal.prev_state, a.out.journal.new_state, "state chaining");
    assert_eq!(b.next, whole.next, "same state whichever way the blocks are batched");
    assert_eq!(a.out.journal.deposit_roots_used, vec![h32(P9_ROOT)]);
    assert!(a.out.journal.claims.is_empty() && b.out.journal.deposit_roots_used.is_empty());
    assert_eq!(b.out.journal.exits, whole.out.journal.exits);
    // a batch ending before the bridge's Init has nothing to give the vault: refused
    assert!(err(native(&st, &p9_blocks(P9_FIRST, 2894))).contains("not initialised"));
    // the executor agrees on the second batch (starting from a non-genesis state)
    assert_eq!(execute_trusted_tip(&b).unwrap().journal, b.journal);
}

#[test]
fn broken_chains_and_wrong_states_are_refused() {
    let st = p9_genesis();
    let blocks = p9_blocks(P9_FIRST, P9_LAST);
    // a gap
    let mut gap = blocks.clone();
    gap.remove(3);
    assert!(err(native(&st, &gap)).contains("not the next block id"));
    // reordered
    let mut swapped = blocks.clone();
    swapped.swap(4, 5);
    assert!(err(native(&st, &swapped)).contains("linkage"));
    // the same batch again from the state it produced (a replay)
    let next = native(&st, &blocks).unwrap().next;
    assert!(err(native(&next, &blocks)).contains("not the next block id"));
    // a byte changed inside a transaction: the stored hash no longer matches
    let mut t = blocks.clone();
    let n = t[7].len();
    t[7][n - 20] ^= 1;
    let e = err(native(&st, &t));
    assert!(e.contains("stored hash") || e.contains("decode"), "{e}");
    // the stored hash itself changed: the next block's parent link breaks
    let mut t = blocks.clone();
    t[2][40] ^= 1;
    assert!(err(native(&st, &t)).contains("stored hash"));
    // a view that is not the committed one
    let mut bad = st.clone();
    bad.view.stand_in = true;
    assert!(err(native(&bad, &blocks)).contains("view"));
    // a state made by another finality source
    let mut other = st.clone();
    other.g2.finality_kind = KIND_LOGOS_LIGHT_CLIENT;
    assert!(err(native(&other, &blocks)).contains("finality source"));
    // empty
    assert!(native(&st, &[]).is_err());
}

#[test]
fn the_trusted_tip_must_be_the_last_block_and_the_witnesses_must_be_used() {
    let st = p9_genesis();
    let blocks = p9_blocks(P9_FIRST, P9_LAST);
    let p = native(&st, &blocks).unwrap();
    // the guest refuses a TrustedTip witness naming another block
    let mut h: GuestHeader = p.header.clone();
    h.finality = borsh::to_vec(&BlockRef { id: P9_LAST - 1, hash: p.next.g2.tip.hash }).unwrap();
    let e = execute(OUTBOUND_GUEST_TRUSTED_TIP_ELF, update_image_id_trusted_tip(), &h, &p.blocks, &[]).err().unwrap();
    assert!(format!("{e:#}").contains("trusted tip"), "{e:#}");
    // a missing accumulator witness
    let mut h = p.header.clone();
    h.inserts.pop();
    let e = execute(OUTBOUND_GUEST_TRUSTED_TIP_ELF, update_image_id_trusted_tip(), &h, &p.blocks, &[]).err().unwrap();
    assert!(format!("{e:#}").contains("accumulator"), "{e:#}");
    // a spare one
    let mut h = p.header.clone();
    h.inserts.push(h.inserts[0].clone());
    let e = execute(OUTBOUND_GUEST_TRUSTED_TIP_ELF, update_image_id_trusted_tip(), &h, &p.blocks, &[]).err().unwrap();
    assert!(format!("{e:#}").contains("unused accumulator"), "{e:#}");
}

#[test]
fn the_release_image_differs_and_refuses_until_the_light_client_exists() {
    assert_ne!(update_image_id_trusted_tip(), update_image_id_release());
    assert_ne!(update_image_id_release(), release_image_id());
    let p = native(&p9_genesis(), &p9_blocks(P9_FIRST, P9_LAST)).unwrap();
    // its input would need a light-client state; even with this one it stops at finality
    let mut h = p.header.clone();
    h.prev.finality_kind = KIND_LOGOS_LIGHT_CLIENT;
    let e = execute(OUTBOUND_GUEST_ELF, update_image_id_release(), &h, &p.blocks, &[]).err().unwrap();
    assert!(format!("{e:#}").contains("not implemented"), "{e:#}");
    // and it refuses a TrustedTip state outright
    let e = execute(OUTBOUND_GUEST_ELF, update_image_id_release(), &p.header, &p.blocks, &[]).err().unwrap();
    assert!(format!("{e:#}").contains("another finality source"), "{e:#}");
}

/// A spent nullifier spent again in a later batch (the sequencer let a repeat claim through) is
/// refused by the accumulator.
#[test]
fn a_repeat_claim_the_sequencer_applied_is_refused() {
    let st = p9_genesis();
    let a = native(&st, &p9_blocks(P9_FIRST, 2900)).unwrap();
    // pretend block 2900's claim had already been seen: its nullifier PDA is in the tree
    let mut pre = st.clone();
    let mut tree = pre.tree().unwrap();
    tree.insert(&a.out.batch.nullifiers[0]).unwrap();
    pre.leaves = tree.leaves().to_vec();
    pre.g2.claim_acc = tree.state();
    let e = err(native(&pre, &p9_blocks(P9_FIRST, 2900)));
    assert!(e.contains("already spent"), "{e}");
}

fn p9_view() -> BridgeView {
    native(&p9_genesis(), &p9_blocks(P9_FIRST, 2899)).unwrap().next.view
}

/// Transactions the sequencer marked successful that the bridge would have refused.
#[test]
fn applied_transactions_the_bridge_would_refuse_stop_the_update() {
    let bridge = AccountId::new(acct(P9_BRIDGE));
    let tp = AccountId::new(acct(P9_TOKEN_PROGRAM));
    let def = AccountId::new(acct(P9_DEF));
    let config = Actor::new(bridge_core::config_address(&bridge), bridge);
    let at = At { block_id: 1, tx: 0 };
    let burn = |amount: u128| RootCall {
        to: Actor::new(AccountId::new([7; 32]), tp),
        message: borsh::to_vec(&bridge_token_core::Message::BurnWithNotice { definition: def, amount, notify: config, memo: [9; 32] }).unwrap(),
    };
    let run = |view: &mut BridgeView, root: &RootCall| {
        let mut out = BatchOut::default();
        Extractor::new(bridge, view, &mut out).root(at, root).map(|()| out)
    };
    let mut v = p9_view();
    // not a denomination
    assert!(matches!(run(&mut v, &burn(50_000_000)), Err(G2Error::Inconsistent { what, .. }) if what.contains("one denomination")));
    // above the claimed value (nothing claimed yet at 2899)
    assert!(matches!(run(&mut v, &burn(100_000_000)), Err(G2Error::Inconsistent { what, .. }) if what.contains("burns exceed claims")));
    // a burn notifying another actor is not counted from its root call (a sponsored burn's
    // forwarder is private and is counted from its boundary effect; a public one is refused)
    let mut other = burn(100_000_000);
    other.message = borsh::to_vec(&bridge_token_core::Message::BurnWithNotice {
        definition: def,
        amount: 100_000_000,
        notify: Actor::new(AccountId::new([1; 32]), bridge),
        memo: [9; 32],
    })
    .unwrap();
    assert_eq!(run(&mut v, &other).unwrap(), BatchOut::default());
    // a claim of an unregistered token
    let claim = |eth_token: [u8; 20], denom: u128, secret: [u8; 32]| RootCall {
        to: Actor::new(AccountId::new([5; 32]), bridge),
        message: borsh::to_vec(&bridge_core::Message::Instruction(bridge_core::Instruction::Claim {
            eth_token,
            denom_index: 0,
            denom,
            root: [0; 32],
            leaf_index: 0,
            path: vec![],
            secret,
            cfg_id: [0; 32],
            recipient_opening: None,
        }))
        .unwrap(),
    };
    assert!(matches!(run(&mut v, &claim([3; 20], 100_000_000, [1; 32])), Err(G2Error::Inconsistent { what, .. }) if what.contains("not registered")));
    assert!(matches!(run(&mut v, &claim(h20(P9_USDC), 7, [1; 32])), Err(G2Error::Inconsistent { what, .. }) if what.contains("wrong amount")));
    // a good claim: counted, nullifier PDA = the bridge's PDA of
    // SHA256("RFP022/nfc" | k | signer | token | u256(denom)), the signer being the root call's account
    let out = run(&mut v, &claim(h20(P9_USDC), 100_000_000, [1; 32])).unwrap();
    let nf = claim_nullifier(&[1; 32], &AccountId::new([5; 32]), &h20(P9_USDC), 100_000_000);
    assert_eq!(out.nullifiers, vec![*nullifier_address(&bridge, &nf).value()]);
    // the same secret claimed by another signer is another nullifier (review H2)
    let mut other_signer = claim(h20(P9_USDC), 100_000_000, [1; 32]);
    other_signer.to = Actor::new(AccountId::new([6; 32]), bridge);
    let out2 = run(&mut v, &other_signer).unwrap();
    assert_ne!(out2.nullifiers, out.nullifiers);
    // a second Init
    let init = RootCall {
        to: Actor::new(AccountId::new([5; 32]), bridge),
        message: borsh::to_vec(&bridge_core::Message::Instruction(bridge_core::Instruction::Init {
            vault: [0; 20],
            evm_chain_id: 1,
            lez_chain_id: [0; 32],
            zone_id: [0; 32],
            token_program: tp,
            freezer: tp,
            attestor: tp,
        }))
        .unwrap(),
    };
    assert!(matches!(run(&mut v, &init), Err(G2Error::Inconsistent { what, .. }) if what.contains("twice")));
    // an Initialize not signed by the header account (the bridge program id) is refused by the
    // bridge, so an applied one is the sequencer's fault
    let initialize = |signer: AccountId| RootCall {
        to: Actor::new(signer, bridge),
        message: borsh::to_vec(&bridge_core::Message::Instruction(bridge_core::Instruction::Initialize {
            admin: tp,
            vault: [1; 20],
            evm_chain_id: 1,
            lez_chain_id: [1; 32],
            token_program: tp,
            freezer: tp,
            attestor: tp,
        }))
        .unwrap(),
    };
    let mut fresh = BridgeView::default();
    assert!(matches!(run(&mut fresh, &initialize(AccountId::new([5; 32]))), Err(G2Error::Inconsistent { what, .. }) if what.contains("header account")));
    run(&mut fresh, &initialize(bridge)).unwrap();
    let zone = bridge_core::source_id(&bridge_core::RootSource::StandIn);
    assert_eq!(fresh.cfg_id, bridge_core::cfg_id(1, &[1; 20], &[1; 32], &zone, &bridge));
}

#[test]
fn journal_abi_layout() {
    let j = UpdateJournal {
        cfg_id: [1; 32],
        prev_state: [2; 32],
        new_state: [3; 32],
        deposit_roots_used: vec![[4; 32]],
        claims: vec![ClaimDelta { token: [5; 20], denom_index: 1, count: 2 }],
        exits: vec![Exit { token: [6; 20], denom_index: 0, commitment: [7; 32] }, Exit { token: [0; 20], denom_index: 2, commitment: [8; 32] }],
        claim_acc_root: [9; 32],
        claim_acc_count: 3,
        wound_down: vec![[10; 20]],
    };
    let b = j.abi_encode(JournalVersion::V1);
    let w = |i: usize| &b[32 * i..32 * i + 32];
    let u = |i: usize| u64::from_be_bytes(w(i)[24..].try_into().unwrap());
    assert_eq!(b.len(), 32 * (1 + 6 + 2 + 4 + 7));
    assert_eq!(u(0), 32, "outer offset of the dynamic tuple");
    assert_eq!((w(1), w(2), w(3)), (&[1u8; 32][..], &[2u8; 32][..], &[3u8; 32][..]));
    assert_eq!((u(4), u(5), u(6)), (192, 256, 384), "offsets from the tuple start");
    assert_eq!((u(7), w(8)), (1, &[4u8; 32][..]));
    assert_eq!((u(9), &w(10)[12..], u(11), u(12)), (1, &[5u8; 20][..], 1, 2));
    assert_eq!((u(13), &w(14)[12..], u(15), w(16)), (2, &[6u8; 20][..], 0, &[7u8; 32][..]));
    let mut back = UpdateJournal::abi_decode_v1(&b).unwrap();
    back.claim_acc_root = j.claim_acc_root;
    back.claim_acc_count = j.claim_acc_count;
    back.wound_down = j.wound_down.clone();
    assert_eq!(back, j);
    // V2 appends the three fields to the tuple head and one tail
    let b2 = j.abi_encode(JournalVersion::V2);
    assert_eq!(b2.len(), b.len() + 32 * 3 + 32 * 2);
    assert_eq!(&b2[32 * 7..32 * 8], &[9u8; 32][..]);
    let mut dirty = b.clone();
    dirty[32 * 10 + 3] = 1; // non-zero address padding
    assert!(UpdateJournal::abi_decode_v1(&dirty).is_none());
}

#[test]
fn the_state_commitment_binds_every_field() {
    let base = G2State::genesis(KIND_TRUSTED_TIP, [1; 32], BlockRef { id: 5, hash: [2; 32] });
    let c = base.commitment();
    let muts: Vec<Box<dyn Fn(&mut G2State)>> = vec![
        Box::new(|s| s.finality_kind = KIND_LOGOS_LIGHT_CLIENT),
        Box::new(|s| s.l1_state[0] ^= 1),
        Box::new(|s| s.bridge_program[0] ^= 1),
        Box::new(|s| s.tip.id += 1),
        Box::new(|s| s.tip.hash[0] ^= 1),
        Box::new(|s| s.claim_acc.root[0] ^= 1),
        Box::new(|s| s.claim_acc.count += 1),
        Box::new(|s| s.view_hash[0] ^= 1),
        Box::new(|s| s.exit_count += 1),
        Box::new(|s| s.wound_down.push([0; 20])),
    ];
    for m in muts {
        let mut s = base.clone();
        m(&mut s);
        assert_ne!(s.commitment(), c);
    }
}

// ------------------------------------------------------------------------------- private

fn tok(eth: &str, def: &str, denoms: &[u128], claims: &[u64], burns: &[u64], cv: u128, bv: u128) -> TokenView {
    TokenView { eth_token: h20(eth), definition: acct(def), denoms: denoms.to_vec(), claims: claims.to_vec(), burns: burns.to_vec(), claimed_value: cv, burned_value: bv }
}

/// private5's bridge config before block 782 (`lez-bridge show`, run/private5).
fn private5_view(claims: &[u64], burns: &[u64], cv: u128, bv: u128) -> BridgeView {
    BridgeView {
        initialized: true,
        cfg_id: h32("0x5a5052abbd25f33c42a9460ac7da7ed7a0ef44058455dfe780e7374a82f9d6e5"),
        token_program: acct("BS5aAhTshWXLQxn16DwfTJwnDZhyDyxEZj4AkMUPrp9D"),
        stand_in: true,
        tokens: vec![tok("0x1613beb3b2c4f22ee086b2b38c1476a3ce7f78e8", "FJAT3Fz5SWJKoxkcXtprTJxcMyAG888FLv7Mk2HkteMs", &[100_000_000, 1_000_000_000], claims, burns, cv, bv)],
    }
}
const PRIVATE5_BRIDGE: &str = "E4ZJuR5AA7Zf1exBDA6zmdrAoBCw2jX7gSUR696LSpNv";

/// sepolia2's bridge config before block 4998 (one public attested claim, block 2853).
fn sepolia2_view() -> BridgeView {
    BridgeView {
        initialized: true,
        cfg_id: h32("0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11"),
        token_program: acct("4VScJ7mtwaanQ5gcSZZw2iAMHYAbLrVmbYKwEGx2eSex"),
        stand_in: false,
        tokens: vec![
            tok("0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e", "FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ", &[100_000_000, 1_000_000_000, 10_000_000_000], &[1, 0, 0], &[0, 0, 0], 100_000_000, 0),
            tok("0x0000000000000000000000000000000000000000", "CswQqK9jzUHf5sraiM8G9HfgMT2bcJ8GhVpbAA9trwQ6", &[10_000_000_000_000_000, 100_000_000_000_000_000, 1_000_000_000_000_000_000], &[0, 0, 0], &[0, 0, 0], 0, 0),
        ],
    }
}
const SEPOLIA2_BRIDGE: &str = "JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7";

/// A state at `id - 1` with this view (the hash of block id - 1 is block id's parent hash).
fn state_before(bridge: &str, id: u64, view: BridgeView) -> HostState {
    let raw = block("private", id);
    let p = wire::parse_block(&raw).unwrap();
    let mut s = HostState::genesis(acct(bridge), BlockRef { id: id - 1, hash: p.header.prev_block_hash });
    s.g2.view_hash = view.hash();
    s.view = view;
    s
}

#[test]
fn private_claims_and_burns_extract() {
    // private5 782: a stand-in private claim of 100 (one-time recipient, sponsored)
    let p = extract_unchecked(&state_before(PRIVATE5_BRIDGE, 782, private5_view(&[0, 0], &[0, 0], 0, 0)), &[block("private", 782)]).unwrap();
    assert_eq!(p.out.stats.private_verified, 1, "one private transaction reaches the bridge (its receipt accepted unchecked here)");
    assert_eq!(p.out.journal.claims, vec![ClaimDelta { token: h20("0x1613beb3b2c4f22ee086b2b38c1476a3ce7f78e8"), denom_index: 0, count: 1 }]);
    assert_eq!(p.out.batch.nullifiers.len(), 1);
    assert!(p.out.journal.deposit_roots_used.is_empty(), "a private claim publishes no root (it was recorded by AttestRoot earlier)");
    // private5 884: a private burn of 100 (after claims [1, 1])
    let p = extract_unchecked(&state_before(PRIVATE5_BRIDGE, 884, private5_view(&[1, 1], &[0, 0], 1_100_000_000, 0)), &[block("private", 884)]).unwrap();
    assert_eq!(p.out.stats.private_verified, 1);
    assert_eq!(p.out.journal.exits.len(), 1);
    assert_eq!(p.out.journal.exits[0].denom_index, 0);
    assert_eq!(p.next.view.tokens[0].burns, vec![1, 0]);
    // sepolia2 4998: a private ATTESTED claim (RFP-021)
    let p = extract_unchecked(&state_before(SEPOLIA2_BRIDGE, 4998, sepolia2_view()), &[block("private", 4998)]).unwrap();
    assert_eq!(p.out.stats.private_verified, 1);
    assert_eq!(p.out.journal.claims, vec![ClaimDelta { token: h20("0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e"), denom_index: 0, count: 1 }]);
    assert_eq!(p.next.view.tokens[0].claims, vec![2, 0, 0], "= `lez-bridge show` for sepolia2 today");
    assert!(p.out.journal.deposit_roots_used.is_empty(), "the attested path publishes no root");
}

/// The 3.0.x receipts in the fixtures (LEZ rc1; LEZ rc3 makes them with the same risc0 3.0.5) are
/// refused by the risc0 3.1 verifier under any circuit id, natively and so in the statement.
#[test]
fn a_risc0_3_0_privacy_receipt_is_refused_by_the_3_1_verifier() {
    use outbound_core::update::ReceiptVerifier as _;
    for id in [782, 884, 4998] {
        let (journal, proof) = private_tx(id);
        assert!(!NativeVerifier::for_circuit(outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1).verify(&journal, &proof), "block {id}");
        assert!(!NativeVerifier::default().verify(&journal, &proof), "block {id}");
    }
    let e = err(native_rc1(&state_before(SEPOLIA2_BRIDGE, 4998, sepolia2_view()), &[block("private", 4998)]));
    assert!(e.contains("privacy-circuit receipt does not verify"), "{e}");
}

#[test]
#[ignore = "needs a privacy receipt made with risc0 3.1 (LEZ's are 3.0.x; GHSA-g6qg-9j8p-7j87)"]
fn a_tampered_private_boundary_fails_its_receipt() {
    let raw = block("private", 4998);
    let p = wire::parse_block(&raw).unwrap();
    let (journal, proof) = p
        .txs
        .iter()
        .find_map(|t| match t {
            wire::ParsedTx::Private { journal_payload, proof, .. } => Some((wire::journal_frame(journal_payload), proof.to_vec())),
            wire::ParsedTx::Public { .. } => None,
        })
        .unwrap();
    use outbound_core::update::ReceiptVerifier as _;
    let rc1 = || NativeVerifier::for_circuit(outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1);
    assert!(rc1().verify(&journal, &proof));
    for at in [4, journal.len() / 2, journal.len() - 1] {
        let mut j = journal.clone();
        j[at] ^= 1;
        assert!(!rc1().verify(&j, &proof), "byte {at}");
    }
    // a damaged seal does not verify
    let mut wrong = proof.clone();
    let n = wrong.len();
    wrong[n / 2] ^= 1;
    assert!(!rc1().verify(&journal, &wrong));
    // under the circuit the guest pins (LEZ rc3's) the rc1 receipt does not verify: the pin matters
    assert_ne!(outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID, outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1);
    assert!(!NativeVerifier::default().verify(&journal, &proof));
}

/// The trusted-tip guest (`unchecked-private`) takes a private claim or burn on the sequencer's
/// word: natively and in the executor it extracts them with NO assumption, so a risc0 3.0.x receipt
/// (all LEZ rc1/rc3 ones) cannot stop an update. The private fixtures in the executor, each against
/// the native journal: a private attested claim (4998), a stand-in private claim (782) and a
/// private burn (884).
#[test]
fn the_trusted_tip_guest_extracts_private_transactions_without_receipts() {
    for (id, st) in [
        (4998, state_before(SEPOLIA2_BRIDGE, 4998, sepolia2_view())),
        (782, state_before(PRIVATE5_BRIDGE, 782, private5_view(&[0, 0], &[0, 0], 0, 0))),
        (884, state_before(PRIVATE5_BRIDGE, 884, private5_view(&[1, 1], &[0, 0], 1_100_000_000, 0))),
    ] {
        let p = native(&st, &[block("private", id)]).unwrap();
        assert!(p.receipts.is_empty(), "block {id}: no receipt kept, none passed");
        assert_eq!(p.out.stats.private_verified, 1, "block {id}: one private transaction reaches the bridge");
        let run = execute_trusted_tip(&p).unwrap();
        assert_eq!(run.assumptions, 0);
        assert_eq!(run.journal, p.journal, "block {id}");
        println!("block {id} ({} bytes, private): {} user cycles, {} segments, 0 assumptions", p.blocks[0].len(), run.user_cycles, run.segments);
    }
    // the same as the receipt-checking statement would extract (AcceptAll vs SequencerTrusted)
    let st = state_before(PRIVATE5_BRIDGE, 884, private5_view(&[1, 1], &[0, 0], 1_100_000_000, 0));
    assert_eq!(native(&st, &[block("private", 884)]).unwrap().journal, extract_unchecked(&st, &[block("private", 884)]).unwrap().journal);
}

/// The receipt-checking statement (the light-client build's policy) with a receipt as an
/// assumption: needs a privacy receipt made with risc0 3.1, which LEZ does not make yet.
#[test]
#[ignore = "needs a privacy receipt made with risc0 3.1 (LEZ's are 3.0.x; GHSA-g6qg-9j8p-7j87)"]
fn a_private_claim_in_the_executor_uses_its_receipt_as_an_assumption() {
    let p = native_rc1(&state_before(SEPOLIA2_BRIDGE, 4998, sepolia2_view()), &[block("private", 4998)]).unwrap();
    let run = execute_trusted_tip(&p).unwrap();
    assert_eq!(run.assumptions, 1);
    assert_eq!(run.stats.private_verified, 1);
    println!("block 4998 ({} bytes, private attested claim): {} user cycles, {} segments; {:?}", p.blocks[0].len(), run.user_cycles, run.segments, run.stats);
    // the other private fixtures in the executor too: a stand-in private claim and a private burn
    for (id, view) in [
        (782, private5_view(&[0, 0], &[0, 0], 0, 0)),
        (884, private5_view(&[1, 1], &[0, 0], 1_100_000_000, 0)),
    ] {
        let q = native_rc1(&state_before(PRIVATE5_BRIDGE, id, view), &[block("private", id)]).unwrap();
        let r = execute_trusted_tip(&q).unwrap();
        assert_eq!(r.assumptions, 1);
        println!("block {id} ({} bytes): {} user cycles, {} segments; {:?}", q.blocks[0].len(), r.user_cycles, r.segments, r.stats);
    }
    // without the receipt the executor cannot resolve env::verify
    assert!(execute(OUTBOUND_GUEST_TRUSTED_TIP_ELF, update_image_id_trusted_tip(), &p.header, &p.blocks, &[]).is_err());
}

#[test]
#[ignore = "needs a privacy receipt made with risc0 3.1 (LEZ's are 3.0.x; GHSA-g6qg-9j8p-7j87)"]
fn accept_all_extracts_the_same_as_native_verification() {
    let st = state_before(PRIVATE5_BRIDGE, 884, private5_view(&[1, 1], &[0, 0], 1_100_000_000, 0));
    let blocks = [block("private", 884)];
    let input = UpdateInput { prev: st.g2.clone(), view: st.view.clone(), finality: borsh::to_vec(&BlockRef { id: 884, hash: wire::parse_block(&blocks[0]).unwrap().hash }).unwrap(), blocks: vec![&blocks[0]] };
    let mut acc = HostAcc { tree: st.tree().unwrap(), witnesses: vec![] };
    let a = update::<TrustedTip>(&input, &mut acc, &mut AcceptAll, &NoMeter).unwrap();
    assert_eq!(a.journal, native_rc1(&st, &blocks).unwrap().out.journal);
}

/// The accumulator key is the claim nullifier PDA's account id. In both privacy modes it is
/// public: a private claim's `SpendClaim` delivery names the PDA and carries the seed that
/// derives it (`with_pda_seeds`), and a public claim lists the PDA among its public actors.
#[test]
fn the_nullifier_pda_and_its_seed_are_public_in_both_modes() {
    use lee_core::execution_state::BoundaryStep;
    for (id, bridge, view) in [
        (782, PRIVATE5_BRIDGE, private5_view(&[0, 0], &[0, 0], 0, 0)),
        (4998, SEPOLIA2_BRIDGE, sepolia2_view()),
    ] {
        let raw = block("private", id);
        let bp = AccountId::new(acct(bridge));
        let p = extract_unchecked(&state_before(bridge, id, view), &[raw.clone()]).unwrap();
        let key = p.out.batch.nullifiers[0];
        let parsed = wire::parse_block(&raw).unwrap();
        let mut found = 0;
        for tx in &parsed.txs {
            let wire::ParsedTx::Private { execution, .. } = tx else { continue };
            for s in &execution.boundary {
                let BoundaryStep::PrivateToPublic(d) = s else { continue };
                if *d.envelope.to.account_id.value() != key {
                    continue;
                }
                assert!(d.pda_seeds.iter().any(|seed| AccountId::for_public_pda(&bp, seed) == d.envelope.to.account_id), "block {id}: seed");
                found += 1;
            }
        }
        assert_eq!(found, 1, "block {id}: one SpendClaim delivery to the key, with its seed");
    }
    // Public claims: the old public9 blocks were made by the program before the 2026-11-14 fixes,
    // whose nullifier was SHA256("RFP022/nfc" | k): that PDA is among their actors. The key G2 now
    // derives (new formula) is checked against blocks of the fixed program in
    // `the_fixed_programs_nullifier_pda_is_the_key_g2_derives`.
    for id in [2900, 2901] {
        let raw = block("public9", id);
        let whole = wire::Block::try_from_slice(&raw).unwrap();
        let bp = AccountId::new(acct(P9_BRIDGE));
        let in_actors = whole.body.transactions.iter().any(|t| match t {
            wire::LeeTransaction::Public(tx) => {
                let Ok(bridge_core::Message::Instruction(bridge_core::Instruction::Claim { secret, .. })) =
                    borsh::from_slice::<bridge_core::Message>(&tx.message.execution.root.message)
                else {
                    return false;
                };
                let old = bridge_core::sha256(&[bridge_core::NFC_DOMAIN, &secret[..]]);
                let pda = nullifier_address(&bp, &old);
                tx.message.public_context.actors.iter().any(|a| a.account_id == pda)
            }
            wire::LeeTransaction::PrivacyPreserving(_) => false,
        });
        assert!(in_actors, "block {id}: the old program's nullifier PDA is among the public claim's actors");
    }
}

/// The key G2 derives for a public claim (the new nullifier, with the root call's account as
/// `dest`) is exactly the nullifier PDA the FIXED program spent in that transaction: the claims of
/// the secfix run, two by the same secret for two accounts (review H2) and one relayed.
#[test]
fn the_fixed_programs_nullifier_pda_is_the_key_g2_derives() {
    let bridge = AccountId::new(acct(SF_BRIDGE));
    let mut keys = Vec::new();
    for id in [1022, 1023, 1027] {
        let raw = block("secfix", id);
        let whole = wire::Block::try_from_slice(&raw).unwrap();
        for (i, t) in whole.body.transactions.iter().enumerate() {
            let wire::LeeTransaction::Public(tx) = t else { continue };
            let root = &tx.message.execution.root;
            if root.to.program_account_id != bridge {
                continue;
            }
            let mut view = BridgeView {
                initialized: true,
                cfg_id: [0; 32],
                token_program: acct(SF_TOKEN_PROGRAM),
                stand_in: true,
                tokens: vec![tok(SF_USDC, SF_DEF, &[100_000_000, 1_000_000_000], &[0, 0], &[0, 0], 0, 0)],
            };
            let mut out = BatchOut::default();
            Extractor::new(bridge, &mut view, &mut out).root(At { block_id: id, tx: i }, root).unwrap();
            assert_eq!(out.nullifiers.len(), 1, "block {id}");
            let key = out.nullifiers[0];
            assert!(
                tx.message.public_context.actors.iter().any(|a| *a.account_id.value() == key),
                "block {id}: G2's key is not the nullifier PDA the fixed program spent"
            );
            keys.push(key);
        }
    }
    assert_eq!(keys.len(), 3, "three public claims");
    assert!(keys[0] != keys[1], "the same secret for two accounts gives two nullifiers");
}

// ------------------------------------------------------------------------------- proving path
// Nothing here proves: the gate, the receipt check and the guest pin are tested on their own,
// the receipt check on a real succinct receipt (an rc1 privacy-circuit receipt from a fixture).

fn no_r0vm() -> Vec<PathBuf> {
    vec![PathBuf::from("/nonexistent/r0vm")]
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_string())
}

#[test]
fn the_prove_gate_admits_only_the_local_cpu_prover() {
    let ok: &[(&str, &str)] = &[("RISC0_PROVER", "local"), ("RAYON_NUM_THREADS", "4")];
    prove_gate(env_of(ok), &no_r0vm()).unwrap();
    for (pairs, want) in [
        (&[("RAYON_NUM_THREADS", "4")][..], "RISC0_PROVER"),
        (&[("RISC0_PROVER", "ipc"), ("RAYON_NUM_THREADS", "4")][..], "RISC0_PROVER"),
        (&[("RISC0_PROVER", "local")][..], "RAYON_NUM_THREADS"),
        (&[("RISC0_PROVER", "local"), ("RAYON_NUM_THREADS", "8")][..], "RAYON_NUM_THREADS"),
        (&[("RISC0_PROVER", "local"), ("RAYON_NUM_THREADS", "4"), ("RISC0_DEV_MODE", "1")][..], "RISC0_DEV_MODE"),
        (&[("RISC0_PROVER", "local"), ("RAYON_NUM_THREADS", "4"), ("RISC0_SERVER_PATH", "/x/r0vm")][..], "RISC0_SERVER_PATH"),
        (&[("RISC0_PROVER", "local"), ("RAYON_NUM_THREADS", "4"), ("BONSAI_API_KEY", "k")][..], "BONSAI_API_KEY"),
    ] {
        let pairs: &'static [(&'static str, &'static str)] = Box::leak(pairs.to_vec().into_boxed_slice());
        let e = prove_gate(env_of(pairs), &no_r0vm()).unwrap_err().to_string();
        assert!(e.contains(want), "{pairs:?}: {e}");
    }
    // an r0vm anywhere it could be picked up refuses
    let dir = std::env::temp_dir().join(format!("r0vm-gate-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".risc0/extensions/v3.0.5-cargo-risczero")).unwrap();
    std::fs::write(dir.join(".risc0/extensions/v3.0.5-cargo-risczero/r0vm"), b"").unwrap();
    let c = r0vm_candidates(Some("/usr/bin:/bin"), Some(dir.to_str().unwrap()));
    let e = prove_gate(env_of(ok), &c).unwrap_err().to_string();
    assert!(e.contains("an r0vm exists"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Runs today: a real (risc0 3.0.x) succinct receipt survives the file round trip and is refused
/// by the 3.1 verifier; a fake receipt is refused as fake.
#[test]
fn check_receipt_refuses_fakes_and_3_0_receipts() {
    use risc0_zkvm::{FakeReceipt, InnerReceipt, MaybePruned, Receipt, ReceiptClaim, sha::Digest};
    let (journal, proof) = private_tx(4998);
    let r = Receipt::new(borsh::from_slice::<InnerReceipt>(&proof).unwrap(), journal.clone());
    let rc1 = outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1;
    let f = std::env::temp_dir().join(format!("receipt3-{}.bin", std::process::id()));
    save_receipt(&r, &f).unwrap();
    let back = load_receipt(&f).unwrap();
    std::fs::remove_file(&f).unwrap();
    assert_eq!(borsh::to_vec(&back).unwrap(), borsh::to_vec(&r).unwrap());
    let e = check_receipt(&r, rc1, Some(&journal)).unwrap_err().to_string();
    assert!(e.contains("does not verify"), "{e}");
    let claim = ReceiptClaim::ok(Digest::from_bytes(rc1), MaybePruned::Pruned(Digest::from(sha256(&journal))));
    let fake = Receipt::new(InnerReceipt::Fake(FakeReceipt::new(claim)), journal.clone());
    assert!(check_receipt(&fake, rc1, Some(&journal)).unwrap_err().to_string().contains("fake"));
}

#[test]
#[ignore = "needs a privacy receipt made with risc0 3.1 (LEZ's are 3.0.x; GHSA-g6qg-9j8p-7j87)"]
fn check_receipt_verifies_a_real_succinct_receipt_and_refuses_the_rest() {
    use risc0_zkvm::{FakeReceipt, InnerReceipt, MaybePruned, Receipt, ReceiptClaim, sha::Digest};
    let p = native_rc1(&state_before(SEPOLIA2_BRIDGE, 4998, sepolia2_view()), &[block("private", 4998)]).unwrap();
    let r = &p.receipts[0];
    let rc1 = outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID_RC1;
    let j = r.journal.bytes.clone();
    assert_eq!(check_receipt(r, rc1, Some(&j)).unwrap(), "succinct");
    // round trip through the receipt file
    let f = std::env::temp_dir().join(format!("receipt-{}.bin", std::process::id()));
    save_receipt(r, &f).unwrap();
    let back = load_receipt(&f).unwrap();
    std::fs::remove_file(&f).unwrap();
    assert_eq!(check_receipt(&back, rc1, Some(&j)).unwrap(), "succinct");
    // another image id, another journal, a tampered journal, a fake receipt: refused
    assert!(check_receipt(r, outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID, None).is_err());
    assert!(check_receipt(r, rc1, Some(b"other")).unwrap_err().to_string().contains("not the expected journal"));
    let mut t = r.clone();
    t.journal.bytes[4] ^= 1;
    assert!(check_receipt(&t, rc1, None).is_err());
    let claim = ReceiptClaim::ok(Digest::from_bytes(rc1), MaybePruned::Pruned(Digest::from(sha256(&j))));
    let fake = Receipt::new(InnerReceipt::Fake(FakeReceipt::new(claim)), j.clone());
    assert!(check_receipt(&fake, rc1, Some(&j)).unwrap_err().to_string().contains("fake"));
}

#[test]
fn a_guest_elf_runs_only_under_its_pinned_image_id() {
    let tt = Guest::trusted_tip();
    let f = std::env::temp_dir().join(format!("guest-{}.bin", std::process::id()));
    std::fs::write(&f, &tt.elf).unwrap();
    let g = Guest::from_file(&f, tt.image).unwrap();
    assert_eq!(g.image, update_image_id_trusted_tip());
    let e = Guest::from_file(&f, release_image_id()).err().unwrap().to_string();
    assert!(e.contains("is not the pinned"), "{e}");
    std::fs::remove_file(&f).unwrap();
    assert!(Guest::embedded_pinned(Guest::trusted_tip(), Some(release_image_id())).is_err());
    assert!(Guest::embedded_pinned(Guest::release(), Some(release_image_id())).is_ok());
}

/// The same update at po2 19 has about twice the segments and the same journal.
#[test]
fn the_segment_size_changes_segments_not_the_journal() {
    let p = prepare(&p9_genesis(), p9_blocks(2893, 2899)).unwrap();
    let g = Guest::trusted_tip();
    let a = execute_guest(&g, &p, None).unwrap();
    let b = execute_guest(&g, &p, Some(19)).unwrap();
    assert_eq!(a.journal, b.journal);
    assert!(b.segments >= a.segments, "{} vs {}", b.segments, a.segments);
    println!("public9 2893..2899: {} segments at po2 20, {} at po2 19", a.segments, b.segments);
    assert!(execute_guest(&g, &p, Some(30)).is_err());
}

/// Branch open-relay: the walker reads a refunded public claim (`Instruction::Relayed` as the root
/// call) and a private execution whose boundary carries the claim's `Effect::Refund` shape and a
/// `Effect::SponsoredBurn` from a private forwarder (presented under an alias), and extracts the
/// claim (counted, nullifier spent) and the burn (counted, its exit) exactly once each.
/// Extraction level (an `Extractor` over a root call and a `ProvenExecution`), not a stored block:
/// no chain fixture of this branch's programs is recorded.
#[test]
fn a_relayed_claim_a_refund_and_a_sponsored_burn_extract() {
    use bridge_core::{Effect, Instruction, Message};
    use bridge_token_core::BurnNotice;
    use lee_core::{ProvenExecution, execution_state::{BoundaryStep, Delivery}, program::MessageEnvelope};
    let bridge = AccountId::new([0xB0; 32]);
    let tp = AccountId::new([0x70; 32]);
    let def = AccountId::new([0xDE; 32]);
    let usdc = [0xAA; 20];
    let config = bridge_core::config_address(&bridge);
    let mut v = BridgeView {
        initialized: true,
        cfg_id: [1; 32],
        token_program: *tp.value(),
        stand_in: true,
        tokens: vec![TokenView { eth_token: usdc, definition: *def.value(), denoms: vec![100], claims: vec![0], burns: vec![0], claimed_value: 0, burned_value: 0 }],
    };
    let at = At { block_id: 1, tx: 0 };
    let claimer = AccountId::new([0xA1; 32]);
    let claim = Instruction::Claim { eth_token: usdc, denom_index: 0, denom: 100, root: [2; 32], leaf_index: 0, path: vec![], secret: [3; 32], cfg_id: [1; 32], recipient_opening: None };
    let relayed = Message::Instruction(Instruction::Relayed { refund_to: AccountId::new([0x91; 32]), claim: Box::new(claim) });
    let root = RootCall { to: Actor::new(claimer, bridge), message: borsh::to_vec(&relayed).unwrap() };
    let mut out = BatchOut::default();
    Extractor::new(bridge, &mut v, &mut out).root(at, &root).unwrap();
    let nf = nullifier_address(&bridge, &claim_nullifier(&[3; 32], &claimer, &usdc, 100));
    assert_eq!(out.nullifiers, vec![*nf.value()], "the relayed claim's nullifier");
    assert_eq!((v.tokens[0].claims.clone(), v.tokens[0].claimed_value), (vec![1], 100), "the relayed claim counted once");

    let w = AccountId::new([0x77; 32]);
    let delivery = |e: Effect| {
        BoundaryStep::PrivateToPublic(Delivery {
            envelope: MessageEnvelope { from: Actor::new(w.blinded(&[9; 32]), bridge), to: Actor::new(config, bridge), message: borsh::to_vec(&Message::Effect(e)).unwrap() },
            inherited_authorizations: Default::default(),
            inherits_entry_authorizations: false,
            pda_seeds: Default::default(),
        })
    };
    let notice = BurnNotice { definition: def, amount: 100, memo: [5; 32] };
    let ex = ProvenExecution {
        boundary: vec![delivery(Effect::Refund { to: AccountId::new([0x91; 32]) }), delivery(Effect::SponsoredBurn { token_program: tp, notice, to: w })],
        ..ProvenExecution::default()
    };
    let mut out2 = BatchOut::default();
    Extractor::new(bridge, &mut v, &mut out2).private(at, &ex).unwrap();
    assert_eq!(out2.exits, vec![Exit { token: usdc, denom_index: 0, commitment: [5; 32] }], "the sponsored burn's exit, once");
    assert_eq!((v.tokens[0].burns.clone(), v.tokens[0].burned_value), (vec![1], 100));
    assert!(out2.nullifiers.is_empty());
    // A sponsored burn from a foreign program is refused, as the bridge refuses it.
    let bad = ProvenExecution {
        boundary: vec![delivery(Effect::SponsoredBurn { token_program: AccountId::new([0x55; 32]), notice: BurnNotice { definition: def, amount: 100, memo: [6; 32] }, to: w })],
        ..ProvenExecution::default()
    };
    assert!(Extractor::new(bridge, &mut v, &mut BatchOut::default()).private(at, &bad).is_err());
}
