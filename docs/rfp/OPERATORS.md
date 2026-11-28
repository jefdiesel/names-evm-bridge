# Operating the light client and the bridge's services

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFPs' own words)

- **RFP-021 Usability #3:** "Provide the **update-submitting component** … provided as a **Logos module accompanied by a
  Logos Core headless CLI/daemon**, runnable standalone, supporting configurable Ethereum consensus and execution RPC
  endpoints, structured logging, and a clean shutdown path. Document the operator journey end-to-end: install,
  configure, run, monitor."
- **RFP-021 Supportability #5:** "Provide documentation in repo for the CLI and any operator-facing components,
  covering the core user and operator journeys respectively."
- **RFP-021 Supportability #7:** "Document the operator role for an instance: what running an operator costs, at what
  cadence updates should be submitted, how anyone can start one without permission, what it requires (a consensus
  endpoint and LEZ gas, and no standing on Ethereum), what an operator should expect when others are running one
  concurrently, and how to detect that an instance has stalled. State plainly that no party is obliged to operate and
  that liveness rests on interested parties choosing to spend the gas."
- **RFP-021 Reliability #10:** "… Document the deferral strategy and its effect on update latency."
- **RFP-022 Usability #3:** "Any long-running off-chain component the design requires must be provided as a **Logos
  module accompanied by a Logos Core headless CLI/daemon**, runnable standalone, using LEZ module and Ethereum module for
  the respective chains access, configurable finality depth, structured logging, and a clean shutdown path. Document the
  operator journey end-to-end: install, configure, run, monitor."
- **RFP-022 Reliability #13:** "Provide documentation in repo for the CLI and any operator-facing components, covering
  the core user and operator journeys respectively." (The user half is [USERS.md](USERS.md).)
- **RFP-022 Performance #6 / RFP-021 Performance #6:** "Document the compute resources (CPU, RAM, time) required to run
  any off-chain component the design requires." (§7.)

## Status

- **This file: an operator's map, complete for what exists, outline for what does not.** It names the real commands
  and where each lives. The detailed journeys of the light-client follower (install, configure, run, monitor, deferral,
  concurrency, stall detection) are written beside its code on branch `lc-daemon`
  (`lez-bridge/attest/follower/README.md`), with the module and IDL documents there; the operator reward and update
  costs in `docs/LC-INCENTIVE.md` and `docs/LC-COSTS.md` (branch `lc-incentive`; the second not written when this was); the bridge services' operations in
  `docs/OPS-V5-V6.md` (branch `v6-ops`, private: it names hosts and paths). This file does not repeat them.
- **RUN on our public chain:** the light-client loops (`lc-public`, `lc-handoff`), the claim relays (v4–v7), the
  settlement service and the v5 outbound updater (`v5-unattended`).
- **TESTED, not run against a LEZ chain:** `lc-follower` (branch `lc-daemon`: unit tests and a stand-in chain).
- **NOT BUILT:** the bridge's relay, settlement and updater as Logos modules with a headless CLI/daemon (RFP-022 U3, M5);
  a release relayer daemon; a published, packaged operator release of any of them.

## 1. Who may operate, and what for

Most roles below are open in code: they need no registration, allowlist or standing on Ethereum. Two are not: the
update submitter of v4–v6 (one immutable key) and, on v7, the signers (three fixed keys); the last column says so.
**No party is obliged to operate any of them; liveness rests on interested parties choosing to spend the gas.** Today we
are the only operator of each on our public chain ([SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) §8).

| Role | Needs | Paid today | Exception |
|---|---|---|---|
| Light-client update submitter | a beacon endpoint, a public LEZ account with gas, the `lez-bridge` CLI of branch `public-update` | no (a reward program is written and unit-tested on branch `lc-incentive`, `docs/LC-INCENTIVE.md`; not deployed) | — |
| Claim relay (LEZ fee payer) | a funded public LEZ account with no bridge role, the CLI, the relay and the filter | no (fees on Ethereum go to the desk owner; an open, refunded relay RUN locally on branch `open-relay`) | — |
| Release relayer | a Sepolia account with gas and a release seal | the relay fee the exit allows (0 on our service) | — |
| Outbound updater, v4–v6 | **the vault's one submitter key** | no | not open: only that key can submit (`UpdateGatedVerifier.SUBMITTER`) |
| v7 update submitter, contester, finalizer | a Sepolia account with gas; for a submit, a proof carrying 2 of 3 signatures | no | the signatures come from the three signer keys, which are ours |

## 2. Light-client follower (RFP-021 U3, Sup7, R4, R5, R9, R10)

