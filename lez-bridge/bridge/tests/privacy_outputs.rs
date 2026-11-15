//! What a claim and a burn publish (audit B4: RFP-022 Privacy #3, RFP-021 Functionality #8) and
//! whether refusals say which deposit they were about (audit B5: RFP-022 Privacy #5).
//!
//! # What these tests can and cannot show
//!
//! The simulator (`common/mod.rs`) has no private accounts: the claimer's (or burner's) own
//! actor states sit in the same map as every public account, and it has no sender presentation:
//! the `from` of every message is the canonical actor, where LEZ shows a private sender under a
//! fresh blinded id. So "public output" here is a model, made of exactly:
//! - every message delivered after the transaction's own root instruction (the root instruction
//!   is the claimer's private witness): its bytes, its envelope `to` (an alias stays an alias, as
//!   LEZ journals it) and its PDA seeds, but **not** its `from`;
//! - every event, and which actor emitted it;
//! - the new state of every changed account, except the claimer's/burner's own actors, which are
//!   named in each test and would be private notes in LEZ.
//!
//! The tests check that this set equals an exact allowlist and that no 32-byte (20-byte for an
//! Ethereum address) window of it equals a secret. They do not show that LEZ's private execution
//! hides the root instruction, the claimer's own states or the sender: that rests on LEZ, and on
//! the proving run the README describes (grep the blocks for `V`, `F` and, as the positive
//! control, `A`).

mod common;

use common::*;

const D100: u128 = 100_000_000;

/// One labelled public blob.
type Blob = (String, Vec<u8>);

/// The public outputs of the last transaction in `w` (see the crate docs), given the world
/// `before` it and the actor states that would be private.
fn public_outputs(before: &World, w: &World, private: &[(AccountId, AccountId)]) -> Vec<Blob> {
    let mut out = Vec::new();
    assert!(w.trace[0].0.is_none(), "the trace starts with the transaction's own instruction");
    for (i, (_, to, m, seeds)) in w.trace.iter().enumerate().skip(1) {
        out.push((format!("message {i} bytes"), m.clone()));
        out.push((format!("message {i} to"), to.account_id.value().to_vec()));
        for s in seeds {
            out.push((format!("message {i} seed"), s.as_bytes().to_vec()));
        }
    }
    for (actor, ev) in &w.emitted {
        out.push((format!("event of {:?}", actor.account_id), [&ev.selector[..], &ev.data[..]].concat()));
    }
    for key in changed(before, w) {
        if !private.contains(&key) {
            out.push((format!("state of {:?}", key.0), w.state(key.0, key.1)));
        }
    }
    out
}

fn changed(before: &World, w: &World) -> BTreeSet<(AccountId, AccountId)> {
    let keys: BTreeSet<_> = before.states.keys().chain(w.states.keys()).copied().collect();
    keys.into_iter().filter(|k| before.states.get(k) != w.states.get(k)).collect()
}

/// Where `needle` appears in the public blobs (any window of its length), if anywhere.
fn find(blobs: &[Blob], needle: &[u8]) -> Option<String> {
    blobs.iter().find(|(_, b)| b.windows(needle.len()).any(|x| x == needle)).map(|(l, _)| l.clone())
}

#[track_caller]
fn absent(blobs: &[Blob], what: &str, needle: &[u8]) {
    assert!(needle.len() >= 20, "{what}: too short to scan for");
    if let Some(at) = find(blobs, needle) {
        panic!("{what} is public: found in {at}");
    }
}

/// A Merkle-Patricia node is secret as a whole and in every 32-byte window.
#[track_caller]
fn node_absent(blobs: &[Blob], what: &str, node: &[u8]) {
    if node.len() >= 20 {
        absent(blobs, what, node);
    }
    for (i, w) in node.windows(32).enumerate() {
        absent(blobs, &format!("{what}[{i}..{}]", i + 32), w);
    }
}

fn padded(a: &[u8; 20]) -> [u8; 32] {
    let mut o = [0u8; 32];
    o[12..].copy_from_slice(a);
    o
}

