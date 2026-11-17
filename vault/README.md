# BridgeVault (RFP-022, Ethereum side)

`src/BridgeVault.sol` locks ERC-20s and ETH in fixed denominations for the private LEZ bridge.

- **Deposits:** `deposit(token, denomIndex, C)` and `depositETH(denomIndex, C)`.
  - SafeERC20, so USDT-style tokens that return nothing work.
  - A balance-delta check refuses fee-on-transfer tokens.
  - Each leaf goes into an append-only SHA-256 tree of depth 32 (the same zero hashes as the Ethereum deposit contract).
  - Every root is kept forever (`isDepositRoot`).
- **Outbound state:** `update(journal, seal)` checks a G2 proof against a fixed image ID, then appends exits. The journal must chain on the current state (`StaleState`) and move it (`NoStateAdvance`), so an applied journal can't be replayed as long as states never repeat (a real G2 batch always moves the LEZ tip forward).
- **Release:** `release(...)` checks a G3 proof whose journal binds recipient, relayer and fee, then spends a nullifier after the release delay.
- **Solvency is enforced by the contract itself:** claims ≤ deposits, burns ≤ claims and releases ≤ burns, per token and denomination. Fees are capped at 2%, and the fee withdrawal can only take accrued fees.
- **Admin:** registry with decimal checks, append-only denominations, fees, caps, release delay (with an immutable minimum), freeze, two-step admin handover.
  - Caps (F#11): per-token and global deposit and release caps over a rolling window (two-bucket sliding estimate). Global caps count each token by an admin-set `capWeight` (global units per smallest token unit, scaled by 1e18; 0 exempts the token). Any cap rejection is `CapExceeded` and consumes nothing, so the same call can be retried later. `capState(token)` and `globalCapState()` return the current usage. The estimate is not a hard bound: in the worst case close to 2x a cap can pass within one window length.
  - Freeze (F#14): deposits and releases are frozen separately, per token or globally (`freeze`/`freezeAll(deposits, releases)`); the freezer or admin freezes, only the admin unfreezes. Only the global release freeze stops `update`.
  - Release delay (F#12): an exit root recorded at time t waits, per token, the lowest delay in force at any moment since t: the delay when it was recorded, cut by any later lowering. A raise applies only to roots recorded after it, never lengthens a recorded root's wait, and never makes a releasable root wait again; a root in the same second as a change gets the lower of the two. `releasableAt(token, root)` gives the time; `releaseDelayState(token)` and `delaySteps(token, i)` expose the per-token history.
  - Relay-fee menu: `setRelayFeeMenu(token, uint256[][])` (admin only) sets at most 4 non-zero relay fees per denomination, each within the 2% bound; fee 0 (self-submission) is always allowed. A non-zero fee needs a relayer (`ZeroRelayer`) and a value on the menu (`RelayFeeNotOnMenu`). The exit commits only to `maxRelayFee`, so a menu change never strands an exit (`test_MenuChangeNeverStrandsExit`).
  - Payees: `release` refuses recipient = the vault or address(0) (`BadRecipient`) and a fee-earning relayer = the vault (`BadRelayer`). The recipient is fixed in the exit commitment, so an exit burned to such an address can never be released; clients must refuse to build one.
- **Fixed verification:** the verifier address and the image IDs are immutable, so verification can't be swapped.
- **Deposit desk (demo networks only; deployed on Sepolia in front of v2, `DEPLOYMENTS.md`):** `src/desk/DepositDesk.sol` sits in front of a vault and takes a bridge fee of `feeBps` (owner-set, hard cap 300 = 3%, rounded down; **30 = 0.3% on Sepolia since 2026-11-14**, `script/SepoliaDeskSetFee.s.sol`) of the denomination, in the deposited asset, on top of the vault's amount; it forwards the deposit in the same call, so the leaf is the same as a direct deposit's, and emits `FeePaid(token, lezPublicKey, commitment, denomIndex, leafIndex, fee)`. The owner can set the rate within 3% and withdraw collected fees, nothing else. Not part of the trust-minimised bridge (`DESIGN.md` §3.6). Runtime 3,932 bytes (CI `forge build --sizes`, run `43501320143`). Tests: `test/DepositDesk.t.sol` (25, against the real vault), `test/SepoliaDeskScript.t.sol` (2). Deploy: `script/SepoliaDesk.s.sol`; change the rate: `script/SepoliaDeskSetFee.s.sol` (owner key, `FEE_BPS`; `DEPLOYMENTS.md`). The production fee is the vault's own `depositFee`/`withdrawFee` (≤ 2%); fee policy and what the fee funds: `../research/open-settlement.md`.

```sh
forge test                          # 182 tests (CI run 43587383272): unit, fuzz, a stateful invariant suite (Invariant.t.sol), cross-language vectors, gas
FOUNDRY_PROFILE=ci forge test       # what CI runs: 1024 fuzz runs, 256 invariant runs x depth 128
FOUNDRY_PROFILE=deep forge test     # occasional deep run, slow
```

`foundry.toml` sets `extra_output = ["storageLayout"]` and read access to `out/` (`Privacy.t.sol` and `Admin.t.sol` read the build artifacts). An existing checkout built before that change needs `forge build --force` once.

**Size:** built with via-IR and `optimizer_runs = 200`; the runtime is 20,614 bytes, 3,962 under EIP-170 (the v2 Sepolia deploy, `DEPLOYMENTS.md`); the current source, with the `NoStateAdvance` check, is 20,642 bytes (CI `forge build --sizes`). The v1 build (`optimizer_runs` 10000) was 22,721 bytes. `src/UpdateGatedVerifier.sol` (v4, `public-demo/V4-PLAN.md`): runtime 2,049 bytes (`forge build --sizes`, 2026-11-15); `BridgeVault` unchanged. `src/failsafe/BridgeVaultFailsafe.sol` (the failsafe demo variant, branch `failsafe`, `../public-demo/prove/failsafe-20261117/README.md`): runtime 23,133 bytes, 1,443 under EIP-170 (`forge build --sizes`, Foundry 1.5.1, 2026-11-17); `BridgeVault` unchanged (its runtime is still 20,642 bytes and its source byte-identical).

**Gas** (`../research/gas-sepolia-vs-local.md`): transaction receipts from a local anvil (Foundry 1.9.0), mock verifier, caps on, measured on the v1 source. Sepolia runs Ethereum's next fork, Glamsterdam, which reprices state creation (EIP-8037, EIP-8038, EIP-2780); all 18 v1 Sepolia receipts were reproduced to the unit with `--hardfork amsterdam`, and the same transactions under `--hardfork osaka` give today's mainnet-rule costs. v2 deposits on Sepolia cost about 1,400 gas more than v1's (deposit 2: 510,476 vs 509,076).

| Operation | Mainnet rules (Osaka) | Glamsterdam (emulated) |
|---|---:|---:|
| Deploy BridgeVault | 5,140,054 | 36,133,590 (v2 on Sepolia: 32,895,648) |
| ERC-20 deposit, first ever / steady | 258,328 / 174,993 | 897,568 / 410,133 |
| ETH deposit, first / second | 178,670 / 166,286 | 478,630 / 385,426 |
| `update`, 1 exit / 32 exits | 219,084 / 1,118,712 | 756,104 / 1,368,452 |
| `release`, first / same recipient again | 163,848 / 129,648 | 458,808 / 262,968 |

- **Add RISC Zero Groth16 verification, about 230k execution gas, to `update` and `release`.** Measured with `eth_estimateGas` on a direct `verify` call with risc0-ethereum's test seal: 256,164 under local Osaka minus 26,640 intrinsic gas, about 229.5k (250,376 on Sepolia). Nothing has run through `update` or `release` with a real seal.
- `forge test --match-contract GasTest --isolate --gas-report` gives call-level `gasleft()` figures under forge's own EVM. They do not see Glamsterdam state gas, so quote the receipts above.

Not built yet:
- the contest window for competing G2 chains (M4);
- an integration test against the real `RiscZeroGroth16Verifier` with a real receipt.

Deployments: `DEPLOYMENTS.md` (Sepolia demo vaults: deposits only, no exit possible).
