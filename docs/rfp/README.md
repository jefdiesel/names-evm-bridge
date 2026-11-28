# Documents the RFPs require: index

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

Every requirement of Logos RFP-021 (Ethereum state attestation) and RFP-022 (private wrapped ERC-20 and ETH bridge)
whose deliverable is a document, a specification, an analysis or a statement, with the RFP's words, the file that
answers it, and its status on 2026-11-18. RFP texts: `logos-co/rfp` at `920fad92`, `RFPs/RFP-021-ethereum-state-attestation.md`
and `RFPs/RFP-022-wrapped-erc20.md`. Requirement ids as in `docs/REQUIREMENTS.md` (F, U, R, P, Sup, S, PP, VS, Soft;
DR = a Design Rationale paragraph; Sup = Supportability, VS = Verification Security). "Gap audit" is
`proposal/RFP-GAP-AUDIT-2026-11-19.md`, a private file untracked in the working copy of `main`.

**Status words.** *Complete*: answers the requirement for what exists today, with every part labelled run, tested,
designed or not built. *Outline*: the structure and what is known, with what must happen first. *Elsewhere*: written
(or being written) in another file or branch, not repeated here. *Not ours*: the deliverable belongs to another party.

| Files in this folder | Answers |
|---|---|
| [SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) | the threat and trust model of both systems: assets, trust per vault version, roles, attacks, damage bounds, observers, blocking, outages |
| [LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) | the light client at specification level; checkpoint and replacement; statements; the Recommended Consumer Pattern |
| [VERIFICATION-STATEMENTS.md](VERIFICATION-STATEMENTS.md) | RFP-021 VS1–VS6 and RFP-022 S2, S3, S6, S8: claim, re-check, limits |
| [COMMITTED-AMOUNTS.md](COMMITTED-AMOUNTS.md) | RFP-022 Soft1 migration path; Soft2 outline |
| [OPERATORS.md](OPERATORS.md) | operator journeys: light-client follower, relays, release, updater; resources |
| [USERS.md](USERS.md) | public vs private at each step; the trust model in plain words; how to deposit, claim, bridge back, recover |
| [DEPLOYERS.md](DEPLOYERS.md) | the deployer journey, decimals, delisting, switching configurations, fees, migration |
| [REUSE.md](REUSE.md) | RFP-021 R6 and RFP-022 R8: what is reused, changed and ours, with versions and licences |
| [../DECISIONS.md](../DECISIONS.md) | the design decision log (RFP-021 Sup11, RFP-022 Sup4) |

## RFP-021

