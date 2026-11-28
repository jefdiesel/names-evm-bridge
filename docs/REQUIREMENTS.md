# Requirements: scope and state, one row per requirement

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

Every numbered requirement of Logos RFP-022 (private wrapped ERC-20 and ETH bridge) and RFP-021 (Ethereum state attestation), in the RFP's own words (shortened), with the scope of this proposal, the milestone that delivers it, and its state on 2026-11-18 (states first counted 2026-11-14, moved on 2026-11-16, and brought to `docs/EVIDENCE.md` on main `5322a30` on 2026-11-18, branch `rfp-docs`). RFP texts: `logos-co/rfp`, `RFPs/RFP-022-wrapped-erc20.md` and `RFPs/RFP-021-ethereum-state-attestation.md` at commit `916fffc` (wording unchanged at `920fad92`). The test or run behind each state is named in `DESIGN.md` §12 under the same id; this table re-labels those states.

- **Scope:** In; In-dev (in scope, with a deviation stated in the proposal and in `DESIGN.md` §11); Cond (delivered only if the week-2 gate on a trustless outbound proof says go, milestones M2 and M6); Desc (descoped, reason given).
- **Today:** PUB ran on a public network (the Sepolia testnet, or our own public LEZ v0.4.0-rc3 chain, which runs standalone and is not a Logos network; named); LOC ran on a local chain; TST unit or simulation tests only; DES designed (for a requirement whose deliverable is a document: written in this repository, file named, not yet reviewed by Logos); NS not started. [ext] marks a dependency on another party.
- **What counts.** A state counts when `docs/EVIDENCE.md` on main records it, including results whose code is still on a branch (the branch is named: v6 `v6-lc`, v7 `v7-seqsig`, the light client's public path `public-update`, the open relay `open-relay`). Work on branches that the evidence page does not record yet is noted as "branch X: …" and does not count.

## Counts (hard requirements)

127 numbered hard requirements: 68 in RFP-022, 59 in RFP-021.

| Scope | Count | Which |
|---|---:|---|
| In, unconditionally | 112 | |
| In, with a stated deviation | 6 | RFP-022 F3, F10, F11, F13, F14, F16 |
| In, with an outbound part that depends on the week-2 gate | 4 | RFP-022 F12 (also a deviation), R10, P5, PP3 |
| Conditional on the week-2 gate | 3 | RFP-022 F7, S9, PP2 |
| Descoped (independent audit; Logos procures it, M8 supports it) | 2 | RFP-022 S1, RFP-021 Sup8 |

| Furthest state any part has reached today (2026-11-18) | Count | On 2026-11-16 |
|---|---:|---:|
| PUB, a public network | 30 | 16 |
| LOC, a local chain | 30 | 38 |
| TST, tests only | 25 | 26 |
| DES, designed (or a document written) | 23 | 26 |
| NS, not started | 19 | 21 |

**Summary:** **60 of 127** hard requirements have run on a chain (30 PUB, 30 LOC) and **67** have not yet run on any chain (25 TST, 23 DES, 19 NS), as of 2026-11-18.

Soft requirements (10): 5 offered (RFP-022 Soft1 as a written migration path only, Soft3, Soft4, Soft5; RFP-021 Soft5), 5 not offered (RFP-022 Soft2; RFP-021 Soft1–4).

## RFP-022

| Id | RFP words | Scope | M | Today |
|---|---|---|---|---|
| F1 | "vault that escrows deposits of any ERC-20 … plus native ETH" | In | 1 | PUB Sepolia (v2–v7). Branch `erc20-path`: a USDC/USDT/DAI/WETH matrix, TST |
| F2 | "A deposit must not … reveal its LEZ destination" | In | 1 | PUB Sepolia |
| F3 | mint "on cryptographic verification … consume the attestation primitive" | In-dev (nullifier) | 1, 3 | PUB our public chain: claims with no attester key on v6 and v7 (LEZ 23408, 23909, 24717, 27401; code on branches `v6-lc`, `v7-seqsig`); the default v5 still takes roots from our attester |
| F4 | "both a private LEZ account and a public LEZ account" | In | 1 | PUB our public chain: private (LEZ 3069, 15255, 23909), public (23408) |
| F5 | burn "must not … reveal its Ethereum destination" | In | 1 | PUB our public chain (public burns; private burns LEZ 15395 on v5, 24073 on v6) |
| F6 | burn from "either a private or a public LEZ account" | In | 1 | PUB our public chain (public, self-paid and relayed; private on v5 and v6) |
| F7 | vault releases "on cryptographic verification of a valid LEZ burn" | Cond | 4, 6 | PUB Sepolia + our public chain, real proofs, RISC Zero verifier 3.1.0: v4, v5 and v6 with one submitter key's word on which LEZ blocks count; v7 (branch `v7-seqsig`) on 2 of 3 stand-in signer keys of ours with a contest window (signers check neither privacy receipts nor public execution yet, so on our one sequencer v7 trusts us); LEZ blocks never proved final; trustless G2 Cond |
| F8 | amounts "must not identify"; "effect on anonymity-set size" | In | 1, 7 | PUB denominations; LOC pairing measured over 20 users (`privacy-test-20261117`, development receipts); set-size analysis NS |
| F9 | "recover every one of their own unclaimed deposits and unreleased burns" | In | 1, 5 | LOC public and private accounts, 45/45 deposits and 30/30 exits from the phrase alone (`privacy-test-20261117`, development receipts); web-page deposits not seed-derived. Branch `imported-accounts`: imported Names accounts, TST |
| F10 | admin "can register … and deregister a token"; delisting docs | In-dev (own admin) | 1, 7 | PUB register; TST deregister; delisting doc `docs/rfp/DEPLOYERS.md` §4 |
| F11 | "Global and per-token deposit and redemption caps" | In-dev (no LEZ cap) | 4 | TST; set on Sepolia |
| F12 | finality depth "configurable by the admin authority per deployment" | In-dev; outbound Cond | 4, 6 | inbound PUB (v6, v7: Ethereum finality through the light client, fixed per instance); outbound NS (not configurable) |
| F13 | "protocol fee … on both minting and burning … on LEZ" | In-dev (Ethereum side) | 1 | TST |
| F14 | freeze "globally or for a single registered token … independently" | In-dev (own code) | 1, 4 | TST; LEZ global freeze LOC; the vault's admin timelock is closed in code on main, not deployed (no vault v2–v7 has it); the LEZ bridge has none yet |
| F15 | each deployment "refers to a specific" deployment on the other chain | In | 1, 4 | Ethereum side PUB; LEZ side PUB our public chain (v3 onward; v6, v7 bind the light-client instance in `zoneId`) |
| F16 | "multiple entities each operate under their own independent configuration" | In-dev (one config per program) | 1, 7 | TST |
| F17 | ERC-20s whose transfer "do not return a boolean (USDT)" | In | 1 | TST. Branch `erc20-path`: USDT in a four-token matrix, TST |
| U1 | "a Logos core module" for users and admin | In | 5 | PUB our public chain (unreleased Names Wallet builds: claims LEZ 4852, 5084, 5249, 27401); TST. Branch `private-in-wallet`: private claim and burn calls, TST |
| U2 | "a Logos mini-app" incl. registry view and admin UI | In | 5 | PUB our public chain ("Claim a deposit", unreleased); admin UI TST |
| U3 | "Logos module accompanied by a Logos Core headless CLI/daemon" | In | 5 | NS as modules (the relay, settlement service and outbound updater run as Python scripts on our public chain) |
| U4 | "an IDL … using the SPEL framework" | In | 5 | NS. Branch `lc-daemon`: an IDL for the RFP-021 program with SPEL, not the bridge's |
| U5 | mitigations "enabled by default"; leak indicator | In | 4, 5 | LOC one-time recipient; TST indicator; delay NS. Branch `privacy-m5`: the delay default |
| U6 | the private path "is the pre-selected option" | In | 5 | TST |
| U7 | "explain what is public and what is private at each step" | In | 7 | DES (`docs/rfp/USERS.md` §2) |
| U8 | "clear, actionable error messages" that do not reveal the deposit | In | 1 | LOC |
| U9 | "change the targeted Ethereum RPC … and … LEZ sequencer" | In | 5 | TST |
| U10 | modules "in a module catalog of the team's own" | In | 5 | NS |
| R1 | "Minting is atomic" | In | 1 | LOC |
| R2 | "Redemption is atomic at each stage" | In | 1 | PUB burns and releases (v4–v7); failure paths TST |
| R3 | "No deposit can be claimed twice and no burn redeemed twice" | In | 1, 4 | LOC one chain; across chains TST (v3) |
| R4 | "A valid claim remains valid indefinitely" | In | 1 | TST |
| R5 | "Position recovery is complete" | In | 1 | LOC public and private accounts (`privacy-test-20261117`, development receipts) |
| R6 | RPC failure "able to resume without duplicating work" | In | 5 | NS. Branch `lc-daemon`: the RFP-021 follower's backoff and resume, TST |
| R7 | "interrupted user-side operation does not consume … entitlement" | In | 4 | DES |
| R8 | "mature, audited proof-system implementations" | In | 1 | TST (RISC Zero: Ethereum-side provers and verifier 3.1.0, LEZ side 3.0.5 as LEZ rc3; Helios; `docs/rfp/REUSE.md`) |
| R9 | third-party outage: "document the effect, and mitigate it" | In | 7 | DES (`docs/rfp/SECURITY-AND-TRUST.md` §8) |
| R10 | "full deposit and redemption round trip … included in CI" | In; with real proofs Cond | 6, 7 | PUB round trips with real proofs: v4, v5 (private), v6 (public and private), v7 (LEZ blocks never proved final); LOC the judge demo (executed); not in CI (the judge-demo workflow runs on demand only) |
| R11 | "Every hard requirement has at least one corresponding test" | In | 7 | DES (NO TEST rows remain) |
| R12 | developer docs for the core module "including position recovery" | In | 5, 7 | NS as one journey (pieces: `modules/CLI-CONTRACT.md`, `modules/NAMES-*.md`) |
| R13 | docs for "the CLI and any operator-facing components" | In | 5, 7 | DES (`docs/rfp/USERS.md`, `docs/rfp/OPERATORS.md`) |
| R14 | docs for "the deployer journey" | In | 7 | DES (outline, `docs/rfp/DEPLOYERS.md`) |
| P1 | claim and mint "within a single LEZ public transaction" | In | 1 | LOC 390,756 cycles |
| P2 | extra LEZ-side verification and "amortised per-deposit cost" | In | 1, 7 | LOC measured; amortised DES |
| P3 | "Ethereum-side gas cost of a deposit, of a release" | In | 1, 4 | PUB deposits, updates and releases with real seals (v4–v7) |
| P4 | proving "on a mid-range laptop and on the lowest specification" | In | 4 | LOC one desktop |
| P5 | latency "broken down by" finality, proving, delay, verification | In; outbound Cond | 7 | DES |
| P6 | "compute resources … of any off-chain component" | In | 1, 7 | PUB outbound updater and wrap (v4, v5 unattended; an 8 GB and a 16 GB desktop proving back to back, 2026-11-17); LOC light-client private updater |
| P7 | state growth "projected at 1M and 10M operations" | In | 7 | TST partial |
| Sup1 | "deployed and tested on a public Ethereum testnet and LEZ testnet 0.4" | In | 7 | PUB Sepolia; testnet 0.4 NS [ext] |
| Sup2 | "modules work on both Linux and MacOS" | In | 5 | NS (the judge demo runs on both; the modules have not) |
| Sup3 | built with "logos-modules-release-action" | In | 5 | NS |
| Sup4 | "design decision log in repo" | In | 1 | DES (`docs/DECISIONS.md`; entries before 2026-11-19 reconstructed) |
| S1 | "independent third-party … audit"; report published | Desc: Logos procures; we support (M8) | 8 | NS |
| S2 | both sides "must reject invalid proofs" | In | 1, 4 | PUB a changed seal refused by Sepolia's verifier (v4 round trip) and the attester's root refused by the v6 bridge (LEZ 23349); LOC; TST |
| S3 | a submitter cannot "redirect funds, inflate their fee, or replay" | In | 1, 4 | TST (H2 fix, v3) |
| S4 | caps: "document recommended defaults and the reasoning" | In | 4 | DES (`DESIGN.md` §6.1a) |
| S5 | freeze "exercisable independently on each half" | In | 1, 4 | LOC; TST; the vault's admin timelock is closed in code on main, not deployed (no vault v2–v7 has it); the LEZ bridge has none yet |
| S6 | "total wrapped supply on LEZ must never exceed the vault's holdings" | In | 1, 4 | LOC; TST (C1 fix, v3) |
| S7 | user docs state "the trustless verification model" | In | 7 | DES (`docs/rfp/USERS.md` §1) |
| S8 | "The verification logic cannot be swapped" | In | 1 | PUB immutable LEZ programs on our public chain; v4 still releasing beside v5 (a migration); TST |
| S9 | failsafe that "must not become a backdoor" | Cond (built in M6; with no-go, documented only) | 6 | LOC Ethereum half (`failsafe-20261117`, executed; variant not deployed; known limit: wind-down fixes `finalState` without checking LEZ minting stopped, so a late exit can be paid while the same deposit is reclaimed); LEZ half DES (M6's LEZ mint freeze closes it) |
| PP1 | "Inbound unlinkability" | In | 7 | LOC pairing measured, 1/20 against 1 by chance (`privacy-test-20261117`, development receipts); leak checks |
| PP2 | "Outbound unlinkability" | Cond | 6, 7 | LOC pairing measured, 0/20 (same record; releases executed, not proved) |
| PP3 | no destination revealed "over full event and state diffs for a complete round trip" | In; round trip Cond | 7 | LOC full capture of complete round trips, storage diffs included (same record, development receipts); with real proofs NS |
| PP4 | linking information "must never leave the user's control" | In | 1, 5 | LOC (our settlement service is an exception when used: `docs/rfp/USERS.md` §5) |
| PP5 | rejected, repeat and cap rejection "indistinguishable" | In | 1 | LOC; TST |
| PP6 | "document precisely what that party learns"; switch per operation | In | 5 | LOC two open relays with failover (branch `open-relay`); per-claim relay choice exists; doc `docs/rfp/USERS.md` §5 |
| PP7 | "The default configuration must be the private one" | In | 1, 5 | LOC one-time recipient; rest DES |
| Soft1 | "Committed amounts"; "document the intended migration path" | In: path only | 7 | DES (outline, `docs/rfp/COMMITTED-AMOUNTS.md`: the deployed vaults cannot adopt it in place) |
| Soft2 | "viewing keys" | Desc: not offered | — | NS |
| Soft3 | "per-token release delay" | In | 1 | TST |
| Soft4 | "pluggable" proof-system components | In | 1 | DES (image ids per vault version) |
| Soft5 | "Common Ethereum wallet compatibility" | In | 1 | PUB deposit page, Sepolia |

## RFP-021

| Id | RFP words | Scope | M | Today |
|---|---|---|---|---|
| F1 | "Run the Ethereum light client inside the LEZ program" | In | 1 | PUB our public chain: finality updates as public transactions and six committee handoffs over seven days unattended (2026-11-18 00:36 to 2026-11-25 02:51 UTC: 354 transactions, none refused; `lc-public`, `lc-handoff`, `public-demo/prove/lc-7day`; code on branch `public-update`); LOC proved private update |
| F2 | "reject any header that is not" finalised | In | 1 | TST (incl. the consensus-specs vectors, branch `public-update`) |
| F3 | inclusion of "an account, a contract storage slot, and an event log" | In | 1, 3 | PUB storage slot (v6 and v7 claims); LOC account and receipt-log statements on real Sepolia data (`statements-20261117`, branch `public-update`) |
| F4 | predicates "at least equality and inequality" | In | 1 | TST (`Eq`, `Ne`; ordering comparisons not built) |
| F5 | chain binding "tested across mainnet and at least one testnet with the same contract address" | In | 3 | LOC E07 on a local chain (`error-codes-20261117`); TST; same-address test NS |
| F6 | "deterministic, collision-resistant identifier" | In | 1 | TST |
| F7 | "implemented and tested as a LEZ program on testnet" | In | 3, 7 | PUB our public chain (inclusion checked inside the v6 and v7 claim transactions); testnet NS [ext] |
| F8 | "must not introduce any correlation signal" | In | 1, 7 | LOC byte searches; full diffs NS |
| F9 | parameters write-once; core module API to "verify an instance's checkpoint" | In | 1, 3 | TST; API NS [ext] |
| F10 | outside the weak-subjectivity period "replaced by a freshly initialised instance" | In | 3 | TST the refusal past the bound (CLI pre-check and native tests, branch `public-update`; the instance behind v6 and v7 runs that image); doc `docs/rfp/LIGHT-CLIENT-SPEC.md` §6, §8 |
| F11 | "Advancing the tracked consensus state is permissionless" | In | 1 | PUB updates from a throwaway account with no role (`lc-handoff`); LOC fresh account (rc1) |
| F12 | "liveness roles, not trust roles" | In | 1, 7 | DES (`docs/rfp/SECURITY-AND-TRUST.md` §7, §8) |
| F13 | "no instruction that rewrites an initialised instance's checkpoint" | In | 1 | TST |
| F14 | "any Ethereum-consensus chain, mainnet or testnet" | In | 1 | PUB Sepolia instances on our public chain; TST mainnet |
| F15 | "Multiple independent instances coexist under the same program" | In | 1 | LOC |
| U1 | "a Logos core module" for consumers and operators | In | 3, 5 | NS. Branch `lc-daemon`: `lez_light_client` module, TST |
| U2 | CLI incl. a dry run with "the same typed error codes" | In | 3 | LOC CLI with typed codes and its dry run (`cli-ws-20261117`, `error-codes-20261117`, branch `public-update`) |
| U3 | "update-submitting component … Logos module … headless CLI/daemon" | In | 3 | NS as a module (a shell loop follows Sepolia on our public chain). Branch `lc-daemon`: `lc-follower`, TST |
| U4 | "an IDL … using the SPEL framework" | In | 3 | NS. Branch `lc-daemon`: the SPEL IDL, TST |
| U5 | "clear, actionable error messages for all failure modes" (eight named) | In | 3 | LOC 13 typed codes, five shown on a local chain, all in native tests (`error-codes-20261117`) |
| U6 | "reference consumer programs": ERC-20 and native ETH | In | 3 | LOC both, on real Sepolia data, each refusing a replay (`statements-20261117`) |
| R1 | "Verification is deterministic" | In | 1 | TST |
| R2 | "read-only with respect to consumer state" | In | 1 | TST |
| R3 | "remains valid indefinitely" through rotations | In | 1 | TST |
| R4 | "Committee handoff tracking survives interruption" | In | 1, 3 | TST program; daemon NS. Branch `lc-daemon`: the daemon half, TST |
| R5 | RPC failure: "able to resume without duplicating work" | In | 3 | NS. Branch `lc-daemon`: TST |
| R6 | "stating what is reused, what is rewritten, and the cost and audit-surface consequences" | In | 1, 3 | TST; statement `docs/rfp/REUSE.md` (whether r0vm-helios's audit covers our Helios revision: not checked) |
| R7 | "a stalled instance resumes correctly … no gap in the verified handoff chain" | In | 3 | PUB six committee handoffs over seven days on our public chain (`lc-handoff`, `public-demo/prove/lc-7day`); LOC three-period catch-up (`catchup-20261117`); a deliberate stall on a public chain NS |
| R8 | "a duplicate or already-applied update is rejected cleanly" | In | 1 | TST |
| R9 | "checks whether an update is still needed immediately before submitting" | In | 3 | NS. Branch `lc-daemon`: TST |
| R10 | "defers submission by an interval it varies per instance" | In | 3 | NS. Branch `lc-daemon`: TST |
| R11 | "Statement submissions are independent of one another" | In | 1 | TST |
| R12 | "CI must be green on the default branch" | In | 1 | TST suites green in CI |
| P1 | attestation and predicate "within a single LEZ transaction" | In | 1, 3 | PUB a finality update in one public transaction, 9.3–9.6M gas (`lc-public`, `lc-handoff`); LOC statements 263K cycles; LOC low participation in two steps (synthetic committee) |
| P2 | "cycle count, gas cost, proof time, and proof size for each" | In | 1, 3 | PUB gas per update step (`lc-handoff`); LOC; proof size NS |
| P3 | "amortised per-attestation cost of sync-committee tracking" | In | 7 | DES |
| P4 | proving "on a mid-range laptop", "including … a private transaction" | In | 4 | LOC one desktop |
| P5 | "end-to-end attestation latency" breakdown | In | 7 | DES |
| P6 | "compute resources … of any off-chain component" | In | 1 | LOC updater |
| P7 | state growth "over one and five years" | In | 7 | DES |
| P8 | "cycle-count delta" to "a hypothetical native precompile" | In | 1 | TST |
| P9 | "No action's cycle count or gas cost scales with accumulated on-chain storage" | In | 1 | DES (fixed-size store) |
| Sup1 | "deployed and tested on LEZ testnet 0.4" | In | 3, 7 | NS [ext] |
| Sup2 | "End-to-end integration tests … included in CI" | In | 7 | NS |
| Sup3–5 | docs: core module, "Recommended Consumer Pattern", CLI and operators | In | 7 | DES (`docs/rfp/LIGHT-CLIENT-SPEC.md` §7–§9, `docs/rfp/OPERATORS.md`) |
| Sup6 | "trust assumptions document … the cost of the attack it admits" | In | 7 | DES (`docs/rfp/SECURITY-AND-TRUST.md`) |
| Sup7 | "Document the operator role" | In | 3 | DES (`docs/rfp/OPERATORS.md` §1–§2) |
| Sup8 | "independent third-party security audit"; report published | Desc: Logos procures; we support (M8) | 8 | NS |
| Sup9–10 | catalog, "logos-modules-release-action" | In | 5 | NS |
| Sup11 | "design decision log in repo" | In | 1 | DES (`docs/DECISIONS.md`) |
| VS1 | "reject a fabricated chain … exercised explicitly" | In | 1 | LOC; TST |
| VS2 | the six named rejection cases | In | 1 | LOC E03, E07 and 341 of 512 refused on local chains (branch `public-update`); TST |
| VS3 | no verification "through a ProgramId held in mutable account state" | In | 1 | PUB immutable deploys on our public chain (v6 record) |
| VS4 | "failure modes that follow from a compromised or colluding sync committee" | In | 7 | DES (`docs/rfp/SECURITY-AND-TRUST.md` §5.3) |
| VS5 | docs state "the trustless verification model" | In | 7 | DES (`docs/rfp/USERS.md` §1, `docs/rfp/LIGHT-CLIENT-SPEC.md` §9) |
| VS6 | "accepted on proof validity alone" | In | 1 | PUB (`lc-handoff`, a throwaway account with no role) |
| Soft1–4 | recorded-statement account, batching, historical state, richer predicates | Desc: not offered | — | NS |
| Soft5 | "Operator incentive" | In | 3 | DES. Branch `lc-incentive`: payout rules TST |
