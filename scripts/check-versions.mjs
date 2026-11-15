#!/usr/bin/env node
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// check-versions: fails on any version that does not line up with VERSIONS.toml.
//
//   node scripts/check-versions.mjs [--deployment NAME] [--onchain [--rpc URL]] [--root DIR]
//                                   [--versions FILE] [--verbose]
//
// Offline it reads every Cargo.lock and Cargo.toml in the tree, the two deployment registries
// (modules/lez_bridge/rust-lib/deployments.json, web/deposit/src/lib/deployments.js) and every
// [[pin]] (docs, scripts, BUILD.txt copies), and checks VERSIONS.toml's own records (selector vs
// verifier, control roots, known-bad versions, the genesis G2 state). --onchain also reads each
// verifier and vault on Sepolia (raw JSON-RPC; --rpc takes a comma-separated list tried in order,
// default https://sepolia.gateway.tenderly.co then PublicNode).
// --deployment limits the deployment checks (records, registries, pins, on-chain reads) to one
// deployment; the workspaces are always checked against builds_for, the deployment this tree
// builds. No dependencies; Node 18 or later.
//
// Exit: 0 no FAIL (WARN and SKIP lines are printed), 1 at least one FAIL, 2 usage or parse error.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// ---------------------------------------------------------------------------------------------
// A strict TOML subset: [table], [[array]], key = string | literal | integer | bool | [array] |
// { inline table }. One key per line. Anything else throws.

export function parseToml(text) {
  const root = {};
  let cur = root;
  const lines = text.split(/\r?\n/);
  const err = (n, m) => { throw new Error(`VERSIONS.toml line ${n + 1}: ${m}`); };
  for (let n = 0; n < lines.length; n++) {
    const p = { s: lines[n], i: 0, n };
    skipWs(p);
    if (p.i >= p.s.length || p.s[p.i] === '#') continue;
    if (p.s.startsWith('[[', p.i)) {
      p.i += 2;
      const keys = parseKeyPath(p, err);
      if (!p.s.startsWith(']]', p.i)) err(n, 'expected ]]');
      p.i += 2;
      endOfLine(p, err);
      const parent = walkTo(root, keys.slice(0, -1), n, err);
      const k = keys[keys.length - 1];
      if (parent[k] === undefined) parent[k] = [];
      if (!Array.isArray(parent[k])) err(n, `${keys.join('.')} is not an array of tables`);
      const t = {};
      parent[k].push(t);
      cur = t;
    } else if (p.s[p.i] === '[') {
      p.i += 1;
      const keys = parseKeyPath(p, err);
      if (p.s[p.i] !== ']') err(n, 'expected ]');
      p.i += 1;
      endOfLine(p, err);
      cur = walkTo(root, keys, n, err);
    } else {
      const k = parseKey(p, err);
      skipWs(p);
      if (p.s[p.i] !== '=') err(n, 'expected =');
      p.i += 1;
      skipWs(p);
      const v = parseValue(p, err);
      endOfLine(p, err);
      if (Object.prototype.hasOwnProperty.call(cur, k)) err(n, `duplicate key ${k}`);
      cur[k] = v;
    }
  }
  return root;
}

function walkTo(root, keys, n, err) {
  let t = root;
  for (const k of keys) {
    if (t[k] === undefined) t[k] = {};
    t = t[k];
    if (Array.isArray(t)) t = t[t.length - 1];
    if (t === null || typeof t !== 'object') err(n, `${keys.join('.')} is not a table`);
  }
  return t;
}
function skipWs(p) { while (p.i < p.s.length && (p.s[p.i] === ' ' || p.s[p.i] === '\t')) p.i++; }
function endOfLine(p, err) {
  skipWs(p);
  if (p.i < p.s.length && p.s[p.i] !== '#') err(p.n, `unexpected text: ${p.s.slice(p.i)}`);
}
function parseKeyPath(p, err) {
  const keys = [];
  for (;;) {
    skipWs(p);
    keys.push(parseKey(p, err));
    skipWs(p);
    if (p.s[p.i] !== '.') return keys;
    p.i++;
  }
}
function parseKey(p, err) {
  if (p.s[p.i] === '"' || p.s[p.i] === "'") return parseString(p, err);
  const m = /^[A-Za-z0-9_-]+/.exec(p.s.slice(p.i));
  if (!m) err(p.n, 'expected a key');
  p.i += m[0].length;
  return m[0];
}
function parseString(p, err) {
  const q = p.s[p.i];
  p.i++;
  let out = '';
  while (p.i < p.s.length) {
    const c = p.s[p.i];
    if (c === q) { p.i++; return out; }
    if (q === '"' && c === '\\') {
      const e = p.s[p.i + 1];
      const map = { '"': '"', '\\': '\\', n: '\n', t: '\t', r: '\r' };
      if (e in map) { out += map[e]; p.i += 2; continue; }
      if (e === 'u' && /^[0-9a-fA-F]{4}$/.test(p.s.slice(p.i + 2, p.i + 6))) {
        out += String.fromCharCode(parseInt(p.s.slice(p.i + 2, p.i + 6), 16));
        p.i += 6;
        continue;
      }
      err(p.n, `bad escape \\${e}`);
    }
    out += c;
    p.i++;
  }
  err(p.n, 'unterminated string (strings are one line)');
}
function parseValue(p, err) {
  const c = p.s[p.i];
  if (c === '"' || c === "'") return parseString(p, err);
  if (c === '[') {
    p.i++;
    const a = [];
    for (;;) {
      skipWs(p);
      if (p.s[p.i] === ']') { p.i++; return a; }
      a.push(parseValue(p, err));
      skipWs(p);
      if (p.s[p.i] === ',') { p.i++; continue; }
      if (p.s[p.i] === ']') { p.i++; return a; }
      err(p.n, 'expected , or ] (arrays are one line)');
    }
  }
  if (c === '{') {
    p.i++;
    const t = {};
    for (;;) {
      skipWs(p);
      if (p.s[p.i] === '}') { p.i++; return t; }
      const k = parseKey(p, err);
      skipWs(p);
      if (p.s[p.i] !== '=') err(p.n, 'expected = in inline table');
      p.i++;
      skipWs(p);
      if (Object.prototype.hasOwnProperty.call(t, k)) err(p.n, `duplicate key ${k}`);
      t[k] = parseValue(p, err);
      skipWs(p);
      if (p.s[p.i] === ',') { p.i++; continue; }
      if (p.s[p.i] === '}') { p.i++; return t; }
      err(p.n, 'expected , or }');
    }
  }
  const m = /^(true|false|-?[0-9][0-9_]*)(?![A-Za-z0-9_.:-])/.exec(p.s.slice(p.i));
  if (!m) err(p.n, `unsupported value: ${p.s.slice(p.i)}`);
  p.i += m[0].length;
  if (m[1] === 'true') return true;
  if (m[1] === 'false') return false;
  return Number(m[1].replace(/_/g, ''));
}

