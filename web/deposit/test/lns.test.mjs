// LNS names as a bridge destination (src/lib/lns.js), offline: the registry's own vectors
// (logos-names SPEC.md §10), real records read from the LEZ 0.4 bridge test network
// (test/vectors/lns-v1.json, made by tools/lns-vectors.mjs), and a fake chain for the rest.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Nothing may touch the network at import time.
const realFetch = globalThis.fetch;
globalThis.fetch = () => { throw new Error('fetch at import time'); };
const lns = await import('../src/lib/lns.js');
globalThis.fetch = realFetch;
const {
  normalizeName, nameRecordAccount, decodeRecord, decodeProfile, openProfile, resolve, confirmSame,
  confirmationLine, shortAccount, base58Encode, base58Decode, LNS_PROGRAM, CLOCK_ACCOUNT, CLOCK_PROGRAM, GRACE_MS,
} = lns;

const V = JSON.parse(readFileSync(new URL('./vectors/lns-v1.json', import.meta.url), 'utf8'));
const hex = (h) => Uint8Array.from(h.match(/../g) ?? [], (b) => parseInt(b, 16));
const toHex = (b) => Buffer.from(b).toString('hex');
const utf8 = new TextEncoder();
const CHANNEL = 'ab'.repeat(32);
const ALICE_ACCOUNT = '27rmGqE6HAju3WaLgtiz99CFWBQfSfftujVUGwCP24oB';

// ------------------------------------------------------------------------------------------
// a fake chain: answers the batches lns.js sends; counts and keeps every request body

const sha = async (b) => new Uint8Array(await crypto.subtle.digest('SHA-256', b));
const fakeDerive = async (name) => ({ id: await sha(utf8.encode(`id/${name}`)), key: await sha(utf8.encode(`key/${name}`)) });

const u32 = (n) => Uint8Array.of(n, n >>> 8, n >>> 16, n >>> 24);
const u64 = (n) => { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(n), true); return b; };
const cat = (...p) => { const o = new Uint8Array(p.reduce((n, x) => n + x.length, 0)); let i = 0; for (const x of p) { o.set(x, i); i += x.length; } return o; };
const str = (s) => cat(u32(utf8.encode(s).length), utf8.encode(s));
function encodeProfile({ name, account = null, npk = null, vpk = new Uint8Array(0), texts = {} }) {
  const e = Object.entries(texts);
  const raw = cat(str(name), account ? cat(Uint8Array.of(1), base58Decode(account)) : Uint8Array.of(0),
    npk ? cat(Uint8Array.of(1), npk) : Uint8Array.of(0), u32(vpk.length), vpk, u32(e.length), ...e.flatMap(([k, v]) => [str(k), str(v)]));
  const padded = new Uint8Array(Math.ceil(raw.length / 256) * 256); padded.set(raw); return padded;
}
async function seal(key, plain) {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const k = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['encrypt']);
  return cat(nonce, new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: utf8.encode('logos-names/v5/record') }, k, plain)));
}
const record = ({ sealed, expires = 2n ** 64n - 1n, version = 6, claimed = 0, tag = new Uint8Array(32).fill(9) }) =>
  cat(Uint8Array.of(version), tag, u64(expires), u32(sealed.length), sealed, version >= 6 ? Uint8Array.of(claimed) : new Uint8Array(0));

class FakeChain {
  constructor() { this.accounts = new Map(); this.block = 100; this.time = 1_800_000_000_000; this.channel = CHANNEL; this.bodies = []; this.tick = null; this.status = 200; }
  async put(name, profile, opts = {}, derive = fakeDerive) {
    const { id, key } = await derive(name);
    const bytes = profile === null ? record({ sealed: new Uint8Array(0), ...opts }) : record({ sealed: await seal(key, encodeProfile({ name, ...profile })), ...opts });
    this.accounts.set(await nameRecordAccount(id), { nonce: 0, data: { actor_states: { [LNS_PROGRAM]: Array.from(bytes) } } });
  }
  clock() { return { nonce: 0, data: { actor_states: { [CLOCK_PROGRAM]: Array.from(cat(u64(this.block), u64(this.time))) } } }; }
  fetch = async (url, init) => {
    this.bodies.push(init.body);
    if (this.status !== 200) return new Response('{"jsonrpc":"2.0","id":null,"error":{"code":-32005,"message":"rate limited; slow down"}}', { status: this.status, headers: { 'Retry-After': '10' } });
    const calls = JSON.parse(init.body);
    let clockReads = 0;
    const answers = calls.map(({ id, method, params }) => {
      if (method === 'getChannelId') return { jsonrpc: '2.0', id, result: this.channel };
      if (method === 'getAccount' && params[0] === CLOCK_ACCOUNT) {
        if (clockReads++ > 0 && this.tick) this.tick();
        return { jsonrpc: '2.0', id, result: this.clock() };
      }
      if (method === 'getAccount') return { jsonrpc: '2.0', id, result: this.accounts.get(params[0]) ?? null };
      return { jsonrpc: '2.0', id, error: { code: -32601, message: 'method not served here' } };
    });
    return new Response(JSON.stringify(answers.reverse()), { status: 200 }); // order must not matter
  };
}

