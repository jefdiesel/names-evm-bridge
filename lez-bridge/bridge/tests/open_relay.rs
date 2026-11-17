// SPDX-License-Identifier: MIT OR Apache-2.0
//! The open claim relay (relay refund, `Instruction::Relayed`) and the sponsored burn
//! (`Effect::SponsoredBurn`), branch `open-relay`, 2026-11-17.
//!
//! Every test runs with `atomic_asyncs` on: the refund's and the burn sponsor's native credit is an
//! async message LEZ delivers inside the same transaction (a public payer's credit, or a credit the
//! wallet promotes to a witnessed private account), so a failing credit undoes the whole
//! transaction. The debit from the float is a sync message, so a short float fails the claim or
//! burn itself either way.

mod common;
use common::*;

const RELAY1: AccountId = AccountId::new([0x91; 32]);
const RELAY2: AccountId = AccountId::new([0x92; 32]);
/// A fresh one-time account a burner's wallet makes for one sponsored burn.
const W1: AccountId = AccountId::new([0x77; 32]);
const W2: AccountId = AccountId::new([0x78; 32]);
const R: u128 = 1_250_000;
const S: u128 = 6_000_000;
const D100: u128 = 100_000_000;

fn set_refund(w: &mut World, amount: u128) -> Result<(), String> {
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetRelayRefund { amount })))
}

fn set_burn_sponsor(w: &mut World, amount: u128) -> Result<(), String> {
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetBurnSponsor { amount })))
}

fn relayed(refund_to: AccountId, claim: Message) -> Message {
    let Message::Instruction(i) = claim else { unreachable!() };
    instr(Instruction::Relayed { refund_to, claim: Box::new(i) })
}

/// Four 100 deposits under one attested root: Alice (k1, k2), Bob (k3), Carol (k4). Float 100M,
/// relay refund R.
fn world() -> (World, Vec<([u8; 32], AccountId)>, Vec<[u8; 32]>, [u8; 32]) {
    let mut w = setup();
    w.atomic_asyncs = true;
    let ds = vec![([1u8; 32], ALICE), ([2u8; 32], ALICE), ([3u8; 32], BOB), ([4u8; 32], CAROL)];
    let (leaves, root) = vault(&ds.iter().map(|(k, who)| (*k, *who, D100)).collect::<Vec<_>>());
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    fund_float(&mut w, 100_000_000);
    set_refund(&mut w, R).unwrap();
    (w, ds, leaves, root)
}

// ---------------------------------------------------------------------------------------------
// A. Open claim relay

#[test]
fn a_relayed_claim_refunds_the_named_payer_exactly_the_configured_amount() {
    let (mut w, ds, leaves, root) = world();
    let (f0, p0) = (w.float(), w.native(RELAY1));
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    assert_eq!(w.wrapped(ALICE), D100, "the claim minted to the claimer");
    assert_eq!(w.native(RELAY1), p0 + R, "the payer got exactly the refund");
    assert_eq!(w.float(), f0 - R, "the float paid exactly the refund");
    assert_eq!(w.native(ALICE), 0, "the claimer holds no native LEZ");
}

#[test]
fn any_payer_is_refunded_with_no_registration() {
    // Two relays nobody registered, each with its own account; a third party (Dave) too.
    let (mut w, ds, leaves, root) = world();
    let f0 = w.float();
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    w.tx(BOB, BRIDGE, &relayed(RELAY2, claim(ds[2].0, 2, &leaves, root, 0, D100))).unwrap();
    w.tx(CAROL, BRIDGE, &relayed(DAVE, claim(ds[3].0, 3, &leaves, root, 0, D100))).unwrap();
    assert_eq!((w.native(RELAY1), w.native(RELAY2), w.native(DAVE)), (R, R, R));
    assert_eq!(w.float(), f0 - 3 * R);
    let c = w.config();
    assert!([c.admin, c.freezer, c.attestor].iter().all(|a| ![RELAY1, RELAY2, DAVE].contains(a)), "no relay holds a role");
}

