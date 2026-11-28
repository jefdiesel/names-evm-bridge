// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Offline tests for scripts/check-image-ids.mjs (synthetic fixtures + real repo records check).
//
//   node --test scripts/check-image-ids.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseBuildOutput, runImageIdCheck, formatImageIdReport } from './check-image-ids.mjs';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const ID_TOKEN = '0x' + '11'.repeat(32);
const ID_BRIDGE = '0x' + '22'.repeat(32);
const ID_UPD = '0x' + '33'.repeat(32);
const ID_REL = '0x' + '44'.repeat(32);
const ID_LEGACY_UPD = '0x' + 'aa'.repeat(32);
const ID_LEGACY_REL = '0x' + 'bb'.repeat(32);
const GOOD_DIGEST = 'sha256:' + 'ab'.repeat(32);

function makeFixture({
  digest = GOOD_DIGEST,
  reproPinUpd = ID_UPD,
  reproBuildV2 = true,
  v2Upd = ID_UPD,
  jsonV2Upd = ID_UPD,
  jsV2Upd = ID_UPD,
} = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'check-image-ids-'));
  const versions = `
schema = 1
builds_for = "v2"
live = "v2"

[toolchain]
docker_builder_image = { v = "risczero/risc0-guest-builder", src = "fixture" }
docker_builder_tag = { v = "r0.1.97.0", src = "fixture" }
docker_builder_digest = { v = "${digest}", src = "fixture" }

[[repro_guest]]
name = "token_guest"
image_id = { v = "${ID_TOKEN}", src = "fixture" }

[[repro_guest]]
name = "bridge_guest"
image_id = { v = "${ID_BRIDGE}", src = "fixture" }

[[repro_guest]]
name = "outbound_guest_trusted_tip"
image_id = { v = "${reproPinUpd}", src = "fixture" }

[[repro_guest]]
name = "release_guest"
image_id = { v = "${ID_REL}", src = "fixture" }

[deployments.v1]
label = "legacy non-docker deployment"
status = "deployed"
repro_build = false
web_id = "sepolia-v1"
update_image = { v = "${ID_LEGACY_UPD}", elf_sha256 = "00", src = "fixture" }
release_image = { v = "${ID_LEGACY_REL}", elf_sha256 = "00", src = "fixture" }

[deployments.v2]
label = "reproducible docker deployment"
status = "deployed"
repro_build = ${reproBuildV2}
web_id = "sepolia-v2"
update_image = { v = "${v2Upd}", elf_sha256 = "00", src = "fixture" }
release_image = { v = "${ID_REL}", elf_sha256 = "00", src = "fixture" }

[[deployments.v2.lez_side]]
name = "public"
module_entry = "bridge-v2"
bridge_image = { v = "${ID_BRIDGE.slice(2)}", elf_sha256 = "00", src = "fixture" }
token_image = { v = "${ID_TOKEN.slice(2)}", elf_sha256 = "00", src = "fixture" }
`;
  fs.writeFileSync(path.join(dir, 'VERSIONS.toml'), versions);

  const regJsonPath = path.join(dir, 'modules/lez_bridge/rust-lib/deployments.json');
  fs.mkdirSync(path.dirname(regJsonPath), { recursive: true });
  fs.writeFileSync(
    regJsonPath,
    JSON.stringify({
      default: 'bridge-v2',
      deployments: [
        { name: 'bridge-v2', bridge: { update_image: jsonV2Upd, release_image: ID_REL } },
      ],
    }),
  );

  const regJsPath = path.join(dir, 'web/deposit/src/lib/deployments.js');
  fs.mkdirSync(path.dirname(regJsPath), { recursive: true });
  fs.writeFileSync(
    regJsPath,
    `const SEPOLIA_V2 = {\n  id: 'sepolia-v2',\n  bridge: {\n    updateImage: '${jsV2Upd}',\n    releaseImage: '${ID_REL}',\n  },\n};\n`,
  );

  return dir;
}

const BUILT_OK = [
  '# repro-build output',
  `token_guest, ${ID_TOKEN}`,
  `bridge_guest, ${ID_BRIDGE}`,
  `outbound_guest_trusted_tip, ${ID_UPD}`,
  `release_guest, ${ID_REL}`,
].join('\n');

