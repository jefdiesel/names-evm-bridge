# Committed amounts (RFP-022 Soft #1) and viewing keys (Soft #2): the migration path

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFP's own words)

- **RFP-022 Soft #1, "Committed amounts":** "Record transferred values on the LEZ side as value commitments rather than
  restricting transfers to a fixed set of permitted amounts, removing the amount-visibility constraint. This merges all
  per-amount anonymity sets into one and reduces the need for users to split transfers. Whatever is delivered under the
  hard requirements should be designed so this can be adopted later without redeploying the vault or resetting
  accumulated anonymity; document the intended migration path even if it is not implemented."
- **RFP-022 Soft #2:** "Optional viewing keys allowing a user to *voluntarily* disclose their own bridge activity to a
  chosen third party, without weakening privacy for anyone else and without any protocol-level disclosure capability."
- **RFP-022 Functionality #8** (the baseline this replaces): "Proposals must state the mechanism chosen (fixed
  denominations are the expected baseline) and its effect on anonymity-set size."
- The proposal (`proposal/FORM-short.md` line 215): "RFP-022 Soft1, Soft2 | not built | the migration path to committed
  amounts is written in M7".

## Status

- **Soft #1: an outline with a proposed target, written now, not reviewed, nothing built.** §1–§2 describe what exists
  (code and measurements). §3–§5 are design proposed in this document. §6 says what blocks it.
- **The deployed vaults cannot adopt it in place.** Every vault version so far (v2–v7) would have to be replaced by a
  new vault and bridge instance, so the anonymity set starts again at the new vault (§5). That falls short of Soft #1's
  "without redeploying the vault or resetting accumulated anonymity".
- **Soft #2: not offered** (`docs/REQUIREMENTS.md`, Soft2 "Desc: not offered"); §7 is an outline only.

## 1. The current design: fixed denominations

- **Ethereum.** Each registered token has up to 8 denominations (`MAX_DENOMS`); a deposit or a release moves exactly one
  denomination (`vault/src/BridgeVault.sol` `deposit`, `release`). The deposit leaf and the exit leaf bind the
  denomination: `SHA256(tag ‖ cfgId ‖ token ‖ denom ‖ C)`. The vault counts deposits, claims, burns and releases per
  (token, denomination index) and enforces claims ≤ deposits and releases ≤ burns per (token, denomination), and burned
  value ≤ claimed value per token (`update`, lines 404-443; `release`, 461-515).
- **LEZ.** A claim mints exactly one denomination; a burn destroys exactly one (`DESIGN.md` §3.2, §4). The bridge config
  keeps public counters per (token, denomination) and per-token `claimed_value` / `burned_value`
  (`lez-bridge/bridge/src/lib.rs` lines 1151-1152, 1279-1280).
- **Proposed denominations** (DESIGNED, `DESIGN.md` §3.1): 100, 1,000 and 10,000 for USDC/USDT/DAI; 0.1, 1 and 10 for
  ETH/WETH. The Sepolia vaults use ETH 0.01/0.1/1 and dUSDC 100/1,000/10,000.

## 2. What fixed denominations leak, and the anonymity set they leave

- **A claim's denomination is public:** the token program's `mint` publishes the credited amount and writes the
  definition's `total_supply` (`lez-bridge/token/src/lib.rs` `mint`, lines 393-410); the credit crosses into a private
  account as a journaled message (`DESIGN.md` §8.3, §8.4 "Minted amounts are public"); the bridge's per-denomination
  counter moves.
- **So the anonymity set of a claim** is the earlier deposits of the same token **and denomination** under the root or
  anchor it uses (`DESIGN.md` §3.1). With three denominations, a token's deposits split into three sets; a user who
  wants a non-round amount splits it into several deposits, each its own event (the mini-app spaces them hours apart,
  `modules/lez_bridge/rust-lib/src/plan.rs`).
- **Measured** (record `public-demo/prove/privacy-test-20261117/README.md`; local chains,
  development receipts): within one (token, denomination) set of 20, an adversary reading every public byte paired 1 of
  20 claims with their deposits (chance: 1). The set size, not the cryptography, sets what chance means: a uniform guess
  in a set of n is right with probability 1/n.
- **What committed amounts would change:** the claim would no longer say which denomination it consumed, so a claim's
  set becomes every earlier deposit of that token: for set sizes n₁, n₂, n₃ per denomination, one set of
  n₁ + n₂ + n₃. Ethereum amounts stay public by construction (RFP-022 Design Rationale).

## 3. Target design (proposed here; not reviewed, not built)

