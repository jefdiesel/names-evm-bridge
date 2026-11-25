<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Light-client operator incentive (RFP-021 Soft #5)

State (`DESIGN.md` labels): **(2) tested in simulation and unit tests only**, branch `lc-incentive`:
- the payout rules: 21 unit tests (`lez-bridge/attest/reward/core/tests/rules.rs`);
- the program: 29 whole-transaction tests (`lez-bridge/attest/reward/program/tests/rewarded.rs`) on
  the attestation program's real Sepolia fixtures. They run in our own simulator, which runs LEZ's
  `native_token` code and refuses any actor the hook did not declare, but is not LEZ's executor.
- the cost harness: 3 tests (`lez-bridge/attest/reward/cycles`). Every case runs natively on
  updates the follower recorded on our public chain.

Not deployed, never run on a chain. Its executor cycle count is the first step of
`RUN-AFTER-3AM.txt`, one command.

RFP-021 Soft Requirements #5, word for word: "**Operator incentive.** Propose a mechanism that
rewards whoever advances the consensus state, so that liveness does not rest solely on interested
parties choosing to spend the gas. Any such mechanism must preserve the permissionless property in
Functionality #11: it may make operating attractive, but must not make it exclusive."

## 1. Where it lives: a new program version

The deployed `eth_attest` program cannot pay the right party, and no program beside it can. LEZ
v0.4.0-rc3 gives a program only the `ReceiveInput` of each delivery: receiver, sender, `is_authorized`,
its own actor's state and the message (`lee/state_machine/core/src/program/mod.rs` `ReceiveInput`;
the guest reads nothing else, `read_input_frame`). In detail:
- **Same transaction is impossible.** A public transaction has exactly one root call
  (`lee/state_machine/src/public_transaction/message.rs`, `PublicExecution { root: RootCall, .. }`),
  and `eth_attest::instruct` refuses any instruction that is not that root call
  (`input.from.is_none()`). Another program cannot submit an update for someone.
- **A later transaction cannot learn who submitted.** Nothing in the deployed program records the
  submitter:
  - the store, the anchors and the `Advanced` event carry no account;
  - the submitter's own actor is left untouched (`Response::keep_state()`);
  - programs cannot read events, past transactions or their signers;
  - a declared but untouched actor leaves no trace in state (`validated_state_diff/mod.rs`
    `settle`: only reached actors enter the diff);
  - the fee program keeps no per-transaction payer (`lez/programs/fee/core/src/state.rs` `FeeState`);
  - the clock holds only the block id and time (`lez/programs/clock/core/src/lib.rs`
    `ClockAccountData`).

  So a reward claimed in a second transaction for a plain update of the deployed program is
  claimable by anyone who saw it, which is exactly the redirect to prevent.

**What a new version needs, at minimum:** the update's own transaction must write who is to be
paid into state only the program's code can write. `eth_attest_rewarded`
(`lez-bridge/attest/reward/program`) does this. It links `eth_attest` and `eth_attest_core`
**byte-unchanged**: every message of the deployed program (Borsh tags 0, 1, 2) goes to
`eth_attest::handle`, and one new tag (3) carries the reward messages. At its own header it is a
new program id with new instances. The deployed program, its instances, the bridge guest and every
pinned image are untouched; consumers move as RFP-021 Verification Security #3 describes.

## 2. Two modes, one rule

- **`Paid { payee, inner }`: one transaction.** `inner` runs exactly as `eth_attest` runs it. Then:
  1. the instance's pool reads the store back (the deployed program's own `AssertCurrent { prev: next }`);
  2. it reads its real balance (LEZ `native_token` `ReadState`; the fee program reads its inbox the
     same way, `FeeState.pending`);
  3. it credits the payee.
- **`Recorded { payee, inner }`: two transactions, for when `Paid` does not fit the 10M cap.**
  - The update writes only a receipt into the submitter's own actor: the transition and the payee,
    about 115 bytes and no extra delivery.
  - `Claim { submitter }`, sent later by anyone, settles every receipt that actor holds. Each is
    paid to the payee written in it and then consumed.
  - A receipt can be written only by this program's code, into the submitter's own actor, in the
    transaction whose `Advance` the store accepted.

