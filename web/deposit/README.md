# Logos bridge deposit (web)

A static page that lets someone with an ordinary Ethereum wallet (MetaMask, or any EIP-1193 / EIP-6963 wallet) make a deposit into the RFP-022 bridge vault. It is the supplementary interface of the RFP's Soft requirement #5.

**Two ways to deposit (since 2026-11-13):**
- **Made on the page (the default).** Connect a wallet, pick the amount, paste your Logos account, save the claim file, deposit: five actions plus the wallet's own confirmations. The page makes the secret in the tab (`crypto.getRandomValues`): master secret `s`, claim key `k = SHA-256("RFP022/ck" ‖ s)`, commitment `C = SHA-256("RFP022/dep" ‖ k ‖ dest ‖ token ‖ u256(amount) ‖ cfgId)` (`src/lib/create.js`; byte-identical to the CLI's `commitment` for the four vectors in `test/vectors/commitment-cli.json`). The secret lives in memory and in the claim file (`logos-bridge-claim-<C prefix>.json`: chain, vault, cfgId, token, amount, Logos account, `s`, `k`, `C`, and after the deposit the transaction, block and leaf index). The Deposit button stays off until the person ticks "I saved the claim file". The secret is never written to the URL, storage or any request (tested in `test/create-flow.test.mjs` and in the browser by `nameswallet-site/bridge-src/shoot.mjs`). After sending, the address bar gets the deposit's public facts so a reload resumes; the secret is gone with the tab.
- **From a Names Wallet deposit code.** The page **never sees the deposit secret or the Logos destination**: the wallet seals both into `C` and hands the page a *deposit code* (the *deposit request* below) holding only public facts. A prominent button on the page ("I have a deposit code from my Names Wallet") opens the paste box, and a link `https://nameswallet.app/bridge/#logosdep1…` opens the code directly. On this path the page makes no claim file and mentions none: it shows the commitment's first and last 6 hex digits large ("compare with your wallet"), the fee as for any deposit, and after the deposit "Your Names Wallet will show this deposit and offer the claim". A deposit made on the page and reopened from its link is marked `#made:logosdep1…` so the page still points to its claim file (`src/lib/entry.js`). The `lez_bridge` core module makes these codes with the same bytes (`modules/lez_bridge/rust-lib/src/request.rs`, checked against `test/vectors/request-v1.json`).

Either way the page checks the deposit against the chain, asks the wallet to sign the approval (ERC-20 only) and the deposit, and follows it to Ethereum finality. **Claims:** only the team's local LEZ test chain (`seq2`) can claim today; the public Logos testnet does not run this bridge.

**Status (2026-11-14; updated 2026-11-27):** built and tested locally. It is prepared to be served at `https://nameswallet.app/bridge/` as part of the Names Wallet site (a byte-for-byte copy made by `nameswallet-site/bridge-src/sync-bridge.sh`; that repository holds its `/bridge/*` header lines, checks and screenshots), and is served there (the page answered HTTP 200 on 2026-11-13). It works on **Sepolia only**: a request or a wallet on any other chain, Ethereum mainnet included, is refused (`SUPPORTED_CHAIN_IDS` in `src/lib/deployments.js`). The live default (`LIVE_DEFAULT` in `src/lib/deployments.js`) moved from v3 to v4 and then **Sepolia vault v5** (`0xAFbD…eD10`, since 2026-11-15; `vault/DEPLOYMENTS.md`, `docs/EVIDENCE.md`), which releases by real Groth16 proofs on Sepolia (v4 stays listed, with a 1-hour delay; v3 and v2 stay listed, and no honest proof can release them). Note on privacy: an LNS-name deposit always resolves to the holder's published **public** account (`LNS.md` §2), so claiming it is linkable by construction.

**Styling:** the page takes its colours, typeface and corner radius from the Names Wallet site's `/assets/tokens.css`, which `index.html` links by that absolute path. When this folder is served on its own, `assets/tokens.css` here is a stand-in with the same values (not in the deployable set).