/// (to, message) of every message after the root, in delivery order.
fn deliveries(w: &World) -> Vec<(Actor, Vec<u8>, BTreeSet<PdaSeed>)> {
    w.trace[1..].iter().map(|(_, to, m, s)| (*to, m.clone(), s.clone())).collect()
}

fn bytes<M: borsh::BorshSerialize>(m: &M) -> Vec<u8> {
    borsh::to_vec(m).unwrap()
}

fn seeds(s: &[PdaSeed]) -> BTreeSet<PdaSeed> {
    s.iter().copied().collect()
}

// ---------------------------------------------------------------------------------------------
// B4: the full public-output allowlist.

/// A stand-in world where Alice's deposit (hash-derived secret) sits at index 1 of a three-leaf
/// tree, so every level-0 and level-1 sibling of her path is a real hash, never zero padding.
struct StandIn {
    w: World,
    k: [u8; 32],
    leaves: Vec<[u8; 32]>,
    root: [u8; 32],
}

fn stand_in() -> StandIn {
    let mut w = setup();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 100);
    let k = secret("b4", 1);
    let leaves = vec![
        leaf_for(&cfg(), &secret("b4", 0), &BOB, &USDC, D100),
        leaf_for(&cfg(), &k, &ALICE, &USDC, D100),
        leaf_for(&cfg(), &secret("b4", 2), &CAROL, &USDC, D100),
    ];
    let root = root_of(&leaves);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    StandIn { w, k, leaves, root }
}

/// The secrets of Alice's deposit `k` at `idx` of `leaves`, labelled.
fn claim_secrets(k: &[u8; 32], leaves: &[[u8; 32]], idx: usize) -> Vec<(String, Vec<u8>)> {
    let c = deposit_commitment(k, &ALICE, &USDC, D100, &cfg());
    let mut s = vec![
        ("the secret k".to_owned(), k.to_vec()),
        ("the commitment C".to_owned(), c.to_vec()),
        ("the leaf".to_owned(), leaves[idx].to_vec()),
        ("the real recipient V".to_owned(), ALICE.value().to_vec()),
        ("the opening o".to_owned(), recipient_opening(k).to_vec()),
    ];
    for (h, p) in merkle_path(leaves, idx).iter().enumerate() {
        s.push((format!("path element {h}"), p.to_vec()));
    }
    s
}

#[test]
fn a_stand_in_claim_publishes_exactly_the_allowlist() {
    let StandIn { mut w, k, leaves, root } = stand_in();
    let o = recipient_opening(&k);
    let a = ALICE.blinded(&o);
    let nf = claim_nullifier(&k, &ALICE, &USDC, D100);
    let before = w.clone();
    w.tx_with_openings(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 1, &leaves, 0, D100, Some(o)), &[o]).unwrap();
    let config = config_address(&BRIDGE);
    // Changed accounts: the nullifier PDA (new), the config (counters), the definition (supply),
    // the float; and Alice's own token and native actors, which would be private.
    let private = [(ALICE, TOKEN), (ALICE, NATIVE)];
    let want: BTreeSet<_> = [(nullifier_address(&BRIDGE, &nf), BRIDGE), (config, BRIDGE), (DEF, TOKEN), (config, NATIVE)]
        .into_iter()
        .chain(private)
        .collect();
    assert_eq!(changed(&before, &w), want);
    // Messages, exactly and in order.
    assert_eq!(
        deliveries(&w),
        vec![
            (Actor::new(root_address(&BRIDGE, &root), BRIDGE), bytes(&Message::Effect(Effect::AssertRoot)), seeds(&[root_seed(&root)])),
            (Actor::new(nullifier_address(&BRIDGE, &nf), BRIDGE), bytes(&Message::Effect(Effect::SpendClaim)), seeds(&[nullifier_seed(&nf)])),
            (
                Actor::new(config, BRIDGE),
                bytes(&Message::Effect(Effect::Claim { eth_token: USDC, denom_index: 0, denom: D100, to: a, cfg_id: cfg(), one_time: true })),
                seeds(&[config_seed()])
            ),
            (Actor::new(DEF, TOKEN), bytes(&Tok::MintBy { to: a, amount: D100 }), seeds(&[])),
            (Actor::native_balance(config), bytes(&native_token::Message::Transfer { to: a, amount: 7 }), seeds(&[config_seed()])),
            (Actor::new(a, TOKEN), bytes(&Tok::Credit { definition: DEF, amount: D100 }), seeds(&[])),
            (Actor::native_balance(a), bytes(&native_token::Message::Credit(7)), seeds(&[])),
        ]
    );
    // Events: one Claimed, from the config.
    let events: Vec<_> = w.emitted.iter().map(|(by, e)| (*by, e.data.clone())).collect();
    assert_eq!(events, vec![(Actor::new(config, BRIDGE), bytes(&Claimed { eth_token: USDC, denom_index: 0 }))]);
    // No window of any of it is a secret.
    let public = public_outputs(&before, &w, &private);
    for (what, s) in claim_secrets(&k, &leaves, 1) {
        absent(&public, &what, &s);
    }
    // Positive controls: the scanner does find what is meant to be public.
    assert!(find(&public, a.value()).is_some(), "A is public");
    // The nullifier itself never appears: its PDA address and seed (hashes of it) do.
    absent(&public, "the nullifier itself", &nf);
    assert!(find(&public, nullifier_address(&BRIDGE, &nf).value()).is_some(), "the nullifier PDA is public");
    assert!(find(&public, nullifier_seed(&nf).as_bytes()).is_some(), "the nullifier seed is public");
    assert!(find(&public, &cfg()).is_some(), "the deployment id is public");
}

