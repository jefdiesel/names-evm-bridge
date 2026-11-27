# Security and trust model: the RFP-021 light client and the RFP-022 bridge

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFPs' own words)

- **RFP-021 Supportability #6:** "Provide a **trust assumptions document** covering: the honest sync-committee
  supermajority assumption and the cost of the attack it admits; the weak-subjectivity checkpoint, who supplies it and
  how a verifier confirms it independently; what an attestation does and does not prove; what every off-chain
  participant in the design can observe; whether each can affect safety or only liveness; and the conditions under
  which the guarantees degrade or fail."
- **RFP-021 Verification Security #4:** "Document the failure modes that follow from a compromised or colluding sync
  committee, and what a consuming application can do to bound its exposure (for example, caps and freeze authorities on
  the consumer side, as RFP-022 specifies)."
- **RFP-021 Functionality #12:** "Proposals must identify every off-chain participant the design requires, state for
  each whether it can affect safety or only liveness, and justify that none of them, individually or as a class, can
  block an attestation from eventually being produced."
- **RFP-022 Design Rationale, "Trust model":** "Proposals must identify every such participant and justify that none of
  them, individually or as a class, can block a user's deposit or burn from eventually completing." And: "the freeze
  authority can necessarily operate at token and protocol granularity only … This is a designed consequence of the
  privacy guarantees, not a gap, and must be documented as such."
- **RFP-022 Reliability #9:** "If any off-chain component is necessary, and expected to be run by a 3rd party, document
  the effect, and mitigate it, of an outage or malfunction of said component."
- **RFP-022 Security #4** (bounds part): "Caps (Functionality #11) bound the maximum value at risk in any rolling
  window; proposals must document recommended defaults and the reasoning behind them." The defaults and their reasoning
  are `DESIGN.md` §6.1a; §6 below uses them as damage bounds.
- **RFP-022 Out of Scope** (network anonymity): "IP-level correlation … must be disclosed as residual leakage under
  Privacy Preservation #6." Disclosed in §3 and §7.

User-facing statements of the trust model (RFP-021 VS5, RFP-022 S7) are in [USERS.md](USERS.md); this file is the
technical model they summarise.

## Status

- **Complete** for what exists on 2026-11-18. Every row says whether it is run, tested, designed or not built.
- **RUN on public networks** (Sepolia and our public LEZ v0.4.0-rc3 chain; re-checked by `scripts/verify-evidence.sh`,
  rows `v4-round-trip`, `v5-private-round-trip`, `v5-unattended`, `v6`, `v6-round-trips`, `v6-wallet-claim`, `v7`,
  `v7-equivocation`, `lc-public`, `lc-handoff`; seven-day unattended light-client run with six committee handoffs,
  2026-11-18 00:36 to 2026-11-25 02:51 UTC, 354 transactions, none refused: `public-demo/prove/lc-7day`).
- **RUN locally:** the signer gate with 3 of 4 keys and a halt (record `public-demo/prove/seq-sig-20261117/`), the
  privacy measurement (`privacy-test-20261117/`), the failsafe's Ethereum half (`failsafe-20261117/`), all on `main`;
  the open relay (record on branch `open-relay`).
- **TESTED:** the vault suite (`vault/test/`), the LEZ program suites (`lez-bridge/**/tests/`), named below.
- **DESIGNED, not built:** the accredited-committee route (`proposal/SEQUENCER-SIGNATURES-STUDY.md` §8 (b1)), the
  participation floor of 427, a minimum anchor age, a LEZ-side claim cap, the failsafe's LEZ half.
- **NOT DONE:** any independent audit; any run on a Logos network.

## 1. What is not covered (read this first)

- **No audit.** Nothing here has had an independent third-party audit (RFP-021 Sup8, RFP-022 S1). The only reviews are
  internal ones by AI agents (`research/security-review-2026-11-14.md`; some branch records name one more review each).
- **Our public chain is not anchored.** The LEZ chain at `https://lez.nameswallet.app/` runs in `standalone` mode: it
  posts nothing to the Logos Blockchain, and its channel is unknown there (`proposal/SEQUENCER-SIGNATURES-STUDY.md` §1
  item 5). We run its only sequencer. So every LEZ-side result below rests on our sequencer applying blocks honestly.