#[test]
fn a_failed_claim_refunds_nothing() {
    let (mut w, ds, leaves, root) = world();
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    let before = w.clone();
    // The repeat (nullifier spent), a deposit not in the root, another signer, frozen minting.
    err_is(w.tx(ALICE, BRIDGE, &relayed(RELAY2, claim(ds[0].0, 0, &leaves, root, 0, D100))), CLAIM_REFUSED);
    err_is(w.tx(ALICE, BRIDGE, &relayed(RELAY2, claim([9; 32], 1, &leaves, root, 0, D100))), CLAIM_REFUSED);
    err_is(w.tx(DAVE, BRIDGE, &relayed(RELAY2, claim(ds[1].0, 1, &leaves, root, 0, D100))), CLAIM_REFUSED);
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    let frozen = w.clone();
    err_is(w.tx(ALICE, BRIDGE, &relayed(RELAY2, claim(ds[1].0, 1, &leaves, root, 0, D100))), MINT_FROZEN);
    assert!(w.states == frozen.states);
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    assert_eq!(w.native(RELAY2), 0, "no refused claim refunded anyone");
    assert_eq!(w.float(), before.float(), "the float paid nothing for refused claims");
}

#[test]
fn the_refund_is_the_config_s_amount_never_the_instruction_s() {
    // The instruction carries no amount; the admin's value applies, capped by the program.
    let (mut w, ds, leaves, root) = world();
    err_is(set_refund(&mut w, MAX_RELAY_REFUND + 1), "relay refund above the program's cap");
    err_is(w.tx(DAVE, BRIDGE, &instr(Instruction::Admin(AdminOp::SetRelayRefund { amount: 1 }))), "only the admin");
    set_refund(&mut w, MAX_RELAY_REFUND).unwrap();
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    assert_eq!(w.native(RELAY1), MAX_RELAY_REFUND);
    // Zero turns refunds off: the relayed claim still applies and pays nothing.
    set_refund(&mut w, 0).unwrap();
    let f0 = w.float();
    w.tx(ALICE, BRIDGE, &relayed(RELAY2, claim(ds[1].0, 1, &leaves, root, 0, D100))).unwrap();
    assert_eq!((w.native(RELAY2), w.float(), w.wrapped(ALICE)), (0, f0, 2 * D100));
}

#[test]
fn a_short_float_fails_the_refunded_claim_and_an_unrefunded_one_still_applies() {
    let (mut w, ds, leaves, root) = world();
    let mut empty = setup();
    empty.atomic_asyncs = true;
    std::mem::swap(&mut w, &mut empty);
    // A bridge whose float holds less than the refund.
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    set_refund(&mut w, R).unwrap();
    fund_float(&mut w, R - 1);
    let before = w.clone();
    let e = w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).expect_err("a short float");
    assert!(e.contains("holds less than the transferred amount"), "{e}");
    assert!(w.states == before.states, "nothing minted, the nullifier unspent");
    // The same deposit, no refund named: the float is not touched and the claim applies.
    w.tx(ALICE, BRIDGE, &claim(ds[0].0, 0, &leaves, root, 0, D100)).unwrap();
    assert_eq!((w.wrapped(ALICE), w.float()), (D100, R - 1));
}

#[test]
fn relayed_wraps_only_public_claims() {
    let (mut w, ds, leaves, root) = world();
    // A one-time recipient already draws the sponsor amount: one float payment per deposit.
    let one_time = claim_to(ds[0].0, 0, &leaves, root, 0, D100, Some(recipient_opening(&ds[0].0)));
    err_is(w.tx_with_openings(ALICE, BRIDGE, &relayed(RELAY1, one_time), &[recipient_opening(&ds[0].0)]), RELAYED_ONE_TIME);
    err_is(w.tx(ADMIN, BRIDGE, &relayed(RELAY1, instr(Instruction::Admin(AdminOp::SetRelayRefund { amount: 1 })))), RELAYED_NOT_A_CLAIM);
    err_is(w.tx(ATTESTOR, BRIDGE, &relayed(RELAY1, instr(Instruction::AttestRoot { root: [5; 32] }))), RELAYED_NOT_A_CLAIM);
    let nested = relayed(RELAY1, relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100)));
    err_is(w.tx(ALICE, BRIDGE, &nested), RELAYED_NOT_A_CLAIM);
    assert_eq!(w.native(RELAY1), 0);
}