/// An attested world where Alice's deposit sits at index 1 of a three-leaf tree; its state
/// holds the vault plus enough other accounts that the account proof has several nodes.
struct Attested {
    w: World,
    id: [u8; 32],
    k: [u8; 32],
    leaves: Vec<[u8; 32]>,
    root: [u8; 32],
    state: SynthState,
    header: Vec<u8>,
}

fn attested() -> Attested {
    let mut w = World::default();
    let id = plant_instance(&mut w, &instance_config(EVM_CHAIN));
    w.tx(DEF, TOKEN, &Tok::NewFungibleDefinition { name: "Wrapped USDC".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN })
        .unwrap();
    w.tx(BRIDGE, BRIDGE, &init_attested(source(id), EVM_CHAIN)).unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: USDC, definition: DEF, decimals: 6, denoms: vec![D100] })))
        .unwrap();
    set_sponsor(&mut w, 7);
    fund_float(&mut w, 100);
    let k = secret("b4a", 1);
    let leaves = vec![
        leaf_for(&acfg(), &secret("b4a", 0), &BOB, &USDC, D100),
        leaf_for(&acfg(), &k, &ALICE, &USDC, D100),
        leaf_for(&acfg(), &secret("b4a", 2), &CAROL, &USDC, D100),
    ];
    let root = root_of(&leaves);
    let mut vault_acct = SynthAccount::new(VAULT).with(is_deposit_root(&root), word(1));
    for i in 0..6u64 {
        vault_acct = vault_acct.with(is_deposit_root(&secret("b4a/other-root", i)), word(1));
    }
    let mut accounts = vec![vault_acct];
    for i in 0..8u8 {
        accounts.push(SynthAccount::new([0x40 + i; 20]));
    }
    let state = SynthState { accounts };
    let header = header_rlp(state.state_root(), [0x99; 32], 12_000_000, 0);
    plant_anchor(&mut w, &id, EVM_CHAIN, Anchor::BlockHash(keccak(&header)));
    plant_anchor(&mut w, &id, EVM_CHAIN, Anchor::StateRoot(state.state_root()));
    Attested { w, id, k, leaves, root, state, header }
}

impl Attested {
    fn claim(&self, block_hash: bool, opening: Option<[u8; 32]>) -> Instruction {
        let anchor = if block_hash {
            AnchorWitness { anchor: Anchor::BlockHash(keccak(&self.header)), header_rlp: Some(self.header.clone()) }
        } else {
            AnchorWitness { anchor: Anchor::StateRoot(self.state.state_root()), header_rlp: None }
        };
        Instruction::ClaimAttested {
            eth_token: USDC,
            denom_index: 0,
            denom: D100,
            root: self.root,
            leaf_index: 1,
            path: merkle_path(&self.leaves, 1),
            secret: self.k,
            cfg_id: acfg(),
            source: source(self.id),
            vault: VAULT,
            evm_chain_id: EVM_CHAIN,
            anchor,
            proof: self.state.storage_proof(&VAULT, &is_deposit_root(&self.root)),
            recipient_opening: opening,
        }
    }
}