- **The keys are ours.** The deposit-root attester (v2–v5), the update submitter (v4–v6), the three v7 signer keys, the
  vault admin, freezer and fee recipient (one throwaway deployer key on v5, `vault/DEPLOYMENTS.md` v5 table) and the LEZ
  bridge admin and freezer (the chain operator on v6 and v7, their records) are held by the team.
- **LEZ testnet 0.4 is not live.** Nothing ran on it. All funds are test funds.
- **Not analysed:** whether the GitHub-hosted wrap runner learns more than a release's public journal
  (`DESIGN.md` §4 step 4); network-level anonymity (out of scope in RFP-022, disclosed in §7).
- **Failsafe known limit (not deployed):** `BridgeVaultFailsafe.sol` fixes `finalState` on the first update after
  `windDown()` without checking that LEZ minting has stopped (today's LEZ mint freeze is reversible and G2 does not
  track it), so if LEZ minting is not frozen first or is unfrozen later, a late exit can be paid while the same deposit
  is reclaimed (forged mints before the final also consume reclaim room). The variant is not deployed; M6's irreversible
  LEZ mint freeze closes it (`public-demo/prove/failsafe-20261117/README.md`).
- **v7 signers:** today's signers check neither privacy receipts nor public execution yet (they re-hash and re-extract
  blocks); with our stand-in keys on our one sequencer, v7 trusts us.
- **No admin timelock today:** neither the vault nor the LEZ bridge enforces a timelock on changing the freezer
  (`setFreezer`) or lifting freezes (`unfreeze`/`unfreezeAll`), so a compromised admin can unfreeze and replace the
  freezer in one block; M4 adds an admin timelock.
- **Reproducible guest builds:** reproducible guest builds with RISC Zero's Docker builder are not done yet (set up in
  M1).

## 2. Assets

| Asset | Where | What protects it |
|---|---|---|
| Escrowed ETH and ERC-20s | each Sepolia vault | the vault's conservation rule, caps, release delay, freeze, and the proofs it checks (§5, §6) |
| Backing of the wrapped supply | LEZ wrapped definitions | mint only by the bridge program's `MintBy` (typed `Authority::Program`, `DESIGN.md` §3.2); claims ≤ deposits per (token, denomination) |
| Unclaimed deposits | vault | the claim nullifier; the deposit secret `k` held by the depositor |
| Unreleased exits | vault exit tree | the release nullifier; the exit secret `k'` held by the burner |
| User secrets (phrase, master secrets `s`, `k`, `k'`, `app_secret` values) | the user's machine | never in a reply, a log or argv (`AGENTS.md` "Secrets"; `DESIGN.md` §1.1) |
| The link deposit ↔ claim, burn ↔ release | both chains | commitments, one-time addresses, fixed denominations, relays (§7) |
| A light-client instance's verified state | LEZ PDAs of `eth_attest` | sync-committee signature checks; write-once config (§4 of [LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md)) |
| Liveness of claims and exits | off-chain services | §8 |

## 3. Trust assumptions, version by version

Common to every version:
- **Ethereum** (Sepolia here): its finality, and for the light-client path an honest supermajority of the current
  512-member sync committee (§5.1).
