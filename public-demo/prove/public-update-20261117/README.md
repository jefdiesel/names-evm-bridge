# A Sepolia finality update as an ordinary public transaction (2026-11-17)

**Result.** A real Sepolia light-client finality update (506 of 512 signers) was verified by the
RFP-021 attestation program **inside one public LEZ transaction**, executed by the sequencer
itself, with **no proof anywhere**: block 46 of a **local** LEZ v0.4.0-rc3 chain, 9,285,563 user
cycles (92.9% of the 10M per-transaction cap). The store's finalized header moved from the `Init`
checkpoint (Sepolia slot 11599904) to slot 11599936 and the new anchor was recorded.
A second real update (also 506/512, 9,285,383 cycles) followed the same way in block 71 (slot 11599968,
`updates 2`), and a consumer used its anchor at once: the reference probe proved, in a public
transaction (block 72), that the live Sepolia v2 vault's `isDepositRoot[root]` is 1 under the
anchor this public update recorded.

State label (`DESIGN.md`): **(1) run on a chain: a local LEZ sequencer** on the build host,
loopback only. Not the public demo chain, not a Logos network. Branch `public-update`.

## What changed in the program

- `Instruction::UpdatePublic { config, prev, current_committee, keys, update }`
  (`lez-bridge/attest/core/src/lib.rs`), handled behind eth_attest's feature `public-update`
  (`lez-bridge/attest/program/src/lc.rs` `verify_finality_public`, `fast::verify`).
- Every Helios check runs, unchanged, through the vendored Helios 0.12.0's one added function
  `verify_generic_update_with` (`lez-bridge/attest/vendor/helios-consensus-core/SPIKE-PATCH.diff`).
  Only the sync-committee signature check is ours, exactly as in the lcbench spike
  (`DESIGN.md` §9.3a): RISC Zero's blst fork; the signers' key is the committee's aggregate minus
  the non-signers, from uncompressed key witnesses that must be on the curve, not infinity, and
  compress to the committed bytes of the committee witness; the signature is decompressed and
  subgroup-checked once (Helios's redundant `sig * r` check is not repeated).
- The committee is still bound by today's flat hash (`committee_hash`), so the store format and
  `Effect::Advance` are unchanged. Helios's store gets an empty committee (the keys reach the check
  only through the hook), so the 24,624-byte witness is hashed but never decoded.
- The result is the same `Effect::Advance { prev, next }`, built only right after the
  verification succeeded, and the store applies it with the same checks as for `Update`: the
  sender is this program, `prev` equals the store's state field for field, `next` advances it
  (later finalized slot), same instance and chain.
- Refused on this path (they stay on the private `Update`): a store that knows its next committee,
  an update signed outside the store's period, an update finalized in the next period, any rotation.
- Guest: `lez-bridge/methods/attest_guest_public` (the feature on, Helios and blst patched). It
  needs RISC Zero's C toolchain for riscv32im, so it is built only on the build host
  (`lez-bridge/attest/cycles/build-public.sh`). `methods/attest_guest` keeps the feature off.

## Cycles

Executor (RISC Zero 3.0.5, user cycles, which LEZ meters as execution gas), every program call of
the one transaction, summed. Raw output: `cycles-fixtures.txt`, `measure-update1.log`.

| Update | Signers | Submitter's actor | Store `Advance` | `RecordAnchor` | **Whole transaction** | of 10M |
|---|---:|---:|---:|---:|---:|---:|
| Sepolia fixture (`attest/fixtures/sepolia`) | 509/512 | 9,194,729 | 39,510 | 18,130 | **9,252,369** | 92.5% |
| mainnet fixture (two anchors) | 507/512 | 9,447,607 | 46,809 | 18,236 + 18,234 | **9,530,886** | 95.3% |
| **Real Sepolia update 1, sent in block 46** | **506/512** | 9,227,923 | 39,510 | 18,130 | **9,285,563** | **92.9%** |
| **Real Sepolia update 2, sent in block 71** | **506/512** | 9,227,743 | 39,510 | 18,130 | **9,285,383** | **92.9%** |

Today's private path on the same kind of update is 37.2M (`DESIGN.md` §9.3), proved in about 2 h.

