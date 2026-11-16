# Check the Sepolia round trip yourself (read-only, a few minutes)

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

Our first round trip with real proofs ran on 2026-11-15 on the public Sepolia testnet and our own
public LEZ v0.4.0-rc3 test chain (not a Logos network), with vault v4
`0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed` (`vault/DEPLOYMENTS.md`). This page lets anyone check,
without a key, an account or a build, that:

- each committed Groth16 seal is accepted by RISC Zero's `RiscZeroGroth16Verifier` 3.1.0 on Sepolia
  (`0x8c915b5071206C6E328BC18529586c513408aB3b`) for the committed image id and journal;
- the vault's update and release transactions carry exactly those seals and succeeded;
- a seal with one byte changed is refused.

**What it does not show:** that the LEZ blocks the update covers are final. The update image is
TrustedTip: which LEZ blocks count is the word of the one submitter key behind
`vault/src/UpdateGatedVerifier.sol`, and the deposit roots on LEZ come from our operator attester
(`DESIGN.md` §4).

Each `public-demo/prove/receipts/<name>/` holds `receipt.bin` (the succinct receipt), `journal.hex`,
`image-id.txt`, `seal.hex` (the Groth16 seal the public wrap run produced: runs 43750086609,
43753474401 and 43765416775 of `.github/workflows/wrap-outbound.yml`) and a `README.md` with the
proving numbers.

Needs Foundry's `cast` and `xxd`/`shasum` (macOS and most Linux). From the repository root:

```sh
V=0x8c915b5071206C6E328BC18529586c513408aB3b
RPC=https://sepolia.gateway.tenderly.co
check() {  # check <receipt dir> <tx hash>
  D=public-demo/prove/receipts/$1
  SEAL=$(tr -d ' \n' < $D/seal.hex); IMG=$(tr -d ' \n' < $D/image-id.txt); J=$(tr -d ' \n' < $D/journal.hex)
  DIGEST=0x$(printf '%s' "${J#0x}" | xxd -r -p | shasum -a 256 | cut -c1-64)
  echo "== $1: image $IMG, journal digest $DIGEST"
  cast call $V "verify(bytes,bytes32,bytes32)" $SEAL $IMG $DIGEST --rpc-url $RPC && echo "verify: ACCEPTED (no revert)"
  cast tx $2 input --rpc-url $RPC | grep -q "${SEAL#0x}" && echo "the transaction carries this seal"
  echo "block $(cast receipt $2 blockNumber --rpc-url $RPC), status $(cast receipt $2 status --rpc-url $RPC), gas $(cast receipt $2 gasUsed --rpc-url $RPC)"
}
check v4-update-1  0x7fd725cd4480a247760aa4abca19fef20d67b39c3e1c43a64c8c192c9a3bb369
check v4-release-1 0x8b39bdc7df743f9b6bb23b90866586b02908e64b0d06beaa88e5d1820d74c7bb
check v4-update-2  0xf7519de9d2964e8d720a81cda7309ab89ef45cca99bd6baedf4dfa1b3d1cab62
# a changed seal must revert
BAD=$(tr -d ' \n' < public-demo/prove/receipts/v4-update-1/seal.hex | sed 's/.$/0/')
cast call $V "verify(bytes,bytes32,bytes32)" $BAD 0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017 0xefb78ef095c427abe54498bb1bfd1553d54e1a3e8112c6f591efaed087c21458 --rpc-url $RPC 2>&1 | head -1
# the vault's own record: state updates and the first release
VAULT=0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed
cast call $VAULT "g2State()(bytes32)" --rpc-url $RPC
cast logs --address $VAULT --from-block 12142003 --to-block 12142503 "Released(bytes32 indexed,address indexed,uint32,address,address,uint256)" --rpc-url $RPC | grep -E "blockNumber|transactionHash"
```

Output on 2026-11-16 (`verify` returns nothing on success, shown as `0x`; the last error is the
changed seal being refused):

```
== v4-update-1: image 0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017, journal digest 0xefb78ef095c427abe54498bb1bfd1553d54e1a3e8112c6f591efaed087c21458
0x
verify: ACCEPTED (no revert)
the transaction carries this seal
block 12142209, status 1 (success), gas 899746
== v4-release-1: image 0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4, journal digest 0x4dbeddc91b5ced7142922a3492c8d86935449e83312199ffb67875335dcfcda2
0x
verify: ACCEPTED (no revert)
the transaction carries this seal
block 12142503, status 1 (success), gas 447552
== v4-update-2: image 0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017, journal digest 0x92caa09155af27b9328d4539852068f37aebb6067bcc4efe918c49e283869830
0x
verify: ACCEPTED (no revert)
the transaction carries this seal
block 12143000, status 1 (success), gas 671254
Error: server returned an error response: error code 3: execution reverted, data: "0x439cc0cd"
0x155b12e64ebabcf228c227fd78c759a0ebf75ecd116fc0ac7ce71c8209a5dab0
  blockNumber: 12142503
  transactionHash: 0x8b39bdc7df743f9b6bb23b90866586b02908e64b0d06beaa88e5d1820d74c7bb
```

A second `Released` event on v4 (2026-11-16) is a withdrawal made from the Names Wallet 0.17 build;
its release proof was wrapped through `.github/workflows/wrap-private.yml`, so its journal and seal
are not committed here.
