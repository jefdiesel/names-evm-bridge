//! Adversarial claim and burn tests (audit IDs B2, B3, B7, B8 and the bridge half of B16), on the
//! shared simulator (`common/mod.rs`; read its docs for what it does not model).
//!
//! "Unchanged" below means every actor state in the simulated world is byte-for-byte what it was
//! (supply, claim counters, claimed/burned value, the float and every balance included), checked
//! with the explicit numbers as well so a failure says what moved.

mod common;

use common::*;
use eth_attest_core::inclusion::InclusionClass;

const D100: u128 = 100_000_000;
const D1000: u128 = 1_000_000_000;

/// The numbers a refused claim or burn must not move, for the default deployment.
#[derive(Debug, PartialEq)]
struct Numbers {
    supply: u128,
    claims: Vec<u64>,
    burns: Vec<u64>,
    claimed_value: u128,
    burned_value: u128,
    float: u128,
    balances: Vec<u128>,
}

fn numbers(w: &World) -> Numbers {
    let t = &w.config().tokens[0];
    Numbers {
        supply: w.supply(),
        claims: t.claims.clone(),
        burns: t.burns.clone(),
        claimed_value: t.claimed_value,
        burned_value: t.burned_value,
        float: w.float(),
        balances: [ALICE, BOB, CAROL].iter().map(|a| w.wrapped(*a) + w.native(*a)).collect(),
    }
}

/// `r` is a refusal containing `want`, and the world is exactly `before`.
#[track_caller]
fn refused(w: &World, before: &World, r: Result<(), String>, want: &str) {
    let e = r.expect_err("expected a refusal");
    assert!(e.contains(want), "wanted '{want}', got: {e}");
    assert_eq!(numbers(w), numbers(before), "a refused attempt moved a number ({want})");
    assert!(w.states == before.states, "a refused attempt changed some state ({want})");
}

fn attest_root(w: &mut World, root: [u8; 32]) {
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
}

fn nullifier_spent(w: &World, k: &[u8; 32], dest: AccountId, token: EthAddress, denom: u128) -> bool {
    !w.state(nullifier_address(&BRIDGE, &claim_nullifier(k, &dest, &token, denom)), BRIDGE).is_empty()
}

// ---------------------------------------------------------------------------------------------
// B2 (RFP-022 R#3): a claimed deposit can't be claimed again in any other shape.

#[test]
fn a_claimed_deposit_is_refused_in_every_retry_shape() {
    let mut w = setup();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 100);
    let k = secret("b2", 1);
    let o = recipient_opening(&k);
    let mut leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("b2", 2), &BOB, &USDC, D100)];
    let root1 = root_of(&leaves);
    attest_root(&mut w, root1);
    // The first claim: one-time recipient, sponsored.
    w.tx_with_openings(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, Some(o)), &[o]).unwrap();
    assert_eq!((w.wrapped(ALICE), w.native(ALICE), w.float(), w.supply()), (D100, 7, 93, D100));
    // Later deposits, a later root that still contains Alice's leaf; also Bob reuses k for a
    // deposit of his own.
    leaves.push(leaf_for(&cfg(), &secret("b2", 3), &CAROL, &USDC, D1000));
    leaves.push(leaf_for(&cfg(), &k, &BOB, &USDC, D100));
    let root2 = root_of(&leaves);
    attest_root(&mut w, root2);
    let before = w.clone();
    let retry = |idx: usize, ls: &[[u8; 32]], opening: Option<[u8; 32]>| claim_msg(cfg(), USDC, k, idx, ls, 0, D100, opening);
    // The same deposit under the later root, with and without the opening.
    // Every retry is the one claim refusal; natively its witness is fine (`message_fault` None),
    // so the spent nullifier refused it, where the other signer's has a NotInRoot fault.
    let r = w.tx_with_openings(ALICE, BRIDGE, &retry(0, &leaves, Some(o)), &[o]);
    refused(&w, &before, r, CLAIM_REFUSED);
    assert_eq!(message_fault(ALICE, &retry(0, &leaves, Some(o))), None);
    let r = w.tx(ALICE, BRIDGE, &retry(0, &leaves, None));
    refused(&w, &before, r, CLAIM_REFUSED);
    // Under the original root without the opening, or with another opening.
    let r = w.tx(ALICE, BRIDGE, &retry(0, &leaves[..2], None));
    refused(&w, &before, r, CLAIM_REFUSED);
    let o2 = secret("b2/other-opening", 0);
    let r = w.tx_with_openings(ALICE, BRIDGE, &retry(0, &leaves[..2], Some(o2)), &[o2]);
    refused(&w, &before, r, CLAIM_REFUSED);
    // Another signer with Alice's secret (and opening): the commitment names Alice.
    let r = w.tx_with_openings(BOB, BRIDGE, &retry(0, &leaves, Some(o)), &[o]);
    refused(&w, &before, r, CLAIM_REFUSED);
    assert_eq!(message_fault(BOB, &retry(0, &leaves, Some(o))), Some(ClaimFault::NotInRoot));
    // The same deposit claimed as another denomination index.
    let r = w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 1, D100, None));
    refused(&w, &before, r, CLAIM_REFUSED);
    // Bob's own deposit that reused k: since the 2026-11-14 fixes the nullifier binds the
    // destination, so it is another nullifier and Bob's deposit is claimable (before them it was
    // refused, and whoever learned k could burn a deposit this way: review H2).
    assert_eq!(message_fault(BOB, &retry(3, &leaves, None)), None);
    w.tx(BOB, BRIDGE, &retry(3, &leaves, None)).unwrap();
    assert_eq!(w.wrapped(BOB), D100);
    // ...once.
    let before = w.clone();
    let r = w.tx(BOB, BRIDGE, &retry(3, &leaves, None));
    refused(&w, &before, r, CLAIM_REFUSED);
}

