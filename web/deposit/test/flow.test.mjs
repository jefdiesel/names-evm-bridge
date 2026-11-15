// Walks the real flow module (src/lib/flow.js) through every step with a mock EIP-1193
// wallet over an in-memory chain that follows BridgeVault's deposit rules.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createFlow } from '../src/lib/flow.js';
import { DEV } from '../src/lib/deployments.js';
import { encodeRequest } from '../src/lib/request.js';
import { createMockChain, createMockWallet, ADDR } from '../dev/mock-chain.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';

// The desk-off path (the vault called directly): pinned off here, whatever deployments.js says;
// the desk-on path is test/desk-flow.test.mjs.
DEPLOYMENTS[0].desk = '';
import { selector } from '../src/lib/keccak.js';

const C1 = '0x' + '1c'.repeat(32);
const ethReq = (over = {}) => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1, ...over });
const usdcReq = (over = {}) => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.usdc, denomIndex: 0, amount: 100_000_000n, commitment: C1, ...over });

const tick = () => new Promise((r) => setImmediate(r));
async function until(flow, pred, what, max = 2000) {
  for (let i = 0; i < max; i++) {
    if (pred(flow.state)) return flow.state;
    await tick();
  }
  throw new Error(`timed out waiting for ${what}; step=${flow.state.step} error=${JSON.stringify(flow.state.error)} notice=${JSON.stringify(flow.state.notice)}`);
}

function setup({ walletChainId = 1n, chainOpts = {}, behaviour = {} } = {}) {
  const chain = createMockChain(chainOpts);
  const sent = [];
  const provider = createMockWallet(chain, { walletChainId, behaviour, onSend: (k) => sent.push(k) });
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'mock', name: 'Mock wallet', icon: '', provider }]);
  const steps = [];
  flow.subscribe((s) => {
    if (steps[steps.length - 1] !== s.step) steps.push(s.step);
  });
  return { chain, provider, flow, sent, steps };
}

test('ETH: wrong network -> switch -> checks -> deposit -> receipt -> finality', async () => {
  const { flow, sent, steps, chain } = setup();
  assert.equal(flow.load(ethReq()), true);
  assert.equal(flow.state.step, 'connect');
  assert.equal(flow.state.deployment.id, DEPLOYMENTS[0].id); // the live default (v3; v4 once LIVE_DEFAULT names it)
  assert.equal(flow.state.token.symbol, 'ETH');

  await flow.connect('mock');
  assert.equal(flow.state.step, 'network');
  assert.equal(flow.state.walletChainId, 1n);

  await flow.switchNetwork();
  const ready = await until(flow, (s) => s.step === 'ready', 'ready');
  assert.equal(ready.needsApproval, false);
  for (const c of ready.checks) assert.ok(['pass'].includes(c.status), `${c.id}: ${c.status} ${c.detail}`);
  assert.equal(ready.anon.same, 1n); // one earlier 0.01 ETH deposit
  assert.equal(ready.anon.total, 5n);
  assert.ok(ready.fee && ready.fee.gas === 470_000n);
  assert.equal(ready.data.total, 10n ** 16n);

  await flow.deposit();
  const done = await until(flow, (s) => s.step === 'done', 'done');
  assert.deepEqual(sent, ['deposit']);
  assert.equal(done.tx.leafIndex, 5n);
  assert.equal(done.tx.mine, true);
  assert.equal(done.finality.done, true);
  assert.equal(done.finality.mode, 'tag');
  assert.ok(done.finality.finalized >= done.tx.blockNumber);
  assert.equal(done.anon.same, 2n);
  assert.ok(chain.S.used.has(C1));
  // the order the person sees
  const order = ['request', 'connect', 'network', 'checking', 'ready', 'deposit-sign', 'deposit-wait', 'finalizing', 'done'];
  assert.deepEqual(steps.filter((s) => order.includes(s)), order);
});

