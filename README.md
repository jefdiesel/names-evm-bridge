# Private ERC-20 bridge for LEZ (RFP-022) with Ethereum state attestation (RFP-021)

**Work in progress.** This repository implements two Logos RFPs on LEZ `v0.4.0-rc3` (`bafd07be`):

- **RFP-022, private wrapped-ERC-20 and ETH bridge:**
  - the Ethereum vault (`vault/`), and a demo-network fee contract in front of it (`vault/src/desk/`);
  - the LEZ wrapped-token and bridge programs (`lez-bridge/token`, `lez-bridge/bridge`);
  - the release proof (`lez-bridge/release`, `lez-bridge/methods/release_guest`), the outbound state proof with the LEZ tip trusted (G2-lite, `lez-bridge/outbound`) and the Groth16 wrap (`lez-bridge/outbound/wrap`, `.github/workflows/wrap-outbound.yml`, `wrap-private.yml`);
  - the CLI (`lez-bridge/cli`) and the recovery client (`lez-bridge/client`);
  - a Logos core module and mini-app (`modules/`) and a static deposit page for ordinary Ethereum wallets (`web/deposit/`).
- **RFP-021, trustless Ethereum state attestation:**
  - a sync-committee light client inside a LEZ program, with storage-slot, account and receipt-log inclusion checks (`lez-bridge/attest`);
  - its CLI client (`lez-bridge attest …`);
  - `attest-spike/`, the earlier measurement harness.

[`DESIGN.md`](DESIGN.md) is the design as built: refusals, gas, privacy, deviations, and requirement matrices with test names. Where it and the code disagree, the code wins.

Security problems: report them privately as [`SECURITY.md`](SECURITY.md) says. Everything deployed is on test networks with test coins, and nothing has been audited.

## Current status (2026-11-17)

