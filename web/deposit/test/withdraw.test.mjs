// SPDX-License-Identifier: MIT OR Apache-2.0
// The send-back codes (src/lib/withdraw.js) against the independent vectors.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  encodeWithdrawTo, decodeWithdrawTo, withdrawWords, encodeRelease, decodeRelease, codeFromInput, codeKind,
} from '../src/lib/withdraw.js';

const V = JSON.parse(readFileSync(new URL('./vectors/withdraw-v1.json', import.meta.url)));

test('every valid withdraw-to vector encodes, decodes and gives its words', async () => {
  for (const v of V.withdrawTo.valid) {
    const f = { ...v.fields, chainId: BigInt(v.fields.chainId) };
    assert.equal(encodeWithdrawTo(f), v.string, v.name);
    const d = decodeWithdrawTo(v.string);
    assert.equal(d.chainId, f.chainId);
    assert.equal(d.vault, v.fields.vault);
    assert.equal(d.recipient, v.fields.recipient);
    assert.equal(d.feeChoice, v.fields.feeChoice);
    assert.equal(decodeWithdrawTo(v.string.toUpperCase()).recipient, v.fields.recipient);
    const w = await withdrawWords(d);
    assert.deepEqual(w.indexes, v.indexes, v.name);
    assert.equal(w.words_text, v.words_text);
  }
});

test('every invalid withdraw-to vector is refused with its code', () => {
  for (const v of V.withdrawTo.invalid) {
    assert.throws(() => decodeWithdrawTo(v.string), (e) => e.code === v.code, `${v.name}: expected ${v.code}`);
  }
});

test('every valid release vector round-trips', () => {
  for (const v of V.release.valid) {
    const f = { ...v.fields, chainId: BigInt(v.fields.chainId), relayFee: BigInt(v.fields.relayFee) };
    assert.equal(encodeRelease(f), v.string, v.name);
    const d = decodeRelease(v.string);
    for (const k of ['vault', 'root', 'nullifier', 'token', 'recipient', 'relayer', 'seal']) assert.equal(d[k], v.fields[k], `${v.name} ${k}`);
    assert.equal(d.denomIndex, v.fields.denomIndex);
    assert.equal(d.relayFee, BigInt(v.fields.relayFee));
  }
});

test('every invalid release vector is refused with its code', () => {
  for (const v of V.release.invalid) {
    assert.throws(() => decodeRelease(v.string), (e) => e.code === v.code, `${v.name}: expected ${v.code}`);
  }
});

test('links and pasted text are read; kinds are told apart', () => {
  const s = V.withdrawTo.valid[0].string;
  assert.equal(codeFromInput(` https://nameswallet.app/bridge/#${s}\n`), s);
  assert.equal(codeKind(`https://x/#${s}`), 'withdraw');
  assert.equal(codeKind(V.release.valid[0].string), 'release');
  assert.equal(codeKind('logosdep1qq'), 'deposit');
  assert.equal(codeKind('#withdraw'), null);
});

// "Bridge to Ethereum" by a pasted address (the bridge's NAMES-SEND-BACK.md §3.1a): the wallet turns a
// checksummed address into a code for the vault in use. It must be exactly the code this page makes for
// that address (relay-fee choice 0, the page's default), so the words, the burn and the release are the same.
import { withdrawFor } from '../src/lib/send-back.js';
const A = JSON.parse(readFileSync(new URL('./vectors/withdraw-address-v1.json', import.meta.url)));

test('the page makes the same code and words as the wallet’s address route', async () => {
  for (const v of A.valid) {
    const w = await withdrawFor(v.address, { chainId: v.chainId, vault: v.vault });
    assert.equal(w.code, v.code, v.name);
    assert.equal(w.words_text, v.words_text, v.name);
    assert.equal(decodeWithdrawTo(v.code).recipient, v.address.toLowerCase());
  }
  assert.ok(A.invalid.some((v) => v.code === 'no_checksum') && A.invalid.some((v) => v.code === 'bad_checksum'));
});