test('dUSDC: approve exactly the amount, then deposit', async () => {
  const approvals = [];
  const { flow, sent, provider, chain } = setup({ walletChainId: 11155111n, chainOpts: { usdcBalance: 250_000_000n } });
  const origRequest = provider.request;
  provider.request = async (a) => {
    if (a.method === 'eth_sendTransaction' && a.params[0].data.startsWith(selector('approve(address,uint256)'))) approvals.push(BigInt('0x' + a.params[0].data.slice(74)));
    return origRequest(a);
  };
  flow.load(usdcReq());
  await flow.connect('mock');
  const ready = await until(flow, (s) => s.step === 'ready', 'ready');
  assert.equal(ready.needsApproval, true);
  assert.equal(ready.checks.find((c) => c.id === 'allowance').status, 'action');
  assert.equal(ready.fee.for, 'approve');

  await flow.approve();
  const ready2 = await until(flow, (s) => s.step === 'ready' && !s.needsApproval, 'ready after approval');
  assert.deepEqual(approvals, [100_000_000n], 'approves exactly the denomination, never unlimited');
  assert.equal(ready2.checks.find((c) => c.id === 'simulate').status, 'pass');

  await flow.deposit();
  await until(flow, (s) => s.step === 'done', 'done');
  assert.deepEqual(sent, ['approve', 'deposit']);
  assert.equal(chain.S.allowance.get(ADDR.me), 0n);
  assert.equal(chain.S.usdcBal.get(ADDR.me), 150_000_000n);
});

test('a declined deposit returns to ready with nothing sent', async () => {
  const { flow, sent } = setup({ walletChainId: 11155111n, behaviour: { rejectNext: new Set(['deposit']) } });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  await flow.deposit();
  assert.equal(flow.state.step, 'ready');
  assert.match(flow.state.notice.text, /declined/);
  assert.deepEqual(sent, []);
});

test('wallet without Sepolia (4902): the page adds it (EIP-3085) and carries on', async () => {
  const { flow, provider } = setup({ behaviour: { switchError: 4902 } });
  flow.load(ethReq());
  await flow.connect('mock');
  await flow.switchNetwork();
  assert.equal(provider.added.length, 1);
  assert.equal(provider.added[0].chainId, '0xaa36a7');
  assert.deepEqual(provider.added[0].rpcUrls, ['https://ethereum-sepolia-rpc.publicnode.com']);
  await until(flow, (s) => s.step === 'ready', 'ready after adding Sepolia');
});

test('wallet without Sepolia, adding declined: a plain notice, nothing sent', async () => {
  const { flow, sent } = setup({ behaviour: { switchError: 4902, rejectNext: new Set(['add']) } });
  flow.load(ethReq());
  await flow.connect('mock');
  await flow.switchNetwork();
  assert.equal(flow.state.step, 'network');
  assert.match(flow.state.notice.text, /declined adding Sepolia/);
  assert.deepEqual(sent, []);
});

test('wallet events: account change re-checks, an empty account list or disconnect forgets the wallet', async () => {
  const { flow, provider } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  provider.emit('accountsChanged', ['0x' + '99'.repeat(20)]);
  assert.equal(flow.state.account, '0x' + '99'.repeat(20));
  provider.emit('accountsChanged', []);
  assert.equal(flow.state.account, null);
  assert.equal(flow.state.wallet, null);
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready again');
  provider.emit('disconnect', { code: 4900 });
  assert.equal(flow.state.wallet, null);
  assert.equal(flow.state.step, 'connect');
});

test('a locked wallet that shares no account: a plain notice, still not connected', async () => {
  const { flow } = setup({ walletChainId: 11155111n, behaviour: { locked: true } });
  flow.load(ethReq());
  await flow.connect('mock');
  assert.equal(flow.state.account, null);
  assert.match(flow.state.notice.text, /Unlock it and try again/);
});

test('connection declined (4001): a plain notice, nothing else asked', async () => {
  const { flow, provider } = setup({ walletChainId: 11155111n, behaviour: { rejectConnect: true } });
  flow.load(ethReq());
  await flow.connect('mock');
  assert.equal(flow.state.wallet, null);
  assert.match(flow.state.notice.text, /declined the connection/);
  assert.deepEqual(provider.calls.filter((m) => m !== 'eth_requestAccounts'), []);
});

