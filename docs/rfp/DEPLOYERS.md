# Standing up an independent deployment (outline)

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFP's own words)

- **RFP-022 Reliability #14:** "Provide documentation in repo for the deployer journey, covering how an entity stands up
  its own independently configured deployment (Functionality #16): registering tokens, setting fees, configuring the
  admin and freeze authorities, and the finality depth."
- **RFP-022 Functionality #10:** "Documentation needs to be provided to handle registration changes with minimum impact
  for users, including delisting of a token." (§4.)
- **RFP-022 Functionality #16:** "Document how a client identifies and switches between configurations." (§5.)
- **RFP-022 Design Rationale, "Token registry and decimal normalisation":** "the wrapped LEZ mint for each token must
  document its own decimals and the exact conversion applied on mint and burn, and registration must reject any token
  whose values cannot be represented exactly in the chosen LEZ mint precision." (§3.)
- **RFP-022 Design Rationale, "Fee structure":** "Configuration proposals must specify: … Denomination … Rate …
  Collection point … Routing." (§6.)
- **RFP-022 Security #8:** "Proposals must document the migration mechanism and how in-flight deposits and burns are
  honoured across it." (§7.)

## Status

- **Outline.** The steps below are the ones we ran for vaults v4–v7 on Sepolia and our public chain
  (`public-demo/V4-PLAN.md` §2, `V5-PLAN.md`, `public-demo/RUNBOOK.md` (not published) §18, §21, `vault/DEPLOYMENTS.md`), stated
  generally. They are operator-run scripts with our paths, not a packaged deployer tool; an outside deployer has not
  followed them. The full deployer journey is M7.
- **What must happen first for a complete document:** the admin and freeze roles on RFP-001 and RFP-002 libraries
  (today our own code, deviation F10/F14); an LEZ testnet 0.4 to deploy on; the deployer UI (the mini-app's admin screen
  is TESTED only); the recommended defaults set in M4 (`DESIGN.md` §6.1a).

## 1. One deployment = one vault + one bridge program instance

- The vault and the bridge are bound to each other by `cfgId = SHA256(CFG_TAG ‖ u256(evmChainId) ‖ vault ‖ lezChainId ‖
  zoneId ‖ programId)`, derived on both sides, never supplied (`DESIGN.md` §2). `lezChainId` is the LEZ chain's Bedrock
  channel id (declared at `Initialize`; the CLI checks it, `lez-bridge chain-id`); `zoneId` is the root-source id (the
  operator stand-in or one RFP-021 light-client instance, `lez-bridge source-id`).
- A program id on LEZ rc3 is its loader header account, so the same bridge ELF deployed at a new header is a new
  program with its own config: a new deployment needs no code change (`AGENTS.md`, Conventions).
- Isolation between deployments is tested (`lez-bridge/bridge/tests/isolation.rs`, `vault/test/CrossConfig.t.sol`). One
  bridge program deployment holds one configuration; several entities means several deployments (deviation F16).

## 2. Steps (as run for v4–v7)

1. **Versions first.** `node scripts/check-versions.mjs --onchain --deployment <name>` must pass before anything is
   deployed (add `[deployments.<name>]` to `VERSIONS.toml`, copied from `v4`).
2. **One real seal per pinned image** must pass `cast call <verifier> "verify(bytes,bytes32,bytes32)"` on RISC Zero's
   bare Groth16 verifier 3.1.0 before a vault pins the image (`V4-PLAN.md` step S0). Never the 3.0.0 verifier, the
   router or an emergency stop.
3. **LEZ: deploy the bridge program** at a fresh header key: `lez-bridge deploy --which bridge --elf … --payer …`
   (immutable by default); read its ids with `program-id`, `chain-id`, `source-id`.
4. **The vault's genesis state:** `outbound_host genesis --block <tip before Initialize>` prints the non-zero
   `INITIAL_G2_STATE` (a zero genesis state can never take an update; `AGENTS.md`, Conventions).
5. **Ethereum: deploy the gate and the vault.** For v4 and v5, `vault/script/SepoliaV4.s.sol` (`run()`, then
   `configure()`; v6 and v7 used their branches' scripts) takes the
   image ids, the genesis state and the two release delays (`MIN_RELEASE_DELAY_SECONDS`, `RELEASE_DELAY_SECONDS`) as
   required inputs; a threshold-signature gate: `vault/script/SeqSigDeploy.s.sol` (the gate first, with the vault's
   predicted address). The deposit desk, if wanted: `vault/script/SepoliaDesk.s.sol`.
6. **Configure the vault** (admin): `registerToken(token, decimals, denoms, depositFees, withdrawFees, releaseDelay)`,
   `setRelayFeeMenu`, `setCaps`, `setGlobalCaps`, `setCapWeight`; then `setFreezer` and `proposeAdmin` / `acceptAdmin`
   to hand the roles to their holders.
7. **LEZ: wrapped definitions and initialisation.** `lez-bridge create-wrapped` (mint authority `Program(bridge)`);
   `lez-bridge init` (stand-in root source) or `init-attested --attest-program … --instance …` (light-client root
   source), signed by the bridge's header account, naming admin, freezer and attestor (none may be the header); then
   `register` per token (it refuses a definition whose authority is not the bridge), `set-sponsor` (≤ 50,000,000) and
   `fund-float`.
8. **Check both sides agree:** the vault's `cfgId()` equals the bridge config's (`lez-bridge show`), and
   `check-versions --onchain` passes again.
9. **Clients:** a bridge code (`web/deposit/tools/bridge-code.mjs <id>`, `logosbr1…`) for wallets to add the
   deployment; the operator's steps for a new vault in `modules/NAMES-SEND-BACK.md` §9.4.
10. **Services:** a claim relay per bridge instance, an outbound updater (the gate's submitter for an
    `UpdateGatedVerifier`; signers for a `SeqSigGate`), a light-client follower for the chosen instance
    ([OPERATORS.md](OPERATORS.md)).

## 3. Decimals and conversion

A wrapped definition uses the Ethereum token's own smallest units: a claim of denomination `d` mints exactly `d` units,
a burn of `d` records an exit of `d`; there is no conversion (`DESIGN.md` §3.2, §4; two 100 dUSDC claims gave a supply of
200,000,000, a 6-decimal token, `DESIGN.md` §3.3a). The vault refuses a registration whose decimals differ from the
token's (`BadDecimals`); the LEZ bridge requires 18 for ETH. LEZ balances are `u128` (`lee/state_machine/core/src/account.rs`,
LEZ rc3), so a denomination is representable exactly if it is below 2¹²⁸; the CLI's `register` takes denominations as
`Balance` (`lez-bridge/cli/src/main.rs`, `Register`), so a larger one cannot be registered on LEZ. The program cannot
read the vault's denominations; the core module checks a bridge code's decimals and denominations against both the vault and the LEZ config before
keeping it (`modules/lez_bridge/rust-lib/src/bridges.rs`). The
ERC-20 decimals matrix (USDC, USDT, DAI, WETH) is tested on branch `erc20-path` (`docs/ERC20-PATH.md`,
`vault/test/Erc20Matrix.t.sol`). Rebasing and
fee-on-transfer tokens: the vault refuses fee-on-transfer at deposit; rebasing tokens must not be registered (policy,
not detectable, `DESIGN.md` §3.1).

## 4. Registration changes and delisting (F10)

- **Delisting:** `deregisterToken` on the vault stops new deposits; existing deposits still claim and every exit still
  releases (`test_DeregisteredTokenStillReleases`). `DeregisterToken` on LEZ keeps claims and burns working
  (`deregistered_tokens_still_claim_and_burn`). For least impact: announce it, deregister on the vault first, leave LEZ
  registered until unclaimed deposits are claimed, and keep an updater running until exits are released.
- **Denominations** are append-only on both sides (`addDenomination`; no removal), at most 8.
- **Fees and the relay-fee menu** can change; a menu change never strands an exit (`test_MenuChangeNeverStrandsExit`).
- **Release delay:** a raise applies only to exit roots recorded after it (`testFuzz_DelayChangesNeverReblock`).
- **Caps:** carried over across a window change; a cap of 0 refuses that token's deposits or releases.

## 5. How a client identifies and switches configurations (F16)

A deployment is identified by its `cfgId` and the pair (vault address + chain id, bridge program id + LEZ endpoint).
Clients keep a list (`modules/lez_bridge/rust-lib/deployments.json`, `web/deposit/src/lib/deployments.js`); a wallet adds
one from its bridge code after checking it on both chains (`bridges.rs`: `bridge_check`, `bridge_add`, `bridge_use`);
every stored deposit and exit keeps the deployment it was made on (`modules/NAMES-SEND-BACK.md` §9).

## 6. Fees (RFP-022 "Fee structure")

- **Denomination:** the bridged token. **Rate:** 0 to 2% per token and denomination (`MAX_FEE_BPS` 200), admin-set, the
  same for every deposit of a denomination. **Collection point:** on Ethereum, on top of the deposit (`depositFee`) and
  out of the release (`withdrawFee`), so LEZ balances stay whole denominations. **Routing:** `feeRecipient`
  (`DESIGN.md` §6.1, §11).
- This is a deviation from F13's "on LEZ" (`DESIGN.md` §11); the gap audit proposes a native-LEZ fee instead
  (`proposal/RFP-GAP-AUDIT-2026-11-19.md` A-2, a private file untracked in the working copy of `main`), not decided.

## 7. Migrating to a new version (S8)

Verification is fixed per vault (verifier, image ids) and per LEZ program (immutable). A change ships as a new vault and a
new bridge instance beside the old ones; the old vault stops deposits and keeps releasing against its own exit roots;
unclaimed deposits stay claimable on the old bridge; burns never move between vaults (`DESIGN.md` §10; v4 still released
beside v5). The old deployment needs its own updater for as long as exits remain.

## 8. Finality depth

- **Inbound:** Ethereum finality of the chosen light-client instance, fixed per instance (write-once); a deployment picks
  its instance. An admin-set minimum anchor age is DESIGNED (M4).
- **Outbound:** not configurable today. On v4–v6 the submitter chooses the LEZ tip (our updater uses head − 20); on v7 the
  signers sign only blocks a set depth under the head (`seqsig attest --final-depth`). The Logos Blockchain depth k
  belongs to the accredited-committee route (DESIGNED, M6).

## Sources

`DESIGN.md` §2, §3, §6, §10, §11; `AGENTS.md`; `public-demo/V4-PLAN.md`; `vault/DEPLOYMENTS.md`;
`vault/src/BridgeVault.sol`; `lez-bridge/cli/src/main.rs`; `modules/NAMES-SEND-BACK.md` §9.
