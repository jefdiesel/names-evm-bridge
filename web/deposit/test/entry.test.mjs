// The two ways in (a Names Wallet deposit code, a deposit made here) and what the page says after.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFragment, madeFragment, compareDigits, claimPath, namesAfter, finalInMinutes, depositedWords } from '../src/lib/entry.js';
import { encodeRequest } from '../src/lib/request.js';

const CODE = encodeRequest({ chainId: 11155111n, vault: '0x26af59855b99a8af199361289a5ed6088729e60e', token: '0x0000000000000000000000000000000000000000', denomIndex: 0, amount: 10n ** 16n, commitment: '0x' + 'ab'.repeat(16) + 'cd'.repeat(16) });

test('a Names Wallet code in the fragment is a code, not made here; a made-here link is marked', () => {
  assert.deepEqual(readFragment('#' + CODE), { kind: 'code', madeHere: false, text: CODE });
  assert.deepEqual(readFragment(madeFragment(CODE)), { kind: 'code', madeHere: true, text: CODE });
  assert.equal(madeFragment(CODE), '#made:' + CODE);
  assert.equal(readFragment('#request').kind, 'paste');
  assert.equal(readFragment('#settings').kind, 'settings');
  assert.equal(readFragment('').kind, 'none');
  assert.equal(readFragment('#made%3A' + CODE).madeHere, true, 'a percent-encoded colon too');
});

test('the comparison digits are the commitment’s first and last 6 hex digits, as the module gives them', () => {
  assert.deepEqual(compareDigits('0x' + 'ab'.repeat(16) + 'cd'.repeat(16)), { first: 'ababab', last: 'cdcdcd' });
  assert.deepEqual(compareDigits('0xABCDEF' + '0'.repeat(52) + '123456'), { first: 'abcdef', last: '123456' });
});

test('after a deposit from a Names Wallet code the page names the wallet and never a claim file', () => {
  assert.equal(claimPath({ createdHere: false, madeHere: false }), 'names');
  assert.equal(claimPath({ createdHere: false, madeHere: true }), 'file');
  assert.equal(claimPath({ createdHere: true, madeHere: false }), 'here');
  const words = namesAfter('the bridge test network (LEZ 0.4)');
  assert.match(words, /Your Names Wallet will show this deposit and offer the claim/);
  assert.doesNotMatch(words, /claim file|save|official/i);
});

test('minutes to final: the bridge module’s rule (receive.rs), whole epochs of 32 blocks, plus one', () => {
  // mins = ceil((block - finalized) / 32) * 32 * 12 / 60 + 1, integer maths
  assert.equal(finalInMinutes(1000, 1000), 0);
  assert.equal(finalInMinutes(1000, 1200), 0);
  assert.equal(finalInMinutes(1001, 1000), 7);     // 1 epoch: 384 s -> 6 min, +1
  assert.equal(finalInMinutes(1032, 1000), 7);
  assert.equal(finalInMinutes(1033, 1000), 13);    // 2 epochs: 768 s -> 12, +1
  assert.equal(finalInMinutes(1070, 1000), 20);    // 3 epochs: 1152 s -> 19, +1
  assert.equal(finalInMinutes(12135103n, 12135033n), 20);
});

test('the Deposited screen says the wait first, then what to do, by path', () => {
  assert.equal(depositedWords('names', false), 'It takes up to about 20 minutes to become final on Sepolia. Then your Names Wallet offers the claim.');
  assert.equal(depositedWords('here', false), 'It takes up to about 20 minutes to become final on Sepolia. Then claim with your file in the Names Wallet.');
  assert.equal(depositedWords('names', true), 'Final on Sepolia. Open your Names Wallet to claim.');
  assert.equal(depositedWords('file', true), 'Final on Sepolia. Claim with your file in the Names Wallet.');
});
