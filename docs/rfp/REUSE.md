# Reuse statement: what we take from audited and mature code, and what is ours

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFPs' own words)

- **RFP-021 Reliability #6:** "Proposals must integrate mature, audited implementations of necessary cryptographic
  primitives and the light-client protocol. Evaluating what existing implementations offer, and stating what is reused,
  what is rewritten, and the cost and audit-surface consequences of that split, is a scoped part of the work to be
  budgeted in the proposal (see Design Rationale, "Assess reuse before rewriting")."
- **RFP-022 Reliability #8:** "Proposals must integrate mature, audited proof-system implementations rather than
  reimplementing zero-knowledge primitives from scratch."

## Status

- **Complete for the code on `main` and on branch `public-update`** (the light client's public path, which the instance
  behind vaults v6 and v7 runs), read from the manifests and `Cargo.lock` files named below on 2026-11-18.
- **Not checked:** whether any audit of Helios, r0vm-helios or sp1-helios covers Helios rev `2af9b418`; whether
  RISC Zero's blst fork has been audited; licences of crates not in the `attest-spike` sweep (marked "not checked").
- **Not done:** an audit of our own code (RFP-021 Sup8, RFP-022 S1).

## 1. Reused unchanged

| Component | Version, pinned by | Used for | Licence | Source |
|---|---|---|---|---|
| Helios `consensus-core` (a16z) | 0.12.0, git rev `2af9b41887e069144ee56cd710f3ddca195473f8` | the light-client update checks (`verify_generic_update`, `apply_generic_update`): slots, periods, branches, the committee selection, the signing root and fork domains, store updates | MIT (repo `LICENSE`; the crate has no `license` field) | `lez-bridge/attest/program/Cargo.toml`; `lez-bridge/methods/attest_guest/Cargo.lock` |
| RISC Zero zkVM | `risc0-zkvm` =3.0.5 on the LEZ side (as LEZ v0.4.0-rc3); 3.1.0 for the Ethereum-side provers and the update and release guests | every proof; LEZ's own privacy circuit | Apache-2.0 | `lez-bridge/cli/Cargo.toml`, `VERSIONS.toml`; advisory GHSA-g6qg-9j8p-7j87 is why the Ethereum side is on 3.1.0 |
| RISC Zero's accelerated BLS12-381 | `zkcrypto-bls12_381` fork, tag `bls12_381/v0.8.0-risczero.1` | BLS12-381 field and curve operations in the guest (private path) | MIT / Apache-2.0 | `lez-bridge/methods/attest_guest/Cargo.toml` `[patch.crates-io]` |
| RISC Zero's accelerated SHA-256 | `RustCrypto-hashes` forks, tags `sha2-v0.9.9-risczero.0`, `sha2-v0.10.9-risczero.0` | SSZ hashing and every SHA-256 in guests | MIT OR Apache-2.0 | same |
| `ethereum_hashing` | `ncitron/ethereum_hashing` rev `7ee70944…` (the patch Helios's own workspace applies) | SSZ hashing | Apache-2.0 | same |
| RISC Zero's blst fork (public path only) | `blst` =0.3.16, guest patched to `risc0/blst` tag `v0.3.16-risczero.0` | the public path's BLS signature check | Apache-2.0 | branch `public-update`: `lez-bridge/attest/program/Cargo.toml`, `lez-bridge/methods/attest_guest_public/Cargo.toml` |
| `alloy-trie`, `alloy-rlp`, `alloy-primitives` | 0.9.9, 0.3.17, 1.7.3 | Merkle-Patricia proofs of accounts, storage slots and receipts; RLP; keccak | MIT OR Apache-2.0 | `lez-bridge/Cargo.lock` |
| `ethereum_ssz`, `tree_hash`, `ssz_types` | 0.9.1 / 0.10.4, 0.10.0 / 0.12.1, 0.11.0 | SSZ decoding and roots | not checked | `lez-bridge/Cargo.lock` |
| LEZ crates (`lee_core` and others) | tag `v0.4.0-rc3`, commit `bafd07be` | the program model, accounts, PDAs, block and transaction formats | as LEZ's repository | every LEZ dependency (`AGENTS.md`) |
| LEZ's keccak patch | `logos-blockchain/sponges` rev `3a56e997…` | keccak in LEZ-side guests | not checked | `lez-bridge/Cargo.toml`, guest manifests |
| `ed25519-dalek` with RISC Zero's `curve25519-dalek` fork | 2.2.0; `risc0/curve25519-dalek` rev `385adda1…` and `risc0/RustCrypto-crypto-bigint` rev `3ab63a6f…` | the signer gate's Ed25519 checks (v7) | BSD-3-Clause | `lez-bridge/outbound-seqsig/core/Cargo.toml`, `lez-bridge/methods/outbound_guest_seqsig/Cargo.toml` |
| RISC Zero's `RiscZeroGroth16Verifier` | 3.1.0, deployed by RISC Zero on Sepolia at `0x8c915b50…aB3b` (not vendored; we call it) | the vault's proof check | Apache-2.0 (risc0-ethereum) | `vault/src/IRiscZeroVerifier.sol`; `VERSIONS.toml` |
| RISC Zero's Groth16 prover image | `risczero/risc0-groth16-prover:v2026-10-20.1` (Docker) | the wrap of a succinct receipt to Groth16 | as RISC Zero publishes it | `lez-bridge/outbound/wrap/` |
| OpenZeppelin Contracts | 5.1.0, submodule commit `69c8def5…` | `SafeERC20`, `ReentrancyGuard`, ERC-20 interfaces in the vault | MIT | `vault/lib/openzeppelin-contracts` |
| forge-std | submodule commit `f3dae6e6…` | tests only | MIT / Apache-2.0 | `vault/lib/forge-std` |
| consensus-specs light-client sync vectors | v1.6.1 | tests only: 22 of 22 applicable cases pass | CC0-1.0 (not checked) | branch `public-update`, spec-vectors record |

`fastrlp` 0.3.1 and 0.4.0 (MPL-2.0, file-level copyleft) are in the attestation guest's dependency tree through alloy
(`DESIGN.md` §9.5; `attest-spike/README.md` "Licences"); everything else in that sweep was permissive.

## 2. Changed (ours to audit)

- **Two Helios functions through hooks, on the public path only** (branch `public-update`,
  `lez-bridge/attest/vendor/helios-consensus-core/`, diff in `SPIKE-PATCH.diff`):
  1. the sync-committee signature check is supplied by the caller (`verify_generic_update_with`); our caller computes
     the signers' key as the committee's aggregate minus the non-signers, from uncompressed key witnesses that must be
     on the curve, not infinity, and equal to the committed bytes, and checks it with blst;
  2. the next committee's root by direct SHA-256 (tested equal to Helios's).
  Every other Helios check is unchanged. The audit surface added: the hook, the key-witness check, the blst FFI wrapper
  and RISC Zero's bigint glue in its blst fork (`DESIGN.md` §9.3a, "Audit surface").
