//! Whole-transaction tests of the bridge's RFP-021 path: a claim proves the vault's
//! `isDepositRoot[root]` entry under an execution anchor finalized by an attestation instance.
//!
//! The vault's Ethereum state is synthetic (a state trie built with alloy-trie, so every proof is
//! a real Merkle-Patricia proof). The attestation instance is the real `eth_attest` program, but
//! its config and finalized anchors are written straight into the simulator's state: that is the
//! test-only way to stand up an instance at a chosen anchor; the program has no such code path.
//!
//! The simulator and the fixture are in `common/mod.rs` (seed grants, admission of new bridge
//! PDAs, aliases from a transaction's openings and LEZ's native token code; see its docs for
//! what it does not model: private accounts, sender presentation, promotion, fees).

mod common;

use common::*;
use eth_attest_core::inclusion::InclusionClass;

/// A claim refusal: on chain the one text [`CLAIM_REFUSED`]; natively (what the CLI shows its
/// user) the expected cause. `None`: the witness is fine and public state refused it.
#[track_caller]
fn refused_as(r: Result<(), String>, signer: AccountId, i: &Instruction, cause: Option<ClaimFault>) {
    err_is(r, CLAIM_REFUSED);
    assert_eq!(witness_fault(signer, i), cause);
}

/// [`refused_as`] for an inclusion fault of `class`.
#[track_caller]
fn refused_by_proof(r: Result<(), String>, signer: AccountId, i: &Instruction, class: InclusionClass) {
    err_is(r, CLAIM_REFUSED);
    assert_eq!(inclusion_class(signer, i), Some(class));
}

// ---------------------------------------------------------------------------------------------

#[test]
fn init_attested_binds_the_source_and_checks_the_instance() {
    let f = attested_setup();
    let c = f.w.config();
    assert_eq!(c.root_source, RootSource::Attest(source(f.id)));
    assert_eq!(VAULT_DEPOSIT_ROOT_SLOT, 83);
    // The instance must exist, for this chain.
    let mut w = World::default();
    err_contains(w.tx(BRIDGE, BRIDGE, &init_attested(source([9; 32]), EVM_CHAIN)), "no such instance");
    let id = plant_instance(&mut w, &instance_config(1));
    err_contains(w.tx(BRIDGE, BRIDGE, &init_attested(source(id), EVM_CHAIN)), "instance is for another chain");
    assert!(w.state(config_address(&BRIDGE), BRIDGE).is_empty());
    // No admin op changes the source; the stand-in attestor op leaves it alone.
    let mut f = attested_setup();
    f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetAttestor { attestor: ATTESTOR }))).unwrap();
    assert_eq!(f.w.config().root_source, RootSource::Attest(source(f.id)));
}

#[test]
fn a_claim_through_the_attestation_mints_once() {
    let mut f = attested_setup();
    f.tx(ALICE, f.claim(0)).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
    refused_as(f.tx(ALICE, f.claim(0)), ALICE, &f.claim(0), None);
    // Bob claims against the block-hash anchor (header supplied) instead.
    let (header, block_anchor) = (f.header.clone(), f.block_anchor);
    let bob = with(f.claim(1), |x| *x.anchor = AnchorWitness { anchor: block_anchor, header_rlp: Some(header) });
    f.tx(BOB, bob).unwrap();
    assert_eq!(f.w.wrapped(BOB), 1_000_000_000);
    assert_eq!(f.w.config().tokens[0].claims, vec![1, 1]);
    // Wrapped tokens burn for exit as with the stand-in.
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    f.w.tx(BOB, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: 100_000_000, notify, memo: [3; 32] }).unwrap();
    assert_eq!(f.w.config().tokens[0].burns, vec![1, 0]);
}

#[test]
fn what_an_attested_claim_publishes() {
    let mut f = attested_setup();
    f.tx(ALICE, f.claim(0)).unwrap();
    // Messages that leave the claimer's actor: the anchor check, the nullifier, the claim.
    let claimer = Actor::new(ALICE, BRIDGE);
    let out: Vec<_> = f.w.trace.iter().filter(|(from, _, _, _)| *from == Some(claimer)).collect();
    let targets: Vec<AccountId> = out.iter().map(|(_, to, _, _)| to.account_id).collect();
    assert_eq!(
        targets,
        vec![
            anchor_address(&ATTEST, &f.id, &f.state_anchor),
            nullifier_address(&BRIDGE, &claim_nullifier(&f.keys[0], &ALICE, &USDC, 100_000_000)),
            config_address(&BRIDGE),
        ]
    );
    // Neither the deposit root, nor the vault slot, nor the leaf is in any of them.
    let slot = is_deposit_root(&f.root);
    let leaf = f.leaves[0];
    for (_, _, m, _) in &out {
        for secret in [&f.root, &slot, &leaf] {
            assert!(!m.windows(32).any(|w| w == secret.as_slice()), "a message leaks a private value");
        }
    }
}

