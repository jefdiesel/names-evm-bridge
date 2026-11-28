// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whole-transaction tests of M6, the failsafe wind-down on LEZ:
//! - a claim before the wind-down mints;
//! - `WindDownAttested` (anyone) proves `BridgeVaultFailsafe`'s `WIND_DOWN_SLOT` non-zero under a
//!   finalized anchor and stops minting for good;
//! - a claim after it is refused with `MINT_WOUND_DOWN`, also after `AdminOp::Unfreeze`;
//! - burns of claimed tokens still go through;
//! - bad proofs (slot zero, a forged value, another contract, an unfinalized anchor, wrong
//!   cfg/program/chain) and a second wind-down are refused;
//! - the stand-in `AttestWindDown` (attestor only) on a stand-in config; the sources don't mix;
//! - a config written before the field existed decodes and re-encodes byte for byte.

mod common;

use common::*;

/// Sets the vault's wind-down slot to 1 (what `windDown()` writes), plants both a state-root and
/// a block-hash anchor finalized by the fixture's instance; returns `(state_anchor, block_anchor, header)`.
fn plant_vault_wind_down(f: &mut Fixture) -> (Anchor, Anchor, Vec<u8>) {
    f.state.accounts[0].storage.push((VAULT_WIND_DOWN_SLOT, word(1)));
    let state_anchor = Anchor::StateRoot(f.state.state_root());
    plant_anchor(&mut f.w, &f.id, EVM_CHAIN, state_anchor);
    let header = header_rlp(f.state.state_root(), [0; 32], 12_000_100, 0);
    let block_anchor = Anchor::BlockHash(keccak(&header));
    plant_anchor(&mut f.w, &f.id, EVM_CHAIN, block_anchor);
    (state_anchor, block_anchor, header)
}

fn wind_down_attested(f: &Fixture, anchor: AnchorWitness) -> Instruction {
    Instruction::WindDownAttested {
        cfg_id: acfg(),
        source: source(f.id),
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        anchor,
        proof: f.state.storage_proof(&VAULT, &VAULT_WIND_DOWN_SLOT),
    }
}

#[test]
fn the_wind_down_slot_is_the_vaults() {
    // vault/test/failsafe/Failsafe.t.sol `test_WindDownSlotIsSetOnlyByWindDown` checks the same literal
    assert_eq!(keccak(VAULT_WIND_DOWN_TAG), VAULT_WIND_DOWN_SLOT);
}

#[test]
fn configs_before_the_field_decode_and_reencode_unchanged() {
    let f = attested_setup();
    let mut c = f.w.config();
    assert_eq!(c.mint_wound_down, WoundDown(false));
    let open = borsh::to_vec(&c).unwrap();
    // the stored state is that encoding: nothing for the flag
    assert_eq!(f.w.state(config_address(&BRIDGE), BRIDGE), open);
    c.mint_wound_down = WoundDown(true);
    let shut = borsh::to_vec(&c).unwrap();
    assert_eq!(shut.len(), open.len() + 1);
    assert_eq!(&shut[..open.len()], &open[..]);
    assert_eq!(shut[open.len()], 1);
    assert_eq!(borsh::from_slice::<Config>(&shut).unwrap(), c);
    // any other trailing byte, or two, does not decode
    let mut bad = open.clone();
    bad.push(2);
    assert!(borsh::from_slice::<Config>(&bad).is_err());
    let mut two = shut.clone();
    two.push(1);
    assert!(borsh::from_slice::<Config>(&two).is_err());
}

