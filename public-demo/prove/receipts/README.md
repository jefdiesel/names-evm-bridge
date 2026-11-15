# Receipts for the Groth16 wrap

One directory per proved receipt that `.github/workflows/wrap-outbound.yml` (or
`public-demo/prove/wrap-outbound.sh` on an x86 machine) wraps for RISC Zero's on-chain
`RiscZeroGroth16Verifier` 3.1.0. They are proofs: public data, no secret.

| File | What | Where it comes from |
|---|---|---|
| `receipt.bin` | the succinct receipt (borsh), about 220 KB | the prover's run directory: `prove/receipt.bin` |
| `journal.hex` | its journal, `0x`-hex: what the vault hashes (`sha256(journal)`) | `prove/journal.hex` |
| `image-id.txt` | the image the on-chain call names, `0x`-hex, one line (`image-id` in the first directory) | the update image `0x098f5a1a…3017` or the release image `0x6bcbd714…72a4` |
| `README.md` | optional: what it proves, the run, its numbers | written by hand |

- An outbound update: `public-demo/prove/prove-outbound-rc3.sh` (`first-4524-4533/`).
- A G3 release: `public-demo/prove/prove-release-rc3.sh`. Its journal is the 256 bytes
  `abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)`, the vault's
  release digest preimage.

Then `gh workflow run wrap-outbound.yml --ref main -f receipt=<directory>`; it prints ACCEPTED or
REFUSED for the read-only `verify(seal, imageId, sha256(journal))` call.
