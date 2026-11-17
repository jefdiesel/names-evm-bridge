//! RFP-022 Privacy #5 and Usability #8: every refusal on the claim, burn and root paths, driven
//! with two different deposits (secrets, depositors, amounts, leaf indices), two burns or two
//! roots. Each class must give byte-identical texts for both, leave the world unchanged, and
//! carry no rendering of a witness value; the deposit-dependent claim classes must be the one
//! text `CLAIM_REFUSED`. Also: a scan of the programs' source for panics that could print a
//! value, and the claimer's root `Response` (its public footprint) for both claim paths.
//!
//! The simulator is `common/mod.rs`; read its docs for what it does not model.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;

const D100: u128 = 100_000_000;
const D1000: u128 = 1_000_000_000;
/// A token the bridge never registered.
const XTOKEN: EthAddress = [0x77; 20];
/// A deployment id that is not this bridge's.
const OTHER_CFG: [u8; 32] = [0x99; 32];

/// A deposit: secret, depositor, denomination index and amount.
#[derive(Clone, Copy)]
struct Dep {
    k: [u8; 32],
    who: AccountId,
    di: u32,
    amount: u128,
}

fn deps() -> [Dep; 2] {
    [
        Dep { k: secret("pt", 1), who: ALICE, di: 0, amount: D100 },
        Dep { k: secret("pt", 2), who: BOB, di: 1, amount: D1000 },
    ]
}

/// Renderings a panic text could carry a value in.
fn renderings(v: &[u8]) -> Vec<String> {
    vec![format!("{v:?}"), hex(v), format!("{:?}", v.to_vec())]
}

/// No text carries any rendering of any of `secrets`.
#[track_caller]
fn carries_none(texts: &[(&str, String)], secrets: &[(String, Vec<u8>)]) {
    for (case, text) in texts {
        for (what, v) in secrets {
            for r in renderings(v) {
                assert!(!text.contains(&r), "{case}: the refusal carries {what}: {text}");
            }
        }
    }
}

/// Runs `m` from `signer` (with `openings`) on a copy of `w` prepared by `prep`; returns the
/// refusal, after checking it left the world exactly as it was.
fn refusal(w: &World, prep: impl FnOnce(&mut World), signer: AccountId, m: &Message, openings: &[[u8; 32]]) -> String {
    let mut w = w.clone();
    prep(&mut w);
    let before = w.clone();
    let e = w.tx_with_openings(signer, BRIDGE, m, openings).expect_err("expected a refusal");
    assert!(w.states == before.states, "a refusal changed state: {e}");
    e
}

/// The claim-witness secrets of a stand-in deposit at `idx` of `leaves`.
fn witness_secrets(d: &Dep, leaves: &[[u8; 32]], idx: usize, cfg: [u8; 32]) -> Vec<(String, Vec<u8>)> {
    let c = deposit_commitment(&d.k, &d.who, &USDC, d.amount, &cfg);
    let mut s = vec![
        ("the secret".to_owned(), d.k.to_vec()),
        ("the commitment".to_owned(), c.to_vec()),
        ("the leaf".to_owned(), leaves[idx].to_vec()),
        ("the nullifier".to_owned(), claim_nullifier(&d.k, &d.who, &USDC, d.amount).to_vec()),
        ("the nullifier PDA".to_owned(), nullifier_address(&BRIDGE, &claim_nullifier(&d.k, &d.who, &USDC, d.amount)).value().to_vec()),
        ("the opening".to_owned(), recipient_opening(&d.k).to_vec()),
        ("the depositor".to_owned(), d.who.value().to_vec()),
        ("the amount".to_owned(), d.amount.to_be_bytes().to_vec()),
    ];
    for (h, p) in merkle_path(leaves, idx).iter().enumerate() {
        s.push((format!("path element {h}"), p.to_vec()));
    }
    s
}

