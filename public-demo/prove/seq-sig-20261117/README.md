# Sequencer signatures, run locally (2026-11-17)

This is the withdrawal route the proposal calls "sequencer signatures". The Ethereum vault takes a LEZ
state update when a threshold T of a fixed set of n signer keys has signed the LEZ block the update
ends at. Anyone may submit the update; there is no submitter key and no admin. Branch `seq-sig`.

Revision 3 follows two independent reviews:
- The first found the threshold rule sound but one key able to stall every update for good, so the
  contest rule is now equivocation-based.
- The second found that the gate's global effects could be driven through a fake vault, so the
  gate now serves exactly one vault.

**State: (1) on throwaway local chains only** (a local LEZ v0.4.0-rc3 chain and anvil), and (2) in
forge and Rust tests. **Mock-verified: proofs executed, not proved.**
- The vault's verifier is the gate, in front of the test `MockVerifier`.
- Every G2, evidence and G3 journal below comes out of RISC Zero's executor.
- Nothing was proved, and nothing ran on Sepolia or the public demo chain.

**The signers are STAND-INS.** They are five Ed25519 keys of ours, made by `seqsig keygen`: four
form the set and one is an outsider. They sign a checkpoint with the attestor command. They are not
LEZ sequencer keys, for two reasons:
- LEZ v0.4.0-rc3 standalone block headers carry no signature;
- these chains have no Logos Blockchain node, so the signatures real LEZ sequencers put on their
  inscription transactions do not exist here.

## What a signature must vouch for

The G2 guest takes private transactions on the sequencer's word (`SequencerTrusted`, as the
TrustedTip build does). A signer that only attests "this block was published" would therefore let
one block producer's made-up private burn through.

A signer must re-execute blocks `prev+1..N` and check every privacy receipt before it signs.

`seqsig attest` refuses to sign unless it has replayed the range itself. The replay is
`outbound_host::prepare` run natively:
- it recomputes every block hash and checks the `prev_block_hash` chain;
- it extracts every claim, burn, root and registry change as the bridge program would, and builds
  the claim accumulator;
- the replay's cfgId must equal the one it signs for, and it must end at block N.

It writes the replay's result (claims, exits, new G2 state) next to the signature. What it still
does NOT check:
- **Privacy receipts.** LEZ rc3 makes them with risc0 3.0.5, which this risc0 3.1 host refuses
  (advisory GHSA-g6qg-9j8p-7j87).
- **Public execution.** LEZ never proves it.

A production signer must cover both, for example by being a sequencer that verified each
transaction on admission.

Two more rules hold before the attestor signs:
- **Finality.** These chains have no Bedrock finality, so the rule is depth: block N must be at
  least D blocks under the sequencer's head (`--final-depth`, default 20, the depth the public demo
  updater uses; the run uses 3).
- **Sign once.** The attestor keeps an append-only record of every checkpoint it signed, keyed by
  (cfgId, prevId, prevHash, blockId) and written to disk before the signature exists. It refuses to
  sign another hash for a key already in the record.
- **Test flag.** `--test-conflicting-hash` signs without the record. It stands for a dishonest signer.

## The contest rule

The update journal commits:
- the signer set hash and the threshold;
- the gate's ejected mask;
- a **bitmap of the set keys that signed**;
- the checkpoint `(prevId, prevHash, blockId, blockHash)`.

Evidence (the evidence guest) proves which set keys signed some checkpoint, in the same message format.