**As the sequencer charged it.** A public fee is exec gas × exec base fee + wire bytes × storage
base fee. The wire length of each transaction was read back from the sequencer (`getTransaction`,
`txlen.log`): **27,773 bytes** for both updates (method check: it gives 26,286 for the `Init` of
block 35, the CLI's own count there).
- Block 46 (update 1): fee 74,506,688 at base fees 8 / 8 (`send-update1.log`: payer balance
  99,999,904,475,784 → 99,999,829,969,096), so exec gas = (74,506,688 − 27,773 × 8) / 8 =
  **9,285,563**, exactly the executor's sum.
- Block 71 (update 2): fee 74,505,248 at 8 / 8 (`send-update2.log`), so exec gas =
  **9,285,383**, again exactly the executor's sum.

So the sequencer metered the transaction at the executor's user-cycle sum. (rc3 blocks carry no
gas receipt; this is derived from the payer's balance and the fee state read just before and after,
with no other transaction from that payer in between.) The wallet's dry run (`dry-update1.log`,
which also holds a first attempt with the older CLI that has no `raw-call`) reported only
"simulated and not sent", no cycle count. Each transaction declared the cap (`--gas-limit
10000000`; the CLI's default is 2,000,000) and was included and **applied, not reverted** (`finish`
checks the block's revert flag; a reverted public transaction pays its whole limit, 80,000,000
here, and these paid 9.29M × 8 + bytes).

## What ran (build host, all loopback, 2026-11-17 EST)

