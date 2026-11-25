# Verification statements: what each claims, how to re-check it, and its limits

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFPs' own words)

**RFP-021, "+ Verification Security":**
1. "Verification must reject a fabricated chain: given a self-consistent header and inclusion proof over state that was
   never finalised on the canonical chain, verification must fail. This is the central adversarial test of the RFP and
   must be exercised explicitly, not implied by other tests."
2. "Verification must reject: a header signed by a committee that was never validly handed off from the configured
   checkpoint; a sync-committee signature below the required participation threshold; a valid inclusion proof against a
   header that is not finalised; a valid finalised header with a tampered state or receipts root; a proof replayed
   against a different chain ID; and a predicate asserted over state the inclusion proof does not cover."
3. "**The verification logic cannot be swapped.** No consensus or inclusion verification may be reached through a
   `ProgramId` held in mutable account state. Change requires deploying a new version and having consumers move to it;
   document the migration mechanism and what consumers must do. …"
4. "Document the failure modes that follow from a compromised or colluding sync committee, and what a consuming
   application can do to bound its exposure …"
5. "User-facing and developer-facing documentation must state the trustless verification model and the liveness-only
   role of any off-chain participant …"
6. "An update is accepted on proof validity alone and never on the submitter's identity: any account submitting a valid
   proof advances the state, and any account submitting an invalid proof is rejected."

**RFP-022, the verification items of "+ Security":**
- S2: "Proof verification must be independently verifiable. Both the LEZ program and the Ethereum vault must reject
  invalid proofs, including those with incorrect public inputs, proofs for incorrect chain state, tampered headers, and
  replayed proofs."
- S3: "A malicious party submitting on a user's behalf must not be able to redirect funds, inflate their fee, or replay
  the user's submission to a different destination …"
- S6: "Soundness of supply: total wrapped supply on LEZ must never exceed the vault's holdings. Minting without a valid
  deposit, minting twice from one deposit, and releasing without a valid burn must all fail."
- S8: "**The verification logic cannot be swapped.** No party may change what the bridge accepts as a valid proof after
  deployment. Change requires deploying a new version and migrating to it. Proposals must document the migration
  mechanism and how in-flight deposits and burns are honoured across it."

## Status

