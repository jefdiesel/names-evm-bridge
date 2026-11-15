# attest-spike: Ethereum light-client update + storage proof in a RISC Zero 3.0.5 guest

This is the first running code for RFP-021 (trustless Ethereum state attestation for LEZ).
It only measures cycles. **Executor only: nothing here generates a proof.** The host uses
`ExecutorImpl::run()` and reads `Session.{user,paging,reserved,total}_cycles` and the segment
count. There is no prover call anywhere (an unthrottled proof once kernel-panicked the test machine).

- **Guest A** (`methods/guest-consensus`) runs Helios `consensus-core` 0.12.0, unmodified. It
  takes a bootstrap, which must match a trusted checkpoint root, then N `light_client/updates`
  items and one finality update, all as SSZ straight from the beacon API, decoded with Helios's
  own `from_ssz_bytes`. BLS12-381 uses RISC Zero's `bls12_381` fork. The guest commits the
  finalized beacon header root, its slot and the execution commitment.
- **Guest B** (`methods/guest-inclusion`) takes the execution block hash that guest A
  finalized and the execution header RLP. It checks `keccak256(rlp) == hash`, reads the state
  root, and verifies the `eth_getProof` account proof and then the storage proof with
  `alloy-trie`. The proved slot is a real Solidity `mapping(bytes32 => bool)` entry. The guest
  commits (chain id, block hash, number, state root, address, slot key, value).
  `guest-inclusion-sw` is the same source without the keccak accelerator patch.
- Mode 1 of guest A replays the BLS and SSZ steps of the signature check on the same crates,
  so the cost can be broken down with `env::cycle_count()` deltas.

## Answer in one place

- **Does a whole update fit one LEZ public transaction? No.**
  - A finality update within a period costs **36.31M** user cycles, 3.6× the 10M cap.
  - A committee-handoff update costs **44.88M**, 4.5× the cap.
  - The cap is per transaction and also per block, so at least 4 to 5 blocks' worth of
    execution gas are needed per update.
- **Can it be split into staged public transactions along the pairing seam (Miller loop in one,
  final exponentiation in another)? Not with this BLS crate.**
  - The final exponentiation alone is **10.83M**, above the cap. It would itself have to be
    split, or the BLS backend changed.
  - Our pairing is **19.73M** with the zkcrypto `bls12_381` crate. RISC Zero's accelerated `blst`
    fork is reported elsewhere at about 7.85M for the same pairing; that figure was not
    reproduced here.
- **Can it be placed in a private execution? Nothing caps it there.** It would mean proving
  about 36M cycles on the user's device. A verifier using `blst` and the committee's aggregate key
  minus the non-signers' keys is reported at about 14.6M (not reproduced here).
  Proving time was not measured, by rule.
- **The inclusion step is cheap.** One mapping-entry attestation costs **0.26M** with the
  keccak accelerator and **1.01M** with software keccak, 2.6% and 10% of the cap. It fits one
  transaction comfortably.

## Reproduce

The numbers below were measured on an Apple M4 machine (16 GB,
macOS 15.8.2), always under `nice` with `-j 4`.

```sh
./run.sh        # full run (as below)
./run.sh fast   # headline + B + negatives
```

`run.sh` sets `CARGO_TARGET_DIR=$PWD/target` and `CARGO_BUILD_JOBS=4`
(this also caps the nested guest builds that `risc0-build` starts). It runs
`nice -n 19 cargo build --release -j 4 -p host`, stops if the build fails, then runs
`nice -n 19 ./target/release/host fixtures` and tees the output to `results/run-<UTC>.txt`.

Every number below is from **`results/run-20261112T153157Z.txt`**, a run of `./run.sh` on
2026-11-12. The executor takes 0.4 to 3.0 s per scenario. Re-running on the same fixtures
gives the same counts. Rebuilding the guest after a code change can shift counts by about
1K cycles, from code and heap layout.

**The fixtures are a snapshot.** `scripts/fetch_fixtures.py sepolia|mainnet` (run on the
laptop with `python3 -I`; read-only public HTTP, generic user agent) replaces them with
current data, which changes the slots, participation and numbers. The Sepolia header-RLP step
was added to the script after this snapshot was fetched. The snapshot's
`el_block_header.rlp.hex` was written by the same `header_rlp()` function, and its keccak was
checked against the block hash.

## Pins

