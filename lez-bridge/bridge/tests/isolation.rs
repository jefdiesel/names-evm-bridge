//! Isolation between bridge deployments (audit B6: RFP-022 Functionality #15, #16): two bridge
//! programs (different program ids, so different cfg ids, configs and PDAs) with their own
//! vaults, admins and wrapped definitions in one simulated world (`common/mod.rs`), plus two
//! attestation instances and a second attestation program id for the RFP-021 path.

mod common;

use common::*;

const D100: u128 = 100_000_000;

fn cfg2() -> [u8; 32] {
    cfg_id(EVM_CHAIN, &VAULT2, &LEZ_CHAIN, &source_id(&RootSource::StandIn), &BRIDGE2)
}

/// Bridge 2's deployment id in the attested pair (instance `y`, vault 2).
fn acfg2(y: [u8; 32]) -> [u8; 32] {
    let src = AttestSource { attest_program: ATTEST, attest_instance: y, deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT };
    cfg_id(EVM_CHAIN, &VAULT2, &LEZ_CHAIN, &source_id(&RootSource::Attest(src)), &BRIDGE2)
}

/// `r` is a refusal containing `want` and the world is exactly `before`.
#[track_caller]
fn refused(w: &World, before: &World, r: Result<(), String>, want: &str) {
    let e = r.expect_err("expected a refusal");
    assert!(e.contains(want), "wanted '{want}', got: {e}");
    assert!(w.states == before.states, "a refused attempt changed some state ({want})");
}

fn admin(op: AdminOp) -> Message {
    instr(Instruction::Admin(op))
}

/// `setup()`'s bridge (BRIDGE: VAULT, ADMIN, FREEZER, ATTESTOR, USDC -> DEF) plus a second,
/// independent stand-in deployment (BRIDGE2: VAULT2, ADMIN2, FREEZER2, ATTESTOR2, USDC -> DEF2).
fn two_bridges() -> World {
    let mut w = setup();
    w.programs.insert(BRIDGE2, Prog::Bridge);
    w.tx(DEF2, TOKEN, &Tok::NewFungibleDefinition { name: "Wrapped USDC (2)".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE2)), supply_to: ADMIN2 })
        .unwrap();
    w.tx(
        BRIDGE2,
        BRIDGE2,
        &instr(Instruction::Initialize {
            admin: ADMIN2,
            vault: VAULT2,
            evm_chain_id: EVM_CHAIN,
            lez_chain_id: LEZ_CHAIN,
            token_program: TOKEN,
            freezer: FREEZER2,
            attestor: ATTESTOR2,
        }),
    )
    .unwrap();
    w.tx(ADMIN2, BRIDGE2, &admin(AdminOp::RegisterToken { eth_token: USDC, definition: DEF2, decimals: 6, denoms: vec![D100, 1_000_000_000] }))
        .unwrap();
    assert_ne!(cfg(), cfg2());
    assert_eq!(w.config_of(BRIDGE2).cfg_id, cfg2());
    w
}

#[test]
fn a_deposit_for_one_deployment_is_refused_by_the_other() {
    let mut w = two_bridges();
    let k = secret("b6", 1);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("b6", 2), &BOB, &USDC, D100)];
    let root = root_of(&leaves);
    // Roots are public: both attestors record it (the worst case for bridge 2).
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ATTESTOR2, BRIDGE2, &instr(Instruction::AttestRoot { root })).unwrap();
    let before = w.clone();
    // Bridge 2 with bridge 1's cfg id: the leaf matches, the config refuses.
    let r = w.tx(ALICE, BRIDGE2, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None));
    refused(&w, &before, r, "claim for another deployment");
    let o = recipient_opening(&k);
    let r = w.tx_with_openings(ALICE, BRIDGE2, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, Some(o)), &[o]);
    refused(&w, &before, r, "claim for another deployment");
    // With its own cfg id, the leaf is not the vault's.
    let m = claim_msg(cfg2(), USDC, k, 0, &leaves, 0, D100, None);
    let r = w.tx(ALICE, BRIDGE2, &m);
    refused(&w, &before, r, CLAIM_REFUSED);
    assert_eq!(message_fault(ALICE, &m), Some(ClaimFault::NotInRoot));
    // Bridge 1 takes it; bridge 2 still refuses it and has minted nothing.
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None)).unwrap();
    assert_eq!((w.bal(ALICE, DEF), w.bal(ALICE, DEF2)), (D100, 0));
    let before = w.clone();
    let r = w.tx(ALICE, BRIDGE2, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None));
    refused(&w, &before, r, "claim for another deployment");
    assert_eq!((w.supply_of(DEF2), w.config_of(BRIDGE2).tokens[0].claims.clone()), (0, vec![0, 0]));
    // Nullifier sets are per deployment: a bridge-2 deposit with the same secret is claimable
    // there (each deposit is bound to its own cfg id).
    let leaves2 = vec![leaf_for(&cfg2(), &k, &ALICE, &USDC, D100), leaf_for(&cfg2(), &secret("b6", 3), &BOB, &USDC, D100)];
    w.tx(ATTESTOR2, BRIDGE2, &instr(Instruction::AttestRoot { root: root_of(&leaves2) })).unwrap();
    w.tx(ALICE, BRIDGE2, &claim_msg(cfg2(), USDC, k, 0, &leaves2, 0, D100, None)).unwrap();
    assert_eq!((w.bal(ALICE, DEF), w.bal(ALICE, DEF2)), (D100, D100));
    let nf = claim_nullifier(&k, &ALICE, &USDC, D100);
    assert_ne!(nullifier_address(&BRIDGE, &nf), nullifier_address(&BRIDGE2, &nf));
}

