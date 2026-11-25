# The RFP-021 light client, seven days unattended on a public LEZ chain

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

One follower process tracked Sepolia's beacon chain and posted every update to the RFP-021
light-client program on our public LEZ v0.4.0-rc3 chain, with no restart and no hand edits.
Every update it made is listed in [`updates.csv`](updates.csv), and every log line that is not
a transaction is in [`notices.txt`](notices.txt). Anyone can re-read all of them from the chain
with [`check.py`](check.py).

## The seven days

From **2026-11-18 00:36 UTC** (LEZ block 23262) to **2026-11-25 02:51 UTC** (LEZ block 64101):
7 days 2 h 14 min.

- **354 transactions:** 340 finality updates, and 7 sync-committee period updates of two
  transactions each (14). None refused.
- **Six committee handoffs crossed:** the period updates for periods 1417 to 1422, at LEZ blocks
  27983, 34644, 41188, 47725, 54266 and 60803 (2026-11-18 20:16 to 2026-11-24 13:05 UTC). The
  update at block 23262 is the start of period 1416, when the run began.
- **Longest gap between two updates:** 1 h 00 min 38 s (from 2026-11-24 14:06 UTC, the cycle skipped when the payer balance could not be read). The loop runs every 30 minutes.
- **Notices, all in `notices.txt`:** six "no finality update yet" waits (the beacon node had
  nothing newer than the store), one error from the public Sepolia beacon endpoint, and one
  skipped cycle when the payer's balance could not be read. None stopped the run.

## Still running

The same process is still running. The record here goes to **2026-11-25 18:33 UTC**
(LEZ block 67866): 387 transactions in all, seven handoffs (period 1423 at LEZ block 67379).
Its fee payer was topped up on 2026-11-25 and is funded to about 12 December.

## Check it yourself

```sh
python3 check.py              # every row, about 4 minutes
python3 check.py --sample 20  # 20 rows spread over the run
```

For each row, `check.py` fetches the transaction by its hash and the block the row names from
`https://lez.nameswallet.app/` (JSON-RPC, `getTransaction` and `getBlock`) and checks that the
transaction's bytes are inside that block. Standard library only. The same blocks and
transactions can be browsed at [nameswallet.app/explorer](https://nameswallet.app/explorer/).

## `updates.csv` columns

| Column | Meaning |
|---|---|
| `utc` | when the follower logged the transaction as included |
| `kind` | `finality`, `period-begin` or `period-finish` (a period update is two transactions) |
| `tx` | transaction hash on LEZ |
| `lez_block` | the LEZ block it is in |
| `gas`, `fee` | gas used and fee paid (test LEZ) |
| `slot_from`, `slot_to` | for finality updates: the store's finalized Sepolia slot before and after |
| `period`, `signers` | for period updates: the Sepolia sync-committee period and how many of 512 signed |

## What this does and does not show

- It shows a light client that keeps up with real Sepolia data for a week, across committee
  handoffs, with each update an ordinary public LEZ transaction under the 10M gas cap.
- **Still trusted:** this is our own LEZ chain (Logos has no public 0.4 testnet yet), and we run
  its only sequencer. The Sepolia data comes from public beacon endpoints; the program checks the
  committee signatures itself.
- Testnet only: these coins have no value.
