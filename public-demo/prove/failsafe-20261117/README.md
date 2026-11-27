# Failsafe (RFP-022 Security #9): demo build and local run, 2026-11-17

**State:** (1) run on local chains only: a throwaway LEZ v0.4.0-rc3 sequencer and anvil, both deleted
after the run. **mock-verified: proofs executed, not proved; LEZ finality trusted for the test.** Nothing
here is deployed on Sepolia or on the public demo chain. Design: `research/failsafe-design.md`.

**The design could not be built as written within this task's rules** (no change to `BridgeVault.sol`,
`UpdateGatedVerifier.sol`, any existing guest or any pinned lock). Its LEZ half needs a new bridge
program version with an irreversible wind-down flag (§2.5), and G2 must report it and commit the
accumulator in its journal (§2.6 R5, `JournalVersion::V2`). Both mean new pinned images. The
obstacles, in code:

- `lez-bridge/bridge/src/lib.rs:1226`: `AdminOp::Unfreeze` clears any mint freeze, so on today's
  program "frozen for good" is not enforceable on LEZ.
- `lez-bridge/outbound/core/src/view.rs:30-33`: G2 does not track freezes, so no G2 state can tell
  the vault that LEZ minting stopped.
- `lez-bridge/outbound/core/src/update.rs:269`: G2 commits `JournalVersion::V1`, with no
  accumulator root in the journal. `update.rs:252` and `state.rs:32`: `wound_down` is always empty.

What is built is the largest sound part. It covers the Ethereum half completely: the deposit stop,
the final accumulator, and the reclaim with its proof. The accumulator reaches the vault inside
`g2State`, through the existing update path. The LEZ half is the existing, reversible mint freeze,
and the limits below say what that leaves open.

## What it does

The case the design says CAN be covered: deposits not yet claimed on LEZ (position P1) when inbound
minting (the LEZ claim check, the attestation) or the release proof (G3) is found broken.

1. **Wind-down.** On LEZ the freezer freezes minting (`lez-bridge freeze --mint`). Once the vault's
   `g2State` covers that LEZ block, the vault admin calls `windDown()`: no deposit is accepted again,
   ever (`IsWoundDown`; no function clears `windDownAt`). The **first G2 update after `windDown`**
   fixes `finalState` (its `newState`). That G2 state commits to the claim-nullifier accumulator,
   so the final list of LEZ claims reaches the vault through the existing update path; no G2 guest
   was changed. Later updates still bring exits in, but their claim counts are ignored. Between
   `windDown` and the final, claims above deposits are clamped; after `windDown` an exit above the
   claimed value is dropped. Either would otherwise stop updates for good.
2. **Reclaim.** A depositor proves with the new guest G4 (`reclaim_guest`) that they hold the master
   secret `s` of a deposit in one of the vault's deposit roots, and that the deposit's claim
   nullifier is NOT in `finalState`'s accumulator. The proof names the Ethereum recipient, relayer
   and relay fee. Anyone submits `reclaim(...)`. The vault pays exactly that deposit's denomination
   (minus the relay fee to the relayer) to exactly that recipient. It pays once per deposit (reclaim
   nullifier `SHA256("RFP022/nfrc" | s)`) and only while `claims + reclaims <= deposits` for that
   token and denomination.
3. **No admin payout.** The new admin functions are `windDown` (moves nothing, names no one) and
   `unfreezeReclaims`. The freezer gets `freezeReclaims`, which is liveness only. Nothing lets the
   admin or the freezer name a payee or move principal. A test checks the ABI against a fixed list
   of state-changing functions.

## What changed (nothing deployed or pinned changed)

