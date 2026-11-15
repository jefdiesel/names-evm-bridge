// The deposit-code path keeps its code (owner bug 2026-11-15: a code link opened into a tab that
// had started a deposit here was ignored, and the page asked for an account). src/app.js is DOM
// code, so these pin its structure; dev/harness.js has the code-* scenarios that run it.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
const between = (a, b) => app.slice(app.indexOf(a), app.indexOf(b, app.indexOf(a)));

test('code mode has no account, token or amount step, and no Change', () => {
  const code = between('const CODE_STEPS = [', '];');
  assert.deepEqual([...code.matchAll(/id: '([a-z]+)'/g)].map((m) => m[1]), ['code', 'connect', 'deposit']);
  // only the file path's steps build the account field and the amount/token controls, and only they offer Change
  assert.match(app, /if \(id === 'amount'\) return amountControls\(\);\s*if \(id === 'account'\) return accountControls\(\);/);
  assert.match(app, /const change = !st\.code && \(x\.id === 'amount' \|\| x\.id === 'account'\)/);
});

test('a code link always takes over: no createdHere guard on hashchange, and the draft never replaces a code', () => {
  const onHash = between("window.addEventListener('hashchange'", '});');
  assert.ok(!/createdHere\) return/.test(onHash), 'hashchange is not ignored for a tab that started a deposit here');
  assert.ok(onHash.includes('fromHash();'));
  const upd = between('function updateDraft(', '\n}\n');
  assert.ok(upd.includes("if (ui.mode !== 'make') return;"), 'updateDraft does nothing outside the make path');
  const enter = between('function enterCode(', '\n}\n');
  for (const k of ["ui.mode = 'request';", "ui.makeStep = 'amount';", 'ui.lns = null;', 'clearTimeout(draftTimer);']) assert.ok(enter.includes(k), k);
  // an unreadable code stays on the code path (paste box), never the make path
  assert.ok(enter.includes("ui.mode = REFUSED.includes(S().error?.code) ? 'request' : 'paste';"));
});

test('from the first paint the code mode shows the amount, the line and the compare digits; leaving it is explicit', () => {
  assert.ok(app.includes("wrap.append(title(`Deposit ${amt(s.request.amount)} ${sym(s)}`), h('p', { class: 'sub' }, 'From your Names Wallet’s deposit code. This page never sees its secret.'), compareDigits(s.request.commitment));"));
  assert.ok(app.includes("hashLink('Use a different code', '#request', () => setMode('paste'))") && app.includes("hashLink('Start over', './', startOver)"));
  assert.ok(!app.includes('Make a deposit here instead'));
});

test('the code path shows four compare words (compare-words.js) with the check line; the hex digits are behind "Digits"', () => {
  assert.ok(app.includes("import { compareWords } from './lib/compare-words.js';"));
  assert.ok(app.includes("const COMPARE_LINE = 'Check these four words match your Names Wallet. If they differ, stop.';"));
  assert.ok(app.includes("more('Digits',"));
  assert.ok(app.includes('(w) => (words.textContent = w.words_text)'));
  // the word list is a file of the page (no new origin), listed in the manifest
  const man = readFileSync(new URL('../MANIFEST.sha256', import.meta.url), 'utf8');
  assert.match(man, /  src\/lib\/compare-words\.js\n/);
  assert.match(man, /  src\/lib\/vendor\/bip39-english\.js\n/);
});