#[test]
fn a_deposit_claimed_without_an_opening_is_refused_with_one() {
    let mut w = setup();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 100);
    let k = secret("b2", 9);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D1000), leaf_for(&cfg(), &secret("b2", 10), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&leaves));
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 1, D1000, None)).unwrap();
    assert_eq!((w.wrapped(ALICE), w.float()), (D1000, 100), "no opening: no sponsoring");
    let before = w.clone();
    let o = recipient_opening(&k);
    let r = w.tx_with_openings(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 1, D1000, Some(o)), &[o]);
    refused(&w, &before, r, CLAIM_REFUSED);
}

#[test]
fn an_attested_claim_is_refused_under_every_other_anchor_and_root() {
    let mut f = attested_setup();
    set_sponsor(&mut f.w, 7);
    fund_float(&mut f.w, 100);
    let (i, o) = f.claim_one_time(0);
    f.w.tx_with_openings(ALICE, BRIDGE, &instr(i), &[o]).unwrap();
    assert_eq!((f.w.wrapped(ALICE), f.w.native(ALICE), f.w.float()), (D100, 7, 93));
    // The instance later finalizes a newer state, in which the vault also marked a later root
    // (three deposits, still holding Alice's leaf) true.
    let mut leaves = f.leaves.clone();
    leaves.push(leaf_for(&cfg(), &secret("b2a", 1), &CAROL, &USDC, D100));
    let later_root = root_of(&leaves);
    let mut later = f.state.clone();
    later.accounts[0] = later.accounts[0].clone().with(is_deposit_root(&later_root), word(1));
    let later_state = Anchor::StateRoot(later.state_root());
    plant_anchor(&mut f.w, &f.id, EVM_CHAIN, later_state);
    let later_header = header_rlp(later.state_root(), [0; 32], 12_000_064, 0);
    let later_block = Anchor::BlockHash(keccak(&later_header));
    plant_anchor(&mut f.w, &f.id, EVM_CHAIN, later_block);
    let before = f.w.clone();
    // Block-hash anchor instead of state-root anchor, without and with the opening.
    let r = f.w.tx(ALICE, BRIDGE, &instr(f.claim_by_block_hash(0)));
    refused(&f.w, &before, r, CLAIM_REFUSED);
    let (bh, _) = f.claim_one_time(0);
    let bh = with(bh, |x| *x.anchor = AnchorWitness { anchor: f.block_anchor, header_rlp: Some(f.header.clone()) });
    let r = f.w.tx_with_openings(ALICE, BRIDGE, &instr(bh), &[o]);
    refused(&f.w, &before, r, CLAIM_REFUSED);
    // A later anchor (both kinds), same root.
    let proof_later = later.storage_proof(&VAULT, &is_deposit_root(&f.root));
    let i = with(f.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor: later_state, header_rlp: None };
        *x.proof = proof_later.clone();
    });
    let r = f.w.tx(ALICE, BRIDGE, &instr(i));
    refused(&f.w, &before, r, CLAIM_REFUSED);
    let i = with(f.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor: later_block, header_rlp: Some(later_header.clone()) };
        *x.proof = proof_later.clone();
    });
    let r = f.w.tx(ALICE, BRIDGE, &instr(i));
    refused(&f.w, &before, r, CLAIM_REFUSED);
    // A later root that still contains the leaf, under the later anchor.
    let mut i = with(f.claim(0), |x| {
        *x.root = later_root;
        *x.anchor = AnchorWitness { anchor: later_state, header_rlp: None };
        *x.proof = later.storage_proof(&VAULT, &is_deposit_root(&later_root));
    });
    if let Instruction::ClaimAttested { path, .. } = &mut i {
        *path = merkle_path(&leaves, 0);
    }
    let r = f.w.tx(ALICE, BRIDGE, &instr(i));
    refused(&f.w, &before, r, CLAIM_REFUSED);
    // Another signer.
    let r = f.w.tx_with_openings(BOB, BRIDGE, &instr(f.claim_one_time(0).0), &[o]);
    refused(&f.w, &before, r, CLAIM_REFUSED);
    assert_eq!(witness_fault(BOB, &f.claim_one_time(0).0), Some(ClaimFault::NotInRoot));
}

