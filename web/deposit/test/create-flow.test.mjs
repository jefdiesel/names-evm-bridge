// The deposit made on the page, through the real flow and a mock wallet: connect, choose,
// Logos account, save the claim file, deposit, receipt. The secret never leaves memory.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createFlow } from '../src/lib/flow.js';
import { encodeRequest } from '../src/lib/request.js';
import { verifyClaimFile } from '../src/lib/create.js';
import { createMockChain, createMockWallet, ADDR } from '../dev/mock-chain.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';

// The desk-off path (the vault called directly): pinned off here, whatever deployments.js says;
// the desk-on path is test/desk-flow.test.mjs.
DEPLOYMENTS[0].desk = '';

const ACCOUNT = '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f';
const tick = () => new Promise((r) => setImmediate(r));
async function until(flow, pred, what, max = 3000) {
  for (let i = 0; i < max; i++) {
    if (pred(flow.state)) return flow.state;
    await tick();
  }
  throw new Error(`timed out waiting for ${what}; step=${flow.state.step} error=${JSON.stringify(flow.state.error)} notice=${JSON.stringify(flow.state.notice)}`);
}

// storage that records every write, standing in for the browser's
const writes = [];
const fakeStorage = () => ({ setItem: (k, v) => writes.push([k, String(v)]), getItem: () => null, removeItem() {} });
globalThis.localStorage = fakeStorage();
globalThis.sessionStorage = fakeStorage();

function setup({ walletChainId = 11155111n, chainOpts = {}, behaviour = {} } = {}) {
  const chain = createMockChain(chainOpts);
  const sent = [];
  const provider = createMockWallet(chain, { walletChainId, behaviour, onSend: (k) => sent.push(k) });
  const requests = [];
  const orig = provider.request;
  provider.request = async (a) => {
    requests.push(JSON.stringify(a, (k, v) => (typeof v === 'bigint' ? v.toString() : v)));
    return orig(a);
  };
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'mock', name: 'Mock wallet', icon: '', provider }]);
  return { chain, provider, flow, sent, requests };
}

test('made here: connect, choose, account, save, deposit, receipt; the secret stays in memory', async () => {
  const { flow, sent, requests } = setup();
  await flow.connect('mock');
  assert.equal(flow.state.account, ADDR.me);
  assert.equal(await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: ACCOUNT }), true);
  assert.equal(flow.state.createdHere, true);
  const { s, k, commitment } = flow.state.draft;
  assert.match(s, /^0x[0-9a-f]{64}$/);
  const ready = await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  assert.equal(ready.request.commitment, commitment);
  assert.ok(ready.checks.every((c) => c.status === 'pass'), JSON.stringify(ready.checks));

  await flow.deposit();
  assert.deepEqual(sent, [], 'no deposit before the claim file is saved');
  const file = flow.claimFile();
  assert.equal(await verifyClaimFile(file), true);
  assert.equal(file.logosAccount, ACCOUNT);
  assert.equal(file.masterSecret, s);
  assert.equal(file.claimKey, k);
  assert.equal(file.deposit, null);
  flow.markSaved(true);
  await flow.deposit();
  const done = await until(flow, (x) => x.step === 'done', 'done');
  assert.deepEqual(sent, ['deposit']);
  const after = flow.claimFile();
  assert.equal(after.deposit.tx, done.tx.hash);
  assert.equal(after.deposit.leafIndex, String(done.tx.leafIndex));

  const everything = requests.join('\n') + JSON.stringify(writes);
  for (const secret of [s, k, s.slice(2), k.slice(2)]) assert.ok(!everything.includes(secret), 'the secret is in no wallet request and no storage write');
  assert.ok(requests.some((r) => r.includes(commitment.slice(2))), 'the commitment, which is public, is sent');
});

