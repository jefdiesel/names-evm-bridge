// LNS names as a bridge destination: from a name a person types (`alice`) to the public LEZ
// account its holder published, read straight from the LEZ 0.4 bridge test network's JSON-RPC.
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Format facts and their sources: ../../LNS.md. In short (logos-names, branch rc3-port, 045a6fd):
//   name  --Argon2id(64 MiB, 3 passes, salt "logos-names/v5")-->  master          (SPEC.md §3)
//   id    = SHA-256("logos-names/v5/id"  ‖ master)      what the chain knows the name by
//   key   = SHA-256("logos-names/v5/key" ‖ master)      opens the record; never sent anywhere
//   record account = PDA(registry, SHA-256("logos-names/v5/name/" ‖ id))   (core record_address)
//   record = borsh NameRecord { version u8, owner tag [32], expires u64 ms, sealed Vec<u8>, claimed bool }
//   sealed = nonce(12) ‖ AES-256-GCM(key, aad "logos-names/v5/record") of a borsh Profile
//            { name, account Option<[32]>, npk Option<[32]>, vpk Vec<u8>, texts Vec<(key, value)> }
//
// The public account a payer may use is the profile's `account`, set by the holder. The holder's
// own account is not on chain (the record keeps only a tag of it), so there is nothing else to pay.
//
// This module does NOT compute Argon2id. `resolve` takes `derive(name) -> {id, key}` (two 32-byte
// Uint8Arrays) from its caller. Everything else uses crypto.subtle (SHA-256, AES-GCM).
// No network at import time, no storage of any kind: every resolve reads the chain again.

export const LNS_RPC = 'https://lez.nameswallet.app/';
/** The LNS names registry on the LEZ 0.4 bridge test network (rc3-port, deployed 2026-11-13). */
export const LNS_PROGRAM = '6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA';
export const NAME_MAX = 32;
/** How long after `expires` a name is still held (and then not registered): 30 days, core GRACE_MS. */
export const GRACE_MS = 30n * 24n * 60n * 60n * 1000n;
const U64_MAX = 2n ** 64n - 1n;
const FORWARD_KEY = 'forward';
const RECORD_AAD = 'logos-names/v5/record';
const NAME_SEED_DOMAIN = 'logos-names/v5/name/';
const NAME_PATTERN = /^(?!-)[a-z0-9-]{1,32}(?<!-)$/;
const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const utf8 = new TextEncoder();

// ---------------------------------------------------------------------------------------------
// bytes

function concat(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}
const toHex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
async function sha256(bytes) {
  return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', bytes));
}

export function base58Encode(bytes) {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let s = '';
  while (n > 0n) { s = ALPHABET[Number(n % 58n)] + s; n /= 58n; }
  for (const b of bytes) { if (b !== 0) break; s = '1' + s; }
  return s;
}

/** base58 → bytes, or null when a character is not base58. */
export function base58Decode(text) {
  let n = 0n;
  for (const ch of text) {
    const v = ALPHABET.indexOf(ch);
    if (v < 0) return null;
    n = n * 58n + BigInt(v);
  }
  const bytes = [];
  while (n > 0n) { bytes.unshift(Number(n & 0xffn)); n >>= 8n; }
  for (const ch of text) { if (ch !== '1') break; bytes.unshift(0); }
  return Uint8Array.from(bytes);
}

const isAccountId = (text) => text.length > 0 && base58Decode(text)?.length === 32;

// ---------------------------------------------------------------------------------------------
// what was typed

/**
 * What the destination field holds. Per SPEC.md §2 the input is first tried as a LEZ account id
 * (base58 of 32 bytes, optionally labelled `Public/` or `Private/`), and only otherwise as a name;
 * a name that also decodes as an account id is never a name.
 *   { kind: 'account', id, label: 'Public' | 'Private' | '' }   hand to the page's account parser
 *   { kind: 'name', name }                                      resolve it
 *   { kind: 'invalid', message }
 * Name normalisation is SPEC §2 / core normalize_name: trim, lower ASCII letters only, drop one
 * leading `lns:`, then one `@`, then one trailing `.lez`; then 1–32 of a-z 0-9 '-', not starting
 * or ending with '-'. (Names of 1–4 characters are locked for registration on 0.4, but some are
 * held, so lookup accepts the whole 1–32 range.)
 */