// ---------------------------------------------------------------------------------------------
// B3 (RFP-022 R#1, R#2): late failures undo the whole claim or burn.

#[test]
fn a_mint_refused_after_the_nullifier_effect_undoes_the_claim() {
    const DEFX: AccountId = AccountId::new([0xD7; 32]);
    let mut w = setup();
    // A definition whose mint authority is Carol (an account), not the bridge, registered anyway.
    w.tx(DEFX, TOKEN, &Tok::NewFungibleDefinition { name: "Not the bridge's".into(), total_supply: 0, mint_authority: Some(Authority::Account(CAROL)), supply_to: ADMIN })
        .unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEFX, decimals: 18, denoms: vec![100] })))
        .unwrap();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 50);
    let k = secret("b3", 1);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &DAI, 100), leaf_for(&cfg(), &secret("b3", 2), &BOB, &DAI, 100)];
    attest_root(&mut w, root_of(&leaves));
    let o = recipient_opening(&k);
    let m = claim_msg(cfg(), DAI, k, 0, &leaves, 0, 100, Some(o));
    let before = w.clone();
    let r = w.tx_with_openings(ALICE, BRIDGE, &m, &[o]);
    refused(&w, &before, r, "only the mint authority may mint");
    // The refusal came after the nullifier effect ran: the trace shows SpendClaim delivered, and
    // yet the PDA is empty.
    assert!(w.trace.iter().any(|(_, _, msg, _)| *msg == borsh::to_vec(&Message::Effect(Effect::SpendClaim)).unwrap()));
    assert!(!nullifier_spent(&w, &k, ALICE, DAI, 100));
    assert_eq!(w.config().tokens[1].claims, vec![0]);
    assert_eq!((w.supply_of(DEFX), w.float()), (0, 50));
    // Carol hands the authority to the bridge program: the same claim now goes through.
    w.tx(CAROL, TOKEN, &Tok::SetAuthority { definition: DEFX, new_authority: Some(Authority::Program(BRIDGE)) }).unwrap();
    w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap();
    assert!(nullifier_spent(&w, &k, ALICE, DAI, 100));
    assert_eq!((w.bal(ALICE, DEFX), w.supply_of(DEFX), w.float(), w.native(ALICE)), (100, 100, 43, 7));
    assert_eq!(w.config().tokens[1].claims, vec![1]);
}

#[test]
fn a_claim_refused_by_a_token_freeze_leaves_the_nullifier_unspent() {
    // Stand-in: the config's freeze check runs after SpendClaim.
    let mut w = setup();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 50);
    let k = secret("b3", 3);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("b3", 4), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&leaves));
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    let o = recipient_opening(&k);
    let m = claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, Some(o));
    let before = w.clone();
    let r = w.tx_with_openings(ALICE, BRIDGE, &m, &[o]);
    refused(&w, &before, r, "minting is frozen");
    assert!(!nullifier_spent(&w, &k, ALICE, USDC, D100));
    w.tx(ADMIN, BRIDGE, &unfreeze(Some(USDC), true, false)).unwrap();
    w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap();
    assert_eq!((w.wrapped(ALICE), w.float(), w.config().tokens[0].claims.clone()), (D100, 43, vec![1, 0]));
    // Attested: the same.
    let mut f = attested_setup();
    set_sponsor(&mut f.w, 7);
    fund_float(&mut f.w, 50);
    f.w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    let (i, o) = f.claim_one_time(0);
    let before = f.w.clone();
    let r = f.w.tx_with_openings(ALICE, BRIDGE, &instr(i.clone()), &[o]);
    refused(&f.w, &before, r, "minting is frozen");
    assert!(!nullifier_spent(&f.w, &f.keys[0], ALICE, USDC, D100));
    f.w.tx(ADMIN, BRIDGE, &unfreeze(Some(USDC), true, false)).unwrap();
    f.w.tx_with_openings(ALICE, BRIDGE, &instr(i), &[o]).unwrap();
    assert_eq!((f.w.wrapped(ALICE), f.w.float()), (D100, 43));
}

