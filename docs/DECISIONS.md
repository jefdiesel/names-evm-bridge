# Design decision log

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

Answers RFP-021 Supportability #11 and RFP-022 Supportability #4: "Provide a design decision log in repo recording each
major design decision and choice: the context, the options considered, the option chosen, and the rationale and
trade-offs. The log is updated as decisions are made and is current at each milestone delivery."

**How this log was made.** It was started on 2026-11-18 (branch `rfp-docs`). Every entry dated before that is
**reconstructed** from the git history, `DESIGN.md`, `research/` and the proposal notes; its date is the date of the
first commit or record that shows the decision, not necessarily when it was first discussed. Entries name their
sources; "owner's decision" means the project owner decided it (recorded in the source named). From now on, add an
entry when a decision is made, newest last, and keep old entries: a reversed decision gets a new entry that points
back.

**Fields.** Context and decision · Alternatives · Why · What would reverse it · Source.

## Scope and architecture

**D1 (2026-11-12, reconstructed). One codebase for both RFPs; the bridge consumes the RFP-021 primitive.**
- Alternatives: bid RFP-022 alone with its own inclusion check; separate repositories.
- Why: both RFPs prefer combined proposals; RFP-022 F3 requires consuming RFP-021 rather than rebuilding it.
- Reverse if: only one RFP is awarded (each is priced to stand alone, proposal "Milestones").
- Source: first commit `1d07ade`; `proposal/FORM-short.md` Summary.

**D2 (2026-11-12, reconstructed). Fixed denominations per token, not committed amounts.**
- Alternatives: value commitments on LEZ (RFP-022 Soft1).
- Why: the RFP's expected baseline; LEZ 0.4 publishes a mint's amount and supply (`DESIGN.md` §8.4).
- Reverse if: LEZ offers a mint with hidden amounts; then a new vault and bridge (`docs/rfp/COMMITTED-AMOUNTS.md`).
- Source: `DESIGN.md` §3.1; `vault/src/BridgeVault.sol` (`MAX_DENOMS`).

**D3 (2026-11-12, reconstructed). The inbound statement is the vault's `isDepositRoot[root]` storage slot, not a
per-deposit event.**
- Alternatives: prove each `Deposit` log; an operator-attested root only.
- Why: one anchor serves every deposit under it; the root is not published by a private claim; one statement per root.
- Trade-off: uniqueness cannot be keyed on the statement id (D4).
- Source: `DESIGN.md` §3.2; `research/attested-claim-sepolia.md`.

**D4 (2026-11-12, reconstructed). Uniqueness keyed on our own claim nullifier, not on the statement identifier
(deviation from RFP-022 F3).**
- Alternatives: key on the statement id (one claim per root, and it reveals the root).
- Why: the statement is per root, shared by every deposit under it; `commitmentUsed` makes the nullifier one-to-one with
  deposits.
- Reverse if: a per-deposit statement with a private nullifier keyed on it is adopted (gap audit A-6).
- Source: `DESIGN.md` §11.

**D5 (2026-11-12, reconstructed). The protocol fee is charged on Ethereum, at deposit and release (deviation from
RFP-022 F13).**
- Alternatives: a fee in the wrapped token on LEZ; a fee in native LEZ.
- Why: a wrapped-token fee leaves `denom − fee` balances that cannot exit as one denomination.
- Reverse if: a native-LEZ fee fixed per denomination is chosen (gap audit A-2, not decided).
- Source: `DESIGN.md` §11, §6.1.

**D6 (2026-11-12, reconstructed). Our own token program (a copy of the Names `wallet-tokens` prototype plus `MintBy`)
and our own admin and freeze code (deviation from RFP-022 F10, F14 and LP-0013).**
- Alternatives: upstream token program, RFP-001 and RFP-002 libraries.
- Why: no LEZ 0.4 token program with mint authorities or ATA when checked on 2026-11-06.
- Reverse if: lez-programs' 0.4 branch and the RFP-001/002 libraries build on LEZ 0.4 (proposal M4, M5).
- Source: `DESIGN.md` §11; proposal §7.

