# Privacy and recovery test, 2026-11-17 (RFP-022 PP1, PP2, PP3, F9, R5)

**Chains:** a throwaway LEZ v0.4.0-rc3 sequencer (loopback, its own genesis, no Bedrock node) and a local
anvil. Not the public demo chain, not Sepolia. **Mode: development receipts** (below). Scripts:
`lez-bridge/scripts/privacy/` (`harness.py`, `adversary.py`, `score.py`; README there).

**Label: dev receipts.** The public outputs of each private transaction are the real ones; the receipt is a
risc0 development receipt; updates and releases were executed, not proved, and mock-verified. Run 2026-11-17
15:33-16:16 (Part A), 16:16-16:29 (Part B) for the run of record (`a2`, seed 20261119), and 14:59-15:21 for
an interleaved replicate, on the Apple M4 build host. An earlier run with the same schedule (`a20`, seed
20261117, 13:45-14:59) gave the same picture: main set 1 / 20, 1 / 20, 0 / 20 and 0 / 20, every control at
5 / 5, recovery 45 / 45 and 30 / 30. Its capture and ground truth were deleted by mistake in a cleanup, so
its numbers are not the record; the tables below are all from `a2`, whose capture, truth and adversary
output are kept on the build host.

**In short**

| Measurement | Result |
|---|---|
| Pairing, main set N = 20 (one deposit, private claim, private burn, relayed release each), everything the adversary has | inbound 1 / 20, outbound 0 / 20, burn -> claim inside LEZ 0 / 20, round trip 1 / 20; 1 expected at random |
| The same with users who claim soon after depositing (interleaved replicate) | inbound 3 / 20 (2.68 expected for a guess among the deposits before each claim; "latest earlier deposit" right for 6 / 20); outbound 1 / 20; timing is the signal, no byte or fee feature narrowed |
| Positive controls (5 users each) | public-account claim 5 / 5; secret from the depositor's address 5 / 5; per-user fee on both Ethereum transactions 5 / 5 (round trip); release sent by the depositor 5 / 5 (round trip); synthetic planted address, substring and fee values 5 / 5 each |
| Recovery from the seed phrase alone, 15 users (10 private, 5 public) | 45 / 45 deposits, 30 / 30 exits found; 15 / 15 claims and 15 / 15 releases completed from the restored wallets |
| A real leak found | none that pairs a deposit with a claim or a burn with a release; one narrowing signal: a claim from an account that already holds private state is larger and reserves a different fee, so repeat claimers are separable from first-time claimers (below) |

## Part A: can an observer of both chains pair them? (PP1, PP2, PP3)

**N = 20** users in the main set (ETH 0.01), each with its own Ethereum account and its own LEZ wallet (own seed
phrase, one private account): one deposit, one private claim to a one-time address (sponsored from the bridge's
float), one private burn to a fresh seed-derived Ethereum address, and its release, sent by a relayer account
that no user controls. Four control cohorts of 5, each in its own (token, denomination) set, ran on the same
chains at the same time. Schedule: every deposit in random order (1-6 s apart), one attested root, every claim in
random order, every burn in random order, one G2 update, every release in random order. 215 Ethereum blocks and
168 LEZ blocks were captured.

**What the adversary reads:** `capture/` only. That is every Ethereum block, transaction, receipt and log, and
every transaction's storage writes (`debug_traceTransaction`, prestate diff mode: 152 transactions, no errors;
each changed slot and value goes into that transaction's values). It is every LEZ block's bytes (`getBlock`), and
the deployment's public constants (vault, cfg id, bridge program, config account, wrapped definitions, the
denominations). The LEZ sequencer serves account state only as it is now (`getAccount`, no history), so
"account-state changes" are what the blocks carry, plus one end-state view of every id the blocks name as an
actor (`lez-accounts.json`: 5,760 ids read, 57 existing accounts: the bridge, the token program, the two
definitions, the deployer, the c1 users' public accounts and one claim-nullifier account per claim, each named
in one block and holding the spent marker; no one-time address exists as an account, so no balance follows a
private claim). The adversary does not read these views (no pairing feature uses them); they are kept for
audit. The ground truth (`truth/`) is written apart and read only by
`score.py`.

