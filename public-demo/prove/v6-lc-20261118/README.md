# v6: a bridge whose deposit roots are verified by the RFP-021 light client (2026-11-18)

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

**Result.** A 0.01 ETH deposit into the Sepolia vault **v6** was claimed on the team's public demo chain in
LEZ block **23408** by an ordinary **public** transaction. **No attester key: the deposit root was verified
by the light client.** The bridge proved the vault's `isDepositRoot[root] = 1` (mapping slot 83) with an
`eth_getProof` under Sepolia block 12158083, an execution anchor that the RFP-021 light-client instance had
finalized from Sepolia's own sync-committee signatures in a public update (LEZ block 23406). The operator's
attester, the root source of v2–v5, was refused by this bridge (LEZ block 23349, reverted). Nothing was proved
anywhere: every LEZ step is a public transaction the sequencer executes.

What this does not remove: the team still runs the only sequencer, the chain operator is the v6 bridge's LEZ
admin and freezer (as for v5), and **no updater serves v6, so v6 has no exit running today** (Limits).

State labels (`DESIGN.md`): **(1) run on a chain**: Sepolia (vault, desk, deposit) and **the team's public
demo chain** (LEZ v0.4.0-rc3, operator-run, `https://lez.nameswallet.app/`); the rehearsal ran on a
**local** LEZ sequencer and **anvil**. Not a Logos network.

## What v6 changes, and what it does not

- **The root source.** v2-v5 bridges take vault deposit roots from the operator's attester (the stand-in
  root source). The v6 bridge is created with `InitializeAttested`: its config names an RFP-021
  light-client instance, and a claim proves the vault's `isDepositRoot[root]` (mapping slot 83) under an
  execution anchor that instance finalized (`AssertAnchor`). No attester key: deposit roots are verified by
  the light client.
- **No rebuild.** The bridge source at `public-update` equals the v3 canonical build's (`c9ec880`): the
  only change under `lez-bridge/bridge/` since is a test. `eth_attest_core` changed only by appended enum
  variants (new `Instruction` and `Effect` variants after the existing ones; borsh indexes of the old ones
  unchanged), new functions and new panic texts; `Message`, `Query` (`AssertAnchor`, `AssertInstance`),
  `Anchor`, `AnchorWitness`, `StorageProof`, `LcSummary` and the PDA seeds are byte-identical. The
  attestation program answers both queries with the same logic and new texts (`RFP021-E01`, `E07`). So
  v6 uses the v3 canonical bridge ELF (image `0dabbd6c…9ffa`) at a new header and v5's two Ethereum
  images (update `0x098f5a1a…3017`, release `0x6bcbd714…72a4`); the G2-lite view already decodes
  `ClaimAttested` (`lez-bridge/outbound/core/src/view.rs`), shown in the rehearsal (step 8).
- **The vault:** v5's design and settings exactly. Only what the root source forces differs: `zoneId`
  (the source id of the light-client instance), `programId` (the new bridge header), the genesis G2 state
  (a new bridge), and so the `cfgId` and the addresses. Outbound keeps v5's design (TrustedTip updates
  through an `UpdateGatedVerifier` with the same submitter). **No updater serves v6**: the outbound updater
  stays on v5 (one update about every two hours), so v6 has no exit running today; a v6 burn would wait for
  a v6 update.

## Part 1: the light client across handoffs, public (live)

The program from branch `public-update` (`UpdatePublic`, `PeriodBegin`/`PeriodFinish`, `CloseJob`,
`FinalityBegin`/`FinalityFinish`, the statements; built with the weak-subjectivity rule), deployed as a
**new** program beside the finality-only one of `public-update-20261117/PUBLIC-CHAIN.md` (left unchanged).

| What | Value |
|---|---|
| Program (header, immutable) | `9Wb7sBxsHBV1RPhHj4qPhQaRaiNn72L7EFjAsQdmuhNR` |
| Image id (the deploying CLI's) | `a7ea6565630eb33779b45ef84199a9e84033858030f97c5b936ea2f4b4b85b05` (the catch-up record's image, `catchup-20261117/`) |
| ELF | `attest_guest_public.bin`, 1,087,032 bytes, sha256 `2879adfa52e6ab316c261f67be6b73cba76ed095051abbea79ef836703652d2b` |
| Instance | `0xe4b7d26a10ec6c712759dc0ce887c57425901229bee42f7f6465683cc0363600` (Sepolia, chain 11155111) |
| Checkpoint | Sepolia finalized slot 11602304 (period 1416), root `0x19b8049e…ca72` |
| Payer | `9NHsr7e7rzvjCDQSab2bwUQrgLznAnyRhFCk3ZYLjXSB`, a throwaway key with no role; funded 30,000,000,000 test LEZ by the chain operator (block 23232, tx `5fb6526b…b59f`) |