**D7 (2026-11-12, reconstructed). One configuration per bridge program deployment (deviation from RFP-022 F16).**
- Alternatives: several configurations keyed by `cfgId` in one program.
- Why: simpler isolation; several deployments give the same separation.
- Reverse if: same-program configurations are required as written (gap audit A-4).
- Source: `DESIGN.md` §10, §11.

**D8 (2026-11-12, reconstructed). No LEZ-side claim cap (deviation from RFP-022 F11).**
- Why: mints are bounded by claims ≤ deposits and the vault's deposit caps.
- Trade-off: on a false light-client anchor the mint freeze is the only stop on LEZ (`DESIGN.md` §9.8).
- Reverse if: the optional cap of M4 is built (it must refuse with `claim refused`).
- Source: `DESIGN.md` §11.

**D9 (2026-11-12, reconstructed). Verification is fixed at deployment: LEZ programs deployed immutable, a vault with
no setter for verifier or image ids; a change is a new version beside the old one.**
- Alternatives: upgradeable programs; a verifier router.
- Why: RFP-022 S8 and RFP-021 VS3.
- Trade-off: every change to a proof image needs a new vault, and anonymity sets restart.
- Source: `DESIGN.md` §10; `test_NoSetterForVerificationInAbi`.

**D10 (2026-11-12, reconstructed). One program pair binds one chain pair through `cfgId`, derived on both sides.**
- Alternatives: unify wrapped tokens across EVM chains (larger anonymity set).
- Why: blast-radius containment, as RFP-022 assumes.
- Source: `DESIGN.md` §2.

## The light client (RFP-021)

**D11 (2026-11-12, reconstructed). Reuse Helios consensus-core inside a LEZ program, with RISC Zero's accelerated
crates; write the program, accounts, statements and CLI ourselves.**
- Alternatives: fork r0vm-helios or sp1-helios (they prove off-chain for an EVM destination); write the light-client
  logic from the specs.
- Why: LEZ verifies inside its own RISC Zero execution, so the light-client logic is what transfers, not the
  destination-chain contracts.
- Source: `DESIGN.md` §9; `attest-spike/README.md`; `docs/rfp/REUSE.md`.