// ---------------------------------------------------------------------------------------------
// Small helpers.

const ROLES = ['lez_chain', 'lez_host', 'lez_guests', 'eth_guests', 'eth_provers', 'research', 'none'];
const CRATES = { risc0_zkvm: 'risc0-zkvm', risc0_circuit_recursion: 'risc0-circuit-recursion', risc0_groth16: 'risc0-groth16', risc0_build: 'risc0-build' };
const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

const val = (x) => (x !== null && typeof x === 'object' && !Array.isArray(x) && 'v' in x ? x.v : x);
const srcOf = (x) => (x !== null && typeof x === 'object' && 'src' in x ? x.src : '');
const isHex = (s) => typeof s === 'string' && /^(0x)?[0-9a-fA-F]+$/.test(s);
const normHex = (s) => String(s).toLowerCase().replace(/^0x/, '');
const short = (s) => { const t = String(s); return t.length > 22 ? `${t.slice(0, 12)}…${t.slice(-6)}` : t; };

export function semverCmp(a, b) {
  const pa = String(a).split(/[.+-]/).map((x) => (/^\d+$/.test(x) ? Number(x) : x));
  const pb = String(b).split(/[.+-]/).map((x) => (/^\d+$/.test(x) ? Number(x) : x));
  for (let i = 0; i < 3; i++) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  return 0;
}

/** Whether a Cargo requirement (=X, X, ^X, ~X, *) admits version v. */
export function reqAdmits(req, v) {
  const r = String(req).trim();
  if (r === '*' || r === '') return true;
  let m = /^=\s*(\S+)$/.exec(r);
  if (m) return semverCmp(m[1], v) === 0;
  m = /^~\s*(\S+)$/.exec(r);
  if (m) {
    const [ma, mi] = m[1].split('.');
    const [va, vi] = String(v).split('.');
    return ma === va && (mi === undefined || mi === vi) && semverCmp(v, m[1]) >= 0;
  }
  m = /^\^?\s*(\S+)$/.exec(r);
  if (!m) return false;
  const base = m[1].split('.');
  const vv = String(v).split('.');
  if (semverCmp(v, m[1]) < 0) return false;
  const lead = base.findIndex((x) => x !== '0');
  const upto = lead === -1 ? base.length - 1 : lead;
  for (let i = 0; i <= upto && i < base.length; i++) if (base[i] !== vv[i]) return false;
  return true;
}

export function base58ToHex(s) {
  let n = 0n;
  for (const c of s) {
    const i = B58.indexOf(c);
    if (i < 0) throw new Error(`not base58: ${s}`);
    n = n * 58n + BigInt(i);
  }
  return n.toString(16).padStart(64, '0');
}

/** A value shown in a doc matches the declared one: equal, or an abbreviation (prefix…suffix). */
export function valuesMatch(shown, want) {
  const a = String(shown).trim();
  const b = String(want).trim();
  if (a === b) return true;
  if (isHex(a) && isHex(b)) return normHex(a) === normHex(b);
  const ab = /^(0x)?([0-9a-fA-F]+)(?:…|\.\.\.)([0-9a-fA-F]+)$/.exec(a);
  if (ab && isHex(b)) {
    const full = normHex(b);
    return full.startsWith(ab[2].toLowerCase()) && full.endsWith(ab[3].toLowerCase());
  }
  return false;
}

const globMatch = (pat, s) => new RegExp(`^${String(pat).split('*').map((x) => x.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*')}$`).test(String(s ?? ''));

export function parseLock(text) {
  const pkgs = [];
  for (const block of text.split(/\n(?=\[\[package\]\])/)) {
    const name = /^name = "([^"]+)"/m.exec(block);
    const version = /^version = "([^"]+)"/m.exec(block);
    if (!name || !version) continue;
    const source = /^source = "([^"]+)"/m.exec(block);
    pkgs.push({ name: name[1], version: version[1], source: source ? source[1] : '' });
  }
  return pkgs;
}

// ---------------------------------------------------------------------------------------------
// The checker.

