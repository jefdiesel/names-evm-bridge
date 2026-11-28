// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native tests of M6's failsafe mint-stop tracking in `outbound_core` (G2):
//! - claims before wind-down succeed and produce `wound_down = []`, `mint_stopped_block = 0`;
//! - `WindDownAttested` / `AttestWindDown` (public and private) puts `GLOBAL` in the state's and the
//!   journal's `wound_down` (kept in later states) and the LEZ block (`block_id`) in the journal of
//!   the batch that holds it (`mint_stopped_block`);
//! - any claim after wind-down (same block, later block, later batch) is refused with
//!   `G2Error::Inconsistent` (closes the failsafe double-dip at G2);
//! - burns after wind-down still extract cleanly against pre-stop claims;
//! - `UpdateJournal` V1 (unchanged) and V2 (10-field failsafe layout) ABI encode/decode strictly.

use bridge_core::{
    AdminOp, AttestSource, EthAddress, GLOBAL, Instruction, Message as BridgeMessage, RootSource,
    VAULT_DEPOSIT_ROOT_SLOT, cfg_id, config_address,
    eth_attest_core::{
        Anchor,
        inclusion::{AccountFields, AccountProof, AnchorWitness, StorageProof},
    },
    source_id,
};
use bridge_token_core::Message as TokenMessage;
use lee_core::{
    RootCall,
    account::{AccountId, Actor},
};
use outbound_core::{
    G2Error,
    journal::{JournalVersion, UpdateJournal},
    view::{At, BatchOut, BridgeView, ClaimDelta, Exit, Extractor},
};

const BRIDGE: AccountId = AccountId::new([0xB0; 32]);
const TOKEN: AccountId = AccountId::new([0x70; 32]);
const ATTEST: AccountId = AccountId::new([0xE7; 32]);
const ADMIN: AccountId = AccountId::new([0xAD; 32]);
const FREEZER: AccountId = AccountId::new([0xF2; 32]);
const ATTESTOR: AccountId = AccountId::new([0xA7; 32]);
const ALICE: AccountId = AccountId::new([0xA1; 32]);
const BOB: AccountId = AccountId::new([0xB1; 32]);
const CAROL: AccountId = AccountId::new([0xC1; 32]);
const DEF: AccountId = AccountId::new([0xDE; 32]);
const USDC: EthAddress = [0xAA; 20];
const VAULT: EthAddress = [0x11; 20];
const EVM_CHAIN: u64 = 11_155_111;
const LEZ_CHAIN: [u8; 32] = [0x4C; 32];
const INSTANCE: [u8; 32] = [0x33; 32];

fn attest_source() -> AttestSource {
    AttestSource { attest_program: ATTEST, attest_instance: INSTANCE, deposit_root_slot: VAULT_DEPOSIT_ROOT_SLOT }
}

fn attested_cfg_id() -> [u8; 32] {
    let zone = source_id(&RootSource::Attest(attest_source()));
    cfg_id(EVM_CHAIN, &VAULT, &LEZ_CHAIN, &zone, &BRIDGE)
}

fn dummy_anchor() -> AnchorWitness {
    AnchorWitness { anchor: Anchor::StateRoot([0x11; 32]), header_rlp: None }
}

fn dummy_proof() -> StorageProof {
    StorageProof {
        account: AccountProof {
            fields: AccountFields { nonce: 0, balance: [0; 32], storage_root: [0; 32], code_hash: [0; 32] },
            nodes: Vec::new(),
        },
        value: [0; 32],
        nodes: Vec::new(),
    }
}

fn root_call(signer: AccountId, i: Instruction) -> RootCall {
    RootCall { to: Actor::new(signer, BRIDGE), message: borsh::to_vec(&BridgeMessage::Instruction(i)).unwrap() }
}

