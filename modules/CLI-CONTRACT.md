# `lez-bridge` CLI contract for the `lez_bridge` core module

Status: **partly implemented, 2026-11-13 (branch `sdk-adopt`).** The module (`modules/lez_bridge/`) is
written against this contract through one adapter (`rust-lib/src/cli.rs`). The CLI now implements the
global `--json` (§0.3, §2, §3), `--secret-file` (§0.4), the exit codes (§0.6), the error object (§1), the
Names Wallet scheme `--names-app` and `--emit-call` (§7). The **temporary text parsers**
(`rust-lib/src/text.rs`) stay as the fallback for a CLI whose `--help` does not list `--json`, tested on
output captured from the real binary on 2026-11-13 (`rust-lib/testdata/cli/`). Still asked of the CLI:
`--read-only` (§2.3c), `deposit-request` in the shape of §2.5, `claim-material`, `nullifier`, the
guard's `--control-file` and memory events (§3). Nothing here changes the programs or the vault.

**Two wallet paths.** In **Names Wallet mode** (the module's default, `config().wallet == "names"`,
§7) the person's accounts and keys are in the Names Wallet; the CLI only reads the chain and builds
calls (`--emit-call`), and never holds a secret on argv. In **CLI wallet mode** (`cli`, developer
setups) the CLI's own wallet folder holds the accounts and the module runs sends as dry-run jobs (§3).

Binary the module was tested against: `<local path>`,
built 2026-11-12 21:09, sha256 `10dd3a2d…5f9f` (embeds the v2 images).

## 0. Rules the module relies on

1. **The module never uses a shell.** Every argument is one argv entry. The module always passes
   `--sequencer <url>` (the CLI default `127.0.0.1:3230` belongs to another chain on the test machine) and sets
   `LEE_WALLET_HOME_DIR`, `TMPDIR`, `HOME` and `PATH`; the rest of the environment is cleared.
2. **Loopback stays enforced in the CLI** (`local_only`). The module also refuses a non-loopback
   sequencer for any command that opens a wallet, until the CLI drops that rule for a public testnet.
3. **`--json`** (global, **implemented**): the command prints **one JSON document** on stdout (reads), or an
   **NDJSON event stream** (jobs, §3) ending in exactly one `result` or `error` event. Human text goes to
   stderr only: at start the CLI keeps the real stdout on a duplicate descriptor for the JSON and points
   descriptor 1 at stderr, so the LEZ wallet crate's own lines (its sync lines, its dry-run report) also
   land on stderr (`cli/src/out.rs`). The module detects support by `lez-bridge --help` listing `--json`,
   and reads only lines that start with `{` (its `cli::run` merges stdout and stderr, stdout first).
4. **Secrets never on argv** (**implemented** as `--secret-file <path>`: a file only its owner can read,
   mode `0600` checked, 64 hex characters with an optional `0x`, read and left alone; `-` reads stdin).
   It is on `claim`, `claim-attested` (with `--leaves` and `--index`, or, since branch `first-claim`,
   with neither: the leaves are then rebuilt from the vault's logs through `--eth-rpc` up to the
   finalized block and the leaf is found), `commitment` and
   `exit-commitment`. Derived secrets need no file: `--key-index N` derives them from the wallet's
   own key (`recover.rs`).
   - **Names Wallet path (§7): no secret ever goes on argv.** The module writes `k` (from `app_secret`)
     to a fresh `0600` file in its private temporary folder, passes `--secret-file`, and removes the
     file as soon as the CLI exits, whatever happened; the call file the CLI writes (it carries `k`
     inside the claim message) is read and removed the same way. Burns need no secret at all: the
     module passes the exit commitment `E`, computed from `k'` in the module.
   - **CLI wallet mode:** `commitment`/`exit-commitment` go through a `--secret-file` 0600 file when
     the CLI speaks `--json`; the claim plan and the background claim job pass `--secret-file -` and
     the job runner writes the secret to the step's stdin (`jobs::Spec::inputs`), so it is on no argv
     and in no file. Only a CLI older than `--json` still gets `--secret` (dry runs only).
5. **No proof without the guard.** A send that proves (private claim, private burn, shield) runs only
   when the module's `proving.enabled` is true, wrapped in the configured guard argv (for example `<lock> <guard.sh>`), with `RISC0_PROVER=local RAYON_NUM_THREADS=4`. In this build the
   module forces `--dry-run` on every send.
