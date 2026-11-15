# Attested claims: Sepolia deposits claimed on LEZ through RFP-021

This file covers two demo pairs. **v2** (2026-11-13, fixed code at commit `5e2ce69`) is the current one. **v1** (2026-11-12/13, pre-fix code) is kept below as history. Both v1 results are real: a public claim in LEZ block 2209, and a private claim the user proved (666 s, 9,374 MB peak, LEZ block 2326).

**Where it ran.**
- Ethereum: the public Sepolia testnet.
- LEZ: a **local** 0.4 sequencer, `seq2` on the test machine (127.0.0.1:3250). LEZ testnet 0.4 is not live.
- No stand-in attestor exists in either bridge config (`attestor 1111…1111`, i.e. zero).

## v2 (current demo, fixed code)

Source logs on the test machine:
- `attested/run.log` (run sepolia2), for every LEZ step;
- `lez.log` (run sepolia2), for the wallet and the deploys;
- `sepolia-demo-v2.json` (run sepolia2), the Sepolia side (also `vault/deployments/sepolia-demo-v2.json`; the transaction table is in `vault/DEPLOYMENTS.md`).

### Programs (all deployed immutable on `seq2`, 11-12 21:16–21:24 local)

| Program | Account | Image id | vs v1 |
|---|---|---|---|
| token | `4VScJ7mtwaanQ5gcSZZw2iAMHYAbLrVmbYKwEGx2eSex` | `633555cc…ab5d` | **unchanged** image (new account) |
| bridge | `JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7` (bytes32 `0xfff80541…e910`) | `249a19c6386bfab7b70018312585f4555d9a302cd972308ea4b1a77db74f0129` | new |
| attestation | `4VhxvwLN5n3u3rZbsz2VNFVavLwc1niNvgxq8JJ15veK` | `84f6c998c5879d917175a83fa15af9fef0d93cb3097c49d8f072a6bddb6f7712` | new |
| probe | `7VDaNr8MGJrtpGruCJcp9huQz5s4n7VtwN1b1nAsBr5y` | `b9dcbb1872a66e36155a187f90f37ba63721f5ef1802ac930f58c8bbbfc7f255` | new |
| G3 release guest (Ethereum side) | — | `0x6bcbd714…72a4` | **unchanged** |

- **How the release guest was checked:** its `release_guest.bin` in `target-attest` is still sha256 `4238085e…fe05` (built 11-12 14:10), and `release/` and `methods/release_guest` have no commits since `1d07ade`. The id was read from the build's `methods.rs`; nothing was executed.
- **Image check:** `attested/check-images.py` (run sepolia2) confirms that the CLI binary embeds exactly these deployed guest ELFs. It ran twice and printed "ALL IMAGE IDS MATCH" both times:
  - after the deploys, `create-wrapped` and P2's funding, but before `attest init` and every bridge transaction;
  - again at the end.
- **The v1 binary is kept** as `target/release/lez-bridge.v1-20261112` (sha256 `b6725215…1a12`).

### The chain of facts

