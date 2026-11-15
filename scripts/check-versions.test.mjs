// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Tests for scripts/check-versions.mjs: an aligned fixture tree passes; each misalignment class
// fails with its message. Offline (the on-chain mode runs against a stand-in fetch).
//
//   node --test scripts/check-versions.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runCheck, parseToml, valuesMatch, reqAdmits, base58ToHex, controlRootFromChain, format } from './check-versions.mjs';

const FIXTURE = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures', 'check-versions', 'aligned');

/** A fresh copy of the aligned tree, with `edits` applied: { file: [[from, to], ...] | null }. */
function tree(edits = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'check-versions-'));
  fs.cpSync(FIXTURE, dir, { recursive: true });
  for (const [file, list] of Object.entries(edits)) {
    const p = path.join(dir, file);
    if (list === null) { fs.rmSync(p, { recursive: true, force: true }); continue; }
    let s = fs.existsSync(p) ? fs.readFileSync(p, 'utf8') : '';
    for (const [from, to] of list) {
      if (from === '') { s += to; continue; }
      assert.ok(s.includes(from), `fixture ${file} has no ${JSON.stringify(from)}`);
      s = s.split(from).join(to);
    }
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, s);
  }
  return dir;
}

async function run(edits, opts = {}) {
  const root = tree(edits);
  try {
    const res = await runCheck({ root, ...opts });
    return { res, fails: res.findings.filter((f) => f.level === 'FAIL'), warns: res.findings.filter((f) => f.level === 'WARN'), text: format(res, true) };
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

const failsWith = (r, check, re) => assert.ok(r.fails.some((f) => f.check === check && re.test(f.msg)), `expected FAIL ${check} ${re}\n${r.text}`);

test('the aligned tree passes (no FAIL, no WARN; private sources print SKIP)', async () => {
  const r = await run({});
  assert.deepEqual(r.fails, [], r.text);
  assert.deepEqual(r.warns, [], r.text);
  assert.ok(r.res.findings.some((f) => f.level === 'SKIP' && /private, not in this copy/.test(f.msg)), r.text);
});

test('a lock version that differs from the role fails', async () => {
  const r = await run({ 'ws/guest/Cargo.lock': [['version = "3.1.0"', 'version = "3.1.1"']] });
  failsWith(r, 'lock', /ws\/guest \[eth_guests\]: risc0-zkvm 3\.1\.1 in Cargo\.lock, next declares 3\.1\.0/);
});

test('a Cargo.toml requirement that refuses the declared version fails', async () => {
  const r = await run({ 'ws/guest/Cargo.toml': [['"=3.1.0"', '"=3.0.5"']] });
  failsWith(r, 'toml', /risc0-zkvm "=3\.0\.5" in ws\/guest\/Cargo\.toml does not admit 3\.1\.0/);
});

test('prover selector vs the verifier selector fails', async () => {
  const r = await run({ 'VERSIONS.toml': [['selector = { v = "0xe71e6620", src = "fixture" }\ncontrol_root = { v = "e6feef13db7aed4a731c7b0852de1f5c89b137726073f625d05333184d3a551d", src = "fixture" }\nbn254_control_id = { v = "de81d091ba14062acb5023b3b36deee95624e3fd47475cb7ff64aed5f9eac20a", src = "fixture" }\n\n[deployments.next.roles',
    'selector = { v = "0x73c457ba", src = "fixture" }\ncontrol_root = { v = "a54dc85ac99f851c92d7c96d7318af41dbe7c0194edfcc37eb4d422a998c1f56", src = "fixture" }\nbn254_control_id = { v = "c07a65145c3cb48b6101962ea607a4dd93c753bb26975cb47feb00d3666e4404", src = "fixture" }\n\n[deployments.next.roles']] });
  failsWith(r, 'selector', /ws\/host \[eth_provers\]: these versions give Groth16 selector 0xe71e6620, but next's verifier .* has 0x73c457ba/);
});

test('an undeclared prover combination fails', async () => {
  const r = await run({ 'ws/host/Cargo.lock': [['version = "3.0.6"', 'version = "3.0.5"']] });
  failsWith(r, 'selector', /ws\/host \[eth_provers\]: risc0-zkvm 3\.1\.0 \/ risc0-circuit-recursion 4\.1\.0 \/ risc0-groth16 3\.0\.5 is no declared \[\[prover\]\] combination/);
});

test('a known-bad crate version fails with the advisory id', async () => {
  const r = await run({ 'VERSIONS.toml': [['[deployments.next.roles.eth_provers]\nrisc0_zkvm = { v = "3.1.0"', '[deployments.next.roles.eth_provers]\nrisc0_zkvm = { v = "3.0.5"']] });
  failsWith(r, 'known-bad', /VERSIONS next \[eth_provers\]: risc0-zkvm 3\.0\.5 < 3\.1\.0 \(GHSA-g6qg-9j8p-7j87\)/);
});

test('a known-bad verifier fails, and its deploy script too', async () => {
  const r = await run({
    'VERSIONS.toml': [['version = { v = "3.1.0", src = "fixture" }\nselector = { v = "0xe71e6620", src = "fixture" }\ncontrol_root = { v = "e6feef13db7aed4a731c7b0852de1f5c89b137726073f625d05333184d3a551d", src = "fixture" }\nbn254_control_id = { v = "de81d091ba14062acb5023b3b36deee95624e3fd47475cb7ff64aed5f9eac20a", src = "fixture" }\n\n[[deployments.v1.lez_side]]',
      'version = { v = "3.0.0", src = "fixture" }\nselector = { v = "0xe71e6620", src = "fixture" }\ncontrol_root = { v = "e6feef13db7aed4a731c7b0852de1f5c89b137726073f625d05333184d3a551d", src = "fixture" }\nbn254_control_id = { v = "de81d091ba14062acb5023b3b36deee95624e3fd47475cb7ff64aed5f9eac20a", src = "fixture" }\n\n[[deployments.v1.lez_side]]'],
    ['', '\n[[pin]]\nfile = "docs/DEPLOYMENTS.md"\nsection = "## Demo vault v1"\nre = \'\\| `releaseImageId\\(\\)` \\| `(0x[0-9a-f]{64})`\'\nvalue = "deployments.v1.release_image"\ndeploy_script = "v1"\n']],
  });
  failsWith(r, 'known-bad', /VERSIONS v1: verifier .* is 3\.0\.0 < 3\.1\.0 \(GHSA-mm6c-35cm-g4x2\)/);
  failsWith(r, 'script-known-bad', /docs\/DEPLOYMENTS\.md deploys against v1's verifier 3\.0\.0/);
});

test('an [[accepted]] entry turns the FAIL into a WARN with its reason', async () => {
  const r = await run({ 'VERSIONS.toml': [
    ['[deployments.next.roles.eth_provers]\nrisc0_zkvm = { v = "3.1.0"', '[deployments.next.roles.eth_provers]\nrisc0_zkvm = { v = "3.0.5"'],
    ['', '\n[[accepted]]\ncheck = "known-bad"\nid = "GHSA-g6qg-9j8p-7j87"\ndeployment = "next"\nrole = "eth_*"\nreason = "fixture reason"\n'],
  ] });
  assert.ok(!r.fails.some((f) => f.check === 'known-bad'), r.text);
  assert.ok(r.warns.some((f) => f.check === 'known-bad' && f.accepted === 'fixture reason'), r.text);
  assert.match(r.text, /WARN known-bad \(accepted: fixture reason\)/);
});

test('an image id in a doc that disagrees fails (abbreviated values match by prefix and suffix)', async () => {
  const ok = await run({});
  assert.ok(ok.res.findings.some((f) => f.level === 'OK' && /docs\/DEPLOYMENTS\.md: deployments\.v1\.update_image/.test(f.msg)), ok.text);
  const r = await run({ 'docs/DEPLOYMENTS.md': [['`0x098f5a1a…3017`', '`0x098f5a1a…3018`']] });
  failsWith(r, 'pin', /docs\/DEPLOYMENTS\.md: 0x098f5a1a…3018 where VERSIONS deployments\.v1\.update_image is 0x098f5a1a/);
});

test('a pin whose pattern no longer matches fails', async () => {
  const r = await run({ 'docs/DEPLOYMENTS.md': [['`releaseImageId()`', '`releaseImage()`']] });
  failsWith(r, 'pin', /docs\/DEPLOYMENTS\.md: no match for/);
});

test('the two registries disagreeing with VERSIONS fail', async () => {
  const r = await run({
    'modules/lez_bridge/rust-lib/deployments.json': [['"cfg_id": "0x9a37708a', '"cfg_id": "0x9a37708b']],
    'web/deposit/src/lib/deployments.js': [["vault: '0x26af59855b99a8af199361289a5ed6088729e60e'", "vault: '0x26af59855b99a8af199361289a5ed6088729e60f'"]],
  });
  failsWith(r, 'registry', /deployments\.json demo-v1-public: cfg_id 0x9a37708b.* ≠ 0x9a37708a/);
  failsWith(r, 'registry', /deployments\.js demo-v1: vault 0x26af.*60f ≠/);
});

test('a registry default that is not the live deployment fails', async () => {
  const r = await run({ 'web/deposit/src/lib/deployments.js': [["const LIVE_DEFAULT = 'demo-v1'", "const LIVE_DEFAULT = 'demo-v0'"]] });
  failsWith(r, 'registry', /LIVE_DEFAULT demo-v0 is not v1's web_id demo-v1/);
});

test('a bridge program id that is not program_id fails', async () => {
  const r = await run({ 'VERSIONS.toml': [['program_id = { v = "0x363397603aee', 'program_id = { v = "0x463397603aee']] });
  failsWith(r, 'registry', /bridge 4eaeK3yiqATX.* is 363397603aee.*, not program_id/);
});

test('a LEZ tag in a Cargo.toml or a LEZ commit in a lock that differs fails', async () => {
  const r = await run({
    'ws/host/Cargo.toml': [['tag = "v0.4.0-rc3"', 'tag = "v0.4.0-rc2"']],
    'ws/cli/Cargo.lock': [['#bafd07be0701398f9bc435f73ab05e9a7936c301', '#0000000000000000000000000000000000000000']],
  });
  failsWith(r, 'lez', /ws\/host: LEZ dependency not on tag v0\.4\.0-rc3/);
  failsWith(r, 'lez', /ws\/cli\/Cargo\.lock: LEZ source .* is not tag v0\.4\.0-rc3 at bafd07be/);
});

test('the LEZ host on another control root than the LEZ chain fails', async () => {
  const r = await run({ 'ws/cli/Cargo.lock': [['version = "4.0.4"', 'version = "4.1.0"']] });
  failsWith(r, 'control-root', /ws\/cli \[lez_host\]: risc0-circuit-recursion 4\.1\.0 .* differs from the LEZ chain's 4\.0\.4/);
});

test('two versions of one risc0 crate in a lock fail', async () => {
  const r = await run({ 'ws/guest/Cargo.lock': [['', '\n[[package]]\nname = "risc0-zkvm"\nversion = "3.0.5"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n']] });
  failsWith(r, 'lock', /ws\/guest\/Cargo\.lock: two versions of risc0-zkvm \(3\.1\.0, 3\.0\.5\)/);
});

test('coverage: an undeclared Cargo.lock fails, a missing declared workspace fails unless private', async () => {
  const r = await run({ 'ws/stray/Cargo.lock': [['', 'version = 4\n']], 'ws/guest': null });
  failsWith(r, 'coverage', /ws\/stray\/Cargo\.lock is in no \[\[workspace\]\]/);
  failsWith(r, 'coverage', /ws\/guest: declared workspace is missing$/);
  assert.ok(r.res.findings.some((f) => f.level === 'SKIP' && /ws\/private-tool: declared workspace is missing \(private/.test(f.msg)), r.text);
});

test('a deployed vault with a zero genesis G2 state fails', async () => {
  const r = await run({ 'VERSIONS.toml': [['0xc3fb14d600000000000000000000000000000000000000000000000000c373e7', '0x0000000000000000000000000000000000000000000000000000000000000000']] });
  failsWith(r, 'g2-zero', /VERSIONS v1: the vault's genesis g2State is zero/);
});

test('--deployment <planned>: a missing genesis G2 state and missing image ids fail (step 0 of a deploy)', async () => {
  const quiet = await run({});
  assert.ok(quiet.res.findings.some((f) => f.level === 'SKIP' && /VERSIONS next: no genesis G2 state yet/.test(f.msg)), quiet.text);
  const r = await run({}, { deployment: 'next' });
  failsWith(r, 'g2-zero', /VERSIONS next: no genesis G2 state yet .*declare it before the deploy/);
  failsWith(r, 'schema', /VERSIONS next: update_image, release_image not declared/);
});

// --- on chain, against a stand-in JSON-RPC
const SEL = { '0xffa1ad74': 'VERSION', '0x053c238d': 'SELECTOR', '0x9181e4b1': 'CR0', '0x8989fa2e': 'CR1', '0x258038e2': 'BN', '0x522d9a28': 'upd', '0x7b4d5f9a': 'rel', '0xfd64db0a': 'g2', '0xed9305e3': 'cfg', '0x2b7ac3f3': 'ver', '0x98812da8': 'pid',
  '0x08c84e70': 'gver', '0x872109a5': 'gupd', '0xf28a6123': 'gsub', '0xc872da3c': 'gkind' };
const word = (h) => `0x${h.replace(/^0x/, '').padEnd(64, '0')}`;
const abiStr = (s) => `0x${'20'.padStart(64, '0')}${s.length.toString(16).padStart(64, '0')}${Buffer.from(s).toString('hex').padEnd(64, '0')}`;
function fakeChain(over = {}) {
  const v = {
    VERSION: abiStr('3.1.0'), SELECTOR: word('e71e6620'), CR0: word('5c1fde52087b1c734aed7adb13effee6'), CR1: word('1d553a4d183353d025f673607237b189'),
    BN: '0x0ac2eaf9d5ae64ffb75c4747fde32456e9ee6db3b32350cb2a0614ba91d081de',
    upd: '0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017', rel: '0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4',
    g2: '0xc3fb14d600000000000000000000000000000000000000000000000000c373e7', cfg: '0x9a37708a79334bbce6946e4dab4d9f6530538e867fcec10083b05b7d19f20d15',
    ver: '0x0000000000000000000000008c915b5071206c6e328bc18529586c513408ab3b', pid: '0x363397603aee2f10bc26aad792cd96692e8bda7faa7c952879564513c822e5af', ...over,
  };
  return async (url, init) => {
    const { id, method, params } = JSON.parse(init.body);
    const result = method === 'eth_getCode' ? '0x6080' : v[SEL[params[0].data]];
    return { status: 200, json: async () => ({ jsonrpc: '2.0', id, result }) };
  };
}

test('--onchain: matching reads pass; a wrong selector and a zero g2State fail', async () => {
  const ok = await run({}, { onchain: true, rpc: 'http://stand-in', fetchImpl: fakeChain() });
  assert.deepEqual(ok.fails, [], ok.text);
  assert.ok(ok.res.findings.some((f) => f.level === 'SKIP' && /next: no vault address/.test(f.msg)), ok.text);
  const r = await run({}, { onchain: true, rpc: 'http://stand-in', fetchImpl: fakeChain({ SELECTOR: word('73c457ba'), g2: `0x${'0'.repeat(64)}` }) });
  failsWith(r, 'onchain', /verifier 0x8c915b.*SELECTOR 0x73c457ba ≠ 0xe71e6620/);
  failsWith(r, 'g2-zero', /v1: vault .* g2State\(\) reads zero on chain/);
});

test('--onchain: a g2State that is not the declared genesis fails unless VERSIONS records an accepted update', async () => {
  const moved = { g2: '0x37d1af10c6a123f2feeadd9eeafb297d65340bc56b15bfdf98b5eabbb1b95003' };
  const r = await run({}, { onchain: true, rpc: 'http://stand-in', fetchImpl: fakeChain(moved) });
  failsWith(r, 'g2-zero', /v1: vault .* g2State\(\) 37d1af10.* is not the declared genesis state .* no accepted update/);
  const ok = await run({ 'VERSIONS.toml': [['genesis_g2_state = { v = "0xc3fb14d6', 'g2_updates = { v = 1, src = "fixture" }\ngenesis_g2_state = { v = "0xc3fb14d6']] }, { onchain: true, rpc: 'http://stand-in', fetchImpl: fakeChain(moved) });
  assert.deepEqual(ok.fails, [], ok.text);
});

// --- the helpers
test('parseToml: the subset parses, anything else throws', () => {
  const t = parseToml('a = 1\n[x.y]\nb = "s" # c\n[[z]]\nk = [\'l\', "m"]\nq = { v = "1", src = "s" }\n[[z]]\nk = []\n[x."y"]\nc = true\n');
  assert.deepEqual(t, { a: 1, x: { y: { b: 's', c: true } }, z: [{ k: ['l', 'm'], q: { v: '1', src: 's' } }, { k: [] }] });
  assert.throws(() => parseToml('a = 1\na = 2\n'), /line 2: duplicate key a/);
  assert.throws(() => parseToml('a = """x"""\n'), /line 1/);
  assert.throws(() => parseToml('a = 1.5\n'), /line 1: unsupported value/);
  assert.throws(() => parseToml('just words\n'), /line 1: expected =/);
  assert.throws(() => parseToml('a = [1,\n2]\n'), /line 1/);
});

test('helpers: value matching, requirements, base58, the on-chain control root', () => {
  assert.ok(valuesMatch('0x6bcbd714…72a4', '0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4'));
  assert.ok(valuesMatch('0x6bcbd714...72a4', '0x6BCBD714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4'));
  assert.ok(!valuesMatch('0x6bcbd714…72a5', '0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4'));
  assert.ok(valuesMatch('0x26AF59855b99a8af199361289A5Ed6088729E60E', '0x26af59855b99a8af199361289a5ed6088729e60e'));
  assert.ok(reqAdmits('=3.0.5', '3.0.5') && !reqAdmits('=3.0.5', '3.1.0'));
  assert.ok(reqAdmits('3.0.5', '3.1.0') && !reqAdmits('3.0.5', '4.0.0') && !reqAdmits('3.0.5', '3.0.4'));
  assert.ok(reqAdmits('3', '3.1.0') && reqAdmits('~3.0.5', '3.0.9') && !reqAdmits('~3.0.5', '3.1.0'));
  assert.equal(base58ToHex('4eaeK3yiqATXhpCEYEFMMtjUn1bS7HLxPwfrH31mXCvr'), '363397603aee2f10bc26aad792cd96692e8bda7faa7c952879564513c822e5af');
  // The v1/v2 verifier (3.0.0) as read on Sepolia (vault/DEPLOYMENTS.md): CONTROL_ROOT_0/1 → the crate's root.
  assert.equal(controlRootFromChain('0x41af18736dc9d7921c859fc95ac84da5', '0x561f8c992a424deb37ccdf4e19c0e7db'), 'a54dc85ac99f851c92d7c96d7318af41dbe7c0194edfcc37eb4d422a998c1f56');
});

// --- a vault behind an UpdateGatedVerifier (v4): the vault names the gate, the gate the RISC Zero verifier
const GATE = '0xe5236Bc2a03235145C0FaCF327fdc9D1c90F4967';
const SUBM = '0xB81aa37D80F8a9588ce9B09B6d6145D1FC4095f4';
const GATE_KIND = 'update-gated: RISC Zero Groth16 verifies every proof; updates only from SUBMITTER (LEZ finality is its word)';
const gateEdit = { 'VERSIONS.toml': [['', `\n[deployments.v1.gate]\naddress = { v = "${GATE}", src = "fixture" }\nsubmitter = { v = "${SUBM}", src = "fixture" }\nkind_text = { v = "${GATE_KIND}", src = "fixture" }\n`]] };
const behindGate = (over = {}) => fakeChain({
  ver: `0x${'0'.repeat(24)}${GATE.slice(2).toLowerCase()}`, gver: '0x0000000000000000000000008c915b5071206c6e328bc18529586c513408ab3b',
  gupd: '0x098f5a1ab0900f06552cca2538999e17954114ad72ac98c7c085c2d1917e3017', gsub: `0x${'0'.repeat(24)}${SUBM.slice(2).toLowerCase()}`, gkind: abiStr(GATE_KIND), ...over,
});

test('--onchain with a gate: the vault names the gate, the gate the verifier, the image and the submitter', async () => {
  const ok = await run(gateEdit, { onchain: true, rpc: 'http://stand-in', fetchImpl: behindGate() });
  assert.deepEqual(ok.fails, [], ok.text);
  assert.ok(ok.res.findings.some((f) => f.level === 'OK' && /v1: gate 0xe5236b.* in front of verifier 0x8c915b/i.test(f.msg)), ok.text);
  const sub = await run(gateEdit, { onchain: true, rpc: 'http://stand-in', fetchImpl: behindGate({ gsub: word('0'.repeat(24) + '11'.repeat(20)) }) });
  failsWith(sub, 'onchain', /v1: gate 0xe5236B.*SUBMITTER\(\)/);
  // the vault pointing straight at the bare verifier while a gate is declared
  const bare = await run(gateEdit, { onchain: true, rpc: 'http://stand-in', fetchImpl: behindGate({ ver: '0x0000000000000000000000008c915b5071206c6e328bc18529586c513408ab3b' }) });
  failsWith(bare, 'onchain', /v1: vault .*verifier\(\)/);
  const img = await run(gateEdit, { onchain: true, rpc: 'http://stand-in', fetchImpl: behindGate({ gupd: word('ab') }) });
  failsWith(img, 'onchain', /v1: gate .*UPDATE_IMAGE_ID\(\)/);
});
