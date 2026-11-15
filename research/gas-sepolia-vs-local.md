# Gas on Sepolia (Amsterdam fork) vs local Foundry (2026-11-12)

## 1. Why the same transactions cost 3–9x more on Sepolia

**Reproduced exactly.** I fetched the 18 successful Sepolia transactions and replayed their exact calldata, sender and nonces on a local anvil. I used Foundry v1.9.0, verified by sha256 against its release asset; the installed 1.5.1 panics with "unimplemented Amsterdam".
- With `--hardfork amsterdam`, all 18 receipts match Sepolia to the unit: 36,133,590 for the vault, 4,098,169 for DemoUSDC, 509,076 for deposit 2, and so on.
- With `--hardfork osaka`, the same transactions give the mainnet-rule costs: 5,140,054, 618,975, 193,116 and so on.
- So the difference comes entirely from the fork's gas schedule.

**What the RPC shows:**
- The Sepolia receipts have no split field. Their keys are the standard ones: gasUsed, cumulativeGasUsed, effectiveGasPrice and the rest (publicnode, Geth/v1.17.9).
- publicnode refuses `debug_traceTransaction` ("method does not exist").
- The headers carry `blockAccessListHash` and `slotNumber`, and the gas limit is 199,803,925.

**Fork and EIP sources:**
- The fork is Glamsterdam. The meta EIP-7773 (https://eips.ethereum.org/EIPS/eip-7773) gives the Sepolia activation as 2026-10-06 13:53:36 UTC (epoch 353024) and the mainnet date as "not yet set".
- Hoodi is listed for 2026-10-26, but one press report says Hoodi was delayed to examine EIP-8037 (https://forklog.com/en/news/ethereum-developers-delay-glamsterdam-in-hoodi-testnet-to-examine-eip-8037). I did not resolve that conflict.
- EIPs 8037, 8038 and 2780 are in **Last Call** (deadline 2026-11-01), so the values can still change.

**The causes (EIP texts read from github.com/ethereum/EIPs):**
- **EIP-8037, "State Creation Gas Cost Increase"** (https://eips.ethereum.org/EIPS/eip-8037). This is the main cause. It sets `CPSB = 1530` gas per state byte.
  - A new storage slot costs 64 × 1530 = **97,920 state gas** (was 20,000).
  - A new account costs 120 × 1530 = **183,600** (was 32,000 or 25,000).
  - Code deposit costs **1,530 per byte** (was 200), plus 6 per 32-byte word of execution gas.
  - State gas is a **separate dimension** with a "reservoir". The user pays the sum of both dimensions, and receipt `gasUsed` includes both, which is why no new receipt field appears.
  - Quote: "the transaction gas limit set in EIP-7825 only applies to execution-gas, while state-gas is capped by `tx.gas`", with `tx.gas ≤ 2^32−1`.
- **EIP-8038, "State-Access Gas Cost Update"** (https://eips.ethereum.org/EIPS/eip-8038).
  - `STORAGE_WRITE` rises from 2,800 to 10,000, so a cold change to an existing slot goes from 5,000 to 12,100.
  - A new slot's execution part becomes 2,100 + 10,000.
  - `COLD_ACCOUNT_ACCESS` rises from 2,600 to 3,000, and `CREATE_ACCESS` is 12,000.
- **EIP-2780, "Resource-Based Intrinsic Transaction Gas".** `TX_BASE_COST` is 12,000. Every replayed call costs exactly 6,000 less in intrinsic gas than under Osaka. That is consistent with 12,000 + 3,000 cold recipient access; I did not read its full formula.
- **Ruled out:**
  - **Calldata pricing (EIP-7623/7976):** the floor never binds here. The 25,322-byte vault creation fits the model below with no calldata term.
  - **Block access lists (EIP-7928):** no gas effect is visible.
  - **Contract size (EIP-7954):** irrelevant, since 22,721 B is under the old limit.
  - **Initcode metering:** no change is needed to fit the data.

**Model check (the per-transaction formula reproduces the data):**

| Item | Osaka | Amsterdam | Difference |
|---|---|---|---|
| New storage slot (cold) | 22,100 | 110,020 (97,920 state + 12,100 execution) | +87,920 |
| Changed slot (cold) | 5,000 | 12,100 | +7,100 |
| Intrinsic | | | −6,000 |

| Transaction | Predicted difference | Actual difference | Residual |
|---|---|---|---|
| `approve` / `setCapWeight` ×2 (1 new slot each) | 87,920 − 6,000 = 81,920 | 81,920 exactly | 0 |
| Vault creation | −29,000 + 183,600 + 1,330 × 22,721 + 6 × 711 + 7 × 87,920 = 30,993,236 | 30,993,536 | 300 |
| DemoUSDC creation | 3,478,994 | 3,479,194 | 200 |
| Deposit 2 (3 new, 8 changed slots) | 314,560 | 315,960 | 1,400 (cold-call +400s and warm +100s, not itemised) |

The vault creation splits into **35,632,170 state gas and about 501,420 execution gas** (derived).

## 2. Local reproduction

Foundry ≥ 1.9.0 can emulate the fork: anvil `--hardfork amsterdam`, and forge `--hardfork`, which is separate from `--evm-version`. It is validated by the exact match above.
- The measurement script, `vault/script/GasMeasure.s.sol`, sends every step as a real local transaction (mock verifier, mock tokens), so the figures come from receipts.
- `Gas.t.sol`'s `gasleft()` deltas **do not see state gas under Amsterdam**: EIP-8037 says "The `GAS` opcode returns `gas_left` only (excluding the reservoir)". Don't use them for Amsterdam figures.
- anvil's `eth_estimateGas` ran 12,132–12,180 above actual for Amsterdam deposits and about 4,950 above for Osaka.
- `release` and `update` cannot be estimated on Sepolia without valid G2/G3 proofs.

## 3a. Mainnet rules (Osaka): receipts from local anvil 1.9.0 `--hardfork osaka`, mock verifier, current source

The Amsterdam column is emulated (anvil 1.9.0 `--hardfork amsterdam`, validated by the replay), not observed on Sepolia.

| Operation | Osaka, caps on | Osaka, caps off | Amsterdam (emulated), caps on / off |
|---|---|---|---|
| Deploy BridgeVault | 5,140,054 | 5,140,054 | 36,133,590 |
| ERC-20 deposit, first ever (#0) | 258,328 | 239,415 | 897,568 / 864,255 |
| ERC-20 deposit, steady (#2, #8, #128, #256) | 174,993 | 156,080 | 410,133 / 376,820 |
| ERC-20 deposit that first reaches a new tree level (#1, #3, #7, #127, #255) | 192,731–212,352 | 173,818–193,439 | 508,691–528,312 / 475,378–494,999 |
| USDT-style deposit, first / second | 233,906 / 185,409 | 214,993 / 166,496 | 718,806 / 427,849 (caps on) |
| ETH deposit, first / second | 178,670 / 166,286 | 159,757 / 147,373 | 478,630 / 385,426 (caps on) |
| `update`, 1 exit (first update) | 219,084 | same | 756,104 |
| `update`, 2 exits | 184,834 | same | 405,774 |
| `update`, 32 exits | 1,118,712 | same | 1,368,452 |
| `release`, first / same recipient again | 163,848 / 129,648 | 144,843 / 110,643 | 458,808 / 262,968 (caps on) |

Notes on the table:
- `update` with 4/8/16 exits under Osaka: 247,950 / 370,743 / 616,066.
- Add Groth16 verification, about **+230k**, to `update` and `release`; see below.
- A release to a **fresh** recipient was not measured; the run was interrupted. Derived: about +17k under Osaka and +98k under Amsterdam over the "again" row.

**`Gas.t.sol` call-level figures** (forge 1.5.1, `foundry.toml` `evm_version = cancun`, `--isolate`; `gasleft` deltas, so no 21k intrinsic or calldata). Not re-run under `--hardfork osaka`.
- **Caps on:**
  - ERC-20 deposits #0, #1, #2, #3, #127, #255, #299: 297,849, 198,052, 180,315, 200,844, 210,372, 213,180, 185,758.
  - USDT-style 256,341 / 190,744; ETH 207,749 / 178,266.
  - First deposit in a new cap bucket 184,808; same bucket 187,449.
  - `update` with 1/2/4/8/16/32 exits: 222,445 / 188,209 / 251,361 / 374,188 / 619,599 / 1,122,417.
  - `release` 203,382 / 134,982.
- **Caps off:**
  - ERC-20 deposits #1 to #299: 179,139, 161,402, 181,931, 191,459, 194,267, 166,845 (#0 not captured).
  - USDT-style 220,328 / 171,831; ETH 171,736 / 159,353.
  - `update` same as caps on; `release` 150,177 / 115,977.
- The anvil receipts are the transaction-level figures to quote.

**Groth16, measured by estimateGas:**
- Inputs: risc0-ethereum v3.0.1 `contracts/test/TestReceiptV3_0.sol`. The seal is 260 B starting `0x73c457ba`; IMAGE_ID `0x11d264ed…ea1a`; journalDigest = sha256("just a simple receipt") = `0x3b8839d2…91be`. Its BN254_CONTROL_ID matches the on-chain `0x04446e66…7ac0`.
- `eth_call` to `verify` on the Sepolia verifier succeeds. A one-byte-modified seal reverts with `0x439cc0cd` (VerificationFailed).
- estimateGas for a direct `verify` call: **Sepolia 250,376; local Amsterdam 250,164; local Osaka 256,164**. The local runs used anvils with the verifier's code copied in, not a fork.
- Osaka intrinsic for that 420-byte calldata is 26,640, so verification execution is about **229.5k** (derived). Inside `update`/`release` it adds about 230k plus about 3k of extra seal calldata (derived). It writes no state, so it is nearly the same under both rule sets.

## 3b. Observed on Sepolia (Amsterdam rules), receipts

| Transaction | Hash | Gas used | Osaka replay of the same tx |
|---|---|---|---|
| Create DemoUSDC | `0x27a170fb…d923` | 4,098,169 | 618,975 |
| Create BridgeVault | `0x42a59675…1322` | 36,133,590 | 5,140,054 |
| `registerToken` dUSDC | `0xdd8c3b67…9f4d` | 1,037,498 | 251,818 |
| `registerToken` ETH | `0x7859c576…3e23` | 827,892 | 211,452 |
| Deposit 1 (dUSDC) | `0x220a9257…7bac` | 800,033 | 241,613 |
| Deposit 2 (dUSDC) | `0x110800c5…f6` | 509,076 | 193,116 |
| Deposit 3 (dUSDC) | `0x779e97d1…25ff` | 508,450 | 192,490 |
| Deposit 4 (dUSDC) | `0x2ca97537…03d4` | 511,879 | 195,919 |
| Deposit 5 (ETH) | `0xa9351249…581b` | 469,970 | 170,010 |

- Each Sepolia deposit created 3 new slots for one-off reasons: a first-touched tree level, or a first-used `counters` entry.
- A **steady** deposit has 2 new slots: about 410k with caps on (emulated).
- `DEPLOYMENTS.md`'s 427,090 live estimate is consistent with that plus the estimator's overshoot.

## 4. Design consequences

**Per-deposit new slots, steady state: 2**, `commitmentUsed[c]` and `isDepositRoot[root]`. One-offs come on top:
- `filled[h]` the first time each tree level is reached (32 times ever);
- `counters[token][denom]` on first use;
- the first-ever bucket and balance slots.

(Slot identities were inferred from the storage layout and the per-transaction storage diffs.)

**Per-deposit changed slots:** 5 in the vault (tree nextIndex, root, `filled[h]`, and the token and global cap buckets) and 3 in the token.

**What the new pricing does to the cost structure:**
- The 2 new slots cost 44,200 of 174,993 gas under Osaka (25%), and 220,040 of 410,133 under Amsterdam (54%).
- The two-bucket caps and the 32-level tree only rewrite existing slots, so they add no state growth. Under EIP-8038 they pay +7,100 per write: the cost of caps on rises from about 19k to about 33k per deposit.
- `exitRootTime` adds 1 new slot per update; `releaseNullifierSpent` and a fresh recipient's balance add 2 per release.

**Options if this pricing reaches mainnet** (not implemented; savings derived from the per-slot model):
- **(A) Replace the permanent `isDepositRoot` with a fixed ring of recent roots**, and let G2 prove older roots from the L1 tree state.
  - Saves about 97.9k per deposit (about 24%) under Amsterdam, about 17k under Osaka.
  - Cost: it changes R#4 ("roots kept forever").
- **(B) Move the duplicate-commitment check into the G2 guest**, which sees every leaf, and drop `commitmentUsed`.
  - Saves about 108k per deposit (about 26%).
  - Together with (A), a steady deposit drops to about 205k.

## 5. Does the 36M creation fit?

- **Mainnet today (Osaka):** the creation costs 5,140,054, under EIP-7825's per-transaction cap of 2^24 = 16,777,216 (Final). It fits. I did not query mainnet's live block gas limit.
- **Amsterdam as specified:** it fits. The 2^24 cap applies only to execution gas, which is about 0.5M here. The 35.6M of state gas must fit under `tx.gas ≤ 2^32−1` and the block's state-gas room (Sepolia's limit is 199.8M).
- **New pricing *without* the reservoir:** it would not fit. EIP-8037 itself says a flat 16.7M cap "would limit the maximum contract size that can be deployed to roughly 7.5kB". The vault would then need splitting into pieces of 7.5 kB or less (libraries or facets), each deployed in its own transaction.
- So the task's "would not fit" holds only in that hypothetical.

## How to reproduce

**Requirement:** Foundry **≥ 1.9.0** for `--hardfork amsterdam` (anvil and forge). The installed 1.5.1 panics with "unimplemented Amsterdam". 1.9.0 does not need to replace the installed version: download the release asset into a separate directory and call its binaries by path.

**Files** (`research/gas-replay/`):
- `fetch.py <txs.json> <verifier.hex>`: fetches the Sepolia transactions and receipts (hashes listed in the script) from publicnode, and the Sepolia verifier's code.
- `replay.py <hardfork> <port> txs.json verifier.hex <out.json>`: replays them on an anvil already running on `<port>` (`anvil --hardfork <hf> --gas-limit 1000000000`). It funds the deployer, copies in the verifier code and records each receipt. `out-osaka.json` and `out-amsterdam.json` are the results behind §1 and §3b.
- `measure.sh <hardfork> <true|false caps> <port>`: starts an anvil and runs the `GasMeasure` stages (`setUpAll(bool)`, `deposits(0,300)`, `extraDeposits()`, `updates()`, then +1 h, `releases()`) with `forge script --broadcast --slow --hardfork <hf>`. Set `S` (a work directory for logs and broadcast copies), `F` (the Foundry 1.9.0 directory, default `$S/foundry19`) and `VAULT_COPY` (a scratch copy of `vault/`, default `$S/vcopy`) before running it.9.0 directory) and the `cd` (to `vault/`) before running it.
- `summ.py <dir> <hf>-<caps>`: summarises the broadcast JSONs into the §3a figures and writes `deps-<hf>-<caps>.json` (per-deposit gas for deposits 0–299; the saved copies are included).
- `slots.py <port> <block>...`: prints the new and changed storage slots per transaction (prestate diff), the basis of §4's slot identities.

**Script:** `vault/script/GasMeasure.s.sol` (mock verifier and mock tokens; each stage is a separate `--sig`). It compiles with forge 1.5.1 too, but needs 1.9.0 to run against an Amsterdam anvil. The saved revision is the one from the interrupted second round: its `releases()` adds a third release to a fresh recipient. The §3a figures come from the first round, whose `releases()` had only the first two; the three-release version never completed a run.

**Raw data:** the raw broadcast receipts (`bc-*.json`) were not kept. `deps-*.json` holds only the 300-deposit series per configuration, so the setup, USDT, ETH, `update` and `release` rows in §3a can only be regenerated by re-running `measure.sh` with Foundry 1.9.0.

**Caution:** `Gas.t.sol`'s `gasleft()` deltas do not see state gas under Amsterdam (EIP-8037: `GAS` excludes the reservoir), so use receipts from the anvil runs, not in-test deltas, for Amsterdam figures.

## Not determined
- Mainnet's live block gas limit and fork: not queried by `eth_config`/`eth_getBlock`.
- Hoodi's actual status: the meta EIP and the press report conflict. No mainnet date exists beyond "not yet set".
- Client release notes: not read.
- A public tracing endpoint other than publicnode: none tried.
- A fresh live-Sepolia `eth_estimateGas` for a further deposit: not run. Only the 427,090 in `DEPLOYMENTS.md` exists.
- The fresh-recipient `release` measurement.
- The forge 1.9 `--hardfork osaka` re-run of `Gas.t.sol`.
- The caps-off deposit #0 in-test figure.
- EIP-2780's exact intrinsic formula.

Other sources: Foundry v1.9.0 release notes (https://github.com/foundry-rs/foundry/releases/tag/v1.9.0); EIP-7825 (https://eips.ethereum.org/EIPS/eip-7825).
