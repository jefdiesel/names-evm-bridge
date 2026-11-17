# A sync-committee handoff through the public path, on a local chain (2026-11-17)

**Result.** On a **local** LEZ v0.4.0-rc3 chain, with **real Sepolia data across the period
1415 → 1416 boundary**, the RFP-021 program took a full committee handoff in **ordinary public
transactions**: the period-1415 update that teaches the store the next committee (two
transactions, Begin and Finish), the first finality update signed by that **next** committee
(the rotation into 1416, one transaction), and one more in-period finality update. A consumer
then proved a Sepolia storage slot under the newest anchor (the reference probe, public). Every
transaction was executed by the sequencer itself; **no proof was made anywhere**. Each one is
under LEZ's 10M gas cap, and the gas the sequencer charged equals the executor's count to the
cycle.

State label (`DESIGN.md`): **(1) run on a chain: a local LEZ sequencer** on the build host,
loopback only (127.0.0.1:3462). Not the public demo chain, not a Logos network. Branch
`public-update`.

## Transactions

Gas as metered: each fee is exec gas × exec base fee + wire bytes × storage base fee (base fees
8 / 8 for all four updates); the wire length is read back from the sequencer (`getTransaction`,
`txlen.log`), the balances and fee state just before and after are in each `send-*.log`. For all
four, the fee-derived gas equals the executor's whole-transaction sum exactly (`measure-*.log`).

| Step | LEZ block | tx | Sepolia | Signers | Gas (= executor) | Of 10M |
|---|---:|---|---|---:|---:|---:|
| Deploy the program (immutable) | 12–13 | — | — | — | — | — |
| `Init` at the finalized checkpoint, slot **11599840 (period 1415)**, root `0xb0bb4400…de89` | 14 | `c0f28eae3f60386e7ecc5c0584db4172eb3a61f6a08e386b4848d81eeaf2936f` | — | — | 9,116,553 | 91.2% |
| **Period update 1415, Begin** (every check but the signature; opens the job) | **16** | `fe1344780ef686a36ac52965a6d6d74cf63f2e82600643fc3908950d23e8cbfe` | signature slot 11591745, attested 11591744, finalized 11591680 | 509/512 | **3,160,342** | 31.6% |
| **Period update 1415, Finish** (the signature; the store learns the next committee) | **17** | `03f73c8179f71d6b597d3b94f4852f04cd9194ce4a5692217f2743d94abf4451` | (same update) | 509/512 | **9,052,989** | 90.5% |
| **Rotation**: finality update signed by the NEXT committee, into period 1416 | **19** | `e60be2d9eab9e9612efa4771e5c947a38e1e10eceee909ae8015d4f03b18e87c` | signature slot 11600353 (period 1416), finalized **11600288** | 506/512 | **9,348,407** | 93.5% |
| `CloseJob` of the finished job (by anyone; the job PDA is emptied) | 20 | `8f14dfcade2de546854b106a62c30dff303c37f5c6ac069eeca1ae6897396b89` | — | — | not measured | — |
| **In-period finality update** (period 1416) | **29** | `58c03ce114382d52995bf167430a5b65c8f7c8a5d7d1d3be73aeb461b4d77e5b` | signature slot 11600385, finalized **11600320** | 506/512 | **9,289,642** | 92.9% |
| Consumer: the live v2 vault's `isDepositRoot[0x43d1e353…aac5]` = 1 under the newest anchor (reference probe) | 30 | `aabd49ecb964e72f61ede8372558e1a336e22ef56eba3674198ef53534b83dfe` | execution block 12155956, anchor `0x21eb76e7…15a5` | — | 273,363 | 2.7% |

Store after each step (`show-*.log`):
- after Finish (block 17): `updates 1`, finalized slot 11599840 (unchanged: the period update's
  finalized header, 11591680, is older), current committee `0xa1cc4756…0ced`, **next committee
  `0x8fe228b9…8891` learned**;
- after the rotation (block 19): `updates 2`, finalized slot 11600288 (period 1416), **current
  committee `0x8fe228b9…8891`**, next unknown, anchor `0xf75aed1e…4d2b` recorded;
- after block 29: `updates 3`, finalized slot 11600320, anchor `0x21eb76e7…15a5` recorded.