- **The program on `main` uses Helios unmodified** (private path).

## 3. Written by us

- The LEZ attestation program `eth_attest` (instances, accounts, the store, anchors, job accounts, the
  weak-subjectivity rule), `eth_attest_core` (statements, predicates, statement ids, chain binding), the reference
  consumers, the typed error codes and the CLI.
- The bridge and token programs, the vault and its SHA-256 incremental tree (`Sha256IncrementalTree.sol`), `UpdateGatedVerifier`, `SeqSigGate`, the deposit desk, the failsafe vault
  variant.
- The guests: the outbound update (G2-lite: LEZ block decoding, extraction, the claim accumulator), the release proof
  (G3), the reclaim proof (G4), the signer and evidence guests.
- The recovery client, the core module, the mini-app, the deposit page, the services (relay, settlement, updater,
  faucet, follower).

## 4. Why this split, and what it costs

- **Alternatives assessed:** r0vm-helios (RISC Zero, audited by zkSecurity per RFP-021) and sp1-helios prove the light
  client off-chain for an EVM destination; LEZ verifies in its own RISC Zero execution, so their Solidity side does not
  apply and what transfers is the light-client logic itself, which is Helios's. We take Helios directly and used
  r0vm-helios only as a reference for the patch set (no code reused; `attest-spike/README.md`).
- **Cost:** with Helios unchanged, one finality update is about 37M cycles, over LEZ's 10M cap, so it runs privately
  with about 2 h of desktop proving (`DESIGN.md` §9.0a, §9.3). The two hooks bring it to about 9.3M cycles as one public
  transaction (`docs/EVIDENCE.md` row `lc-public`). That saving is what puts code of ours on the verification path.
- **Audit surface, in order of risk:** our hooks and key-witness check (new, reviewed once by an AI agent); RISC Zero's
  blst fork (has only run honest updates inside the zkVM); our inclusion code over `alloy-trie` (fuzzed:
  `seeded_mutations_of_real_proofs_never_panic`, `structured_random_proofs_never_panic_and_the_node_check_is_sound`);
  the bridge, vault and guests (all ours, reviewed internally on 2026-11-14).
- **Budget:** the proposal's audit estimate is about two auditor-weeks for the RFP-021 scope, including the signature
  hook and key-witness check, and four for RFP-022 (proposal §11); no quote.

## Sources

The manifests and lockfiles named in the tables; `attest-spike/README.md` "Licences"; `DESIGN.md` §9.3, §9.3a, §9.5;
`VERSIONS.toml`; `.gitmodules`; `vault/lib/openzeppelin-contracts/package.json` (5.1.0, read in the main working copy).