**Where things have run:**
- **Ethereum:** demo vaults on the **public Sepolia testnet** (`vault/DEPLOYMENTS.md`).
  - **v4** (`0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed`, 2026-11-15) and **v5** (`0xAFbDF192434E9888Bb23DAE6f2D15Eea6246eD10`, the same design with no release delay, the deposit page's default): every update and release proof is checked by RISC Zero's `RiscZeroGroth16Verifier` 3.1.0 (`0x8c915b5071206C6E328BC18529586c513408aB3b`), behind `vault/src/UpdateGatedVerifier.sol`, which takes updates only from one submitter key and releases from anyone.
  - **Our first round trip with real proofs ran on v4 on 2026-11-15:** a team deposit (Sepolia block 12142034), a relayed claim and a burn on the public demo chain (LEZ blocks 10359, 10361), the outbound update proved on CPU and accepted (Sepolia block 12142209), the release proved and paid after the one-hour delay (block 12142503); a second update was accepted in block 12143000. Proofs, seals and numbers: `public-demo/prove/receipts/v4-*/`; re-check them read-only with `docs/VERIFY-ROUND-TRIP.md`. The v4 and v5 vaults, gates and fee contracts are source-verified on Sourcify (exact match). **What it trusts:** which LEZ blocks are final is the submitter's word (one key, ours); deposit roots come from the operator attester; private transactions and public execution are taken as the sequencer applied them. LEZ blocks carry no state commitment, so the proof walks every block (about 20,000 cycles each, `DESIGN.md` §4). Since 2026-11-16 an unattended updater of ours proves and posts v5's updates, about every two hours, and a settlement service sends releases; each Groth16 wrap runs on a GitHub-hosted runner. A forged outbound update could release up to everything deposited and not yet released, within the caps and the release delay; v5 has no release delay, a choice for this test network only. If we stopped, exits on v4 and v5 would stop (only our submitter key can post updates); deposits stay in the vault. Test funds only.
  - **A private round trip ran on v5 on 2026-11-16:** a private claim and a private burn on the public demo chain (LEZ blocks 15255, 15395), the update accepted (Sepolia block 12149823) and the release paid (block 12149867); record in `public-demo/prove/receipts/v5-update-1/`.
  - v1 and v2 take deposits only (placeholder update image). v3 (2026-11-14, the fixed LEZ programs, an operator-attested verifier) was deployed with its G2 state at zero, which no update can follow, so nothing can be released from it; the deploy scripts now refuse that.
  - RISC Zero advisories GHSA-mm6c-35cm-g4x2 (the verifier contracts) and GHSA-g6qg-9j8p-7j87 (the crates), both 2026-11-11: verifiers up to 3.0.0 accept forged claims. v1/v2 bound 3.0.0: no honest proof can release either, a forged one could (test ETH only); the outbound and release provers here use risc0-zkvm 3.1.0. LEZ v0.4.0-rc3 itself pins 3.0.5 (LEZ's `dev` branch has 3.1.0); by our reading, private transactions on an rc3 chain rest on its operator until a LEZ release ships the fix.
  - **Version alignment:** `VERSIONS.toml` and `scripts/check-versions.mjs` (in CI; `--onchain` before any deploy) check prover, verifier, image ids, LEZ versions and genesis state against each other.
- **LEZ, local:** local LEZ 0.4 standalone sequencers. The trustless RFP-021 runs (a proved light-client update, attested claims of Sepolia deposits, public and private) ran there: on LEZ v0.4.0-rc3 on 2026-11-14 (Run 2, `DESIGN.md` §9.0a, logs in `public-demo/prove/run2-20261114/`), and earlier on LEZ v0.4.0-rc1.
- **LEZ, public demo chain:** a LEZ v0.4.0-rc3 sequencer run by the maintainer at `https://lez.nameswallet.app/` (JSON-RPC, POST only), with one bridge instance per vault (v2 to v5). Each takes vault roots from an **operator attester** (`public-demo/attester/root-attester.py`), not from the RFP-021 light client. A relay we run co-signs public claims and public burns and pays their LEZ fee (`public-demo/relayer/`); a test-LEZ faucet answers on the same endpoint. It is a demo network, not a Logos network. LEZ testnet 0.4 is not live.
- **The open question:** a trustless outbound proof needs a LEZ state commitment and a way to show LEZ finality outside LEZ. We asked the LEZ team about their plans in logos-blockchain/logos-execution-zone issue #1129, and proposed slashing on proof of double-signing in issue #1130.

States used below and in `DESIGN.md`: **(1)** run on a chain (which one is named), **(2)** tested in simulation or unit tests only, **(3)** designed only, **(4)** not started. **Mock-verified** means a local run where the vault used the test `MockVerifier` and the zkVM guests were executed, not proved.

| Part | Where | State |
|---|---|---|
| Vault deposits (ERC-20 and ETH), registry, caps and relay-fee menu configuration | `vault/src/BridgeVault.sol`, `vault/DEPLOYMENTS.md` | 1, Sepolia demo vaults v2 (9 deposits), v3 (10) and v4 (6), read 2026-11-16 |
| Vault `update`, `release`, caps, freeze, delay history, payee checks, `NoStateAdvance` | `vault/src/BridgeVault.sol`, `vault/src/UpdateGatedVerifier.sol` | 1 on Sepolia (v4: two updates, a release by proof; LEZ finality the submitter's word); 1 on local anvil, mock-verified; 2 |
| Deposit desk (demo-network bridge fee in front of each demo vault: owner-set, hard cap 3%, 0.3% since 2026-11-14; not part of the trust model) | `vault/src/desk/`, `DESIGN.md` §3.6 | 1, deployed on Sepolia, one per vault |
| Wrapped-token program (`MintBy`, `BurnWithNotice`) | `lez-bridge/token` | 1 |
| Bridge program, RFP-021 root source: attested claims of Sepolia deposits, public and private (proved), with no trusted key | `lez-bridge/bridge`, `lez-bridge/cli` | 1 (Sepolia → local LEZ: rc3, and earlier rc1) |
| Bridge program, operator-attested root source: public claims of Sepolia deposits, and one private claim (proved; LEZ block 3069) | `lez-bridge/bridge`, `public-demo/attester/` | 1 (Sepolia → public demo chain, rc3) |
| One claim refusal (`claim refused`), local diagnosis in the CLI | `lez-bridge/bridge`, `lez-bridge/cli` | 1 |
| One-time recipient address and fee float for private claims | `lez-bridge/bridge`, `lez-bridge/cli` | 1 (local) |
| RFP-021 attestation program, consumer library, reference consumer, CLI client: `Init` at a real Sepolia checkpoint, **a proved finality update**, statements | `lez-bridge/attest`, `lez-bridge/cli/src/attest.rs` | 1 (local LEZ: rc3, and earlier rc1) |
| RFP-021 period update (committee handoff) | `lez-bridge/attest` | 2 (verified natively, dry run only) |
| Burn on LEZ (exit commitment) | `lez-bridge/bridge` | 1 (public demo chain, self-paid and relayed; local) |
| Recovery from the wallet: seed-derived deposit and exit secrets, scan of both chains, `recover` | `lez-bridge/client`, `lez-bridge/cli/src/recover.rs`, `lez-bridge/scripts/e2e-recovery*.sh` | 1 (local, public accounts); private path dry run only |
| Release proof (G3) guest | `lez-bridge/release`, `lez-bridge/methods/release_guest` | 1: proved (20.9 s, CPU), wrapped and paid on Sepolia (v4) |
| Outbound state proof, G2-lite (LEZ block linkage, burn and claim extraction; the LEZ tip trusted) | `lez-bridge/outbound` | 1: two updates proved on CPU and accepted on Sepolia (v4), LEZ finality the submitter's word |
| Round trip deposit → claim → burn → update → release | `lez-bridge/scripts/e2e-roundtrip.sh`; `public-demo/prove/receipts/v4-*/` | 1 on Sepolia + the public demo chain with real proofs (once, v4, **LEZ finality trusted**); 1 on local chains, mock-verified |
| Logos Blockchain finality for G2 (the trustless outbound proof) | | 4 |
| Groth16 wrap of update and release proofs | `lez-bridge/outbound/wrap`, `.github/workflows/wrap-outbound.yml`, `wrap-private.yml` | 1, by hand on a GitHub-hosted runner (about 164 s per update, 243 s per release) |
| Contest window for competing G2 chains | | 4 |
| Failsafe (wind-down and reclaim) | `research/failsafe-design.md` | 3 |
| Relay (co-signing LEZ fee payer for public claims and burns) | `lez-bridge/cli/src/relay.rs`, `public-demo/relayer/`, `DESIGN.md` §3.6 | 1 on the public demo chain, one operator (the team's) |
| Open, fee-paid settlement roles (update rewards, open claim relays, G2 update rewards); release relayer daemon | `research/open-settlement.md` | 3 |
| Logos core module and mini-app (Names Wallet mode by default) | `modules/` | 1 once: "Claim a deposit" from a team test Basecamp with an unreleased Names Wallet build, into a never-used account, the relay paying (public demo chain, LEZ block 4852, 2026-11-14); otherwise 2 (unit tests, offscreen renders); not released |
| Deposit page for ordinary Ethereum wallets | `web/deposit/` | 2 (102 offline tests); live read-only checks on Sepolia |
| SPEL IDL, catalog release, update daemon, wrap service | | 4 |
| Deployment on LEZ testnet 0.4 | | 4 (not live) |
| CI (`.github/workflows/ci.yml`) | | version alignment; 198 forge tests (1 fork test skipped); 214 LEZ-side Rust tests (no guest build) |

Releases with real proofs have run on Sepolia (v4). LEZ finality is trusted in every outbound run: no outbound proof here is trustless.

## Layout

| Path | What |
|---|---|
| `vault/` | Foundry project: `BridgeVault.sol` and its SHA-256 incremental tree; `src/demo/` (operator-attested demo verifier, deployed for vault v3 on 2026-11-14); `src/desk/` (deposit desk); tests (unit, fuzz, stateful invariants, gas, cross-language vectors); local, Sepolia and gas scripts; `DEPLOYMENTS.md` and `deployments/*.json` |
| `lez-bridge/` | Rust workspace for LEZ v0.4.0-rc3: `token/`, `bridge/`, `attest/{core,program,probe,methods}`, `methods/` (guests), `cli/`, `client/`, `scripts/`. `release/`, `outbound/` and `attest/cycles/` are separate workspaces |
| `modules/` | Logos core module `lez_bridge` (Rust library, Names Wallet mode and CLI wallet mode), mini-app `lez_bridge_ui` (QML), the CLI contract, recorded fixtures |
| `web/deposit/` | Static deposit page (no dependencies), its tests and `MANIFEST.sha256` |
| `public-demo/` | The operator attester, the fee-grants daemon and its tests, the RPC filter, recorded results of the public demo chain; `prove/run2-20261114/`: logs of the rc3 trustless run (local chain); `prove/run1-20261114/`: logs of a private claim on the public demo chain |
| `attest-spike/` | Executor-only cycle measurements of a Helios light-client update and an `eth_getProof` storage proof in RISC Zero 3.0.5 |
| `research/` | Notes: what LEZ and the Logos Blockchain let a third party prove; the cost of checking a Logos Blockchain leader proof; the attested claims from Sepolia; Ethereum gas under Sepolia's fork and mainnet rules; the failsafe design |
| `DESIGN.md` | Design as built, trust model, privacy analysis, refusals, gas, deviations, requirement matrices |

## Build and test

**Requirements:**
- Foundry (CI pins v1.5.1);
- rustup. The Rust toolchain is pinned in `rust-toolchain.toml` (1.98.1).
- **For zkVM guests:** RISC Zero's `rzup` with cargo-risczero, r0vm and the risc0 Rust toolchain at 3.0.5, the versions LEZ v0.4.0-rc3 pins.
- Node.js (deposit page and mini-app logic tests; nothing to install) and Python 3 (daemon tests).

The first build fetches LEZ, Helios and the RISC Zero crypto forks from GitHub. Every git dependency is public.

### Vault (Ethereum)

```sh
git submodule update --init      # forge-std and OpenZeppelin, pinned (see the end of this file)
cd vault
forge build --sizes              # fails if a contract exceeds EIP-170
forge test                       # 198 tests (1 fork test skipped without SEPOLIA_FORK_RPC): unit, fuzz, stateful invariants, gas, vectors, desk, demo verifier
FOUNDRY_PROFILE=ci forge test    # what CI runs
```

### LEZ programs (no guest build)

```sh
cd lez-bridge
RISC0_SKIP_BUILD=1 cargo test -j 4 -p bridge_token_core -p bridge_core -p eth_attest_core -p eth_attest -p eth_attest_probe
RISC0_SKIP_BUILD=1 cargo test -j 4 -p bridge_client --no-fail-fast
(cd release && RISC0_SKIP_BUILD=1 cargo test -j 4 -p release_core)
```

These are what CI runs: 149 tests (token 27, bridge 78, attestation 44), `bridge_client` 32, `release_core` 3. They run the programs natively against LEZ's whole-transaction simulator. The attestation tests verify real Sepolia (Gloas) and mainnet (Fulu) light-client updates from `lez-bridge/attest/fixtures/`.

### Guests, CLI, release and outbound tests

These need the RISC Zero toolchain. Nothing here generates a proof, except the CLI's private transactions.

```sh
cd lez-bridge
cargo build --release -j 4 -p lez_bridge_cli     # the lez-bridge CLI (embeds the token, bridge, attestation and probe guests)
(cd release && cargo test --release -j 4)        # release_core + release_host (G3 guest in the executor)
(cd outbound && cargo test --release -j 4)       # G2-lite, executor only, fixtures from real LEZ blocks
```

### Module, page, daemon

```sh
cd web/deposit && npm test                        # node --test, offline
node modules/lez_bridge_ui/logic.test.mjs         # from the repository root
python3 -I public-demo/attester/test_fee_grants.py
```

The core module's Rust tests (`modules/lez_bridge/rust-lib`) need the Logos Rust SDK checked out beside `rust-lib` as `logos-rust-sdk-src`.

### End to end on local chains

`lez-bridge/scripts/e2e-local.sh` runs deposit → claim → burn on anvil and a local LEZ standalone sequencer; `e2e-recovery.sh` and `e2e-recovery-extra.sh` the recovery checks; `e2e-roundtrip.sh` the mock-verified round trip. They need `anvil`, `cast` and `forge` on the `PATH`, a running sequencer and a release build of the CLI; read each script's header for its variables.

- A private LEZ transaction is proved in-process by the CLI. It needs about 9.5 GB of memory.
- `scripts/guard.sh` wraps each call in a memory and thermal watchdog. It uses macOS tools (`footprint`, `memory_pressure`, `pmset`); on other systems it only runs the command.
- The scripts used for the attested runs and the long private runs are not published, because they are tied to one test machine.

**Public test keys.** The e2e scripts contain private keys that are well-known public test keys and must never hold real funds: anvil's and Hardhat's default accounts (`0xac0974be…ff80`, account 0; `0x7c852118…07a6`, account 3) and the LEZ test-network genesis account A (`PRIVATE_KEY_PUB_ACC_A` in `lez/testnet_initial_state/src/lib.rs` of the LEZ repository).

The Sepolia demo deployer, `0x37a46E7334Dd061E0C7E2298247EE9278d0d321F`, is a throwaway test account. Its key is not in this repository.

## Re-run the demo

One command, from a clean clone, on Linux (x86-64 or arm64) or an Apple-silicon Mac:

```sh
git clone <this repository> bridge && cd bridge
scripts/judge-demo.sh            # --keep leaves the chain running; --help lists the options
```

It checks its prerequisites first and prints the exact install command for each missing one: Rust (rustup), RISC Zero's `rzup` with its Rust toolchain and cargo-risczero 3.0.5, Foundry, clang and pkg-config, and on Linux `libssl-dev` and `libpcsclite-dev`. On Linux x86-64 the CPU must have the ADX and BMI2 instructions (`adx` and `bmi2` in `/proc/cpuinfo`): the LEZ kit's sequencer and wallet link upstream's prebuilt rapidsnark library (a dependency of the Logos node code, not of this repository's code), which uses them, and stop with "Illegal instruction" without them, so the script refuses such a CPU before building. On such a CPU, `scripts/judge-demo.sh --rapidsnark-from-source` builds rapidsnark v0.0.8 here without assembly instead and links the kit against it (needs `cmake` and `m4`; same results, slower field arithmetic; the transcript says so in step 3): that path passed all 17 steps on an Intel Xeon E5-2680 v3 under Ubuntu 26.04, cold from a clean clone and a fresh account: install from the printed hints 155 s, kit with rapidsnark 21 minutes, bridge 25 minutes (12 build jobs), steps 5–17 in 9 minutes; about 12 GB of builds plus 8 GB of toolchains (`docs/judge-demo-linux.log`). Then it fetches the public LEZ 0.4 testnet kit (`github.com/jefdiesel/logos-execution-zone`, branch `testnet-0.4`) at a pinned commit, builds, brings up a throwaway LEZ v0.4.0-rc3 chain and a local anvil, and runs deposit, operator-attested root, a relayed public claim into a never-used account, burn, outbound update and release, ending with the balances reconciled on both sides. Each step prints its command and PASS or FAIL, a summary table closes the run, and the exit status is 1 if any step failed. Without `--keep` everything it started is stopped and its run directory deleted; the builds stay in `run/judge-demo/build/` for the next run (`--purge` deletes them too). `docs/judge-demo.log` is a recorded run (Apple M4, repository commit `5ec591a`) and `docs/judge-demo-linux.log` a cold one on Ubuntu x86-64 with `--rapidsnark-from-source` (commit `3132e80`); it predates the change of two labels (the light-client root and private claims), so those two lines read differently from what the script prints now. `.github/workflows/judge-demo.yml` runs the same script on Ubuntu and uploads the transcript; `docs/judge-demo-linux-github.log` is its run of 2026-11-15 (workflow run 43712831729, commit `3cdad70`) on a GitHub-hosted Linux x86-64 runner with 7 GB of memory, without `--rapidsnark-from-source`: all 17 steps passed in 136 minutes, with the kit's binaries restored from the workflow's cache, the bridge built in 7,667 s at 2 build jobs and steps 5–17 in 524 s.

**Time and machine:** measured on an Apple M4 with 16 GB at 4 build jobs: the first run's builds took about 24 minutes with a warm shared compiler cache (kit 631 s, CLI 544 s, outbound host 249 s; a cold build is longer: the kit alone is 40-55 minutes on a GitHub Ubuntu runner) and about 12 GB of disk; the demo itself took 9 minutes. Later runs reuse the builds. 8 GB of memory or more. On Ubuntu 26.04 x86-64 (a 48-thread Xeon E5-2680 v3, 16 build jobs, another build running alongside, repository commit `31e5c87`) steps 1-4 passed from a clean clone: everything of this repository (the CLI, the outbound host and its guests, the vault) built, cold, with the kit in 2,041 s and the bridge in 5,215 s, peaking at about 12 GB in the work directory. That CPU has no ADX, so the kit's binaries stopped at step 5 (see the CPU requirement above); full Linux runs are in `docs/judge-demo-linux.log` (that CPU, with `--rapidsnark-from-source`) and `docs/judge-demo-linux-github.log` (a GitHub-hosted runner, without it).

**What the labels mean:**

| Label in the output | Meaning |
|---|---|
| throwaway LEZ 0.4 (v0.4.0-rc3) chain you run yourself | one standalone sequencer from the testnet kit, no Logos Blockchain anchoring, LEZ's published test genesis keys. Not a Logos network |
| local Ethereum node (anvil), not Sepolia | every Ethereum transaction goes to a local anvil |
| mock-verified | the vault checks seals with the TEST `MockVerifier`, which accepts `imageId \| sha256(journal)`. No real verifier runs |
| operator-attested … Not the trustless path | the demo operator records the vault's deposit root on LEZ. The RFP-021 light-client path (a proved sync-committee update, then attested claims) has been proved only on local chains (LEZ rc3 on 2026-11-14, and earlier rc1); it does not run here. The label printed by the script predates the rc3 run |
| the relay co-signs as fee payer | the never-used account signs its own claim and the relay's dedicated payer pays the fee, so the account needs no LEZ. A demo-network convenience, not part of the trust-minimised design |
| Simulated: proofs executed, not proved, and LEZ finality trusted | the G2-lite outbound guest and the G3 release guest run in RISC Zero's executor; nothing is proved, and the last LEZ block of each batch is taken as final |
| not shown | private claims (each takes about 11 minutes and 9.4 GB to prove; the label printed by the script predates the rc3 private proofs, `DESIGN.md` §3.3a, §9.0a), the light client, Sepolia, the deposit desk |

## Measured numbers

**Machine:** an Apple M4 with 16 GB, 4 threads, LEZ's CPU prover (`RISC0_PROVER=local`). The proving figures come from local run logs; `DESIGN.md` cites each run by name. Included: the proof log of the rc1 private attested claim in LEZ block 4998 (`modules/fixtures/source/out-private-claim.log`) and a selection of the rc3 Run 2 and Run 1 logs (`public-demo/prove/run2-20261114/`, `public-demo/prove/run1-20261114/`).

| What | Number | Source |
|---|---|---|
| Proving one private LEZ transaction, rc3: attested claim of a Sepolia deposit | 676.0 s, 9,391 MB peak footprint, LEZ block 463 (local chain) | `DESIGN.md` §9.0a, `public-demo/prove/run2-20261114/out-claim-private.log` |
| Proving one private LEZ transaction, rc3, public demo chain: claim under the operator-attested root | 569.8 s, 9,537 MB peak footprint, LEZ block 3069 | `DESIGN.md` §3.3a, `public-demo/prove/run1-20261114/out-claim.log` |
| Proving the RFP-021 finality update, rc3 (real Sepolia, 507/512 signers) | 7,283.0 s, 9,457 MB peak footprint, LEZ block 950 (local chain) | `DESIGN.md` §9.0a, `public-demo/prove/run2-20261114/summary.txt` |
| Proving one private LEZ transaction, rc1: attested claims of Sepolia deposits; stand-in claims and burn | 666.2 s and 670.9 s, 9,374 and 9,403 MB peak footprint; about 600–770 s and 9.5 GB | `DESIGN.md` §3.3, §12 (P4) |
| Proving the RFP-021 finality update, rc1 (real Sepolia, 509/512 signers) | 7,074.7 s, 9,483 MB peak footprint, LEZ block 1859 | `DESIGN.md` §9.0 |
| Attestation `Init` at a real Sepolia checkpoint (public) | 9,105,915 cycles of the 10M per-transaction cap | `DESIGN.md` §9.0 |
| Attested public claim / stand-in public claim (rc1) | 390,756 / 150,889 cycles | `research/attested-claim-sepolia.md` |
| Operator-attested public claim on the public demo chain (rc3) | fee 1,232,488 at base fees 8/8; 152,168 cycles | `public-demo/claim-p2-public-rc3-20261113.txt`, `DESIGN.md` §1 |
| Executor cycles: update 37.2M (Sepolia) / 39.5M (mainnet); period update 46.3M | | `lez-bridge/attest/cycles/results/run-20261112T165250Z.txt` |
| Release guest (G3) | 36.4K cycles, 1 segment | `lez-bridge/README.md` (executor) |
| Ethereum gas, steady ERC-20 deposit with caps: mainnet rules (Osaka) / Sepolia's fork (Glamsterdam) | 174,993 / 410,133 | `research/gas-sepolia-vs-local.md` |
| RISC Zero Groth16 verification (to add to `update` and `release`) | about 230k execution gas | `research/gas-sepolia-vs-local.md` |
| One Logos Blockchain leader-proof check in a RISC Zero guest | 49.2M cycles; 9,778 s to prove, 4.85 GB | `research/spike-pol-cycles.md` |

**Not measured:** proof size for any action; the Groth16 wrap; `update` or `release` with a real seal; a period update (committee handoff) proved; more than one run of each rc3 proof; proving on a laptop, on a GPU, or a stated minimum spec.

## Trust model (summary of `DESIGN.md` §0, §5, §7)

- **Inbound, Ethereum → LEZ, RFP-021 path.** A LEZ program follows Ethereum's sync committee. The claimer's own execution checks an `eth_getProof` storage proof that the vault's `isDepositRoot[root]` is `true`, under an execution anchor that program finalized. This has run with Sepolia deposits on a local LEZ chain, with no trusted key.
- **Inbound, operator-attested path (the public demo chain only).** An operator key records vault roots. It could record a fake root. It exists so that visitors can try the flow before the light client runs on a shared network; it is labelled as such everywhere.
- **Outbound, LEZ → Ethereum: not trustless on LEZ 0.4, for any design.** LEZ publishes no state root and never proves public execution, and no Logos Blockchain light client can work from headers alone (`research/lez-and-l1-provability.md`). Closing the gap needs a ledger root in the Logos Blockchain header (a LIP) or a LEZ zone validity prover.
- **Solvency bound.** The vault enforces `released ≤ burned ≤ claimed ≤ deposited` per token, against anyone. A fully malicious LEZ sequencer committee could forge burns up to the claimed-but-unburned value, never more than was deposited, and only within the release caps and delay.
- **Roles.** Admin: registry, caps, fees (each at most 2%), relay-fee menu, release delay (at least an immutable minimum), sponsor amount, unfreezing; it cannot touch principal or change the verifier or image ids. Freezer: freeze only. Relayers, attestation updaters and provers: liveness only.

## What a claim and a burn make public (summary of `DESIGN.md` §3.4 and §8)

- **Private claim:** published are the token and fixed denomination, the time, the anchor, instance, vault and `cfg_id`, the claim nullifier, a one-time recipient address and the fixed sponsor amount, the fee. Not published: the claimer's account, the deposit secret, commitment and leaf, the deposit root. Checked by byte searches of blocks and allowlist tests; these are leak checks, not a measured unlinkability result.
- **Public claim:** publishes the deposit secret, so it links the depositor to the claimer. By design.
- **Refusals:** every deposit-dependent claim failure gives the same text, `claim refused`; the CLI names the cause locally before anything is sent.
- **Burn:** publishes the exit commitment and the denomination, not the Ethereum destination.
- **Ethereum:** a deposit shows the depositor, token, denomination and commitment; a release shows the recipient, relayer and relay fee.

## Fixtures and recorded data

- `lez-bridge/attest/fixtures/`, `attest-spike/fixtures/`: Ethereum beacon-chain light-client data fetched from public beacon APIs; `meta.json` records the endpoints and fetch times.
- `lez-bridge/outbound/fixtures/`: LEZ blocks from local test chains (rc1 and rc3).
- `lez-bridge/release/fixtures/`, `vault/test/fixtures/`, `lez-bridge/bridge/tests/fixtures/`, `lez-bridge/client/tests/vectors/`: generated from this repository's code and fixed test inputs.
- `modules/fixtures/`: replies recorded from the CLI against the Sepolia v2 demo; local paths replaced by `<local path>`.
- `research/gas-replay/`, `vault/deployments/*.json`: public Sepolia data.

## Not in this repository

- Local run logs (except the selection above), wallets and secrets.
- The operations notes of the public demo chain (tunnel, edge rules, host setup) and the scripts tied to one test machine.
- Foundry broadcast logs of the Sepolia deployments. Their transactions are on chain, and `vault/DEPLOYMENTS.md` lists them.
- The mini-app's rendered screenshots (they show local paths); `modules/lez_bridge_ui/tools/render.py` regenerates them.

## Licence

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Workspace crates declare `MIT OR Apache-2.0`, and source files we wrote carry `SPDX-License-Identifier: MIT OR Apache-2.0`. The Solidity test vectors generated by the release and outbound hosts (`vault/test/fixtures/`, `lez-bridge/release/fixtures/`) carry the `MIT` header their generators write.

**Third-party code:**
- Helios `consensus-core` 0.12.0 (MIT), used unmodified;
- RISC Zero crates and forks (Apache-2.0 / MIT);
- the BIP-39 English word list (MIT, Pavol Rusnak), unchanged, for the compare words: `modules/lez_bridge/rust-lib/data/bip39-english.txt` (notice beside it) and `web/deposit/src/lib/vendor/bip39-english.js`;
- forge-std and OpenZeppelin Contracts (submodules, under their own licences).

The attestation guest's dependency tree includes `fastrlp` (MPL-2.0, file-level copyleft), through alloy.

## Vault dependencies (git submodules)

Run `git submodule update --init` after cloning. Pinned to the commits in `vault/foundry.lock`:

- `vault/lib/forge-std`: https://github.com/foundry-rs/forge-std, v1.17.0, commit `f3dae6e6ee381f25eb6a246f7da9b85c91a68219`
- `vault/lib/openzeppelin-contracts`: https://github.com/OpenZeppelin/openzeppelin-contracts, v5.1.0, commit `69c8def5f222ff96f2b5beff05dfba996368aa79`