Cross-check: the committee learned here, `0x8fe228b9…8891`, is the period-1416 committee that the
finality-only run of this branch (`public-demo/prove/public-update-20261117/`) read from its
bootstrap in period 1416.

## Program, image, commit

- Program `DKjavTXCFHwi14YTkwAVqVHy2TzWBytNT7ALi41PEVhg`; instance
  `0xd681456fe74b497b6dbde580bc6b0dd29f64c33982452b3a3adab1c0c08a480c`; probe
  `2SQEqfPjt6cWng1Z9v1vXDge66GEUctjgDg9JUks5RkH`.
- Guest `lez-bridge/methods/attest_guest_public`, image id
  `160d7581a3d20b39725fe5f310826a54199baffea74cdb35b865c26a5e63cfa5`, ELF sha256
  `fa94373856754980563f1e43aba7cdadc86ba44f089f6fc14bf3a924c281b84e` (`elf.sha256`,
  `cycles-fixtures.txt`), built at commit `a058052` (the program sources of the commit that adds
  this record are the same) with `lez-bridge/attest/cycles/build-public.sh`, at the build host's
  `$W/src/lez-bridge` (guest builds embed their directory). The finality-only run's image
  `36873168…cc7d` stays reproducible at tag `public-update-run1`.
- LEZ `v0.4.0-rc3` (`bafd07be`); sequencer and CLI as in the finality-only run's README.

## Re-run after the review fixes (chain 127.0.0.1:3464, `rerun-h2/`)

The review's fixes change the image, so the handoff ran again on a fresh local chain with the
same real data: the frozen period-1415 bootstrap, the period-1415 update (served again), and a
newer finality update for the rotation. Program `DVM9PDn8VGnGAyw24jbesZ62GGcMJ1ppYBeFmmZSjwzi`,
instance `0x4e66926d34a1d27ad37b07a077f662a8eaea614f49b4bf09cfb04ff30139ba1d`, guest image
**`bf0dc63811a4e79e9bc9a0c4d30cdfe67a19fd6cbaaec257e78ad07d10f3354c`**, ELF sha256
`da281aa537d02968bb0a90a5614a9c460be16e14e4906287845c5c8d7e175abd` (built at commit `6ab95c1`).

| Step | LEZ block | tx | Gas (= executor) |
|---|---:|---|---:|
| `Init`, slot 11599840 (period 1415) | 14 | `0451aff161b3c070735102cab385803ba792f1e6c13145bccd3c00821839233f` | 9,116,860 |
| Period-1415 update, Begin (now also asserts the live store is `prev`) | 16 | `bfe3afe081a9f2f71b909357e626611c1dbccc73dfba6dd047978feb8e6de97b` | 3,193,398 |
| `CloseJob` of the LIVE job, forced through with `--allow-revert` | 18 | `8047cd33cd7d9b1031354b9450b72cd00a6515c5e430e513be2cfc68ed0ed71b` | **reverted**; the job PDA still holds its job afterwards (`close-live-job.log`) |
| Period-1415 update, Finish (store check first, then the signature in the job PDA) | 19 | `f05a9262f8b7a15b6fc62274ef780e25bb98c204322d6533b7657059cf238256` | 9,318,848 |
| Rotation: finality update signed by the next committee (506/512, finalized 11600512, period 1416) | 20 | `25ad1e58767693d921f45a609ad377abf620bcab6032c2a73af1bd073b58c0c0` | 9,347,926 |
| `CloseJob` of the finished job | 21 | `2902d31fc9fb37d8349ae02ebdd8ce60e36de65e0946be223a7fa63489a2d04a` | not measured |

The statements and the reference consumers then ran on this chain
(`public-demo/prove/statements-20261117/`). The first run above (image `160d7581…`) stays
reproducible at tag `public-handoff-run1`.

## Review, 2026-11-17

- Reviewed once **from source** by a separate AI agent; the reviewer built and ran nothing; **no
  soundness bug found; not audited.** Its fixes are in (above): Begin and Finish assert the live
  store first, the job PDA checks the signature, `JobClose` checks its address, an empty store
  counts as stale, and the missing tests were added (`public_update.rs`, 14 tests).