#[test]
fn a_relay_that_learns_the_secret_cannot_redirect_the_mint_or_burn_the_deposit() {
    // The relay sees Alice's half-signed claim, secret included, before it lands.
    let (mut w, ds, _, _) = world();
    let k = ds[0].0;
    // (1) It claims Alice's deposit as itself: the commitment binds Alice, so it is refused.
    let (leaves, root) = vault(&[(k, ALICE, D100), (k, RELAY1, D100)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    err_is(w.tx(RELAY1, BRIDGE, &relayed(RELAY1, claim(k, 0, &leaves, root, 0, D100))), CLAIM_REFUSED);
    // (2) It made its own cheap deposit with Alice's k (leaf 1) and claims it first: that spends
    // its own nullifier, not Alice's (the nullifier binds the destination, review H2).
    w.tx(RELAY1, BRIDGE, &relayed(RELAY1, claim(k, 1, &leaves, root, 0, D100))).unwrap();
    assert_ne!(claim_nullifier(&k, &RELAY1, &USDC, D100), claim_nullifier(&k, &ALICE, &USDC, D100));
    // Alice's claim, relayed by anyone, still applies and mints to Alice.
    w.tx(ALICE, BRIDGE, &relayed(RELAY2, claim(k, 0, &leaves, root, 0, D100))).unwrap();
    assert_eq!((w.wrapped(ALICE), w.wrapped(RELAY1)), (D100, D100));
    // (3) It cannot change Alice's instruction's payee: the program refunds whom the signed
    // instruction names (the CLI and relay tests check the signature covers it).
    assert_eq!((w.native(RELAY1), w.native(RELAY2)), (R, R));
}

#[test]
fn the_float_pays_at_most_one_refund_per_deposit() {
    // A griefer's own real deposits each draw one refund, once; repeats draw nothing.
    let (mut w, ds, leaves, root) = world();
    let f0 = w.float();
    for (i, (k, who)) in ds.iter().enumerate() {
        // Self-paid claims naming the claimer itself: a rebate, the same amount.
        w.tx(*who, BRIDGE, &relayed(*who, claim(*k, i, &leaves, root, 0, D100))).unwrap();
        for _ in 0..3 {
            assert!(w.tx(*who, BRIDGE, &relayed(*who, claim(*k, i, &leaves, root, 0, D100))).is_err());
        }
    }
    assert_eq!(f0 - w.float(), R * ds.len() as u128, "exactly one refund per deposit");
    assert_eq!(w.config().tokens[0].claims, vec![4, 0]);
}

#[test]
fn every_refund_is_the_same_amount_and_the_same_messages() {
    // The refund is no fingerprint: two claims by different relays carry the same transfer amount.
    let (mut w, ds, leaves, root) = world();
    let amounts = |w: &World| -> Vec<u128> {
        w.trace
            .iter()
            .filter(|(_, to, _, _)| to.program_account_id == NATIVE && to.account_id == config_address(&BRIDGE))
            .filter_map(|(_, _, m, _)| match borsh::from_slice::<native_token::Message>(m) {
                Ok(native_token::Message::Transfer { amount, .. }) => Some(amount),
                _ => None,
            })
            .collect()
    };
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    let a = amounts(&w);
    let n_a = w.trace.len();
    w.tx(BOB, BRIDGE, &relayed(RELAY2, claim(ds[2].0, 2, &leaves, root, 0, D100))).unwrap();
    assert_eq!((a, amounts(&w)), (vec![R], vec![R]));
    assert_eq!(n_a, w.trace.len(), "the same number of messages");
}

#[test]
fn an_attested_claim_can_be_relayed_and_refunded() {
    let mut f = attested_setup();
    f.w.atomic_asyncs = true;
    fund_float(&mut f.w, 10_000_000);
    set_refund(&mut f.w, R).unwrap();
    let i = Instruction::Relayed { refund_to: RELAY1, claim: Box::new(f.claim(0)) };
    f.tx(ALICE, i.clone()).unwrap();
    assert_eq!((f.w.wrapped(ALICE), f.w.native(RELAY1), f.w.float()), (D100, R, 10_000_000 - R));
    err_is(f.tx(ALICE, i), CLAIM_REFUSED);
    assert_eq!(f.w.native(RELAY1), R);
}

#[test]
fn a_relayed_claim_publishes_the_claim_s_allowlist_plus_the_payee() {
    // Past the root call: the three claim messages, the mint, the refund effect, the float's
    // transfer and the credit. The refund names only the payee (already public as fee payer).
    let (mut w, ds, leaves, root) = world();
    w.tx(ALICE, BRIDGE, &relayed(RELAY1, claim(ds[0].0, 0, &leaves, root, 0, D100))).unwrap();
    let refunds: Vec<_> = w
        .trace
        .iter()
        .filter_map(|(_, to, m, _)| match borsh::from_slice::<Message>(m) {
            Ok(Message::Effect(Effect::Refund { to: payee })) if to.account_id == config_address(&BRIDGE) => Some(payee),
            _ => None,
        })
        .collect();
    assert_eq!(refunds, vec![RELAY1]);
    assert!(w.trace.iter().any(|(_, to, _, _)| *to == Actor::native_balance(RELAY1)), "the credit reaches the payer");
}

// ---------------------------------------------------------------------------------------------
// B. Sponsored burn

/// Anyone tops up the burn float (its own PDA, opened by `SetBurnSponsor`) with a plain transfer.
fn fund_burn_float(w: &mut World, amount: u128) {
    let have = w.native(FUNDER);
    w.states.insert((FUNDER, NATIVE), encode_balance(have + amount).into_inner());
    w.tx(FUNDER, NATIVE, &native_token::Message::Transfer { to: burn_float_address(&BRIDGE), amount }).unwrap();
}

fn bfloat(w: &World) -> u128 {
    w.native(burn_float_address(&BRIDGE))
}

/// Alice holds one claimed 100; the burn sponsor is S, the burn float 100M, the claims' float 100M.
fn burn_world() -> World {
    let (mut w, _, _, _) = claimed_one();
    w.atomic_asyncs = true;
    fund_float(&mut w, 100_000_000);
    set_burn_sponsor(&mut w, S).unwrap();
    fund_burn_float(&mut w, 100_000_000);
    w
}

/// The simulator has no private accounts: a fresh private account the burner's wallet witnesses
/// (LEZ admits it by its witness) stands here as an account that exists with nothing in it, and
/// whose actors are presented under an alias (`World::private`), as LEZ presents a private sender.
fn witnessed(w: &mut World, one_time: AccountId) {
    w.states.entry((one_time, NATIVE)).or_default();
    w.private.insert(one_time);
}

fn sponsored_burn(w: &mut World, who: AccountId, one_time: AccountId, amount: u128) -> Result<(), String> {
    witnessed(w, one_time);
    let notify = Actor::new(one_time, BRIDGE);
    w.tx(who, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount, notify, memo: [4; 32] })
}

#[test]
fn a_burn_through_a_one_time_actor_pays_the_burn_sponsor_to_it() {
    let mut w = burn_world();
    let (f0, b0, alice_native) = (w.float(), bfloat(&w), w.native(ALICE));
    sponsored_burn(&mut w, ALICE, W1, D100).unwrap();
    assert_eq!(w.wrapped(ALICE), 0, "one denomination destroyed");
    assert_eq!(w.native(W1), S, "the one-time account got exactly the sponsor");
    assert_eq!((bfloat(&w), w.float()), (b0 - S, f0), "paid from the burn float; the claims' float untouched");
    assert_eq!(w.config().sponsored_burns, 1);
    assert_eq!(w.native(ALICE), alice_native, "the burner's own native LEZ is not touched");
    let t = &w.config().tokens[0];
    assert_eq!((t.burns.clone(), t.burned_value), (vec![1, 0], D100), "recorded like a direct burn");
    let ex: Vec<Exited> = w.emitted.iter().filter(|(a, _)| a.account_id == config_address(&BRIDGE)).map(|(_, e)| borsh::from_slice(&e.data).unwrap()).collect();
    assert_eq!(ex, vec![Exited { eth_token: USDC, denom_index: 0, exit_commitment: [4; 32] }]);
    // Nothing past the root call names the burner.
    assert_never_named(&w, ALICE);
}

#[test]
fn a_sponsored_burn_is_bounded_like_any_burn() {
    let mut w = burn_world();
    sponsored_burn(&mut w, ALICE, W1, D100).unwrap();
    // Burns above claims: Alice holds nothing more; a holder minted outside the bridge cannot
    // reach `burned <= claimed` either (the notice path below refuses it).
    let f0 = bfloat(&w);
    assert!(sponsored_burn(&mut w, ALICE, W2, D100).is_err());
    // Not a denomination; frozen redemption.
    let (mut w2, ks, leaves, root) = claimed_one();
    w2.atomic_asyncs = true;
    set_burn_sponsor(&mut w2, S).unwrap();
    fund_burn_float(&mut w2, 100_000_000);
    w2.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, D100)).unwrap();
    err_contains(sponsored_burn(&mut w2, ALICE, W1, 50_000_000), "a burn is exactly one denomination");
    w2.tx(FREEZER, BRIDGE, &freeze(None, false, true)).unwrap();
    err_contains(sponsored_burn(&mut w2, ALICE, W1, D100), BURN_FROZEN);
    assert_eq!((w2.native(W1), w2.native(W2), w.native(W2)), (0, 0, 0));
    assert_eq!(bfloat(&w), f0);
}