test('pre-checks block before any signature', async (t) => {
  const cases = [
    ['amount differs from the vault denomination', () => ethReq({ amount: 2n * 10n ** 16n }), {}, 'amount-mismatch'],
    ['denomination index out of range', () => ethReq({ denomIndex: 5 }), {}, 'bad-denomination'],
    ['token not registered', () => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: '0x' + '42'.repeat(20), denomIndex: 0, amount: 1n, commitment: C1 }), {}, 'not-registered'],
    ['global freeze', () => ethReq(), { state: { globalFrozen: true } }, 'frozen'],
    ['balance too low', () => ethReq(), { ethBalance: 10n ** 15n }, 'insufficient-balance'],
    ['token balance too low', () => usdcReq(), { usdcBalance: 1n }, 'insufficient-balance'],
  ];
  for (const [name, req, chainOpts, code] of cases) {
    await t.test(name, async () => {
      const { flow, sent } = setup({ walletChainId: 11155111n, chainOpts });
      flow.load(req());
      await flow.connect('mock');
      const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
      assert.equal(s.error.code, code);
      assert.deepEqual(sent, []);
    });
  }
  await t.test('cap exceeded', async () => {
    const { flow, chain } = setup({ walletChainId: 11155111n, chainOpts: { ethBalance: 10n ** 19n } });
    chain.S.tokens[ADDR.eth].cap.used = 5n * 10n ** 18n - 10n ** 15n;
    flow.load(ethReq());
    await flow.connect('mock');
    const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
    assert.equal(s.error.code, 'cap-exceeded');
  });
  await t.test('per-token freeze', async () => {
    const { flow, chain } = setup({ walletChainId: 11155111n });
    chain.S.tokens[ADDR.eth].frozen = true;
    flow.load(ethReq());
    await flow.connect('mock');
    assert.equal((await until(flow, (x) => x.step === 'blocked', 'blocked')).error.code, 'frozen');
  });
});

test('a used commitment that was this wallet’s own deposit resumes at finality', async () => {
  const { flow, chain, provider } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  await flow.deposit();
  await until(flow, (s) => s.step === 'done', 'done');
  // reload the page with the same link
  const flow2 = createFlow({ sleep: () => tick(), now: () => 0 });
  flow2.setWallets([{ id: 'mock', name: 'Mock', icon: '', provider }]);
  flow2.load(ethReq());
  await flow2.connect('mock');
  const s = await until(flow2, (x) => x.step === 'done', 'recovered done');
  assert.equal(s.tx.recovered, true);
  assert.equal(s.tx.mine, true);
  assert.match(s.notice.text, /already deposited from this address/);
  assert.ok(chain.S.used.has(C1));
});

test('a used commitment for another token is refused', async () => {
  const { flow, chain } = setup({ walletChainId: 11155111n });
  chain.S.used.add(C1);
  chain.S.logs.push({ address: ADDR.vault, topics: [chain.S.logs[0]?.topics?.[0] || (await import('../src/lib/vault.js')).DEPOSIT_TOPIC, '0x' + ADDR.usdc.slice(2).padStart(64, '0')], data: '0x' + '0'.repeat(64) + C1.slice(2) + '0'.repeat(63) + '9' + '0'.repeat(64), transactionHash: '0xaa' });
  flow.load(ethReq());
  await flow.connect('mock');
  const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
  assert.equal(s.error.code, 'commitment-used');
});

test('race: the commitment is taken between checks and signature (front-run)', async () => {
  const { flow, chain, sent } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  chain.S.used.add(C1); // someone copies C from the mempool or the link
  await flow.deposit();
  assert.equal(flow.state.step, 'blocked');
  assert.equal(flow.state.error.code, 'commitment-used');
  assert.deepEqual(sent, [], 'the final simulation stops it before the wallet is asked');
});

test('finality falls back to an honest time estimate if the node has no finalized tag', async () => {
  const { flow } = setup({ walletChainId: 11155111n, chainOpts: { finalizedSupported: false } });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  await flow.deposit();
  const s = await until(flow, (x) => x.step === 'done', 'done');
  assert.equal(s.finality.mode, 'estimate');
});

