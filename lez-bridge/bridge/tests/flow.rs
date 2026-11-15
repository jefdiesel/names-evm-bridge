//! Whole-transaction tests of the stand-in path: the simulator in `common/mod.rs` runs the token
//! and bridge programs, and LEZ's own native token code, together (see its docs for what it
//! models and what it does not: private accounts, sender presentation, promotion, fees).

mod common;

use common::*;

// ---------------------------------------------------------------------------------------------

#[test]
fn hashes_and_tree_match_the_solidity_vault() {
    // Vectors from vault/test/Vectors.t.sol: cfgId 0x..22, token 0x..AA, denom 100e6, C = 1, 2, 3.
    let cfg = {
        let mut c = [0u8; 32];
        c[31] = 0x22;
        c
    };
    let mut token = [0u8; 20];
    token[19] = 0xAA;
    let c = |i: u8| {
        let mut b = [0u8; 32];
        b[31] = i;
        b
    };
    let hex = |s: &str| -> [u8; 32] {
        let s = s.trim_start_matches("0x");
        let mut o = [0u8; 32];
        for i in 0..32 {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    };
    let leaves = [
        deposit_leaf(&cfg, &token, 100_000_000, &c(1)),
        deposit_leaf(&cfg, &token, 100_000_000, &c(2)),
        deposit_leaf(&cfg, &token, 100_000_000, &c(3)),
    ];
    assert_eq!(leaves[0], hex("0x989d86fd05b092a4c8813f55dadd7fe91f22613ed224ed048e975e85b03d7f9b"));
    assert_eq!(leaves[1], hex("0x268dcd2d37cd543db8910d79638b314baf930072f9d94af49036453fc3084d2b"));
    assert_eq!(leaves[2], hex("0xdb3c6052763f80a5fb65302f8a3bc9c9d264be65e6b3d9996a6c75e0be38c9e7"));
    let roots = [
        "0x4c7f3798294337ffb524de6feadaa4525b094c840d2bde5291fa4af211e386fe",
        "0x3f07025253b4cc4a91c972d6e59cba62ff64d35e93180a62715b5427d7157321",
        "0xd5ed53e62c4cfaa30c5091797e906f96fa66e84ce3976be79eee96abfc638f38",
    ];
    for n in 1..=3 {
        for i in 0..n {
            let path = merkle_path(&leaves[..n], i);
            assert_eq!(merkle_root(&leaves[i], i as u64, &path), hex(roots[n - 1]), "n={n} i={i}");
        }
    }
    // The empty tree's root is the deposit contract's.
    assert_eq!(
        zero_hashes()[32],
        hex("0xc6f67e02e6e4e1bdefb994c6098953f34636ba2b6ca20a4721d2b26a886722ff")
    );
}

#[test]
fn claim_mints_once_and_burn_records_the_exit() {
    let mut w = setup();
    let (ka, kb) = ([1u8; 32], [2u8; 32]);
    let (leaves, root) = vault(&[(ka, ALICE, 100_000_000), (kb, BOB, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();

    w.tx(ALICE, BRIDGE, &claim(ka, 0, &leaves, root, 0, 100_000_000)).unwrap();
    assert_eq!(w.wrapped(ALICE), 100_000_000);
    assert_eq!(w.supply(), 100_000_000);

    // The same deposit can't be claimed twice, by anyone.
    assert_eq!(w.tx(ALICE, BRIDGE, &claim(ka, 0, &leaves, root, 0, 100_000_000)).unwrap_err(), CLAIM_REFUSED);

    w.tx(BOB, BRIDGE, &claim(kb, 1, &leaves, root, 1, 1_000_000_000)).unwrap();
    assert_eq!(w.wrapped(BOB), 1_000_000_000);
    assert_eq!(w.config().tokens[0].claims, vec![1, 1]);

    // Wrapped tokens move like any token.
    w.tx(ALICE, TOKEN, &Tok::Transfer { definition: DEF, to: BOB, amount: 100_000_000 }).unwrap();
    assert_eq!(w.wrapped(BOB), 1_100_000_000);

    // Burn exactly one denomination with an exit commitment.
    let e = exit_commitment(&[9u8; 32], &[0x0E; 20], &USDC, 100_000_000, 2_000_000, &cfg());
    w.events.clear();
    w.tx(
        BOB,
        TOKEN,
        &Tok::BurnWithNotice { definition: DEF, amount: 100_000_000, notify: Actor::new(config_address(&BRIDGE), BRIDGE), memo: e },
    )
    .unwrap();
    assert_eq!(w.wrapped(BOB), 1_000_000_000);
    assert_eq!(w.supply(), 1_000_000_000);
    assert_eq!(w.config().tokens[0].burns, vec![1, 0]);
    let exited: Vec<Exited> = w
        .events
        .iter()
        .filter_map(|ev| borsh::from_slice::<Exited>(&ev.data).ok())
        .collect();
    assert_eq!(exited, vec![Exited { eth_token: USDC, denom_index: 0, exit_commitment: e }]);
}

#[test]
fn a_claim_needs_an_attested_root() {
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 100_000_000)]);
    let m = claim(k, 0, &leaves, root, 0, 100_000_000);
    let err = w.tx(ALICE, BRIDGE, &m).unwrap_err();
    // The one claim refusal (RFP-022 Privacy #5); the witness itself is fine.
    assert_eq!(err, CLAIM_REFUSED);
    assert_eq!(message_fault(ALICE, &m), None);
    assert_eq!(w.wrapped(ALICE), 0);
}

#[test]
fn only_the_attestor_records_roots() {
    let mut w = setup();
    let err = w.tx(ALICE, BRIDGE, &instr(Instruction::AttestRoot { root: [5; 32] })).unwrap_err();
    assert!(err.contains("only the attestor"), "{err}");
}

#[test]
fn a_claim_is_bound_to_its_destination() {
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    // Bob knows Alice's secret but the commitment names Alice's account.
    let m = claim(k, 0, &leaves, root, 0, 100_000_000);
    let err = w.tx(BOB, BRIDGE, &m).unwrap_err();
    assert_eq!(err, CLAIM_REFUSED);
    assert_eq!(message_fault(BOB, &m), Some(ClaimFault::NotInRoot));
}

#[test]
fn a_claim_cannot_mint_a_larger_denomination() {
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    // Claim the 100 deposit as a 1000: the leaf doesn't match.
    let m = claim(k, 0, &leaves, root, 1, 1_000_000_000);
    let err = w.tx(ALICE, BRIDGE, &m).unwrap_err();
    assert_eq!(err, CLAIM_REFUSED);
    assert_eq!(message_fault(ALICE, &m), Some(ClaimFault::NotInRoot));
    // Right leaf, wrong index for the amount: the config refuses.
    let err = w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, 100_000_000)).unwrap_err();
    assert!(err.contains("wrong amount"), "{err}");
    assert_eq!(w.wrapped(ALICE), 0);
}