fn init_attested_call() -> RootCall {
    root_call(
        BRIDGE,
        Instruction::InitializeAttested {
            admin: ADMIN,
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            lez_chain_id: LEZ_CHAIN,
            token_program: TOKEN,
            freezer: FREEZER,
            source: attest_source(),
        },
    )
}

fn init_stand_in_call() -> RootCall {
    root_call(
        BRIDGE,
        Instruction::Initialize {
            admin: ADMIN,
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            lez_chain_id: LEZ_CHAIN,
            token_program: TOKEN,
            freezer: FREEZER,
            attestor: ATTESTOR,
        },
    )
}

fn register_usdc_call() -> RootCall {
    root_call(
        ADMIN,
        Instruction::Admin(AdminOp::RegisterToken {
            eth_token: USDC,
            definition: DEF,
            decimals: 6,
            denoms: vec![100_000_000, 1_000_000_000],
        }),
    )
}

fn claim_attested_call(who: AccountId, secret: [u8; 32]) -> RootCall {
    root_call(
        who,
        Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: 0,
            denom: 100_000_000,
            root: [0x22; 32],
            leaf_index: 0,
            path: Vec::new(),
            secret,
            cfg_id: attested_cfg_id(),
            source: attest_source(),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor: dummy_anchor(),
            proof: dummy_proof(),
            recipient_opening: None,
        },
    )
}

fn wind_down_attested_call(who: AccountId) -> RootCall {
    root_call(
        who,
        Instruction::WindDownAttested {
            cfg_id: attested_cfg_id(),
            source: attest_source(),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor: dummy_anchor(),
            proof: dummy_proof(),
        },
    )
}

fn burn_call(who: AccountId, memo: [u8; 32]) -> RootCall {
    RootCall {
        to: Actor::new(who, TOKEN),
        message: borsh::to_vec(&TokenMessage::BurnWithNotice {
            definition: DEF,
            amount: 100_000_000,
            notify: Actor::new(config_address(&BRIDGE), BRIDGE),
            memo,
        })
        .unwrap(),
    }
}

#[test]
fn journal_v1_and_v2_abi_roundtrip() {
    let j = UpdateJournal {
        cfg_id: [1; 32],
        prev_state: [2; 32],
        new_state: [3; 32],
        deposit_roots_used: vec![[4; 32]],
        claims: vec![ClaimDelta { token: USDC, denom_index: 0, count: 1 }],
        exits: vec![Exit { token: USDC, denom_index: 0, commitment: [7; 32] }],
        claim_acc_root: [9; 32],
        claim_acc_count: 1,
        wound_down: vec![GLOBAL],
        mint_stopped_block: 500,
    };

    let b1 = j.abi_encode(JournalVersion::V1);
    let mut d1 = UpdateJournal::abi_decode_v1(&b1).expect("V1 decodes");
    assert_eq!((d1.claim_acc_root, d1.claim_acc_count, d1.wound_down.len(), d1.mint_stopped_block), ([0; 32], 0, 0, 0));
    d1.claim_acc_root = j.claim_acc_root;
    d1.claim_acc_count = j.claim_acc_count;
    d1.wound_down = j.wound_down.clone();
    d1.mint_stopped_block = j.mint_stopped_block;
    assert_eq!(d1, j);

    let b2 = j.abi_encode(JournalVersion::V2);
    // V2 adds 4 head words (claimAccRoot, claimAccCount, offset(woundDown), mintStoppedBlock) + 2 tail words (len + 1 addr)
    assert_eq!(b2.len(), b1.len() + 32 * 4 + 32 * 2);
    // Word 7 (bytes 224..256): claimAccRoot; word 10 (bytes 320..352): mintStoppedBlock
    assert_eq!(&b2[32 * 7..32 * 8], &[9; 32]);
    assert_eq!(u64::from_be_bytes(b2[32 * 10 + 24..32 * 11].try_into().unwrap()), 500);
    assert_eq!(UpdateJournal::abi_decode_v2(&b2), Some(j.clone()));

    // Non-canonical padding in V2 is rejected.
    let mut dirty = b2.clone();
    dirty[32 * 10] = 1;
    assert_eq!(UpdateJournal::abi_decode_v2(&dirty), None);
}