6. **Exit codes** (**implemented**, from the error's text in `out::classify`): `0` ok · `1` refused
   before sending (local diagnosis) · `2` usage (clap) · `3` chain or RPC unreachable · `4` proof failed
   (only once the job reached `prove`) · `5` included but reverted · `6` wallet missing · `130`
   cancelled (the default signal exit; not set by the CLI). A guard kill shows as the guard's signal.

## 1. Error object (all commands, `--json`)

```json
{"event":"error","code":"already_claimed","message":"deposit already claimed: its nullifier is spent; nothing sent","sent":false,"local_only":true}
```

`local_only: true` marks a diagnosis that ran on this machine before anything was sent; such messages
may name trie paths or node bytes (they never leave the machine). On chain the only text for every
deposit-dependent failure stays `claim refused` (DESIGN §3.4). Codes, mapped from today's texts:

| code | today's text (prefix) | user-facing meaning |
|---|---|---|
| `cfg_mismatch` | `claim for another deployment` | the deposit was made for another bridge |
| `token_not_registered` | `token not registered` | unsupported token |
| `minting_frozen` | `minting is frozen on this bridge` | all claims paused by the freezer |
| `minting_frozen_token` | `minting is frozen for this token` | this token's claims paused |
| `bad_denomination` | `bad denomination` | invalid amount |
| `wrong_amount` | `wrong amount` | invalid amount |
| `wrong_leaf` | `leaf N is not this deposit` | the deposit list or the secret does not match |
| `proof_invalid` | `the statement does not verify` / storage-proof errors | verification failure |
| `root_not_attested` | `root … is not attested` | insufficient finality (stand-in source) |
| `anchor_not_finalized` | `insufficient finality` | insufficient finality (RFP-021 source) |
| `already_claimed` | `deposit already claimed` | already claimed |
| `float_empty` | `the bridge float holds` | sponsorship float too low; nothing proven |
| `not_loopback` | `is not loopback`, `is neither loopback nor the public demo chain` | sequencer refused by the CLI |
| `wallet_missing` | `no wallet yet` | no wallet |
| `reverted` | `was included in block N but reverted` | lost a race (freeze, double send) after simulation |
| `cap_exceeded` | (Ethereum side only; LEZ has no claim cap) | cap exceeded |
| `already_burned` | `refused: this exit commitment was already burned` | the same `E` twice could be released only once |
| `unreachable`, `proof_failed`, `failed` | connection errors; a failure after `prove`; anything else | |

`sent` is true once the job passed `submit`; `local_only` is true for a code-1 error before that.

## 2. Reads (`--json`, one document on stdout)

### 2.1 `show --bridge <B>` → bridge config (**`--json` implemented**; text parsed by `text::parse_show`)

The JSON also has `token_program`, `pending_admin`, and per token `decimals` (the bridge `Config`'s, which
the Names Wallet token source needs, §7).

```json
{"cfg_id":"0x7f1ab05e…4b11","vault":"0x8c8b2664de28e5287158db4db65d448638871b74","evm_chain_id":11155111,
 "lez_chain_id":"0x00…00","zone_id":"0x00…00",
 "admin":"6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV","freezer":"6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV",
 "attestor":"11111111111111111111111111111111","mint_frozen":false,"burn_frozen":false,
 "root_source":{"kind":"rfp-021","program":"4VhxvwLN5n3u3rZbsz2VNFVavLwc1niNvgxq8JJ15veK","instance":"0x09dd3320…b4de","slot":83},
 "sponsor_amount":"20000000","float":"80000000",
 "tokens":[{"eth_token":"0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e","definition":"FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ",
   "denoms":["100000000","1000000000","10000000000"],"claims":[2,0,0],"burns":[0,0,0],
   "claimed_value":"200000000","burned_value":"0","registered":true,"mint_frozen":false,"burn_frozen":false}]}
```
All amounts are **decimal strings** (u128 overflows JS numbers). Today's text (captured):
`token 0x… def <B58> denoms [a, b, c] claims [..] burns [..] claimed_value N burned_value N registered B mint_frozen B burn_frozen B`.

### 2.2 `balances --token-program <T>` → holdings (**`--json` implemented**, with the `accounts` array asked below)

```json
{"synced_to":5075,"holdings":[
 {"account":"2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC","kind":"private","definition":"FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ","balance":"100000000"},
 {"account":"6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f","kind":"public","definition":"FBqNAwJs9T4eNDw9pQXxXrLZNcnR1sTnnBEYzhudLQZJ","balance":"100000000"}]}
```
Today: `Public/<id>  <definition>  <balance>` and `Private/…` lines after the wallet's
`Latest block is N` / `Syncing…` / `Synced to block N in Xs` lines (the parser skips those). It took
10.7 s for 77 blocks, so the module serves it from a cache refreshed in the background (§4).
**Ask:** also list the wallet's accounts with zero holdings and their kind, so the UI can offer an
empty private account as the claim target (`accounts` array: `{account, kind, label}`).

### 2.3 `attest show --program <A> --instance <I> [--anchor-block-hash H]` (**`--json` implemented**; `block_number` is the anchor record's, `null` for a Gloas header; also `execution_block_number`)

```json
{"program":"4VhxvwLN…","instance":"0x09dd…b4de","chain_id":11155111,"network":"sepolia","updates":0,
 "min_participants":342,"finalized_slot":11566976,"finalized_root":"0x0776…2951",
 "latest_anchor":{"kind":"block_hash","value":"0x5e81…dd7d","recorded":true,"block_number":null},
 "asked_anchor":{"kind":"block_hash","value":"0x5e81…dd7d","recorded":true,"beacon_slot":11566976,"block_number":null}}
```
**Ask:** `block_number` of the anchor (the CLI has it when it fetched the header; for a Gloas header
today it prints `-`). The module looks it up with `eth_getBlockByHash` meanwhile.

### 2.3b `wallet account list` → the wallet's accounts (**`--json` implemented** natively from the key chain, nothing written; text parsed by `cli::parse_accounts`)

```json
{"accounts":[{"account":"2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC","kind":"private","path":"/0","label":null},
             {"account":"6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f","kind":"public","path":"/0","label":null}]}
```
Today: `[/path ]Public/<id>` or `Private/<id>` per line (captured in `testdata/cli/accounts.txt`). The module
uses it to pre-select a private claim target, to refuse a public deposit target without confirmation,
and to tell whether this wallet holds the LEZ admin or freezer key.

### 2.3c Reads must not write the wallet

Measured 2026-11-13 on a wallet copy: `show`, `attest show` and `wallet account list` leave the wallet
files unchanged; `balances` changes them (it syncs and stores). **Ask:** a `--read-only` global flag
(or make every read command read-only) so that reads can run while a job holds the wallet; until then
the module never runs `balances` while a job runs and never starts a job while `balances` runs.

### 2.4 Pure computations (no wallet, no chain)

| command | today (text) | `--json` (**implemented**) |
|---|---|---|
| `cfg-id --vault --chain-id --lez-chain-id --zone-id --bridge` | `CFG_ID=0x…` / `PROGRAM_ID=0x…` | `{"cfg_id":"0x…","program_id":"0x…"}` |
| `source-id [--attest-program --instance --deposit-root-slot]` (2026-11-14 fixes) | `ZONE_ID=0x…` (the root-source id the bridge derives, the vault's `zoneId`) | `{"zone_id":"0x…"}` |
| `chain-id` (2026-11-14 fixes) | `LEZ_CHAIN_ID=0x…` (the chain's Bedrock channel id, from its `sequencer_stake` config) | `{"lez_chain_id":"0x…"}` |
| `commitment --secret(-file) --dest --eth-token --denom --cfg-id` | `0x…` | `{"commitment":"0x…"}` |
| `exit-commitment --secret(-file) --eth-dest --eth-token --denom --max-relay-fee --cfg-id` | `0x…` / `release nullifier 0x…` | `{"exit_commitment":"0x…","release_nullifier":"0x…"}` |
| `definition --token-program --definition` | `name …` / `supply N` / `authority Some(B)` / `authority_kind program` (since the 2026-11-14 fixes: `program`, `account` or `none`) | `{"name":"…","supply":"N","authority":"B","authority_kind":"program"}` |

### 2.5 New reads the module needs

| command | output | why |
|---|---|---|
| `deposit-request --account <V> --eth-token <t> --denom-index <i> [--deposit-index n]` | `{"deposit_index":n,"commitment":"0x…","denom":"…","vault":"0x…","chain_id":…,"calldata":"0x…","value_wei":"…"}` (secret derived from the seed, never printed) | seed-based deposits, so `recover` works (F9) |
| `claim-material --eth-rpc <url> --deposit-root 0x… [--anchor-block-hash H] --out <dir>` | `{"anchor":"0x…","block_number":…,"proof":"<dir>/eth_getProof.json","header_rlp":"<dir>/header.rlp.hex","verified":true}` | today the module reaches the same files through `attest prove-slot --fetch-only --save-dir` (which needs `--probe` and `--from` it never uses) |
| `nullifier --bridge <B> --secret(-file)/--deposit-index` | `{"nullifier":"0x…","spent":true}` | claimed/unclaimed per deposit without a dry run |
| `recover --bridge B --eth-rpc <url> [--gap g] [--names-app lez_bridge]` | **implemented**: `bridge_client::scan::Recovery` (`deposits`, `exits`, `next`, `scanned_to`, `candidates`, `tree_ok`, `warnings`) plus `accounts_scanned`, `lez_mode`, `scheme` (`bridge-v1` or `names-app:<APP>`) | F9/R5: scan `Deposit` events for `C_i`, check `nf_c`; burns for `E_j`, check `nf_r` |

## 3. Jobs (sends): NDJSON event stream

`claim-attested`, `claim`, `burn`, `shield`, `freeze`, `unfreeze`, `set-sponsor`, `fund-float`,
`register`, `attest update`. With `--json` each prints events, one per line, as they happen:

`claim --relay` (no value: the `--sequencer` endpoint's relay methods; implemented on branch `grant-by-id`, rc3, run on the public demo chain 11-13): a **public** claim whose fee the demo
relayer pays by co-signing (DESIGN §3.6). It adds a `relay` phase (`{"event":"phase","phase":"relay",
"relay":URL,"attempt":n}`), its `submitted` event carries `"relay":true`, and `result` carries
`"relay":true`; with `--dry-run` the relay only checks (`{"dry_run":true,"relay":true}`). The module
does not run it: in Names Wallet mode it builds the SAME transaction without the CLI and the
Names Wallet signs it (§8); the golden vector `rust-lib/testdata/relayed-claim-v1.json` pins the
two together (CLI test `relay::tests::the_wallet_s_build_of_the_module_s_call_is_claim_relay_s_transaction`).

```json
{"event":"phase","phase":"fetch","at_ms":1794555758000}
{"event":"plan","root":"0xbe1e…d3e8","recipient":"B8UnZGJdFP3dtvUFeXsp8NTySWBY2qU8FwDK4oari3wT","recipient_kind":"one_time","sponsor":"20000000","float":"80000000"}
{"event":"phase","phase":"simulate","at_ms":…}
{"event":"simulated","public_cycles":81568,"fee_at_head":"5723160","reserve":"9300135","fee_payer":"2if1EJVc…","elapsed_ms":323}
{"event":"phase","phase":"prove","at_ms":…,"estimate_s":671,"threads":4}
{"event":"progress","segments_done":12,"segments_total":38}
{"event":"phase","phase":"submit","at_ms":…}
{"event":"submitted","tx":"f40c24d4…bc61"}
{"event":"phase","phase":"confirm","at_ms":…}
{"event":"result","ok":true,"block":4998,"elapsed_s":670.9,"dry_run":false}
```

A dry run ends after `simulated` with `{"event":"result","ok":true,"dry_run":true}`. A refusal ends with
an `error` event (§1). Phases are always in this order; a phase may be skipped (public sends have no
`prove`).

**Implemented (2026-11-13), and how it differs from the sketch above:** `phase: fetch` at the job's
start; `plan` (claims: `root`, `recipient`, `recipient_kind` `one_time|by_name|public`, `sponsor`,
`float`); `phase: simulate`; for a private non-dry send `phase: prove` (with `threads`) is printed
**before** the wallet's call, because the LEZ wallet simulates and proves inside one call; a dry run
then prints `simulated` with `elapsed_ms` and `private` only (the wallet's own report, `public cycles`,
`fee at head`, `reserve`, stays on stderr as text); a send prints `phase: submit`, `submitted`
(`tx`, `elapsed_ms`), `phase: confirm` (`tx`), and the final `result` (`block`, `elapsed_s`,
`dry_run`). No `progress` events (the prover gives none). `--emit-call` jobs (§7) end with a `result`
carrying the call's public summary. **The module's job runner in CLI wallet mode still derives phases
from the text lines below** (it does not pass `--json` to background jobs yet):

| phase | text line that starts it (today) |
|---|---|
| fetch | (job start) |
| simulate | `pre-check passed:` |
| prove | `privacy-preserving: proving on this computer` |
| submit | `proved and submitted in Xs` / `submitted in Xs` |
| confirm | `tx <hash>` |
| done | `applied in block N` (or `dry run: simulated in …; nothing proven or sent`) |
| failed | `Error: …` (mapped to §1 codes), `… was included in block N but reverted` |

**Pause / resume / cancel.** Cancel = SIGTERM to the job's process group, SIGKILL after 5 s; the CLI
must leave the wallet consistent when stopped before `submit` (it saves the wallet only after
inclusion today; keep it so). Pause/resume: a raw `SIGSTOP`/`SIGCONT` to the group works for the CLI
alone, but **under `guard.sh` a module `SIGCONT` can undo a pause the guard made for low memory**.
**Ask:** a `--control-file <path>` honoured by the guard (and the CLI's job runner when it has one):
the module writes `pause` or `run`; the guard owns the actual signals. Until then the module only
offers pause/resume when no guard wraps the job, and says why otherwise.

**Memory.** **Ask:** a `{"event":"memory","footprint_mb":N}` every few seconds from the guard (it
already logs `footprint=` every 2 s). Today the module samples the group's RSS with `ps`, which
under-reads after macOS compresses pages (DESIGN: 6.5 GB RSS at a 9.4 GB footprint), and labels it so.

## 4. The job record the module keeps (not the CLI's concern; for reference)

`<data>/jobs/<id>.json` (0600) beside `<id>.log` (0600, the CLI's output):

```json
{"id":"claim-1794555758000","kind":"claim","subject":"dep-…","state":"running","phase":"prove",
 "phases":[{"phase":"fetch","at":…},{"phase":"simulate","at":…},{"phase":"prove","at":…}],
 "started":…,"finished":null,"paused_ms":0,"paused_since":null,"pid":…,"pgid":…,
 "dry_run":true,"guarded":false,"estimate_s":671,"peak_rss_mb":…,"exit":null,"error":null,"result":null}
```

States: `running`, `paused`, `done`, `failed`, `cancelled`, `unknown` (the module restarted while it ran
and its log does not say how it ended). One wallet job at a time (two would each save the wallet from the
copy they read at start); a second start is refused with the running job's id.

## 5. Secrets and recovery (what the seed work must provide)

- `k = KDF(seed, cfgId, "deposit", i)`, `k_j = KDF(seed, cfgId, "exit", j)` (DESIGN §10).
- The module today generates `k` with the OS RNG and keeps it in `<data>/deposits.json` (0600), never in a
  reply, a job record or a log; `--secret` is passed on argv for dry runs only (rule 0.4). Once
  `deposit-request` exists, the module stores only the index.
- `recover` replaces the module's local scan (which can only find deposits whose secret this install
  made or imported).

## 6. Text formats the temporary parsers read (pinned by tests on captured output)

Still needed for a CLI built before `sdk-adopt` (its `--help` lists no `--json`).

`rust-lib/testdata/cli/*.txt` holds the captures (read-only calls and dry runs against `seq2`, wallet
copy, 2026-11-13). The parser tests fail loudly if a line they rely on changes. **When `--json` lands,
delete `text.rs` and its tests.**

## 7. The Names Wallet path (`--names-app`, `--emit-call`)

What the module does in Names Wallet mode (`rust-lib/src/names_mode.rs`, `wallet.rs`), with the Names
Wallet Provider SDK of the Names branch `bridge-sdk` (`docs/wallet/PROVIDER-SDK.md`, `BRIDGE-SDK.md` §5).
Every `names` call is made by the core module `lez_bridge`, never by the QML: the wallet keys values and
requests to the calling module's name, and `--names-app lez_bridge` reproduces exactly those values.

**Gates, in order, before anything is proposed:** `request_access` (connected, with a private account) →
the bridged tokens accepted as a token source (`propose_token_source` once, on first run: definitions
from `show --json` with the bridge `Config`'s `decimals`, origin `Sepolia 0x…`) → the `app_secret`
grant for the scope → the proposal. A method that hits a gate answers `{needs, request, message}`
(`needs`: `connect`, `private_account`, `token_source`, `secret`); the mini-app shows it, polls
`wallet_request`, and runs the action again once approved.

**Secrets.** `app_secret(Private/V, scope, hex(context), tag, i, n)`, `n ≤ 64`:

| what | scope | context | tag | value |
|---|---|---|---|---|
| deposit `i` | `RFP022/deposit` | `cfgId(32) | token(20) | u256(denom)` | `RFP022/ck` | `k` |
| exit `j` | `RFP022/exit` | `cfgId | token | u256(denom) | ethDest(20) | u256(maxRelayFee)` | `RFP022/ek` | `k'` |
| release-address key `j` | `RFP022/release` | `cfgId | token | u256(denom) | u256(maxRelayFee)` | `RFP022/release-key` | the secp256k1 key (the CLI only; the module names destinations) |

The module computes `C`, `E` and `nf_r` from these values itself (same formulas as `bridge_core`,
pinned by `client/tests/vectors/kdf-names-v1.json`), keeps the deposit's **index** in
`positions.json`, never `k`. Deposit request: one batch `first=0,count=64`, matched against the vault's
`Deposit` events; the next index is past the last used one and past every index this computer handed
out. Recovery (`recover`): batches of 64 per token and denomination until `gap` unused indexes follow the
last used one; exits are not scanned by the module (they need LEZ blocks): `lez-bridge --names-app
lez_bridge recover --eth-dest <address>` from the phrase finds them.

**Claims.** The module runs
`lez-bridge --sequencer S --json claim-attested --bridge … --from V --eth-token … --denom-index … --denom … --cfg-id … --leaves <file> --index <leaf> --proof <eth_getProof.json> --anchor-block-hash <H> --header-rlp <file> --secret-file <0600 file> --emit-call <new file>`.
The CLI runs `plan_claim` and the local diagnosis (`diagnose_claim`, §1 codes) exactly as a send, then
writes, instead of sending, the `propose_private_call` JSON: `programs` `[bridge, token, attest]`,
`accounts` in `ClaimPlan::mentions`' order after the implicit root (the anchor PDA by seed of the
attestation program, the nullifier PDA and the config PDA of the bridge by seed, the definition, V's
token actor with `openings`, and when sponsored the config's and V's `native` actors), `message` =
`borsh(Message::Instruction(ClaimAttested{…}))`, `openings` `[SHA256("RFP022/alias" | k)]`,
`fee_payer` `account` when sponsored (else `wallet`), `declared` `{credits: [{asset: definition,
amount: denom}], max_fee}` (`max_fee` = the sponsor amount when sponsored, else `--max-fee` or
20,000,000). Each PDA seed is computed from the programs' formulas and checked against the program
crates' addresses before it is written. With `--json` the stream ends with
`{"event":"result","ok":true,"emitted":<path>,"message_bytes":…,"programs":…,"fee_payer":…,"declared":…,"recipient":…,"recipient_kind":…,"sponsored":…,"sponsor":…,"root":…,"dry_run":true}`.
`claim --emit-call` (stand-in root source) is the same with the root PDA instead of the anchor.
The module then calls `propose_private_call(Private/V, <the file's JSON>, "Claim 100 dUSDC from Sepolia (deposit dep-…)")`.
`claim_plan` builds the call without proposing it; `claim` builds it again and proposes it.

**Burns.** `lez-bridge --json burn --bridge … --token-program … --from V --definition D --amount <denom> --exit E --max-fee <cap> --emit-call <new file>`:
refuses an `E` the bridge's LEZ transactions already carry (block scan through the cache, read once, up
to 120 s the first time), and a frozen or non-denomination burn; writes `programs` `[token, bridge]`,
`accounts` `[the definition (token program), the config PDA (bridge program)]`, `message` =
`borsh(BurnWithNotice{definition, amount, notify: (config, bridge), memo: E})`, no openings,
`fee_payer` `wallet`, `declared` `{debits: [{asset: D, amount: denom}], max_fee}`. The module proposes
one piece at a time (the wallet keeps at most 4 waiting requests per app), and on a vault that can never
release (the demo) builds the call to show it and does not propose it.

**Status.** `request_status` drives the module's wallet jobs (`wr-…`): phases `approve` (`check`
running or ready, waiting for the person), `simulate`, `prove`, `submit`, `confirm`, `done` (with
`tx`). `check: failed` or `refused` shows only "The Names Wallet's check stopped this call. Open the
Names app to see why." (code `wallet_check`); the wallet's own diagnosis stays in the Names app. No
pause or resume (the Names app's); cancel (`cancel_request`) until `submit`. The module never proves.

**Not covered by the module's tests** (no Names Wallet ran): the generated C++/QML interface of the
new methods (`wallet_status`, `wallet_connect`, `wallet_request`, `set_wallet_mode`), module-to-module
calls through `LogosModuleSDK` in Basecamp, and anything after approval (simulation in the Names CLI,
proving, sending).

## 8. The Names Wallet's public claim, relayed (no CLI; branch `wallet-claim`)

Status: run on a throwaway loopback rc3 chain with anvil, its own relay and filter, and an
isolated logosctl session running the Names module (wallet-next 54be22c) and this module
(`modules/tests/e2e-names-relayed-20261114.log`, 18 PASS: claimed into a never-used account that
keeps 0 native LEZ; repeat refused; two claims at once, the second re-signed by the wallet after
"busy"). On the bridge test network: reads and the check only (no claim sent).

"Claim a deposit" in the mini-app: a claim file from the deposit page into a PUBLIC account of the
Names Wallet that holds no LEZ. The wallet signs the claim; the bridge's relay sends it and pays the
fee. The module (`rust-lib/src/relayed.rs`, `lez.rs`) needs no `lez-bridge` CLI for it: the CLI only
talks to loopback sequencers (rule 0.2) and its measuring dry run needs the claimer's key, which
only the wallet holds. Everything comes from ONE config, the deployment (`rust-lib/deployments.json`:
`bridge_program`, `token_program`, `lez_rpc`, `relay`, `vault`, `desk`, `network`); the on-chain
bridge Config must agree with it (cfg id, vault, chain, token program, stand-in roots) or the
check stops.

| Step | What the module does | Reads / writes |
|---|---|---|
| `relayed_claim_check(file text or path)` | the claim file checked as the page (`verifyClaimFile`) and `claim-own.sh` check it: format v1, chain, vault, cfgId, token and amount (against the Config's denominations), the master secret gives the claim key, the commitment recomputed; a `Private/` account is refused with where to go | none |
| | the Config (borsh, fail-closed, `public-demo/WALLET-READ.md` (not published) §3), minting not frozen; for a bridge from after the 2026-11-14 security fixes (non-zero `lez_chain_id`), the chain's channel id from its `sequencer_stake` config must equal it (`wrong_chain` otherwise) | `getAccount` (LEZ) |
| | the deposit's leaf from the vault's `Deposit` logs, finalized blocks only; every rebuilt prefix root equals the vault's own `root` in the logs; the newest root whose PDA exists is used (as the CLI uses the newest), up to 8 tried | `eth_getLogs`, `eth_getBlockByNumber` (Ethereum); `getAccount` (LEZ) |
| | the nullifier PDA empty (nullifier `SHA256("RFP022/nfc" ‖ k ‖ account ‖ token ‖ u256(amount))` for a fixed bridge, `SHA256("RFP022/nfc" ‖ k)` for a pre-fix one such as the live v2 demo, told apart by its zero `lez_chain_id`); `bridge_relayInfo` (payer, caps); `getFeeState` (the reserve at the gas cap must fit the relay's fee cap) | LEZ endpoint |
| | builds the call (below); keeps it with the key IN MEMORY only, under the check's id | nothing written |
| `relayed_claim_check_with(file, options)` | the same check; `{"wallet_hint": false}` leaves out the `wallet` part (`{skipped: true}`), so nothing is asked of the Names Wallet (for a caller that is the wallet, `NAMES-EMBED.md` §2.2) | as above, minus Names |
| `relayed_claim(id, {})` | gates: connected (`request_access`, or `request_access_with(reason, {"account_hint": "Public/<account>"})` when the wallet's `get_config` `features` lists it, probed once per session; a refusal falls back to `request_access`, no answer never does), the file's account among the granted public accounts, the wallet on LEZ 0.4 and on this chain (`getChannelId` against the wallet's public `get_config` zone channel, when it gives one), the bridge and token programs declared (`declare_programs`, once); then `propose_relayed_call(Public/<account>, call, summary)`. Safe to send again: a claim with a running job gets that job back; after a `declare_programs` or `propose_relayed_call` that got no answer at all (it may be waiting in the Names app) it answers `{needs: "uncertain", uncertain: "declare" \| "propose:<id>", message, lapses_at}` and proposes nothing until `{"after_uncertain": true}` or 11 minutes later. A `busy` wallet is asked again (2, 4, 6, 8 s, no attempt after 30 s), then the error is "Your Names Wallet is busy; nothing was sent; try again." (`DESIGN.md` §1.1) | Names Wallet |
| `relayed_claim_build(id)` | for a host that signs itself (the Names app, `NAMES-EMBED.md` Part B): the checked claim's call, exactly `propose_relayed_call`'s input (`account`, `call`), plus `watch` (nullifier, definition); no wallet asked, nothing written; the claim key only inside `call.message` | none |
| `relayed_claim_submit(id, tx hex)` | that host's half-signed transaction: a job at `relay` (`host: names_app`), posted to the relay; a job that says `needs: "sign_again"` (stale nonce, or a restart) takes the new signature | LEZ endpoint (relay) |
| `receive_status(id)` | a stored wallet-made deposit, for a 60 s poll: `requested` / `deposited` (confirmations, minutes to final) / `final` (no recorded root with its leaf yet) / `claimable` / `claimed`; the recorded-root accounts and the deposit's nullifier account (kept at request or recovery time), no wallet call; Sepolia logs re-read only when `depositCount()` moved (`reads` counts the calls) | Sepolia, LEZ endpoint |
| `receives()` | this install's wallet-made deposits with the stage `receive_status` last saw (it now saves it with the row and marks the row claimed), each row with `compare` (the commitment's digits and four words); no chain or wallet read | none |
| `receive_request(id)` | a stored deposit's code again (`request`, `link`, `request_qr`, `compare`, amount in words, stage last seen), rebuilt from the stored chain id, vault, token, denomination and commitment and checked against the code kept at request time; refused for an unknown id or a deposit that kept no vault (older builds; `recover` fills it in), never guessed (`NAMES-RECEIVE.md`) | none |
| `relayed_claim_check_deposit(id, options)` | `relayed_claim_check` for a stored PUBLIC wallet-made deposit, `k` from `app_secret(Public/<id>, RFP022/deposit, …, key_index, 1)`; same reply and `id`; `{needs: "secret"}` first if the values aren't granted | Names Wallet, Sepolia, LEZ endpoint |
| `relayed_claim_status(job)` | `request_status` → the half-signed `transaction` (memory only) → `bridge_relayClaim [hex, false]` in a background thread → `getTransaction` (block) → the nullifier PDA spent → the account's wrapped and native balances | Names Wallet, LEZ endpoint |

**The call** (`propose_relayed_call`'s `call_json`; golden vector `rust-lib/testdata/relayed-claim-v1.json`,
written by `modules/tools/make-relayed-claim-vector.py` (not published) and checked by the module, by `bridge_core`
`tests/relayed_claim_vector.rs` (in CI) and by the CLI): `program` the bridge; `message` =
`borsh(Message::Instruction(Claim{eth_token, denom_index, denom, root, leaf_index, path (32),
secret, cfg_id, recipient_opening: None}))` (1,175 bytes); `accounts` = the root, nullifier and
config PDAs (`{seed, of: bridge, program: bridge}`: a seed alone, since the wallet's app-facing parser refuses a seed with an address written beside it), the definition and the claimer under
the token program; NO entry is `new` (the relay refuses admission evidence; the bridge creates the
nullifier account by its own seed); `fee` = `{payer: Public/<relay payer>, gas_limit: relay
max_gas, tip: "0", max_fee: relay max_fee}`; `privacy` = `bridge_public_claim`. **Difference from
`claim --relay`:** the CLI measures the claim (cycles + 25%, about 190,000 gas) and scales `max_fee`;
the module cannot simulate, so it declares the relay's caps (250,000 / 2,500,000 on the bridge test
network) after checking the head fees fit. The chain charges the cycles used, not the gas limit
(LEZ `fee_actual_base`), so the relay pays the same; only the admission reserve is higher.

**Busy.** The filter answers HTTP 503 "relay busy" while another relay call runs: the same signed
transaction is posted again after `Retry-After`. The relay refuses a claim whose payer nonce is
stale ("stale nonce": its payer had another claim in flight); and a submitted claim can stay out of
every block when another claim used the payer's nonce first. For both, the module waits until the
chain shows the payer's nonce moved, then calls `resign_relayed_call` (the wallet re-signs the SAME
call for the new payer nonce with no second approval, at most 3 times within 10 minutes of the
approval, Names `SIGNING.md` §6) and posts that. The wallet's reply `code` decides (its words only
for builds without one): `unchanged` (the payer's other claim hasn't landed: wait), `landed_or_used`
(the account was used since the approval: the module asks the chain for the nullifier; spent is
done, unspent fails with code `landed_or_used`; never a new proposal in a loop), `ask_again` or
`refused` (the call is proposed again and the person approves once more).

The two programs are declared once with `declare_programs([bridge, its token program])`, nothing
else; the bridge program is not in the Program Registry yet, so the wallet's declaration sheet asks
for its risk tick.

**Errors** (`error.code`): `bad_file`, `private_account`, `other_chain`, `other_vault`,
`cfg_mismatch`, `token_not_registered`, `bad_denomination`, `wrong_amount`, `wrong_leaf`,
`not_in_vault`, `not_final`, `leaves_mismatch`, `root_not_attested`, `already_claimed`,
`minting_frozen`, `minting_frozen_token`, `no_bridge`, `bridge_unreadable`, `bad_config`,
`no_relay`, `relay_cap`, `relay_funds`, `relay_once`, `relay_refused`, `relay_busy`,
`relay_unavailable`, `rate_limited`, `unreachable`, `eth_unreachable`, `wrong_account`, `busy`,
`reverted`, `wallet_failed`, `wallet_rejected`, `landed_or_used`.

**Secrets (rule 4).** The claim key and master secret are in the module's memory only, in no
reply, log or file of the module (the key field is overwritten when the checked claim is dropped;
the kept call and the signed transaction, which also carry it, are freed but not overwritten); no temporary file is needed (no CLI runs). The claim
message carrying the key goes to the wallet (its own 0600 store); the half-signed transaction it
returns is held in memory until the relay has it. `wallet.json` (0600) keeps the job's public facts
(request id, account, amount, nullifier, tx, block). Through logosctl the file is passed BY PATH.