| Part | File | Note |
|---|---|---|
| Vault variant | `vault/src/failsafe/BridgeVaultFailsafe.sol` | A copy of `BridgeVault.sol` with additions marked `FAILSAFE`. Runtime **23,133 bytes**, 1,443 under EIP-170 (`forge build --sizes`, Foundry 1.5.1). Storage is appended at slots 92–96; `isDepositRoot` stays at 83. `BridgeVault.sol` is byte-identical to main (sha256 `42773a68…f82c`), runtime 20,642 bytes. |
| G4 statement | `lez-bridge/reclaim/core` | Reuses `bridge_core` (commitment, leaf, claim nullifier, nullifier PDA), `claim_acc` and `outbound_core::state::G2State`. None of them is changed. |
| G4 guest | `lez-bridge/methods/reclaim_guest` | A new guest with its own lock (risc0-zkvm 3.1.0, role `eth_guests`). Its image id depends on the build directory (a guest build embeds it): the build host's build, which the run used, had `0xb7a81fd5…25fd`. No deployment or document pins it. |
| G4 host | `lez-bridge/reclaim/host` | Executor only. `ids`, `vector --write`, `reclaim --master-file …`. It has its own workspace and lock (role `eth_provers`). |
| CLI | `lez-bridge/cli`: `deposit-secret` | Writes a seed-derived deposit's master secret `s` to a new mode-600 file. It prints only `C` and the reclaim nullifier. No new dependency; `Cargo.lock` unchanged. |
| Deploy, e2e | `vault/script/FailsafeDeploy.s.sol`, `lez-bridge/scripts/e2e-failsafe.sh` | Local only. |
| Versions | `VERSIONS.toml` | Two new `[[workspace]]` entries. `node scripts/check-versions.mjs`: 0 FAIL. No deployment, image pin or existing lock changed. |

Not touched: `BridgeVault.sol`, `UpdateGatedVerifier.sol`, every existing guest and every existing
`Cargo.lock`.

## Non-membership structure

The accumulator is the one G2 already keeps (`lez-bridge/outbound/claim-acc`): an **indexed Merkle
tree**, depth 32, SHA-256 with the vault's zero hashes. Each leaf is
`SHA256("RFP022/acc-leaf" | key | nextKey | u64be(nextIndex))`, a link of a sorted list, and
index 0 is the sentinel. The key is the claim-nullifier PDA id
`for_public_pda(programId, nullifier_seed(SHA256("RFP022/nfc" | k | dest | token | u256(denom))))`.
A public or private claim writes that key, and G2 inserts it.

Non-membership of `x` is shown by a low leaf `L` with `L.key < x < L.nextKey` (or `nextKey = 0`)
plus its 32-sibling path to the root. The root is `G2State.claim_acc.root`. The guest receives the
`G2State` **preimage** and commits `SHA256("RFP022/g2-state/v1" | borsh(state))`, which the vault
compares with its `finalState`.

The guest's journal (288 bytes) is
`abi.encode(cfgId, depositRoot, finalState, nf_rc, token, denom, recipient, relayer, relayFee)`.
The same `s`, `dest`, `token`, `denom` and `programId` build `C`, the deposit leaf, `cfgId` and the
accumulator key. A proof that the deposit is unclaimed is therefore about the very deposit whose
membership it shows.

## Tests

- **Rust** (`lez-bridge/reclaim/host/tests/reclaim.rs`, executor, build host): 8 tests passed.
  - An unclaimed deposit reclaims, and the guest's journal equals the native one.
  - For a claimed deposit, no witness exists and none forged from any leaf verifies; the guest
    panics, so there is no journal.
  - A proof against an older accumulator names another final state.
  - A wrong secret, another account, token or denomination, or a deposit not in the tree gives a
    root the vault never had.
  - The final state must be the cfg program's.
  - Payee, relayer and fee are bound into the digest, with one nullifier per deposit.
  - The formulas equal the client KDF's.
  - The fixture is current.
