// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regression tests for the independent security review of 2026-11-14
//! (`research/security-review-2026-11-14.md`): each test is an attack the review ran (or a gap it
//! named). The first versions of these tests (commit "bridge tests: regression tests for the
//! 2026-11-14 security review") failed on the reviewed code; they pass on the fixed code.
//!
//! In this simulator `BRIDGE` doubles as the program's loader header account: on LEZ rc3 the
//! program id IS the header account id, an ordinary key-derived account whose key the deployer
//! holds (C1). `w.tx(BRIDGE, ..)` is "the header key signs".

mod common;

use common::*;

const D100: u128 = 100_000_000;
const D1000: u128 = 1_000_000_000;

fn notify() -> Actor {
    Actor::new(config_address(&BRIDGE), BRIDGE)
}

fn initialize(admin: AccountId, lez_chain_id: [u8; 32]) -> Message {
    instr(Instruction::Initialize {
        admin,
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        lez_chain_id,
        token_program: TOKEN,
        freezer: FREEZER,
        attestor: ATTESTOR,
    })
}

// ---------------------------------------------------------------------------------------------
// C1: the header key is not a mint authority.

/// The holder of the bridge's header key signs a token `Mint` as the bridge's id.
#[test]
fn c1_the_header_key_cannot_mint_the_wrapped_token() {
    let (mut w, _rest, _leaves, _root) = claimed_one();
    let supply = w.supply();
    let r = w.tx(BRIDGE, TOKEN, &Tok::Mint { definition: DEF, to: CAROL, amount: 1_000_000 * D100 });
    assert!(r.is_err(), "the header key minted");
    assert!(r.unwrap_err().contains("only the mint authority may mint"));
    assert_eq!((w.bal(CAROL, DEF), w.supply()), (0, supply));
    // The bridge's counters agree with the supply: nothing unbacked to exit against.
    assert_eq!(w.config().tokens[0].claimed_value, w.supply());
    assert!(w.tx(BRIDGE, TOKEN, &Tok::Mint { definition: BRIDGE, to: CAROL, amount: 1 }).is_err());
}

/// The holder of the bridge's header key rotates or renounces the wrapped token's authority.
#[test]
fn c1_the_header_key_cannot_take_the_mint_authority() {
    let (mut w, rest, leaves, root) = claimed_one();
    for new_authority in [Some(Authority::Account(CAROL)), Some(Authority::Account(BRIDGE)), Some(Authority::Program(CAROL)), None] {
        let r = w.tx(BRIDGE, TOKEN, &Tok::SetAuthority { definition: DEF, new_authority });
        assert!(r.is_err(), "the header key changed the authority to {new_authority:?}");
    }
    // The bridge still mints the next claim.
    w.tx(ALICE, BRIDGE, &claim(rest[0], 1, &leaves, root, 0, D100)).unwrap();
    assert_eq!(w.wrapped(ALICE), 2 * D100);
}

/// A stranger (not the header) is refused too: what the old test covered, kept beside the new one.
#[test]
fn c1_a_stranger_cannot_mint_or_rotate() {
    let mut w = setup();
    assert!(w.tx(CAROL, TOKEN, &Tok::Mint { definition: DEF, to: CAROL, amount: 1 }).is_err());
    assert!(w.tx(CAROL, TOKEN, &Tok::SetAuthority { definition: DEF, new_authority: Some(Authority::Account(CAROL)) }).is_err());
}

/// The bridge's code still mints: `MintBy` from its config, for a real claim only.
#[test]
fn c1_the_bridge_program_still_mints_by_program() {
    let (w, _rest, _leaves, _root) = claimed_one();
    assert_eq!((w.wrapped(ALICE), w.supply()), (D100, D100));
}

/// A definition whose authority is the bridge's id typed as an ACCOUNT (what a careless
/// `create-wrapped` would make) is not mintable by the bridge: the claim fails and changes
/// nothing, so registering one can never reintroduce C1 silently.
#[test]
fn c1_an_account_typed_bridge_authority_is_not_the_program() {
    const DEFA: AccountId = AccountId::new([0xDA; 32]);
    let mut w = setup();
    w.tx(DEFA, TOKEN, &Tok::NewFungibleDefinition { name: "Account-typed".into(), total_supply: 0, mint_authority: Some(Authority::Account(BRIDGE)), supply_to: ADMIN })
        .unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEFA, decimals: 18, denoms: vec![100] }))).unwrap();
    let k = secret("c1", 1);
    let leaves = vec![leaf_for(&cfg(), &k, &ALICE, &DAI, 100), leaf_for(&cfg(), &secret("c1", 2), &BOB, &DAI, 100)];
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: root_of(&leaves) })).unwrap();
    let e = w.tx(ALICE, BRIDGE, &claim_msg(cfg(), DAI, k, 0, &leaves, 0, 100, None)).unwrap_err();
    assert!(e.contains("only the mint authority may mint"), "{e}");
    // ...and the header key, which COULD sign for that account authority, is what the CLI's
    // `register` refuses to register (it reads the definition back; see cli/src/main.rs).
}

