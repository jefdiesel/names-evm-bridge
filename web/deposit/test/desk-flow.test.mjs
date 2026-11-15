// The bridge fee through the DepositDesk (vault/src/desk/DepositDesk.sol): the page's one switch
// (deployments.js `desk`), every deposit through the desk when it is on, the fee in every amount
// the wallet sees, and the desk's account argument, always zero (the page asks for no key; a public
// claim's fee is the relayer's co-signature, DESIGN.md §3.6). The desk-off path is
// test/flow.test.mjs and test/create-flow.test.mjs, unchanged.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createFlow, bpsText } from '../src/lib/flow.js';
import { encodeRequest } from '../src/lib/request.js';
import { verifyClaimFile } from '../src/lib/create.js';
import { DEPLOYMENTS, deskOf } from '../src/lib/deployments.js';
import { selector } from '../src/lib/keccak.js';
import { ZERO32, decodeRevert } from '../src/lib/vault.js';
import { createMockChain, createMockWallet, ADDR } from '../dev/mock-chain.js';
import { readFileSync } from 'node:fs';

const DESK = '0x' + 'de5c'.repeat(10);
const C1 = '0x' + '1c'.repeat(32);
// an outside public account (not a team account): the account of the secp256k1 generator's x as
// its key, as base58 (only an address; the page never sees a key)
const OUTSIDE = 'CntQ7q2MHGxso9ebbdv6XVYtzhKnGJYHnhYA1V6Aaxg5';

const tick = () => new Promise((r) => setImmediate(r));
async function until(flow, pred, what, max = 3000) {
  for (let i = 0; i < max; i++) {
    if (pred(flow.state)) return flow.state;
    await tick();
  }
  throw new Error(`timed out waiting for ${what}; step=${flow.state.step} error=${JSON.stringify(flow.state.error)} notice=${JSON.stringify(flow.state.notice)}`);
}

function setup({ chainOpts = {}, desk = { address: DESK } } = {}) {
  const chain = createMockChain({ ...chainOpts, desk });
  const sent = [];
  const txs = [];
  const provider = createMockWallet(chain, { walletChainId: 11155111n, onSend: (k, tx) => (sent.push(k), txs.push(tx)) });
  const flow = createFlow({ sleep: () => tick(), now: () => (1794540000 + 200000) * 1000 });
  flow.setWallets([{ id: 'mock', name: 'Mock wallet', icon: '', provider }]);
  return { chain, flow, sent, txs };
}

/** Turn the desk on for one test (the shipped config has it off). */
async function withDesk(address, fn) {
  const before = DEPLOYMENTS[0].desk;
  DEPLOYMENTS[0].desk = address;
  try {
    await fn();
  } finally {
    DEPLOYMENTS[0].desk = before;
  }
}

const ethReq = () => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C1 });
const usdcReq = () => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.usdc, denomIndex: 0, amount: 100_000_000n, commitment: C1 });

// ------------------------------------------------------------------ the switch

test('the switch: the desk is empty (off) or a valid address (on), nothing else', () => {
  const d = DEPLOYMENTS[0].desk;
  assert.ok(d === '' || /^0x[0-9a-fA-F]{40}$/.test(d), `desk must be '' or an address, not ${d}`);
  assert.equal(deskOf(DEPLOYMENTS[0]) === null, d === '');
  assert.equal(deskOf({ desk: '' }), null);
  assert.equal(deskOf({ desk: 'not an address' }), null);
  assert.equal(deskOf({ desk: '0x' + '0'.repeat(40) }), null);
  assert.equal(deskOf({ desk: DESK.toUpperCase().replace('0X', '0x') }), DESK);
});

test('the desk ABI the page encodes matches the Solidity selectors pinned in vault/test/DepositDesk.t.sol', () => {
  assert.equal(selector('depositETHWithFee(uint32,bytes32,bytes32)'), '0x03321b04');
  assert.equal(selector('depositWithFee(address,uint32,bytes32,bytes32)'), '0xa93bf283');
  assert.equal(selector('quote(address,uint32)'), '0xea48dc6c');
  assert.equal(selector('feeBps()'), '0x24a9d853');
  assert.equal(selector('vault()'), '0xfbfa77cf');
  assert.equal(decodeRevert(selector('WrongAmount()')).code, 'wrong-amount');
  assert.equal(decodeRevert(selector('UnexpectedValue()')).code, 'unexpected-value');
});