#[test]
fn mint_before_wind_down_ok_and_mint_after_wind_down_refused() {
    let mut f = attested_setup();
    assert!(!f.w.config().mint_wound_down.0);

    // 1. A claim before the wind-down mints (Alice, 100 USDC). Bob's claim is made now, under the
    //    fixture's anchor (planting the wind-down slot changes the synthetic state root).
    f.tx(ALICE, f.claim(0)).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
    let bob_claim = f.claim(1);

    // 2. The vault winds down at a finalized Ethereum block; Carol (no bridge role) proves it.
    let (wd_anchor, _, _) = plant_vault_wind_down(&mut f);
    f.tx(CAROL, wind_down_attested(&f, AnchorWitness { anchor: wd_anchor, header_rlp: None })).unwrap();
    assert!(f.w.config().mint_wound_down.0);
    let ev = f
        .w
        .emitted
        .iter()
        .find_map(|(_, e)| borsh::from_slice::<MintWoundDown>(&e.data).ok())
        .expect("MintWoundDown event");
    assert_eq!(ev.cfg_id, acfg());

    // 3. A valid claim after it is refused; Bob's nullifier stays unspent.
    err_is(f.tx(BOB, bob_claim.clone()), MINT_WOUND_DOWN);
    assert_eq!(f.w.wrapped(BOB), 0);
    let bob_nf = claim_nullifier(&f.keys[1], &BOB, &USDC, 1_000_000_000);
    assert!(f.w.state(nullifier_address(&BRIDGE, &bob_nf), BRIDGE).is_empty());

    // 4. No admin op reopens minting.
    f.w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    f.w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    assert!(!f.w.config().mint_frozen);
    assert!(f.w.config().mint_wound_down.0);
    err_is(f.tx(BOB, bob_claim), MINT_WOUND_DOWN);

    // 5. A second wind-down is refused.
    err_is(f.tx(CAROL, wind_down_attested(&f, AnchorWitness { anchor: wd_anchor, header_rlp: None })), ALREADY_WOUND_DOWN);

    // 6. Burns of claimed tokens still go through.
    burn(&mut f.w, ALICE).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 0);
    assert_eq!(f.w.config().tokens[0].burns, vec![1, 0]);
}

#[test]
fn wind_down_attested_works_with_block_hash_anchor() {
    let mut f = attested_setup();
    let alice_claim = f.claim(0);
    let (_, block_anchor, header) = plant_vault_wind_down(&mut f);
    f.tx(CAROL, wind_down_attested(&f, AnchorWitness { anchor: block_anchor, header_rlp: Some(header) })).unwrap();
    assert!(f.w.config().mint_wound_down.0);
    err_is(f.tx(ALICE, alice_claim), MINT_WOUND_DOWN);
}

