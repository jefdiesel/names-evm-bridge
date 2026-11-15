// DEV ONLY. Announces mock wallets over EIP-6963 (or none), sets the URL fragment, then loads
// the real app and acts like a person: clicks, typing, ticking. Scenarios below.
import { createMockChain, createMockWallet, ADDR } from './mock-chain.js';
import { encodeRequest } from '../src/lib/request.js';
import { DEPLOYMENTS, deskOf, DEV } from '../src/lib/deployments.js';
import { encodeRelease } from '../src/lib/withdraw.js';

const sc = new URLSearchParams(location.search).get('s') || 'start';
const C = '0x' + 'a7c3e95b1d0f4826'.repeat(4);
const ALT = '0x7e57000000000000000000000000000000000a17';
const ACCOUNT = '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f';
const EXAMPLE = 'logosdep1qyqqqqqqqz4rdfuv3vnxfh3gu558zkxmfkm963yx8zr3kaqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqu3cdun0cyqqp930ngllc3rfp2kg60gw4zhjflmrk494scj9xnqsmmjxkyualkc02axs7z';
const eth = (o = {}) => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.eth, denomIndex: 0, amount: 10n ** 16n, commitment: C, ...o });
const eth01 = (o = {}) => eth({ denomIndex: 1, amount: 10n ** 17n, ...o });
const usdc = (o = {}) => encodeRequest({ chainId: 11155111n, vault: ADDR.vault, token: ADDR.usdc, denomIndex: 0, amount: 100_000_000n, commitment: C, ...o });