export function normalizeName(input) {
  const raw = String(input ?? '').trim();
  if (!raw) return { kind: 'invalid', message: 'Type an LNS name or paste a LEZ account.' };
  const labelled = raw.match(/^(public|private)\/(.*)$/i);
  if (labelled) {
    const id = labelled[2].trim();
    if (!isAccountId(id)) return { kind: 'invalid', message: 'That LEZ account looks incomplete. Copy it again from Names Wallet.' };
    return { kind: 'account', id, label: labelled[1][0].toUpperCase() + labelled[1].slice(1).toLowerCase() };
  }
  if (isAccountId(raw)) return { kind: 'account', id: raw, label: '' };
  let bare = raw.replace(/[A-Z]/g, (c) => c.toLowerCase());
  if (bare.startsWith('lns:')) bare = bare.slice(4);
  if (bare.startsWith('@')) bare = bare.slice(1);
  if (bare.endsWith('.lez')) bare = bare.slice(0, -4);
  if (!NAME_PATTERN.test(bare)) {
    return { kind: 'invalid', message: 'That is neither an LNS name nor a LEZ account. A name is up to 32 letters, digits or dashes, like alice.' };
  }
  // SPEC §2 / cli parse_name: a name that is also an account id is refused, never read as either.
  if (isAccountId(bare)) return { kind: 'invalid', message: 'That is neither an LNS name nor a LEZ account. Paste the account as Public/… or type the name alone.' };
  return { kind: 'name', name: bare };
}

// ---------------------------------------------------------------------------------------------
// the record

/** The record account of the name whose id (32 bytes, from `derive`) is `id`: core record_address. */
export async function nameRecordAccount(id, program = LNS_PROGRAM) {
  if (!(id instanceof Uint8Array) || id.length !== 32) throw new UsageError('a name id is 32 bytes');
  const programBytes = base58Decode(program);
  if (programBytes?.length !== 32) throw new UsageError(`not a program id: ${program}`);
  const prefix = new Uint8Array(32);
  prefix.set(utf8.encode('/LEE/v0.2/AccountId/PDA/'));
  const seed = await sha256(concat(utf8.encode(NAME_SEED_DOMAIN), id));
  return base58Encode(await sha256(concat(prefix, programBytes, seed)));
}

class Reader {
  constructor(bytes, what) { this.b = bytes; this.o = 0; this.what = what; }
  take(n) {
    if (n > this.b.length - this.o) throw new Error(`${this.what} is truncated`);
    const s = this.b.subarray(this.o, this.o + n);
    this.o += n;
    return s;
  }
  u8() { return this.take(1)[0]; }
  u32() { const [a, b, c, d] = this.take(4); return (a | (b << 8) | (c << 16) | (d << 24)) >>> 0; }
  u64() { return Array.from(this.take(8)).reduceRight((v, x) => (v << 8n) | BigInt(x), 0n); }
  bool() { const v = this.u8(); if (v > 1) throw new Error(`${this.what}: not a boolean`); return v === 1; }
  vec() { return this.take(this.u32()); }
  string() { return new TextDecoder('utf-8', { fatal: true }).decode(this.vec()); }
  option(read) { const t = this.u8(); if (t > 1) throw new Error(`${this.what}: bad option tag`); return t ? read() : null; }
  get rest() { return this.b.length - this.o; }
}

/**
 * A NameRecord (core/src/lib.rs `NameRecord`), fail-closed: an unknown version or a byte too many
 * or too few throws. Versions 3–5 end at `sealed`; version 6 (LEZ 0.4) adds `claimed`.
 * `expires` is a BigInt of ms since the epoch, or null for a name that never lapses (u64::MAX,
 * deposit mode, which is how this registry runs).
 */
export function decodeRecord(bytes) {
  const r = new Reader(Uint8Array.from(bytes), 'the name record');
  const version = r.u8();
  if (version < 3 || version > 6) throw new Error(`the name record has unknown version ${version}`);
  const ownerTag = toHex(r.take(32));
  const expiresRaw = r.u64();
  const sealed = Uint8Array.from(r.vec());
  const claimed = version >= 6 ? r.bool() : false;
  if (r.rest) throw new Error(`the name record has ${r.rest} bytes too many`);
  return { version, ownerTag, expires: expiresRaw === U64_MAX ? null : expiresRaw, sealed, claimed };
}