// ------------------------------------------------------------------------------------------
// the registry's own vectors (SPEC.md §10)

test('record accounts match the registry spec vectors (SPEC §10)', async () => {
  for (const [name, id] of V.spec.derivations) {
    assert.equal(normalizeName(name).kind, 'name', name);
    assert.equal(hex(id).length, 32);
  }
  for (const [id, account] of V.spec.recordAccounts) assert.equal(await nameRecordAccount(hex(id), V.spec.program), account);
  await assert.rejects(nameRecordAccount(new Uint8Array(31)), /32 bytes/);
});

test('the spec profile opens with jef’s key, to its public account; nothing else opens it', async () => {
  const { key, sealed, profile } = V.spec.sealedProfile;
  assert.deepEqual(await openProfile(hex(key), hex(sealed), 'jef'), profile);
  assert.equal(await openProfile(hex(key), hex(sealed), 'ada'), null, 'a profile names its own name');
  assert.equal(await openProfile(new Uint8Array(32).fill(9), hex(sealed), 'jef'), null, 'wrong key');
  for (const at of [0, 12, hex(sealed).length - 1]) {
    const t = hex(sealed); t[at] ^= 1;
    assert.equal(await openProfile(hex(key), t, 'jef'), null, `tampered byte ${at}`);
  }
  assert.equal(await openProfile(hex(key), new Uint8Array(0), 'jef'), null, 'nothing set');
});

// ------------------------------------------------------------------------------------------
// real records from the LEZ 0.4 bridge test network

test('live: the derivation from a name id gives the record account the chain holds', async () => {
  assert.equal(V.program, LNS_PROGRAM);
  assert.ok(V.live.length >= 3);
  for (const v of V.live) {
    assert.equal(await nameRecordAccount(hex(v.id)), v.recordAccount, v.name);
    assert.equal(v.recordAccount, v.referenceRecordAccount, `${v.name}: same as the reference`);
  }
});

test('live: real records decode exactly and open with their name’s key', async () => {
  const held = V.live.filter((v) => v.result.code === 'no_public_account');
  assert.ok(held.length >= 1, 'at least one real record');
  for (const v of held) {
    const answers = JSON.parse(v.exchanges.at(-1).response);
    const state = answers.find((a) => a.id === 3).result.data.actor_states[LNS_PROGRAM];
    const r = decodeRecord(state);
    assert.equal(r.version, 6);
    assert.equal(r.expires, null, 'deposit mode: never lapses (u64::MAX)');
    assert.equal(r.claimed, false);
    assert.equal(r.ownerTag.length, 64);
    const p = await openProfile(hex(v.key), r.sealed, v.name);
    assert.equal(p.name, v.name);
    assert.equal(p.account, null, 'publishes no public account');
    assert.equal(p.payablePrivately, true);
  }
});

function replay(exchanges) {
  const sent = [];
  let i = 0;
  const fetch = async (url, init) => {
    sent.push(init.body);
    const ex = exchanges[i++];
    assert.ok(ex, 'more requests than recorded');
    assert.deepEqual(JSON.parse(init.body), ex.request);
    return new Response(ex.response, { status: ex.status });
  };
  return { fetch, sent };
}

test('live: resolve replays the real reads to the same answers, and never sends the name', async () => {
  for (const v of V.live) {
    const { fetch, sent } = replay(v.exchanges);
    const got = await resolve(v.name, { fetch, derive: async () => ({ id: hex(v.id), key: hex(v.key) }) });
    assert.deepEqual(got, v.result, v.name);
    for (const body of sent) {
      assert.ok(!body.includes(v.name), 'the name never goes to the network');
      for (const call of JSON.parse(body)) assert.ok(['getChannelId', 'getAccount'].includes(call.method));
    }
  }
  const nf = V.live.find((v) => v.result.code === 'not_found');
  assert.ok(nf, 'a real not-registered read');
});

