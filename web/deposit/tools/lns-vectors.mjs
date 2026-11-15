#!/usr/bin/env node
// Writes test/vectors/lns-v1.json: real LNS records read from the LEZ 0.4 bridge test network,
// with the exact JSON-RPC batches src/lib/lns.js sends and the answers it got, so the tests replay
// them offline. Read-only calls only (getChannelId, getAccount).
//
//   node tools/lns-vectors.mjs --ref <logos-names>/nameswallet-lns/lns-src/page/src/names.js [name ...]
//
// --ref is the names web page's bundle (logos-names branch web-register-04), used ONLY here, for its
// Argon2id deriveName (SPEC.md §3); this page has no Argon2 of its own. It is never shipped.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { writeFileSync } from 'node:fs';
import { resolve as resolvePath, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolve, nameRecordAccount, LNS_RPC, LNS_PROGRAM } from '../src/lib/lns.js';

const args = process.argv.slice(2);
const at = args.indexOf('--ref');
if (at < 0) { console.error('usage: lns-vectors.mjs --ref <names.js> [name ...]'); process.exit(2); }
const ref = await import(resolvePath(args[at + 1]));
args.splice(at, 2);
const names = args.length ? args : ['m2-test-111', 'lns-smoke-ourzerofour', 'zq7-rehearsal-04a'];
const hex = (b) => Buffer.from(b).toString('hex');

const live = [];
for (const name of names) {
  const derived = await ref.deriveName(name);
  const exchanges = [];
  const recordingFetch = async (url, init) => {
    const response = await fetch(url, { ...init, headers: { ...init.headers, 'User-Agent': 'lns-vectors/1' } });
    const text = await response.text();
    exchanges.push({ request: JSON.parse(init.body), status: response.status, response: text });
    return new Response(text, { status: response.status, headers: response.headers });
  };
  const result = await resolve(name, { derive: async () => derived, fetch: recordingFetch });
  live.push({
    name,
    id: hex(derived.id),
    key: hex(derived.key),
    recordAccount: await nameRecordAccount(derived.id),
    referenceRecordAccount: await ref.recordAddress(LNS_PROGRAM, derived.id),
    exchanges,
    result,
  });
  console.log(name, JSON.stringify(result));
  await new Promise((r) => setTimeout(r, 500));
}

const here = dirname(fileURLToPath(import.meta.url));
const out = {
  format: 'lns-vectors', version: 1,
  note: 'Real reads from the LEZ 0.4 bridge test network, replayed offline by test/lns.test.mjs. id/key come from the reference Argon2id (logos-names web-register-04 names.js deriveName); they are public: anyone who knows the name computes them.',
  readAt: new Date().toISOString(),
  rpc: LNS_RPC,
  program: LNS_PROGRAM,
  reference: { repo: 'logos-names', branch: 'web-register-04', commit: 'eab4932eeb9fd627793a7c531c961770ef21a73a', registry: 'rc3-port 045a6fd2e80f9bed855424c0ea0ae48ca229d58b' },
  // Copied from logos-names rc3-port 045a6fd SPEC.md §10 (lines 640-700), asserted there by
  // seal/src/lib.rs and resolver/names.test.js (lines 34-60, 105-107).
  spec: {
    program: 'US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx',
    derivations: [
      ['jef', '7041d9e447f06bdfe327d870402b1524cfe38bd03445b8fc3d0e87bc978eb763', 'aad91a1a3dd0fb56a213212fbea58fe25d9139db42fc59ce2116c3cfa3440088'],
      ['logos', '4a8bf61b00dbc63907a935aa15ed534bb32d866978aa1f2128f9b6f37e0c4025', 'd67dd628fa51fa62bd32ea82c3e9ff1b67f93b01103be956eaebc52f1699df44'],
      ['a', '24b73f0b70605c7f27fb91f9952b550e13c4cc38dcf325be45442d1a2cb085b4', 'cd9ded7c94d52743db4ee67152766480be5bb5199e83b6696e502cf818933150'],
      ['abcdefghijklmnopqrstuvwxyz012345', '0044f00be946536689d83e364ac25462c65c615d74059652d1f3e2977bd75403', '79c7aa8c06d4983a59b235915ae4f5767918824a9150d5d8b576761b7d3654f6'],
      ['a-b-c', '707decf4fde6d9fb61d255c23b34e042c846a0f4b0834151dcb2c1ef4160e849', 'e3666a8572f7cabe94f6717491bff34c70eeca216436a142185bd8a86b6ccf31'],
    ],
    recordAccounts: [
      ['7041d9e447f06bdfe327d870402b1524cfe38bd03445b8fc3d0e87bc978eb763', '5b5xVGob2GYvrq21MSuxm1MfieucDEy9yxhDwdXndbpD'],
      ['000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f', '7bwwvMLtxqqNYiXsd5bYaq8e31E1odYf6jAK5z7tmUdd'],
    ],
    sealedProfile: {
      name: 'jef',
      key: 'aad91a1a3dd0fb56a213212fbea58fe25d9139db42fc59ce2116c3cfa3440088',
      sealed: '000102030405060708090a0b7a39f7ff67b20894263a28d040c0f2711c04ba247fcf5cb107e27b59'
        + '05b8a0664b28cd25512addcb74af3efa5a8d6eb3bbb8e89fb8dba0aa2739252efed2be01ab019998'
        + 'a07d416c788d8c9b699891e85bedd1118da7089377220bdb0ab8ae29661bf20e006c5fbffb222237'
        + '2ea0a2589ed49023186a77e9afee1af338f4a2377afa40da018789ca715318d981b556f71f9640eb'
        + 'd056d8dcbe2b71845b45a578721ccc2c889df37b926c7da5a5202dbf3d50dc04883673134ce0772f'
        + 'bcd7fbc8f31f455bb39b3d867d5e6772c86e59f42a9d04ffaacaa6119b1156d238665c0cf50028a9'
        + 'cbf8b3189fc45a15a724c5775a835a8ccdff6a573d8e046d78e847b7da5668bc8529aedb3bdc70a9'
        + '9db9b9b4',
      profile: { name: 'jef', account: '8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR', payablePrivately: false, texts: { chat: 'hi' } },
    },
  },
  live,
};
writeFileSync(join(here, '../test/vectors/lns-v1.json'), JSON.stringify(out, (_k, v) => (typeof v === 'bigint' ? v.toString() : v), 1) + '\n');