test('bpsText: 300 -> 3%, 250 -> 2.5%, 0 -> 0%', () => {
  assert.equal(bpsText(300n), '3%');
  assert.equal(bpsText(250n), '2.5%');
  assert.equal(bpsText(5n), '0.05%');
  assert.equal(bpsText(0n), '0%');
});


// ------------------------------------------------------------------ desk on: requests

test('desk on, ETH request: the fee is checked, shown in the total, sent to the desk with no key', () =>
  withDesk(DESK, async () => {
    const { flow, sent, txs, chain } = setup();
    flow.load(ethReq());
    await flow.connect('mock');
    const ready = await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
    for (const c of ready.checks) assert.equal(c.status, 'pass', `${c.id}: ${c.detail}`);
    assert.ok(ready.checks.some((c) => c.id === 'desk'));
    assert.equal(ready.data.desk, DESK);
    assert.equal(ready.data.feeBps, 300n);
    assert.equal(ready.data.bridgeFee, 3n * 10n ** 14n);
    assert.equal(ready.data.total, 103n * 10n ** 14n, '0.01 ETH + 3%');
    await flow.deposit();
    const done = await until(flow, (s) => s.step === 'done', 'done');
    assert.deepEqual(sent, ['deposit']);
    assert.equal(txs[0].to, DESK);
    assert.equal(BigInt(txs[0].value), 103n * 10n ** 14n);
    assert.ok(txs[0].data.startsWith('0x03321b04') && txs[0].data.endsWith(ZERO32.slice(2)), 'no key: the float pays a private claim');
    assert.equal(done.tx.leafIndex, 5n, 'the vault’s Deposit log is found behind the desk');
    assert.equal(done.tx.mine, true);
    assert.equal(chain.S.deskFees.get(ADDR.eth), 3n * 10n ** 14n);
    assert.ok(chain.S.used.has(C1));
  }));

test('desk on, dUSDC: one exact approval of the desk for denomination + 3%, then the deposit', () =>
  withDesk(DESK, async () => {
    const { flow, sent, txs, chain } = setup({ chainOpts: { usdcBalance: 250_000_000n } });
    flow.load(usdcReq());
    await flow.connect('mock');
    const ready = await until(flow, (s) => s.step === 'ready' && s.needsApproval && !s.busy, 'approve');
    assert.equal(ready.data.total, 103_000_000n);
    await flow.approve();
    assert.equal(txs[0].to, ADDR.usdc);
    assert.equal(txs[0].data.slice(34, 74), DESK.slice(2), 'the spender is the desk');
    assert.equal(BigInt('0x' + txs[0].data.slice(74)), 103_000_000n, 'approves exactly denomination + fee, never unlimited');
    await until(flow, (s) => s.step === 'ready' && !s.needsApproval && !s.busy, 'ready');
    await flow.deposit();
    await until(flow, (s) => s.step === 'done', 'done');
    assert.deepEqual(sent, ['approve', 'deposit']);
    assert.equal(chain.S.usdcBal.get(ADDR.me), 147_000_000n);
    assert.equal(chain.S.deskAllowance.get(ADDR.me), 0n);
    assert.equal(chain.S.deskFees.get(ADDR.usdc), 3_000_000n);
  }));

test('desk on: the balance check counts the fee (0.01 ETH exactly is not enough)', () =>
  withDesk(DESK, async () => {
    const { flow, sent } = setup({ chainOpts: { ethBalance: 10n ** 16n } });
    flow.load(ethReq());
    await flow.connect('mock');
    const s = await until(flow, (x) => x.step === 'blocked', 'blocked');
    assert.equal(s.error.code, 'insufficient-balance');
    assert.match(s.error.message, /0\.0103 Sepolia ETH deposit with its bridge fee/);
    assert.deepEqual(sent, []);
  }));

