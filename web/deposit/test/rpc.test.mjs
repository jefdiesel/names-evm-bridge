// Reads go through the page's own RPC first; a broken wallet RPC is caught before a signature;
// either one down falls back to the other; both down says which.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createFlow } from '../src/lib/flow.js';
import { encodeRequest } from '../src/lib/request.js';
import { createMockChain, createMockWallet, ADDR } from '../dev/mock-chain.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';

// The desk-off path, pinned (test/desk-flow.test.mjs covers the desk).
DEPLOYMENTS[0].desk = '';

const C1 = '0x' + '1c'.repeat(32);
const ethReq = () => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 });
const tick = () => new Promise((r) => setImmediate(r));
async function until(flow, pred, what, max = 3000) {
  for (let i = 0; i < max; i++) {
    if (pred(flow.state)) return flow.state;
    await tick();
  }
  throw new Error(`timed out waiting for ${what}; step=${flow.state.step} error=${JSON.stringify(flow.state.error)}`);
}
const down = () => ({ label: 'PublicNode', request: async () => { throw Object.assign(new Error('HTTP 503'), { code: -32603, transport: true }); }, on() {}, removeListener() {} });

function setup({ walletBehaviour = {}, page = 'ok' } = {}) {
  const chain = createMockChain();
  const sent = [];
  const wallet = createMockWallet(chain, { walletChainId: 11155111n, behaviour: walletBehaviour, onSend: (k) => sent.push(k) });
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'w', name: 'Wallet', icon: '', provider: wallet }]);
  const pageRpc = page === 'down' ? down() : Object.assign(createMockWallet(chain, { walletChainId: 11155111n }), { label: 'PublicNode' });
  flow.setReadProvider(pageRpc);
  return { chain, wallet, pageRpc, flow, sent };
}

test('the wallet’s RPC answers 404 to reads: the page reads through its own and still gets ready', async () => {
  const { flow, wallet } = setup({ walletBehaviour: { brokenRpc: true } });
  flow.load(ethReq());
  await flow.connect('w');
  const s = await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  assert.ok(s.checks.every((c) => c.status === 'pass'));
  assert.ok(!wallet.calls.includes('eth_call'), 'no read went to the broken wallet RPC');
});

test('the wallet’s RPC is broken at send time: a plain message, the raw text kept aside, nothing sent', async () => {
  const { flow, sent } = setup({ walletBehaviour: { brokenRpc: true } });
  flow.load(ethReq());
  await flow.connect('w');
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  await flow.deposit();
  assert.deepEqual(sent, []);
  assert.equal(flow.state.step, 'ready');
  assert.equal(flow.state.walletRpc.words, 'error 404');
  assert.match(flow.state.walletRpc.raw, /Request failed with status code 404/);
});

test('adding a working Sepolia connection clears the wallet problem when the wallet takes it', async () => {
  const { flow, wallet, sent } = setup({ walletBehaviour: { brokenRpc: true, healOnAdd: true } });
  flow.load(ethReq());
  await flow.connect('w');
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  await flow.deposit();
  assert.ok(flow.state.walletRpc);
  await flow.addSepolia();
  assert.equal(wallet.added[0].rpcUrls[0], 'https://ethereum-sepolia-rpc.publicnode.com');
  assert.equal(flow.state.walletRpc, null);
  assert.deepEqual(sent, []);
});

test('the page’s RPC is down: reads fall back to the wallet', async () => {
  const { flow } = setup({ page: 'down' });
  flow.load(ethReq());
  await flow.connect('w');
  const s = await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready via the wallet');
  assert.ok(s.checks.every((c) => c.status === 'pass'));
});

test('both down: one plain sentence naming both, the raw texts kept for Details', async () => {
  const { flow, sent } = setup({ page: 'down', walletBehaviour: { brokenRpc: true } });
  flow.load(ethReq());
  await flow.connect('w');
  const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
  assert.equal(s.error.code, 'read-failed');
  assert.match(s.error.message, /^Could not read Sepolia: PublicNode \(error 503\) and your wallet’s Sepolia connection \(error 404\) did not answer\./);
  assert.doesNotMatch(s.error.message, /Request failed with status code/);
  assert.match(s.error.raw, /Request failed with status code 404/);
  assert.deepEqual(sent, []);
});

test('a revert is an answer, not a failure: it is not retried on the other connection', async () => {
  const { flow, wallet } = setup();
  flow.load(encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 2n * 10n ** 16n, commitment: C1 }));
  await flow.connect('w');
  const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
  assert.equal(s.error.code, 'amount-mismatch');
  assert.ok(!wallet.calls.includes('eth_call'));
});
