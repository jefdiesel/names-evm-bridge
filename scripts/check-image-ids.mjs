#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// check-image-ids: compare Docker-rebuilt guest image ids (from scripts/repro-build.sh)
// against VERSIONS.toml ([[repro_guest]] and [deployments.*]) and the deployment records
// (modules/lez_bridge/rust-lib/deployments.json, web/deposit/src/lib/deployments.js,
// vault/deployments/sepolia-demo-v2.json, vault/DEPLOYMENTS.md).
//
//   node scripts/check-image-ids.mjs --input FILE [--root DIR] [--versions FILE]
//                                    [--deployment NAME] [--strict-legacy]
//                                    [--allow-unpinned] [--verbose]
//   node scripts/check-image-ids.mjs --records-only [--root DIR] [--versions FILE]
//                                    [--deployment NAME] [--allow-unpinned] [--verbose]
//
// Legacy deployments built natively without Docker (marked `repro_build = false` in
// VERSIONS.toml: v2, v3, v4, v5) are reported as WARN when a Docker-rebuilt image id differs
// from their immutable on-chain id (or FAIL with --strict-legacy). Deployments with
// `repro_build = true` (the default for any new deployment) FAIL on any mismatch.
//
// Exit: 0 no FAIL (WARN and SKIP lines are printed), 1 at least one FAIL, 2 usage or parse error.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseToml, valuesMatch } from './check-versions.mjs';

const val = (x) => (x !== null && typeof x === 'object' && !Array.isArray(x) && 'v' in x ? x.v : x);
const normHex = (s) => String(s ?? '').trim().toLowerCase().replace(/^0x/, '');
const isHex64 = (s) => /^(0x)?[0-9a-fA-F]{64}$/.test(String(s ?? '').trim());
const short = (s) => {
  const t = String(s);
  return t.length > 22 ? `${t.slice(0, 12)}…${t.slice(-6)}` : t;
};

/**
 * Parse `guest name, image id` lines from scripts/repro-build.sh.
 * Ignores blank lines, `#` comments, and an optional `guest,image_id` header line.
 */
export function parseBuildOutput(text) {
  const map = new Map();
  const errors = [];
  const lines = String(text).split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (!line || line.startsWith('#')) continue;
    const parts = line.split(',');
    if (parts.length !== 2) {
      errors.push(`line ${i + 1}: expected "guest name, image id", got ${JSON.stringify(line)}`);
      continue;
    }
    const name = parts[0].trim();
    const id = parts[1].trim();
    if (i === 0 && name.toLowerCase() === 'guest' && id.toLowerCase().replace(/\s+/g, '_') === 'image_id') {
      continue;
    }
    if (!/^[A-Za-z0-9_-]+$/.test(name)) {
      errors.push(`line ${i + 1}: invalid guest name ${JSON.stringify(name)}`);
      continue;
    }
    if (!isHex64(id)) {
      errors.push(`line ${i + 1}: ${name}: invalid 32-byte hex image id ${JSON.stringify(id)}`);
      continue;
    }
    const norm = `0x${normHex(id)}`;
    if (map.has(name)) {
      errors.push(`line ${i + 1}: duplicate guest ${name} (${map.get(name)} and ${norm})`);
      continue;
    }
    map.set(name, norm);
  }
  return { map, errors };
}

/**
 * Run image-id verification against VERSIONS.toml and the deployment records.
 */