export async function runCheck(opts = {}) {
  const root = path.resolve(opts.root || path.join(path.dirname(fileURLToPath(import.meta.url)), '..'));
  const vfile = opts.versions || path.join(root, 'VERSIONS.toml');
  const V = parseToml(fs.readFileSync(vfile, 'utf8'));
  const out = [];
  const accepted = V.accepted || [];

  const add = (level, check, msg, scope = {}) => {
    if (level === 'FAIL') {
      const a = accepted.find((e) => e.check === check
        && (e.id === undefined || e.id === scope.id)
        && ['deployment', 'role', 'workspace', 'file'].every((k) => e[k] === undefined || (scope[k] !== undefined && globMatch(e[k], scope[k]))));
      if (a) { level = 'WARN'; scope = { ...scope, accepted: a.reason }; }
    }
    out.push({ level, check, msg, ...scope });
  };
  const rel = (p) => path.relative(root, p) || '.';
  const read = (p) => fs.readFileSync(path.join(root, p), 'utf8');
  const exists = (p) => fs.existsSync(path.join(root, p));

  // --- the file's own shape
  const deps = V.deployments || {};
  // Workspaces are always checked against builds_for (what this tree builds); --deployment limits
  // the deployment records, registries, pins and on-chain reads to one deployment.
  const target = V.builds_for;
  if (!deps[target]) throw new Error(`builds_for = "${target}" is not a deployment`);
  if (opts.deployment && !deps[opts.deployment]) throw new Error(`unknown deployment ${opts.deployment} (VERSIONS.toml has: ${Object.keys(deps).join(', ')})`);
  if (!deps[V.live]) add('FAIL', 'schema', `live = "${V.live}" is not a deployment`);
  const only = opts.deployment ? [opts.deployment] : Object.keys(deps);
  for (const [name, d] of Object.entries(deps)) {
    if (!V.lez?.[d.lez]) add('FAIL', 'schema', `${name}: lez = "${d.lez}" is not under [lez]`);
    for (const r of Object.keys(d.roles || {})) if (!ROLES.includes(r)) add('FAIL', 'schema', `${name}: unknown role ${r}`);
  }
  const T = deps[target];
  const lezT = V.lez?.[T.lez] || {};
  const recursionRow = (ver) => (V.recursion || []).find((r) => r.version === ver);
  const proverRow = (z, rc, g) => (V.prover || []).find((p) => p.risc0_zkvm === z && p.risc0_circuit_recursion.includes(rc) && p.risc0_groth16.includes(g));

  // --- known-bad crate versions in a set of versions
  const knownBadCrates = (versions, role) => {
    const hits = {};
    for (const kb of V.known_bad || []) {
      if (kb.kind !== 'crate' || (kb.roles && !kb.roles.includes(role))) continue;
      const have = versions[kb.crate];
      if (have && semverCmp(have, kb.below) < 0) (hits[kb.id] ||= []).push(`${kb.crate} ${have} < ${kb.below}`);
    }
    return hits;
  };

  // --- rules on one set of role versions (a workspace's lock, or a deployment's record)
  const roleRules = (where, dname, d, role, versions, scope) => {
    const tag = `${where} [${role}]`;
    if (role === 'lez_host') {
      const chain = d.roles?.lez_chain;
      const mine = recursionRow(versions['risc0-circuit-recursion']);
      const theirs = chain && recursionRow(val(chain.risc0_circuit_recursion));
      if (!versions['risc0-circuit-recursion']) {
        // no recursion crate in the record: nothing to compare
      } else if (!mine || !theirs) {
        add('FAIL', 'control-root', `${tag}: risc0-circuit-recursion ${versions['risc0-circuit-recursion']} or ${dname}'s lez_chain recursion has no [[recursion]] row`, scope);
      } else if (mine.control_root !== theirs.control_root) {
        add('FAIL', 'control-root', `${tag}: risc0-circuit-recursion ${mine.version} (control root ${short(mine.control_root)}) differs from the LEZ chain's ${theirs.version} (${short(theirs.control_root)}): LEZ is expected to refuse the private-claim receipts this host makes (control ids differ; not tested)`, scope);
      } else add('OK', 'control-root', `${tag}: control root = LEZ chain's`, scope);
    }
    if (role === 'eth_provers') {
      const z = versions['risc0-zkvm'];
      const rc = versions['risc0-circuit-recursion'];
      const g = versions['risc0-groth16'];
      const row = proverRow(z, rc, g);
      const ver = d.verifier || {};
      if (!row) {
        add('FAIL', 'selector', `${tag}: risc0-zkvm ${z} / risc0-circuit-recursion ${rc} / risc0-groth16 ${g} is no declared [[prover]] combination, so its Groth16 selector is unknown`, scope);
      } else if (ver.kind !== 'risc0-groth16') {
        add('SKIP', 'selector', `${tag}: ${dname}'s verifier is ${ver.kind || 'not declared'}, no RISC Zero selector to match (prover gives ${row.selector})`, scope);
      } else if (normHex(row.selector) !== normHex(val(ver.selector))) {
        add('FAIL', 'selector', `${tag}: these versions give Groth16 selector ${row.selector}, but ${dname}'s verifier ${short(val(ver.address))} (${val(ver.version)}) has ${val(ver.selector)}: the vault would refuse every proof`, scope);
      } else add('OK', 'selector', `${tag}: selector ${row.selector} = ${dname}'s verifier`, scope);
    }
    for (const [id, list] of Object.entries(knownBadCrates(versions, role))) {
      add('FAIL', 'known-bad', `${tag}: ${list.join(', ')} (${id})`, { ...scope, id });
    }
  };

  // ============================================================ workspaces (against the target)
  const skipDirs = new Set(['vault/lib', ...((V.walk && V.walk.skip) || [])]);
  const found = { locks: [], tomls: [] };
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name);
      const r = rel(p);
      if (e.isDirectory()) {
        if (e.name.startsWith('.') || e.name.startsWith('target') || e.name === 'node_modules' || skipDirs.has(r)) continue;
        walk(p);
      } else if (e.name === 'Cargo.lock') found.locks.push(path.dirname(r));
      else if (e.name === 'Cargo.toml') found.tomls.push(r);
    }
  };
  walk(root);
  const wss = V.workspace || [];
  const wsByPath = new Map(wss.map((w) => [w.path, w]));
  for (const l of found.locks) if (!wsByPath.has(l)) add('FAIL', 'coverage', `${l}/Cargo.lock is in no [[workspace]] of VERSIONS.toml: give it a role`);
  const ownerOf = (tomlPath) => {
    const dir = path.dirname(tomlPath);
    let best = null;
    for (const w of wss) if ((dir === w.path || dir.startsWith(`${w.path}/`)) && (!best || w.path.length > best.path.length)) best = w;
    return best;
  };
  const tomlsByWs = new Map();
  for (const t of found.tomls) {
    const w = ownerOf(t);
    if (!w) { add('FAIL', 'coverage', `${t} belongs to no [[workspace]] of VERSIONS.toml`); continue; }
    if (!tomlsByWs.has(w.path)) tomlsByWs.set(w.path, []);
    tomlsByWs.get(w.path).push(t);
  }

  /** A workspace role's declared versions in the target; `image` overrides risc0_zkvm. */
  const declFor = (w, role) => {
    const decl = T.roles?.[role];
    const img = w.image ? T[w.image] : undefined;
    if (decl && img && typeof img === 'object' && img.risc0_zkvm) return { ...decl, risc0_zkvm: { v: img.risc0_zkvm, src: `${target}.${w.image}` } };
    return decl;
  };

  for (const w of wss) {
    const scopeW = { workspace: w.path, deployment: target };
    if (!exists(w.path)) {
      add(w.private ? 'SKIP' : 'FAIL', 'coverage', `${w.path}: declared workspace is missing${w.private ? ' (private, not in this copy)' : ''}`, scopeW);
      continue;
    }
    if (!exists(`${w.path}/Cargo.lock`)) { add('FAIL', 'coverage', `${w.path}: declared workspace has no Cargo.lock`, scopeW); continue; }
    for (const r of w.roles) if (!ROLES.includes(r) || r === 'lez_chain') add('FAIL', 'schema', `${w.path}: role ${r} is not a workspace role`, scopeW);
    const pkgs = parseLock(read(`${w.path}/Cargo.lock`));
    const versions = {};
    for (const c of Object.values(CRATES)) {
      const vs = [...new Set(pkgs.filter((p) => p.name === c).map((p) => p.version))];
      if (vs.length > 1) add('FAIL', 'lock', `${w.path}/Cargo.lock: two versions of ${c} (${vs.join(', ')})`, scopeW);
      if (vs.length) versions[c] = vs.sort(semverCmp)[0];
    }
    // LEZ in the lock
    const lezSrc = [...new Set(pkgs.map((p) => p.source).filter((s) => s.includes('logos-execution-zone')))];
    for (const s of lezSrc) {
      const m = /\?tag=([^#]+)#([0-9a-f]{40})$/.exec(s);
      if (!m || m[1] !== val(lezT.tag) || m[2] !== val(lezT.commit)) add('FAIL', 'lez', `${w.path}/Cargo.lock: LEZ source ${s.replace(/^git\+https:\/\/github\.com\//, '')} is not tag ${val(lezT.tag)} at ${String(val(lezT.commit)).slice(0, 8)}`, scopeW);
    }
    if (lezSrc.length && !out.some((f) => f.check === 'lez' && f.workspace === w.path && f.level === 'FAIL')) add('OK', 'lez', `${w.path}/Cargo.lock: LEZ ${val(lezT.tag)} (${String(val(lezT.commit)).slice(0, 8)})`, scopeW);

    for (const role of w.roles) {
      const scope = { ...scopeW, role };
      if (role === 'none') {
        if (Object.keys(versions).length) add('FAIL', 'lock', `${w.path}: role none, but the lock has ${Object.entries(versions).map(([k, v]) => `${k} ${v}`).join(', ')}`, scope);
        continue;
      }
      const decl = declFor(w, role);
      if (!decl) { add('FAIL', 'schema', `${w.path}: ${target} declares no versions for role ${role}`, scope); continue; }
      if (w.image && !(T[w.image] && typeof T[w.image] === 'object')) add('FAIL', 'schema', `${w.path}: image = "${w.image}" is not an image of ${target}`, scope);
      for (const [k, c] of Object.entries(CRATES)) {
        if (decl[k] === undefined) continue;
        const want = val(decl[k]);
        if (versions[c] === undefined) {
          if (k === 'risc0_zkvm') add('FAIL', 'lock', `${w.path} [${role}]: the lock has no ${c}, ${target} declares ${want}`, scope);
          continue;
        }
        if (versions[c] !== want) add('FAIL', 'lock', `${w.path} [${role}]: ${c} ${versions[c]} in Cargo.lock, ${target} declares ${want} (${srcOf(decl[k])})`, scope);
        else add('OK', 'lock', `${w.path} [${role}]: ${c} ${want}`, scope);
      }
      roleRules(w.path, target, T, role, versions, scope);
    }

    // Cargo.toml requirements and LEZ tags, one line per workspace and issue
    const reqBad = [];
    const lezBad = [];
    for (const t of tomlsByWs.get(w.path) || []) {
      const text = read(t);
      for (const line of text.split('\n')) {
        if (/^\s*#/.test(line)) continue;
        const m = /^\s*(risc0-zkvm|risc0-build|risc0-groth16|risc0-circuit-recursion)\s*=\s*(?:"([^"]+)"|\{[^}]*\bversion\s*=\s*"([^"]+)")/.exec(line);
        if (m) {
          const crate = m[1];
          const req = m[2] || m[3];
          const key = Object.keys(CRATES).find((k) => CRATES[k] === crate);
          for (const role of w.roles) {
            const want = val(declFor(w, role)?.[key]);
            if (want && !reqAdmits(req, want)) reqBad.push(`${crate} "${req}" in ${t} does not admit ${want} [${role}]`);
          }
        }
        if (line.includes('logos-execution-zone')) {
          const tag = /\btag\s*=\s*"([^"]+)"/.exec(line);
          if (!tag || tag[1] !== val(lezT.tag) || /\b(rev|branch)\s*=/.test(line)) lezBad.push(`${t}: ${line.trim().slice(0, 90)}`);
        }
      }
    }
    if (reqBad.length) add('FAIL', 'toml', `${w.path}: ${reqBad.length} Cargo.toml requirement(s) refuse ${target}'s version: ${reqBad.slice(0, 3).join('; ')}${reqBad.length > 3 ? '; …' : ''}`, scopeW);
    if (lezBad.length) add('FAIL', 'lez', `${w.path}: LEZ dependency not on tag ${val(lezT.tag)}: ${lezBad.join('; ')}`, scopeW);
  }

  // ============================================================ deployment records
  for (const dname of only) {
    const d = deps[dname];
    const scopeD = { deployment: dname };
    for (const [role, decl] of Object.entries(d.roles || {})) {
      const versions = {};
      for (const [k, c] of Object.entries(CRATES)) if (decl[k] !== undefined) versions[c] = val(decl[k]);
      roleRules(`VERSIONS ${dname}`, dname, d, role, versions, { ...scopeD, role });
    }
    const ver = d.verifier || {};
    if (ver.kind === 'risc0-groth16') {
      for (const kb of V.known_bad || []) {
        if (kb.kind === 'verifier' && val(ver.version) && semverCmp(val(ver.version), kb.below) < 0) {
          add('FAIL', 'known-bad', `VERSIONS ${dname}: verifier ${short(val(ver.address))} is ${val(ver.version)} < ${kb.below} (${kb.id})`, { ...scopeD, id: kb.id });
          for (const pin of V.pin || []) {
            if (pin.deploy_script === dname) add('FAIL', 'script-known-bad', `${pin.file} deploys against ${dname}'s verifier ${val(ver.version)} (${kb.id})`, { file: pin.file, deployment: dname, id: kb.id });
          }
        }
      }
      const rr = (V.recursion || []).find((r) => r.control_root === val(ver.control_root));
      const pr = (V.prover || []).find((p) => normHex(p.selector) === normHex(val(ver.selector)));
      if (!rr) add('FAIL', 'verifier', `VERSIONS ${dname}: verifier control root ${short(val(ver.control_root))} is no [[recursion]] row's`, scopeD);
      if (!pr) add('FAIL', 'verifier', `VERSIONS ${dname}: verifier selector ${val(ver.selector)} is no [[prover]] row's`, scopeD);
      else if (rr && !pr.risc0_circuit_recursion.includes(rr.version) && !pr.risc0_circuit_recursion.some((v) => recursionRow(v)?.control_root === rr.control_root)) add('FAIL', 'verifier', `VERSIONS ${dname}: verifier selector ${pr.selector} and control root ${short(rr.control_root)} come from different recursion versions`, scopeD);
      if (rr && pr) add('OK', 'verifier', `VERSIONS ${dname}: verifier ${val(ver.version)} selector ${pr.selector}, control root of recursion ${rr.version}`, scopeD);
    }
    const g2 = val(d.genesis_g2_state);
    const named = opts.deployment === dname; // step 0 of a deploy for this deployment
    if (d.status === 'deployed' && (!g2 || /^(0x)?0+$/.test(g2))) add('FAIL', 'g2-zero', `VERSIONS ${dname}: the vault's genesis g2State is zero: no G2-lite update can start from it, so it can never release`, scopeD);
    else if (!g2) add(named ? 'FAIL' : 'SKIP', 'g2-zero', `VERSIONS ${dname}: no genesis G2 state yet (${d.status}); it must be non-zero, from outbound_host genesis at the tip before the bridge's Initialize${named ? ': declare it before the deploy' : ''}`, scopeD);
    else if (/^(0x)?0+$/.test(g2)) add('FAIL', 'g2-zero', `VERSIONS ${dname}: genesis G2 state is zero`, scopeD);
    else add('OK', 'g2-zero', `VERSIONS ${dname}: genesis G2 state non-zero`, scopeD);
    if (named && d.status !== 'deployed') {
      const missing = ['update_image', 'release_image'].filter((k) => !val(d[k]) || (d[k].elf_sha256 !== undefined && !d[k].elf_sha256));
      if (missing.length) add('FAIL', 'schema', `VERSIONS ${dname}: ${missing.join(', ')} not declared (image id and ELF sha256): declare them before the deploy`, scopeD);
    }
    for (const s of d.lez_side || []) {
      const pid = val(d.program_id);
      if (pid && val(s.bridge_program)) {
        const h = base58ToHex(val(s.bridge_program));
        if (h !== normHex(pid)) add('FAIL', 'registry', `VERSIONS ${dname}: bridge ${short(val(s.bridge_program))} is ${short(h)}, not program_id ${short(pid)}`, scopeD);
        else add('OK', 'registry', `VERSIONS ${dname}/${s.name}: bridge program = program_id`, scopeD);
      }
    }
  }

  // ============================================================ registries
  const regJson = 'modules/lez_bridge/rust-lib/deployments.json';
  const regJs = 'web/deposit/src/lib/deployments.js';
  const sides = [];
  for (const [dname, d] of Object.entries(deps)) for (const s of d.lez_side || []) sides.push({ dname, d, s });
  if (exists(regJson)) {
    const j = JSON.parse(read(regJson));
    const live = deps[V.live];
    if (!(live?.lez_side || []).some((s) => s.module_entry === j.default)) add('FAIL', 'registry', `${regJson}: default "${j.default}" is not the live deployment ${V.live}'s entry`);
    for (const e of j.deployments || []) {
      const hit = sides.find((x) => x.s.module_entry === e.name);
      if (!hit) { add('FAIL', 'registry', `${regJson}: entry ${e.name} is no lez_side.module_entry in VERSIONS.toml`); continue; }
      if (!only.includes(hit.dname)) continue;
      const { d, s, dname } = hit;
      const pairs = [['vault', d.vault], ['vault_create_block', d.vault_create_block], ['cfg_id', d.cfg_id], ['desk', d.desk], ['chain_id', d.chain_id], ['bridge_program', s.bridge_program], ['token_program', s.token_program]];
      const bad = [];
      for (const [k, want] of pairs) {
        if (k === 'desk' && s.name !== 'public') continue; // the seq2 side has no desk
        if (!valuesMatch(String(e[k] ?? ''), String(val(want) ?? ''))) bad.push(`${k} ${e[k]} ≠ ${val(want)}`);
      }
      if (bad.length) add('FAIL', 'registry', `${regJson} ${e.name}: ${bad.join('; ')}`, { deployment: dname });
      else add('OK', 'registry', `${regJson} ${e.name} = VERSIONS ${dname}/${s.name}`, { deployment: dname });
    }
  } else add('FAIL', 'registry', `${regJson} is missing`);
  if (exists(regJs)) {
    const text = read(regJs);
    const ld = /const LIVE_DEFAULT = '([^']+)'/.exec(text);
    if (!ld || ld[1] !== deps[V.live]?.web_id) add('FAIL', 'registry', `${regJs}: LIVE_DEFAULT ${ld ? ld[1] : '(not found)'} is not ${V.live}'s web_id ${deps[V.live]?.web_id}`);
    const blocks = [...text.matchAll(/^const (SEPOLIA_[A-Z0-9_]+) = \{([\s\S]*?)^\};/gm)];
    if (!blocks.length) add('FAIL', 'registry', `${regJs}: no deployment objects found (the checker's pattern needs updating)`);
    for (const b of blocks) {
      const field = (k) => { const m = new RegExp(`^\\s*${k}: '?([^',\\n]+)'?,`, 'm').exec(b[2]); return m ? m[1] : undefined; };
      const id = field('id');
      const dname = Object.keys(deps).find((n) => deps[n].web_id === id);
      if (!dname) { add('FAIL', 'registry', `${regJs}: ${b[1]} (id ${id}) is no deployment's web_id in VERSIONS.toml`); continue; }
      if (!only.includes(dname)) continue;
      const d = deps[dname];
      const bad = [];
      for (const [k, want] of [['vault', d.vault], ['deployBlock', d.vault_create_block], ['cfgId', d.cfg_id], ['desk', d.desk], ['chainId', d.chain_id]]) {
        const have = field(k);
        if (!valuesMatch(String(have ?? ''), String(val(want) ?? ''))) bad.push(`${k} ${have} ≠ ${val(want)}`);
      }
      if (bad.length) add('FAIL', 'registry', `${regJs} ${id}: ${bad.join('; ')}`, { deployment: dname });
      else add('OK', 'registry', `${regJs} ${id} = VERSIONS ${dname}`, { deployment: dname });
    }
  } else add('FAIL', 'registry', `${regJs} is missing`);

  // ============================================================ pins
  const resolve = (p) => {
    const parts = p.split('.');
    let x = V;
    for (let i = 0; i < parts.length; i++) {
      const k = parts[i];
      if (Array.isArray(x)) x = x.find((e) => e.name === k);
      else x = x?.[k];
      if (x === undefined) return undefined;
    }
    return val(x);
  };
  for (const pin of V.pin || []) {
    const dm = /^deployments\.([^.]+)\./.exec(pin.value);
    if (dm && !only.includes(dm[1])) continue;
    const scope = { file: pin.file, deployment: dm ? dm[1] : undefined };
    if (!exists(pin.file)) { add(pin.private ? 'SKIP' : 'FAIL', 'pin', `${pin.file}: missing${pin.private ? ' (private, not in this copy)' : ''}`, scope); continue; }
    const want = resolve(pin.value);
    if (want === undefined || want === '') { add('FAIL', 'pin', `${pin.file}: VERSIONS has no value at ${pin.value}`, scope); continue; }
    let text = read(pin.file);
    let shown = [];
    if (pin.json) {
      let x = JSON.parse(text);
      for (const k of pin.json.split('.')) x = x?.[k];
      if (x !== undefined) shown = [String(x)];
    } else {
      if (pin.section) {
        const i = text.indexOf(`\n${pin.section}`);
        if (i < 0) { add('FAIL', 'pin', `${pin.file}: section "${pin.section}" not found`, scope); continue; }
        const j = text.indexOf('\n## ', i + 1 + pin.section.length);
        text = text.slice(i, j < 0 ? undefined : j);
      }
      shown = [...text.matchAll(new RegExp(pin.re, 'gm'))].map((m) => m[1]);
    }
    if (!shown.length) { add('FAIL', 'pin', `${pin.file}: no match for ${pin.json ? `json ${pin.json}` : `/${pin.re}/`} (the file changed: update it or the pin)`, scope); continue; }
    const bad = shown.filter((s) => !valuesMatch(s, want));
    if (bad.length) add('FAIL', 'pin', `${pin.file}: ${[...new Set(bad)].join(', ')} where VERSIONS ${pin.value} is ${want}`, scope);
    else add('OK', 'pin', `${pin.file}: ${pin.value} = ${short(want)}`, scope);
  }

  // ============================================================ on chain
  if (opts.onchain) await onchainChecks({ V, deps, only, add, rpc: opts.rpc || 'https://sepolia.gateway.tenderly.co,https://ethereum-sepolia-rpc.publicnode.com', fetchImpl: opts.fetchImpl || globalThis.fetch });

  return { findings: out, target: opts.deployment ? `${opts.deployment}, tree builds for ${target}` : target };
}