test('desk on: a desk that forwards to another vault, or no desk code, blocks before any signature', () =>
  withDesk(DESK, async () => {
    const a = setup({ desk: { address: DESK, vault: '0x' + '77'.repeat(20) } });
    a.flow.load(ethReq());
    await a.flow.connect('mock');
    assert.equal((await until(a.flow, (x) => x.step === 'blocked', 'blocked')).error.code, 'desk-vault');
    const b = setup({ desk: { address: '0x' + '55'.repeat(20) } }); // the page's desk has no code here
    b.flow.load(ethReq());
    await b.flow.connect('mock');
    assert.equal((await until(b.flow, (x) => x.step === 'blocked', 'blocked')).error.code, 'no-desk');
    assert.deepEqual([...a.sent, ...b.sent], []);
  }));

test('desk on: the fee changes after the checks: the re-read quote refuses, nothing is sent', () =>
  withDesk(DESK, async () => {
    const desk = { address: DESK, feeBps: 300n };
    const { flow, sent } = setup({ desk }); // the mock reads feeBps live from this object
    flow.load(ethReq());
    await flow.connect('mock');
    await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
    desk.feeBps = 100n;
    await flow.deposit();
    assert.deepEqual(sent, []);
    assert.equal(flow.state.step, 'blocked');
    // caught before the signature request by the re-read of the desk's quote (review C)
    assert.equal(flow.state.error.code, 'quote-changed');
  }));

// ------------------------------------------------------------------ desk on: made here

test('desk on, made here, the team’s public account: the fee is charged, the desk gets a zero account, claim file carries the fee', () =>
  withDesk(DESK, async () => {
    const { flow, sent, txs } = setup();
    await flow.connect('mock');
    await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f' });
    await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
    flow.markSaved(true);
    await flow.deposit();
    await until(flow, (s) => s.step === 'done', 'done');
    assert.deepEqual(sent, ['deposit']);
    assert.ok(txs[0].data.endsWith(ZERO32.slice(2)));
    const file = flow.claimFile();
    assert.equal(await verifyClaimFile(file), true);
    assert.deepEqual(file.bridgeFee, { desk: DESK, feeBps: '300', amount: '300000000000000' });
    assert.equal(file.lezPublicKey, undefined);
  }));

test('desk on, made here, own public account: the address is all it takes; the desk gets a zero account', () =>
  withDesk(DESK, async () => {
    const { flow, sent, txs } = setup();
    await flow.connect('mock');
    await flow.setDraft({ token: ADDR.eth, denomIndex: 1, account: 'Public/' + OUTSIDE });
    assert.equal(flow.state.draft.accountId, OUTSIDE);
    assert.equal('publicKeyWanted' in flow.state.draft, false, 'no key is asked for');
    flow.ackUnclaimable(true);
    await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
    const file = flow.claimFile();
    assert.equal(file.lezPublicKey, undefined);
    assert.equal(file.bridgeFee.amount, '3000000000000000');
    flow.markSaved(true);
    await flow.deposit();
    await until(flow, (s) => s.step === 'done', 'done');
    assert.deepEqual(sent, ['deposit']);
    assert.equal(BigInt(txs[0].value), 103n * 10n ** 15n);
    assert.ok(txs[0].data.endsWith(ZERO32.slice(2)), 'lezPublicKey is zero');
    assert.equal(await verifyClaimFile(flow.claimFile()), true);
  }));

test('desk on, made here, private account: same, zero account', () =>
  withDesk(DESK, async () => {
    const { flow, sent, txs } = setup();
    await flow.connect('mock');
    await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: 'Private/' + OUTSIDE });
    flow.ackUnclaimable(true);
    await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
    flow.markSaved(true);
    await flow.deposit();
    await until(flow, (s) => s.step === 'done', 'done');
    assert.deepEqual(sent, ['deposit']);
    assert.ok(txs[0].data.endsWith(ZERO32.slice(2)));
  }));

