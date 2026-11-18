// SPDX-License-Identifier: MIT OR Apache-2.0
// "Bridge to Ethereum" on the page (src/lib/send-back.js) against the mock chain: the
// withdraw-to code for a connected address, the release checks (recorded, delay, spent, the
// simulation with the proof), the release call and its words.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createMockChain, createMockWallet, ADDR } from '../dev/mock-chain.js';
import { rpc } from '../src/lib/providers.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';
import { decodeWithdrawTo, withdrawWords, encodeRelease, decodeRelease } from '../src/lib/withdraw.js';
import { withdrawFor, releaseChecks, sendRelease, releaseCalldata, RELEASE_SIG, PRIVATE_HINT, accountHistory, historyLine } from '../src/lib/send-back.js';
import { readFileSync as readSource } from 'node:fs';
import { selector } from '../src/lib/keccak.js';

const ROOT = '0x' + '5a'.repeat(32);
const NF = '0x' + '6b'.repeat(32);
const SEAL = '0xe71e6620' + '77'.repeat(256);

function setup(opts = {}) {
  const chain = createMockChain({ exitRoots: { [ROOT]: 0n }, ...opts });
  const now = chain.timeOf(chain.S.head);
  if (opts.recordedAgo !== undefined) chain.S.exitRoots.set(ROOT, now - BigInt(opts.recordedAgo));
  const wallet = createMockWallet(chain, { walletChainId: 11155111n });
  return { chain, r: rpc(wallet), now };
}

const rel = (o = {}) => decodeRelease(encodeRelease({ chainId: 11155111n, vault: DEPLOYMENTS[0].vault, root: ROOT, nullifier: NF, token: ADDR.eth, denomIndex: 0, recipient: ADDR.me, relayer: '0x' + '00'.repeat(20), relayFee: 0n, seal: SEAL, ...o }));

test('the withdraw-to code names the connected address on the live vault, with its words', async () => {
  const w = await withdrawFor('0x5A1E0000000000000000000000000000000C0DE5');
  const d = decodeWithdrawTo(w.code);
  assert.equal(d.recipient, ADDR.me);
  assert.equal(d.vault, DEPLOYMENTS[0].vault.toLowerCase());
  assert.equal(d.feeChoice, 0);
  assert.equal(w.words_text, (await withdrawWords(d)).words_text);
  assert.equal(w.code.length, 94);
});

test('a release waits for the delay, then passes every check and is sent once', async () => {
  const { chain, r } = setup({ recordedAgo: 600 });
  const early = await releaseChecks(r, rel(), { from: ADDR.me });
  assert.equal(early.ok, false);
  assert.ok(early.wait && early.wait.seconds > 2900 && early.wait.seconds <= 3000, String(early.wait && early.wait.seconds) + (early.problem || ""));
  chain.S.exitRoots.set(ROOT, chain.timeOf(chain.S.head) - 3700n);
  const c = await releaseChecks(r, rel(), { from: ADDR.me });
  assert.equal(c.ok, true, c.problem);
  assert.equal(c.amount, 10n ** 16n);
  assert.deepEqual(c.checks.map((x) => x.key), ['recorded', 'delay', 'simulated']);
  const expected = await withdrawWords({ chainId: 11155111n, vault: DEPLOYMENTS[0].vault.toLowerCase(), recipient: ADDR.me, feeChoice: 0 });
  assert.equal(c.words_text, expected.words_text, 'the same four words as the withdraw-to code');
  const sent = await sendRelease(r, rel(), c.dep, ADDR.me, { sleep: async () => {} });
  assert.equal(sent.ok, true);
  assert.deepEqual(chain.S.released, [{ nullifier: NF, recipient: ADDR.me }]);
  const again = await releaseChecks(r, rel(), { from: ADDR.me });
  assert.equal(again.done, true);
});

test('an unrecorded exit, a refused proof and an unlisted vault are refused before any prompt', async () => {
  const { chain, r } = setup({ recordedAgo: 7200 });
  const other = await releaseChecks(r, rel({ root: '0x' + '01'.repeat(32) }), { from: ADDR.me });
  assert.match(other.problem, /not recorded/);
  chain.S.verifyOk = false;
  const bad = await releaseChecks(r, rel(), { from: ADDR.me });
  assert.match(bad.problem, /verifier refused/);
  const unlisted = await releaseChecks(r, rel({ vault: '0x' + '12'.repeat(20) }), { from: ADDR.me });
  assert.match(unlisted.problem, /doesn’t list/);
});

test('the release call data is the vault ABI: eight head words, then the 260-byte seal', () => {
  const d = releaseCalldata(rel());
  assert.equal(d.slice(0, 10), selector(RELEASE_SIG));
  const body = d.slice(10);
  assert.equal(BigInt('0x' + body.slice(0, 64)), 256n);
  assert.equal(BigInt('0x' + body.slice(64 * 8, 64 * 9)), 260n);
  assert.equal(body.length, 64 * 9 + 2 * 288);
  assert.equal(body.slice(64 * 9, 64 * 9 + 8), 'e71e6620');
});

test('withdraw mode suggests a fresh account beside Connect wallet, without requiring it', () => {
  assert.ok(PRIVATE_HINT.startsWith('Want it private?'));
  assert.ok(!/must|required|need to/i.test(PRIVATE_HINT.split(':')[0]));
  const app = readSource(new URL('../src/app.js', import.meta.url), 'utf8');
  assert.match(app, /primary\('Connect wallet'[^\n]*PRIVATE_HINT/, 'the hint sits beside Connect wallet in withdraw mode');
});

test('the connected account’s past: nothing sent and nothing held, sent from, holding, not readable', async () => {
  const fake = (count, bal, fail) => ({
    request: async (m) => { if (fail) throw new Error('down'); assert.equal(m, 'eth_getTransactionCount'); return '0x' + count.toString(16); },
    balance: async () => { if (fail) throw new Error('down'); return bal; },
  });
  const eth = (wei) => `${wei} wei`;
  const zero = await accountHistory(fake(0, 0n), '0x' + '11'.repeat(20));
  assert.deepEqual(historyLine(zero, eth), { kind: 'quiet', text: 'Nothing has been sent from this account and it holds nothing.' });
  const sent = historyLine(await accountHistory(fake(3, 0n), '0x' + '11'.repeat(20)), eth);
  assert.equal(sent.kind, 'warn');
  assert.ok(sent.text.startsWith('This account has been used before (3 transactions sent). Anyone can see its history.'));
  assert.ok(sent.text.endsWith('Or carry on: it’s your choice.'));
  const holds = historyLine(await accountHistory(fake(0, 5n), '0x' + '11'.repeat(20)), eth);
  assert.ok(holds.text.includes('(holds 5 wei)'));
  await assert.rejects(accountHistory(fake(0, 0n, true), '0x' + '11'.repeat(20)));
  assert.deepEqual(historyLine(null), { kind: 'unknown', text: 'Couldn’t check this account’s history.' });
  for (const t of [historyLine(zero).text, sent.text]) assert.ok(!/\bunused\b|^New account/i.test(t));
});