// ---------------------------------------------------------------------------------------------
// H1: init is the deployer's only.

/// A stranger front-runs the operator's init with the operator's public inputs.
#[test]
fn h1_a_stranger_cannot_initialize() {
    let mut w = World::default();
    let e = w.tx(CAROL, BRIDGE, &init_with(VAULT, CAROL, CAROL)).unwrap_err();
    assert!(e.contains(INIT_BY_HEADER), "{e}");
    let e = w.tx(CAROL, BRIDGE, &initialize(CAROL, LEZ_CHAIN)).unwrap_err();
    assert!(e.contains(INIT_BY_HEADER), "{e}");
    assert!(w.state(config_address(&BRIDGE), BRIDGE).is_empty());
    // The deployer still initialises afterwards, with the roles it names.
    w.tx(BRIDGE, BRIDGE, &initialize(ADMIN, LEZ_CHAIN)).unwrap();
    let c = w.config();
    assert_eq!((c.admin, c.freezer, c.attestor), (ADMIN, FREEZER, ATTESTOR));
    assert_eq!(c.cfg_id, cfg());
}

/// The retired first-come `Init` and `InitAttested` are refused for everyone, the header too.
#[test]
fn h1_the_retired_init_is_refused_even_from_the_header() {
    let mut w = World::default();
    let old = instr(Instruction::Init {
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        lez_chain_id: LEZ_CHAIN,
        zone_id: ZONE,
        token_program: TOKEN,
        freezer: FREEZER,
        attestor: ATTESTOR,
    });
    for who in [BRIDGE, ADMIN, CAROL] {
        let e = w.tx(who, BRIDGE, &old).unwrap_err();
        assert!(e.contains(INIT_RETIRED), "{e}");
    }
    let id = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    let old = instr(Instruction::InitAttested {
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        lez_chain_id: LEZ_CHAIN,
        zone_id: ZONE,
        token_program: TOKEN,
        freezer: FREEZER,
        source: source(id),
    });
    for who in [BRIDGE, CAROL] {
        assert!(w.tx(who, BRIDGE, &old).unwrap_err().contains(INIT_RETIRED));
    }
    assert!(w.state(config_address(&BRIDGE), BRIDGE).is_empty());
}

/// A stranger cannot init the attested mode either (it would pick the attestation source).
#[test]
fn h1_a_stranger_cannot_initialize_attested() {
    let mut w = World::default();
    let id = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    let e = w.tx(CAROL, BRIDGE, &init_attested(source(id), EVM_CHAIN)).unwrap_err();
    assert!(e.contains(INIT_BY_HEADER), "{e}");
    w.tx(BRIDGE, BRIDGE, &init_attested(source(id), EVM_CHAIN)).unwrap();
    assert_eq!(w.config().cfg_id, acfg());
}

/// After init the header key holds no role: it cannot be named admin, freezer or attestor, and
/// a second Initialize is refused.
#[test]
fn h1_the_header_takes_no_role_and_inits_once() {
    let mut w = World::default();
    let e = w.tx(BRIDGE, BRIDGE, &initialize(BRIDGE, LEZ_CHAIN)).unwrap_err();
    assert!(e.contains("the header account holds no role"), "{e}");
    let e = w.tx(BRIDGE, BRIDGE, &init_with(VAULT, BRIDGE, ATTESTOR)).unwrap_err();
    assert!(e.contains("the header account holds no role"), "{e}");
    let e = w.tx(BRIDGE, BRIDGE, &init_with(VAULT, FREEZER, BRIDGE)).unwrap_err();
    assert!(e.contains("the header account holds no role"), "{e}");
    w.tx(BRIDGE, BRIDGE, &initialize(ADMIN, LEZ_CHAIN)).unwrap();
    let e = w.tx(BRIDGE, BRIDGE, &initialize(CAROL, LEZ_CHAIN)).unwrap_err();
    assert!(e.contains("already initialised"), "{e}");
    // The header key is refused every admin op.
    let e = w.tx(BRIDGE, BRIDGE, &instr(Instruction::Admin(AdminOp::SetAttestor { attestor: CAROL }))).unwrap_err();
    assert!(e.contains("only the admin"), "{e}");
    assert!(w.tx(BRIDGE, BRIDGE, &instr(Instruction::AttestRoot { root: [1; 32] })).unwrap_err().contains("only the attestor"));
}