test('desk off: the vault is called directly (the switch is the address alone)', () => withDesk('', async () => {
  const { flow, sent, txs } = setup({ desk: null });
  await flow.connect('mock');
  await flow.setDraft({ token: ADDR.eth, denomIndex: 0, account: OUTSIDE });
  flow.ackUnclaimable(true);
  const ready = await until(flow, (s) => s.step === 'ready' && !s.busy, 'ready');
  assert.equal(ready.data.desk, null);
  assert.equal(ready.data.total, 10n ** 16n);
  assert.ok(!ready.checks.some((c) => c.id === 'desk'));
  flow.markSaved(true);
  await flow.deposit();
  await until(flow, (s) => s.step === 'done', 'done');
  assert.equal(txs[0].to, ADDR.vault);
  assert.deepEqual(sent, ['deposit']);
  assert.equal(flow.claimFile().bridgeFee, undefined);
}));

test('the page states the fee in the owner’s words and keeps the network label', () => {
  const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
  assert.ok(app.includes('It covers the claim’s fee on the bridge test network.'));
  assert.ok(app.includes('Bridge fee ('));
  assert.ok(app.includes('bridge test network. Not on LEZ 0.3.'));
  assert.ok(!/official/i.test(app), 'no "official"');
  assert.ok(!/public key/i.test(app), 'the page asks for no public key');
  assert.ok(app.includes('The bridge pays the claim’s fee (the bridge fee covers it), so your account needs no LEZ.'));
});

test('the form says what happens now and what later; the claim steps come after the deposit', () => {
  const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
  // owner feedback 2026-11-14: the form read as "claim now"; no instruction for the later step before the deposit
  assert.ok(app.includes("h('b', {}, 'Now: ')") && app.includes("h('b', {}, 'Later: ')"));
  assert.ok(!app.includes('To claim it, open'), 'no claim instruction on the form');
  // the claim runs in the Names Wallet: its own labels (logos-names-wallet-next 5f5f6df)
  assert.ok(app.includes('Wallet → LEZ 0.4 → “Claim a bridge deposit”.') && app.includes('“Choose the claim file”: the one you saved here.') && app.includes('Press “Claim” and approve it.'));
  assert.ok(app.includes("h('strong', {}, 'Keep the claim file you saved.')"));
});

