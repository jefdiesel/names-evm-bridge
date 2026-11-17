# Evidence: what ran, where, and how to check it

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

One row per result. **Where** says which network. **Proofs** says what kind: *real* (proved, and for Ethereum wrapped to Groth16 and checked by RISC Zero's verifier 3.1.0 on Sepolia), *none needed* (an ordinary public LEZ transaction the sequencer executes), *executed* (the proof program was run but not proved; a mock verifier on a local Ethereum node) or *development receipts* (LEZ's development mode: the circuit runs and the block contents are real, the receipt is a stub).

Nothing here ran on a Logos network: LEZ testnet 0.4 is not live. "Our public chain" is our own LEZ v0.4.0-rc3 sequencer at `https://lez.nameswallet.app/` (JSON-RPC over POST). Nothing is audited. All funds are test funds. Work dated 2026-11-17 was built that day on branches beside `main`; where a separate AI agent reviewed it, the row says so.

`B` below is `https://github.com/jefdiesel/names-evm-bridge/blob`.

## On public networks

| What | Where | Proofs | Record |
|---|---|---|---|
| Round trip: deposit, relayed claim, burn, state update, release | Sepolia vault v4 + our public chain, 2026-11-15 | real | deposit Sepolia block 12142034; update `0x7fd725cd…bb369` (block 12142209); release `0x8b39bdc7…c7bb` (block 12142503); second update block 12143000. Re-check: `docs/VERIFY-ROUND-TRIP.md`; receipts in `public-demo/prove/receipts/` |
| Private round trip: private claim, private burn, release to an address never used before | Sepolia vault v5 + our public chain, 2026-11-16 | real | claim LEZ block 15255; burn LEZ block 15395; update `0xe5053592…2a24` (block 12149823); release `0xc8bea057…0258` (block 12149867). The release was sent by our settlement service, which held the exit secret |
| State updates proved and posted unattended, about every two hours; releases sent by a settlement service | same, since 2026-11-16 | real | every update is a transaction to the gate `0x0F3F80D37B1616C0E9B182D3A87FE4f6220715d1` |
| Two machines proving back-to-back ranges of the state update, both accepted in order | same, 2026-11-17 | real | LEZ 21442..21921 on an 8 GB desktop in 7,925.9 s: `0x099f47c1e9a42d99cd2c5aea8b431612897c49de583cf8523bdc96d44eefecd4` (block 12157050). LEZ 21922..22415 on a 16 GB desktop in 2,412.6 s: `0xb88188c4d52ab04052fe2df4f5a56e3e409476cf2a6ce92073878fc387327109` (block 12157228). The second run was started by hand and handed to the updater; the two proofs overlapped for about five minutes |
| RFP-021 light client, public finality path: a real Sepolia finality update verified in one public transaction, then a loop every 30 minutes | our public chain, from 2026-11-17 (LEZ block 21641) until Sepolia period 1416 ends on 2026-11-18 (this image does not cross a committee handoff) | none needed | `B/lc-public/public-demo/prove/public-update-20261117/PUBLIC-CHAIN.md`, with the commands to read each transaction and the store |
| Deposit page for ordinary Ethereum wallets; takes an LNS name as the destination | live | — | `https://nameswallet.app/bridge/`; `web/deposit/LNS.md` |

## RFP-021 light client on local LEZ v0.4.0-rc3 chains (branch `public-update`)

All of these are ordinary public transactions on real Sepolia data unless the row says otherwise; no proof is made anywhere. Records are under `B/public-update/public-demo/prove/`.