/// Asserts both deposits' refusals are, case by case, the expected text (or, for `None`,
/// simply the same text), and carry none of either deposit's secrets.
#[track_caller]
fn same_per_class(a: &[(&str, String)], b: &[(&str, String)], expected: &[(&str, Option<&str>)]) {
    assert_eq!(a.len(), expected.len());
    for ((ca, ta), ((cb, tb), (ce, want))) in a.iter().zip(b.iter().zip(expected)) {
        assert_eq!((ca, cb), (ce, ce));
        assert_eq!(ta, tb, "{ca}: the refusal depends on which deposit was claimed");
        if let Some(want) = want {
            assert_eq!(ta, want, "{ca}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Stand-in claims

struct StandIn {
    w: World,
    leaves: Vec<[u8; 32]>,
    /// An attested root that holds neither deposit.
    small: [u8; 32],
}

/// Both deposits (index 1 and 2) under an attested root, plus a leaf of each for an unregistered
/// token (3, 4) and for another deployment (5, 6).
fn stand_in() -> StandIn {
    let mut w = setup();
    let ds = deps();
    let mut leaves = vec![leaf_for(&cfg(), &secret("pt/other", 0), &CAROL, &USDC, D100)];
    leaves.extend(ds.iter().map(|d| leaf_for(&cfg(), &d.k, &d.who, &USDC, d.amount)));
    leaves.extend(ds.iter().map(|d| leaf_for(&cfg(), &d.k, &d.who, &XTOKEN, d.amount)));
    leaves.extend(ds.iter().map(|d| leaf_for(&OTHER_CFG, &d.k, &d.who, &USDC, d.amount)));
    let small = root_of(&leaves[..1]);
    for root in [small, root_of(&leaves)] {
        w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root })).unwrap();
    }
    StandIn { w, leaves, small }
}

fn edit(m: &Message, f: impl FnOnce(&mut Instruction)) -> Message {
    let Message::Instruction(mut i) = m.clone() else { unreachable!() };
    f(&mut i);
    instr(i)
}

const STAND_IN_CASES: [(&str, Option<&str>); 14] = [
    ("not in the root", Some(CLAIM_REFUSED)),
    ("another signer", Some(CLAIM_REFUSED)),
    ("another amount in the leaf", Some(CLAIM_REFUSED)),
    ("a path of 31", Some(CLAIM_REFUSED)),
    ("leaf index 2^32", Some(CLAIM_REFUSED)),
    ("root not attested", Some(CLAIM_REFUSED)),
    ("repeat claim", Some(CLAIM_REFUSED)),
    ("minting frozen globally", Some(MINT_FROZEN)),
    ("minting frozen for the token", Some(MINT_FROZEN)),
    ("bad denomination", Some("bad denomination")),
    ("wrong amount for the denomination", Some("wrong amount for this denomination")),
    ("token not registered", Some("token not registered")),
    ("another deployment", Some("claim for another deployment")),
    // LEZ's native token text: it names the config's account (public, the same for every
    // deposit), not the claimer or the one-time recipient.
    ("the float is short", None),
];

fn stand_in_refusals(s: &StandIn, j: usize) -> Vec<(&'static str, String)> {
    let d = deps()[j];
    let idx = 1 + j;
    let honest = claim_msg(cfg(), USDC, d.k, idx, &s.leaves, d.di, d.amount, None);
    let mut extended = s.leaves.clone();
    extended.push(leaf_for(&cfg(), &secret("pt/later", 0), &CAROL, &USDC, D100));
    let o = recipient_opening(&d.k);
    let none = |_: &mut World| {};
    let out = vec![
        ("not in the root", refusal(&s.w, none, d.who, &edit(&honest, |i| if let Instruction::Claim { root, .. } = i { *root = s.small }), &[])),
        ("another signer", refusal(&s.w, none, DAVE, &honest, &[])),
        ("another amount in the leaf", refusal(&s.w, none, d.who, &claim_msg(cfg(), USDC, d.k, idx, &s.leaves, d.di, d.amount + 1, None), &[])),
        ("a path of 31", refusal(&s.w, none, d.who, &edit(&honest, |i| if let Instruction::Claim { path, .. } = i { path.pop(); }), &[])),
        ("leaf index 2^32", refusal(&s.w, none, d.who, &edit(&honest, |i| if let Instruction::Claim { leaf_index, .. } = i { *leaf_index += 1 << 32 }), &[])),
        ("root not attested", refusal(&s.w, none, d.who, &claim_msg(cfg(), USDC, d.k, idx, &extended, d.di, d.amount, None), &[])),
        ("repeat claim", refusal(&s.w, |w| w.tx(d.who, BRIDGE, &honest).unwrap(), d.who, &honest, &[])),
        ("minting frozen globally", refusal(&s.w, |w| w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap(), d.who, &honest, &[])),
        ("minting frozen for the token", refusal(&s.w, |w| w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap(), d.who, &honest, &[])),
        ("bad denomination", refusal(&s.w, none, d.who, &claim_msg(cfg(), USDC, d.k, idx, &s.leaves, 7, d.amount, None), &[])),
        ("wrong amount for the denomination", refusal(&s.w, none, d.who, &claim_msg(cfg(), USDC, d.k, idx, &s.leaves, 1 - d.di, d.amount, None), &[])),
        ("token not registered", refusal(&s.w, none, d.who, &claim_msg(cfg(), XTOKEN, d.k, 3 + j, &s.leaves, d.di, d.amount, None), &[])),
        ("another deployment", refusal(&s.w, none, d.who, &claim_msg(OTHER_CFG, USDC, d.k, 5 + j, &s.leaves, d.di, d.amount, None), &[])),
        ("the float is short", refusal(&s.w, |w| set_sponsor(w, 7), d.who, &claim_msg(cfg(), USDC, d.k, idx, &s.leaves, d.di, d.amount, Some(o)), &[o])),
    ];
    // The native diagnosis (what the CLI tells its user) still tells the classes apart.
    assert_eq!(message_fault(d.who, &edit(&honest, |i| if let Instruction::Claim { root, .. } = i { *root = s.small })), Some(ClaimFault::NotInRoot));
    assert_eq!(message_fault(DAVE, &honest), Some(ClaimFault::NotInRoot));
    assert_eq!(message_fault(d.who, &edit(&honest, |i| if let Instruction::Claim { path, .. } = i { path.pop(); })), Some(ClaimFault::PathShape));
    assert_eq!(message_fault(d.who, &honest), None, "repeat, frozen, not attested: the witness is fine");
    out
}

#[test]
fn every_stand_in_claim_refusal_is_the_same_for_two_deposits() {
    let s = stand_in();
    let (a, b) = (stand_in_refusals(&s, 0), stand_in_refusals(&s, 1));
    same_per_class(&a, &b, &STAND_IN_CASES);
    assert!(a[13].1.contains("holds less than the transferred amount"), "{}", a[13].1);
    for (j, texts) in [(0, &a), (1, &b)] {
        let d = deps()[j];
        let mut secrets = witness_secrets(&d, &s.leaves, 1 + j, cfg());
        secrets.push(("the root".to_owned(), root_of(&s.leaves).to_vec()));
        carries_none(texts, &secrets);
    }
}

// ---------------------------------------------------------------------------------------------
// Attested claims

/// An account proof whose joined path runs past 64 nibbles (B16-NYB), as Alice's claim.
fn overlong(i: Instruction) -> Instruction {
    let list = |items: &[Vec<u8>]| {
        let payload: Vec<u8> = items.concat();
        let mut out = if payload.len() < 56 { vec![0xc0 + payload.len() as u8] } else { vec![0xf8, payload.len() as u8] };
        out.extend(payload);
        out
    };
    let string = |b: &[u8]| [&[0x80 + b.len() as u8][..], b].concat();
    let leaf = list(&[string(&[0x20, 0x12]), string(&[0x01; 20])]);
    let mut ext_key = vec![0x00];
    ext_key.extend_from_slice(&[0xAB; 32]);
    let ext = list(&[string(&ext_key), string(&keccak(&leaf))]);
    with(i, |x| {
        *x.anchor = AnchorWitness { anchor: Anchor::StateRoot(keccak(&ext)), header_rlp: None };
        x.proof.account.nodes = vec![ext.clone(), leaf.clone()];
    })
}

const ATTESTED_CASES: [(&str, Option<&str>); 14] = [
    ("not in the root", Some(CLAIM_REFUSED)),
    ("another signer", Some(CLAIM_REFUSED)),
    ("a root the vault never marked", Some(CLAIM_REFUSED)),
    ("a storage proof that lies", Some(CLAIM_REFUSED)),
    ("an account proof one node short", Some(CLAIM_REFUSED)),
    ("a header that does not hash to the anchor", Some(CLAIM_REFUSED)),
    ("an account proof path past 64 nibbles", Some(CLAIM_REFUSED)),
    ("repeat claim", Some(CLAIM_REFUSED)),
    ("minting frozen globally", Some(MINT_FROZEN)),
    ("minting frozen for the token", Some(MINT_FROZEN)),
    ("bad denomination", Some("bad denomination")),
    // From the attestation program: insufficient finality, about the public anchor only. On LEZ
    // an anchor PDA that does not exist fails earlier, as the runtime's UnadmittedPublicActor
    // (the simulator admits only bridge PDAs); the bridge cannot seed another program's PDA.
    ("an anchor the instance did not finalize", Some("RFP021-E01 header not finalized: anchor not finalized by this instance")),
    ("another chain id", Some("RFP021-E07 chain id mismatch: anchor is for another chain")),
    ("another attestation instance", Some("claim for another attestation source")),
];

fn attested_refusals(f: &Fixture, who: usize) -> Vec<(&'static str, String)> {
    let signer = [ALICE, BOB][who];
    let honest = f.claim(who);
    let leaf = f.leaves[who];
    // A root that holds the deposit but that the vault never marked: honest exclusion proof, or
    // the same proof claiming 1.
    let ghost = [leaf, secret("pt/ghost", who as u64)];
    let ghost_root = root_of(&ghost);
    let ghosted = |value: Option<[u8; 32]>| {
        let mut proof = f.state.storage_proof(&VAULT, &is_deposit_root(&ghost_root));
        if let Some(v) = value {
            proof.value = v;
        }
        let mut i = with(honest.clone(), |x| {
            *x.root = ghost_root;
            *x.proof = proof;
        });
        if let Instruction::ClaimAttested { leaf_index, path, .. } = &mut i {
            *leaf_index = 0;
            *path = merkle_path(&ghost, 0);
        }
        i
    };
    let mut header = f.header.clone();
    header[40] ^= 1;
    let mut later = f.state.clone();
    later.accounts[2] = SynthAccount::new([0x21; 20]).with(word(1), word(1));
    let unfinalized = with(honest.clone(), |x| {
        *x.anchor = AnchorWitness { anchor: Anchor::StateRoot(later.state_root()), header_rlp: None };
        *x.proof = later.storage_proof(&VAULT, &is_deposit_root(&f.root));
    });
    let other = instance_id(&ATTEST, &InstanceConfig { salt: [1; 32], ..instance_config(EVM_CHAIN) });
    let anchor = f.state_anchor;
    let none = |_: &mut World| {};
    let cases: Vec<(&'static str, Instruction, Box<dyn FnOnce(&mut World)>, AccountId)> = vec![
        ("not in the root", { let mut i = honest.clone(); if let Instruction::ClaimAttested { leaf_index, .. } = &mut i { *leaf_index = 1 - who as u64 } i }, Box::new(none), signer),
        ("another signer", honest.clone(), Box::new(none), CAROL),
        ("a root the vault never marked", ghosted(None), Box::new(none), signer),
        ("a storage proof that lies", ghosted(Some(word(1))), Box::new(none), signer),
        ("an account proof one node short", with(honest.clone(), |x| { x.proof.account.nodes.pop(); }), Box::new(none), signer),
        ("a header that does not hash to the anchor", with(honest.clone(), |x| *x.anchor = AnchorWitness { anchor: f.block_anchor, header_rlp: Some(header.clone()) }), Box::new(none), signer),
        ("an account proof path past 64 nibbles", overlong(honest.clone()), Box::new(none), signer),
        ("repeat claim", honest.clone(), Box::new({ let h = honest.clone(); move |w: &mut World| w.tx(signer, BRIDGE, &instr(h)).unwrap() }), signer),
        ("minting frozen globally", honest.clone(), Box::new(|w: &mut World| w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap()), signer),
        ("minting frozen for the token", honest.clone(), Box::new(|w: &mut World| w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), true, false)).unwrap()), signer),
        ("bad denomination", { let mut i = honest.clone(); if let Instruction::ClaimAttested { denom_index, .. } = &mut i { *denom_index = 7 } i }, Box::new(none), signer),
        ("an anchor the instance did not finalize", unfinalized, Box::new(none), signer),
        ("another chain id", with(honest.clone(), |x| *x.evm_chain_id = 1), Box::new(none), signer),
        (
            "another attestation instance",
            with(honest.clone(), |x| x.source.attest_instance = other),
            Box::new(move |w: &mut World| {
                plant_instance(w, &InstanceConfig { salt: [1; 32], ..instance_config(EVM_CHAIN) });
                plant_anchor(w, &other, EVM_CHAIN, anchor);
            }),
            signer,
        ),
    ];
    // The native diagnosis tells the deposit-dependent classes apart, by fault and by class.
    use eth_attest_core::inclusion::InclusionClass as C;
    let classes: Vec<Option<C>> = cases[2..7].iter().map(|(_, i, _, s)| inclusion_class(*s, i)).collect();
    assert_eq!(classes, vec![Some(C::Predicate), Some(C::StorageProof), Some(C::AccountProof), Some(C::Anchor), Some(C::AccountProof)]);
    assert_eq!(witness_fault(signer, &cases[0].1), Some(ClaimFault::NotInRoot));
    assert_eq!(witness_fault(CAROL, &honest), Some(ClaimFault::NotInRoot));
    assert_eq!(witness_fault(signer, &honest), None);
    cases.into_iter().map(|(case, i, prep, s)| (case, refusal(&f.w, prep, s, &instr(i), &[]))).collect()
}