- **RISC Zero proof systems.** The STARK (zkVM, LEZ's privacy circuit) has no trusted setup. The Groth16 wrap the vault
  verifies rests on RISC Zero's trusted setup: sound if at least one contributor in each phase destroyed their secret
  (`DESIGN.md` §7). The vaults bind the bare verifier 3.1.0 `0x8c915b50…aB3b`; 3.0.0 is under advisory
  GHSA-g6qg-9j8p-7j87, and v1/v2 are bound to it, so a forged proof could release from them (`DESIGN.md` §1).
- **The LEZ sequencer** (ours): liveness, ordering and censorship, and validity of public execution. Under the TrustedTip
  update image, private claims and burns are also taken on its word (`unchecked-private`, `DESIGN.md` §4).
- **The token issuer** (inherited, per RFP-022): blacklisting the vault or a recipient, or upgrading the token.
- **Code correctness** of the vault, the gates, the LEZ programs and the guests: unaudited.

| Version | In (Ethereum → LEZ): who decides a deposit is real | Out (LEZ → Ethereum): who decides which LEZ blocks count | Release delay | State |
|---|---|---|---|---|
| v2, v3 | our attester key (stand-in root source) | nobody: neither vault can take an update (`DESIGN.md` §1) | — | deposits and claims RUN; no release possible |
| v4 | our attester key | one immutable submitter key at `UpdateGatedVerifier` | 1 h | round trip RUN with real proofs (`v4-round-trip`) |
| v5 (the page's and the wallet's default) | our attester key | the same submitter key | 0 (test-network choice, `vault/DEPLOYMENTS.md` v5) | RUN, public and private; unattended updates (`v5-private-round-trip`, `v5-unattended`) |
| v6 | **the RFP-021 light client**: a claim proves `isDepositRoot[root]` under an anchor the instance finalized; the attester's `AttestRoot` was refused on chain (LEZ block 23349) | one submitter key, as v5 | 0 | RUN (`v6`, `v6-round-trips`, `v6-wallet-claim`); no standing updater (its one update ran by hand) |
| v7 | the light client, as v6 | **2 of 3 fixed signer keys** in `SeqSigGate`; anyone submits, contests and finalizes; today's signers check neither privacy receipts nor public execution yet, so with our stand-in keys on our one sequencer v7 trusts us | contest window 600 s (test flag; the gate's minimum is otherwise 6 h, `SeqSigGate.sol` `MIN_CONTEST_WINDOW`) | RUN (`v7`, `v7-equivocation`); the three keys are ours |
| Accredited-committee route, as proposed (M2/M6) | the light client | **a quorum of the channel's accredited committee**, bounded by the caps and the contest window; not by stake today (each public-testnet key holds the minimum stake, and LEZ slashing is a two-thirds vote with no evidence check; issue #1130) | the contest window, sized at M2 | DESIGNED (`proposal/SEQUENCER-SIGNATURES-STUDY.md` §8 (b1), a private study not in this branch's tree); needs an LEZ 0.4 chain anchored to the Logos Blockchain with several independent accredited keys, which does not exist yet. The study also finds the gate's 6 h minimum window below Logos Blockchain finality as it runs today (about 10 h at k = 1000) and recommends raising it |

What no version trusts: a relayer, a release relayer, the settlement service or the wrap runner for safety (they can
stop, not redirect; §4), and the light-client update submitter (the program checks every update).

## 4. Roles: who can do what

The proposal form's participant table (`proposal/FORM-short.md` lines 192-199) lists the off-chain roles. This table
adds the on-chain roles and corrects two points where the code says more than the form or `DESIGN.md` §7 said
before 2026-11-18 (`DESIGN.md` §7 is corrected on this branch) (marked
**correction**).