#[test]
fn an_attested_claim_publishes_exactly_the_allowlist_on_both_anchors() {
    for block_hash in [false, true] {
        let mut f = attested();
        let o = recipient_opening(&f.k);
        let a = ALICE.blinded(&o);
        let nf = claim_nullifier(&f.k, &ALICE, &USDC, D100);
        let claim = f.claim(block_hash, Some(o));
        let Instruction::ClaimAttested { anchor, proof, .. } = &claim else { unreachable!() };
        let (anchor, proof) = (anchor.anchor, proof.clone());
        assert!(proof.account.nodes.len() >= 2 && proof.nodes.len() >= 2, "multi-node proofs");
        let before = f.w.clone();
        f.w.tx_with_openings(ALICE, BRIDGE, &instr(claim.clone()), &[o]).unwrap();
        let config = config_address(&BRIDGE);
        let private = [(ALICE, TOKEN), (ALICE, NATIVE)];
        let want: BTreeSet<_> = [(nullifier_address(&BRIDGE, &nf), BRIDGE), (config, BRIDGE), (DEF, TOKEN), (config, NATIVE)]
            .into_iter()
            .chain(private)
            .collect();
        assert_eq!(changed(&before, &f.w), want, "block_hash={block_hash}");
        let assert_anchor = eth_attest_core::Message::Query(eth_attest_core::Query::AssertAnchor { instance_id: f.id, evm_chain_id: EVM_CHAIN, anchor });
        assert_eq!(
            deliveries(&f.w),
            vec![
                (Actor::new(anchor_address(&ATTEST, &f.id, &anchor), ATTEST), bytes(&assert_anchor), seeds(&[])),
                (Actor::new(nullifier_address(&BRIDGE, &nf), BRIDGE), bytes(&Message::Effect(Effect::SpendClaim)), seeds(&[nullifier_seed(&nf)])),
                (
                    Actor::new(config, BRIDGE),
                    bytes(&Message::Effect(Effect::ClaimAttested {
                        eth_token: USDC,
                        denom_index: 0,
                        denom: D100,
                        to: a,
                        cfg_id: acfg(),
                        source: source(f.id),
                        vault: VAULT,
                        evm_chain_id: EVM_CHAIN,
                        one_time: true
                    })),
                    seeds(&[config_seed()])
                ),
                (Actor::new(DEF, TOKEN), bytes(&Tok::MintBy { to: a, amount: D100 }), seeds(&[])),
                (Actor::native_balance(config), bytes(&native_token::Message::Transfer { to: a, amount: 7 }), seeds(&[config_seed()])),
                (Actor::new(a, TOKEN), bytes(&Tok::Credit { definition: DEF, amount: D100 }), seeds(&[])),
                (Actor::native_balance(a), bytes(&native_token::Message::Credit(7)), seeds(&[])),
            ],
            "block_hash={block_hash}"
        );
        let events: Vec<_> = f.w.emitted.iter().map(|(by, e)| (*by, e.data.clone())).collect();
        assert_eq!(events, vec![(Actor::new(config, BRIDGE), bytes(&Claimed { eth_token: USDC, denom_index: 0 }))]);
        let public = public_outputs(&before, &f.w, &private);
        for (what, s) in claim_secrets(&f.k, &f.leaves, 1) {
            absent(&public, &what, &s);
        }
        // RFP-021 F#8: nor the deposit root, the slot key, the hashed slot path or any node.
        let slot = is_deposit_root(&f.root);
        absent(&public, "the deposit root", &f.root);
        absent(&public, "the storage slot key", &slot);
        absent(&public, "keccak(slot), the trie path", &keccak(&slot));
        absent(&public, "the vault's storage root", &proof.account.fields.storage_root);
        for (i, n) in proof.nodes.iter().enumerate() {
            node_absent(&public, &format!("storage proof node {i}"), n);
        }
        for (i, n) in proof.account.nodes.iter().enumerate() {
            node_absent(&public, &format!("account proof node {i}"), n);
        }
        // Positive controls.
        assert!(find(&public, a.value()).is_some(), "A is public");
        // The nullifier itself never appears: its PDA address and seed (hashes of it) do.
    absent(&public, "the nullifier itself", &nf);
    assert!(find(&public, nullifier_address(&BRIDGE, &nf).value()).is_some(), "the nullifier PDA is public");
    assert!(find(&public, nullifier_seed(&nf).as_bytes()).is_some(), "the nullifier seed is public");
        assert!(find(&public, anchor.value()).is_some(), "the anchor is public");
    }
}