/// `cast abi-encode` of the failsafe vault's `UpdateJournal`; `vault/test/failsafe/Failsafe.t.sol`
/// (`test_V2JournalVectorIsOutboundCores`) pins the same bytes against the vault's struct.
const V2_VECTOR: [&str; 16] = [
    "000000000000000000000000000000000000000000000000000000000000002001010101010101010101010101010101",
    "010101010101010101010101010101010202020202020202020202020202020202020202020202020202020202020202",
    "030303030303030303030303030303030303030303030303030303030303030300000000000000000000000000000000",
    "000000000000000000000000000001400000000000000000000000000000000000000000000000000000000000000180",
    "000000000000000000000000000000000000000000000000000000000000020009090909090909090909090909090909",
    "090909090909090909090909090909090000000000000000000000000000000000000000000000000000000000000003",
    "000000000000000000000000000000000000000000000000000000000000028000000000000000000000000000000000",
    "000000000000000000000000000001f40000000000000000000000000000000000000000000000000000000000000001",
    "040404040404040404040404040404040404040404040404040404040404040400000000000000000000000000000000",
    "00000000000000000000000000000001000000000000000000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000",
    "000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000001",
    "000000000000000000000000aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa00000000000000000000000000000000",
    "000000000000000000000000000000000707070707070707070707070707070707070707070707070707070707070707",
    "0000000000000000000000000000000000000000000000000000000000000001000000000000000000000000ffffffff",
    "ffffffffffffffffffffffffffffffff",
];

#[test]
fn journal_v2_matches_the_vaults_abi() {
    let j = UpdateJournal {
        cfg_id: [1; 32],
        prev_state: [2; 32],
        new_state: [3; 32],
        deposit_roots_used: vec![[4; 32]],
        claims: vec![ClaimDelta { token: USDC, denom_index: 1, count: 2 }],
        exits: vec![Exit { token: USDC, denom_index: 0, commitment: [7; 32] }],
        claim_acc_root: [9; 32],
        claim_acc_count: 3,
        wound_down: vec![GLOBAL],
        mint_stopped_block: 500,
    };
    let hex = V2_VECTOR.concat();
    let want: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
    assert_eq!(j.abi_encode(JournalVersion::V2), want);
    assert_eq!(UpdateJournal::abi_decode_v2(&want), Some(j));
}

#[test]
fn extractor_allows_claim_before_wind_down_and_refuses_claim_after() {
    let mut view = BridgeView::default();
    let mut out = BatchOut::default();

    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 10, tx: 0 }, &init_attested_call()).unwrap();
    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 11, tx: 0 }, &register_usdc_call()).unwrap();

    // Claim before wind-down OK.
    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 12, tx: 0 }, &claim_attested_call(ALICE, [1; 32])).unwrap();
    assert_eq!(out.claims, vec![ClaimDelta { token: USDC, denom_index: 0, count: 1 }]);
    assert!(!out.wound_down);
    assert_eq!(out.mint_stopped_block, 0);
    let view_hash = view.hash();

    // Stand-in wind-down refused on attested config.
    let err = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 13, tx: 0 }, &root_call(ADMIN, Instruction::AttestWindDown))
        .unwrap_err();
    assert!(matches!(err, G2Error::Inconsistent { what, .. } if what.contains("stand-in wind-down")));

    // Attested wind-down at block 15 records block 15; the view (and its hash) does not change.
    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 15, tx: 0 }, &wind_down_attested_call(CAROL)).unwrap();
    assert!(out.wound_down);
    assert_eq!(out.mint_stopped_block, 15);
    assert_eq!(view.hash(), view_hash);

    // Claim after wind-down (same block and later block) is refused.
    let err_same = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 15, tx: 1 }, &claim_attested_call(BOB, [2; 32]))
        .unwrap_err();
    assert!(matches!(err_same, G2Error::Inconsistent { what, .. } if what.contains("after minting was wound down")));

    let err_later = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 16, tx: 0 }, &claim_attested_call(BOB, [2; 32]))
        .unwrap_err();
    assert!(matches!(err_later, G2Error::Inconsistent { what, .. } if what.contains("after minting was wound down")));

    // Second wind-down is refused.
    let err_twice = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 16, tx: 1 }, &wind_down_attested_call(CAROL))
        .unwrap_err();
    assert!(matches!(err_twice, G2Error::Inconsistent { what, .. } if what.contains("wound down twice")));

    // Burn against Alice's pre-wind-down claim still extracts cleanly.
    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 17, tx: 0 }, &burn_call(ALICE, [0xEE; 32])).unwrap();
    assert_eq!(out.exits, vec![Exit { token: USDC, denom_index: 0, commitment: [0xEE; 32] }]);
}