#[test]
fn one_deployment_s_roles_cannot_act_on_the_other() {
    let mut w = two_bridges();
    let before = w.clone();
    for (a, b, admin_a, freezer_a, attestor_a, admin_b) in
        [(BRIDGE, BRIDGE2, ADMIN, FREEZER, ATTESTOR, ADMIN2), (BRIDGE2, BRIDGE, ADMIN2, FREEZER2, ATTESTOR2, ADMIN)]
    {
        // a's admin on b's config.
        for op in [
            AdminOp::SetSponsor { amount: 1 },
            AdminOp::RegisterToken { eth_token: DAI, definition: DEF, decimals: 18, denoms: vec![1] },
            AdminOp::AddDenomination { eth_token: USDC, denom: 7 },
            AdminOp::DeregisterToken { eth_token: USDC },
            AdminOp::Unfreeze { eth_token: None, mint: true, burn: true },
            AdminOp::SetFreezer { freezer: admin_a },
            AdminOp::SetAttestor { attestor: admin_a },
            AdminOp::ProposeAdmin { next: admin_a },
        ] {
            let r = w.tx(admin_a, b, &admin(op));
            refused(&w, &before, r, "only the admin");
        }
        // a's freezer, and a's admin, cannot freeze b.
        for who in [freezer_a, admin_a] {
            let r = w.tx(who, b, &freeze(None, true, true));
            refused(&w, &before, r, "only the freezer or admin");
        }
        // a's attestor cannot record b's roots.
        let r = w.tx(attestor_a, b, &instr(Instruction::AttestRoot { root: [0x33; 32] }));
        refused(&w, &before, r, "only the attestor");
        // A handover proposed on b to someone else cannot be accepted by a's admin.
        let mut x = w.clone();
        x.tx(admin_b, b, &admin(AdminOp::ProposeAdmin { next: CAROL })).unwrap();
        let proposed = x.clone();
        let r = x.tx(admin_a, b, &admin(AdminOp::AcceptAdmin));
        refused(&x, &proposed, r, "only the proposed admin accepts");
        let _ = a;
    }
}

#[test]
fn one_bridge_cannot_mint_the_other_s_wrapped_token() {
    let mut w = two_bridges();
    // Bridge 1's admin registers bridge 2's definition as a token of bridge 1.
    w.tx(ADMIN, BRIDGE, &admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEF2, decimals: 18, denoms: vec![100] })).unwrap();
    let k = secret("b6", 10);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &DAI, 100), leaf_for(&cfg(), &secret("b6", 11), &BOB, &USDC, D100)];
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: root_of(&leaves) })).unwrap();
    let before = w.clone();
    let r = w.tx(ALICE, BRIDGE, &claim_msg(cfg(), DAI, k, 0, &leaves, 0, 100, None));
    refused(&w, &before, r, "only the mint authority may mint");
    assert!(w.state(nullifier_address(&BRIDGE, &claim_nullifier(&k, &ALICE, &DAI, 100)), BRIDGE).is_empty());
    assert_eq!(w.supply_of(DEF2), 0);
    // A bridge-1 actor minting bridge 2's definition directly.
    let mut asyncs = VecDeque::new();
    let mint = borsh::to_vec(&Tok::MintBy { to: ALICE, amount: 1 }).unwrap();
    let r = w.deliver(Some(Actor::new(config_address(&BRIDGE), BRIDGE)), Actor::new(DEF2, TOKEN), mint, BTreeSet::new(), None, &mut asyncs);
    assert!(r.unwrap_err().contains("only the mint authority may mint"));
    let mut w = before.clone();
    // Bridge 1's config forging a claim effect at bridge 2's config: another program's message
    // there can only be a burn notice, and this one is not.
    let forged = borsh::to_vec(&Message::Effect(Effect::Claim { eth_token: USDC, denom_index: 0, denom: D100, to: ALICE, cfg_id: cfg2(), one_time: false }))
        .unwrap();
    let mut asyncs = VecDeque::new();
    let r = w.deliver(
        Some(Actor::new(config_address(&BRIDGE), BRIDGE)),
        Actor::new(config_address(&BRIDGE2), BRIDGE2),
        forged,
        BTreeSet::from([config_seed()]),
        None,
        &mut asyncs,
    );
    assert!(r.unwrap_err().contains("burn notice"));
    assert_eq!((w.supply_of(DEF2), w.bal(ALICE, DEF2)), (0, 0));
    // A burn of bridge 1's token reported to bridge 2: not bridge 2's token.
    let mut w = two_bridges();
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &USDC, D100), leaf_for(&cfg(), &secret("b6", 12), &BOB, &USDC, D100)];
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: root_of(&leaves) })).unwrap();
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 0, &leaves, 0, D100, None)).unwrap();
    let before = w.clone();
    let notify2 = Actor::new(config_address(&BRIDGE2), BRIDGE2);
    let r = w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify: notify2, memo: [5; 32] });
    refused(&w, &before, r, "not a bridged token");
    assert_eq!((w.bal(ALICE, DEF), w.supply_of(DEF)), (D100, D100));
}