#[test]
fn wind_down_attested_refuses_zero_slot_unfinalized_anchor_and_wrong_source() {
    let mut f = attested_setup();
    let alice_claim = f.claim(0);

    // 1. The slot is zero (the vault has not wound down): its exclusion proof is refused.
    let zero_wd = wind_down_attested(&f, AnchorWitness { anchor: f.state_anchor, header_rlp: None });
    err_is(f.tx(CAROL, zero_wd.clone()), WIND_DOWN_REFUSED);
    // so is the same proof claiming a non-zero value
    let mut liar = zero_wd;
    if let Instruction::WindDownAttested { proof, .. } = &mut liar {
        proof.value = word(1);
    }
    err_is(f.tx(CAROL, liar), WIND_DOWN_REFUSED);
    assert!(!f.w.config().mint_wound_down.0);

    // 2. The slot set in ANOTHER contract (`OTHER`).
    let mut other_state = f.state.clone();
    other_state.accounts[1].storage.push((VAULT_WIND_DOWN_SLOT, word(1)));
    let other_anchor = Anchor::StateRoot(other_state.state_root());
    plant_anchor(&mut f.w, &f.id, EVM_CHAIN, other_anchor);
    // naming OTHER as the vault: the config refuses it
    let other_wd = Instruction::WindDownAttested {
        cfg_id: acfg(),
        source: source(f.id),
        vault: OTHER,
        evm_chain_id: EVM_CHAIN,
        anchor: AnchorWitness { anchor: other_anchor, header_rlp: None },
        proof: other_state.storage_proof(&OTHER, &VAULT_WIND_DOWN_SLOT),
    };
    err_contains(f.tx(CAROL, other_wd), "claim for another attestation source");
    // OTHER's proof presented as VAULT's: the proof check refuses it
    let spoof_wd = Instruction::WindDownAttested {
        cfg_id: acfg(),
        source: source(f.id),
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        anchor: AnchorWitness { anchor: other_anchor, header_rlp: None },
        proof: other_state.storage_proof(&OTHER, &VAULT_WIND_DOWN_SLOT),
    };
    err_is(f.tx(CAROL, spoof_wd), WIND_DOWN_REFUSED);

    // 3. The slot set on VAULT, under an anchor the instance never finalized.
    let mut unfin_state = f.state.clone();
    unfin_state.accounts[0].storage.push((VAULT_WIND_DOWN_SLOT, word(1)));
    let unfin_anchor = Anchor::StateRoot(unfin_state.state_root());
    let unfin_wd = Instruction::WindDownAttested {
        cfg_id: acfg(),
        source: source(f.id),
        vault: VAULT,
        evm_chain_id: EVM_CHAIN,
        anchor: AnchorWitness { anchor: unfin_anchor, header_rlp: None },
        proof: unfin_state.storage_proof(&VAULT, &VAULT_WIND_DOWN_SLOT),
    };
    err_contains(f.tx(CAROL, unfin_wd), "anchor not finalized by this instance");

    // 4. A good proof with a wrong cfg id, a fake attestation program (`YES`) or another chain.
    let (wd_anchor, _, _) = plant_vault_wind_down(&mut f);
    let good = wind_down_attested(&f, AnchorWitness { anchor: wd_anchor, header_rlp: None });

    let mut bad_cfg = good.clone();
    if let Instruction::WindDownAttested { cfg_id, .. } = &mut bad_cfg {
        *cfg_id = [0x99; 32];
    }
    err_contains(f.tx(CAROL, bad_cfg), "wind-down for another deployment");

    let mut bad_prog = good.clone();
    if let Instruction::WindDownAttested { source, .. } = &mut bad_prog {
        source.attest_program = YES;
    }
    err_contains(f.tx(CAROL, bad_prog), "claim for another attestation source");

    let mut bad_chain = good.clone();
    if let Instruction::WindDownAttested { evm_chain_id, .. } = &mut bad_chain {
        *evm_chain_id = 1;
    }
    err_contains(f.tx(CAROL, bad_chain), "anchor is for another chain");

    // Minting is still open after every refused attempt.
    assert!(!f.w.config().mint_wound_down.0);
    f.tx(ALICE, alice_claim).unwrap();
    assert_eq!(f.w.wrapped(ALICE), 100_000_000);
    // and the good one works
    f.tx(CAROL, good).unwrap();
    assert!(f.w.config().mint_wound_down.0);
}

#[test]
fn stand_in_wind_down_and_source_isolation() {
    // 1. Stand-in config: a claim before; only the attestor winds down; then claims and
    //    AttestRoot are refused, also after an Unfreeze; burns go through.
    let (mut w, ks, leaves, root) = two_deposits();
    w.tx(ALICE, BRIDGE, &claim(ks[0], 0, &leaves, root, 0, 100_000_000)).unwrap();
    err_contains(w.tx(CAROL, BRIDGE, &instr(Instruction::AttestWindDown)), "only the attestor winds down");
    err_contains(w.tx(ADMIN, BRIDGE, &instr(Instruction::AttestWindDown)), "only the attestor winds down");
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestWindDown)).unwrap();
    assert!(w.config().mint_wound_down.0);
    err_is(w.tx(ALICE, BRIDGE, &claim(ks[1], 1, &leaves, root, 1, 1_000_000_000)), MINT_WOUND_DOWN);
    err_is(w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: [0xEE; 32] })), MINT_WOUND_DOWN);
    w.tx(ADMIN, BRIDGE, &unfreeze(None, true, false)).unwrap();
    err_is(w.tx(ALICE, BRIDGE, &claim(ks[1], 1, &leaves, root, 1, 1_000_000_000)), MINT_WOUND_DOWN);
    err_is(w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestWindDown)), ALREADY_WOUND_DOWN);
    burn(&mut w, ALICE).unwrap();
    assert_eq!(w.wrapped(ALICE), 0);

    // 2. An attested config refuses the stand-in form.
    let mut f = attested_setup();
    err_contains(f.w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestWindDown)), "not the stand-in");
    assert!(!f.w.config().mint_wound_down.0);
}