| Step | LEZ block | tx | Gas |
|---|---:|---|---:|
| Deploy (11 transactions) | 23246–23257 | `bc996817…7ef0` … `38606c3e…1d6d` | — |
| `Init` | 23258 | `eceecf2c96c1149d4112474c1861283ea855f417ecdbcc4355792eed7765b0f6` | 9,117,222 |
| Period update 1416, `PeriodBegin` (506/512): learns nothing yet, opens the job | 23262 | `79affd3ef13a21f78afb81b4657466922c02670ab094276e075e1a4fffa7edf3` | 3,193,829 |
| Period update 1416, `PeriodFinish`: the store learns period 1417's committee `0x1f920a67…a0cc` | 23263 | `47b265596e7f16673b29f1f5dc076dd9fb20a64ee49583665d33b1204f7152d4` | 9,416,729 |
| Finality update (492/512, the current committee, the next one known): finalized slot 11602304 → 11602336 | 23300 | `de4628647555ca79a6bff331a8aa608a7a368b459b23f1ea3716f57a157c52ba` | 9,456,025 |

The committee learned is the one the CLI record learned from the same period's update
(`cli-ws-20261117/`, `0x1f920a67…a0cc`). Later updates are in the follower's log.

**The follower** (`public-demo/prove/lc-handoff.sh`, nohup, a log line per attempt, every 30 minutes):
1. if the store does not know its next committee, the store's own period update in two transactions
   (`--kind period`; a Begin whose Finish did not land is finished on the next attempt);
2. a finality update when Sepolia's finality is past the store: signed by the current committee, or, after
   the boundary, by the next one (the rotation).

**Not yet shown on the public chain: a rotation.** The first one is due after period 1416 ends
(2026-11-18 19:32:48 UTC, Sepolia slot 11,608,064): a finality update signed by the 1417 committee, then
the period-1417 update. The same steps ran on local chains on real data (`public-handoff-20261117/`,
`catchup-20261117/`).

It sends nothing when nothing changed (the CLI refuses an update that does not advance the store, and the
wallet simulates every transaction first; a failing simulation is not sent). It stops under a balance floor
or if an applied update did not move the store. Cost: a finality update is about 76M test LEZ, a period
update about 101M once a period (about 27 hours), so about 3.7 billion a day; the payer holds about 8 days.

## Part 2: v6, public