| # | Fact | Identifier |
|---|---|---|
| 1 | Vault v2 | `0x8C8b2664De28e5287158db4DB65D448638871b74`, created in Sepolia block 12122976 (32,895,648 gas); `cfgId` `0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11` |
| 2 | Deposit for P2 (`6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f`): 100 dUSDC, leaf 3 | tx `0x7a0ca82a6dd36e9276fc0c098b12645a4c9d4fcce34397a51cc62ba189c2e7b0`, block 12122996, commitment `0x6cdd53cd…878d` (recomputed with `lez-bridge commitment`) |
| 3 | Last deposit and root | block 12122997, slot 11566945; root `0xbe1e080d85207b6b7832392daf38ce65347cabe7902bed16e3251ee03a21d3e8`, which is also the root rebuilt from the 5 `Deposit` events |
| 4 | Finalized checkpoint | slot 11566976 (epoch 361468), root `0x0776a815…2951`; cross-checked on Nimbus |
| 5 | Anchor (the execution block the checkpoint header commits to) | block 12123027, hash `0x5e81a69dd5b2dfd18a00c0021f7954b7971421221897b3ec752a4002561ddd7d`, header `slotNumber` 11566975, 02:35:00 UTC |
| 6 | Instance A | `0x09dd33204d65597cdb66e90f793a4fe3478cbe226a047a77e228b4dcf05ab4de`. Config PDA `3ET657oRnkE7mDunZGjSfZdHQQFhp48yg6AowDmw1zho`, store PDA `DpvY3iTbi1Kg149U8FY8HgP8wmzAAWYFsyULCjFp78Uy`, anchor PDA `9kvUCizAjm6czTcE8pmWvHB6TEYdJBBWur2XFrANTe7D`. LEZ block 2817, tx `df1f55f2…b106`. One attempt |
| 7 | Storage proof | `isDepositRoot[root]`, slot index 83 (re-checked with `forge inspect … storage-layout`), key `0x8a4947a6…81cd`, value 1. From `eth_getProof` at block 12123027 (thirdweb; 9 account + 3 storage nodes). The header RLP was rebuilt (682 bytes) and hashes to the anchor. Saved read-only, sha256 in `attested/material.sha256` |
| 8 | Probe sanity check | statement `0x8002da11…fb1f`, LEZ block 2818, 256,582 gas |
| 9 | Bridge bound to A | `init-attested`, LEZ block 2820, tx `26a582c8…3acc`. The CLI `cfg-id`, the vault `cfgId()` and the expected value were all equal before sending; the on-chain config equals them after |
| 10 | Setup | dUSDC `FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ` (block 2759) and ETH `CswQqK9jzUHf5sraiM8G9HfgMT2bcJ8GhVpbAA9trwQ6` (2760), both minted by the bridge. Registered matching the vault read-back (2821/2822). Sponsor 20,000,000 (2823), float 100,000,000 (2824), P2 funded with 50,000,000 (2761) |
| 11 | **The claim** | LEZ block **2853**, tx `6c856871507e166c4649d57d2c14643a9485d5b85675c8b7cc53b5cb331cd99e` |
| 12 | Result | P2 holds 100,000,000 wrapped dUSDC. Supply 0 → 100,000,000, `claims [1,0,0]`, `claimed_value` 100000000, float unchanged |

### Gas (dry-run metered cycles; fee at base fees 8/8)