// ---------------------------------------------------------------------------------------------
// M1 / M4: the chain and the root source are bound in the cfg id.

#[test]
fn m1_initialize_needs_the_lez_chain_id() {
    let mut w = World::default();
    let e = w.tx(BRIDGE, BRIDGE, &initialize(ADMIN, [0; 32])).unwrap_err();
    assert!(e.contains("init needs the LEZ chain id"), "{e}");
    // Two chains (two declared channel ids) give two cfg ids for the same vault and program.
    let other = cfg_id(EVM_CHAIN, &VAULT, &[0x4D; 32], &source_id(&RootSource::StandIn), &BRIDGE);
    assert_ne!(other, cfg());
}

#[test]
fn m4_the_root_source_is_bound_in_the_cfg_id() {
    let mut w = World::default();
    let x = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    let y = plant_instance(&mut w, &InstanceConfig { checkpoint: [0xC1; 32], ..instance_config(EVM_CHAIN) });
    // Each attestation instance (checkpoint, forks, genesis root) is its own deployment id; so is
    // the slot index, and the stand-in.
    let ids = [
        cfg_for_source(source(x)),
        cfg_for_source(source(y)),
        cfg_for_source(AttestSource { deposit_root_slot: 84, ..source(x) }),
        cfg(),
    ];
    for i in 0..ids.len() {
        for j in 0..i {
            assert_ne!(ids[i], ids[j]);
        }
    }
    w.tx(BRIDGE, BRIDGE, &init_attested(source(y), EVM_CHAIN)).unwrap();
    let c = w.config();
    assert_eq!((c.zone_id, c.cfg_id), (source_id(&RootSource::Attest(source(y))), cfg_for_source(source(y))));
    // The byte layout source_id documents.
    let mut pre = vec![1u8];
    pre.extend_from_slice(ATTEST.value());
    pre.extend_from_slice(&y);
    pre.extend_from_slice(&VAULT_DEPOSIT_ROOT_SLOT.to_le_bytes());
    assert_eq!(source_id(&RootSource::Attest(source(y))), sha256(&[SOURCE_DOMAIN, &pre]));
    assert_eq!(source_id(&RootSource::StandIn), sha256(&[SOURCE_DOMAIN, &[0]]));
}

// ---------------------------------------------------------------------------------------------
// H2: a leaked secret cannot destroy the deposit.

/// Carol learns Alice's `k` (a public claim, a relay, a reverted claim) and deposits the smallest
/// denomination with the same `k` for herself, then claims it first. Alice's deposit must still
/// be claimable.
#[test]
fn h2_a_leaked_secret_cannot_burn_the_deposit_with_a_cheaper_one() {
    let mut w = setup();
    let k = [0x42u8; 32];
    let leaves: Vec<[u8; 32]> = [(ALICE, D1000), (CAROL, D100)]
        .iter()
        .map(|(dest, d)| leaf_for(&cfg(), &k, dest, &USDC, *d))
        .collect();
    let root = root_of(&leaves);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(CAROL, BRIDGE, &claim(k, 1, &leaves, root, 0, D100)).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, D1000)).expect("Alice's deposit was burned by Carol's");
    assert_eq!((w.bal(ALICE, DEF), w.bal(CAROL, DEF)), (D1000, D100));
    // Each claim is still once only.
    assert!(w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, D1000)).unwrap_err().contains(CLAIM_REFUSED));
    assert!(w.tx(CAROL, BRIDGE, &claim(k, 1, &leaves, root, 0, D100)).unwrap_err().contains(CLAIM_REFUSED));
}