#[test]
fn an_unattested_or_false_root_is_refused() {
    let mut f = attested_setup();
    // Deposits under a root the vault never recorded (absent entry, i.e. false).
    let k = [3u8; 32];
    let (leaves, root) = deposits(&[(k, ALICE, 100_000_000)]);
    let mk = |proof_value: Option<[u8; 32]>| {
        let mut proof = f.state.storage_proof(&VAULT, &is_deposit_root(&root));
        if let Some(v) = proof_value {
            proof.value = v;
        }
        Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: 0,
            denom: 100_000_000,
            root,
            leaf_index: 0,
            path: merkle_path(&leaves, 0),
            secret: k,
            cfg_id: acfg(),
            source: source(f.id),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor: AnchorWitness { anchor: f.state_anchor, header_rlp: None },
            proof,
            recipient_opening: None,
        }
    };
    let (honest, liar) = (mk(None), mk(Some(word(1))));
    // Honest exclusion proof: the entry is false.
    refused_by_proof(f.tx(ALICE, honest.clone()), ALICE, &honest, InclusionClass::Predicate);
    // Claiming it true anyway.
    refused_by_proof(f.tx(ALICE, liar.clone()), ALICE, &liar, InclusionClass::StorageProof);
    // A value other than true (a non-bool word in the slot) is refused too.
    let mut g = attested_setup();
    g.state.accounts[0].storage[1].1 = word(2);
    let anchor = Anchor::StateRoot(g.state.state_root());
    plant_anchor(&mut g.w, &g.id, EVM_CHAIN, anchor);
    let i = with(g.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
    });
    refused_by_proof(g.tx(ALICE, i.clone()), ALICE, &i, InclusionClass::Predicate);
    assert_eq!(f.w.wrapped(ALICE) + g.w.wrapped(ALICE), 0);
}

#[test]
fn an_anchor_the_instance_did_not_finalize_is_refused() {
    let mut f = attested_setup();
    // The same vault state with one more entry: a valid trie, but its root was never finalized.
    let mut later = f.state.clone();
    later.accounts[2] = SynthAccount::new([0x21; 20]).with(word(1), word(1));
    let anchor = Anchor::StateRoot(later.state_root());
    let i = with(f.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
        *x.proof = later.storage_proof(&VAULT, &is_deposit_root(&f.root));
    });
    err_contains(f.tx(ALICE, i), "anchor not finalized by this instance");
    // An anchor finalized by another instance is no better.
    let other = plant_instance(&mut f.w, &InstanceConfig { salt: [1; 32], ..instance_config(EVM_CHAIN) });
    plant_anchor(&mut f.w, &other, EVM_CHAIN, anchor);
    let i = with(f.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
        *x.proof = later.storage_proof(&VAULT, &is_deposit_root(&f.root));
    });
    err_contains(f.tx(ALICE, i), "anchor not finalized by this instance");
    // A tampered header for the block-hash anchor.
    let mut header = f.header.clone();
    header[40] ^= 1;
    let block_anchor = f.block_anchor;
    let i = with(f.claim(0), |x| *x.anchor = AnchorWitness { anchor: block_anchor, header_rlp: Some(header) });
    refused_by_proof(f.tx(ALICE, i.clone()), ALICE, &i, InclusionClass::Anchor);
    assert_eq!(f.w.wrapped(ALICE), 0);
}

