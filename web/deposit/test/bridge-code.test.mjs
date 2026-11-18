// SPDX-License-Identifier: MIT OR Apache-2.0
// The bridge code (src/lib/bridge-code.js) against the independent vectors, and the page's own live
// deployments encoding to the same codes (the drift check: the module checks its list the same way).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  encodeBridgeCode, decodeBridgeCode, bridgeCodeWords, bridgeCodeOf, bridgeFieldsOf, BR_HRP, BRIDGE_CODE_TAG, CFG_TAG,
} from '../src/lib/bridge-code.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';

const V = JSON.parse(readFileSync(new URL('./vectors/bridge-code-v1.json', import.meta.url)));
const hex = (b) => '0x' + Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');

test('the vectors are for this format', () => {
  assert.equal(V.hrp, BR_HRP);
  assert.equal(V.tag, BRIDGE_CODE_TAG);
  assert.equal(V.cfgTag, CFG_TAG);
});

test('every valid vector encodes, decodes (lower and upper case, and in a link) and gives its words', async () => {
  for (const v of V.valid) {
    assert.equal(await encodeBridgeCode(v.fields), v.code, v.name);
    for (const input of [v.code, v.code.toUpperCase(), `https://nameswallet.app/bridge/#${v.code}`, `  ${v.code}\n`]) {
      const d = await decodeBridgeCode(input);
      assert.deepEqual(d.fields, v.fields, v.name);
      assert.equal(hex(d.payload), v.payload, v.name);
    }
    const w = await bridgeCodeWords((await decodeBridgeCode(v.code)).payload);
    assert.deepEqual(w.indexes, v.indexes, v.name);
    assert.deepEqual(w.words, v.words, v.name);
    assert.equal(w.words_text, v.words_text, v.name);
  }
});

test('every invalid vector is refused with its code, and nothing is encoded from bad fields', async () => {
  for (const v of V.invalid) {
    await assert.rejects(decodeBridgeCode(v.code), (e) => e.code === v.error, `${v.name}: expected ${v.error}`);
  }
  const good = V.valid[0].fields;
  await assert.rejects(encodeBridgeCode({ ...good, vault: V.valid[1].fields.vault }), (e) => e.code === 'cfg_id');
  await assert.rejects(encodeBridgeCode({ ...good, endpoint: 'http://x.example/' }), (e) => e.code === 'endpoint');
  await assert.rejects(encodeBridgeCode({ ...good, release: 'sometimes' }), (e) => e.code === 'field');
});

test('the page’s live deployment and v4 encode to the vectors’ v5 and v4 codes', async () => {
  const live = DEPLOYMENTS[0];
  assert.equal(live.id, 'sepolia-v5');
  for (const [id, name] of [['sepolia-v5', 'v5'], ['sepolia-v4', 'v4']]) {
    const d = DEPLOYMENTS.find((x) => x.id === id);
    const want = V.valid.find((x) => x.name === name);
    const c = await bridgeCodeOf(d);
    assert.deepEqual(c.fields, want.fields, id);
    assert.equal(c.code, want.code, id);
    assert.equal(c.words_text, want.words_text, id);
  }
  // Deployments without a bridge block (v3, v2: their vaults can't pay out) have no code.
  for (const d of DEPLOYMENTS.filter((x) => !['sepolia-v5', 'sepolia-v4'].includes(x.id))) assert.equal(bridgeFieldsOf(d), null, d.id);
});

test('the Verify page shows the live deployment’s code and words, as text', async () => {
  const page = readFileSync(new URL('../about/verify/index.html', import.meta.url), 'utf8');
  const c = await bridgeCodeOf(DEPLOYMENTS[0]);
  assert.ok(page.includes(c.code), 'about/verify shows the live bridge code');
  assert.ok(page.includes(c.words_text), 'about/verify shows its four words');
});