**Sepolia** (sent by the conductor from the throwaway deployer `0x37a46E73…321F`; `vault/script/SepoliaV4.s.sol`,
`SepoliaDesk.s.sol`, `SepoliaDeskDeposit.s.sol`, unchanged; the addresses came out as predicted from the
deployer's nonce):

| What | Value |
|---|---|
| `UpdateGatedVerifier` (the vault's `verifier()`) | `0x4aFB8eeCd80B95e79D9280D462D13dA14f9aAC1B`, block 12158040, tx `0xecae8b10d327a2f1b0aa1f0f2546d567eabd54432cedd9ef64320798d2660293`; `VERIFIER()` RISC Zero 3.1.0 `0x8c91…aB3b`, `UPDATE_IMAGE_ID()` `0x098f…3017`, `SUBMITTER()` `0xB81a…95f4` (all as v5) |
| `BridgeVault` v6 | `0x95cea20e75d3B352B49d13cb0cDdC268a14C427e`, block 12158041, tx `0x19046790894fb9de2447f50b45fae5875a531ddcd5669269bd638f52d1d315b6` |
| `cfgId()` | `0x7d1a304a12a486c730d94321244bd6d15741208c1ef9d00e352acda8589e6f60` (= the v6 bridge Config's) |
| `zoneId()` | `0x21617f2f28040bbbaa8eed893bd78b65c2e5056ab7f55fda7b3f736a97c03fff` (the light-client instance's source id) |
| `programId()` | `0x3274ad532fcf5a9e62430ad67c2d8552885c1fc395151dc4f315ffddc8ede952` (bridge `4PxaLAfP…QWed`) |
| `lezChainId()`, image ids, `minReleaseDelay()` 0, admin / freezer / fee recipient | as v5 |
| `g2State()` | `0x9c7fcc5c382e1a4af98a8dff1277ffaf4ab5eb15d869d46ff8f7d39334c0fb04` (genesis at LEZ block 23273) |
| `configure()` | 9 transactions, blocks 12158045–12158053 (first `0x3a6c793d…c4d1`, last `0xe4ebe294…56fc`): v5's tokens, denominations, zero fees, relay-fee menus, caps and weights, delay 0 |
| DepositDesk v6 | `0x8Aab670f14A53A41f20a6aA75137b13C0234A1c3`, block 12158055, tx `0xe041f0d7e8b0d608f38368bba35a4db9c5249a2d83538ef0b41478c47917c865`, 30 bps |
| Test deposit | 0.01 ETH + 0.3% through the desk, commitment `0x974417d0…9103` (for the claimer below), leaf 0, tx `0x3ca068134e2d611f46cbffbb3c1c6afdd719a2b00445d08957d1bad748b5f706`, block 12158063, 820,681 gas. A first attempt (`0xbb75456c…04bc`, block 12158058) ran out of gas and reverted: forge's estimate times 250 is too low for a vault's FIRST deposit (it creates the tree's storage); give it a higher gas limit |

`node scripts/check-versions.mjs --onchain --deployment v6` passes (0 FAIL): the vault pins the declared image
ids, `cfgId`, `programId` and gate, and the gate fronts verifier 3.1.0 with v5's update image and submitter.

**The public demo chain:**

| Step | LEZ block | tx |
|---|---:|---|
| Bridge: the v3 canonical ELF (image `0dabbd6c…9ffa`) at a new header `4PxaLAfPZDvpfZaR8SsC2Y7pYMPNwPPGVGMpjEVPQWed`, immutable | 23267–23273 | last `ae6679a3…43a4` |
| G2 genesis `outbound_host genesis --block 23273` (read only) | tip 23273 | — |
| Wrapped ETH (v6) `5WLrfRYHGY14wnNG7sa8b6Bzy9ZXPCeB4XnHK2qiuP5t` and wrapped dUSDC (v6) `CoAEWHQUT5JLvrCn2Vkeu88pdbuo891Y9NcnqRaH7msi`, authority `Program(bridge)`, on the v3 token program `5w6nTkft…ArbU7` | 23341, 23342 | `29822ba2…f7e2`, `4ce0df52…5bf3` |
| **`InitializeAttested`** (the header signs): vault v6, chain 11155111, root source = program `9Wb7sBxs…uhNR` instance `0xe4b7d26a…3600` slot 83; admin and freezer = the chain operator (as v5); the bridge derived `cfg_id` `0x7d1a304a…6f60`, equal to the vault's | 23343 | `b0b33f3aa6d30c0f0b2143fb948d2f4590545e3a05e34aa407e879e9770f3b6a` |
| Register ETH and dUSDC (the vault's denominations), `set-sponsor` 25,000,000, `fund-float` 1,000,000,000 | 23344–23347 | `59919111…b52a` … `68e33fe8…117e` |
| **The operator attester refused:** `AttestRoot` of the v6 vault's real root `0xd4057439…e3f6`, signed by the attester key of v2–v5; refused in simulation, then forced: **included and reverted**, `this bridge takes roots from an RFP-021 attestation, not the stand-in` | 23349 | `09f70f7007158ebf8469ff7b51f1e755d61708a3252376fc8c9a52649999d49b` |
| Light-client finality updates past the deposit (the follower, then one attempt by hand); the latest anchor is Sepolia block 12158083, after the deposit's 12158063 | 23384, 23406 | `65a2cc53…ddb0`, `839a3d2f01a7dd5a80fc36ae1a630421cb88655fddcbc025861f1849116c7d39` |
| **Public claim** (`ClaimAttested`) of leaf 0 by the never-used account `47RPQc4YZfctuX5Nd1qTtpDFmdAvoyWzPunwYAZFvkp2`, self-paid: the header RLP rebuilt from `eth_getBlockByHash` (684 bytes, hashes to the anchor), `eth_getProof` of the vault at block 12158083 (8 account + 3 storage nodes), `AssertAnchor` to the light-client instance; 378,802 metered cycles, 7,099 wire bytes. The bridge now reads `claims [1, 0, 0]`, `claimed_value 10000000000000000` for ETH | **23408** | **`0d8ea4315cc86d667c99a9e66d2848df36b2d51705942d6d3161c9ab8757d5b1`** |
| A repeat of the claim: refused by the CLI's pre-check (`deposit already claimed: its nullifier is spent`), nothing sent | — | — |

**This was the first claim through this bridge build under a real light-client anchor of the new image** (the
rehearsal's anchor was synthetic). It applied on the first send.

**Outbound, executor only (no proof, nothing submitted):**
- v5's update image from the v6 genesis (tip 23273) over blocks 23274..23409: the journal counts the claim
  (`claims [ETH, denomination 0, 1]`, one nullifier), 3,485,351 user cycles, 22 segments, journal equal to the
  native one. So an update for v6 can be made with v5's image; none is made today.
- The v5 updater's pinned walk from its accepted tip 23105 to 23409 (the v6 init, the refused attester call,
  the claim): 13,169,562 user cycles, 92 segments, journal equal to the native one; no v5 claim counted.

**The deposit page entry v6 would need** (not added: v5 stays the default; for `web/deposit/src/lib/deployments.js`):

```js
const SEPOLIA_V6 = {
    id: 'sepolia-v6',
    label: 'Sepolia vault v6',
    chainId: 11155111,
    vault: '0x95cea20e75d3b352b49d13cb0cddc268a14c427e',
    deployBlock: 12158041,
    cfgId: '0x7d1a304a12a486c730d94321244bd6d15741208c1ef9d00e352acda8589e6f60',
    desk: '0x8Aab670f14A53A41f20a6aA75137b13C0234A1c3',
    demo: true,
    demoAccount: null,
    demoNote: 'How: no attester key: the bridge test network verifies each deposit root with an Ethereum light client that follows Sepolia\u2019s sync committees. No withdrawal service runs for this vault yet. Test coins only.',
    demoShort: 'Deposit roots are verified by a light client, not an attester key.',
    tokens: SEPOLIA_DEMO_V2.tokens,
};
// and add SEPOLIA_V6 to ALL_DEPLOYMENTS; LIVE_DEFAULT stays 'sepolia-v5'
```

The module's `deployments.json` would need the bridge `4PxaLAfP…QWed`, token program `5w6nTkft…ArbU7`, the
definitions above, and no relay (the claim relay serves v5).

**Re-read over RPC (any machine, read only):**

```sh
E=https://lez.nameswallet.app/
rpc() { curl -s -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" $E; }
# [base64 transaction, block]: the claim (block 23408, 7,099 bytes), InitializeAttested (23343), the refused
# attester call (23349), the period Finish (23263), the finality update past the deposit (23406)
for h in 0d8ea4315cc86d667c99a9e66d2848df36b2d51705942d6d3161c9ab8757d5b1 b0b33f3aa6d30c0f0b2143fb948d2f4590545e3a05e34aa407e879e9770f3b6a \
         09f70f7007158ebf8469ff7b51f1e755d61708a3252376fc8c9a52649999d49b 47b265596e7f16673b29f1f5dc076dd9fb20a64ee49583665d33b1204f7152d4 \
         839a3d2f01a7dd5a80fc36ae1a630421cb88655fddcbc025861f1849116c7d39; do
  rpc getTransaction "[\"$h\"]" | python3 -c 'import sys,json,base64;t,b=json.load(sys.stdin)["result"];print(sys.argv[1][:8],"block",b,"bytes",len(base64.b64decode(t)))' $h
done
# Sepolia side
cast call 0x95cea20e75d3B352B49d13cb0cDdC268a14C427e 'zoneId()(bytes32)' --rpc-url https://sepolia.gateway.tenderly.co
cast call 0x95cea20e75d3B352B49d13cb0cDdC268a14C427e 'depositCount()(uint64)' --rpc-url https://sepolia.gateway.tenderly.co
cast call 0x95cea20e75d3B352B49d13cb0cDdC268a14C427e 'isDepositRoot(bytes32)(bool)' 0xd40574394fb20b6986b1e2c77ffb2a1ae659e642b769bf2c700787748c8ce3f6 --rpc-url https://sepolia.gateway.tenderly.co
cast block 12158083 -f hash --rpc-url https://sepolia.gateway.tenderly.co   # 0x35997cec…2503, the claim's anchor
```

Run 2026-11-18 from a workstation: the five transactions came back in blocks 23408, 23343, 23349, 23263 and
23406 (claim 7,099 bytes).

## Rehearsal (local LEZ chain + anvil; `rehearsal.log`)

`public-demo/v6/rehearse-v6.sh`, stages `chains lc lc-finality bridge eth claim g2`:
- **Light client, real Sepolia data:** the same image and CLI through `lc-handoff.sh`: `Init`, the period
  update (Begin 3,193,829 and Finish 9,416,729 gas), a finality update after it (9,456,075, signed by the
  current committee with the next one known), and an attempt with nothing new that sent nothing.
- **A second bridge header initialised against that real instance** (`InitializeAttested`; its
  `AssertInstance` answered by the new program); an instance id that does not exist is refused.
- **Ethereum on anvil** (chain id 11155111; a mock stands in ONLY for RISC Zero's Groth16 check):
  `SepoliaV4.s.sol` through its harness (deployer = anvil #0) with the light-client `zoneId`, the vault's
  `g2State`, `zoneId` and `cfgId` read back and equal to the CLI's; `configure()`; the desk through
  `SepoliaDeskEnvHarness`; a 0.01 ETH deposit through the desk with the real `SepoliaDeskDeposit.s.sol`.
- **The claim:** a light client cannot follow anvil, so the anchor past the deposit comes from the TEST-ONLY
  image `attest_guest_synthetic` (the same program plus `InitSynthetic`, `public-demo/v6/synth-init.py`),
  whose first store anchors the anvil block after the deposit. The v6-style bridge, bound to it, took a
  **public claim** (`ClaimAttested`, `--anchor-state-root`). Refused: the **replay** (locally, and on chain
  when forced: reverted, `claim refused`), a deposit under a root the vault never had (locally: the
  predicate does not hold; on chain when forced: reverted), and an anchor no instance recorded (reverted).
- **Outbound:** v5's update image (executor) from the bridge's genesis over the claim: journal equal to the
  native one, accepted through the gate from the submitter only; the vault's counters read
  1 deposit, 1 claim. **The walker** (the updater's pinned host, guest ELF and image, po2 18) from the
  chain's start over every block (the light client's deploy, Init, period Begin and Finish, finality, the
  bridges, the claim and the reverted claims): 15,980,525 user cycles, 125 segments, journal equal to the
  native one. On the public chain itself, read only, the same pinned host and guest walked from the v5 vault's accepted
state (tip 22865) to block 23297, which holds this light client's deploy, `Init` and period update and the
v6 bridge's deploy: 15,334,538 user cycles, 104 segments, journal equal to the native one.
- The log keeps three failures of the script itself (a missing exec bit, a variable clash, a wrong-root test
  that the CLI's own leaf check stopped first); each was fixed and the stage run again.

**Sepolia hand-off, on an anvil fork of Sepolia** (`public-demo/v6/handoff-anvil-test.sh`): the v6 values
through the same harnesses against the real verifier 3.1.0 (selector `0xe71e6620` read on the fork): gate,
vault (`zoneId`, `lezChainId`, `g2State` read back; `cfgId` equal to the CLI's), `configure()`, the desk at
30 bps, and one deposit with the unchanged deposit script.

## Limits

- **Trust:** the light client's statement holds as far as the sequencer applied the transactions
  honestly; nobody re-executes them (no proof). Every check runs in public, deterministic execution
  anyone with the ELF can replay.
- **Participation:** one finality update fits LEZ's 10M gas cap down to about 442 of 512 signers, and a
  period Finish needs near 450; below that the wallet's simulation fails and nothing is sent.
  The follower does not use the pairing-seam split (`finality-split-20261117/`).
- **Weak subjectivity:** if the follower stops for more than about 6 days, the instance cannot be updated
  again; a new instance and a new bridge would be needed (the bridge's source is fixed at init).
- **Liveness:** the follower is one process started by hand (no restart after a reboot).
- **No updater serves v6, so v6 has no exit running today.** The outbound updater stays on v5 (one update
  about every two hours). A v6 burn would wait for a v6 update; the executor run above shows v5's image can
  make one.
- **What is still trusted:** the team runs the only sequencer, and the chain operator is the v6 bridge's LEZ
  admin and freezer (as for v5). No attester key is involved in v6 claims.
- **Not yet shown on the public chain:** a committee rotation (due after 19:32:48 UTC today; see Part 1).
- Reviewed by nobody beyond this run; not audited.