| Role | Held today by | Can | Cannot | Source |
|---|---|---|---|---|
| Vault admin | one throwaway deployer key (v5: also freezer and fee recipient) | register and deregister tokens, add denominations, set fees (≤ 2%), the relay-fee menu, caps, global caps, cap weights, release delay (≥ the immutable minimum), freezer, fee recipient; propose a new admin; unfreeze. **Correction:** it can also freeze (`onlyFreezer` admits the admin), and a cap of 0 with a non-zero window refuses every deposit or release of that token, so the admin can stop a token's releases without the freeze. **No timelock today:** `setFreezer` and `unfreeze`/`unfreezeAll` take effect immediately in one block (M4 adds a timelock) | change the verifier, the image ids or `cfgId` (immutable; `test_NoSetterForVerificationInAbi`); take principal (`withdrawFees` pays only `accruedFees` to the fee recipient); change where a release pays (fixed in the proof) | `vault/src/BridgeVault.sol` `onlyFreezer` (lines 324-327), `_use` (564-571), `withdrawFees` (795-801) |
| Vault freezer | the same key | freeze deposits and/or releases, per token or globally; a global release freeze also blocks `update` | unfreeze; move funds | `BridgeVault.sol` 405, 808-819 |
| Vault fee recipient | the same key | receive collected fees (anyone may trigger `withdrawFees`) | anything else | 795-801, 851-855 |
| Update submitter (v4–v6) | one key of ours, immutable in `UpdateGatedVerifier` | decide which LEZ blocks an update covers; so push an update over made-up blocks, bounded by conservation, caps and the delay | change what a proof proves; release to an address no exit names | `vault/src/UpdateGatedVerifier.sol` header |
| v7 signer keys | three keys of ours on three machines, threshold 2 | 2 together can sign an update over made-up blocks (safety needs f ≤ T − 1 = 1) or sign a conflict that halts the gate. Today's signers check neither privacy receipts nor public execution yet, so with our stand-in keys on our one sequencer v7 trusts us | one key alone: neither forge nor halt; a key that signs two hashes for one block is ejected (run on Sepolia, `v7-equivocation`) | `vault/src/SeqSigGate.sol` header |
| Gate caller (v7: `submit`, `contest`, `finalize`) | anyone | stage a correctly signed update, contest with evidence, finalize after the window | stage an update with fewer than T signers (refused read-only on Sepolia, `v7` row) | `SeqSigGate.sol` 264-367 |
| LEZ bridge admin | the chain operator (v6, v7 records); ours on every version | register and deregister tokens, add denominations, unfreeze, set the freezer, set the sponsor amount (≤ 50,000,000), propose an admin, set the attestor. **Correction:** on a stand-in config (v2–v5) `SetAttestor` lets the admin name any attestor, itself included, so the admin can record a made-up root and mint unbacked wrapped tokens on that chain. `DESIGN.md` §7's admin row did not say this before this branch; the 2026-11-14 review notes only that `SetAttestor` does not revoke roots already recorded (its L1). **No timelock today** on freezer changes or lifting freezes (M4 adds one) | on an attested config (v6, v7): record any root (`AttestRoot` is refused whatever the attestor, `an_attested_config_refuses_the_stand_in_root_under_any_flag`); change the root source (no instruction writes it); mint by itself | `lez-bridge/bridge/src/lib.rs` `admin` (from line 1165; `SetAttestor` line 1228), `AttestRoot` effect (1071-1080) |
| LEZ bridge freezer | the chain operator | freeze minting and/or burning, per token or globally | unfreeze | `bridge/src/lib.rs` 1167-1170 |
| Deposit-root attester (v2–v5) | our operator key | record any vault root, so mint unbacked wrapped tokens on that chain | anything on v6 or v7 | `DESIGN.md` §7; refusal on v6: LEZ block 23349 |
| Bridge deployer (loader header key) | ours | one `Initialize` (picks admin, freezer, attestor, chain id, root source); re-deploy the same program id on another chain | mint, rotate the mint authority, take a role, re-initialise (fixed programs) | `DESIGN.md` §3.2, §7 |
| Light-client update submitter | anyone; only our follower submits today | advance an instance with an update the program verifies | insert a header the committee did not sign | `DESIGN.md` §9.2; `lc-public`, `lc-handoff` |
| Instance creator / checkpoint source | anyone may `Init` an instance | choose the checkpoint, and so the chain, of the instance it creates | change any instance's config after creation (`effects_only_come_from_the_program_and_config_never_changes`) | [LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §2 |
| Claim relay (LEZ fee payer) | ours alone on our public chain, one per bridge instance (`public-demo/RUNBOOK.md` (not published) §13, §13.3) | withhold its co-signature; see the claim (a public claim carries `k`) and the claimer's IP before inclusion | change the claim or take the mint (the claimer's signature covers the message) | `DESIGN.md` §3.6 |
| Release relayer | anyone (`release` is open; the vault checks the proof) | send a release and take the relay fee the exit allowed | redirect, or take more than the fee bound in the exit (`test_RevertRelayerRedirectsOrOvercharges`) | `BridgeVault.sol` 461-515 |
| Settlement service | ours (`public-demo/settle/settle.py`) | prove, wrap and send a release when a wallet sends its exit secret; so learn which burn pays which address | redirect (the recipient is fixed in the exit) | `public-demo/RUNBOOK.md` (not published) §22 |
| Groth16 wrap runner | a GitHub-hosted runner | see the release receipt it wraps | change the journal (the vault checks the seal over it) | `DESIGN.md` §4 step 4 |
| Deposit desk owner | ours | set the bridge fee 0–3% on its own contract, withdraw collected fees | hold, redirect or delay a deposit | `DESIGN.md` §3.6 |
| Token issuer | the issuer | blacklist the vault or a recipient; upgrade the token | — | RFP-022 Trust model |

