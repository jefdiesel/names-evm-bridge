# Security review of 2026-11-14: findings, fixes, and what the live v2 demo still carries

Two independent reviews of commit `d5d27e9` (2026-11-14): **review A** (the LEZ bridge, token and
attestation programs) and **review B** (the claim relay). This file records, for each finding,
whether we confirmed it, how branch `security-fixes` fixes it, the tests, and what stays true of
the **live v2 demo** (the public demo chain's bridge `JECKB…nUx7`, its token program, the claim
relay and the Sepolia v2 vault `0x8C8b…1b74`). The runs and tests below sent nothing to the public
chain or to Sepolia. **Update 2026-11-14 (later):** the fixed programs were deployed as a second
bridge instance, v3, beside v2 on the public demo chain, with a new Sepolia vault
`0x26af…E60E` (`vault/DEPLOYMENTS.md`, `public-demo/RUNBOOK.md` (not published) §16); initialised; first public claim LEZ block 4703; the claim relay serves v3 since 14:13 with the reviewed relayer and filter (Review B fixes); relayed claim on v3: block 4802; page: v3 since 2026-11-14 once deployed; wallet claim on v3: once, LEZ block 4852, on a team test Basecamp with an unreleased wallet build (0.17), not yet on the owner's Basecamp; no private claim on v3 yet. v2
keeps the old programs and everything said below about the live v2 demo stays true of it.

State labels as in `DESIGN.md`: **(1)** run on a chain (here a throwaway loopback LEZ v0.4.0-rc3
chain and a local anvil, on the build host, fresh keys); **(2)** simulator or unit tests.
Fail-before / pass-after evidence: the regression tests were pushed alone first and failed in CI
run `43588904344` (6 of 7: `c1_the_header_key_cannot_mint_the_wrapped_token`,
`c1_the_header_key_cannot_take_the_mint_authority`, `h1_a_stranger_cannot_init`,
`h2_a_leaked_secret_cannot_burn_the_deposit_with_a_cheaper_one`, `m2_the_sponsor_amount_is_capped`,
`m3_a_definition_registered_twice_is_refused`; the seventh, `c1_a_stranger_cannot_mint_or_rotate`,
is the old behaviour kept as a control).

## Run on a throwaway rc3 chain, (1)

`lez-bridge/scripts/e2e-security.sh` on a loopback LEZ v0.4.0-rc3 chain (unpatched rc3 sequencer,
fresh genesis and keys, channel `0xd416…f31d`) and a local anvil vault, build host, 2026-11-14. Log:
`lez-bridge/scripts/e2e-security-rc3-20261114.txt`. Every negative step was sent with the hidden
`--allow-revert`, so it was included and reverted on chain (a failure before inclusion counts as
unexpected); the two H1 cases were also sent without it, to show the program's own refusal text in
the wallet's simulation (the others are identified by their on-chain revert and the unit tests). Result:
**0 unexpected results.** The chain, anvil and relayer were stopped and deleted afterwards.

**The programs before the fixes** (the live v2 rc3 images: bridge `7758856e…675f`, token
`a3ab615c…487b`, sha256-checked copies of the live ELFs), deployed at a fresh header `3Tnzz…Kyt3`:
- the header account signs an ordinary public transaction: a native transfer from it, block 18
  (and again block 981; header nonce 3);
- **C1 exploit, confirmed:** the header key signs token `Mint` for the wrapped definition: applied in
  block 982, supply 0 → 1,000,000,000,000, credited to the header account; then `SetAuthority`:
  applied in block 988, the mint authority moved to the deployer's ordinary account.

**The fixed programs** (bridge image `ae988739…0c0b`, token `cdf31d51…dd7b`; compared by hand with
the old deploy's image ids above: the script's own comparison printed `old: ?` in this run because
the old ids were saved only on a fresh old deploy, and it now fails when they are unknown), fresh
header `GUmAg…AGUe`:
- H1: a stranger's `Initialize` reverted (block 1007; simulation text "only this program's header
  account initialises the bridge"); the retired `Init` signed by the header key reverted (1008;
  "Init is retired"); the CLI refused an init without the header key and one naming another chain
  id; the header key's `Initialize` (sent through the same raw encoding, as its positive control)
  applied (1009) with the deployer's roles, `lez_chain_id` = the chain's channel id, `zone_id` = the
  stand-in source id `0x4bd1…2afb`, and `cfg_id` `0xdd5c…ab7e` equal to the anvil vault's `cfgId`
  (vault built with those two values); a second `Initialize` reverted (1010).
