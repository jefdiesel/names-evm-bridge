# Open settlement: who settles the bridge, and how they get paid

Written 2026-11-14 from main `74c55c3`. Design only: nothing here is built unless a line says so. State labels are those of `DESIGN.md`: (1) run on a chain, (2) tested in simulation or unit tests only, (3) designed only, (4) not started.

## 0. The decision and what it means

Two decisions by the project owner on 2026-11-14:

1. **Settlement is an open role.** Proving light-client updates, relaying and co-signing claims, submitting outbound state updates and relaying releases must be things anyone can do for a fee, with no permission and no registration. The team runs the first instance of each, and only the first.
2. **The bridge fee is adjustable and set to a fair amount.** On the Sepolia demo the deposit desk's fee is now **0.3% (30 bps)**, owner-set, with a hard cap of 3% (300 bps) fixed in the contract (§7).

Both RFPs already ask for the first:
- RFP-022, Trust model: "Where an off-chain participant is unavoidable, for example relaying a transaction or generating a proof, that role must be permissionless so any single participant declining to act never blocks the user". Also Usability #5 (a permissionless relayer is a default) and Privacy Preservation #6 (the user can switch between submitters per operation).
- RFP-022, The privacy requirement: "Permissionless relayers paid out of the bridged amount are the conventional mitigation, with the destination bound into the proof so a relayer cannot redirect funds or overcharge."
- RFP-021, Functionality #11 (anyone may advance the state, with no allowlist, role or registration) and #12 (liveness roles only); Reliability #9 and #10 (check before submitting; vary the deferral so cost does not always land on the same operator); Supportability #7 (document the operator role and say plainly that nobody is obliged to operate).
- RFP-021, Soft Requirement #5, Operator incentive: "Propose a mechanism that rewards whoever advances the consensus state … it may make operating attractive, but must not make it exclusive."

The rule for every mechanism below: **paying a settler must add no trust.** A settler can only stop (liveness). It can never mint, redirect, take a fee it was not offered, or learn something that links a deposit to a claim beyond what the job already shows.

## 1. Jobs at a glance