/// Gives `who` `n` token kinds other than the wrapped ones (one unit of each).
fn fill_holdings(w: &mut World, who: AccountId, n: u8) {
    for i in 0..n {
        let def = AccountId::new([0x30 + i; 32]);
        w.tx(def, TOKEN, &Tok::NewFungibleDefinition { name: format!("Filler {i}"), total_supply: 1, mint_authority: None, supply_to: who }).unwrap();
    }
}

/// A claim whose token credit fails (the recipient already holds `MAX_HOLDINGS` kinds), with the
/// credit delivered inside the claim's transaction as LEZ does for a promoted async message (see
/// `World::atomic_asyncs`): the whole claim is undone, and goes through once a slot is free.
/// (With the simulator's default, the credit would run as its own later transaction and be undone
/// alone, leaving supply above the sum of balances; in LEZ an unpromoted credit waits in the
/// mailbox instead of being lost, so that is a simulator artifact, not a program bug.)
#[test]
fn a_claim_whose_credit_fails_in_its_transaction_is_undone_entirely() {
    let mut w = setup();
    w.atomic_asyncs = true;
    fill_holdings(&mut w, ALICE, bridge_token_core::MAX_HOLDINGS as u8);
    let k = secret("lf1", 1);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("lf1", 2), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&leaves));
    let m = claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None);
    let before = w.clone();
    let r = w.tx(ALICE, BRIDGE, &m);
    refused(&w, &before, r, "async: the recipient holds too many token kinds");
    assert!(!nullifier_spent(&w, &k, ALICE, USDC, D100));
    assert_eq!((w.supply(), w.holdings_sum(DEF), w.config().tokens[0].claimed_value), (0, 0, 0));
    // Alice spends one kind entirely: the same claim now lands.
    w.tx(ALICE, TOKEN, &Tok::Transfer { definition: AccountId::new([0x30; 32]), to: BOB, amount: 1 }).unwrap();
    w.tx(ALICE, BRIDGE, &m).unwrap();
    assert_eq!((w.wrapped(ALICE), w.supply(), w.holdings_sum(DEF)), (D100, D100, D100));
}

#[test]
fn refused_burns_change_nothing() {
    let mut w = setup();
    let k = secret("b3", 5);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D1000), leaf_for(&cfg(), &secret("b3", 6), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&leaves));
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 1, D1000, None)).unwrap();
    // A definition the bridge never registered, which Alice holds.
    const DEFY: AccountId = AccountId::new([0xD9; 32]);
    w.tx(DEFY, TOKEN, &Tok::NewFungibleDefinition { name: "Unbridged".into(), total_supply: 500, mint_authority: None, supply_to: ALICE }).unwrap();
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    let burn_of = |definition: AccountId, amount: u128| Tok::BurnWithNotice { definition, amount, notify, memo: [0x4E; 32] };
    let before = w.clone();
    let r = w.tx(ALICE, TOKEN, &burn_of(DEFY, 100));
    refused(&w, &before, r, "not a bridged token");
    assert_eq!((w.bal(ALICE, DEFY), w.supply_of(DEFY)), (500, 500));
    // Not exactly one denomination: below, between and above the denominations, and zero.
    for (amount, want) in [(50_000_000, "a burn is exactly one denomination"), (D100 + 1, "a burn is exactly one denomination"), (2 * D1000, "insufficient token balance"), (0, "burn amount must be positive")] {
        let r = w.tx(ALICE, TOKEN, &burn_of(DEF, amount));
        refused(&w, &before, r, want);
    }
    // Under a burn freeze, global or for the token.
    for scope in [None, Some(USDC)] {
        let mut x = w.clone();
        x.tx(FREEZER, BRIDGE, &freeze(scope, false, true)).unwrap();
        let frozen = x.clone();
        let r = x.tx(ALICE, TOKEN, &burn_of(DEF, D100));
        refused(&x, &frozen, r, "redemption is frozen");
        assert_eq!((x.wrapped(ALICE), x.supply()), (D1000, D1000));
    }
    // The control: an honest burn of one denomination goes through.
    w.tx(ALICE, TOKEN, &burn_of(DEF, D100)).unwrap();
    assert_eq!((w.wrapped(ALICE), w.supply(), w.config().tokens[0].burned_value), (900_000_000, 900_000_000, D100));
}