- **Today on our public chain:** a shell loop, `public-demo/prove/lc-handoff.sh` (branch `v6-lc`), every 30 minutes,
  for the instance behind v6 and v7 (`0xe4b7d26a…3600`, program `9Wb7sBxs…uhNR`). It crossed six committee handoffs over
  seven days unattended (`2026-11-18 00:36` to `2026-11-25 02:51 UTC`, 354 transactions, none refused; `lc-handoff`,
  `public-demo/prove/lc-7day`). The finality-only loop `public-demo/prove/lc-public-loop.sh` (branch `lc-public`) ran
  from 2026-11-17 until period 1416 ended.
- **By hand** (CLI of branch `public-update`):
  ```sh
  lez-bridge attest show --json [--program P --instance I]           # read the config and the store
  lez-bridge attest update-public --kind finality ...                # one public finality update
  lez-bridge attest update-public --kind period --step begin|finish|both ...   # a period update (handoff)
  lez-bridge attest close-job ...                                    # close a stale period job
  ```
  Each send is first checked natively with the program's own code; a refusal prints its `RFP021-Enn` code and sends
  nothing (`docs/ERROR-CODES.md` on `public-update`). Exact flags: `lez-bridge attest <command> --help`.
- **The component (branch `lc-daemon`, TESTED only):** `lc-follower check|run|status --config <file>`; reads Ethereum
  through the Logos `beacon_api_module` when Logos Core answers, else configured beacon endpoints; re-reads the store
  just before each send; defers each update by `SHA256("lc-follower/defer/v1" ‖ operator ‖ instance ‖ target) mod
  (max_defer_s + 1)` seconds (default `max_defer_s` 300: on average 150 s with one operator, 300/(n+1) s with n); stops
  itself near the weak-subjectivity deadline. Its README has the full journey and the measured cadence.
- **Cadence:** every 30 minutes on our chain; at each send the new head was a median 16.5 minutes old (follower README,
  from the loops' logs).
- **Detecting a stall:** `lez-bridge attest show --json` gives the store's finalized slot; compare it with Ethereum's
  finalized slot. The follower reports `lag_slots`; more than about 300 slots (an hour) with its defaults means stalled.
  Anyone may then start one.
- **Concurrency:** another operator's update makes yours a skip (no gas) if seen before sending; a race inside the CLI's
  own few seconds ends in a refusal or a revert, which costs that transaction's fee.
- **Cost:** about 9.3–9.6M gas per finality update and 3.2M + 9.3–9.6M for a period update on our public chain
  (`lc-handoff` row); what that costs in LEZ and the amortised figures: `docs/LC-COSTS.md` (branch `lc-incentive`, not written when this
  was).
- **The private path** (on `main`): `lez-bridge attest update` from a private account proves the update locally: about
  2 h and 9.5 GB on a 16 GB desktop (`DESIGN.md` §9.0a). Not practical as a standing operator.

## 3. Claim relay (RFP-022 PP6, U3)

- **What it does:** co-signs a public claim, or a public burn to its bridge (RUNBOOK §19), as its fee payer, after strict checks
  (`DESIGN.md` §3.6; caps per deposit, per IP, per UTC day; a payer floor; every decision one JSON log line).
- **Run it:** `public-demo/relayer/relayer.py` with `lez-bridge relay-cosign`, reached only through the RPC filter
  `public-demo/proxy/lez-rpc-proxy.py --relay <loopback>` (JSON-RPC `bridge_relayInfo`, `bridge_relayClaim`,
  `bridge_relayBurn`); on the build host `public-demo/demo.sh start relayer`. Settings are environment variables
  (`PAYER`, `MAX_GAS`, `MAX_FEE`, `DAILY_MAX`, `IP_DAILY_MAX`, `MIN_PAYER_BALANCE`, `MAX_RELAYS`, `DESK_STATE`,
  `BURN_DAILY_MAX`, `IP_DAILY_BURNS`); the module docstring and `public-demo/RUNBOOK.md` (not published) §13 and §19 list them. One relay
  per bridge instance (v6 and v7: §13.3).
- **Worst case it pays:** `(DAILY_MAX + BURN_DAILY_MAX) × MAX_FEE` native LEZ a rolling day (`relayer.py` docstring).
- **Tests:** `python3 -I public-demo/relayer/test_relayer.py`, `public-demo/relayer/e2e-relay.sh` (throwaway chain).
- **Not built:** the relay as a Logos module with a headless daemon; a relay list and per-claim choice in the wallet
  (M5). An open relay refunded inside the claim it pays for, with client failover: RUN locally on branch `open-relay`.

## 4. Release relayer and the settlement service

- **Anyone can release.** `release(seal, root, nullifier, token, denomIndex, recipient, relayer, relayFee)` on the vault
  is open; recipient, relayer and fee are bound in the proof (`vault/src/BridgeVault.sol` 517-571). A release code
  (`logosrel1…`) carries these; the deposit page's `#release` view checks it and sends the call from any Ethereum wallet
  (`web/deposit/src/lib/send-back.js`).
- **Making the seal:** the release proof (about 36K cycles, 20.9 s, 1.1 GB) from the exit secret
  (`lez-bridge exit-secret … --out F`), then the Groth16 wrap on an x86-64 Linux machine with Docker
  (`lez-bridge/outbound/wrap`, `outbound_wrap wrap|check|seal`), or on a GitHub-hosted runner
  (`.github/workflows/wrap-private.yml` for a journal that names a person). The proving scripts under
  `public-demo/prove/` are the owner's, run on the build host.
- **Our settlement service** (`public-demo/settle/settle.py`; RUNBOOK §22): proves, wraps and releases when a wallet sends
  its exit secret, from its own key, fee 0. It learns which burn pays which address (a privacy exception the wallet
  states). `python3 -I public-demo/settle/test_settle.py` tests it offline.
- **Not built:** a release relayer daemon that reads the relay-fee menu and takes a menu fee (M5).

## 5. Outbound updater

- **v4–v6:** `public-demo/updater/updater.py cycle [--no-submit] | check | status` (RUNBOOK §23): the v5 vault's next
  update over the final LEZ blocks (head − 20 on our chain), proved, wrapped through `wrap-private.yml`, seal checked
  read-only, submitted through the gate. The loop starts a cycle on the next full hour; a cycle that proves takes about
  2 h, so updates landed about every three hours on 2026-11-18 (`docs/OPS-V5-V6.md` §1.2, branch `v6-ops`, a private
  file). That file also holds the plan for a second updater instance for v6. **Only the gate's submitter key can submit**, so only we can run it
  for v4–v6. v6 has no standing updater (its one update ran by hand).
- **v7:** `SeqSigGate.submit` is open; the proof must carry 2 of 3 signatures. The signers use `seqsig attest` (branch
  `seq-sig`: re-reads — re-hashes and re-extracts — the range natively, signs only blocks a set depth under the head,
  keeps a sign-once record; **it checks neither privacy receipts nor public execution yet**, so with our stand-in keys
  on our one sequencer v7 trusts us). The conductor's runner is `public-demo/v7/update-v7.sh` (branches `v7-seqsig`,
  `v7-in-wallet`). Watchers contest with `seqsig evidence` and `SeqSigGate.contest` inside the window.
