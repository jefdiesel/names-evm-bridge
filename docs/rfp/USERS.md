# Using the bridge: what is public, what you trust, how to recover

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

## What this document answers (the RFPs' own words)

- **RFP-022 Usability #7:** "Documentation and UI must clearly explain what is public and what is private at each step
  on both chains, so users can judge their own exposure."
- **RFP-022 Security #7:** "User-facing documentation must state the trustless verification model and the
  liveness-only role of any off-chain participant (see Design Rationale, "Trust model")."
- **RFP-021 Verification Security #5:** "User-facing and developer-facing documentation must state the trustless
  verification model and the liveness-only role of any off-chain participant …" (the user-facing half; the developer
  half is [LIGHT-CLIENT-SPEC.md](LIGHT-CLIENT-SPEC.md)).
- **RFP-022 Reliability #13** (user half): "Provide documentation in repo for the CLI and any operator-facing
  components, covering the core user and operator journeys respectively."
- **RFP-022 Privacy Preservation #4:** "Document every component that handles user data; such information must be
  absent from all submitted transaction data." **#6:** "Where a third party submits on the user's behalf, document
  precisely what that party learns …"
- **RFP-022 Functionality #9 / Reliability #5** (how recovery works for a user): "A user must be able to recover every
  one of their own unclaimed deposits and unreleased burns from credentials they already hold …"

## Status