// ---------------------------------------------------------------------------------------------
// B7 (RFP-022 R#4): a valid claim stays valid forever.

#[test]
fn a_claim_under_an_old_root_works_after_newer_roots_and_delisting() {
    let mut w = setup();
    set_sponsor(&mut w, 3);
    fund_float(&mut w, 10);
    let (ka, kb) = (secret("b7", 1), secret("b7", 2));
    let mut leaves = vec![leaf_for(&cfg(), &ka, &ALICE, &USDC, D100), leaf_for(&cfg(), &kb, &BOB, &USDC, D1000)];
    let old = leaves.clone();
    attest_root(&mut w, root_of(&old));
    // Many later deposits and roots.
    for i in 0..5 {
        leaves.push(leaf_for(&cfg(), &secret("b7/later", i), &CAROL, &USDC, D100));
        attest_root(&mut w, root_of(&leaves));
    }
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::DeregisterToken { eth_token: USDC }))).unwrap();
    assert!(!w.config().tokens[0].registered);
    // Alice claims against the oldest root (one-time, sponsored), Bob against the oldest too.
    let o = recipient_opening(&ka);
    w.tx_with_openings(ALICE, BRIDGE, &claim_msg(cfg(), USDC, ka, 0, &old, 0, D100, Some(o)), &[o]).unwrap();
    w.tx(BOB, BRIDGE, &claim_msg(cfg(), USDC, kb, 1, &old, 1, D1000, None)).unwrap();
    assert_eq!((w.wrapped(ALICE), w.wrapped(BOB), w.native(ALICE), w.float()), (D100, D1000, 3, 7));
    assert_eq!(w.config().tokens[0].claims, vec![1, 1]);
}

#[test]
fn an_attested_claim_under_the_first_anchor_works_after_the_instance_moved_on() {
    let mut f = attested_setup();
    // The instance finalizes several newer states (planted, as its store would record them; the
    // real program never deletes an anchor PDA, see attest/program/tests/audit.rs B13).
    let mut state = f.state.clone();
    for i in 0..4u64 {
        state.accounts[3] = SynthAccount::new([0x22; 20]).with(word(u128::from(i)), word(1));
        plant_anchor(&mut f.w, &f.id, EVM_CHAIN, Anchor::StateRoot(state.state_root()));
        let h = header_rlp(state.state_root(), [0; 32], 12_000_100 + i, 0);
        plant_anchor(&mut f.w, &f.id, EVM_CHAIN, Anchor::BlockHash(keccak(&h)));
    }
    f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::DeregisterToken { eth_token: USDC }))).unwrap();
    // Both claims against the first anchors, of both kinds.
    f.tx(ALICE, f.claim(0)).unwrap();
    f.tx(BOB, f.claim_by_block_hash(1)).unwrap();
    assert_eq!((f.w.wrapped(ALICE), f.w.wrapped(BOB)), (D100, D1000));
}

// ---------------------------------------------------------------------------------------------
// B8: the freeze matrix on LEZ.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    None,
    GlobalMint,
    GlobalBurn,
    TokenMint,
    TokenBurn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Claim,
    AttestedClaim,
    Burn,
    AttestRoot,
    OtherTokenClaim,
    OtherTokenBurn,
}

const OK: &str = "ok";
const MF: &str = "minting is frozen";
const BF: &str = "redemption is frozen";

/// The expected outcome of each operation under each single flag (the token flags are on USDC).
const TABLE: [(Flag, [(Op, &str); 6]); 5] = [
    (Flag::None, [(Op::Claim, OK), (Op::AttestedClaim, OK), (Op::Burn, OK), (Op::AttestRoot, OK), (Op::OtherTokenClaim, OK), (Op::OtherTokenBurn, OK)]),
    (Flag::GlobalMint, [(Op::Claim, MF), (Op::AttestedClaim, MF), (Op::Burn, OK), (Op::AttestRoot, MF), (Op::OtherTokenClaim, MF), (Op::OtherTokenBurn, OK)]),
    (Flag::GlobalBurn, [(Op::Claim, OK), (Op::AttestedClaim, OK), (Op::Burn, BF), (Op::AttestRoot, OK), (Op::OtherTokenClaim, OK), (Op::OtherTokenBurn, BF)]),
    (Flag::TokenMint, [(Op::Claim, MF), (Op::AttestedClaim, MF), (Op::Burn, OK), (Op::AttestRoot, OK), (Op::OtherTokenClaim, OK), (Op::OtherTokenBurn, OK)]),
    (Flag::TokenBurn, [(Op::Claim, OK), (Op::AttestedClaim, OK), (Op::Burn, BF), (Op::AttestRoot, OK), (Op::OtherTokenClaim, OK), (Op::OtherTokenBurn, OK)]),
];