**Keep fixed denominations on Ethereum; commit amounts on LEZ.** Ethereum deposits and releases stay in denominations,
because their amounts are public on Ethereum anyway and a free amount there would fingerprint a round trip ("Amounts
correlate", RFP-022 Design Rationale). What changes is the LEZ side and how the vault counts.

1. **Claim.** In the claimer's own execution the claim proves one deposit of *some* registered denomination of the
   token (the Merkle path and, on the light-client path, the vault storage proof, as today), spends the claim nullifier
   (whose PDA is already an opaque hash), and credits the claimer's private balance with that denomination through a
   message that publishes no amount. Its public part names the token, not the denomination.
2. **Inside LEZ.** Wrapped balances in private accounts already hide amounts: a private-to-private token transfer hides
   from, to and amount (`DESIGN.md` §8.3, path 3). A user may merge several claimed deposits into one private balance.
3. **Burn.** A burn destroys an amount from a private balance and records one exit commitment per Ethereum denomination
   it asks for, `E = SHA256("RFP022/exit" ‖ k' ‖ ethDest ‖ token ‖ denom ‖ maxRelayFee ‖ cfgId)` as today; the burn's
   public part carries the exit commitments and the token, not the amounts.
4. **Exit tree.** The exit leaf drops the public denomination: `SHA256(EXIT_LEAF_TAG ‖ cfgId ‖ token ‖ E)`. The release
   proof (G3) already takes `denom` as a private input and binds it through `E`; the release reveals it, as every
   Ethereum release must.
5. **Vault accounting.** Per token by value, on public data only: released value ≤ deposited value. Claims and burns per
   denomination are no longer visible to the update proof, so the per-(token, denomination) rules "claims ≤ deposits"
   and "burned value ≤ claimed value" move to LEZ, where the token program's balance rules and the claim nullifier
   enforce them. The vault's strongest guarantee today, that it never pays out more than was deposited per token
   (`DESIGN.md` §5), is kept; the intermediate checks that catch a forged update earlier are lost.
6. **Supply.** The wrapped definition's public `total_supply` cannot move per claim, or it publishes the amount. Either
   the token keeps supply as a commitment, or supply moves only in batches (it then publishes batch totals), or the
   wrapped token keeps no supply counter and the vault is the only reference for backing. Which one depends on what the
   LEZ token program offers (§6).

## 4. What must change, component by component

| Component | Change | Kind |
|---|---|---|
| Vault (`BridgeVault.sol`) | exit leaf without the public denomination; per-token value conservation in `update` and `release`; a new update journal format (exits without amounts, no per-denomination claim deltas) | a new vault version (S8) |
| Update proof (G2, `lez-bridge/outbound/`) | extract burns' exit commitments without amounts; no per-denomination claim counts; the claim accumulator stays | a new update image |
| Release proof (G3, `lez-bridge/release/`) | the exit leaf formula of §3.4 | a new release image |
| Bridge program (`lez-bridge/bridge/`) | a claim credit with no published amount; no public per-denomination counters or per-token value totals | a new program (immutable programs) |
| Token program | a mint and burn whose amounts are not published (§6) | upstream or our own token program |
| Recovery client and scan (`lez-bridge/client/`) | positions keyed by token, not denomination; exits found by commitment as today | client change |
| Core module, mini-app, CLI | merge and split plans; the leak indicator drops "denomination" and keeps token and timing | client change |
| Deposit page | none for deposits (still one denomination per deposit) | — |

## 5. Migration path

1. Deploy the new vault and the new bridge instance beside the old ones (as v4 → v5 → v6 → v7 ran beside each other,
   `vault/DEPLOYMENTS.md`).
2. The old vault stops taking deposits (`deregisterToken` per token; existing positions still claim and release,
   `test_DeregisteredTokenStillReleases`, `deregistered_tokens_still_claim_and_burn`).
3. Unclaimed deposits on the old vault stay claimable on the old bridge; old exits keep releasing against the old vault's
   own exit roots (`DESIGN.md` §10). The old vault needs an update submitter for as long as exits remain.
4. Wallets list both deployments (a bridge code per deployment, `modules/NAMES-SEND-BACK.md` §9) and move users to the
   new one by default.

**What this does not do:** carry anonymity over. A deposit in the old vault cannot be claimed under the new bridge
(`cfgId` binds one vault, and burns never move between vaults), so the new vault's anonymity set starts empty. A vault
designed now so that a later committed-amount bridge could reuse it would still need a new update image, and the update
image is immutable in the vault by design (RFP-022 S8; Soft #4 itself says "redeployment and migration would be
necessary"). We have not found a way to meet both S8 and Soft #1's "without redeploying the vault"; this is stated as a
gap, not solved.

## 6. What blocks it on LEZ 0.4

- **A mint publishes its amount.** Our token program's `mint` writes `total_supply` and sends `Credit { amount }`
  (`lez-bridge/token/src/lib.rs` lines 393-410). A public program cannot credit a private account without the message
  being journaled at the boundary (`DESIGN.md` §8.4, citing LEZ rc1 `execution_state.rs` and `circuit_io.rs`). Whether
  the upstream token program on lez-programs' 0.4 branch offers a confidential mint: **not checked**.
- **Program state is public.** Any per-token total the bridge keeps moves by each claim's amount.
- **The update proof cannot see private values.** Under TrustedTip, private claims and burns are extracted on the
  sequencer's word with no receipt check (RISC Zero 3.1.0 provers cannot verify LEZ rc3's 3.0.5 receipts; `DESIGN.md`
  §4), and LEZ publishes no state root (issue #1129). So per-token value totals on LEZ cannot be proved to Ethereum
  today; §3.5's design avoids needing them.
- **No events from private execution** (`EMITS_EVENTS = false`, `DESIGN.md` §8.4).
- **The deployed vaults are immutable** and count per (token, denomination).

## 7. Cost estimate, from measured numbers

- **User side.** Today's private claim proof took 676.0 s at 9,391 MB peak on a 16 GB desktop (rc3, `DESIGN.md`
  §9.0a); its public part is 81,568 cycles (`DESIGN.md` §12.1 P1). The release proof is 36,393 user cycles for about 35
  SHA-256 calls (the exit leaf, `E`, a 32-level path and the nullifier), proved in 20.9 s at 1,129 MB (`DESIGN.md` §4).
  So a SHA-256-based commitment adds on the order of a thousand cycles per hash (derived from those two numbers, not
  measured on its own), small next to LEZ's privacy circuit, which dominates the claim's 11 minutes and is unchanged.
- **Ethereum side.** `release` paid 447,552 gas (v4, real seal) and `update` 671,254 to 899,746 gas
  (`DESIGN.md` §12.1 P3). Per-token instead of per-(token, denomination) counters writes no more storage per update;
  the exit leaf is one word shorter. No measurable change is expected; this is an estimate, not a measurement.
- **Update proof.** About 20,000 cycles per LEZ block today (`DESIGN.md` §4); dropping amount extraction does not add
  cycles (estimate).
- **Not estimated:** an elliptic-curve commitment scheme (Pedersen-style) with homomorphic totals. The only curve
  numbers we have are BLS12-381 pairings (19.73M cycles, `DESIGN.md` §9.3); a per-commitment cost would have to be
  measured. RFP-022 itself calls committed amounts "stronger but considerably heavier".
- **Work:** not sized. It depends on §6's token primitive, which is not ours.

## 8. Viewing keys (Soft #2): outline only

- **Not offered.** Nothing in the code gives a scanning-only key.
- **What exists that comes close:** `lez-bridge exit-secret` and `deposit-secret` write one position's secret to a
  mode-600 file. Handing such a secret to a third party discloses that position, and for an exit lets them prove its
  release (to the address fixed in it); it is more than a viewing key should give.
- **What a viewing key would be:** a per-account key derived from the same root as the bridge secrets
  (`DESIGN.md` §10, `lez-bridge/client/src/kdf.rs`) that lets a holder recompute deposit commitments and exit
  commitments, so find the account's positions on both chains, without deriving `k`, `k'` or any signing key. The current
  derivation puts `k = SHA256("RFP022/ck" ‖ s)` inside the commitment, so a view key would need commitments that bind a
  value derived from `s` separately from `k`: a change to the commitment formula, and so to the vault, the guests and
  every client (`AGENTS.md`, "Hashes and domain tags").
- **What must happen first:** a decision to offer it; then a new commitment format (a new vault version).

## Sources

`vault/src/BridgeVault.sol`; `lez-bridge/bridge/src/lib.rs`; `lez-bridge/token/src/lib.rs`; `DESIGN.md` §3, §4, §5,
§8, §9.3, §10, §12.1; `public-demo/prove/privacy-test-20261117/README.md`; `docs/REQUIREMENTS.md`;
`proposal/FORM-short.md` §7. Privacy measurement plans for M5: `docs/PRIVACY-M5.md` (branch `privacy-m5`, being
written).