- M2: `SetSponsor` 50,000,001 reverted (1012); 50,000,000 applied (1013).
- C1: the header key's `Mint` (1015), `SetAuthority` to an account (1016) and renounce (1017) all
  reverted; supply stayed 0, authority `Program(bridge)`. Positive controls of the same encodings on
  an account-authority definition applied (1018–1020: create, mint 5, rotate), and `register`
  refused that definition ("not the bridge program").
- H2: on anvil, V deposited 1000 with secret `k`; A deposited 100 with the same `k` for A. A claimed
  first (1022); **V's claim then applied (1023)**: V holds 1000, A 100. The repeat was refused by the
  CLI pre-check ("already claimed", which reads the nullifier PDA the client computes with the new
  formula) and reverted on chain (1024).
- Relay: a never-used account X (absent, 0 native) claimed a third deposit with `claim --relay`
  through `public-demo/relayer/relayer.py` (payer bound to this chain with `--bind-payer`): applied in
  1027; X holds 100 wrapped and 0 native LEZ; the payer paid 1,208,792. Repeats refused.
- Burn of 100 applied (1028); counters `claims [2, 1] burns [1, 0]`, claimed 1,200,000,000, burned
  100,000,000.
- Outbound fixtures `lez-bridge/outbound/fixtures/secfix/` are these blocks 1021–1028; the test
  `the_fixed_programs_nullifier_pda_is_the_key_g2_derives` checks that the key G2-lite derives for
  each public claim (1022, 1023, 1027) is the nullifier PDA the fixed program spent.

CI: green on run `43612552495` (vault forge suites with the regenerated `OutboundVector.sol`; token,
bridge and attestation tests; `bridge_client`; `release_core`). On the build host: CLI unit tests
(15, including the relay's), the release workspace (`release exit 0`), the outbound workspace (17 passed, 1 ignored as before,
`the_fixed_programs_nullifier_pda_is_the_key_g2_derives` included; `OutboundVector.sol` regenerated
byte-identical to the committed one). Not run: the core module's tests (`modules/`), the web page's tests (unchanged code).

Not run on a chain: private claims (no proving), the attested path (`InitializeAttested`; simulator
only), the H1 front-run as a true race (the stranger's `Initialize` was sent before the deployer's;
the refusal does not depend on ordering).


## What the live v2 demo is still exposed to (plain statements for disclosure)

The public demo chain runs the programs from before these fixes, bound to the Sepolia v2 vault,
which can never release (its update image id is a placeholder), so no Sepolia funds can leave the
vault through any of this. On that chain:

1. **Whoever holds the v2 bridge's header key can mint unlimited wrapped tokens and take over the
   wrapped tokens' mint authority** (C1). The key is in the team's deploy wallet (copies on the
   build host). Confirmed on an rc3 chain with the same program images (see above). Effect on the demo: unbacked wrapped
   dUSDC/ETH on the demo chain, and burns of them counted against honest claims.
2. **The bridge `Init` is first-come** (H1). On the live chain the config exists, so this is closed
   there; it reopens only if the chain is re-generated and the bridge redeployed at the same
   header, until the team's own `Init` lands.
3. **Anyone who learns a deposit's claim key `k` before its claim lands can make that deposit
   unclaimable for good** with a cheaper deposit of their own using the same `k` (H2 / review B
   M3). `k` is published by every public claim (after which the deposit is claimed, so harmless),
   and seen by the relay operator (including check-only requests) and by anyone reading a claim
   that reverted or was dropped. Funds stay in the vault (which cannot release on v2 anyway).
4. **The same Sepolia v2 deposit is claimable once per chain instance** (M1): it already was, on
   three chain instances. Only the holder of the v2 header key can deploy at the v2 header; but LEZ
   signatures carry no chain id, so the team's own past deploy and init transactions could be
   replayed on a fresh chain where the same accounts' nonces match (read in the rc3 source, not run).
5. **The admin can set any sponsor amount** (M2) and **register one wrapped definition for two
   Ethereum tokens** (M3). The admin is the team's operator key.
