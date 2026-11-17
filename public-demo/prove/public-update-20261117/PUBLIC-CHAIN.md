# The public update path on the team's public demo chain (2026-11-17)

**Result.** The RFP-021 attestation program with the public update path (the image of `README.md` beside this
file) runs on the **team's public demo chain** (LEZ v0.4.0-rc3, operator-run, reachable at
`https://lez.nameswallet.app/`). A live Sepolia finality update (506 of 512 signers) was verified inside one
ordinary public transaction, LEZ block **21641**, with no proof anywhere. A consumer then used the new anchor in
block **21643**: the reference probe proved that the live Sepolia **v5** vault's `isDepositRoot[root]` is 1. A loop
sends one more such update every 30 minutes until the current sync-committee period ends.

State label (`DESIGN.md`): **(1) run on a chain: the team's public demo chain** (operator-attested, rc3). This is not
a Logos network, and the sequencer is the team's own.

## Ids

| What | Value |
|---|---|
| Program (header account, immutable) | `GGo5C4hf4kMF2198SqWka6BRgUCL1LDNASJaBADFnxtT` |
| Image id (computed by the deploying CLI from the ELF) | `36873168360c8f4348eea122ce43eddb570a43e79fdd3914ab93869017eecc7d` |
| ELF | `attest_guest_public.bin`, 998,300 bytes, sha256 `07596242e2b81917263baa5ccf4d8392e092d7c28f2bf41a7d61ddf056b7d895` (the same file as the local run, `elf.sha256`) |
| Instance | `0xd75ae73188e10f726678b5495e5cc0db3ccd9104c8ca3ddbbb2ca96bd5683eb1` (Sepolia, chain 11155111) |
| Config PDA / store PDA | `7fo7mBataPPrf6pnrrBLFzyC7xUq5ApToyLVSdVDkFSS` / `726S4ZeFJEo6ezHYubKqqBmgxs8pN8L8fuk9fGgUehPK` |
| Anchor PDA of the Init checkpoint / of update 1 | `3aCPJtqPgar9wtGUHfpp3azFMCpWeJfzoqm7RCKNCu4g` / `yrFnyYrUNDYKXM6a37fQM6aQvVM7DhoPKmHfLLKr9qZ` |
| Reference probe (consumer) | `3Eo8HutrUK552X6QNkTWXbxTmtyrxZxySK5qw92HvwvM` (image `d58e703f…fa40`) |
| Submitter and fee payer | `EKKwftRVbKfVafDfUEQer8ypTEZBLQK3A5vMDReNEwkX`: a throwaway key made for this run, with no role in the bridge. It was funded with 5,000,000,000 test LEZ from the chain's operator (block 21616, tx `9aa63829…4348`) |

## Transactions

Fees are in native test LEZ. Each fee is the payer's balance before minus after, read from the sequencer, with no
other transaction from that payer in between. Base fees were exec 8 / storage 8 at each block's opening.
Execution gas = (fee − wire bytes × 8) / 8, with the wire length read back by `getTransaction`.