#[test]
fn a_notice_from_another_program_pays_nothing() {
    // A program that is not the bridge's token program sends notice-shaped bytes to a bridge actor.
    let mut w = burn_world();
    witnessed(&mut w, W1);
    let f0 = bfloat(&w);
    let snapshot = w.clone();
    let mut asyncs = VecDeque::new();
    let forged = BurnNotice { definition: DEF, amount: D100, memo: [6; 32] };
    let r = w.deliver(Some(Actor::new(DEF, YES)), Actor::new(W1, BRIDGE), borsh::to_vec(&forged).unwrap(), BTreeSet::new(), None, &mut asyncs);
    err_contains(r, "a burn notice comes from the token program");
    w = snapshot;
    assert_eq!((bfloat(&w), w.native(W1)), (f0, 0));
    // And nobody can send the effect itself: effects come only from the bridge's own actors.
    let e = Effect::SponsoredBurn { token_program: TOKEN, notice: forged, to: W1 };
    err_contains(w.tx(DAVE, BRIDGE, &Message::Effect(e)), "only this program may send an effect");
}

#[test]
fn a_direct_burn_is_unchanged_and_unsponsored() {
    let mut w = burn_world();
    let (f0, b0) = (w.float(), bfloat(&w));
    burn(&mut w, ALICE).unwrap();
    assert_eq!((w.float(), bfloat(&w), w.wrapped(ALICE)), (f0, b0, 0));
}