#[test]
fn stand_in_extractor_wind_down_and_source_isolation() {
    let mut view = BridgeView::default();
    let mut out = BatchOut::default();

    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 1, tx: 0 }, &init_stand_in_call()).unwrap();
    Extractor::new(BRIDGE, &mut view, &mut out).root(At { block_id: 2, tx: 0 }, &register_usdc_call()).unwrap();

    // Attested wind-down refused on stand-in config.
    let err = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 3, tx: 0 }, &wind_down_attested_call(CAROL))
        .unwrap_err();
    assert!(matches!(err, G2Error::Inconsistent { what, .. } if what.contains("stand-in config")));

    // Stand-in wind-down at block 4 succeeds and stops subsequent roots and claims.
    Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 4, tx: 0 }, &root_call(ATTESTOR, Instruction::AttestWindDown))
        .unwrap();
    assert!(out.wound_down);
    assert_eq!(out.mint_stopped_block, 4);

    let err = Extractor::new(BRIDGE, &mut view, &mut out)
        .root(At { block_id: 5, tx: 0 }, &root_call(ATTESTOR, Instruction::AttestRoot { root: [0xAA; 32] }))
        .unwrap_err();
    assert!(matches!(err, G2Error::Inconsistent { what, .. } if what.contains("after minting was wound down")));
}

#[cfg(feature = "trusted-tip")]
mod full_update_tests {
    use super::*;
    use borsh::BorshDeserialize as _;
    use bridge_core::{Effect, claim_nullifier, nullifier_address};
    use lee_core::execution_state::BoundaryStep;
    use outbound_core::{
        finality::{BlockRef, KIND_TRUSTED_TIP, TrustedTip},
        state::G2State,
        update::{AcceptAll, HostAcc, NoMeter, UpdateInput, update},
        wire,
    };

    const PUBLIC_TEMPLATE: &[u8] = include_bytes!("../../fixtures/public9/block-2895.bin");
    const PRIVATE_TEMPLATE: &[u8] = include_bytes!("../../fixtures/private/block-4998.bin");

    fn public_tx(root: RootCall) -> wire::LeeTransaction {
        let mut block = wire::Block::try_from_slice(PUBLIC_TEMPLATE).unwrap();
        let tx = block
            .body
            .transactions
            .drain(..)
            .find_map(|t| match t {
                wire::LeeTransaction::Public(mut p) => {
                    p.message.execution.root = root.clone();
                    Some(wire::LeeTransaction::Public(p))
                }
                _ => None,
            })
            .expect("public fixture tx");
        tx
    }