- **Two assumptions the public path adds over the private one:** (1) it trusts the committee's
  `aggregate_pubkey` field, where Helios sums the participating keys itself; (2) signer keys are
  never decompressed or subgroup-checked (only the aggregate and the non-signers are, bound byte
  for byte to the committee). Both are safe only for committees in beacon states Ethereum itself
  produces: the consensus specs validate every sync-committee key when the aggregate is formed
  (`eth_aggregate_pubkeys` asserts `KeyValidate`, `specs/altair/bls.md`), and the committee is
  bound by the next-committee branch to a header the previous committee signed.
- **Does LEZ revert the whole message chain when a nested effect panics?** Yes. From the source
  (LEZ `v0.4.0-rc3`): `handle_message(...)?` in `lee/state_machine/core/src/execution_state.rs`
  (around line 1320) returns the first error out of the whole delivery, and state is only staged
  until the transaction completes; on the chain: block 18 above, where `JobClose` emptied the job
  and its nested `AssertStale` panicked, the transaction was included **reverted** and the job PDA
  still held its job.
- **Does the timestamp window bind the whole transaction?** Yes: every handled message's window
  is intersected into the transaction's (`execution_state.rs` line 1329, `self.validity =
  self.validity.intersect(transition.response.validity)`, an empty intersection is an error), and
  the result is checked once against the block (`lee/state_machine/src/validated_state_diff/mod.rs`
  lines 364 and 585, `execution.validity.is_valid_at(block_id, timestamp)`). So Finish's window
  (the signature slot's start) holds for the store write it causes. Native test:
  `finish_carries_the_signature_slot_time_window`.

## What was built (`lez-bridge/attest/program/src/public.rs`)

- **Period update in two public transactions.** `PeriodBegin` runs every Helios check except
  the signature (Helios's own `verify_generic_update_with`, the signature check deferred through
  its hook), applies the update to stand-in committees and writes a **job PDA** keyed on
  SHA-256(tag ‖ `prev` ‖ SHA-256(whole update SSZ)), holding `prev`, `next`, the signing root, the
  signature, the participation bits and the commitment of the committee Helios selected.
  `PeriodFinish` (anyone) first has the store assert it still equals `prev`, then the job PDA
  checks that signature with the committee's SSZ and key witnesses (since the review; in the first
  run the submitter's actor checked it), marks itself done and sends the store the same `Advance { prev, next }` as every
  other path; the store applies it only if it still equals `prev`. A second `Begin` of the same
  job, a second `Finish`, a `Finish` whose job differs from the stored one and a `Finish` after
  the store moved are refused. `CloseJob` (anyone) empties a job only once the store has moved
  past its `prev` (the store asserts it).
- **The rotation.** `UpdatePublic` now takes the SSZ of whichever committee Helios selects by the
  signature period (the next one once the store knows it), checked against that commitment.
- **Helios patch 2** (`lez-bridge/attest/vendor/helios-consensus-core/SPIKE-PATCH.diff`): the next
  committee's SSZ root by direct SHA-256 calls (`sync_committee_root`) instead of `tree_hash`;
  tested equal to `tree_hash_root` on two real committees. This is what makes Begin 3.16M instead
  of about 9-10M. Every other Helios function is unchanged.
- **Witness flag bits** (review, low item): an uncompressed key witness whose first byte has any
  flag bit set is refused (`if unc[0] & 0xe0 != 0 { return None }`).

## Cycles on the recorded fixtures (executor, `attest_cycles public`, pinned)

| Transaction | User cycles | Of 10M |
|---|---:|---:|
| Finality update, Sepolia 509/512 | 9,256,538 | 92.6% |
| Finality update, mainnet 507/512 | 9,537,210 | 95.4% |
| Period update 1410, Begin | 3,161,339 | 31.6% |
| Period update 1410, Finish | 9,053,299 | 90.5% |
| Rotation into 1411 (finality update signed by the next committee) | 9,315,279 | 93.2% |
| Period update 1411, Begin (second handoff) | 3,160,342 | 31.6% |
| Period update 1411, Finish | 9,053,479 | 90.5% |

`lez-bridge/attest/cycles/tests/public_cycles.rs` pins all seven within 1% and under the cap
(build host, `build-public.sh test`; not in CI, the guest needs RISC Zero's C toolchain).

## Tests (native, `lez-bridge/attest/program/tests/public_update.rs`, in CI with the feature)

8 tests: the public finality path equals the private one; mainnet; the **full handoff, public,
equal to the private path step by step** (period update 1410 Begin and Finish, the rotation into
1411, then the period-1411 update: a second handoff); the direct committee root equals
`tree_hash_root`; tampered next-committee branch and next-committee key, wrong period, a forged
signature (Begin opens, Finish refuses), replayed Begin, wrong committee at Finish, altered job,
Finish after the store moved, a live job not closable, stale and forged jobs closed by a stranger,
a finished job not finishable again; rotation with the current committee as witness refused;
witness cases: missing, extra, reordered, another member's key, wrong aggregate, flag bits set,
off-curve, infinity, index 600; infinity signature, signature not on G2, participation below
policy, a committee witness not the store's.

## Commands

```sh
lez-bridge/attest/cycles/build-public.sh                 # build + the fixtures (incl. the handoff)
lez-bridge/attest/cycles/build-public.sh workspace       # native tests (feature on) + panic texts
lez-bridge/attest/cycles/build-public.sh test            # the pinned executor counts
R=$W/chain-h SEQ_BIN=$W/bin/sequencer_service.rc3 CLI=$W/bin/lez-bridge.rc3 NETCFG=$W/bin/local.json \
  RPC=127.0.0.1:3462 METRICS=127.0.0.1:3463 public-demo/rc3/chain-rc3.sh (not published) setup   # then: start
