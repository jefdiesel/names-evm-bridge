// The page's frame (2026-11-14): two columns on wide screens, the testing page, the shared links.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { WALLET_TEST_BUILD, REPORT_PROBLEM, TESTING_PAGE } from '../src/lib/links.js';

const read = (p) => readFileSync(new URL(`../${p}`, import.meta.url), 'utf8');

test('the wallet link is one constant, never a dead link: /basecamp/ until the test-build page is live', () => {
  assert.ok(WALLET_TEST_BUILD.startsWith('/basecamp/'));
  const app = read('src/app.js');
  assert.ok(app.includes("import { WALLET_TEST_BUILD, TESTING_PAGE } from './lib/links.js';"));
  assert.ok(app.includes("'Get the Names Wallet test build for LEZ 0.4'"));
  // every static anchor for it carries the constant (tools/chrome.mjs writes it)
  for (const p of ['about/testing/index.html']) {
    const hrefs = [...read(p).matchAll(/<a href="([^"]*)" data-link="wallet-test-build"/g)].map((m) => m[1]);
    assert.ok(hrefs.length >= 1, p);
    assert.ok(hrefs.every((x) => x === WALLET_TEST_BUILD), `${p}: ${hrefs}`);
  }
});

test('the testing page: in the manifest, same CSP as the other about pages, its report link from the constant', () => {
  assert.equal(TESTING_PAGE, 'about/testing/');
  assert.match(read('MANIFEST.sha256'), /  about\/testing\/index\.html\n/);
  const csp = (t) => t.match(/http-equiv="Content-Security-Policy" content="([^"]*)"/)[1];
  const page = read('about/testing/index.html');
  assert.equal(csp(page), csp(read('about/security/index.html')));
  assert.match(page, /<title>Testing this bridge · Names Wallet bridge \(Sepolia testnet\)<\/title>/);
  const report = [...page.matchAll(/<a href="([^"]*)" data-link="report-problem"/g)].map((m) => m[1]);
  assert.deepEqual(report, [REPORT_PROBLEM]);
  assert.ok(/Email <a href="mailto:bridge@nameswallet\.app" data-link="report-problem">bridge@nameswallet\.app<\/a>\. Say which way you used \(A or B\)/.test(page));
  assert.equal(REPORT_PROBLEM, 'mailto:bridge@nameswallet.app');
  assert.ok(!/offici/i.test(page) && !/pay nothing/i.test(page));
  assert.ok(/In test builds that have “Receive from Ethereum”/.test(page));
  // both ways, numbered
  assert.ok(/A\. From your Names Wallet/.test(page) && /B\. With a claim file/.test(page));
  // linked from the bridge page (the app adds it to the side panel and the notes)
  assert.ok(read('src/app.js').includes("h('a', { href: TESTING_PAGE }, 'Testing this bridge')"));
});

test('the layout: one main, the steps card and the side panel in a grid; the panel is an aside, placed in one host only', () => {
  const html = read('index.html');
  assert.equal((html.match(/<main\b/g) || []).length, 1);
  assert.match(html, /<div class="bridge-grid">\s*<section class="card" id="card" aria-labelledby="card-h"><\/section>\s*<div class="side" id="side" hidden><\/div>\s*<\/div>/);
  const app = read('src/app.js');
  assert.ok(app.includes("h('aside', { class: 'side-in', 'aria-labelledby': 'side-h' })"));
  assert.ok(app.includes("const sideHost = wide && sideCol ? sideCol : host;"), 'the panel moves, it is never rendered twice');
  assert.ok(app.includes("if (prev && prev.node.isConnected && prev.node.parentNode !== host) prev.node.remove();"));
  const css = read('app.css');
  assert.ok(/@media \(min-width: 900px\) \{\s*\.bridge-grid \{ grid-template-columns:/.test(css));
  assert.ok(/body > main \{ flex: 1 0 auto; \}/.test(css), 'the footer sits at the bottom on short content');
});
