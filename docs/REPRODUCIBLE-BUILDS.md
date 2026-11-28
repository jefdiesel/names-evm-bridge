# Reproducible zkVM Guest Builds

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

**Status (2026-11-28):** every deployable guest (10) builds in RISC Zero's Docker builder with the same image id and the same ELF sha256 on two clean machines: two fresh GitHub-hosted `ubuntu-latest` runners, no cache of any kind (CI job `repro-guest-builds`, pre-merge run 46095730461 on a test branch of the public repository, the same guest inputs as this commit; about 39 minutes per build). The ids are pinned in `VERSIONS.toml` `[[repro_guest]]` and listed below. **No deployed image id reproduces:** every deployed guest (v2–v7) was built natively before this, and its id differs from the Docker one even where the guest's source and lock have not changed since the deploy (table at the end). Not yet checked: a third machine outside GitHub (an arm64 Mac running the amd64 builder under emulation).

## Docker image ids (main, 2026-11-28)

| Guest | Image id | ELF sha256 (`<guest>.bin`, user ELF + kernel) |
|---|---|---|
| `token_guest` | `0x243bc09d66c1e742725b5fb6192b0a2d06772d95d7d00759d12dce8aa46b91de` | `e81a57701fc68dbbe53511078e544a8337dea29a889c24767b2a3ba3d4856af0` |
| `bridge_guest` | `0x4003fd0d9112728364dec5c784983fe72f2e9fe5fbe270974cbd8af9bd129adb` | `b7ba2c04ae9f053724a173404952d09a50398b5e4bfbbc32e29eccb11ebde684` |
| `attest_guest` | `0x4b7209e82ed3c3647863e9d6700948b833126fc0e6cbe428411248827717d1ff` | `7433279fa849097e9eaa324f0b1837571f4364204c7c2687a07aa9a698e0c068` |
| `attest_probe_guest` | `0xad23410addf9b681fcce9e474d03714bd04cbc2b21d0fc9ae2376bd405460f29` | `7afe3be0ad4ff54b400f87cb2cfa4a3b58f99094b08420711887d9ece53172c2` |
| `outbound_guest` | `0xc12da7fa92b942ad3b2d52222c62ddebe970625d95dfd9f9d98d54f59d410589` | `1046ad4bbae30ae470c5eeaf576e88e9c3daa84db69146e56888096b221c3847` |
| `outbound_guest_trusted_tip` | `0xee5056bafa30aa172bc69cf074552a01292e20edb1582e8fd7803a719626d7ba` | `13d1abc022e03fd061ec16f09373dcfe27fb4898ded68e436673d6690b34bc5f` |
| `release_guest` | `0x98728e5b83c7b3b2e808b9d44aae8eed58d240c139a523a65e1a26ca4f1e32f9` | `3c4e85a274bc64163e11a2f7fef9b7ca77a6b874a2128362703ed690d4c0e8d9` |
| `outbound_guest_seqsig` | `0x807ea870608801031f19cf3595d7bd86364b88ee50f746e5ec16c0a26d99dd01` | `7d7236780f756ccb72971f11c54c1421be78cc6e591893dfa09509f99aa61ce2` |
| `seqsig_evidence_guest` | `0x17c58454aa6db2c2eb27e288c11b663fe1df3ec4b23a63b56bd17a7cdf2efac9` | `b9929c47840463cc63caa1e9b75ce3c419f1dde68ef3d3754015bd498df769bc` |
| `reclaim_guest` | `0x5ef74cc78ad5d5560962b928f14511beb14d7adc4f2d5596220938842276937c` | `63f6ad46d9aad0ea1016b2077f2127cfbd41caa4db83b6ab00cfb38834ccaa61` |

`release_guest` is built twice per run (through `outbound_host` and `release_host`); the script fails if the two differ. A change to a guest's source, its `Cargo.lock`, or its host's `risc0-build`/kernel version changes its id: re-run the CI job and re-pin in the same commit.

## Why native guest builds were not reproducible

Earlier builds of the LEZ programs and Ethereum-verified guests ran `risc0_build::embed_methods()` natively on the host without Docker (`public-demo/v3/BUILD.txt`, `public-demo/v4/BUILD-outbound.txt`). Native builds embed absolute host and Cargo registry paths in panic locations and depend on the host OS, architecture, and local toolchain, so rebuilding the same commit on another machine or in another directory produced a different ELF and a different RISC Zero image ID (`DESIGN.md` header).

