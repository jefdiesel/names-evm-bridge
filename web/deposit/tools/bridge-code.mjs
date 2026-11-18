#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
// Prints the bridge code (logosbr1…) and its four words for a deployment of src/lib/deployments.js
// (default: the live one, DEPLOYMENTS[0]). Offline. The operator's step for a new vault: add its
// entry with a `bridge` block, then put this output on about/verify/ (test/bridge-code.test.mjs
// checks the page shows the live one's) and hand the code to wallets (Settings → Bridge → Add a bridge).
//
//   node tools/bridge-code.mjs [deployment id] [--json]
import { DEPLOYMENTS } from '../src/lib/deployments.js';
import { bridgeCodeOf } from '../src/lib/bridge-code.js';

const args = process.argv.slice(2);
const id = args.find((a) => !a.startsWith('--')) || DEPLOYMENTS[0].id;
const d = DEPLOYMENTS.find((x) => x.id === id);
if (!d) {
  console.error(`no deployment ${id}; known: ${DEPLOYMENTS.map((x) => x.id).join(', ')}`);
  process.exit(2);
}
const c = await bridgeCodeOf(d);
if (!c) {
  console.error(`${id} has no bridge block, so no bridge code`);
  process.exit(3);
}
if (args.includes('--json')) console.log(JSON.stringify({ id, code: c.code, words: c.words, words_text: c.words_text, fields: c.fields }, null, 1));
else console.log(`${id}\nwords: ${c.words_text}\ncode:  ${c.code}`);
