# The second v4 outbound update, proved and accepted on Sepolia (2026-11-15)

A proof, not a secret: everything here is public data.

- `receipt.bin`: the succinct RISC Zero receipt of one G2-lite (TrustedTip) update over LEZ blocks
  10363..10873 of the team's public demo chain (511 blocks, 376,100 bytes: 4 roots, 4 claims,
  2 exits, 4 nullifiers, no private transaction), chained from the first update's state
  `0xa725eec7…c623` to `0x155b12e6…dab0`. Proved by the owner on the build host (Apple M4 desktop,
  16 GB, in-process CPU prover, 4 threads) with `public-demo/prove/prove-outbound-rc3.sh`:
  10,907,068 user cycles, 28 segments (po2 19), 2,575.1 s of proving, peak footprint 4,746 MB
  (guard wall 2,576 s; whole script 2,609 s). It verified natively against the image id.
- `image-id.txt`: the v4 update image `0x098f5a1a…3017`.
- `journal.hex`: its journal; journal digest `0x92caa091…9830`.

**Wrap:** `.github/workflows/wrap-outbound.yml`, public run 43765416775: Groth16 in 164.3 s,
ACCEPTED by `verify` on `RiscZeroGroth16Verifier` 3.1.0 on Sepolia (read-only call).

**On chain:** `submitUpdate` from the submitter key, Sepolia block 12143000, tx
`0xf7519de9d2964e8d720a81cda7309ab89ef45cca99bd6baedf4dfa1b3d1cab62`, 671,254 gas; `StateUpdated`
with 2 exit leaves, first exit index 1. The vault's `g2State()` reads `0x155b12e6…dab0` since.
