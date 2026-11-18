# v7: withdrawals on 2 of 3 signer keys, on public networks (2026-11-18)

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

**Result.** A 0.01 ETH round trip through the Sepolia vault **v7** with no attester key in and no submitter key out.
- **In:** the deposit's root was verified by the RFP-021 light client on the team's public demo chain, and the
  deposit was claimed there by an ordinary public transaction (LEZ block 24717).
- **Out:** after a public burn (LEZ block 24718), **2 of 3 signer keys, on two separate machines**, each replayed the
  LEZ blocks itself and signed block 24720. A real RISC Zero proof that the update over 24617..24720 carries those
  two signatures was wrapped to Groth16 and accepted by RISC Zero's verifier 3.1.0 through the `SeqSigGate`.
- **Paid:** the update was submitted by a key with no role at the gate (Sepolia block 12159910), finalized after the
  600 s window with no contest (12159962), and the release paid 0.01 ETH to an address never used before (12159991).
- **Refused:** the same journal claiming only 1 of 3 signers was refused by the gate (`BadSignerBitmap`, read only).
- **Ejected (later the same day):** a second update was submitted, then contested with a real evidence proof that one
  of its two signers had also signed a different hash for the same block (made on purpose, with a test flag, by our
  own key). The gate ejected that key and cancelled the update; the vault did not move. See "Equivocation on
  Sepolia (2026-11-18)" below.

v7 is v6's bridge with no single key on the way out. Inbound is v6's: deposit roots come from the RFP-021
light-client instance `0xe4b7d26a…3600` of program `9Wb7sBxs…uhNR` on the team's public demo chain
(`https://lez.nameswallet.app/`), so no attester key. Outbound replaces v6's submitter: the Sepolia vault v7
takes a state update only through a `SeqSigGate` (branch `seq-sig`, `public-demo/prove/seq-sig-20261117/README.md`).
The gate needs a real RISC Zero proof, checked by RISC Zero's Groth16 verifier 3.1.0, that 2 of a fixed set of
3 signer keys signed the LEZ block the update ends at. Anyone may submit; a 10-minute contest window follows.

The three keys are held on **three separate machines**. Each machine made its own key, and each one replays
the blocks itself before it signs. They are still **stand-ins**: keys of the team, not LEZ sequencer keys.
LEZ v0.4.0-rc3 headers carry no signature, and the demo chain has one sequencer.

State labels (`DESIGN.md`): **(1) run on a chain**: Sepolia (vault, gate, desk, deposit, update, release) and **the
team's public demo chain** (LEZ v0.4.0-rc3, operator-run, `https://lez.nameswallet.app/`); the rehearsal ran on a local
LEZ rc3 sequencer, anvil and an anvil fork of Sepolia. Not a Logos network. Proofs: **real** (proved, wrapped to
Groth16, checked by RISC Zero's verifier 3.1.0 on Sepolia) for the update and the release; the LEZ steps are public
transactions (no proof needed).

## The run (2026-11-18)

Run by the team from the operator's runbook (`public-demo/v7/RUNBOOK-v7.md`, private). Every value below was read back
on chain afterwards (commands under "Re-read"). Deviations: the claim came one follower cycle earlier than estimated;
the optional reverted 1-of-3 transaction was not sent (a read-only call only); no equivocation on Sepolia during the
round trip (as planned; it was run afterwards, on a second update: "Equivocation on Sepolia (2026-11-18)" below).

**S0, before the deploy: a real seal of the update image.** The rehearsal's batch (local LEZ blocks 28..50, 2
signatures) was proved with the canonical seq-sig ELF: 2,822,215 user cycles, 7 segments, 619.2 s of proving (627 s
for the whole script), peak footprint 4,710 MB, verified natively. It was wrapped to Groth16 on a GitHub-hosted x86
runner (workflow run 44248843496, wrap 164 s); `verify(seal, 0x93c88f77…955d, sha256(journal))` on RISC Zero's
verifier 3.1.0 `0x8c91…aB3b` returned `0x` (accepted), and a changed journal digest was refused. Only then was the
image pinned in a vault.

**The public demo chain (LEZ):**