// Function selectors, from `cast sig` (Foundry), 2026-11-15.
const SIG = {
  VERSION: '0xffa1ad74', SELECTOR: '0x053c238d', CONTROL_ROOT_0: '0x9181e4b1', CONTROL_ROOT_1: '0x8989fa2e',
  BN254_CONTROL_ID: '0x258038e2', KIND: '0xc872da3c', updateImageId: '0x522d9a28', releaseImageId: '0x7b4d5f9a',
  g2State: '0xfd64db0a', cfgId: '0xed9305e3', verifier: '0x2b7ac3f3', programId: '0x98812da8',
};

const revBytes = (h) => normHex(h).match(/../g).reverse().join('');
/** The crate's control root from the verifier's CONTROL_ROOT_0/1 (each a byte-reversed half). */
export const controlRootFromChain = (cr0, cr1) => revBytes(normHex(cr0).slice(0, 32)) + revBytes(normHex(cr1).slice(0, 32));
const abiString = (h) => {
  const b = Buffer.from(normHex(h), 'hex');
  if (b.length < 64) return '';
  const len = Number(BigInt(`0x${b.subarray(32, 64).toString('hex')}`));
  return b.subarray(64, 64 + len).toString('utf8');
};

async function onchainChecks({ V, deps, only, add, rpc, fetchImpl }) {
  let id = 0;
  const urls = String(rpc).split(',').map((u) => u.trim()).filter(Boolean);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // Each endpoint in order; a rate limit (HTTP 429 or a "rate limit" error) is retried after 1, 2
  // and 4 s before the next endpoint. Reads only: eth_getCode and eth_call at "latest".
  const call = async (method, params) => {
    let last;
    for (const url of urls) {
      for (let attempt = 0; attempt < 4; attempt++) {
        try {
          const r = await fetchImpl(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ jsonrpc: '2.0', id: ++id, method, params }) });
          const j = r.status === 429 ? { error: { message: 'HTTP 429' } } : await r.json();
          if (!j.error) return j.result;
          last = new Error(`${method} via ${url}: ${j.error.message || JSON.stringify(j.error)}`);
          if (!/rate|429|limit/i.test(last.message)) throw last;
        } catch (e) {
          last = e;
          if (!/rate|429|limit|fetch failed|ECONN|ETIMEDOUT/i.test(e.message)) throw e;
        }
        if (attempt < 3) await sleep(1000 * 2 ** attempt);
      }
    }
    throw last;
  };
  const read = (to, fn) => call('eth_call', [{ to, data: SIG[fn] }, 'latest']);
  for (const dname of only) {
    const d = deps[dname];
    const scope = { deployment: dname };
    const ver = d.verifier || {};
    const vaddr = val(ver.address);
    try {
      if (vaddr) {
        const code = await call('eth_getCode', [vaddr, 'latest']);
        if (!code || code === '0x') add('FAIL', 'onchain', `${dname}: no contract at verifier ${vaddr}`, scope);
        else if (ver.kind === 'risc0-groth16') {
          const version = abiString(await read(vaddr, 'VERSION'));
          const sel = `0x${normHex(await read(vaddr, 'SELECTOR')).slice(0, 8)}`;
          const cr = controlRootFromChain(await read(vaddr, 'CONTROL_ROOT_0'), await read(vaddr, 'CONTROL_ROOT_1'));
          const bn = revBytes(await read(vaddr, 'BN254_CONTROL_ID'));
          const bad = [];
          if (version !== val(ver.version)) bad.push(`VERSION ${version} ≠ ${val(ver.version)}`);
          if (sel !== normHex(val(ver.selector)).replace(/^/, '0x')) bad.push(`SELECTOR ${sel} ≠ ${val(ver.selector)}`);
          if (cr !== normHex(val(ver.control_root))) bad.push(`control root ${short(cr)} ≠ ${short(val(ver.control_root))}`);
          if (val(ver.bn254_control_id) && bn !== normHex(val(ver.bn254_control_id))) bad.push(`BN254 control id ${short(bn)} ≠ ${short(val(ver.bn254_control_id))}`);
          if (bad.length) add('FAIL', 'onchain', `${dname}: verifier ${vaddr}: ${bad.join('; ')}`, scope);
          else add('OK', 'onchain', `${dname}: verifier ${short(vaddr)} reads VERSION ${version}, SELECTOR ${sel}, control root ${short(cr)}`, scope);
        } else if (ver.kind === 'demo-attested') {
          const kind = abiString(await read(vaddr, 'KIND'));
          if (kind !== val(ver.kind_text)) add('FAIL', 'onchain', `${dname}: verifier ${vaddr} KIND() "${kind}" ≠ "${val(ver.kind_text)}"`, scope);
          else add('OK', 'onchain', `${dname}: verifier ${short(vaddr)} is demo-attested ("${kind.slice(0, 40)}…")`, scope);
        }
      }
      const vault = val(d.vault);
      if (!vault) { add('SKIP', 'onchain', `${dname}: no vault address in VERSIONS.toml (${d.status}); after the deploy, record it and run --onchain again`, scope); continue; }
      const code = await call('eth_getCode', [vault, 'latest']);
      if (!code || code === '0x') { add('FAIL', 'onchain', `${dname}: no contract at vault ${vault}`, scope); continue; }
      const got = {};
      for (const fn of ['updateImageId', 'releaseImageId', 'cfgId', 'programId', 'g2State', 'verifier']) got[fn] = normHex(await read(vault, fn));
      got.verifier = got.verifier.slice(24);
      const bad = [];
      const cmp = (fn, want) => { if (want && got[fn] !== normHex(want)) bad.push(`${fn}() ${short(got[fn])} ≠ ${short(want)}`); };
      cmp('updateImageId', val(d.update_image));
      cmp('releaseImageId', val(d.release_image));
      cmp('cfgId', val(d.cfg_id));
      cmp('programId', val(d.program_id));
      cmp('verifier', vaddr);
      if (bad.length) add('FAIL', 'onchain', `${dname}: vault ${vault}: ${bad.join('; ')}`, scope);
      else add('OK', 'onchain', `${dname}: vault ${short(vault)} pins the declared image ids, cfgId, programId and verifier`, scope);
      if (/^0+$/.test(got.g2State)) add('FAIL', 'g2-zero', `${dname}: vault ${short(vault)} g2State() reads zero on chain: it can take no update and never release`, scope);
      else if (val(d.genesis_g2_state) && got.g2State !== normHex(val(d.genesis_g2_state))) {
        if (Number(val(d.g2_updates) || 0) > 0) add('OK', 'onchain', `${dname}: g2State() ${short(got.g2State)} (moved on from the genesis state; VERSIONS records ${val(d.g2_updates)} accepted update(s))`, scope);
        else add('FAIL', 'g2-zero', `${dname}: vault ${short(vault)} g2State() ${short(got.g2State)} is not the declared genesis state ${short(val(d.genesis_g2_state))} and VERSIONS records no accepted update: the vault was deployed with another starting state`, scope);
      }
      else add('OK', 'onchain', `${dname}: g2State() = the declared genesis state`, scope);
    } catch (e) {
      add('FAIL', 'onchain', `${dname}: on-chain read failed: ${e.message}`, scope);
    }
  }
}