/// The same `k` for the same account but another denomination or token is another nullifier;
/// the same deposit is one nullifier (the vault refuses a repeated commitment, `CommitmentUsed`).
#[test]
fn h2_the_nullifier_binds_destination_token_and_denomination() {
    let k = [7u8; 32];
    let base = claim_nullifier(&k, &ALICE, &USDC, D100);
    assert_ne!(base, claim_nullifier(&k, &CAROL, &USDC, D100));
    assert_ne!(base, claim_nullifier(&k, &ALICE, &DAI, D100));
    assert_ne!(base, claim_nullifier(&k, &ALICE, &USDC, D1000));
    assert_eq!(base, sha256(&[NFC_DOMAIN, &k, ALICE.value(), &USDC, &u256(D100)]));
}

/// Same for the attested path.
#[test]
fn h2_attested_a_second_deposit_with_the_same_secret_cannot_burn_the_first() {
    let mut w = World::default();
    let id = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    w.tx(DEF, TOKEN, &Tok::NewFungibleDefinition { name: "W".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN }).unwrap();
    w.tx(BRIDGE, BRIDGE, &init_attested(source(id), EVM_CHAIN)).unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: USDC, definition: DEF, decimals: 6, denoms: vec![D100, D1000] }))).unwrap();
    let k = [0x43u8; 32];
    let (leaves, root) = deposits(&[(k, ALICE, D1000), (k, CAROL, D100)]);
    let state = SynthState { accounts: vec![SynthAccount::new(VAULT).with(is_deposit_root(&root), word(1)), SynthAccount::new([0x21; 20])] };
    let anchor = Anchor::StateRoot(state.state_root());
    plant_anchor(&mut w, &id, EVM_CHAIN, anchor);
    let mk = |idx: usize, di: u32, denom: u128| {
        instr(Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: di,
            denom,
            root,
            leaf_index: idx as u64,
            path: merkle_path(&leaves, idx),
            secret: k,
            cfg_id: acfg(),
            source: source(id),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor: AnchorWitness { anchor, header_rlp: None },
            proof: state.storage_proof(&VAULT, &is_deposit_root(&root)),
            recipient_opening: None,
        })
    };
    w.tx(CAROL, BRIDGE, &mk(1, 0, D100)).unwrap();
    w.tx(ALICE, BRIDGE, &mk(0, 1, D1000)).expect("Alice's attested deposit was burned");
    assert_eq!((w.bal(ALICE, DEF), w.bal(CAROL, DEF)), (D1000, D100));
}

// ---------------------------------------------------------------------------------------------
// M2: the sponsor amount is bounded.

#[test]
fn m2_the_sponsor_amount_is_capped() {
    let mut w = setup();
    let r = w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: 1_000_000_000_000 })));
    assert!(r.unwrap_err().contains("above the program's cap"));
    let r = w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: MAX_SPONSOR_AMOUNT + 1 })));
    assert!(r.is_err());
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: MAX_SPONSOR_AMOUNT }))).unwrap();
    assert_eq!(w.config().sponsor_amount, MAX_SPONSOR_AMOUNT);
}

/// The review's probe: the admin sets the sponsor amount to the whole float, one claim drains it.
/// Now one claim draws at most the cap, whatever the float holds.
#[test]
fn m2_one_claim_draws_at_most_the_cap() {
    let (mut w, ks, leaves, root) = two_deposits();
    fund_float(&mut w, 10 * MAX_SPONSOR_AMOUNT);
    assert!(w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: 10 * MAX_SPONSOR_AMOUNT }))).is_err());
    set_sponsor(&mut w, MAX_SPONSOR_AMOUNT);
    let o = [0x0F; 32];
    w.tx_with_openings(ALICE, BRIDGE, &claim_to(ks[0], 0, &leaves, root, 0, D100, Some(o)), &[o]).unwrap();
    assert_eq!(w.float(), 9 * MAX_SPONSOR_AMOUNT);
    let _ = root;
}

// ---------------------------------------------------------------------------------------------
// M3: one wrapped definition per Ethereum token.

#[test]
fn m3_a_definition_registered_twice_is_refused() {
    let mut w = setup(); // USDC -> DEF
    let r = w.tx(
        ADMIN,
        BRIDGE,
        &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEF, decimals: 18, denoms: vec![D100] })),
    );
    assert!(r.unwrap_err().contains("definition already registered"));
    assert_eq!(w.config().tokens.len(), 1);
    // A burn of DEF is still matched to USDC's entry, which counted the claims.
    let (mut w, _rest, _leaves, _root) = claimed_one();
    w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify: notify(), memo: [1; 32] }).unwrap();
}