#[test]
fn a_burn_publishes_exactly_the_allowlist() {
    let StandIn { mut w, k, leaves, .. } = stand_in();
    w.tx(ALICE, BRIDGE, &claim_msg(cfg(), USDC, k, 1, &leaves, 0, D100, None)).unwrap();
    let exit_secret = secret("b4/exit", 0);
    let eth_dest: EthAddress = sha256(&[b"b4/eth-dest"])[..20].try_into().unwrap();
    let e = exit_commitment(&exit_secret, &eth_dest, &USDC, D100, 2_000_000, &cfg());
    let config = config_address(&BRIDGE);
    let before = w.clone();
    w.tx(ALICE, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: D100, notify: Actor::new(config, BRIDGE), memo: e }).unwrap();
    let private = [(ALICE, TOKEN)];
    let want: BTreeSet<_> = [(DEF, TOKEN), (config, BRIDGE)].into_iter().chain(private).collect();
    assert_eq!(changed(&before, &w), want);
    assert_eq!(
        deliveries(&w),
        vec![
            (Actor::new(DEF, TOKEN), bytes(&Tok::BurnFrom { amount: D100 }), seeds(&[])),
            (Actor::new(config, BRIDGE), bytes(&BurnNotice { definition: DEF, amount: D100, memo: e }), seeds(&[])),
        ]
    );
    let events: Vec<_> = w.emitted.iter().map(|(by, ev)| (*by, ev.data.clone())).collect();
    assert_eq!(events, vec![(Actor::new(config, BRIDGE), bytes(&Exited { eth_token: USDC, denom_index: 0, exit_commitment: e }))]);
    let public = public_outputs(&before, &w, &private);
    absent(&public, "the exit secret", &exit_secret);
    absent(&public, "the Ethereum destination", &eth_dest);
    absent(&public, "the Ethereum destination (padded)", &padded(&eth_dest));
    absent(&public, "the burner's account", ALICE.value());
    absent(&public, "the release nullifier", &release_nullifier(&exit_secret));
    assert!(find(&public, &e).is_some(), "E is public");
}

// ---------------------------------------------------------------------------------------------
// B5 (RFP-022 Privacy #5): refusals must not depend on the deposit, nor carry witness-derived
// values, and the deposit-dependent classes (not in the root, root not attested, already
// claimed) are one text. Minting frozen is its own class on purpose: it depends only on the
// config's or the token's public flag, it is the same for every deposit, and Usability #8 asks
// for "program or per-token frozen" to be reportable (see "Refusals" in the bridge crate docs).
// `tests/panic_texts.rs` drives every claim, burn and root refusal with two deposits.

/// The refusals for a deposit, each with the world it left.
struct Refusals {
    not_in_root: String,
    /// Claimed against its own (correct) root before the attestor recorded it.
    not_attested: String,
    claimed: String,
    frozen: String,
    /// The root the claimer's actor computed for the not-in-root claim (a witness value).
    computed_root: [u8; 32],
}