**Unit of a LEZ event:** a whole block. The harness sends one bridge transaction at a time and waits for it,
so each claim or burn sits in its own block (beside the sequencer's clock transaction). The adversary reads
claims and burns at block level and needs no transaction parser; a busier chain would mix transactions in a
block, which only makes pairing harder.

**How it pairs:** for each problem, a score per (target, candidate) from each feature, a hard filter on the
(token, denomination) set and on time order, then a maximum-weight one-to-one assignment (Hungarian). Mode
"allowed" uses token, denomination, time order, time rank and the time gap between the two events; "everything" adds the published root and
exit-root bounds and
every feature: 32-byte and 20-byte values of one event found anywhere in the other's bytes; shared 8-byte
substrings; a recomputed deposit commitment `C` from every published 32-byte value (and from the padding,
SHA-256 and Keccak of every Ethereum address seen) as the secret, with every published account id as the
destination; a recomputed release nullifier from every 32-byte value of a burn; shared fee-like integers
(u64 at every offset, Ethereum gas prices, priority fees, values, gas used); transaction size and gas rank;
the same Ethereum account on both ends; and, for the round trip, the chain of the other three pairings.

**Chance:** in a set of n, a uniformly random one-to-one matching gets 1 right on average (n x 1/n), whatever n.
P(at least k right) is exact (fixed points of a random permutation): for n = 20, P(>= 1) = 0.632, P(>= 2) =
0.264, P(>= 3) = 0.080, P(>= 4) = 0.019; for n = 5, P(5 of 5) = 1/120 = 0.0083. Where time order or a bound
leaves fewer candidates, `score.md` also gives the expected number right for a uniform guess among each
target's feasible candidates (the sum of 1 / set size). In this run it is 1.00 for the main set on every
problem: one root, one update and phase-separated steps leave every candidate feasible.

### Scores (`results/score.md`, `results/score.json`)

| Problem | Main set (N = 20), allowed | Main set, everything | Expected, random matching | Expected, guess among feasible |
|---|---|---|---|---|
| Inbound: claim -> deposit | 1 / 20 (P = 0.632) | 1 / 20 (P = 0.632) | 1 | 1.00 |
| Outbound: release -> burn | 0 / 20 | 0 / 20 | 1 | 1.00 |
| Inside LEZ: burn -> claim | 0 / 20 | 0 / 20 | 1 | 1.00 |
| Round trip: release -> deposit | 1 / 20 (P = 0.632) | 1 / 20 (P = 0.632) | 1 | 1.00 |

No feature narrowed a main-set user's candidates beyond the set and time order. Every matching feature
(exact32, exact20, sub8, derivedC, derivedNf, fee) kept 20 of 20 for inbound and outbound. The time features
always name a candidate: time rank and time gap were each right 1 time in 20, as chance gives. Size kept the two median-gas deposits
for every claim, because all claims are the same size; it held the truth 3 times in 20. Inside LEZ, sub8 kept 16.2
claims per burn (truth kept 16 of 20) and fee 10.7 (truth kept 10 of 20), what chance gives for sets that size:
values found in every burn and in only some claim blocks (in `a20`, the same check pointed every burn at one
claim block). That is noise.

**Integers that change from claim to claim, checked with the ground truth:** reading a u128 at every offset
finds two values per private claim (offsets 1342 and 1439) that differ from claim to claim and stay under
2^24. They are not amounts. They are the last three bytes of two 32-byte ids the claim publishes by design (the
one-time address A at offset 1313, and a second per-claim account id at 1410), read across into zero padding.
Checked by locating A's bytes in the block. The burn's value at offset 730 is 8,760,479 in 19 of 20 burns and
8,760,492 in one. For the 20 main users, no equality, sum, difference or 50,000,000-minus relation held between
a user's own claim values and own burn values more often than across users: each combination took 20 distinct
values over the own pairs. No relation means nothing to code as a feature.

### Positive controls (each must be paired)

| Control (5 users, own set) | What it does | Caught on | Score | By feature |
|---|---|---|---|---|
| c1 public account | claim and burn from a public LEZ account | inbound | **5 / 5** (P = 0.0083) | `derivedC`: the claim publishes the secret k and the account, so C is recomputed and found in the deposit log |
| | | inside LEZ | **5 / 5** | the burn and the claim name the same public account (exact32, sub8, fee) |
| | | outbound, round trip | 0 / 5 | the release still does not link to the burn |
| c2 address secret | deposit secret = the depositor's Ethereum address padded; claim names its account | inbound | **5 / 5** (P = 0.0083) | `derivedC` with k = the padded sender address |
| c3 fee fingerprint | one per-user number as the Ethereum priority fee of the deposit and of the relayed release, and as the LEZ `--gas-limit` of the claim and burn | round trip | **5 / 5** (P = 0.0083) | `fee`: the same priority fee on both Ethereum transactions |
| | | inbound | 0 / 5 | chance: a private claim does not publish its gas limit (below) |
| c4 self-release | the release is sent by the user's own depositing account | round trip | **5 / 5** (P = 0.0083) | `sender` (and exact32: the same address in both) |

The round-trip score for c1 stays at chance because only the release-to-burn link carries it, and that link holds.
c2's outbound came out 3 / 5 (P = 0.09) from the time features alone (timing and gap each 1 / 5 on their
own): not significant across 16 control scores.

