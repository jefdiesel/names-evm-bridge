# The RFP-021 light client: specification-level description

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFP's own words)

- **RFP-021 Design Rationale, "Trust assumptions":** "the specification must say how a verifier confirms the
  checkpoint independently, and when an instance that has fallen outside the weak-subjectivity period must be
  replaced."
- **Functionality #10:** "An instance that has fallen outside the weak-subjectivity period is replaced by a freshly
  initialised instance rather than refreshed in place. Document when an instance must be replaced, and what a consumer
  must do to migrate to a replacement."
- **Verification Security #3:** "Change requires deploying a new version and having consumers move to it; document the
  migration mechanism and what consumers must do. The consumer documentation (Supportability #4) must state that the
  same rule applies to consumers: a program holding this module's `ProgramId` in mutable state lets whoever controls that
  pointer substitute a module that attests to anything."
- **Supportability #4:** "The consumer documentation must include a **"Recommended Consumer Pattern"** section covering:
  enforcing uniqueness with a nullifier keyed on the statement identifier from Functionality #6, verifying the chain
  binding matches the consumer's expectation, handling each typed error code, and the recommended behaviour when an
  attestation cannot be produced (refuse the action, never fall back to an unverified input)." (§9 below.)
- It also describes, for consumers, what Functionality #1–#9, #13–#15 deliver. The full developer integration journey
  (Supportability #3) and the CLI and operator journeys (Supportability #5) are outlined in
  [OPERATORS.md](OPERATORS.md); the module, daemon and IDL documents are being written on branch `lc-daemon`.

## Status

- **Program on `main`:** `eth_attest` with `Init` and the private-path `Update` (`lez-bridge/attest/program`). RUN on
  local chains: a real Sepolia finality update proved on CPU and accepted (rc1 block 1859, rc3 block 950; `DESIGN.md`
  §9.0, §9.0a).
- **Public path (branch `public-update`; the program behind vaults v6 and v7, deployed on our public chain as
  `9Wb7sBxs…uhNR`):** `UpdatePublic`, `PeriodBegin`/`PeriodFinish`, `FinalityBegin`/`FinalityFinish`, `CloseJob`, the
  weak-subjectivity rule, typed codes `RFP021-E01`…`E13`, account and receipt-log statements. RUN on our public chain:
  finality updates (`lc-public`) and one committee handoff, period 1416 → 1417, run by the follower loop (`lc-handoff`).
  RUN on local chains: a three-period catch-up, low participation in two steps (synthetic committee), statements and
  both reference consumers (records on `public-update`). Reviewed once by a separate AI agent; not audited.
- **Not built:** the checkpoint-verification API in the Logos Ethereum core module [ext]; a public-path run past more
  than one handoff on a public chain; a deliberate multi-period stall on a public chain (M3); any LEZ testnet 0.4 run.

## 1. Objects

- **Instance config** (`InstanceConfig`, `lez-bridge/attest/core/src/lib.rs`): `evm_chain_id`,
  `genesis_validators_root`, `genesis_time`, `seconds_per_slot`, `forks` (eight slots: genesis … gloas; an unscheduled
  fork has epoch `u64::MAX`), `checkpoint` (the beacon block root the bootstrap must hash to), `min_participants`
  (≥ 342 of 512), `salt`. Checked once by `validate()`.
- **Instance id:** `SHA256("RFP021/instance/v1" ‖ program ‖ borsh(config))`. Another config, or another program, is
  another instance.
- **PDAs of the program, per instance** (seeds `SHA256(domain ‖ instance id …)`): `config` (written once), `store`
  (`LcSummary`: the finalized beacon header with its execution anchor and SHA-256 commitments to the current and next
  sync committees, 259 bytes), one `anchor` account per execution anchor the instance ever finalized (never deleted),
  and on the public path one `job` account per (store state, whole update).
- **Anchors:** an execution state root, or an execution block hash (then the consumer supplies the header RLP). Under
  Gloas (Sepolia) the light-client header carries only the execution block hash (`DESIGN.md` §9.1).

## 2. Initialisation (the one trusted input)

`Init { config, bootstrap }`: the program checks `config.validate()`, checks that `bootstrap` (the SSZ of
`/eth/v1/beacon/light_client/bootstrap/{root}`) hashes to `config.checkpoint`, then writes the config and the first
store. **Anyone may create an instance**; a second `Init` of the same config is refused. No instruction rewrites a config
or a store other than by a verified update (`effects_only_come_from_the_program_and_config_never_changes`; RFP-021 F9,
F13). Recovery from a wrong checkpoint is a new instance, never an edit (F13).

## 3. What is verified per update

An update is accepted on its validity alone; the submitter's identity plays no part (VS6;
`a_real_finality_update_advances_the_store_permissionlessly`; on our public chain the submitter is a throwaway key with
no role, v6 record).

1. **Helios's update checks.** The program calls Helios consensus-core at git rev `2af9b418` (`verify_generic_update`,
   which follows the consensus-specs `validate_light_client_update`): slot ordering, the finality branch, the next-committee
   branch for a period update, the committee selected by the signature period, and the sync-committee aggregate BLS
   signature over the signing root of the attested header, under the fork version and genesis validators root of the
   instance's config. We did not re-audit Helios; on branch `public-update` the consensus-specs light-client sync vectors
   v1.6.1 pass, 22 of 22 applicable cases, through three verification paths (spec-vectors record).