| Transaction | v2 | v1 |
|---|---:|---:|
| `attest init` | 9,105,689 | 9,105,915 |
| Probe statement | 256,582 | 232,012 |
| **Attested public claim** | **390,756** (7,281 bytes, fee 3,184,296 = P2's balance drop) | 365,838 |
| Private claim, public part (dry run) | 81,568 | 80,937 |
| A refused claim that is sent anyway | 2,000,000 (full declared gas), fee 16,058,248 | 2,000,000 |

The v2 claim costs 24,918 cycles (+6.8%) more than v1's. Likely cause, not measured separately: the new proof-node pre-check in `attest/core` and one more account-trie node (9 against 8).

### Refusals: the CLI's local pre-check against the program's own answer

- **How the program's answer was reached:** the pre-check was skipped with a hidden test flag, `--no-precheck`, added for this run (`cli/src/main.rs`, 10 lines).
- All rows are dry runs from P2, except the sent repeat claim.

| Case | CLI pre-check (local, nothing sent) | Program (`--no-precheck --dry-run`) |
|---|---|---|
| Storage node tampered | "the vault proof does not show this root as a deposit root: storage proof invalid: value mismatch at path …; nothing sent" | **`claim refused`** (`bridge/src/lib.rs:961`) |
| Value 0, honest nodes | "the vault proof does not show this root as a deposit root: storage proof invalid: value mismatch …; nothing sent" | **`claim refused`** |
| Another block's header under anchor A | "… anchor witness invalid: execution header does not hash to the anchor; nothing sent" | **`claim refused`** |
| Valid proof under block 12123000, which A never recorded | "insufficient finality: the attestation instance has not finalized this anchor (yet); wait for a later finality update, or prove against an anchor it recorded; nothing sent" | "Program violated execution rules" (runtime) |
| Valid proof under throwaway instance B's recorded anchor. B is `0x9a3ce869…d6d4`, block 2850, anchor `0x4ea9509a…0c16`, accepted by the probe in block 2851 | same "insufficient finality" message | "Program violated execution rules" (runtime) |
| Repeat claim | "deposit already claimed: its nullifier is spent; nothing sent" | **`claim refused`** (`lib.rs:1019`) |
| Repeat claim, **sent** with the pre-check skipped | — | included in LEZ block **2855 and reverted**; P2 was charged 16,058,248 (46,815,704 → 30,757,456); counters and supply unchanged |

- **The program's texts carry no values.** The four deposit-dependent cases all panic with exactly `claim refused`.
- **The local pre-check messages do contain trie paths and node bytes.** They never leave the machine.
- **2 of the 6 cases do not return `claim refused` on chain:** the two anchor cases.
  - They fail at the LEZ runtime before the program runs. No anchor PDA exists under A, and `AssertAnchor` carries no seed, so the runtime refuses to admit the empty account (inference, as in v1).
  - In the simulator, which has no admission check, the attestation program refuses with "anchor not finalized by this instance" (`bridge/tests/attest_claim.rs`, `an_anchor_the_instance_did_not_finalize_is_refused`, at HEAD `5e2ce69`).
  - That outer error differs from `claim refused`.
  - It depends only on public data (which anchors A recorded), not on the deposit.
- **The text `claim refused` does not appear in either block** (2853, 2855).

### What a public observer sees (LEZ block 2853, 8,120 bytes)

Same as v1 (below, §4). Present in the block:
- P2;
- the root, the vault, dUSDC, the anchor, the instance and `cfg_id`;
- **the secret `k`**;
- the Merkle sibling leaf 2.

Absent: the commitment, leaf 3, the nullifier bytes, the slot key, V2 and F. A public-account claim publishes its secret by design, which links the Sepolia depositor to P2.

### Private claim on v2: proved (2026-11-13)

- **Proved:** started by the user at 06:42; proved and submitted in 670.9 s (guard wall 685 s), peak footprint 9,403 MB, applied in LEZ block 4998 (`prove-private-claim.log` (run sepolia2)). Float 100,000,000 → 80,000,000; dUSDC `claims [2,0,0]`, supply 200,000,000. A second claim was refused locally ("deposit already claimed").
- **Privacy check of block 4998:** V2, F, P2, the deposit root, the slot key, the commitment and all five secrets are absent; the one-time address, the vault, the anchor, the instance and `cfg_id` are present. V2 is absent from all 2,189 blocks 2817..5005.
- **Dry run (before the proof):** V2 `2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC`, leaf 0, deposit tx `0x5ca75297…598e`. It applies; one-time recipient `AePC3xbb5yVhcpHfUkwCqCqNtoCsgVQnBCfbz8G4vGkN`; sponsor 20,000,000 from the float; public cycles 81,568.
- **Script:** `prove-private-claim.sh` (run sepolia2), written from the v1 script and run once by the user (result above). It has the same gates:
  - another proof running;
  - material and leaves hashes;
  - the secret gives V2's commitment;
  - the anchor is recorded;
  - a dry-run preflight;
  - at least 50% free memory;
  - other memory-heavy local services paused (SIGSTOP/CONT);
  - `guard.sh` with `RISC0_PROVER=local`, 4 threads, 11 GB cap, `TMPDIR=run/sepolia2/tmp`.
- **What it does after the proof:**
  - public checks and a double-claim dry run;
  - a search of the claim block for V2, F, P2, the one-time address A (positive control: the script says so if A is missing), the root, the slot key, the vault, the anchor, the instance, `cfg_id`, the commitment and **all five secrets**;
  - a search of every block from 2817 to the head for V2, which fails closed if `getBlock` errors;
  - a count of written files that contain any secret.
- **Expiry:** nothing in the proof material expires. The claim depends on `seq2`'s state, the float at least 20M, minting open, leaf 0 unspent, the wallet `wallet` (run sepolia2), and the current binary.

### Not done on v2

- The private proof: the user starts it.
- Claims of leaves 1, 2 and 4.
- Outbound: the update image is a placeholder.
- A claim naming B as its `source`: simulator only, as in v1.

## v1 (superseded 2026-11-12/13: pre-fix code, kept as evidence)

Vault `0xA34adbe8413838668b0521230A4d96C6B7e91484`, bridge `55yo6…6q9o`, attestation program `E6ohnq…Ek4q6`. Its matching CLI is `target/release/lez-bridge.v1-20261112` on the test machine. The `bridge/src/lib.rs` line numbers below refer to the pre-fix source.


**What ran.** A deposit made in the Sepolia demo vault was claimed by the bridge program on the **local** LEZ 0.4 sequencer `seq2` (the test machine, 127.0.0.1:3250). The vault's deposit root was taken from an RFP-021 attestation instance that was initialised from a real Sepolia finalized checkpoint. No stand-in attestor key was involved, and no attestor key exists in this bridge's config (`attestor 1111…1111`, i.e. zero).

**Mode.** Public-account claim (claimer P), sent for real. The private claim (claimer V) was prepared as a dry run, then **proved by the user** with `prove-private-claim.sh` (run sepolia1).
- `attested/out-private-claim.log`: "proved and submitted in 666.2s", "applied in block 2326", "guard: exit=0 wall=679s peak_footprint=9374MB".
- `attested/privacy-check.txt`: in block 2326, V, F, P, the root, the slot key, the commitment and `k` are absent; the one-time address A is found (positive control). "V absent from all 148 blocks 2179..2326".

**Not a public testnet on the LEZ side.** Ethereum is the public Sepolia testnet (reads only, no Sepolia transaction was sent). LEZ is a local 0.4 sequencer, because LEZ testnet 0.4 is not live.

Source logs on the test machine: `attested/run.log` (run sepolia1) (every step), `attested/standin-gas.log`, `attested/checkpoint.env`, `attested/material.sha256`.

### 1. The chain of facts

| # | Fact | Identifier | Where it was checked |
|---|---|---|---|
| 1 | Deposit for P: 100 dUSDC, leaf 3 | Sepolia tx `0x2ca9753b40deb5fa4da8bf87f2f2127fbe36f9b51ade3602f26ddfa880f703d4`, block 12121223, commitment `0xd7d6996c…fe25` | `Deposit` event (rebuilt leaves); `lez-bridge commitment` for P equals `C_P100` |
| 2 | Last deposit (leaf 4) and the root after it | block 12121224, beacon slot 11565149; root `0x88031a1cde4ab9bbbc7c9320070e80487b8e531c47ecb02c82ad0080fd810394` | event root of leaf 4 and `depositRoot()`; the root rebuilt from the 5 event leaves is the same |
| 3 | Finalized checkpoint after the deposits | beacon slot 11566176 (epoch 361443), root `0x5883fff4405ef17e2a017814ceba1468d3442c7349a5b3e751224c4bbc7d4692` | lodestar-sepolia, cross-checked on the Nimbus beacon API; bootstrap verified natively (Helios) and again by the program |
| 4 | The execution block the checkpoint's header commits to (the anchor) | block 12122237, hash `0x60850e3cb158b31c896af167ee03c064007debc8792a874833d15add9b568618`, header `slotNumber` 11566175, 2026-11-12 23:55:00 UTC | Gloas light-client header (block hash only); `eth_getBlockByHash`. Under Gloas the checkpoint block (slot 11566176) carries the hash of the payload from slot 11566175 |
| 5 | Attestation instance A on LEZ | instance `0xc0c73624ec6b6a7dad608ae2a476165e7a784ac3f4a21befea5135763bb994bb`, program `E6ohnqgGYynZe2URYondAtx7foZ8VkSCLhYpAT9Ek4q6` (immutable), config PDA `2LX4vvjA1wZFembX1LfvAc79eDwd3FJrakrx8vX2iEeR`, store PDA `AQ8N94bHeS9HWAE2rM7dS3ykEsosXo3fnWsmBw1vSrAT` | `attest init`, public, LEZ block 2179, tx `7c1fea25254f1b55fab70a9275c80df758b840f2092b7a38eede97a3357c13ac`. One attempt |
| 6 | The anchor recorded by instance A | anchor PDA `3nc82yu6syHVZW7mDPv78oCrzPxUXsMimQHdjgpXZa8z` | `attest show`: "RECORDED by beacon slot 11566176" |
| 7 | Storage-proof target | vault `0xA34adbe8413838668b0521230A4d96C6B7e91484`, `isDepositRoot[0x8803…0394]`, slot index 83, key `0xcc872c95…0dfc`, value 1 | `eth_getProof` at block 12122237 from thirdweb (8 account + 3 storage nodes); the header RLP was rebuilt (677 bytes) and hashes to the anchor |
| 8 | Sanity check through the probe consumer | statement id `0xf9829fef…92b2` | `attest prove-slot --eq 1`, public, LEZ block 2181 |
| 9 | Bridge bound to instance A | `init-attested`: LEZ block 2190, tx `cf496641c38548119d6907938cf761d4cd06e7631a8f690ee9b85b71533e0050`; `root_source rfp-021 … instance 0xc0c7…94bb slot 83` | `cfg_id` from the CLI = vault `cfgId()` on Sepolia = on-chain config = `0xda620b16999ff986739727aef71641d74f04e745fd197693ed1211b8f2fc0118`, checked before and after |
| 10 | The claim | LEZ block **2209**, tx `3a0a15cde7ee4ffc21ccb7187f6689105f1da5dde06db638f3b869e73747cbb0`, claimer P `HPqQSHYDmQ9Ma9ySWFFeGZDqN7fCiVnFrqzH8ag29BzW` | applied |
| 11 | Result | P holds **100.000000** wrapped dUSDC (definition `4y4Q5U3eXZFgmZL6Gy9Xa2rWRnfw6sbgEWuziYKiA2TR`, supply 0 → 100,000,000); config `claims [1, 0, 0]`, `claimed_value 100000000`; float unchanged at 100,000,000 (a public claimer is not sponsored) | `balances`, `definition`, `show` |

**Setup on LEZ** (same sequencer, all public transactions from F `6iArKUXx…2VQV`):
- Wrapped definitions, mint authority = bridge program `55yo6Et5XtzSLFYNe9gjQhhBhd3X4LcidsUaPanw6q9o`:
  - dUSDC `4y4Q5U3e…A2TR` (block 2187);
  - ETH `75pyXeNt…bkHw` (block 2188).
- Registration, matching the vault's `tokenConfig` read from Sepolia:
  - dUSDC `0xAc20…a71e`, 6 decimals, 100 / 1,000 / 10,000 (block 2191);
  - ETH `0x0…0`, 18 decimals, 0.01 / 0.1 / 1 (block 2192).
- Sponsor amount 20,000,000 (block 2193); float topped up to 100,000,000 (block 2194).
- P funded with 50,000,000 native LEZ from F (block 2195).

**Image ids.** Nothing was rebuilt: no CLI or program edit was needed, and the binary's mtime is unchanged (2026-11-12 15:17:44 local).
- `attested/check-images.py` confirms that the CLI binary embeds the exact guest ELFs whose ids are deployed: token `633555cc…`, bridge `234c6659…`, attest `50a29f83…`, probe `c3c00884…`.
- It ran after `attest init` and the probe (blocks 2179/2181) but before any bridge transaction, and again at the end. Both runs printed "ALL IMAGE IDS MATCH".

### 2. Gas (metered cycles, LEZ cap 10,000,000 per transaction)

| Transaction | Metered cycles | Wire bytes | Fee (base fees 8/8) | Source |
|---|---:|---:|---:|---|
| `attest init`, instance A (public) | 9,105,915 | 26,286 | 73,057,608 | dry run; the fee-derived figure agrees (block 2179) |
| Probe statement `isDepositRoot[root] == 1` (public) | 232,012 | 5,880 | 1,903,136 | dry run = fee-derived (block 2181) |
| **Attested claim, public claimer P** | **365,838** | **7,253** | **2,984,728** | dry run; P's balance fell 50,000,000 → 47,015,272, exactly the quoted fee (block 2209) |
| Stand-in claim, public claimer, same bridge image `234c6659…` | 150,889 | 1,893 | 1,222,256 | dry run on run `public6`'s bridge and anvil vault (`attested/standin-gas.log`). To get it, one more 100 USDC deposit was made on the local anvil (port 8550, public6 vault, index 3) and its root recorded with `AttestRoot` on public6's bridge (LEZ block 2203, tx `7b6c1749…0f5cfd`). That deposit's secret was not kept; it was never claimed |
| Attested claim, private claimer V: public part only | 80,937 | — | reserve 5,718,112 | dry run |
| `init-attested` / `register` (each) | 63,503 / 41,639 and 41,775 | | | dry runs |

- **The attested public claim costs 2.4× the stand-in claim in cycles (+214,949) and 3.8× in bytes.** The difference is the storage proof and header verification (keccak, RLP, trie walk) in the claimer's actor, plus `AssertAnchor`.
- **The stand-in path also needs one `AttestRoot` transaction per root,** which was not metered here. The attested path needs no per-root transaction: one instance anchor serves every deposit up to it.
- All of these fit far under the 10M cap. Executor-only rows in DESIGN §9.3 are a different harness and are not compared here.

### 3. Refusals (all dry runs from P, before the real claim unless noted)

| Case | Result |
|---|---|
| Storage proof with one nibble of the last storage node flipped (a copy) | **REVERTS**: "storage proof invalid: value mismatch at path …" (`bridge/src/lib.rs:812`) |
| Honest nodes but the value set to 0 | **REVERTS**: "storage proof invalid: value mismatch …" |
| Anchor A's hash with another block's header | **REVERTS**: "execution header does not hash to the anchor" |
| Valid proof and header under Sepolia block 12121226 (after the deposits, never recorded by A) | **REVERTS**: "Program violated execution rules" |
| Valid proof under instance B's recorded anchor (`0x1f70d9d7…84de`, block 12122269). B is a throwaway second instance, `0x456bad51…96f7` (LEZ block 2205); its anchor was accepted by the probe (block 2206) | **REVERTS**: "Program violated execution rules"; `attest show` on A: "NOT recorded by this instance" |
| The same deposit again, after the real claim | **REVERTS**: "deposit already claimed" (`bridge/src/lib.rs:868`) |

- **The two anchor cases are one code path.** In both, no anchor PDA exists under instance A for that anchor.
  - The wallet prints only the outer error (`LeeError::InvalidProgramBehavior`).
  - Inference, not observed: LEZ refuses to admit the empty PDA, because `AssertAnchor` carries no seed, before the attestation program's own check ("anchor not finalized by this instance") runs. The simulator tests (`an_anchor_the_instance_did_not_finalize_is_refused`) see the program's message because the simulator has no admission check.
  - Either way the claim fails closed.
- **Why B was made.** The "other instance" is B, made for this test, not the older `0xbfc7…d61b`. That instance's anchors (slots 11564960, 11564992) predate the deposits, so a claim against them would fail on the storage proof (value 0), not on the anchor.
- **Not tested on the chain: a claim that names instance B as its `source`.** Under B, the anchor PDA does exist. The bridge config's `WRONG_SOURCE` check ("claim for another attestation source") refuses it. That check is covered only in the simulator (`bridge/tests/attest_claim.rs`, `a_wrong_vault_slot_index_or_source_is_refused`). The CLI cannot send such a claim, because it copies `source` from the config.
- **The spent nullifier is shown only by the double-claim refusal** (`bridge/src/lib.rs:868`). Its PDA was not read directly.
- **A reverted public claim still costs its full declared gas** (2,000,000 cycles; fee 16,058,024 at the head). The wallet's dry run shows the revert for free.

### 4. What a public observer sees in the claim block (LEZ block 2209, 8,092 bytes)

Searched with `blocksearch.py` (run private4):

| Value | In the block |
|---|---|
| Claimer and recipient P | found |
| Deposit root, vault address, dUSDC address, anchor block hash, instance id, `cfg_id` | found |
| **The deposit secret `k`** | **found** |
| Merkle path (e.g. leaf 2, a sibling of leaf 3) | found |
| P's commitment, leaf 3, the nullifier (as raw bytes), the slot key | absent (each is derived, so anyone holding `k` recomputes them) |
| V, F | absent |

**Consequence.** A **public-account** claim publishes the whole instruction, the secret included. With `k`, anyone can recompute the commitment and match it to Sepolia tx `0x2ca97…` from the depositor `0x37a4…321F`. So this mode links the depositor to P by design. It shows that the attestation path works; it is not the private path. The private claim keeps `k`, the root, the proof and V inside V's proof (DESIGN §3.2), and only `AssertAnchor(instance, chain, anchor)`, the nullifier and the config effect are public. That claim was proved later (§5: LEZ block 2326, V absent from all blocks).

### 5. Private claim: prepared here, then proved by the user (666 s, 9,374 MB, LEZ block 2326)

- **Dry run** (V `DMv13rBB…2HJ1`, leaf 0, 100 dUSDC, tx `0x220a9257…7bac`):
  - it applies;
  - one-time recipient `4jVRemj834vS89hgeWwxHePs7yScNv7uktt4tv12Zn47`;
  - sponsor 20,000,000 from the float;
  - public cycles 80,937.
- **Script** `prove-private-claim.sh` (run sepolia1) runs these steps in order:
  1. Refuses to start if another proof is running.
  2. Checks the frozen proof material by sha256, and that the secret gives V's commitment.
  3. Checks the anchor is recorded.
  4. Runs a dry-run preflight.
  5. Pauses other memory-heavy local services with SIGSTOP (resumed on exit) and requires at least 50% free memory.
  6. Proves once under `scripts/guard.sh`, with `RISC0_PROVER=local RAYON_NUM_THREADS=4 MAX_FOOTPRINT_GB=11` and `TMPDIR=run/sepolia1/tmp`.
  7. Runs the public checks and a double-claim dry run.
  8. Searches the claim block for V, F, P, A, the root, the slot key, the vault, the anchor, the instance, `cfg_id`, the commitment and `k`, and every block from 2179 to the head for V.
- **Proved (11-12 evening, by the user):** 666.2 s, 9,374 MB peak footprint, LEZ block 2326 (files cited in the v1 intro above). This was the first real proof that ran keccak (the coprocessor patch) inside a private execution, and it went through.
- **What expires:** nothing in the proof material.
  - The anchor PDA is never deleted.
  - The storage proof and header are saved and read-only (`attested/slot-A`, hashes in `attested/material.sha256`); no RPC is needed at proving time.
- **What the claim still depends on:**
  - `seq2` keeps running with its state;
  - the float is at least 20,000,000;
  - minting is not frozen;
  - leaf 0's nullifier is unspent;
  - the wallet in `wallet` (run sepolia1) is intact;
  - the CLI binary is not rebuilt with different guest images.

### 6. Not done

- The ETH deposit (leaf 4) and V's other dUSDC deposits (leaves 1 and 2) were not claimed. ETH is registered, so leaf 4 is claimable the same way (`--eth-token 0x0…0 --denom 10000000000000000`).
- **Outbound is not possible on this demo vault:** its `updateImageId` is a placeholder (`vault/DEPLOYMENTS.md`).
- No light-client update was needed or run on instance A: its checkpoint already postdates the deposits. A later deposit needs a proved update, or a new instance.
- The exact inner error of the two anchor refusals is not printed (§3).
- A claim naming another instance as its `source` was tested only in the simulator (§3).

### 7. Bugs

**None found.** `init-attested` and `claim-attested` ran correctly the first time, with no CLI or program edit and no rebuild.

Observations, not fixed:
- The wallet shows only "Program violated execution rules" and hides the inner LEZ error.
- `claim-attested` does not verify the storage proof natively before sending (`attest prove-slot` does), so a bad proof costs a reverted transaction unless dry-run first.
- A reverted public claim is charged its full declared gas: 2,000,000 cycles, a fee of 16,058,024 at base fee 8.
- Only the LEZ side is local. Nothing ran on a LEZ public testnet.