test('made here: a token needs the exact approval first; the deposit still waits for the saved file', async () => {
  const { flow, sent } = setup({ chainOpts: { usdcBalance: 250_000_000n } });
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.usdc, denomIndex: 0, account: ACCOUNT });
  await until(flow, (x) => x.step === 'ready' && x.needsApproval && !x.busy, 'approve');
  await flow.approve();
  await until(flow, (x) => x.step === 'ready' && !x.needsApproval && !x.busy, 'ready to deposit');
  await flow.deposit();
  assert.deepEqual(sent, ['approve']);
  flow.markSaved(true);
  await flow.deposit();
  await until(flow, (x) => x.step === 'done', 'done');
  assert.deepEqual(sent, ['approve', 'deposit']);
});

test('made here: wrong LEZ account inputs are explained and make no secret', async () => {
  const { flow } = setup();
  await flow.connect('mock');
  for (const [text, code] of [['0x5a1e0000000000000000000000000000000c0de5', 'eth-address'], [ACCOUNT.slice(0, 20), 'incomplete'], [ACCOUNT.replace('N', '0'), 'unrecognised'], ['7a6223abde07c0c278d203973fd233ab3e47b2bfa53c8ed3d740123456789abc', 'blockchain-key']]) {
    assert.equal(await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: text }), false);
    assert.equal(flow.state.draft.accountError.code, code);
    assert.equal(flow.state.draft.s, undefined);
    assert.equal(flow.state.request, null);
  }
  assert.equal(await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: '' }), false);
  assert.equal(flow.state.draft.accountError, null, 'an empty field is not an error yet');
});

test('made here: changing the amount after saving makes a new secret and needs a new save', async () => {
  const { flow, sent } = setup();
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: ACCOUNT });
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  const first = flow.state.draft;
  flow.markSaved(true);
  assert.equal(flow.state.saved, true);
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: ` Private/${ACCOUNT} ` });
  assert.equal(flow.state.draft.s, first.s, 'the same deposit, written differently, keeps its secret');
  assert.equal(flow.state.saved, true);
  await flow.setDraft({ token: ADDR.eth, denomIndex: 1, account: ACCOUNT });
  assert.notEqual(flow.state.draft.s, first.s);
  assert.notEqual(flow.state.draft.commitment, first.commitment);
  assert.equal(flow.state.saved, false);
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready again');
  await flow.deposit();
  assert.deepEqual(sent, []);
});

test('made here: a reload after sending resumes from the public link alone; the secret is gone with the tab', async () => {
  const one = setup({ behaviour: { alreadyConnected: true } });
  await one.flow.connect('mock');
  await one.flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: ACCOUNT });
  await until(one.flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  one.flow.markSaved(true);
  await one.flow.deposit();
  await until(one.flow, (x) => x.step === 'done', 'done');
  const link = encodeRequest(one.flow.state.request); // what the page puts in the address bar after sending
  // a new page over the same chain: only the public link
  const provider = createMockWallet(one.chain, { walletChainId: 11155111n, behaviour: { alreadyConnected: true } });
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'mock', name: 'Mock wallet', icon: '', provider }]);
  flow.load(link);
  await flow.restore('mock');
  const back = await until(flow, (x) => x.step === 'done' || x.step === 'finalizing', 'resumed');
  assert.equal(back.tx.hash, one.flow.state.tx.hash);
  assert.equal(back.createdHere, false);
  assert.equal(back.draft, null);
  assert.equal(flow.claimFile(), null);
});

test('made here: a vault whose configuration differs is refused before anything is signed', async () => {
  const s2 = setup();
  s2.chain.S.cfgId = '0x' + '00'.repeat(31) + '01';
  await s2.flow.connect('mock');
  await s2.flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: ACCOUNT });
  const b = await until(s2.flow, (x) => x.step === 'blocked', 'blocked');
  assert.equal(b.error.code, 'cfg-mismatch');
  assert.deepEqual(s2.sent, []);
});

import { DEMO_ACCOUNT, TEST_CHAIN_ACCOUNTS, isTestChainAccount, LOGOS_SIDE, findDeployment } from '../src/lib/deployments.js';
import { base58Encode } from '../src/lib/create.js';
import { randomBytes } from 'node:crypto';