6. **The relay's own issues** (review B): see that section below; the relay fixes are on this
   branch, not deployed.

## Findings

### Review A (programs)

**C1 CRITICAL: the header key is a mint authority. Confirmed, on a chain (see above). Fixed.**
- Cause: on LEZ rc3 the program id is its loader header's account id (`AccountId::from(&PublicKey)`), an
  ordinary account whose key the deployer holds; nothing in rc3 stops it signing (`check_signers`
  only matches signers to nonces and authorized accounts). The token stored the authority as a bare
  account id, so `Mint`/`SetAuthority` signed by that account passed as "the authority".
- Fix (`lez-bridge/token/src/lib.rs`): `Authority::{Account(id), Program(id)}`. A program authority
  mints only through `MintBy`, whose sender program the runtime stamps (never a signature), and can
  never be rotated or renounced; an account authority never mints through `MintBy`.
  `create-wrapped` makes `Program(bridge)`; `register` reads the definition back and refuses any
  other authority (the bridge cannot read a definition itself). Also: `BurnWithNotice` refuses a
  notice addressed to the token program itself (review L4).
- Tests (2): `c1_the_header_key_cannot_mint_the_wrapped_token`,
  `c1_the_header_key_cannot_take_the_mint_authority`, `c1_a_stranger_cannot_mint_or_rotate`,
  `c1_the_bridge_program_still_mints_by_program`, `c1_an_account_typed_bridge_authority_is_not_the_program`
  (`bridge/tests/review_2026_11_14.rs`); token unit tests
  `the_program_ids_own_key_cannot_mint_a_program_authority_definition`,
  `the_program_ids_own_key_cannot_rotate_a_program_authority`, `a_program_authority_cannot_be_renounced_either`,
  `an_account_authority_cannot_mint_by_program`, `a_self_minting_definition_with_a_program_authority_is_refused`,
  `an_account_authority_still_rotates_to_a_program`, `a_burn_notice_to_this_token_program_is_refused`,
  `no_own_program_message_has_a_burn_notice_length`. (1): `e2e-security.sh` steps `c1-mint`,
  `c1-setauth`, `c1-renounce` on the fixed programs.

**H1 HIGH: first-come `Init`. Confirmed (simulator; the code path is the same on chain). Fixed.**
- Fix (`lez-bridge/bridge/src/lib.rs`): `Initialize` / `InitializeAttested` (new variants 6 and 7)
  must be signed by the program's own header account: the instruction reaches that account's own
  actor of the bridge, so `receiver.account_id == program`. The deployer alone holds that key, so
  nothing can front-run it, at a first deploy or at a redeploy at the same header. The roles come
  from the instruction (admin explicit) and may not be the header, so after init the header key
  has no power over the bridge. `Init` / `InitAttested` are refused for everyone (kept in the enum
  so older blocks decode). LEZ rc3 offered no deploy-and-init in one transaction (one root call per
  transaction) and an image constant would tie the image id to one deployer; the header signature
  needs nothing new from LEZ.
- Residual: LEZ signatures carry no chain id, so the whole deploy + `Initialize` sequence can be
  replayed verbatim on another chain where the same accounts' nonces match; that recreates the same
  deployment (same roles), not a hijack. See M1.
- Tests (2): `h1_a_stranger_cannot_initialize`, `h1_the_retired_init_is_refused_even_from_the_header`,
  `h1_a_stranger_cannot_initialize_attested`, `h1_the_header_takes_no_role_and_inits_once`;
  G2's view refuses an applied `Initialize` not signed by the header. (1): steps `h1-stranger`,
  `h1-legacy`, `h1-again`, `h1-cli-*`.

**H2 HIGH: nullifier `H(nfc | k)` lets a leaked `k` burn a deposit. Confirmed. Fixed.**
- Fix: `nf_c = SHA256("RFP022/nfc" ‖ k ‖ dest ‖ token ‖ u256(denom))`. It binds what the commitment
  binds except `cfgId` (the nullifier PDA is already the program's own). Only `dest` can spend it,
  because a claim's `dest` is its signer. It composes with the outbound claim accumulator (keyed by
  the nullifier PDA; G2 computes it from a public claim's instruction with the root call's account as
  `dest`, and takes it from the `SpendClaim` PDA for a private claim) and with the failsafe's G4
  (which already has `dest`, token and denomination as private inputs, `research/failsafe-design.md`
  §2.7). Every place it lives was changed together: bridge `claim_nullifier`, `client/src/scan.rs`,
  CLI (`main.rs` plan, `relay.rs` pre-check), `outbound/core/src/view.rs`, the KDF vector
  generators and their JSON (regenerated: only the `claim_nullifier` fields changed), DESIGN §2.