export function runImageIdCheck(opts = {}) {
  const root = path.resolve(opts.root || path.join(path.dirname(fileURLToPath(import.meta.url)), '..'));
  const vfile = opts.versions || path.join(root, 'VERSIONS.toml');
  const V = parseToml(fs.readFileSync(vfile, 'utf8'));
  const out = [];
  const add = (level, check, msg, scope = {}) => out.push({ level, check, msg, ...scope });
  const read = (p) => fs.readFileSync(path.join(root, p), 'utf8');
  const exists = (p) => fs.existsSync(path.join(root, p));

  const deps = V.deployments || {};
  if (opts.deployment && !deps[opts.deployment]) {
    throw new Error(`unknown deployment ${opts.deployment} (VERSIONS.toml has: ${Object.keys(deps).join(', ')})`);
  }
  const only = opts.deployment ? [opts.deployment] : Object.keys(deps);
  const reproGuests = V.repro_guest || [];
  if (!reproGuests.length) {
    add('FAIL', 'schema', 'VERSIONS.toml has no [[repro_guest]] entries');
  }

  // 1. Builder image digest pin check.
  const tc = V.toolchain || {};
  const bImg = val(tc.docker_builder_image) || '';
  const bTag = val(tc.docker_builder_tag) || '';
  const bDig = val(tc.docker_builder_digest) || '';
  if (!bImg || !bTag) {
    add('FAIL', 'digest', 'VERSIONS.toml [toolchain] must declare docker_builder_image and docker_builder_tag');
  } else if (/^sha256:[0-9a-f]{64}$/.test(bDig)) {
    add('OK', 'digest', `Docker builder pinned by digest: ${bImg}:${bTag}@${short(bDig)}`);
  } else {
    add(
      opts.allowUnpinned ? 'WARN' : 'FAIL',
      'digest',
      `VERSIONS.toml [toolchain].docker_builder_digest is not a pinned sha256 digest (${bDig || '(empty)'}): inspect ${bImg}:${bTag} with docker buildx imagetools inspect and pin it`,
    );
  }

  // 2. Parse built ids (unless --records-only).
  let built = null;
  if (!opts.recordsOnly) {
    let raw = opts.builtText;
    if (raw === undefined) {
      if (!opts.input) throw new Error('pass --input FILE (or --records-only)');
      raw = opts.input === '-' ? fs.readFileSync(0, 'utf8') : fs.readFileSync(opts.input, 'utf8');
    }
    const parsed = parseBuildOutput(raw);
    for (const e of parsed.errors) add('FAIL', 'parse', e);
    built = parsed.map;
    if (built.size === 0 && parsed.errors.length === 0) {
      add('FAIL', 'parse', 'no "guest name, image id" lines found in build output');
    }

    // Compare against [[repro_guest]].
    const declaredNames = new Set(reproGuests.map((g) => g.name));
    for (const g of reproGuests) {
      const got = built.get(g.name);
      if (!got) {
        add('FAIL', 'missing', `${g.name}: declared in VERSIONS.toml [[repro_guest]] but absent from build output`, { guest: g.name });
        continue;
      }
      const want = val(g.image_id);
      if (!want) {
        add(
          opts.allowUnpinned ? 'WARN' : 'FAIL',
          'repro-pin',
          `${g.name}: Docker build produced ${got}, but VERSIONS.toml [[repro_guest]] image_id is not pinned yet (verify on two machines and pin it)`,
          { guest: g.name },
        );
      } else if (!isHex64(want)) {
        add('FAIL', 'schema', `${g.name}: VERSIONS.toml [[repro_guest]] image_id is not 64 hex chars (${want})`, { guest: g.name });
      } else if (normHex(got) !== normHex(want)) {
        add('FAIL', 'repro-pin', `${g.name}: built ${got} ≠ VERSIONS.toml [[repro_guest]] ${want}`, { guest: g.name });
      } else {
        add('OK', 'repro-pin', `${g.name}: built ${short(got)} = VERSIONS.toml [[repro_guest]]`, { guest: g.name });
      }
    }
    for (const [name, got] of built.entries()) {
      if (!declaredNames.has(name)) {
        add('WARN', 'extra', `${name} (${short(got)}): present in build output but not declared in VERSIONS.toml [[repro_guest]] (research guest?)`, { guest: name });
      }
    }
  }

  // 3. Compare against [deployments.*] in VERSIONS.toml (built vs declared).
  const compareDeploymentImage = (dname, isRepro, field, guestName, declaredVal, elfSha) => {
    if (!declaredVal) return;
    // Skip placeholder image ids (e.g. v2 update_image = keccak256("RFP022/demo/no-update-image")).
    if (elfSha === '') {
      add('SKIP', 'deployment', `${dname}.${field}: placeholder image (no ELF); skipped`, { deployment: dname, guest: guestName });
      return;
    }
    if (!built) return;
    const got = built.get(guestName);
    if (!got) {
      add('FAIL', 'deployment', `${dname}.${field}: guest ${guestName} missing from build output`, { deployment: dname, guest: guestName });
      return;
    }
    if (normHex(got) === normHex(declaredVal)) {
      add('OK', 'deployment', `${dname}.${field} (${guestName}): built ${short(got)} = deployed ${short(declaredVal)}`, { deployment: dname, guest: guestName });
    } else if (isRepro || opts.strictLegacy) {
      add('FAIL', 'deployment', `${dname}.${field} (${guestName}): built ${got} ≠ deployed ${declaredVal}`, { deployment: dname, guest: guestName });
    } else {
      add(
        'WARN',
        'legacy-non-docker',
        `${dname}.${field} (${guestName}): Docker build ${short(got)} ≠ deployed ${short(declaredVal)} (legacy non-Docker build, repro_build = false; do not change on-chain record)`,
        { deployment: dname, guest: guestName },
      );
    }
  };

  for (const dname of only) {
    const d = deps[dname];
    // Default to true (fail closed) for any deployment that does not explicitly set repro_build = false.
    const isRepro = d.repro_build !== false;
    const updGuest = d.update_guest || 'outbound_guest_trusted_tip';
    const relGuest = d.release_guest || 'release_guest';
    if (d.update_image) {
      compareDeploymentImage(dname, isRepro, 'update_image', updGuest, val(d.update_image), d.update_image.elf_sha256);
    }
    if (d.release_image) {
      compareDeploymentImage(dname, isRepro, 'release_image', relGuest, val(d.release_image), d.release_image.elf_sha256);
    }
    for (const side of d.lez_side || []) {
      if (side.bridge_image) {
        compareDeploymentImage(dname, isRepro, `lez_side.${side.name}.bridge_image`, 'bridge_guest', val(side.bridge_image), side.bridge_image.elf_sha256);
      }
      if (side.token_image) {
        compareDeploymentImage(dname, isRepro, `lez_side.${side.name}.token_image`, 'token_guest', val(side.token_image), side.token_image.elf_sha256);
      }
    }
  }

  // 4. Cross-check deployment record files against VERSIONS.toml (and built ids for repro deployments).
  const checkRecordValue = (file, label, shown, want, dname, isRepro, guestName) => {
    if (!shown || !want) return;
    if (!valuesMatch(shown, want)) {
      add('FAIL', 'record', `${file} ${label}: ${shown} ≠ VERSIONS.toml ${dname} (${want})`, { file, deployment: dname });
      return;
    }
    add('OK', 'record', `${file} ${label} = VERSIONS.toml ${dname} (${short(want)})`, { file, deployment: dname });
    if (built && isRepro && guestName && built.has(guestName)) {
      const got = built.get(guestName);
      if (normHex(got) !== normHex(want)) {
        add('FAIL', 'record', `${file} ${label}: ${shown} ≠ Docker-built ${guestName} (${got})`, { file, deployment: dname, guest: guestName });
      }
    }
  };

  // 4a. modules/lez_bridge/rust-lib/deployments.json
  const regJson = 'modules/lez_bridge/rust-lib/deployments.json';
  if (exists(regJson)) {
    const j = JSON.parse(read(regJson));
    const sideByModule = new Map();
    for (const [dname, d] of Object.entries(deps)) {
      for (const s of d.lez_side || []) {
        if (s.module_entry) sideByModule.set(s.module_entry, { dname, d, s });
      }
    }
    for (const entry of j.deployments || []) {
      const hit = sideByModule.get(entry.name);
      if (!hit || !only.includes(hit.dname)) continue;
      const { dname, d } = hit;
      const isRepro = d.repro_build !== false;
      if (entry.bridge?.update_image) {
        checkRecordValue(regJson, `${entry.name}.bridge.update_image`, entry.bridge.update_image, val(d.update_image), dname, isRepro, d.update_guest || 'outbound_guest_trusted_tip');
      }
      if (entry.bridge?.release_image) {
        checkRecordValue(regJson, `${entry.name}.bridge.release_image`, entry.bridge.release_image, val(d.release_image), dname, isRepro, d.release_guest || 'release_guest');
      }
    }
  } else {
    add('SKIP', 'record', `${regJson}: not present`, { file: regJson });
  }

  // 4b. web/deposit/src/lib/deployments.js
  const regJs = 'web/deposit/src/lib/deployments.js';
  if (exists(regJs)) {
    const text = read(regJs);
    const blocks = [...text.matchAll(/^const (SEPOLIA_[A-Z0-9_]+) = \{([\s\S]*?)^\};/gm)];
    for (const b of blocks) {
      const idMatch = /^\s*id:\s*'([^']+)'/m.exec(b[2]);
      if (!idMatch) continue;
      const dname = Object.keys(deps).find((n) => deps[n].web_id === idMatch[1]);
      if (!dname || !only.includes(dname)) continue;
      const d = deps[dname];
      const isRepro = d.repro_build !== false;
      const upd = /^\s*updateImage:\s*'([^']+)'/m.exec(b[2]);
      const rel = /^\s*releaseImage:\s*'([^']+)'/m.exec(b[2]);
      if (upd) {
        checkRecordValue(regJs, `${idMatch[1]}.bridge.updateImage`, upd[1], val(d.update_image), dname, isRepro, d.update_guest || 'outbound_guest_trusted_tip');
      }
      if (rel) {
        checkRecordValue(regJs, `${idMatch[1]}.bridge.releaseImage`, rel[1], val(d.release_image), dname, isRepro, d.release_guest || 'release_guest');
      }
    }
  } else {
    add('SKIP', 'record', `${regJs}: not present`, { file: regJs });
  }

  // 4c. vault/deployments/sepolia-demo-v2.json (when v2 is in scope)
  const v2Json = 'vault/deployments/sepolia-demo-v2.json';
  if (only.includes('v2') && deps.v2 && exists(v2Json)) {
    const j = JSON.parse(read(v2Json));
    const d = deps.v2;
    const isRepro = d.repro_build !== false;
    const seq2 = (d.lez_side || []).find((s) => s.name === 'seq2');
    checkRecordValue(v2Json, 'updateImageId', j.updateImageId, val(d.update_image), 'v2', isRepro, null);
    checkRecordValue(v2Json, 'releaseImageId', j.releaseImageId, val(d.release_image), 'v2', isRepro, 'release_guest');
    if (seq2) {
      checkRecordValue(v2Json, 'lezImages.bridge', j.lezImages?.bridge, val(seq2.bridge_image), 'v2', isRepro, 'bridge_guest');
      checkRecordValue(v2Json, 'lezImages.token', j.lezImages?.token, val(seq2.token_image), 'v2', isRepro, 'token_guest');
    }
  }

  // 4d. vault/DEPLOYMENTS.md image-id pins declared in VERSIONS.toml
  const depMd = 'vault/DEPLOYMENTS.md';
  if (exists(depMd)) {
    const fullMd = read(depMd);
    const resolve = (p) => {
      const parts = p.split('.');
      let x = V;
      for (const k of parts) {
        x = Array.isArray(x) ? x.find((e) => e.name === k) : x?.[k];
        if (x === undefined) return undefined;
      }
      return val(x);
    };
    for (const pin of V.pin || []) {
      if (pin.file !== depMd || !/_image$/.test(pin.value)) continue;
      const dm = /^deployments\.([^.]+)\./.exec(pin.value);
      if (dm && !only.includes(dm[1])) continue;
      const dname = dm ? dm[1] : undefined;
      const want = resolve(pin.value);
      let slice = fullMd;
      if (pin.section) {
        const i = fullMd.indexOf(`\n${pin.section}`);
        if (i < 0) {
          add('FAIL', 'record', `${depMd}: section "${pin.section}" not found`, { file: depMd, deployment: dname });
          continue;
        }
        const j = fullMd.indexOf('\n## ', i + 1 + pin.section.length);
        slice = fullMd.slice(i, j < 0 ? undefined : j);
      }
      const shown = [...slice.matchAll(new RegExp(pin.re, 'gm'))].map((m) => m[1]);
      if (!shown.length) {
        add('FAIL', 'record', `${depMd}: no match for /${pin.re}/ (${pin.value})`, { file: depMd, deployment: dname });
        continue;
      }
      const bad = shown.filter((s) => !valuesMatch(s, want));
      if (bad.length) {
        add('FAIL', 'record', `${depMd}: ${bad.join(', ')} ≠ VERSIONS.toml ${pin.value} (${want})`, { file: depMd, deployment: dname });
      } else {
        add('OK', 'record', `${depMd}: ${pin.value} = ${short(want)}`, { file: depMd, deployment: dname });
      }
    }
  }

  return { findings: out };
}