test('DEV only: unknown vault needs explicit acknowledgement before any action', async () => {
  DEV.allowUnlistedVaults = true;
  try {
  const { flow, sent } = setup({ walletChainId: 11155111n });
  flow.load(encodeRequest({ chainId: 11155111n, vault: '0x' + '77'.repeat(20), token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 }));
  assert.equal(flow.state.deployment, null);
  await flow.connect('mock');
  const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
  assert.equal(s.error.code, 'no-code'); // no contract there on the mock chain
  assert.equal(s.checks.find((c) => c.id === 'vault').status, 'warn');
  await flow.deposit();
  assert.deepEqual(sent, []);
  } finally {
    DEV.allowUnlistedVaults = false;
  }
});

test('retired v1 vault is refused outright', () => {
  const { flow } = setup();
  flow.load(encodeRequest({ chainId: 11155111n, vault: '0xa34adbe8413838668b0521230a4d96c6b7e91484', token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 }));
  assert.equal(flow.state.step, 'blocked');
  assert.equal(flow.state.error.code, 'retired');
});

test('Sepolia only: a mainnet request is refused before any read, even for an acknowledged unknown vault', async () => {
  for (const [chainId, vault] of [
    [1n, ADDR.vault],
    [1n, '0x7e57000000000000000000000000000000000a17'],
    [560048n, ADDR.vault], // Hoodi: another test network, also refused in this build
  ]) {
    const { flow, sent, provider } = setup({ walletChainId: chainId });
    assert.equal(flow.load(encodeRequest({ chainId, vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 })), false);
    assert.equal(flow.state.step, 'blocked');
    assert.equal(flow.state.error.code, 'unsupported-chain');
    assert.match(flow.state.error.message, /only on Sepolia/);
    assert.match(flow.state.error.message, /chain id 11155111/);
    if (chainId === 1n) assert.match(flow.state.error.message, /Ethereum mainnet \(chain id 1\)/);
    assert.equal(flow.state.deployment, null);
    await flow.connect('mock');
    flow.acknowledgeVault(true);
    for (let i = 0; i < 20; i++) await tick();
    assert.equal(flow.state.step, 'blocked', 'connecting a wallet does not unblock it');
    await flow.switchNetwork();
    await flow.approve();
    await flow.deposit();
    assert.deepEqual(sent, []);
    const asked = provider.calls.filter((m) => !['eth_requestAccounts', 'eth_accounts', 'eth_chainId'].includes(m));
    assert.deepEqual(asked, [], `nothing read from or sent to chain ${chainId}`);
  }
});

test('Sepolia only: a wallet on Ethereum mainnet is asked to switch to Sepolia and nothing is sent until it does', async () => {
  const switched = [];
  const { flow, sent, provider } = setup({ walletChainId: 1n });
  const orig = provider.request;
  provider.request = async (a) => {
    if (a.method === 'wallet_switchEthereumChain') switched.push(a.params[0].chainId);
    return orig(a);
  };
  flow.load(ethReq());
  await flow.connect('mock');
  assert.equal(flow.state.step, 'network');
  assert.equal(flow.state.chain.name, 'Sepolia');
  await flow.deposit();
  await flow.approve();
  assert.deepEqual(sent, [], 'nothing sent while the wallet is on mainnet');
  assert.ok(!provider.calls.includes('eth_call'), 'the vault is not even read through a mainnet wallet');
  await flow.switchNetwork();
  assert.deepEqual(switched, ['0xaa36a7'], 'the only switch ever requested is to Sepolia');
});

test('Sepolia labels: amounts are written in Sepolia ETH; the demo token is named a Sepolia test token', async () => {
  const { flow } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  assert.equal(flow.state.token.unit, 'Sepolia ETH');
  assert.equal(flow.state.chain.explorer, 'https://sepolia.etherscan.io');
  await flow.connect('mock');
  const s = await until(flow, (x) => x.step === 'ready', 'ready');
  assert.match(s.checks.find((c) => c.id === 'balance').detail, /Sepolia ETH/);
  const u = setup({ walletChainId: 11155111n });
  u.flow.load(usdcReq());
  assert.equal(u.flow.state.token.name, 'demo USDC (Sepolia test token)');
  assert.equal(u.flow.state.token.unit, 'dUSDC');
});

