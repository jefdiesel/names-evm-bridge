// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reference consumers (`attest/consumer`, branch public-update), natively: an ERC-20 transfer
//! to a given contract and a native ETH transfer to a given contract (its `Deposit` event), each
//! claimed once under an anchor of an attestation instance. Synthetic receipts and header here
//! (the anchor record is written straight into the simulator, as in `instance.rs`); the real-data
//! run on a chain is `public-demo/prove/statements-20261117/`.

mod common;
use common::*;
use eth_consumer::{Claimed, Config, DEPOSIT_TOPIC, Kind, Msg, TRANSFER_TOPIC, nullifier_address};

const TOKEN: [u8; 20] = [0x70; 20];
const VAULT: [u8; 20] = [0x5A; 20];
const WETH: [u8; 20] = [0xEE; 20];

fn addr_word(a: [u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(&a);
    w
}

fn transfer(token: [u8; 20], from: [u8; 20], to: [u8; 20], amount: u128) -> Log {
    Log { address: token, topics: vec![TRANSFER_TOPIC, addr_word(from), addr_word(to)], data: word(amount).to_vec() }
}

fn deposit(contract: [u8; 20], dst: [u8; 20], amount: u128) -> Log {
    Log { address: contract, topics: vec![DEPOSIT_TOPIC, addr_word(dst)], data: word(amount).to_vec() }
}

struct Setup {
    w: World,
    n: Net,
    witness: AnchorWitness,
    receipts: Vec<Vec<u8>>,
}

/// An instance, and a finalized header whose block has these receipts.
fn setup() -> Setup {
    let mut w = World::default();
    let n = sepolia(&mut w);
    let receipts = vec![
        receipt(true, &[transfer(TOKEN, [0xA1; 20], VAULT, 5_000_000)]),
        receipt(true, &[deposit(WETH, [0xB2; 20], 10u128.pow(16)), transfer(TOKEN, [0xA1; 20], [0x99; 20], 7)]),
        receipt(true, &[deposit(WETH, [0xB2; 20], 0)]),
    ];
    let (root, _) = receipts_trie(&receipts, 0);
    let header = header_rlp([0x77; 32], root, 99, 0);
    let anchor = Anchor::BlockHash(alloy_primitives::keccak256(&header).0);
    w.states.insert(
        (anchor_address(&ATTEST, &n.id, &anchor), ATTEST),
        borsh::to_vec(&AnchorRecord { instance_id: n.id, evm_chain_id: n.cfg.evm_chain_id, anchor, beacon_slot: 1, beacon_root: [0; 32], block_number: None }).unwrap(),
    );
    Setup { w, n, witness: AnchorWitness { anchor, header_rlp: Some(header) }, receipts }
}

fn deployment(s: &mut Setup, chain: u64, kind: Kind) -> Config {
    let config = Config { attest: ATTEST, instance_id: s.n.id, evm_chain_id: chain, kind };
    s.w.tx(ALICE, CONSUMER, &Msg::Init { config }).unwrap();
    config
}

fn claim(s: &mut Setup, who: AccountId, config: Config, tx: u64, log_index: u64) -> Result<(), String> {
    let (_, proof) = receipts_trie(&s.receipts, tx);
    s.w.tx(who, CONSUMER, &Msg::Claim { config, witness: s.witness.clone(), proof, log_index })
}

#[test]
fn an_erc20_transfer_to_the_contract_is_claimed_once() {
    let mut s = setup();
    let sepolia = s.n.cfg.evm_chain_id;
    let c = deployment(&mut s, sepolia, Kind::Erc20 { token: TOKEN, recipient: VAULT });
    claim(&mut s, BOB, c, 0, 0).unwrap();
    let got: Claimed = borsh::from_slice(&s.w.state(BOB, CONSUMER)).unwrap();
    assert_eq!((got.sender, got.amount, got.recipient), ([0xA1; 20], word(5_000_000), VAULT));
    assert!(!s.w.state(nullifier_address(&CONSUMER, &c, &got.statement_id), CONSUMER).is_empty());
    // The same log again, by anyone: refused.
    err_contains(claim(&mut s, CAROL, c, 0, 0), "already claimed");
    // A transfer of the same token to someone else, and a Deposit log: not this deployment's.
    err_contains(claim(&mut s, BOB, c, 1, 1), "not the configured transfer");
    err_contains(claim(&mut s, BOB, c, 1, 0), "not the configured transfer");
    // A configuration nobody deployed (another recipient) is refused by the config check.
    let made_up = Config { kind: Kind::Erc20 { token: TOKEN, recipient: [0x99; 20] }, ..c };
    err_contains(claim(&mut s, BOB, made_up, 1, 1), "no such deployment");
}

#[test]
fn a_native_eth_transfer_through_the_deposit_event_is_claimed_once() {
    let mut s = setup();
    let sepolia = s.n.cfg.evm_chain_id;
    let c = deployment(&mut s, sepolia, Kind::Eth { contract: WETH });
    claim(&mut s, BOB, c, 1, 0).unwrap();
    let got: Claimed = borsh::from_slice(&s.w.state(BOB, CONSUMER)).unwrap();
    assert_eq!((got.sender, got.amount, got.recipient), ([0xB2; 20], word(10u128.pow(16)), WETH));
    err_contains(claim(&mut s, BOB, c, 1, 0), "already claimed");
    // A zero-value deposit and an ERC-20 transfer are not ETH transfers to the contract.
    err_contains(claim(&mut s, BOB, c, 2, 0), "not the configured transfer");
    err_contains(claim(&mut s, BOB, c, 0, 0), "not the configured transfer");
}

#[test]
fn a_deployment_for_another_chain_is_refused_by_the_attestation() {
    let mut s = setup();
    let c = deployment(&mut s, 1, Kind::Erc20 { token: TOKEN, recipient: VAULT });
    // The statement is built for chain 1, but the anchor is the Sepolia instance's.
    err_contains(claim(&mut s, BOB, c, 0, 0), "anchor is for another chain");
    // An anchor the instance never finalized (another header) is refused too.
    let mut s2 = setup();
    let sepolia = s2.n.cfg.evm_chain_id;
    let c2 = deployment(&mut s2, sepolia, Kind::Erc20 { token: TOKEN, recipient: VAULT });
    let other = header_rlp([0x78; 32], [0; 32], 100, 1);
    s2.witness = AnchorWitness { anchor: Anchor::BlockHash(alloy_primitives::keccak256(&other).0), header_rlp: Some(other) };
    err_contains(claim(&mut s2, BOB, c2, 0, 0), "receipt proof invalid");
    // A deployment exists once.
    err_contains(s2.w.tx(CAROL, CONSUMER, &Msg::Init { config: c2 }), "this deployment exists");
}