| Step | LEZ block | tx |
|---|---:|---|
| Bridge header `CdijfhGnf1NjyCvYTZcGijQZzZ6soNJQ75Piex1ZTM93`: the v3 canonical bridge ELF (image `0dabbd6c…9ffa`), immutable | ..24616 | — |
| G2 genesis at tip 24616: `0x49a2f8786fd7d4b7caa0e59c1c974c84917b5e0b977a8c852eb42dab6b5ffa4e`; program id `0xacd8a7ec…03f2`; `lezChainId` `0x2da7f7df…5374` and `zoneId` `0x21617f2f…3fff` (v6's light-client instance) | 24616 | — |
| Wrapped ETH `BWDmKjDqkJDrLt1DDXGPeL4DBF8RsGAwB9BQvT7Hzpqg`, wrapped dUSDC `2pimanqGA48xLVRtCR3wqRu6NAkJZbyAcFxgE8yoFCa3` (authority `Program(bridge)`) | before 24635 | |
| **`InitializeAttested`**: vault v7, root source = program `9Wb7sBxs…uhNR` instance `0xe4b7d26a…3600` slot 83; admin and freezer = the chain operator; the bridge derived the vault's cfgId | 24635 | `12055baccd475d938cc6ddfe7bfc46100b049230c08276f1f08da2bb75685c58` |
| Register ETH and dUSDC, sponsor 25,000,000, float 1,000,000,000 | after 24635 | |
| Light-client finality update past the deposit (slot 11603968 → 11604128; the follower, unchanged) | 24715 | `aaa782051d0ad8c781d17027c2790f92974ee92e2a01db73791ea61b103d06d4` |
| **Public claim** (`ClaimAttested`, no attester, no proof) into a never-used account, anchored at Sepolia block 12159680 | **24717** | `6e5ba2f3e8bfc08afa049a683229a35fe7d2d9eb97e49c5123df486deabf9458` |
| **Public burn** of the 0.01 wETH: exit index 0, release address `0x268b1b3a2ecfa5bdcac2a8d53b58d0718e72f0be` (derived for this burn, never used before), max relay fee 200,000,000,000,000 | **24718** | `d9edebd975340012367f4adc09012d1c98433701f24f8f0086701bb1c95d4b8b` |

**Sepolia** (the throwaway deployer `0x37a46E73…321F`, from nonce 111; the gate and vault landed at the pins):

| What | Value |
|---|---|
| `SeqSigGate` | `0x5E891d52eB9a729feCFDd9051685FDeaCd9F0FaE`, block 12159634, tx `0xa1ab8e5dcf56d751a2dc407e96f3e009adddbcad3b456999d8735647cf74a048`, 12,612,379 gas. `VAULT()` the vault, `SIGNERS()` 3, `THRESHOLD()` 2, `CONTEST_WINDOW()` 600, `TEST_ONLY_SHORT_WINDOW()` true, `SIGNER_SET_HASH()` `0x1f82d03a…b8cf`, `UPDATE_IMAGE_ID()` `0x93c88f77…955d`, `EVIDENCE_IMAGE_ID()` `0x9df4de66…d21d`, `VERIFIER()` 3.1.0 |
| `BridgeVault` v7 | `0x4D7B360cf385d3477F1A67c1AEdEB6C17c26d23E`, block 12159635, tx `0x03fb6e3ae19e00939ddd1b0c7396c29efc692f8fc005ec0755a400b6de421911`, 33,047,977 gas |
| `cfgId()` | `0x7ef9153b69a348ff39e3daf7b893f1b775513489c71c7331d88b4a5cf30652d5` (= the bridge Config's) |
| `configure()` | 9 transactions, blocks 12159638–12159646 (v5/v6's values, delay 0) |
| DepositDesk v7 | `0x9b4040A990aEe283F1DE5451fFBaEE3679507fb5`, block 12159647, tx `0x44e92497ca001af7959c7ad0af7e04baf9b7eef8f67dcae9a7473da0266f3654`, 30 bps |
| Read-backs | `readback-v7.sh`: `READBACK OK`; `node scripts/check-versions.mjs --onchain --deployment v7`: 0 FAIL |
| Source | the vault, the gate and the desk are `exact_match` on Sourcify (the gate verified 2026-11-18 with `forge verify-contract --verifier sourcify`) |
| Deposit | 0.01 ETH + 0.3% through the desk, leaf 0, tx `0x043444d8c98da05274f5551cb0379e260f51d498188a7400edd75c9436ced1f5`, block 12159659 (slot 11604106), 820,681 gas (multiplier 1000) |

Gas against the plan's estimate (about 13M gate, 33M vault): 12,612,379 and 33,047,977.

**The signatures.** N = 24720 (20 blocks under the head). Signer 1 (the build host) and signer 2 (a second desktop)
each derived the genesis state from the chain themselves (both printed the vault's `0x49a2f878…fa4e`), fetched and
replayed 24617..24720, and signed block 24720: about 1 min each, at the same time, from different network addresses.
Signer 3 (a laptop) stayed offline, as a third key may. Signer bitmap `11` (set indexes 0 and 1; the set is sorted).

**The update proof** (owner-started; the build host's in-process CPU prover under `guard.sh`, 4 threads): LEZ
24617..24720, 4,607,091 user cycles (the TrustedTip statement on the same batch 2,602,032; the signature check
2,007,709 for 2 signatures), 12 segments, 1,021.9 s of proving (1,034 s for the whole script), peak footprint
4,685 MB, verified natively. Journal 768 bytes (`journal.hex` in this folder, with the 260-byte seal `seal.hex`; both public), digest
`0x50712c9798df330eaf38061f9e983eebce1f599423a8cc28d14b941f9f5be3c2`. Wrapped on a GitHub-hosted x86 runner
(`wrap-private.yml` by `fetch_id`): `ON-CHAIN: ACCEPTED`.

| Step | Sepolia block | tx | Gas |
|---|---:|---|---:|
| **1 of 3 refused** (before the submit, read only): the gate refuses the real journal with all but one signer bit cleared, with the real seal, in an `eth_call` of `submit`: it reverts `0x731ea350` = `BadSignerBitmap()`, while the unaltered journal's `eth_call` returned `0x`. No proof exists for a one-signer journal: the guest refuses to make one (rehearsal: `seq-sig refused: 1 distinct valid signers, threshold 2`) | — | — | — |
| **`submit()`**: anyone may submit; this one came from the deployer, which holds no role at the gate (it is the vault's admin and freezer). Staged, bitmap `11`, finalizable at 1794986520 | 12159910 | `0x8ada43b942e8d472fcde88004f203f14e256a2e0ebd55d7cafe23e8c4a7f193c` | 1,091,441 |
| The 600 s window: nobody contested | | | |
| **`finalize()`** (anyone; again from the deployer): `g2State` `0x319f231d0e47d5194a34a63cd7d2213cd495d042eaf3712f0547119aaa5ef936`, `exitCount` 1, `exitRoot` `0xe277ff83…3cae` | 12159962 | `0xfd128bcdccd4391d2219f31c50bf063c8d3afc8dde6b4219865c53592ac8b889` | 666,170 |
| The release proof (v5's release image, 1 segment, 29 s), wrapped (workflow run 44258503328, accepted) | | | |
| **`release()`**: 0.01 ETH to `0x268b…f0be` (balance 0 → 10,000,000,000,000,000 wei; it has sent nothing, nonce 0); the vault's balance is 0 | 12159991 | `0x8744765abe02f5572638567a1ee5aa3d4c98ca8d3b0310a332f0beae098e0409` | 631,562 |

Vault counters for ETH 0.01: deposits 1, claims 1, burns 1, releases 1. The gate after the round trip: `ejected()` 0,
`halted()` false, nothing pending (`ejected()` is 1 since the equivocation run below). `VERSIONS.toml`
`[deployments.v7]`: deployed, `g2_updates` 1.

### Re-read (any machine, read only)

```sh
R=https://sepolia.gateway.tenderly.co; G=0x5E891d52eB9a729feCFDd9051685FDeaCd9F0FaE; V=0x4D7B360cf385d3477F1A67c1AEdEB6C17c26d23E
for t in 0xa1ab8e5dcf56d751a2dc407e96f3e009adddbcad3b456999d8735647cf74a048 0x03fb6e3ae19e00939ddd1b0c7396c29efc692f8fc005ec0755a400b6de421911 \
         0x44e92497ca001af7959c7ad0af7e04baf9b7eef8f67dcae9a7473da0266f3654 0x043444d8c98da05274f5551cb0379e260f51d498188a7400edd75c9436ced1f5 \
         0x8ada43b942e8d472fcde88004f203f14e256a2e0ebd55d7cafe23e8c4a7f193c 0xfd128bcdccd4391d2219f31c50bf063c8d3afc8dde6b4219865c53592ac8b889 \
         0x8744765abe02f5572638567a1ee5aa3d4c98ca8d3b0310a332f0beae098e0409; do
  cast receipt $t --rpc-url $R --json | python3 -c 'import json,sys;r=json.load(sys.stdin);print(int(r["blockNumber"],16),int(r["gasUsed"],16),r["status"])'
done
cast call $V 'verifier()(address)' --rpc-url $R                                  # the gate
cast call $V 'g2State()(bytes32)' --rpc-url $R                                   # 0x319f231d…f936
cast call $V 'counters(address,uint256)(uint64,uint64,uint64,uint64)' 0x0000000000000000000000000000000000000000 0 --rpc-url $R   # 1 1 1 1
cast call $G 'THRESHOLD()(uint256)' --rpc-url $R; cast call $G 'SIGNERS()(uint256)' --rpc-url $R; cast call $G 'TEST_ONLY_SHORT_WINDOW()(bool)' --rpc-url $R
cast balance 0x268b1b3a2ecfa5bdcac2a8d53b58d0718e72f0be --rpc-url $R             # 10000000000000000
# the 1-of-3 refusal, from this folder's journal.hex and seal.hex (the update's own; the same bytes are in the
# submit transaction's calldata): clear all but the lowest signer bit, then submit in a read-only call
D=$(python3 -c 'import sys; j=bytes.fromhex(open("journal.hex").read().strip()[2:]); t=len(j)-256; b=int.from_bytes(j[t+96:t+128],"big"); print("0x"+(j[:t+96]+(b&-b).to_bytes(32,"big")+j[t+128:]).hex())')
S=$(cat seal.hex)
cast call $G 'submit(bytes,bytes)' $D $S --rpc-url $R                       # reverts 0x731ea350 = BadSignerBitmap() (since S1's ejection the unaltered journal does too: the pair at 12159909 is the control)
cast call $G 'submit(bytes,bytes)' $D $S --rpc-url $R --block 12159909      # the same, at the block before the submit
cast call $G 'submit(bytes,bytes)' $(cat journal.hex) $S --rpc-url $R --block 12159909   # 0x: the unaltered journal passed then
cast call $G 'submit(bytes,bytes)' $(cat journal.hex) $S --rpc-url $R --block 12162505   # reverts 0x3902a9c9 = StaleState(): the vault moved on
cast call $G 'submit(bytes,bytes)' $(cat journal.hex) $S --rpc-url $R       # now reverts 0x731ea350 = BadSignerBitmap(): one of its two signers (S1) was ejected later
cast call 0x8c915b5071206C6E328BC18529586c513408aB3b 'verify(bytes,bytes32,bytes32)' $S \
  0x93c88f77e60ee5698849163ec51d6b8647e58df752984dcb15ca2f011c96955d 0x50712c9798df330eaf38061f9e983eebce1f599423a8cc28d14b941f9f5be3c2 --rpc-url $R   # 0x: accepted
# LEZ side: [base64 transaction, block]
E=https://lez.nameswallet.app/
for h in 12055baccd475d938cc6ddfe7bfc46100b049230c08276f1f08da2bb75685c58 aaa782051d0ad8c781d17027c2790f92974ee92e2a01db73791ea61b103d06d4 \
         6e5ba2f3e8bfc08afa049a683229a35fe7d2d9eb97e49c5123df486deabf9458 d9edebd975340012367f4adc09012d1c98433701f24f8f0086701bb1c95d4b8b; do
  curl -s -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\"params\":[\"$h\"]}" $E \
    | python3 -c 'import sys,json,base64;t,b=json.load(sys.stdin)["result"];print(sys.argv[1][:8],"block",b,"bytes",len(base64.b64decode(t)))' $h
done
```

Re-run 2026-11-18 from a workstation: the seven receipts came back with the blocks and gas above, status 1; the
one-signer journal reverted `0x731ea350` at the latest block and at 12159909, the unaltered journal returned `0x` at
12159909 and reverted `0x3902a9c9` (`StaleState()`) afterwards (re-checked at block 12162505; since S1's ejection at
12162508 it reverts `0x731ea350` first, `BadSignerBitmap()`, re-run the same day); the update's seal (from the submit's calldata, 260 bytes) was accepted by the verifier
(`0x`); the vault read `g2State` `0x319f231d…f936`, counters 1 1 1 1, balance 0; the recipient held
10,000,000,000,000,000 wei with nonce 0; the four LEZ transactions came back in blocks 24635, 24715, 24717 and 24718
(932, 29,275, 7,154 and 671 bytes).

## Equivocation on Sepolia (2026-11-18)

**Result.** On the same gate, after the round trip: a second update was staged, then contested with a real evidence
proof that one of its two signers had also signed a different hash for the same LEZ block. The gate ejected that key
for good and cancelled the update; the vault's state did not move and the gate was not halted.

**How it was made (read this first).** All three keys are ours. The conflicting signature was made **on purpose**, by
our own signer S1, with the test flag `seqsig attest --test-conflicting-hash` (it signs a given hash and writes no
sign-once record; a signer without the flag refuses a second hash for the same block). The contest window is the
10-minute test setting (`TEST_ONLY_SHORT_WINDOW`). This shows the gate's contest path with real proofs on Sepolia; it
does not show a contester catching a dishonest signer it did not control.

**The second update.** LEZ blocks 24721..26120 of the team's public demo chain (1,400 blocks, 2,822 transactions,
none private, none reverted), from the vault's tip 24720 (hash `0x4d99c122dbb1f8c6bbdb82a083d2a15b6193bd90ff6b91a708f62274729ff1bd`)
to block 26120 (hash `0xc2e59496a178bcb379fcfe7143b109ac7ed9692a633eaa8973fc0ea5b4331aee`; both hashes as the public
endpoint's `getBlock` returns them). Signer S1 (the build host, signer `0x05d1ef9d…1076`, set index 0) and signer S2
(signer `0x6b4b81e4…52d6`, index 1) each replayed the range and signed block 26120 (`checkpoint-s1-26120.json`,
`checkpoint-s2-26120.json` in this folder: the signer, its signature and the signed values). The update proof
(owner-started; in-process CPU prover under `guard.sh`, 4 threads): 32,137,262 user cycles, 83 segments, 7,489 s wall,
peak footprint 4,781 MB, verified natively; signer bitmap `11`, ejected mask 0 in the journal; journal digest
`0xfdfded242eb2da4a8b1fa1c939633bc7d9e3e51becd335578baf5635dd3d6807` (`update2-journal.hex`, 576 bytes; its vault part
`update2-vault-journal.hex`, digest `0xb5a6d76d…7bce`, the `journalDigest` in the gate's events), image
`0x93c88f77…955d`. Wrapped to Groth16 on the public repository's `wrap-private.yml` (run 44330602862, wrap 164 s;
`update2-seal.hex`); the verifier's `verify` accepted it read-only.

**The conflicting signature.** S1 also signed, with the test flag, a different hash for the same (cfgId, tip 24720,
block 26120): `0x66db93276c5cf9eaaadf041df4193bbeb209ddef5252aaef816e47b1525c1597`, the keccak256 of the text
"v7 Sepolia equivocation demo 2026-11-18: a block 26120 that is not the chain's" (`conflict-s1-26120.json`). The
evidence proof over that one checkpoint (the evidence image `0x9df4de66…d21d`; signer bitmap `1` = S1): 1,133,939 user
cycles, 3 segments, 225 s wall, peak footprint 4,596 MB, verified natively; journal digest
`0xed0c5de5d32c8fc9d3a83e282be00097402f527922bfc15765bc25247b9d95f8` (`evidence-journal.hex`, 224 bytes). Wrapped on
`wrap-private.yml` (run 44308584650, wrap 244 s; `evidence-seal.hex`); `verify` accepted it read-only. The evidence
proof and its wrap were made about three hours **before** the submit: we were both the equivocator and the contester,
so the 10-minute window was never a race here.

**Sepolia** (both from the deployer `0x37a46E73…321F`, which holds no role at the gate):

| Step | Sepolia block | tx | Gas |
|---|---:|---|---:|
| **`submit()`** of the second update: staged, event `UpdateAccepted` (block 26120, signers `11`, finalizable at 1795018404) | 12162506 | `0x47f4cb216fd2e3939a3a916c99f5c5d43b1fee1c938ce9a1186878bc10e53eff` | 974,563 |
| **`contest()`** with the evidence journal and seal, 24 s later: event `Equivocation` (ejected keys `1` = S1, cancelled true, block 26120, the chain's hash, the conflicting hash) | 12162508 | `0x0291308ed09e18de620482673237e06d70ecc9d90fce682e90ecfd2500e91b84` | 479,624 |

After the contest (read back the same day): `ejected()` 1 (it was 0 at block 12162507), `activeSigners()` 2, `dead()`
false, `halted()` false, `finalizableAt()` 0 (nothing pending; it was 1795018404 at block 12162507); `refuted(…)` is
true for the cancelled checkpoint (24720 `0x4d99…f1bd` → 26120 `0xc2e5…1aee`); the vault's `g2State()` is still
`0x319f231d0e47d5194a34a63cd7d2213cd495d042eaf3712f0547119aaa5ef936` and `exitCount()` still 1, as at block 12162505.
Re-submitting the second update now reverts `0x731ea350` (`BadSignerBitmap()`: S1 is in its bitmap); at block
12162505 the same call returned `0x`. `VERSIONS.toml` `g2_updates` stays 1.

**What it does not show, and what it leaves.**
- **Our keys, a test flag.** All three signers are ours; S1's second signature was made on purpose with
  `--test-conflicting-hash`, which skips the sign-once record. Nobody dishonest was caught.
- **A test window.** 600 s (`TEST_ONLY_SHORT_WINDOW`). The evidence was ready before the submit; a contester starting
  cold would need to prove (225 s here) and wrap (244 s here, plus the queue) inside the window.
- **S1 is ejected for good on this gate.** S2 and S3 can still update: 2 active keys = the threshold, so one more
  ejection would kill the gate (`dead()` true). The set cannot be changed without a new gate.
- **No stake was slashed.** The keys are stand-ins, not staked sequencer keys, and LEZ has no evidence-based slashing
  (we proposed it upstream: logos-blockchain/logos-execution-zone issue #1130).
- **The cancelled checkpoint is refuted on the gate:** that exact update (24720 → 26120 with that hash) cannot be
  submitted again; a later update from tip 24720 has to end at a block other than 26120.
- **Halting** (a conflicting quorum) is still shown on anvil only (the `seq-sig` record).

### Re-read (any machine, read only; from this folder)

```sh
R=https://sepolia.gateway.tenderly.co; G=0x5E891d52eB9a729feCFDd9051685FDeaCd9F0FaE; V=0x4D7B360cf385d3477F1A67c1AEdEB6C17c26d23E
for t in 0x47f4cb216fd2e3939a3a916c99f5c5d43b1fee1c938ce9a1186878bc10e53eff 0x0291308ed09e18de620482673237e06d70ecc9d90fce682e90ecfd2500e91b84; do
  cast receipt $t --rpc-url $R --json | python3 -c 'import json,sys;r=json.load(sys.stdin);print(int(r["blockNumber"],16),int(r["gasUsed"],16),r["status"],r["to"])'
done   # 12162506 974563 0x1 the gate; 12162508 479624 0x1 the gate
# UpdateAccepted(digest, newState, blockId, blockHash, signers, finalizableAt), then
# Equivocation(digest, ejectedKeys, cancelled, blockId, blockHash, conflictingHash): one 32-byte word per field, in hex
# without leading zeros (6608 = block 26120, 3 = signers 11, 6afdcea4 = 1795018404)
cast logs --from-block 12162506 --to-block 12162508 --address $G --rpc-url $R --json | python3 -c '
import json,sys
for l in json.load(sys.stdin):
    d=l["data"][2:]; print(int(l["blockNumber"],16), l["topics"][0][:10], [d[i:i+64].lstrip("0") or "0" for i in range(0,len(d),64)])'
cast call $G 'ejected()(uint64)' --rpc-url $R                           # 1 (bit 0 = S1)
cast call $G 'ejected()(uint64)' --rpc-url $R --block 12162507          # 0, before the contest
cast call $G 'activeSigners()(uint256)' --rpc-url $R                    # 2
cast call $G 'dead()(bool)' --rpc-url $R; cast call $G 'halted()(bool)' --rpc-url $R   # false, false
cast call $G 'finalizableAt()(uint256)' --rpc-url $R                    # 0: nothing pending
cast call $G 'finalizableAt()(uint256)' --rpc-url $R --block 12162507   # 1795018404, while the second update was pending
cast call $G 'refuted(bytes32)(bool)' $(cast keccak $(cast abi-encode 'f(uint256,bytes32,uint256,bytes32)' 24720 \
  0x4d99c122dbb1f8c6bbdb82a083d2a15b6193bd90ff6b91a708f62274729ff1bd 26120 0xc2e59496a178bcb379fcfe7143b109ac7ed9692a633eaa8973fc0ea5b4331aee)) --rpc-url $R   # true
cast call $V 'g2State()(bytes32)' --rpc-url $R; cast call $V 'g2State()(bytes32)' --rpc-url $R --block 12162505   # both 0x319f231d…f936
cast call $V 'exitCount()(uint64)' --rpc-url $R                         # 1, as before
H() { python3 -c 'import sys,hashlib;print("0x"+hashlib.sha256(bytes.fromhex(open(sys.argv[1]).read().strip()[2:])).hexdigest())' $1; }
cast call 0x8c915b5071206C6E328BC18529586c513408aB3b 'verify(bytes,bytes32,bytes32)' $(cat update2-seal.hex) \
  0x93c88f77e60ee5698849163ec51d6b8647e58df752984dcb15ca2f011c96955d $(H update2-journal.hex) --rpc-url $R      # 0x: accepted
cast call 0x8c915b5071206C6E328BC18529586c513408aB3b 'verify(bytes,bytes32,bytes32)' $(cat evidence-seal.hex) \
  0x9df4de66eb874e5c8570a5e4f4332b3a505de9e80c0f7dc8ea67f77daba2d21d $(H evidence-journal.hex) --rpc-url $R     # 0x: accepted
cast call $G 'submit(bytes,bytes)' $(cat update2-journal.hex) $(cat update2-seal.hex) --rpc-url $R --block 12162505   # 0x: it passed then
cast call $G 'submit(bytes,bytes)' $(cat update2-journal.hex) $(cat update2-seal.hex) --rpc-url $R   # now reverts 0x731ea350 = BadSignerBitmap()
cast keccak "v7 Sepolia equivocation demo 2026-11-18: a block 26120 that is not the chain's"         # 0x66db9327…1597, the conflicting hash
# LEZ: block 26120's real hash, from the public endpoint (id, then the hash)
curl -s -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[26120]}' https://lez.nameswallet.app/ \
  | python3 -c 'import sys,json,base64;b=base64.b64decode(json.load(sys.stdin)["result"]);print(int.from_bytes(b[:8],"little"),"0x"+b[40:72].hex())'
```

Run 2026-11-18 from a workstation: every value above came back as stated; the two receipts' calldata hold this
folder's journals and seals.

## What v7 changes against v6

Every value fixed at deploy (immutable on Sepolia, or fixed at the LEZ bridge's `InitializeAttested`). "New"
means a value that follows from a new instance (addresses, ids), not a choice.

| Value | v6 | v7 | Why |
|---|---|---|---|
| Vault's `verifier()` | `UpdateGatedVerifier` `0x4aFB…aC1B` | a `SeqSigGate` (new) | the route |
| Updates accepted from | `SUBMITTER()` `0xB81a…95f4` only | anyone, with 2 of 3 signatures in the proof | the route |
| Vault `updateImageId()` | `0x098f…3017` (TrustedTip) | the seq-sig image `0x93c88f77…955d` (TrustedTip statement + signatures) | the route |
| Gate `EVIDENCE_IMAGE_ID()` | none | `0x9df4de66…d21d` | the route (contest) |
| Vault `releaseImageId()`, gate `RELEASE_IMAGE_ID()` | `0x6bcb…72a4` | `0x6bcb…72a4` (the same) | copied |
| RISC Zero verifier behind the gate | 3.1.0 `0x8c91…aB3b` | 3.1.0 `0x8c91…aB3b` | copied |
| Signer set (`SIGNER_SET_HASH()`), `SIGNERS()` | none | the three public keys (sorted; index = bitmap bit), n = 3 | decided |
| `THRESHOLD()` | none | 2 | decided (the gate needs n/2 < T <= n − 1) |
| `CONTEST_WINDOW()` | none (no window) | 600 s | decided |
| `TEST_ONLY_SHORT_WINDOW()` | none | **true** (see below) | forced by 600 s |
| Gate `VAULT()`, `CFG_ID()` | (the gate serves any vault naming it) | the v7 vault, its cfgId (computed by the gate) | the gate's design |
| Vault `zoneId()` | `0x2161…3fff` (light-client instance `0xe4b7…3600`, slot 83) | the same | copied (same instance) |
| Vault `lezChainId()` | v5's | the same | copied |
| Vault `programId()` | bridge `4PxaLAfP…QWed` | a new header (BP7) of the same v3 canonical ELF `0dabbd6c…9ffa` | new instance |
| Vault `initialG2State` | genesis at LEZ 23273 | genesis at the tip right after BP7's deploy | new instance |
| Vault `cfgId()`, addresses | v6's | new | new instance |
| `minReleaseDelay`, tokens' release delay | 0, 0 | 0, 0 | copied |
| Admin, freezer, fee recipient | the deployer `0x37a4…321F` | the deployer | copied |
| Tokens, denominations, deposit and withdraw fees, relay-fee menus, caps, cap weights | `SepoliaV4.configure()` | `SepoliaV7.configure()`, the same values | copied |
| DemoUSDC | v5's `0xAc20…a71e` (reused) | the same (reused) | copied |
| Deposit desk | `0x8Aab…A1c3`, 30 bps | a new desk for the v7 vault, 30 bps | copied |
| LEZ bridge: root source | `InitializeAttested`: program `9Wb7…uhNR`, instance `0xe4b7…3600`, slot 83 | the same | copied |
| LEZ bridge: admin, freezer | the chain operator | the chain operator | copied |
| LEZ: token program, wrapped definitions | `5w6n…rbU7`, v6's definitions | the same program, new definitions (authority `Program(BP7)`) | new instance |
| LEZ: sponsor, float | 25,000,000; 1,000,000,000 | the same | copied |


## What a signer checks, and what it does not

`public-demo/v7/signer.sh sign` runs `seqsig attest`. Before it signs block N, it:
- derives the G2 genesis state itself, from the chain (`signer.sh genesis --block <tip> --bridge <BP7>`),
  rather than trusting a file;
- fetches every block from the vault's last accepted tip to N from the public endpoint, at most 3 blocks a
  second;
- replays them natively (`outbound_host::prepare`): it recomputes each block hash and the `prev_block_hash`
  chain, and extracts every claim, burn, root and registry change the way the bridge program does;
- requires the replay's cfgId to be the vault's, and the replay to end at N;
- requires N to be at least 20 blocks under the head (this chain has no Bedrock finality, so depth is the
  rule, as for the v5 updater);
- writes (cfgId, previous tip, N) and the hash to its append-only sign-once record before the signature
  exists, and refuses a second, different hash for the same entry.

**It does not check:**
- **Privacy receipts.** Private claims and burns are taken on the sequencer's word. LEZ rc3 makes those
  receipts with risc0 3.0.5, which the 3.1 host refuses (advisory GHSA-g6qg-9j8p-7j87).
- **Public execution.** No proof of it exists on LEZ.
- **That its blocks are the ones the other signers saw.** All three machines read the same endpoint of the
  same single sequencer. Three keys on three machines spread the custody of the signing keys; they do not
  give three views of the chain.

## Immutable values that were decided, and the one the gate forced

- **Decided by the owner:** 3 signers, threshold 2, one key each on three separate machines, a 10-minute contest
  window, everything else copied from v6/v5.
- **Forced by the gate: `TEST_ONLY_SHORT_WINDOW = true`.** `SeqSigGate` refuses a window under
  `MIN_CONTEST_WINDOW` (6 hours) unless its constructor flag `TEST_ONLY_SHORT_WINDOW` is true. The flag is an
  immutable, readable on chain, and it also lets a gate take T = n (v7 does not: T = 2 of 3). The deploy script
  takes it as a REQUIRED input with no default, so it is never set silently. The options are:
  1. `TEST_ONLY_SHORT_WINDOW=true` with `CONTEST_WINDOW=600` (chosen for the run; the gate says on chain
     that it is a test gate);
  2. `TEST_ONLY_SHORT_WINDOW=false` with a window of at least 21,600 s (6 h; the round trip then waits 6 h
     between submit and finalize);
  3. lower `MIN_CONTEST_WINDOW` in `SeqSigGate.sol` (a contract change, outside this branch's scope).
- **Not a choice, but worth knowing:** the order of the keys is their sorted order, and a key's index in it is its
  bit in every signer bitmap. That follows from the public keys alone.
- No other immutable is forced by the gate's design.


## Rehearsal

`public-demo/v7/rehearse-v7.sh` on the build host, run `r2`, 2026-11-18 (stages `chains` … `equivocate`, then
`fork`). The same scripts as the runbook: `SepoliaV7.s.sol` (through its harness, deployer = anvil #0),
`readback-v7.sh`, `SepoliaV7Ops.s.sol`, the real `SepoliaDesk.s.sol` and `SepoliaDeskDeposit.s.sol`,
`prove-seqsig-rc3.sh --check`, `prove-release-rc3.sh --check`, `signer.sh`, and the canonical `seqsig` and ELFs of
`public-demo/v7/BUILD-seqsig.txt`. **Mock-verified: proofs executed, not proved** (a `MockGroth16Verifier` stands in
for RISC Zero's check only); the light-client anchor on anvil is test-only (`attest_guest_synthetic`); the three
signers were three directories on one host. A throwaway LEZ rc3 chain and anvil (chain id 11155111, block gas limit
200M as Sepolia's), fresh keys only, both stopped afterwards.

| Step | Result |
|---|---|
| Bridge header BP7 (v3 canonical ELF `0dabbd6c…`), then the genesis at once (tip 27) | as `v7-public.sh` |
| Three signer keys (mode 600), the set (sorted; index = bit) | `seqsig set` = the gate's `SIGNER_SET_HASH()` (read back) |
| `SepoliaV7.s.sol`: pins from `cast nonce` / `cast compute-address` / `lez-bridge cfg-id`, simulate, broadcast | the gate and the vault landed at the pinned addresses with the pinned cfgId and set hash; `readback-v7.sh`: `READBACK OK` (26 values) |
| `configure()`, the desk (30 bps), the first deposit through the desk (multiplier 1000) | applied; first deposit 268,361 gas |
| `InitializeAttested` (cfgId = the vault's), register, sponsor, float; `check-init` | the walk from genesis reads the vault's cfgId |
| Public claim (`ClaimAttested`, no proof), LEZ block 42 | applied |
| Public burn to a seed-derived address (balance 0 before), `exit-secret` (mode 600) | applied, block 43 |
| Signers a and b derive the genesis themselves (= the vault's), replay 28..50, sign block 50 | signed; signer a again on the same hash: allowed (`already in the signing record`) |
| Signer c on a block 1 under the head (depth 3 here, 20 in the run) | refused: `not final by this attestor's rule`, nothing recorded |
| `prove-seqsig-rc3.sh --check` (the run's script, pinned host and ELF, gate and vault reads) | 2,822,215 user cycles, **7 po2-19 segments**; TrustedTip on the same batch 804,618; the signature check 2,015,044 for 2 signatures |
| 1 of 3 in the guest | refused: `seq-sig refused: 1 distinct valid signers, threshold 2` (no journal) |
| 1 of 3 at the gate (the real journal with one bitmap bit, its seal) | refused before the verifier: `BadSignerBitmap` |
| `submit` (SepoliaV7Ops, anvil #5: no role) | staged, bitmap `101`; the vault unchanged; 363,544 gas |
| `finalize` inside the window | refused: `the contest window is still open` (the script's own check) |
| `finalize` after 600 s (anvil #6: no role) | applied: g2State moved, exitCount 1; 207,566 gas |
| `prove-release-rc3.sh --check` against the anvil vault | the root recorded, the nullifier unspent |
| `release` (SepoliaV7Ops, anvil #6) | paid 0.01 ETH to the fresh address; 139,813 gas; vault counters 1 1 1 1 |
| Equivocation: update 2 (a, b) staged; a signs another hash for block 98 (`--test-conflicting-hash`, no record); evidence guest (1,135,835 user cycles, 2 segments); `contest` | a ejected (mask `001`), update 2 cancelled; 2 keys active = T, the gate is not dead; resubmitting update 2 refused (`BadSignerBitmap`); `contest` 87,835 gas |
| **Fork of Sepolia** (block 12158928), the REAL verifier 3.1.0 (`VERSION` 3.1.0, `SELECTOR` 0xe71e6620) | pins, simulate, broadcast, `READBACK OK`, `configure()`, the desk, the first deposit (multiplier 1000), all applied on the fork (anvil's gas schedule: this shows the flags and the scripts work, not that Sepolia's gas fits; see the runbook's gas note); `submit` with a mock seal reverted inside the verifier (empty revert data: not one of the gate's own errors) |
| The run's own deploy branch on a fresh anvil: `REUSE_USDC` (a DemoUSDC minted by the deployer), the three REAL public keys, pins at nonces N and N + 1, `EXPECTED_SET_HASH` = `signer-set.json`'s | the gate and vault at the pins; `READBACK OK`; `configure()` with the reused token |

Gas is anvil's with the mock verifier; on Sepolia `submit` and `release` each add one Groth16 verify (about
300k gas; v4's whole `update` cost 671,254–899,746, `DESIGN.md` §4).

The run's log stopped five times on the scripts themselves, each fixed and the stage run again (anvil's default block
gas limit under the multiplier 1000; `cast compute-address`'s output format on the build host's Foundry 1.8.3; the
prover's `--out` option; the window read from forge's simulation instead of the chain; forge refusing to read files
outside the project, so values now pass in the environment).

**The signers on their machines, against the public chain (read only):** `signer.sh selftest` (a throwaway key in a
temp dir: derive v6's genesis at LEZ 23273, fetch 23274..23573 from `https://lez.nameswallet.app/` at 3 a second,
replay, sign 23573 at depth 20) on each of the three machines: 333 s, 347 s and 390 s; each derived v6's genesis
state `0x9c7fcc5c…fb04` (= v6's `VERSIONS.toml` value) and signed the same block hash `0x2177f210…bd9f`. The keys for
the run were then made on each machine (`signer.sh keygen`), and only their public keys collected
(`public-demo/v7/signer-set.json`).

**Executor measurement for the run's update:** the canonical seq-sig ELF over those 300 public blocks (v6's bridge,
two throwaway keys signing at depth 20): 8,949,029 user cycles, **23 po2-19 segments** (the TrustedTip statement
6,930,728, the two-signature check 2,024,821). At about 90 s a segment that is about 35 min of proving for 300
blocks; `--check` gives the real count at run time.

**Canonical build** (`public-demo/v7/BUILD-seqsig.txt`, commit `3c767ef`, a clean archive, `--locked`): update
image `0x93c88f77…955d`, evidence image `0x9df4de66…d21d`; 13 + 9 tests passed. A second build of the same commit in
another directory gave the same evidence image (and v4's release image) but another update image (`0xf708db95…76ee`):
the update guest embeds its build directory, as the v4 G2 guests do, so it is not reproducible across directories;
every proof uses the canonical ELF (`--elf`/`--image-id`, pinned in `prove-seqsig-rc3.sh` and `VERSIONS.toml`).


## Runbook (summary; followed as written in the run above)

The commands are in the operator's runbook, `public-demo/v7/RUNBOOK-v7.md` (private: it names machines and paths).
In order:
0. Version check; optionally S0, one real seal of the update image checked read-only by the verifier (the rehearsal's
   batch: 7 segments, about 10 min, then the wrap).
1. LEZ: deploy the bridge header BP7, take the genesis state at once, the ids.
2. Sepolia: pins from the deployer's nonce, `SepoliaV7.s.sol` simulated then broadcast, `readback-v7.sh`,
   `configure()`, the desk; `VERSIONS.toml` and `check-versions --onchain --deployment v7`.
3. LEZ: the deposit commitment, wrapped tokens, `InitializeAttested`, register, sponsor, float, `check-init`.
4. Sepolia: one 0.01 ETH deposit through the desk (gas multiplier 1000).
5. Finality and the light-client follower's next update past the deposit (about 30-45 min), then the public claim.
6. The public burn to a seed-derived, never-used address; the exit secret on the build host only.
7. 20 blocks later, two of the three signers each derive the genesis and sign the same block N.
8. The update proof (owner-started; about 35 min for 300 blocks), its Groth16 wrap on a hosted runner (15-20 min).
9. The 1-of-3 journal refused read-only (before the submit: once an update is pending the gate answers `PendingExists`
   first); `submit` from a key with no role; 600 s; `finalize`.
10. The release proof (one segment), its wrap, `release` to the fresh address.

## Limits

- **Stand-in keys.** The three keys are the team's, each made on and kept on one of three of the team's machines;
  they are not LEZ sequencer keys (LEZ v0.4.0-rc3 headers carry no signature). All three read the same endpoint of
  the one sequencer: they spread custody of the signing keys, not of the chain's truth.
- **A test window.** 600 s is allowed only by `TEST_ONLY_SHORT_WINDOW = true`, visible on chain; the gate's normal
  minimum is 6 h. 10 minutes is too short for a contester who must prove evidence (about 1.1M cycles, 3 segments and
  225 s on Sepolia's run) and wrap it on a hosted runner (244 s there, plus the queue).
- **Still trusted:** the team runs the only sequencer; private claims and burns are taken on the sequencer's word (as
  in v5 and v6: the signers check neither privacy receipts nor public execution); the chain operator is the bridge's
  LEZ admin and freezer, and the deployer is the vault's admin and freezer.
- **One update, by hand.** No standing signer service and no updater serves v7: each update is the runbook again
  (signers, proof, wrap, submit, the window, finalize).
- **Finality is depth:** 20 blocks under the head (about 5 min); the chain has no Bedrock finality.
- **The update image is not reproducible across build directories**; the canonical ELF is kept on the build host and
  pinned by hash (`public-demo/v7/BUILD-seqsig.txt`, `VERSIONS.toml`).
- **Equivocation ran on Sepolia with our own keys and a test-flag signature** ("Equivocation on Sepolia" above): S1 is
  ejected for good on this gate, so S2 and S3 must both sign every later update. **Halting** (a conflicting quorum) is
  shown on anvil only (the `seq-sig` record).
- Nothing is audited. All funds are test funds.