const V = '2if1EJVcZwFT3oXxF4uewB2mk2XDXpmCWwTtBwUQ6HUC'; // the team's private account
const P2 = TEST_CHAIN_ACCOUNTS.find((a) => a.kind === 'public').id; // the team's public account

test('v3 offers no demo account: the private V needs the tick there (its v3 claim is unproven), v2 keeps it', () => {
  assert.equal(DEMO_ACCOUNT, null);
  assert.equal(isTestChainAccount(V), false);
  assert.equal(isTestChainAccount(P2), true);
  const v2 = findDeployment(11155111, '0x8c8b2664de28e5287158db4db65d448638871b74');
  assert.equal(v2.demoAccount, V);
  assert.ok(TEST_CHAIN_ACCOUNTS.every((a) => isTestChainAccount(a.id, v2)));
});

test('the team’s public account: on the team’s test chain, no extra tick', async () => {
  const { flow, sent } = setup();
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: P2 });
  assert.equal(flow.state.draft.claimable, true);
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  flow.markSaved(true);
  await flow.deposit();
  await until(flow, (x) => x.step === 'done', 'done');
  assert.deepEqual(sent, ['deposit']);
});

test('any other account: nothing is sent until the person ticks that it can’t be claimed yet', async () => {
  const own = base58Encode(Uint8Array.from(randomBytes(32)));
  const { flow, sent } = setup();
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: `Public/${own}` });
  assert.equal(flow.state.draft.claimable, false);
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  flow.markSaved(true);
  await flow.deposit();
  assert.deepEqual(sent, [], 'saved but not acknowledged: nothing sent');
  flow.ackUnclaimable(true);
  await flow.deposit();
  await until(flow, (x) => x.step === 'done', 'done');
  assert.deepEqual(sent, ['deposit']);
  // a new account resets the acknowledgement
  const s2 = setup();
  await s2.flow.connect('mock');
  await s2.flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: own });
  s2.flow.ackUnclaimable(true);
  await s2.flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: base58Encode(Uint8Array.from(randomBytes(32))) });
  assert.equal(s2.flow.state.unclaimableAck, false);
});

test('the wallet’s RPC fails while sending (first try): nothing sent, a plain message, the second try works', async () => {
  const { flow, sent } = setup({ behaviour: { sendRpcFailsOnce: true } });
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: P2 });
  await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
  flow.markSaved(true);
  await flow.deposit();
  assert.deepEqual(sent, []);
  assert.equal(flow.state.step, 'ready');
  assert.equal(flow.state.notice, null, 'no raw library text as the message');
  assert.equal(flow.state.walletRpc.words, 'error 404');
  await flow.deposit();
  await until(flow, (x) => x.step === 'done', 'done on the second try');
  assert.deepEqual(sent, ['deposit']);
  assert.equal(flow.state.walletRpc, null);
});

test('one switch: when visitors can claim themselves (LOGOS_SIDE.selfClaim), an own account is claimable but still confirmed once', async () => {
  assert.equal(LOGOS_SIDE.selfClaim, false, 'no published claim tool yet');
  assert.equal(LOGOS_SIDE.publicRpc, 'https://lez.nameswallet.app/', 'the bridge test network is public');
  assert.equal(LOGOS_SIDE.network, 'the bridge test network (LEZ 0.4)');
  LOGOS_SIDE.selfClaim = true;
  try {
    const { flow, sent } = setup();
    await flow.connect('mock');
    await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: base58Encode(Uint8Array.from(randomBytes(32))) });
    assert.equal(flow.state.draft.claimable, true);
    await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
    flow.markSaved(true);
    await flow.deposit();
    assert.deepEqual(sent, [], 'an account the team does not hold still needs “This is my account”');
    flow.ackUnclaimable(true);
    await flow.deposit();
    await until(flow, (x) => x.step === 'done', 'done');
    assert.deepEqual(sent, ['deposit']);
  } finally {
    LOGOS_SIDE.selfClaim = false;
  }
});