    fn private_effect_tx(effect: Effect) -> wire::LeeTransaction {
        let mut block = wire::Block::try_from_slice(PRIVATE_TEMPLATE).unwrap();
        let mut p = block
            .body
            .transactions
            .drain(..)
            .find_map(|t| match t {
                wire::LeeTransaction::PrivacyPreserving(p) => Some(p),
                _ => None,
            })
            .expect("private fixture tx");
        p.message.execution.public_root = None;
        let mut step = p
            .message
            .execution
            .boundary
            .drain(..)
            .find(|s| matches!(s, BoundaryStep::PrivateToPublic(_)))
            .expect("boundary step");
        if let BoundaryStep::PrivateToPublic(ref mut d) = step {
            d.envelope.from = Actor::new(CAROL, BRIDGE);
            d.envelope.to = Actor::new(config_address(&BRIDGE), BRIDGE);
            d.envelope.message = borsh::to_vec(&BridgeMessage::Effect(effect)).unwrap();
        }
        p.message.execution.boundary.push(step);
        wire::LeeTransaction::PrivacyPreserving(p)
    }

    fn seal_block(block_id: u64, prev_hash: [u8; 32], transactions: Vec<wire::LeeTransaction>) -> (Vec<u8>, BlockRef) {
        let mut b = wire::Block {
            header: wire::BlockHeader { block_id, prev_block_hash: prev_hash, hash: [0; 32], timestamp: 1_700_000_000 + block_id },
            body: wire::BlockBody { transactions, reverted: Vec::new() },
            bedrock_status: wire::BedrockStatus::Finalized,
        };
        let raw0 = borsh::to_vec(&b).unwrap();
        let hash = wire::block_hash(&raw0).unwrap();
        b.header.hash = hash;
        let raw = borsh::to_vec(&b).unwrap();
        (raw, BlockRef { id: block_id, hash })
    }