**Where the controls differ from the brief's wording.** A private claim has no public field where an Ethereum
address could appear. So c2 makes the address an input of the secret, and makes the claim name its account so
that C can be recomputed. The bridge CLI's private claim takes no tip, and its gas limit is not published (checked
in a gate run: the value appears neither as u64 nor as u128). So c3's per-user number shows on the Ethereum side
(deposit and release) and is caught on the round trip, not inbound. No real control exercised `exact20`, `sub8`
or `fee` on a private claim block, so those features got synthetic tests:

**Synthetic planted controls** (a test of the adversary, not of the bridge: `plant.py`, `results/planted-summary.txt`).
A copy of the frozen capture had one value of each user's own deposit appended to the LEZ claim blocks of 15
main users: the sender address for 5 (exact20), 8 bytes of the commitment for 5 (sub8), and the deposit's gas
used as a u64 for the users whose gas used is unique in the set (fee; 4 in this run). Result: 5 / 5, 5 / 5 and
4 / 4 paired, and each feature alone singled out the right deposit every time. The main set as a whole: 16 / 20
(P = 1.9e-14, the planted 14 plus chance).

### Signals that narrow, or could, beyond token, denomination and time

| Signal | What the run showed | What it reveals |
|---|---|---|
| Claim kind by size and shape | Every private claim to a one-time address is 30,586 bytes of block in either ETH set; a claim that names its account (`--reveal-recipient`, from an account that already held a shielded fee credit) is 33,858; a public claim 2,524. Burns: 32,566 private, 1,302 public. | Which kind of claim or burn it was, never which user within a kind. **Dev-mode sizes:** a real receipt is about 223 KB instead of the 24 KB stub, again the same for every private transaction. |
| Root bound (stand-in) | A private claim names its root through the root's account (the root bytes are absent; the attest block, the first to name that account, names both). Public claims publish the root in clear. One root here: the adversary inferred it for all 40 claims (from the account the attest block is the first to name) and the bound kept 20 of 20, truth 20 of 20. | With a root attested after each deposit, a claim's root bounds its deposit to those at or before it. Clients should claim under the newest root. |
| Exit root bound | Each release names the exit root it proves under; one update here (40 exits), so no narrowing. | With several updates, a release bounds its burn to those before its update. |
| Order and timing | Phase-separated schedule: time rank and time gap were right about 1 time in 20 on every problem. | A user who claims soon after depositing narrows the set by timing: measured in the interleaved run below. |
| Fee amounts in a private claim | The claim publishes its fee reserve (5,649,536 for a first claim: the figure the wallet's dry run prints, block offset 1543) and the sponsor (50,000,000). The user's `--gas-limit` is not among the published values (searched as u64 and u128; c3 inbound stayed at chance). | Nothing that matched an Ethereum value or another user's transaction. For accounts that already hold private state, see "Repeat claimers" below. |
| **Repeat claimers** | A claim from a private account that already holds private state (its second claim, or its third after the restore in Part B) is 34,322 bytes with a fee reserve of 9,180,496, for all 20 such claims (10 second claims, 10 after the restore); a first claim from a fresh account is 30,586 bytes with 5,649,536, for all 20 (offset 1543). | **Which claims come from accounts that claimed before.** It does not say which earlier claim, and every repeat claim looks alike, but it splits the anonymity set into first-time and repeat claimers. A wallet can avoid it by claiming into a fresh private account each time. |
| Burn fee reserve | 8,760,479 in 19 of 20 main-set burns (8,760,492 in one); the 0.1 ETH set 8,760,700; the USDC 1000 set (c2) three values among five. The odd main value did not repeat on that account's second burn. | The set (token, denomination), which is allowed. Within a set, a value that changes from one transaction to the next, with no account fingerprint seen. |
| Sponsor and float | The float pays a fixed 50,000,000 to each one-time address. | The same for every sponsored claim; the float's balance only times claims, which the blocks time anyway. |
| Deposit gas | 138,035 to 207,287 by leaf index (Merkle insertion), public anyway. | Nothing beyond the leaf index the event already publishes. |
| Release gas | 119,406 to 119,430 for ETH (calldata zero bytes). | No match with any burn. |
| Fee funding for a named-recipient claim | c2 users funded V with `shield` from a public account the operator funded. | That public account to a one-time address of V; harmless here because the claim names V anyway. A private claimer who is not sponsored needs such funding. |
| Relayer co-signatures | Not exercised: claims were sponsored, releases sent by one relayer with relay fee 0. | The relayed public claim links nothing private but is public by construction. |
| Shared bytes between burn and claim blocks | In `a20`, before a fix, exact32 kept one claim for 17 of 20 burns, 16 wrong: each was a window made of one byte of a field plus 31 bytes of an id repeated in every bridge block. Windows that straddle a constant are now dropped. | Noise, not a link. |

### Interleaved run: users who claim soon after depositing (`results/interleaved/`)

A second run of the same harness on fresh chains (seed 20261118, N = 20, main set only, no controls). Its schedule
is different: deposits and claims interleaved, each user claiming 0.5 to 3.5 other deposits after its own; the
attestor records a new root after every deposit, so each claim names the newest root at its time. Burns came in
two batches with a G2 update after each, and a quarter of the users released between the two updates. **Its
capture and ground truth were not retained** (deleted with `a20`); only the scores and the adversary's output in
`results/interleaved/` remain, and they were made before the adversary recorded which root it inferred for each
claim. The root inference was checked afterwards on `a2`'s whole-chain capture, which has two roots: every one of
the 40 Part A claims was mapped to the first root and every one of the 30 Part B claims to the second, the
vault's root after Part B's deposits.

| Problem | Allowed | Everything | Mean candidates left by time order (of 20) | Expected, guess among feasible |
|---|---|---|---|---|
| Inbound: claim -> deposit | 3 / 20 | 3 / 20 | 11.9 | 2.68 |
| Outbound: release -> burn | 1 / 20 | 1 / 20 | 17.5 | 1.25 |
| Inside LEZ: burn -> claim | 2 / 20 | 2 / 20 | 20 | 1.00 |
| Round trip: release -> deposit | 2 / 20 | 1 / 20 | 20 | 1.00 |

- **Timing narrows when users claim soon after depositing.** Time order alone cuts a claim's candidates from 20 to
  11.9 on average. Picking the latest deposit before each claim (`gap` alone) is right for 6 of 20 claims, against
  2.68 expected for a uniform guess among the feasible deposits; the one-to-one assignment got 3. This is the
  allowed signal (time), and it is why clients should wait for several later deposits of the same
  denomination before claiming (the mini-app suggests a claim time; nothing enforces it, `DESIGN.md` §3.5).
- **The root bound adds nothing beyond time** when claimers use the newest root: `bound` kept exactly the
  candidates time order kept (11.9 of 11.9), truth kept 20 of 20. (The same numbers would also appear if no root
  had been inferred; the inference itself is shown on `a2` above.) A client that claimed under an older root would
  narrow its own set to the deposits under that root.
- **The exit-root bound** likewise kept what time order kept (17.5 of 17.5): releases between the two updates
  could only be for first-batch burns, which time order already implies.
- No byte, derived-value or fee feature narrowed any main-set user (kept sets equal to the time-order sets);
  inside LEZ, `sub8` kept 13 claims per burn (truth kept 12 of 20) and `fee` 2.5 (truth kept 3 of 20), about
  what chance gives for sets that size.

## Part B: recovery from the seed phrase alone (F9, R5)

**Users:** 15 of Part A's: main-00 to main-09 (private LEZ accounts, ETH 0.01) and c1_public-00 to -04 (public
accounts, USDC 100). Each wallet was created from its own seed phrase (written to a mode-600 file, a throwaway
test value). After Part A's capture was frozen, each user made two more seed-derived deposits
(`deposit-request`), claimed one, left one unclaimed, burned the new token to a seed-derived address and left
that exit unreleased (in the exit tree after a second G2 update). Expected per user: deposits {0 claimed,
1 claimed, 2 claimable}, exits {0 released, 1 unreleased}.

