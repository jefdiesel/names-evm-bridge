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
- **No admin timelock today:** neither the vault nor the LEZ bridge enforces a timelock on changing
  the freezer (`setFreezer`) or lifting freezes (`unfreeze` / `unfreezeAll`), so a compromised admin
  can unfreeze and replace the freezer immediately. M4 adds an admin timelock.
- **Reproducible guest builds:** local guest builds embed machine/registry paths, and reproducible
  guest builds with RISC Zero's Docker builder are not done yet (set up in M1).

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