- **Cost:** a proof grows with the LEZ blocks it covers, about 20,000 cycles a block; 494 blocks took 2,412.6 s on a
  16 GB desktop and 480 blocks 7,925.9 s on an 8 GB one (`two-machines` row); the wrap 164 s on a GitHub-hosted runner;
  `update` 290,000–900,000 gas (proposal §4).
- **Before any proving run or deploy:** `node scripts/check-versions.mjs --onchain --deployment <name>` must pass
  (`AGENTS.md`, "Version alignment").

## 6. Monitoring

- `scripts/verify-evidence.sh` re-reads the recorded transactions and fixed values; it is not a liveness monitor.
- `lez-bridge show` (the bridge config: tokens, counters, roles) and `lez-bridge attest show --json` (an instance).
- Sepolia: the vault's `g2State()`, `exitCount()`, `depositCount()`; on v7 the gate's `pending()`, `finalizableAt()`,
  `activeSigners()`, `halted()` (`SeqSigGate.sol`).
- The services write JSON log lines (relay: one per decision; settlement: job id, stages, run ids and transaction hashes
  only; follower: one per event).

## 7. Compute resources (RFP-021 P6, RFP-022 P6)

| Component | Measured | Source |
|---|---|---|
| Light-client follower (public path) | not measured (it calls the CLI, which simulates and sends one transaction) | follower README; to measure in M3 |
| Light-client update, private path | 7,283.0 s, 9,457 MB peak, Apple M4 (16 GB), 4 threads, CPU prover | `DESIGN.md` §9.0a |
| Outbound update (G2-lite) | 97 blocks 526.8 s / 4,699 MB; 511 blocks 2,575.1 s / 4,746 MB (16 GB M4); 480 blocks 7,925.9 s on an 8 GB desktop | `DESIGN.md` §4; `two-machines` row |
| Release proof (G3) | 20.9 s, 1,129 MB | `DESIGN.md` §4 |
| Groth16 wrap | 164.3 s per update, 243 s per release, GitHub-hosted x86 runner (4 CPUs, 15 GB) | `DESIGN.md` §4 |
| Claim relay, settlement service | not measured | — |

## Sources

`AGENTS.md`; `DESIGN.md` §3.6, §4, §9; `public-demo/RUNBOOK.md` (not published) §13, §19, §22, §23; `public-demo/relayer/relayer.py`;
`public-demo/settle/settle.py`; `public-demo/updater/updater.py`; `vault/src/SeqSigGate.sol`,
`UpdateGatedVerifier.sol`; `lez-bridge/attest/follower/README.md` (branch `lc-daemon`);
`public-demo/prove/cli-ws-20261117/README.md` (branch `public-update`); `docs/EVIDENCE.md`.