/**
 * A Profile (seal/src/lib.rs `Profile`) from the start of an opened record. As in the reference
 * (`open_profile`), what follows the profile is padding; here it must also be zeros.
 */
export function decodeProfile(bytes) {
  const r = new Reader(bytes, 'the profile');
  const name = r.string();
  const account = r.option(() => base58Encode(r.take(32)));
  const npk = r.option(() => toHex(r.take(32)));
  const vpk = r.vec();
  const texts = {};
  const count = r.u32();
  if (count > 16) throw new Error('the profile has too many entries');
  for (let i = 0; i < count; i += 1) {
    const key = r.string();
    texts[key] = r.string();
  }
  if (bytes.subarray(r.o).some((x) => x !== 0)) throw new Error('the profile is followed by data');
  return { name, account, payablePrivately: npk !== null && vpk.length > 0, texts };
}

/**
 * Opens a record's sealed profile with the name's key. Null when there is nothing set: empty,
 * does not open (AES-GCM refuses it), does not decode, or names another name (seal open_profile).
 */
export async function openProfile(key, sealed, name) {
  if (!(key instanceof Uint8Array) || key.length !== 32) throw new UsageError('a name key is 32 bytes');
  if (sealed.length < 12 + 16) return null;
  let plain;
  try {
    const k = await globalThis.crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['decrypt']);
    plain = new Uint8Array(await globalThis.crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: sealed.subarray(0, 12), additionalData: utf8.encode(RECORD_AAD) }, k, sealed.subarray(12)));
  } catch { return null; }
  try {
    const profile = decodeProfile(plain);
    return profile.name === name ? profile : null;
  } catch { return null; }
}

// ---------------------------------------------------------------------------------------------
// the chain

/** LEZ 0.4's clock: account `/LEZ/ClockProgramAccount/0000000` under the built-in clock program. */
export const CLOCK_ACCOUNT = '4BdcjoXkq786TMWcBGGHqcxeLYMZmn17rL4eM9ZyRWNT';
export const CLOCK_PROGRAM = '2hwaRW8sMnyKLLRbDRw8k68tiv9bnfq98oxcdtcp157Z';

class RpcFailure extends Error {}
/** A caller's mistake (a missing or wrong `derive`, a bad id): thrown, never turned into a refusal. */
class UsageError extends TypeError {}

function clockOf(result) {
  const s = result?.data?.actor_states?.[CLOCK_PROGRAM];
  if (!Array.isArray(s) || s.length < 16) throw new RpcFailure("the chain's clock can't be read");
  const b = Uint8Array.from(s);
  const u64 = (o) => Array.from(b.subarray(o, o + 8)).reduceRight((v, x) => (v << 8n) | BigInt(x), 0n);
  return { block: Number(u64(0)), time: u64(8) };
}

/** One JSON-RPC batch; the answers in the order asked (matched by id). */
async function batch(rpc, fetchFn, calls) {
  const body = calls.map(([method, params], i) => ({ jsonrpc: '2.0', id: i + 1, method, params }));
  let response;
  try {
    response = await fetchFn(rpc, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      referrerPolicy: 'no-referrer',
      credentials: 'omit',
    });
  } catch (e) {
    throw new RpcFailure(`the network can't be reached (${e?.message ?? e})`);
  }
  let text;
  try { text = await response.text(); } catch (e) {
    throw new RpcFailure(`the network's answer was cut off (${e?.message ?? e})`);
  }
  if (response.status === 429 || response.status === 503) {
    const wait = Number(response.headers?.get?.('Retry-After')) || 10;
    throw new RpcFailure(`the network is busy; try again in ${wait} s`);
  }
  let answers;
  try { answers = JSON.parse(text); } catch { throw new RpcFailure(`the network answered HTTP ${response.status}`); }
  if (!Array.isArray(answers)) throw new RpcFailure(answers?.error?.message ?? `the network answered HTTP ${response.status}`);
  return calls.map((_, i) => {
    const a = answers.find((x) => x && x.id === i + 1);
    if (!a) throw new RpcFailure('the network left a call unanswered');
    if (a.error) throw new RpcFailure(`${calls[i][0]}: ${a.error.message}`);
    return a.result;
  });
}