| Evidence against a pending update (same cfgId, prevId, prevHash and blockId; another blockHash) | Effect |
|---|---|
| Signed by a key that IS in the update's bitmap | **Equivocation.** The key is ejected for good (`ejected` in gate storage). If fewer than T of the update's signers remain active, the update is cancelled and its checkpoint can never be staged again; otherwise it stays staged and finalizes |
| Signed by at least T keys that are not ejected | **Two quorums conflict.** The gate halts for good: no submit, finalize or contest. Releases of exits applied before still pay |
| Signed only by keys NOT in the bitmap | Cancels nothing (`NoEquivocation`) |
| Over another previous tip (another history before the update's start) or another block id (evidence signed in advance) | Not a conflict (`NotTheSameCheckpoint`). A reorg AFTER the previous tip (same previous tip and block id, other hash) IS a conflict |

Ejection rules:
- An ejected key's signatures never count again: the guest refuses them, and the gate refuses a
  bitmap that includes it.
- THRESHOLD stays measured against the original n.
- When fewer than T keys remain active, the gate is dead: `submit` refuses (`GateDead`).
- The anchor stays the immutable set hash, which the gate computes from the keys passed to its
  constructor.

**One vault per gate.** `VAULT` and its `CFG_ID` are immutables. The vault's constructor needs the
gate's address, so the deploy order is:
1. deploy the gate with the vault's predicted address (the deployer's next CREATE address);
2. deploy the vault;
3. check that the vault landed at that address and has that cfgId (`script/SeqSigDeploy.s.sol`).

The gate computes `CFG_ID` itself with the vault's formula and compares journals and evidence with
it, never with a live call. `submit`, `contest` and `finalize` take no vault argument. `verify`
passes the update image only for `VAULT`.

**A dead or halted gate leaves the vault taking deposits.** The lever that works is for the vault's
freezer to freeze DEPOSITS (`freezeAll(true, …)`); a deployer then brings up a new gate and vault
version. Two things are unrecoverable until the failsafe vault of `DESIGN.md` §10 exists:
- deposits that were never claimed;
- exits burned on LEZ but never applied to the vault.

## Safety and liveness

The parameters are:
- n keys and a threshold T with n/2 < T ≤ n − 1;
- f dishonest keys;
- o honest keys offline.

An honest key signs only a checkpoint it has replayed and considers final, and its record keeps it
from signing two hashes for one (cfgId, previous tip, block id).

- **Safety.** An update over blocks the honest keys did not vouch for needs T valid signatures from
  keys that are not ejected. It is impossible while **f ≤ T − 1**.
- **Halt.**
  - With one LEZ history, honest keys sign no conflict, so a halt needs **f ≥ T** dishonest keys on
    the conflict.
  - If honest keys could be shown different forks (an equivocating sequencer serving different
    blocks to different signers), honest signatures could conflict too. Then a halt needs only
    f ≥ 2T − n. In that case f ≤ n − T does not prevent a halt when T ≤ 2n/3; for example n = 5,
    T = 3, f = 2.
- **Liveness.** Updates can be signed while **n − f − o ≥ T**.
  - An update is cancelled only when an equivocation leaves it with fewer than T active signers.
  - Every equivocator is ejected.
  - So at most f updates are ever cancelled at this gate (one vault per gate).
  - An ejection invalidates only in-progress proofs that counted the ejected key, which must then be
    redone.
- **Both together** hold for **f + o ≤ n − T**. This also gives f ≤ T − 1, since T > n/2, so the
  earlier min(T − 1, n − T) is just n − T. Then updates finalize after at most f cancellations,
  each costing one contest window.
- **The run's case**, n = 4 and T = 3, gives f + o ≤ 1.
- **When safety is lost.** With f ≥ T, false updates can be signed. Honest keys can stop them only
  if the dishonest quorum also equivocated on a checkpoint an honest observer holds; that is not
  guaranteed.

## What was built

