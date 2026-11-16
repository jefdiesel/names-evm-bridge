# The first v5 outbound update (2026-11-16)

A proof, not a secret: everything here is public data.

- `receipt.bin`: the succinct RISC Zero receipt of one G2-lite (TrustedTip) update over LEZ blocks
  11599..15397 of the team's public demo chain (3,799 blocks, 3,222,267 bytes: 3 roots, 2 claims,
  1 exit, 2 nullifiers; two of the transactions are private, a claim and a burn, and this image does
  not re-check their privacy receipts), from the v5 vault's starting state `0xa339efab…4051` to
  `0x7c04d3d5…0e6b`. Proved on the build host (Apple M4 desktop, 16 GB, in-process CPU prover,
  4 threads) with `public-demo/prove/prove-outbound-rc3.sh`: 76,837,121 user cycles, 195 segments
  (po2 19), 17,549.1 s of proving, peak footprint 4,798 MB (guard wall 17,551 s). It verified
  natively against the image id.
- `image-id.txt`: the update image `0x098f5a1a…3017`.
- `journal.hex`: its journal; journal digest
  `0x831b13308cf4cdabe2179e7328c36a69eeca4ed0969adf7bb28bc10783d4a489`.

Wrap and on-chain result: see `public-demo/RUNBOOK.md` (not published).