/**
 * Reads `accounts` at one block: [channel, clock, ...accounts, clock] in one batch, retried
 * while the two clock reads differ (a block landed in between). Returns the channel, the block
 * and its time, and each account's `getAccount` result (null when not on the chain).
 */
async function readAtOneBlock(rpc, fetchFn, accounts) {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const out = await batch(rpc, fetchFn, [
      ['getChannelId', []], ['getAccount', [CLOCK_ACCOUNT]],
      ...accounts.map((a) => ['getAccount', [a]]), ['getAccount', [CLOCK_ACCOUNT]],
    ]);
    const before = clockOf(out[1]);
    const after = clockOf(out[out.length - 1]);
    if (before.block === after.block) {
      if (typeof out[0] !== 'string' || !/^[0-9a-f]{64}$/.test(out[0])) throw new RpcFailure('the network sent no chain id');
      return { channel: out[0], block: before.block, time: before.time, results: out.slice(2, -1) };
    }
  }
  throw new RpcFailure('the network kept moving during the read; try again');
}

const fail = (code, message, extra = {}) => ({ ok: false, code, message, ...extra });

/** One name's record at one block, opened; or a refusal. */
async function lookup(name, { rpc, fetchFn, derive, program }) {
  const derived = await derive(name);
  if (!(derived?.id instanceof Uint8Array) || derived.id.length !== 32 || !(derived.key instanceof Uint8Array) || derived.key.length !== 32) {
    throw new UsageError('derive(name) must give {id, key}, two 32-byte Uint8Arrays');
  }
  const recordAccount = await nameRecordAccount(derived.id, program);
  const read = await readAtOneBlock(rpc, fetchFn, [recordAccount]);
  const at = { channel: read.channel, block: read.block, blockTime: Number(read.time) };
  const state = read.results[0]?.data?.actor_states?.[program];
  if (!Array.isArray(state) || state.length === 0) return { found: false, at, recordAccount };
  let record;
  try { record = decodeRecord(state); } catch (e) {
    return { error: fail('bad_record', `The record of ${name} can't be read (${e.message}). Paste the LEZ account instead.`, at) };
  }
  if (record.expires !== null && read.time >= record.expires + GRACE_MS) return { found: false, at, recordAccount };
  const profile = await openProfile(derived.key, record.sealed, name);
  return { found: true, at, recordAccount, record, profile };
}

/**
 * Where a bridge deposit to the LNS name `name` goes. `name` is a normalised name
 * (normalizeName(...).name). Options:
 *   derive(name) -> {id, key}   REQUIRED: Argon2id of SPEC.md §3 (not in this module)
 *   rpc      the chain's JSON-RPC (default LNS_RPC)
 *   fetch    the fetch to use (default globalThis.fetch)
 *   program  the registry (default LNS_PROGRAM)
 * Returns
 *   { ok: true, name, forwardedFrom, account: 'Public/<id>', accountId, record, block, blockTime, channel }
 * or { ok: false, code, message } with code one of
 *   invalid_name, not_found, no_public_account, expired, forward_not_found, bad_record, rpc_error.
 * Only account ids go to the network, never the name. Nothing is cached or stored.
 */