// ------------------------------------------------------------------------------------------
// what was typed

test('normalizeName: accounts first, then names per SPEC §2', () => {
  assert.deepEqual(normalizeName(`Public/${ALICE_ACCOUNT}`), { kind: 'account', id: ALICE_ACCOUNT, label: 'Public' });
  assert.deepEqual(normalizeName(` private/${ALICE_ACCOUNT} `), { kind: 'account', id: ALICE_ACCOUNT, label: 'Private' });
  assert.deepEqual(normalizeName(ALICE_ACCOUNT), { kind: 'account', id: ALICE_ACCOUNT, label: '' });
  assert.deepEqual(normalizeName('1'.repeat(32)), { kind: 'account', id: '1'.repeat(32), label: '' }, '32 ones: an account, never a name');
  for (const typed of ['alice', 'Alice', ' @alice ', 'lns:alice', 'ALICE.lez', 'LNS:@Alice.LEZ']) assert.deepEqual(normalizeName(typed), { kind: 'name', name: 'alice' }, typed);
  for (const name of ['a', 'a-b-c', 'm2-test-111', 'abcdefghijklmnopqrstuvwxyz012345']) assert.deepEqual(normalizeName(name), { kind: 'name', name });
  for (const bad of ['', '  ', '-alice', 'alice-', 'al ice', 'alice.eth', 'a'.repeat(33), 'álice', 'Public/abc', `Public/${ALICE_ACCOUNT}x`, '0x' + 'ab'.repeat(20)]) {
    assert.equal(normalizeName(bad).kind, 'invalid', JSON.stringify(bad));
  }
  for (const dressed of ['lns:' + '1'.repeat(32), '@' + '1'.repeat(32), '1'.repeat(32) + '.lez']) {
    assert.equal(normalizeName(dressed).kind, 'invalid', `${dressed}: a name that is also an account id is refused (SPEC §2)`);
  }
  assert.deepEqual(normalizeName('alice.lez.lez'), { kind: 'invalid', message: normalizeName('x y').message });
});

// ------------------------------------------------------------------------------------------
// decoding, fail-closed

test('decodeRecord refuses anything but an exact record', () => {
  const ok = record({ sealed: Uint8Array.of(1, 2, 3), expires: 5n });
  assert.deepEqual(decodeRecord(ok), { version: 6, ownerTag: '09'.repeat(32), expires: 5n, sealed: Uint8Array.of(1, 2, 3), claimed: false });
  assert.equal(decodeRecord(record({ sealed: new Uint8Array(0), version: 5 })).claimed, false, 'version 5 ends at sealed');
  assert.throws(() => decodeRecord(cat(ok, Uint8Array.of(0))), /too many/);
  assert.throws(() => decodeRecord(ok.subarray(0, ok.length - 1)), /truncated/);
  assert.throws(() => decodeRecord(record({ sealed: new Uint8Array(0), version: 7 })), /unknown version/);
  assert.throws(() => decodeRecord(record({ sealed: new Uint8Array(0), version: 2 })), /unknown version/);
  assert.throws(() => decodeRecord(record({ sealed: new Uint8Array(0), claimed: 2 })), /boolean/);
  assert.throws(() => decodeProfile(cat(encodeProfile({ name: 'a' }).subarray(0, 255), Uint8Array.of(1))), /followed by data/);
});

// ------------------------------------------------------------------------------------------
// resolve against the fake chain

test('a name with a public account resolves to it, read at one block', async () => {
  const chain = new FakeChain();
  await chain.put('alice', { account: ALICE_ACCOUNT, npk: new Uint8Array(32).fill(4), vpk: new Uint8Array(1184).fill(5) });
  const r = await resolve('alice', { fetch: chain.fetch, derive: fakeDerive });
  assert.equal(r.ok, true);
  assert.equal(r.account, `Public/${ALICE_ACCOUNT}`);
  assert.equal(r.accountId, ALICE_ACCOUNT);
  assert.equal(r.name, 'alice');
  assert.equal(r.forwardedFrom, null);
  assert.equal(r.block, 100);
  assert.equal(r.channel, CHANNEL);
  assert.equal(r.record.expires, null);
  assert.equal(r.record.payablePrivately, true);
  assert.equal(confirmationLine(r), 'alice → Public/27rmGq…24oB on LEZ 0.4. The deposit is bound to this account, not to the name.');
  assert.equal(shortAccount(ALICE_ACCOUNT), 'Public/27rmGq…24oB');
  assert.equal(chain.bodies.length, 1, 'one batch');
  assert.ok(!chain.bodies[0].includes('alice'));
  assert.doesNotThrow(() => JSON.stringify(r), 'no BigInt in the result');
});