| Step | LEZ block | tx | Notes |
|---|---:|---|---|
| Chain: `chain-rc3.sh setup` + `start`, fresh operator | 1 | — | RPC 127.0.0.1:3460, metrics 3461; sequencer `sequencer_service.rc3` (sha256 `bd3a59dd…`, the demo chain's rc3 build with its WAL cap patch, LEZ `v0.4.0-rc3` `bafd07be`) |
| Deploy the program, immutable | 32–34 | `6491cff1…`, `7ead969a…` (last two) | program **`4ApCnXczkTNPMUMisUDAUPNX5Pjn6eEJejsWNnqx7qJb`**; ELF `attest_guest_public.bin` sha256 `07596242e2b81917263baa5ccf4d8392e092d7c28f2bf41a7d61ddf056b7d895`; image id `36873168360c8f4348eea122ce43eddb570a43e79fdd3914ab93869017eecc7d` (`cycles-fixtures.txt`) |
| `Init` at the Sepolia finalized checkpoint, public | 35 | `51afdde22189a207b7817df17c81da5fee72f107e0e66a2e804b50eeca145a20` | slot 11599904 (period 1416), root `0x982d5b5e…e7a7`; instance `0x672612194eb967a11efd248a41029559eadbb1944ef7d598e52e035f597b908c`; 9,107,563 gas (fee-derived) |
| **Update 1, public** | **46** | **`7c9a4115a862ce64b0d8af1a06b8251a0e95ab3f81048df37adf591f64201322`** | signature slot 11600001, attested 11600000, finalized **11599936**, **506/512**; new anchor block hash `0x06cdaa0b58d66154062791858b8565128dd1163ae3c8d44d828f63497fe3af1f` recorded |
| Probe program (reference consumer) deployed | — | — | `D584zdpXbR2ZApP9mXw2MouQjDZEXtcPjLnR3rbEYvDQ` (`deploy-probe.log`) |
| **Update 2, public** | **71** | **`a70c6a8732633519cb38c0f2e52029feb1ecd0690e1d7059394312ff1c445426`** | signature slot 11600034, attested 11600033, finalized **11599968**, **506/512**; anchor `0x5a4de05ed218280cc9ba46096efbf5cef37f980fc88ada68e2136a15c84a7239` recorded; store `updates 2` (`show-update2.log`) |
| Consumer statement under update 2's anchor, public | 72 | `2ef6c04346067d10fb6296b060f0f40fb91720bc6f0956166e651b3fa8335da0` | v2 vault `0x8C8b…1b74` `isDepositRoot[0x43d1e353…aac5]` (mapping slot 83) = 1; Sepolia execution block 12155607 (`eth_getProof`, 9 account + 4 storage nodes); statement id `0x31c4149b…ccf7`; 270,170 gas (fee-derived) (`probe-statement.log`) |

Store read back after update 1 (`show-update1.log`): `updates 1`, finalized beacon slot 11599936,
root `0x8f386a92…fe24`, execution block hash `0x06cdaa0b…af1f`, committee unchanged, next
committee unknown, latest anchor **recorded**.

## Commands

`$W` is the run's work directory on the build host; in the copied logs its absolute path is replaced by `$W/` (nothing else was edited). `ci-equivalent.log` stays private (local paths throughout).

```sh
# build (under the shared build lock, nice 19, 4 jobs) and the fixture measurement
lez-bridge/attest/cycles/build-public.sh            # attest_cycles public; exit 1 at or over the cap
lez-bridge/attest/cycles/build-public.sh test       # tests/public_cycles.rs: pins the counts (±1%) under the cap
lez-bridge/attest/cycles/build-public.sh workspace  # eth_attest --features public-update: native tests + panic texts
# chain (throwaway, loopback)
R=$W/chain SEQ_BIN=$W/bin/sequencer_service.rc3 CLI=$W/bin/lez-bridge.rc3 NETCFG=$W/bin/local.json \
  RPC=127.0.0.1:3460 METRICS=127.0.0.1:3461 public-demo/rc3/chain-rc3.sh (not published) setup   # then: start
public-demo/prove/public-update.sh deploy     # deploy --which attest --elf attest_guest_public.bin (immutable)
public-demo/prove/public-update.sh init       # attest init --checkpoint finalized (public)
public-demo/prove/public-update.sh fetch      # attest update --from <public> --save-dir: verifies natively, saves, sends nothing
public-demo/prove/public-update.sh measure    # attest_cycles public-real: executor sum + the UpdatePublic message
public-demo/prove/public-update.sh dry|send   # raw-call --gas-limit 10000000 [--dry-run] with the store and anchor PDAs
public-demo/prove/public-update.sh show
public-demo/prove/public-update.sh txlen      # wire length of each sent tx, from the sequencer
# a consumer under the new anchor (the run used one directory, slot-N, for update 2)
public-demo/prove/public-update.sh probe-deploy           # attest-probe + the v2 vault's depositRoot() (cast)
U=update2 public-demo/prove/public-update.sh fetch        # then AT ONCE (eth_getProof ~128 blocks):
U=update2 public-demo/prove/public-update.sh probe-fetch  # prove-slot --fetch-only: header + proof frozen
U=update2 public-demo/prove/public-update.sh measure && U=update2 public-demo/prove/public-update.sh send
U=update2 public-demo/prove/public-update.sh probe        # prove-slot --load-dir: the statement, public
```

The run itself did the probe steps by hand with the same commands (`deploy-probe.log`,
`probe-fetch.log`, `probe-statement.log`); the script stages were added afterwards from them.

The rc3 CLI used for deploy, init, fetch and show is the one of Run 2 (sha256 `700bda76…`); it has
no command for the new instruction, so the message is built by `attest_cycles public-real` from the
saved update and sent with the hidden test command `raw-call` of a later rc3 CLI build
(`lez-bridge.rawcall`, sha256 `5316f238…`, the send-back-relay build). No CLI was rebuilt.

## Tests

- `lez-bridge/attest/program/tests/public_update.rs` (5 tests, native, **in CI** with
  `--features eth_attest/public-update`): the public path gives the same store and accounts as
  the private one; mainnet fixture; flipped signature, cleared participation bit, altered finalized
  header, missing/extra/reordered witness, a signer's key at a non-signer's index, a wrong
  aggregate and a wrong committee witness are refused and change nothing; a store a period behind
  or with a known next committee is refused; only the submitter's own signed actor runs it.
  The existing 18 + 18 attestation tests and the panic-text test pass with the feature on.
- **CI:** the public repository's CI runs on this branch and passes (the first attempts, in the working repository, were refused by GitHub over account billing; CI's cargo command had passed on the build host as a stand-in).
- `lez-bridge/attest/cycles/tests/public_cycles.rs` (**build host only**, not in CI: the guest
  needs RISC Zero's C toolchain): runs `attest_cycles public` and fails if either fixture's whole
  transaction reaches 10M or moves more than 1% from 9,252,369 (Sepolia) / 9,530,886 (mainnet).
  Passed on the build host 2026-11-17 (`cargo test --release`, 0.91 s).

## Participation sweep (executor, synthetic committee)

Executor only (no chain, no proof), on the deployed public guest (image `36873168…cc7d`). The
recorded Sepolia finality update is kept byte for byte (attested and finalized headers, finality
branch, signature slot); only the **sync committee, the participation bits and the aggregate
signature are synthetic**: 512 test keys from fixed public seeds
(`SHA256("RFP021/synthetic-committee/v1" ‖ i)`, nothing secret), the store's committee commitment
set to that committee's hash. A finality update carries no committee branch, so no header, branch
or state root changes. Each update is signed by exactly N members over the signing root Helios
computes, checked natively by the program's own `verify_finality_public`, then executed: the
submitter's actor, the store's `Advance` and `RecordAnchor`, summed as one transaction. Non-signers
are spread evenly over the 512 positions. Raw output: `sweep.txt`.

```sh
lez-bridge/attest/cycles/build-public.sh          # build (lock, nice 19, 4 jobs) + the fixture run
lez-bridge/attest/cycles/build-public.sh run sweep    # attest_cycles sweep (src/sweep.rs)
```

| Signers N/512 | Submitter's actor | Whole transaction | Of 10M | Fits | Begin (composed) | Finish (composed) |
|---:|---:|---:|---:|---|---:|---:|
| 512 | 9,161,483 | 9,219,123 | 92.19% | yes | 5,580,346 | 3,638,777 |
| 506 | 9,227,923 | 9,285,563 | 92.86% | yes | 5,646,786 | 3,638,777 |
| 480 | 9,511,483 | 9,569,123 | 95.69% | yes | 5,930,346 | 3,638,777 |
| 469 | 9,633,854 | 9,691,494 | 96.91% | yes | 6,052,717 | 3,638,777 |
| 450 | 9,841,590 | 9,899,230 | 98.99% | yes | 6,260,453 | 3,638,777 |
| 445 | 9,895,589 | 9,953,229 | 99.53% | yes | 6,314,452 | 3,638,777 |
| 443 | 9,917,388 | 9,975,028 | 99.75% | yes | 6,336,251 | 3,638,777 |
| 442 | 9,928,798 | 9,986,438 | 99.86% | yes | 6,347,661 | 3,638,777 |
| **441** | 9,939,182 | **9,996,822** | 99.97% | **yes (last)** | 6,358,045 | 3,638,777 |
| 440 | 9,950,587 | 10,008,227 | 100.08% | NO | 6,369,450 | 3,638,777 |
| 430 | 10,059,447 | 10,117,087 | 101.17% | NO | 6,478,310 | 3,638,777 |
| **427** | 10,097,830 | 10,155,470 | 101.55% | NO | **6,516,693** | **3,638,777** |
| 420 | 10,174,699 | 10,232,339 | 102.32% | NO | 6,593,562 | 3,638,777 |
| 400 | 10,392,605 | 10,450,245 | 104.50% | NO | 6,811,468 | 3,638,777 |
| **342** | 11,037,115 | 11,094,755 | 110.95% | NO | **7,455,978** | **3,638,777** |

- **Lowest N that fits one transaction: 441/512** (3,178 cycles of margin). From 440 down it needs
  the two-step split or the private path. Each non-signer costs about 11.0K cycles (between 512 and
  342: 1,875,632 / 170 = 11,033).
- **Cross-check:** the synthetic 506/512 run gives a submitter's actor of 9,227,923 and a whole
  transaction of 9,285,563, exactly the real Sepolia update 1 (also 506/512) in block 46.
- **Begin and Finish are COMPOSED, not built:** Begin = submitter's actor − 3,581,137 (blst's final
  exponentiation + `is_one`, measured with the same blst fork in
  `lez-bridge/attest/lcbench/results/run-20261114T175725Z.txt`); Finish = 3,581,137 + `Advance` +
  `RecordAnchor`. Neither includes the job account's write (Begin) or read and check (Finish), and
  a real split would also carry the Miller-loop result between the steps. At 427/512: Begin about
  6.52M, Finish about 3.64M; at 342/512: Begin about 7.46M, Finish about 3.64M; both under the cap
  with room for the job account.