#[test]
fn the_burn_sponsor_is_capped_and_zero_pays_nothing() {
    let mut w = burn_world();
    err_is(set_burn_sponsor(&mut w, MAX_BURN_SPONSOR + 1), "burn sponsor above the program's cap");
    set_burn_sponsor(&mut w, 0).unwrap();
    let f0 = bfloat(&w);
    sponsored_burn(&mut w, ALICE, W1, D100).unwrap();
    assert_eq!((bfloat(&w), w.native(W1), w.config().tokens[0].burned_value), (f0, 0, D100));
}

#[test]
fn a_short_float_fails_the_sponsored_burn_whole() {
    let (mut w, _, _, _) = claimed_one();
    w.atomic_asyncs = true;
    set_burn_sponsor(&mut w, S).unwrap();
    fund_burn_float(&mut w, S - 1);
    fund_float(&mut w, 100 * S); // the claims' float is not drawn on
    witnessed(&mut w, W1);
    let before = w.clone();
    err_contains(sponsored_burn(&mut w, ALICE, W1, D100), "holds less than the transferred amount");
    assert!(w.states == before.states, "the tokens are not burned and no exit is recorded");
}

#[test]
fn every_sponsored_burn_draws_once_and_conservation_holds() {
    // Ten 100s claimed by Alice; each burn through its own one-time account draws S once.
    let mut w = setup();
    w.atomic_asyncs = true;
    let ks: Vec<[u8; 32]> = (0..10u8).map(|i| [i + 10; 32]).collect();
    let (leaves, root) = vault(&ks.iter().map(|k| (*k, ALICE, D100)).collect::<Vec<_>>());
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    for (i, k) in ks.iter().enumerate() {
        w.tx(ALICE, BRIDGE, &claim(*k, i, &leaves, root, 0, D100)).unwrap();
    }
    set_burn_sponsor(&mut w, S).unwrap();
    fund_burn_float(&mut w, 1_000_000_000);
    let f0 = bfloat(&w);
    for i in 0..10u8 {
        sponsored_burn(&mut w, ALICE, AccountId::new([0x60 + i; 32]), D100).unwrap();
    }
    assert!(sponsored_burn(&mut w, ALICE, AccountId::new([0x5F; 32]), D100).is_err(), "nothing left to burn");
    let t = &w.config().tokens[0];
    assert_eq!((t.burned_value, t.claimed_value, w.supply()), (10 * D100, 10 * D100, 0));
    assert_eq!(f0 - bfloat(&w), 10 * S, "one sponsor per claim (ten claims, ten burns)");
    assert_eq!(w.config().sponsored_burns, 10);
}