| Part | Where | What it does |
|---|---|---|
| Checkpoint format and checks | `lez-bridge/outbound-seqsig/core` (`seqsig_core`) | Builds the message `SHA256("RFP022/seqsig/checkpoint/v1" \| cfgId \| u64be(prevId) \| prevHash \| u64be(blockId) \| blockHash)`. Builds the set hash `SHA256("RFP022/seqsig/set/v1" \| u32be(n) \| keys)` over keys that are strictly ascending, canonical and of large order. Checks the threshold rule n/2 < T ≤ n, counts distinct non-ejected signers, and builds the evidence journal. Ed25519 is `ed25519-dalek` 2.2.0 `verify_strict`, which also refuses a non-canonical S |
| Attestor and driver | `lez-bridge/outbound-seqsig/host` (`seqsig`) | `keygen` (mode-600 key file; prints only the public key), `set`, `attest` (replays, then signs), `update` (native G2, the native check, then the seq-sig guest and the TrustedTip guest in the executor; `--ejected` mask), `evidence` (one or more signatures over one checkpoint) |
| Update guest | `lez-bridge/methods/outbound_guest_seqsig` (new image) | Does exactly what the TrustedTip guest does (`update::<TrustedTip>`), then the signature check over the checkpoint it derives from its own input. Journal: the vault's `abi.encode(UpdateJournal)` followed by `abi.encode(signerSetHash, threshold, ejectedMask, signerBitmap, prevId, prevHash, blockId, blockHash)` (256 bytes). Its G2 states keep TrustedTip's finality kind (1): only the vault's pinned image id tells the two routes apart |
| Evidence guest | `lez-bridge/methods/seqsig_evidence_guest` (new image) | Checks that every given signature by a set key over one checkpoint verifies. Journal: `abi.encode(cfgId, signerSetHash, signerBitmap, prevId, prevHash, blockId, blockHash)` (224 bytes) |
| Ethereum gate | `vault/src/SeqSigGate.sol` | Bound to one vault (immutable `VAULT`, `CFG_ID`). `submit`, `contest`, `finalize`, as above. The constructor takes the keys, recomputes the set hash, and refuses unsorted keys, non-canonical encodings (y ≥ p) and the y values of the small-order points; it also requires n/2 < T ≤ n − 1 and a window of at least `MIN_CONTEST_WINDOW` (6 h); T = n and a shorter window are allowed only with the immutable `TEST_ONLY_SHORT_WINDOW` (visible on chain; the run sets it). `verify` passes the update image only through its own transient mark, passes `RELEASE_IMAGE_ID` to the verifier, and refuses any other image. Runtime 7,999 bytes |
| Vault | `vault/src/BridgeVault.sol`, **unchanged** | Its conservation checks run as before, at `submit` (simulated) and at `finalize`. Exits exist in the vault only after `finalize`, so a release during the window is refused (`UnknownExitRoot`) |

**Off-curve keys.** The gate does not check that a key is on the curve; the guests do. A set holding
an off-curve key gets no journal, so such a gate would take no update.

**Unchanged by this branch:**
- the TrustedTip guest, the release guest, `outbound/core`, `claim-acc`, `bridge`, `token`, `BridgeVault.sol` and `UpdateGatedVerifier.sol`;
- every existing `Cargo.lock` (lock hashes compared before and after);
- `node scripts/check-versions.mjs`: 0 FAIL, with the three new locks listed in `VERSIONS.toml`.

**Sizes** (EIP-170, 24,576 bytes): `SeqSigGate` 7,999 bytes; `BridgeVault` 20,642, unchanged.

## Commands

```sh
# build host, through the lock (nice 19, 4 jobs)
cd lez-bridge/outbound-seqsig
cargo test --release -j 4 -p seqsig_core -p seqsig_host -- --nocapture   # 13 + 9 tests; 8 of the 9 run both guests in the executor
cargo build --release -j 4 -p seqsig_host -p outbound_host
# the end-to-end run (throwaway chain on 3470/3471, proxy port 3475 reserved, anvil on 8570; both stopped at the end)
R=<fresh dir> KIT=<testnet kit>/testnet SEQ_BIN=… WALLET_BIN=… CLI_SRC=<lez-bridge CLI> \
  HOST=<target>/release/outbound_host SEQSIG=<target>/release/seqsig lez-bridge/scripts/e2e-seqsig.sh
# anywhere
cd vault && forge test --match-contract SeqSigGate    # 34 tests
```

