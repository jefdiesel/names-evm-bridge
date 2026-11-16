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