fn set_flag(w: &mut World, flag: Flag) {
    let m = match flag {
        Flag::None => return,
        Flag::GlobalMint => freeze(None, true, false),
        Flag::GlobalBurn => freeze(None, false, true),
        Flag::TokenMint => freeze(Some(USDC), true, false),
        Flag::TokenBurn => freeze(Some(USDC), false, true),
    };
    w.tx(FREEZER, BRIDGE, &m).unwrap();
}

/// Registers DAI (wrapped by DEF2) on the bridge in `w`.
fn add_dai(w: &mut World) {
    w.tx(DEF2, TOKEN, &Tok::NewFungibleDefinition { name: "Wrapped DAI".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN })
        .unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEF2, decimals: 18, denoms: vec![100] })))
        .unwrap();
}

/// Runs `op` on a fresh world with `flag` set. Returns "ok" or the exact refusal, after checking
/// that a refusal changed nothing and a success did what the operation does.
fn run(flag: Flag, op: Op) -> String {
    if op == Op::AttestedClaim {
        // The RFP-021 config: Alice's first claim, under the flag.
        let mut f = attested_setup();
        set_flag(&mut f.w, flag);
        let before = f.w.clone();
        return match f.tx(ALICE, f.claim(0)) {
            Ok(()) => {
                assert_eq!(f.w.wrapped(ALICE), D100);
                OK.into()
            }
            Err(e) => {
                assert!(f.w.states == before.states);
                e
            }
        };
    }
    let mut w = setup();
    add_dai(&mut w);
    // Two USDC and two DAI deposits by Alice; the first of each claimed before the flag is set,
    // so she has something to burn.
    let (ku, kd) = ([secret("b8u", 0), secret("b8u", 1)], [secret("b8d", 0), secret("b8d", 1)]);
    let leaves = vec![
        leaf_for(&cfg(), &ku[0], &ALICE, &USDC, D100),
        leaf_for(&cfg(), &ku[1], &ALICE, &USDC, D100),
        leaf_for(&cfg(), &kd[0], &ALICE, &DAI, 100),
        leaf_for(&cfg(), &kd[1], &ALICE, &DAI, 100),
    ];
    attest_root(&mut w, root_of(&leaves));
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, ku[0], 0, &leaves, 0, D100, None)).unwrap();
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), DAI, kd[0], 2, &leaves, 0, 100, None)).unwrap();
    set_flag(&mut w, flag);
    let before = w.clone();
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    let r = match op {
        Op::Claim => w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, ku[1], 1, &leaves, 0, D100, None)),
        Op::Burn => w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify, memo: [1; 32] }),
        Op::AttestRoot => w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: secret("b8/root", 0) })),
        Op::OtherTokenClaim => w.tx(ALICE, BRIDGE, &claim_msg(cfg(), DAI, kd[1], 3, &leaves, 0, 100, None)),
        Op::OtherTokenBurn => w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF2, amount: 100, notify, memo: [2; 32] }),
        Op::AttestedClaim => unreachable!(),
    };
    match r {
        Ok(()) => {
            match op {
                Op::Claim => assert_eq!(w.wrapped(ALICE), 2 * D100),
                Op::Burn => assert_eq!((w.wrapped(ALICE), w.config().tokens[0].burned_value), (0, D100)),
                Op::AttestRoot => assert!(!w.state(root_address(&BRIDGE, &secret("b8/root", 0)), BRIDGE).is_empty()),
                Op::OtherTokenClaim => assert_eq!(w.bal(ALICE, DEF2), 200),
                Op::OtherTokenBurn => assert_eq!((w.bal(ALICE, DEF2), w.config().tokens[1].burned_value), (0, 100)),
                Op::AttestedClaim => unreachable!(),
            }
            OK.into()
        }
        Err(e) => {
            assert!(w.states == before.states, "{flag:?} {op:?}: a refusal changed state");
            e
        }
    }
}