    #[test]
    fn g2_update_tracks_mint_stop_and_refuses_late_claims() {
        let genesis_tip = BlockRef { id: 100, hash: [0x10; 32] };
        let g0 = G2State::genesis(KIND_TRUSTED_TIP, *BRIDGE.value(), genesis_tip);
        let mut acc = HostAcc { tree: claim_acc::Tree::new(), witnesses: Vec::new() };

        // Batch 1 (blocks 101..=103): init, register, Alice claims -> no stop proof yet.
        let (b101, r101) = seal_block(101, genesis_tip.hash, vec![public_tx(init_attested_call())]);
        let (b102, r102) = seal_block(102, r101.hash, vec![public_tx(register_usdc_call())]);
        let (b103, r103) = seal_block(103, r102.hash, vec![public_tx(claim_attested_call(ALICE, [1; 32]))]);

        let out1 = update::<TrustedTip>(
            &UpdateInput {
                prev: g0.clone(),
                view: BridgeView::default(),
                finality: borsh::to_vec(&r103).unwrap(),
                blocks: vec![&b101, &b102, &b103],
            },
            &mut acc,
            &mut AcceptAll,
            &NoMeter,
        )
        .unwrap();

        assert!(out1.journal.wound_down.is_empty());
        assert_eq!(out1.journal.mint_stopped_block, 0);
        assert!(out1.next.wound_down.is_empty());
        assert_eq!(out1.journal.claims, vec![ClaimDelta { token: USDC, denom_index: 0, count: 1 }]);
        let alice_nf = claim_nullifier(&[1; 32], &ALICE, &USDC, 100_000_000);
        assert!(acc.tree.contains(nullifier_address(&BRIDGE, &alice_nf).value()));

        // Batch 2 (block 104): WindDownAttested lands -> journal V2 carries GLOBAL + mint_stopped_block = 104.
        let (b104, r104) = seal_block(104, r103.hash, vec![public_tx(wind_down_attested_call(CAROL))]);
        let out2 = update::<TrustedTip>(
            &UpdateInput {
                prev: out1.next.clone(),
                view: out1.view.clone(),
                finality: borsh::to_vec(&r104).unwrap(),
                blocks: vec![&b104],
            },
            &mut acc,
            &mut AcceptAll,
            &NoMeter,
        )
        .unwrap();

        assert_eq!(out2.journal.wound_down, vec![GLOBAL]);
        assert_eq!(out2.journal.mint_stopped_block, 104);
        assert_eq!(out2.next.wound_down, vec![GLOBAL]);
        assert_eq!(out2.next.view_hash, out1.next.view_hash, "the wind-down leaves the view as it was");
        assert_ne!(out2.next.commitment(), out1.next.commitment());
        let encoded_v2 = out2.journal.abi_encode(JournalVersion::V2);
        assert_eq!(UpdateJournal::abi_decode_v2(&encoded_v2), Some(out2.journal.clone()));

        // Batch 3a (block 105): a malicious sequencer includes Bob's claim AFTER block 104 -> REFUSED by G2,
        // in a later batch than the wind-down (seeded from the previous state's `wound_down`).
        let (b105_bad, r105_bad) = seal_block(105, r104.hash, vec![public_tx(claim_attested_call(BOB, [2; 32]))]);
        let err = update::<TrustedTip>(
            &UpdateInput {
                prev: out2.next.clone(),
                view: out2.view.clone(),
                finality: borsh::to_vec(&r105_bad).unwrap(),
                blocks: vec![&b105_bad],
            },
            &mut acc,
            &mut AcceptAll,
            &NoMeter,
        )
        .err()
        .expect("late claim must be refused by G2");
        assert!(matches!(err, G2Error::Inconsistent { block_id: 105, what, .. } if what.contains("after minting was wound down")));

        // Batch 3b (block 105): Alice's honest burn after wind-down succeeds; GLOBAL stays, no stop block.
        let (b105_ok, r105_ok) = seal_block(105, r104.hash, vec![public_tx(burn_call(ALICE, [0xAA; 32]))]);
        let out3 = update::<TrustedTip>(
            &UpdateInput {
                prev: out2.next.clone(),
                view: out2.view.clone(),
                finality: borsh::to_vec(&r105_ok).unwrap(),
                blocks: vec![&b105_ok],
            },
            &mut acc,
            &mut AcceptAll,
            &NoMeter,
        )
        .unwrap();
        assert_eq!(out3.journal.wound_down, vec![GLOBAL]);
        assert_eq!(out3.next.wound_down, vec![GLOBAL]);
        assert_eq!(out3.journal.mint_stopped_block, 0);
        assert!(out3.journal.claims.is_empty());
        assert_eq!(out3.journal.exits, vec![Exit { token: USDC, denom_index: 0, commitment: [0xAA; 32] }]);
    }

    #[test]
    fn g2_update_extracts_private_wind_down_effect() {
        let genesis_tip = BlockRef { id: 200, hash: [0x20; 32] };
        let g0 = G2State::genesis(KIND_TRUSTED_TIP, *BRIDGE.value(), genesis_tip);
        let mut acc = HostAcc { tree: claim_acc::Tree::new(), witnesses: Vec::new() };

        let (b201, r201) = seal_block(201, genesis_tip.hash, vec![public_tx(init_attested_call())]);
        let (b202, r202) = seal_block(202, r201.hash, vec![public_tx(register_usdc_call())]);
        let priv_wd = private_effect_tx(Effect::WindDownAttested {
            cfg_id: attested_cfg_id(),
            source: attest_source(),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
        });
        let (b203, r203) = seal_block(203, r202.hash, vec![priv_wd]);

        let out = update::<TrustedTip>(
            &UpdateInput {
                prev: g0,
                view: BridgeView::default(),
                finality: borsh::to_vec(&r203).unwrap(),
                blocks: vec![&b201, &b202, &b203],
            },
            &mut acc,
            &mut AcceptAll,
            &NoMeter,
        )
        .unwrap();

        assert_eq!(out.journal.wound_down, vec![GLOBAL]);
        assert_eq!(out.journal.mint_stopped_block, 203);
        assert_eq!(out.stats.private_verified, 1);
    }
}