#[test]
fn a_burn_must_be_one_denomination_and_from_the_token_program() {
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, 1_000_000_000)).unwrap();
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    let err = w
        .tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: 50_000_000, notify, memo: [0; 32] })
        .unwrap_err();
    assert!(err.contains("exactly one denomination"), "{err}");
    assert_eq!(w.wrapped(ALICE), 1_000_000_000, "a refused burn burns nothing");
}

#[test]
fn fragments_redeem_by_value() {
    // The only claim was one 1000; the holder exits it as ten 100s.
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, 1_000_000_000)).unwrap();
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    let burn = Tok::BurnWithNotice { definition: DEF, amount: 100_000_000, notify, memo: [0; 32] };
    for n in 0..10 {
        w.tx(ALICE, TOKEN, &burn).unwrap_or_else(|e| panic!("burn {n}: {e}"));
    }
    let t = &w.config().tokens[0];
    assert_eq!((t.claims.clone(), t.burns.clone()), (vec![0, 1], vec![10, 0]));
    assert_eq!((t.claimed_value, t.burned_value), (1_000_000_000, 1_000_000_000));
    assert_eq!(w.supply(), 0);
    // An 11th: the holder has nothing left, so the token program refuses it first...
    let err = w.tx(ALICE, TOKEN, &burn).unwrap_err();
    assert!(err.contains("insufficient token balance"), "{err}");
    // ...and the bridge's own bound refuses the notice if one ever arrived.
    let err = notice(&mut w, 100_000_000).unwrap_err();
    assert!(err.contains("burns exceed claims"), "{err}");
    assert_eq!(w.config().tokens[0].burned_value, 1_000_000_000);
}

#[test]
fn the_bridge_bounds_burns_by_claimed_value() {
    // Notices straight from the token program: the bound is the token's total value.
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 1_000_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    // Nothing claimed yet: nothing may exit.
    assert!(notice(&mut w, 100_000_000).unwrap_err().contains("burns exceed claims"));
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 1, 1_000_000_000)).unwrap();
    notice(&mut w, 100_000_000).unwrap();
    // 100 + 1000 > 1000.
    assert!(notice(&mut w, 1_000_000_000).unwrap_err().contains("burns exceed claims"));
    for _ in 0..9 {
        notice(&mut w, 100_000_000).unwrap();
    }
    assert!(notice(&mut w, 100_000_000).unwrap_err().contains("burns exceed claims"));
    let t = &w.config().tokens[0];
    assert_eq!((t.burns.clone(), t.burned_value), (vec![10, 0], 1_000_000_000));
    // A notice from anything but the token program is refused.
    let forged = bridge_token_core::BurnNotice { definition: DEF, amount: 100_000_000, memo: [0; 32] };
    let mut asyncs = VecDeque::new();
    let err = w
        .deliver(
            Some(Actor::new(ALICE, ATTESTOR)),
            Actor::new(config_address(&BRIDGE), BRIDGE),
            borsh::to_vec(&forged).unwrap(),
            BTreeSet::new(),
            None,
            &mut asyncs,
        )
        .unwrap_err();
    assert!(err.contains("comes from the token program"), "{err}");
}