2. **Our additions** (`lez-bridge/attest/program/src/lc.rs`):
   - the committee witnesses must match the store's commitments (else E13);
   - at least `min_participants` signers (else E11), and Helios's 3 × signers ≥ 2 × 512;
   - the time window: an update applies only from the start of its signature slot (`not_before_ms`, enforced by the
     sequencer through the transaction's timestamp window), and, on the public-path image, only before the store's
     weak-subjectivity deadline (§6);
   - the store applies `Advance{prev, next}` only if `prev` equals its state (a racing or replayed update fails, E09)
     and `next` is progress: a later finalized slot or a newly learned committee (else E10).
3. **Where it runs.**
   - **Private path (`Update`, on `main`):** verified in the submitter's own actor as a privately proved execution (one
     finality update is about 37M cycles, over LEZ's 10M public cap); only the ~520-byte `Advance` reaches the store. The
     store trusts it because the sender program is stamped by the runtime and the image is an immutable program
     (`DESIGN.md` §9.2). Cost: about 2 h of CPU proving per update (7,283.0 s on rc3).
   - **Public path (`UpdatePublic`, branch `public-update`):** an ordinary public transaction. The signature is checked
     against the committee's aggregate key minus the non-signers' keys, given as uncompressed key witnesses that must be
     on the curve, not infinity, and equal to the committed bytes; the next committee's root is computed by direct
     SHA-256 (a second Helios hook, tested equal to Helios's). 9,285,563 cycles at 506/512 on a local chain (92.9% of the
     cap). One transaction is estimated to fit down to about 442 signers; below that a finality update is split at the
     pairing seam (`FinalityBegin` / `FinalityFinish`), run on a local chain at 427 and 342 signers of a synthetic
     committee (finality-split record).
     **Open:** whether skipping a per-key G1 subgroup check is sound rests on every sync-committee key having been
     validated when the committee was formed; `DESIGN.md` §9.3a says M3 confirms this against the specs, while the
     proposal form states it as settled (see the index's contradictions list).

## 4. Sync-committee handoffs

- The store's period is the period of its **finalized header**, not the wall clock (`DESIGN.md` §9.6). An update is
  accepted if its signature slot is in the store's period, or in the next period when the store knows the next
  committee.
- **Learning the next committee:** a period update (an item of `/eth/v1/beacon/light_client/updates`) signed and
  finalized in the store's period records it. On the public path this takes two transactions with a job account:
  `PeriodBegin` (every check but the signature; opens the job keyed by the store and the whole update) and
  `PeriodFinish` (the signature; applied only if the store has not moved). Anyone may finish a job; `CloseJob` closes a
  job whose store has moved on.
- **Rotation:** an update whose finalized header lies in the next period rotates the committees (current ← next).
- **Catch-up after a stall:** submit, in order, the period update of the store's period (if the next committee is
  unknown), the update of each later period up to Ethereum's, then a current finality update. Each step verifies the
  handoff before it; nothing is skipped; anchors recorded before the stall stay valid (R3, R7).
- **Run:** one handoff on our public chain, 2026-11-18 (LEZ blocks 27862, 27983–27985; `lc-handoff`); a catch-up over
  periods 1413 → 1416 with three handoffs on a local chain (catch-up record, `public-update`). Not run: a deliberate
  stall of several periods on a public chain (M3).
- **A period with no qualifying update** (finalized header outside the attested header's period) would block a
  catch-up; we have not seen one and have no test for it (`DESIGN.md` §9.6).

## 5. Ethereum forks

- `InstanceConfig.forks` names eight forks, genesis through Gloas; Helios at `2af9b418` handles signing domains up to
  Gloas (`DESIGN.md` §9.7).
- **"A new instance before the fork epoch":** once a fork's epoch is published, anyone initialises a new instance whose
  schedule includes it, before that epoch, and consumers move to it (§8). An instance whose schedule lacks the fork stops
  verifying at the fork, because updates are then signed under a fork version it does not know
  (`a_mainnet_instance_with_fulu_scheduled_later_refuses_the_real_update_by_its_signature`). Mainnet has no Gloas epoch
  in `configs/mainnet.yaml` at consensus-specs `313cbfe`, so a mainnet instance made today stops at Gloas.
- A fork after Gloas has no slot in the config or in Helios: it needs a new program image, and so a new program id.

## 6. The checkpoint, weak subjectivity, and when to replace an instance

**Who supplies the checkpoint:** whoever calls `Init`. Every consumer that binds to an instance trusts that checkpoint,
once. It is public: `lez-bridge attest show --json` prints an instance's config.

**How a verifier confirms it independently (a procedure proposed in this document; untested, and no tool ships it).**
The Logos Ethereum core module has no checkpoint-verification API yet (RFP-021 F9 [ext]: `logos-evm-beacon-api-module`
0.2.0 offers `lc_request` only). Until it does:
1. Read the instance's config from the chain and recompute its id with `lez-bridge attest id` (nothing sent): the id
   must equal the instance the consumer binds to.
2. Check `evm_chain_id`, `genesis_validators_root`, `genesis_time`, `seconds_per_slot` and the fork schedule against the
   network's published consensus configuration.
3. The config holds the checkpoint's block root, not its slot. Ask at least two independent beacon nodes (one of them
   your own, if you can) for the header with that root (standard beacon API `/eth/v1/beacon/headers/{block_id}`), take
   its slot, then ask each node for the canonical header at that slot and check that it is the same root and that the
   slot is at or below the node's finalized checkpoint (`/eth/v1/beacon/states/head/finality_checkpoints`).
4. Optionally fetch the bootstrap for that root and check that it hashes to it, as `Init` did.

**The weak-subjectivity bound (branch `public-update`; the image behind v6 and v7):** `ws_epochs` is 3,532 epochs
(about 15.7 days) for mainnet, from Electra's guide at 8.4M ETH or more, and 1,348 epochs (about 6 days) for every other
chain, the lower value of Gloas's guide (`eth_attest_core::ws_epochs`). The deadline is the start of the store's
finalized slot plus that bound. The program refuses, on every path, an update signed at or past the deadline (E04), and
the deadline ends every update's timestamp window, so once the chain's clock passes it no update applies
(`e04_an_instance_past_the_weak_subjectivity_bound_refuses_updates`,
`the_clock_ends_every_update_s_window_at_the_store_s_deadline`). The program on `main` (private path) does not enforce
it (`DESIGN.md` §11).

**Replace an instance when:**
- its store is past the weak-subjectivity deadline (it refuses every update; the follower stops itself at a margin
  before it, `lc-follower` README on branch `lc-daemon`);
- Ethereum has scheduled a fork its config does not name (§5);
- its checkpoint fails the procedure above;
- a committee capture is suspected (one false period update captures the instance for good; `DESIGN.md` §9.8).

**How:** a new `Init` from a fresh, independently checked checkpoint (a new instance id; the old instance is never
edited, F13). The old instance's anchors stay valid and its old statements stay true (R3).

## 7. Statements offered to consumers

A consumer verifies inclusion **in its own transaction** and sends one query to the attestation program in the same
transaction (RFP-021 F7):
- **Inclusion** (`eth_attest_core::inclusion`, run in the consumer's actor): an account (nonce, balance, storage root,
  code hash), a storage slot, or a receipt log (block, transaction index, log index), each proved with `alloy-trie`
  under the roots of the anchored header. Roots are never taken from the caller (F3).
- **Predicate:** `Eq` or `Ne` against a 32-byte value (F4; ordering comparisons are not built, see the index).
- **Finality:** `AssertAnchor(instance, chain id, anchor)` to the attestation program: refused unless that instance
  recorded that anchor for that chain id (E01, E07). Without it the statement means nothing.
- **Statement id** (F6): `SHA256(tag ‖ u64be(chain id) ‖ instance id ‖ the fact's fields ‖ predicate ‖ anchor kind ‖
  anchor)`, tags `RFP021/statement/storage/v1`, `.../account/v1`, `.../log/v1`. Storage statements also have
  `fact_id` (tag `RFP021/fact/storage/v1`), which leaves out the anchor and the instance but not the chain or the fact
  (`a_fact_id_ignores_anchor_and_instance_but_not_the_chain_or_the_fact`).
- **Cost:** a storage-slot statement 245,836 cycles, an account statement 172,039 (executor, `DESIGN.md` §9.3); on a
  local chain an account statement 218,212 gas and a log statement 88,930 (cli-ws record).
- **Reference consumers** (branch `public-update`, `lez-bridge/attest/consumer`): an ERC-20 `Transfer` log to a given
  contract, and native ETH to a contract proved through its `Deposit(address,uint256)` event (WETH9). A plain ETH
  transfer to a contract that emits no event is not provable this way (not built). Each keys a nullifier on (its
  deployment, statement id). RUN on a local chain, each refusing a replay (statements record).

## 8. Moving consumers to a new instance or a new program (F10, VS3)

- **Verification cannot be swapped.** The attestation program is deployed immutable; a new version is a new program id.
  A consumer must name the attestation program and the instance in state no one can rewrite: our bridge writes them
  once at `InitializeAttested` and no instruction changes them (`lez-bridge/bridge/src/lib.rs`, `Config::root_source`).
  **The same rule binds consumers:** a program that keeps this module's program id, or an instance id, in state some key
  can change lets that key point it at a module that attests to anything.
- **Migrating** therefore means a new consumer deployment (or a new write-once configuration of it) bound to the new
  instance. For our bridge it means a new bridge instance and a new vault (the root source is part of `cfgId`); the old
  vault should stop deposits after a last update has anchored them, and keeps releasing against its own exit roots
  (`DESIGN.md` §9.7, §10).
- **Uniqueness across the move:** a nullifier keyed on a statement id does not carry over, because the id includes the
  instance id. A consumer that must act once per Ethereum fact across instances keys its nullifier on an
  instance-independent id (`fact_id` for storage facts) and keeps its nullifier set across the move, or refuses facts
  older than its cut-over. The reference consumers key on the statement id and do not handle this (designed guidance;
  not built).

## 9. Recommended Consumer Pattern (Supportability #4)

1. **Bind immutably.** Hold the attestation program id, the instance id and the chain id in write-once state. Check the
   instance once with the procedure of §6 before binding.
2. **Check finality and inclusion in the same transaction.** Verify the inclusion proof in your own execution and send
   `AssertAnchor` in that transaction. Never act on a predicate computed in an earlier transaction.
3. **Check the chain binding.** The statement's chain id must be yours; `AssertAnchor` refuses another chain's anchor
   (E07).
4. **Enforce uniqueness.** Key a nullifier PDA on the statement id (once per statement), on `fact_id` (once per fact,
   whatever block it was proven at), or, as our bridge does, on a value derived from a user secret (deviation RFP-022 F3,
   `DESIGN.md` §11). Spend it in the same transaction.
5. **Handle each typed code** (`docs/ERROR-CODES.md` on branch `public-update`):

   | Code | Meaning | What a consumer does |
   |---|---|---|
   | E01 header not finalized | the anchor is not one the instance recorded | wait for an update past the event, retry |
   | E02 signature invalid | bad update | nothing (operators: fetch again) |
   | E03 handoff not verified | wrong period's update | operators: submit the right period update first |
   | E04 checkpoint stale or invalid | past the weak-subjectivity deadline, or a bootstrap that does not match | move to a replacement instance (§8) |
   | E05 inclusion proof invalid | the proof does not verify under the anchor | rebuild the proof at that anchor |
   | E06 predicate not satisfied | the fact is not what was claimed | refuse the action |
   | E07 chain id mismatch | another chain's anchor | refuse |
   | E08 malformed input | decoding failed | fix the input |
   | E09 stale or replayed | the store moved | operators: re-read and resubmit |
   | E10 not newer | the update brings nothing new | nothing |
   | E11 participation below policy | too few signers | wait for a better update |
   | E12 not permitted | an effect from another program | refuse |
   | E13 witness mismatch | committee witness not the store's | operators: re-read the store |

6. **When an attestation cannot be produced, refuse the action.** Never fall back to an unverified input, an operator's
   word or a cached result. A delay is the failure mode, not a loss.
7. **Bound your exposure** to a colluding committee: caps per window, a freeze, a delay, optionally a minimum anchor age
   ([SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) §5.3).
8. **Migrate deliberately** (§8).

## 10. Failure modes a consumer must expect

- No update for a while (no submitter): statements about new Ethereum state wait; old anchors keep working.
- A stalled instance past its deadline: E04 on every update; replace it.
- A fork the instance does not know: updates stop verifying at the fork epoch.
- A committee capture: false anchors; only the consumer's caps, freeze and delay bound the loss.
- A receipt-log consumer that needs plain ETH transfers to a contract with no event: not provable today.

## Sources

`DESIGN.md` §9 (all subsections), §11; `lez-bridge/attest/core/src/lib.rs` and `inclusion.rs`,
`lez-bridge/attest/program/src/lc.rs` and `tests/` (on `main` and on `public-update`); `docs/ERROR-CODES.md` and the
records `catchup-20261117`, `cli-ws-20261117`, `error-codes-20261117`, `public-handoff-20261117`,
`statements-20261117` (branch `public-update`); `public-demo/prove/v6-lc-20261118/README.md` (branch `v6-lc`);
`docs/EVIDENCE.md` rows `lc-public`, `lc-handoff`; `lez-bridge/attest/follower/README.md` (branch `lc-daemon`).
Operator reward: `docs/LC-INCENTIVE.md` (branch `lc-incentive`: payout rules unit-tested, program written, not
deployed); update costs: `docs/LC-COSTS.md` (same branch, not written when this was).