| Job | Open today? | Paid today? | Proposed mechanism | Milestone | Trust added |
|---|---|---|---|---|---|
| (a) Light-client update prover (RFP-021) | **Yes**, in the program: any account may submit (1, local rc1) | No | Payee bound inside the proved update; per-instance reward pool in native LEZ, paid when the store accepts the update; reward per elapsed time, not per update | M3 (program side); Ethereum-side payout: follow-on after G2 | None (a pool funder can only stop funding) |
| (b) Claim relay, the LEZ fee payer of a public claim | **No**: one operator on the demo chain (1) | No (the desk fee on Ethereum is the operator's budget, by accounting only) | Any relayer co-signs; the claim refunds the relayer a fixed amount of native LEZ from the bridge's float in the same transaction | M5, after the claim-nullifier fix (in progress) | None |
| (b′) Private-claim fee (the float) | Yes: any private claim draws on it (1, local) | Yes, from the float | Unchanged; cap the sponsor amount | M4 | None |
| (c) Outbound state update (G2) on Ethereum | **Yes** in the vault: `update()` is permissionless (2); G2 itself not built | No | G2 journal commits the prover's payee; a fixed reward per accepted update from bridge fees on Ethereum | M6 (only if M2 says go) | None beyond G2's own |
| (d) Release relay on Ethereum | **Yes** in the vault (2): anyone may call `release()` | **Yes** in the vault (2): a relay fee from the released amount, at most the exit's `maxRelayFee`, from an admin menu | Exists. Add the relayer daemon | M5 (daemon) | None (relayer and fee are in the G3 journal) |
| (e) Solver: fronting funds before settlement | No | No | Later layer: needs claim-right assignment that keeps the privacy model | Not in this bid | Would add counterparty risk for the solver, not the user |

## 2. (a) Inbound light-client update prover (RFP-021)

**What it is.** One Ethereum sync-committee update (finality or period) verified in the submitter's own actor of `eth_attest`, as a private, locally proved execution, because one update (37.2M user cycles on Sepolia, 39.5M on mainnet, 46.3M for a period update; `lez-bridge/attest/cycles/results/run-20261112T165250Z.txt`) does not fit LEZ's 10M public cap. Only a small `Effect::Advance { prev, next }` reaches the public store (`DESIGN.md` §9.2).

**Who can do it today.** Anyone. `instruct` asserts only that the submitting account signs (`lez-bridge/attest/program/src/lib.rs`, `instruct`: "Any account may submit"); the store accepts `Advance` only from the program's own code, only if `prev` equals its state, and only if `next` advances it (`apply`, `Effect::Advance`; `is_advanced_by` in `lez-bridge/attest/core/src/lib.rs`). Run (1) on the local rc1 chain: a fresh private account with no prior relationship to the instance submitted a proved update (block 1859, `DESIGN.md` §9.0).

**What stops it being open in practice.**
- **Cost.** One finality update took **7,074.7 s and 9,483 MB peak** on an Apple M4 desktop with 16 GB, CPU prover, 4 threads, LEZ rc1 (`DESIGN.md` §9.0). That is the only proving measurement. A period update has a cycle count (46.3M) and no proof time. **No GPU number exists:** the GPU benchmark bundle has never run on a GPU.
- **What M3 changes (designed; measured in the executor only, `DESIGN.md` §9.3a).** A cheaper signature path brings a finality update's verification to 8.9–9.5M cycles, and M3 commits to a two-step public update (each step at most about 7.3M, estimated) next to the private path. Then a submitter pays only gas, with no 2 h proof, and the race below becomes a race between gas payers. Nothing of this is built; the proved private updates remain what runs today.
- **No daemon.** The update-submitting component (RFP-021 Usability #3, Reliability #9 and #10) is not built (M3).
- **No reward.** A prover spends about 2 h of a desktop and its own LEZ fee, and gets nothing. A faster prover makes that work worthless, because the store refuses a stale `prev`.

**How it is paid today.** It is not.

**The open, paid version (3).**

1. **Bind a payee inside the proof.** `Instruction::Update` gains a `payee` field (a LEZ public key chosen by the submitter, possibly fresh). The program copies it into `Advance { prev, next, payee }`. The circuit proves that this program's image produced that message (`DESIGN.md` §9.2, points 1–3), so whoever copies the transaction cannot change the payee without proving the update again. The submitter itself stays a blinded private account; only the payee key is public.
2. **Where the money comes from.** Bridge fees are collected on Ethereum today: the desk's `feesHeld` and the vault's `accruedFees`, paid out by `withdrawFees` to `feeRecipient` (`vault/src/desk/DepositDesk.sol`, `vault/src/BridgeVault.sol`). A LEZ-side prover therefore needs either LEZ-side funds or a way to claim on Ethereum. Three options:

   | Option | How | What is provable | Trade-off |
   |---|---|---|---|
   | **A. LEZ-side reward pool (chosen)** | A per-instance pool PDA of `eth_attest` holds native LEZ. Anyone tops it up (the same pattern as the bridge's float, `fund-float`). When the store applies an `Advance` that earns a reward, it moves the reward from the pool into a reward PDA keyed by the payee, in the same transaction. The payee withdraws later by signing with its key | Fully on LEZ: the store applies `Advance` in public execution, and the transfer is the program's own custody move under its seed, as the float's payment to a one-time address is today (`DESIGN.md` §3.5) | Someone must turn Ethereum fees into native LEZ and top the pool up: an off-chain step and a liveness role. An empty pool pays nothing, and the update still applies, so liveness of the light client never depends on the pool |
   | **B. Claim on Ethereum by proving authorship** | The payee is an Ethereum address. A reward contract on Ethereum pays once it is shown that LEZ accepted that `Advance` | Only with a proof of LEZ state on Ethereum, which is G2 (`DESIGN.md` §0: no LEZ state root on 0.4; G2 is gated at M2). Before G2: not provable at all | Pays straight from where the fees are, with no conversion. Inherits G2's trust (LEZ block validity) and its schedule. Follow-on, not in this bid |
   | C. Wrapped-token share minted at claim | The bridge mints a fee share of each claim to a pool, backed by the vault's deposit fee | Provable, but it changes the vault's conservation accounting | Fractional amounts cannot exit as one denomination (the reason fees are on Ethereum, `DESIGN.md` §11, F13). Rejected |

   Why a reward PDA and not a direct payment to the payee's account: on LEZ rc3 a public account that is not yet in state is admitted only by its signature, admission evidence, or a PDA seed of the sending program (`DESIGN.md` §3.6, "Why an account id cannot simply be funded"). A payment to a never-used account would make the whole update revert. A PDA the program seeds is always admitted.
3. **Pay for time, not for updates.** `is_advanced_by` accepts any later finalized slot (`lez-bridge/attest/core/src/lib.rs`). A flat reward per accepted update could be farmed: one update per new finalized epoch, which is up to 225 a day (computed: 86,400 s ÷ (32 slots × 12 s); constants from `ethereum/consensus-specs`, `presets/mainnet/phase0.yaml` `SLOTS_PER_EPOCH: 32` and `configs/mainnet.yaml` `SLOT_DURATION_MS: 12000`, read 2026-11-14). The rule instead:
   - a **period update** that newly records the next sync committee is always rewarded (it is the one the instance cannot live without, below);
   - a finality update is rewarded only if its finalized slot is at least `reward_min_slots` past the last rewarded one;
   - the reward amount and `reward_min_slots` are part of the instance's write-once configuration (RFP-021 Functionality #9 and #13), so nobody can change them after creation. No amount is proposed here: neither proving time nor native LEZ has a price yet.
4. **The race, stated plainly.** A reward turns concurrent submission into a contest. The fastest prover wins each reward; a slower prover's work is refused as stale and is wasted. RFP-021 Reliability #9 and #10 ask the daemon to check that an update is still needed right before submitting and to vary its deferral so cost spreads across operators. With a reward, deferral still cuts wasted proofs, but it cannot make the contest even: operators with faster hardware will win more often. That is "attractive, not exclusive" (Soft #5): nothing stops a slower operator from submitting, and nothing gives anyone priority.

**Liveness when nobody proves.**
- Anchors already recorded stay valid for ever (anchor PDAs are never deleted; RFP-021 Reliability #3). Claims of deposits under those anchors keep working.
- Deposits made after the last finalized anchor wait until someone submits an update.
- **A long gap costs proofs, not the instance** (corrected 2026-11-14; an earlier version said a gap of more than one period strands the instance, which is wrong). The light-client rule accepts an update signed in the store's sync-committee period, or the next one when the next committee is known ("assert update_signature_period in (store_period, store_period + 1)"; `validate_light_client_update`, Altair light-client sync protocol in `ethereum/consensus-specs`, read 2026-11-14). The store's period is that of its finalized header, not the wall clock, and `eth_attest` adds no upper time bound. So a stalled instance catches up by applying each missed period's update in order (each carries the next committee), then a current finality update (`DESIGN.md` §9.6). One period is 256 epochs (`presets/mainnet/altair.yaml`, `EPOCHS_PER_SYNC_COMMITTEE_PERIOD: 256`), about 27.3 h (computed: 256 × 32 × 12 s = 98,304 s).
- Limits: each missed period is one more private proof (a period update is 46.3M cycles, never proved, about 2.5 h on our CPU by estimate); beacon nodes SHOULD serve old updates for about 146.8 days (phase-0 rule) or 63.6 days (Gloas rule); and a catch-up past the weak-subjectivity period (about 15.7 days under the Electra rule mainnet runs, 6 to 7 days under Gloas; consensus-specs guides) is not safe, so the instance is then replaced (RFP-021 Functionality #10). Tested across one period boundary only (`committee_handoff_period_update_then_finality_signed_by_the_next_committee`); the CLI's automatic pick is wrong after two or more boundaries; M3 fixes the pick and adds a multi-period catch-up test (`DESIGN.md` §9.6).
- For the bridge, a replaced instance is serious: a bridge config fixes its attestation instance (`InitAttested`; no instruction changes it, `DESIGN.md` §3.2), so new deposits would need a new bridge deployment and vault. Period updates always earn the reward because they are the step every catch-up needs, and paying for them keeps the instance within one cheap step of current.

**Change and size.** `eth_attest`: the `payee` field in `Update` and `Advance`, reward fields in `InstanceConfig`, a pool PDA and a reward PDA per payee, and a `Withdraw` instruction. Size not measured. The program is deployed immutable, so this is a new program image, hence new instances. The bridge config binds the attestation program, so the bridge is redeployed too, which changes `cfgId` and needs a new vault. That redeploy happens anyway for LEZ testnet 0.4 and for the security fixes (done in the code, not deployed), so the change rides on it.

**Milestone.** M3 delivers the payee binding, the pool, the reward rule, their tests and the operator documentation (RFP-021 Supportability #7). The Ethereum-side payout (option B) and any automatic conversion of fees into native LEZ are a documented follow-on.

## 3. (b) Claim relay: the fee payer of a public claim

**What it is.** A public LEZ transaction's fee payer must sign it and must hold the fee reserve before the claim runs (`lee::is_fee_authorized`; `lez/chain_state/src/apply.rs`, "Phase 1: Reserve"; `DESIGN.md` §3.5). A claimer's account that was never used holds nothing and cannot be credited by its id alone. So the claimer signs its claim and a second signer, the relay's payer, is named as `FeeDeclaration.payer` and co-signs (`lez-bridge/cli/src/relay.rs`).

**Who can do it today.** One operator: the team's relay on the public demo chain since 2026-11-13 (a small operator service, not published, around `lez-bridge relay-cosign` in `lez-bridge/cli/src/relay.rs`; `DESIGN.md` §3.6). First relayed claim: LEZ block 552, the relay's payer paid 1,233,528 native LEZ, the claimer's account holds 0 native LEZ. It accepts only the public stand-in `Claim` instruction, not `ClaimAttested`.

**What stops it being open today.**
- **Nobody pays a relayer.** The team's payer spends its own native LEZ; the desk's fee on Ethereum is its budget only by accounting (`public-demo/attester/fee-grants.py` prints fees collected next to LEZ paid; both are test money and are not converted).
- **Discovery.** `claim --relay URL` already lets a user choose any relay per claim (Privacy Preservation #6), and the relay code is generic, but there is no list of relays and no Logos module (Usability #3).
- **A relayer sees the claim secret before the claim lands.** A public claim publishes its secret `k`, and the relay receives it before inclusion. An independent review (2026-11-14) found that the claim nullifier depends on `k` alone, so whoever learns `k` while the nullifier is unspent can spend it with a cheaper deposit of its own and make the real deposit unclaimable. **Fixed in the code (2026-11-14, not deployed):** the nullifier now binds the destination, token and denomination (`DESIGN.md` §2). Open relays wait for the v3 deployment that carries it; the live v2 demo keeps the old nullifier.
- **Cross-chain replay.** A LEZ public message hash carries no chain id, so a payer's co-signature could be replayed on another LEZ chain where the nonces match (same review). An open relay uses one payer per chain until LEZ binds a chain id.

**Can the bridge pay the relayer inside the claim?** Yes, within LEZ's rules, in native LEZ:
- The payer must sign and reserve before execution, so the relayer always fronts the reserve from its own balance. Nothing inside the claim can change that.
- Inside the claim, the bridge config can send a fixed `relay_refund` of native LEZ from its float to the payer, under the config's seed, as it already pays `sponsor_amount` to a one-time address (`DESIGN.md` §3.5). The payer signed, so it is admitted. The refund lands in the same transaction the relayer paid for.
- If the claim reverts, the refund reverts with it, and the relayer has paid at most its `max_fee`. A checked claim can revert if the admin or freezer freezes minting first (`DESIGN.md` §3.6, "What a griefer can cost") or, until the nullifier fix, if another claim spends the same secret first (same review).
- A fee taken from the minted wrapped token, like the vault's release relay fee, is not used: the claimer would hold `denom − fee`, which cannot exit as one denomination (`DESIGN.md` §3.5).
- The refund is `min(relay_refund, float balance)`: an empty float pays nothing and the claim still applies, so the float's level never blocks a claim (unlike today's sponsor payment, which needs the float, `an_empty_float_fails_the_claim_cleanly`).
- The refund is one fixed value for every claim, so it does not single out a claim (RFP-022 Fee structure: "a distinctive fee is itself a fingerprint").
- A claimer that pays its own fee can name itself and take the refund. That is bounded by the nullifier (one refund per deposit) and equals what a relayer would get, so it is a fee rebate, not a leak of funds. The measured cost it stands against: 1,206,760 to 1,233,528 native LEZ per relayed claim on rc3 (`DESIGN.md` §3.6).

**Private claims.** LEZ 0.4 has no third-party payer for a private transaction's fee (`DESIGN.md` §8.4). A private claim mints to a one-time address and the float pays that address its fee inside the proof (`DESIGN.md` §3.5). That is already open (any private claim draws on the float) and already paid (by the float). The same review found the float can be drained one claim at a time and that the admin's sponsor amount has no upper bound; this design adds a bound (M4).

**Built on branch `open-relay` (2026-11-17, not deployed; record `public-demo/prove/open-relay-20261117/README.md`).** As above, with one deviation: the program cannot read its float's balance without a `ReadState` round trip, so a refunded claim pays exactly `relay_refund` and fails whole when the float holds less, instead of paying `min(relay_refund, float)`; a claim that names no payee never touches the float, so the float's level still never blocks a claim, and a relay checks the float before co-signing. The refund is opt-in per claim (`Instruction::Relayed`), refused on a one-time recipient (one float payment per deposit), and the sponsored private burn (`Effect::SponsoredBurn`, `DESIGN.md` §3.5) is added the same way.

**Change and size.** Bridge program: a `relay_refund` amount in the config (admin-set, capped like the sponsor amount) and the refund effect in `Claim` and `ClaimAttested`. Relay: accept `ClaimAttested`; allow exactly one credit-only actor for the payer. Size not measured. The bridge is immutable, so this ships with the redeploy above.

**Milestone.** M5, which already promises "claims for users with an empty LEZ account through a permissionless co-signing relayer": the refund, the relay as a Logos module with a headless daemon, and a relay list in the module's configuration. It depends on the nullifier fix.

## 4. (c) Outbound state updates (G2) and (d) release relaying, on Ethereum

**(c) What exists.**
- `update(journal, seal)` is permissionless: "Permissionless: anyone may submit; a stale update is refused" (`vault/src/BridgeVault.sol`, `update`). Tested with a mock verifier (2); run mock-verified on local anvil.
- Nothing pays the submitter.
- G2 itself is not built: Logos Blockchain finality (4); G2-lite, with the LEZ tip trusted, in progress (`DESIGN.md` §4).
- The Sepolia v2 vault can never accept an update: its `updateImageId` is a placeholder (`vault/DEPLOYMENTS.md`).
- Cost evidence: one Logos Blockchain leader-proof check is 49.2M cycles and took 9,778 s to prove on the same CPU (`research/spike-pol-cycles.md`). G2 needs GPU provers.

**(c) The paid version (3).**
- The G2 journal commits a `payee` address. `update` calldata can be copied from the mempool by anyone, so a reward paid to `msg.sender` would go to whoever copies it; a payee inside the proved journal cannot be changed.
- The vault pays a fixed reward per accepted update, per token, out of collected fees only (`accruedFees`), never principal, and only if fees cover it. An update with no reward available still applies.
- Vault change: one journal field and the payout. Size not measured; the current source has 3,934 bytes left under EIP-170 (`DESIGN.md` §6.3). It ships with the G2 vault version.
- Milestone: M6, only if the milestone-M2 gate says go. If M2 says no-go there is no G2 and nothing to pay.

**(d) What exists, exactly.**
- `release(seal, root, nullifier, token, denomIndex, recipient, relayer, relayFee)` can be called by anyone (`vault/src/BridgeVault.sol`, `release`).
- The user fixes a `maxRelayFee` inside the exit commitment `E` at burn time (`DESIGN.md` §2).
- The G3 journal binds `recipient`, `relayer` and `relayFee`; the guest refuses `relayFee > maxRelayFee` (`DESIGN.md` §4).
- A non-zero fee must be on the admin's menu: at most 4 non-zero values per denomination, each within 2%; 0 is always allowed; a menu change never strands an exit (`setRelayFeeMenu`; `test_MenuChangeNeverStrandsExit`). The Sepolia demo menus are 0.5% and 1%.
- The vault pays `relayFee` to `relayer` and the rest, minus the withdraw fee, to the recipient (`test_ReleasePaysRecipientAndRelayer`; `test_RevertRelayerRedirectsOrOvercharges`).
- State: (2). No release has run with a real proof, and none can run on the Sepolia v2 vault.
- Missing: the relayer daemon (M5). A relayer could also offer the Groth16 wrap, which runs only on x86 (`DESIGN.md` §4, step 4); what a wrap service learns is not analysed (Privacy Preservation #4).

## 5. (e) Solvers: fronting funds before settlement

**What it would be.** A solver gives the user the wrapped asset on LEZ before the user's own claim can land (or ETH on Ethereum before a release), for a fee, and is repaid when the proof lands. Public bridges do this: Across's relayer fee covers "the opportunity cost of capital the relayer locks while waiting for reimbursement (about 1.5 hours)" and reorganisation risk (docs.across.to, "Fees in the system", read 2026-11-14); Hop's bonder "takes a fee for fronting the liquidity of your transfer at the destination chain and taking on risk" (docs.hop.exchange, "Fees", read 2026-11-14).

**What it needs here.**
- **Claim-right assignment.** A deposit commitment binds its LEZ destination (`C = SHA256("RFP022/dep" ‖ k ‖ dest ‖ …)`, `DESIGN.md` §2). To be repaid, the solver must end up with the minted denomination. Either the user deposits with `dest` = the solver's account and hands the solver `k`, or a new escrow program becomes a valid destination.
- **The privacy problem.** In both cases the solver learns which Ethereum deposit it is financing (it needs the leaf to claim) and where it sent the fronted funds. That is exactly the link the bridge exists to hide, and RFP-022 Privacy Preservation #4 says it must never leave the user's control. A version that keeps the model would need: a destination that is an escrow PDA; a zero-knowledge proof the user gives the solver off chain ("a deposit under root R, destination = this escrow, nullifier unspent") that names no leaf; and an escrow that releases to the solver only when the user's own claim lands. None of that exists; whether the solver can be repaid without ever holding the witness is an open question.
- **Outbound.** The same, with `ethDest` fixed in the exit commitment `E`.

**The solver's risks.** Inbound: a deposit fronted before Ethereum finality can be reorganised away; the light client can stall (§2), so the proof may arrive late or, after a period-long gap, only through a new instance; the admin can freeze minting; and until the nullifier fix the secret can be front-run. Outbound: G2 may never exist (M2 no-go); the release delay, caps and freeze all apply to the solver's repayment.

**Why it is a later layer, not in this bid.** It needs a new program, a new privacy analysis and capital. It also works against the RFP's own timing advice ("the UI must encourage some delay"): fronting shortens exactly the wait that widens the anonymity set. The settlement roles above are what the RFPs require; fronting is an optional market on top.

## 6. Liveness summary

- **Light-client updates:** anyone; with a reward, an incentive exists; without one, the instance waits, and catching up later costs one proof per missed period, up to the weak-subjectivity bound, after which it must be replaced (§2).
- **Claims:** public claims need a fee payer: the claimer itself, or any relay (open in M5). Private claims need the float to hold funds; anyone may top it up.
- **Outbound updates and releases:** anyone; a release relayer is paid from the exit; an update submitter is paid only once G2 exists (M6).
- **Funding the LEZ-side pools from Ethereum fees** is the one job with no on-chain path before G2: someone converts and tops up. That role can only stop (the pools then pay nothing, and every settlement job still works for anyone who pays their own costs).

## 7. Fee policy

**One adjustable bridge fee.**
- **In the trust-minimised vault:** the deposit fee and the withdraw fee, set per token and denomination by the admin (`setFees`), each at most 2% (`MAX_FEE_BPS = 200`, a constant), 0 allowed, charged in the bridged asset, routed to `feeRecipient` (`vault/src/BridgeVault.sol`; the F13 mapping in `DESIGN.md` §11). The Sepolia vaults set both to 0.
- **On the demo network:** the deposit desk's `feeBps`, owner-set, at most 300 (3%, a constant), in the deposited asset, on top of the vault's amount (`vault/src/desk/DepositDesk.sol`). **30 bps (0.3%) since 2026-11-14** (`setFeeBps(30)`, Sepolia tx `0xdf39c20d…05cd`, block 12132632; `feeBps()` reads 30 through PublicNode). The page reads the rate on chain, and a rate change between the quote and the transaction makes the deposit revert. The desk is a demo-network convenience, not part of the trust-minimised bridge (`DESIGN.md` §3.6). Two deposits have gone through it, both at 300 bps.
- **Who sets it:** one admin key today (the throwaway demo deployer on Sepolia). RFP-022 names the baseline: "a governance-activatable fee switch gated by the admin authority per RFP-001" (Fee structure, item 2). Later: the RFP-001 admin authority, and rate changes published with a delay so that a quote holds. A fee contract whose rate can only fall, or that follows a fixed schedule, is an option, not built.
- **Not a fingerprint:** one fixed value per token and denomination, the same for every deposit (Functionality #13).

**What it funds.** `feeRecipient` can be a contract: `_pay` sends ETH with a plain call and ERC-20s with `safeTransfer`, and `withdrawFees` may be called by anyone but pays only `feeRecipient` (`vault/src/BridgeVault.sol`). A splitter contract there routes fees with no vault change:
1. G2 update rewards on Ethereum (§4, M6);
2. top-ups of the LEZ-side pools: light-client update rewards (§2) and the bridge's float, which pays private claimers' fees and, from M5, relay refunds (§3);
3. operations.
No split is fixed and no revenue is projected.

**What "fair" is anchored to.** Published fees, read 2026-11-14:

| Service | Fee | Source |
|---|---|---|
| RAILGUN (a privacy system on Ethereum, not a bridge) | 0.25% per shield and 0.25% per unshield: "The smart contracts take a 0.25% deduction per shield and unshield interaction" | docs.railgun.org/wiki/learn/railgun-fees |
| Hop (bridge) | "Bonder fee (0.05-0.30%)", varies by asset and route | docs.hop.exchange/basics/fees |
| Stargate (bridge) | V1: "a flat 6 bps fee for all transactions via V1 pools"; V2: dynamic, no figure given | stargateprotocol.gitbook.io/stargate/v2-user-docs/whats-new-in-stargate-v2/fees |
| Across (bridge) | no fixed percentage: an LP fee and a relayer fee that vary by token, route and size | docs.across.to/reference/fees-in-the-system |

- 0.3% is at the top of the sourced bridge range (Hop's upper bound) and close to RAILGUN's per-leg fee. A round trip here can also pay a release relay fee if the user uses a relayer (0, 0.5% or 1% on the Sepolia menus) and any withdraw fee (0 on Sepolia).
- **Why 30 bps for the demo:** a round figure inside the range of sourced fees, replacing the 3% the desk was deployed with (its cap). It is test money with no price, so it measures nothing; it is adjustable and the page reads it on chain.
- **What it has to cover, measured only:** about 1.21M to 1.23M native LEZ per relayed public claim on rc3 (`DESIGN.md` §3.6); about 2 h of a 16 GB desktop CPU per light-client update today (§2; gas for two public steps after M3); G2 proving on GPUs, not priced. Native LEZ on the test networks has no price, so fee and cost cannot be compared in money yet.

## 8. What this design does not meet

- **RFP-022 Trust model ("should not depend on any specific off-chain participant") on the demo chain today:** one relay, one root attester (operator-attested by design, `DESIGN.md` §3.2) and one funder of the float, all the team's. This design removes the relay dependency in M5; the attester goes when claims use the light client on a public chain.
- **RFP-022 Usability #5, a permissionless relayer as the default, for private claims:** LEZ 0.4 has no third-party payer for a private fee. Private claims rely on the float, which anyone can use and anyone can top up, but which someone must top up.
- **RFP-021 Reliability #10 (spread cost across operators) together with Soft #5 (a reward):** once there is a reward, operators compete, and faster hardware wins more often. Deferral reduces wasted proofs; it does not make the contest even.
- **Paying the LEZ-side prover from Ethereum fees** has no trustless path before G2. Until then a person converts fees and tops up a pool; that person can stop paying rewards, nothing more.
- **Not built:** every mechanism in §2–§5 is (3) designed only, except the release relay fee (2) and the float (1, local).
