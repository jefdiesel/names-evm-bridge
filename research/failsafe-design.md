# RFP-022 Security #9 failsafe: design

- **Date:** 2026-11-13
- **Status:** design, not implemented.
- **All byte and cycle figures in this file are estimates. None was measured.**

Sources read: the spec (S8, S9, Trust model, Loss of access, F14, R4), `DESIGN.md` (§0, §3.4, §5, §6.3, §10, §12), `vault/src/BridgeVault.sol` (all of it), `lez-bridge/bridge/src/lib.rs`, `lez-bridge/release/core/src/lib.rs` and the release guest, and `research/lez-and-l1-provability.md`.

## Decisions taken (2026-11-13)

1. **Reclaim lives in a separate immutable module** (`BridgeReclaim`), created in the vault's constructor. The vault's counters are the hard bound on what it can pay. This choice is to be re-checked against measured bytecode.
2. **Over-mint handling as specified in §2.3.** Claim deltas are clamped and unbacked exits dropped only for a token whose wind-down was requested. An ACTIVE token still reverts.
3. **Accumulator key = the claim-nullifier PDA account id**, in the indexed Merkle tree of §2.6.
   - The outbound-guest (G2) worker has been given §2.6 as its interface.
   - The recovery worker has been given the secret convention `s → k = SHA256("RFP022/ck" ‖ s)` (§2.1).

## What happens without a failsafe (the proposal must state this)

| Case | What happens to funds and in-flight positions | Why no failsafe |
|---|---|---|
| **(b)** G2 image | Releases are frozen (global, which also stops `update`) and LEZ burns are frozen. The vault's funds stay locked indefinitely: no admin path exists (S8). The admin may unfreeze only if analysis shows the bug was never triggered. P1 can still claim on LEZ, but those tokens can't exit | G2 is the only source on Ethereum of LEZ claims and burns. A second G2 needs a second light client and parser in another zkVM (~30B cycles a day). An optimistic final needs a challenger who either proves with the same G2 or is trusted |
| **(c)** G3 image, positions P2–P4 | Release freeze; locked | An alternative release path the admin can switch on is an S8 swap and a second drain path. A duplicate of the 36K-cycle G3 shares its failure modes |
| **(d)** RISC Zero verifier / zkVM | Freeze everything, reclaims included; locked. Losses before the freeze are bounded by the counters, by caps (≤~2× a cap per window) and by the delay on new roots | G4, G2 and LEZ's private-claim circuit share one prover. A 2-of-2 on reclaims doesn't help, because the accumulator they check against is proven by RISC Zero |
| **(e)** Vault accounting | Freeze; locked. Mitigation is before deployment: invariants and audit | The reclaim path shares the vault's code |
| **Wrapped tokens (P2) whenever outbound is broken** | Locked with the vault's funds | No proof of LEZ holdings is possible on 0.4 (§2.10) |

## 0. Verdict

- **Inbound wind-down plus reclaim.** This one mechanism meets all three S9 constraints for deposits not yet claimed (position P1), when the bug is in inbound verification (case a) or in the release proof G3 (case c).
- **Cases with no compliant mechanism on LEZ 0.4.** For a bug in G2 (b), in the proof system (d) or in vault accounting (e), and for wrapped positions P2–P4 under (c), nothing meets the constraints. The spec allows that answer if the absence is documented; the table above does so.
- **The counters already cap payouts.** The vault's counters keep total principal paid ≤ total deposited, per token, whatever any proof says (§2.8). So the failsafe only decides *who* is paid, never *how much*.
- **Why DESIGN §10's "freeze both halves" has to change.**
  - A freeze is reversible, so the claim set it cuts off is never final.
  - `update` is blocked by `globalReleasesFrozen`, so the final batch could not land during an incident.
  - Over-minting makes `update` revert for good (`ClaimsExceedDeposits` / `BurnsExceedClaims`). Then the final batch can never land.
  - All three are fixed below.

## 1. Threat cases × in-flight positions

The four positions:
- **P1:** deposited, not claimed.
- **P2:** claimed; wrapped tokens held on LEZ.
- **P3:** burned, not yet in the exit tree.
- **P4:** in the exit tree, not released.