| Id | The RFP's words | File | Status |
|---|---|---|---|
| DR "Trust assumptions" | "the proposal must state it plainly and quantify the cost of the attack it admits"; "the specification must say how a verifier confirms the checkpoint independently, and when an instance … must be replaced" | SECURITY-AND-TRUST §5.3; LIGHT-CLIENT-SPEC §6 | Complete (the checkpoint check is a manual procedure: no tool, API [ext]) |
| F10 | "Document when an instance must be replaced, and what a consumer must do to migrate to a replacement" | LIGHT-CLIENT-SPEC §6, §8 | Complete; the refusal itself runs on branch `public-update` only |
| F12 | "identify every off-chain participant … state for each whether it can affect safety or only liveness, and justify that none of them, individually or as a class, can block an attestation" | SECURITY-AND-TRUST §7, §8 | Complete |
| U3 | "Document the operator journey end-to-end: install, configure, run, monitor" | OPERATORS §2 → `lez-bridge/attest/follower/README.md` (branch `lc-daemon`) | Elsewhere (the component is TESTED, not run on a chain) |
| R6 | "stating what is reused, what is rewritten, and the cost and audit-surface consequences of that split" | REUSE | Complete; whether any audit covers Helios rev `2af9b418` is not checked |
| R10 | "Document the deferral strategy and its effect on update latency" | OPERATORS §2 → follower README (branch `lc-daemon`) | Elsewhere |
| P1 | "the shortfall must be documented with the specific component responsible" | — (`DESIGN.md` §9.2, §9.3a) | Elsewhere: the private update path is over the 10M cap because of BLS curve operations; the public path fits |
| P2–P8 | "Measure and document …" (cycles, gas, proof time and size, amortised tracking cost, user-device proving, latency, resources, storage growth, precompile delta) | `docs/LC-COSTS.md` (branch `lc-incentive`, being written); OPERATORS §7 (resources) | Elsewhere; proof size per action, laptop numbers and storage projections not measured |
| P9 | "the proposal justifies why and documents the mitigations" | — (`DESIGN.md` §12.2: fixed-size store, anchors O(1)) | Elsewhere; no test (gap audit A-19) |
| Sup3 | "documentation in repo for the core module, covering the developer integration journey for a consuming LEZ program" | LIGHT-CLIENT-SPEC §7–§9; module docs (branch `lc-daemon`) | Outline here; the module itself is TESTED on a branch |
| Sup4 | "a "Recommended Consumer Pattern" section covering: … nullifier … chain binding … each typed error code … refuse the action, never fall back" | LIGHT-CLIENT-SPEC §9 | Complete |
| Sup5 | "documentation in repo for the CLI and any operator-facing components" | OPERATORS; `docs/ERROR-CODES.md` and `cli-ws-20261117` record (branch `public-update`) | Complete as a map; detailed CLI reference is `--help` |
| Sup6 | "a trust assumptions document covering …" (six named parts) | SECURITY-AND-TRUST, the six parts in order: §5.3; LIGHT-CLIENT-SPEC §6; §5.4; §7; §7; §10 | Complete |
| Sup7 | "Document the operator role for an instance …" | OPERATORS §1–§2; follower README; `docs/LC-INCENTIVE.md` (branch `lc-incentive`) | Complete for cadence, requirements, concurrency, stall detection, "no party is obliged"; cost: elsewhere |
| Sup8 | "the audit report must be published" | — | Not ours: Logos procures the audit (proposal §11) |
| Sup11 | "a design decision log in repo recording each major design decision …" | `docs/DECISIONS.md` | Complete; entries before 2026-11-18 reconstructed and marked |
| Sup1, Sup2, Sup9, Sup10 | deployment on LEZ testnet 0.4; e2e tests in CI; a module catalog; `logos-modules-release-action` | — | Not documents: deliverables of M3, M5, M7 (`docs/REQUIREMENTS.md`) |
| VS3 | "document the migration mechanism and what consumers must do" and the consumer rule on mutable `ProgramId`s | LIGHT-CLIENT-SPEC §8; VERIFICATION-STATEMENTS VS3 | Complete |
| VS4 | "Document the failure modes that follow from a compromised or colluding sync committee, and what a consuming application can do" | SECURITY-AND-TRUST §5.3 | Complete (the 427 floor, anchor age and LEZ claim cap are DESIGNED) |
| VS5 | "User-facing and developer-facing documentation must state the trustless verification model and the liveness-only role of any off-chain participant" | USERS §1; LIGHT-CLIENT-SPEC §9 | Complete |
| VS1, VS2, VS6 | statements to be "exercised explicitly" | VERIFICATION-STATEMENTS | Complete (native tests; VS2 in the zkVM on the public path not done) |
| Soft1 | "Document that a consumer reading such an account trusts whoever wrote it" | — | Not offered |
| Soft5 | "Propose a mechanism that rewards whoever advances the consensus state" | `docs/LC-INCENTIVE.md` (branch `lc-incentive`) | Elsewhere |

## RFP-022