test('parseBuildOutput: parses valid lines, skips comments/header, catches bad lines and duplicates', () => {
  const ok = parseBuildOutput(`guest,image_id\n# note\n\ntoken_guest, ${ID_TOKEN.slice(2)}\n`);
  assert.deepEqual(ok.errors, []);
  assert.equal(ok.map.get('token_guest'), ID_TOKEN);

  const bad = parseBuildOutput(`token_guest, ${ID_TOKEN}\ntoken_guest, ${ID_BRIDGE}\nbad line\nfoo, 0x1234\n`);
  assert.equal(bad.errors.length, 3);
});

test('aligned reproducible build passes with 0 FAIL and only legacy non-Docker WARNs', () => {
  const dir = makeFixture();
  try {
    const res = runImageIdCheck({ root: dir, builtText: BUILT_OK });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    const warns = res.findings.filter((f) => f.level === 'WARN');
    assert.deepEqual(fails, [], formatImageIdReport(res, true));
    assert.equal(warns.length, 2, formatImageIdReport(res, true));
    assert.ok(warns.every((w) => w.check === 'legacy-non-docker'));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('--strict-legacy turns legacy non-Docker mismatches into FAIL', () => {
  const dir = makeFixture();
  try {
    const res = runImageIdCheck({ root: dir, builtText: BUILT_OK, strictLegacy: true });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    assert.equal(fails.length, 2, formatImageIdReport(res, true));
    assert.ok(fails.every((f) => f.check === 'deployment'));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('mismatch against [[repro_guest]] or a repro_build=true deployment fails', () => {
  const dir = makeFixture({ v2Upd: '0x' + 'ff'.repeat(32), jsonV2Upd: '0x' + 'ff'.repeat(32), jsV2Upd: '0x' + 'ff'.repeat(32) });
  try {
    const res = runImageIdCheck({ root: dir, builtText: BUILT_OK });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    assert.ok(fails.some((f) => f.check === 'deployment' && /v2\.update_image/.test(f.msg)), formatImageIdReport(res, true));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }

  const dir2 = makeFixture();
  try {
    const tampered = BUILT_OK.replace(ID_BRIDGE, '0x' + '99'.repeat(32));
    const res = runImageIdCheck({ root: dir2, builtText: tampered });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    assert.ok(fails.some((f) => f.check === 'repro-pin' && /bridge_guest/.test(f.msg)), formatImageIdReport(res, true));
  } finally {
    fs.rmSync(dir2, { recursive: true, force: true });
  }
});

test('missing guest fails', () => {
  const dir = makeFixture();
  try {
    const partial = `token_guest, ${ID_TOKEN}\nbridge_guest, ${ID_BRIDGE}\n`;
    const res = runImageIdCheck({ root: dir, builtText: partial });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    assert.ok(fails.some((f) => f.check === 'missing' && /release_guest/.test(f.msg)), formatImageIdReport(res, true));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('unpinned digest and empty repro_guest image_id fail by default and warn with --allow-unpinned', () => {
  const dir = makeFixture({ digest: 'sha256:PLACEHOLDER', reproPinUpd: '' });
  try {
    const strict = runImageIdCheck({ root: dir, builtText: BUILT_OK });
    const fails = strict.findings.filter((f) => f.level === 'FAIL');
    assert.ok(fails.some((f) => f.check === 'digest'), formatImageIdReport(strict, true));
    assert.ok(fails.some((f) => f.check === 'repro-pin' && /outbound_guest_trusted_tip/.test(f.msg)), formatImageIdReport(strict, true));

    const relaxed = runImageIdCheck({ root: dir, builtText: BUILT_OK, allowUnpinned: true });
    assert.deepEqual(relaxed.findings.filter((f) => f.level === 'FAIL'), [], formatImageIdReport(relaxed, true));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('deployment record drift from VERSIONS.toml fails', () => {
  const dir = makeFixture({ jsonV2Upd: '0x' + 'ee'.repeat(32), jsV2Upd: '0x' + 'dd'.repeat(32) });
  try {
    const res = runImageIdCheck({ root: dir, builtText: BUILT_OK });
    const fails = res.findings.filter((f) => f.level === 'FAIL');
    assert.ok(fails.some((f) => f.check === 'record' && /deployments\.json/.test(f.msg)), formatImageIdReport(res, true));
    assert.ok(fails.some((f) => f.check === 'record' && /deployments\.js/.test(f.msg)), formatImageIdReport(res, true));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('real repository VERSIONS.toml and deployment records pass --records-only --allow-unpinned', () => {
  const res = runImageIdCheck({ root: REPO_ROOT, recordsOnly: true, allowUnpinned: true });
  const fails = res.findings.filter((f) => f.level === 'FAIL');
  assert.deepEqual(fails, [], formatImageIdReport(res, true));
});