#[test]
fn duplicate_denominations_are_refused() {
    let mut w = setup();
    let other: EthAddress = [0xBB; 20];
    let err = w
        .tx(
            ADMIN,
            BRIDGE,
            &instr(Instruction::Admin(AdminOp::RegisterToken {
                eth_token: other,
                definition: DEF2,
                decimals: 6,
                denoms: vec![100, 1000, 100],
            })),
        )
        .unwrap_err();
    assert!(err.contains("duplicate denomination"), "{err}");
    assert_eq!(w.config().tokens.len(), 1);
    let err = w
        .tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::AddDenomination { eth_token: USDC, denom: 1_000_000_000 })))
        .unwrap_err();
    assert!(err.contains("duplicate denomination"), "{err}");
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::AddDenomination { eth_token: USDC, denom: 10_000_000 })))
        .unwrap();
    let t = &w.config().tokens[0];
    assert_eq!(t.denoms, vec![100_000_000, 1_000_000_000, 10_000_000]);
    assert_eq!((t.claims.len(), t.burns.len()), (3, 3));
}

#[test]
fn cfg_id_matches_the_vault_formula() {
    // sha256(abi.encodePacked(keccak256("RFP022/cfg/v1"), uint256(11155111), 0x11..11, 0x4c..4c,
    // 0x5a..5a, 0xb0..b0)), computed outside Rust (cast abi-encode --packed | sha256).
    let expected = "a7e47fcb60fd899df456feae294b210a956efa4dfaa3ed4487105b4040320c3a";
    let base = cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &ZONE, &BRIDGE);
    let got: String = base.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(got, expected);
    // Every input is bound.
    assert_ne!(cfg_id(EVM_CHAIN + 1, &VAULT, &LEZ_CHAIN, &ZONE, &BRIDGE), base);
    assert_ne!(cfg_id(EVM_CHAIN, &[0x12; 20], &LEZ_CHAIN, &ZONE, &BRIDGE), base);
    assert_ne!(cfg_id(EVM_CHAIN, &VAULT, &ZONE, &LEZ_CHAIN, &BRIDGE), base);
    assert_ne!(cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &ZONE, &TOKEN), base);
    // Since the 2026-11-14 fixes the zone slot holds the derived root-source id.
    assert_eq!(cfg(), cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &source_id(&RootSource::StandIn), &BRIDGE));
}

#[test]
fn init_derives_cfg_id_from_the_program_id() {
    let w = setup();
    let c = w.config();
    assert_eq!(c.cfg_id, cfg());
    assert_eq!((c.vault, c.evm_chain_id, c.lez_chain_id, c.zone_id), (VAULT, EVM_CHAIN, LEZ_CHAIN, source_id(&RootSource::StandIn)));
    // A deposit committed under any other id (e.g. one the admin made up) can't be claimed.
    let mut w = w;
    let k = [1u8; 32];
    let wrong = [0x22; 32];
    let leaves = vec![deposit_leaf(&wrong, &USDC, 100_000_000, &deposit_commitment(&k, &ALICE, &USDC, 100_000_000, &wrong))];
    let root = merkle_root(&leaves[0], 0, &merkle_path(&leaves, 0));
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    let mut m = claim(k, 0, &leaves, root, 0, 100_000_000);
    if let Message::Instruction(Instruction::Claim { cfg_id, .. }) = &mut m {
        *cfg_id = wrong;
    }
    let err = w.tx(ALICE, BRIDGE, &m).unwrap_err();
    assert!(err.contains("another deployment"), "{err}");
}

#[test]
fn freeze_stops_claims_and_burns_and_only_admin_unfreezes() {
    let mut w = setup();
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, true)).unwrap();
    assert!(w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 0, 100_000_000)).unwrap_err().contains("frozen"));
    assert!(w.tx(FREEZER, BRIDGE, &unfreeze(Some(USDC), true, true)).unwrap_err().contains("only the admin"));
    w.tx(ADMIN, BRIDGE, &unfreeze(Some(USDC), true, true)).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 0, 100_000_000)).unwrap();
}

#[test]
fn init_happens_once_and_admin_ops_need_the_admin() {
    let mut w = setup();
    let again = init_with(VAULT, ALICE, ALICE);
    assert!(w.tx(BRIDGE, BRIDGE, &again).unwrap_err().contains("already initialised"));
    assert!(w.tx(ALICE, BRIDGE, &again).unwrap_err().contains(INIT_BY_HEADER));
    let op = instr(Instruction::Admin(AdminOp::SetAttestor { attestor: ALICE }));
    assert!(w.tx(ALICE, BRIDGE, &op).unwrap_err().contains("only the admin"));
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::ProposeAdmin { next: BOB }))).unwrap();
    w.tx(BOB, BRIDGE, &instr(Instruction::Admin(AdminOp::AcceptAdmin))).unwrap();
    assert_eq!(w.config().admin, BOB);
}