The run used:
- the testnet kit's sequencer and wallet (LEZ v0.4.0-rc3 plus the kit's patches, at the judge demo's
  pinned kit commit);
- a `lez-bridge` CLI built from the fixed programs on 2026-11-14 (prebuilt, not built from this commit);
- `outbound_host` and `seqsig` built from this branch.

## Cycles (RISC Zero executor, user cycles)

The guests check signatures with RISC Zero's `curve25519-dalek` 4.1.3 fork at a pinned rev, which
uses its bigint accelerator (patched in the two guests' `Cargo.toml`).
- The `crypto-bigint` fork enters the update guest only through it; the TrustedTip guest's lock has
  no `crypto-bigint`.
- The stock crate was measured first. To reproduce that column, remove the two patch lines from both
  guests' `Cargo.toml` and run the host tests again.

| Measurement (recorded batch `public9`, `outbound-seqsig/host/tests/seqsig.rs`) | Stock crate (revision 1) | Fork (in the branch) |
|---|---|---|
| One more signature (4 of 4 minus 3 of 4) | 3,528,923 | 902,942 |
| Signature check, 3 signatures (rev. 2 includes checking the 4 set keys) | 10,448,514 | 2,960,400 |
| Whole seq-sig update, 3 of 4 | 11,228,975 | 3,743,910 |
| Evidence guest, one signature (rev. 2 includes the set-key checks) | 3,544,404 | 1,205,042 |

Revision 2's set-key checks (decompression, canonical encoding, small order) add about 0.27M cycles
for 4 keys.

Run `try8` (revision 3), figures from `run-summary.txt`. Bitmaps are over set indexes: k0 = 0,
k1 = 1, k2 = 3, k3 = 2.

| Update in the run | LEZ blocks | Signers (bitmap) | Ejected mask | Seq-sig guest | TrustedTip guest, same batch | Signature check |
|---|---|---|---|---|---|---|
| u1 (applied) | 8..94 | k0, k1, k3 (`0111`) | `0000` | 8,439,173 | 5,452,959 | 2,988,695 |
| u1, 4 of 4, threshold 4 (refused at the gate) | 8..94 | all (`1111`) | `0000` | 9,331,284 | 5,452,959 | 3,879,962 |
| u2 (cancelled: k1 equivocated) | 95..125 | k0, k1, k2 (`1011`) | `0000` | 3,655,828 | 653,430 | 2,998,739 |
| u2 again without k1 (refused: refuted) | 95..125 | k0, k2, k3 (`1101`) | `0010` | 3,653,897 | 653,430 | 2,996,808 |
| u3 (applied) | 95..131 | k0, k2, k3 (`1101`) | `0010` | 3,740,167 | 772,942 | 2,964,080 |
| u4 (pending at the halt) | 132..157 | k0, k2, k3 (`1101`) | `0010` | 3,505,312 | 537,799 | 2,963,500 |

| Evidence | Signatures | User cycles |
|---|---|---|
| k3's conflicting hash for block 125 | 1 | 1,199,622 |
| k1's signature on the real block 125 | 1 | 1,206,117 |
| k1's conflicting hash for block 125 | 1 | 1,206,898 |
| k0, k2 and k3 on a conflicting hash for block 157 | 3 | 2,981,497 |

Each signature adds about 0.89M user cycles: 3,879,962 − 2,988,695 = 891,267.

## The end-to-end run

`lez-bridge/scripts/e2e-seqsig.sh`, run `try8` on the build host, 2026-11-17, 15:59 to 16:07, ended
`ALL STEPS PASSED`. Its transcript, cut to the steps, is `run-summary.txt`. The setup was:
- a throwaway LEZ rc3 chain (the kit, LEZ's published genesis test key, 3 s blocks) on 127.0.0.1:3470;
- anvil (chain 31337) on 127.0.0.1:8570;
- threshold 3 of 4, release delay 0;
- contest window 60 s, under the gate's 6 h minimum, so the gate was deployed with
  `TEST_ONLY_SHORT_WINDOW = true` (readable on chain).

**Deploy.** The gate was bound to the vault's predicted address, and the deploy script checked that
the vault landed there with the gate's `CFG_ID`. The gate computed the set hash from the keys, and it
equals `seqsig set`'s.

**Attestors.** Every checkpoint was signed only after the attestor replayed the range, waited until
the block was 3 under the head (`--final-depth 3`), and wrote it to its sign-once record.

**Accounts** are anvil's public test accounts:
- `0x90F7…b906` deployed the contracts and is the vault admin;
- `0x9965…A4dc` submitted, and `0x976E…0aa9` finalized, contested and released. Neither has any role.

| Step | Result | Gas |
|---|---|---|
| 3 deposits of 0.01 ETH, claims of all 3 on LEZ (stand-in root attestor), burn of exit 0 | applied | 204,282 / 155,803 / 138,035 |
| Executor: 2 of 4; k1 twice; outsider k4 under index 2; threshold claimed as 2 | guest refused each: `2 distinct valid signers, threshold 3` (twice), `signature for key 2 does not verify`, `threshold 2 not in (n/2, n]` | |
| `submit` of a journal with threshold 4 (all 4 signed) | refused: `WrongThreshold` | |
| `submit` of update 1 with the 4-of-4 run's seal | refused: `BadSeal` | |
| `submit` of update 1 (k0, k1, k3; blocks 8..94) by the no-role account | staged; the vault's `g2State` unchanged | **366,346** |
| `release` of exit 0 during the window; `finalize` during the window | refused: `UnknownExitRoot`; `ContestWindowOpen` | |
| `finalize` after 60 s, by the other no-role account; `release` of exit 0 | applied; paid 0.00995 ETH | **210,556**; 123,179 |
| Burn of exit 1; `submit` of update 2 (k0, k1, k2; blocks 95..125) | staged | **308,570** |
| `contest` with k3's conflicting hash for block 125 (k3 did not sign update 2) | refused: `NoEquivocation`; update 2 still pending | |
| `contest` with k1's signature on the real block 125 | refused: `NoConflict` | |
| k1's attestor re-signs its update-2 checkpoint; then is given a record that says it signed another hash there | signs (same hash); then refuses: `refusing to sign: this key already signed block 125 …` | |
| `contest` with k1's conflicting hash (`--test-conflicting-hash`, without the record) for the same (cfgId, previous tip 94, block 125); k1 had signed update 2 | **equivocation:** k1 ejected (mask `0010`, 3 keys active); update 2 kept only 2 active signers, so it was cancelled (`g2State` still update 1's, exit count 1) | **87,674** |
| `submit` of update 2's journal again (it counted k1) | refused: `BadSignerBitmap` | |
| Executor: k0, k1, k2 with the new mask | guest refused: `key 1 is ejected; its signature does not count` | |
| `submit` of the same checkpoint signed by k0, k2, k3 with the new mask | refused: `Refuted` | |
| Update 3 (k0, k2, k3; blocks 95..131, includes exit 1): `submit`, `finalize` after 60 s; `release` of exit 1 | applied; paid 0.00995 ETH | 308,686 / 152,949; 123,179 |
| Update 4 (k0, k2, k3; blocks 132..157): `submit` | staged | 211,481 |
| `contest` with k0, k2 and k3 (3 active keys, a quorum) on a conflicting hash for block 157 | **two quorums:** the gate halted; update 4 dropped | **55,262** |
| `submit` and `finalize` after the halt | refused: `Halted` | |
| Vault counters (ETH, 0.01) | deposits 3, claims 3, burns 2, releases 2 | |

**Equivocation in the run.** k1 really equivocated: it signed update 2's checkpoint and then a
conflicting hash for the same (cfgId, previous tip, block). A single local sequencer cannot fork, so
the conflicting hash is made up, and an honest attestor's record would have refused it; the test flag
skips the record.

**Why update 2 was cancelled.** It was the honest update, but with k1 ejected it had only 2 active
signers, fewer than T = 3. An update that keeps T active signers after an ejection stays staged and
finalizes (the forge test `test_EquivocatorEjectedButQuorumKeepsTheUpdate`).

**The halt in the run.** k0, k2 and k3 likewise signed both update 4 and a made-up conflict.

**Gas** is anvil's `gasUsed` with the mock verifier.
- `submit` includes the reverted simulation of the vault's `update`.
- Update 1's journal also carries 3 claims and a deposit root, while updates 2 to 4 carry at most
  one exit, so update 1's `submit` and `finalize` cost more.

## Tests

| Where | Count | What |
|---|---|---|
| `outbound-seqsig/core` (Rust, native) | 13 | 3 of 4; 2 of 4; a duplicate counted once; an ejected key's signature refused; an outside key (under a member's index, and out of range); signatures bound to cfgId, previous tip and end block; threshold rule (n/2 < T ≤ n); T = n and the 64-key maximum (65 refused); small-order and non-canonical set keys; a non-canonical S; evidence; the golden vectors (journal tail, evidence journal); RFC 8032 test 2 |
| `outbound-seqsig/host/tests/seqsig.rs` (Rust, both guests in the executor, with RISC Zero's curve fork, on `public9`; and the attestor's record) | 9 | 3 of 4 accepted, journal = TrustedTip journal + tail with bitmap; 2 of 4 refused; a duplicate counted once (refused); an outside key refused; another cfgId, history or end block refused; an ejected key refused and the mask committed; small-order keys (identity, order 8), a non-canonical key encoding and a non-canonical S refused; the evidence guest; the attestor's record refuses a second hash for a checkpoint key it signed |
| `vault/test/SeqSigGate.t.sol` (forge, mock verifier) | 34 | See the list below |