#[test]
fn a_public_forwarder_collects_no_sponsor() {
    // A public burner names its own bridge actor, or another public account's, as `notify`:
    // the forwarder is presented under its own id, so the config refuses the sponsor (and the
    // burn with it). A public burner pays a public fee; the sponsor is for a private fee.
    let mut w = burn_world();
    w.states.entry((W1, NATIVE)).or_default(); // exists, public (not in `private`)
    let before = w.clone();
    let notify = Actor::new(ALICE, BRIDGE);
    err_contains(w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify, memo: [4; 32] }), SPONSORED_BURN_PRIVATE);
    let notify = Actor::new(W1, BRIDGE);
    err_contains(w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify, memo: [4; 32] }), SPONSORED_BURN_PRIVATE);
    assert!(w.states == before.states, "nothing burned, nothing paid");
    // The same burn through a private one-time account applies. A public burner naming a
    // private one-time account is paid too: that W pays the private fee.
    sponsored_burn(&mut w, ALICE, W2, D100).unwrap();
    assert_eq!(w.native(W2), S);
}

#[test]
fn one_large_claim_split_into_small_burns_draws_one_sponsor() {
    // The farm the review found: claim one 1000, burn it as ten 100s, each sponsored. Now the
    // second sponsored burn is refused (one per claim); direct burns of the rest still work.
    let mut w = setup();
    w.atomic_asyncs = true;
    let k = [0x31; 32];
    let (leaves, root) = vault(&[(k, ALICE, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, 1_000_000_000)).unwrap();
    set_burn_sponsor(&mut w, S).unwrap();
    fund_burn_float(&mut w, 100 * S);
    let b0 = bfloat(&w);
    sponsored_burn(&mut w, ALICE, W1, D100).unwrap();
    let before = w.clone();
    err_contains(sponsored_burn(&mut w, ALICE, W2, D100), NO_BURN_SPONSOR_LEFT);
    witnessed(&mut w, W2);
    assert!(w.states.iter().filter(|(k, _)| k.0 != W2).eq(before.states.iter().filter(|(k, _)| k.0 != W2)), "the refused burn changed nothing");
    for _ in 0..9 {
        burn(&mut w, ALICE).unwrap();
    }
    assert_eq!((b0 - bfloat(&w), w.wrapped(ALICE), w.config().sponsored_burns), (S, 0, 1));
}

#[test]
fn the_claims_float_and_the_burn_float_cannot_drain_each_other() {
    // Claims' float empty: a sponsored burn is still paid (from the burn float).
    let mut w = burn_world();
    let cfg = config_address(&BRIDGE);
    let drained = w.native(cfg);
    w.states.insert((cfg, NATIVE), encode_balance(0).into_inner());
    assert!(drained > 0);
    sponsored_burn(&mut w, ALICE, W1, D100).unwrap();
    assert_eq!(w.native(W1), S);
    // Burn float empty: a sponsored one-time claim is still paid (from the claims' float).
    let (mut w, ks, leaves, root) = claimed_one();
    w.atomic_asyncs = true;
    set_sponsor(&mut w, 25_000_000);
    set_burn_sponsor(&mut w, S).unwrap();
    fund_float(&mut w, 25_000_000);
    assert_eq!(bfloat(&w), 0);
    let o = recipient_opening(&ks[0]);
    w.tx_with_openings(ALICE, BRIDGE, &claim_to(ks[0], 1, &leaves, root, 0, D100, Some(o)), &[o]).unwrap();
    assert_eq!(w.native(ALICE.blinded(&o)) + w.native(ALICE), 25_000_000);
    // And a sponsored burn now fails whole (empty burn float), the claims' float untouched.
    let f = w.float();
    err_contains(sponsored_burn(&mut w, ALICE, W1, D100), "holds less than the transferred amount");
    assert_eq!(w.float(), f);
}