Enabling RISC Zero's Docker builder (`RISC0_USE_DOCKER=1` with `risc0_build::DockerOptionsBuilder::root_dir(...)`) compiles every guest inside a fixed Linux container image with fixed mount paths (`/src`) and a fixed Rust/RISC-V toolchain, so anyone building from the same commit and lockfiles reproduces the exact same image ID.

## What is pinned

Every build-affecting version is declared in `VERSIONS.toml` and enforced offline by `scripts/check-versions.mjs` (in CI on every push and pull request).

| Component | Pinned value | Source / enforcement |
|---|---|---|
| Host Rust toolchain | `1.98.1` | `lez-bridge/rust-toolchain.toml`, `attest-spike/rust-toolchain.toml`, `VERSIONS.toml` `[toolchain].rust` |
| `rzup` | `0.5.0` | `VERSIONS.toml` `[toolchain].rzup` |
| RISC Zero Rust toolchain | `1.97.0` | `VERSIONS.toml` `[toolchain].risc0_rust` |
| `cargo-risczero` | `3.0.5` | `VERSIONS.toml` `[toolchain].cargo_risczero` |
| LEZ dependency | `v0.4.0-rc3` (`bafd07be0701398f9bc435f73ab05e9a7936c301`) | `VERSIONS.toml` `[lez.rc3]`, every `Cargo.toml` and `Cargo.lock` |
| LEZ guests (`lez_guests`) | `risc0-zkvm =3.0.5`, `risc0-build =3.0.5` | `VERSIONS.toml` `[deployments.v5.roles.lez_guests]` |
| Ethereum guests (`eth_guests`) | `risc0-zkvm =3.1.0` (`outbound_guest*`, `seqsig_*`, `reclaim_guest`) and `=3.0.5` (`release_guest`); `risc0-build =3.0.7` | `VERSIONS.toml` `[deployments.v5.roles.eth_guests]` and `[deployments.v5.roles.eth_provers]` |
| Docker builder image | `risczero/risc0-guest-builder` | `VERSIONS.toml` `[toolchain].docker_builder_image` |
| Docker builder tag | `r0.1.97.0` | `VERSIONS.toml` `[toolchain].docker_builder_tag` |
| Docker builder digest | `sha256:7ae0a27fa72b10fb5c1066005bd949ae43dc4ead8a284f2d6572b99c82ab4468` | `VERSIONS.toml` `[toolchain].docker_builder_digest` (pin before M1 acceptance; see below) |
| Lockfiles | Tracked `Cargo.lock` per workspace (`--locked` everywhere) | `VERSIONS.toml` `[[workspace]]`, `scripts/check-versions.mjs` |

### Pinning and verifying the Docker builder digest

`scripts/repro-build.sh` refuses to pull by mutable tag alone. It pulls strictly by content-addressed digest (`docker pull <image>@<digest>`), checks `RepoDigests` matches the pinned digest, tags the verified image locally, and passes `RISC0_DOCKER_CONTAINER_TAG=<tag>@<digest>`, so risc0-build's generated Dockerfile says `FROM risczero/risc0-guest-builder:r0.1.97.0@sha256:…` and Docker itself refuses any other image. This also overrides risc0-build's own default tags (3.0.5: `r0.1.88.0`; 3.0.7: `r0.1.97.0`), so the LEZ guests (risc0-build 3.0.5) and the Ethereum guests (3.0.7) build in the same image. The pinned digest is the tag's multi-platform index (linux/amd64 plus an attestation); the image is published for linux/amd64 only.

To obtain or update the digest on a machine with Docker:

```sh
docker buildx imagetools inspect risczero/risc0-guest-builder:r0.1.97.0
# or:
docker pull risczero/risc0-guest-builder:r0.1.97.0
docker image inspect --format='{{json .RepoDigests}}' risczero/risc0-guest-builder:r0.1.97.0
```

Record the `sha256:...` digest in the same commit in three places (kept in sync by `scripts/check-versions.mjs`):
1. `VERSIONS.toml` (`[toolchain].docker_builder_digest`)
2. `scripts/repro-build.sh` (`BUILDER_DIGEST`)
3. This file (table above)

## Guest inventory

### Deployable guests (built by default by `scripts/repro-build.sh`)

