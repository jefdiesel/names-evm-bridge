// SPDX-License-Identifier: MIT OR Apache-2.0
// "Bridge to Ethereum" on this page: the withdraw-to code for the connected wallet, and the
// release a release code names (the checks, then the vault's `release` call). DOM-free, so the
// tests run it against the mock chain (test/send-back.test.mjs). Codes: ./withdraw.js.

import { selector } from './keccak.js';
import { encodeValue, words, asUint, asBool } from './abi.js';
import { decodeRevert, extractRevertData } from './vault.js';
import { DEPLOYMENTS } from './deployments.js';
import { encodeWithdrawTo, withdrawWords } from './withdraw.js';

export const RELEASE_SIG = 'release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)';
/** The release's gas on anvil in the v4 rehearsal: 139,391 (public-demo/V4-PLAN.md §3). */
export const RELEASE_GAS_ABOUT = 140_000n;

// The vault's and the verifier's refusals a release can meet (BridgeVault.sol, RiscZeroGroth16Verifier).
const RELEASE_ERRORS = [
  ['UnknownExitRoot()', 'The vault has not recorded this exit yet.'],
  ['ReleaseTooEarly()', 'The vault’s delay has not passed yet.'],
  ['NullifierSpent()', 'This send-back was released already.'],
  ['BadRecipient()', 'The recipient can’t receive a release.'],
  ['IsFrozen()', 'Releases are paused on this vault.'],
  ['VerificationFailed()', 'Ethereum’s verifier refused the proof in this release code.'],
  ['SelectorMismatch(bytes4,bytes4)', 'The proof is for another verifier version.'],
  ['ReleasesExceedBurns()', 'The vault counts more releases than burns.'],
  ['RelayFeeNotOnMenu()', 'This relay fee is not on the vault’s menu.'],
  ['ZeroRelayer()', 'A relay fee needs a relayer.'],
];
const BY_SEL = new Map(RELEASE_ERRORS.map(([sig, words]) => [selector(sig), words]));

export function releaseWhy(err) {
  const data = extractRevertData(err);
  if (data && BY_SEL.has(data.slice(0, 10).toLowerCase())) return BY_SEL.get(data.slice(0, 10).toLowerCase());
  return decodeRevert(data).message;
}

const strip = (h) => String(h).replace(/^0x/i, '').toLowerCase();

/** Call data of `release(seal, root, nullifier, token, denomIndex, recipient, relayer, relayFee)`. */
export function releaseCalldata(r) {
  const seal = strip(r.seal);
  const pad = seal + '0'.repeat((64 - (seal.length % 64)) % 64);
  return (
    selector(RELEASE_SIG) +
    encodeValue('uint256', 8n * 32n) +
    encodeValue('bytes32', r.root) +
    encodeValue('bytes32', r.nullifier) +
    encodeValue('address', r.token) +
    encodeValue('uint32', r.denomIndex) +
    encodeValue('address', r.recipient) +
    encodeValue('address', r.relayer) +
    encodeValue('uint256', r.relayFee) +
    encodeValue('uint256', BigInt(seal.length / 2)) +
    pad
  );
}

/** The listed deployment a code names, or null. */
export function deploymentOf(chainId, vault, list = DEPLOYMENTS) {
  return list.find((d) => BigInt(d.chainId) === BigInt(chainId) && d.vault.toLowerCase() === String(vault).toLowerCase()) || null;
}

/** The withdraw-to code for `recipient` on the live deployment, with its words. */
export async function withdrawFor(recipient, dep = DEPLOYMENTS[0], feeChoice = 0) {
  const w = { chainId: BigInt(dep.chainId), vault: dep.vault.toLowerCase(), recipient: recipient.toLowerCase(), feeChoice };
  return { code: encodeWithdrawTo(w), ...(await withdrawWords(w)), fields: w };
}

/**
 * Everything the page checks before it asks the wallet to send a release, read through `r`
 * (providers.js `rpc`): `{ ok, wait, done, dep, amount, denom, words_text, checks: [{key, ok, text}], problem }`.
 * `from` is the connected account (anyone may send a release).
 */