**Then, per user:** list the wallet directory recursively (`storage.json` with the keys and private account
state, `wallet_config.json`, the `bridge-client/` scan cache), delete all of it, list it again (empty), restore
with `lez-bridge wallet restore-keys --depth 4` from the phrase alone, and run `lez-bridge --json recover` against
both chains (a full scan of every LEZ block and of the vault's logs). Then, from the restored wallet: claim the
claimable deposit (`claim --key-index 2`) and release the unreleased exit (`exit-secret` from the restored keys,
the G3 release executed, sent by the relayer), and `recover` again. The completion step took the key index and exit index from
the harness's own record, not from `recover`'s answer. They are the same values `recover` returned for every
user (`results/recovery.json`: claimable index 2 and unreleased exit index 1 for all 15).

| | Private accounts (10 users) | Public accounts (5 users) | Total |
|---|---|---|---|
| Deposits found / expected | 30 / 30 (20 claimed, 10 claimable) | 15 / 15 (10 claimed, 5 claimable) | **45 / 45** |
| Exits found / expected | 20 / 20 (10 released, 10 unreleased) | 10 / 10 (5 released, 5 unreleased) | **30 / 30** |
| Positions found that were not made | 0 | 0 | 0 |
| Claim from the restored wallet | 10 / 10 applied (private claim, one-time address) | 5 / 5 applied | **15 / 15** |
| Release from the restored keys | 10 / 10 paid | 5 / 5 paid | **15 / 15** |
| `recover` afterwards | every deposit claimed, every exit released | the same | |

Files: `results/recovery.json` (per user: found, expected, what `recover` returned, the directory file counts
before and after), `results/recovery-wipe-examples.txt` (the directory listings before and after the delete
and the restore output, for one private and one public user; local paths replaced by `<run>`).

**Paths covered:** private claim (one-time address, sponsored), private burn (seed-derived release address),
public claim, public burn, and release from a seed-derived exit secret. A private account that already held
state (a claimed and burned note, a fee balance) was restored and made a new private claim with no local state.
The gate run before Part A did the same for one private user by hand (2/2 deposits, 1/1 exit, then a claim and
a release from the restored wallet).

**Not covered, by design:** deposits whose secret is not seed-derived are not found by the scan: deposits made
on the deposit page (a random secret in a claim file: `DESIGN.md` §10) and deposits made with a raw `--secret`
(the c2 control cohort, not in Part B). Exits burned to an address the user typed (`burn --eth-dest`) need that
address named again (`recover --eth-dest`); none were used here. Recovery time: the 15 restores and full scans took 84 s in all
(16:24:03 to 16:25:27; about 190 LEZ blocks by then, 13 accounts and 5,205 candidate positions per wallet; in
`a20`, on a longer chain, about 60 s each); `--lez-mode point` (faster, tells the sequencer which nullifiers you
watch) was not used.



## What is kept

The run of record's capture (the adversary's input, frozen after Part A, and the whole chain after Part B), its
ground truth (including each Part B user's directory listings, restore output and `recover` answers) and the
adversary's output are kept on the build host; the seed phrases, keys, wallets and the chain home were deleted
(`harness.py scrub`). SHA-256 of the frozen capture: `lez-blocks.json` `e8ecc6febdcd955a…`, `ethereum.json`
`7ef8e444f378ddcd…`; of `truth/deposits.jsonl` `4160237789ba5c79…`. The files in `results/` here are copies of
the scores, the adversary's pairing and features, the Part B counts and the planted-control summary.

## Mode, and why

A private claim or burn normally needs a real proof (about 10 minutes and 9.5 GB each), and agents never
prove. LEZ rc3 supports risc0's development mode on both sides (its `Justfile` runs the sequencer and its
integration tests with `RISC0_DEV_MODE=1`; risc0-zkvm 3.0.5's `LocalProver` then uses `DevModeProver`, and
`Receipt::verify` accepts a `FakeReceipt` only when the variable is set; neither the CLI nor the sequencer
binary is built with `disable-dev-mode`). So:

- the sequencer was started with `RISC0_DEV_MODE=1` (checked in its process environment by the harness);
- every wallet call ran with `RISC0_DEV_MODE=1` and without `RISC0_PROVER`, under `lez-bridge/scripts/guard.sh`
  with a 3 GB footprint cap (a real proof needs about 9.5 GB) and a timeout;
- the privacy circuit still runs in risc0's executor, so **the public outputs in the block are the real ones**
  (boundary messages, one-time addresses, nullifiers, commitments, fee amounts, the program-image claims); only
  the receipt is a development receipt;
- the first private claim (gate run) applied in 13 s at an 11 MB peak footprint; its block was 30,666 bytes
  against 711 for an empty one, i.e. the dev stub of about 24 KB that LEZ's own benchmark notes (a real receipt
  is about 223 KB, the same for every private transaction).
- Outbound: G2-lite updates (TrustedTip guest) and G3 releases were executed by `outbound_host`, not proved, and
  accepted by the vault's test MockVerifier, as in `lez-bridge/scripts/e2e-roundtrip.sh` and the judge demo.

Binaries (build host copies, none built for this test): the lez-bridge CLI built from `00ad102`, whose
`lez-bridge/` tree equals main's at `76e71e2`; the unpatched rc3 `sequencer_service`; the v4 canonical
`outbound_host` (`7bb6fdec…`).