| Case | What an attacker gets, and the bound | P1 | P2 | P3 | P4 |
|---|---|---|---|---|---|
| **(a)** Inbound: LEZ claim check, `eth_attest` inclusion, attestation | Forged mints. Through G2 they count against `claims ≤ deposits` per (token, denom), so F forged claims use up P1's room. Above deposits, `update` reverts forever today. There is no LEZ cap | **Reclaim**, minus F. The last F reclaimers of that (t, d) bear the loss, because forged claims can't be told apart | Normal burn/release. Forged supply competes for exits. On an over-mint, the last exits are dropped (§2.3) | Normal | Normal |
| **(b)** G2 image | Forged exits up to `claimedValue − burnedValue`. Forged claim counts can raise `claimedValue` up to deposits, so the whole vault is reachable after the delay. Bounded by caps, delay and the contest window (planned) | **Not recoverable**: reclaim needs G2's accumulator. Claiming on LEZ still works, but the tokens can't exit | Locked | Locked | Locked; forged exits look like real ones |
| **(c)** G3 image | Forged releases up to `burns − releases` per (t, d), at once against old roots, so the delay doesn't help. Caps bound it | **Reclaim**: G2 and G4 are intact | Locked | Locked | At risk, then locked |
| **(d)** RISC Zero verifier / zkVM | Everything, including G4 and LEZ's private-claim circuit. Bounded by the counters, by caps × time until the freeze, and by the delay on new roots | Not recoverable | Not | Not | Not |
| **(e)** Vault accounting | Depends on the bug. Reclaim shares `_charge`/`_pay` and the counters | Not reliably recoverable | Not | Not | Not |

## 2. Mechanism

### 2.1 Payout address and the reclaim credential

- **(i) Store the depositor per leaf.** Rejected. It costs a slot per deposit (≈98k gas under Glamsterdam) and ties the payout to a public address.
- **(ii) Prover names the recipient, with `k` as the only secret.** Unsafe, so rejected in that form.
  - A public claim publishes `k` and `dest` (DESIGN §3.4).
  - One that loses the race to the wind-down is included and reverted, and its nullifier stays unspent.
  - Anyone could then reclaim that deposit to their own address. That breaks S3 on the failsafe path.
- **(iii) Bind a refund address into `C`.** Works, but it fixes the payout at deposit time and changes `C`.
- **Chosen: (ii), with a secret no claim ever reveals.** This is a client convention only. Nothing changes on chain, and the KDF isn't built yet.
  - `s = KDF(seed, cfgId, "dep", i)`. This is the deposit master secret; it never appears in any transaction.
  - `k = SHA256("RFP022/ck" ‖ s)`. It is used everywhere `k` is used today: `C`, `nf_c`, public claims.
  - A reclaim needs `s` and names its Ethereum recipient in the journal.
- **Why this counts as "the address the proof specifies":** only the holder of `s` can make the proof, and the recipient, relayer and fee are bound in the digest, as in G3.
- **Recovery:** `s` comes from the seed alone, so F9 still holds.
- **Old deposits:** those made with a raw `k` (the Sepolia demo only) cannot be reclaimed.

### 2.2 State machine, per key (a token, or `GLOBAL = address(type(uint160).max)`, as for freeze)

```
ACTIVE --vault admin windDown(key)--> REQUESTED      deposits of key refused forever; LEZ claims continue;
                                                    update runs even under the global release freeze if key = GLOBAL
REQUESTED --anyone: LEZ WindDownAttested(storage proof windDownAt[key] != 0, RFP-021)--> LEZ wound down
                                                    (irreversible Config flag; claims refused)
--any G2 prover: the batch holding the WoundDown event lists key--> FINAL
                                                    (vault stores finalAt and claimAccRoot; claim deltas for key ignored)
FINAL + delay ("lowest delay since finalAt", the exit-root rule) --> OPEN: reclaims, with no deadline
```

**What can and cannot move the state:**
- No transition goes backwards.
- Only `windDown` writes `windDownAt`. Only a G2 final whose key was already requested writes `finals`.
- The LEZ admin has **no** wind-down power. The LEZ flag can only follow the vault.

**Trust at each step:**
- **Vault → LEZ** is trustless (RFP-021). A forged wind-down proof could only stop minting.
- **LEZ refusing later claims, and G2's cut,** rest on the committee, like every other G2 fact (DESIGN §0).
- **Solvency relies on neither.**

**Why the vault admin is the trigger:**
- Wind-down moves no funds.
- Its worst misuse, closing a token, is weaker than the existing power to freeze forever, and it leaves every user a way out.
- No timelock: it would only delay the remedy.
- Not the freezer, because the step is irreversible.