#[test]
fn a_wrong_vault_slot_index_or_source_is_refused() {
    let mut f = attested_setup();
    let root = f.root;
    // The entry proven in another contract that has it set: the config's vault differs.
    let other_proof = f.state.storage_proof(&OTHER, &is_deposit_root(&root));
    let i = with(f.claim(0), |x| {
        *x.vault = OTHER;
        *x.proof = other_proof.clone();
    });
    err_contains(f.tx(ALICE, i), "claim for another attestation source");
    // The other contract's proof presented as the vault's.
    let i = with(f.claim(0), |x| *x.proof = other_proof);
    refused_by_proof(f.tx(ALICE, i.clone()), ALICE, &i, InclusionClass::AccountProof);
    // Wrong slot index: the vault has a true word at keccak(root, 84) in this state...
    let mut g = attested_setup();
    g.state.accounts[0].storage.push((mapping_slot(&root, 84), word(1)));
    let anchor = Anchor::StateRoot(g.state.state_root());
    plant_anchor(&mut g.w, &g.id, EVM_CHAIN, anchor);
    let proof84 = g.state.storage_proof(&VAULT, &mapping_slot(&root, 84));
    let i = with(g.claim(0), |x| {
        x.source.deposit_root_slot = 84;
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
        *x.proof = proof84.clone();
    });
    // ...the claimer's own check passes, and the config refuses the slot index.
    err_contains(g.tx(ALICE, i), "claim for another attestation source");
    // With the configured index, that proof is for the wrong slot.
    let i = with(g.claim(0), |x| {
        *x.anchor = AnchorWitness { anchor, header_rlp: None };
        *x.proof = proof84;
    });
    refused_by_proof(g.tx(ALICE, i.clone()), ALICE, &i, InclusionClass::StorageProof);
    // A claim aimed at a program that agrees to every anchor, or at another instance: refused
    // by the config, so the "yes" never counts.
    let i = with(f.claim(0), |x| x.source.attest_program = YES);
    err_contains(f.tx(ALICE, i), "claim for another attestation source");
    let other = plant_instance(&mut f.w, &InstanceConfig { salt: [1; 32], ..instance_config(EVM_CHAIN) });
    plant_anchor(&mut f.w, &other, EVM_CHAIN, f.state_anchor);
    let i = with(f.claim(0), |x| x.source.attest_instance = other);
    err_contains(f.tx(ALICE, i), "claim for another attestation source");
    // Another chain id.
    let i = with(f.claim(0), |x| *x.evm_chain_id = 1);
    err_contains(f.tx(ALICE, i), "anchor is for another chain");
    assert_eq!(f.w.wrapped(ALICE) + g.w.wrapped(ALICE), 0);
    // The honest claim still works after all that.
    f.tx(ALICE, f.claim(0)).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
}

#[test]
fn the_deposit_must_be_in_the_attested_root_and_bound_to_its_claimer() {
    let mut f = attested_setup();
    // Bob replays Alice's secret: the commitment names Alice.
    refused_as(f.tx(BOB, f.claim(0)), BOB, &f.claim(0), Some(ClaimFault::NotInRoot));
    // A root the vault marked true but which doesn't hold the deposit.
    let (_, older) = deposits(&[(f.keys[0], ALICE, 100_000_000)]);
    let older_proof = f.state.storage_proof(&VAULT, &is_deposit_root(&older));
    let i = with(f.claim(1), |x| {
        *x.root = older;
        *x.proof = older_proof;
    });
    refused_as(f.tx(BOB, i.clone()), BOB, &i, Some(ClaimFault::NotInRoot));
}

#[test]
fn the_sources_do_not_mix() {
    // An attested config refuses the stand-in root recording and the stand-in claim.
    let mut f = attested_setup();
    f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetAttestor { attestor: ATTESTOR }))).unwrap();
    err_contains(f.w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: f.root })), "not the stand-in");
    // Even with the root PDA somehow set, the stand-in claim is refused by the config.
    f.w.states.insert((root_address(&BRIDGE, &f.root), BRIDGE), vec![1]);
    let stand_in = instr(Instruction::Claim {
        eth_token: USDC,
        denom_index: 0,
        denom: 100_000_000,
        root: f.root,
        leaf_index: 0,
        path: merkle_path(&f.leaves, 0),
        secret: f.keys[0],
        cfg_id: acfg(),
        recipient_opening: None,
    });
    err_contains(f.w.tx(ALICE, BRIDGE, &stand_in), "not the stand-in");
    // A stand-in config refuses an attested claim, whatever it proves.
    let mut s = attested_setup();
    s.w.states.remove(&(config_address(&BRIDGE), BRIDGE));
    s.w.tx(BRIDGE, BRIDGE, &init_with(VAULT, FREEZER, ATTESTOR)).unwrap();
    s.w.tx(
        ADMIN,
        BRIDGE,
        &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: USDC, definition: DEF, decimals: 6, denoms: vec![100_000_000, 1_000_000_000] })),
    )
    .unwrap();
    assert_eq!(s.w.config().root_source, RootSource::StandIn);
    // The root source is bound into the cfg id (zoneId = source_id), so the stand-in config's cfg
    // id already differs from the attested claim's.
    assert_ne!(s.w.config().cfg_id, acfg());
    err_contains(s.tx(ALICE, s.claim(0)), "claim for another deployment");
}

