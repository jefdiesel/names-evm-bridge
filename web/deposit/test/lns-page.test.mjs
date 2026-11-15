// The page's use of LNS names (src/app.js + src/lib/lns.js + src/lib/lnsderive.js), 2026-11-15.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (p) => readFileSync(new URL(`../${p}`, import.meta.url), 'utf8');

test('the account field takes an account or a name; a name is resolved, bound to its account, and checked again before the deposit', () => {
  const app = read('src/app.js');
  assert.ok(app.includes("'Your LEZ 0.4 account or LNS name'") && app.includes("'Public/… or a name like alice'"));
  assert.ok(app.includes('r = await lnsResolve(name, { derive });'));
  // bound to the account: the draft (and so the claim file) gets r.account, never the name
  assert.ok(app.includes('await flow.setDraft({ token: ui.token, denomIndex: ui.denomIndex, account: r.account })'));
  // the confirmation line in the check box, with the full account, before the claim file
  assert.ok(app.includes('lnsLine.textContent = valid && l ? confirmationLine(l.r) : \'\';'));
  // again just before the deposit; a change stops it
  assert.ok(app.includes('c = await confirmSame(l.r, { derive });'));
  assert.ok(app.includes("() => depositNow()") && !/primary\(`Deposit \$\{amt\(total\)\} \$\{sym\(\)\}`, \(\) => flow\.deposit\(\)/.test(app));
  // the two lines about names under "Which account, and why"
  assert.ok(app.includes('An LNS name works only if it is registered on LEZ 0.4 and its holder published a public account'));
});

test('the CSP lets the page (and its 404 page, same header) read the LEZ 0.4 RPC; the about pages stay closed', () => {
  const meta = read('index.html').match(/http-equiv="Content-Security-Policy" content="([^"]*)"/)[1];
  assert.match(meta, /connect-src https:\/\/ethereum-sepolia-rpc\.publicnode\.com https:\/\/lez\.nameswallet\.app;/);
  assert.match(meta, /worker-src 'none'/);
  const headers = read('_headers');
  assert.ok(headers.includes(meta), '_headers carries the same string');
  // the 404 page is served under the same /bridge/* header, so it carries the same CSP
  assert.equal(read('404.html').match(/http-equiv="Content-Security-Policy" content="([^"]*)"/)[1], meta);
  for (const p of ['about/privacy/index.html', 'about/security/index.html', 'about/verify/index.html', 'about/testing/index.html']) assert.ok(!read(p).includes('lez.nameswallet.app'), p);
});

test('Argon2id is the one vendored file, with its licence, version and source', () => {
  const v = read('src/lib/vendor/argon2id.js');
  assert.match(v, /@noble\/hashes 2\.4\.0/);
  assert.match(v, /The MIT License \(MIT\)/);
  assert.match(v, /Copyright \(c\) 2022 Paul Miller/);
  assert.match(v, /web-register-04, eab4932/);
  assert.match(read('README.md'), /single exception to “no vendored libraries”|single exception to "no vendored libraries"/);
  assert.match(read('MANIFEST.sha256'), /  src\/lib\/vendor\/argon2id\.js\n/);
});

test('names are checked, never "looked up": LNS has no directory (owner, 2026-11-15)', () => {
  const app = read('src/app.js');
  assert.ok(app.includes('`Checking ${l.name} on LEZ 0.4…`') && app.includes("'Go back to step 3 to check the name again.'"));
  assert.ok(app.includes('The page checks the exact name you type. Names cannot be searched or listed.'));
  // no user-visible "look up" left (identifiers and comments may keep the word)
  const strings = [...app.matchAll(/(['`])((?:(?!\1).)*)\1/g)].map((m) => m[2]);
  assert.ok(!strings.some((t) => /look(?:ing)? ?up|lookup/i.test(t) && /\s/.test(t)), strings.filter((t) => /look(?:ing)? ?up|lookup/i.test(t)).join(' | '));
  const lns = read('src/lib/lns.js');
  assert.ok(lns.includes("fail('not_found', 'No such name on LEZ 0.4.'"));
});