Every guest that is or will be deployed on LEZ or verified by an Ethereum vault has `RISC0_USE_DOCKER=1` support in its host/methods `build.rs` with `DockerOptionsBuilder::default().root_dir(...)` pointing to `lez-bridge/`:

| Guest package | Workspace / lockfile | Host/methods `build.rs` | Role | `risc0-zkvm` | `risc0-build` | Target |
|---|---|---|---|---|---|---|
| `token_guest` | `lez-bridge` | `lez-bridge/methods/build.rs` | `lez_guests` | `3.0.5` | `3.0.5` | LEZ wrapped-token program |
| `bridge_guest` | `lez-bridge` | `lez-bridge/methods/build.rs` | `lez_guests` | `3.0.5` | `3.0.5` | LEZ bridge program |
| `attest_guest` | `lez-bridge/methods/attest_guest` | `lez-bridge/attest/methods/build.rs` | `lez_guests` | `3.0.5` | `3.0.5` | LEZ RFP-021 light-client program |
| `attest_probe_guest` | `lez-bridge/methods/attest_probe_guest` | `lez-bridge/attest/methods/build.rs` | `lez_guests` | `3.0.5` | `3.0.5` | LEZ RFP-021 reference consumer |
| `outbound_guest` | `lez-bridge/methods/outbound_guest` | `lez-bridge/outbound/host/build.rs` | `eth_guests` | `3.1.0` | `3.0.7` | G2 release update (M6 if go) |
| `outbound_guest_trusted_tip` | `lez-bridge/methods/outbound_guest_trusted_tip` | `lez-bridge/outbound/host/build.rs` | `eth_guests` | `3.1.0` | `3.0.7` | G2-lite TrustedTip update (`BridgeVault.update`) |
| `release_guest` | `lez-bridge/methods/release_guest` | `lez-bridge/outbound/host/build.rs` and `lez-bridge/release/host/build.rs` | `eth_guests` | `3.0.5` | `3.0.7` | G3 release proof (`BridgeVault.release`) |
| `outbound_guest_seqsig` | `lez-bridge/methods/outbound_guest_seqsig` | `lez-bridge/outbound-seqsig/host/build.rs` | `eth_guests` | `3.1.0` | `3.0.7` | G2-lite + threshold signer update (`SeqSigGate`) |
| `seqsig_evidence_guest` | `lez-bridge/methods/seqsig_evidence_guest` | `lez-bridge/outbound-seqsig/host/build.rs` | `eth_guests` | `3.1.0` | `3.0.7` | Equivocation contest evidence (`SeqSigGate`) |
| `reclaim_guest` | `lez-bridge/methods/reclaim_guest` | `lez-bridge/reclaim/host/build.rs` | `eth_guests` | `3.1.0` | `3.0.7` | G4 failsafe reclaim (`BridgeVaultFailsafe`, M6) |

`scripts/repro-build.sh` builds `release_guest` through both `outbound_host` and `release_host` and fails if the two image IDs ever disagree.

### Research / measurement guests (not deployed)