test('one save: the claim file is saved before the deposit only; after it, an optional copy at most', () => {
  // owner 2026-11-14: "the save claim shouldnt be twice". The claim finds the leaf from the vault's
  // logs (relayed.rs: a file with deposit null claims the same), so the copy with the tx is optional.
  const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
  assert.ok(!app.includes('Save updated claim file') && !app.includes('Save the updated one'));
  assert.ok(app.includes('Your claim file is already saved; nothing more to do here.'));
  assert.ok(app.includes('Optional: a copy with the transaction added') && app.includes('not needed to claim'));
  assert.equal((app.match(/primary\('Save claim file', \(\) => saveFile/g) || []).length, 1, 'one save prompt with an action');
});

test('the account field states the one rule: the account’s keys are in the LEZ 0.4 wallet; the tick says it too', () => {
  // near-miss 2026-11-14: an account only in the LEZ 0.3 wallet was pasted; a restored LEZ 0.4 wallet
  // has the same public ids, a new one does not, so the rule is about the keys
  const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
  assert.ok(app.includes('Use a public account whose keys are in your LEZ 0.4 wallet (Names Wallet 0.17), or another LEZ 0.4 account whose keys you hold.'));
  assert.ok(app.includes('on the Wallet page choose LEZ 0.4, then use Copy next to Public account in the Bridge test network card. Make sure LEZ 0.4 is the network selected when you copy.'));
  assert.ok(!app.includes('open Settings → Accounts') && !app.includes('marked Pays'), 'Settings → Accounts shows ids shortened, without Copy');
  assert.ok(app.includes('This page cannot check which wallet holds an account; the Names Wallet checks it when you claim.'));
  assert.ok(app.includes('An account that is only in your LEZ 0.3 wallet cannot claim this deposit.'));
  assert.ok(!app.includes('Create my wallet'), 'the page does not steer people into new keys');
  assert.ok(!app.includes('Not an account from your LEZ 0.3 wallet'));
  assert.ok(app.includes('The keys of this account are in my LEZ 0.4 wallet (Names Wallet 0.17), or I hold them myself.'));
  assert.ok(app.includes('I understand the claim waits for that wallet build.'));
});

test('plain layout: numbered steps, one sentence each, the rest behind a native "More", plain buttons', () => {
  // owner feedback 2026-11-14: "WALLS of text"
  const app = readFileSync(new URL('../src/app.js', import.meta.url), 'utf8');
  assert.ok(app.includes('`Step ${i + 1} of ${n}: ${x.title}`'), 'the step indicator names the step in words');
  assert.ok(app.includes("h('details', { class: 'more' }, h('summary', {}, label)"), 'each "More" is a native <details> with its own name');
  for (const t of ['Connect your wallet', 'Choose the amount', 'Choose where it goes', 'Save the claim file, then deposit', 'Open the deposit code', 'Compare and deposit']) assert.ok(app.includes(`title: '${t}'`), t);
  // buttons say the action only
  assert.ok(app.includes("'Paste deposit code'") && !app.includes("'I have a deposit code from my Names Wallet'"));
  assert.ok(app.includes("primary('Continue'"));
  // what a person must see to avoid losing money stays visible (not inside more())
  assert.ok(app.includes("h('strong', {}, `You send ${amt(d.total)} ${sym()}`)"), 'the total');
  assert.ok(app.includes('Save the claim file before you deposit'), 'the file path');
  assert.ok(app.includes('Check the four words above match your Names Wallet. If they differ, stop.'), 'the code path');
  assert.ok(app.includes('(not released yet)'), 'the unreleased wallet build');
});


// ------------------------------------------------------------------ review C, smaller findings

test('every transaction names its chain (chainId on eth_sendTransaction)', () =>
  withDesk(DESK, async () => {
    const { flow, txs } = setup({ chainOpts: { usdcBalance: 250_000_000n } });
    flow.load(usdcReq());
    await flow.connect('mock');
    await until(flow, (s) => s.step === 'ready' && s.needsApproval && !s.busy, 'approve');
    await flow.approve();
    await until(flow, (s) => s.step === 'ready' && !s.needsApproval && !s.busy, 'ready');
    await flow.deposit();
    await until(flow, (s) => s.step === 'done', 'done');
    assert.equal(txs.length, 2);
    for (const tx of txs) assert.equal(tx.chainId, '0xaa36a7');
  }));

test('a larger standing approval is replaced by one for exactly the quoted total', () =>
  withDesk(DESK, async () => {
    const { flow, chain, txs } = setup({ chainOpts: { usdcBalance: 250_000_000n } });
    chain.S.deskAllowance.set(ADDR.me, 10n ** 12n); // far more than the total
    flow.load(usdcReq());
    await flow.connect('mock');
    const s = await until(flow, (x) => x.step === 'ready' && !x.busy, 'ready');
    assert.equal(s.needsApproval, true, 'an excess approval is not reused');
    await flow.approve();
    const exact = txs.filter((t) => t.data.startsWith('0x095ea7b3')).map((t) => BigInt('0x' + t.data.slice(74, 138)));
    assert.equal(exact[exact.length - 1], s.data.total);
  }));

test('ERC-20: a fee raised after the approval stops the deposit before the wallet asks', () =>
  withDesk(DESK, async () => {
    const desk = { address: DESK, feeBps: 100n };
    const { flow, sent } = setup({ desk, chainOpts: { usdcBalance: 250_000_000n } });
    flow.load(usdcReq());
    await flow.connect('mock');
    await until(flow, (s) => s.step === 'ready' && s.needsApproval && !s.busy, 'approve');
    await flow.approve();
    await until(flow, (s) => s.step === 'ready' && !s.needsApproval && !s.busy, 'ready');
    desk.feeBps = 300n;
    await flow.deposit();
    assert.deepEqual(sent, ['approve']);
    assert.equal(flow.state.error.code, 'quote-changed');
  }));