export function formatImageIdReport({ findings }, verbose = false) {
  const order = { FAIL: 0, WARN: 1, SKIP: 2, OK: 3 };
  const lines = [];
  for (const f of [...findings].sort((a, b) => order[a.level] - order[b.level])) {
    if (f.level === 'OK' && !verbose) continue;
    lines.push(`${f.level.padEnd(4)} ${f.check}: ${f.msg}`);
  }
  const n = (l) => findings.filter((f) => f.level === l).length;
  lines.push(`check-image-ids: ${n('FAIL')} FAIL, ${n('WARN')} WARN, ${n('SKIP')} SKIP, ${n('OK')} OK`);
  return lines.join('\n');
}

function main(argv) {
  const opts = {};
  let verbose = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--records-only') opts.recordsOnly = true;
    else if (a === '--strict-legacy') opts.strictLegacy = true;
    else if (a === '--allow-unpinned') opts.allowUnpinned = true;
    else if (a === '--verbose' || a === '-v') verbose = true;
    else if (['--input', '--root', '--versions', '--deployment'].includes(a) && argv[i + 1]) {
      opts[a.slice(2)] = argv[++i];
    } else if (a === '--help' || a === '-h') {
      const src = fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n');
      console.log(
        src
          .slice(3, src.findIndex((l) => l.startsWith('// Exit:')) + 1)
          .map((l) => l.replace(/^\/\/ ?/, ''))
          .join('\n'),
      );
      return 0;
    } else {
      console.error(`check-image-ids: unknown argument ${a} (see --help)`);
      return 2;
    }
  }
  let res;
  try {
    res = runImageIdCheck(opts);
  } catch (e) {
    console.error(`check-image-ids: ${e.message}`);
    return 2;
  }
  console.log(formatImageIdReport(res, verbose));
  return res.findings.some((f) => f.level === 'FAIL') ? 1 : 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