#[test]
fn nobody_but_the_bridge_mints_the_wrapped_token() {
    let mut w = setup();
    let err = w.tx(ALICE, TOKEN, &Tok::Mint { definition: DEF, to: ALICE, amount: 1 }).unwrap_err();
    assert!(err.contains("only the mint authority"), "{err}");
    // An effect forged from outside the bridge program is refused.
    let forged = Message::Effect(Effect::Claim { eth_token: USDC, denom_index: 0, denom: 100_000_000, to: ALICE, cfg_id: cfg(), one_time: false });
    let err = w.tx(ALICE, BRIDGE, &forged).unwrap_err();
    assert!(err.contains("only this program may send an effect"), "{err}");
}

#[test]
fn deregistered_tokens_still_claim_and_burn() {
    // Bob deposited before the token was delisted: his claim stays valid forever.
    let mut w = setup();
    let (ka, kb) = ([1u8; 32], [2u8; 32]);
    let (leaves, root) = vault(&[(ka, ALICE, 100_000_000), (kb, BOB, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(ka, 0, &leaves, root, 0, 100_000_000)).unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::DeregisterToken { eth_token: USDC }))).unwrap();
    assert!(!w.config().tokens[0].registered);
    w.tx(BOB, BRIDGE, &claim(kb, 1, &leaves, root, 0, 100_000_000)).unwrap();
    assert_eq!(w.wrapped(BOB), 100_000_000);
    assert_eq!(w.config().tokens[0].claims, vec![2, 0]);
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: 100_000_000, notify, memo: [3; 32] }).unwrap();
    // Freezing, not delisting, is what stops claims.
    let kc = [3u8; 32];
    let (leaves, root) = vault(&[(ka, ALICE, 100_000_000), (kb, BOB, 100_000_000), (kc, ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    assert!(w.tx(ALICE, BRIDGE, &claim(kc, 2, &leaves, root, 0, 100_000_000)).unwrap_err().contains("frozen"));
}

// ---------------------------------------------------------------------------------------------
// Freezing minting and redemption independently (RFP-022 Functionality #14, Security #5).

#[test]
fn mint_frozen_stops_claims_but_burns_flow() {
    for scope in [None, Some(USDC)] {
        let (mut w, ks, leaves, root) = claimed_one();
        w.tx(FREEZER, BRIDGE, &freeze(scope, true, false)).unwrap();
        let c = w.config();
        assert_eq!((c.mint_frozen, c.burn_frozen), (scope.is_none(), false));
        assert_eq!((c.tokens[0].mint_frozen, c.tokens[0].burn_frozen), (scope.is_some(), false));
        let err = w.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, 100_000_000)).unwrap_err();
        assert!(err.contains("minting is frozen"), "{scope:?}: {err}");
        assert_eq!(w.wrapped(ALICE), 100_000_000, "a refused claim mints nothing");
        // The refused claim spent no nullifier: it goes through once minting is unfrozen.
        burn(&mut w, ALICE).unwrap_or_else(|e| panic!("{scope:?}: {e}"));
        assert_eq!(w.config().tokens[0].burns, vec![1, 0]);
        w.tx(ADMIN, BRIDGE, &unfreeze(scope, true, false)).unwrap();
        w.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, 100_000_000)).unwrap();
        assert_eq!(w.wrapped(ALICE), 100_000_000);
    }
}

#[test]
fn burn_frozen_stops_burns_but_claims_flow() {
    for scope in [None, Some(USDC)] {
        let (mut w, ks, leaves, root) = claimed_one();
        w.tx(FREEZER, BRIDGE, &freeze(scope, false, true)).unwrap();
        let c = w.config();
        assert_eq!((c.mint_frozen, c.burn_frozen), (false, scope.is_none()));
        assert_eq!((c.tokens[0].mint_frozen, c.tokens[0].burn_frozen), (false, scope.is_some()));
        let err = burn(&mut w, ALICE).unwrap_err();
        assert!(err.contains("redemption is frozen"), "{scope:?}: {err}");
        assert_eq!(w.wrapped(ALICE), 100_000_000, "a refused burn burns nothing");
        assert_eq!(w.supply(), 100_000_000);
        assert_eq!(w.config().tokens[0].burns, vec![0, 0]);
        // A notice that reached the bridge some other way is refused too.
        assert!(notice(&mut w, 100_000_000).unwrap_err().contains("redemption is frozen"));
        w.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, 100_000_000)).unwrap();
        assert_eq!(w.wrapped(ALICE), 200_000_000);
        w.tx(ADMIN, BRIDGE, &unfreeze(scope, false, true)).unwrap();
        burn(&mut w, ALICE).unwrap();
        assert_eq!(w.wrapped(ALICE), 100_000_000);
    }
}