- The message grows with the non-signers (96 + 2 bytes each): 26,659 bytes at 512/512, 43,319 at
  342/512 (`sweep.txt`).

## Review, 2026-11-17

1. Reviewed once by a separate AI agent; **not audited**. The review found no way to advance the
   store to a header the sync committee did not sign.
2. The per-key G1 subgroup check is not repeated because the consensus specs validate every
   sync-committee key when the aggregate is formed (`eth_aggregate_pubkeys` asserts `KeyValidate`,
   `specs/altair/bls.md`).
3. Open: RISC Zero's blst fork has only run the honest path in the zkVM here; the vendored Helios
   was not diffed against upstream beyond `SPIKE-PATCH.diff`.
4. Known liveness issue: a public finality update can make a private period-update proof, built
   on the older store state, stale (the store requires its current state to equal `prev`). An
   updater must therefore pause public updates while a private proof is in flight, or the store
   must accept a committee-only `Advance`. Left to M3.
5. Known low item: the key-witness decoder does not reject a witness whose first byte has flag
   bits set. Such a witness decodes to the same point, so there is no soundness effect; the fix is
   `if unc[0] & 0xe0 != 0 { return None }`. Left unfixed so the branch head keeps reproducing the
   deployed image.
6. Tests not yet written: infinity and off-curve witnesses, an index of 512 or more, an infinity
   signature, a signature outside G2, participation below policy on the public path, 512/512
   participation, and cycles at low participation.