#[test]
fn the_freeze_matrix_matches_the_table() {
    let mut mismatches = Vec::new();
    for (flag, row) in TABLE {
        for (op, want) in row {
            let got = run(flag, op);
            if got != want {
                mismatches.push(format!("{flag:?} x {op:?}: expected {want:?}, got {got:?}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn an_attested_config_refuses_the_stand_in_root_under_any_flag() {
    for flag in [Flag::None, Flag::GlobalMint, Flag::GlobalBurn, Flag::TokenMint, Flag::TokenBurn] {
        let mut f = attested_setup();
        set_flag(&mut f.w, flag);
        f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetAttestor { attestor: ATTESTOR }))).unwrap();
        let before = f.w.clone();
        let r = f.w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: [9; 32] }));
        refused(&f.w, &before, r, "not the stand-in");
    }
}

// ---------------------------------------------------------------------------------------------
// B16, bridge half: malformed and oversized claim inputs are refused with clean errors.

fn assert_clean(e: &str) {
    for marker in ["index out of bounds", "attempt to", "called `Option::unwrap()`", "called `Result::unwrap()`", "Cannot extend", "capacity", "overflow", "slice index", "range end index"] {
        assert!(!e.contains(marker), "an internal panic leaks through: {e}");
    }
}

fn raw(w: &mut World, signer: AccountId, bytes: Vec<u8>) -> Result<(), String> {
    let snapshot = w.clone();
    let mut asyncs = VecDeque::new();
    let r = w.deliver(None, Actor::new(signer, BRIDGE), bytes, BTreeSet::new(), Some(signer), &mut asyncs);
    if r.is_err() {
        *w = snapshot;
    }
    r
}

#[test]
fn malformed_stand_in_claims_are_refused_cleanly() {
    let mut w = setup();
    let k = secret("b16", 1);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("b16", 2), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&leaves));
    let good = claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None);
    let edit = |f: &dyn Fn(&mut Instruction)| {
        let Message::Instruction(mut i) = good.clone() else { unreachable!() };
        f(&mut i);
        instr(i)
    };
    let set_path = |n: usize| {
        edit(&move |i| {
            if let Instruction::Claim { path, .. } = i {
                path.resize(n, [0x5C; 32]);
            }
        })
    };
    let before = w.clone();
    for (what, m, want) in [
        ("no path", set_path(0), CLAIM_REFUSED),
        ("31 siblings", set_path(31), CLAIM_REFUSED),
        ("33 siblings", set_path(33), CLAIM_REFUSED),
        ("100,000 siblings", set_path(100_000), CLAIM_REFUSED),
        (
            "leaf index 2^32",
            edit(&|i| {
                if let Instruction::Claim { leaf_index, .. } = i {
                    *leaf_index = 1 << 32;
                }
            }),
            CLAIM_REFUSED,
        ),
        (
            "leaf index u64::MAX",
            edit(&|i| {
                if let Instruction::Claim { leaf_index, .. } = i {
                    *leaf_index = u64::MAX;
                }
            }),
            CLAIM_REFUSED,
        ),
        (
            "denomination index u32::MAX",
            edit(&|i| {
                if let Instruction::Claim { denom_index, .. } = i {
                    *denom_index = u32::MAX;
                }
            }),
            "bad denomination",
        ),
    ] {
        let r = w.tx(ALICE, BRIDGE, &m);
        let e = r.clone().unwrap_err();
        assert_clean(&e);
        refused(&w, &before, r, want);
        if want == CLAIM_REFUSED {
            assert_eq!(e, CLAIM_REFUSED, "{what}");
            assert_eq!(message_fault(ALICE, &m), Some(ClaimFault::PathShape), "{what}");
        }
    }
    // A deposit of an unregistered token (a well-formed leaf for it, in an attested root).
    let kx = secret("b16", 3);
    let ls = vec![leaf_for(&cfg(), &kx, &ALICE, &[0x77; 20], 5), leaf_for(&cfg(), &secret("b16", 4), &BOB, &USDC, D100)];
    attest_root(&mut w, root_of(&ls));
    let before = w.clone();
    let r = w.tx(ALICE, BRIDGE, &claim_msg(cfg(), [0x77; 20], kx, 0, &ls, 0, 5, None));
    refused(&w, &before, r, "token not registered");
    // Undecodable bytes: empty, a bad variant, a truncated claim, trailing bytes.
    let enc = borsh::to_vec(&good).unwrap();
    // A fixed text: the decoder's own error (`Custom { kind: InvalidData, .. }`) is not printed.
    for bytes in [Vec::new(), vec![0xff; 40], enc[..enc.len() - 1].to_vec(), [&enc[..], &[0]].concat()] {
        let e = raw(&mut w, ALICE, bytes).unwrap_err();
        assert_eq!(e, "message must decode");
        assert_clean(&e);
    }
    assert!(w.states == before.states);
}