#[test]
fn a_frozen_claim_fails_the_same_way_at_either_level() {
    let (mut w, ks, leaves, root) = claimed_one();
    let m = claim(ks[0], 1, &leaves, root, 0, 100_000_000);
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    let global = w.tx(ALICE, BRIDGE, &m).unwrap_err();
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    let token = w.tx(ALICE, BRIDGE, &m).unwrap_err();
    assert_eq!(global, token);
    // Both levels at once: still the same.
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    assert_eq!(w.tx(ALICE, BRIDGE, &m).unwrap_err(), global);
}

#[test]
fn a_token_freeze_leaves_other_tokens_alone() {
    // A second bridged token with its own wrapped definition.
    const DAI: EthAddress = [0xDA; 20];
    const DEF2: AccountId = AccountId::new([0xD2; 32]);
    let mut w = setup();
    w.tx(
        DEF2,
        TOKEN,
        &Tok::NewFungibleDefinition { name: "Wrapped DAI".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN },
    )
    .unwrap();
    w.tx(
        ADMIN,
        BRIDGE,
        &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEF2, decimals: 18, denoms: vec![100] })),
    )
    .unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(Some(DAI), true, true)).unwrap();
    let c = w.config();
    assert_eq!((c.mint_frozen, c.burn_frozen), (false, false));
    assert_eq!((c.tokens[0].mint_frozen, c.tokens[0].burn_frozen), (false, false));
    assert_eq!((c.tokens[1].mint_frozen, c.tokens[1].burn_frozen), (true, true));
    // USDC claims and burns as usual.
    let k = [1u8; 32];
    let (leaves, root) = vault(&[(k, ALICE, 100_000_000)]);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    w.tx(ALICE, BRIDGE, &claim(k, 0, &leaves, root, 0, 100_000_000)).unwrap();
    burn(&mut w, ALICE).unwrap();
    // DAI is frozen both ways.
    let kd = [8u8; 32];
    let dai_leaves = vec![deposit_leaf(&cfg(), &DAI, 100, &deposit_commitment(&kd, &ALICE, &DAI, 100, &cfg()))];
    let dai_root = merkle_root(&dai_leaves[0], 0, &merkle_path(&dai_leaves, 0));
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: dai_root })).unwrap();
    let dai_claim = instr(Instruction::Claim {
        eth_token: DAI,
        denom_index: 0,
        denom: 100,
        root: dai_root,
        leaf_index: 0,
        path: merkle_path(&dai_leaves, 0),
        secret: kd,
        cfg_id: cfg(),
        recipient_opening: None,
    });
    assert!(w.tx(ALICE, BRIDGE, &dai_claim).unwrap_err().contains("minting is frozen"));
    let mut asyncs = VecDeque::new();
    let dai_notice = bridge_token_core::BurnNotice { definition: DEF2, amount: 100, memo: [0; 32] };
    let err = w
        .deliver(
            Some(Actor::new(DEF2, TOKEN)),
            Actor::new(config_address(&BRIDGE), BRIDGE),
            borsh::to_vec(&dai_notice).unwrap(),
            BTreeSet::new(),
            None,
            &mut asyncs,
        )
        .unwrap_err();
    assert!(err.contains("redemption is frozen"), "{err}");
    // Unfrozen, DAI claims.
    w.tx(ADMIN, BRIDGE, &unfreeze(Some(DAI), true, true)).unwrap();
    w.tx(ALICE, BRIDGE, &dai_claim).unwrap();
    assert_eq!(balance_of(&w.state(ALICE, TOKEN), &DEF2), 100);
}

#[test]
fn freezer_freezes_but_only_the_admin_unfreezes() {
    let (mut w, ks, leaves, root) = claimed_one();
    // The freezer (and the admin) may freeze; nobody else.
    assert!(w.tx(ALICE, BRIDGE, &freeze(None, true, true)).unwrap_err().contains("only the freezer or admin"));
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    w.tx(ADMIN, BRIDGE, &freeze(Some(USDC), false, true)).unwrap();
    for (scope, mint, burn) in [(None, true, false), (Some(USDC), false, true), (None, true, true)] {
        let err = w.tx(FREEZER, BRIDGE, &unfreeze(scope, mint, burn)).unwrap_err();
        assert!(err.contains("only the admin"), "{err}");
        assert!(w.tx(ALICE, BRIDGE, &unfreeze(scope, mint, burn)).unwrap_err().contains("only the admin"));
    }
    let c = w.config();
    assert_eq!((c.mint_frozen, c.burn_frozen, c.tokens[0].mint_frozen, c.tokens[0].burn_frozen), (true, false, false, true));
    // Partial unfreeze: each call clears only what it names.
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    w.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, 100_000_000)).unwrap();
    assert!(burn(&mut w, ALICE).unwrap_err().contains("redemption is frozen"));
    w.tx(ADMIN, BRIDGE, &unfreeze(Some(USDC), false, true)).unwrap();
    burn(&mut w, ALICE).unwrap();
}