- **Complete for what runs**, with every flow marked by where it ran. This is the text the wallet and the page should
  point to; the in-app wording (the mini-app's privacy indicator, `modules/lez_bridge/rust-lib/src/privacy.rs`) is
  TESTED, not run by users.
- **RUN on public networks:** deposits on Sepolia through the page and the Names Wallet; public and private claims and
  burns, and releases, on our public chain and Sepolia (`v4-round-trip`, `v5-private-round-trip`, `v6-round-trips`,
  `v6-wallet-claim`, `v7`).
- **RUN locally:** recovery from the phrase alone, 45 of 45 deposits and 30 of 30 exits (record
  `public-demo/prove/privacy-test-20261117/README.md`, development receipts).
- **Not in a released build:** the bridge inside the Names Wallet (test builds only); private claims and burns from the
  wallet (branch `private-in-wallet`); recovery of imported accounts (branch `imported-accounts`).

## 1. What you trust (plain words)

- **Nobody who sends a transaction for you can redirect it.** Where your wrapped tokens go is fixed inside your
  deposit's commitment; where your ETH or tokens go on the way back is fixed inside your burn's exit commitment. A relay
  or service that sends a transaction for you can only send it or not send it.
- **The keys below could still cost depositors money if misused.** A lying deposit-root key (v5) can mint wrapped tokens
  with no deposit behind them on LEZ; a lying update key (v5, v6) or two of the three v7 keys can let made-up burns be
  released, up to what is deposited and not yet released, within the caps.
- **Some of it is still our word, and which part depends on the vault you use:**
  - **v5, the default today:** a key we run tells LEZ which Ethereum deposits exist. If that key lied, wrapped tokens
    with no deposit behind them could be minted on LEZ. And one key of ours decides which LEZ blocks a withdrawal
    covers.
  - **v6:** no key on the way in. LEZ checks Ethereum itself through the light client, which trusts Ethereum's sync
    committee (two thirds of 512 validators) and the checkpoint it started from. The way out is as v5.
  - **v7:** the way in as v6; on the way out 2 of 3 signer keys, all three ours, and a waiting window in which a
    double-signing key can be thrown out. Today's signers check neither privacy receipts nor public execution yet, so
    with our stand-in keys on our one sequencer, v7 still trusts us.
  - **All of them:** we run the only LEZ sequencer; the chain is our test chain, not a Logos network, and not anchored
    to the Logos Blockchain. Nothing is audited. Test funds only.
- **What limits a mistake or an attack:** a vault never pays out more than was deposited, per token; caps limit what
  can leave in a day; a freeze can stop deposits, claims, burns or releases per token, on each chain separately. A
  freeze cannot single out one person: the bridge does not know whose funds are whose.
- **Helpers affect only whether and when, never where.** Relays, the light-client updater, the release relayer and our
  settlement service can delay you, not redirect you. Today we run each of them. If a relay stops, you can claim paying
  your own LEZ fee; if the settlement service stops, you can release with a proof you make yourself. Two roles are not
  replaceable today: if our deposit-root key (v5) stops, claims on v5 wait; if our update key (v5, v6) stops, exits on
  those vaults wait. Your deposit stays in the vault and your claim stays valid meanwhile. Detail:
  [SECURITY-AND-TRUST.md](SECURITY-AND-TRUST.md) §8.

## 2. What is public and what is private at each step

| Step | Public on Ethereum | Public on LEZ | Not published anywhere |
|---|---|---|---|
| **Deposit** | your Ethereum address, the token, the denomination, the time, a commitment `C`, the vault's new root | — | your LEZ destination, your secret |
| **Claim to a private account** (the mini-app's default, TESTED; in the CLI, a private `--from`) | — | the anchor (light-client vaults) or the root (v2–v5), the instance, the vault, `cfgId`, a claim nullifier, a one-time address, the fixed sponsor amount, the token and the minted amount (one denomination), the time | your account, which deposit it was, your secret |
| **Claim to a public account** (the mini-app asks for an explicit choice, TESTED; the CLI claims publicly whenever `--from` is public) | — | all of the above **plus your account and your deposit secret `k`**: anyone can link your Ethereum deposit to your LEZ account | — |
| **Holding and sending privately** | — | a commitment and an encrypted note per private transfer, and its LEZ fee (amount, payer alias; `DESIGN.md` §8.2) | from, to, amount |
| **Burn from a private account** | — | the token, the denomination, an exit commitment `E`, the time | your account, your Ethereum destination |
| **Burn from a public account** | — | your account, the token, the denomination, `E`, the time | your Ethereum destination |
| **State update** (by the operator) | the new exit root and the exit leaves | — | which burn is yours |
| **Release** | the recipient address, the amount after fees, the relayer and its fee (0 or a menu value), the exit root, a release nullifier, the time | — | which burn it pays |

What narrows the crowd you hide in:
- **Token and denomination:** you are one of the deposits (or releases) of the same token and size.
- **Time:** claiming soon after depositing narrows it. In our measurement (20 users, local chains, development receipts),
  when users claimed soon after depositing, "the latest earlier deposit" was right 6 times in 20 against about 3 by
  chance. So wait before claiming: the mini-app suggests a claim time 24 h after the deposit and warns below 10 deposits
  of the same amount.
  A random 1–24 h delay by default is designed, not built (M5).
- **Repeat claims:** a claim from an account that already holds private state is larger, so repeat claimers can be told
  from first-time ones (same measurement). Not fixed yet.
- **Your IP address:** the relay, the RPC endpoints and the deposit page's host see it. Nothing here hides it.

## 3. How to

Commands are the `lez-bridge` CLI (`lez-bridge/cli`; `lez-bridge <command> --help` for every flag). It talks to loopback
chains and to exactly our public endpoint.

### Deposit (Ethereum)

- **The deposit page** (`https://nameswallet.app/bridge/`, `web/deposit/`): connect an Ethereum wallet, pick token and
  denomination, give your LEZ destination (an account or an LNS name such as `alice`; note that an LNS name resolves to
  its holder's published **public** account, so claiming an LNS-name deposit is linkable by construction). The page makes
  the secret in your browser tab and gives you a **claim file**: keep it, it is the only copy (§4). RUN on Sepolia.
- **From the Names Wallet:** "Receive from Ethereum" gives a deposit code (`logosdep1…`) whose secret your wallet
  derives from your phrase; paste it on the page. RUN on our public network with test builds (`modules/NAMES-RECEIVE.md`).
- **From the CLI:** `lez-bridge deposit-request --bridge … --from … --eth-token … --denom-index …` prints the commitment
  and a deposit request for the page; no secret is printed.

### Claim (LEZ)

- **Names Wallet, "Claim a deposit":** into a public account, the bridge's relay paying the LEZ fee. RUN with test builds
  (LEZ blocks 4852, 5084, 5249, 27401). A private claim from the wallet: on branch `private-in-wallet`, not released.
- **CLI, private (the default for a private `--from`):** `lez-bridge claim --bridge … --token-program … --from <private
  account> --eth-token … --denom-index … --denom … --key-index N`. Your machine proves it: about 11 minutes and 9.5 GB of
  memory on a 16 GB desktop (`DESIGN.md` §9.0a); the proposal (§10) says 8 GB will not work. The claim mints to a one-time
  address of your account; the bridge's float pays the LEZ fee.
- **CLI, public:** the same from a public account; add `--relay` for the relay to pay the fee. This publishes your
  deposit secret (§2).
- **Light-client vaults (v6, v7):** `lez-bridge claim-attested …` with an `eth_getProof` of the vault's root under an
  anchor the light client finalized; wait for the next light-client update after your deposit is final (Ethereum
  finality, then up to 30 minutes on our chain).
- **If it fails:** a claim that depends on your deposit fails with one text, `claim refused`, whatever the reason; the
  CLI tells you the reason locally first and never sends it.

### Bridge back to Ethereum

- **Burn:** `lez-bridge burn --bridge … --token-program … --from … --definition … --amount <one denomination>
  [--eth-dest 0x…] [--relay]`. Without `--eth-dest` the release address is derived from your account's key (a fresh
  address; `lez-bridge eth-key` writes its private key to a mode-600 file). From a private account the burn is proved
  on your machine. **Check the address:** a contract without a receive function, or an address the token issuer
  blacklisted, can never be paid, and nobody can change it afterwards.
- **Names Wallet, "Send back to Ethereum":** a public account's burn to an address you connect on the page; RUN with
  test builds; the downloadable build stops at Approve, with no release prover (proposal §8;
  `modules/NAMES-SEND-BACK.md`).
- **Wait** for a state update to cover your burn (on v5 every two to three hours, `docs/OPS-V5-V6.md` on branch `v6-ops`;
  v6 has no standing updater; v7 adds its contest window).
- **Release, two ways:**
  - **Our settlement service** proves, wraps and sends it when your wallet sends it your exit secret. It then knows which
    burn pays which address. The wallet asks first.
  - **Yourself: not packaged for users yet.** `lez-bridge exit-secret … --out F` writes the release proof's witness to a
    mode-600 file; the release proof itself (`outbound_host release --prove`, about 20 s and 1 GB) runs today through
    the owner's scripts on the build host, and its Groth16 wrap needs an x86-64 Linux machine with Docker
    (`lez-bridge/outbound/wrap`). With a seal, anyone can send the vault's `release`; the page's `#release` view sends
    one from a release code (`logosrel1…`). A user-side prover path is M4–M5.

## 4. Recovery from your phrase

- **What is recoverable:** every deposit and exit whose secret was derived from your wallet (the CLI's `--key-index`,
  the Names Wallet's deposit codes and burns). `lez-bridge recover --bridge … [--eth-dest 0x…] [--account-gap N]`
  restores them from your accounts' keys and the two chains alone, with no local state and no server index; `positions`
  shows the last result. With the Names Wallet's phrase add `--names-app lez_bridge`. RUN locally: after the wallet
  folder was deleted, 45 of 45 deposits and 30 of 30 exits found, and each claimed and released (record
  `public-demo/prove/privacy-test-20261117/README.md`, development receipts).
- **Exits to an address you chose** need that address again (`--eth-dest`); exits to a derived address need nothing.
- **Imported accounts** (an account imported into the Names Wallet, not made from its phrase): derived from the phrase
  and the account id under a separate scheme; the CLI's `recover --account` tries it for a named account and for the
  signers of the bridge's transactions (`proposal/IMPORTED-ACCOUNTS-DESIGN.md`; branch `imported-accounts`, TESTED, not
  on `main`, not released).
- **Not recoverable from the phrase:** deposits made on the page with a claim file (the secret was made at random in the
  tab: keep the file), and deposits made with a raw `--secret`.
- **What the scan reveals:** the Ethereum RPC sees one vault-wide log query; the LEZ sequencer sees block downloads.
  `--lez-mode point` is faster but tells the sequencer which nullifiers you watch.

## 5. Components that handle your data (PP4, PP6)

| Component | Holds or sees | Where |
|---|---|---|
| Your wallet / the CLI / the core module | your phrase-derived secrets, in memory and in mode-600 files the module deletes after a call; never in a reply or a log | your machine |
| The deposit page | the claim file's secret, made in your tab; kept nowhere else | your browser |
| The claim relay | your claim before it lands (a public claim's secret `k`), your IP | ours today |
| The settlement service | your exit secret if your wallet sends it, so which burn pays which address; your IP | ours, only if you use it |
| The GitHub-hosted wrap runner | the release receipt it wraps (it names the recipient) | GitHub, only if a service or you use it |
| RPC endpoints (Sepolia, LEZ) | your queries and IP | the providers you pick (the wallet lets you change them) |
| Your transactions | nothing that links the two legs (§2), except a public claim's secret | both chains |

## 6. Not yet shipped

A released Names Wallet build with the bridge; private flows in the wallet; a random claim delay by default; a padded
claim size; the bridge on LEZ testnet 0.4 (not live); an audit.

## Sources

`DESIGN.md` §3, §4, §8, §10; `docs/EVIDENCE.md`; `public-demo/prove/privacy-test-20261117/README.md`;
`lez-bridge/cli/src/main.rs`, `recover.rs`; `modules/NAMES-RECEIVE.md`, `modules/NAMES-SEND-BACK.md`;
`web/deposit/README.md` (the page's security trade-offs, RFP-022 Soft #5); `proposal/IMPORTED-ACCOUNTS-DESIGN.md`
(branch `imported-accounts`); `public-demo/RUNBOOK.md` (not published) §22.
