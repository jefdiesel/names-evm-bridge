# Requirements: scope and state, one row per requirement

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

Every numbered requirement of Logos RFP-022 (private wrapped ERC-20 and ETH bridge) and RFP-021 (Ethereum state attestation), in the RFP's own words (shortened), with the scope of this proposal, the milestone that delivers it, and its state on 2026-11-16 (states first counted 2026-11-14; F5, F6, F7, R2, R10, P6, U1 and U2 moved to PUB on 2026-11-16). RFP texts: `logos-co/rfp`, `RFPs/RFP-022-wrapped-erc20.md` and `RFPs/RFP-021-ethereum-state-attestation.md` at commit `916fffc`. The test or run behind each state is named in `DESIGN.md` §12 under the same id; this table re-labels those states.

- **Scope:** In; In-dev (in scope, with a deviation stated in the proposal and in `DESIGN.md` §11); Cond (delivered only if the week-2 gate on a trustless outbound proof says go, milestones M2 and M6); Desc (descoped, reason given).
- **Today:** PUB ran on a public network (the Sepolia testnet, or the team's operator-attested LEZ 0.4 demo chain; named); LOC ran on a local chain; TST unit or simulation tests only; DES designed; NS not started. [ext] marks a dependency on another party.

## Counts (hard requirements)

127 numbered hard requirements: 68 in RFP-022, 59 in RFP-021.

| Scope | Count | Which |
|---|---:|---|
| In, unconditionally | 112 | |
| In, with a stated deviation | 6 | RFP-022 F3, F10, F11, F13, F14, F16 |
| In, with an outbound part that depends on the week-2 gate | 4 | RFP-022 F12 (also a deviation), R10, P5, PP3 |
| Conditional on the week-2 gate | 3 | RFP-022 F7, S9, PP2 |
| Descoped (independent audit; Logos procures it, M8 supports it) | 2 | RFP-022 S1, RFP-021 Sup8 |

| Furthest state any part has reached today | Count |
|---|---:|
| PUB, a public network | 16 |
| LOC, a local chain | 38 |
| TST, tests only | 26 |
| DES, designed | 26 |
| NS, not started | 21 |

Soft requirements (10): 5 offered (RFP-022 Soft1 as a written migration path only, Soft3, Soft4, Soft5; RFP-021 Soft5), 5 not offered (RFP-022 Soft2; RFP-021 Soft1–4).

## RFP-022

| Id | RFP words | Scope | M | Today |
|---|---|---|---|---|
| F1 | "vault that escrows deposits of any ERC-20 … plus native ETH" | In | 1 | PUB Sepolia |
| F2 | "A deposit must not … reveal its LEZ destination" | In | 1 | PUB Sepolia |
| F3 | mint "on cryptographic verification … consume the attestation primitive" | In-dev (nullifier) | 1, 3 | LOC |
| F4 | "both a private LEZ account and a public LEZ account" | In | 1 | LOC; PUB demo chain |
| F5 | burn "must not … reveal its Ethereum destination" | In | 1 | PUB demo chain (public burns); LOC private |
| F6 | burn from "either a private or a public LEZ account" | In | 1 | PUB demo chain (public, self-paid and relayed); LOC private |
| F7 | vault releases "on cryptographic verification of a valid LEZ burn" | Cond | 4, 6 | PUB Sepolia + demo chain (vault v4: proved update and release, RISC Zero verifier 3.1.0; LEZ finality the submitter's word, roots operator-attested); trustless G2 Cond |
| F8 | amounts "must not identify"; "effect on anonymity-set size" | In | 1, 7 | PUB denominations; set size NS |
| F9 | "recover every one of their own unclaimed deposits and unreleased burns" | In | 1, 5 | LOC public accounts; web-page deposits not seed-derived |
| F10 | admin "can register … and deregister a token"; delisting docs | In-dev (own admin) | 1, 7 | PUB register; TST deregister |
| F11 | "Global and per-token deposit and redemption caps" | In-dev (no LEZ cap) | 4 | TST; set on Sepolia |
| F12 | finality depth "configurable by the admin authority per deployment" | In-dev; outbound Cond | 4, 6 | inbound LOC; outbound NS |
| F13 | "protocol fee … on both minting and burning … on LEZ" | In-dev (Ethereum side) | 1 | TST |
| F14 | freeze "globally or for a single registered token … independently" | In-dev (own code) | 1 | TST; LEZ global freeze LOC |
| F15 | each deployment "refers to a specific" deployment on the other chain | In | 1, 4 | Ethereum side PUB; LEZ side PUB demo chain (v3, deployed, first public claim block 4703) |
| F16 | "multiple entities each operate under their own independent configuration" | In-dev (one config per program) | 1, 7 | TST |
| F17 | ERC-20s whose transfer "do not return a boolean (USDT)" | In | 1 | TST |
| U1 | "a Logos core module" for users and admin | In | 5 | PUB demo chain (unreleased builds in Basecamp); TST |
| U2 | "a Logos mini-app" incl. registry view and admin UI | In | 5 | PUB demo chain ("Claim a deposit", unreleased); admin UI TST |
| U3 | "Logos module accompanied by a Logos Core headless CLI/daemon" | In | 5 | NS |
| U4 | "an IDL … using the SPEL framework" | In | 5 | NS [ext] |
| U5 | mitigations "enabled by default"; leak indicator | In | 4, 5 | LOC one-time recipient; TST indicator; delay NS |
| U6 | the private path "is the pre-selected option" | In | 5 | TST |
| U7 | "explain what is public and what is private at each step" | In | 7 | DES |
| U8 | "clear, actionable error messages" that do not reveal the deposit | In | 1 | LOC |
| U9 | "change the targeted Ethereum RPC … and … LEZ sequencer" | In | 5 | TST |
| U10 | modules "in a module catalog of the team's own" | In | 5 | NS |
| R1 | "Minting is atomic" | In | 1 | LOC |
| R2 | "Redemption is atomic at each stage" | In | 1 | PUB burn and release (v4); failure paths TST |
| R3 | "No deposit can be claimed twice and no burn redeemed twice" | In | 1, 4 | LOC one chain; across chains TST (v3) |
| R4 | "A valid claim remains valid indefinitely" | In | 1 | TST |
| R5 | "Position recovery is complete" | In | 1 | LOC public accounts |
| R6 | RPC failure "able to resume without duplicating work" | In | 5 | NS |
| R7 | "interrupted user-side operation does not consume … entitlement" | In | 4 | DES |
| R8 | "mature, audited proof-system implementations" | In | 1 | TST (RISC Zero: Ethereum-side provers and verifier 3.1.0, LEZ side 3.0.5 as LEZ rc3; Helios) |
| R9 | third-party outage: "document the effect, and mitigate it" | In | 7 | DES |
| R10 | "full deposit and redemption round trip … included in CI" | In; with real proofs Cond | 6, 7 | PUB once with real proofs (v4; LEZ finality the submitter's word); LOC mock-verified; not in CI |
| R11 | "Every hard requirement has at least one corresponding test" | In | 7 | DES (NO TEST rows remain) |
| R12–14 | docs: core module, CLI and operator, deployer journey | In | 5, 7 | NS |
| P1 | claim and mint "within a single LEZ public transaction" | In | 1 | LOC 390,756 cycles |
| P2 | extra LEZ-side verification and "amortised per-deposit cost" | In | 1, 7 | LOC measured; amortised DES |
| P3 | "Ethereum-side gas cost of a deposit, of a release" | In | 1, 4 | PUB deposits and a release (v4, real seal) |
| P4 | proving "on a mid-range laptop and on the lowest specification" | In | 4 | LOC one desktop |
| P5 | latency "broken down by" finality, proving, delay, verification | In; outbound Cond | 7 | DES |
| P6 | "compute resources … of any off-chain component" | In | 1, 7 | PUB outbound updater and wrap (v4); LOC light-client updater |
| P7 | state growth "projected at 1M and 10M operations" | In | 7 | TST partial |
| Sup1 | "deployed and tested on a public Ethereum testnet and LEZ testnet 0.4" | In | 7 | PUB Sepolia; testnet 0.4 NS [ext] |
| Sup2 | "modules work on both Linux and MacOS" | In | 5 | NS |
| Sup3 | built with "logos-modules-release-action" | In | 5 | NS |
| Sup4 | "design decision log in repo" | In | 1 | DES (DESIGN.md §11) |
| S1 | "independent third-party … audit"; report published | Desc: Logos procures; we support (M8) | 8 | NS |
| S2 | both sides "must reject invalid proofs" | In | 1, 4 | LOC; TST |
| S3 | a submitter cannot "redirect funds, inflate their fee, or replay" | In | 1, 4 | TST (H2 fix, v3) |
| S4 | caps: "document recommended defaults and the reasoning" | In | 4 | DES (`DESIGN.md` §6.1a) |
| S5 | freeze "exercisable independently on each half" | In | 1 | LOC; TST |
| S6 | "total wrapped supply on LEZ must never exceed the vault's holdings" | In | 1, 4 | LOC; TST (C1 fix, v3) |
| S7 | user docs state "the trustless verification model" | In | 7 | DES |
| S8 | "The verification logic cannot be swapped" | In | 1 | LOC immutable; TST |
| S9 | failsafe that "must not become a backdoor" | Cond (built in M6; with no-go, documented only) | 6 | DES |
| PP1 | "Inbound unlinkability" | In | 7 | LOC leak checks; measurement NS |
| PP2 | "Outbound unlinkability" | Cond | 6, 7 | DES |
| PP3 | no destination revealed "over full event and state diffs for a complete round trip" | In; round trip Cond | 7 | LOC inbound; round trip NS |
| PP4 | linking information "must never leave the user's control" | In | 1, 5 | LOC (demo "we claim for you" path is an exception until M5) |
| PP5 | rejected, repeat and cap rejection "indistinguishable" | In | 1 | LOC; TST |
| PP6 | "document precisely what that party learns"; switch per operation | In | 5 | DES; per-claim relay choice exists |
| PP7 | "The default configuration must be the private one" | In | 1, 5 | LOC one-time recipient; rest DES |
| Soft1 | "Committed amounts"; "document the intended migration path" | In: path only | 7 | NS |
| Soft2 | "viewing keys" | Desc: not offered | — | NS |
| Soft3 | "per-token release delay" | In | 1 | TST |
| Soft4 | "pluggable" proof-system components | In | 1 | DES (image ids per vault version) |
| Soft5 | "Common Ethereum wallet compatibility" | In | 1 | PUB deposit page, Sepolia |

## RFP-021

| Id | RFP words | Scope | M | Today |
|---|---|---|---|---|
| F1 | "Run the Ethereum light client inside the LEZ program" | In | 1 | LOC proved update |
| F2 | "reject any header that is not" finalised | In | 1 | TST |
| F3 | inclusion of "an account, a contract storage slot, and an event log" | In | 1, 3 | LOC slot; TST account; logs synthetic |
| F4 | predicates "at least equality and inequality" | In | 1 | TST |
| F5 | chain binding "tested across mainnet and at least one testnet with the same contract address" | In | 3 | TST; same-address test NS |
| F6 | "deterministic, collision-resistant identifier" | In | 1 | TST |
| F7 | "implemented and tested as a LEZ program on testnet" | In | 3, 7 | LOC; testnet NS [ext] |
| F8 | "must not introduce any correlation signal" | In | 1, 7 | LOC byte searches; full diffs NS |
| F9 | parameters write-once; core module API to "verify an instance's checkpoint" | In | 1, 3 | TST; API NS [ext] |
| F10 | outside the weak-subjectivity period "replaced by a freshly initialised instance" | In | 3 | DES |
| F11 | "Advancing the tracked consensus state is permissionless" | In | 1 | LOC (fresh account) |
| F12 | "liveness roles, not trust roles" | In | 1, 7 | DES |
| F13 | "no instruction that rewrites an initialised instance's checkpoint" | In | 1 | TST |
| F14 | "any Ethereum-consensus chain, mainnet or testnet" | In | 1 | LOC Sepolia; TST mainnet |
| F15 | "Multiple independent instances coexist under the same program" | In | 1 | LOC |
| U1 | "a Logos core module" for consumers and operators | In | 3, 5 | NS |
| U2 | CLI incl. a dry run with "the same typed error codes" | In | 3 | LOC CLI; codes NS |
| U3 | "update-submitting component … Logos module … headless CLI/daemon" | In | 3 | NS |
| U4 | "an IDL … using the SPEL framework" | In | 3 | NS [ext] |
| U5 | "clear, actionable error messages for all failure modes" (eight named) | In | 3 | DES |
| U6 | "reference consumer programs": ERC-20 and native ETH | In | 3 | LOC probe and bridge; the two NS |
| R1 | "Verification is deterministic" | In | 1 | TST |
| R2 | "read-only with respect to consumer state" | In | 1 | TST |
| R3 | "remains valid indefinitely" through rotations | In | 1 | TST |
| R4 | "Committee handoff tracking survives interruption" | In | 1, 3 | TST program; daemon NS |
| R5 | RPC failure: "able to resume without duplicating work" | In | 3 | NS |
| R6 | "stating what is reused, what is rewritten, and the cost and audit-surface consequences" | In | 1, 3 | TST; audit-surface statement NS |
| R7 | "a stalled instance resumes correctly … no gap in the verified handoff chain" | In | 3 | LOC one period; TST one boundary; more NS |
| R8 | "a duplicate or already-applied update is rejected cleanly" | In | 1 | TST |
| R9 | "checks whether an update is still needed immediately before submitting" | In | 3 | NS |
| R10 | "defers submission by an interval it varies per instance" | In | 3 | NS |
| R11 | "Statement submissions are independent of one another" | In | 1 | TST |
| R12 | "CI must be green on the default branch" | In | 1 | TST suites green in CI |
| P1 | attestation and predicate "within a single LEZ transaction" | In | 1, 3 | LOC 263K cycles; updates: private today, two public steps in M3 (`DESIGN.md` §9.3a) |
| P2 | "cycle count, gas cost, proof time, and proof size for each" | In | 1, 3 | LOC; proof size NS |
| P3 | "amortised per-attestation cost of sync-committee tracking" | In | 7 | DES |
| P4 | proving "on a mid-range laptop", "including … a private transaction" | In | 4 | LOC one desktop |
| P5 | "end-to-end attestation latency" breakdown | In | 7 | DES |
| P6 | "compute resources … of any off-chain component" | In | 1 | LOC updater |
| P7 | state growth "over one and five years" | In | 7 | DES |
| P8 | "cycle-count delta" to "a hypothetical native precompile" | In | 1 | TST |
| P9 | "No action's cycle count or gas cost scales with accumulated on-chain storage" | In | 1 | DES (fixed-size store) |
| Sup1 | "deployed and tested on LEZ testnet 0.4" | In | 3, 7 | NS [ext] |
| Sup2 | "End-to-end integration tests … included in CI" | In | 7 | NS |
| Sup3–5 | docs: core module, "Recommended Consumer Pattern", CLI and operators | In | 7 | DES |
| Sup6 | "trust assumptions document … the cost of the attack it admits" | In | 7 | DES (`DESIGN.md` §9.8) |
| Sup7 | "Document the operator role" | In | 3 | DES |
| Sup8 | "independent third-party security audit"; report published | Desc: Logos procures; we support (M8) | 8 | NS |
| Sup9–10 | catalog, "logos-modules-release-action" | In | 5 | NS |
| Sup11 | "design decision log in repo" | In | 1 | DES |
| VS1 | "reject a fabricated chain … exercised explicitly" | In | 1 | LOC; TST |
| VS2 | the six named rejection cases | In | 1 | TST |
| VS3 | no verification "through a ProgramId held in mutable account state" | In | 1 | LOC immutable |
| VS4 | "failure modes that follow from a compromised or colluding sync committee" | In | 7 | DES (`DESIGN.md` §9.8) |
| VS5 | docs state "the trustless verification model" | In | 7 | DES |
| VS6 | "accepted on proof validity alone" | In | 1 | LOC |
| Soft1–4 | recorded-statement account, batching, historical state, richer predicates | Desc: not offered | — | NS |
| Soft5 | "Operator incentive" | In | 3 | DES |