// steps: ['click', label] | ['fill', '#id', text] | ['tick', '#id'] | ['wait', ms]
const MAKE = [['click', 'Connect wallet'], ['wait', 1200]];
const OTHER = '7mQ2kq9WcY4HnSDsrzvcxgAcmB2k2KVpGq8T2B4a4X1Z'; // a well-formed LEZ account the team holds no keys for
// The live deployment decides the form: v2 offers the team's demo account (the default) beside
// "Use my own account"; v3 has no demo account, so the own-account box is all there is.
const HAS_DEMO = !!DEPLOYMENTS[0].demoAccount;
// the plain layout: the amount step first (Continue), then the account step
const AMOUNT = [...MAKE, ['click', 'Continue'], ['wait', 400]];
const OWN = HAS_DEMO ? [...AMOUNT, ['click', 'Use my own account'], ['wait', 200]] : AMOUNT;
// The default destination, ready to save: the demo account where there is one, else an own
// public account, confirmed with the tick.
const READY = HAS_DEMO ? [...AMOUNT, ['click', 'Continue'], ['wait', 1200]] : [...OWN, ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200]];
// the same, with Demo USDC chosen at the amount step
const READY_USDC = HAS_DEMO ? [...MAKE, ['click', 'Demo USDC'], ['wait', 600], ['click', 'Continue'], ['wait', 400], ['click', 'Continue'], ['wait', 1200]] : [...MAKE, ['click', 'Demo USDC'], ['wait', 600], ['click', 'Continue'], ['wait', 400], ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200]];
const SAVE = [['click', 'Save claim file'], ['wait', 300], ['tick', '#saved-ack']];
// The deposit desk (bridge fee): an own public account, given by its address only (no key is
// asked for: the relayer pays a public claim's fee by co-signing it).
const OWN_PUBLIC = [...OWN, ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200]];
// Send back to Ethereum: a release code for the mock vault's recorded exit root
const ROOT = '0x' + '5a'.repeat(32);
const RELEASE = encodeRelease({ chainId: 11155111n, vault: ADDR.vault, root: ROOT, nullifier: '0x' + '6b'.repeat(32), token: ADDR.eth, denomIndex: 0, recipient: ADDR.me, relayer: '0x' + '00'.repeat(20), relayFee: 0n, seal: '0xe71e6620' + '77'.repeat(256) });
const SCENARIOS = {
  withdraw: { hash: 'withdraw' },
  'withdraw-connected': { hash: 'withdraw', steps: [['click', 'Connect wallet'], ['wait', 800]] },
  'release-paste': { hash: 'release' },
  'release-first': { hash: RELEASE, recordedAgo: 7200 },
  'release-wait': { hash: RELEASE, recordedAgo: 900, steps: [['click', 'Connect wallet'], ['wait', 1200]] },
  'release-ready': { hash: RELEASE, recordedAgo: 7200, steps: [['click', 'Connect wallet'], ['wait', 1200]] },
  'release-done': { hash: RELEASE, recordedAgo: 7200, steps: [['click', 'Connect wallet'], ['wait', 1200], ['click', 'Release'], ['wait', 4500]] },
  'release-refused': { hash: RELEASE, recordedAgo: 7200, verifyOk: false, steps: [['click', 'Connect wallet'], ['wait', 1200]] },
  start: {},
  nowallet: { noWallet: true, steps: [['click', 'Connect wallet']] },
  chooser: { wallets: 2, steps: [['click', 'Connect wallet']] },
  'wrong-network': { walletChainId: 1n, steps: MAKE },
  form: { steps: MAKE },
  'bad-account': { steps: [...OWN, ['fill', '#acct', '0x5a1e0000000000000000000000000000000c0de5'], ['wait', 600]] },
  'blockchain-key': { steps: [...OWN, ['fill', '#acct', '7a6223abde07c0c278d203973fd233ab3e47b2bfa53c8ed3d740123456789abc'], ['wait', 600]] },
  'own-unclaimable': { steps: [...OWN, ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200]] },
  'own-ack': { steps: [...OWN, ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack']] },
  'save-file': { steps: READY },
  'saved-tick': { steps: [...READY, ['click', 'Save claim file'], ['wait', 300]] },
  'make-ready': { steps: [...READY, ...SAVE] },
  'make-usdc': { usdcBalance: 300_000_000n, steps: [...READY_USDC, ...SAVE] },
  'make-sign': { hangSend: true, steps: [...READY, ...SAVE, ['click', 'Deposit']] },
  'send-fails': { sendRpcFailsOnce: true, steps: [...READY, ...SAVE, ['click', 'Deposit'], ['wait', 600]] },
  'make-done': { finalizedLag: -8, steps: [...READY, ...SAVE, ['click', 'Deposit']] },
  'make-finalizing': { finalizedLag: 400, steps: [...READY, ...SAVE, ['click', 'Deposit']] },
  // the Deposited screen's wait (2026-11-15): just sent, in a block (about 20 min to go), final
  'make-sent': { receiptDelayPolls: 100000, steps: [...READY, ...SAVE, ['click', 'Deposit'], ['wait', 1500]] },
  'make-block': { finalizedLag: 70, steps: [...READY, ...SAVE, ['click', 'Deposit'], ['wait', 2500]] },
  'request-sent': { hash: eth01(), receiptDelayPolls: 100000, steps: [['click', 'Connect wallet'], ['wait', 1500], ['click', 'Deposit'], ['wait', 1500]] },
  'request-block': { hash: eth01(), finalizedLag: 70, steps: [['click', 'Connect wallet'], ['wait', 1500], ['click', 'Deposit'], ['wait', 2500]] },
  'own-done': { finalizedLag: -8, steps: [...READY, ...SAVE, ['click', 'Deposit']] },
  // the deposit desk (bridge fee), when deployments.js names one: every path goes through it
  'desk-own': { steps: [...OWN_PUBLIC, ...SAVE] },
  'desk-own-done': { finalizedLag: -8, steps: [...OWN_PUBLIC, ...SAVE, ['click', 'Deposit']] },
  'desk-private': { steps: [...OWN, ['fill', '#acct', `Private/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200], ...SAVE] },
  'desk-usdc-done': { finalizedLag: -8, usdcBalance: 300_000_000n, steps: [...READY_USDC, ...SAVE, ['click', 'Approve'], ['wait', 2500], ['click', 'Deposit']] },
  'desk-fee-changed': { steps: [...READY, ...SAVE, ['feeBps', 100n], ['click', 'Deposit'], ['wait', 600]] },
  'desk-short': { ethBalance: 10n ** 16n, steps: READY },
  paste: { steps: [['click', 'Paste deposit code']] },
  // the deposit-code path, 0.1 ETH (a Names Wallet code); 2026-11-15 owner bug: the code was lost
  'code-first': { hash: eth01() },                                                               // (a) first paint, no wallet
  'code-connect': { hash: eth01(), steps: [['click', 'Connect wallet'], ['wait', 1500]] },         // (a) then Connect
  'code-hashchange': { steps: [...MAKE, ['hash', eth01()], ['wait', 1500]] },                      // (b) pasted into an open, connected tab
  'code-reconnect': { hash: eth01(), steps: [['click', 'Connect wallet'], ['wait', 1200], ['disconnect'], ['wait', 300], ['click', 'Connect wallet'], ['wait', 1500]] }, // (c)
  'code-wrong-chain': { hash: eth01(), walletChainId: 1n, steps: [['click', 'Connect wallet'], ['wait', 800], ['click', 'Switch to Sepolia'], ['wait', 1500]] }, // (d)
  'code-midfile': { steps: [...OWN, ['fill', '#acct', `Public/${OTHER}`], ['wait', 1200], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200], ['hash', eth01()], ['wait', 1500]] }, // (e)
  'code-bad': { hash: 'logosdep1qyqqqqqqqz4rdfex4avc2kue4zh3nymp9zd9a4sgsu57vrsqqqqxxxxbad' },        // unreadable code
  // LNS names (fake LEZ 0.4 RPC, dev/fake-lez.js; the real Argon2id derivation)
  'name-resolves': { lns: true, steps: [...OWN, ['fill', '#acct', 'alice'], ['wait', 4000], ['tick', '#unclaimable-ack'], ['wait', 300]] },
  'name-no-public': { lns: true, steps: [...OWN, ['fill', '#acct', 'carol'], ['wait', 4000]] },
  'name-changed': { lns: true, steps: [...OWN, ['fill', '#acct', 'alice'], ['wait', 4000], ['tick', '#unclaimable-ack'], ['wait', 300], ['click', 'Continue'], ['wait', 1200], ...SAVE, ['lns-change', 'alice'], ['click', 'Deposit'], ['wait', 4000]] },
  // the amount step (after connecting)
  amount: { steps: MAKE },
  // the account step, before an account is pasted
  account: { steps: OWN },
  // the code path (the Names Wallet made the deposit code)
  ready: { hash: eth(), steps: [['click', 'Connect wallet']] },
  // a request's deposit, done: what a reload of a page-made deposit also shows (no secret in the tab)
  'request-done': { hash: eth(), finalizedLag: -8, steps: [['click', 'Connect wallet'], ['wait', 1500], ['click', 'Deposit'], ['wait', 4000]] },
  'usdc-ready': { hash: usdc(), usdcBalance: 300_000_000n, steps: [['click', 'Connect wallet']] },
  // review C: a link naming a vault not on the page's list is refused outright (the published page)
  unlisted: { hash: eth({ vault: ALT }), altVault: ALT, steps: [['click', 'Connect wallet']] },
  // the development-only path behind DEV.allowUnlistedVaults (off in every published build)
  unknown: { devUnlisted: true, hash: eth({ vault: ALT }), altVault: ALT, steps: [['click', 'Connect wallet']] },
  mainnet: { hash: eth({ chainId: 1n }), walletChainId: 1n, steps: [['click', 'Connect wallet']] },
  'mainnet-wallet': { hash: eth(), walletChainId: 1n, steps: [['click', 'Connect wallet']] },
  frozen: { hash: eth(), frozen: true, steps: [['click', 'Connect wallet']] },
  example: { hash: EXAMPLE, steps: [['click', 'Connect wallet']] },
};

const cfg = SCENARIOS[sc] || SCENARIOS.start;
if (cfg.devUnlisted) DEV.allowUnlistedVaults = true;
// The mock chain gets a desk at the address deployments.js names (none when it is empty), so the
// page meets the same contracts it would on Sepolia.
const DESK = deskOf(DEPLOYMENTS[0]);
const desk = DESK ? { address: DESK, feeBps: 30n } : null; // the live rate (0.3%) since 2026-11-14
const chain = createMockChain({ usdcBalance: cfg.usdcBalance ?? 0n, ethBalance: cfg.ethBalance, finalizedLag: cfg.finalizedLag ?? 70, altVault: cfg.altVault, finalizedSupported: true, desk, receiptDelayPolls: cfg.receiptDelayPolls });
if (cfg.frozen) chain.S.globalFrozen = true;
if (cfg.recordedAgo !== undefined) chain.S.exitRoots.set(ROOT, chain.timeOf(chain.S.head) - BigInt(cfg.recordedAgo));
if (cfg.verifyOk === false) chain.S.verifyOk = false;
const icon = (fill) => 'data:image/svg+xml;base64,' + btoa(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="${fill}"/><path d="M8 20l8-10 8 10z" fill="#fff"/></svg>`);
if (!cfg.noWallet) {
  const n = cfg.wallets || 1;
  for (let i = 0; i < n; i++) {
    const provider = createMockWallet(chain, { walletChainId: cfg.walletChainId ?? 11155111n, behaviour: { hangSend: cfg.hangSend, sendRpcFailsOnce: cfg.sendRpcFailsOnce } });
    const info = Object.freeze({ uuid: `mock-${i}`, name: i === 0 ? 'Test Wallet' : 'Second Wallet', icon: icon(i === 0 ? '#e8831d' : '#555555'), rdns: `dev.mock${i}` });
    const announce = () => window.dispatchEvent(new CustomEvent('eip6963:announceProvider', { detail: Object.freeze({ info, provider }) }));
    window.addEventListener('eip6963:requestProvider', announce);
  }
}
if (cfg.hash) history.replaceState(null, '', `${location.pathname}${location.search}#${cfg.hash}`);

// the mock wallet answers reads, so read through it (the page's default is PublicNode, a real
// Sepolia service the harness must not call)
try {
  localStorage.setItem('logosdep.reads', 'wallet');
} catch {
  /* private mode */
}
// keep the last claim file the page offers, so the screenshot driver can check it leaked nowhere
const makeUrl = URL.createObjectURL.bind(URL);
URL.createObjectURL = (blob) => {
  blob.text().then((t) => (window.__lastFile = t));
  return makeUrl(blob);
};
// LNS scenarios: the page's lookups go to a fake LEZ 0.4 RPC (nothing leaves the machine)
let lez = null;
if (cfg.lns) {
  const { FakeLez } = await import('./fake-lez.js');
  lez = new FakeLez();
  await lez.put('alice', { account: OTHER });
  await lez.put('carol', { npk: new Uint8Array(32).fill(4), vpk: new Uint8Array(1184).fill(5) }); // private payments only
  const realFetch = window.fetch.bind(window);
  window.fetch = (url, init) => (String(url).startsWith('https://lez.nameswallet.app') ? lez.fetch(url, init) : realFetch(url, init));
}
await import('../src/app.js');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
await sleep(600);
for (const [kind, a, b] of cfg.steps || []) {
  if (kind === 'wait') await sleep(a);
  else if (kind === 'feeBps' && desk) desk.feeBps = a; // the desk owner changes the fee after the page's checks
  else if (kind === 'hash') location.hash = a; // a link opened or pasted into this tab (hashchange)
  else if (kind === 'disconnect') document.querySelector('.wallet-row button')?.click(), await sleep(300), [...document.querySelectorAll('dialog button')].find((x) => x.textContent.trim() === 'Disconnect')?.click();
  else if (kind === 'lns-change' && lez) await lez.put(a, { account: '9Ws1J5dhq7jYzGB7m7bQb3kPn1C3s2sA7uFpZ8nX4Vht' }); // the holder points the name elsewhere
  else if (kind === 'fill') {
    const el = document.querySelector(a);
    if (el) {
      el.value = b;
      el.dispatchEvent(new Event('input', { bubbles: true }));
    }
  } else if (kind === 'tick') {
    // wait until the box is shown (the draft's hashing may take a moment under load)
    let el = null;
    for (let i = 0; i < 100 && !el; i++) {
      const x = document.querySelector(a || '#saved-ack');
      if (x && x.offsetParent !== null) el = x;
      else await sleep(50);
    }
    if (el && !el.checked) el.click();
  }
  else if (kind === 'click') {
    let btn = null;
    for (let i = 0; i < 100 && !btn; i++) {
      await sleep(50);
      btn = [...document.querySelectorAll('button, a')].find((x) => x.textContent.trim().startsWith(a) && !x.disabled);
    }
    btn?.click();
  }
  await sleep(250);
}
document.documentElement.dataset.harnessDone = '1';