test('refusals: not registered, no public account, nothing set, unreadable', async () => {
  const chain = new FakeChain();
  const opts = { fetch: chain.fetch, derive: fakeDerive };
  assert.equal((await resolve('nobody', opts)).code, 'not_found');
  await chain.put('keys-only', { npk: new Uint8Array(32).fill(4), vpk: new Uint8Array(1184).fill(5) });
  const k = await resolve('keys-only', opts);
  assert.equal(k.code, 'no_public_account');
  assert.match(k.message, /accepts private payments only/);
  await chain.put('blank', null);
  assert.match((await resolve('blank', opts)).message, /has no public account set/);
  // A profile sealed for another name is "nothing set".
  const { key } = await fakeDerive('mallory');
  const { id } = await fakeDerive('victim');
  chain.accounts.set(await nameRecordAccount(id), { nonce: 0, data: { actor_states: { [LNS_PROGRAM]: Array.from(record({ sealed: await seal(key, encodeProfile({ name: 'mallory', account: ALICE_ACCOUNT })) })) } } });
  assert.equal((await resolve('victim', { ...opts, derive: async (n) => (n === 'victim' ? { id, key } : fakeDerive(n)) })).code, 'no_public_account');
  const { id: badId } = await fakeDerive('broken');
  chain.accounts.set(await nameRecordAccount(badId), { nonce: 0, data: { actor_states: { [LNS_PROGRAM]: [6, 1, 2] } } });
  assert.equal((await resolve('broken', opts)).code, 'bad_record');
  assert.equal((await resolve('Alice', opts)).code, 'invalid_name', 'resolve takes a normalised name');
  assert.equal((await resolve('1'.repeat(32), opts)).code, 'invalid_name', 'an account id is never a name');
  await assert.rejects(resolve('alice', { fetch: chain.fetch }), /derive/);
});

test('expiry is judged by the chain’s clock: lapsed is expired, past grace is not registered', async () => {
  const chain = new FakeChain();
  const opts = { fetch: chain.fetch, derive: fakeDerive };
  const now = BigInt(chain.time);
  await chain.put('fresh', { account: ALICE_ACCOUNT }, { expires: now + 1000n });
  await chain.put('lapsed', { account: ALICE_ACCOUNT }, { expires: now - 1000n });
  await chain.put('gone', { account: ALICE_ACCOUNT }, { expires: now - GRACE_MS - 1n });
  assert.equal((await resolve('fresh', opts)).ok, true);
  assert.equal((await resolve('fresh', opts)).record.expires, Number(now + 1000n));
  assert.equal((await resolve('lapsed', opts)).code, 'expired');
  assert.equal((await resolve('gone', opts)).code, 'not_found');
});

test('a forward is followed one hop and shown; to a missing name it is refused', async () => {
  const chain = new FakeChain();
  const opts = { fetch: chain.fetch, derive: fakeDerive };
  await chain.put('brand', { texts: { forward: 'Alice' } });
  await chain.put('alice', { account: ALICE_ACCOUNT, texts: { forward: 'elsewhere' } });
  await chain.put('elsewhere', { account: '8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR' });
  const r = await resolve('brand', opts);
  assert.equal(r.ok, true);
  assert.equal(r.name, 'alice');
  assert.equal(r.forwardedFrom, 'brand');
  assert.equal(r.accountId, ALICE_ACCOUNT, 'one hop only');
  assert.match(confirmationLine(r), /^brand → alice → Public\/27rmGq…24oB on LEZ 0\.4\./);
  await chain.put('dangling', { account: ALICE_ACCOUNT, texts: { forward: 'nobody' } });
  assert.equal((await resolve('dangling', opts)).code, 'forward_not_found');
  await chain.put('selfish', { account: ALICE_ACCOUNT, texts: { forward: 'selfish' } });
  assert.equal((await resolve('selfish', opts)).forwardedFrom, null, 'a forward to itself is ignored');
  await chain.put('junk', { account: ALICE_ACCOUNT, texts: { forward: 'not a name' } });
  assert.equal((await resolve('junk', opts)).accountId, ALICE_ACCOUNT, 'an invalid forward is ignored');
});