## 5. Attacks and what stops each

### 5.1 Against the light client (RFP-021)

| Attack | What stops it | Evidence | State |
|---|---|---|---|
| A fabricated chain: a self-consistent header and inclusion proof over state never finalized (VS1) | a statement needs `AssertAnchor` on an anchor the instance recorded from a committee-signed update | `an_anchor_the_instance_never_finalized_is_refused`, `a_wrong_checkpoint_or_bootstrap_is_refused`; refused on a chain (v2 runs, `DESIGN.md` §3.4) | TESTED; refusal RUN locally |
| A header signed by a committee not handed off from the checkpoint | the store holds commitments to the current and next committee; a witness that does not match is refused | `the_wrong_committee_witness_is_refused` (E13) | TESTED |
| Low participation | `min_participants` ≥ 342 of 512 and Helios's 3 × signers ≥ 2 × 512 | `participation_below_the_instance_policy_is_refused`; 341 refused on a local chain (finality-split record, branch `public-update`) | TESTED; RUN locally |
| A tampered state or receipts root | roots come from the anchored header, never from the caller | `a_tampered_receipts_root_is_refused_with_the_exact_error`; `a_statement_is_refused_for_a_wrong_value_account_or_anchor` | TESTED |
| A proof replayed for another chain or instance | chain id and instance id in the anchor query and in every statement id | `an_update_for_another_instance_or_chain_is_refused`, `a_consumer_bound_to_another_instance_or_chain_refuses_the_statement`; E07 on a local chain (error-codes record, `public-update`) | TESTED; RUN locally |
| A duplicate or racing update | the store applies `Advance{prev, next}` only if `prev` is its state | `replayed_older_and_stale_updates_are_refused` | TESTED |
| Cycle griefing by low participation | the public path computes the signers' key from key witnesses; a low-participation update is split in two steps | `DESIGN.md` §9.3, §9.3a; finality-split record | RUN locally (synthetic committee) |
| Catching an instance up past the weak-subjectivity period | updates past the deadline are refused (E04) | `e04_an_instance_past_the_weak_subjectivity_bound_refuses_updates` (branch `public-update`) | TESTED on a branch; the instance behind v6 and v7 runs that image (v6 record) |
| **A colluding sync committee** (≥ 342 of 512 seats in one period) | **nothing inside the light client.** Bounded only by the consumer's caps, freeze and delay | `DESIGN.md` §9.8 | §5.3 below |

### 5.2 Against the bridge (RFP-022)