The 34 forge tests cover:
- **Constructor:** the set hash computed from the keys; immutables; T ≤ n/2 and T > n refused; window minimum and the test-only flag; unsorted and duplicate keys; small-order and non-canonical keys (9 encodings); 64 keys with T = 64 and 33, T = 32 refused, 65 keys refused.
- **Cross-language vector:** the gate's set hash equals `seqsig set`'s for revision 1's run keys.
- **Accepted path:** 3 of 4 from an account with no role, finalized by another account, then released.
- **Refused journals:** threshold 2 (and 4); a bitmap below T or outside the set; another set hash; the seal binds the tail; a TrustedTip-image seal; wrong cfgId; short journal; replay of an old checkpoint and of `finalize`; no state advance; one pending update at a time.
- **Gate plumbing:** direct `update` refused; `verify` passes only the release image; the transient mark is cleared in-transaction and cannot be lent to another vault; a vault with another verifier.
- **Window and freeze:** release refused during the window and paid after; freeze and unfreeze between submit and finalize.
- **Contest:** equivocation ejects the key and cancels the update (vault unchanged, exits never releasable, checkpoint refuted); a conflict from a non-signer cancels nothing; evidence for a future block id or another previous tip is useless; invalid evidence refused (same hash, set, cfgId, no proof, empty bitmap, wrong length, after the window, nothing pending).
- **Griefing bounded by ejection:** a stale mask is refused; an ejected key in the bitmap is refused; an ejected key's conflict cancels nothing; a second ejection makes the gate dead.
- **Halt:** two quorums halt the gate, and earlier exits still release.
- **One vault per gate:** a second vault, or a fake vault claiming the gate's cfgId and an old state, gets nothing through `verify`; a finalized journal cannot be staged again; a vault at the predicted address with another verifier or another cfgId is refused.
- **Contest edges:** an equivocator ejected while a 4-of-4 update keeps T = 3 active signers and finalizes; halt evidence with 1 ejected + 2 active keys does not halt; a contest at the window's last second; a proof made before an ejection stays valid unless it counted the ejected key; ids above uint64; the golden vectors of the journal tail and the evidence journal (the same bytes as the Rust test).
- **Conservation:** the vault's checks still decide; a 4-round fuzz with claims, exits, contests and releases keeps released ≤ burned ≤ claimed ≤ deposited.