test('invalid request text stays on the request step with the decoder’s message', () => {
  const { flow } = setup();
  assert.equal(flow.load(ethReq().slice(0, -1)), false);
  assert.equal(flow.state.step, 'request');
  assert.equal(flow.state.requestError.code, 'checksum');
});

test('read-only RPC: vault checks run without a wallet; nothing is sent', async () => {
  const chain = createMockChain();
  const ro = createMockWallet(chain, { walletChainId: 11155111n });
  const flow = createFlow({ sleep: () => tick() });
  flow.load(ethReq());
  flow.setReadProvider(ro);
  const s = await until(flow, (x) => !x.busy && x.checks.find((c) => c.id === 'commitment').status === 'pass', 'read-only checks');
  assert.equal(s.step, 'connect');
  assert.equal(s.readOnly, true);
  assert.equal(s.checks.find((c) => c.id === 'balance').status, 'skip');
  assert.equal(s.anon.same, 1n);
  assert.ok(!ro.calls.includes('eth_sendTransaction'));
  assert.ok(!ro.calls.includes('eth_requestAccounts'));
});

test('DEV only: unknown vault with a contract: actions stay disabled until the person acknowledges it', async () => {
  DEV.allowUnlistedVaults = true;
  try {
  const ALT = '0x7e57000000000000000000000000000000000a17';
  const chain = createMockChain({ altVault: ALT });
  const sent = [];
  const provider = createMockWallet(chain, { walletChainId: 11155111n, onSend: (k) => sent.push(k) });
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'mock', name: 'Mock', icon: '', provider }]);
  flow.load(encodeRequest({ chainId: 11155111n, vault: ALT, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 }));
  await flow.connect('mock');
  const s = await until(flow, (x) => x.step === 'ready', 'ready');
  assert.equal(s.checks.find((c) => c.id === 'vault').status, 'warn');
  assert.equal(s.token.unit, 'Sepolia ETH', 'an unknown vault still labels the coin Sepolia ETH');
  await flow.deposit();
  assert.deepEqual(sent, [], 'no deposit before acknowledgement');
  flow.acknowledgeVault(true);
  await until(flow, (x) => x.step === 'ready' && x.vaultAck && !x.busy, 'ready after ack');
  await flow.deposit();
  await until(flow, (x) => x.step === 'done', 'done');
  assert.deepEqual(sent, ['deposit']);
  } finally {
    DEV.allowUnlistedVaults = false;
  }
});

test('example requests are shown but never sent', async () => {
  const { flow, sent } = setup({ walletChainId: 11155111n });
  flow.load(encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: '0x962f9a3ffc44690aac8d3d0ea8af24ff63b54b58624534c10dee46b139dfdb0f' }));
  assert.equal(flow.state.example, true);
  await flow.connect('mock');
  await until(flow, (x) => x.step === 'ready', 'ready');
  await flow.deposit();
  assert.equal(flow.state.step, 'ready');
  assert.deepEqual(sent, []);
});

test('a silent network change after the checks: the deposit is not sent', async () => {
  const { flow, sent, provider } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  provider.setChainSilently(1n); // no chainChanged event
  await flow.deposit();
  assert.equal(flow.state.step, 'network');
  assert.deepEqual(sent, []);
});

test('a flaky RPC during finality does not stop tracking', async () => {
  const { flow, provider } = setup({ walletChainId: 11155111n });
  flow.load(ethReq());
  await flow.connect('mock');
  await until(flow, (s) => s.step === 'ready', 'ready');
  provider.failNext('eth_getBlockByNumber', 2);
  await flow.deposit();
  const s = await until(flow, (x) => x.step === 'done', 'done');
  assert.equal(s.finality.mode, 'tag', 'two failed reads do not switch to the estimate');
});