## Commands (build host; `<W>` = the work directory holding `bin/` and `src/`, `<run>` = `<W>/runs/<name>`)

```sh
python3 <W>/src/lez-bridge/scripts/privacy/harness.py --run <run> --n 20 --controls 5 --seed 20261119 all   # Part A
cp -r <run>/capture <run>/capture-frozen                                                                   # the adversary's input, frozen
python3 <W>/src/lez-bridge/scripts/privacy/adversary.py <run>/capture-frozen <run>/adv
python3 <W>/src/lez-bridge/scripts/privacy/score.py <run> <run>/adv > <run>/adv/score.md
python3 <W>/src/lez-bridge/scripts/privacy/harness.py --run <run> recovery                                  # Part B
python3 <W>/src/lez-bridge/scripts/privacy/plant.py <run> <run>/capture-frozen <run>/capture-planted     # synthetic controls
python3 <W>/src/lez-bridge/scripts/privacy/harness.py --run <run> capture                                   # whole chain after Part B
python3 <W>/src/lez-bridge/scripts/privacy/harness.py --run <run> stop
python3 <W>/src/lez-bridge/scripts/privacy/harness.py --run <run> scrub                                     # delete keys, wallets, chain
```

The run of record (`a2`) ran Part A (`all`) and Part B (`recovery`) in one go with the final scripts
(`results/part-a-run.log`, not exported). The earlier `a20` needed four resumes after harness fixes (a cast
flag, the USDC deposit fee, anvil's finalized-block lag, fee funding for named-recipient claims), and its relayer
was anvil's public test account 1; the harness now makes a fresh relayer key per run. After a run, `harness.py
... stop` and then `scrub` delete the seed phrases, keys, wallets and chain home and keep the capture, the truth
and the results.

## What this does not show

- **Real proofs.** Every private transaction carried a development receipt. Its public outputs are what a real
  proof commits to, but the receipt bytes are a stub (about 24 KB instead of about 223 KB). Nothing here checks
  that a real receipt carries no extra linking data; it is one constant-size succinct receipt per transaction in
  LEZ's design, the same for every user.
- **Real network timing or a real crowd.** One host, a 15-second block time, 20 users in the main set, each user's
  actions in separate phases. A user who claims right after depositing, or whose timing habits differ from
  the crowd's, is not modelled. 20 is a small anonymity set; on a real network the set is every deposit of the
  same token and denomination under the claimed root.
- **Network-level leakage.** IP addresses, RPC request patterns, the sequencer seeing a wallet's reads, the
  relayer seeing the release request: out of scope here (the RFP's own exclusion), and not measured.
- **The trustless root source.** Claims used the stand-in attestor; an RFP-021 claim publishes an execution
  anchor instead of a root PDA (the same kind of bound, `DESIGN.md` §8.2).
- **Finality and the outbound proof.** The G2-lite update is the TrustedTip guest, executed, accepted by a
  mock verifier; nothing about LEZ finality is shown.
- **Other wallets.** Only the `lez-bridge` CLI; the Names Wallet path and the deposit page are not tested here.