| Attack | What stops it | Evidence | State |
|---|---|---|---|
| Claim a deposit twice | claim nullifier PDA; `commitmentUsed` makes it one per deposit | `a_claimed_deposit_is_refused_in_every_retry_shape`; repeat refused on chain (block 2855) | RUN |
| Claim a victim's deposit with a leaked `k` | nullifier bound to claimer, token and denomination (review H2 fix) | `h2_a_leaked_secret_cannot_burn_the_deposit_with_a_cheaper_one`; throwaway-chain run | RUN locally |
| Mint with the program's header key | typed `Authority::Program(bridge)` (review C1 fix) | `c1_*` tests; throwaway-chain run | RUN locally |
| Front-run `Initialize` | only the header account initialises (H1 fix) | `h1_a_stranger_cannot_init` | RUN locally |
| Claim against another deployment, chain or vault | `cfgId` binds EVM chain id, vault, LEZ channel id, root source and program id | `test_RefusedBySameAddressOnOtherChain`, `a_deposit_for_one_deployment_is_refused_by_the_other` | TESTED; the LEZ channel id is declared at `Initialize` and checked by the client, not readable by a program on rc3 (`DESIGN.md` §2) |
| A relayer redirects or overcharges a release | recipient, relayer and fee are in the release journal | `test_RevertRelayerRedirectsOrOvercharges`, `test_CopiedSealCannotBeRedirected` | TESTED |
| Release twice | release nullifier | `test_ReleaseReplayVariantsAllNullifierSpent` | TESTED |
| A stolen submitter key (v4–v6) pushes an update over made-up LEZ blocks | conservation (§6), caps, the release delay (none on v5 and v6), the freezer | `vault/test/Invariant.t.sol`; `UpdateGatedVerifier.sol` header | TESTED; **not stopped** within those bounds |
| Two v7 keys sign made-up blocks | the threshold (2 of 3), the contest window, ejection of an equivocating key | `v7`, `v7-equivocation` | RUN; with 2 colluding keys of 3, **not stopped** within the bounds of §6 |
| A replayed or non-advancing G2 journal | `prevState == g2State` and `NoStateAdvance` | `test_RevertStaleStateAndReplay`, `testFuzz_NoStateAdvanceNeverApplies` | TESTED. **Open:** a journal that returns to an earlier state (A → B → A) is not refused on chain (`DESIGN.md` §4) |
| Swap the verification logic (RFP-022 S8, RFP-021 VS3) | immutable programs (deployed immutable), vault without a setter for verifier or image ids | `test_NoSetterForVerificationInAbi`, `test_VerificationUnchangedAfterEveryAdminCall` | TESTED; immutable deploys RUN |
| Grief the claim relay with claims that revert | the relay pre-checks and caps per deposit, IP and day | `DESIGN.md` §3.6 | RUN on our public chain |
| An admin on a stand-in config sets itself as attestor and mints | **nothing** on v2–v5; v6 and v7 remove the stand-in source | §4 correction | design limit of the stand-in versions |
| A depositor claims on a second LEZ chain running the same program id | since the 2026-11-14 fixes, the declared channel id in `cfgId`; the program cannot read its chain id on rc3 | `DESIGN.md` §2 "Known issue" | v3 onward; v2 still exposed |

### 5.3 What a colluding sync committee can do (RFP-021 VS4)

From `DESIGN.md` §9.8 (consensus-specs at `313cbfe`): 512 seats a period (about 27.3 h), drawn in proportion to
effective balance and known about 27 h ahead; signing a false header for light clients is not slashable. Holding stake
alone, an attacker with half of all stake draws 342 seats with probability about 1.2 × 10⁻¹⁴ per period; even odds need
about two thirds of all stake. Bribing the holders of 342 seats risks none of their stake; we give no ETH figure for it.

- **On the light client:** one false period update can name a committee of the attacker's choosing; from then on the
  attacker signs anything. The only way out is a new instance from an independently checked checkpoint
  ([LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §6).
- **On the bridge (v6, v7):** a false anchor lets anyone prove a made-up vault root and mint wrapped tokens with no
  deposit. There is no LEZ claim cap (deviation F11), so the LEZ mint freeze is the only stop. Once forged claims are
  burned and an update records them, the vault releases up to the deposited-but-unclaimed value of each (token,
  denomination), within release caps and after the release delay, unless the freezer acts; honest claims then push
  claims above deposits and every later update reverts (`DESIGN.md` §9.8; `research/failsafe-design.md` §1).
- **What a consumer can do:** freeze each half (built); vault release caps and release delay (built); an optional LEZ
  claim cap and a minimum anchor age (DESIGNED, M4); a participation floor of 427 instead of 342 (DESIGNED; at f = 2/3
  the chance of 427 seats is about 1.3 × 10⁻¹⁷).

### 5.4 What an attestation proves, and what it does not

**It proves**, for one instance: that the anchor was recorded from an update signed by at least the instance's
`min_participants` members of the committee the instance tracks forward from its checkpoint; that the account, storage
slot or receipt log is included under that anchor's state or receipts root; that the predicate holds of it; and that
it is about the instance's chain id ([LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §3, §7).

**It does not prove:** that the instance's checkpoint is on the canonical chain (the consumer checks that once, §6 of
the spec); that the committee was honest (§5.3); that the fact is still true after the anchor's block (a statement is
about that block); that the fact has not been acted on before (uniqueness is the consumer's nullifier); anything that
hides Ethereum activity; anything about LEZ state on Ethereum.

## 6. Damage bounds

- **Conservation, per token** (`DESIGN.md` §5; `testFuzz_Solvency`, `invariant_all`): released value ≤ burned value ≤
  claimed value ≤ deposited value. Claims ≤ deposits per (token, denomination); burns ≤ claims by value per token;
  releases ≤ burns per (token, denomination). It holds against a forged update too: no update can take more than was
  deposited. It does not stop a forged update from releasing what was deposited and not yet released.
- **Caps** (F11, S4): per token and global, deposit and release, rolling window by a two-bucket estimate; up to about
  twice a cap can pass in one window (`test_RollingEstimateAdmitsAboveCapPerWindowBelowTwice`). Values on the Sepolia
  vaults are those deploys' own choices (50,000 dUSDC and 5 ETH per token per day, 100,000 USD global, `DESIGN.md`
  §6.1a); the recommended defaults (10% of holdings per token per 24 h, floor 10 × the largest denomination) are
  DESIGNED and set in M4.