| Step | LEZ block | tx | Metered |
|---|---:|---|---|
| Deploy the program: 10 segments + header, immutable | 21618–21628 | 11 txs, `153f8604c53f47fd823557a9025584fe31af0ad8fe161777dbd9b0d64c92cd76` … `323d0fadb53a60e33311cdacf9fa03931ea112196528a2a88eb3972ad928874b` | 22,453,424 for all 11 |
| `Init` at Sepolia's finalized checkpoint, public | 21631 | `39d39c9e694f178cf9fef1e81a8271fb69fc4c600edb6f9fb4d8fad999299243` | fee 73,070,792; 26,286 wire bytes; 9,107,563 exec gas |
| Deploy the reference probe: 5 segments + header | 21633–21638 | 6 txs, `d9cc114a8d5b7dca7f0cf9e9118c512f7b5a6b442457b70b0522c62a4b5c06f4` … `da49667f9aacc7b8b4c9da16e588c1470544d22702d425129dfec0fe0dc5faf5` | 9,127,600 for all 6 |
| **Update 1, public (`UpdatePublic`), gas limit 10,000,000** | **21641** | **`9069d7fc0519fb8d9963218d60625f0d11b60c34290745a5895fc9dcbdbd77d8`** | fee **74,506,688**; 27,773 wire bytes; **9,285,563** exec gas. The executor's sum of the transaction's program calls is also 9,285,563 (submitter's actor 9,227,923 + `Advance` 39,510 + `RecordAnchor` 18,130): 92.86% of the 10M cap |
| Consumer statement under update 1's anchor, public | 21643 | `19108d80c486cefd51db03ab8affff6fc8a3b3a55e9d2090fa3761f472b6d20e` | fee 2,143,400; 5,996 wire bytes; 261,929 exec gas |

**Init checkpoint:** Sepolia finalized slot **11600256** (period 1416), beacon root `0xf02ac3d3…e435`, execution block
hash `0xdae92a29…0eeb`.

**Update 1:** signature slot 11600353, attested slot 11600352, finalized slot **11600288**, participation
**506/512**. New anchor: execution block hash `0xf75aed1e60904cb631cb07fe5526506d19cdd66b601e72dd1c03717ff42c4d2b`.
Read back: store `updates 1`, finalized slot 11600288, latest anchor recorded. The transaction was applied, not
reverted: a reverted one pays its whole limit, about 80M.

**Consumer:** the statement covers the v5 vault `0xAFbDF192434E9888Bb23DAE6f2D15Eea6246eD10`, mapping `isDepositRoot`
(storage slot 83) at key `0x64dcf21bd6c5b15649de1a040f0657f7f63bda7ed02e5109d1cf0d1f06677d07`, value 1. Its inputs:
- `eth_getProof` at Sepolia execution block 12155924, the block of update 1's anchor (9 account + 4 storage nodes);
- the vault's `depositRoot()` at the time of the run.

Statement id `0x517a6ec59af70f9903fee087fa930303cc970575286279d7fc2962a2f0310a08`, held by the probe actor of the
submitter.

Later updates from the loop are in its log. The record lists them only when it is next refreshed.

## Check it yourself (any machine, read only)

The endpoint is the RPC filter in front of the sequencer. Each answer is LEZ's own JSON-RPC: a transaction or block
comes back as base64 borsh.

```sh
E=https://lez.nameswallet.app/
rpc() { curl -s -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" $E; }

# the update: [base64 transaction, block]; 27,773 bytes once decoded
rpc getTransaction '["9069d7fc0519fb8d9963218d60625f0d11b60c34290745a5895fc9dcbdbd77d8"]' \
  | python3 -c 'import sys,json,base64;t,b=json.load(sys.stdin)["result"];print("block",b,"bytes",len(base64.b64decode(t)))'
# the Init, the consumer statement, the first and last deploy transactions
for h in 39d39c9e694f178cf9fef1e81a8271fb69fc4c600edb6f9fb4d8fad999299243 19108d80c486cefd51db03ab8affff6fc8a3b3a55e9d2090fa3761f472b6d20e \
         153f8604c53f47fd823557a9025584fe31af0ad8fe161777dbd9b0d64c92cd76 323d0fadb53a60e33311cdacf9fa03931ea112196528a2a88eb3972ad928874b; do
  rpc getTransaction "[\"$h\"]" | python3 -c 'import sys,json,base64;t,b=json.load(sys.stdin)["result"];print(sys.argv[1][:8],"block",b,"bytes",len(base64.b64decode(t)))' $h
done
# the block that carries the update (base64 borsh of the whole block)
rpc getBlock '[21641]' | python3 -c 'import sys,json,base64;print(len(base64.b64decode(json.load(sys.stdin)["result"])),"bytes")'
# the store: the 259-byte summary (instance, chain id, finalized header, anchor, committees, update count)
rpc getAccount '["726S4ZeFJEo6ezHYubKqqBmgxs8pN8L8fuk9fGgUehPK"]' | python3 -c '
import sys,json,struct
b=bytes(json.load(sys.stdin)["result"]["data"]["actor_states"]["GGo5C4hf4kMF2198SqWka6BRgUCL1LDNASJaBADFnxtT"])
print("instance 0x"+b[0:32].hex(), "chain", struct.unpack("<Q",b[32:40])[0])
print("finalized beacon slot", struct.unpack("<Q",b[40:48])[0], "anchor block hash 0x"+b[184:216].hex())
print("updates", struct.unpack("<Q",b[-8:])[0])'
```

The store's layout is `LcSummary` in `lez-bridge/attest/core/src/lib.rs`. With no next committee known (one `None`
byte), the finalized slot is the u64 at byte 40, the execution block hash bytes 184..216, and `updates` the last 8
bytes. To compare with Sepolia, use any beacon API's
`/eth/v1/beacon/light_client/finality_update` (`finalized_header.beacon.slot`). The store trails it by at most one
loop interval plus one epoch.

These commands were run from outside the build host on 2026-11-17. `getTransaction` returned block 21641 and
27,773 bytes. `getBlock` served a 99,627-byte deploy block (21620) and block 21641. The store decoded to slot
11600288 at offset 40.

## The loop

- **Cadence:** every 30 minutes. Each attempt does the following, in order:
  1. Read the balance, the store's finalized slot, and the beacon node's finality update.
  2. Do nothing when Sepolia's finality is not past the store.
  3. Fetch the update and verify it natively.
  4. Build the message and sum its cycles in the executor; skip at or over 9.9M (low participation).
  5. Dry-run it.
  6. Re-read the store and skip if anyone else advanced it since the fetch: a stale `prev` would revert and pay
     the whole limit.
  7. Send with gas limit 10,000,000, then read the store back.

  One log line per attempt.
