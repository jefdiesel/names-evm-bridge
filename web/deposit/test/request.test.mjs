import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  encodeRequest, decodeRequest, parseRequestInput, requestToJSON, requestToBytes, requestUrl,
  bech32mDecodeWords, RequestError, HRP, MAX_LENGTH,
} from '../src/lib/request.js';

const V = JSON.parse(readFileSync(new URL('./vectors/request-v1.json', import.meta.url), 'utf8'));
const fromJSON = (r) => ({ ...r, chainId: BigInt(r.chainId), amount: BigInt(r.amount) });

test('valid vectors encode to the exact string and payload, and decode back', () => {
  assert.ok(V.valid.length >= 5);
  for (const v of V.valid) {
    const req = fromJSON(v.request);
    assert.equal(encodeRequest(req), v.string, v.name);
    assert.equal(Buffer.from(requestToBytes(req)).toString('hex'), v.payloadHex, v.name);
    assert.deepEqual(requestToJSON(decodeRequest(v.string)), v.request, v.name);
    assert.ok(v.string.startsWith(HRP + '1'));
    assert.ok(v.string.length <= MAX_LENGTH);
    // all-uppercase is the same request (QR alphanumeric mode)
    assert.deepEqual(requestToJSON(decodeRequest(v.string.toUpperCase())), v.request);
  }
});

test('invalid vectors fail with the stated code', () => {
  for (const x of V.invalid) {
    assert.throws(() => decodeRequest(x.input), (e) => e instanceof RequestError && e.code === x.error, `${x.name}: expected ${x.error}`);
  }
});

test('lenient inputs (links, whitespace, wrapping, uppercase) parse to the canonical request', () => {
  for (const l of V.lenient) {
    assert.equal(encodeRequest(parseRequestInput(l.input)), l.expect, l.name);
  }
});

test('BIP-350 bech32m vectors pass the checksum layer; mutations fail', () => {
  for (const s of V.bip350.valid) assert.doesNotThrow(() => bech32mDecodeWords(s), s);
  for (const s of V.bip350.mutatedMustFail) {
    assert.throws(() => bech32mDecodeWords(s), (e) => e.code === 'checksum', s);
  }
});

test('every single-character substitution in a request is caught', () => {
  const s = V.valid[0].string;
  const charset = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
  let tried = 0;
  for (let i = HRP.length + 1; i < s.length; i++) {
    for (const c of charset) {
      if (c === s[i]) continue;
      const m = s.slice(0, i) + c + s.slice(i + 1);
      assert.throws(() => decodeRequest(m), RequestError);
      tried++;
    }
  }
  assert.ok(tried > 4000);
});

test('round-trips random requests', () => {
  const rnd = (n) => '0x' + Array.from({ length: n }, () => Math.floor(Math.random() * 256).toString(16).padStart(2, '0')).join('');
  for (let i = 0; i < 300; i++) {
    const req = {
      chainId: BigInt(1 + Math.floor(Math.random() * 2 ** 40)),
      vault: rnd(20),
      token: Math.random() < 0.3 ? '0x' + '00'.repeat(20) : rnd(20),
      denomIndex: Math.floor(Math.random() * 8),
      amount: BigInt(rnd(1 + Math.floor(Math.random() * 32))) || 1n,
      commitment: rnd(32),
    };
    if (/^0x0+$/.test(req.vault) || /^0x0+$/.test(req.commitment)) continue;
    const s = encodeRequest(req);
    assert.ok(s.length >= 150 && s.length <= 199, `length ${s.length}`);
    assert.deepEqual(requestToJSON(decodeRequest(s)), requestToJSON(req));
  }
});

test('encode refuses bad fields', () => {
  const ok = fromJSON(V.valid[0].request);
  const bad = [
    [{ ...ok, chainId: 0n }, 'chain'],
    [{ ...ok, amount: 0n }, 'amount'],
    [{ ...ok, amount: 1n << 256n }, 'amount'],
    [{ ...ok, denomIndex: 256 }, 'denom-index'],
    [{ ...ok, denomIndex: -1 }, 'denom-index'],
    [{ ...ok, vault: '0x' + '00'.repeat(20) }, 'vault'],
    [{ ...ok, commitment: '0x' + '00'.repeat(32) }, 'commitment'],
    [{ ...ok, commitment: '0x1234' }, 'field'],
    [{ ...ok, vault: 'not an address' }, 'field'],
    [{ ...ok, version: 2 }, 'version'],
  ];
  for (const [r, code] of bad) assert.throws(() => encodeRequest(r), (e) => e.code === code, code);
});

test('mixed-case addresses are normalised; the string is always lowercase', () => {
  const ok = fromJSON(V.valid[0].request);
  const s = encodeRequest({ ...ok, vault: '0x8C8b2664De28e5287158db4DB65D448638871b74' });
  assert.equal(s, V.valid[0].string);
  assert.equal(s, s.toLowerCase());
});

test('requestUrl puts the request in the fragment, never the query', () => {
  const ok = fromJSON(V.valid[0].request);
  const u = requestUrl('https://deposit.example/path/?x=1#old', ok);
  assert.equal(u, `https://deposit.example/path/?x=1#${V.valid[0].string}`);
  assert.equal(new URL(u).hash.slice(1), V.valid[0].string);
});

test('empty and non-string input', () => {
  assert.throws(() => parseRequestInput('   '), (e) => e.code === 'empty');
  assert.throws(() => parseRequestInput(42), (e) => e.code === 'type');
  assert.throws(() => parseRequestInput('https://x/#'), (e) => e.code === 'empty');
});