/// Two pending deposits A (Alice, index 2) and B (Bob, index 3) behind two others. `which` is 0
/// for A, 1 for B. Every refusal is checked to leave the world exactly as it was.
fn refusals(which: usize) -> Refusals {
    let mut w = setup();
    let others = vec![leaf_for(&cfg(), &secret("b5", 100), &CAROL, &USDC, D100), leaf_for(&cfg(), &secret("b5", 101), &DAVE, &USDC, D100)];
    let r0 = root_of(&others);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: r0 })).unwrap();
    let ka = secret("b5", 0);
    let kb = secret("b5", 1);
    let mut all = others.clone();
    all.push(leaf_for(&cfg(), &ka, &ALICE, &USDC, D100));
    all.push(leaf_for(&cfg(), &kb, &BOB, &USDC, D100));
    let (who, k, idx) = [(ALICE, ka, 2), (BOB, kb, 3)][which];
    let mine = &all[..=idx];
    let refuse = |w: &mut World, m: &Message| {
        let before = w.clone();
        let e = w.tx(who, BRIDGE, m).unwrap_err();
        assert!(w.states == before.states, "a refusal left a state diff");
        e
    };
    // (i) Claimed against the newest attested root r0, which does not hold it yet (the client
    // built its path in its own view of the tree).
    let mut m = claim_msg(cfg(), USDC, k, idx, mine, 0, D100, None);
    if let Message::Instruction(Instruction::Claim { root, .. }) = &mut m {
        *root = r0;
    }
    let not_in_root = refuse(&mut w, &m);
    let computed_root = root_of(mine);
    // (ii) Against its own root, which no attestor recorded yet.
    let not_attested = refuse(&mut w, &claim_msg(cfg(), USDC, k, idx, mine, 0, D100, None));
    // Attested: both are in.
    let r1 = root_of(&all);
    w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root: r1 })).unwrap();
    let honest = claim_msg(cfg(), USDC, k, idx, &all, 0, D100, None);
    // (iv) Minting frozen.
    let mut frozen_w = w.clone();
    frozen_w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
    let frozen = refuse(&mut frozen_w, &honest);
    // (iii) Already claimed.
    w.tx(who, BRIDGE, &honest).unwrap();
    let claimed = refuse(&mut w, &honest);
    Refusals { not_in_root, not_attested, claimed, frozen, computed_root }
}

/// Renderings a panic text could carry a 32-byte value in.
fn renderings(v: &[u8; 32]) -> Vec<String> {
    vec![format!("{v:?}"), hex(v), format!("{:?}", v.to_vec())]
}

#[test]
fn refusals_leave_no_state_diff_and_two_classes_do_not_depend_on_the_deposit() {
    let (a, b) = (refusals(0), refusals(1));
    // State diffs: checked inside `refusals` (every refusal leaves the world unchanged).
    assert_eq!(a.claimed, b.claimed);
    assert_eq!(a.claimed, CLAIM_REFUSED);
    assert_eq!(a.not_attested, b.not_attested);
    assert_eq!(a.frozen, b.frozen);
    assert_eq!(a.frozen, MINT_FROZEN);
}

#[test]
fn a_not_in_root_refusal_does_not_depend_on_the_deposit_or_carry_witness_values() {
    let (a, b) = (refusals(0), refusals(1));
    assert_eq!(a.not_in_root, b.not_in_root, "the refusal text depends on which deposit was claimed");
    for (r, label) in [(&a, "A"), (&b, "B")] {
        for s in renderings(&r.computed_root) {
            assert!(!r.not_in_root.contains(&s), "deposit {label}: the refusal carries the computed root: {}", r.not_in_root);
        }
    }
}

/// Was `the_three_refusal_classes_are_one` (not in root, already claimed, minting frozen). The
/// deposit-dependent classes are one; minting frozen is deliberately its own (see the section
/// comment), the same text for every deposit, which `refusals_leave_no_state_diff_...` checks.
#[test]
fn the_deposit_dependent_refusal_classes_are_one() {
    let a = refusals(0);
    assert_eq!(a.not_in_root, a.claimed, "not in the root vs already claimed");
    assert_eq!(a.not_attested, a.claimed, "root not attested vs already claimed");
    assert_eq!(a.claimed, CLAIM_REFUSED);
    assert_eq!(a.frozen, MINT_FROZEN, "frozen is the one public-state class");
}

/// Pins the texts exactly (was `today_the_refusal_texts_are_these`, which pinned the leak).
#[test]
fn the_refusal_texts_are_these() {
    for r in [refusals(0), refusals(1)] {
        assert_eq!(
            (r.not_in_root.as_str(), r.not_attested.as_str(), r.claimed.as_str(), r.frozen.as_str()),
            ("claim refused", "claim refused", "claim refused", "minting is frozen")
        );
    }
}

