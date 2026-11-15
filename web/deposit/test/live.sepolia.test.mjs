// Read-only checks against the live Sepolia demo vault v2. Runs only with LIVE=1:
//   LIVE=1 node --test test/live.sepolia.test.mjs
// Sends no transaction. RPC: $SEPOLIA_RPC or https://ethereum-sepolia-rpc.publicnode.com
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { httpProvider, rpc as makeRpc, scanLogs } from '../src/lib/providers.js';
import { call, decode, decodeDepositLog, DEPOSIT_TOPIC, ETH } from '../src/lib/vault.js';
import { DEPLOYMENTS, EXAMPLE_COMMITMENTS } from '../src/lib/deployments.js';
import { createFlow } from '../src/lib/flow.js';
import { encodeRequest } from '../src/lib/request.js';

const LIVE = process.env.LIVE === '1';
const URL = process.env.SEPOLIA_RPC || 'https://ethereum-sepolia-rpc.publicnode.com';
const dep = DEPLOYMENTS.find((d) => d.id === 'sepolia-demo-v2');
const USDC = '0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e';
const r = makeRpc(httpProvider(URL));
const log = (...a) => console.log('  [live]', ...a);

// From vault/DEPLOYMENTS.md, v2 deposits 1-5.
const KNOWN = [
  { leaf: 0n, token: USDC, idx: 0, c: '0xa50e0f44940eca30735460fa9b657d6ab948fc422b29b1293be0f7e5d6b7ad2e' },
  { leaf: 1n, token: USDC, idx: 0, c: '0x188c6f3a5a409e9ac4a5c69fb55213593b4b1774eadca84ede91ec44e300bacd' },
  { leaf: 2n, token: USDC, idx: 1, c: '0x4b7b9d32f6b53f8cbd76261abafa450ce18e450e8ae974e86a61f1344893e514' },
  { leaf: 3n, token: USDC, idx: 0, c: '0x6cdd53cd6e42fe59609b0427aa6ef6fd1996a1b224dddb955211962f3fe0878d' },
  { leaf: 4n, token: ETH, idx: 0, c: '0xffb6f3360e0acd47eb2fb92546674c9762ef4a628364be8d922e4618e3f07e37' },
];

test('live: chain, code and cfgId', { skip: !LIVE }, async () => {
  assert.equal(await r.chainId(), 11155111n);
  const code = await r.code(dep.vault);
  assert.ok(code.length > 1000, 'vault has code');
  assert.equal(decode.bytes32(await r.call(dep.vault, call.cfgId())), dep.cfgId);
  log('cfgId', dep.cfgId, 'code bytes', (code.length - 2) / 2);
});

test('live: token configs match the allowlist', { skip: !LIVE }, async () => {
  for (const [addr, t] of Object.entries(dep.tokens)) {
    const cfg = decode.tokenConfig(await r.call(dep.vault, call.tokenConfig(addr)));
    assert.equal(cfg.registered, true);
    assert.equal(cfg.depositsFrozen, false);
    assert.equal(cfg.decimals, t.decimals);
    assert.deepEqual(cfg.denoms, t.denoms.map(BigInt));
    assert.deepEqual(cfg.depositFees, [0n, 0n, 0n]);
    log(t.symbol, 'denoms', cfg.denoms.join(','), 'fees', cfg.depositFees.join(','), 'releaseDelay', cfg.releaseDelay);
  }
  assert.equal(decode.bool(await r.call(dep.vault, call.globalDepositsFrozen())), false);
});

test('live: caps and headroom', { skip: !LIVE }, async () => {
  for (const addr of Object.keys(dep.tokens)) {
    const c = decode.capState(await r.call(dep.vault, call.capState(addr)));
    const w = decode.uint(await r.call(dep.vault, call.capWeight(addr)));
    assert.equal(c.window, 86400n);
    assert.ok(c.depUsed <= c.depositCap);
    log(dep.tokens[addr].symbol, 'cap', c.depositCap, 'used now', c.depUsed, 'weight', w);
  }
  const g = decode.capState(await r.call(dep.vault, call.globalCapState()));
  log('global cap', g.depositCap, 'used now', g.depUsed, 'window', g.window);
});