test('restore reconnects silently only when the wallet already shares an account', async () => {
  const chain = createMockChain();
  const quiet = createMockWallet(chain, { walletChainId: 11155111n });
  const flow = createFlow({ sleep: () => tick() });
  flow.setWallets([{ id: 'q', name: 'Q', icon: '', provider: quiet }]);
  flow.load(ethReq());
  assert.equal(await flow.restore('q'), false);
  assert.ok(!quiet.calls.includes('eth_requestAccounts'), 'never prompts');
  const known = createMockWallet(chain, { walletChainId: 11155111n, behaviour: { alreadyConnected: true } });
  flow.setWallets([{ id: 'k', name: 'K', icon: '', provider: known }]);
  assert.equal(await flow.restore('k'), true);
  assert.ok(!known.calls.includes('eth_requestAccounts'));
  await until(flow, (s) => s.step === 'ready', 'ready after restore');
});

// ------------------------------------------------------------------ review C, MEDIUM: unlisted vaults

test('an unlisted vault is refused at load, before any read, with no way to acknowledge it', async () => {
  const { flow, sent, provider } = setup({ walletChainId: 11155111n });
  assert.equal(DEV.allowUnlistedVaults, false, 'off in the published build');
  assert.equal(flow.load(encodeRequest({ chainId: 11155111n, vault: '0x' + 'e7'.repeat(20), token: ADDR.eth, denomIndex: 0, amount: 10n ** 18n, commitment: C1 })), false);
  assert.equal(flow.state.step, 'blocked');
  assert.equal(flow.state.error.code, 'unlisted-vault');
  assert.match(flow.state.error.message, /doesn’t know/);
  await flow.connect('mock');
  flow.acknowledgeVault(true);
  for (let i = 0; i < 30; i++) await tick();
  assert.equal(flow.state.step, 'blocked');
  await flow.deposit();
  assert.deepEqual(sent, []);
  assert.ok(!provider.calls.includes('eth_call') && !provider.calls.includes('eth_getCode'), 'nothing read from the unlisted vault');
});

test('DEV only: a vault that reports other decimals for ETH is refused; ETH is always 18 decimals', async () => {
  // the reviewer's PoC (poc-unknown-vault.mjs): a fake vault reports 24 decimals for ETH, so the
  // page showed "Deposit 0.000001 Sepolia ETH" while the wallet was asked to send 1 ETH
  const EVIL = '0x' + 'e7'.repeat(20);
  const W = (v) => BigInt(v).toString(16).padStart(64, '0');
  const sent = [];
  const answers = {
    [selector('tokenConfig(address)')]: () => '0x' + [W(1), W(0), W(0), W(24), W(86400), W(9 * 32), W(11 * 32), W(13 * 32), W(0), W(1), W(10n ** 18n), W(1), W(0), W(1), W(0)].join(''),
  };
  const provider = {
    calls: [],
    async request({ method, params = [] }) {
      this.calls.push(method);
      if (method === 'eth_chainId') return '0xaa36a7';
      if (method === 'eth_accounts' || method === 'eth_requestAccounts') return ['0x' + '11'.repeat(20)];
      if (method === 'eth_getCode') return '0x6000';
      if (method === 'eth_call') {
        const f = answers[params[0].data.slice(0, 10)];
        if (!f) throw new Error('unexpected call');
        return f();
      }
      if (method === 'eth_sendTransaction') sent.push(params[0]);
      throw new Error('unhandled ' + method);
    },
    on() {},
    removeListener() {},
  };
  DEV.allowUnlistedVaults = true;
  try {
    const flow = createFlow({ sleep: () => tick() });
    flow.setWallets([{ id: 'w', name: 'Evil', icon: '', provider }]);
    flow.load(encodeRequest({ chainId: 11155111n, vault: EVIL, token: ADDR.eth, denomIndex: 0, amount: 10n ** 18n, commitment: C1 }));
    assert.equal(flow.state.token.decimals, 18);
    await flow.connect('w');
    const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
    assert.equal(s.error.code, 'decimals');
    assert.equal(s.token.decimals, 18, 'never 24');
    flow.acknowledgeVault(true);
    await flow.deposit();
    assert.deepEqual(sent, []);
  } finally {
    DEV.allowUnlistedVaults = false;
  }
});