**Runbook:**
1. Freeze both halves (fast).
2. `windDown`.
3. `WindDownAttested`, after Ethereum finality and an attestation update past it.
4. The next G2 batch makes it final.
5. Wait the delay.
6. Depositors reclaim; holders burn and release if outbound is trusted.

### 2.3 Vault changes (appended after slot 91; `isDepositRoot` stays at 83)

```solidity
mapping(address => uint256) public windDownAt;                      // slot 92: key => request time (the LEZ-proved slot)
struct Final { uint64 at; bytes32 accRoot; }
mapping(address => Final) public finals;                             // 93
mapping(address => mapping(uint256 => uint64)) public reclaims;      // 94: [token][denomIndex]
uint64 public claimAccCount; bool public reclaimsFrozen;             // 95 (packed)
BridgeReclaim public immutable reclaimModule;                        // created with `new` in the constructor

function windDown(address key) external onlyAdmin;                   // key registered or GLOBAL; second call is a no-op
function freezeReclaims() external onlyFreezer;
function unfreezeReclaims() external onlyAdmin;
function payReclaim(address token, uint32 denomIndex, address recipient, address relayer, uint256 relayFee)
    external nonReentrant;                                           // msg.sender == reclaimModule only
event WindDownRequested(address indexed key); event WindDownFinal(address indexed key, bytes32 accRoot);
event ClaimsClamped(address indexed token, uint32 denomIndex, uint64 excess);
event ExitUnbacked(address indexed token, uint32 denomIndex, bytes32 commitment);
event Reclaimed(address indexed token, uint32 denomIndex, address recipient, address relayer, uint256 relayFee);
error WoundDown(); error NotWoundDown(); error ReclaimTooEarly(); error ReclaimsExceedDeposits();
error BadAccumulatorCount(); error NotReclaimModule();
```

**Deposits.** `_depositTerms` refuses with `WoundDown` if `windDownAt[token]` or `windDownAt[GLOBAL]` is set.

**`update` changes:**
1. `if (globalReleasesFrozen && windDownAt[GLOBAL] == 0) revert IsFrozen()`.
2. `j.claimAccCount == claimAccCount + Σ j.claims[].count`, checked before any clamping, else `BadAccumulatorCount`. This catches a G2 that counts a claim without inserting its key, which is the direction that would allow a double spend.
3. **Claim deltas:**
   - for a token that is FINAL, through its own key or `GLOBAL`: ignored;
   - for a token that is REQUESTED: clamped to deposits, with `ClaimsClamped`, instead of the permanent revert;
   - for an ACTIVE token: unchanged (still reverts).
4. **Exits:** for a REQUESTED token, an exit that would break `burnedValue ≤ claimedValue` is not inserted and not counted, and `ExitUnbacked` is emitted. On an over-mint, the last burners lose. The alternative is a stalled G2, which strands every later exit and every reclaim. An ACTIVE token still reverts.
5. **Final:** for each `key` in `j.woundDown`, if `windDownAt[key] != 0` and `finals[key].at == 0`, set `finals[key] = Final(now, j.claimAccRoot)`.

A batch stuck on an over-mint while the token was ACTIVE can be resubmitted unchanged after `windDown`: it then clamps and proceeds.

**`payReclaim` order:**
1. Check the sender.
2. Recipient is neither the vault nor `address(0)`. Relay checks are shared with `release` through a factored `_checkRelay`: fee 0, or a menu value with a sane relayer.
3. `!reclaimsFrozen`.
4. `at` = the earliest non-zero of `finals[token]` and `finals[GLOBAL]`. If both are zero, `NotWoundDown`. If the delay since `at` has not passed, `ReclaimTooEarly`; this reuses `release`'s delay logic through a factored `_waitOver`.
5. `++reclaims[t][d]`, then require `counters.claims + reclaims ≤ deposits`, else `ReclaimsExceedDeposits`.
6. Charge the release caps.
7. Pay `denom − relayFee` to the recipient and `relayFee` to the relayer.
- No withdraw fee is taken, and the deposit fee is kept.

### 2.4 Reclaim module

The module is immutable, and its address is fixed in the vault's constructor. A bug in it is still bounded by the vault's counters in step 5 above.