Contents:
- [Try it locally with MetaMask on Sepolia](#try-it-locally-with-metamask-on-sepolia)
- [How the page works](#how-the-page-works)
- [Bridge fee (deposit desk), off until its address is set](#bridge-fee-deposit-desk-off-until-its-address-is-set)
- [Deposit request format, v1](#deposit-request-format-v1)
- [Privacy: what is public](#privacy-what-is-public)
- [Security trade-offs](#security-trade-offs)
- [Visual direction](#visual-direction)
- [Files, dependencies and sizes](#files-dependencies-and-sizes)
- [Testing](#testing)
- [Cost](#cost)
- [Not done](#not-done)

## Claim from a claim file (team test chain, today)

A deposit made on the page is claimed on the team's local LEZ chain `seq2` (on the test machine, `http://127.0.0.1:3250`), whose bridge config is bound to RFP-021 attestation instance A. The public Logos testnet does not run this bridge. The claim file gives: `chainId`, `vault`, `cfgId`, `token.address`, `denominationIndex`, `amount`, `logosAccount`, `claimKey` (`k`, what today's CLI takes as `--secret`), `masterSecret` (`s`, for the future failsafe reclaim; the CLI does not take it), `commitment`, and after the deposit `deposit.tx`, `deposit.block`, `deposit.leafIndex`.

It does **not** give, and the claimer also needs:
1. **A wallet on `seq2` that holds `logosAccount`** (`LEE_WALLET_HOME_DIR`), because the commitment binds it: a public claim mints to `--from` itself, so `--from` must be that account; a private claim proves from that private account.
2. **The vault's deposit leaves in order** up to the current root (`--leaves`, one hex leaf per line, rebuilt from the vault's `Deposit` logs), and the deposit's leaf index (`--index`, = `deposit.leafIndex`).
3. **A finalized anchor that includes the deposit, recorded by instance A.** The anchor recorded today (Sepolia block 12123027) predates any new deposit, so someone must first record a newer finalized checkpoint with `lez-bridge attest update …` (a light-client update) and fetch, for that block, the storage proof of `isDepositRoot[root]` (slot index 83: `eth_getProof(vault, [keccak256(abi.encode(root, 83))], block)`) and its header RLP.

Then (public account; values from the claim file in angle brackets):
```sh
export LEE_WALLET_HOME_DIR=<wallet holding the account> LEZ_BRIDGE_SEQUENCER=http://127.0.0.1:3250
lez-bridge claim-attested --bridge JECKBkvoc2e3dC18Wm6zZJEC2BGMwUwLsdZiQzYTnUx7 \
  --token-program 4VScJ7mtwaanQ5gcSZZw2iAMHYAbLrVmbYKwEGx2eSex --from <logosAccount> \
  --eth-token <token.address> --denom-index <denominationIndex> --denom <amount> \
  --secret <claimKey> --cfg-id <cfgId> --leaves leaves.txt --index <deposit.leafIndex> \
  --proof eth_getProof.json --anchor-block-hash <anchor block hash> --header-rlp header.rlp.hex
```
Run it once with `--dry-run` first. A **private** claim is the same command with a private `--from`; it proves locally (about 11 minutes and 9.4 GB on the test machine, run through `scripts/guard.sh`, see `prove-private-claim.sh` (run sepolia2)) and mints to a one-time address, sponsored from the bridge float. `--from` takes the bare account id (the CLI refuses `Public/` or `Private/`; the page strips them).

## Try it locally with MetaMask on Sepolia

You need MetaMask (or another browser wallet) with a little Sepolia ETH: 0.01 for the deposit plus about 0.0005 for gas at the prices seen on 11-13. The demo USDC (dUSDC) can only be minted by the demo operator, and there is no faucet for it. So the try-it path is **0.01 Sepolia ETH**. For dUSDC, ask the operator.

1. **Make a real commitment** on your own machine with the bridge CLI. Use a fresh random secret, and keep it: it is what lets you claim. The vault's `cfgId` is `0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11`.
   ```sh
   SECRET=0x$(openssl rand -hex 32)          # keep this secret
   lez-bridge commitment --secret $SECRET --dest <your LEZ account id> \
     --eth-token 0x0000000000000000000000000000000000000000 \
     --denom 10000000000000000 \
     --cfg-id 0x7f1ab05e910781ef1c6bd7747b8221372377dc785bc09e303e687d2d4f284b11
   ```
   The command prints `0x…`: that is `C`. It sends nothing (`lez-bridge/cli/src/main.rs`, `Command::Commitment`). `--denom` is in base units: wei for ETH (`10000000000000000` = 0.01), 6 decimals for dUSDC (`100000000` = 100).
2. **Turn it into a request.** Pass the same `--denom`:
   ```sh
   cd web/deposit
   node tools/make-request.mjs --commitment 0x<C> --token ETH --denom 10000000000000000 \
     --rpc https://ethereum-sepolia-rpc.publicnode.com --base http://127.0.0.1:8000/
   ```
   This prints the `logosdep1…` string and a link, and adds a terminal QR code with `--qr`. With `--rpc`, the tool reads the vault's denominations from the chain. It refuses a `--denom` the vault does not have.
3. **Serve the page** on this machine only:
   ```sh
   python3 -m http.server 8000 --bind 127.0.0.1
   ```
   Open the printed link in the browser where MetaMask is installed. It looks like `http://127.0.0.1:8000/#logosdep1…`.
4. **In MetaMask**, turn on *Show test networks* in the network menu. The page offers to switch to Sepolia.
5. Click **Connect MetaMask**, then **Switch to Sepolia** if asked. Wait for the checks to pass, then click **Deposit 0.01 ETH**. In MetaMask, check that the contract is `0x8C8b2664De28e5287158db4DB65D448638871b74` and the value is 0.01 ETH, then confirm.
6. The page waits for inclusion and then for finality, usually 13 to 15 minutes. It ends with a receipt: transaction, block, and leaf index. You can close the tab at any time. Opening the same link again finds the deposit through `commitmentUsed[C]` and the `Deposit` event, and resumes. If the wallet has already shared an account with the page, it reconnects without a prompt (`eth_accounts`); otherwise it takes one Connect click.
7. Stop the server with Ctrl-C.

**Example request, for looking only.** Nobody can claim it: its commitment is `SHA-256("logos-deposit-request example 1: unclaimable")`, and the page refuses to send it.
```
logosdep1qyqqqqqqqz4rdfuv3vnxfh3gu558zkxmfkm963yx8zr3kaqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqu3cdun0cyqqp930ngllc3rfp2kg60gw4zhjflmrk494scj9xnqsmmjxkyualkc02axs7z
```
`node tools/make-request.mjs --example --token ETH --denom 10000000000000000` prints a request with a fresh random commitment. That one is also unclaimable, but the page cannot tell, so do not deposit with it.

## How the page works

The flow is a DOM-free state machine (`src/lib/flow.js`). The page (`src/app.js`) only renders its state. The headless test drives the same object with a mock wallet.

| Step | What the person sees |
|---|---|
| `request` | Paste box, *Scan QR code* (only where the browser has `BarcodeDetector`), and where requests come from. Decoder errors are shown inline |
| `connect` | One button per wallet found (EIP-6963, then legacy `window.ethereum`). With no wallet: "open this link inside your wallet app's browser". Most phone users land here from a QR code |
| `network` | "Switch to Sepolia" (`wallet_switchEthereumChain`). Error 4902 explains MetaMask's *Show test networks* |
| `checking` | The checks below, read through the wallet's provider |
| `blocked` | A plain explanation of what failed and what to do, plus *Check again* |
| `ready` | ERC-20: *Approve exactly N*. Then: *Deposit N*. An unknown vault needs a ticked acknowledgement first |
| `approve-sign` / `deposit-sign` | "Confirm in your wallet", with the contract to check |
| `approve-wait` / `deposit-wait` | Waiting for inclusion; transaction link |
| `finalizing` | Blocks finalized versus the deposit block, from the `finalized` block tag |
| `done` | Receipt (tx, block, time, leaf index, commitment, vault) and "What next" |
| `failed` | A reverted or reorged deposit, with the decoded reason |

**Checks, all before any signature:**
1. The vault is on the built-in allowlist (`src/lib/deployments.js`). A retired vault (Sepolia v1) is refused. An unknown vault gets a loud warning and needs an explicit acknowledgement.
2. Contract code exists at the vault on this chain.
3. `tokenConfig(token)`:
   - `registered` is true;
   - `decimals` matches the allowlist;
   - `denoms[denomIndex] == amount`;
   - `depositFees[denomIndex]` is shown and added to the total.

   The amount check is critical. The commitment binds the amount, the vault takes only the index, and a mismatch would lock the deposit for good.
4. `globalDepositsFrozen()` and the per-token `depositsFrozen` are both false.
5. Cap headroom from `capState(token)`, and from `globalCapState()` weighted by `capWeight(token)` / 1e18. Window 0 means no cap. The figure is advisory, because the rolling estimate moves with time.
6. `commitmentUsed(C)` is false. If it is true, the page looks for the `Deposit` event:
   - from this wallet, for this token and amount: it resumes at the receipt;
   - from another wallet, same token and amount: it explains that the Logos wallet can claim that deposit;
   - for another token or amount: it refuses.
7. Balance: ETH ≥ total; for an ERC-20, token balance ≥ total and some ETH for gas. For an ERC-20 the page also reads the allowance.
8. The exact deposit is simulated with `eth_call` from the person's address, and the answer must be exactly one 32-byte word (the new leaf index). A codeless address answers `0x` and is refused. The simulation runs again immediately before the signature request, which catches a commitment taken in the meantime (front-running). Before every signature request the page also re-reads `eth_chainId`, because a wallet can change network without sending a `chainChanged` event. The gas estimate comes from `eth_estimateGas`; the page never hardcodes a limit.

**Approval:** for the exact total, never unlimited, and there is no option for unlimited. If a non-zero allowance exists and an approve to the new value would revert (the USDT behaviour), the page sends `approve(0)` first.

**Errors** are decoded from revert data wherever the wallet puts it (`error.data`, nested `data`, `originalError`, `cause`, or the message). The page knows every vault custom error on the deposit path, the OpenZeppelin `SafeERC20` / `ERC20` / `ReentrancyGuard` errors, `Error(string)` and `Panic(uint256)`. Each message says what happened and what to do, and never refers to any other deposit.

**Finality.**
- The page polls `eth_getBlockByNumber("finalized")` every 12 s until the finalized height reaches the deposit block. It then checks that the canonical hash at that height equals the receipt's `blockHash`. If they differ, it re-reads the receipt (reorg handling).
- If the wallet's node rejects the `finalized` tag three times in a row, the page says so and falls back to a time estimate labelled as an estimate. It marks the deposit done after 30 minutes. A single failed read, or a failed canonical-hash check, is retried on the next poll.
- Measured on 11-13: the finalized head trailed `latest` by 65 blocks. The v1 deposit of 11-12 was seen finalized 13.5 min after inclusion (`vault/DEPLOYMENTS.md`).

## Bridge fee (deposit desk), on since 11-13

**The one switch:** `desk` in the deployment entry of `src/lib/deployments.js`. Empty = off: the page deposits straight into the vault, exactly as before. Set to the DepositDesk's address (today: v3's `0x6b7F38D05751b1ed432a0A0d377A63cFB8c4DBc5`; v2's was `0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2`) = on: **every** deposit on the page goes through the desk (made here, the demo account, private accounts, and requests from a Logos wallet alike) and pays its bridge fee.

With the desk on:
- **Read, never configured:** before any signature the page reads the desk's `vault()` (must be the allowlisted vault), `quote(token, denomIndex)` (must agree with the vault's denomination and fee) and `feeBps()`. A desk without code, or one that forwards to another vault, blocks with a plain message. Check: "Bridge fee read from the deposit desk".
- **Shown before the wallet prompt:** "Bridge fee (0.3%): X. It covers the claim's fee on the bridge test network." and the total the wallet will send (deposit + vault fee, if any + bridge fee, plus the network fee). The details list the desk's address.
- **Counted everywhere:** the balance check, the exact approval (ERC-20: the **desk** is the spender, for denomination + fee), the simulation and the transaction all use the total. ETH: one transaction with `msg.value` = the total. ERC-20: the deployed desks (v2 to v5) take `depositWithFee(token, denomIndex, C, 0)`, where only the exact approval bounds the total; a desk built since 2026-11-27 takes a fifth argument `maxTotal` and reverts `WrongAmount` above it. The page sends that call, with the total it showed as `maxTotal`, only when the deployment entry says `deskMaxTotal: true` (`deskTakesMaxTotal` in `src/lib/deployments.js`; no entry does yet). A fee change between the checks and the send makes the last simulation fail ("wrong-amount"): nothing is sent.
- **No public key.** The account step takes the account address only (`Public/…`, `Private/…` or bare base58, as before the desk). The desk's `lezPublicKey` argument is always zero: the page names no account to the desk, so the deposit and the account stay unlinked on Sepolia. A public claim's fee is paid by the claim relayer, which co-signs the claim as its fee payer (`lez-bridge claim --relay`, DESIGN.md §3.6); a private claim draws on the bridge's float. (From 11-13 until this change the page had an optional "Public key of this account" field for a fee grant; it is gone.)
- **Claim file:** unchanged format (version 1) with one new optional field: `bridgeFee` (`desk`, `feeBps`, `amount` in base units). Files made while the key field existed may also carry `lezPublicKey`; readers ignore it.
- **Resume after reload** works as before: the vault's `Deposit` log is in the same transaction; "from this wallet" is the transaction's sender.

Tests: `test/desk-flow.test.mjs` (13) with a desk in `dev/mock-chain.js` (quote, exact amounts, desk allowance, `FeePaid`). The dev harness puts the mock desk at the address `deployments.js` names, so every harness state runs through it when the switch is set; desk states: `desk-own`, `desk-own-done`, `desk-private`, `desk-usdc-done`, `desk-fee-changed`, `desk-short` (plus `make-ready`, `make-usdc`, `send-fails`, `ready`, which now show the fee). The desk itself: `vault/src/desk/DepositDesk.sol`, live on Sepolia at `0x77E37E1D6e886d8fb988496B9c2EfF3E78e27DF2` (block 12128491).

**Live, read-only, in the browser (11-13 evening):** the staged page with the site's real `_headers` CSP, a test wallet that only shares an address (the demo deployer) and never sends, reads through PublicNode against the live desk: ETH 0.01 → every check passed, "Bridge fee read from the deposit desk: … 0.0003 Sepolia ETH (3% of 0.01 Sepolia ETH); you send 0.0103 Sepolia ETH in all", "Deposit simulated against the chain: passed"; dUSDC 100 → the desk's quote read (3 dUSDC, 103 in all) and the balance check refused (that address holds 100 dUSDC). No CSP violation; the only hosts contacted were the local preview and `ethereum-sepolia-rpc.publicnode.com`. The CSP's `connect-src` needs no change: the desk is read through the same RPC.

## Deposit request format, v1

A request describes a deposit on Ethereum; the funds are then claimed on Logos with the secret the wallet kept. This build accepts requests for **Sepolia (chain id 11155111) only**, and for the Sepolia demo vault the claim happens on the team's local Logos test chain: nothing in a request moves real funds.

### Requirements it meets

- **Public facts only:** no secret, no destination.
- **Self-describing:** it names the chain, vault, token, denomination and commitment, so the page needs nothing else from the Logos side.
- **One canonical string per request,** so another implementation (Rust) can match it byte for byte.
- **Survives copying and QR:**
  - no characters that need percent-encoding in a URL fragment;
  - selectable with a double click;
  - case-insensitive;
  - a checksum that catches typos;
  - short enough for a comfortable QR code.

### Choice: bech32m over a fixed binary layout

Bech32m (BIP-350) fits these requirements:
- **Alphabet:** 32 lowercase letters and digits, with no `1`, `b`, `i` or `o` after the separator. It survives double-click selection and chat apps.
- **Case:** the decoder accepts all-uppercase, which QR codes encode more densely in alphanumeric mode.
- **Checksum:** a 30-bit BCH code built for this alphabet. Every vector test flips every single character, and all of them are caught.
- **Libraries:** maintained ones exist in every language, including the Rust `bech32` crate. *Not verified here* (the crate is not in this machine's cargo cache): check that its length limit admits 200 characters, and use a bech32m-only decode. A generic decode that also accepts the older bech32 checksum would wrongly accept the `bech32 (not bech32m) checksum` vector.

Two alternatives were rejected:
- **A URI** such as `logosbridge:deposit:v1?chain=…&vault=…`. It reads well, but it is about 250 characters, it has no checksum unless one is bolted on, and it allows many spellings of the same request (parameter order, case, leading zeros).
- **Base58check.** It is about 20 % shorter, but case-sensitive, and it gives no structured error detection.

**Checksum strength.** BIP-350's guarantee to detect any 4 errors holds up to 89 characters. Requests are 150–199 characters, so they rely on the code's general strength, as Lightning invoices do: an undetected random error has a chance of about 1 in 10^9.

**What the checksum does not do.** It is a typo defence. It does not stop a malicious page, because anyone can compute a valid checksum. See [Security trade-offs](#security-trade-offs).

### Layout

The string is `bech32m(hrp = "logosdep", payload)`. The payload bytes are:

| Offset | Size | Field | Rule |
|---:|---:|---|---|
| 0 | 1 | version | `0x01`. Any other value: error `version` |
| 1 | 8 | EVM chain id | u64 big-endian, 1 to 2^64−1 (`chain`) |
| 9 | 20 | vault | non-zero (`vault`) |
| 29 | 20 | token | the zero address means native ETH |
| 49 | 1 | denomination index | 0–255. The vault allows at most 8 (`MAX_DENOMS`) |
| 50 | 1 | L, the amount length | 1–32 (`amount`) |
| 51 | L | amount | big-endian, minimal: no leading zero byte, never zero (`amount`) |
| 51+L | 32 | commitment `C` | not all zeros (`commitment`) |

**Total length:**
- The payload is 84 + L bytes; any other length gives `length` or `trailing`.
- The string is 150–199 characters. The decoder refuses more than 200 (`length`).

**The amount field** MUST equal the `--denom` given to `lez-bridge commitment`: the denomination in the token's smallest unit, which `C` binds. The fee is not in the request. The page reads `depositFees[index]` from the vault and shows it.

**Canonical form.** The canonical string is lowercase. Encoders MUST produce it; decoders MUST accept it and its all-uppercase form, and nothing else. A mixed-case string gives error `case`, and padding bits that are not zero give `padding`.

**Lenient input.** A strict `decode` is wrapped by `parseRequestInput` for text a person pastes, scans or opens:
- it removes whitespace, line breaks and zero-width characters;
- for a link, it takes the part after `#`, and accepts an optional `request=` prefix;
- it then decodes strictly.

**In a link,** the request goes in the **URL fragment**: `https://<page>/#logosdep1…`.
- Browsers never send the fragment to a server, and it does not appear in `Referer` headers.
- The page also sets `<meta name="referrer" content="no-referrer">`.
- The query string is never used.

**QR code:** encode the link, or the uppercase string.
- On their own, these encode as **version 9-M** (the 155-character dUSDC request, byte mode) or **version 8-M** (the 199-character bounds vector, uppercase alphanumeric). Both scan comfortably.
- The full `http://127.0.0.1:8731/#…` link is version 10-M.
- A long `https://deposit.example.org/logos/bridge/deposit/#…` link is version 11-M.

**Error codes:** `type`, `empty`, `length`, `charset`, `case`, `format`, `checksum`, `padding`, `hrp`, `version`, `trailing`, `amount`, `chain`, `vault`, `commitment`, `denom-index`, `field`.

### Reference implementation and vectors

- **`src/lib/request.js`**: one file, no imports, plain ES2020 with BigInt. It runs in browsers, Node and Deno, so the Logos side can copy it as is.
  - Exports `encodeRequest`, `decodeRequest`, `parseRequestInput`, `requestUrl`, `requestToBytes`, `requestFromBytes`, `normalizeRequest`, `requestToJSON`, `bech32mEncode`, `bech32mDecode`, `bech32mDecodeWords`, `RequestError`.
- **`test/vectors/request-v1.json`**, regenerated by `node tools/gen-vectors.mjs`:
  - **5 valid requests**, each with the request fields (chain id and amount as decimal strings), payload hex, string and length. They cover the three demo denominations, chain 1 / index 255 / amount 2^256−1 / all-`ff` fields, and chain 2^64−1 / amount 1;
  - **6 lenient inputs**;
  - **20 invalid strings,** each with the expected error code;
  - the official BIP-350 bech32m checksum vectors.

  A Rust implementation should pass the same file. The commitments in the vectors are hashes of public labels, so nobody can claim them.
- **Compare words, v1** (`src/lib/compare-words.js`, not shown on the page yet): four words a wallet and the page can both show for `C`. `h = SHA-256(ASCII "logos-bridge/compare/v1" ‖ C)`; the first 44 bits of `h`, big-endian, are four 11-bit indexes (bits 0–10, 11–21, 22–32, 33–43) into the BIP-39 English word list (index 0 = "abandon"); the text joins the words with " · " (space, U+00B7, space). The Logos bridge core module gives the same words (`compare.words`, `compare.words_text`). `compareWords(C)` is async (it uses `crypto.subtle`) and returns `{ indexes, words, words_text }`. The list is `src/lib/vendor/bip39-english.js`: the BIP-39 English list unchanged (MIT, Pavol Rusnak; the text's SHA-256 is `2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda`), data, not code. **`test/vectors/compare-words-v1.json`**: 12 vectors (all zeros and all `ff` among them) with the commitment, `h`, the indexes, the words and the text, from an independent Python generator; `test/compare-words.test.mjs` reads it, and so does the module's test. Four words are 44 bits: they catch a mistake or a swapped code, not a deliberate forgery made to match.
- **Bridge code, v1** (`src/lib/bridge-code.js`; `#bridge` on the page, and as text on `about/verify/`): one whole bridge deployment for a Names Wallet that doesn't list it yet: `bech32m("logosbr", payload)` with the chain, vault, gate, desk, cfgId and its parts, the bridge program and its Config account, the token program and tokens with their amounts, the release delay, the starting state, the proof image ids and the LEZ endpoint. A reader re-computes the cfgId (the vault's `computeCfgId`) and the Config account before anything else; the wallet then checks it on both chains. Its four words are the compare words of `SHA-256("logos-bridge/bridge-code/v1" ‖ payload)`. Built from the `bridge` block of a `src/lib/deployments.js` entry (v5, v4); `node tools/bridge-code.mjs [id]` prints it. **`test/vectors/bridge-code-v1.json`**: the live v5 and v4 codes, a synthetic one and 23 refusals, from an independent Python generator; `test/bridge-code.test.mjs` reads it (and checks `deployments.js` and `about/verify/` against it), and so does the module's test. The spec is the bridge's design note NAMES-SEND-BACK.md §9.
- **Commitment grid (optional, for wallets):** the page draws `C` as a 16 × 16 grid of its own 256 bits. Row r holds bytes 2r and 2r+1, most significant bit on the left, and a set bit is a filled cell. A Logos wallet that draws the same grid lets the person compare the two at a glance. It is not a hash, just `C` itself.

Example (vector 1): 0.01 ETH, Sepolia demo v2, index 0.

```
payload 01 0000000000aa36a7 8c8b2664de28e5287158db4db65d448638871b74
        0000000000000000000000000000000000000000 00 07 2386f26fc10000
        962f9a3ffc44690aac8d3d0ea8af24ff63b54b58624534c10dee46b139dfdb0f
string  logosdep1qyqqqqqqqz4rdfuv3vnxfh3gu558zkxmfkm963yx8zr3kaqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqu3cdun0cyqqp930ngllc3rfp2kg60gw4zhjflmrk494scj9xnqsmmjxkyualkc02axs7z   (159 chars)
```

## Privacy: what is public

The page says this in plain words in a panel that is always visible: on the right on desktop, under the action on a phone. It also gives a one-line summary under the amount.

**Public on Ethereum, for good:**
- the depositing address;
- token and amount;
- time (block);
- the commitment.

**Not on Ethereum:**
- who receives on Logos;
- the secret;
- which later claim is the person's, as long as other deposits of the same amount exist.

**What the person can do about it:**
- use an address not tied to them. Funding it straight from their usual address links the two;
- use the common amounts;
- claim later, not right away.

**Anonymity set.** The page shows `counters(token, index).deposits`: the deposits of this token and denomination so far.
- The live test checked these numbers against the `Deposit` events.
- On the demo vault this is **1 earlier 0.01 ETH deposit**, so the page says "Tiny. A matching claim on Logos could be any of 2 deposits at best, and timing narrows that further. Expected in a demo vault."
- It never rounds this up into a reassuring number.

**Disclosed:** the wallet's RPC provider sees the person's IP address, address and `C` while the page checks the request. For MetaMask this is usually Infura. If the person picks a read-only RPC in Settings, that RPC sees the IP address and `C`. Network-level privacy is out of the RFP's scope and disclosed here as residual leakage.

**Browser storage:**
- The only thing the page keeps in `localStorage` is the read-RPC choice.
- The request is kept in the URL fragment, so a reload resumes. It therefore also sits in the browser history. That reveals which commitment the person deposited to someone with access to the device. It does not reveal the destination or the secret.

## Security trade-offs

Soft requirement #5 notes that browser-based software is exposed to:
- malicious or compromised browser extensions;
- injected scripts;
- phishing front ends.

The design limits what any of these can gain.

**What the page handles depends on the way.** With a wallet's request it never sees `k` or the destination, so nothing in or around the page can learn where the funds go or steal a correct deposit. With a deposit made on the page, it creates `s` and `k` itself: a tampered copy of the page, or an extension that can read the page, could take them and claim the deposit. The genuine page keeps them in memory and the claim file only; the CSP stops an injected script from sending them anywhere but PublicNode; the files are in `MANIFEST.sha256`. For anything of value, the request path is the safer one.

**What it is built from:**
- **No backend at all.** The page is static files. Nothing runs on a server; a host only serves bytes.
- **No analytics, no runtime dependencies, one vendored library.** Every byte the browser runs is in `src/`: hand-written ES modules, plus `src/lib/vendor/argon2id.js` and one vendored data file, `src/lib/vendor/bip39-english.js` (the BIP-39 English word list, MIT, unchanged, for the compare words).
  - The ABI encoder, Keccak-256, bech32m and the QR encoder are written out here and tested against independent references: Foundry `cast`, BIP-350 vectors, and the browser's own QR decoder.
  - `src/lib/vendor/argon2id.js` is the single exception to "no vendored libraries" (owner's decision, 2026-11-15): Argon2id from `@noble/hashes` 2.4.0 (MIT, Paul Miller), the same bundle the LNS web page ships (logos-names `web-register-04` `eab4932`, `names.js` lines 3–1341, unchanged, plus one export line). An LNS name lookup needs Argon2id (`LNS.md` §1) and the browser has none; the only other ways were the wallet resolving names for the page or a server, which would bill and see every lookup. `test/argon2id.test.mjs` checks it against the registry's own vectors (SPEC §10: RFC 9106 §5.3 and five name derivations). Upstream says argon2 is outside noble's audit scope. It runs on the main thread (`worker-src 'none'`), about half a second per name, behind "Checking …".
  - `MANIFEST.sha256` lists the SHA-256 of every deployable file, so anyone can compare a hosted copy with the reviewed source.
- **Strict Content-Security-Policy** (meta tag, sent again as an HTTP header with the same string):
  - `default-src 'none'`;
  - `script-src 'self'` and `style-src 'self'`, with no inline code, no `eval`, and no `innerHTML` anywhere in the code;
  - `connect-src` is only `https://ethereum-sepolia-rpc.publicnode.com`.

  **Wallet calls are not page network traffic:** they go to the extension through its injected provider, which CSP does not govern. A same-origin `fetch` from the page was blocked by this CSP in the Chrome test, which shows the policy is enforced.

  **What this does and does not stop:**
  - It stops a script injected into the genuine page from fetching, posting, loading images or opening sockets to other hosts.
  - It does not stop top-level navigation: a script could still send data out by redirecting the page or opening a link.
  - It does not bind a tampered copy: whoever serves a modified `index.html` serves their own CSP with it. Against that, compare the files with `MANIFEST.sha256`.

  **Limits of a meta-tag CSP:**
  - browsers ignore `frame-ancestors` in a meta tag (Chrome logs one console line saying so), so a host must also send the policy as a header, with `X-Frame-Options`, `nosniff`, `Permissions-Policy` and `no-referrer`. The meta tag carries the identical string so the two can be compared byte for byte. For this folder at a host's root, `_headers` has them; for nameswallet.app, `bridge-src/bridge.headers` there;
  - ES-module imports get no Subresource Integrity in a meta-only setup.
- **User-chosen RPC.** Usability #9 asks that the person can change the Ethereum RPC. That cannot coexist with a strict static CSP. The page offers a fixed list (PublicNode only), used only when no wallet is connected. Adding another RPC means editing the CSP and `READ_RPCS`, which is deliberate. thirdweb's keyless endpoint was tried and dropped: it rate-limited (HTTP 429) and caps `eth_getLogs` at 1,000 blocks.

**Defences against phishing and typos:**
- The request checksum catches typos and truncation.
- The on-chain pre-checks catch a request for a wrong amount, index, token or network before anything is signed.
- The built-in **allowlist** names the known vault. Anything else shows a red "Unknown vault. Stop and check." panel with the full address and needs a ticked acknowledgement. A retired vault is refused.
- The wallet's own confirmation screen shows the contract and amount. The page tells the person what to compare.

**What a malicious copy of the page could do:**
- **Make you deposit to a different vault** (its own contract, keeping your funds). Defences:
  - the address shown in your wallet's confirmation;
  - this page's allowlist, if you are on the genuine page;
  - the vault address the Logos wallet put in the request.

  The Logos wallet detects it afterwards: the deposit never appears in the real vault's tree, so the claim finds no leaf for its commitment.
- **Make you deposit under a different commitment** to the real vault. The Logos wallet scans the real vault for its own `C` and does not find it. The funds are then locked under a commitment nobody can claim, unless the attacker made that commitment from their own secret, in which case the attacker can claim them. So this is theft-capable, exactly like any phishing page sending your ETH somewhere else.
- **Defence for both:** use the genuine page, at the address the operator publishes, or a local copy whose files match `MANIFEST.sha256`. Compare the vault address in your wallet with `0x26af59855b99a8af199361289A5Ed6088729E60E` (Sepolia demo v3, the page's default since 2026-11-14; v2 `0x8C8b2664De28e5287158db4DB65D448638871b74` is still recognised for its requests).

**What a malicious copy cannot do:**
- learn your Logos destination or secret;
- take a correct deposit away from you;
- change a deposit after you sign it;
- read anything your wallet does not expose.

**Browser extensions.** A malicious extension with access to all pages can rewrite this page and its transactions like any other web page. This is the residual risk Soft #5 names. The Logos-module path avoids it, and the web page cannot. The secret is never in the browser, so even then the destination stays private and a correct deposit stays claimable. A rewritten transaction is what the wallet's confirmation screen is for.

**Front-running** someone's `C` from the mempool or from a shared link only makes their deposit revert (`CommitmentUsed`). The attacker cannot claim their copy without `k`. If they copied the same token and amount, the victim's Logos wallet can claim the attacker's deposit; with another token or amount, nobody can. The page simulates right before the signature request and explains the case if it happens.

## Visual direction

**Part of the Names Wallet site.** The page uses the site's tokens (`/assets/tokens.css`: `--paper --card --ink --dim --line --orange --warn --bad --radius --font --font-head`), its η mark, favicons and heading font, and its header and footer. `app.css` defines no colour of its own. One block at its top maps the tokens to the names the rest of the file uses and mixes the few extra shades the page needs (sunken panels, tinted callouts, the empty cells of the commitment grid) from them with `color-mix()`.

- **No green, on purpose:** the site's `--green` and `--ok` are not used on these pages (the owner's call). Cream paper, near-black ink, and one accent, orange.
- **Roles:** ink for titles, text, passed checks (solid ink marks) and the commitment grid; orange for the primary button (ink text on it: 5.3:1 light; dark-paper text in dark), the current step, link underlines, and, darkened toward ink to hold 3:1 on paper, focus rings and the finality bar; `--warn` and `--bad` only for real cautions and errors (unknown vault, paused deposits, a used commitment, a failed check). Caution text is pulled toward ink in light so it stays above 4.5:1 on its tinted callout.
- **Two cards only:** the deposit and the next action. Details, checks, privacy, the crowd and the phone hand-off are plain sections under a rule.
- **The test network is a label, not a warning:** a "Sepolia testnet" tag in the header in muted text, "Sepolia" next to the vault address and the network, amounts written "0.01 Sepolia ETH", and one plain sentence under the deposit details. Mainnet is refused in code, so the page does not need to shout.
- **Type:** headings and the wordmark in Inter Bold, the site's own file (`/assets/fonts/Inter-Bold.woff2`, preloaded, `font-display: swap`; the CSP allows `font-src 'self'`); body text in the site's system font stack; the system monospace only for real data: addresses, hashes and the request. The amount is the headline.
- **Chrome:** the site's header (the η mark and "Names Wallet" linking home, then "Bridge" and a "Sepolia testnet" tag) and footer, and the site's favicons.
- **The one bold element:** the commitment drawn as its own 256 bits, a 16 × 16 grid in ink.
- **Motion:** a soft pulse while waiting, the finality bar, a spinner on running checks; all off under `prefers-reduced-motion`.
- **Accessibility:** keyboard reachable, 3 px focus ring, skip link, step changes announced and focused, check results in text, `role="progressbar"`, 44–48 px buttons. Contrast is measured on the rendered pages in headless Chrome, every text run against its real background, in both themes (`nameswallet-site/bridge-src/shoot.mjs`): all pass AA.

## Files, dependencies and sizes

**Deployable set** (the only files a host should serve; listed with hashes in `MANIFEST.sha256`):

```
index.html, 404.html, app.css   (the favicon, mark, font and colours are the site's, under /)
about/privacy/index.html   what a deposit makes public and what stays sealed
about/security/index.html  the trade-offs of a browser page
about/verify/index.html    how to check a hosted copy against MANIFEST.sha256
src/app.js                 the page: renders flow state, no innerHTML
src/lib/flow.js            state machine (DOM-free)
src/lib/request.js         deposit request format (standalone, reusable)
src/lib/vault.js           vault/ERC-20 calls, decoders, error table
src/lib/abi.js             minimal ABI encode/decode
src/lib/keccak.js          Keccak-256, selectors, EIP-55
src/lib/providers.js       EIP-6963 discovery, HTTP read provider, chunked log scan
src/lib/deployments.js     allowlist, chains, read RPCs, example commitments
src/lib/units.js           BigInt amount formatting
src/lib/qr.js              QR encoder (byte/alphanumeric, L/M, v1-20)
```

The set is 16 files and about 172 KB uncompressed, with **no third-party dependencies**. `node tools/manifest.mjs` rewrites `MANIFEST.sha256`. Not deployable: this README, `_headers` (read by a host, not served) and `assets/` (the local tokens stand-in).

**Not deployable** (never serve these):
- `dev/`: mock chain and wallet, screenshot harness, CDP screenshot driver, QR probe;
- `test/`;
- `tools/`: `make-request.mjs`, `gen-vectors.mjs`, `gen-fixtures.sh`, `manifest.mjs`;
- `package.json` (scripts only, no dependencies).

**Why no viem:**
- Its ESM build is hundreds of files and wants a bundler.
- The page needs a dozen calls with static arguments, plus one dynamic return (`tokenConfig`'s three `uint256[]`).
- About 150 lines of encoder, checked call by call against `cast calldata`, is a smaller thing to trust than a library.

**QR:**
- **Scanning** uses the browser's own `BarcodeDetector`, available in Chrome on Android and macOS. Where it is missing (Safari, Firefox), the button is hidden, and paste or the link is the path.
- **Generation** is the 307-line `src/lib/qr.js`, about 10 KB.

## Testing

`npm test` runs `node --test` on the four offline suites. No packages are installed.

| Suite | Tests | What |
|---|---:|---|
| `test/request.test.mjs` | 10 | All vectors, BIP-350 vectors, every single-character substitution caught (>4,000 mutations), 300 random round trips, field validation, fragment links |
| `test/abi.test.mjs` | 11 | Calldata against `cast calldata` for 14 calls (`test/fixtures/cast.json`, from `tools/gen-fixtures.sh`); 13 error selectors against `cast sig`; the vault's errors, `Deposit` event and functions against the forge ABI in `vault/out`; Keccak against `cast keccak` (0–300 bytes, across the 136-byte block boundary); EIP-55 against `cast to-check-sum-address`; `tokenConfig` decoding against `cast abi-encode`; revert data in every wallet shape |
| `test/units.test.mjs` | 5 | Formatting, parsing, rounding, display helpers |
| `test/create.test.mjs` | 4 | The commitment against the real CLI (ETH and dUSDC, public, private, random and leading-zero accounts); Logos account parsing; fresh secrets; the claim file's schema and self-consistency |
| `test/create-flow.test.mjs` | 6 | Made on the page through the real flow: connect, choose, account, save, deposit, receipt; the token path; wrong account formats; changing the amount after saving (new secret, new save); reload after sending (resumes from public facts, no secret); a vault with another cfgId refused. The secret is checked absent from every wallet request and storage write |
| `test/wallets.test.mjs` | 8 | Discovery: legacy only, EIP-6963 only, both (one entry), several, late announce, legacy injected late, icon sanitising, no wallet |
| `test/desk-flow.test.mjs` | 15 | The deposit desk: the switch is off as shipped, selectors pinned to the Solidity test, `bpsText`, public-key checks (P2 vector, curve, wrong account), ETH and dUSDC through the desk (exact approval of the desk for denomination + 3%), the fee in the balance check, a desk with another vault or no code, a fee change after the checks, made here with the demo account and with an own key, a wrong key blocks, two quick edits (the later draft wins), desk off calls the vault directly, the page's fee wording |
| `test/flow.test.mjs` | 26 (+8 subtests) | The real state machine with a mock EIP-1193 wallet over an in-memory chain that follows the vault's deposit rules (`dev/mock-chain.js`) |

**What the flow test covers** (`test/flow.test.mjs`):
- the full ETH path: wrong network → switch → checks → deposit → receipt → finality;
- the full dUSDC path, asserting the approval is for exactly 100 dUSDC;
- a declined deposit; error 4902;
- each pre-check failure: amount mismatch, bad index, unregistered token, global and per-token freeze, cap, ETH and token balance;
- reload recovery of the person's own deposit;
- a used commitment for another token;
- a front-run between the checks and the signature;
- the finality fallback;
- an unknown vault without a contract, and one with a contract that needs acknowledgement;
- example requests never sent;
- the retired vault;
- a bad checksum;
- read-only RPC mode, where nothing is sent;
- a silent network change after the checks: nothing is sent;
- a flaky RPC during finality: tracking continues;
- silent reconnect only when the wallet already shares an account, never a prompt.

**Sepolia only** (`test/flow.test.mjs`): a request for Ethereum mainnet or Hoodi is refused at load, before any read, even for an acknowledged unknown vault, and nothing is ever read or sent; a wallet on mainnet is asked to switch to Sepolia (`0xaa36a7`, the only switch ever requested) and nothing is sent until it does; amounts are labelled `Sepolia ETH` and the demo token is named a Sepolia test token.

**Wallet behaviour** (`test/flow.test.mjs`): 4902 adds Sepolia (EIP-3085) and carries on, a declined add, account change, an empty account list and `disconnect` forget the wallet, a locked wallet, a declined connection.

**Result on 11-13:** 77 passed, 0 failed. After the deposit desk (11-13 evening): 104 passed, 0 failed. At main `74c55c3` (2026-11-14): 102 passed, 0 failed.

**Live, read-only**, on the Sepolia v2 vault (`LIVE=1 node --test test/live.sepolia.test.mjs`): 9 of 9 passed through PublicNode on 11-13. Only `eth_call`, `eth_estimateGas`, `eth_getLogs` and other reads were used.

| What was read | Value |
|---|---|
| Code size | 20,614 bytes |
| `cfgId` | `0x7f1a…4b11`, the deployment value |
| ETH and dUSDC configs | registered, not frozen, decimals 18 / 6, denominations 0.01/0.1/1 ETH and 100/1,000/10,000 dUSDC, deposit fees 0, release delay 86,400 s |
| Caps | per token, 1-day window: 5 ETH and 50,000 dUSDC. Usage estimate now: 0.01 ETH and 1,300 dUSDC. Weights 3e9 / 1e18. Global cap 100,000 with 1,330 used |
| `depositCount` | 5 |
| `Deposit` events | exactly 5. Leaves 0–4 match the token, index and commitment in `DEPLOYMENTS.md` |
| `counters` per denomination | ETH 1/0/0; dUSDC 3/1/0, equal to the event tally |
| Known commitments | `commitmentUsed` is true for all 5; the three example commitments are unused |
| `depositETH` simulation with a used commitment | reverts `0x7b000717` (`CommitmentUsed()`) |
| The real `createFlow` against PublicNode | every vault check passes on the example request; anonymity set 1 of 5 |
| Exact `depositETH(0, fresh C)` calldata, simulated from the funded demo deployer with 0.01 ETH | returns one word, leaf index 5 (= `depositCount`); `eth_estimateGas` 390,080 (DEPLOYMENTS.md recorded 428,510 for a fresh commitment on 11-13) |
| ERC-20 `deposit` simulated from an address with no allowance | the live dUSDC reverts `0xfb8f41b2`, which the page decodes as `insufficient-allowance` |

thirdweb's endpoint failed the log and later tests with HTTP 429, which is why it is not offered.

**In a real browser** (installed Google Chrome, headless, a throwaway profile, driven over the DevTools protocol by `dev/shoot.mjs`):
- The real `index.html`, with the PublicNode read RPC, ran every vault check against live Sepolia.
- **QR codes:**
  - `dev/qr-probe.js` encoded 7 inputs × 2 error-correction levels with `src/lib/qr.js`, including request strings, links and a 300-byte string. Chrome's `BarcodeDetector` decoded all **14 of 14** back to the exact input: versions 1–13, byte and alphanumeric modes, version-info blocks from version 7.
  - `dev/qr-svg-probe.js` decoded the SVG QR the page actually rendered under *Continue on your phone*, and it matched the link exactly.
- **Focus:** with keyboard focus on the receipt's *Copy* button during `finalizing`, two finality updates later the button still had focus and was still in the document.
- **Screenshots** of every state at 390 px (phone emulation, 2×) and 1280 px, light and dark, via `dev/harness.html?s=<state>`. That is the real app with a scripted mock wallet announced over EIP-6963. The driver prints each shot's action heading, so the state is confirmed, not assumed from the file name. All 21 phone states were shot, and some at 1280 px and in dark:

  | Group | States |
  |---|---|
  | Request and wallet | `empty`, `invalid`, `nowallet`, `connect`, `network` |
  | Ready | `ready`, `approve`, `usdc-ready` |
  | In flight | `approve-wait`, `sign`, `deposit-wait` |
  | Blocked | `mismatch`, `frozen`, `nousdc`, `unknown` |
  | After the deposit | `finalizing`, `done`, `failed` |
  | Other | `example`, `settings`, `handoff` |

  The `estimate` scenario (finality without the `finalized` tag) exists but needs about 30 s of waiting and was not shot. Run `node dev/shoot.mjs --out <dir> <states…>` with the local server on port 8731, or pass `--base`.

**Not tested:**
- with a real MetaMask or any real wallet;
- camera scanning (`BarcodeDetector` on a live camera); only decoding of generated images was tested;
- any transaction on any chain (no key was available, and the deployer key was not used);
- Safari and Firefox;
- a real screen reader.

## Cost

The page is pure static files with no Worker or server code, so a request costs nothing beyond static asset serving.

| Path | Kind | Worst case under a bot flood |
|---|---|---|
| `/`, `/index.html`, `/app.css`, `/src/**`, `/about/**` | static assets | free (static asset requests are not billed) |
| chain reads | the visitor's wallet provider, or PublicNode from the visitor's browser | not ours |

Nothing in the page writes anywhere. Deploy with `workers_dev: false` and `preview_urls: false` if it ever goes behind a Worker-based static host, and keep `run_worker_first` off.

## Not done

- **Never run with a real wallet or a real transaction.** Do the MetaMask steps above before trusting it.
- **No WalletConnect.** Phone wallets work through their in-app browser only. WalletConnect needs a relay service, which this static, no-third-party design leaves out.
- **The Names Wallet hand-off is new and not yet run on a chain.** The `lez_bridge` module produces the code (`make_deposit_request` / `make_public_deposit_request`: `request`, `request_url`); no deposit through a code is recorded yet. `tools/make-request.mjs` still makes codes by hand. The commitment grid is specified, but no wallet draws it.
- **QR scanning only in browsers with `BarcodeDetector`.** That is Chrome on Android and macOS, not Safari or Firefox. A JS decoder was over the ~50 KB budget.
- **Splitting a larger amount into several denominations and staggering them** (DESIGN §3.1) is not offered. One request is one deposit.
- **Hosted on the Names Wallet site only; no independently published manifest, no signed releases.** A deployment should publish `MANIFEST.sha256` somewhere independent of the host; `about/verify/` says plainly that none is published yet.
- **The only read-RPC preset is PublicNode,** and the allowlist knows only Sepolia demo v3 (the default) and v2.
- **Not checked with a screen reader.**
