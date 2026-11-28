# Security

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

**Report a vulnerability privately.** E-mail bridge@nameswallet.app and please do not open a public
issue for a security problem. Say what you found, how to reproduce it, and which deployment or commit it
affects. We will answer and agree a disclosure date with you.

**What is deployed:** test networks only. The vaults are on the public Sepolia testnet and the LEZ
side runs on our own LEZ v0.4.0-rc3 test chain (`vault/DEPLOYMENTS.md`, `DESIGN.md`). They hold test
coins with no value. Nothing here has been audited.

No bug bounty is offered.

Known limits are written down rather than hidden: the trust model is in `DESIGN.md` §0 and §7, the
2026-11-14 internal review and its fixes in `research/security-review-2026-11-14.md`.

## Known limits

- **Failsafe (`vault/src/failsafe/BridgeVaultFailsafe.sol`, not deployed):** `windDown()` fixes
  `finalState` on the first update after wind-down without checking that LEZ minting has stopped (on
  today's LEZ program the mint freeze is reversible and G2 does not track it). If LEZ minting is not
  frozen first or is unfrozen after `finalState` is fixed, a late exit can be paid while the same
  deposit is reclaimed; forged mints before the final also consume reclaim room (vault solvency
  still holds). This variant is not deployed; the irreversible LEZ mint freeze in M6 closes this
  limit (`public-demo/prove/failsafe-20261117/README.md`).
- **v7 threshold-signature route (`SeqSigGate`):** today's signers (`seqsig attest`) re-hash and
  re-extract blocks, checking neither privacy receipts nor public execution yet. Vault v7 uses
  stand-in keys of ours on our one sequencer, so v7 still trusts us (real accredited sequencer
  signatures are M6, subject to the week-2 gate).
- **Admin timelock (closed in code for the Ethereum vault, not deployed; LEZ bridge still open):**
  `BridgeVault.sol` on main puts every safety-weakening admin action behind `schedule` → `execute`
  no sooner than an immutable constructor-set `adminDelay` (at least `MIN_ADMIN_DELAY`, 1 hour) and
  before `ADMIN_GRACE_PERIOD` (7 days) more, cancellable by the admin or the freezer: lifting
  freezes, changing the freezer, proposing an admin, changing the fee recipient, raising or
  disabling caps, lowering cap weights or release delays, and raising fees. Freezing and tightening
  changes stay instant, and an unfreeze scheduled before the latest freeze cannot run. The verifier
  and image ids have no setter at all. The deployed Sepolia vaults (v2–v7) are immutable and do not
  have this timelock; it applies to new deployments, and the deploy scripts' delay is a placeholder
  the owner decides before any deploy. Known limit: the freezer's veto has no exception, so a stolen
  freezer key can freeze and then cancel every unfreeze and every change of the freezer, keeping
  the vault frozen (no funds move; the remedy is a new vault;
  `test_KnownLimit_FreezerCanVetoItsOwnReplacement`). The LEZ bridge program (`lez-bridge/bridge`)
  does not have an admin timelock yet.
- **Reproducible guest builds:** reproducible guest builds with RISC Zero's Docker builder are done for the code on `main` (2026-11-28): all 10 deployable guests build with the same image id and ELF sha256 on two clean machines (two fresh GitHub-hosted runners, pre-merge CI run 46095730461; ids pinned in `VERSIONS.toml` `[[repro_guest]]`; `docs/REPRODUCIBLE-BUILDS.md`). **The deployed guests (v2–v7) were built natively before that, and none of their image ids reproduces from Docker**, including the release (`0x6bcbd714…`), trusted-tip update (`0x098f5a1a…`) and v7 seq-sig guests (`0x93c88f77…`, `0x9df4de66…`), whose guest sources and locks on main are the deployed builds' (v7: its branch tip); the bridge and token programs (`0dabbd6c…`, `ac175cb5…`) also changed lock since. Their archived ELFs stay the proving inputs for those vaults; a new deployment uses the Docker ids. Not yet checked on a machine outside GitHub.

Public demo faucet and relay (test network, test LEZ only; found 2026-11-27, fix planned, the
running services unchanged until then; each is pinned by a test in `public-demo/`):

- **RPC filter, malformed bridge name:** a relay call whose `bridge` param is a list or an object
  gets no answer (the handler raises, counted as `handler_error`) instead of a 400. Nothing reaches
  a relay and the filter keeps serving.
- **Faucet `for` is not authenticated:** anyone can name any wallet as `for` and use up that
  wallet's one-time grant; it limits honest clients only.
- **Faucet `for` order:** a grant for wallet W through another account, then W's own account
  asking without `for`, pays twice; the reverse order is refused.
- **Faucet `--release`:** releasing an uncertain transfer forgets the account but leaves the
  wallet named by `for` marked used, so that wallet cannot ask again.
- **Dangling reference:** the filter's and relay's `--relay-bridge` / `MAX_TX_BYTES` notes cite
  `modules/V6-IN-WALLET.md`, which is not in this repository yet.