```solidity
contract BridgeReclaim {  // vault = msg.sender at construction
    function reclaim(bytes calldata seal, bytes32 depositRoot, bytes32 accRoot, bytes32 nullifier,
        address token, uint32 denomIndex, address recipient, address relayer, uint256 relayFee) external;
    // vault.isDepositRoot(depositRoot); accRoot == finals(token).accRoot or finals(GLOBAL).accRoot (non-zero);
    // !spent[nullifier]; denom from vault.tokenConfig;
    // verifier.verify(seal, reclaimImageId, sha256(abi.encode(cfgId, depositRoot, accRoot, nullifier, token,
    //     denom, recipient, relayer, relayFee)));
    // spent[nullifier] = true; vault.payReclaim(...); emit Reclaim(nullifier)
}
```

### 2.5 LEZ bridge program (new version, redeploy)

```rust
TokenEntry { …, wound_down: bool }   Config { …, wound_down: bool }   AttestSource { …, wind_down_slot: u64 } // 92
Instruction::WindDownAttested { eth_token: Option<EthAddress>, source: AttestSource, vault: EthAddress,
    evm_chain_id: u64, anchor: AnchorWitness, proof: StorageProof }   // permissionless
Instruction::AttestWindDown { eth_token: Option<EthAddress> }          // stand-in configs only (attestor)
Effect::WindDown { eth_token, source, vault, evm_chain_id }            event WoundDown { eth_token }
```

- **The actor** proves `mapping_slot(word(key), wind_down_slot)` under the anchor with `Predicate::Ne(word(0))`. It sends `assert_anchor`, then `Effect::WindDown`.
- **Cost:** a public transaction of about 260K cycles (estimate).
- **The config:**
  - refuses with `WRONG_SOURCE` unless source, vault and chain are its own;
  - sets the flag; a repeat changes nothing.
- **What the flag blocks:**
  - `mint_claim` refuses with `"minting is wound down"`, a text that depends only on public state (U8);
  - `AttestRoot` and `RegisterToken` are refused after a global wind-down;
  - `AddDenomination` is refused for a wound-down token.
- No instruction clears the flag, and `Unfreeze` doesn't touch it. Burns are unaffected.

### 2.6 G2 journal and accumulator (the contract with the G2 worker)

```solidity
struct UpdateJournal { bytes32 cfgId; bytes32 prevState; bytes32 newState; bytes32[] depositRootsUsed;
    ClaimDelta[] claims; Exit[] exits;
    bytes32 claimAccRoot; uint64 claimAccCount; address[] woundDown; }  // new: root and count after the batch, keys wound down in it
```

**Accumulator: an indexed Merkle tree.**
- Depth 32, SHA-256, the vault's zero hashes.
- Index 0 is a sentinel `(0, 0, 0)`.
- `leaf = SHA256("RFP022/acc-leaf" ‖ key ‖ nextKey ‖ u64be(nextIndex))`. Keys compare as big-endian `uint256`; `nextKey = 0` means +∞.
- **Insert x:** take the low leaf L with `L.key < x < L.nextKey` (or no next key). Append `(x, L.nextKey, L.nextIndex)` and point L at it. That is two path updates, about 66 SHA calls per claim.
- **Non-membership:** the same low-leaf check plus one path, about 33 SHA calls.
- **Rejected alternatives:**
  - a sparse Merkle tree: about 256 hashes per operation (4×);
  - a sorted list or hash chain: no cheap non-membership proof.

**Key: the nullifier PDA account id**, `AccountId::for_public_pda(programId, nullifier_seed(nf_c))`. A `SpendClaim` writes it, so it is public in both claim variants and both privacy modes.

**Rules G2 must follow:**
- **R1:** insert and count every non-reverted `Claim` / `ClaimAttested`, in LEZ order.
- **R2:** reverted transactions count for nothing.
- **R3:** carry the wound-down keys in `newState`. Never count or insert a claim on a wound-down token.
- **R4:** a duplicate key is neither counted nor inserted.
- **R5:** `woundDown` lists the keys whose `WoundDown` event landed in this batch.
- **R6:** `claimAccRoot` and `claimAccCount` are also committed in `newState`.

### 2.7 G4, the reclaim guest (same fixed-layout pattern as G3)

**Private inputs:**
- `s`, `dest`, `token`, `denom`;
- the `cfgId` preimage: `chainId`, `vault`, `lezChainId`, `zoneId`, `programId`;
- `depIndex` and `depPath[32]`;
- the low leaf `(lowKey, lowNextKey, lowNextIndex)`, `lowIndex` and `accPath[32]`.

**Public inputs:** `recipient`, `relayer`, `relayFee`.