- **Release delay / contest window:** v4 1 h; v5 and v6 0, so on them a forged update can be released before a freeze;
  v7 600 s as a test setting (6 h minimum otherwise). The recommended value is the contest window sized at M2
  (`DESIGN.md` §6.1a).
- **Freeze:** each half alone; the vault's global release freeze also stops `update`.
- **LEZ side:** no claim cap today; the mint freeze is the only stop for an unbacked mint on LEZ.

## 7. What each off-chain participant observes

| Participant | Observes | Safety or liveness |
|---|---|---|
| Light-client update submitter | public Ethereum data | liveness only |
| Beacon and execution endpoints | which data a client asks for, and its IP | liveness only (withhold, not forge) |
| Checkpoint source at `Init` | nothing about users | safety for consumers that bind to that instance |
| Claim relay | the claim before inclusion (a public claim carries `k`, so the full link), the claimer's IP | liveness only |
| Release relayer | the release's public arguments | liveness only |
| Settlement service | the exit secret when the wallet sends it, so which burn pays which address | liveness only; a privacy exception to RFP-022 PP4 that a prover on the user's machine avoids |
| Wrap runner (GitHub-hosted) | the release receipt it wraps (the journal names the recipient) | liveness only; not analysed beyond the journal |
| Deposit-root attester (v2–v5) | public vault roots | **safety** (can mint unbacked on LEZ) |
| Update submitter (v4–v6), v7 signers | public LEZ blocks | **safety** of releases, within §6 |
| LEZ sequencer (ours) | every transaction and its sender's IP | liveness; validity of public execution |
| Ethereum and LEZ RPC providers | the queries and IP; `recover --lez-mode point` tells the sequencer which nullifiers a user watches (opt-in) | liveness |

IP-level correlation between a user's Ethereum deposit and later LEZ activity is not hidden by anything here (RFP-022
Out of Scope); the relays and RPC endpoints a user picks see the user's IP.

## 8. Can anyone block a user? (RFP-021 F12, RFP-022 Trust model, R9)

**Open in code:** submitting light-client updates, relaying a claim (an open, refunded relay RUN locally, branch
`open-relay`), relaying a release, and on v7 submitting, contesting and finalizing updates. **Closed in code:** the
deposit-root attester of a stand-in config (v2–v5: one attestor account the admin names) and the update submitter of
v4–v6 (one immutable key in `UpdateGatedVerifier`). **Today one operator, us, runs every role on our public chain.**
So, individually:

| If this stops | Effect today | Mitigation today | Mitigation designed |
|---|---|---|---|
| Our light-client follower | v6/v7 claims wait for the next anchor; nothing is lost | anyone runs `lez-bridge attest update-public` (branch `public-update`) | `lc-follower` as a Logos module (branch `lc-daemon`); operator reward (branch `lc-incentive`) |
| Our attester (v4, v5) | claims on v4/v5 wait; deposits stay in the vault | none on those versions | v6 and later need no attester |
| Our claim relay | a public claimer pays its own LEZ fee; a never-used account cannot claim publicly without a fee payer | self-paid claim; private claims are sponsored from the float | several open relays with failover (RUN locally, M5) |
| Our update submitter (v4–v6) | every exit on that vault waits; a lost key stops updates for good (`UpdateGatedVerifier.sol` header) | none | v7's open gate; the accredited-committee route (M6) |
| Two of the three v7 keys | v7 exits wait | none | real accredited keys (M6) |
| Our settlement service | a wallet without a prover cannot finish a release | none packaged for users: the release prover (`outbound_host release --prove`) runs through owner scripts on the build host, and the wrap needs an x86-64 machine with Docker; anyone can send `release` once a seal exists | a release relayer daemon and a user-side prover path (M4, M5) |
| GitHub-hosted wrap runner | releases and updates cannot be wrapped there | any x86-64 Linux machine with Docker runs `outbound_wrap` | — |
| Our sequencer | the LEZ chain stops | none: we run the only one | an LEZ testnet with independent sequencers [ext] |
| Sepolia RPC providers | reads slow down or fail | fallback list Tenderly → PublicNode → thirdweb (`modules/lez_bridge/rust-lib/src/config.rs`) | — |