- `attest-spike` (`consensus-guest`, `inclusion-guest`, `inclusion-guest-sw`): executor-only cycle measurement harness (`attest-spike/methods/build.rs` also supports `RISC0_USE_DOCKER=1` with `root_dir = attest-spike/`). Pass `--include-research` to `scripts/repro-build.sh` to build them too.
- `lez-bridge/attest/cycles`: embeds `attest_guest`, `attest_probe_guest`, and `bridge_guest` for cycle counts; its `build.rs` also supports `RISC0_USE_DOCKER=1`.
- `lez-bridge/methods/lc_bench_{zk,blst,fast}` (`lez-bridge/attest/lcbench`): research spike only (`lc_bench_blst` also requires RISC Zero's C toolchain); never deployed, excluded from `scripts/repro-build.sh`.
- `colab/src/rfp022-gpu-bench/methods/ (not published)*`: private research GPU benchmark (not exported); excluded.

## How to rebuild and verify

### Prerequisites

- Docker Engine / Docker Desktop running and reachable by `docker info` (the builder image is linux/amd64; a container runtime that cannot start nested containers, such as a Colab runtime, does not work)
- Rust `1.98.1` via `rustup` (automatically selected by `lez-bridge/rust-toolchain.toml`)
- `rzup` with RISC Zero Rust `1.97.0` installed (`rzup install rust 1.97.0`): risc0-build asks rzup for the installed version even for a Docker build, only to choose an LLVM pass name (any version above 1.81 gives the same flags), and panics without it. The guests themselves compile inside the builder image
- Node.js 18+ (zero npm packages needed) and Python 3 (stdlib only)
- ~30 GB free disk space for the builder image and Cargo build directories

### 1. Rebuild all deployable guests in Docker

```sh
scripts/repro-build.sh --out repro-ids.txt
```

This prints one `guest name, image id` line per guest to stdout and writes the same lines to `repro-ids.txt`. Optional flags:
- `--elf-dir DIR` — also copies every compiled ELF (`<guest>.bin`) and writes `DIR/SHA256SUMS`
- `--only bridge_guest,release_guest` — build a subset while iterating
- `--include-research` — also build the `attest-spike` research guests
- `--list` — inspect the guest table without running Docker or Cargo

### 2. Verify image IDs against `VERSIONS.toml` and deployment records

```sh
node scripts/check-image-ids.mjs --input repro-ids.txt --verbose
```

This checks:
1. `[toolchain].docker_builder_digest` in `VERSIONS.toml` is a pinned `sha256:` digest.
2. Every `[[repro_guest]]` in `VERSIONS.toml` was built and matches its pinned Docker image ID.
3. Every deployment in `VERSIONS.toml` marked `repro_build = true` (the default for all new deployments) matches the Docker-rebuilt image IDs.
4. Every deployment record (`modules/lez_bridge/rust-lib/deployments.json`, `web/deposit/src/lib/deployments.js`, `vault/deployments/sepolia-demo-v2.json`, `vault/DEPLOYMENTS.md`) agrees with `VERSIONS.toml` (and, for `repro_build = true` deployments, with the Docker-built IDs).
5. Legacy non-Docker deployments (`repro_build = false`) are reported as `WARN legacy-non-docker` rather than failing (pass `--strict-legacy` to fail on them too).

Offline unit tests for the checker itself:

```sh
node --test scripts/check-image-ids.test.mjs
```

### 3. Build a single host binary with reproducible embedded guests

Any host binary can embed the Docker-built guests directly by setting `RISC0_USE_DOCKER=1`:

```sh
RISC0_USE_DOCKER=1 RISC0_DOCKER_CONTAINER_TAG=r0.1.97.0@sha256:7ae0a27fa72b10fb5c1066005bd949ae43dc4ead8a284f2d6572b99c82ab4468 \
  cargo build --release --locked -j 4 -p outbound_host   # inside lez-bridge/outbound/
```

Without `RISC0_USE_DOCKER=1`, `build.rs` keeps its original native behavior (and `RISC0_SKIP_BUILD=1` continues to skip guest compilation for native unit tests).

### 4. Two-machine check

Done on 2026-11-28 by the CI job (two fresh runners, then a compare job: `diff` of the two id lists and the two `SHA256SUMS`, then `check-image-ids.mjs`). To repeat it on your own machine, run step 1 and compare your `repro-ids.txt` with the table above (or `node scripts/check-image-ids.mjs --input repro-ids.txt`).

## Expected matches vs legacy non-Docker deployed IDs

### Expected to match

- Every `[[repro_guest]]` entry in `VERSIONS.toml` (pinned 2026-11-28 from the two-runner build above).
- Every **new** deployment from M1 onward (testnet 0.4, any new Sepolia vault v8+), which must be built with `RISC0_USE_DOCKER=1` / `scripts/repro-build.sh` and left at the default `repro_build = true` in `VERSIONS.toml`.

### Legacy deployed IDs built without Docker (they do NOT match — do NOT change)

All existing deployments on Sepolia and our public demo chain (v1 through v7) were built natively on the build host **before** Docker guest builds were set up. Their vaults and LEZ programs are immutable on chain, so their recorded image IDs are historical facts and **must never be overwritten** with Docker-built IDs. `VERSIONS.toml` marks v2–v5 with `repro_build = false` so `scripts/check-image-ids.mjs` reports mismatches as `WARN legacy-non-docker` (17 WARN in run 46095730461; v6 and v7 are not in `VERSIONS.toml`).

Checked 2026-11-28: **none of the deployed ids below equals the Docker id of the same guest on main** (the Docker ids were also searched for in every record in per-word and fully reversed byte order: no hit). Two causes, per guest:

- **Native build only** (the guest's source and `Cargo.lock` on main are the ones deployed): `release_guest` (`0x6bcbd714…` deployed, Docker `0x98728e5b…`; inputs unchanged since `c9ec880` apart from a comment and a caret in `release/core/Cargo.toml` that its own lock does not follow), `outbound_guest_trusted_tip` for v4/v5/v6 (`0x098f5a1a…` deployed at `adc0bca`, Docker `0xee5056ba…`; no change to its inputs since), and the v7 seq-sig guests (`0x93c88f77…` update and `0x9df4de66…` evidence, deployed from branch `v7-seqsig`, Docker `0x807ea870…` and `0x17c58454…`; main's guest sources, locks and core crates equal the branch tip `172ffed`, which only differs in host code; that the tip is the deployed commit was not re-checked). Why a native build differs from the Docker one was not isolated (a native build embeds the build machine's registry and source paths, but swapping those strings alone did not reproduce the native outbound ELFs on the build host); the archived ELFs stay the proving inputs.
- **Native build and source drift:** `bridge_guest` and `token_guest` (v3–v5 `0dabbd6c…`/`ac175cb5…`, built at `c9ec880`): `lez-bridge/Cargo.lock` was regenerated since (LEZ rc3's host risc0 versions), so main's guests differ from the deployed ones whatever the build method. v3's update image `0x6b969629…` (`c9ec880`, risc0-zkvm 3.0.5) and every v1/v2 id predate later source fixes too.

| Deployment | Guest / field | On-chain / recorded image ID | Where recorded | Why it may not match Docker |
|---|---|---|---|---|
| **v1** (retired, 2026-11-12) | `release_image` (`release_guest`) | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` | `vault/DEPLOYMENTS.md`, `vault/deployments/sepolia-demo.json` | Native build without Docker |
| **v1** (retired) | `update_image` | `0x5f425420081746e3e77f3c94e7a44e7d0b3bdcc3cd4e114eaa07d7540fa282e3` | `vault/DEPLOYMENTS.md`, `vault/deployments/sepolia-demo.json` | Placeholder `keccak256("RFP022/demo/no-update-image")` (no guest ELF) |
| **v1** (retired, local `seq2`, rc1) | `bridge_guest` / `token_guest` | `234c6659e008329ea92aac4a625e6d7d33a1118e7869090a90f05ed4fa26c61b` / `633555cc0d90901139f0bf541fe2f32912f969579e829e9d6acab2def083ab5d` | `vault/deployments/sepolia-demo.json` | Pre-fix source, LEZ rc1, native build |
| **v2** (2026-11-13) | `update_image` | `0x5f425420081746e3e77f3c94e7a44e7d0b3bdcc3cd4e114eaa07d7540fa282e3` | `VERSIONS.toml`, `vault/DEPLOYMENTS.md`, `vault/deployments/sepolia-demo-v2.json` | Placeholder (no guest ELF) |
| **v2** | `release_image` (`release_guest`) | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` | `VERSIONS.toml`, `vault/DEPLOYMENTS.md`, `vault/deployments/sepolia-demo-v2.json` | Native build without Docker |
| **v2** (`public`, rc3) | `bridge_guest` / `token_guest` | `7758856ee542f70ee70a696a975caa45088e46a94d4b61471b6a87c7faa2675f` / `a3ab615cd9ac3288606e3b74f3586b7edcc304bd80671d8835634c979821487b` | `VERSIONS.toml`, `public-demo/v3/BUILD.txt` (private) | Pre-fix source (before 2026-11-14 fixes), native build |
| **v2** (`seq2`, rc1) | `bridge_guest` / `token_guest` / `attest_guest` / `attest_probe_guest` | `249a19c6386bfab7b70018312585f4555d9a302cd972308ea4b1a77db74f0129` / `633555cc0d90901139f0bf541fe2f32912f969579e829e9d6acab2def083ab5d` / `84f6c998c5879d917175a83fa15af9fef0d93cb3097c49d8f072a6bddb6f7712` / `b9dcbb1872a66e36155a187f90f37ba63721f5ef1802ac930f58c8bbbfc7f255` | `VERSIONS.toml`, `vault/deployments/sepolia-demo-v2.json` | Pre-fix source, LEZ rc1, native build |
| **v3** (2026-11-14) | `update_image` (`outbound_guest_trusted_tip`) | `0x6b96962992970cf6e6a2164175fb1c74092077af4f6b9b95bf95c14eb5921bd6` | `VERSIONS.toml`, `vault/DEPLOYMENTS.md`, `public-demo/v3/BUILD.txt` (private) | Built natively at `c9ec880` with `risc0-zkvm` 3.0.5 |
| **v3** | `release_image` (`release_guest`) | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` | `VERSIONS.toml`, `vault/DEPLOYMENTS.md` | Native build without Docker |
| **v3** (`public`, rc3) | `bridge_guest` / `token_guest` | `0dabbd6c4c3035d1824c7d5ccb958ab93e3d57871bfd1efea76ab72718ef9ffa` / `ac175cb59bff0f5e59d51b566552546087f082a9bcea9571b705633f031eafba` | `VERSIONS.toml`, `public-demo/v3/BUILD.txt` (private) | Built natively at `c9ec880` without Docker |
| **v4** & **v5** (2026-11-15) | `update_image` (`outbound_guest_trusted_tip`) | `0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017` | `VERSIONS.toml`, `vault/DEPLOYMENTS.md`, `public-demo/v4/BUILD-outbound.txt`, `modules/lez_bridge/rust-lib/deployments.json`, `web/deposit/src/lib/deployments.js` | Built natively at `adc0bca` (`risc0-zkvm` 3.1.0) without Docker |
| **v4** & **v5** | `release_image` (`release_guest`) | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` | same as above | Native build (`risc0-zkvm` 3.0.5) without Docker |
| **v4** & **v5** (`public`, rc3) | `bridge_guest` / `token_guest` | `0dabbd6c4c3035d1824c7d5ccb958ab93e3d57871bfd1efea76ab72718ef9ffa` / `ac175cb59bff0f5e59d51b566552546087f082a9bcea9571b705633f031eafba` | `VERSIONS.toml` | Reused v3 native ELFs |
| **v6** (branch `v6-lc`, 2026-11-18) | `update_image` / `release_image` | `0x098f5a1a…3017` / `0x6bcbd714…72a4` (as v5) | `vault/DEPLOYMENTS.md` | Same native v4/v5 ELFs; LEZ side built natively on `v6-lc` |
| **v7** (branch `v7-seqsig`, 2026-11-18) | `update_image` (`outbound_guest_seqsig`) / `release_image` | `0x93c88f77e60ee5698849163ec51d6b8647e58df752984dcb15ca2f011c96955d` / as v5 | `vault/DEPLOYMENTS.md` | Built natively on `v7-seqsig` without Docker |
| Local failsafe demo (not deployed) | `reclaim_guest` | `0xb7a81fd5…25fd` | `public-demo/prove/failsafe-20261117/README.md` | Local native build; pinned nowhere |
| `DESIGN.md` rc3 notes (not deployed as pinned) | `attest_guest` / `attest_probe_guest` | `f593f493…9ebc` / `90b445a0…063a` | `DESIGN.md` header | Local native rc3 build |

**Note on proving against legacy vaults:** because v4 and v5 pin native-built ELFs on chain, proving an update or release for v4 or v5 must continue passing the archived canonical ELF via `outbound_host ... --elf <file> --image-id <id>` (`outbound/host/src/lib.rs`). Any newly deployed vault built with `scripts/repro-build.sh` will pin the Docker-built image IDs directly.

## CI workflow

`.github/workflows/ci.yml` has two jobs for this, run **on demand only** (a cold build takes about 39 minutes per runner, over the 30-minute budget for every push):

```sh
gh workflow run ci.yml --ref main -f run_repro_builds=true
```

- `repro-guest-builds` (matrix `a`, `b`: two fresh `ubuntu-latest` runners, no cache): frees disk, installs the host Rust from `lez-bridge/rust-toolchain.toml` and rzup 0.5.0 with RISC Zero Rust 1.97.0 (for risc0-build's version check only), runs `scripts/repro-build.sh --out … --elf-dir …` and uploads the ids, ELFs and `SHA256SUMS`.
- `repro-guest-compare`: `diff` of the two builds' ids and `SHA256SUMS` (must be empty), then `node scripts/check-image-ids.mjs --input … --verbose` (fails on any `[[repro_guest]]` mismatch; legacy deployments warn).
- The fast `versions` job runs `node --test scripts/check-image-ids.test.mjs` offline on every push and pull request.