- **Foundry** (`vault/test/failsafe/`, 24 tests): the full vault suite passed under
  `FOUNDRY_PROFILE=ci` with Foundry 1.5.1, CI's version: 29 suites, 223 passed, 0 failed, 1 skipped
  (the pre-existing Sepolia fork test, which needs an RPC). `forge build --sizes` exited 0.
  - `Failsafe.t.sol` (15 tests): a reclaim pays once, and a second one gives `NullifierSpent`.
    - A reclaim before wind-down gives `NotWoundDown`; before the final, `NotFinal`.
    - Deposits are refused for good, even after `unfreezeAll`; a second `windDown` gives
      `AlreadyWoundDown`; only the admin may call it.
    - With deposits minus claims used up, a forged reclaim gives `ReclaimsExceedDeposits`.
    - A relayer changing the payee, fee, relayer, denomination or nullifier gets `BadSeal`, or the
      menu refusal first.
    - The relay checks hold, and an unknown root or another final state is refused.
    - Claims after the final are ignored.
    - An over-mint reverts while active and is clamped after wind-down; an unbacked exit is
      dropped after it.
    - The final lands under the global release freeze; the reclaim freeze is liveness only.
    - ETH can be reclaimed, and release caps apply.
  - `FailsafeAdminTest` (2): the ABI has no unlisted state-changing function. Every admin and
    freezer function called after the final leaves principal unchanged and the wind-down and final
    in place, and both depositors still reclaim afterwards.
  - `FailsafeInvariant.t.sol`
    - The handler drives deposits, updates with forged claims and exits, forged releases,
      wind-down, forged reclaims, freezes, caps, fee withdrawals and warps.
    - Invariants:
      - claims + reclaims ≤ deposits and releases ≤ burns, per (token, denomination);
      - burned ≤ claimed, per token;
      - released + reclaimed ≤ deposited, by value;
      - the balance equals deposited − released − reclaimed + accrued fees, exactly;
      - the admin and freezer never get principal;
      - the wind-down and the first final never change.
    - `testFuzz_SolvencyWithReclaims`: reclaims stop exactly at deposits − claims.
    - Reachability was checked by making a successful handler reclaim revert on purpose: every
      invariant then failed, so the campaign does reach paid reclaims.
    - The first CI-profile run caught a test bug: the invariant computed principal per
      denomination, but releases are fungible by value across denominations. It now compares by
      value, as `BridgeVault`'s own bound does.
  - `ReclaimVector.t.sol` (3): replays the guest's executor journals
    (`vault/test/fixtures/ReclaimVector.sol`) on the variant at a fixed address. The vault's digest
    equals the guest's journal digest byte for byte; replay and redirect are refused, as is a
    reclaim before the final or against another final.

## The run

`lez-bridge/scripts/e2e-failsafe.sh` on the build host: a fresh rc3 sequencer on 127.0.0.1:3500/3501
and anvil on 127.0.0.1:8600, both deleted at the end. The bridge and token programs were deployed from
the canonical ELFs (bridge `8914861a…`, token `7704844f…`). Updates ran through the canonical outbound
host (`7bb6fdec…`, TrustedTip image `0x098f5a1a…3017`); the vault variant used the mock verifier.
Users: u1 has a LEZ wallet of its own and its recovery phrase; u2 and u3 are accounts the operator
funds for fees. Anvil account #1 pays the deposits; #2 is the relayer.

Recorded run: commit `84cdaeb`, 2026-11-17 18:09–18:21 EST. About 9 of those minutes were spent
waiting for the build lock's memory gate. Run 4, from the same script before its last review edits,
passed with the same steps and numbers. After the run, the script's anvil keys were written out as literals (the same three
public test keys, with an address check), because the export guard refuses a word-list phrase.

