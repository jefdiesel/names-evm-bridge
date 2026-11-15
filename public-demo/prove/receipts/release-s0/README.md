# The first proved G3 release (2026-11-15): the v4 plan's stop step S0.2

A proof over a SYNTHETIC exit, not a real one: `vault/test/fixtures/release_vector.json`, release
case 0 ("exit 3 (1 ETH) under the oldest root (5 leaves), fee == maxRelayFee"). Its exit secret is
a public test value, so this receipt is public data. No vault recorded its root (the fixture's
cfgId belongs to a test vault on chain 31337); it exists to show that RISC Zero's Sepolia verifier
accepts a real Groth16 seal of the release image.

- `receipt.bin`: the succinct receipt (borsh). Proved by the owner on the build host with
  `public-demo/prove/prove-release-rc3.sh --vector … --case 0`: 36,434 user cycles, 1 segment,
  20.7 s of proving, peak footprint 996 MB; whole script 27 s. It verified natively.
- `image-id`: the release image `0x6bcbd714…72a4` (the vault's `releaseImageId`; the v4 canonical ELF).
- `journal.hex`: the 256-byte journal `abi.encode(cfgId, root, nullifier, token, denom, recipient,
  relayer, relayFee)`, equal to the fixture's; `sha256(journal)` = `0xa79fa5ca…12e8`, the digest
  `release` would pass to the verifier.

Wrap and check: `gh workflow run wrap-outbound.yml --ref main -f receipt=release-s0`.
