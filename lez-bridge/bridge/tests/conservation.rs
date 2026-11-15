//! Cross-side conservation (audit B1): seeded pseudo-random interleavings of deposits (vault
//! leaves), root attestations, claims of any pending deposit under any attested root that holds
//! it, wrapped-token transfers, burns of any registered denomination by any holder, freezes and
//! unfreezes, float top-ups and deliberately invalid attempts, run against the real bridge and
//! token handlers in the simulator (`common/mod.rs`). After every step, per token:
//!
//! - `claims[d] <= deposits[d]` for every denomination;
//! - `burned_value <= claimed_value`;
//! - total supply `== claimed_value - burned_value`;
//! - the sum of every account's balance `==` total supply;
//!
//! and the bridge's counters, every holder's balance and the float equal an independent model,
//! which also predicts, before each step, whether it is accepted.
//!
//! # The trace fixture
//!
//! The accepted deposits, root attestations, claims and burns of every run are written to
//! `tests/fixtures/conservation_trace.json`, so a Foundry test can replay the same history against
//! `BridgeVault` (deposit the leaves in order, check each recorded root, and release the exits).
//! The test regenerates the trace and compares it with the file; to rewrite the file run it with
//! `UPDATE_CONSERVATION_TRACE=1`. Schema `rfp022-conservation-trace/v1`:
//!
//! ```text
//! {
//!   "schema": "rfp022-conservation-trace/v1",
//!   "description": "...",
//!   "deployment": { "bridge_program", "cfg_id", "vault", "lez_chain_id", "zone_id": 0x-hex,
//!                   "evm_chain_id": number },
//!   "tokens": [ { "eth_token": 0x-hex (20 bytes), "denoms": [decimal string, ...] } ],
//!   "runs": [ {
//!     "seed": number, "steps": number,
//!     "events": [   // in step order; only what was accepted
//!       { "step", "kind": "deposit", "leaf_index", "eth_token", "denom_index", "denom",
//!         "commitment", "leaf" },                       // the vault's deposit(token, denom, C)
//!       { "step", "kind": "root", "leaf_count", "root" },  // root after the first leaf_count leaves
//!       { "step", "kind": "claim", "leaf_index", "root", "nullifier", "eth_token",
//!         "denom_index", "denom", "one_time" },
//!       { "step", "kind": "burn", "eth_token", "denom_index", "denom", "exit_secret", "eth_dest",
//!         "max_relay_fee", "exit_commitment", "exit_leaf" }
//!     ],
//!     "final": [ { "eth_token", "deposits": [n per denom], "claims": [..], "burns": [..],
//!                  "claimed_value", "burned_value", "supply" } ]
//!   } ]
//! }
//! ```
//!
//! Byte strings are lowercase `0x`-hex; amounts are decimal strings (they can exceed 2^53);
//! counts, indexes, steps and seeds are JSON numbers. `exit_leaf` is
//! `exit_leaf(cfgId, token, denom, E)`, the leaf the vault's exit tree will hold.

mod common;

use common::*;

const STEPS: usize = 200;
const SEEDS: [u64; 3] = [1, 2, 0x5EED_2026_1112];
const HOLDERS: [AccountId; 4] = [ALICE, BOB, CAROL, DAVE];
const SPONSOR: u128 = 3;
const MAX_RELAY_FEE: u128 = 1_000;

/// splitmix64: a fixed, dependency-free PRNG.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

struct TokenModel {
    eth: EthAddress,
    def: AccountId,
    denoms: Vec<u128>,
    deposits: Vec<u64>,
    claims: Vec<u64>,
    burns: Vec<u64>,
    claimed: u128,
    burned: u128,
    mint_frozen: bool,
    burn_frozen: bool,
}

struct Deposit {
    k: [u8; 32],
    dest: AccountId,
    token: usize,
    denom: usize,
    claimed: bool,
}

struct Model {
    tokens: Vec<TokenModel>,
    deposits: Vec<Deposit>,
    leaves: Vec<[u8; 32]>,
    /// Leaf counts of the attested roots.
    roots: Vec<usize>,
    global_mint: bool,
    global_burn: bool,
    float: u128,
    balances: BTreeMap<(AccountId, usize), u128>,
}