#[test]
fn freezing_stops_attested_claims() {
    let mut f = attested_setup();
    f.w.tx(FREEZER, BRIDGE, &instr(Instruction::Admin(AdminOp::Freeze { eth_token: None, mint: true, burn: false }))).unwrap();
    err_contains(f.tx(ALICE, f.claim(0)), "minting is frozen");
    f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::Unfreeze { eth_token: None, mint: true, burn: false }))).unwrap();
    f.tx(ALICE, f.claim(0)).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
}

// ---------------------------------------------------------------------------------------------
// One-time recipient and float sponsoring on the RFP-021 path.

#[test]
fn an_attested_claim_with_an_opening_mints_to_the_one_time_address_only() {
    let mut f = attested_setup();
    let (i, o) = f.claim_one_time(0);
    let a = ALICE.blinded(&o);
    // Bob, with Alice's secret and opening: the commitment binds Alice.
    refused_as(f.w.tx_with_openings(BOB, BRIDGE, &instr(i.clone()), &[o]), BOB, &i, Some(ClaimFault::NotInRoot));
    f.w.tx_with_openings(ALICE, BRIDGE, &instr(i), &[o]).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
    assert!(f.w.state(a, TOKEN).is_empty());
    // Every message past the claimer's own instruction names A, never Alice.
    let config = config_address(&BRIDGE);
    let effect = f.w.trace.iter().find_map(|(_, to, m, _)| match borsh::from_slice::<Message>(m) {
        Ok(Message::Effect(Effect::ClaimAttested { to: recipient, one_time, .. })) if to.account_id == config => {
            Some((recipient, one_time))
        }
        _ => None,
    });
    assert_eq!(effect, Some((a, true)));
    assert!(f.w.trace.iter().any(|(_, to, _, _)| *to == Actor::new(a, TOKEN)));
    assert!(f.w.trace[0].0.is_none());
    for (_, to, m, _) in &f.w.trace[1..] {
        assert_ne!(to.account_id, ALICE);
        assert!(!m.windows(32).any(|x| x == ALICE.value()), "a message carries the real account");
    }
    // Without an opening the attested claim still mints to the signer.
    f.tx(BOB, f.claim(1)).unwrap();
    assert_eq!(f.w.wrapped(BOB), 1_000_000_000);
}

#[test]
fn the_float_sponsors_an_attested_one_time_claim_once() {
    let mut f = attested_setup();
    f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: 7 }))).unwrap();
    f.w.states.insert((FUNDER, NATIVE), encode_balance(10).into_inner());
    f.w.tx(FUNDER, NATIVE, &native_token::Message::Transfer { to: config_address(&BRIDGE), amount: 10 }).unwrap();
    assert_eq!(f.w.float(), 10);
    let (i, o) = f.claim_one_time(0);
    f.w.tx_with_openings(ALICE, BRIDGE, &instr(i.clone()), &[o]).unwrap();
    assert_eq!((f.w.float(), f.w.native(ALICE), f.w.wrapped(ALICE)), (3, 7, 100_000_000));
    // Double claim: refused, nothing paid.
    refused_as(f.w.tx_with_openings(ALICE, BRIDGE, &instr(i.clone()), &[o]), ALICE, &i, None);
    assert_eq!((f.w.float(), f.w.native(ALICE)), (3, 7));
    // Bob's one-time claim finds 3 < 7 in the float: refused cleanly, nullifier unspent.
    let (j, ob) = f.claim_one_time(1);
    err_contains(f.w.tx_with_openings(BOB, BRIDGE, &instr(j.clone()), &[ob]), "holds less than the transferred amount");
    assert_eq!((f.w.float(), f.w.native(BOB), f.w.wrapped(BOB)), (3, 0, 0));
    assert!(f.w.state(nullifier_address(&BRIDGE, &claim_nullifier(&f.keys[1], &BOB, &USDC, 1_000_000_000)), BRIDGE).is_empty());
    assert_eq!(f.w.config().tokens[0].claims, vec![1, 0]);
    // Topped up, it goes through.
    f.w.states.insert((FUNDER, NATIVE), encode_balance(4).into_inner());
    f.w.tx(FUNDER, NATIVE, &native_token::Message::Transfer { to: config_address(&BRIDGE), amount: 4 }).unwrap();
    f.w.tx_with_openings(BOB, BRIDGE, &instr(j), &[ob]).unwrap();
    assert_eq!((f.w.float(), f.w.native(BOB), f.w.wrapped(BOB)), (0, 7, 1_000_000_000));
}