The forge tests cannot count signatures, because counting happens inside the guest: with too few or
invalid signers there is no journal, so no seal. There, journals stand for what the guests would
commit. The counting itself is tested in the two Rust suites and in the run above.

## Known limits and next steps

This is a demo; months remain for the rest. Items the second review raised that were scoped out of
code:
- **No early drop.** There is no separate call to drop a pending update early: with one vault, the
  only path that ejects keys is the contest on that update, and it cancels the update in the same
  transaction when its active signers fall below T.
- **The public-CI pipe-reader job** (`lez-bridge/scripts/test-pipe-readers.sh`) passes over an export of
  the branch (16 shell scripts, 2026-11-17). On the private worktree it fails only on
  `public-demo/demo.sh`, which comes from main and is not exported.
- **Already written in this round, kept because they pass:**
  - T ≤ n − 1 for gates without the test flag;
  - the relaxed mask check: a proof's mask may be a past value of the gate's, and only its signers must be active;
  - the threshold edges (T = n − 1, T = n, n = 2, 3 and 64);
  - a byte-for-byte golden vector of the journal tail and the evidence journal, checked in Rust and in forge;
  - contests at the window's last second;
  - ids above uint64.
- **Next steps:**
  - real sequencer keys and how the guest obtains them;
  - privacy-receipt and public-execution checks in the signers;
  - signer-set rotation;
  - staking and slashing;
  - the failsafe vault;
  - real proofs on Sepolia.