```
cfgId = SHA256(CFG_TAG‖u256(chainId)‖vault‖lezChainId‖zoneId‖programId);  k = SHA256("RFP022/ck"‖s)
C = SHA256("RFP022/dep"‖k‖dest‖token‖denom‖cfgId);  leaf = depositLeaf;  depositRoot = merkle(leaf, depIndex, depPath)
key = for_public_pda(programId, nullifier_seed(SHA256("RFP022/nfc"‖k‖dest‖token‖u256(denom))));   // nf_c since the 2026-11-14 fixes  require lowKey < key && (lowNextKey==0 || key < lowNextKey)
accRoot = merkle(accLeaf(low…), lowIndex, accPath);  nf_rc = SHA256("RFP022/nfrc"‖s)
journal = abi.encode(cfgId, depositRoot, accRoot, nf_rc, token, denom, recipient, relayer, relayFee)   // 288 bytes
```

- **Cost (estimate):** about 72 hash calls, against G3's ~35 for 36.4K cycles. So roughly **70–90K user cycles, one segment**.
- **Groth16 wrap:** on x86, as for G3.

### 2.8 Solvency (enforced by vault arithmetic alone)

Per (t, d):
- `C + RC ≤ D`. `RC = 0` until FINAL; after FINAL, `C` is frozen and the clamp keeps `C ≤ D`.
- `L ≤ B` (releases ≤ burns).

Per token:
- `burnedValue ≤ claimedValue = Σ_d C·denom_d`.

Combined:
- released value + reclaimed value ≤ `burnedValue + Σ RC·denom` ≤ `Σ (C + RC)·denom` ≤ deposited value.

These hold even if every seal is forged. Fees come only out of `accruedFees`.

### 2.9 Privacy

- **A reclaim publishes:** the deposit root (shared by every deposit under it), `accRoot` (one per final), `nf_rc` (fresh), token, denomination, recipient, relayer, fee and time.
- **It hides:** the leaf, `C`, `nf_c` and `dest`.
- After a global wind-down the deposit root is final. Clients should use it, so every reclaim shows the same root.
- Clients should default to a fresh recipient address and a relayer.

### 2.10 Wrapped tokens on LEZ at wind-down

- Holders burn and release as usual while outbound is trusted.
- If outbound is broken, no proof of holdings is possible on LEZ 0.4:
  - public balances sit in public state that has no root, written by public execution that is never proven;
  - private notes have their spend status in a nullifier set that has no root.

## 3. No-backdoor checklist

| Function | The owner proves the entitlement; no party picks owners | Pays only where the proof says | Scope limited to funds put at risk |
|---|---|---|---|
| `windDown` | Moves nothing and names no one | No payout | Only opens reclaims of that key's unclaimed deposits |
| `update` (final) | Proof only, permissionless | Records a root; no payout | Freezes claim counts of wound-down keys only |
| `BridgeReclaim.reclaim` / `payReclaim` | Needs `s` (seed-derived, as for a claim); no admin input | `recipient` and `relayer` come from the digest; fee from the menu | Deposit proven unclaimed; `C + RC ≤ D`; caps; delay; FINAL only |
| LEZ `WindDownAttested` | Proof of vault state, permissionless | Mints nothing | Stops minting only |
| freeze/unfreeze reclaims | No funds move | — | — |

**After wind-down the admin cannot:** reopen deposits, undo the wind-down, change a final root, swap the image or module, or touch principal. The admin can still throttle payouts through caps and freezes; those powers are liveness only and already exist.

**"Proven vulnerability":** no chain can prove that a bug exists, since a bug is a gap between code and intent. So the scope is enforced by structure instead: per-token keys, unclaimed P1 positions only, and owner proofs only.

## 4. Bytecode (estimates; vault runtime, `via_ir`, runs 200)

| Change | Bytes |
|---|---|
| `windDown` and deposit gate | 210–370 |
| `update`: three journal fields, clamp/skip, final loop, count check, freeze bypass | 600–900 |
| `payReclaim` hook | 450–700 |
| Reclaim freeze/unfreeze | 120–200 |
| New public getters | 250–400 |
| `_waitOver` / `_checkRelay` refactor | −150 to −300 |
| **Total** | **≈1,500–2,300 of the 3,962 left** |

