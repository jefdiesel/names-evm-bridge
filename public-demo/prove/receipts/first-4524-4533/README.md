# The first proved outbound update (2026-11-15)

A proof, not a secret: everything here is public data.

- `receipt.bin`: the succinct RISC Zero receipt (borsh, 223,815 bytes) of one G2-lite update, LEZ blocks 4524..4533 of the team's public demo chain, from the genesis state at block 4523 (`0xc3fb14d6…73e7`). It was proved by the owner on the build host on 2026-11-15 with `public-demo/prove/prove-outbound-rc3.sh`: 296,964 user cycles, 1 segment, 77.6 s, peak footprint 4,701 MB. It verified natively (RUNBOOK §14).
- `image-id`: the guest it proves, the v4 trusted-tip G2-lite image `0x098f5a1a…3017` (canonical build, commit `adc0bcac`).
- `journal.hex`: its journal (`abi.encode(UpdateJournal)`, the v3 bridge's cfgId; no vault takes it: it exists to test the wrap). Journal digest `0x68912219…c74f`.
- `proof.json`: the prover's report.

Used by `.github/workflows/wrap-outbound.yml` (and `wrap-outbound.sh` on an x86 box): the Groth16 wrap, then a read-only `verify` call on Sepolia's `RiscZeroGroth16Verifier` 3.1.0.
