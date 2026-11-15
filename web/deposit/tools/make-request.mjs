#!/usr/bin/env node
// Turn a deposit commitment (from `lez-bridge commitment`) into a deposit request: the
// logosdep1... string, a link with the request in the URL fragment, and a QR code.
//
//   node tools/make-request.mjs --commitment 0x... --token ETH --denom 10000000000000000
//   node tools/make-request.mjs --commitment 0x... --token dUSDC --denom 100000000 --qr
//   node tools/make-request.mjs --example --token ETH --denom 10000000000000000
//
// --denom is the denomination in the token's smallest unit, and MUST be the exact value
// given to `lez-bridge commitment --denom`: the commitment binds it, the vault does not
// check it, and a mismatch would lock the deposit for good. The tool refuses any value
// that is not one of the vault's denominations (read from --rpc if given, otherwise from
// the built-in deployment table, and says which).
//
// Options: --vault (default: the page's live deployment, Sepolia demo v3 since 2026-11-14), --chain-id (default 11155111), --token
// (ETH, a symbol of the deployment, or an address), --denom-index (optional; found from
// --denom), --rpc <url> (verify against the chain), --base <page url> (link prefix),
// --qr (print the link as a QR code), --json. Sends nothing; reads only with --rpc.
//
// --example makes a fresh random commitment instead. Nobody knows a secret for it, so a
// deposit made with it could NEVER be claimed: use it only to look at the page.

import { randomBytes } from 'node:crypto';
import { encodeRequest, requestUrl, normalizeRequest } from '../src/lib/request.js';
import { DEPLOYMENTS, findDeployment } from '../src/lib/deployments.js';
import { call, decode, ETH } from '../src/lib/vault.js';
import { httpProvider, rpc as makeRpc } from '../src/lib/providers.js';
import { formatUnits } from '../src/lib/units.js';
import { encodeQr, qrToTerminal } from '../src/lib/qr.js';

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith('--')) throw new Error(`unexpected argument ${a}`);
    const k = a.slice(2);
    if (['qr', 'json', 'example', 'help'].includes(k)) out[k] = true;
    else out[k] = argv[++i];
  }
  return out;
}

const die = (msg) => {
  console.error(`make-request: ${msg}`);
  process.exit(1);
};

const o = args(process.argv.slice(2));
if (o.help || (!o.commitment && !o.example)) {
  console.log(
    'usage: node tools/make-request.mjs (--commitment 0x<64 hex> | --example) --token ETH|dUSDC|0x.. --denom <base units>\n' +
      '       [--denom-index N] [--vault 0x..] [--chain-id N] [--rpc URL] [--base URL] [--qr] [--json]',
  );
  process.exit(o.help ? 0 : 1);
}

const chainId = BigInt(o['chain-id'] || 11155111);
const dep = o.vault ? findDeployment(chainId, o.vault) : DEPLOYMENTS.find((d) => BigInt(d.chainId) === chainId);
const vault = (o.vault || dep?.vault || die('no --vault given and no known deployment on this chain')).toLowerCase();
if (!dep) console.error(`warning: ${vault} on chain ${chainId} is not a known deployment; the page will show a loud warning.`);

let token = o.token || die('--token is required (ETH, a token symbol, or an address)');
if (/^eth$/i.test(token)) token = ETH;
else if (!/^0x[0-9a-fA-F]{40}$/.test(token)) {
  const hit = dep && Object.entries(dep.tokens).find(([, t]) => t.symbol.toLowerCase() === token.toLowerCase());
  if (!hit) die(`unknown token symbol ${token}; pass its address`);
  token = hit[0];
}
token = token.toLowerCase();
if (!o.denom || !/^[0-9]+$/.test(o.denom)) die('--denom <base units> is required: the same value given to `lez-bridge commitment --denom`');
const denom = BigInt(o.denom);

// the vault's denominations: from the chain if --rpc, else from the deployment table
let denoms;
let decimals;
let source;
if (o.rpc) {
  const r = makeRpc(httpProvider(o.rpc));
  const cid = await r.chainId();
  if (cid !== chainId) die(`--rpc is on chain ${cid}, not ${chainId}`);
  const cfg = decode.tokenConfig(await r.call(vault, call.tokenConfig(token)));
  if (!cfg.registered) die(`the vault does not accept token ${token}`);
  denoms = cfg.denoms;
  decimals = cfg.decimals;
  source = `read from ${o.rpc}`;
} else if (dep?.tokens[token]?.denoms) {
  denoms = dep.tokens[token].denoms.map(BigInt);
  decimals = dep.tokens[token].decimals;
  source = 'from the built-in deployment table (pass --rpc to check the chain)';
} else die('this token is not in the built-in table: pass --rpc to read its denominations from the chain');

const found = denoms.findIndex((d) => d === denom);
if (found === -1) die(`--denom ${denom} is not a denomination of this vault (${denoms.map((d) => formatUnits(d, decimals)).join(', ')}; ${source})`);
const denomIndex = o['denom-index'] === undefined ? found : Number(o['denom-index']);
if (denomIndex !== found) die(`--denom-index ${denomIndex} is ${formatUnits(denoms[denomIndex] ?? 0n, decimals)}, not ${formatUnits(denom, decimals)}`);

let commitment = o.commitment;
if (o.example) {
  commitment = '0x' + randomBytes(32).toString('hex');
  console.error('EXAMPLE ONLY: a random commitment nobody can claim. Do not deposit with this request.');
}
let req;
try {
  req = normalizeRequest({ chainId, vault, token, denomIndex, amount: denom, commitment });
} catch (e) {
  die(e.message);
}

const str = encodeRequest(req);
const base = o.base || 'http://127.0.0.1:8000/';
const url = requestUrl(base, req);
const sym = dep?.tokens[token]?.symbol || (token === ETH ? 'ETH' : token);

if (o.json) {
  console.log(JSON.stringify({ request: str, url, chainId: String(chainId), vault, token, denomIndex, amount: String(denom), commitment: req.commitment, example: !!o.example }, null, 2));
} else {
  console.log(`deposit   ${formatUnits(denom, decimals)} ${sym} (denomination ${denomIndex + 1} of ${denoms.length}, ${source})`);
  console.log(`vault     ${vault}${dep ? ` (${dep.label})` : ' (UNKNOWN)'}`);
  console.log(`request   ${str}`);
  console.log(`link      ${url}`);
}
if (o.qr) console.log('\n' + qrToTerminal(encodeQr(url, { ecl: 'M' })) + '\n');
