<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# RFP-021 cost measurement (Performance #1–#9)

Branch `lc-incentive`, 2026-11-19. Every table between `BEGIN GENERATED` and `END GENERATED` is written by
`scripts/lc_cost_model.py` (standard library only) from raw numbers. Nothing in those tables is typed by hand:
- `docs/lc-costs-data/raw/`: copies of the two followers' logs from our public demo chain. Each copy is
  read-only and its last line gives the cut-off time.
- `docs/lc-costs-data/inputs.json`: every other number, each with its label and source file.

To regenerate, run `python3 -I scripts/lc_cost_model.py --write`. `--check` and
`scripts/test_lc_cost_model.py` fail when the document is stale.

**Labels.**
- **public-chain:** measured on our public demo chain (LEZ v0.4.0-rc3, the team's own sequencer,
  `https://lez.nameswallet.app/`; not a Logos network), by the two followers below.
- **local:** measured on a local LEZ rc3 chain or in the RISC Zero executor.
- **synthetic:** local, on a synthetic sync committee (real Sepolia headers).
- **constant:** a LEZ protocol constant, read from its source.
- **modelled:** computed here from the others.

LEZ testnet 0.4 is not live; nothing here ran on it.

**The two followers (public-chain).**
- `lc-public` runs the finality-only program `GGo5C4hf…xtT`. Its loop ran from 2026-11-17 17:52 to
  2026-11-18 20:07 UTC and stopped at the period-1416 boundary, as designed.
- `lc-handoff` runs the handoff-capable program `9Wb7sBxs…uhNR`, instance `0xe4b7d26a…3600`, from
  2026-11-18 00:36 UTC and still running at the snapshot.

<!-- BEGIN GENERATED snapshot -->
| Snapshot (read-only copy) | lines | first | last (cut-off) | sha256 |
|---|---:|---:|---:|---:|
| `docs/lc-costs-data/raw/lc-public-loop.log` | 55 | 2026-11-17T17:52:46Z | 2026-11-18T20:07:49Z | 349fb6548061098f… |
| `docs/lc-costs-data/raw/lc-handoff-loop.log` | 67 | 2026-11-18T00:36:31Z | 2026-11-19T02:20:25Z | c0138c34fa2e3e32… |
<!-- END GENERATED snapshot -->

Both are evidence rows on `main` (5322a30): `docs/EVIDENCE.md`, check ids `lc-public` and `lc-handoff` in
`docs/evidence-checks.json`. Gas is the executor's user-cycle count, which the sequencer meters exactly
(`public-demo/prove/public-update-20261117/README.md`). The fee is the payer's balance drop. Both are
logged per transaction.

## What was measured, in one table (P2)

RFP-021 Performance #2, word for word: "Cost measurement is a primary output. Measure and document,
with a breakdown by program action: advancing the tracked Ethereum state, and statement verification by
type of statement. Report cycle count, gas cost, proof time, and proof size for each, extending the
measurement methodology from RFP-020. Numbers must be reproducible from the test suite."

**Public chain, every update the followers sent** (gas = cycles; tx bytes derived from the fee):

<!-- BEGIN GENERATED public-stats -->
| Action (public chain, measured) | n | gas min | gas median | gas p95 | gas max | fee median | fee max | tx bytes median (from the fee) | tx bytes max (from the fee) | signers |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| lc-public, finality | 51 | 9,285,203 | 9,426,771 | 9,448,407 | 9,590,709 | 75,646,544 | 76,969,808 | 29,047 (n = 51) | 30,517 | 478–506 |
| lc-handoff, finality (current committee) | 55 | 9,412,245 | 9,455,845 | 9,466,170 | 9,618,239 | 75,880,176 | 85,336,471 | 29,177.0 (n = 54) | 30,647 | 477–496 |
| lc-handoff, rotation (first update by the next committee) | 1 | raw values: 9,480,371 gas, fee 76,074,816 (494/512) | — | — | — | — | — | 28,981 | — | 494–494 |
| lc-handoff, period Begin | 2 | raw values: 3,193,829 gas, fee 25,770,096 (506/512); 3,194,827 gas, fee 25,778,080 (494/512) | — | — | — | — | — | 27,433, 27,433 | — | 494–506 |
| lc-handoff, period Finish | 2 | raw values: 9,416,729 gas, fee 75,547,168 (506/512); 9,572,075 gas, fee 76,799,344 (494/512) | — | — | — | — | — | 26,667, 27,843 | — | 494–506 |