// ---------------------------------------------------------------------------------------------
// The RFP-021 path: instances X and Y of one attestation program, and X's id at a second one.

struct AttestedPair {
    w: World,
    x: [u8; 32],
    y: [u8; 32],
    state: SynthState,
    k1: [u8; 32],
    leaves1: Vec<[u8; 32]>,
    k2: [u8; 32],
    leaves2: Vec<[u8; 32]>,
}

fn attested_pair() -> AttestedPair {
    let mut w = World::default();
    w.programs.insert(BRIDGE2, Prog::Bridge);
    w.programs.insert(ATTEST2, Prog::Attest);
    let x = plant_instance_at(&mut w, ATTEST, &instance_config(EVM_CHAIN));
    let y = plant_instance_at(&mut w, ATTEST, &InstanceConfig { salt: [1; 32], ..instance_config(EVM_CHAIN) });
    // X's id planted under the second attestation program too (config and anchors), test-only.
    w.states.insert((attest_config_address(&ATTEST2, &x), ATTEST2), borsh::to_vec(&instance_config(EVM_CHAIN)).unwrap());
    let (k1, k2) = (secret("b6a", 1), secret("b6a", 2));
    let leaves1 = vec![leaf_for(&acfg(), &k1, &ALICE, &USDC, D100), leaf_for(&acfg(), &secret("b6a", 3), &CAROL, &USDC, D100)];
    let leaves2 = vec![leaf_for(&acfg2(y), &k2, &BOB, &USDC, D100), leaf_for(&acfg2(y), &secret("b6a", 4), &CAROL, &USDC, D100)];
    let (r1, r2) = (root_of(&leaves1), root_of(&leaves2));
    // Worst case: vault 2 also marks bridge 1's root (it never would, the leaves bind cfgId).
    let state = SynthState {
        accounts: vec![
            SynthAccount::new(VAULT).with(is_deposit_root(&r1), word(1)),
            SynthAccount::new(VAULT2).with(is_deposit_root(&r2), word(1)).with(is_deposit_root(&r1), word(1)),
            SynthAccount::new([0x21; 20]),
        ],
    };
    let anchor = Anchor::StateRoot(state.state_root());
    plant_anchor_at(&mut w, ATTEST, &x, EVM_CHAIN, anchor);
    plant_anchor_at(&mut w, ATTEST, &y, EVM_CHAIN, anchor);
    plant_anchor_at(&mut w, ATTEST2, &x, EVM_CHAIN, anchor);
    for (program, def, admin_, vault_, freezer, inst) in [(BRIDGE, DEF, ADMIN, VAULT, FREEZER, x), (BRIDGE2, DEF2, ADMIN2, VAULT2, FREEZER2, y)] {
        w.tx(def, TOKEN, &Tok::NewFungibleDefinition { name: "Wrapped USDC".into(), total_supply: 0, mint_authority: Some(Authority::Program(program)), supply_to: admin_ })
            .unwrap();
        w.tx(
            program,
            program,
            &instr(Instruction::InitializeAttested {
                admin: admin_,
                vault: vault_,
                evm_chain_id: EVM_CHAIN,
                lez_chain_id: LEZ_CHAIN,
                token_program: TOKEN,
                freezer,
                source: AttestSource { attest_program: ATTEST, attest_instance: inst, deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT },
            }),
        )
        .unwrap();
        w.tx(admin_, program, &admin(AdminOp::RegisterToken { eth_token: USDC, definition: def, decimals: 6, denoms: vec![D100] })).unwrap();
    }
    AttestedPair { w, x, y, state, k1, leaves1, k2, leaves2 }
}