test('the read is bracketed by the clock: a block in between is retried; network trouble is rpc_error', async () => {
  const chain = new FakeChain();
  await chain.put('alice', { account: ALICE_ACCOUNT });
  const opts = { fetch: chain.fetch, derive: fakeDerive };
  let moves = 1;
  chain.tick = () => { if (moves-- > 0) chain.block += 1; };
  const r = await resolve('alice', opts);
  assert.equal(r.ok, true);
  assert.equal(r.block, 101);
  assert.equal(chain.bodies.length, 2);
  chain.tick = () => { chain.block += 1; };
  assert.equal((await resolve('alice', opts)).code, 'rpc_error');
  chain.tick = null;
  chain.status = 429;
  const busy = await resolve('alice', opts);
  assert.equal(busy.code, 'rpc_error');
  assert.match(busy.message, /busy; try again in 10 s/);
  const down = await resolve('alice', { derive: fakeDerive, fetch: async () => { throw new TypeError('Failed to fetch'); } });
  assert.equal(down.code, 'rpc_error');
  const html = await resolve('alice', { derive: fakeDerive, fetch: async () => new Response('<html>403</html>', { status: 403 }) });
  assert.equal(html.code, 'rpc_error');
  const cut = await resolve('alice', { derive: fakeDerive, fetch: async () => ({ status: 200, text: async () => { throw new TypeError('network error'); } }) });
  assert.equal(cut.code, 'rpc_error', 'a body that fails to read is rpc_error, not a throw');
});

test('confirmSame: the same account passes; any change, a reset or an older block refuses', async () => {
  const chain = new FakeChain();
  const opts = { fetch: chain.fetch, derive: fakeDerive };
  await chain.put('alice', { account: ALICE_ACCOUNT });
  const first = await resolve('alice', opts);
  chain.block += 5;
  const again = await confirmSame(first, opts);
  assert.equal(again.ok, true);
  assert.equal(again.block, 105);

  await chain.put('alice', { account: '8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR' });
  const moved = await confirmSame(first, opts);
  assert.equal(moved.code, 'changed');
  assert.match(moved.message, /now goes to Public\/8qbHbw/);

  await chain.put('alice', { account: ALICE_ACCOUNT });
  chain.block = 90;
  assert.match((await confirmSame(first, opts)).message, /older block/);
  chain.block = 200;
  chain.channel = 'cd'.repeat(32);
  assert.match((await confirmSame(first, opts)).message, /was reset/);
  chain.channel = CHANNEL;

  chain.accounts.clear();
  assert.equal((await confirmSame(first, opts)).code, 'changed', 'released since');
  chain.status = 503;
  assert.equal((await confirmSame(first, opts)).code, 'rpc_error', 'cannot check: not a pass');

  // A forwarded result is re-resolved from the name that was asked.
  chain.status = 200;
  await chain.put('alice', { account: ALICE_ACCOUNT });
  await chain.put('brand', { texts: { forward: 'alice' } });
  const viaBrand = await resolve('brand', opts);
  assert.equal((await confirmSame(viaBrand, opts)).ok, true);
  await chain.put('brand', { texts: { forward: 'elsewhere' } });
  await chain.put('elsewhere', { account: ALICE_ACCOUNT });
  assert.equal((await confirmSame(viaBrand, opts)).code, 'changed', 'a new forward target is a change, even to the same account');
  await assert.rejects(confirmSame({ ok: false }, opts), TypeError);
});

test('nothing is kept between resolves or page loads', async () => {
  const src = readFileSync(new URL('../src/lib/lns.js', import.meta.url), 'utf8');
  for (const word of ['localStorage', 'sessionStorage', 'indexedDB', 'document.cookie', 'caches.']) assert.ok(!src.includes(word), word);
  assert.ok(!/^import /m.test(src), 'no imports');
  const chain = new FakeChain();
  await chain.put('alice', { account: ALICE_ACCOUNT });
  let derives = 0;
  const derive = async (n) => { derives += 1; return fakeDerive(n); };
  await resolve('alice', { fetch: chain.fetch, derive });
  await resolve('alice', { fetch: chain.fetch, derive });
  assert.equal(chain.bodies.length, 2, 'every resolve reads the chain');
  assert.equal(derives, 2, 'no derivation is cached inside the module');
  assert.equal(toHex(base58Decode(base58Encode(new Uint8Array(32)))), '00'.repeat(32));
});