**As a class:** if every party stops (all submitters, all relays, all beacon endpoints), nothing moves, and nothing is
lost: deposits stay in the vault, claims stay valid indefinitely (R4, `an_attested_claim_under_the_first_anchor_works_after_the_instance_moved_on`),
exits stay releasable once recorded. RFP-021 says this plainly: "liveness rests on interested parties choosing to
spend the gas". An instance left past its weak-subjectivity deadline cannot be advanced again and must be replaced
([LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §6); its anchors stay valid.

**Not met today:** RFP-022's "none of them, individually … can block": the attester (v4, v5) and the update submitter
(v4–v6) are single parties whose refusal blocks claims or exits on those vaults. v6 removes the first; the second needs
the route of §3's last row (M6).

## 9. The freeze works at token and protocol level only

Neither the vault nor the bridge program knows whose deposits, claims or exits are whose: a deposit publishes a
commitment, a private claim mints to a one-time address, a burn publishes an exit commitment. So a freeze can stop a
token or the whole protocol, on either half, and nothing narrower. This is a designed consequence of the privacy
guarantees (RFP-022 Trust model), not a gap.

## 10. Conditions under which the guarantees degrade or fail (RFP-021 Sup6)

- The current sync committee has ≥ 342 dishonest seats (or ≥ the instance's floor): false anchors (§5.3).
- The checkpoint an instance was created from is not on the canonical chain: every statement of that instance is about
  another chain. Consumers must check it before binding ([LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md) §6).
- An instance stays unadvanced past its weak-subjectivity deadline: it refuses updates for good; its old anchors stay
  valid, and old committee members may have exited.
- Ethereum activates a fork the instance's schedule does not name: its updates stop verifying; a fork after Gloas needs a
  new program image (`DESIGN.md` §9.7).
- A bug in Helios's update logic, in our hooks on the public path (signature check, next-committee root), in the
  inclusion code, or in the bridge, vault or guests: unaudited.
- The RISC Zero Groth16 setup was compromised: forged updates and releases, within §6.
- The deposit-root attester (v2–v5), the update submitter (v4–v6) or 2 of the 3 v7 keys act dishonestly: within §6.
- Our sequencer applies invalid blocks or invents private transactions: the update proofs take them on its word.
- A user's machine or phrase is compromised: that user's positions only.

## Sources

`DESIGN.md` §0–§10; `vault/src/BridgeVault.sol`, `UpdateGatedVerifier.sol`, `SeqSigGate.sol`;
`lez-bridge/bridge/src/lib.rs`; `vault/DEPLOYMENTS.md`; `docs/EVIDENCE.md` and `docs/evidence-checks.json`;
`research/security-review-2026-11-14.md`; `research/failsafe-design.md`; `proposal/SEQUENCER-SIGNATURES-STUDY.md`
(untracked in the working copy of `main`, not in this branch);
branch records `public-demo/prove/v6-lc-20261118/README.md` (`v6-lc`), `public-demo/prove/v7-seqsig-20261118/README.md`
(`v7-seqsig`), `public-demo/prove/error-codes-20261117/README.md` and `cli-ws-20261117/README.md` (`public-update`).
Related, on other branches: `docs/OPS-V5-V6.md` (`v6-ops`), `docs/PRIVACY-M5.md` (`privacy-m5`, being written),
`docs/SEQSIG-ACCREDITED.md` (`seqsig-accredited`, not written when this was).