#[test]
fn a_partial_unfreeze_keeps_the_other_flag() {
    let (mut w, ks, leaves, root) = claimed_one();
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, true)).unwrap();
    w.tx(ADMIN, BRIDGE, &unfreeze(Some(USDC), true, false)).unwrap();
    let t = &w.config().tokens[0];
    assert_eq!((t.mint_frozen, t.burn_frozen), (false, true));
    w.tx(ALICE, BRIDGE, &claim(ks[0], 1, &leaves, root, 0, 100_000_000)).unwrap();
    assert!(burn(&mut w, ALICE).unwrap_err().contains("redemption is frozen"));
    assert_eq!(w.wrapped(ALICE), 200_000_000);
    // Freezing one flag again leaves the other as it is.
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    let t = &w.config().tokens[0];
    assert_eq!((t.mint_frozen, t.burn_frozen), (true, true));
}

#[test]
fn a_freeze_or_unfreeze_naming_nothing_is_refused() {
    let mut w = setup();
    for (who, m) in [
        (FREEZER, freeze(None, false, false)),
        (FREEZER, freeze(Some(USDC), false, false)),
        (ADMIN, freeze(None, false, false)),
        (ADMIN, unfreeze(None, false, false)),
        (ADMIN, unfreeze(Some(USDC), false, false)),
    ] {
        let err = w.tx(who, BRIDGE, &m).unwrap_err();
        assert!(err.contains("name minting, redemption or both"), "{err}");
    }
    let c = w.config();
    assert_eq!((c.mint_frozen, c.burn_frozen, c.tokens[0].mint_frozen, c.tokens[0].burn_frozen), (false, false, false, false));
}

#[test]
fn root_attestation_stops_only_for_a_global_mint_freeze() {
    let mut w = setup();
    w.tx(FREEZER, BRIDGE, &freeze(None, false, true)).unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap();
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: [5; 32] })).unwrap();
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    let err = w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: [6; 32] })).unwrap_err();
    assert!(err.contains("minting is frozen"), "{err}");
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: [6; 32] })).unwrap();
}

// ---------------------------------------------------------------------------------------------
// One-time recipient, float sponsoring and PDA seeds (claim privacy, 2026-11-12).

#[test]
fn a_claim_with_an_opening_mints_to_the_one_time_address_only() {
    let (mut w, ks, leaves, root) = two_deposits();
    let o = recipient_opening(&ks[0]);
    let a = ALICE.blinded(&o);
    assert_eq!(claim_recipient(&ALICE, Some(&o)), a);
    assert_ne!(a, ALICE);
    w.events.clear();
    w.tx_with_openings(ALICE, BRIDGE, &claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some(o)), &[o]).unwrap();
    // The credit reached Alice's own token actor through the alias; nothing sits at A.
    assert_eq!(w.wrapped(ALICE), 100_000_000);
    assert!(w.state(a, TOKEN).is_empty());
    assert_eq!(w.supply(), 100_000_000);
    // The effect, the mint and the credit all name A.
    assert_eq!(
        claim_effect(&w),
        Effect::Claim { eth_token: USDC, denom_index: 0, denom: 100_000_000, to: a, cfg_id: cfg(), one_time: true }
    );
    let mint = Tok::MintBy { to: a, amount: 100_000_000 };
    assert!(w.trace.iter().any(|(_, to, m, _)| *to == Actor::new(DEF, TOKEN) && *m == borsh::to_vec(&mint).unwrap()));
    let credit = Tok::Credit { definition: DEF, amount: 100_000_000 };
    assert!(w.trace.iter().any(|(_, to, m, _)| *to == Actor::new(a, TOKEN) && *m == borsh::to_vec(&credit).unwrap()));
    assert_never_named(&w, ALICE);
    // Sponsoring is off: no native transfer at all.
    assert!(w.trace.iter().all(|(_, to, _, _)| to.program_account_id != NATIVE));
}

#[test]
fn an_opening_does_not_let_another_account_claim() {
    let (mut w, ks, leaves, root) = two_deposits();
    let o = recipient_opening(&ks[0]);
    // Bob has Alice's secret and opening: the commitment binds Alice's real account.
    let m = claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some(o));
    let err = w.tx_with_openings(BOB, BRIDGE, &m, &[o]).unwrap_err();
    assert_eq!(err, CLAIM_REFUSED);
    assert_eq!(message_fault(BOB, &m), Some(ClaimFault::NotInRoot));
    assert_eq!(w.supply(), 0);
    // Alice still claims it.
    w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap();
    assert_eq!(w.wrapped(ALICE), 100_000_000);
}

