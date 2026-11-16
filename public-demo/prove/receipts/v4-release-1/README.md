# The first release paid by proof: v4 exit 0 (2026-11-15)

A proof, not a secret: the receipt and journal are public data (the exit secret stays on the
build host).

- The exit: the team's burn of 0.01 wETH (v4) in LEZ block 10361 (tx `352aa5f2…e399`), toward the
  throwaway deployer address; exit leaf 0, recorded by the first update (`../v4-update-1/`), root
  `0x818df8e4…2381`, release nullifier `0x25df3c27…3bce`.
- `receipt.bin`: the succinct G3 receipt. Proved by the owner on the build host (Apple M4 desktop,
  in-process CPU prover, 4 threads) with `public-demo/prove/prove-release-rc3.sh`: 36,393 user
  cycles, 1 segment, 20.9 s of proving, peak footprint 1,129 MB. It verified natively.
- `image-id.txt`: the release image `0x6bcbd714…72a4` (the vault's `releaseImageId`).
- `journal.hex`: the 256-byte journal; digest `0x4dbeddc9…cda2`.

**Wrap:** `.github/workflows/wrap-outbound.yml`, public run 43753474401: Groth16 in 243.4 s,
ACCEPTED by `verify` on `RiscZeroGroth16Verifier` 3.1.0 on Sepolia (read-only call).

**On chain:** `release` on the v4 vault after its one-hour delay, Sepolia block 12142503, tx
`0x8b39bdc7df743f9b6bb23b90866586b02908e64b0d06beaa88e5d1820d74c7bb`, 447,552 gas, relayer none,
relay fee 0; `Released` (nullifier above, denomination index 0); the recipient's balance rose by
exactly 0.01 ETH between blocks 12142502 and 12142503.