| Step | Result |
|---|---|
| Deposits | u1, u2 and u3 each deposit 100 USDC (+1 USDC deposit fee) through `deposit-request`: seed-derived commitments at key index 0. Vault: **303.000000 USDC**. |
| u3 claims on LEZ | `claim --key-index 0`, block 44. G2 update 1 (blocks 2..44, 8 segments) is submitted, and the vault counts deposits 3, claims 1. |
| Wind-down 1/3, LEZ | Global mint freeze, block 45. G2 update 2 (block 45) is submitted, so the vault's `g2State` covers the freeze. |
| Wind-down 2/3, Ethereum | `windDown()` (gas 72,041). A further deposit is refused (`IsWoundDown`), and so is a reclaim (`NotFinal`). u2's LEZ claim is refused: "minting is frozen". |
| Wind-down 3/3, final | G2 update 3 (blocks 46..47), the first update after `windDown`, is submitted (gas 77,625). `finalState = 0x99989726…a25268`; its accumulator has root `0x9040046d…74dc` and count 1 (u3's claim). |
| u1 from the phrase alone | u1's wallet directory, with its keys and the bridge client cache, is deleted. `wallet restore-keys --depth 4` restores from the phrase. It prunes accounts with no LEZ state and u1 never touched LEZ, so one `wallet account new public` re-derives the phrase's next public account: u1's. `recover` lists the deposit (CLAIMABLE: the CLI does not read the vault's wind-down). `deposit-secret` writes `s` to a mode-600 file, and its commitment equals the one deposited. |
| G4, executor | u1 to fresh1 through the relayer (fee 1 USDC): **96,831 user cycles**, 262,144 total, 1 segment. u2 to fresh2 (no fee): **96,870** user cycles, 1 segment. The guest's `FINAL_STATE` equals the vault's. |
| Redirect | The relayer submits u2's proof with itself as recipient: `BadSeal`. |
| Reclaims | The relayer (anvil #2, not the depositor and not the payer #1) submits both as made. Gas: **160,263** with a relay fee (two transfers), **114,997** without. |
| Replay | u1's reclaim again: `NullifierSpent`. |
| u3 (claimed) | `reclaim_host` finds u3's claim-nullifier key in the final accumulator: "REFUSED: claimed", exit 3. No proof exists to submit. |
| Balances | Vault **303.000000 → 103.000000** USDC. fresh1 0 → **99.000000**, fresh2 0 → **100.000000**, relayer 0 → **1.000000**. Counters: deposits 3, claims 1, reclaims 2. The vault holds exactly u3's claimed 100 USDC, backing its wrapped tokens, plus 3 USDC of deposit fees. |

The chain and anvil were stopped and the run directory (phrase file, master-secret files, wallets)
was deleted after the numbers were copied here. The transcript is not committed: it prints full
deploy transactions.

## What is NOT covered (and why)

- **A bug in the state update G2, in the proof system, or in vault accounting.**
  - A reclaim reads G2's accumulator, so a broken G2 can forge "unclaimed". G2 is the only source
    on Ethereum of what LEZ claimed and burned.
  - On LEZ 0.4 there is no second, independent way to know it. A second G2 needs a second light
    client and parser in another zkVM. LEZ's public state has no root and its public execution is
    never proven.
  - A broken RISC Zero verifier or zkVM breaks G2, G3, G4 and LEZ's own private claims together.
  - A vault accounting bug is shared by the reclaim path, which uses the same counters and `_pay`.
  - In these cases the remedy is freeze and lock, with no compliant way to pay anyone (design, top
    table). The vault counters still bound any loss by deposits.
- **Wrapped tokens already minted and held on LEZ (P2).** Their holders are not depositors. A
  reclaim proves a deposit was NOT claimed, so it can say nothing about a claimed one. Proving a
  LEZ balance is impossible on LEZ 0.4: public balances sit in state that has no root, and
  private notes' spend status sits in a nullifier set that has no root. Holders burn and release
  as usual while outbound is trusted; otherwise their funds stay locked in the vault.
- **Burns in flight (P3: burned but not yet in an exit root; P4: in the exit tree, not released).**
  They are not deposits. They exit through G2 and G3 if those are sound, and are locked if not.
  After `windDown`, an exit above the claimed value is dropped, not inserted.
- **Deposits made on the web page with a random secret that was lost.** Only the master secret `s`
  can make a reclaim. A deposit without `s` (a raw `k`, the web page's random secret) cannot be
  reclaimed. A public claim publishes `k`, so letting `k` reclaim would let anyone redirect a claim
  that lost the race to the wind-down. A lost secret is a lost deposit, as for a claim.

## Known limits of this demo (written down, not fixed)

- **LEZ minting is NOT frozen for good on LEZ 0.4 here, and `windDown()` fixes `finalState` without checking LEZ minting has stopped.** The wind-down uses the existing global
  mint freeze, and the LEZ admin can undo it (`lez-bridge/bridge/src/lib.rs:1226`). G2 does not track
  freezes (`lez-bridge/outbound/core/src/view.rs:30-33`), so the vault cannot check one.
  - If LEZ minting is not frozen before `finalState` is fixed, or if a LEZ unfreeze occurs after the
    final, a depositor can claim on LEZ, burn, and have the late exit paid while **also** reclaiming
    the same deposit on Ethereum (later updates still insert exits while `burnedValue <= claimedValue`,
    so the late exit drains backing from other holders' exits, which are then dropped as `ExitUnbacked`).
  - The vault stays solvent (`released + reclaimed <= deposited`): claims count nowhere after the
    final, and `burnedValue <= claimedValue`.
  - **This variant is not deployed.** The design's fix (M6) needs a new bridge program version and a
    new G2 image: an irreversible LEZ mint freeze set by a proof of the vault's `windDownAt` (§2.5),
    and G2 fixing the final from the LEZ event (§2.6 R5), which closes this limit.
- **The final's timing is the operator's.** The first update after `windDown` must reach past the
  LEZ freeze block, and the vault cannot check that. Runbook:
  1. Freeze minting on LEZ.
  2. Wait until the vault's `g2State` covers that block.
  3. `windDown`.
  4. The next update is the final.

  On a gated vault (v4/v5), the one submitter controls this ordering.
- **A reclaimer needs the final accumulator's leaves** (here `next-state.bin` from the outbound host).
  The vault does not publish them, so whoever runs G2 must. Withholding them blocks reclaims, which
  is a liveness failure, not theft.
- **Global wind-down only.** There are no per-token keys and no reclaim delay after the final
  (design §2.2). Reclaims can be paused by the freezer.
- **An over-mint before the final uses up reclaim room.** Forged claims are clamped to deposits, so
  the last reclaimers of that (token, denomination) get `ReclaimsExceedDeposits`. The vault stays
  solvent.
- **Reclaim is inline in a variant vault**, not the design's separate module. That fits (1,443
  bytes left) but leaves little room for the contest window.
- **Executor only.** G4 was never proved or Groth16-wrapped, and its image is pinned nowhere.
  `reclaim_host` reads `outbound_host`'s saved state with its own copy of the struct. The run
  checks the guest's `finalState` equals the vault's, so a layout drift would fail there.
- **A dropped exit makes the vault's `exitCount` differ from G2's `exit_count`.** G2 still counts
  an unbacked exit after `windDown`; the vault drops it. Exit proofs read the vault's own leaves, so
  nothing breaks, but the two counts no longer match.
- **`restore-keys` prunes an account that never touched LEZ.** A depositor whose account only ever
  deposited must re-derive it (`wallet account new public`, the same key tree) before
  `deposit-secret`, as the run did.
- **Privacy.** A reclaim publishes the deposit root, the final state, the reclaim nullifier, token,
  denomination, recipient, relayer and fee, but not which deposit. The anonymity set is the
  unclaimed deposits of that denomination under the root. Clients should use the newest root, a
  fresh recipient and a relayer.

## Next steps

1. A bridge program version with the irreversible wind-down flag and `WindDownAttested`.
2. G2 tracking it, and the vault fixing the final from the event instead of from timing.
3. A reclaim delay.
4. A Groth16 wrap of G4, then a reclaim on a real verifier.
5. Per-token wind-down.
6. An audit.

## Commands

```sh
# anywhere with Foundry (CI uses v1.5.1)
cd vault && FOUNDRY_PROFILE=ci forge build --sizes && FOUNDRY_PROFILE=ci forge test --match-path 'test/failsafe/*'
# build host: G4 tests and fixture (executor only)
cd lez-bridge/reclaim && cargo test --release -j 4
cargo run --release -p reclaim_host -- vector --write <dir>     # copy ReclaimVector.sol to vault/test/fixtures/
# build host: the run (env in the script's header)
R=<fresh dir> CLI=… RECLAIM_HOST=… OUTBOUND_HOST=… GUESTS=… SEQ_BIN=… NETCFG=… CHAIN_SH=… lez-bridge/scripts/e2e-failsafe.sh
```
