# The first v4 outbound update, proved and accepted on Sepolia (2026-11-15)

A proof, not a secret: everything here is public data.

- `receipt.bin`: the succinct RISC Zero receipt of one G2-lite (TrustedTip) update over LEZ blocks
  10266..10362 of the team's public demo chain (97 blocks, 77,695 bytes: 2 roots, 1 claim, 1 exit,
  1 nullifier, no private transaction), from the v4 genesis state at block 10265
  (`0x3d1b762a…4155`) to `0xa725eec7…c623`. Proved by the owner on the build host (Apple M4
  desktop, 16 GB, in-process CPU prover, 4 threads) with `public-demo/prove/prove-outbound-rc3.sh
  --vault 0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed`: 2,278,075 user cycles, 6 segments (po2 19),
  526.8 s of proving, peak footprint 4,699 MB (guard wall 528 s; whole script 565 s). It verified
  natively against the image id.
- `image-id.txt`: the v4 update image `0x098f5a1a…3017` (the vault's `updateImageId`).
- `journal.hex`: its journal; journal digest `0xefb78ef0…1458`.

**Wrap:** `.github/workflows/wrap-outbound.yml` on a GitHub-hosted runner (ubuntu-24.04, 4 CPUs,
15 GB, swap added), public run 43750086609: Groth16 in 164.3 s, verified natively, and ACCEPTED
by `verify` on RISC Zero's `RiscZeroGroth16Verifier` 3.1.0 on Sepolia (read-only call).

**On chain:** `submitUpdate` on the v4 `UpdateGatedVerifier` from its submitter key, Sepolia block
12142209, tx `0x7fd725cd4480a247760aa4abca19fef20d67b39c3e1c43a64c8c192c9a3bb369`, 899,746 gas;
the vault's `StateUpdated` (prev `0x3d1b762a…`, new `0xa725eec7…`, 1 exit leaf, first exit index 0).
LEZ finality: the submitter's word (TrustedTip).
