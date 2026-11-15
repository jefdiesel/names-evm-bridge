# LNS names as a bridge destination

`src/lib/lns.js` lets the deposit page take an LNS name (bare, `alice`) where a person types the
LEZ account to be paid. It resolves the name in the browser to the **public account its holder
published**, straight from the LEZ 0.4 bridge test network's JSON-RPC. Tests: `test/lns.test.mjs`
(14, offline). Vectors: `test/vectors/lns-v1.json`, made by `tools/lns-vectors.mjs`. Written
2026-11-14.

**Wired in on 2026-11-15** (branch `lns-page`): the claim-file path's account step takes a name.
Argon2id is `src/lib/vendor/argon2id.js` (`src/lib/lnsderive.js` calls it; checked by
`test/argon2id.test.mjs`); the page's CSP allows `https://lez.nameswallet.app`; the RPC filter
answers CORS for `https://nameswallet.app` (`public-demo/proxy/lez-rpc-proxy.py --cors-origin`),
live once the filter and the edge rule are deployed (`public-demo/RUNBOOK.md` (not published) §17). Section 5 below
is the record of what blocked it.

Sources, read at these commits (read-only):

- **logos-names `rc3-port` `045a6fd`**: the registry deployed on this chain (`chain.js` of
  web-register-04 says so). `core/src/lib.rs`, `seal/src/lib.rs`, `cli/src/main.rs`, `SPEC.md`.
- **logos-names `web-register-04` `eab4932`**: the LNS web page for 0.4, the browser reference.
  `nameswallet-lns/lns-src/page/src/names.js` and `chain.js`.
- **logos-names-wallet-next `48ba0ee`**: the wallet's payment rule.

## 1. The format

| Fact | Source |
|---|---|
| A name is 1–32 bytes of `a-z 0-9 -`, not starting or ending with `-` | core `validate_name` L603; SPEC §2 L84 |
| Normalising: trim, lower **ASCII** letters only, drop one `lns:`, then one `@`, then one trailing `.lez` | core `normalize_name` L622; SPEC §2 |
| Name or account: the input is tried as an account id first (base58, 32 bytes). A name that also decodes as an id is refused (so `lns:` + 32 ones is invalid, not an account) | SPEC §2 ("Name or address"); cli `parse_name` L836–844 |
| Every 1–4 character name (and 21,718 reserved ones) is locked for **registration**: only the admin registers it, then transfers it. Locked names can still be held, so lookup takes 1–32 | SPEC §8.1 L471–482 |
| `master = Argon2id(name, salt "logos-names/v5", v0x13, 64 MiB, 3 passes, 1 lane, 32 B)` | seal L19–23, `derive` L79, `master` L86–95; SPEC §3 L107 |
| `id = SHA-256("logos-names/v5/id" ‖ master)`, `key = SHA-256("logos-names/v5/key" ‖ master)` | seal L24–25, L79–84 |
| Record account = `PDA(registry, SHA-256("logos-names/v5/name/" ‖ id))`, with PDA = `SHA-256("/LEE/v0.2/AccountId/PDA/" ‖ 8 zero bytes ‖ program ‖ seed)` | core `record_address` L580, `seed` L443; `public-demo/WALLET-READ.md` (not published) §3 |
| Record = borsh `NameRecord { version u8, owner [32] (a tag, not an account), expires u64 ms, sealed Vec<u8>, claimed bool }`, version 6 on 0.4 (3–5 end at `sealed`) | core L127–140, `VERSION` L63 |
| `expires = u64::MAX`: never lapses (deposit mode, this registry's mode). Otherwise lapsed after `expires`; not registered after `expires + 30 days` | core `GRACE_MS` L112; cli `lookup` L1393–1398 |
| `sealed = nonce(12) ‖ AES-256-GCM(key, aad "logos-names/v5/record")` | seal `open` L234, L26 |
| Opened: borsh `Profile { name, account Option<[32]>, npk Option<[32]>, vpk Vec<u8>, texts Vec<{key,value}> }`, zero-padded to 256 B blocks. It counts only if `profile.name` is the name | seal `Profile` L51, `open_profile` L165 |
| `texts.forward` sends a payer to another name, one hop only, and must be shown (`brand → alice`) | SPEC "Forwarding" L324; cli `forward_of` L830, `following` L1379 |

**Argon2id is required for a lookup.** It is not only for the registration commitment: the
`id`, and so the record's address, and the `key` that opens the record both come from `master`.
Without Argon2id a browser can neither find the record nor read the account in it. The reference
browser code computes it in plain JS (`@noble/hashes` 2.x argon2 bundled into `names.js` L978–1341, on top of its utils, `_u64` and blake2b, L3–977;
`DERIVE_PARAMS` L1347, `deriveName` L1452): **481 ms** in Node 24 on this Mac. Its harness
`lns-src/harness/argon2-speed.mjs` times it in Chrome with CPU throttling ×1/×4/×6. No timings
from that harness are committed.

## 2. Which account a payer uses

**The rule implemented: the profile's `account` (the public account the holder published), and
nothing else.** If it is unset, the result is `no_public_account`.

- The holder's own account is not on chain. The record keeps
  `owner_tag = SHA-256("logos-names/v5/owner" ‖ id ‖ account)` (core `owner_tag` L474), which no
  one can turn back into an account. So "pay the holder" is not possible. On 0.4 the account that
  registered a name holds it, but only as this tag.
- This is what the CLI does. `names pay --public` and `payee` take `profile.account` and fail with
  "has no public account set" when it is unset (cli `pay` L2218, `payee` L2532, `pays_publicly`
  L2601). The wallet's `recipient` gives `public` (the profile's account, or null) beside the private keys,
  and follows a forward one hop (wallet `lib.rs` L145–157). Its `pay_to` takes a name or an explicit
  `Public/<id>` (`cli.rs` L3661).