- **Module:** about 1.5–2.5K of its own runtime. Its initcode sits inside the vault's initcode, well under the 49,152-byte limit.
- **Reclaim inline instead:** about +900 bytes, which fits alone but not alongside the contest window that DESIGN §6.3 earmarked this room for.
- **If over the limit:**
  1. add `extsload` and move the composite views to a lens contract (est. −1.5 to −2.5K);
  2. lower `optimizer_runs`.
- CI should fail if `forge build --sizes` shows the vault above 24,576 bytes.

## 5. Tests

| Constraint | Tests |
|---|---|
| Admin can't move funds in any state | Invariant handler adds `windDown`, freezes, caps, menu, fees and roles in every state; every admin selector from the ABI leaves principal unchanged |
| Irreversible | `test_WindDownIrreversible` (`windDownAt`/`finals` survive every call; `unfreezeAll` doesn't reopen deposits); simulator `wind_down_survives_every_admin_op` |
| Reclaim of a claimed deposit fails | Property test on the accumulator crate: no non-membership witness verifies for an inserted key; G4 core and executor |
| Claim after wind-down fails on LEZ | Simulator `a_claim_after_wind_down_is_refused_in_every_shape` (public, private, stand-in, attested, retries) |
| Post-final claims never count | `test_ClaimDeltasAfterFinalIgnored` |
| Reclaims + releases ≤ deposits | `invariant_all` extended, with a mock verifier accepting forged reclaims and finals; `testFuzz_SolvencyWithReclaims`; cross-side conservation trace with wind-down |
| No redirect, no replay | `ReclaimVector.t.sol` (executor journals; each field flipped); `test_ReclaimReplay` |
| Needs both halves and the delay | `test_ReclaimNeedsRequestFinalAndDelay`; `test_FinalWithoutRequestIgnored` |
| No stall | `test_OverMintClampsAfterRequest`; `test_UnbackedExitDroppedAfterRequest`; ACTIVE still reverts |
| A leaked `k` can't reclaim | Simulator: a public claim reverted at wind-down, then only the holder of `s` reclaims |
| S8 and layout | Extend `test_NoSetterForVerificationInAbi` to the module; module address = the CREATE address; slots 83 and 92 pinned |
| LEZ proof | Simulator: `WindDownAttested` with anvil `eth_getProof`; wrong slot, vault or source refused |
| End to end (local) | Deposit N, claim some, request, `WindDownAttested`, host-built final journal with the mock verifier, G4 executor journals: exactly the unclaimed deposits pay out |

## 6. What happens without a failsafe

See the table at the top of this file (kept there because the proposal must state it). In short:
- **(b):** frozen and locked.
- **(c):** reclaim for P1; P2–P4 frozen and locked.
- **(d):** everything frozen and locked.
- **(e):** frozen and locked.

## 7. Staged plan

| Task | Dependencies | Testable now with the mock verifier? |
|---|---|---|
| W0: interface note with the G2 worker (§2.6: leaf, key, journal fields, R1–R6). Done: given as its interface | — | yes |
| W1: `claim-acc` crate (insert with witness, verify, test vectors, property tests), shared by G2 and G4 | W0 | yes |
| W2: secret convention `s → k` in the CLI and bridge core (given to the recovery worker) | — | yes |
| W3: LEZ wind-down flags and instructions, refusals, simulator tests | slot 92 pinned | yes |
| W4: vault storage, `windDown`, `update` rules, hook, module, size measurement, Foundry tests | W0 | yes |
| W5: G4 core, guest, executor cycles, `ReclaimVector` | W1, W2 | yes |
| W6: invariant and conservation extensions | W4, W5 | yes |
| W7: local end-to-end with host-built G2 journals | W3–W5 | yes |
| W8: G2 emits the new fields; real-verifier `update` and reclaim; Groth16 wrap of G4 on x86 | real G2 | no |
| W9: DESIGN §10, matrix row S9, decision log, user runbook | all | — |

## 8. Spec tie-ins

- **R4:** wind-down turns the claim entitlement into a reclaim entitlement with no deadline. Finals and deposit roots never expire.
- **S8:** the reclaim image and module are fixed at deployment; the admin chooses only *when* (as with freeze), not *what* is accepted. Wind-down is also how an old version is retired at migration: unclaimed deposits reclaim, holders burn and release, then deposit into the new version.
- **F10:** a per-token wind-down is a clean delisting with refunds.
- **S5:** freezes stay independent on each half.
- **PP5:** reclaim refusals are fixed 4-byte errors.

**Still open:** re-check the module decision against measured bytecode after W4.