## What is NOT shown

- **Real LEZ sequencer signatures.** The keys are ours. Not built: which keys a real deployment would
  use (the sequencers' inscription keys on the Logos Blockchain, or a key LEZ adds to its headers),
  and how the guest would obtain them.
- **Privacy receipts and public execution.** The signers' replay checks neither, as above.
- **Signer-set rotation.** The set is immutable in the gate; a new set means a new gate and vault.
- **Slashing.** Ejection is the only cost to an equivocating key: it stops counting. Nothing is at
  stake, and a key can still stall one update.
- **Real proofs.** Nothing was proved, and nothing was checked by RISC Zero's Groth16 verifier. The gas
  figures use the mock verifier: a real `submit` and `contest` each add one Groth16 verify (on Sepolia
  the v4 vault's whole `update`, verify included, cost 671,254–899,746 gas, `DESIGN.md` §4).
- **A real contest window.** A contester must PROVE the evidence (about 1.2M cycles) and wrap it to
  Groth16 inside the window. For scale, `DESIGN.md` §4 records a 36k-cycle release proved in 20.9 s
  and wrapped in 243 s; `MIN_CONTEST_WINDOW` is 6 h. The run's 60 s window, with executed proofs,
  shows the mechanism only.
- **Sepolia or the public demo chain.** Nothing here touched either.
- **Forks that end elsewhere.** Evidence counts only against the pending update's own checkpoint
  (same previous tip and block id). A false update whose batch ends at a block id at which the
  honest keys signed nothing is caught only by its T signatures being impossible: that is the
  f < T condition, not the contest.
- **A cancelled checkpoint.** It can never be staged again; the next update must end at another block
  (the run's update 3 does).
- **Status of other documents.** `DESIGN.md` §0, §12 and the older §4 bullet still mark the contest
  window "(4)", for the main design (Logos Blockchain finality).