- A name that publishes only private keys (`npk`/`vpk`) gives a bridge destination nothing to pay.
  The bridge commits to an account id, and private payment to a name is a different flow. Such a
  name is refused with "accepts private payments only". **Both names we know of on this chain are of
  this kind** (section 3). So most names registered the default way will be refused until their
  holders set a public account.

## 3. What is verified against the live chain, and what is inferred

Read-only calls only (`getChannelId`, `getAccount`, `getLastBlockId`), 2026-11-14, channel
`2da7f7df…5374`, around block 6232, through `https://lez.nameswallet.app/`.

| Check | Status |
|---|---|
| Record account from `id`: `m2-test-111` → `81uGmBmYE3pVwYooieD1ZSbuuKxEgQ5CUVdBxVDABTMo`. Same as the reference `recordAddress`, and the chain holds a record there | **verified live** (fixture) |
| Records of `m2-test-111` and `lns-smoke-ourzerofour`: 1354 bytes, decoded exactly: version 6, `expires` = u64::MAX, `claimed` false, sealed 1308 B. They open with the name's key to a profile naming that name, with `account` none and `npk` + 1184-byte `vpk` set | **verified live** (raw bytes in the fixture) |
| `zq7-rehearsal-04a` (an earlier rehearsal name) has no record here: `getAccount` → `null` → `not_found` | **verified live** |
| The clock account `4Bdcjo…RWNT` under `2hwaRW…157Z` holds `[block u64, time ms u64]`. Its block equals `getLastBlockId` | **verified live** |
| JSON-RPC batches of 4 are served (the proxy's `MAX_BATCH` 20; `WALLET-READ.md` §1 still says batches are refused, which is out of date) | **verified live** |
| A name **with** a public account resolves to it | **not verified live**: no such name exists on this chain. Covered by the registry's own vector (SPEC §10 sealed profile of `jef`, account = 32 × `0x02`) and by fake-chain tests |
| Argon2id itself (`id`/`key` from a name) | **not in this module**. The fixture's `id`/`key` come from the reference `deriveName`, and the record found at their address shows they match the registry |
| Forwarding, expiry, the "lapsed vs past grace" split | inferred from code (SPEC, cli). Fake-chain tests only; no live name forwards or expires |
| A bridge claim to a public account that has never been used on chain | **verified live** (2026-11-14): relayed claims into never-used accounts with 0 LEZ, LEZ blocks 4802 and 4852 (`vault/DEPLOYMENTS.md`, v3). The CLI still refuses a plain transfer to such an account (cli L2224–2239); the bridge's mint does not |

## 4. The resolver API (what the integrating worker calls)

```js
import { normalizeName, resolve, confirmSame, confirmationLine } from './lib/lns.js';

const typed = normalizeName(input);    // {kind:'account', id, label} | {kind:'name', name} | {kind:'invalid', message}
// 'account' → the page's existing parseLogosAccount(input), unchanged.
const r = await resolve(typed.name, { derive });   // derive(name) -> {id, key}: Argon2id of SPEC §3 (see 5)
// r.ok → show confirmationLine(r); keep r. Bind the deposit to r.accountId (bare base58), never to the name.
// just before asking the wallet to sign the deposit:
const c = await confirmSame(r, { derive });
// c.ok → deposit to c.accountId; otherwise show c.message and stop (c.code 'changed' or 'rpc_error').
```

- `resolve` → `{ ok: true, name, forwardedFrom, account: 'Public/<id>', accountId, record:{account, version, ownerTag, expires, claimed, payablePrivately}, channel, block, blockTime }`
  or `{ ok: false, code, message, [channel, block, blockTime] }`. Codes: `invalid_name`,
  `not_found`, `no_public_account`, `expired`, `forward_not_found`, `bad_record`, `rpc_error`.
  Only `confirmSame` returns `changed`.
- **Read at one block** (inferred: it assumes the sequencer answers a batch's calls in order, which
  was not checked; at worst `block` is one off, and `confirmSame` is unaffected). One JSON-RPC batch: `[getChannelId, getAccount(clock), getAccount(record),
  getAccount(clock)]`. If the two clock reads differ, a block landed in between, and the batch is
  retried (3 tries, then `rpc_error`). `block` is the height the record was read at. `blockTime`
  is the chain's own time, which judges expiry; the device's clock is not used.
- **Bound before deposit.** `confirmSame` resolves the asked name again and passes only for the same
  account, the same forward, the same channel, at the same block or later. A changed account, a
  re-genesis (new channel), an older block, a name released since or a new forward target, even
  to the same account, gives `changed`. `rpc_error` is never a pass.
- **Nothing kept.** No storage of any kind; no derivation or read is cached; every call reads the
  chain. Nothing runs at import time. The module has no imports.
- **Only account ids go to the network,** never the name. The tests assert this on every request.
- `derive` is required, and `resolve` throws a `TypeError` without it. Argon2id takes about half a
  second, and a forward needs a second one. The caller may memoise `derive` for the page's
  lifetime, in memory only.

## 5. Blocking for integration (not for this commit)

1. **Argon2id in the browser.** Options:
   - **(a)** Vendor the reference's bundled `@noble/hashes` argon2id with the utils, `_u64` and blake2b it
     needs (MIT; **argon2 is outside noble's audit scope**, its README says, though blake2b and the rest are audited; the same code the LNS web page ships; `names.js` L3–1341, about 1,340 lines
     unminified; checked by SPEC §10's RFC 9106 vector and
     the five derivation vectors). This breaks the README's "no vendored libraries" rule. The
     CSP's `worker-src 'none'` keeps it on the main thread: about 0.5 s and 64 MiB per name on a
     laptop, more on a phone. `asyncTick` keeps the page responsive.
   - **(b)** Let Names Wallet resolve the name and hand the page an account, as deposit codes
     already do. The page takes no names.
   - **(c)** Resolve on a server. This is out: every lookup would bill a Worker, and the server
     would see who looks up which name (SPEC §12.3 forbids a relay for this reason).

   Recommended: (a) if names must be typed on the page, else (b).
2. **CORS.** The RPC does not serve browsers today. Tested 2026-11-14 with curl:
   - `OPTIONS /` with `Origin: https://nameswallet.app` (and `bridge.nameswallet.app`,
     `example.com`) → **HTTP 403 at the edge**.
   - `POST` with that Origin → 200, but **no `Access-Control-Allow-Origin`**.

   So a page on `nameswallet.app` cannot read `lez.nameswallet.app`, which is a different origin. A
   "simple" `text/plain` POST is no way around this: it is refused (403), and its answer would be
   unreadable anyway. The fix already exists: `filter/cors.patch` and `--cors-origin` in
   logos-names web-register-04 `nameswallet-lns/lns-src/filter/`, plus an edge rule that lets
   `OPTIONS /` through. Allow `https://nameswallet.app` only.
3. **Allow-list.** Today's allow-list suffices: the resolver uses only `getChannelId` and
   `getAccount`, in a batch of 4 (≤ 20). Each lookup costs 4 requests of the per-IP bucket, 8 with
   a forward, plus the same again for `confirmSame`. That is far under 5 a second, burst 60.
4. **CSP.** `connect-src` in `index.html` and `_headers` lists only
   `https://ethereum-sepolia-rpc.publicnode.com`. Add `https://lez.nameswallet.app`, and do the
   same in nameswallet-site's `bridge-src/bridge.headers`.
5. **Wiring.** `npm test` lists test files by name: add `test/lns.test.mjs`. Add `src/lib/lns.js`
   (and an argon2 file if (a) is chosen) to `MANIFEST.sha256` (`npm run manifest`). Regenerate the
   vectors with `node tools/lns-vectors.mjs --ref <logos-names>/nameswallet-lns/lns-src/page/src/names.js`.

## 6. Cost

Reads go **straight from the visitor's browser to the chain's RPC** (`lez.nameswallet.app`, the
filter on our own machine behind the Cloudflare tunnel). There is no Worker and no new route, so a
lookup costs nothing per request. A bot flood hits the edge's 50-per-10-s rate rule and the
filter's buckets, not a bill. The page's static files are unchanged in kind.

Privacy: the filter and the edge see the visitor's IP and the record account read. A dictionary of
names turns that back into a name, at one Argon2id per guess. That is the same exposure as the
LNS page on this chain. The page and this module never send the name itself.

## 7. Wording for the page

- **Label:** `Your LEZ 0.4 account or LNS name`
- **Placeholder:** `Public/… or a name like alice`
- **Under "Which account, and why":** `An LNS name works only if it is registered on LEZ 0.4 and its holder published a public account; the deposit is bound to that account, not to the name.` and `The page checks the exact name you type. Names cannot be searched or listed. …`
- **While checking:** `Checking alice on LEZ 0.4…` ("check", never "look up": LNS has no directory; the page can only check the one exact name typed, owner 2026-11-15)
- **Confirmation** (`confirmationLine`): `alice → Public/27rmGq…24oB on LEZ 0.4. The deposit is bound to this account, not to the name.`
  With a forward: `brand → alice → Public/27rmGq…24oB on LEZ 0.4. The deposit is bound to this account, not to the name.`
  It is the holder's **published public account**. Show the full `Public/<id>` on tap or hover,
  and in the review step before signing.
- **Refusals** (the `message` of each code):
  - `invalid_name` / invalid input: `That is neither an LNS name nor a LEZ account. A name is up to 32 letters, digits or dashes, like alice.`
  - `not_found`: `No such name on LEZ 0.4.`
  - `no_public_account`: `The LNS name alice accepts private payments only, so the bridge can't pay it. Paste a LEZ account instead.` (or `… has no public account set, …`)
  - `expired`: `The LNS name alice has expired. Ask its holder for a LEZ account.`
  - `forward_not_found`: `brand points to alice, which isn't registered. Ask its holder for a LEZ account.`
  - `bad_record`: `The record of alice can't be read (…). Paste the LEZ account instead.`
  - `rpc_error`: `The LEZ 0.4 network can't be read now: <reason>.` Keep the typed name, offer "Try again", never guess.
  - `changed` (from `confirmSame`, at deposit time): `The LNS name alice changed since you checked it: it now goes to Public/…. Nothing was sent. Check the name with its holder.` (or `…: the LEZ 0.4 network was reset.`)