- **Stops by itself** at the end of sync-committee period **1416**: Sepolia slot **11,608,064**, **2026-11-18
  19:32:48 UTC**. It stops when the update's signature slot or finalized slot falls in the next period, because the
  public path cannot cross a committee handoff (`README.md`, "Not done"). It also stops under a floor of 300,000,000
  test LEZ, or when a send does not move the store.
- **Fees:** one update costs about 74.5M test LEZ at the floor base fee (above). At 48 updates a day that is
  about **3.6 billion test LEZ a day**. To the period boundary that is about 51 updates, about 3.8 billion, under the
  4.82 billion the payer held after update 1.
- **Block space:** the transaction declares the whole 10M gas cap, so it fills its block. The rc3 block builder defers
  (does not drop) a transaction that does not fit the remaining budget of a block. That is `lez/sequencer/core/src/lib.rs`
  at `bafd07be`: the declared-gas pre-screen, "deferred with nothing executed". Other users' transactions in the
  mempool wait one block (about 15 s) at most once per 30 minutes.

## Safety for the bridge's outbound updater (checked before anything was sent)

The v5 vault's hourly updater proves a walk over **every** block of this chain (the G2-lite TrustedTip guest). A block
it cannot walk would stall withdrawals on v5 for good. So the walker was checked against these transactions first.
- **Code** (`lez-bridge/outbound/core/src/wire.rs`, `update.rs`, `view.rs`): rc3 has two transaction kinds, public and
  privacy-preserving. A program deployment is ordinary public transactions to the program loader. A public
  transaction is decoded strictly, and its root call is read only when it targets the bridge, or the token program
  with a burn that notifies the bridge. Every other program's call is skipped after decoding. A block's whole byte
  string is hashed, so the cost grows with block size: about 19.7k cycles for an empty block, 0.24–0.43M for a
  99,627-byte deploy block.
- **History:** no program deployment fell in the range the v5 vault's accepted updates had walked (11599..20961), nor
  anywhere up to block 21441:
  - the only blocks over 8 KB there are two private-transaction blocks, 15255 and 15395 (about 255 KB each), and
    accepted updates walked both;
  - the 14 deploy blocks of the v3 programs (4486–4500, 99,627 bytes each) were walked by the executor in the
    2026-11-15 measurement (`research/outbound-public-measure-2026-11-15.md`).
- **Executor run (no proof)** on a copy of the local chain of `README.md`, which holds exactly these transactions: the
  deploy (blocks 24–34), `Init` (35), two `UpdatePublic` (46, 71), the probe's deploy (50–54) and its statement (72).
  A stand-in bridge was deployed and initialised after them (block 125). The updater's own pinned host, trusted-tip
  guest ELF (image `0x098f5a1a…3017`, the v5 vault's) and po2 18 walked from a genesis at block 20 through block 126.
  It completed: **10,949,116 user cycles, 84 segments, 0 assumptions, journal equal to the native one**.

  | Block | Bytes | Walker cycles (parse + hash + extract) |
  |---|---:|---:|
  | deploy segment (each of 10) | 99,627 | 284,771–434,646 |
  | `Init` | 26,997 | 139,111 |
  | `UpdatePublic` (46 and 71) | 28,484 | 101,026 |
  | probe statement | 6,965 | 43,739 |
  | empty block (average of 45) | < 2,000 | 19,694 |
- **On the public chain itself**, read only, the same host and guest ran in executor mode after each step. Each run
  started from the vault's accepted G2 state (tip 20961) and used the updater's settings:
  - through the deploy (to 21629): 16,873,465 user cycles, 106 segments;
  - through the update and the consumer (to 21647): 19,091,486 user cycles, 123 segments.

  Both completed with the journal equal to the native one.
- **One-off cost to the updater:** about 5.6M user cycles above an empty stretch for the two deploys, the `Init`, the
  update and the statement. That is about 40 extra po2-18 segments, about 50 minutes of proving on the updater's
  proving host at 77 s a segment, once (123 segments for the 686 blocks against about 83 for an empty stretch). Each later update block adds about 81k cycles, about 3.9M a day at 48 a day, about 3.5% of an
  empty chain-day.
- **The updater's own cycle over these blocks:** <<UPDATER>>

## Not done, and limits

- **No committee handoff:** at the period boundary the instance stops following. A period update needs the private
  `Update` (a proof) or the two-step split, neither run here.
- **Participation:** below 441/512 one finality update no longer fits the 10M cap (`README.md`, sweep). Such an
  update is skipped (logged), not sent.
- **Every update fills a block.**
- **Trust:** the light client's statement holds only as far as the sequencer applied the transaction honestly. Nobody
  re-executes it (no proof). Its value is that every check runs in public, deterministic execution that anyone with
  the ELF can replay from the transaction bytes; the sequencer is the team's.
- **A consumer must fetch `eth_getProof` within about 128 Sepolia blocks of the anchor** on public nodes. The
  statement here was frozen right after the update was fetched.
- **The program is not used by the bridge:** the v5 bridge still takes its deposit roots from the operator's
  attester. This instance is a demonstration beside it.
- **Reviewed once, not audited** (`README.md`, Review).