#[test]
fn without_an_opening_a_claim_mints_to_the_signer_and_draws_nothing() {
    let (mut w, ks, leaves, root) = two_deposits();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 20);
    w.tx(ALICE, BRIDGE, &claim(ks[0], 0, &leaves, root, 0, 100_000_000)).unwrap();
    assert_eq!(
        claim_effect(&w),
        Effect::Claim { eth_token: USDC, denom_index: 0, denom: 100_000_000, to: ALICE, cfg_id: cfg(), one_time: false }
    );
    assert_eq!(w.wrapped(ALICE), 100_000_000);
    assert_eq!((w.float(), w.native(ALICE)), (20, 0));
    assert!(w.trace.iter().all(|(_, to, _, _)| to.program_account_id != NATIVE));
}

#[test]
fn the_float_sponsors_a_one_time_claim_once_per_deposit() {
    let (mut w, ks, leaves, root) = two_deposits();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 20);
    assert_eq!((w.float(), w.native(FUNDER)), (20, 0));
    let o = recipient_opening(&ks[0]);
    let a = ALICE.blinded(&o);
    let m = claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some(o));
    w.events.clear();
    w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap();
    // The float paid A, which is Alice's: her native balance holds it, A holds nothing.
    assert_eq!((w.float(), w.native(ALICE), w.native(a)), (13, 7, 0));
    let custody = native_token::Message::Transfer { to: a, amount: 7 };
    let config = config_address(&BRIDGE);
    assert!(w.trace.iter().any(|(_, to, msg, seeds)| *to == Actor::native_balance(config)
        && *msg == borsh::to_vec(&custody).unwrap()
        && *seeds == BTreeSet::from([config_seed()])));
    assert!(w.trace.iter().any(|(_, to, msg, _)| *to == Actor::native_balance(a)
        && *msg == borsh::to_vec(&native_token::Message::Credit(7)).unwrap()));
    assert_never_named(&w, ALICE);
    // The same deposit again: refused, and the float pays nothing.
    let err = w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap_err();
    assert_eq!(err, CLAIM_REFUSED);
    assert_eq!((w.float(), w.native(ALICE)), (13, 7));
    // A claim refused for another reason (minting frozen) pays nothing either.
    let o2 = recipient_opening(&ks[1]);
    let m2 = claim_to(ks[1], 1, &leaves, root, 1, 1_000_000_000, Some(o2));
    w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    assert!(w.tx_with_openings(ALICE, BRIDGE, &m2, &[o2]).unwrap_err().contains("minting is frozen"));
    assert_eq!((w.float(), w.native(ALICE)), (13, 7));
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    w.tx_with_openings(ALICE, BRIDGE, &m2, &[o2]).unwrap();
    assert_eq!((w.float(), w.native(ALICE)), (6, 14));
    assert_eq!(w.wrapped(ALICE), 1_100_000_000);
    assert_eq!(w.config().tokens[0].claims, vec![1, 1]);
}

#[test]
fn an_empty_float_fails_the_claim_cleanly() {
    let (mut w, ks, leaves, root) = two_deposits();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 5);
    let o = recipient_opening(&ks[0]);
    let m = claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some(o));
    let err = w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap_err();
    assert!(err.contains("holds less than the transferred amount"), "{err}");
    // Nothing minted, nothing counted, the nullifier unspent, the float untouched.
    assert_eq!((w.wrapped(ALICE), w.supply(), w.native(ALICE), w.float()), (0, 0, 0, 5));
    assert_eq!(w.config().tokens[0].claims, vec![0, 0]);
    assert!(w.state(nullifier_address(&BRIDGE, &claim_nullifier(&ks[0], &ALICE, &USDC, 100_000_000)), BRIDGE).is_empty());
    // Topped up, the same claim goes through: the deposit was never spent.
    fund_float(&mut w, 2);
    w.tx_with_openings(ALICE, BRIDGE, &m, &[o]).unwrap();
    assert_eq!((w.wrapped(ALICE), w.native(ALICE), w.float()), (100_000_000, 7, 0));
    // An empty float does not stop a claim without a one-time recipient.
    w.tx(ALICE, BRIDGE, &claim(ks[1], 1, &leaves, root, 1, 1_000_000_000)).unwrap();
    assert_eq!(w.wrapped(ALICE), 1_100_000_000);
}

#[test]
fn only_the_admin_sets_the_sponsor_amount() {
    let (mut w, ks, leaves, root) = two_deposits();
    assert_eq!(w.config().sponsor_amount, 0);
    for who in [ALICE, FREEZER, ATTESTOR] {
        let err = w.tx(who, BRIDGE, &instr(Instruction::Admin(AdminOp::SetSponsor { amount: 9 }))).unwrap_err();
        assert!(err.contains("only the admin"), "{err}");
    }
    assert_eq!(w.config().sponsor_amount, 0);
    set_sponsor(&mut w, 9);
    assert_eq!(w.config().sponsor_amount, 9);
    // Back to 0: a one-time claim draws nothing even from a full float.
    set_sponsor(&mut w, 0);
    fund_float(&mut w, 50);
    let o = recipient_opening(&ks[0]);
    w.tx_with_openings(ALICE, BRIDGE, &claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some(o)), &[o]).unwrap();
    assert_eq!((w.float(), w.native(ALICE), w.wrapped(ALICE)), (50, 0, 100_000_000));
}