| What | Result | Record |
|---|---|---|
| Finality update in one transaction | 9,285,563 cycles, 92.9% of the 10M cap, 506 of 512 signers, twice; a consumer used the anchor in the next block. One AI review: no soundness bug found | `public-update-20261117/README.md` |
| Committee handoff | period update in two transactions (3,193,398 and 9,318,848), then the first update signed by the next committee (9,347,926), across the period 1415 to 1416 boundary. One AI review, from source: no soundness bug found; its findings fixed and the run repeated | `public-handoff-20261117/README.md` |
| Catch-up over three periods | from a checkpoint in period 1413 to the head in 1416: three period updates, three handoffs, driven by the CLI alone | `catchup-20261117/README.md` |
| Low participation, two steps | 427 of 512 signers: 6,549,558 and 3,780,392; 342 of 512: 7,485,921 and 3,780,392; 341 refused. **Synthetic committee** (real Sepolia headers and branches). One transaction fits down to about 442 signers (estimate) | `finality-split-20261117/README.md` |
| Statements and reference consumers | a receipt-log statement, an account statement and a storage-slot statement on a chain; the ERC-20 and native-ETH consumers, each refusing a replay; the ERC-20 one also refusing a claim for another chain | `statements-20261117/README.md` |
| Typed error codes | 13 codes; the off-chain dry run returns the same code as the program; five shown on a chain, the rest in native tests | `error-codes-20261117/README.md` |
| CLI and the stale-checkpoint rule | init, public updates, period jobs and statements from the CLI; an update past the weak-subjectivity bound is refused (shown by the CLI's pre-check and native tests) | `cli-ws-20261117/README.md` |
| Consensus-specs light-client vectors v1.6.1 | 22 of 22 applicable sync cases through three verification paths; 17 pre-Capella cases not applicable. Tests the verification code, not a chain | `spec-vectors-20261117/README.md` |
| The earlier private path | a finality update (507 of 512 signers) **proved** on CPU in 7,283 s and accepted; a public claim under its anchor; a private claim proved in 676 s. 2026-11-14, pre-fix programs | `B/main/public-demo/prove/run2-20261114/` |

## Bridge work on local chains

| What | Result | Proofs | Record |
|---|---|---|---|
| Withdrawals on a threshold of signer keys (branch `seq-sig`) | 3 of 4 accepted and paid through a gate anyone can submit to; 2 of 4 refused; a key that signs two hashes for one block is ejected; a conflicting quorum halts the gate. **The keys are stand-ins of ours**, not LEZ sequencer keys. Two AI reviews: no soundness bug found; findings fixed or listed as known limits | executed | `B/seq-sig/public-demo/prove/seq-sig-20261117/README.md` |
| Privacy measurement and recovery (branch `privacy-test`) | 20 users: an adversary reading every public byte paired 1 of 20 claims with its deposit and 0 of 20 releases with its burn (chance is 1); four leaky controls caught 5 of 5 each. Two narrowing signals found: repeat claimers differ in size from first-time ones, and claiming soon after depositing. Recovery from the seed alone, wallet deleted first: 45 of 45 deposits, 30 of 30 exits | development receipts; updates and releases executed | `B/privacy-test/public-demo/prove/privacy-test-20261117/README.md` |
| Open claim relay and sponsored private burn (branch `open-relay`) | two independent relays, each refunded inside the claim it paid for; failover between them; a failed claim refunds nothing; a private burn whose fee the bridge sponsors, once per claim. One AI review: no way found to take funds; a sponsor-farming flaw fixed; known limits listed | private steps: development receipts | `B/open-relay/public-demo/prove/open-relay-20261117/README.md` |
| Failsafe, Ethereum half (branch `failsafe`) | a vault variant with a wind-down and a reclaim proven by the depositor: two unclaimed deposits reclaimed, a claimed one refused, a redirect and a replay refused. The LEZ half (a mint freeze that cannot be lifted) is not built; no review yet | executed | `B/failsafe/public-demo/prove/failsafe-20261117/README.md` |
| The whole round trip on a reviewer's machine | `scripts/judge-demo.sh`, 17 labelled steps; macOS arm64, Linux x86-64, a GitHub-hosted runner | executed | `docs/judge-demo.log` |

## Reviews and tests

- Internal security review, 2026-11-14, by AI agents in separate sessions, not a third-party audit: one critical, two high and several medium issues; all fixed in code except one medium, partly fixed. `research/security-review-2026-11-14.md`.
- CI on every push to this repository: the vault suite, the LEZ program suites, a version-alignment check and a script check. Each branch above has its own green run.