- **The Ethereum vault is not affected:** it never computes `nf_c` (no match in `vault/src`); its
  `CommitmentUsed` rule still makes the nullifier one-to-one with deposits. The web page never
  computes it either.
- Tests (2): `h2_a_leaked_secret_cannot_burn_the_deposit_with_a_cheaper_one`,
  `h2_the_nullifier_binds_destination_token_and_denomination`,
  `h2_attested_a_second_deposit_with_the_same_secret_cannot_burn_the_first`; the existing
  `a_claimed_deposit_is_refused_in_every_retry_shape` now asserts the reuse case claims once; outbound
  host test `applied_transactions_the_bridge_would_refuse_stop_the_update` (two signers, two keys).
  (1): steps `h2-attacker`, `h2-victim` with two anvil vault deposits.

**M1 MEDIUM: `cfg_id` binds no LEZ chain. Confirmed (live config zero; same leaves minted on three
chain instances). Fixed as far as rc3 allows.**
- A program cannot read its chain's identity on rc3: the Bedrock channel id is in the
  `sequencer_stake` config actor (written once at genesis by `InitChannelParams`), and neither that
  program nor the clock (block id and time only) answers another program. So: `Initialize` requires a
  non-zero `lezChainId`, defined as the chain's channel id and bound into `cfgId`; the CLI reads the
  chain's channel id from that config (`lez-bridge chain-id`), refuses an `init` naming another, and
  refuses a claim on a config naming another chain.
- Residual trust: the deployer (the only one who can deploy at the header) must not initialise a
  second chain instance with the same declared id, and the verbatim-replay case above. In standalone
  mode (no Bedrock node: the demo chain) the channel id is a configured value, not a unique channel.
  This changes the `cfgId`, so it applies to the next vault (v3).
- Tests (2): `m1_initialize_needs_the_lez_chain_id`. (1): `chain-id` and the `h1-cli-chain` refusal.

**M2 MEDIUM: float drain / unbounded `SetSponsor`. Confirmed. Fixed.**
- `SetSponsor` is capped at `MAX_SPONSOR_AMOUNT` = 50,000,000 (in the image; the demo sets 25M, a
  private claim cost ~9.1M on the local sequencer). One claim can no longer take the float. A drain
  still needs one real deposit (and, for a private claim, one proof) per draw, and each draw is at
  most the cap; a claimer with its own LEZ can still claim when the float is empty
  (`--reveal-recipient`). Float liveness stays an operator duty (top-ups).
- Tests (2): `m2_the_sponsor_amount_is_capped`, `m2_one_claim_draws_at_most_the_cap`. (1): `m2-over`, `m2-cap`.

