// The compare words (src/lib/compare-words.js) and the vendored BIP-39 English list against the
// shared vectors, which the core module's test reads too (test/vectors/compare-words-v1.json).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { compareWords, compareIndexes, COMPARE_TAG, WORDS_SEPARATOR } from '../src/lib/compare-words.js';
import { BIP39_ENGLISH, BIP39_ENGLISH_TEXT } from '../src/lib/vendor/bip39-english.js';

const V = JSON.parse(readFileSync(new URL('./vectors/compare-words-v1.json', import.meta.url), 'utf8'));
const LIST_SHA256 = '2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda';

test('the vendored list is the canonical BIP-39 English list, with its licence', () => {
  assert.equal(createHash('sha256').update(BIP39_ENGLISH_TEXT, 'ascii').digest('hex'), LIST_SHA256);
  assert.equal(createHash('sha256').update(BIP39_ENGLISH.join('\n') + '\n').digest('hex'), LIST_SHA256);
  assert.equal(BIP39_ENGLISH.length, 2048);
  assert.ok(Object.isFrozen(BIP39_ENGLISH));
  assert.deepEqual([BIP39_ENGLISH[0], BIP39_ENGLISH[2047]], ['abandon', 'zoo']);
  const src = readFileSync(new URL('../src/lib/vendor/bip39-english.js', import.meta.url), 'utf8');
  assert.match(src, /The MIT License \(MIT\)/);
  assert.match(src, /Copyright \(c\) 2013-2016 Pavol Rusnak/);
  assert.match(read('MANIFEST.sha256'), /  src\/lib\/vendor\/bip39-english\.js\n/);
  assert.match(read('MANIFEST.sha256'), /  src\/lib\/compare-words\.js\n/);
});

test('every shared vector gives the same indexes, words and text', async () => {
  assert.equal(V.tag, COMPARE_TAG);
  assert.equal(V.separator, WORDS_SEPARATOR);
  assert.equal(V.wordlist.sha256, LIST_SHA256);
  assert.ok(V.vectors.length >= 8);
  const names = V.vectors.map((v) => v.name);
  assert.ok(names.includes('all zeros') && names.includes('all ones (ff)'));
  for (const v of V.vectors) {
    const got = await compareWords(v.commitment);
    assert.deepEqual(got, { indexes: v.indexes, words: v.words, words_text: v.words_text }, v.name);
    // the same from bytes, and from bare or upper-case hex
    const bytes = Uint8Array.from(Buffer.from(v.commitment.slice(2), 'hex'));
    assert.deepEqual(await compareWords(bytes), got, v.name);
    assert.deepEqual(await compareWords(v.commitment.slice(2).toUpperCase()), got, v.name);
    assert.deepEqual(compareIndexes(Buffer.from(v.hash, 'hex')), v.indexes, v.name);
  }
});

test('the extreme indexes 0 and 2047 split right', () => {
  assert.deepEqual(compareIndexes(new Uint8Array(6)), [0, 0, 0, 0]);
  assert.deepEqual(compareIndexes(new Uint8Array(6).fill(0xff)), [2047, 2047, 2047, 2047]);
});

test('anything but 32 bytes is refused', async () => {
  for (const bad of ['0x', '0x1234', '0x' + '0'.repeat(63), '0x' + 'g'.repeat(64), new Uint8Array(31)]) {
    await assert.rejects(() => compareWords(bad));
  }
});

function read(p) {
  return readFileSync(new URL('../' + p, import.meta.url), 'utf8');
}