// ---------------------------------------------------------------------------------------------

export function format({ findings, target }, verbose = false) {
  const order = { FAIL: 0, WARN: 1, SKIP: 2, OK: 3 };
  const lines = [];
  // FAIL and OK one per line; accepted WARNs grouped by their reason; SKIPs grouped by message.
  const groups = new Map();
  for (const f of [...findings].sort((a, b) => order[a.level] - order[b.level])) {
    if (f.level === 'OK' && !verbose) continue;
    if (f.level === 'WARN' && f.accepted) {
      const k = `WARN|${f.check}|${f.accepted}`;
      if (!groups.has(k)) { groups.set(k, { f, where: [] }); lines.push(k); }
      groups.get(k).where.push(f.msg);
    } else if (f.level === 'SKIP') {
      const k = `SKIP|${f.check}|${f.msg}`;
      if (!groups.has(k)) { groups.set(k, { f, where: [] }); lines.push(k); }
      groups.get(k).where.push(f.msg);
    } else lines.push(`${f.level.padEnd(4)} ${f.check}: ${f.msg}`);
  }
  for (let i = 0; i < lines.length; i++) {
    const g = groups.get(lines[i]);
    if (!g) continue;
    if (g.f.level === 'SKIP') lines[i] = `SKIP ${g.f.check}: ${g.f.msg}${g.where.length > 1 ? ` (×${g.where.length})` : ''}`;
    else lines[i] = `WARN ${g.f.check} (accepted: ${g.f.accepted})\n       ${g.where.join('\n       ')}`;
  }
  const n = (l) => findings.filter((f) => f.level === l).length;
  lines.push(`check-versions (target ${target}): ${n('FAIL')} FAIL, ${n('WARN')} WARN, ${n('SKIP')} SKIP, ${n('OK')} OK`);
  return lines.join('\n');
}

async function main(argv) {
  const opts = {};
  let verbose = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--onchain') opts.onchain = true;
    else if (a === '--verbose' || a === '-v') verbose = true;
    else if (['--root', '--versions', '--deployment', '--rpc'].includes(a) && argv[i + 1]) opts[a.slice(2)] = argv[++i];
    else if (a === '--help' || a === '-h') {
      const src = fs.readFileSync(fileURLToPath(import.meta.url), 'utf8').split('\n');
      console.log(src.slice(3, src.findIndex((l) => l.startsWith('// Exit:')) + 1).map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
      return 0;
    }
    else { console.error(`check-versions: unknown argument ${a} (see --help)`); return 2; }
  }
  let res;
  try {
    res = await runCheck(opts);
  } catch (e) {
    console.error(`check-versions: ${e.message}`);
    return 2;
  }
  console.log(format(res, verbose));
  return res.findings.some((f) => f.level === 'FAIL') ? 1 : 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then((c) => { process.exitCode = c; });
}