**M3 MEDIUM: duplicate definition in `RegisterToken`. Confirmed. Fixed.** `RegisterToken` refuses a
definition already registered (matching G2's view, which always refused it). Test (2):
`m3_a_definition_registered_twice_is_refused`.

**M4 MEDIUM: attested mode trusts the first init's checkpoint; fork table fixed at 8. Partly fixed,
rest documented.**
- Fixed: init is the deployer's alone (H1), and the root source is bound into `cfgId`
  (`zoneId = source_id = SHA256("RFP022/source/v1" ‖ borsh(root source))`). The instance id commits
  to the checkpoint, fork schedule, genesis validators root and participation policy
  (`instance_id = SHA256(tag ‖ program ‖ borsh(InstanceConfig))`), so a v3 vault names on Ethereum
  the one instance its deposits are claimed through, and anyone can check that checkpoint against
  Ethereum before depositing.
- Not fixed (documented): the attestation program still accepts any checkpoint at instance creation
  (no proof it is on Ethereum; checking it needs an external source), records the checkpoint's own
  execution anchor as finalized at `Open`, and has an 8-entry fork table. At the first Ethereum fork
  after Gloas an instance stops verifying; the path is a new instance, so a new source id, a new
  `cfgId` and a new vault.
- Test (2): `m4_the_root_source_is_bound_in_the_cfg_id`.

**LOW (review A).** L4 fixed (above). L1 (stand-in roots are permanent; `SetAttestor` doesn't revoke),
L2 (32 token kinds pushed to an account), L3 (relay pays for a revert), L5 (`RegisterToken` can't
check supply; `Init` can't check immutability) are unchanged and remain as the review describes;
L3 is narrowed by H2's fix and the relay's in-flight tracking.

### Review B (claim relay; fixed, deployed 2026-11-14 14:13 with the relay's switch to v3)

- **M1** payer-nonce collision dropped a claim and locked the claimer out: the relay keeps one
  co-signed claim in flight and answers `busy, retry` (`retry_after` 15 s, no counter charged); the CLI
  waits and rebuilds (6 tries); a claim is final (`submitted`) only once it is in a block, and becomes
  `dropped` (slot given back, at most 3 per deposit) when a nonce moved past it; `--release` checks the
  chain. Tests in `public-demo/relayer/test_relayer.py` (`test_m1_…busy_not_dropped`,
  `test_a_dropped_submission_…`, `test_drop_evidence_…`, `test_a_stuck_…`, `test_release_…`).
- **M2** cross-genesis replay of co-signed claims: `relayer.py --bind-payer` binds the payer to one
  chain (channel id + genesis header hash), only at nonce 0; the relay refuses to start unbound or on
  another chain; `demo.sh new-relay-payer` makes a new key per chain. The LEZ rc3 message hash has no
  chain id, so the claim itself cannot bind the chain (residual).
- **M3** = H2 above; the relay's check-only and refused paths write no secret, instruction or
  transaction to its log or state.
- **M4** per-IP caps: IPv6 keyed on its /64 (relay and proxy); `CF-Connecting-IP` trusted only with the
  proxy's shared secret header; rolling 24 h windows.
- **M5** proxy client table exhaustion: a new client takes a global token before it gets an entry, and
  a full table evicts the oldest entry instead of answering 503 (`ClientTableTest` in
  `public-demo/proxy/test_proxy.py`; the registry session's kit has the same code).
- **LOWs:** worst case restated (L1, see DESIGN §3.6), CLI queue cap (L2), state pruning (L3), a lost
  sequencer answer is `uncertain` (L4), files mode 600 and client tags under day keys deleted after 2
  days (L6), fixed client texts (L7).
- Deployed relay binary provenance: `700bda76…` is byte-identical to the 11-13 `build-release.sh 1`
  output, sources matching commit `7370ff3`; RUNBOOK §11/§13 corrected. Swap steps: RUNBOOK §13.2.
- Python: relayer 30 passed, proxy 43 passed (`/usr/bin/python3` 3.9). On throwaway rc3 chains, (1): the
  new relayer with this branch's CLI and the fixed programs (`e2e-security.sh`, above), and the new relayer with
  the DEPLOYED CLI and the old programs, i.e. RUNBOOK §13.2 stage 1 (`public-demo/relayer/e2e-relay-stage1-rc3-20261114.txt`:
  bind, relayed claim applied, repeat refused, replay not charged, a claim reverted by a freeze cost the payer
  1,504,304, a second deposit relayed). **Not run on a chain:** two relayed claims in one block (M1's
  busy/rebuild path), the per-IP /64 keys and the proxy's client table (unit tests only).


## What changed in formulas and ids

- **Claim nullifier:** `SHA256("RFP022/nfc" ‖ k)` → `SHA256("RFP022/nfc" ‖ k ‖ dest ‖ token ‖ u256(denom))`
  (32 + 10 + 32 + 32 + 20 + 32 = 104 bytes hashed with the tag). Nullifier PDA seed unchanged
  (`SHA256("/RFP022/bridge/claim-nullifier/v1" ‖ nf)`).
- **cfgId:** formula and vault code unchanged; inputs change meaning: `lezChainId` = the LEZ chain's Bedrock
  channel id (non-zero, declared at `Initialize`), `zoneId` = `source_id(root source)`
  = `SHA256("RFP022/source/v1" ‖ 0x00)` for the stand-in, `SHA256("RFP022/source/v1" ‖ 0x01 ‖ attestProgram ‖
  instance ‖ u64le(slot))` for RFP-021. New CLI commands print both (`chain-id`, `source-id`).
- **Instructions:** new `Instruction::Initialize` (variant 6) and `InitializeAttested` (7) with an explicit
  `admin`; `Init` (0) and `InitAttested` (4) refused. `Config`'s layout is unchanged (readers of the live config
  still decode it).
- **Token definitions:** `authority: Option<Authority>` (`0` Account / `1` Program + id) instead of
  `Option<AccountId>`; `NewFungibleDefinition.mint_authority` and `SetAuthority.new_authority` likewise.
  Holdings are unchanged, so balance readers are unaffected.
- **Constants:** `MAX_SPONSOR_AMOUNT` = 50,000,000; `SOURCE_DOMAIN` = `RFP022/source/v1`.
- **Image ids** (built on the build host from this branch, rc3): bridge `ae988739c7fe838820462fd8f591470ba5798c55542fa43c6ffec40bd5560c0b`
  (was `7758856e…675f`), token `cdf31d511e060cac0f48f95377c3371adde078ff15b2d7ee7eb50e135f3cdd7b` (was `a3ab615c…487b`),
  G2-lite update (TrustedTip) `0xb985132a169aa1b420659517b1ebc410c4e03c68c2ef4e01bf23e3767ae4bbf8` (was `0xffc5ff50…1040`).
  The attestation program and the release guest are unchanged. These are local builds, not a reproducible build.
  The canonical v3 build (commit `c9ec880`, tracked `Cargo.lock`) gave bridge `0dabbd6c…9ffa`, token `ac175cb5…afba`, update
  `0x6b969629…1bd6` from the same program sources (`public-demo/V3-PLAN.md` §1 says why ids move); `OutboundVector.sol`
  keeps the working build's update id, which is only a test value there.
- **Vectors regenerated:** `lez-bridge/client/tests/vectors/kdf-v1.json` and `kdf-names-v1.json` (only the
  `claim_nullifier` fields), `bridge/tests/fixtures/conservation_trace.json`, `vault/test/fixtures/OutboundVector.sol`
  (G2-lite journals over the same real public9 blocks, now with the new nullifier keys; the blocks were made
  by the old program, so for those old claims the keys G2 now derives are not the PDAs the old program wrote:
  a test-fixture artefact, noted in `outbound/host/tests/outbound.rs`). `vault/test/Vectors.t.sol` (cfgId,
  leaves, tree) is unchanged.

## What a v3 deployment needs

1. **New programs at a new header** (new program id): build the guests from this branch, deploy token and
   bridge immutable with a fresh deploy key, `create-wrapped` (Program authority) for each token.
2. **Pick the chain identity:** `lez-bridge chain-id` on the chain that will carry v3 (its Bedrock channel
   id; on the standalone demo chain this is a configured value, so it must be fresh for that genesis).
3. **Pick the root source**, then `lez-bridge source-id` (stand-in, or `--attest-program --instance` for
   RFP-021; the instance fixes the checkpoint and fork table, so verify that checkpoint against Ethereum
   before deploying the vault).
4. **Deploy the v3 vault** with `lezChainId` = step 2, `zoneId` = step 3, `programId` = step 1
   (`vault/script/SepoliaDemoV3.s.sol` hardcodes zero for both today, lines 151-152: it must take them as
   inputs; not changed here, the v3 plan is the owner's), and check `cfgId` against
   `lez-bridge cfg-id`.
5. **`init` / `init-attested`** from the deploy wallet (the header key signs; `--payer` pays; name the
   admin, freezer and attestor, none of them the header), then `register` (refuses a definition the bridge
   does not mint), `set-sponsor` (≤ 50,000,000), `fund-float`. After that the header key has no power and
   can be destroyed; keeping it only lets its holder deploy the same program id on another chain (M1).
6. **Relay:** a new payer key for that chain, bound with `relayer.py --bind-payer` (RUNBOOK §13.2), and the
   new CLI only together with the new programs (the new CLI cannot decode the old token definitions).
7. **Clients:** the deposit page and the module need the new vault address and cfgId; the claim tool must
   be the new CLI (new nullifier). Deposits into the v2 vault stay claimable only on the old programs.