**The rule.** Each store transition `prev -> next` is paid once:
- **Per slot.** `floor(slots × rate_num / rate_den)` for the beacon slots it advances, counted
  against a **budget per sync-committee period**.
- **The budget.** Each slot counts toward the period it lies in, and a period counts at most
  `max_slots_per_period` slots over every update covering it. Splitting an advance into several
  updates gains nothing; the test default of 8,192 is every slot.
- **Bonus.** `handoff_bonus` is added if the transition made the store learn a new next committee.
  That is a period update, including a catch-up one that rotates and learns the one after.
- **Clamp.** The total is at most the pool's balance minus what it already owes. The rest is
  forfeited; no IOU is kept.
- **Jobs.** A job's payout is split. `floor(paid × begin_bps / 10,000)` goes to the payee its Begin
  recorded, the rest to the finisher. A Begin alone pays nothing: Begin does not check the
  signature, so anyone can open jobs with forged signatures.

**Pull payment.** A payout only credits an earnings account `(instance, payee)`. The payee takes it
with `Withdraw` in its own transaction. Nothing inside an update transfers to a third party, and
every address involved is a PDA anyone can compute and declare. No payee can make an update fail,
including a Begin payee that can never be credited.

| Term (fixed per instance; its hash is the instance's salt) | Test default (OWNER DECISION) |
|---|---|
| `rate_num / rate_den`: native LEZ per slot advanced | 600,000 / 1 |
| `max_slots_per_period`: slots counted per sync-committee period | 8,192 (full rate) |
| `handoff_bonus`: per new next committee learned | 120,000,000 |
| `period_begin_bps` / `finality_begin_bps`: the Begin's share of a job | 2,500 / 6,500 |
| `nonce` | zero |

**Unrewarded advances.** A plain message (tag 0) advances the store and pays nobody, now or later:
each transition pays only itself. One exception keeps a plain finisher from cutting out a rewarded
Begin. `ClaimBeginShare { job_id }`, sent by anyone, credits that Begin's share to its recorded
payee once the job is done; the finisher's share stays in the pool. It is refused unless all of
these hold:
- the job is `Done`, so its own `Advance` was applied;
- the claim record still waits, so no rewarded Finish took it or holds it for a receipt;
- it was not paid before.

A rewarded `FinalityFinish` after a plain `FinalityBegin` is paid from the open job's own
transition, which the Finish notes before it finalizes (`JobNote`).

## 3. Copying, front-running and the second transaction

- **Where the payee is bound.**
  - Public path: `payee` is in the root message, under the submitter's signature.
  - Private path: it is in the proven execution's public output.
  - `Recorded`: it is in the receipt in the submitter's own actor, which only this program's code
    writes. `Claim` can be sent by anyone, but always pays the payee written in the receipt.
  - `Withdraw` pays only the signer's own earnings.
- **What the binding does not stop.** Update material is public Ethereum data. Anyone can submit
  the same update with their own payee; whoever the sequencer orders first is paid, and the other
  is refused (`RFP021-E09`). That is competition, not theft. It does not remove the race RFP-021
  Reliability #8–#10 describe; it pays its winner.
- **The loser pays.** LEZ includes a transaction refused in execution and charges its whole gas
  limit (`public-demo/prove/statements-20261117/README.md`: the replays "reverted", paying the 10M
  limit).
- **Our proposal's wording.**
  - "A copier cannot redirect it" holds per transaction; it does not stop racing.
  - "Pays per time advanced" alone would pay nothing for a period update, hence the bonus.

## 4. Farming

- **Splitting.** An advance split into many updates earns what one update earns: the per-period
  budget counts slots, not updates. Tested at budgets 8,192, 5,000 and 100, split into 2, 3, 7 and
  100 updates. Each extra update also costs about 75.9M units in fees.
- **Replays and zero advances.** The store refuses them (`RFP021-E09`/`E10`). A receipt pays once.
- **Bonus.** The bonus needs a next committee the store did not hold before, so at most one per
  period.
- **Bound.** A pool credits at most `rate × slots elapsed + bonus × periods`, whatever the number
  of updates; per period, at most `rate × max_slots_per_period + bonus`.
- **Griefing.**
  - Forged-signature Begins cost their sender gas and pay nothing.
  - `CloseClaim` cannot erase a record a receipt waits on.

**Known limits:**
- `CloseJob` on a done job, sent before `ClaimBeginShare`, erases the proof the share needs. The
  Begin payee's follower should claim as soon as it sees a plain Finish (tested:
  `closing_a_done_job_before_the_share_is_claimed_loses_the_share`).