#[test]
fn malformed_attested_claims_are_refused_cleanly() {
    let mut f = attested_setup();
    let before = f.w.clone();
    let m = |i: Instruction| instr(i);
    let bad_header = f.header[..f.header.len() - 1].to_vec();
    // On chain each is the one claim refusal; natively (the CLI's diagnosis) its class.
    let cases: Vec<(&str, Instruction, InclusionClass)> = vec![
        ("no account proof nodes", with(f.claim(0), |x| x.proof.account.nodes.clear()), InclusionClass::AccountProof),
        ("no storage proof nodes", with(f.claim(0), |x| x.proof.nodes.clear()), InclusionClass::StorageProof),
        ("storage node truncated", with(f.claim(0), |x| {
            x.proof.nodes.last_mut().unwrap().pop();
        }), InclusionClass::StorageProof),
        ("garbage storage node appended", with(f.claim(0), |x| x.proof.nodes.push(vec![0xC0; 70])), InclusionClass::StorageProof),
        ("1 MiB storage node", with(f.claim(0), |x| x.proof.nodes.insert(0, vec![0xC0; 1 << 20])), InclusionClass::StorageProof),
        ("block hash without header", with(f.claim(0), |x| *x.anchor = AnchorWitness { anchor: f.block_anchor, header_rlp: None }), InclusionClass::Anchor),
        ("state root with a header", with(f.claim(0), |x| x.anchor.header_rlp = Some(f.header.clone())), InclusionClass::Anchor),
        ("truncated header", with(f.claim(0), |x| *x.anchor = AnchorWitness { anchor: Anchor::BlockHash(keccak(&bad_header)), header_rlp: Some(bad_header.clone()) }), InclusionClass::Anchor),
        ("1 MiB header", with(f.claim(0), |x| *x.anchor = AnchorWitness { anchor: f.block_anchor, header_rlp: Some(vec![0xff; 1 << 20]) }), InclusionClass::Anchor),
    ];
    for (what, i, class) in cases {
        let e = f.w.tx(ALICE, BRIDGE, &m(i.clone())).unwrap_err();
        assert_eq!(e, CLAIM_REFUSED, "{what}");
        assert_eq!(inclusion_class(ALICE, &i), Some(class), "{what}");
        assert_clean(&e);
        assert!(f.w.states == before.states, "{what}");
    }
}

/// An account proof whose joined path runs past 64 nibbles (see attest/program/tests/audit.rs).
fn overlong_path_claim(f: &Fixture) -> Instruction {
    let list = |items: &[Vec<u8>]| {
        let payload: Vec<u8> = items.concat();
        let mut out = vec![];
        let n = payload.len();
        if n < 56 {
            out.push(0xc0 + n as u8);
        } else {
            out.push(0xf8);
            out.push(n as u8);
        }
        out.extend(payload);
        out
    };
    let string = |b: &[u8]| {
        let mut out = vec![0x80 + b.len() as u8];
        out.extend_from_slice(b);
        out
    };
    let leaf = list(&[string(&[0x20, 0x12]), string(&[0x01; 40])]);
    let mut ext_key = vec![0x00];
    ext_key.extend_from_slice(&[0xAB; 32]);
    let ext = list(&[string(&ext_key), string(&keccak(&leaf))]);
    let anchor = Anchor::StateRoot(keccak(&ext));
    with(f.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
        x.proof.account.nodes = vec![ext.clone(), leaf.clone()];
    })
}

/// B16-NYB: the proof passes every hash check and used to panic inside nybbles ("Cannot extend:
/// resulting length would exceed maximum capacity") in the claimer's actor. Now the inclusion
/// library refuses it before alloy-trie walks it, and the bridge says only CLAIM_REFUSED.
#[test]
fn an_overlong_proof_path_is_refused_cleanly_by_the_bridge() {
    let mut f = attested_setup();
    let before = f.w.clone();
    let i = overlong_path_claim(&f);
    let e = f.tx(ALICE, i.clone()).unwrap_err();
    assert_eq!(e, CLAIM_REFUSED);
    assert_clean(&e);
    assert!(f.w.states == before.states);
    let Some(ClaimFault::Inclusion(native)) = witness_fault(ALICE, &i) else { panic!("an inclusion fault") };
    assert_eq!(native, eth_attest_core::inclusion::InclusionError::AccountProof("proof path longer than 64 nibbles".into()));
}