export ELF=$W/elf-h/attest_guest_public.bin CHECKPOINT=0xb0bb44007299221f4e545390c0ea3f46363b70e9680b2d303953e3486378de89 \
  BOOTSTRAP=public-demo/prove/public-handoff-20261117/bootstrap_11599840.ssz   # fetched 12:22, sha256 93534310…1140
public-demo/prove/public-handoff.sh deploy && public-demo/prove/public-handoff.sh init
U=p1415 KIND=period public-demo/prove/public-handoff.sh fetch      # the period-1415 update (CLI, verified natively, nothing sent)
U=p1415 public-demo/prove/public-handoff.sh measure                # executor: both steps; .begin/.finish messages, the job PDA
U=p1415 public-demo/prove/public-handoff.sh send-begin
U=p1415 public-demo/prove/public-handoff.sh send-finish
U=rot   KIND=finality public-demo/prove/public-handoff.sh fetch && U=rot public-demo/prove/public-handoff.sh measure
U=rot   public-demo/prove/public-handoff.sh send                   # the rotation
U=p1415 public-demo/prove/public-handoff.sh close-job
public-demo/prove/public-handoff.sh probe-deploy
U=fin2 KIND=finality public-demo/prove/public-handoff.sh fetch && U=fin2 public-demo/prove/public-handoff.sh probe-fetch
U=fin2 public-demo/prove/public-handoff.sh measure && U=fin2 public-demo/prove/public-handoff.sh send
U=fin2 public-demo/prove/public-handoff.sh probe && public-demo/prove/public-handoff.sh txlen
```

`$W` is the build host's work directory; in the copied logs its absolute path is written as `$W/`
and the build's `ls` listing of the guest files is dropped from `cycles-fixtures.txt` (nothing else edited).

## Not done, and limits

- **One boundary on a chain, not two.** A second handoff (1414 → 1415 → 1416) needs a bootstrap
  in period 1414; the beacon node serves bootstraps only for recent checkpoints (period-1414 and
  early-1415 roots answered HTTP 500 on 2026-11-17 12:21), so it ran only on the fixtures (1410 →
  1411 → 1412, natively and in the executor).
- **Finish at low participation:** the signature step is 9.05M at 509/512 and each non-signer adds
  about 11K (the finality sweep in `public-demo/prove/public-update-20261117/`), so Finish should
  stop fitting near 423/512 (estimate from those measurements, not run). The rotation is a finality
  update and fits down to about 441/512 like the finality path. Below that a step must split again
  at the pairing seam (final exponentiation alone, about 3.6M) or go private. Not built.
- **`CloseJob`'s cycles were not measured**, and Begin's job write is included in Begin's count
  and the job PDA's state is about 800 bytes per open job, paid by its opener until someone closes it.
- **Liveness, as the review noted:** a public update can make a private proof built on an older
  store state stale (and vice versa); one updater at a time.
- The CLI has no command for these instructions; messages come from `attest_cycles public-real`
  and go out through the hidden `raw-call` of a later rc3 CLI build (as in the finality-only run).
- Not run on the public demo chain or any Logos network; the deployed attestation program there is
  unchanged.