## Not done, and limits

- **Period updates** (committee handoff) and **low participation** were built after this record, the same day: see `../public-handoff-20261117/README.md` (the two-step period update and the handoff), `../finality-split-20261117/README.md` (two steps down to 342 of 512 signers, synthetic committee) and `../catchup-20261117/README.md`. One transaction fits down to about 442 signers (estimate).
- **The transaction fills a block:** the block cap is also 10M, so nothing else fits beside it.
- **No per-key G1 subgroup check** on the witnesses: see Review, point 2.
- **Audit surface:** the hook, the witness check and the blst FFI wrapper are ours; Helios's
  update logic is its own.
- **Not run on the public demo chain** or any Logos network; the deployed attestation program on
  the demo chain is unchanged. The fixtures and the real update are Sepolia (Gloas); mainnet only
  as a fixture in the executor.
- The sequencer's gas is fee-derived (above, exact given the wire length read back); rc3 has no gas receipt.
- **Reproducing the deployed ELF: the branch head reproduces it** (commit `aa74046`; first built from `1efe784`, whose program sources are identical). Rebuilt 2026-11-17 on the build host from `git archive aa74046`, every source touched, at the same path as the deployed build (`$W/src/lez-bridge`; guest builds embed their directory), with `build-public.sh`: ELF sha256 `07596242e2b81917263baa5ccf4d8392e092d7c28f2bf41a7d61ddf056b7d895`, image id `36873168360c8f4348eea122ce43eddb570a43e79fdd3914ab93869017eecc7d`, identical to the deployed ELF. (Commit `acc9673` had 6 extra doc lines in `attest/program/src/lib.rs`, which shift panic line numbers and gave image `0075ead0…1ede`; they were removed again. Their content: with blst and key witnesses an in-period finality update costs about 9.3M cycles for the whole transaction, so the sequencer verifies it in public execution; period updates and rotations stay on the private `Update`.)
- Image id: printed by `attest_cycles` after the later test build, whose guest ELF still hashes to `07596242…`, the deployed file's sha256 (`elf.sha256`); it was not read from the chain.
- **A consumer needs `eth_getProof` within about 128 blocks:** the probe under update 1's anchor
  could not be made (`probe.log`: PublicNode and Tenderly answer "distance to target block exceeds
  maximum proof window"), so the vault proof was frozen right after update 2 was fetched
  (`probe-fetch.log`) and used once update 2 landed. An archive node removes that constraint.
- The CLI has no `--public` update command yet; the message came from the executor harness.
