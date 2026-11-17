# The official consensus-specs light-client vectors (2026-11-17)

**Result.** The light-client sync vectors from the Ethereum consensus specs release **v1.6.1**
(`consensus-spec-tests`, minimal preset) ran natively against the verification code that the
RFP-021 programs use. They passed 22 of 22 applicable cases and 70 of 70 `process_update` steps,
each step through three paths, with 0 failures. Every update that applied was checked again with
one participation bit cleared, and all 132 such checks were refused. Branch `public-update`;
test `lez-bridge/attest/program/tests/spec_vectors.rs`; log `vectors.log`.

**Licence.** The consensus-specs repository is CC0-1.0, read on 2026-11-17 from
`api.github.com/repos/ethereum/consensus-specs/license` (`LICENSE`). The vectors are **not
vendored**. `build-public.sh vectors` downloads the release's `minimal.tar.gz` at test time and
keeps only `*/light_client/*`. Without `CONSENSUS_SPEC_TESTS`, the test only prints that it was
skipped.

## What ran

For each `light_client/sync` case, the test first checks the bootstrap against the case's trusted
block root, using Helios's `verify_bootstrap` (as `Init` does). It then runs each
`process_update` at the step's own `current_slot`. After every step the finalized header (slot
and root) must equal the vector's. Each step goes through three paths:

| Path | What it is |
|---|---|
| 0 | the vendored Helios 0.12.0 with its own signature check, then `apply_update`, with the program's rule that an update must advance the store (the private path's composition) |
| 1 | the vendored Helios hook `verify_generic_update_with`, with the program's own blst check: `public::key_witnesses` (aggregate minus non-signers, uncompressed) and `public::fast::verify` (as `UpdatePublic`) |
| 2 | the same hook with the check split at the pairing seam: `fast::miller`, the canonical 576-byte value, then `fast::final_is_one` (as `FinalityBegin`/`FinalityFinish`) |

To run the minimal preset's 32-member committee, `key_witnesses` and `fast::verify`/`miller`
now take any committee size (`bits.len()` keys, then the aggregate). Every program caller still
requires the 512-member length (`sig_ok`, and the `FinalityBegin` path), so on-chain behaviour
is unchanged. The public guest's image id changes with this source (`a7ea6565…5b05`; the pinned executor counts still hold).

## Counts (`vectors.log`)

| | Count | Why |
|---|---:|---|
| sync cases run (Capella, Deneb, Electra, Fulu, including the fork-transition and legacy-store cases) | **22 pass** | |
| `process_update` steps run | **70 pass** (210 path-checks) | 44 updates applied on every path; the other 26 steps expect the store unchanged and the program refused or ignored them |
| tampered re-checks (one participation bit cleared) | **132 / 132 refused** | |
| sync cases from Altair and Bellatrix | 17 not applicable | their light-client headers carry no execution header; Helios and the program take Capella and later, and Sepolia and mainnet are past Electra |
| optimistic-header checks | 70 not applicable | the program keeps no optimistic header |
| updates after a `force_update` | 12 not applicable | the program has no forced update: an update must verify |
| `upgrade_store` steps | 2, no-ops | Helios's headers are fork-tagged already |
| `update_ranking` cases | 6 not applicable | the program applies the first valid update and never ranks |
| `single_merkle_proof` cases | 22 not applicable | each root is the hash of a whole `BeaconState`/`BeaconBlockBody`, types neither Helios nor the program has. The same four branches (finality, current committee, next committee, execution payload) are checked inside every bootstrap and update above |
| `data_collection` cases | 10 not applicable | a full node's side |

## What this is NOT

- **Not the program's instruction handlers.** That means not the weak-subjectivity rule, the
  participation policy or the job PDAs. Those run on real Sepolia data in the other tests and
  records.
- **Not the mainnet preset.** The program's images use the mainnet preset (512 members): the same
  code at another size. The mainnet preset ships no sync or ranking vectors.
- **Not run on a chain.** The vectors' 32-member committees cannot go to the 512-member image on
  a chain, so no chain run applies to this item.

## Command

```sh
lez-bridge/attest/cycles/build-public.sh vectors   # fetch v1.6.1 minimal (light_client only), run the test
```

`$W` is the build host's work directory.