| What | Version / rev |
|---|---|
| risc0-zkvm (guest and host), risc0-build | `=3.0.5`, matching LEZ `v0.4.0-rc1`; rzup: cargo-risczero 3.0.5, r0vm 3.0.5, risc0 rust 1.97.0 |
| host toolchain | rustc 1.98.1 (`rust-toolchain.toml`) |
| Helios `helios-consensus-core` | git `a16z/helios` tag `0.12.0` = rev `2af9b41887e069144ee56cd710f3ddca195473f8` (2026-10-05). No consensus-core change between 0.12.0 and master `2922a47` (2026-11-10) |
| BLS12-381 (used) | `bls12_381 0.8.0` patched to `risc0/zkcrypto-bls12_381` tag `bls12_381/v0.8.0-risczero.1` = `a3192e1bc58921d42d03d99957abf3be55f9d435`, uses `risc0-bigint2 1.4.14` |
| BLS12-381 (`blst`, found, not built) | `risc0/blst` tag `v0.3.16-risczero.0` = `7d1fc3e6d8cf0d52a291f52455233bf8d6432ed2` (2026-01-23), `risc0-bigint2 ^1.4.11` |
| SHA-256 | `sha2 0.9.9` → `risc0/RustCrypto-hashes` tag `sha2-v0.9.9-risczero.0` (`c2b5d71`); `sha2 0.10.9` → tag `sha2-v0.10.9-risczero.0` (`8631fab`) |
| `ethereum_hashing 0.7.0` | `ncitron/ethereum_hashing` rev `7ee70944ed4fabe301551da8c447e4f4ae5e6c35` (the same patch Helios's own workspace applies) |
| Keccak (guest B) | `tiny-keccak 2.0.2` → `risc0/tiny-keccak` tag `tiny-keccak/v2.0.2-risczero.0` (`8fcc866`); `alloy-primitives 1.7.3` with feature `tiny-keccak` (1.x defaults to `sha3`, which would leave the patch unused) |
| MPT | `alloy-trie 0.9.9` (`verify_proof`, `TrieAccount`), `alloy-rlp 0.3.17` |
| SSZ | `ethereum_ssz 0.9.1`, `ssz_types 0.11.0`, `tree_hash 0.10.0` (Helios's versions) |

The guest `Cargo.lock` files in `methods/guest-*/` record the full resolution. Every
`[patch]` resolves to its git source and no "patch not used" warning remains. There is one
`risc0-zkvm-platform` (2.2.3) and one `sha2 0.10.9`, the patched one.

**Why `blst` was not measured.** Its build compiles C and assembly with `cc` for
`riscv32im-risc0-zkvm-elf` (it defines `__R0VM__`). The test machine has no RISC-V C toolchain:
`rzup show` lists only rust, r0vm and cargo-risczero, and Apple clang has no RISC-V target.
Measuring it would mean installing RISC Zero's C toolchain on the shared test machine, which was not
done.

## Fixtures (`fixtures/`, see `meta.json` in each)

**Sepolia** was fetched at 2026-11-12T15:00:34Z. It is on **Gloas** (fork version
`0x90000076`, epoch 353024), and the beacon API answers `Eth-Consensus-Version: gloas`.

- Finality update from `lodestar-sepolia.chainsafe.io`: attested slot 11563488, finalized
  slot 11563424, signature slot 11563489 (period 1411), 509/512 participation.
- `light_client/updates` for periods 1410 and 1411 from Lodestar. Each is 26,424 B, 509/512.
- Bootstraps at slot 11550720 (period 1410, root `0x9129…8f78`) and slot 11558912
  (period 1411, root `0xf54b…25d3`), from `unstable.sepolia.beacon-api.nimbus.team`.
  Lodestar answers bootstrap with 500 "syncCommitteeWitness not available" and publicnode
  with 404. Both checkpoint roots were cross-checked against publicnode and Lodestar `headers/{slot}`.
- Execution side:
  - Finalized EL block 12119519, hash `0xc637…11f1`, state root `0x2975…c647`.
  - The header JSON comes from publicnode. Publicnode has no `debug_getRawHeader`, so the RLP
    is rebuilt; `keccak` of the rebuilt RLP matches the block hash.
  - `eth_getProof` comes from `sepolia.rpc.thirdweb.com`. Publicnode, Tenderly and 1rpc refuse
    proofs at the finalized block ("distance to target block exceeds maximum proof window").
  - The block is canonical: `eth_getBlockByNumber` on the proof endpoint returns the same hash
    and state root.

**Mainnet** was fetched at 2026-11-12T15:01:51Z. It is on **Fulu** (`0x06000000`, epoch
411392); Gloas is not scheduled on mainnet. It has a bootstrap at the then-finalized checkpoint
(slot 15639232, root `0xac26…9f6a`) and a finality update finalized at slot 15639264
(507/512 participation). Neither Nimbus nor Lodestar served a bootstrap for a checkpoint early
in the previous period, so the mainnet handoff scenario is not run.

**Storage slot.** The contract is the OP Sepolia `L1CrossDomainMessenger` proxy
`0x58Cc85b8D04EA49cC6DBd3CbFFd00B4B8D6cb3ef`, and the entry is
`mapping(bytes32 => bool) successfulMessages` at slot index **N = 203**. The key is message
hash `0xce67…3df3`, relayed in tx `0x8e7f2af2…20c6` (block 12116935).

- The slot key is `keccak256(abi.encode(bytes32 key, uint256 N))`, i.e.
  `keccak256(key ‖ uint256(203) big-endian)` over 64 bytes, which gives
  `0x4cd52750e4fc393f9bb4ad1efdb4cbe3e9de0379e51dbc148da8c429956e09b5`. The value is `1`.
- N = 203 was found empirically: `cast storage` at the derived key was non-zero only for
  N = 203 out of 195 to 215, and `eth_call successfulMessages(h)` returns `true`.
- The guest derives the slot key itself, and the host checks it against the key the proof was
  fetched for. The proof has 8 account nodes and 5 storage nodes.

## The LEZ cap: what it is and what it counts

LEZ `v0.4.0-rc1` is commit `1806e873`.

- `lez/programs/fee/core/src/market.rs`: `MAX_GAS_EXEC = 2 * TARGET_GAS_EXEC = 10,000,000`.
- `validity.rs` rejects a transaction `gas_limit` above it (`GasLimitAboveCap`).
- `accumulate_exec_gas` also caps the **block's** execution-gas total at the same 10M
  (`ExecGasCapExceeded`).
- Public execution gas is the executor's **user cycles**. `lee/state_machine/src/program/mod.rs`
  sets `session_limit(cycle_budget)` and charges `session.user_cycles` (or `SessionInfo::cycles()`,
  which in risc0 3.0.5 is the sum of segment user cycles). risc0's session limit compares
  against `cycles.user`.
- The comparisons below therefore use user cycles. Splitting an update over transactions does
  not raise throughput: each block holds at most 10M of execution gas in total.

## Results (user cycles unless stated)

### Whole sessions

| Scenario | user | paging | reserved | total | segments (2^20) | user ÷ 10M |
|---|---:|---:|---:|---:|---:|---:|
| A. Sepolia (Gloas) same-period: bootstrap + finality update | 44,914,925 | 1,339,849 | 1,455,434 | 47,710,208 | 46 | 4.49 |
| A. Sepolia (Gloas) committee handoff: bootstrap(P-1) + update(P-1) + finality update signed in P | 89,848,330 | 2,709,382 | 2,862,704 | 95,420,416 | 91 | 8.98 |
| A. Mainnet (Fulu) same-period: bootstrap + finality update | 47,318,361 | 1,402,900 | 1,610,387 | 50,331,648 | 48 | 4.73 |
| B. Sepolia mapping entry, keccak accelerator | 261,115 | 46,703 | 216,470 | 524,288 | 1 | 0.026 |
| B. same, software keccak (no accelerator) | 1,012,489 | 71,189 | 95,970 | 1,179,648 | 2 | 0.101 |

### Per operation (guest A marks, `env::cycle_count()` deltas)

| Step | Sepolia same-period | Sepolia handoff | Mainnet same-period |
|---|---:|---:|---:|
| bootstrap SSZ decode | 346,306 | 346,306 | 359,346 |
| `verify_bootstrap` (committee root + branch + header root) | 8,124,039 | 8,124,039 | 8,216,458 |
| `apply_bootstrap` | 58,855 | 58,855 | 62,183 |
| period update: decode / `verify_update` / `apply_update` | | 352,182 / **44,361,293** / 166,959 | |
| finality update: decode | 8,708 | 8,714 | 37,264 |
| `verify_finality_update` | **36,273,554** | **36,273,562** (next committee; rotates) | **38,531,896** |
| `apply_finality_update` | 31,362 | 78,633 | 39,000 |
| output (header root, execution commitment) | 14,791 | 14,791 | 14,790 |

These are the operations the RFP asks about:

- **A finality update within a period** (decode + verify + apply): **36,313,624** on Sepolia
  with 3 non-signers, and **38,608,160** on mainnet with 5 non-signers.
- **A committee-handoff update**, i.e. the period update that installs the next committee
  (decode + verify + apply): **44,880,434**. The first finality update signed by the new
  committee, which rotates the store, costs **36,360,909**.
- The **bootstrap** costs 8,529,200 on Sepolia. It is a one-off per instance, but see
  "Not done" on what loading the committee from LEZ account state per transaction would cost.

### Breakdown of `verify_finality_update` (Sepolia, mode 1 replay on the same crates)

| Step | Cycles | Note |
|---|---:|---|
| aggregate pubkey, real bits (3 non-signers): decompress aggregate key + subtract 3 keys | 4,143,619 | Helios starts from `aggregate_pubkey` when more than half sign (mainnet, 5 non-signers: 6,207,663) |
| signing root (header root, fork data root, domain) | 17,935 | |
| signature G2 decompress (`from_compressed`, includes the subgroup check) | 1,457,401 | |
| Helios's extra G2 subgroup check `sig * r` | 8,042,412 | redundant, see findings |
| hash_to_curve to G2 (XMD:SHA-256, SSWU, clear cofactor) | 2,809,931 | |
| `G2Prepared` ×2 | 2,403,043 | |
| multi Miller loop, 2 pairs | 6,499,114 | |
| final exponentiation + compare | **10,832,740** | alone above the 10M cap |
| **sum of the above** | **36,206,195** | vs 36,273,554 for `verify_finality_update` in the same run; the remaining 67,359 (0.19%) is header roots, finality and execution branches, and clones |
| pairing only (prepare + Miller + final exp) | **19,734,897** | |

Per-key costs and alternatives (the alternatives are not what Helios does):

| What | Cycles |
|---|---:|
| G1 pubkey decompress with subgroup check (Helios), per key (16 keys: 16,343,936) | 1,021,496 |
| G1 decompress without the subgroup check, per key | 175,863 |
| G1 from an uncompressed witness + on-curve check, per key | 2,662 |
| aggregate pubkey, worst passing case: 342/512 sign, 170 subtracted (cost only, Helios's method) | **176,473,321** |
| `SyncCommittee` `hash_tree_root` via `tree_hash 0.10` (what Helios uses) | 8,013,246 |
| the same root with direct `sha2` calls (asserted equal) | 1,545,735 |
| `sha256` of 64 bytes, `sha2 0.9` risczero, per call (1024 calls: 1,258,676) | 1,229 |

### Guest B per step (accelerated / software keccak)

| Step | accelerated | software |
|---|---:|---:|
| `keccak256(header_rlp) == block_hash`, decode state root (Amsterdam header, 23 fields) | 11,575 | 88,445 |
| slot key `keccak256(abi.encode(key, 203))` | 2,201 | 17,575 |
| account proof, 8 nodes | 118,163 | 548,614 |
| storage proof, 5 nodes | 63,694 | 294,283 |
| sum (the rest of the session is input reading) | 195,633 | 948,917 |

### Negative tests (all pass: each one panics in the executor for the stated reason)

Guest A:
- Valid G2 signature taken from another update (wrong message) → `invalid sync committee signature`.
- One signature byte flipped → `invalid sync committee signature`.
- One participation bit cleared (wrong signer set) → `invalid sync committee signature`.
- One bootstrap committee pubkey altered (wrong committee) → `invalid current sync committee proof`.
- Wrong trusted checkpoint root → `invalid header hash`.
- Finalized beacon header altered → `invalid finality proof`.
- Finalized execution block hash altered → `invalid execution payload proof`.
- Next-period finality update without the handoff update → `invalid sync committee period`.

Guest B:
- Storage proof leaf byte flipped → `storage proof invalid`.
- Claimed value 2 instead of 1 → `storage proof invalid`.
- Different mapping key, i.e. an absent entry claimed true → `storage proof invalid`.
- Account `storage_root` altered → `account proof invalid`.
- Execution header altered → `does not hash to the finalized block hash`.

## Findings

1. **A whole update does not fit, and the pairing-seam split fails with this crate.**
   - Our pairing (19.73M) matches the 19.7M reported elsewhere for the same zkcrypto
     `bls12_381` crate.
   - Our final exponentiation is 10.83M on its own, above the cap.
   - Helios's BLS code is `src/types/bls.rs`, 137 lines, typed on `bls12_381`. Moving it to
     `blst` would fork that one file while the light-client logic stays verbatim. That swap is
     unmeasured here; see Pins.
   - The RFP's precompile question (Performance #8):
     - The curve operations (aggregation, decompression, subgroup checks, hash_to_curve,
       pairing) account for 36.19M of the 36.27M.
     - The non-curve remainder is 0.09M: signing root, header roots, branches and clones.
     - So a precompile covering the curve operations would leave a finality update far under
       the cap. A precompile is reachable only from public transactions.
2. **Two Helios costs total about 12.2M; removing them needs no change to the protocol logic.**
   - Helios runs an extra G2 subgroup check (`sig * Scalar::from_raw(r)`, 8.04M) after
     `G2Affine::from_compressed` has already rejected points outside the subgroup.
   - The run includes a probe on an on-curve G2 point outside the subgroup:
     - **Natively, with upstream `bls12_381`, the Helios check is vacuous.** `from_raw(r) == 0`,
       so it returns "in subgroup" for the bad point.
     - **In the zkVM, with the RISC Zero fork, it is a real check.** The fork's `from_raw` is
       non-Montgomery and does not reduce mod r, so the check returns false for the bad point.
     - In both builds `from_compressed` rejects the point. The code is sound either way; on
       the RISC Zero fork the extra check is pure cost.
   - Helios decompresses each non-signer key with a subgroup check, at 1.02M per key.
     - Using uncompressed witnesses would cost about 2.7K per key, plus binding the witness to
       the committed compressed key, which is not measured.
     - Skipping the per-key subgroup check would need a soundness argument that is not made
       here. The keys come from the committed `SyncCommittee` and passed the beacon chain's
       deposit checks.
   - With both changes a finality update would be about 24.1M
     (36.27M − 8.04M − 4.14M + 4 × 2.7K, before binding and point additions). That is still
     2.4× the cap, because the pairing alone is 19.7M. This is arithmetic from measured parts,
     not a measured whole.
3. **Low participation is a cycle-griefing vector in the unmodified code.** At the minimum
   passing participation (342/512), Helios's method costs 176.47M for aggregation alone.
   - Using the measured parts, a finality update would then be about **208.6M** user cycles
     (36.27M − 4.14M + 176.47M).
   - Our fixtures have only 3 and 5 non-signers. Quoting only their numbers would understate
     the worst case.
4. **`tree_hash 0.10` costs 5.2× direct hashing.** It spends 8.01M per 512-key committee root,
   against 1.55M for the same root computed with direct `sha2` calls. This cost appears in
   `verify_bootstrap` and in every update that carries `next_sync_committee` (included in the
   44.36M `verify_update`).
5. **Helios's `verify_*` does not enforce the 2/3 supermajority.** `apply_*` silently skips an
   update without it. The guest therefore asserts that `apply` changed the store; otherwise it
   would commit an old header as progress.
6. **Inclusion is cheap, with a caveat on the accelerator.**
   - With the keccak accelerator, the permutations are proven by RISC Zero's separate keccak
     circuit and are **not** in user cycles. So 0.26M understates the proving cost.
   - Whether LEZ's sequencer and prover support the keccak coprocessor is not verified. The
     software figure, 1.01M, is the safe one for planning.

## Fork coverage

- **Sepolia is on Gloas**, active since epoch 353024 with fork version `0x90000076`. Helios
  0.12.0 handles it: bootstraps, period updates and the finality update verify natively on the
  host and in the guest. Helios's built-in Sepolia config already carries Gloas at epoch 353024
  / `0x90000076`. We pass the forks from the live `/eth/v1/config/fork_schedule`.
- **In Gloas the light-client header has no execution payload header.** It carries only
  `execution_block_hash`. Per the Gloas LC spec this is
  `signed_execution_payload_bid.message.parent_block_hash` (gindex 2856), proven against
  `body_root`. It is the last **delivered** execution payload as of that beacon block.
  - Here the finalized beacon slot 11563424 maps to EL block 12119519, whose header has
    `slotNumber` 11563423: a one-slot lag.
  - The state root therefore has to come from the EL header RLP, checked by keccak against that
    hash. That is guest B's first step, 11.6K cycles.
  - The Amsterdam header appends `blockAccessListHash` and then `slotNumber` after
    `requestsHash`. The other order does not hash to the block hash.
- **Mainnet is on Fulu**, epoch 411392, and Gloas is not scheduled there. Helios handles Fulu
  through the Electra-format LC header, which carries the `state_root` directly.
- No fallback to older data or to spec-test vectors was needed.

## Licences

| Component | Licence | LICENSE file |
|---|---|---|
| Helios (`a16z/helios`; consensus-core reused unmodified) | MIT | yes, repo root. Neither the crate's nor the workspace's `Cargo.toml` has a `license` field, so `cargo metadata` shows none |
| breakdown replay in `guest-consensus/src/main.rs` (copied from Helios `bls.rs` / `utils.rs`) | MIT (Helios) | covered by Helios's |
| r0vm-helios (`boundless-xyz`; used only as a reference for the patch set, no code reused) | `license = "MIT"` in `Cargo.toml` | **no LICENSE file** at the repo root |
| `risc0/zkcrypto-bls12_381` fork | MIT / Apache-2.0 | yes (LICENSE-MIT, LICENSE-APACHE) |
| `risc0/blst` fork (not built) | Apache-2.0 | yes |
| `risc0/RustCrypto-hashes` sha2 forks | MIT OR Apache-2.0 | yes |
| `risc0/tiny-keccak` fork | CC0-1.0 | yes |
| `ncitron/ethereum_hashing` fork | Apache-2.0 | yes |
| `alloy-trie 0.9.9`, `alloy-primitives`, `alloy-rlp` | MIT OR Apache-2.0 | yes for alloy-trie (in the crate) |
| `risc0-zkvm 3.0.5`, `risc0-bigint2` | Apache-2.0 | not in the published crate package |
| this spike | MIT (Cargo metadata) | no LICENSE file yet |

A `cargo metadata` sweep of the resolved guest graphs covered 348 packages for consensus and
154 for inclusion, for the riscv32im target.

- All are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, CC0, Unlicense) with these exceptions:
  - **`fastrlp` 0.3.1 and 0.4.0 are MPL-2.0** (file-level copyleft), pulled in through alloy in
    the consensus guest.
  - `helios-consensus-core` has no licence metadata (above).
- The resolved graph includes optional crates such as `secp256k1`, which may not be compiled
  into the ELF.

## Not done relative to RFP-021

- **No LEZ program.** This is a bare risc0 executor harness. Nothing was run through
  `lee::V03State` or the sequencer, so there is no LEZ gas, transaction or account model.
- **No proving.** There is no proof time, proof size, peak proving memory, or private-path
  latency (Performance #2 and #4). Proving the 36M-cycle update on the test machine was deliberately
  not attempted.
- **No `blst` measurement and no split design.** Neither the final-exponentiation split nor a
  `blst`-backed Helios `bls.rs` was built.
- **Store state.** The bootstrap is re-verified on every run. A LEZ instance would persist the
  store in accounts. What reading 512 committee keys from account data costs per transaction
  (8.0M with `tree_hash`, 1.55M with direct hashing, or nothing if LEZ account inputs are
  trusted pre-state) is not measured.
- **Clock.** `expected_current_slot` is supplied by the host as the update's signature slot,
  not from a LEZ clock.
- **Guest A and guest B are separate runs.** B's block hash is checked against A's output by
  the host, with no composition. B's `chain_id` is a host input that nothing in the proof binds,
  so chain binding (Functionality #5, #14, #15) is not implemented. A commits only the genesis
  validators root and the checkpoint.
- **Only one statement type.** There is no account-only claim beyond the account proof, and no
  event log or receipt-trie inclusion (Functionality #3). There is also no predicate interface
  (#4), nullifier identifier (#6), or privacy analysis (#8).
- **Instance management is not done:** weak-subjectivity handling, instance initialisation and
  immutability, permissionless submission, duplicate updates, or multiple instances
  (#9 to #13, Reliability #4 and #7 to #10).
- **None of the deliverables exist yet:** no CLI, Logos module, IDL, update-submitter daemon,
  or reference consumers (Usability #1 to #6).
- **Low participation is a cost estimate only.** It is not a real low-participation update.
  The consensus-spec-tests light-client vectors were not run.
- **No mainnet handoff run**, because no public endpoint served the needed bootstrap.
- **SHA acceleration has no unpatched control.** The figure is 1,229 cycles per 64-byte sha256
  call, but there is no unpatched-sha2 build to compare it with.
- **The keccak accelerator's coprocessor cost is unmeasured**, and LEZ's support for it is
  unconfirmed.