/// The rendering alloy-trie's `Nibbles` Debug uses for a full 64-nibble path: `Nibbles(0x<packed hex>)`.
fn nibbles_hex(b: &[u8; 32]) -> String {
    format!("Nibbles(0x{})", hex(b))
}

/// Attested refusals: a claim not in its root, and a storage proof that lies about the entry.
/// Returns the world, the not-in-root text, the root the claimer's actor computed for it, the
/// lying-proof text, the storage slot of the (ghost) root that proof was about, and the two
/// instructions.
fn attested_refusals() -> (Attested, String, [u8; 32], String, [u8; 32], [Instruction; 2]) {
    let mut f = attested();
    // Not in the root: a path for another index.
    let mut i = f.claim(false, None);
    if let Instruction::ClaimAttested { leaf_index, .. } = &mut i {
        *leaf_index = 0;
    }
    let computed = merkle_root(&f.leaves[1], 0, &merkle_path(&f.leaves, 1));
    let before = f.w.clone();
    let not_in_root = f.w.tx(ALICE, BRIDGE, &instr(i.clone())).unwrap_err();
    assert!(f.w.states == before.states);
    // A root the vault never marked, claimed true anyway.
    let ghost = [deposit_leaf(&acfg(), &USDC, D100, &deposit_commitment(&f.k, &ALICE, &USDC, D100, &acfg())), secret("b5a", 9)];
    let ghost_root = root_of(&ghost);
    let mut proof = f.state.storage_proof(&VAULT, &is_deposit_root(&ghost_root));
    proof.value = word(1);
    let mut lie_i = with(f.claim(false, None), |x| {
        *x.root = ghost_root;
        *x.proof = proof;
    });
    if let Instruction::ClaimAttested { leaf_index, path, .. } = &mut lie_i {
        *leaf_index = 0;
        *path = merkle_path(&ghost, 0);
    }
    let lie = f.w.tx(ALICE, BRIDGE, &instr(lie_i.clone())).unwrap_err();
    assert!(f.w.states == before.states);
    (f, not_in_root, computed, lie, is_deposit_root(&ghost_root), [i, lie_i])
}

#[test]
fn attested_refusals_carry_no_witness_values() {
    let (f, not_in_root, computed, lie, slot, _) = attested_refusals();
    let mut leaks = Vec::new();
    if renderings(&computed).iter().any(|s| not_in_root.contains(s)) {
        leaks.push(format!("not-in-root refusal carries the computed root: {not_in_root}"));
    }
    // The lie is about a root the claimer chose: that root, its slot, the slot's hash (the trie
    // path) and the vault's storage root are all witness values.
    let storage_root = f.state.accounts[0].storage_root();
    for (what, v) in [("the slot", slot), ("keccak(slot)", keccak(&slot)), ("the vault storage root", storage_root)] {
        if renderings(&v).into_iter().chain([nibbles_hex(&v)]).any(|s| lie.contains(&s)) {
            leaks.push(format!("storage-proof refusal carries {what}: {lie}"));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// Was `today_an_attested_storage_refusal_names_the_slot_path`, which pinned the leak. On chain
/// both are the one claim refusal; natively (what the CLI shows its user) they are told apart,
/// and only there does the detailed InclusionError, with the slot path, appear.
#[test]
fn attested_refusals_are_the_one_claim_refusal_and_are_told_apart_natively() {
    let (_, not_in_root, _, lie, slot, [i, lie_i]) = attested_refusals();
    assert_eq!((not_in_root.as_str(), lie.as_str()), (CLAIM_REFUSED, CLAIM_REFUSED));
    assert_eq!(witness_fault(ALICE, &i), Some(ClaimFault::NotInRoot));
    let Some(ClaimFault::Inclusion(e)) = witness_fault(ALICE, &lie_i) else { panic!("a storage-proof fault") };
    assert_eq!(e.class(), eth_attest_core::inclusion::InclusionClass::StorageProof);
    assert!(e.to_string().contains(&nibbles_hex(&keccak(&slot))), "the native detail keeps the path: {e}");
}