export async function resolve(name, options = {}) {
  const { rpc = LNS_RPC, fetch: fetchFn = globalThis.fetch, derive, program = LNS_PROGRAM } = options;
  if (typeof derive !== 'function') throw new UsageError('resolve needs options.derive (Argon2id, SPEC.md §3)');
  if (typeof name !== 'string' || !NAME_PATTERN.test(name) || isAccountId(name)) {
    return fail('invalid_name', 'That is not an LNS name.');
  }
  const ctx = { rpc, fetchFn: fetchFn.bind ? fetchFn.bind(globalThis) : fetchFn, derive, program };
  try {
    const asked = await lookup(name, ctx);
    if (asked.error) return asked.error;
    if (!asked.found) return fail('not_found', 'No such name on LEZ 0.4.', asked.at);
    let used = asked;
    let usedName = name;
    let forwardedFrom = null;
    // SPEC.md "Forwarding": one hop, shown to the payer; a bad or self target is ignored.
    const target = asked.profile && typeof asked.profile.texts[FORWARD_KEY] === 'string' ? normalizeName(asked.profile.texts[FORWARD_KEY]) : null;
    if (target?.kind === 'name' && target.name !== name) {
      const next = await lookup(target.name, ctx);
      if (next.error) return next.error;
      if (!next.found) return fail('forward_not_found', `${name} points to ${target.name}, which isn't registered. Ask its holder for a LEZ account.`, next.at);
      if (next.at.channel !== asked.at.channel) return fail('rpc_error', 'The network changed during the read; try again.');
      used = next; usedName = target.name; forwardedFrom = name;
    }
    const shown = forwardedFrom ? `${forwardedFrom} → ${usedName}` : usedName;
    const { record, at } = used;
    if (record.expires !== null && BigInt(at.blockTime) >= record.expires) {
      return fail('expired', `The LNS name ${shown} has expired. Ask its holder for a LEZ account.`, at);
    }
    if (!used.profile || !used.profile.account) {
      if (used.profile?.payablePrivately) return fail('no_public_account', `The LNS name ${shown} accepts private payments only, so the bridge can't pay it. Paste a LEZ account instead.`, at);
      return fail('no_public_account', `The LNS name ${shown} has no public account set, so the bridge can't pay it yet. If it is yours: in the Names Wallet, open Names and press "Let people pay it…" next to ${shown}, then check again. Or paste a LEZ account instead.`, at);
    }
    return {
      ok: true,
      name: usedName,
      forwardedFrom,
      account: `Public/${used.profile.account}`,
      accountId: used.profile.account,
      record: {
        account: used.recordAccount,
        version: record.version,
        ownerTag: record.ownerTag,
        expires: record.expires === null ? null : Number(record.expires),
        claimed: record.claimed,
        payablePrivately: used.profile.payablePrivately,
      },
      channel: at.channel,
      block: at.block,
      blockTime: Number(at.blockTime),
    };
  } catch (e) {
    if (e instanceof UsageError) throw e;
    return fail('rpc_error', `The LEZ 0.4 network can't be read now: ${e.message}.`);
  }
}

/**
 * Just before the deposit: resolves the same name again and returns the fresh result only if it
 * still goes to the same account on the same chain, at the same block or later. Otherwise
 * { ok: false, code: 'changed' } (or the second resolve's own refusal). `first` is an ok result
 * of resolve; the options are resolve's. The page must refuse to deposit unless this is ok, and
 * must bind the deposit to `account`, never to the name.
 */
export async function confirmSame(first, options = {}) {
  if (!first?.ok) throw new TypeError('confirmSame needs an ok result of resolve');
  const asked = first.forwardedFrom ?? first.name;
  const again = await resolve(asked, options);
  if (!again.ok) {
    if (again.code === 'rpc_error') return again;
    return fail('changed', `The LNS name ${asked} is no longer what you checked (${again.message}) Nothing was sent. Check the name with its holder.`, { before: first, after: again });
  }
  const same = again.accountId === first.accountId && again.name === first.name && again.forwardedFrom === first.forwardedFrom && again.channel === first.channel && again.block >= first.block;
  if (!same) {
    const why = again.channel !== first.channel ? 'the LEZ 0.4 network was reset'
      : again.block < first.block ? 'the network answered from an older block'
        : `it now goes to ${again.account}`;
    return fail('changed', `The LNS name ${asked} changed since you checked it: ${why}. Nothing was sent. Check the name with its holder.`, { before: first, after: again });
  }
  return again;
}

/** A short form of a public account for the confirmation line: `Public/27rmGq…24oB`. */
export function shortAccount(account) {
  const id = String(account).replace(/^Public\//, '');
  return `Public/${id.length > 12 ? `${id.slice(0, 6)}…${id.slice(-4)}` : id}`;
}

/** The confirmation line the page shows under the field (LNS.md, "Wording"). */
export function confirmationLine(result) {
  const from = result.forwardedFrom ? `${result.forwardedFrom} → ${result.name}` : result.name;
  return `${from} → ${shortAccount(result.account)} on LEZ 0.4. The deposit is bound to this account, not to the name.`;
}
