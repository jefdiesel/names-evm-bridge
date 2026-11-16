// SPDX-License-Identifier: MIT OR Apache-2.0
// The page's deployment list: v3 (the fixed programs, an operator-attested vault) is the live
// default since 2026-11-14; v2 stays listed (one line, LIVE_DEFAULT), each with its own desk, cfgId and note.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEPLOYMENTS, deskOf, findDeployment } from '../src/lib/deployments.js';

test('the default is the newest listed of v3, v4, v5; the older ones stay listed', () => {
  const ids = DEPLOYMENTS.map((d) => d.id);
  const want = ['sepolia-demo-v2', 'sepolia-demo-v3', ...['sepolia-v4', 'sepolia-v5'].filter((x) => ids.includes(x))];
  assert.deepEqual([...ids].sort(), [...want].sort());
  assert.ok(want.slice(1).includes(DEPLOYMENTS[0].id), DEPLOYMENTS[0].id);
});

// v4 (public-demo/V4-PLAN.md step C1): only once the apply script wrote it. Its visible line says the way
// back is checked by proof, never that it cannot release.
test('v4, when listed: a proof-checked vault, honestly labelled', (t) => {
  const v4 = DEPLOYMENTS.find((d) => d.id === 'sepolia-v4');
  if (!v4) return t.skip('v4 is not listed yet');
  assert.match(v4.vault, /^0x[0-9a-f]{40}$/);
  assert.match(v4.cfgId, /^0x[0-9a-f]{64}$/);
  assert.ok(Number.isInteger(v4.deployBlock) && v4.deployBlock > 12134824);
  assert.ok(deskOf(v4));
  assert.equal(v4.demoAccount, null);
  // The default's line until v5 (public-demo/v5/apply-v5-entries.py --default), then an earlier vault's.
  assert.equal(v4.demoShort, DEPLOYMENTS[0].id === 'sepolia-v4'
    ? 'Withdrawals back to Ethereum are checked by proof on Ethereum. The first proofs are being set up; until then a burn waits.'
    : 'An earlier test vault: withdrawals work, with a one-hour wait.');
  assert.match(v4.demoNote, /operator’s word until a finality proof/);
  assert.doesNotMatch(v4.demoShort + v4.demoNote, /\bnever\b|can’t pay|wrong starting value/i);
  assert.equal(findDeployment(11155111, v4.vault), v4);
});

// v5 (public-demo/V5-PLAN.md step C1): only once the apply script wrote it.
test('v5, when listed: v4\'s design with no release delay, honestly labelled', (t) => {
  const v5 = DEPLOYMENTS.find((d) => d.id === 'sepolia-v5');
  if (!v5) return t.skip('v5 is not listed yet');
  assert.match(v5.vault, /^0x[0-9a-f]{40}$/);
  assert.match(v5.cfgId, /^0x[0-9a-f]{64}$/);
  assert.ok(Number.isInteger(v5.deployBlock) && v5.deployBlock > 12142003);
  assert.ok(deskOf(v5));
  assert.equal(v5.demoAccount, null);
  assert.equal(v5.demoShort, 'Withdrawals back to Ethereum are checked by proof on Ethereum.');
  assert.match(v5.demoNote, /operator’s word until a finality proof/);
  assert.doesNotMatch(v5.demoShort + v5.demoNote, /\bnever\b|can’t pay|one-hour|delay/i);
  assert.notEqual(v5.vault, DEPLOYMENTS.find((d) => d.id === 'sepolia-v4').vault);
  assert.equal(findDeployment(11155111, v5.vault), v5);
});

test('v3 values as deployed (vault/DEPLOYMENTS.md)', () => {
  const v3 = findDeployment(11155111, '0x26af59855b99A8AF199361289A5Ed6088729E60E');
  assert.ok(v3);
  assert.equal(v3.id, 'sepolia-demo-v3');
  assert.equal(v3.cfgId, '0x9a37708a79334bbce6946e4dab4d9f6530538e867fcec10083b05b7d19f20d15');
  assert.equal(v3.deployBlock, 12134824);
  assert.equal(deskOf(v3), '0x6b7f38d05751b1ed432a0a0d377a63cfb8c4dbc5');
  assert.notEqual(deskOf(v3), deskOf(findDeployment(11155111, '0x8c8b2664de28e5287158db4db65d448638871b74')));
  // v3 can never release (outbound state deployed at zero, 2026-11-15): its notes say so plainly,
  // and nothing on the page still says a release would need the operator key
  assert.equal(v3.demoShort, 'This test vault was set up with a wrong starting value, so it can’t pay ETH back out. Deposits can be claimed on the bridge test network; withdrawals come with the next vault.');
  assert.equal(v3.demoNote, 'Why: the vault on Sepolia was deployed with its outbound starting state at zero, and no update the bridge produces starts from zero. The bridge’s way back (burn, update, release) is built and runs in the local demo; it needs a new vault. Test coins only.');
  // the visible line never says "never"/"ever" (read as the whole bridge having no way back)
  assert.doesNotMatch(v3.demoShort, /\bnever\b|\bever\b/i);
  assert.doesNotMatch(v3.demoNote + v3.demoShort, /operator key|13 May 2027|not available yet/);
  assert.equal(v3.demoAccount, null, 'no demo account on v3: a private claim there is unproven');
  assert.match(DEPLOYMENTS.find((d) => d.id === 'sepolia-demo-v2').demoNote, /for good/);
});

// V4-PLAN step C1: the v4 entry is written by public-demo/v4/apply-v4-entries.py from read-back values;
// a build must never carry one of its <<V4:KEY>> placeholders (or any other unfilled marker).
test('no deployment holds a placeholder', () => {
  const walk = (v, path) => {
    if (typeof v === 'string') assert.doesNotMatch(v, /<<V4:|<<[A-Z_]+>>|PLACEHOLDER/, `${path} holds a placeholder`);
    else if (v && typeof v === 'object') for (const [k, x] of Object.entries(v)) walk(x, `${path}.${k}`);
  };
  for (const d of DEPLOYMENTS) walk(d, d.id);
});