#[test]
fn every_attested_claim_refusal_is_the_same_for_two_deposits() {
    let f = attested_setup();
    let (a, b) = (attested_refusals(&f, 0), attested_refusals(&f, 1));
    same_per_class(&a, &b, &ATTESTED_CASES);
    for (who, texts) in [(0, &a), (1, &b)] {
        let d = Dep { k: f.keys[who], who: [ALICE, BOB][who], di: who as u32, amount: [D100, D1000][who] };
        let mut secrets = witness_secrets(&d, &f.leaves, who, acfg());
        let slot = is_deposit_root(&f.root);
        secrets.extend([
            ("the root".to_owned(), f.root.to_vec()),
            ("the slot".to_owned(), slot.to_vec()),
            ("keccak(slot), the trie path".to_owned(), keccak(&slot).to_vec()),
            ("the vault storage root".to_owned(), f.state.accounts[0].storage_root().to_vec()),
        ]);
        carries_none(texts, &secrets);
        for (case, t) in texts.iter() {
            assert!(!t.contains("Nibbles("), "{case}: a trie path leaks: {t}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Burns and roots

#[test]
fn every_burn_refusal_is_the_same_for_two_burns() {
    let s = stand_in();
    let mut w = s.w.clone();
    let ds = deps();
    for (j, d) in ds.iter().enumerate() {
        w.tx(d.who, BRIDGE, &claim_msg(cfg(), USDC, d.k, 1 + j, &s.leaves, d.di, d.amount, None)).unwrap();
    }
    // A definition the bridge never registered, which each burner holds.
    for d in ds {
        let def = AccountId::new([0xD8 - d.di as u8; 32]);
        w.tx(def, TOKEN, &Tok::NewFungibleDefinition { name: "Unbridged".into(), total_supply: 500, mint_authority: None, supply_to: d.who }).unwrap();
    }
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    // Two burns: Alice's 100 and Bob's 1000, each with its own exit commitment.
    let burns: Vec<(Dep, [u8; 32], EthAddress)> = ds
        .iter()
        .map(|d| {
            let eth: EthAddress = sha256(&[b"pt/eth", &d.k])[..20].try_into().unwrap();
            (*d, exit_commitment(&secret("pt/exit", u64::from(d.di)), &eth, &USDC, d.amount, 0, &cfg()), eth)
        })
        .collect();
    let refuse = |w: &World, prep: &dyn Fn(&mut World), who: AccountId, m: &Tok| {
        let mut w = w.clone();
        prep(&mut w);
        let before = w.clone();
        let e = w.tx(who, TOKEN, m).expect_err("expected a refusal");
        assert!(w.states == before.states, "a refusal changed state: {e}");
        e
    };
    let mut per_burn = Vec::new();
    for (d, e, eth) in &burns {
        let other_def = AccountId::new([0xD8 - d.di as u8; 32]);
        let burn = |definition: AccountId, amount: u128| Tok::BurnWithNotice { definition, amount, notify, memo: *e };
        let none: &dyn Fn(&mut World) = &|_| {};
        let texts = vec![
            ("not a bridged token", refuse(&w, none, d.who, &burn(other_def, 100))),
            ("not one denomination", refuse(&w, none, d.who, &burn(DEF, d.amount / 2))),
            ("redemption frozen globally", refuse(&w, &|w| w.tx(FREEZER, BRIDGE, &freeze(None, false, true)).unwrap(), d.who, &burn(DEF, d.amount))),
            ("redemption frozen for the token", refuse(&w, &|w| w.tx(FREEZER, BRIDGE, &freeze(Some(USDC), false, true)).unwrap(), d.who, &burn(DEF, d.amount))),
            ("a notice another program sent", {
                let mut x = w.clone();
                let before = x.clone();
                let mut asyncs = VecDeque::new();
                let forged = BurnNotice { definition: DEF, amount: d.amount, memo: *e };
                let r = x.deliver(Some(Actor::new(d.who, YES)), notify, borsh::to_vec(&forged).unwrap(), BTreeSet::new(), None, &mut asyncs);
                assert!(x.states == before.states);
                r.expect_err("expected a refusal")
            }),
            ("burns exceed claims", {
                // Everything claimed is burned; then a notice (as from the token program) for more.
                let mut x = w.clone();
                for b in deps() {
                    x.tx(b.who, TOKEN, &Tok::BurnWithNotice { definition: DEF, amount: b.amount, notify, memo: [0x42; 32] }).unwrap();
                }
                let before = x.clone();
                let mut asyncs = VecDeque::new();
                let notice = BurnNotice { definition: DEF, amount: d.amount, memo: *e };
                let r = x.deliver(Some(Actor::new(DEF, TOKEN)), notify, borsh::to_vec(&notice).unwrap(), BTreeSet::new(), None, &mut asyncs);
                assert!(x.states == before.states);
                r.expect_err("expected a refusal")
            }),
        ];
        let secrets = vec![
            ("the exit commitment".to_owned(), e.to_vec()),
            ("the Ethereum destination".to_owned(), eth.to_vec()),
            ("the burner".to_owned(), d.who.value().to_vec()),
        ];
        carries_none(&texts, &secrets);
        per_burn.push(texts);
    }
    same_per_class(
        &per_burn[0],
        &per_burn[1],
        &[
            ("not a bridged token", Some("not a bridged token")),
            ("not one denomination", Some("a burn is exactly one denomination")),
            ("redemption frozen globally", Some(BURN_FROZEN)),
            ("redemption frozen for the token", Some(BURN_FROZEN)),
            ("a notice another program sent", Some("a burn notice comes from the token program")),
            ("burns exceed claims", Some("burns exceed claims")),
        ],
    );
}

#[test]
fn every_root_refusal_is_the_same_for_two_roots() {
    let roots = [secret("pt/root", 1), secret("pt/root", 2)];
    let attest = |w: &mut World, by: AccountId, root: [u8; 32]| {
        let before = w.clone();
        let e = w.tx(by, BRIDGE, &instr(Instruction::AttestRoot { root })).expect_err("expected a refusal");
        assert!(w.states == before.states);
        e
    };
    let per_root: Vec<Vec<(&str, String)>> = roots
        .iter()
        .map(|root| {
            let mut w = setup();
            let not_attestor = attest(&mut w, ALICE, *root);
            w.tx(FREEZER, BRIDGE, &freeze(None, true, false)).unwrap();
            let frozen = attest(&mut w, ATTESTOR, *root);
            let mut f = attested_setup();
            f.w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::SetAttestor { attestor: ATTESTOR }))).unwrap();
            let not_stand_in = attest(&mut f.w, ATTESTOR, *root);
            let texts = vec![("not the attestor", not_attestor), ("minting frozen", frozen), ("an RFP-021 config", not_stand_in)];
            carries_none(&texts, &[("the root".to_owned(), root.to_vec())]);
            texts
        })
        .collect();
    same_per_class(
        &per_root[0],
        &per_root[1],
        &[
            ("not the attestor", Some("only the attestor records roots")),
            ("minting frozen", Some(MINT_FROZEN)),
            ("an RFP-021 config", Some("this bridge takes roots from an RFP-021 attestation, not the stand-in")),
        ],
    );
}

// ---------------------------------------------------------------------------------------------
// The claimer's own response: the footprint the program controls

/// The claimer's actor run on its own, as the transaction's root: its `Response` is every
/// message the claim will send, built before any other actor runs.
fn root_response(signer: AccountId, m: &Message, pre_state: Vec<u8>) -> Result<Response, String> {
    let input = ReceiveInput {
        receiver: Actor::new(signer, BRIDGE),
        from: None,
        is_authorized: true,
        pre_state: ActorState::from(pre_state),
        message: borsh::to_vec(m).unwrap(),
    };
    catch_unwind(AssertUnwindSafe(|| handle(&input))).map_err(panic_text)
}

/// (program, message variant tag, seed count) of each message: what a failed claim's footprint
/// looks like whichever deposit it is for.
fn shape(r: &Response) -> Vec<(AccountId, u8, usize)> {
    r.sync_messages.iter().map(|m| (m.to.program_account_id, m.message[1], m.pda_seeds.len())).collect()
}

#[test]
fn the_claimers_actor_sends_the_same_three_messages_for_every_claim() {
    // Stand-in: both deposits, with and without an opening.
    let s = stand_in();
    let mut responses = Vec::new();
    for (j, d) in deps().iter().enumerate() {
        for opening in [None, Some(recipient_opening(&d.k))] {
            let m = claim_msg(cfg(), USDC, d.k, 1 + j, &s.leaves, d.di, d.amount, opening);
            let r = root_response(d.who, &m, Vec::new()).unwrap();
            // It reads no state: whatever its own pre-state holds, the response is the same, so
            // it cannot depend on whether the deposit was claimed, frozen or the root recorded.
            assert_eq!(root_response(d.who, &m, vec![0xAB; 50]).unwrap(), r);
            assert!(r.post_state.is_none() && r.events.is_empty() && r.async_messages.is_empty());
            let root = root_of(&s.leaves);
            let nf = claim_nullifier(&d.k, &d.who, &USDC, d.amount);
            let to: Vec<Actor> = r.sync_messages.iter().map(|m| m.to).collect();
            assert_eq!(
                to,
                vec![
                    Actor::new(root_address(&BRIDGE, &root), BRIDGE),
                    Actor::new(nullifier_address(&BRIDGE, &nf), BRIDGE),
                    Actor::new(config_address(&BRIDGE), BRIDGE)
                ]
            );
            // The assertion and the spend carry no payload at all.
            assert_eq!(r.sync_messages[0].message, borsh::to_vec(&Message::Effect(Effect::AssertRoot)).unwrap());
            assert_eq!(r.sync_messages[1].message, borsh::to_vec(&Message::Effect(Effect::SpendClaim)).unwrap());
            responses.push(r);
        }
    }
    let first = shape(&responses[0]);
    assert!(responses.iter().all(|r| shape(r) == first), "the footprint's shape depends on the deposit");
    // A witness that proves nothing never gets as far as a message.
    let d = deps()[0];
    let bad = claim_msg(cfg(), USDC, d.k, 1, &s.leaves, d.di, d.amount, None);
    assert_eq!(root_response(DAVE, &bad, Vec::new()).unwrap_err(), CLAIM_REFUSED);

    // RFP-021: both deposits, both anchor kinds.
    let f = attested_setup();
    let mut responses = Vec::new();
    for who in 0..2 {
        let signer = [ALICE, BOB][who];
        for i in [f.claim(who), f.claim_by_block_hash(who)] {
            let r = root_response(signer, &instr(i.clone()), Vec::new()).unwrap();
            assert_eq!(root_response(signer, &instr(i.clone()), vec![0xAB; 50]).unwrap(), r);
            let Instruction::ClaimAttested { anchor, .. } = &i else { unreachable!() };
            assert_eq!(r.sync_messages[0].to, Actor::new(anchor_address(&ATTEST, &f.id, &anchor.anchor), ATTEST));
            assert!(r.sync_messages[0].pda_seeds.is_empty(), "AssertAnchor goes to another program's PDA: no seed of ours derives it");
            assert_eq!(r.sync_messages[1].to.account_id, nullifier_address(&BRIDGE, &claim_nullifier(&f.keys[who], &signer, &USDC, [D100, D1000][who])));
            responses.push(r);
        }
    }
    let first = shape(&responses[0]);
    assert!(responses.iter().all(|r| shape(r) == first));
    let lie = with(f.claim(0), |x| x.proof.value = word(2));
    assert_eq!(root_response(ALICE, &instr(lie), Vec::new()).unwrap_err(), CLAIM_REFUSED);
}

// ---------------------------------------------------------------------------------------------
// Source scan

/// The source before its `#[cfg(test)]` module, without `//` comments.
fn program_source(src: &str) -> String {
    let cut = src.find("#[cfg(test)]").unwrap_or(src.len());
    src[..cut]
        .lines()
        .map(|line| {
            let mut in_str = false;
            let mut prev = ' ';
            let mut end = line.len();
            for (i, c) in line.char_indices() {
                if c == '"' && prev != '\\' {
                    in_str = !in_str;
                }
                if !in_str && c == '/' && prev == '/' {
                    end = i - 1;
                    break;
                }
                prev = c;
            }
            &line[..end]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text between the `(` at `open` and its matching `)`, string-aware.
fn macro_args(src: &str, open: usize) -> &str {
    let bytes = src.as_bytes();
    let (mut depth, mut in_str, mut i) = (0i32, false, open);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'(' | b'[' | b'{' if !in_str => depth += 1,
            b')' | b']' | b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return &src[open + 1..i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced macro call");
}

/// Splits macro arguments at top-level commas.
fn top_level_args(args: &str) -> Vec<&str> {
    let (mut out, mut depth, mut in_str, mut start) = (Vec::new(), 0i32, false, 0);
    let bytes = args.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'(' | b'[' | b'{' if !in_str => depth += 1,
            b')' | b']' | b'}' if !in_str => depth -= 1,
            b',' if !in_str && depth == 0 => {
                out.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if !args[start..].trim().is_empty() {
        out.push(args[start..].trim());
    }
    out
}

/// Problems with `src`: value-printing panics. A panic or assert message must be one string
/// literal whose placeholders are all upper-case constants (`{CLAIM_REFUSED}`), with no
/// arguments after it; `assert_eq!`/`assert_ne!` (which print both sides), `.unwrap()` and
/// `.expect(` (which print the error's Debug) are not used at all.
fn scan(name: &str, src: &str) -> Vec<String> {
    let code = program_source(src);
    let mut problems = Vec::new();
    for bad in ["assert_eq!", "assert_ne!", "debug_assert_eq!", ".unwrap()", ".expect("] {
        for (at, _) in code.match_indices(bad) {
            let line = code[..at].lines().count();
            problems.push(format!("{name}:{line}: {bad}"));
        }
    }
    for mac in ["panic!(", "assert!(", "unreachable!(", "todo!(", "unimplemented!("] {
        for (at, _) in code.match_indices(mac) {
            if mac == "assert!(" && code[..at].ends_with("debug_") {
                continue;
            }
            let line = code[..at].lines().count();
            let args = top_level_args(macro_args(&code, at + mac.len() - 1));
            let message = if mac == "assert!(" { args.get(1..) } else { args.get(0..) }.unwrap_or(&[]);
            let Some((fmt, rest)) = message.split_first() else { continue };
            if !rest.is_empty() || !(fmt.starts_with('"') && fmt.ends_with('"')) {
                problems.push(format!("{name}:{line}: {mac} message is not one string literal: {}", message.join(", ")));
                continue;
            }
            let mut chars = fmt.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '{' {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        continue;
                    }
                    let name_: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    if name_.is_empty() || !name_.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                        problems.push(format!("{name}:{line}: {mac} interpolates {{{name_}}}"));
                    }
                }
            }
        }
    }
    problems
}

#[test]
fn no_program_panic_can_print_a_value() {
    let bridge = scan("bridge/src/lib.rs", include_str!("../src/lib.rs"));
    let probe = scan("attest/probe/src/lib.rs", include_str!("../../attest/probe/src/lib.rs"));
    let attest = scan("attest/program/src/lib.rs", include_str!("../../attest/program/src/lib.rs"));
    assert!(bridge.is_empty() && probe.is_empty(), "{bridge:#?}\n{probe:#?}");
    // The only exceptions: the light-client update and bootstrap verifiers' errors (Helios),
    // about public beacon-chain data, never on a claim, burn or root path; their texts are pinned
    // by attest/program/tests/instance.rs.
    assert_eq!(attest.len(), 2, "{attest:#?}");
    assert!(attest.iter().all(|p| p.contains("interpolates {e}")), "{attest:#?}");
    // The public path (branch public-update): the same kind of exception, five verifiers
    // (finality, period Begin and Finish, finality Begin and Finish at the pairing seam); texts
    // pinned by attest/program/tests/public_update.rs and finality_split.rs.
    let public = scan("attest/program/src/public.rs", include_str!("../../attest/program/src/public.rs"));
    assert_eq!(public.len(), 5, "{public:#?}");
    assert!(public.iter().all(|p| p.contains("interpolates {e}")), "{public:#?}");
    // The scanner does find what it looks for.
    let planted = "fn f(r: Result<u8, ()>, x: u8) {\n    let a = r.expect(\"decodes\");\n    assert_eq!(a, x, \"same\");\n    assert!(a == x, \"{x} differs\");\n    panic!(\"{}\", x);\n}\n";
    assert_eq!(scan("planted", planted).len(), 4, "{:#?}", scan("planted", planted));
    assert!(scan("ok", "fn f() { assert!(true, \"{CLAIM_REFUSED}\"); panic!(\"fixed\") }").is_empty());
}