- **Statements** VS1, VS2, VS3, VS6, S2, S3, S6, S8: complete as statements of what holds today, each with its limits.
  VS4 and VS5 are documents: [SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) §5.3 (VS4) and
  [USERS.md](USERS.md) §1 (VS5, user-facing) with [LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §9 (developer-facing).
- **Re-checkable by anyone, read only:** the public-network rows through `scripts/verify-evidence.sh` (rows named in
  square brackets below, with the check's label in quotes). **Re-checkable from source:** the tests named, with `forge`
  (anywhere) or `cargo` (a Linux or macOS machine with the RISC Zero toolchain; CI runs the vault suite and the LEZ
  program suites on `main`).
- **Not done:** VS2 cases inside the zkVM on the public path (only the honest path has run there); any audit.

## How to re-check

```sh
scripts/verify-evidence.sh                                  # live, read only, about a minute
scripts/verify-evidence.sh --offline docs/evidence-answers   # the answers saved on 2026-11-18, no network
cd vault && git submodule update --init --recursive && forge test --match-test <name>
cd lez-bridge && RISC0_SKIP_BUILD=1 cargo test --locked -p eth_attest -p bridge_core <name>
```

Each line of `verify-evidence.sh` is tagged with its row; the expected value and the record it comes from are in
`docs/evidence-checks.json`. Tests on branch `public-update` need that branch checked out.

## RFP-021

### VS1: a fabricated chain is rejected

- **Claim.** A storage statement over a self-consistent header and state (a made-up state in which the vault entry is
  true, its header, and a valid proof under that header) passes every local inclusion check and is refused, because no
  instance finalized that header: "anchor not finalized by this instance" (E01).
- **Re-check.** `an_anchor_the_instance_never_finalized_is_refused` (`lez-bridge/attest/program/tests/instance.rs`; the
  test builds the fabricated state and first asserts that the local check passes). An instance created from a checkpoint
  the bootstrap does not hash to is refused: `a_wrong_checkpoint_or_bootstrap_is_refused`.
- **Limits.** The tests run the program natively in the repository's simulator, not in the zkVM. On a chain, an anchor
  never recorded was refused on the local rc1 chain with the v2 bridge (`DESIGN.md` §3.4), not on our public chain.

### VS2: the six named rejections

| Case | Test (native) | On a chain |
|---|---|---|
| A committee never validly handed off | `the_wrong_committee_witness_is_refused` (E13); handoff order: `committee_handoff_period_update_then_finality_signed_by_the_next_committee`; another genesis validators root, fork version or epoch: `an_instance_with_another_*` (`audit.rs`) | E03 (a period-1415 update on a 1416 store) refused on a local chain (error-codes record, `public-update`) |
| Participation below the threshold | `participation_below_the_instance_policy_is_refused` (E11); `e11_participation_below_policy` (`public-update`) | 341 of 512 refused on a local chain (finality-split record, synthetic committee) |
| A valid inclusion proof against a header not finalised | `an_anchor_the_instance_never_finalized_is_refused` (E01) | refused on the local rc1 chain (`DESIGN.md` §3.4) |
| A finalised header with a tampered state or receipts root | `a_tampered_receipts_root_is_refused_with_the_exact_error`, `a_tampered_log_proof_is_refused_with_the_exact_error`; tampered finalized header: `e01_header_not_finalized` (`public-update`) | — |
| A proof replayed for another chain ID | `an_update_for_another_instance_or_chain_is_refused`, `a_consumer_bound_to_another_instance_or_chain_refuses_the_statement` | E07 on a local chain (error-codes record) |
| A predicate over state the proof does not cover | `a_statement_is_refused_for_a_wrong_value_account_or_anchor`; `e05_e06_inclusion_proof_and_predicate` (`public-update`) | — |

Also: the consensus-specs light-client sync vectors v1.6.1, 22 of 22 applicable cases, every tampered re-check refused
(spec-vectors record, `public-update`). **Limits:** native tests; on the public path, RISC Zero's blst fork has run only
honest updates inside the zkVM (proposal §1); the RFP's "same contract address on mainnet and a testnet" test (F5) does
not exist yet.

### VS3: verification cannot be swapped

- **Claim.** The attestation program and the bridge program are deployed immutable; a consumer (our bridge) names the
  attestation program and instance in `Config::root_source`, written once at `InitializeAttested` and changed by no
  instruction. A new version is a new program id and a new instance; consumers move by deploying anew
  ([LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §8).
- **Re-check.** `effects_only_come_from_the_program_and_config_never_changes`; `the_sources_do_not_mix`,
  `an_attested_config_refuses_the_stand_in_root_under_any_flag` (bridge). The deploy logs print "immutable" (`DESIGN.md`
  §3.2, §9.0). [v6] "InitializeAttested of the v6 bridge" shows the configuring transaction.
- **Limits.** The bridge's `Config` is an account other instructions do change (roles, registry, freezes); only its root
  source has no writer. `verify-evidence.sh` cannot read whether a LEZ program header is immutable (`AGENTS.md`,
  "Can't check").

### VS6: accepted on proof validity alone

- **Claim.** The attestation program holds no role, allowlist or registration: any account that submits a valid update
  advances the store, and an invalid one is refused.
- **Re-check.** `a_real_finality_update_advances_the_store_permissionlessly`; [lc-public] "update 1, public
  (UpdatePublic)"; [lc-handoff] the four handoff transactions, sent by a throwaway account with no role (v6 record).
- **Limits.** Only our follower has submitted on our public chain; RUN from a fresh account on a local chain (rc1 block
  1859). The private path (on `main`) needs a prover, not only gas.

## RFP-022

### S2: both sides reject invalid proofs

- **Claim, Ethereum.** The vault refuses a bad seal, a tampered journal and a seal moved to another release; RISC Zero's
  verifier 3.1.0 refuses a changed seal.
- **Re-check.** [v4-round-trip] "update 1", "release", "update 2" (each committed seal through the verifier) and
  "update 1's seal with its last byte changed" (refused); `docs/VERIFY-ROUND-TRIP.md`; tests `test_RevertUpdateBadSeal`,
  `test_TamperedJournalsRejected`, `test_CopiedSealCannotBeRedirected`.
- **Claim, LEZ.** Every deposit-dependent claim failure is one refusal, `claim refused`; attested claims are refused
  under any other anchor or root.
- **Re-check.** `an_attested_claim_is_refused_under_every_other_anchor_and_root`, `malformed_attested_claims_are_refused_cleanly`;
  [v6] "the operator attester's AttestRoot (refused on chain; this RPC shows inclusion, not the revert)".
- **Limits.** "Proofs for incorrect chain state, tampered headers" on the vault side: the update image proves block
  linkage and extraction, not LEZ finality. Which LEZ blocks count is the word of one submitter key (v4–v6) or of 2 of 3
  keys of ours (v7). A forged LEZ tip is not detectable by the vault ([SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) §3).

### S3: a submitter cannot redirect, inflate or replay

- **Claim.** A deposit commitment binds the LEZ destination; an exit commitment binds the Ethereum destination and the
  most relay fee; the release journal binds recipient, relayer and fee; nullifiers stop replays.
- **Re-check.** `test_RevertRelayerRedirectsOrOvercharges`, `test_CopiedSealCannotBeRedirected`,
  `a_claim_is_bound_to_its_destination`, `the_deposit_must_be_in_the_attested_root_and_bound_to_its_claimer`;
  [v6-round-trips] "release 1 paid (round trip 2, private)" was sent by a relayer that is neither depositor nor claimer
  (v6 record).
- **Limits.** A public-account claim publishes `k` by design (the public path is not private); the relay sees it before
  inclusion and still cannot change the claim.

### S6: soundness of supply

- **Claim.** Per token, released ≤ burned ≤ claimed ≤ deposited; a deposit is claimed once; an exit is released once.
- **Re-check.** `invariant_all`, `testFuzz_Solvency`; `value_is_conserved_across_random_interleavings_and_the_trace_fixture_is_current`;
  [v6-round-trips] "v6 vault exitCount() is 2"; [v7] "v7 vault exitCount() is 1".
- **Limits.** "Releasing without a valid burn" holds only as far as the LEZ blocks an update covers are real: a forged
  update can release what was deposited and not yet released, within caps. On a stand-in config (v2–v5) the attester,
  and the LEZ admin through `SetAttestor`, can mint without a deposit on LEZ.

### S8: verification cannot be swapped; migration

- **Claim.** No vault setter touches the verifier or the image ids; LEZ programs are immutable. A change ships as a new
  vault and bridge instance; the old vault keeps releasing against its own exit roots, and unclaimed deposits stay
  claimable on the old bridge (`DESIGN.md` §10).
- **Re-check.** `test_NoSetterForVerificationInAbi`, `test_VerificationUnchangedAfterEveryAdminCall`; [v4-round-trip]
  "v4 vault updateImageId()", "releaseImageId()" and the v5–v7 equivalents; the vaults and gates are source-verified on
  Sourcify (`vault/DEPLOYMENTS.md`). v4 releasing beside v5 is the migration in practice.
- **Limits.** An in-flight exit stays on its own vault (the `cfgId` binds the vault, burns never move between vaults);
  each vault version needs its own updater to keep releasing (v6 has none standing).

## Sources

`docs/EVIDENCE.md`, `docs/evidence-checks.json`, `scripts/verify-evidence.py`, `docs/VERIFY-ROUND-TRIP.md`;
`lez-bridge/attest/program/tests/{instance,audit}.rs` (`main`) and `error_codes.rs` (`public-update`);
`vault/test/`; `lez-bridge/bridge/tests/`; `DESIGN.md` §3.4, §5, §9, §10, §12.