**D12 (2026-11-12, reconstructed). The first update path is private: the submitter verifies the update in its own,
locally proved execution; only `Advance{prev, next}` reaches the store.**
- Alternatives: a public update (about 37M cycles against LEZ's 10M cap: does not fit); a BLS precompile (not ours).
- Why: unmetered private execution was the only way to run the unchanged verifier on LEZ 0.4.
- Trade-off: about 2 h of desktop proving per update; a submitter needs a prover, not only gas.
- Source: `DESIGN.md` §9.2.

**D13 (2026-11-14, reconstructed). The public path: signers' key from key witnesses with RISC Zero's blst, split at the
pairing seam into two steps when needed.**
- Alternatives: keep only the private path; ask for a precompile first.
- Why: measured at about 9.3M cycles for a finality update at normal participation, under the cap.
- Trade-off: two Helios functions replaced through hooks become our audit surface; no per-key subgroup check (confirmed against `ethereum/consensus-specs` `specs/altair/bls.md`, where `eth_aggregate_pubkeys` asserts `KeyValidate`; see `DESIGN.md` §9.3a).
- Built and run on branch `public-update` (2026-11-17), on our public chain since: six committee handoffs over seven days unattended (2026-11-18 00:36 to 2026-11-25 02:51 UTC, 354 transactions, none refused; `public-demo/prove/lc-7day`).
- Source: `research/lc-cycles-2026-11-14.md`; `DESIGN.md` §9.3a; `docs/EVIDENCE.md` rows `lc-public`, `lc-handoff`, `public-demo/prove/lc-7day`.

**D14 (2026-11-14, reconstructed). Participation floor ≥ 342 of 512 in code; 427 proposed for bridge instances.**
- Why: Helios's two-thirds rule; a higher floor raises the seats a colluding committee needs.
- Trade-off: a period with no update at the floor cannot be crossed.
- Reverse if: participation measurements at M3 show 427 blocks updates in practice.
- Source: `DESIGN.md` §6.1a, §9.8.

**D15 (2026-11-17, reconstructed). Weak-subjectivity bound enforced by the program: 3,532 epochs on mainnet, 1,348
elsewhere; past it an instance is replaced, never refreshed.**
- Why: RFP-021 F10; the lower Gloas value for chains other than mainnet.
- Source: branch `public-update` (`ws_epochs`); `public-demo/prove/cli-ws-20261117/README.md` there.

**D16 (2026-11-17, reconstructed). Ethereum forks are adopted by a new instance made before the fork epoch.**
- Alternatives: an instance whose config can name a successor (not designed).
- Trade-off: for the bridge, each Ethereum fork means a new bridge instance and vault.
- Source: `DESIGN.md` §9.7.

## The bridge's way in

**D17 (2026-11-12, reconstructed). A stand-in operator attester for deposit roots on chains without a light client.**
- Why: the demo needed claims before the light client could follow Sepolia publicly.
- Trade-off: the attester (and the LEZ admin, through `SetAttestor`) can mint unbacked wrapped tokens on that chain.
- Reversed for v6 and v7 (D30); the default v5 still uses it.
- Source: `DESIGN.md` §3.2, §7.

**D18 (2026-11-12, reconstructed). Private claims mint to a one-time address, and the bridge's float pays their LEZ
fee.**
- Why: LEZ 0.4 has no address-free credit and no third-party payer for a private fee (`DESIGN.md` §8.4).
- Source: `DESIGN.md` §3.5.

**D19 (2026-11-12, reconstructed). One refusal text, `claim refused`, for every deposit-dependent claim failure; the
CLI diagnoses locally.**
- Why: RFP-022 PP5.
- Source: `DESIGN.md` §3.4; `lez-bridge/bridge/tests/panic_texts.rs`.

**D20 (2026-11-13, reconstructed). Bridge secrets derive from the wallet seed; a recovery scan of both chains rebuilds
positions.**
- Alternatives: a random secret per deposit kept in a file (what the web page does).
- Why: RFP-022 F9 prefers secrets re-derivable from the seed.
- Trade-off: web-page deposits are not recoverable from a seed (stated).
- Source: commit `0d67d3e`; `DESIGN.md` §10.

**D21 (2026-11-13, reconstructed). A deposit desk with a bridge fee, and a relayed claim whose fee payer co-signs,
instead of granting native LEZ to an account named on Ethereum.**
- Why: the grant linked the deposit to the account on Sepolia and needed a public key the Names Wallet does not show.
- Owner's decision (2026-11-14): the desk fee is 0.3%, capped at 3% in the contract.
- Source: `DESIGN.md` §3.6 ("Superseded: the fee grant"); `research/open-settlement.md` §0.

**D22 (2026-11-14, owner's decision, reconstructed). Settlement is an open role: anyone may prove updates, relay claims,
submit outbound updates and relay releases for a fee; the team runs the first instance of each.**
- Source: `research/open-settlement.md` §0.

## Security fixes and deployments

**D23 (2026-11-14, reconstructed). Fix the review's findings in new programs deployed beside the old ones (v3), not by
upgrading.**
- Findings: typed mint authority (C1), header-signed `Initialize` (H1), the claim nullifier bound to the claimer (H2),
  chain id and root source in `cfgId`, a sponsor cap.
- Why: D9; the live v2 programs keep their exposure, stated.
- Source: `research/security-review-2026-11-14.md`.

**D24 (2026-11-13, reconstructed). Run our own public LEZ chain (v0.4.0-rc3, standalone) because LEZ testnet 0.4 is not
live; move from rc1 to rc3.**
- Trade-off: it is not a Logos network, not anchored to the Logos Blockchain, and we run its only sequencer.
- Reverse if: LEZ testnet 0.4 goes live (proposal M7).
- Source: commit `b64c9b4`; `public-demo/RUNBOOK.md` (not published) §11.

**D25 (2026-11-15, reconstructed). Ethereum-side provers on risc0-zkvm 3.1.0 and vaults bound to RISC Zero's bare
Groth16 verifier 3.1.0; never the 3.0.0 verifier, the router or an emergency stop.**
- Why: advisory GHSA-g6qg-9j8p-7j87; the router and stop can be switched off by RISC Zero.
- Trade-off: the update guest cannot check LEZ rc3's 3.0.5 privacy receipts, so private claims and burns are taken on
  the sequencer's word (`unchecked-private`).
- Source: commit `adc0bca`; `DESIGN.md` §4; `AGENTS.md` Conventions.

**D26 (2026-11-15, reconstructed). Updates through `UpdateGatedVerifier` from one fixed submitter key (v4–v6).**
- Alternatives: an open `update` (anyone could prove an update over made-up blocks and fork the vault's state).
- Why: the TrustedTip image proves no LEZ finality.
- Trade-off: that key decides which LEZ blocks count; a lost key stops updates for good.
- Reverse: v7's gate (D31).
- Source: `vault/src/UpdateGatedVerifier.sol` header.

**D27 (2026-11-15, owner's decision, reconstructed). v5: v4's design with no release delay, made the default.**
- Trade-off: only caps bound an update over a forged LEZ tip.
- Source: `public-demo/V5-PLAN.md` §1; `vault/DEPLOYMENTS.md` v5.

**D28 (2026-11-15, reconstructed). The Groth16 wrap runs on a GitHub-hosted x86 runner; a journal that names a person
travels as an encrypted bundle.**
- Why: RISC Zero's Groth16 prover is x86-only.
- Trade-off: the runner sees the release receipt (not analysed beyond the journal).
- Source: `DESIGN.md` §4 step 4; `.github/workflows/wrap-private.yml`.

**D29 (2026-11-15, reconstructed). One declared truth for versions (`VERSIONS.toml`) and a checker that gates every
deploy, pin and proving run.**
- Source: `AGENTS.md` "Version alignment"; commit `74d03c4`.

**D30 (2026-11-18, reconstructed). v6: deposit roots from the light client; deployed beside v5, which stays the
default.**
- Why v5 stays the default: the released wallet build makes the v5 form of the claim; v6 has no standing updater.
- Source: `vault/DEPLOYMENTS.md` v6; `proposal/V5-VS-V6.md` (private, untracked on `main`).

## The way out

**D31 (2026-11-17, reconstructed). Build the threshold-of-signer-keys route first, with a contest window, ejection on
equivocation and a halt on a conflicting quorum; move it to real accredited sequencer signatures in M6 unless Logos
rejects that trust model at the week-2 gate.**
- Alternatives: a Logos Blockchain light client in the proof (one leader-proof check took 9,778 s on CPU; ledger roots
  are not in headers); no exit (priced no-go).
- Why: LEZ 0.4 publishes no state root (issue #1129); the signatures exist upstream.
- Trade-off: stated as committee trust, "a quorum of the channel's accredited committee", not bounded by stake today.
- Source: `DESIGN.md` §0, §4; `proposal/SEQUENCER-SIGNATURES-STUDY.md` §8 (private, untracked on `main`); v7 record
  (branch `v7-seqsig`).

**D32 (2026-11-18, reconstructed). v7 with 2 of 3 stand-in keys on three machines and a 600 s test window.**
- Why: to run the gate with real proofs on Sepolia before real sequencer keys exist.
- Known limit: today's v7 signers check neither privacy receipts nor public execution yet (they re-hash and re-extract
  blocks); with our stand-in keys on our one sequencer, v7 trusts us.
- Source: `vault/DEPLOYMENTS.md` v7; `docs/EVIDENCE.md` rows `v7`, `v7-equivocation`.

**D33 (2026-11-13 design, 2026-11-17 Ethereum half, reconstructed). Failsafe: a wind-down and a reclaim proven by the
depositor, never an admin recovery.**
- Why: RFP-022 S9's "must not become a backdoor".
- Trade-off: covers unclaimed deposits after an inbound or release-proof bug; no compliant mechanism for a G2,
  proof-system or accounting bug on LEZ 0.4.
- Known limit (2026-11-27): the local Ethereum-half variant (`BridgeVaultFailsafe.sol`, not deployed) fixes `finalState`
  on the first update after `windDown()` without checking that LEZ minting has stopped, so a late exit can be paid while
  the same deposit is reclaimed (forged mints before the final also use up reclaim room). M6's irreversible LEZ mint
  freeze closes this limit.
- M6 as built (2026-11-27, closed in code, not deployed; `DESIGN.md` §10): the vault sets a hashed slot
  (`keccak256("RFP022/failsafe/wind-down/v1")`, not the numbered slot 92 of the design, which a later layout of either
  vault could fill); anyone proves it on LEZ and the bridge stops minting for good; G2 carries the wind-down as `GLOBAL`
  in the state's existing `wound_down` and refuses later claims; the vault fixes its final only from such a state. The
  LEZ program records no block height (it cannot read one; a caller-supplied height would be unchecked): G2 reports the
  wind-down's block in the journal of the update that holds it.
- Source: `research/failsafe-design.md`; `public-demo/prove/failsafe-20261117/README.md`.

## Proposal and process

**D34 (2026-11-16, owner's decision, reconstructed). The independent audit is not in the price; Logos procures it; we
remediate (M8).** Source: proposal §11; `proposal/FORM-short.md` "OWNER decisions".

**D35 (2026-11-14, owner's decision, reconstructed). The repository stays private; a public copy is exported through an
allow list with a guard.** Source: `AGENTS.md` "Git"; `scripts/export-public.sh`.

**D36 (2026-11-18, owner's decision, reconstructed). An account imported into the Names Wallet works in every bridge
flow: its secrets derive from the recovery phrase and the account id under a separate scheme.**
- Trade-off: whoever holds the wallet's first private account's key chain can derive the imported accounts' bridge
  values (not their funds).
- Source: `proposal/IMPORTED-ACCOUNTS-DESIGN.md` (branch `imported-accounts`).

**D37 (2026-11-27, owner's decision). Add an admin timelock in M4 on freezer changes and lifting freezes.**
- Context and decision: today neither `BridgeVault.sol` nor the LEZ bridge has a timelock on changing the freezer
  (`setFreezer`, `SetFreezer`) or lifting freezes (`unfreeze`/`unfreezeAll`, `Unfreeze`): the admin can do both at once.
  This is disclosed as a known limit, and M4 adds an admin timelock.
- Follow-up (2026-11-27): the vault half is closed in code on main (`BridgeVault.sol` `schedule`/`execute`/`cancel`,
  immutable `adminDelay`; `vault/README.md`), not deployed; the deploy value of `adminDelay` is the owner's decision at
  deploy time. The LEZ bridge half is still open.
- Source: proposal #281 Technical Approach §6; `SECURITY.md` "Known limits".

**D38 (2026-11-27, owner's decision). Reproducible guest builds with RISC Zero's Docker builder are set up in M1 (done
for main on 2026-11-28; the deployed native ids do not reproduce).**
- Context and decision: guest image ids differ across machines today (the guests embed the cargo registry's absolute
  path, `DESIGN.md` header). M1's acceptance includes every guest image id rebuilt identically on two machines with RISC
  Zero's Docker builder.
- Result (2026-11-28): `scripts/repro-build.sh` builds the 10 deployable guests in `risczero/risc0-guest-builder:r0.1.97.0`
  pinned by digest; two fresh GitHub-hosted runners gave identical ids and ELF sha256 (pre-merge CI run 46095730461), pinned in
  `VERSIONS.toml`. None of the deployed v2–v7 ids equals the Docker id of its guest, even for guests whose inputs are
  unchanged since the deploy (release, trusted-tip update; v7 seq-sig against its branch tip): the deployed records stay as they are and new
  deployments use Docker ids (`docs/REPRODUCIBLE-BUILDS.md`).
- Source: proposal #281 Milestone 1; `DESIGN.md` header; `SECURITY.md` "Known limits".