export async function releaseChecks(r, rel, { from, list = DEPLOYMENTS } = {}) {
  const checks = [];
  const out = { ok: false, wait: null, done: false, dep: null, checks, problem: null };
  const dep = deploymentOf(rel.chainId, rel.vault, list);
  if (!dep) {
    out.problem = 'This release code names a vault this page doesn’t list. Nothing was sent.';
    return out;
  }
  out.dep = dep;
  const vault = dep.vault;
  const c = async (sig, args) => words(await r.call(vault, selector(sig) + args.map(([t, v]) => encodeValue(t, v)).join('')));
  if (asBool((await c('releaseNullifierSpent(bytes32)', [['bytes32', rel.nullifier]]))[0])) {
    out.done = true;
    checks.push({ key: 'unspent', ok: false, text: 'Released already.' });
    return out;
  }
  const seen = asUint((await c('exitRootTime(bytes32)', [['bytes32', rel.root]]))[0]);
  if (seen === 0n) {
    out.problem = 'The vault has not recorded this exit. Check the release code with your Names Wallet.';
    return out;
  }
  checks.push({ key: 'recorded', ok: true, text: 'The vault recorded this exit.' });
  const cfg = await c('tokenConfig(address)', [['address', rel.token]]);
  const at = (i) => Number(asUint(cfg[i]) / 32n);
  const arr = (off) => Array.from({ length: Number(asUint(cfg[off])) }, (_, k) => asUint(cfg[off + 1 + k]));
  const denoms = arr(at(5));
  const wfees = arr(at(7));
  const denom = denoms[rel.denomIndex];
  if (denom == null) {
    out.problem = 'The vault has no such denomination. Nothing was sent.';
    return out;
  }
  out.denom = denom;
  out.amount = denom - (wfees[rel.denomIndex] || 0n) - BigInt(rel.relayFee);
  const fee = BigInt(rel.relayFee);
  let choice = fee === 0n ? 0 : null;
  if (choice === null) {
    const m = await r.call(vault, selector('relayFeeMenu(address,uint32)') + encodeValue('address', rel.token) + encodeValue('uint32', rel.denomIndex));
    const ws = words(m);
    const menu = Array.from({ length: Number(asUint(ws[1])) }, (_, k) => asUint(ws[2 + k]));
    const i = menu.findIndex((x) => x === fee);
    choice = i === -1 ? null : i + 1;
  }
  if (choice !== null) out.words_text = (await withdrawWords({ chainId: rel.chainId, vault: vault.toLowerCase(), recipient: rel.recipient, feeChoice: choice })).words_text;
  const releasable = asUint((await c('releasableAt(address,bytes32)', [['address', rel.token], ['bytes32', rel.root]]))[0]);
  const block = await r.block('latest');
  const now = BigInt(block.timestamp);
  if (now < releasable) {
    out.wait = { until: releasable, seconds: Number(releasable - now) };
    checks.push({ key: 'delay', ok: false, text: 'The vault’s one-hour delay has not passed yet.' });
    return out;
  }
  checks.push({ key: 'delay', ok: true, text: 'The vault’s delay has passed.' });
  try {
    await r.call(vault, releaseCalldata(rel), { from });
  } catch (e) {
    out.problem = `${releaseWhy(e)} Nothing was sent.`;
    return out;
  }
  checks.push({ key: 'simulated', ok: true, text: 'The release goes through in a simulation, proof included.' });
  out.ok = true;
  return out;
}

/** Sends the release from `from` and waits for its receipt. */
export async function sendRelease(r, rel, dep, from, { polls = 120, sleep = (ms) => new Promise((s) => setTimeout(s, ms)) } = {}) {
  const hash = await r.send({ from, to: dep.vault, data: releaseCalldata(rel) });
  for (let i = 0; i < polls; i++) {
    const rc = await r.receipt(hash);
    if (rc) return { hash, ok: rc.status === '0x1', block: BigInt(rc.blockNumber) };
    await sleep(3000);
  }
  return { hash, ok: null };
}