test('live: anonymity-set counters agree with the Deposit events', { skip: !LIVE }, async () => {
  const total = decode.uint(await r.call(dep.vault, call.depositCount()));
  assert.ok(total >= 5n, `depositCount ${total}`);
  const { logs, complete } = await scanLogs(r, { address: dep.vault, topics: [DEPOSIT_TOPIC] }, BigInt(dep.deployBlock), await r.blockNumber());
  assert.ok(complete);
  const events = logs.map(decodeDepositLog);
  assert.equal(BigInt(events.length), total, 'one event per leaf');
  for (const k of KNOWN) {
    const e = events.find((x) => x.leafIndex === k.leaf);
    assert.ok(e, `leaf ${k.leaf}`);
    assert.equal(e.token, k.token);
    assert.equal(e.denomIndex, k.idx);
    assert.equal(e.commitment, k.c);
    assert.equal(decode.bool(await r.call(dep.vault, call.commitmentUsed(k.c))), true);
  }
  const sets = {};
  for (const e of events) sets[`${e.token}:${e.denomIndex}`] = (sets[`${e.token}:${e.denomIndex}`] || 0n) + 1n;
  for (const [addr, t] of Object.entries(dep.tokens)) {
    for (let i = 0; i < t.denoms.length; i++) {
      const c = decode.counters(await r.call(dep.vault, call.counters(addr, i)));
      assert.equal(c.deposits, sets[`${addr}:${i}`] || 0n, `${t.symbol} #${i}`);
      log(`${t.symbol} denomination ${i + 1}: ${c.deposits} deposit(s), ${c.claims} claim(s) recorded`);
    }
  }
  log('depositCount', total, 'events', events.length);
});

test('live: example commitments are unused', { skip: !LIVE }, async () => {
  for (const c of EXAMPLE_COMMITMENTS) assert.equal(decode.bool(await r.call(dep.vault, call.commitmentUsed(c))), false);
});

test('live: a used commitment is refused by simulation with CommitmentUsed()', { skip: !LIVE }, async () => {
  const from = '0x37a46E7334Dd061E0C7E2298247EE9278d0d321F';
  await assert.rejects(r.call(dep.vault, call.depositETH(0, KNOWN[4].c), { from, value: 10n ** 16n }), (e) => String(e.data || '').startsWith('0x7b000717'));
});

test('live: the real flow, read-only, on an example request', { skip: !LIVE }, async () => {
  const flow = createFlow({ sleep: (ms) => new Promise((res) => setTimeout(res, Math.min(ms, 50))) });
  flow.load(encodeRequest({ chainId: 11155111n, vault: dep.vault, token: ETH, denomIndex: 0, amount: 10n ** 16n, commitment: [...EXAMPLE_COMMITMENTS][0] }));
  assert.equal(flow.state.example, true);
  flow.setReadProvider(httpProvider(URL));
  for (let i = 0; i < 200 && (flow.state.busy || flow.state.checks.find((c) => c.id === 'commitment').status === 'pending'); i++) await new Promise((res) => setTimeout(res, 100));
  const s = flow.state;
  for (const id of ['vault', 'code', 'token', 'amount', 'open', 'cap', 'commitment']) assert.equal(s.checks.find((c) => c.id === id).status, 'pass', id);
  assert.equal(s.checks.find((c) => c.id === 'balance').status, 'skip');
  log('flow anonymity set for 0.01 ETH:', s.anon.same, 'of', s.anon.total, 'deposits;', s.checks.find((c) => c.id === 'cap').detail);
});

test('live: the exact depositETH calldata simulates and returns the next leaf index', { skip: !LIVE }, async () => {
  // eth_call / eth_estimateGas only: a simulation from a funded address, nothing is signed or sent.
  const from = '0x37a46E7334Dd061E0C7E2298247EE9278d0d321F'; // the demo deployer (holds Sepolia ETH)
  const fresh = '0x' + [...crypto.getRandomValues(new Uint8Array(32))].map((b) => b.toString(16).padStart(2, '0')).join('');
  const data = call.depositETH(0, fresh);
  const ret = await r.call(dep.vault, data, { from, value: 10n ** 16n });
  assert.match(ret, /^0x[0-9a-f]{64}$/);
  const count = decode.uint(await r.call(dep.vault, call.depositCount()));
  assert.equal(decode.uint(ret), count, 'returns the leaf index the deposit would get');
  const gas = await r.estimateGas({ from, to: dep.vault, data, value: '0x' + (10n ** 16n).toString(16) });
  log('depositETH simulation: leaf', decode.uint(ret), 'estimateGas', gas);
  assert.ok(gas > 100_000n && gas < 2_000_000n);
});

test('live: an ERC-20 deposit without allowance reverts with a decodable error', { skip: !LIVE }, async () => {
  const { decodeRevert, extractRevertData } = await import('../src/lib/vault.js');
  const from = '0x000000000000000000000000000000000000dEaD';
  let err = null;
  try {
    await r.call(dep.vault, call.deposit(USDC, 0, '0x' + '5a'.repeat(32)), { from });
  } catch (e) {
    err = e;
  }
  assert.ok(err, 'reverts');
  const d = decodeRevert(extractRevertData(err));
  log('ERC-20 deposit without allowance ->', d.code, extractRevertData(err)?.slice(0, 10));
  assert.equal(d.code, 'insufficient-allowance');
});