- A receipt claimed more than 8 periods behind the newest one the pool has paid counts no slots,
  only its bonus. Followers claim right away.
- Private submitters' receipts can be claimed only by their owner, in a private transaction.

## 5. Edge cases

- **Empty pool.** The update applies, 0 is credited, and the `Paid` event shows `owed` and `paid`.
- **Committee handoffs.** The learning period update earns the bonus plus its slots; a rotation
  earns its slots. The Begin and Finish split follows the measured gas (3.19M against 9.57M).
- **Catch-up over several periods.** Each step pays its own slots under each period's budget. With
  the default budget a 12,704-slot rotation is paid every slot (tested), and the per-period budget
  bounds what a long stall can cost.
- **Isolation.** One pool per instance, keyed by the instance id. Two instances never share funds,
  budgets or earnings (tested).

## 6. What is fixed at creation (OWNER DECISIONS)

The terms hash is the instance's `InstanceConfig.salt`, so the terms are part of the instance id.
`OpenPool` (anyone, once) accepts only terms whose hash is the salt, and checks the instance exists
with the deployed program's own `AssertInstance`. Decisions, each fixed forever for an instance:
1. the rate;
2. the per-period slot budget (and whether it should be below full rate at all);
3. the bonus;
4. the two Begin shares;
5. no withdrawal of pool funds (today: none);
6. no IOU (today: none);
7. plain updates allowed and unpaid (today: yes);
8. the 8-period claim window.

The test defaults are sized from measured fees: 600,000 per slot pays 90M for a 30-minute update,
against a fee of about 75.9M.

## 7. Cost

- **Paid.** On top of the plain update, a paid update adds `Pay`, `AssertCurrent`, the built-in
  `ReadState` and its reply, and one or two earnings credits. Public updates already use
  9.41–9.62M of the 10M cap.
- **Recorded.** Adds only the receipt write to the root actor's own state.
- **Claim.** Carries `Pay` and the credits.
- **Not measured.** The executor counts are not measured yet; `RUN-AFTER-3AM.txt` step 1 measures
  every case on recorded public-chain updates. How LEZ meters its built-in native token is step 2.

## 8. For the daemon (branch `lc-daemon`; interface only)

`lez-bridge/attest/reward/program/src/hook.rs`:
- **Messages:** `paid_message`, `recorded_message`, `claim_message`, `withdraw_message`,
  `claim_begin_share_message` and `open_pool_message`.
- **Actor lists.** Each message has the public actors it reaches: `declared_actors` (pass
  `recorded_payee(<claim account>)` for a Finish), `claim_actors`, `withdraw_actors`,
  `claim_begin_share_actors`, `open_pool_actors` and `close_claim_actors`.
- **Integration:** `PaidSubmitter` (`payee()`, `record()`, `send_root`) plus `submit(...)`. With no
  payee configured, the follower sends the plain message.

The follower should:
- use `Recorded` whenever the measured `Paid` cost at the update's participation would not fit;
- claim its receipts right after;
- send `ClaimBeginShare` when a plain Finish lands on a job its Begin named;
- keep its R9/R10 behaviour unchanged.

The payee is a plain account id and needs no key on the follower.