| Id | The RFP's words | File | Status |
|---|---|---|---|
| DR "Linkability" | "The applicant must document every correlation the design leaves observable …: which signal carries it, why it remains …, and what an observer can infer from it" | USERS §2; SECURITY-AND-TRUST §7; proposal §5 table; `docs/PRIVACY-M5.md` (branch `privacy-m5`) | Complete for the user view; the measured analysis: elsewhere |
| DR "Trust model" | "Proposals must identify every such participant and justify that none of them … can block a user's deposit or burn"; the freeze's granularity "must be documented as such" | SECURITY-AND-TRUST §4, §8, §9 | Complete; states where it is **not met today** (attester v4/v5, submitter v4–v6) |
| DR "Token registry" | "the wrapped LEZ mint for each token must document its own decimals and the exact conversion applied on mint and burn" | DEPLOYERS §3 | Complete |
| DR "Finality" | "proposals must recommend a specific default depth for each direction and document the residual risk it leaves" | `DESIGN.md` §6.1a; DEPLOYERS §8 | Elsewhere; outbound depth belongs to the M6 route |
| DR "Fee structure" | "Configuration proposals must specify: Denomination … Rate … Collection point … Routing" | DEPLOYERS §6 | Complete (a deviation from F13, stated) |
| DR "One program pair" | "Applicants are welcome to argue for either direction, with the reasoning made explicit" | — (`DESIGN.md` §2: separation, `cfgId` per chain pair) | Elsewhere |
| F8 | "Proposals must state the mechanism chosen … and its effect on anonymity-set size" | COMMITTED-AMOUNTS §1–§2 | Complete as a statement; the anonymity-set analysis with real proofs is M7 |
| F10 | "Documentation needs to be provided to handle registration changes with minimum impact for users, including delisting" | DEPLOYERS §4 | Complete |
| F16 | "Document how a client identifies and switches between configurations" | DEPLOYERS §5 | Complete (several configurations per program not built: deviation) |
| U3 | "Document the operator journey end-to-end: install, configure, run, monitor" | OPERATORS §3–§6; `docs/OPS-V5-V6.md` (branch `v6-ops`) | Outline: the components are scripts, not Logos modules (NOT BUILT, M5) |
| U7 | "Documentation and UI must clearly explain what is public and what is private at each step on both chains" | USERS §2 | Complete (the UI's indicator is TESTED only) |
| R8 | "Proposals must integrate mature, audited proof-system implementations rather than reimplementing zero-knowledge primitives from scratch" | REUSE | Complete |
| R9 | "document the effect, and mitigate it, of an outage or malfunction of said component" | SECURITY-AND-TRUST §8 | Complete; mitigations mostly DESIGNED (M5) |
| R12 | "documentation in repo for the core module, covering the developer integration journey for both flows including position recovery" | — (`modules/CLI-CONTRACT.md`, `modules/NAMES-EMBED.md`, `NAMES-RECEIVE.md`, `NAMES-SEND-BACK.md`) | Elsewhere, partly; not written as one journey (M7) |
| R13 | "documentation in repo for the CLI and any operator-facing components, covering the core user and operator journeys" | USERS §3–§4; OPERATORS | Complete as a map |
| R14 | "documentation in repo for the deployer journey …" | DEPLOYERS | Outline (operator scripts, not a packaged tool; RFP-001/RFP-002 roles not adopted) |
| P1–P7 | "Document the compute-unit cost …", "Document the Ethereum-side gas cost …", "Measure and document wall-clock time and peak memory …", "Document end-to-end deposit latency … and redemption latency", "Document the compute resources", "Document the growth rate and on-chain storage cost … at 1M and 10M operations" | OPERATORS §7 (resources); `DESIGN.md` §3.3, §6.2, §9.0a | Elsewhere and partly measured; latency breakdown, laptop numbers and 1M/10M projections not written (M4, M7) |
| Sup4 | "a design decision log in repo …" | `docs/DECISIONS.md` | Complete; entries before 2026-11-18 reconstructed and marked |
| Sup1, Sup2, Sup3 | deployed and tested on Sepolia and LEZ testnet 0.4; modules on Linux and macOS; built with `logos-modules-release-action` | — | Not documents: deliverables of M5, M7 (`docs/REQUIREMENTS.md`) |
| S1 | "the audit report must be published" | — | Not ours (Logos procures; M8 remediation) |
| S4 | "proposals must document recommended defaults and the reasoning behind them" | `DESIGN.md` §6.1a; SECURITY-AND-TRUST §6 | Elsewhere; set and tested in M4 |
| S7 | "User-facing documentation must state the trustless verification model and the liveness-only role of any off-chain participant" | USERS §1 | Complete |
| S8 | "Proposals must document the migration mechanism and how in-flight deposits and burns are honoured across it" | DEPLOYERS §7; VERIFICATION-STATEMENTS S8 | Complete |
| S9 | "Proposals must specify a failsafe strategy … or … document explicitly why a failsafe should not be implemented" | — (`research/failsafe-design.md`; record `failsafe-20261117`) | Elsewhere: Ethereum half RUN locally (variant not deployed; known limit: wind-down fixes `finalState` without checking LEZ minting stopped, so a late exit can be paid while the same deposit is reclaimed), LEZ half DESIGNED (M6's LEZ mint freeze closes it) |
| PP4 | "Document every component that handles user data" | USERS §5 | Complete |
| PP6 | "document precisely what that party learns" | USERS §5; SECURITY-AND-TRUST §7 | Complete (switching relay per operation: the CLI's `--relay <url>`; a wallet relay list is M5) |
| Out of scope (network) | IP-level correlation "must be disclosed as residual leakage under Privacy Preservation #6" | USERS §2; SECURITY-AND-TRUST §7 | Complete |
| Soft1 | "document the intended migration path even if it is not implemented" | COMMITTED-AMOUNTS §3–§7 | Outline; finds the deployed design cannot adopt it without a new vault |
| Soft2 | viewing keys | COMMITTED-AMOUNTS §8 | Not offered; outline only |
| Soft4 | "Design the proof-system components as pluggable … (redeployment and migration would be necessary, per Security #8)" | — (`DESIGN.md` §10: image ids per vault version) | Elsewhere |
| Soft5 | "the proposal must document its security trade-offs" | — (`web/deposit/README.md`) | Elsewhere |

## Where the existing documents disagreed with each other or with the code (found 2026-11-18)

All of them are fixed on this branch (`rfp-docs`): `DESIGN.md` (§0, §1, §3.6, §4, §7, §8.4, §9.3a, §9.5, §9.6, §11, §12),
`vault/DEPLOYMENTS.md` (v5) and `docs/REQUIREMENTS.md` (every row brought to `docs/EVIDENCE.md` on main; counts on
2026-11-18: **60 of 127** run — PUB 30, LOC 30 — and **67** not yet run — TST 25, DES 23, NS 19, against 16, 38, 26, 26, 21 on 2026-11-16). Line numbers below are
those of `main` `5322a30`, before the fixes.

1. `DESIGN.md:539` (§7, Admin row) lists what the admin can do without the LEZ bridge admin's `SetAttestor`
   (`lez-bridge/bridge/src/lib.rs:1228`), which on a stand-in config (v2–v5) lets the admin mint unbacked wrapped
   tokens; nor that the vault admin can freeze (`vault/src/BridgeVault.sol:380-383`) or stop a token's releases with a
   cap of 0 (`BridgeVault.sol:620-627`).
2. `DESIGN.md:408` and `:413` say the sequencer-signature route ran "on throwaway local chains only" and list "real proofs
   on Sepolia" as not shown; vault v7 ran it with real proofs on Sepolia, including an ejection (`docs/EVIDENCE.md`,
   rows `v7` and `v7-equivocation`). (`DESIGN.md:55` and `:407` concern G2's own fork contest on Logos Blockchain
   finality, which is still not built.)
3. `DESIGN.md:81` ("v5 has no deposit yet (read 2026-11-16)") and `vault/DEPLOYMENTS.md:49` ("No deposit, update or
   release yet") against the v5 private round trip and unattended updates of 2026-11-16 (`docs/EVIDENCE.md`).
4. `DESIGN.md:877` (§11, "Weak subjectivity … Not enforced") against branch `public-update`'s E04 refusal, which the
   light-client instance behind v6 and v7 runs (v6 record). Resolved 2026-11-27: `main` enforces the same bound.
5. `DESIGN.md:762-766` (§9.5 "Not built") lists typed error codes, the two reference consumers and receipt-log statements
   on real data; `docs/EVIDENCE.md` (local rows) records them run on branch `public-update`. `DESIGN.md:801` says the CLI
   picks the wrong update after two boundaries; the catch-up record says the CLI alone drove a three-period catch-up.
6. `DESIGN.md:550` says the public update is "Measured in the executor only; not built"; it runs on our public chain
   (`lc-public`, `lc-handoff`).
7. `DESIGN.md:739` previously said the dropped per-key subgroup check was to be confirmed against the specs in M3 while
   the proposal form (§1) stated it as settled. Resolved (2026-11-27): confirmed in `ethereum/consensus-specs`
   `specs/altair/bls.md` (`eth_aggregate_pubkeys`: `assert all(bls.KeyValidate(pubkey) for pubkey in pubkeys)`);
   `DESIGN.md` §9.3a and `LIGHT-CLIENT-SPEC.md` §3 now quote the spec line.
8. `DESIGN.md:619` cites `lez-bridge/token/src/lib.rs:370-387` for "minted amounts are public"; those lines are now
   `definition_fields` / `definition_state`; `mint` is at 393-410.
9. `docs/REQUIREMENTS.md:112`, `:114`, `:121`, `:128`, `:131`, `:132`, `:139` (RFP-021 F1, F3, F10, U2, U5, U6, R7) give
   states from 2026-11-16 (LOC, TST, DES, NS) that `docs/EVIDENCE.md` now records as run on our public chain or on local
   chains; `:38` (RFP-022 F3 "LOC") and `:42` (F7) do not mention v6 and v7.
10. `DESIGN.md:354` gives the relay's `DAILY_MAX` as 100 (the code default, `public-demo/relayer/relayer.py:123`); the
    deployed relay runs 20 (`public-demo/RUNBOOK.md` (not published) §13). Not a contradiction of the code, but the worst case quoted
    differs from the deployed one.

## Not on the export list

These files are not on `scripts/export-public.sh`'s `ALLOW` list and have not been run through its guard. They cite
private files (`proposal/…`, `AGENTS.md`, `public-demo/RUNBOOK.md` (not published), `docs/OPS-V5-V6.md`); `proposal/SEQUENCER-SIGNATURES-STUDY.md`
and `proposal/RFP-GAP-AUDIT-2026-11-19.md` are untracked files in the working copy of `main`, not in this branch.
Publishing them is the owner's decision.
