# AGENTS.md: LEZ Bridge mini-app and core module

For coding agents (and people in a hurry). Last checked: 2026-11-15 (branch `v4-live-2`: each receive is read on the deployment it was made on, `deployment` on `receives`/`receive_status`, plain words when the relay no longer serves a deposit's bridge; before that `v4-live`: v4 is the default deployment, a vault that releases by proof, with its own release words; before that `receive-request`: `receive_request(id)` shows a stored deposit's code again; `compare` on `receives` and `receive_status`; v3 reads as a vault that can't pay out; before that `compare-words`).
Latest version: <!-- LATEST-URL: set when the repository is published -->

Two Logos Basecamp packages for the private Ethereum ↔ LEZ bridge (RFP-022):

- **`lez_bridge`**, the core module (Rust, `lez_bridge/rust-lib/`). It reads the bridge on LEZ
  and the vault on Sepolia, checks claim files and deposits, builds the calls the Names Wallet
  signs, hands signed claims to the bridge's relay, and keeps this install's deposits. It holds
  no key and never proves; the Names Wallet signs and proves.
- **`lez_bridge_ui`**, the mini-app (QML, `lez_bridge_ui/qml/`): the registry view, the admin
  screen, deposit planning, redeem and exits, recovery, and "Claim a deposit" from a claim file.

Claiming and "Receive from Ethereum" also live inside the Names Wallet, which hosts the core
module (below). The mini-app stays for everything else the RFP asks of a Logos mini-app.

Full detail:
- `CLI-CONTRACT.md` (every call; §8 is the relayed claim);
- `NAMES-EMBED.md` (claiming from another app: Part A today, Part B one confirmation);
- `NAMES-RECEIVE.md` (a deposit whose secret the wallet makes);
- `NAMES-SEND-BACK.md` ("Send back to Ethereum": a public account's burn, the release proof, the release code; §7 is what Names must add);
- `../DESIGN.md` §1.1 (concurrency, what replies carry, busy, the freeze fix).

The public copy of this repository does not carry `NAMES-EMBED.md`, `NAMES-RECEIVE.md` or
`tests/` yet. They are in the working repository.

## The network

| | |
|---|---|
| LEZ | 0.4 (v0.4.0-rc3), the **bridge test network**, run by this project. Not Logos's public testnet. It is reset now and then; a reset wipes every account and can change the ids below. |
| LEZ endpoint and relay | `https://lez.nameswallet.app/` (the relay's methods `bridge_relayInfo` and `bridge_relayClaim` are on the same endpoint) |
| Ethereum | Sepolia (chain 11155111), read through public RPCs with a fallback list (`lez_bridge/rust-lib/src/config.rs`) |

Deployments live in **one file**, `lez_bridge/rust-lib/deployments.json` (Sepolia side:
`../vault/DEPLOYMENTS.md`):

| Deployment | Vault (Sepolia) | Deposit desk (0.3% fee) | Bridge program (LEZ) | Token program (LEZ) |
|---|---|---|---|---|
| `bridge-test-network-v4` (default) | `0xC2c6A059eAB928966B24FA5549d441A3B17B54Ed` | `0x25C1653888e7a4E2c35393A5656b7d1645487fd5` | `BKXe3q5JcKeiSrKv9FUBjZw3AowbvYgyHZ3SYPgDxJBf` | `5w6nTkftFFDy8mtdpnKpZVetGQJ2WCKdhmpJsAYArbU7` |
| `bridge-test-network-v3` | `0x26af59855b99a8af199361289A5Ed6088729E60E` | `0x6b7F38D05751b1ed432a0A0d377A63cFB8c4DBc5` | `4eaeK3yiqATXhpCEYEFMMtjUn1bS7HLxPwfrH31mXCvr` | `5w6nTkftFFDy8mtdpnKpZVetGQJ2WCKdhmpJsAYArbU7` |
| `bridge-test-network` (v2) | `0x8C8b2664De28e5287158db4DB65D448638871b74` | `0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2` | `JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7` | `3gF78HRJNCfabkWFiszahVVacq181aU8SSGua8Cj9ZE4` |

v4 is the default since 2026-11-15. Its vault checks update and release proofs on Ethereum with RISC
Zero's Groth16 verifier 3.1.0, behind an update gate; `release: "proof"`. Its note: "Withdrawals back
to Ethereum are checked by proof on Ethereum. The first proofs are being set up; until then a burn
waits." `release` / `exits` / redeem `blocked` give that note and the order of a payout: a proved
update, then the release proof, then the delay (test `the_v4_vault_says_it_releases_by_proof`).

v3 and v2 stay selectable: `use_deployment("bridge-test-network-v3")` and
`use_deployment("bridge-test-network")`. Neither pays anything out on Ethereum: v2's update image is a
placeholder; v3 was deployed with its outbound
starting state (`g2State`) at zero, so no update can start from it (`../vault/DEPLOYMENTS.md`).
Deposits on both can be claimed on the bridge test network. In `deployments.json` v3 has
`release: "never"` with `release_note` (the one line the apps show) and `release_detail` (the
why); the module gives them as `config().notices.release`, `registry().vault.release_note` /
`release_detail` and in `release` / `exits` / redeem `blocked`, and the mini-app shows them. The
`operator` release kind stays in the code for a later vault; no deployment uses it.

## Rules for agents

- **Never put a private key, seed phrase, wallet file, claim file, claim key (`claimKey`),
  master secret (`masterSecret`) or deposit secret in a prompt, a log, a commit or an issue.**
  A claim file holds two secrets: pass it to the module by path, never paste its text into
  your context.
- **A deposit code is public** (`logosdep1…`, and its `https://nameswallet.app/bridge/#…` link):
  it holds the commitment, never the secret. Sharing it is fine.
- **Never send a transaction or propose a call to a wallet without showing the person first.**
  The calls under "Needs a person" make a request in the Names Wallet, or start a deposit or
  claim. Show what the module answered (amount, account, fee, the "Not private" notice) and let
  the person decide.
- **Reads are safe:** the calls under "Read-only" sign nothing, send nothing and ask no wallet.
- **Busy versus no answer:**
  - `{success: false, busy: true}` means nothing happened. Ask again with a short backoff
    (2, 4, 6 s).
  - No answer at all (a timeout) means the call may have been taken. Look first, through
    `relayed_claims` or the Names app. Never re-propose blindly: after an unanswered proposal the
    module answers `needs: "uncertain"` until the person has looked.

## Build and install

On a build host with Nix:

```sh
modules/build-lgx.sh <out dir> both    # the module, then the mini-app against it; "module" for the module only
```

The script writes the build identity into build-id.txt beside `lez_bridge/rust-lib/Cargo.toml`: this checkout's
short git commit (`-dirty` with local changes) and the UTC date. It builds a copy of the sources,
because Nix builds a git checkout from its tracked files only. A module build without that file
refuses to build (`lez_bridge/rust-lib/build.rs`).

Checks on a built plugin:
- `grep -a lez-bridge-scaffold-try-lock=3` shows the scaffold fix (a call that arrives while
  another waits answers "busy" instead of freezing the module). `config().scaffold` says the same
  at run time.
- `grep -a lez-bridge-build-commit=` shows the commit.

**Telling builds apart:** `config().build` is `{commit, built, version}`, and `features` lists
`build_id`. After an update, read `config().build.commit`: when it shows the new commit, the
running module is the new one. `version` alone is not enough; it stays 0.1.0 across builds.

**Install in Basecamp 0.3.2** (no restart needed):
1. Open Package Manager: the cube icon, then **Install Local Package**.
2. Install the **module** `.lgx` first, then the mini-app `.lgx`. "Unsigned" is expected.
3. Open "LEZ Bridge" from the launcher.

After the Names Wallet is reinstalled in a running Basecamp, an older module build can freeze
(builds before the scaffold fix). In the Module Inspector, unload LEZ Bridge, then unload Names,
then open both again.

## The module's calls

From QML: `logos.callModuleAsync("lez_bridge", method, [strings…], callback, timeoutMs)`. Every
argument is a string. Every reply is `{success, value, error}` as JSON text. **Basecamp 0.3.2
encodes it twice**: parse up to twice (`lez_bridge_ui/qml/logic.mjs` `unwrap`). A reply without
`success` comes from Basecamp's bridge (`timeout`, `Module not connected`, `Method not found`).

### Read-only (sign nothing, send nothing, ask no wallet)

| Call | Reply (`value`) |
|---|---|
| `config()` | Settings, the deployment, `features` (what this build answers), `notices.not_private` (the words before a public-account receive), `notices.release` (`{text, detail}`: the deployment's own words about paying out, v4's or v3's; null for a deployment without them), `scaffold`, `build` (`{commit, built, version}`) |
| `registry()` | Tokens with denominations (`tokens[].denoms[]`: `index`, `value`, `text`, fees), caps, freeze state on both sides, release delay. Reads both chains: once per screen, not a poll. |
| `plan_deposit(token, amount)` | A split of an amount into denominations, with fees and anonymity sets |
| `receives()` | This install's wallet-made deposits with the stage `receive_status` last saw, each with `compare` and `deployment` (`{id, label, current, network}`, the deployment the deposit was made on; `null` + `deployment_problem` for one from an older build); no chain read |
| `receive_status(id)` | `{stage: requested\|deposited\|final\|claimable\|claimed\|unknown, text, confirmations, final_in_min, leaf, block, claimed_block, compare, deployment, relay, action, reads}`, read on the deployment the deposit was made on (`unknown` + `action: "recover"`: an older build kept none; nothing read). For a 60 s poll; no wallet call. |
| `receive_request(id)` | A stored deposit's code again, for a receive made in an earlier session: `{id, request, link, request_url, request_qr, compare, symbol, denom_text, amount_text, chain_id, vault, deployment, stage, text, …}`. Rebuilt from the stored deposit, byte for byte the first reply's code; no wallet call, no secret, no chain read. Refused for an unknown id, or a deposit from an older build that kept no vault (`recover` fills it in): never a guessed code (`NAMES-RECEIVE.md`). |
| `relayed_claim_check_with(claim, "{\"wallet_hint\":false}")` | The six checks of a claim file (path or text): `{ok, id, deposit, relay, checks[], privacy, …}` or `{ok: false, error: {code, message}}`. Nothing proposed. |
| `relayed_claim_status(job)` | `{state, phase: approve\|relay\|confirm\|done, note, tx, block, result, error, needs}`. Poll every 2.5 s, one call at a time. It moves the job on (it may hand a signed claim to the relay); it proposes nothing new. |
| `relayed_claims()` | Every claim job, newest first |
| `send_back_link()`, `send_back_check(code)`, `send_backs()`, `send_back_status(id)`, `send_back_release_code(id)` | Send back to Ethereum (`NAMES-SEND-BACK.md` §5): the page link, a withdraw-to code's address and words, the list, the stages read from LEZ and Sepolia (60 s poll), the `logosrel1…` code. No secret in any reply. |
| `send_back_build(id)` | The burn as the relayed call the Names Wallet signs (golden vector `lez_bridge/rust-lib/testdata/public-burn-v1.json`) |
| `deposits()`, `holdings()`, `accounts()`, `jobs()`, `job(id)`, `exits()`, `admin_status()`, `privacy_report(choices)` | The mini-app's other views (`CLI-CONTRACT.md`). `holdings` and `accounts` ask the wallet for its accounts. |

### Needs a person (makes a wallet request, or starts something)

| Call | What it does |
|---|---|
| `make_deposit_request(token, denom_index, account)` | A private-account deposit code. The first time, it asks the wallet for this app's values (`needs: "secret"`). The reply's `compare` is what the person checks against the deposit page: `first`/`last` (6 hex digits each), `text`, and `words` (4 words) / `words_text` (joined with " · "); below. |
| `make_public_deposit_request(token, denom_index, "Public/<id>", "true")` | A public-account deposit code. Show `config().notices.not_private` first; without `"true"` it is refused. Same `compare` as above. |
| `relayed_claim_check(claim)` / `relayed_claim_check_deposit(id, options)` | The checks. `_check_deposit` re-derives the claim key through the wallet (`needs: "secret"` until granted). |
| `relayed_claim(check_id, options)` | Gates (connect, add an account, declare the programs once), then proposes the claim. The person approves in the Names app. |
| `relayed_claim_build(check_id)` / `relayed_claim_submit(check_id, tx)` | For a host that signs itself (`NAMES-EMBED.md` Part B). The build's `call.message` holds the claim key. |
| `send_back_plan(token, denom_index, "Public/<id>", code)` | Derives the exit secret through the wallet (`needs: "secret"` the first time) and stores the exit (its index, never the secret); refused on a vault that can't release |
| `send_back_burn(id, options)` / `send_back_submit(id, tx)` | Proposes the burn to Names (`needs: "names_update"` until Names has §7) / hands a signed burn to the relay (`bridge_relayBurn`) |
| `send_back_witness(id)`, `send_back_prove(id)`, `send_back_seal(id, seal)`, `set_release_prover(argv)` | The release proof's inputs (secret in a 0600 file, paths only in the reply); the helper prover as a job; the Groth16 seal (checked with a read-only `verify` on Sepolia); the helper's argv |
| `relayed_claim_cancel(job)`, `relayed_claim_forget(check_id)` | Cancels a claim waiting for approval; drops a checked claim from memory |
| `wallet_connect()`, `wallet_request(id)` | Asks for access; follows a request (poll every 2.5 s until `approved`/`done`, then repeat the call that asked) |
| `claim(id, options)`, `redeem(plan_id, options)`, `release(exit_id, relayer)` | Private claim, burn, release (dry runs in this build unless proving is set up) |
| `admin_action({op, …})` | `freeze`, `unfreeze`, `set_sponsor`, `fund_float`, `register_token` as LEZ dry runs (sends are off in this build); Ethereum-side admin ops answer "not available" |
| `use_deployment`, `set_eth_rpc`, `set_eth_proof_rpc`, `set_sequencer`, `set_cli`, `set_wallet_mode`, `set_fixtures`, `import_deposit` | Change settings or local data |

A `needs` reply is `{needs, request: {request: <id>, state, …}, message}` and always carries a
request the wallet really made. Anything else is an error with words that say what to do.

**The compare words** (`compare.words`, `compare.words_text`; on the two deposit request calls, in
both wallet modes, through `deposit_reply` in `lib.rs`, and on `receives`, `receive_status` and
`receive_request`, all from `compare::json`):
`h = SHA-256(ASCII "logos-bridge/compare/v1" | 32 commitment bytes)`; the first 44 bits of `h`,
big-endian, are four 11-bit indexes (bits 0-10, 11-21, 22-32, 33-43) into the BIP-39 English list
(index 0 = `abandon`); `words_text` joins them with space, U+00B7, space. Fixed: the deposit page
computes the same (`web/deposit/src/lib/compare-words.js`). Code `lez_bridge/rust-lib/src/compare.rs`;
the list `lez_bridge/rust-lib/data/bip39-english.txt` (unchanged, MIT, SHA-256 `2f5eed53…24dbda`,
embedded at build; notice beside it); vectors `../web/deposit/test/vectors/compare-words-v1.json`
from `tools/make-compare-words-vectors.py` (python3, independent of both).

## Inside the Names Wallet

The Names app declares `lez_bridge` as an optional dependency, and Basecamp starts the module when
Names opens. Names bundles only the module. The mini-app need not be installed for that.

- The module needs no setup. With no settings file it uses v4.
- It has **no timer**. A claim job moves only while someone calls `relayed_claim_status`, and
  `receive_status` reads only when called. The host's screen polls, one call at a time.
- The wallet keys every request to the app `lez_bridge`, whoever calls the module. A grant or
  declaration made once stays.

## Tests

| What | Where | Needs |
|---|---|---|
| Module unit tests | `cd modules/lez_bridge/rust-lib && cargo +1.98.1 test -j 4` | The build host, with the Logos Rust SDK staged beside `rust-lib` as logos-rust-sdk-src (see `../AGENTS.md`) |
| Mini-app logic | `node modules/lez_bridge_ui/logic.test.mjs` | Anywhere |
| Compare words vectors | `python3 -I modules/tools/make-compare-words-vectors.py (not published) modules/lez_bridge/rust-lib/data/bip39-english.txt > web/deposit/test/vectors/compare-words-v1.json`; read by `compare.rs` (module tests) and `web/deposit/test/compare-words.test.mjs` | Anywhere |
| `tests/e2e-names-relayed.sh` | "Claim a deposit" end to end | Throwaway chains on the build host (anvil, a loopback rc3 chain, its own relay), the Names module lgx and its CLI |
| `tests/e2e-receive-public.sh` | "Receive from Ethereum" on the public network | A fresh throwaway Names wallet; someone makes the Sepolia deposit from the code it prints |
| `tests/repro-names-reload.sh` | The freeze after a names reload | An isolated logosctl session; `UNLOAD=1` |
| `tests/logosctl-e2e.sh` | The module over Logos Core IPC | A local sequencer |
| `../scripts/judge-demo.sh` | The whole bridge from a clean clone | Linux or macOS arm64 (`../AGENTS.md`) |

Mini-app renders: `lez_bridge_ui/tools/render.py`, offscreen, from `fixtures/v2-demo/`.

## What was checked, and how

| What | How | When |
|---|---|---|
| A claim from the Names Wallet, from a claim file, into a public account, the relay paying | Run on the bridge test network: LEZ block 5249 | 2026-11-14 |
| A deposit code made by the wallet's secret, deposited through the v3 desk, `receive_status` to claimable, claimed through the relay into a never-funded account, the repeat refused, recovery from the wallet | Run on the public network: Sepolia block 12136637, LEZ block 6054; transcript `tests/e2e-receive-public-20261114.log` | 2026-11-14 |
| The module freezing after a names reload, and its fix | Reproduced and checked in an isolated logosctl session (`tests/repro-names-reload-20261114.log`). Busy answers inside a nested call: unit tests only. | 2026-11-14 |
| `grant_add_account` (adding an account to an existing access), Part B build/submit, the busy and no-answer rules, `needs` always carrying a request | Unit tests only, against a stand-in Names module | 2026-11-14 |
| Private claims from the module | Never run on v3 (needs a proving run) | — |

Keep this file current. When a command, an id, a call or its reply changes, change it here in the
same commit.