Gas and fee are logged per transaction. Tx bytes are not read from the chain: they are fee ÷ 8 − gas, so a row at another base fee (block 27,985, base 9) has none. Groups under 20 transactions show their raw values, no percentiles.
<!-- END GENERATED public-stats -->

**Every other action** (local, synthetic, recorded private runs):

<!-- BEGIN GENERATED actions -->
| Action | cycles = gas | wire bytes | proof time (s) | label | source |
|---|---:|---:|---:|---:|---:|
| Init (instance at a real Sepolia checkpoint), public | 9,105,915 | — | none (public) | local | DESIGN.md §9.0 (rc1, block 1222) |
| Init on the rc3 handoff image (checkpoint slot 11599840), public | 9,116,553 | 26,286 | none (public) | local | public-demo/prove/public-handoff-20261117/init.log (fee 73,142,712 at base fees 8/8, 26,286 wire bytes) |
| Finality update, one public transaction, executor at 506/512 | 9,252,369 | — | none (public) | local | DESIGN.md §9.3b (attest_cycles public, Sepolia fixture) |
| Period update Begin, public (period 1415, 509/512; re-run after review) | 3,193,398 | — | none (public) | local | public-demo/prove/public-handoff-20261117/README.md, section 'Re-run after the review fixes' |
| Period update Finish, public (period 1415, 509/512; re-run after review) | 9,318,848 | — | none (public) | local | public-demo/prove/public-handoff-20261117/README.md, section 'Re-run after the review fixes' |
| Rotation (first update by the next committee), public, 506/512 (re-run) | 9,347,926 | — | none (public) | local | public-demo/prove/public-handoff-20261117/README.md, section 'Re-run after the review fixes' |
| Low participation 427/512: FinalityBegin | 6,549,558 | — | none (public) | synthetic | public-demo/prove/finality-split-20261117/README.md |
| Low participation 427/512: FinalityFinish | 3,780,392 | — | none (public) | synthetic | public-demo/prove/finality-split-20261117/README.md |
| Low participation 342/512: FinalityBegin | 7,485,921 | — | none (public) | synthetic | public-demo/prove/finality-split-20261117/README.md |
| Low participation 342/512: FinalityFinish | 3,780,392 | — | none (public) | synthetic | public-demo/prove/finality-split-20261117/README.md |
| Statement: receipt log (ERC-20 Transfer), public | 174,001 | — | none (public) | local | public-demo/prove/statements-20261117/README.md |
| Statement: account field (balance), public | 216,605 | — | none (public) | local | public-demo/prove/statements-20261117/README.md |
| Statement: storage slot (probe), public | 263,347 | — | none (public) | local | DESIGN.md §9.0 (rc1, block 1223) |
| Reference consumer: ERC-20 claim, public | 216,310 | 6,379 | none (public) | local | public-demo/prove/statements-20261117/README.md, txlen.log |
| Reference consumer: ETH claim (WETH9 Deposit), public | 158,267 | 3,321 | none (public) | local | public-demo/prove/statements-20261117/README.md, txlen.log |
| AssertAnchor (the consumer's public trace), executor | 15,997 | — | none (public) | local | DESIGN.md §9.3 |
| Finality update, PRIVATE path (unchanged Helios, 37.2M cycles), proved | — (unmetered: proved) | — | 7,283 | local | DESIGN.md §9.0a (rc3 Run 2, Apple M4 (16 GB), 4 threads, CPU prover) |
| Finality update, PRIVATE path, proved (rc1) | — (unmetered: proved) | — | 7,075 | local | DESIGN.md §9.0 (rc1) |
| Attested claim in a PRIVATE transaction (storage-slot statement + mint), proved | — (unmetered: proved) | — | 676 | local | DESIGN.md §9.0a (rc3 Run 2) |
<!-- END GENERATED actions -->

**How the fee is made** (checked against every logged fee):

<!-- BEGIN GENERATED fee-model -->
- The model reproduces **111 of 111** logged fees exactly (`fee = base_exec × gas + base_stor × wire bytes`; base fees from `market.rs`; wire bytes = the image's intercept + 98 per missing signer, none for a period Begin, which carries no key witnesses).
- Wire-byte intercept (512 signers), lc-handoff finality: 27,217 bytes.
- Wire-byte intercept (512 signers), lc-handoff period-begin: 27,433 bytes.
- Wire-byte intercept (512 signers), lc-handoff period-finish: 26,079 bytes.
- Wire-byte intercept (512 signers), lc-public finality: 27,185 bytes.
- This is a CONSISTENCY check, not a measurement of transaction size: the 4 intercepts and the 98-byte slope are taken from these same rows, so the match shows the fee formula, the base-fee rule and bytes linear in missing signers hold together. The one independent byte count is the sequencer's own read-back of 27,773 bytes for a 506/512 update on a local chain (`public-demo/prove/public-update-20261117/txlen.log`), which equals the lc-public intercept + 6 × 98.
- Block 27,985 paid an execution base fee of 9, not 8: block 27,984 before it carried period-finish (9,572,075 gas), above the 5M target (logged 85,336,471, modelled 85,336,471).
<!-- END GENERATED fee-model -->

- **Proof size.** The public path makes no proof: the sequencer executes the update, and the "size" is
  the transaction (tx bytes above, 27–31 KB). The private path's proof is the constant below, not
  measured from our own receipts.
- **Reproducible from the test suite?** Partly. The executor counts are pinned by
  `lez-bridge/attest/cycles/tests/public_cycles.rs` (build host). The public-chain numbers are
  measurements, not test outputs. The test suite reproduces every table here from the logged raw
  numbers (`scripts/test_lc_cost_model.py`), not the numbers themselves.
- **Missing:** `CloseJob` gas, and a private transaction's real size from one of our blocks
  (`RUN-AFTER-3AM.txt` steps 2 and 6).

## P1: one transaction

RFP: "Verifying an attestation and evaluating its predicate must complete within a single LEZ
transaction at the per-transaction cycle budget in force at the time the work is completed, in the
direct consumption path of Functionality #7. If this proves infeasible at the time the work is
completed, the measurement in Performance #2 stands as the output and the shortfall must be documented
with the specific component responsible."

- **Answered (local).** Each statement kind and both reference consumers verified and evaluated their
  predicate inside one public transaction. That took 158,267–263,347 gas, 1.6–2.6% of the 10M cap
  (table above; `public-demo/prove/statements-20261117/README.md`). Inside a private transaction, the
  bridge's attested claim (storage-slot statement plus mint) was proved in 676.0 s (§ "Private path").
- **The update is not P1's subject.** It is in P2. A public FINALITY update fits one transaction
  above about 442 signers; the rotation (about 447) and the period Finish (about 461) need more, and
  below the Finish's limit a period update has no public path today (§ P3, participation).
- **Which statements.** Storage-slot and account statements work under any later anchor. A
  receipt-LOG statement works only for a log in an anchor's own block (§ P3); both reference
  consumers are log claims.

## P3: amortised tracking cost

RFP: "Document the amortised per-attestation cost of sync-committee tracking at the recommended
operating cadence, separately from the per-attestation verification cost, since the two amortise
differently across consumers."

**Recommended cadence: a finality update every 30 minutes** (what both followers run), plus the two
period steps once per period. A consumer of STATE statements that can wait longer saves in
proportion; for receipt-log statements a slower cadence means fewer provable logs (below). The floor
that keeps an instance alive is one period update per period, well inside the weak-subjectivity
bound of 1,348 epochs, about 6 days, on Sepolia, and only while the period Finish fits (about 461
signers or more).

**Cost against cadence** (modelled from the public-chain medians):

<!-- BEGIN GENERATED cadence -->
Per update, handoff image on our public chain: finality 9,455,845 gas, fee 75,880,176 (median of 54 at base fee 8); each period adds a Begin (3,194,328 gas, fee 25,774,088) and a Finish (9,494,402 gas, fee 76,173,256), each the MEAN of the 2 period updates observed (506 and 494 signers). The 'once per period' row holds only while the period Finish fits one transaction (§ P3, participation). Fees are in native LEZ units; the testnet token has no price, so no currency figure is given. Fees scale linearly with the base fee; ×2 and ×10 are assumptions, not measurements.

| Cadence | updates/day | gas/day | fee/day at base 8 | fee/month (30 d) at base 8 | fee/month at base 16 (×2, assumed) | fee/month at base 80 (×10, assumed) |
|---|---:|---:|---:|---:|---:|---:|
| every epoch (6.4 min, the fastest finality moves) | 225 | 2,138,717,329 | 17,162,641,758 | 514,879,252,734 | 1,029,758,505,469 | 5,148,792,527,344 |
| 15 min | 96.00 | 918,913,324 | 7,374,099,054 | 221,222,971,614 | 442,445,943,229 | 2,212,229,716,144 |
| 30 min (our followers) | 48.00 | 465,032,764 | 3,731,850,606 | 111,955,518,174 | 223,911,036,349 | 1,119,555,181,744 |
| 1 h | 24.00 | 238,092,484 | 1,910,726,382 | 57,321,791,454 | 114,643,582,909 | 573,217,914,544 |
| 6 h | 4.00 | 48,975,584 | 393,122,862 | 11,793,685,854 | 23,587,371,709 | 117,936,858,544 |
| once per period (27.3 h, liveness floor) | 0.88 | 19,463,005 | 156,293,719 | 4,688,811,562 | 9,377,623,125 | 46,888,115,625 |
<!-- END GENERATED cadence -->

**Per attestation** (modelled):

<!-- BEGIN GENERATED amortised -->
**These figures hold for STATE statements** (a storage slot, an account field: state persists into every later anchor). A RECEIPT-LOG statement proves a log only under the anchor of the log's OWN block: an anchor is the execution block of a finalized header an update recorded, and the consumer gives one header that must hash to it (`lez-bridge/attest/core/src/inclusion.rs` `AnchorWitness::roots`); there is no header ancestry. So a log is provable only if its block happens to be an anchor block; every other log is never provable today. Both reference consumers are log claims. Missing: block ancestry (a header chain from a recorded anchor back to the log's block, or beacon `block_roots` / `historical_summaries` proofs).

| Cadence | anchor blocks/day | share of Sepolia blocks a log can be in (at most) |
|---|---:|---:|
| every epoch (6.4 min, the fastest finality moves) | 225 | 0.03 |
| 15 min | 96.00 | 0.01 |
| 30 min (our followers) | 48.00 | 0.01 |
| 1 h | 24.00 | 0.00 |
| 6 h | 4.00 | 0.00 |
| once per period (27.3 h, liveness floor) | 0.88 | 0.00 |

Tracking cost per attestation = the day's tracking cost ÷ attestations that day. The attestation's own verification is separate and does not amortise: 158,267–263,347 gas per statement or reference-consumer claim (local chain, `inputs.json`).

| Attestations/day | tracking gas each (30 min) | tracking fee each (30 min, base 8) | tracking gas each (6 h) | tracking fee each (6 h, base 8) |
|---|---:|---:|---:|---:|
| 1 | 465,032,764 | 3,731,850,606 | 48,975,584 | 393,122,862 |
| 10 | 46,503,276 | 373,185,061 | 4,897,558 | 39,312,286 |
| 100 | 4,650,328 | 37,318,506 | 489,756 | 3,931,229 |
| 1,000 | 465,033 | 3,731,851 | 48,976 | 393,123 |
| 10,000 | 46,503 | 373,185 | 4,898 | 39,312 |
<!-- END GENERATED amortised -->

**Cost against signer participation** (public-chain fit; synthetic executor sweep below 477 signers):

<!-- BEGIN GENERATED participation -->
- lc-public (n = 51, 478–506 signers): gas ≈ 9,219,917 + 10,878 × missing signers (largest residual 14,207); one transaction fits while missing signers ≤ 71 (≥ 441 signers) — an extrapolation below 478 signers.
- lc-handoff (n = 55, 477–496 signers): gas ≈ 9,237,906 + 10,868 × missing signers (largest residual 14,493); one transaction fits while missing signers ≤ 70 (≥ 442 signers) — an extrapolation below 477 signers.
- Rotation (first update by the next committee) (lc-handoff, n = 1): one transaction fits down to about 447 signers (from 9,480,371 at 494/512 with the finality slope 10,868; too few points for a real fit, an extrapolation).
- Period Finish (lc-handoff, n = 2): one transaction fits down to about 461 signers (from 9,572,075 at 494/512 with the finality slope 10,868; 461 with its own 2-point slope 12,946; too few points for a real fit, an extrapolation).
- So a committee handoff needs MORE signers than a finality update: the period Finish (a whole signature check plus the committee) stops fitting first, and its split at the pairing seam is not built (`public-demo/prove/finality-split-20261117/README.md`). Below that limit a period update has no public path today, only the private proof; the once-per-period liveness floor of § P3 holds only above it.

| Signers | Finality, one tx, handoff image (model) | Label | Finality, one tx, executor (synthetic) | FinalityBegin + FinalityFinish, executor (synthetic) | Finality path at the 10M cap |
|---|---:|---:|---:|---:|---:|
| 512 | 9,237,906 | fit, extrapolated | 9,224,671 | 9,395,452 | one transaction |
| 506 | 9,303,114 | fit, extrapolated | — | — | one transaction |
| 492 | 9,455,265 | public-chain fit | — | — | one transaction |
| 477 | 9,618,285 | public-chain fit | — | — | one transaction |
| 460 | 9,803,041 | fit, extrapolated | — | — | one transaction |
| 441 | 10,009,532 | fit, extrapolated | 10,000,427 | 10,171,595 | two steps (Begin + Finish) |
| 427 | 10,161,684 | fit, extrapolated | 10,158,667 | 10,329,950 | two steps (Begin + Finish) |
| 400 | 10,455,119 | fit, extrapolated | 10,452,659 | 10,623,491 | two steps (Begin + Finish) |
| 342 | 11,085,462 | fit, extrapolated | 11,095,486 | 11,266,313 | two steps (Begin + Finish) |
<!-- END GENERATED participation -->

**The one public committee handoff** (public-chain, n = 1, raw values):

<!-- BEGIN GENERATED handoff -->
| Public-chain handoff, period 1416 → 1417 (n = 1) | LEZ block | gas | fee | signers |
|---|---:|---:|---:|---:|
| rotation | 27,862 | 9,480,371 | 76,074,816 | 494 |
| period-begin | 27,983 | 3,194,827 | 25,778,080 | 494 |
| period-finish | 27,984 | 9,572,075 | 76,799,344 | 494 |
| finality after it (base fee 9) | 27,985 | 9,455,895 | 85,336,471 | 492 |
<!-- END GENERATED handoff -->

## P4: proving on the user's device

RFP: "Any proving the user's own device must perform has to be practical on the desktop hardware
Basecamp runs on. Measure and document wall-clock time and peak memory on a mid-range laptop and on the
lowest specification the team declares as supported, and state that minimum explicitly. This includes
running the attestation in a private transaction."

<!-- BEGIN GENERATED private -->
| Recorded private run (Apple M4 (16 GB), CPU prover, 4 threads) | seconds | peak MB | label | source |
|---|---:|---:|---:|---:|
| Finality update, PRIVATE path (unchanged Helios, 37.2M cycles), proved | 7,283 | 9,457 | local | DESIGN.md §9.0a (rc3 Run 2, Apple M4 (16 GB), 4 threads, CPU prover) |
| Finality update, PRIVATE path, proved (rc1) | 7,075 | 9,483 | local | DESIGN.md §9.0 (rc1) |
| Attested claim in a PRIVATE transaction (storage-slot statement + mint), proved | 676 | 9,391 | local | DESIGN.md §9.0a (rc3 Run 2) |

- Proof size on the private path: every private LEZ transaction carries the fixed succinct receipt, 223,551 bytes, padded payload 512 bytes, verified for 409,764 gas (LEZ constants, `market.rs`; not measured by us).
- Modelled, not run: a private update on the public path's cheaper signature check (9.29M cycles instead of 37.23M) would take about 1,816 s (proportional to cycles, no fixed cost) to 2,270 s (a line through the two recorded runs, 676 s at 0.40M and 7,283 s at 37.23M: a fixed cost of about 604 s plus 179.4 s per million cycles). Two single runs of different programs, no variance measured: an estimate, not a measurement.
<!-- END GENERATED private -->

- **Measured (local):** one machine, an Apple M4 (16 GB) desktop, with these recorded runs.
- **Missing:**
  - a mid-range laptop;
  - the declared minimum specification, which we have not declared yet;
  - a private attestation on the cheaper signature path.

  These need proving runs that the owner starts (`RUN-AFTER-3AM.txt`, part 2). We propose 16 GB of
  RAM as the minimum, since every run peaked at 9.4–9.5 GB. That is a proposal, not a measurement.

## P5: latency

RFP: "Document end-to-end attestation latency, from the Ethereum event to a verified statement usable on
LEZ, broken down by finality wait, update availability, and on-chain verification."

<!-- BEGIN GENERATED latency -->
For a STATE statement (storage slot, account field). A receipt-log statement is provable only when the log's block is itself a recorded anchor block (§ P3); for any other log there is no latency, it is not provable.

| Part (seconds; lc-public log) | n | min | median | p95 | max | Set by |
|---|---:|---:|---:|---:|---:|---:|
| Finality wait: finalized header → signature slot of the update that proves it | 51 | 780 | 804 | 996 | 1,008 | Ethereum |
| Update availability: signature slot → the follower's fetch began (run id) | 51 | 0 | 108 | 338 | 646 | beacon node + follower |
| On-chain verification: fetch began → included in a LEZ block (fetch, executor measure, dry run, send) | 51 | 13 | 14 | 29 | 31 | follower + sequencer |
| Cadence: store advance per update (30-min loop, not latency) | 51 | 1,524 | 1,920 | 1,920 | 1,920 | policy |

| End to end at a 30-min cadence (modelled) | typical (s) | sum of each part's maximum (s) | from |
|---|---:|---:|---:|
| finality wait (+ up to 31 slots for a state change early in its epoch) | 804 | 1,380 | public-chain + spec |
| waiting for the follower's next send | 900 | 1,800 | loop policy |
| update availability | 108 | 646 | public-chain |
| on-chain verification of the update | 14 | 31 | public-chain |
| the consumer's statement transaction (one LEZ block) | 15.00 | 15.00 | public-chain block time |
| total | 1,841 | 3,872 | 30.7 min typical, 64.5 min if every part were at its maximum (not an observed run) |

LEZ block time over the run: 15.0 s per block (blocks and times of consecutive sends).
<!-- END GENERATED latency -->

**Missing:** one timed run from a real Ethereum state change to a consumer's statement on LEZ
(`RUN-AFTER-3AM.txt` step 4), and block ancestry for logs outside anchor blocks (no design yet).

## P6: off-chain resources

RFP: "Document the compute resources (CPU, RAM, time) required to run any off-chain component the
design requires."

- **Measured (public chain), time.** The follower's cycle (fetch, native BLS check, dry run, send,
  inclusion) takes 13–31 s per update (P5's "on-chain verification" row). Between cycles it sleeps.
- **Measured (local), the private path.** About 2 h and 9.5 GB per update (P4).
- **Missing:** the follower's CPU time and peak RAM. They need one `/usr/bin/time` run on a
  throwaway local chain (`RUN-AFTER-3AM.txt`). The daemon that replaces the loops is another branch's
  (`lc-daemon`).

## P7: state growth

RFP: "Document the growth rate and on-chain storage cost of any module state that accumulates with
usage, in particular committee-tracking state, with projections over one and five years of continuous
operation."

<!-- BEGIN GENERATED storage -->
Fixed per instance: config, store (259 bytes). Grows: one anchor record per update that moves the finalized header (114 bytes on Gloas, 244 before it: two records), never deleted (R3). Period jobs: about 321 a year, 1 byte each once done, 0 after `CloseJob`. Bytes are account data (Borsh), without the LEZ state's per-account overhead, which we have not measured. The fee schedule (`market.rs`) prices execution gas and transaction bytes; we found no charge for state held.

| Cadence | anchors/year | bytes, 1 year (Gloas) | bytes, 5 years (Gloas) | bytes, 5 years (pre-Gloas) |
|---|---:|---:|---:|---:|
| every epoch (6.4 min, the fastest finality moves) | 82,181 | 9,368,662 | 46,843,312 | 100,261,125 |
| 15 min | 35,064 | 3,997,296 | 19,986,480 | 42,778,080 |
| 30 min (our followers) | 17,532 | 1,998,648 | 9,993,240 | 21,389,040 |
| 1 h | 8,766 | 999,324 | 4,996,620 | 10,694,520 |
| 6 h | 1,461 | 166,554 | 832,770 | 1,782,420 |
| once per period (27.3 h, liveness floor) | 321 | 36,596 | 182,982 | 391,645 |
<!-- END GENERATED storage -->

**Committee-tracking state itself does not grow.** The store keeps commitments to two committees,
not the keys, and is replaced in place. What accumulates is anchors, by design, because R3 keeps
every attestation valid.

The operator incentive (`docs/LC-INCENTIVE.md`) adds:
- one fixed pool per instance (with an 8-period slot tally);
- one earnings account per payee, emptied on `Withdraw`;
- one claim record per rewarded job, emptied when it is paid or closed;
- about 115 bytes per unclaimed receipt in a submitter's own actor, emptied by `Claim`.

## P8: precompile delta

RFP: "Document the cycle-count delta between the in-program BLS12-381 path and a hypothetical native
precompile, so the measurement can inform whether a follow-on precompile RFP is warranted. A
precompile is reachable only from public transactions, which the comparison must state."

<!-- BEGIN GENERATED precompile -->
- Old path (unchanged Helios): curve operations are 36,206,195 of 36,273,554 cycles; the non-curve remainder is 67,359 (local, `attest-spike`).
- Public path today: 9,455,845 gas per finality update (median, public chain). With a BLS12-381 precompile, about 124,999 would remain (the non-curve remainder, the store's `Advance` 39,510 and one anchor record 18,130), plus the committee witness hash and decoding, not measured apart: a delta of roughly 9,330,846 cycles per update (modelled).
- A precompile is reachable only from public transactions: the private path would not gain from it.
<!-- END GENERATED precompile -->

**Missing:** the public path's non-curve part, measured on its own. The executor has phase marks
(`cycle-marks`) for this (`RUN-AFTER-3AM.txt`).

## P9: no cost scales with storage

RFP: "No action's cycle count or gas cost scales with accumulated on-chain storage. Where a design
cannot avoid it, the proposal justifies why and documents the mitigations."

<!-- BEGIN GENERATED p9 -->
- lc-public: after removing the signer effect, gas changes by -29.0 per update over 51 updates (residuals -14,207 to +929); the store kept growing its anchors throughout.
- lc-handoff: after removing the signer effect, gas changes by +1.1 per update over 55 updates (residuals -14,493 to +940); the store kept growing its anchors throughout.
<!-- END GENERATED p9 -->

**By construction:**
- every action reads fixed-size accounts (config, the 259-byte store, one anchor, one job);
- anchors are separate accounts, never a list;
- nothing iterates over history.

The logs agree: over every update of both runs on the public chain, gas does not trend upward once the signer count
is accounted for.

## The operator incentive's budget (Soft #5)

<!-- BEGIN GENERATED reward -->
- Test defaults (`RewardTerms::test_defaults`, OWNER DECISION): 600,000/1 per slot, bonus 120,000,000 per period. A pool pays at most 1,616,402,460,938 native units a year whatever the number of updates (2,629,800 slots, 321.0 periods).
- Break-even: a finality update (median fee 75,880,176) pays for itself after 126.5 slots (25.3 min) of advance; the tracking bill at a 30-min cadence is 1,363,058,433,773 a year at base fee 8.
<!-- END GENERATED reward -->

## Where our proposal's words differ from the RFP's

1. **P1.** FORM §1 is headed "(RFP-021 P1, P2, P8)" and reports update costs. The RFP's P1 is about
   verifying an attestation and its predicate in one transaction on the direct path (F7), not about
   the update.
2. **The signer range.** FORM's row "RFP-021 P1–P9" says public updates RUN at "492–511 signers (real)".
   The two followers' logs show 477–506 (the table above).
3. **P2.** It asks for numbers "reproducible from the test suite". Our public-chain numbers are
   measurements from logs; only the executor counts are pinned by a test.
4. **P3.** It asks for the cost "at the recommended operating cadence". FORM names no cadence; this
   document recommends 30 minutes.
5. **P4.** It covers "running the attestation in a private transaction" on "a mid-range laptop and on
   the lowest specification the team declares". FORM commits laptop and minimum-spec measurements only
   under RFP-022 P4 (M4), and declares no minimum.
6. **The handoff limit.** FORM §1 says the period update's second step "is estimated to stop fitting
   near 450/512 signers"; the public-chain Finish points put it near 461 (§ P3, participation).

These rows, and the two Soft5 phrases (`docs/LC-INCENTIVE.md` §3), are proposed as edits in the
`rfp-docs` branch's form-edits list (rows 40–47).
