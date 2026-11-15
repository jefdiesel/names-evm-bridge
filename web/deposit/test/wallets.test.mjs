// Wallet discovery: EIP-6963 announcements and the legacy window.ethereum, in every order a
// real browser produces.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { discoverWallets } from '../src/lib/providers.js';

function fakeWindow() {
  const listeners = new Map();
  const timers = [];
  const win = {
    addEventListener: (t, fn) => (listeners.get(t) || listeners.set(t, []).get(t)).push(fn),
    removeEventListener: (t, fn) => listeners.set(t, (listeners.get(t) || []).filter((f) => f !== fn)),
    dispatchEvent: (e) => (listeners.get(e.type) || []).slice().forEach((fn) => fn(e)),
    setTimeout: (fn, ms) => timers.push({ fn, ms }) - 1,
    clearTimeout: (i) => (timers[i] = null),
    runTimers: (upTo) => timers.forEach((t) => t && t.ms <= upTo && (t.fn(), (t.ms = Infinity))),
  };
  return win;
}
const provider = (name) => ({ name, request: async () => null, on() {}, removeListener() {} });
const announcer = (win, info, p) => {
  const announce = () => win.dispatchEvent({ type: 'eip6963:announceProvider', detail: { info, provider: p } });
  win.addEventListener('eip6963:requestProvider', announce);
  return announce;
};
const ICON = 'data:image/svg+xml;base64,PHN2Zy8+';
const run = (win) => {
  let latest = [];
  const h = discoverWallets(win, (l) => (latest = l));
  return { h, get list() { return latest; } };
};

test('legacy only: found after the short wait, named from its flags', () => {
  const win = fakeWindow();
  win.ethereum = { ...provider('mm'), isMetaMask: true };
  const r = run(win);
  assert.equal(r.list.length, 0);
  win.runTimers(400);
  assert.deepEqual(r.list.map((w) => [w.id, w.name]), [['legacy', 'MetaMask']]);
});

test('6963 only: announced at once, with its icon and rdns', () => {
  const win = fakeWindow();
  announcer(win, { uuid: 'u1', name: 'Rabby Wallet', icon: ICON, rdns: 'io.rabby' }, provider('r'));
  const r = run(win);
  assert.deepEqual(r.list.map((w) => [w.name, w.rdns, w.icon]), [['Rabby Wallet', 'io.rabby', ICON]]);
});

test('both, same provider: one entry, not two', () => {
  const win = fakeWindow();
  const p = provider('mm');
  win.ethereum = p;
  announcer(win, { uuid: 'u1', name: 'MetaMask', icon: ICON, rdns: 'io.metamask' }, p);
  const r = run(win);
  win.runTimers(5000);
  assert.deepEqual(r.list.map((w) => w.name), ['MetaMask']);
});

test('several wallets, plus a different legacy one: all listed', () => {
  const win = fakeWindow();
  announcer(win, { uuid: 'u1', name: 'MetaMask', icon: ICON, rdns: 'io.metamask' }, provider('a'));
  announcer(win, { uuid: 'u2', name: 'Rabby Wallet', icon: ICON, rdns: 'io.rabby' }, provider('b'));
  win.ethereum = provider('c');
  const r = run(win);
  win.runTimers(5000);
  assert.deepEqual(r.list.map((w) => w.name), ['MetaMask', 'Rabby Wallet', 'Browser wallet']);
});

test('late announce: a wallet that announces after the page asked still appears', () => {
  const win = fakeWindow();
  const r = run(win);
  assert.equal(r.list.length, 0);
  win.dispatchEvent({ type: 'eip6963:announceProvider', detail: { info: { uuid: 'late', name: 'Late Wallet', icon: '', rdns: 'x.late' }, provider: provider('l') } });
  assert.deepEqual(r.list.map((w) => w.name), ['Late Wallet']);
});

test('legacy injected late (in-app browsers): found by a later look or its initialized event', () => {
  const win = fakeWindow();
  const r = run(win);
  win.runTimers(400);
  assert.equal(r.list.length, 0);
  win.ethereum = provider('late');
  win.runTimers(1500);
  assert.deepEqual(r.list.map((w) => w.id), ['legacy']);
  const win2 = fakeWindow();
  const r2 = run(win2);
  win2.ethereum = provider('evt');
  win2.dispatchEvent({ type: 'ethereum#initialized' });
  assert.deepEqual(r2.list.map((w) => w.id), ['legacy']);
});

test('icons: only data:image URIs are kept', () => {
  const win = fakeWindow();
  announcer(win, { uuid: 'u1', name: 'Evil', icon: 'javascript:alert(1)', rdns: 'x' }, provider('e'));
  announcer(win, { uuid: 'u2', name: 'Remote', icon: 'https://evil.example/i.png', rdns: 'y' }, provider('f'));
  const r = run(win);
  assert.deepEqual(r.list.map((w) => w.icon), ['', '']);
});

test('no wallet: an empty list, and nothing breaks', () => {
  const win = fakeWindow();
  const r = run(win);
  win.runTimers(5000);
  assert.deepEqual(r.list, []);
});