impl AttestedPair {
    /// A claim of the deposit `k` at index 0 of `leaves` under `cfg`, against `vault`'s entry,
    /// verified against `(attest, instance)`.
    fn claim(&self, k: [u8; 32], leaves: &[[u8; 32]], cfg: [u8; 32], vault: EthAddress, attest: AccountId, instance: [u8; 32]) -> Message {
        let root = root_of(leaves);
        instr(Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: 0,
            denom: D100,
            root,
            leaf_index: 0,
            path: merkle_path(leaves, 0),
            secret: k,
            cfg_id: cfg,
            source: AttestSource { attest_program: attest, attest_instance: instance, deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT },
            vault,
            evm_chain_id: EVM_CHAIN,
            anchor: AnchorWitness { anchor: Anchor::StateRoot(self.state.state_root()), header_rlp: None },
            proof: self.state.storage_proof(&vault, &is_deposit_root(&root)),
            recipient_opening: None,
        })
    }
}

#[test]
fn attested_claims_are_bound_to_their_deployment_instance_and_attestation_program() {
    let mut p = attested_pair();
    let (x, y, k1, k2) = (p.x, p.y, p.k1, p.k2);
    let (l1, l2) = (p.leaves1.clone(), p.leaves2.clone());
    assert_ne!(x, y);
    let before = p.w.clone();
    // Bound to instance X, sent to bridge 1 as for instance Y (which finalized the same anchor).
    let r = p.w.tx(ALICE, BRIDGE, &p.claim(k1, &l1, acfg(), VAULT, ATTEST, y));
    refused(&p.w, &before, r, "claim for another attestation source");
    // The same instance id X under another attestation program id (which agrees the anchor).
    let r = p.w.tx(ALICE, BRIDGE, &p.claim(k1, &l1, acfg(), VAULT, ATTEST2, x));
    refused(&p.w, &before, r, "claim for another attestation source");
    // Bridge 1's deposit and claim, sent to bridge 2 as they are.
    let r = p.w.tx(ALICE, BRIDGE2, &p.claim(k1, &l1, acfg(), VAULT, ATTEST, x));
    refused(&p.w, &before, r, "claim for another deployment");
    // Re-aimed at bridge 2's own instance and vault (which marks the root true here).
    let r = p.w.tx(ALICE, BRIDGE2, &p.claim(k1, &l1, acfg(), VAULT2, ATTEST, y));
    refused(&p.w, &before, r, "claim for another deployment");
    let m = p.claim(k1, &l1, acfg2(y), VAULT2, ATTEST, y);
    let r = p.w.tx(ALICE, BRIDGE2, &m);
    refused(&p.w, &before, r, CLAIM_REFUSED);
    assert_eq!(message_fault(ALICE, &m), Some(ClaimFault::NotInRoot));
    // Bridge 2's honest deposit bound to bridge 1's instance X.
    let r = p.w.tx(BOB, BRIDGE2, &p.claim(k2, &l2, acfg2(y), VAULT2, ATTEST, x));
    refused(&p.w, &before, r, "claim for another attestation source");
    // ...or to bridge 1's vault.
    let m = p.claim(k2, &l2, acfg2(y), VAULT, ATTEST, y);
    let r = p.w.tx(BOB, BRIDGE2, &m);
    refused(&p.w, &before, r, CLAIM_REFUSED);
    let Some(ClaimFault::Inclusion(e)) = message_fault(BOB, &m) else { panic!("an inclusion fault") };
    assert_eq!(e.class(), eth_attest_core::inclusion::InclusionClass::Predicate);
    // The honest claims go through, each on its own deployment and token.
    p.w.tx(ALICE, BRIDGE, &p.claim(k1, &l1, acfg(), VAULT, ATTEST, x)).unwrap();
    p.w.tx(BOB, BRIDGE2, &p.claim(k2, &l2, acfg2(y), VAULT2, ATTEST, y)).unwrap();
    assert_eq!((p.w.bal(ALICE, DEF), p.w.bal(ALICE, DEF2), p.w.bal(BOB, DEF), p.w.bal(BOB, DEF2)), (D100, 0, 0, D100));
}

#[test]
fn an_attested_bridge_s_admin_cannot_act_on_the_other() {
    let mut p = attested_pair();
    let before = p.w.clone();
    for (who, program) in [(ADMIN, BRIDGE2), (ADMIN2, BRIDGE)] {
        let r = p.w.tx(who, program, &admin(AdminOp::SetSponsor { amount: 5 }));
        refused(&p.w, &before, r, "only the admin");
        let r = p.w.tx(who, program, &freeze(None, true, false));
        refused(&p.w, &before, r, "only the freezer or admin");
    }
}
