// The vendored Argon2id (src/lib/vendor/argon2id.js) against the LNS registry's own vectors
// (logos-names SPEC.md §10, rc3-port): RFC 9106 §5.3, then the five name derivations.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { argon2idAsync } from '../src/lib/vendor/argon2id.js';
import { deriveName } from '../src/lib/lnsderive.js';

const hex = (b) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
const fill = (n, v) => new Uint8Array(n).fill(v);

test('RFC 9106 §5.3: Argon2id v0x13, 32 KiB, 3 passes, 4 lanes, with secret and associated data', async () => {
  const out = await argon2idAsync(fill(32, 1), fill(16, 2), { t: 3, m: 32, p: 4, version: 0x13, key: fill(8, 3), personalization: fill(12, 4), dkLen: 32 });
  assert.equal(hex(out), '0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659');
});

// SPEC §10, "Derivation": master, id and key of five names
const V = [
  ['jef', '60e3df20e1c473ab71c666b913af96619fa14b49593e92598bd7709582e5a74e', '7041d9e447f06bdfe327d870402b1524cfe38bd03445b8fc3d0e87bc978eb763', 'aad91a1a3dd0fb56a213212fbea58fe25d9139db42fc59ce2116c3cfa3440088'],
  ['logos', '140f118f9f0c7a99f04e3214d8e084590cd69687be6effb7f57dcc1a06a35372', '4a8bf61b00dbc63907a935aa15ed534bb32d866978aa1f2128f9b6f37e0c4025', 'd67dd628fa51fa62bd32ea82c3e9ff1b67f93b01103be956eaebc52f1699df44'],
  ['a', 'c42903b96a43d01c6c912b647c21902bffd17afdf932e5f8c407dea08a32b494', '24b73f0b70605c7f27fb91f9952b550e13c4cc38dcf325be45442d1a2cb085b4', 'cd9ded7c94d52743db4ee67152766480be5bb5199e83b6696e502cf818933150'],
  ['abcdefghijklmnopqrstuvwxyz012345', 'a9296b3bfd39d63908326e19c2d1b20a27cb6b47df1829725c68355bc3585f5b', '0044f00be946536689d83e364ac25462c65c615d74059652d1f3e2977bd75403', '79c7aa8c06d4983a59b235915ae4f5767918824a9150d5d8b576761b7d3654f6'],
  ['a-b-c', 'c098ecc6c6af9cfcf449666ce012e3e930e076d870cb135ed8f1f058e7728362', '707decf4fde6d9fb61d255c23b34e042c846a0f4b0834151dcb2c1ef4160e849', 'e3666a8572f7cabe94f6717491bff34c70eeca216436a142185bd8a86b6ccf31'],
];

test('SPEC §10 derivations: master, id and key of jef, logos, a, a 32-character name and a-b-c', async () => {
  for (const [name, master, id, key] of V) {
    const d = await deriveName(name);
    assert.equal(hex(d.master), master, `${name} master`);
    assert.equal(hex(d.id), id, `${name} id`);
    assert.equal(hex(d.key), key, `${name} key`);
  }
});