impl Model {
    fn bal(&self, who: AccountId, t: usize) -> u128 {
        self.balances.get(&(who, t)).copied().unwrap_or(0)
    }

    fn add(&mut self, who: AccountId, t: usize, v: u128) {
        *self.balances.entry((who, t)).or_insert(0) += v;
    }

    fn sub(&mut self, who: AccountId, t: usize, v: u128) {
        let b = self.balances.get_mut(&(who, t)).expect("a balance");
        *b -= v;
    }

    fn mint_frozen(&self, t: usize) -> bool {
        self.global_mint || self.tokens[t].mint_frozen
    }

    fn burn_frozen(&self, t: usize) -> bool {
        self.global_burn || self.tokens[t].burn_frozen
    }

    fn attested_count(&self) -> usize {
        self.roots.iter().copied().max().unwrap_or(0)
    }
}

fn h(b: &[u8]) -> String {
    format!("\"0x{}\"", hex(b))
}

fn s(v: u128) -> String {
    format!("\"{v}\"")
}

fn list<T: std::fmt::Display>(xs: &[T]) -> String {
    format!("[{}]", xs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
}

fn world() -> World {
    let mut w = setup();
    w.tx(DEF2, TOKEN, &Tok::NewFungibleDefinition { name: "Wrapped DAI".into(), total_supply: 0, mint_authority: Some(Authority::Program(BRIDGE)), supply_to: ADMIN })
        .unwrap();
    w.tx(ADMIN, BRIDGE, &instr(Instruction::Admin(AdminOp::RegisterToken { eth_token: DAI, definition: DEF2, decimals: 18, denoms: vec![100, 1_000] })))
        .unwrap();
    set_sponsor(&mut w, SPONSOR);
    fund_float(&mut w, 30);
    w
}

fn model() -> Model {
    let tok = |eth, def, denoms: Vec<u128>| TokenModel {
        eth,
        def,
        deposits: vec![0; denoms.len()],
        claims: vec![0; denoms.len()],
        burns: vec![0; denoms.len()],
        denoms,
        claimed: 0,
        burned: 0,
        mint_frozen: false,
        burn_frozen: false,
    };
    Model {
        tokens: vec![tok(USDC, DEF, vec![100_000_000, 1_000_000_000]), tok(DAI, DEF2, vec![100, 1_000])],
        deposits: Vec::new(),
        leaves: Vec::new(),
        roots: Vec::new(),
        global_mint: false,
        global_burn: false,
        float: 30,
        balances: BTreeMap::new(),
    }
}

/// The invariants, and agreement with the model, after `step`.
fn check(w: &World, m: &Model, seed: u64, step: usize, what: &str) {
    let c = w.config();
    for (i, t) in m.tokens.iter().enumerate() {
        let e = &c.tokens[i];
        let at = format!("seed {seed} step {step} ({what}), token {i}");
        for d in 0..t.denoms.len() {
            assert!(e.claims[d] <= t.deposits[d], "{at}: claims[{d}] {} > deposits {}", e.claims[d], t.deposits[d]);
        }
        assert!(e.burned_value <= e.claimed_value, "{at}: burned > claimed");
        let supply = w.supply_of(t.def);
        assert_eq!(supply, e.claimed_value - e.burned_value, "{at}: supply != claimed - burned");
        assert_eq!(w.holdings_sum(t.def), supply, "{at}: sum of balances != supply");
        // The model.
        assert_eq!((&e.claims, &e.burns, e.claimed_value, e.burned_value), (&t.claims, &t.burns, t.claimed, t.burned), "{at}: counters");
        for who in HOLDERS {
            assert_eq!(w.bal(who, t.def), m.bal(who, i), "{at}: balance of {who:?}");
        }
        assert_eq!((e.mint_frozen, e.burn_frozen), (t.mint_frozen, t.burn_frozen), "{at}: token flags");
    }
    assert_eq!((c.mint_frozen, c.burn_frozen), (m.global_mint, m.global_burn), "seed {seed} step {step}: global flags");
    assert_eq!(w.float(), m.float, "seed {seed} step {step}: float");
}

/// `r` matches the prediction `want` (`None`: accepted; `Some(text)`: refused with it), and a
/// refusal changed nothing.
#[track_caller]
fn expect(r: Result<(), String>, want: Option<&str>, w: &World, before: &World, at: &str) {
    match (r, want) {
        (Ok(()), None) => {}
        (Err(e), Some(t)) => {
            assert!(e.contains(t), "{at}: wanted refusal '{t}', got '{e}'");
            assert!(w.states == before.states, "{at}: a refusal changed state");
        }
        (Ok(()), Some(t)) => panic!("{at}: accepted, but the model predicted '{t}'"),
        (Err(e), None) => panic!("{at}: refused with '{e}', but the model predicted acceptance"),
    }
}

/// One run. Returns the run's JSON object.
fn run(seed: u64) -> String {
    let mut rng = Rng(seed);
    let mut w = world();
    let mut m = model();
    let mut events: Vec<String> = Vec::new();
    let notify = Actor::new(config_address(&BRIDGE), BRIDGE);
    let mut n_secret = 0u64;
    let mut fresh = |tag: &str| {
        n_secret += 1;
        secret(&format!("conservation/{seed}/{tag}"), n_secret)
    };
    let (mut n_claims, mut n_burns, mut n_refused) = (0, 0, 0);
    for step in 0..STEPS {
        let before = w.clone();
        let at = format!("seed {seed} step {step}");
        // Pick an action that applies in this state.
        let pending: Vec<usize> = (0..m.deposits.len()).filter(|&i| !m.deposits[i].claimed && i < m.attested_count()).collect();
        let holders_with: Vec<(AccountId, usize)> = HOLDERS.iter().flat_map(|&a| (0..2).map(move |t| (a, t))).filter(|&(a, t)| m.bal(a, t) > 0).collect();
        let action = loop {
            let a = rng.below(100);
            let kind = match a {
                0..20 => "deposit",
                20..30 => "root",
                30..55 => "claim",
                55..68 => "transfer",
                68..83 => "burn",
                83..91 => "freeze",
                91..94 => "fund",
                _ => "invalid",
            };
            let ok = match kind {
                "root" => m.leaves.len() > m.attested_count(),
                "claim" => !pending.is_empty(),
                "transfer" | "burn" => !holders_with.is_empty(),
                _ => true,
            };
            if ok {
                break kind;
            }
        };
        match action {
            "deposit" => {
                let (dest, t) = (rng.pick(&HOLDERS), rng.below(2));
                let d = rng.below(m.tokens[t].denoms.len());
                let k = fresh("k");
                let tm = &m.tokens[t];
                let c = deposit_commitment(&k, &dest, &tm.eth, tm.denoms[d], &cfg());
                let leaf = deposit_leaf(&cfg(), &tm.eth, tm.denoms[d], &c);
                events.push(format!(
                    "{{\"step\": {step}, \"kind\": \"deposit\", \"leaf_index\": {}, \"eth_token\": {}, \"denom_index\": {d}, \"denom\": {}, \"commitment\": {}, \"leaf\": {}}}",
                    m.leaves.len(),
                    h(&tm.eth),
                    s(tm.denoms[d]),
                    h(&c),
                    h(&leaf)
                ));
                m.tokens[t].deposits[d] += 1;
                m.leaves.push(leaf);
                m.deposits.push(Deposit { k, dest, token: t, denom: d, claimed: false });
            }
            "root" => {
                let n = m.leaves.len();
                let root = root_of(&m.leaves);
                let want = m.global_mint.then_some("minting is frozen");
                let r = w.tx(ATTESTOR, BRIDGE, &instr(Instruction::AttestRoot { root }));
                expect(r, want, &w, &before, &at);
                if want.is_none() {
                    m.roots.push(n);
                    events.push(format!("{{\"step\": {step}, \"kind\": \"root\", \"leaf_count\": {n}, \"root\": {}}}", h(&root)));
                } else {
                    n_refused += 1;
                }
            }
            "claim" => {
                let i = rng.pick(&pending);
                // Any attested root that holds the deposit (older roots stay valid).
                let holding: Vec<usize> = m.roots.iter().copied().filter(|&n| n > i).collect();
                let n = rng.pick(&holding);
                let prefix = m.leaves[..n].to_vec();
                let (k, dest, t, d) = (m.deposits[i].k, m.deposits[i].dest, m.deposits[i].token, m.deposits[i].denom);
                let opening = (rng.below(2) == 0).then(|| recipient_opening(&k));
                let denom = m.tokens[t].denoms[d];
                let msg = claim_msg(cfg(), m.tokens[t].eth, k, i, &prefix, d as u32, denom, opening);
                let want = if m.mint_frozen(t) {
                    Some("minting is frozen")
                } else if opening.is_some() && m.float < SPONSOR {
                    Some("holds less than the transferred amount")
                } else {
                    None
                };
                let r = w.tx_with_openings(dest, BRIDGE, &msg, opening.as_slice());
                expect(r, want, &w, &before, &at);
                if want.is_none() {
                    let tm = &mut m.tokens[t];
                    tm.claims[d] += 1;
                    tm.claimed += denom;
                    m.deposits[i].claimed = true;
                    m.add(dest, t, denom);
                    if opening.is_some() {
                        m.float -= SPONSOR;
                    }
                    n_claims += 1;
                    events.push(format!(
                        "{{\"step\": {step}, \"kind\": \"claim\", \"leaf_index\": {i}, \"root\": {}, \"nullifier\": {}, \"eth_token\": {}, \"denom_index\": {d}, \"denom\": {}, \"one_time\": {}}}",
                        h(&root_of(&prefix)),
                        h(&claim_nullifier(&k, &dest, &m.tokens[t].eth, denom)),
                        h(&m.tokens[t].eth),
                        s(denom),
                        opening.is_some()
                    ));
                } else {
                    n_refused += 1;
                }
            }
            "transfer" => {
                let (from, t) = rng.pick(&holders_with);
                let to = rng.pick(&HOLDERS.iter().copied().filter(|a| *a != from).collect::<Vec<_>>());
                let bal = m.bal(from, t);
                let amount = 1 + (rng.next() as u128 % bal);
                let r = w.tx(from, TOKEN, &Tok::Transfer { definition: m.tokens[t].def, to, amount });
                expect(r, None, &w, &before, &at);
                m.sub(from, t, amount);
                m.add(to, t, amount);
            }
            "burn" => {
                let (who, t) = rng.pick(&holders_with);
                let d = rng.below(m.tokens[t].denoms.len());
                let denom = m.tokens[t].denoms[d];
                let exit_secret = fresh("exit");
                let eth_dest: EthAddress = fresh("dest")[..20].try_into().unwrap();
                let e = exit_commitment(&exit_secret, &eth_dest, &m.tokens[t].eth, denom, MAX_RELAY_FEE, &cfg());
                let want = if m.bal(who, t) < denom {
                    Some("insufficient token balance")
                } else if m.burn_frozen(t) {
                    Some("redemption is frozen")
                } else {
                    None
                };
                let r = w.tx(who, TOKEN, &Tok::BurnWithNotice { definition: m.tokens[t].def, amount: denom, notify, memo: e });
                expect(r, want, &w, &before, &at);
                if want.is_none() {
                    let tm = &mut m.tokens[t];
                    tm.burns[d] += 1;
                    tm.burned += denom;
                    m.sub(who, t, denom);
                    n_burns += 1;
                    events.push(format!(
                        "{{\"step\": {step}, \"kind\": \"burn\", \"eth_token\": {}, \"denom_index\": {d}, \"denom\": {}, \"exit_secret\": {}, \"eth_dest\": {}, \"max_relay_fee\": {}, \"exit_commitment\": {}, \"exit_leaf\": {}}}",
                        h(&m.tokens[t].eth),
                        s(denom),
                        h(&exit_secret),
                        h(&eth_dest),
                        s(MAX_RELAY_FEE),
                        h(&e),
                        h(&exit_leaf(&cfg(), &m.tokens[t].eth, denom, &e))
                    ));
                } else {
                    n_refused += 1;
                }
            }
            "freeze" => {
                // Unfreezing a little more often than freezing keeps the run moving.
                let set = rng.below(5) < 2;
                let scope = rng.below(3); // 0 global, 1 USDC, 2 DAI
                let (mint, burn) = [(true, false), (false, true), (true, true)][rng.below(3)];
                let eth = (scope > 0).then(|| m.tokens[scope - 1].eth);
                let (who, msg) = if set { (FREEZER, freeze(eth, mint, burn)) } else { (ADMIN, unfreeze(eth, mint, burn)) };
                w.tx(who, BRIDGE, &msg).unwrap_or_else(|e| panic!("{at}: {e}"));
                let (fm, fb) = if scope == 0 {
                    (&mut m.global_mint, &mut m.global_burn)
                } else {
                    let tm = &mut m.tokens[scope - 1];
                    (&mut tm.mint_frozen, &mut tm.burn_frozen)
                };
                if mint {
                    *fm = set;
                }
                if burn {
                    *fb = set;
                }
            }
            "fund" => {
                fund_float(&mut w, 10);
                m.float += 10;
            }
            _ => {
                // A deliberately invalid attempt; each must be refused and change nothing.
                n_refused += 1;
                let claimed: Vec<usize> = (0..m.deposits.len()).filter(|&i| m.deposits[i].claimed).collect();
                match rng.below(5) {
                    0 if !claimed.is_empty() => {
                        // Claim a claimed deposit again, under the newest root.
                        let i = rng.pick(&claimed);
                        let n = m.attested_count();
                        let (k, dest, t, d) = (m.deposits[i].k, m.deposits[i].dest, m.deposits[i].token, m.deposits[i].denom);
                        let msg = claim_msg(cfg(), m.tokens[t].eth, k, i, &m.leaves[..n], d as u32, m.tokens[t].denoms[d], None);
                        let r = w.tx(dest, BRIDGE, &msg);
                        expect(r, Some(CLAIM_REFUSED), &w, &before, &at);
                        assert_eq!(message_fault(dest, &msg), None, "{at}: a repeat's witness is fine");
                    }
                    1 if m.leaves.len() > m.attested_count() => {
                        // Claim the newest deposit under a root nobody attested yet.
                        let i = m.leaves.len() - 1;
                        let (k, dest, t, d) = (m.deposits[i].k, m.deposits[i].dest, m.deposits[i].token, m.deposits[i].denom);
                        let msg = claim_msg(cfg(), m.tokens[t].eth, k, i, &m.leaves, d as u32, m.tokens[t].denoms[d], None);
                        let r = w.tx(dest, BRIDGE, &msg);
                        expect(r, Some(CLAIM_REFUSED), &w, &before, &at);
                        assert_eq!(message_fault(dest, &msg), None, "{at}: the root PDA refused it");
                    }
                    2 if !pending.is_empty() => {
                        // Someone else claims a pending deposit.
                        let i = rng.pick(&pending);
                        let n = m.attested_count();
                        let (k, dest, t, d) = (m.deposits[i].k, m.deposits[i].dest, m.deposits[i].token, m.deposits[i].denom);
                        let thief = rng.pick(&HOLDERS.iter().copied().filter(|a| *a != dest).collect::<Vec<_>>());
                        let msg = claim_msg(cfg(), m.tokens[t].eth, k, i, &m.leaves[..n], d as u32, m.tokens[t].denoms[d], None);
                        let r = w.tx(thief, BRIDGE, &msg);
                        expect(r, Some(CLAIM_REFUSED), &w, &before, &at);
                        assert_eq!(message_fault(thief, &msg), Some(ClaimFault::NotInRoot), "{at}");
                    }
                    3 if !holders_with.is_empty() => {
                        // Burn more than the balance.
                        let (who, t) = rng.pick(&holders_with);
                        let amount = m.bal(who, t) + 1;
                        let r = w.tx(who, TOKEN, &Tok::BurnWithNotice { definition: m.tokens[t].def, amount, notify, memo: [0; 32] });
                        expect(r, Some("insufficient token balance"), &w, &before, &at);
                    }
                    _ => {
                        // A burn notice the token program did not send.
                        let t = rng.below(2);
                        let forged = BurnNotice { definition: m.tokens[t].def, amount: m.tokens[t].denoms[0], memo: [0; 32] };
                        let mut asyncs = VecDeque::new();
                        let thief = rng.pick(&HOLDERS);
                        let r = w.deliver(Some(Actor::new(thief, YES)), notify, borsh::to_vec(&forged).unwrap(), BTreeSet::new(), None, &mut asyncs);
                        assert!(r.unwrap_err().contains("a burn notice comes from the token program"), "{at}");
                        assert!(w.states == before.states, "{at}: a forged notice changed state");
                        w = before.clone();
                    }
                }
            }
        }
        check(&w, &m, seed, step, action);
    }
    assert!(n_claims >= 10 && n_burns >= 5 && n_refused >= 5, "seed {seed}: too little happened ({n_claims} claims, {n_burns} burns, {n_refused} refusals)");
    let fin: Vec<String> = m
        .tokens
        .iter()
        .map(|t| {
            format!(
                "{{\"eth_token\": {}, \"deposits\": {}, \"claims\": {}, \"burns\": {}, \"claimed_value\": {}, \"burned_value\": {}, \"supply\": {}}}",
                h(&t.eth),
                list(&t.deposits),
                list(&t.claims),
                list(&t.burns),
                s(t.claimed),
                s(t.burned),
                s(t.claimed - t.burned)
            )
        })
        .collect();
    format!(
        "    {{\n      \"seed\": {seed},\n      \"steps\": {STEPS},\n      \"events\": [\n        {}\n      ],\n      \"final\": [\n        {}\n      ]\n    }}",
        events.join(",\n        "),
        fin.join(",\n        ")
    )
}

fn trace() -> String {
    let runs: Vec<String> = SEEDS.iter().map(|&seed| run(seed)).collect();
    let tokens = [(USDC, vec![100_000_000u128, 1_000_000_000]), (DAI, vec![100, 1_000])]
        .iter()
        .map(|(eth, ds)| format!("{{\"eth_token\": {}, \"denoms\": [{}]}}", h(eth), ds.iter().map(|d| s(*d)).collect::<Vec<_>>().join(", ")))
        .collect::<Vec<_>>()
        .join(",\n    ");
    format!(
        "{{\n  \"schema\": \"rfp022-conservation-trace/v1\",\n  \"description\": \"Accepted deposits, root attestations, claims and burns of seeded random interleavings run against the LEZ bridge and token programs (bridge/tests/conservation.rs). Replay against BridgeVault: deposit the leaves in order, check each root, release the exits; per token the released value must not exceed the claimed value.\",\n  \"deployment\": {{\"bridge_program\": {}, \"cfg_id\": {}, \"vault\": {}, \"evm_chain_id\": {EVM_CHAIN}, \"lez_chain_id\": {}, \"zone_id\": {}}},\n  \"tokens\": [\n    {tokens}\n  ],\n  \"runs\": [\n{}\n  ]\n}}\n",
        h(BRIDGE.value()),
        h(&cfg()),
        h(&VAULT),
        h(&LEZ_CHAIN),
        h(&source_id(&RootSource::StandIn)),
        runs.join(",\n")
    )
}

#[test]
fn value_is_conserved_across_random_interleavings_and_the_trace_fixture_is_current() {
    let t = trace();
    // Deterministic: the same seeds give the same trace.
    assert_eq!(t, trace());
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/conservation_trace.json");
    if std::env::var_os("UPDATE_CONSERVATION_TRACE").is_some() {
        std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures")).unwrap();
        std::fs::write(path, &t).unwrap();
    }
    let on_disk = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e} (run with UPDATE_CONSERVATION_TRACE=1)"));
    assert!(on_disk == t, "{path} is stale: rerun with UPDATE_CONSERVATION_TRACE=1");
}