#[test]
fn every_effect_carries_the_seed_of_its_pda() {
    let (w, ks, leaves, root) = two_deposits();
    let check = |input: ReceiveInput| {
        let response = bridge_core::handle(&input);
        assert!(!response.sync_messages.is_empty());
        for m in &response.sync_messages {
            if m.to.program_account_id == NATIVE {
                // The float's custody transfer: the config's own native actor, under its seed.
                assert_eq!(m.to, Actor::native_balance(config_address(&BRIDGE)));
                assert_eq!(m.pda_seeds, BTreeSet::from([config_seed()]));
                continue;
            }
            if m.to.program_account_id != BRIDGE {
                continue; // the token program's MintBy, or another program's query
            }
            let seeds: Vec<_> = m.pda_seeds.iter().collect();
            assert_eq!(seeds.len(), 1, "one seed per effect");
            assert_eq!(AccountId::for_public_pda(&BRIDGE, seeds[0]), m.to.account_id, "the seed derives the PDA");
        }
        response
    };
    let root_input = |who: AccountId, m: &Message| ReceiveInput {
        receiver: Actor::new(who, BRIDGE),
        from: None,
        is_authorized: true,
        pre_state: ActorState::empty(),
        message: borsh::to_vec(m).unwrap(),
    };
    // The claimer's actor: AssertRoot, SpendClaim, Claim.
    let r = check(root_input(ALICE, &claim_to(ks[0], 0, &leaves, root, 0, 100_000_000, Some([3; 32]))));
    assert_eq!(r.sync_messages.len(), 3);
    let _ = check(root_input(BRIDGE, &init_with(VAULT, FREEZER, ATTESTOR)));
    let _ = check(root_input(ADMIN, &freeze(None, true, false)));
    let _ = check(root_input(ATTESTOR, &instr(Instruction::AttestRoot { root: [5; 32] })));
    // The config: RecordRoot to a root PDA; a sponsored claim's MintBy and custody transfer.
    let mut c = w.config();
    c.sponsor_amount = 7;
    let config_input = |e: Effect, pre: Vec<u8>| ReceiveInput {
        receiver: Actor::new(config_address(&BRIDGE), BRIDGE),
        from: Some(Actor::new(ALICE, BRIDGE)),
        is_authorized: false,
        pre_state: ActorState::from(pre),
        message: borsh::to_vec(&Message::Effect(e)).unwrap(),
    };
    let _ = check(config_input(Effect::AttestRoot { by: ATTESTOR, root: [5; 32] }, borsh::to_vec(&c).unwrap()));
    let e = Effect::Claim { eth_token: USDC, denom_index: 0, denom: 100_000_000, to: AccountId::new([0x77; 32]), cfg_id: cfg(), one_time: true };
    let r = check(config_input(e, borsh::to_vec(&c).unwrap()));
    assert!(r.sync_messages.iter().any(|m| m.to.program_account_id == NATIVE));
}

#[test]
fn a_new_pda_reached_without_its_seed_is_not_admitted() {
    // The simulator's model of LEZ's `admit`: the same SpendClaim without the seed is refused,
    // with it the nullifier PDA is created.
    let mut w = setup();
    let pda = nullifier_address(&BRIDGE, &claim_nullifier(&[1; 32], &ALICE, &USDC, 100_000_000));
    let spend = borsh::to_vec(&Message::Effect(Effect::SpendClaim)).unwrap();
    let mut asyncs = VecDeque::new();
    let from = Some(Actor::new(ALICE, BRIDGE));
    let err = w.deliver(from, Actor::new(pda, BRIDGE), spend.clone(), BTreeSet::new(), None, &mut asyncs).unwrap_err();
    assert!(err.contains("unadmitted"), "{err}");
    // A seed for another PDA does not admit it either.
    let other = BTreeSet::from([nullifier_seed(&claim_nullifier(&[2; 32], &ALICE, &USDC, 100_000_000))]);
    assert!(w.deliver(from, Actor::new(pda, BRIDGE), spend.clone(), other, None, &mut asyncs).is_err());
    let seed = BTreeSet::from([nullifier_seed(&claim_nullifier(&[1; 32], &ALICE, &USDC, 100_000_000))]);
    w.deliver(from, Actor::new(pda, BRIDGE), spend, seed, None, &mut asyncs).unwrap();
    assert_eq!(w.state(pda, BRIDGE), vec![1]);
}
