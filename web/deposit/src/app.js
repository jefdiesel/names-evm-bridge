// The page: one card that advances. By default it makes the deposit here (connect, amount,
// Logos account, save the claim file, deposit); a deposit code from the Names Wallet is the second way.
// No innerHTML anywhere: every node is built with the DOM API, so nothing read from the chain,
// a wallet or a request (token symbols, wallet names) can inject markup.
//
// The deposit secret, when made here, lives only in the flow's memory and in the claim file the
// person saves. It is never written to the URL, to storage, or into any request.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

import { createFlow, FINALITY_TYPICAL_SECONDS, bpsText } from './lib/flow.js';
import { discoverWallets, httpProvider } from './lib/providers.js';
import { READ_RPCS, DEPLOYMENTS, DEMO_ACCOUNT, LOGOS_SIDE, chainInfo } from './lib/deployments.js';
import { formatUnits, shortHex, groupHex, aboutDuration } from './lib/units.js';
import { checksumAddress } from './lib/keccak.js';
import { encodeQr, qrPath } from './lib/qr.js';
import { readFragment, madeFragment, compareDigits as digitsOf, claimPath, namesAfter, finalInMinutes, depositedWords } from './lib/entry.js';
import { encodeRequest } from './lib/request.js';
import { claimFileName } from './lib/create.js';
import { WALLET_TEST_BUILD, TESTING_PAGE } from './lib/links.js';
import { compareWords } from './lib/compare-words.js';
import { normalizeName, resolve as lnsResolve, confirmSame, confirmationLine } from './lib/lns.js';
import { deriveName } from './lib/lnsderive.js';

const flow = createFlow();
// Two columns from this width (app.css uses the same breakpoint): the side panel beside the card.
const WIDE = matchMedia('(min-width: 900px)');
const $ = (id) => document.getElementById(id);
const DEP = DEPLOYMENTS[0];
const SEPOLIA = 11155111n;
const ETH = '0x0000000000000000000000000000000000000000';

// ------------------------------------------------------------------ tiny DOM builder

const SVG_NS = 'http://www.w3.org/2000/svg';
function h(tag, attrs, ...kids) {
  const svg = tag.startsWith('svg:');
  const el = svg ? document.createElementNS(SVG_NS, tag.slice(4)) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === false || v == null) continue;
    if (k === 'class') el.setAttribute('class', v);
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'text') el.textContent = v;
    else el.setAttribute(k, v === true ? '' : String(v));
  }
  for (const kid of kids.flat(Infinity)) {
    if (kid == null || kid === false) continue;
    el.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  return el;
}

// ------------------------------------------------------------------ per-browser settings
// Only two things are ever stored: the read setting and the last wallet's rdns
// (never an address, never a secret).

const store = {
  get(k) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  },
  set(k, v) {
    try {
      if (v == null) localStorage.removeItem(k);
      else localStorage.setItem(k, v);
    } catch {
      /* private mode */
    }
  },
};
// Sepolia is read through the page's own pinned RPC (PublicNode) unless the person chose their
// wallet's connection in Settings; either falls back to the other if it does not answer.
let readsVia = store.get('logosdep.reads') === 'wallet' ? 'wallet' : 'page';
const applyReadRpc = () => {
  const rpc = READ_RPCS[0];
  flow.setReadProvider(Object.assign(httpProvider(rpc.url), { label: 'PublicNode' }));
  flow.setReadPrefer(readsVia);
};

// ------------------------------------------------------------------ helpers

const S = () => flow.state;
const unitOf = (token) => DEP.tokens[token]?.unit || DEP.tokens[token]?.symbol || 'tokens';
const amt = (v, s = S()) => (s.token?.decimals != null ? formatUnits(v, s.token.decimals) : String(v));
const sym = (s = S()) => s.token?.unit || s.token?.symbol || '';
const addr = (a) => {
  try {
    return checksumAddress(a);
  } catch {
    return a;
  }
};
const explorer = (kind, v) => (S().chain?.explorer ? `${S().chain.explorer}/${kind}/${v}` : null);
const ext = (href, text) => (href ? h('a', { href, target: '_blank', rel: 'noopener noreferrer' }, text) : text);
const isMobile = () => matchMedia('(pointer: coarse)').matches && /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent);

function copyButton(text, label, word = 'Copy') {
  return h('button', {
    type: 'button',
    class: 'link-btn',
    'aria-label': `Copy ${label}`,
    onclick: async (e) => {
      const b = e.currentTarget;
      try {
        await navigator.clipboard.writeText(text);
        b.textContent = 'Copied';
      } catch {
        b.textContent = 'Select and copy';
      }
      setTimeout(() => (b.textContent = word), 1600);
    },
    text: word,
  });
}

function bitfield(hex, size = 96) {
  const bytes = hex.slice(2).match(/../g).map((b) => parseInt(b, 16));
  const svg = h('svg:svg', { class: 'bits', viewBox: '0 0 16 16', width: size, height: size, role: 'img', 'aria-label': 'The commitment as a 16 by 16 grid of its 256 bits' });
  for (let r = 0; r < 16; r++) {
    for (let c = 0; c < 16; c++) {
      const on = (bytes[r * 2 + (c >> 3)] >> (7 - (c & 7))) & 1;
      svg.append(h('svg:rect', { x: c + 0.08, y: r + 0.08, width: 0.84, height: 0.84, rx: 0.12, class: on ? 'on' : 'off' }));
    }
  }
  return svg;
}

// ------------------------------------------------------------------ keyed sections (keep focus)

const rendered = new Map();
function section(host, id, key, build) {
  const prev = rendered.get(id);
  if (prev && prev.key === key && prev.node.isConnected && prev.node.parentNode === host) return prev.node;
  // moved to another host (the side panel between the card and its own column): never in both
  if (prev && prev.node.isConnected && prev.node.parentNode !== host) prev.node.remove();
  const node = build();
  if (prev && prev.node.isConnected) {
    if (node) prev.node.replaceWith(node);
    else prev.node.remove();
  } else if (node) host.append(node);
  rendered.set(id, { key, node: node || h('span') });
  return node;
}
function order(host, ids) {
  const want = ids.map((id) => rendered.get(id)?.node).filter((n) => n && n.parentNode === host);
  const have = [...host.children].filter((n) => want.includes(n));
  if (want.every((n, i) => have[i] === n)) return;
  for (const n of want) host.append(n);
}
const keyOf = (v) => JSON.stringify(v, (k, x) => (typeof x === 'bigint' ? x.toString() : x));

// ------------------------------------------------------------------ what the person has chosen

const ui = {
  mode: 'make', // 'make' (deposit made here) | 'request' (a deposit code from the Names Wallet) | 'paste'
  // A deposit made on this page and reopened from its link (`#made:logosdep1…`): its claim is
  // the claim file's. A plain `#logosdep1…` is a Names Wallet code: no claim file on that path.
  madeHere: false,
  token: ETH,
  denomIndex: 0,
  dest: DEMO_ACCOUNT ? 'demo' : 'own', // 'demo': the team's demo account (the default where the deployment offers one) | 'own': the visitor's account
  account: '', // what the visitor typed for 'own'
  fileOffered: false, // the Save button was used at least once for this deposit
  // The file path's steps after connecting: 'amount' → 'account' → 'save' (one shown at a time).
  makeStep: 'amount',
};
const destAccount = () => (ui.dest === 'demo' ? DEMO_ACCOUNT : ui.account);
let wallets = [];
let draftTimer = null;
function updateDraft(delay = 0) {
  clearTimeout(draftTimer);
  if (ui.mode !== 'make') return; // a deposit code's state is never replaced by a draft
  // an LNS name in the account box (LNS.md): resolved on LEZ 0.4 after a longer pause (Argon2id)
  const typed = ui.dest === 'own' && !DEMO_ACCOUNT ? normalizeName(ui.account) : null;
  const name = typed?.kind === 'name' ? typed.name : null;
  if (!name) lnsSeq++; // a lookup still running is for text that is gone
  draftTimer = setTimeout(() => {
    if (ui.mode !== 'make') return;
    ui.fileOffered = false;
    if (name) return void lookUpName(name);
    ui.lns = null;
    flow.setDraft({ token: ui.token, denomIndex: ui.denomIndex, account: destAccount() }).catch(() => {});
  }, name ? Math.max(delay, 600) : delay);
}

// ------------------------------------------------------------------ LNS names (src/lib/lns.js)
// The deposit is bound to the account the name gives, never to the name; the name is looked up
// again just before the deposit (confirmSame), and a change stops it. Argon2id runs on the main
// thread (the CSP allows no worker) behind "Checking…", handing control back every 50 ms.
// LNS has no directory: the page can only check the one exact name typed (owner, 2026-11-15).
const derived = new Map(); // name -> Promise<{id, key}>, memory only, for this page's lifetime
const derive = (n) => {
  if (!derived.has(n)) derived.set(n, deriveName(n).catch((e) => (derived.delete(n), Promise.reject(e))));
  return derived.get(n);
};
let lnsSeq = 0;
const rpcFailed = (e) => ({ ok: false, code: 'rpc_error', message: `The LEZ 0.4 network can’t be read now: ${e?.message || e}.` });
async function lookUpName(name) {
  const seq = ++lnsSeq;
  ui.lns = { name, state: 'looking' };
  ui.lnsCheck = null;
  await flow.setDraft({ token: ui.token, denomIndex: ui.denomIndex, account: '' }).catch(() => {});
  render(S());
  say(`Checking ${name} on LEZ 0.4…`);
  let r;
  try {
    r = await lnsResolve(name, { derive });
  } catch (e) {
    r = rpcFailed(e);
  }
  if (seq !== lnsSeq) return;
  if (!r.ok) {
    ui.lns = { name, state: 'refused', r };
    render(S());
    say(r.message);
    return;
  }
  ui.lns = { name, state: 'ok', r };
  await flow.setDraft({ token: ui.token, denomIndex: ui.denomIndex, account: r.account }).catch(() => {});
  if (seq === lnsSeq) render(S());
}
/** The deposit, with a name's account checked again first. */
async function depositNow() {
  const s = S();
  const l = ui.lns;
  if (s.createdHere && l?.state === 'ok') {
    ui.lnsCheck = 'checking';
    render(S());
    let c;
    try {
      c = await confirmSame(l.r, { derive });
    } catch (e) {
      c = rpcFailed(e);
    }
    if (!c.ok || c.accountId !== S().draft?.accountId) {
      ui.lnsCheck = { message: c.ok ? `The LNS name ${l.name} changed since you checked it. Nothing was sent. Check the name with its holder.` : c.message };
      render(S());
      say(ui.lnsCheck.message);
      return;
    }
    ui.lnsCheck = null;
  }
  return flow.deposit();
}

// ------------------------------------------------------------------ the wallet, top right of the card

function walletRow(s) {
  if (!s.account) return null;
  const w = wallets.find((x) => x.id === s.wallet?.id);
  const onSepolia = s.walletChainId === SEPOLIA;
  return h(
    'div',
    { class: 'wallet-row' },
    h(
      'button',
      { type: 'button', class: 'btn btn-quiet btn-small chip', onclick: openWalletPanel, 'aria-haspopup': 'dialog', title: `${s.wallet?.name}: ${addr(s.account)}` },
      w?.icon ? h('img', { src: w.icon, alt: '', width: 18, height: 18 }) : h('span', { class: 'dot', 'aria-hidden': 'true' }),
      h('span', { class: 'mono' }, shortHex(addr(s.account))),
      h('span', { class: onSepolia ? 'chip-net' : 'chip-net wrong' }, onSepolia ? 'Sepolia' : 'Wrong network'),
    ),
  );
}

// ------------------------------------------------------------------ the card

function renderCard(s) {
  const host = $('card');
  const r = s.request;
  const refused = REFUSED.includes(s.error?.code);
  const sent = !!s.tx || ['deposit-wait', 'finalizing', 'done', 'failed'].includes(s.step);
  const connected = !!s.account;
  const onSepolia = s.walletChainId === SEPOLIA;

  section(host, 'wallet', keyOf([s.wallet?.id, s.account, s.walletChainId, wallets.length]), () => walletRow(s));
  section(host, 'head', keyOf([ui.mode, ui.madeHere, refused, s.step === 'done', s.step === 'finalizing', sent, r && !s.createdHere ? encodeRequest(r) : null, s.token?.unit]), () => head(s, { refused, sent }));

  // the steps (before sending): "Step n of N", the current one open, the others one line each.
  // Built once per step, so typing in the account box keeps focus.
  const showSteps = !sent && !refused && s.step !== 'failed' && !(ui.mode === 'request' && ui.madeHere);
  const st = showSteps ? stepState(s, { connected, onSepolia }) : null;
  section(host, 'form', st ? keyOf(stepKey(s, st)) : 'none', () => (st ? flowSteps(s, st) : null));
  if (st && st.current === 'amount') syncAmount();
  if (st && st.current === 'account') syncAccount(s);

  const after = s.step === 'finalizing' || s.step === 'done';
  // its own section, so the heading (which takes focus) is not rebuilt at every finality read
  const prog = after ? progressLine(s) : null;
  section(host, 'progress', prog ? keyOf([s.finality?.finalized, s.tx?.blockNumber]) : 'none', () => (after ? progressLine(s) : null));
  // The side panel: one node, in its own column on wide screens, in the card on narrow ones
  // (so the reading order is the visual order and nothing is said twice).
  const wide = WIDE.matches;
  const sideCol = $('side');
  const sideHost = wide && sideCol ? sideCol : host;
  const sideKey = keyOf([wide, after, sent, refused, ui.mode, ui.madeHere, ui.makeStep, connected, onSepolia, s.step, s.finality, s.tx?.hash, s.tx?.blockNumber, ui.token, ui.denomIndex, s.draft?.accountId, s.data?.total, s.data?.desk]);
  section(sideHost, 'side', refused ? 'none' : sideKey, () => (refused ? null : sidePanel(s, { after, sent, connected, onSepolia })));
  if (sideCol) sideCol.hidden = !(wide && sideCol.childElementCount);
  section(host, 'next', after ? keyOf([ui.madeHere, s.step, s.finality, s.tx?.blockNumber, s.draft?.claimable, s.createdHere]) : 'none', () => (after ? nextSteps(s) : null));
  section(host, 'action', keyOf(actionKey(s, { connected, onSepolia, sent, refused })), () => action(s, { connected, onSepolia, sent, refused }));
  section(host, 'notes', keyOf([ui.mode, sent, refused, !!s.account, s.data?.desk, ui.makeStep]), () => notes(s, { sent, refused }));
  section(host, 'details', keyOf([refused, r ? encodeRequest(r) : null, s.checks, s.anon, s.fee, s.tx?.hash, s.tx?.blockNumber, s.tx?.feePaid, s.step === 'done', !!s.draft?.k, s.walletRpc, s.error?.raw, s.data?.desk, s.data?.bridgeFee]), () => (r && !refused ? details(s) : null));
  order(host, after ? ['wallet', 'head', 'progress', 'side', 'next', 'action', 'notes', 'details'] : ['wallet', 'head', 'form', 'action', 'side', 'notes', 'details']);
}

// Requests the page refuses outright, before any read: another chain, or a vault not on its list.
const REFUSED = ['unsupported-chain', 'unlisted-vault'];

/** The commitment's first and last 6 hex digits, large: the person compares them with the wallet. */
/** The deposit code's four compare words (src/lib/compare-words.js; the Names Wallet shows the same
 * four), large, with the line to check them; the commitment's first and last 6 hex digits, which the
 * wallet also shows, behind "Digits". The words come from SHA-256 (async): the box is filled in
 * place a moment after it is drawn. Memoised per commitment, in memory only. */
const wordsOf = new Map();
function compareDigits(commitment) {
  const { first, last } = digitsOf(commitment);
  const words = h('p', { class: 'compare-words', id: 'compare-words' }, '…');
  const box = h(
    'div',
    { class: 'compare', role: 'group', 'aria-labelledby': 'compare-words compare-line' },
    words,
    h('p', { class: 'compare-line', id: 'compare-line' }, COMPARE_LINE),
    more('Digits', h('p', {}, 'The same deposit as its commitment’s first and last 6 hex digits, also shown in your Names Wallet:'), h('p', { class: 'compare-digits mono', 'aria-label': `starts ${first.split('').join(' ')}, ends ${last.split('').join(' ')}` }, `${first} … ${last}`)),
  );
  if (!wordsOf.has(commitment)) wordsOf.set(commitment, compareWords(commitment));
  wordsOf.get(commitment).then(
    (w) => (words.textContent = w.words_text),
    () => (words.textContent = `${first} … ${last}`),
  );
  return box;
}
const COMPARE_LINE = 'Check these four words match your Names Wallet. If they differ, stop.';
const COMPARE_MORE = 'Your Names Wallet shows the same four words (and the same first and last 6 digits) for this deposit. If they differ, stop: this code would deposit to someone else.';

function head(s, { refused, sent }) {
  const wrap = h('div', { class: 'card-head' });
  const title = (t) => h('h1', { id: 'card-h', tabindex: '-1' }, t);
  if (refused) wrap.append(title(s.error.code === 'unlisted-vault' ? 'Unknown vault' : 'Sepolia only'));
  else if (s.step === 'done' || s.step === 'finalizing') {
    // The wait is said where the person looks first (owner, 2026-11-15: "nothing mentions that 20 minutes")
    const path = claimPath({ createdHere: s.createdHere, madeHere: ui.madeHere });
    wrap.append(
      title('Deposited'),
      h('p', { class: 'sub' }, `${amt(s.request.amount)} ${sym(s)} is in the bridge’s vault on Sepolia.`),
      h('p', { class: 'wait-line' }, h('strong', {}, depositedWords(path, s.step === 'done'))),
    );
  }
  else if (sent && s.step !== 'failed') wrap.append(title(`Depositing ${amt(s.request.amount)} ${sym(s)}`));
  else if (ui.mode === 'paste') wrap.append(title('Open a deposit code')); // the step says how
  else if (ui.mode === 'request' && s.request && ui.madeHere) wrap.append(title(`Deposit ${amt(s.request.amount)} ${sym(s)}`), h('p', { class: 'sub' }, 'The deposit made on this page, found again from its link.'));
  else if (ui.mode === 'request' && s.request) wrap.append(title(`Deposit ${amt(s.request.amount)} ${sym(s)}`), h('p', { class: 'sub' }, 'From your Names Wallet’s deposit code. This page never sees its secret.'), compareDigits(s.request.commitment));
  else wrap.append(title('Bridge Sepolia ETH to Logos'), h('p', { class: 'sub' }, 'Send test ETH from Sepolia to an account on the bridge test network.'));
  return wrap;
}

// --- the form: token, amount, LEZ account

// Which account (no demo account, v3): the one rule is that the account's keys must be in the
// person's LEZ 0.4 wallet (Names Wallet 0.17 keeps one apart from the LEZ 0.3 one), or held by the
// person for the claim tool. A LEZ 0.4 wallet restored from the same recovery phrase has the same
// public account ids as LEZ 0.3 (checked by the wallet session, 2026-11-14); one created new has
// different ones. The words follow what the 0.17 wallet shows: the Wallet page's network switch
// (LEZ 0.4) and its "Bridge test network" card, whose "Public account" line has Copy (the full id;
// Settings → Accounts shows ids shortened, without Copy). That build's card can also list LEZ 0.3
// accounts and the switch does not always refresh it, hence "LEZ 0.4 selected when you copy".
// Restoring versus making a LEZ 0.4 wallet is the wallet's own flow: the page does not steer
// people into new keys.
const ACCOUNT_WHICH = 'Use a public account whose keys are in your LEZ 0.4 wallet (Names Wallet 0.17), or another LEZ 0.4 account whose keys you hold.';
const ACCOUNT_HINT = DEMO_ACCOUNT
  ? 'The account ID your wallet shows (Public/… or Private/… is fine).'
  : 'In Names Wallet 0.17: on the Wallet page choose LEZ 0.4, then use Copy next to Public account in the Bridge test network card. Make sure LEZ 0.4 is the network selected when you copy. An account that is only in your LEZ 0.3 wallet cannot claim this deposit. (A LEZ 0.4 wallet restored from the same recovery phrase has the same public accounts; one created new has different ones.)';
// What the page cannot know: it never talks to the wallet. The claim does: the Names Wallet's
// "Claim a bridge deposit" checks the account against its own, and the lez_bridge module it calls
// refuses an account the wallet has not given (modules/lez_bridge/rust-lib/src/relayed.rs, relayed_gates).
const NO_WALLET_CHECK = 'This page cannot check which wallet holds an account; the Names Wallet checks it when you claim.';
const WRONG_LINE = 'Paste the account ID, not a key. A wrong account can’t be fixed later: a deposit to it could never be claimed.';
// The wording follows LOGOS_SIDE in deployments.js, the one place that says where the Logos
// side runs and whether own accounts can claim yet.
const WHERE_LINE = `The coins arrive in an account on ${LOGOS_SIDE.network}.`;
// The order of things, before the deposit (no demo account): what happens now, what happens later.
// Nothing here tells the person to do the later step yet; the Deposited screen does (CLAIM_STEPS).
const ORDER_NOW = (unit) => `you deposit ${unit} here, on Sepolia.`;
// The claim runs inside the Names Wallet (logos-names-wallet-next 5f5f6df: the LEZ 0.4 card's
// "Claim a bridge deposit", a screen of that title, "Choose the claim file", then "Claim").
const IN_WALLET = 'in the Names Wallet (Wallet → LEZ 0.4 → “Claim a bridge deposit”)';
const ORDER_LATER = LOGOS_SIDE.selfClaim
  ? `you claim it into your LEZ 0.4 account on ${LOGOS_SIDE.network}, ${IN_WALLET}.`
  : `you claim it into your LEZ 0.4 account on ${LOGOS_SIDE.network}, ${IN_WALLET}. That wallet build (0.17) is not released yet.`;
const DEMO_LINE = DEMO_ACCOUNT ? `The test ETH goes to the team’s demo account, ${DEMO_ACCOUNT.slice(0, 8)}…${DEMO_ACCOUNT.slice(-4)}, on the bridge test network. It shows the whole flow; it is not yours to spend.` : '';
// A claim is signed by the receiving account and needs the claim file (review C): nobody else
// can claim an own-account deposit, and the team can claim a demo deposit only with the file.
// Secondary facts about an own-account claim (shown small, before and after the deposit).
const OWN_FEE = 'The bridge pays the claim’s fee (the bridge fee covers it), so your account needs no LEZ.';
const PUBLIC_LINK = 'A claim to a public account links the Sepolia deposit to that account publicly.';
// After the deposit: the claim, as steps, in the Names Wallet (labels as above).
const CLAIM_WHEN = (final) =>
  LOGOS_SIDE.selfClaim
    ? final ? `Claim it on ${LOGOS_SIDE.network} with the claim file you saved:` : `When the deposit is final, claim it on ${LOGOS_SIDE.network} with the claim file you saved:`
    : final
      ? `Once Names Wallet 0.17 is released, claim it on ${LOGOS_SIDE.network} with the claim file you saved:`
      : `When the deposit is final and Names Wallet 0.17 is released, claim it on ${LOGOS_SIDE.network} with the claim file you saved:`;
const CLAIM_STEPS = ['Open the Names Wallet in Basecamp.', 'Wallet → LEZ 0.4 → “Claim a bridge deposit”.', '“Choose the claim file”: the one you saved here.', 'Press “Claim” and approve it.'];
const claimHowto = () => h('ol', { class: 'claim-howto' }, CLAIM_STEPS.map((t) => h('li', {}, t)));
const DEMO_CLAIM = 'The demo account is the team’s, and a claim needs both your claim file and that account’s signature: the team can claim this deposit only with your claim file. Without it, the deposit stays in the vault.';
const OWN_TICK_WHICH = 'The keys of this account are in my LEZ 0.4 wallet (Names Wallet 0.17), or I hold them myself.';
// An LNS name gives its holder's published account, which may not be the depositor's: whoever's
// account it is claims with the claim file.
const OWN_TICK_NAME = `This is the account the name gives. Its keys are in my LEZ 0.4 wallet (Names Wallet 0.17), or I will give the claim file to the name’s holder. I understand the claim waits for that wallet build.`;
const LNS_WHICH = 'An LNS name works only if it is registered on LEZ 0.4 and its holder published a public account; the deposit is bound to that account, not to the name.';
const LNS_PRIVACY = 'The page checks the exact name you type. Names cannot be searched or listed. Checking takes about a second, and only the record’s account is sent to the network, never the name.';
const OWN_TICK = DEMO_ACCOUNT
  ? (LOGOS_SIDE.selfClaim ? 'This is my account' : 'This is my account. I understand the claim waits for that wallet build.')
  : LOGOS_SIDE.selfClaim ? OWN_TICK_WHICH : `${OWN_TICK_WHICH} I understand the claim waits for that wallet build.`;
// The deposit desk (bridge fee): the owner's words.
const FEE_COVERS = 'It covers the claim’s fee on the bridge test network.';

/** The site's network label (partials/README.md §4), once per screen, above the action. */
const onLabel = () => h('p', { class: 'site-on' }, 'Runs on ', h('b', {}, 'Sepolia'), ' and ', h('b', {}, 'LEZ 0.4'), ' ', h('span', {}, 'bridge test network. Not on LEZ 0.3.'));

function segmented(name, label, options, onPick) {
  return h(
    'fieldset',
    { class: 'field' },
    h('legend', {}, label),
    h('div', { class: 'seg', role: 'group', 'data-seg': name }, options.map(([value, text]) => h('button', { type: 'button', class: 'seg-btn', 'data-value': value, 'aria-pressed': 'false', onclick: () => onPick(value) }, text))),
  );
}

// --- the steps: "Step n of N", one sentence and the control; the rest behind "More"

// Two paths, each a short numbered list. The file path makes the secret here (a claim file);
// the code path deposits a Names Wallet's deposit code (the wallet keeps the secret).
const FILE_STEPS = [
  { id: 'connect', title: 'Connect your wallet' },
  { id: 'amount', title: 'Choose the amount' },
  { id: 'account', title: 'Choose where it goes' },
  { id: 'save', title: 'Save the claim file, then deposit' },
];
const CODE_STEPS = [
  { id: 'code', title: 'Open the deposit code' },
  { id: 'connect', title: 'Connect your wallet' },
  { id: 'deposit', title: 'Compare and deposit' },
];
// One plain sentence per step (what to do now). Everything else is under "More".
const STEP_LINE = {
  connect: 'Connect the Ethereum wallet you deposit from, on Sepolia.',
  amount: 'Choose the token and the amount to send from Sepolia.',
  account: DEMO_ACCOUNT ? 'Choose the LEZ account the coins go to.' : null, // v3: ACCOUNT_WHICH
  save: LOGOS_SIDE.selfClaim
    ? 'Save the claim file before you deposit: the claim needs it, later, in the Names Wallet.'
    : 'Save the claim file before you deposit: the claim needs it, later, in Names Wallet 0.17 (not released yet).',
  code: 'Paste the code or link from your Names Wallet, or scan its QR code.',
  deposit: 'Check the four words above match your Names Wallet. If they differ, stop.',
};

/** Which step is current, for either path. */
function stepState(s, { connected, onSepolia }) {
  const code = ui.mode !== 'make';
  const list = code ? CODE_STEPS : FILE_STEPS;
  let current;
  if (code) current = ui.mode === 'paste' || !s.request ? 'code' : !connected || !onSepolia ? 'connect' : 'deposit';
  else current = !connected || !onSepolia ? 'connect' : ui.makeStep;
  return { code, list, current, index: list.findIndex((x) => x.id === current), connected, onSepolia };
}

/** What makes the steps' DOM change (never the typed account while its step is open). */
function stepKey(s, st) {
  return [st.code, st.current, st.connected, st.onSepolia, s.account, st.current !== 'amount' ? [ui.token, ui.denomIndex] : null, st.current === 'save' ? ui.account : null, s.request?.commitment && st.code ? s.request.commitment : null, ui.dest];
}

/** A closed disclosure (native <details>, keyboard and screen-reader friendly): the why, the fees'
 * breakdown, the notes. Each has its own name, so a list of them is not five "More"s. */
function more(label, ...kids) {
  const k = kids.filter(Boolean);
  return k.length ? h('details', { class: 'more' }, h('summary', {}, label), ...k) : document.createComment('');
}

function goStep(id) {
  ui.makeStep = id;
  if (id === 'account' && ui.lnsCheck?.message) {
    ui.lnsCheck = null;
    updateDraft(); // the name is looked up again
  }
  render(S());
  setTimeout(() => $('step-h')?.focus(), 0);
}

/** The one-line summary of a finished step. */
function stepSummary(s, id) {
  const t = DEP.tokens[ui.token];
  if (id === 'connect') return `Wallet ${shortHex(addr(s.account))}, on Sepolia`;
  if (id === 'amount') return `Amount: ${formatUnits(BigInt(t.denoms[ui.denomIndex]), t.decimals)} ${t.unit || t.symbol}`;
  if (id === 'account' && ui.lns?.state === 'ok' && s.draft?.accountId) return `To: ${ui.lns.r.name} → ${s.draft.accountId.slice(0, 6)}…${s.draft.accountId.slice(-6)}`;
  if (id === 'account') return ui.dest === 'demo' ? 'To: the team’s demo account' : s.draft?.accountId ? `To: ${s.draft.accountLabel ? s.draft.accountLabel + '/' : ''}${s.draft.accountId.slice(0, 6)}…${s.draft.accountId.slice(-6)}` : 'To: your LEZ 0.4 account';
  if (id === 'code') return s.request ? `Deposit code: ${amt(s.request.amount)} ${sym(s)}` : 'Deposit code';
  return '';
}

function flowSteps(s, st) {
  const n = st.list.length;
  const items = st.list.map((x, i) => {
    if (i < st.index) {
      const change = !st.code && (x.id === 'amount' || x.id === 'account') ? h('button', { type: 'button', class: 'link-btn', 'aria-label': `Change: ${x.title.toLowerCase()}`, onclick: () => goStep(x.id) }, 'Change') : null;
      return h('li', { class: 'done' }, h('span', { class: 'step-mark', 'aria-hidden': 'true' }, '✓'), h('span', { class: 'sr-only' }, `Step ${i + 1}, done: `), h('span', {}, stepSummary(s, x.id)), change);
    }
    if (i > st.index) return h('li', { class: 'todo' }, h('span', { class: 'step-mark', 'aria-hidden': 'true' }, String(i + 1)), h('span', {}, h('span', { class: 'sr-only' }, `Step ${i + 1}, later: `), x.title));
    return h('li', { class: 'current', 'aria-current': 'step' }, h('span', { class: 'step-mark', 'aria-hidden': 'true' }, String(i + 1)), h('div', {}, h('h2', { id: 'step-h', tabindex: '-1' }, `Step ${i + 1} of ${n}: ${x.title}`), ...stepBody(s, x.id)));
  });
  return h('section', { class: 'flow', 'aria-labelledby': 'step-h' }, h('ol', { class: 'flow-steps' }, items));
}

function stepBody(s, id) {
  if (id === 'amount') return amountControls();
  if (id === 'account') return accountControls();
  if (id === 'deposit') return [h('p', { class: 'step-line' }, STEP_LINE.deposit), more('Why compare', h('p', {}, COMPARE_MORE))];
  if (id === 'save') return [h('p', { class: 'step-line' }, STEP_LINE.save)];
  return [h('p', { class: 'step-line' }, STEP_LINE[id])];
}

function amountControls() {
  const tokens = Object.entries(DEP.tokens).map(([a, t]) => [a, t.symbol === 'ETH' ? 'Sepolia ETH' : 'Demo USDC']);
  return [
    h('p', { class: 'step-line' }, STEP_LINE.amount),
    segmented('token', 'Token', tokens, (v) => {
      ui.token = v;
      ui.denomIndex = 0;
      updateDraft();
      render(S());
    }),
    h('p', { class: 'hint', id: 'token-hint' }),
    segmented('amount', 'Amount', [], (v) => {
      ui.denomIndex = Number(v);
      updateDraft();
      render(S());
    }),
  ];
}

function accountControls() {
  const input = h('input', {
    id: 'acct',
    type: 'text',
    inputmode: 'text',
    autocomplete: 'off',
    autocapitalize: 'off',
    spellcheck: 'false',
    placeholder: DEMO_ACCOUNT ? 'Paste a LEZ account' : 'Public/… or a name like alice',
    'aria-describedby': 'acct-which acct-status',
    oninput: (e) => {
      ui.account = e.target.value;
      updateDraft(150);
      if (ui.lns) {
        ui.lns = null; // a new text: the last lookup no longer applies
        syncAccount(S());
      }
    },
  });
  input.value = ui.account;
  return [
    h('p', { class: 'step-line', id: 'acct-which' }, DEMO_ACCOUNT ? STEP_LINE.account : ACCOUNT_WHICH),
    DEMO_ACCOUNT
      ? h('div', { class: 'seg', role: 'group', 'aria-label': 'Where it goes on Logos', 'data-seg': 'dest' }, [
          h('button', { type: 'button', class: 'seg-btn', 'data-value': 'demo', 'aria-pressed': String(ui.dest === 'demo'), onclick: () => pickDest('demo') }, 'Use the demo account'),
          h('button', { type: 'button', class: 'seg-btn', 'data-value': 'own', 'aria-pressed': String(ui.dest === 'own'), onclick: () => pickDest('own') }, 'Use my own account'),
        ])
      : null,
    h('p', { class: 'hint', id: 'demo-note', hidden: !DEMO_ACCOUNT }, DEMO_LINE),
    h(
      'div',
      { class: 'own', id: 'own-box', hidden: ui.dest !== 'own' },
      h('label', { for: 'acct' }, DEMO_ACCOUNT ? 'Your LEZ account' : 'Your LEZ 0.4 account or LNS name'),
      input,
      h('p', { class: 'hint', id: 'acct-status' }),
      h(
        'div',
        { class: 'callout caution', id: 'unclaimable', role: 'group', 'aria-labelledby': 'unclaimable-h', hidden: true },
        h('strong', { id: 'unclaimable-h' }, DEMO_ACCOUNT ? '! Check that this is your account' : '! Check that your LEZ 0.4 wallet holds this account’s keys'),
        h('p', { id: 'acct-lns', hidden: true }),
        h('p', { class: 'acct-confirm mono', id: 'acct-confirm' }),
        h('label', { class: 'ack' }, h('input', { type: 'checkbox', id: 'unclaimable-ack', onchange: (e) => flow.ackUnclaimable(e.target.checked) }), h('span', { id: 'unclaimable-tick' }, OWN_TICK)),
      ),
    ),
    more(
      'Which account, and why',
      h('p', {}, ACCOUNT_HINT),
      DEMO_ACCOUNT ? null : h('p', {}, LNS_WHICH),
      DEMO_ACCOUNT ? null : h('p', {}, LNS_PRIVACY),
      h('p', {}, WRONG_LINE),
      DEMO_ACCOUNT ? null : h('p', {}, NO_WALLET_CHECK),
      h('p', {}, `${OWN_FEE} ${PUBLIC_LINK}`),
      DEMO_ACCOUNT ? h('p', {}, WHERE_LINE) : h('ol', { class: 'order', 'aria-label': 'What happens when' }, h('li', {}, h('b', {}, 'Now: '), h('span', { id: 'order-now' }, ORDER_NOW('Sepolia ETH'))), h('li', {}, h('b', {}, 'Later: '), ORDER_LATER)),
    ),
  ];
}

function pickDest(dest) {
  if (ui.dest === dest) return;
  ui.dest = dest;
  updateDraft();
  render(S());
  say(dest === 'demo' ? 'Demo account chosen.' : 'Your own account: paste it below.');
  if (dest === 'own') setTimeout(() => $('acct')?.focus(), 0);
}

/** Bring the form's controls in line with the choices without rebuilding it. */
// The one live region (#live). Two messages within half a second are joined, so the second does not
// silently replace the first before a screen reader has read it.
let liveAt = 0;
function say(text) {
  const live = $('live');
  const now = Date.now();
  live.textContent = now - liveAt < 500 && live.textContent ? `${live.textContent} ${text}` : text;
  liveAt = now;
}

// A mistake in the account box is shown under it (aria-describedby); it is also read out once
// through the live region, after typing has paused, so a screen-reader user hears it without
// leaving the box. The same mistake is not repeated; a new one is.
let acctErrSaid = null;
let acctErrTimer = null;
function sayAccountError(err) {
  const key = err ? err.code || err.message : null;
  if (!key) {
    clearTimeout(acctErrTimer);
    acctErrSaid = null;
    return;
  }
  if (key === acctErrSaid) return;
  clearTimeout(acctErrTimer);
  acctErrTimer = setTimeout(() => {
    acctErrSaid = key;
    say(err.message);
  }, 700);
}

function syncAmount() {
  const t = DEP.tokens[ui.token];
  const segTok = document.querySelector('[data-seg="token"]');
  segTok?.querySelectorAll('.seg-btn').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.value === ui.token)));
  const segAmt = document.querySelector('[data-seg="amount"]');
  if (segAmt && segAmt.dataset.token !== ui.token) {
    segAmt.dataset.token = ui.token;
    segAmt.replaceChildren(
      ...t.denoms.map((d, i) =>
        h('button', { type: 'button', class: 'seg-btn', 'data-value': i, 'aria-pressed': 'false', 'aria-label': `${formatUnits(BigInt(d), t.decimals)} ${t.unit || t.symbol}`, onclick: () => ((ui.denomIndex = i), updateDraft(), render(S())) }, formatUnits(BigInt(d), t.decimals)),
      ),
    );
  }
  segAmt?.querySelectorAll('.seg-btn').forEach((b) => b.setAttribute('aria-pressed', String(Number(b.dataset.value) === ui.denomIndex)));
  const legend = segAmt?.closest('fieldset')?.querySelector('legend');
  if (legend) legend.textContent = `Amount in ${t.unit || t.symbol}`;
  const tokHint = $('token-hint');
  if (tokHint) {
    tokHint.textContent = t.symbol === 'ETH' ? '' : 'A test token only the team can hand out. Ask them for some.';
    tokHint.hidden = t.symbol === 'ETH';
  }
}

function syncAccount(s) {
  const t = DEP.tokens[ui.token];
  const orderNow = $('order-now');
  if (orderNow) orderNow.textContent = ORDER_NOW(t.unit || t.symbol);
  document.querySelector('[data-seg="dest"]')?.querySelectorAll('.seg-btn').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.value === ui.dest)));
  const own = ui.dest === 'own';
  const demoNote = $('demo-note');
  if (demoNote) demoNote.hidden = own || !DEMO_ACCOUNT;
  const ownBox = $('own-box');
  if (ownBox) ownBox.hidden = !own;
  const status = $('acct-status');
  const l = own ? ui.lns : null;
  const err = !own ? null : l?.state === 'refused' ? { code: l.r.code, message: l.r.message } : s.draft?.accountError;
  const input = $('acct');
  const valid = own && !err && s.draft?.accountId && (!l || l.state === 'ok');
  if (status && input) {
    status.textContent = err ? err.message : l?.state === 'looking' ? `Checking ${l.name} on LEZ 0.4…` : valid ? `${l ? `${l.r.forwardedFrom ? `${l.r.forwardedFrom} → ` : ''}${l.r.name} → ` : ''}LEZ account ${s.draft.accountLabel ? s.draft.accountLabel + '/' : ''}${s.draft.accountId.slice(0, 8)}…${s.draft.accountId.slice(-4)}.` : '';
    status.hidden = !err && !valid && l?.state !== 'looking';
    status.classList.toggle('error', !!err);
    status.classList.toggle('ok', !!valid);
    input.setAttribute('aria-invalid', err ? 'true' : 'false');
    sayAccountError(err);
  }
  const block = $('unclaimable');
  const showBlock = !!(valid && !s.draft.teamHeld);
  const conf = $('acct-confirm');
  if (conf && showBlock && conf.dataset.id !== s.draft.accountId) {
    conf.dataset.id = s.draft.accountId;
    conf.replaceChildren(...groupedId(s.draft.accountId));
  }
  if (block && block.hidden === showBlock) {
    block.hidden = !showBlock;
    if (showBlock) say('Check that this is your account, then tick the box.');
  }
  const lnsLine = $('acct-lns');
  if (lnsLine) {
    lnsLine.hidden = !(valid && l);
    lnsLine.textContent = valid && l ? confirmationLine(l.r) : '';
  }
  const tick = $('unclaimable-tick');
  if (tick) tick.textContent = valid && l ? OWN_TICK_NAME : OWN_TICK;
  const ack = $('unclaimable-ack');
  if (ack) ack.checked = !!s.unclaimableAck;
}

/** The account step is done: a valid account the team holds, or the person's own, ticked. */
const accountOk = (s) => ui.dest === 'demo' ? !!s.draft?.accountId : !!(s.draft?.accountId && !s.draft.accountError && (!ui.lns || ui.lns.state === 'ok') && (s.draft.teamHeld || s.unclaimableAck));

/** An account id in groups of 4, its first and last 6 characters in bold, for checking by eye. */
function groupedId(id) {
  const first = id.slice(0, 6);
  const last = id.slice(-6);
  const mid = id.slice(6, -6).match(/.{1,4}/g) || [];
  return [h('b', {}, first), ' ', mid.join(' '), ' ', h('b', {}, last)];
}

// --- the one action

function actionKey(s, o) {
  return [ui.mode, ui.dest, ui.makeStep, ui.lnsCheck, ui.lns?.state, o, s.step, s.busy, s.error, s.notice, s.needsApproval, s.saved, ui.fileOffered, s.example, s.vaultAck, s.draft?.commitment, !!s.draft?.accountId, s.draft?.claimable, s.unclaimableAck, s.draft?.accountError?.code, s.data?.total, s.data?.desk, s.tx?.hash, s.approveHash, wallets.length, s.readOnly, s.finality?.mode, s.walletRpc];
}

// A real link (reader modes keep links, not buttons) that the script handles in place.
const hashLink = (text, href, act) =>
  h('a', {
    href,
    onclick: (e) => {
      e.preventDefault();
      act();
    },
  }, text);

const statusLine = (kind, text) => h('p', { class: `status ${kind}`, role: kind === 'bad' ? 'alert' : 'status' }, h('span', { class: 'status-mark', 'aria-hidden': 'true' }), h('span', {}, text));
const primary = (label, onclick, { disabled = false, why = null } = {}) => [h('button', { type: 'button', class: 'btn btn-primary btn-wide', disabled, onclick, 'aria-describedby': why ? 'why' : null }, label), why ? h('p', { class: 'why', id: 'why' }, why) : null].filter(Boolean);

// The deposit desk's fee and total describe the action button, so a screen reader reads them
// with it (they are also announced once when the deposit is ready, in render()).
function action(s, flags) {
  const p = actionPanel(s, flags);
  const btn = p.querySelector('.btn-primary');
  if (btn && p.querySelector('#bridge-fee')) btn.setAttribute('aria-describedby', [btn.getAttribute('aria-describedby'), 'bridge-fee', 'bridge-total'].filter(Boolean).join(' '));
  return p;
}

function actionPanel(s, { connected, onSepolia, sent, refused }) {
  const p = h('section', { class: 'action' });
  const r = s.request;
  if (s.notice) p.append(statusLine(s.notice.kind === 'error' ? 'bad' : s.notice.kind === 'success' ? 'ok' : 'info', s.notice.text));

  if (refused) {
    p.append(statusLine('bad', s.error.message), h('button', { type: 'button', class: 'btn btn-quiet btn-wide', onclick: () => setMode('paste') }, 'Paste deposit code'), h('p', { class: 'alt' }, hashLink('Start over', './', startOver)));
    return p;
  }
  if (!sent && s.step !== 'failed') p.prepend(onLabel());
  if (ui.mode === 'paste') return pasteView(p);

  // after sending
  if (s.step === 'deposit-sign' || s.step === 'approve-sign') {
    p.append(statusLine('busy', `Waiting for your wallet: confirm the ${s.step === 'approve-sign' ? 'approval' : 'deposit'} in ${s.wallet?.name || 'it'}.`));
    if (s.data?.desk) p.append(h('p', { class: 'small' }, s.step === 'approve-sign' ? 'The approval is for the deposit desk ' : 'The contract is the deposit desk ', h('span', { class: 'mono' }, groupHex(addr(s.data.desk))), `, for ${amt(s.data.total)} ${sym()}.`));
    return p;
  }
  if (s.step === 'approve-wait') {
    p.append(statusLine('busy', 'Approval sent. Waiting for Sepolia to include it.'));
    return p;
  }
  if (s.step === 'deposit-wait') {
    p.append(statusLine('busy', 'Sent. Waiting for the first block (about 12 seconds).'), h('p', { class: 'small' }, 'Transaction ', ext(explorer('tx', s.tx.hash), shortHex(s.tx.hash, 10, 6))));
    return p;
  }
  if (s.step === 'finalizing' || s.step === 'done') {
    // One save only, before the deposit (the claim finds the leaf from the vault's logs; a file
    // with no transaction claims the same). The copy with the transaction is optional.
    if (s.createdHere && s.saved)
      p.append(
        statusLine('ok', 'Your claim file is already saved; nothing more to do here.'),
        more('Optional copy of the claim file', h('p', {}, h('button', { type: 'button', class: 'link-btn', onclick: () => saveFile(true) }, 'Optional: a copy with the transaction added'), '; not needed to claim. Keep it private, like the first.')),
      );
    return p;
  }
  if (s.step === 'failed') {
    p.append(statusLine('bad', s.error?.message || 'The deposit failed.'), h('button', { type: 'button', class: 'btn btn-quiet btn-wide', onclick: () => flow.retry() }, 'Check again'));
    return p;
  }

  // before sending
  if (!connected) {
    p.append(...primary('Connect wallet', () => connectWallet()));
    if (ui.mode === 'make') p.append(codeEntry());
    else p.append(h('p', { class: 'alt' }, hashLink('Use a different code', '#request', () => setMode('paste')), ' · ', hashLink('Start over', './', startOver)));
    return p;
  }
  if (!onSepolia) {
    p.append(statusLine('info', `Your wallet is on ${chainInfo(s.walletChainId).name}.`), ...primary('Switch to Sepolia', () => flow.switchNetwork(), { disabled: s.busy }));
    return p;
  }
  // the file path's first steps: one "Continue" each
  if (ui.mode === 'make' && ui.makeStep === 'amount') {
    p.append(...primary('Continue', () => goStep('account')), codeEntry());
    return p;
  }
  if (ui.mode === 'make' && ui.makeStep === 'account') {
    const ok = accountOk(s);
    const why = ok ? null : ui.lns?.state === 'looking' ? `Checking ${ui.lns.name}…` : !s.draft?.accountId ? (DEMO_ACCOUNT ? 'Paste your LEZ account above, or use the demo account.' : 'Paste your LEZ 0.4 account or type an LNS name above.') : 'Tick the box above.';
    p.append(...primary('Continue', () => goStep('save'), { disabled: !ok, why }));
    return p;
  }
  if (ui.mode === 'make' && !r) {
    p.append(...primary('Save claim file', null, { disabled: true, why: 'Go back to step 3 and paste your account.' }));
    return p;
  }

  // the wallet's own Sepolia connection failed before or during a signature: nothing was sent
  if (s.walletRpc) {
    const again = s.needsApproval ? `Approve ${amt(s.data.total)} ${sym()}` : `Deposit ${amt(s.data.total)} ${sym()}`;
    p.append(
      statusLine('bad', `Nothing was sent: your wallet’s Sepolia connection did not answer (${s.walletRpc.words}). Try again; if it fails again, change the Sepolia RPC URL in your wallet’s network settings to https://ethereum-sepolia-rpc.publicnode.com.`),
      ...primary(`Try again: ${again}`, () => (s.needsApproval ? flow.approve() : depositNow())),
      h('button', { type: 'button', class: 'btn btn-quiet btn-wide', onclick: () => flow.addSepolia() }, 'Add a working Sepolia connection'),
      h('p', { class: 'small' }, 'Wallets that already have Sepolia may ignore that button; then change the URL by hand.'),
    );
    return p;
  }

  // checks
  if (s.step === 'checking' || s.busy) p.append(statusLine('busy', 'Checking the vault and your balance…'));
  else if (s.step === 'blocked') {
    p.append(statusLine('bad', s.error?.message || 'A check failed.'));
    if (s.error?.code === 'insufficient-balance' && s.token?.howToGet) p.append(h('p', { class: 'small' }, s.token.howToGet));
    p.append(h('button', { type: 'button', class: 'btn btn-quiet btn-wide', onclick: () => flow.retry() }, 'Check again'));
    return p;
  }
  if (s.step !== 'ready') return p;

  const unknown = !s.deployment;
  if (unknown) {
    p.append(
      h(
        'div',
        { class: 'callout danger', role: 'alert' },
        h('strong', {}, 'Unknown vault. Stop and check.'),
        h('p', {}, h('span', { class: 'mono' }, addr(r.vault)), ` on Sepolia is not on this page’s list. Continue only if you compared it with an address the bridge team published.`),
        h('label', { class: 'ack' }, h('input', { type: 'checkbox', checked: s.vaultAck, onchange: (e) => flow.acknowledgeVault(e.target.checked) }), 'I checked this address.'),
      ),
    );
  }
  if (s.example) {
    p.append(...primary(`Deposit ${amt(r.amount)} ${sym()}`, null, { disabled: true, why: 'This is the published example. It can never be claimed, so the page won’t send it.' }));
    return p;
  }
  const total = s.data.total;
  const blockedByAck = unknown && !s.vaultAck;
  // visible: what the wallet will send in all; the fee's breakdown is under "More"
  p.append(totalLine(s));
  const moreBox = () => more('Fee and claim details', ...(s.data.desk ? feeLines(s) : []), s.needsApproval ? h('p', {}, `Approve allows the ${s.data.desk ? 'deposit desk' : 'vault'} to take exactly ${amt(total)} ${sym()}, once. Then you deposit.`) : null, s.createdHere && !DEMO_ACCOUNT ? h('p', {}, `Later: ${ORDER_LATER}`) : null);

  // made here: an account that can't claim yet (LOGOS_SIDE) needs the tick first, then the claim file
  if (s.createdHere && !s.draft?.teamHeld && !s.unclaimableAck) {
    p.append(...primary('Save claim file', null, { disabled: true, why: 'Go back to step 3 and tick the box.' }));
    return p;
  }
  if (s.createdHere && !s.saved) {
    p.append(statusLine('ok', 'Ready.'));
    if (!ui.fileOffered) {
      p.append(...primary('Save claim file', () => saveFile(false)), moreBox());
      return p;
    }
    p.append(
      h('label', { class: 'ack' }, h('input', { type: 'checkbox', id: 'saved-ack', onchange: (e) => flow.markSaved(e.target.checked) }), h('span', {}, h('strong', {}, 'I saved the claim file. '), 'Without it the deposit cannot be claimed. Nobody can recover it for you.')),
      h('p', { class: 'small' }, h('button', { type: 'button', class: 'link-btn', onclick: () => saveFile(false) }, 'Save again')),
      ...primary(s.needsApproval ? `Approve ${amt(total)} ${sym()}` : `Deposit ${amt(total)} ${sym()}`, null, { disabled: true, why: 'Tick the box once the file is saved.' }),
      moreBox(),
    );
    return p;
  }

  p.append(statusLine('ok', 'Ready.'));
  if (s.needsApproval) {
    p.append(...primary(`Approve ${amt(total)} ${sym()}`, () => flow.approve(), { disabled: blockedByAck, why: blockedByAck ? 'Tick the box above first.' : null }));
  } else {
    if (ui.lnsCheck === 'checking') p.append(statusLine('busy', `Checking ${ui.lns?.name} on LEZ 0.4 again…`));
    else if (ui.lnsCheck?.message) p.append(statusLine('bad', ui.lnsCheck.message));
    p.append(...primary(`Deposit ${amt(total)} ${sym()}`, () => depositNow(), { disabled: blockedByAck || ui.lnsCheck === 'checking' || !!ui.lnsCheck?.message, why: blockedByAck ? 'Tick the box above first.' : ui.lnsCheck?.message ? 'Go back to step 3 to check the name again.' : null }));
  }
  p.append(moreBox());
  return p;
}

/** One visible line: what the wallet sends in all (the deposit, any fees, then the network fee). */
function totalLine(s) {
  const d = s.data;
  const parts = [`${amt(d.denom)} deposit`];
  if (d.fee > 0n) parts.push(`${amt(d.fee)} vault fee`);
  if (d.desk) parts.push(`${amt(d.bridgeFee)} bridge fee at ${bpsText(d.feeBps)}`);
  return h('p', { class: 'fee-line', id: 'bridge-total' }, h('strong', {}, `You send ${amt(d.total)} ${sym()}`), ` (${parts.join(' + ')}), plus the Sepolia network fee.`);
}

/** The bridge fee and the total, before any wallet prompt (deposit desk on). Plain text, no colour-only signal. */
function feeLines(s) {
  const d = s.data;
  const parts = [`${amt(d.denom)} deposit`];
  if (d.fee > 0n) parts.push(`${amt(d.fee)} vault fee`);
  parts.push(`${amt(d.bridgeFee)} bridge fee`);
  return [
    h('p', { id: 'bridge-fee' }, h('strong', {}, `Bridge fee (${bpsText(d.feeBps)}): ${amt(d.bridgeFee)} ${sym()}.`), ' ', FEE_COVERS),
    h('p', {}, `Your wallet will ${s.token.isEth ? 'send' : 'approve, then send,'} ${amt(d.total)} ${sym()} in all: ${parts.join(' + ')}. Plus the Sepolia network fee.`),
  ];
}

/** The fee and the total in one sentence, for the live region. */
function feeWords(s) {
  const d = s.data;
  return `Bridge fee (${bpsText(d.feeBps)}): ${amt(d.bridgeFee)} ${sym()}. Your wallet will send ${amt(d.total)} ${sym()} in all.`;
}

/** The way in for a deposit code from the Names Wallet: prominent, beside making one here. */
function codeEntry() {
  return h('a', { class: 'btn btn-quiet btn-wide', href: '#request', onclick: (e) => { e.preventDefault(); setMode('paste'); } }, 'Paste deposit code');
}

function pasteView(p) {
  const ta = h('textarea', { id: 'req-input', spellcheck: 'false', autocapitalize: 'off', autocomplete: 'off', placeholder: 'logosdep1…', 'aria-label': 'Deposit code', 'aria-invalid': S().requestError ? 'true' : null, 'aria-describedby': 'req-err' });
  if (S().requestText) ta.value = S().requestText;
  p.append(
    h(
      'form',
      {
        class: 'paste',
        onsubmit: (e) => {
          e.preventDefault();
          openRequest(ta.value);
        },
      },
      ta,
      S().requestError ? h('p', { class: 'hint error', id: 'req-err', role: 'alert' }, S().requestError.message) : h('span', { id: 'req-err' }),
      h('button', { type: 'submit', class: 'btn btn-primary btn-wide' }, 'Open code'),
      'BarcodeDetector' in window && navigator.mediaDevices?.getUserMedia ? h('button', { type: 'button', class: 'btn btn-quiet btn-wide', onclick: openScanner }, 'Scan QR code') : null,
    ),
    h('p', { class: 'alt' }, hashLink('Start over', './', startOver)),
  );
  return p;
}

function notes(s, { sent, refused }) {
  if (refused || ui.mode === 'paste' || sent) return null;
  const lines = h('div', { class: 'notes' });
  const dep = s.deployment || DEP;
  if (dep.demoNote) lines.append(h('p', { class: 'locked' }, dep.demoShort || dep.demoNote), more('About this test vault', dep.demoShort ? h('p', {}, dep.demoNote) : null));
  lines.append(h('p', {}, 'Public on Sepolia: your address, amount and time. Not public: who receives it on Logos. ', h('a', { href: 'about/privacy/' }, 'Privacy')));
  lines.append(h('p', {}, hashLink('Settings', '#settings', openSettings), ' · ', h('a', { href: TESTING_PAGE }, 'Testing this bridge')));
  return lines;
}

// --- after the deposit: what happens next, the current step marked

const isDemo = (s) => !!DEMO_ACCOUNT && s.draft?.accountId === DEMO_ACCOUNT;

/** The wallet test build: one link text everywhere (src/lib/links.js has the address). */
const walletLink = () => h('a', { href: WALLET_TEST_BUILD, 'data-link': 'wallet-test-build' }, 'Get the Names Wallet test build for LEZ 0.4');

/** The side panel: before the deposit what happens and the wallet link; after it the status. */
function sidePanel(s, { after, sent, connected, onSepolia }) {
  const side = h('aside', { class: 'side-in', 'aria-labelledby': 'side-h' });
  if (after) {
    side.append(h('h2', { id: 'side-h' }, 'Status'), statusList(s));
  } else if (sent) {
    side.setAttribute('aria-label', 'Links'); // the card says it was sent; only the links here
    side.removeAttribute('aria-labelledby');
  } else {
    const code = ui.mode !== 'make';
    side.append(
      h('h2', { id: 'side-h' }, 'What happens'),
      h(
        'ol',
        { class: 'what' },
        h('li', {}, 'You deposit on Sepolia.'),
        h('li', {}, 'It becomes final on Sepolia: up to about 20 minutes.'),
        h('li', {}, code ? 'Your Names Wallet shows it and offers the claim (LEZ 0.4).' : 'You claim it in your Names Wallet (LEZ 0.4).'),
      ),
    );
    // a visual summary of what the steps already show (hidden from screen readers: said in the card)
    if (!code && connected && onSepolia && ui.makeStep !== 'amount') {
      const t = DEP.tokens[ui.token];
      const rows = [['Amount', `${formatUnits(BigInt(t.denoms[ui.denomIndex]), t.decimals)} ${t.unit || t.symbol}`]];
      if (ui.makeStep === 'save' && s.draft?.accountId) rows.push(['To', `${s.draft.accountId.slice(0, 6)}…${s.draft.accountId.slice(-6)}`]);
      if (ui.makeStep === 'save' && s.data?.total != null) rows.push(['You send', `${amt(s.data.total)} ${sym()}`]);
      side.append(h('dl', { class: 'summary', 'aria-hidden': 'true' }, rows.flatMap(([k, v]) => [h('dt', {}, k), h('dd', {}, v)])));
    }
  }
  // after the deposit the claim section (in the card) carries the wallet link: not twice
  if (!after) side.append(h('p', { class: 'side-links' }, walletLink()));
  side.append(h('p', { class: 'side-links' }, h('a', { href: TESTING_PAGE }, 'Testing this bridge')));
  return side;
}

/** After the deposit: Sent → Final on Sepolia → Claim in Names Wallet. */
function statusList(s) {
  const done = s.step === 'done';
  const names = claimPath({ createdHere: s.createdHere, madeHere: ui.madeHere }) === 'names';
  const item = (state, title, ...body) => h('li', { class: state, 'aria-current': state === 'current' ? 'step' : null }, h('span', { class: 'step-mark', 'aria-hidden': 'true' }, state === 'done' ? '✓' : ''), h('div', {}, h('strong', {}, title), h('span', { class: 'sr-only' }, state === 'done' ? ' (done)' : state === 'current' ? ' (now)' : ' (next)'), ...body));
  return h(
    'ol',
    { class: 'steps' },
    item('done', 'Sent', h('p', {}, ext(explorer('tx', s.tx.hash), `Transaction ${shortHex(s.tx.hash, 8, 6)} on ${new URL(s.chain.explorer).hostname}`))),
    item(done ? 'done' : 'current', done ? 'Final on Sepolia' : 'Becoming final on Sepolia', done ? h('p', {}, 'It can no longer be reversed.') : finalityView(s)),
    item(done ? 'current' : 'todo', names ? 'Claim in your Names Wallet' : 'Claim in Names Wallet'),
  );
}

function nextSteps(s) {
  const done = s.step === 'done';
  const own = s.createdHere && !s.draft?.teamHeld;
  // Not made in this tab: a Names Wallet code (the wallet holds the secret and offers the claim;
  // no claim file on this path), or a deposit made here and reopened from its `#made:` link.
  const path = claimPath({ createdHere: s.createdHere, madeHere: ui.madeHere });
  const keep = h('p', { class: 'keep' }, h('strong', {}, 'Keep the claim file you saved.'), ' Without it the deposit cannot be claimed.');
  const claim = path === 'names'
    ? [h('p', {}, namesAfter(LOGOS_SIDE.network))]
    : path === 'file'
      ? [h('p', {}, `Made on this page: ${CLAIM_WHEN(done)}`), claimHowto(), keep]
      : own
        ? [h('p', {}, CLAIM_WHEN(done)), claimHowto(), keep, more('Fee and privacy', h('p', {}, `${OWN_FEE} ${PUBLIC_LINK}`))]
        : [h('p', {}, DEMO_CLAIM)];
  const dep = s.deployment;
  return h(
    'section',
    { class: 'next', 'aria-labelledby': 'next-h' },
    h('h2', { id: 'next-h' }, path === 'names' ? 'Claim in your Names Wallet' : 'Claim in Names Wallet'),
    ...claim,
    h('p', { class: 'wallet-link' }, walletLink()),
    dep?.demoNote ? h('p', { class: 'locked' }, dep.demoShort || dep.demoNote) : null,
    dep?.demoShort ? more('About this test vault', h('p', {}, dep.demoNote)) : null,
  );
}

// --- details: everything technical, closed by default

function details(s) {
  const r = s.request;
  const d = s.deployment;
  const rows = [
    ['Network', `${s.chain.name} (chain id ${r.chainId})`],
    ['Vault', [h('span', { class: 'mono' }, groupHex(addr(r.vault))), ' ', ext(explorer('address', r.vault), 'Explorer'), h('span', { class: 'sub' }, d ? d.label : 'Not on this page’s list')]],
    ['Token', s.token.isEth ? `${sym(s)}, the test network’s own coin` : `${sym(s)}, ${s.token.name}`],
    ['Vault fee', s.data?.fee == null ? 'Read from the vault' : s.data.fee === 0n ? 'None' : `${amt(s.data.fee)} ${sym(s)}`],
    ...(s.data?.desk
      ? [
          ['Bridge fee', `${amt(s.data.bridgeFee)} ${sym(s)} (${bpsText(s.data.feeBps)}). ${FEE_COVERS}`],
          ['Deposit desk', [h('span', { class: 'mono' }, groupHex(addr(s.data.desk))), ' ', ext(explorer('address', s.data.desk), 'Explorer'), h('span', { class: 'sub' }, 'Forwards the deposit to the vault in the same transaction and keeps the bridge fee.')]],
        ]
      : []),
    ['Network fee', s.tx?.feePaid != null ? `${ethAmount(s.tx.feePaid)} (paid)` : s.fee ? `About ${ethAmount(s.fee.cost)}` : 'After the checks'],
    ['Same amount', crowdLine(s)],
    ['Commitment', [h('span', { class: 'mono' }, shortHex(r.commitment, 10, 8)), ' ', copyButton(r.commitment, 'commitment')]],
  ];
  if (s.tx?.hash) rows.push(['Transaction', ext(explorer('tx', s.tx.hash), shortHex(s.tx.hash, 10, 8))]);
  if (s.walletRpc) rows.push(['Wallet error', h('span', { class: 'mono' }, s.walletRpc.raw)]);
  if (s.error?.raw) rows.push(['Read error', h('span', { class: 'mono' }, s.error.raw)]);
  if (s.tx?.blockNumber) rows.push(['Block', `${s.tx.blockNumber}`], ['Deposit', s.anon?.total ? `${Number(s.tx.leafIndex) + 1} of ${s.anon.total} in this vault` : `${Number(s.tx.leafIndex) + 1}`]);
  const box = h(
    'details',
    { class: 'details' },
    h('summary', {}, 'Technical details'),
    h('dl', { class: 'facts' }, rows.flatMap(([k, v]) => [h('dt', {}, k), h('dd', {}, v)])),
    h('div', { class: 'grid-row' }, bitfield(r.commitment), h('p', { class: 'small' }, s.createdHere ? 'The commitment, drawn as its 256 bits. It seals your LEZ account and secret; neither can be read from it.' : 'The commitment from your Names Wallet, drawn as its 256 bits. Your wallet can draw the same grid for comparison.')),
    s.checks.some((c) => c.status !== 'pending') ? h('ul', { class: 'checks', 'aria-label': 'Checks' }, s.checks.map(checkItem)) : null,
    h('p', { class: 'small' }, h('a', { href: 'about/security/' }, 'Security'), ' · ', h('a', { href: 'about/verify/' }, 'Verify this page')),
  );
  if (!s.createdHere && !s.tx) {
    const qr = h('details', { class: 'handoff' }, h('summary', {}, 'Continue on your phone'));
    qr.addEventListener('toggle', () => {
      if (!qr.open || qr.querySelector('svg')) return;
      const url = `${location.origin}${location.pathname}#${encodeRequest(r)}`;
      const code = encodeQr(url, { ecl: 'M' });
      const n = code.size + 8;
      qr.append(h('svg:svg', { viewBox: `0 0 ${n} ${n}`, role: 'img', 'aria-label': 'QR code of this deposit link', 'shape-rendering': 'crispEdges' }, h('svg:path', { d: qrPath(code, 4) })), h('p', { class: 'small' }, 'Scan it, then open the link in your wallet app’s browser.'));
    });
    box.append(qr);
  }
  return box;
}

/** A Sepolia ETH amount for people: 4 decimals at most, never "0". */
function ethAmount(wei) {
  if (wei > 0n && wei < 10n ** 14n) return 'less than 0.0001 Sepolia ETH';
  return `${formatUnits(wei, 18, { maxFraction: 4 })} Sepolia ETH`;
}

function crowdLine(s) {
  if (!s.anon) return 'After the checks';
  const n = Number(s.anon.same);
  if (s.tx?.blockNumber) return n <= 1 ? `${n} deposit of this amount so far, so yours is not hidden among others yet.` : `${n} deposits of this amount so far: a claim on Logos could be any of them.`;
  return n === 0 ? 'None of this amount yet, so yours would not be hidden among others.' : `${n} earlier deposit${n === 1 ? '' : 's'} of this amount: yours would be one of ${n + 1}.`;
}

function checkItem(c) {
  const marks = { pass: '✓', fail: '×', warn: '!', action: '•' };
  const words = { pass: 'passed', fail: 'failed', warn: 'warning', action: 'action needed', pending: 'not run yet', skip: 'skipped', running: 'running' };
  return h('li', {}, h('span', { class: `mark ${c.status}`, 'aria-hidden': 'true' }, marks[c.status] || ''), h('span', {}, c.label, h('span', { class: 'sr-only' }, `: ${words[c.status]}.`), c.detail ? h('span', { class: 'detail' }, c.detail) : null));
}

/** One calm line that follows the finality read (the `finalized` tag); nothing when the wallet's
 * node can't report it (then the sentence above stands alone). */
function progressLine(s) {
  const f = s.finality || {};
  if (s.step === 'done' || f.mode !== 'tag' || f.finalized == null || !s.tx?.blockNumber) return null;
  const mins = finalInMinutes(s.tx.blockNumber, f.finalized);
  return h('p', { class: 'small progress-line', role: 'status' }, mins > 0 ? `In a block · final in about ${mins} min` : 'In a block · final now');
}

function finalityView(s) {
  const f = s.finality || {};
  const tx = s.tx;
  let pct = 0;
  let label;
  if (f.mode === 'estimate') {
    pct = Math.min(100, Math.round(((f.elapsed || 0) / FINALITY_TYPICAL_SECONDS) * 100));
    label = `${aboutDuration(f.elapsed || 0).replace('about ', '')} since it was included (estimate)`;
  } else {
    const fin = f.finalized;
    const start = f.startFinalized ?? fin;
    if (fin != null && tx.blockNumber > start) pct = Math.max(0, Math.min(100, Number(((fin - start) * 100n) / (tx.blockNumber - start))));
    const left = fin == null ? null : tx.blockNumber > fin ? tx.blockNumber - fin : 0n;
    // the bridge module's rule (finalInMinutes), so the page and the Names Wallet agree
    label = left == null ? 'Waiting for finality…' : left === 0n ? 'Final' : `Final in about ${finalInMinutes(tx.blockNumber, fin)} min`;
  }
  const bar = h('span');
  bar.style.width = `${pct}%`;
  return h('div', { class: 'finality' }, h('div', { class: 'meter', role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct, 'aria-label': 'Progress to finality' }, bar), h('p', { class: 'small', 'aria-live': 'polite' }, label));
}

// ------------------------------------------------------------------ wallets

function rememberWallet(w) {
  store.set('logosdep.wallet', w ? w.rdns || w.id : null);
}

async function connectWallet(id) {
  if (!id) {
    if (wallets.length === 1) id = wallets[0].id;
    else return openChooser();
  }
  closeDialogs();
  const w = wallets.find((x) => x.id === id);
  await flow.connect(id);
  if (S().account) {
    rememberWallet(w);
    if (ui.mode === 'make') updateDraft();
  }
}

function closeDialogs() {
  for (const d of document.querySelectorAll('dialog[open]')) d.close();
}

function openChooser() {
  const dlg = $('wallets');
  dlg.replaceChildren();
  const list = h('div', { class: 'wallet-list' });
  if (wallets.length) {
    for (const w of wallets) list.append(h('button', { type: 'button', class: 'btn btn-quiet btn-wide wallet-btn', onclick: () => connectWallet(w.id) }, w.icon ? h('img', { src: w.icon, alt: '', width: 28, height: 28 }) : h('span', { class: 'dot big', 'aria-hidden': 'true' }), w.name));
  } else if (isMobile()) {
    const target = `${location.host}${location.pathname}${ui.mode === 'request' && location.hash.startsWith('#logosdep1') ? location.hash : ''}`;
    list.append(
      h('p', {}, 'Open this page in your wallet app’s browser.'),
      h('a', { class: 'btn btn-primary btn-wide', href: `https://link.metamask.io/dapp/${target}` }, 'Open in MetaMask'),
      h('p', { class: 'small' }, 'Another wallet: open its browser and go to ', h('span', { class: 'mono' }, `${location.host}${location.pathname}`), ' ', copyButton(`${location.origin}${location.pathname}`, 'page address')),
    );
  } else {
    list.append(h('p', {}, 'No wallet found in this browser.'), h('p', { class: 'small' }, 'Install a wallet extension, such as MetaMask or Rabby, then reload this page.'));
  }
  dlg.append(h('h2', { id: 'wallets-title' }, wallets.length ? 'Connect a wallet' : 'No wallet yet'), list, h('div', { class: 'row' }, h('button', { type: 'button', class: 'btn btn-quiet', onclick: () => dlg.close() }, 'Close')));
  dlg.showModal();
  dlg.querySelector('button, a')?.focus();
}

function openWalletPanel() {
  const s = S();
  const dlg = $('wallets');
  dlg.replaceChildren(
    h('h2', { id: 'wallets-title' }, s.wallet?.name || 'Wallet'),
    h('p', { class: 'mono' }, addr(s.account)),
    h('p', { class: 'small' }, s.walletChainId === SEPOLIA ? 'On Sepolia.' : `On ${chainInfo(s.walletChainId).name}. This page works on Sepolia only.`),
    h(
      'div',
      { class: 'row' },
      h('button', { type: 'button', class: 'btn btn-quiet', onclick: () => dlg.close() }, 'Close'),
      h(
        'button',
        {
          type: 'button',
          class: 'btn btn-quiet',
          onclick: () => {
            rememberWallet(null);
            flow.disconnect();
            dlg.close();
          },
        },
        'Disconnect',
      ),
    ),
  );
  dlg.showModal();
}

// ------------------------------------------------------------------ the claim file

function saveFile(updated) {
  const file = flow.claimFile();
  if (!file) return;
  const blob = new Blob([JSON.stringify(file, null, 2) + '\n'], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = h('a', { href: url, download: claimFileName(file.commitment) });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
  if (!updated) {
    ui.fileOffered = true;
    render(S());
  }
}

// ------------------------------------------------------------------ modes and the URL

function setMode(mode) {
  ui.mode = mode;
  render(S());
  $('card-h')?.focus();
}

function startOver() {
  flow.clear();
  ui.mode = 'make';
  ui.fileOffered = false;
  ui.makeStep = 'amount';
  ui.lns = null;
  ui.lnsCheck = null;
  history.replaceState(null, '', `${location.pathname}${location.search}`);
  draftAsked = null;
  render(S());
  $('card-h')?.focus();
}

function openRequest(text) {
  enterCode(text);
  if (ui.mode === 'request' && S().request) {
    // the request is public; keep it in the fragment so a reload resumes (never the query)
    history.replaceState(null, '', `${location.pathname}${location.search}#${encodeRequest(S().request)}`);
  }
  render(S());
  $('card-h')?.focus();
}

function fromHash() {
  const f = readFragment(location.hash);
  if (f.kind === 'none') return;
  // the no-script fallbacks of two in-page links
  if (f.kind === 'paste') return void (ui.mode = 'paste');
  if (f.kind === 'settings') return void setTimeout(openSettings, 0);
  // a deposit made here, reopened from its own `#made:` link: its claim is the claim file's
  enterCode(f.text, f.madeHere);
}

/** Into the deposit-code path, whatever was on the page before: the code's amount and account are
 * fixed, so nothing of the make-a-deposit-here path (amount, account, tick) is kept or asked for.
 * A code that can't be read stays on the code path (the paste box, with why), never falls back. */
function enterCode(text, madeHere = false) {
  clearTimeout(draftTimer);
  ui.madeHere = madeHere;
  ui.makeStep = 'amount';
  ui.lns = null;
  ui.lnsCheck = null;
  ui.fileOffered = false;
  ui.mode = 'request';
  if (!flow.load(text)) ui.mode = REFUSED.includes(S().error?.code) ? 'request' : 'paste';
}

// After a deposit made here is sent, the address bar gets its public facts only (the deposit
// code's format, marked `made:` so a reload knows the claim file claims it), so a reload can find
// the deposit again. The secret is in the claim file and nowhere else.
let linked = null;
function linkSent(s) {
  if (!s.createdHere || !s.tx?.hash || linked === s.request.commitment) return;
  linked = s.request.commitment;
  history.replaceState(null, '', `${location.pathname}${location.search}${madeFragment(encodeRequest(s.request))}`);
}

// ------------------------------------------------------------------ settings and scanner

function openSettings() {
  const dlg = $('settings');
  dlg.replaceChildren();
  const opts = [
    { id: 'page', label: 'PublicNode (default)', hint: 'Sees your IP address, the vault being read and your address.' },
    { id: 'wallet', label: 'Your wallet’s connection', hint: 'Whatever Sepolia service your wallet uses.' },
  ];
  dlg.append(
    h(
      'form',
      {
        method: 'dialog',
        onsubmit: (e) => {
          e.preventDefault();
          readsVia = new FormData(e.target).get('reads') === 'wallet' ? 'wallet' : 'page';
          store.set('logosdep.reads', readsVia === 'wallet' ? 'wallet' : null);
          applyReadRpc();
          dlg.close();
        },
      },
      h('h2', { id: 'settings-title' }, 'Read Sepolia through'),
      h('fieldset', { class: 'plain' }, h('legend', { class: 'sr-only' }, 'Read Sepolia through'), opts.map((o) => h('label', { class: 'choice' }, h('input', { type: 'radio', name: 'reads', value: o.id, checked: readsVia === o.id }), h('span', {}, o.label, h('span', { class: 'hint' }, o.hint))))),
      h('p', { class: 'small' }, 'If one does not answer, the page tries the other. Sending always goes through your wallet.'),
      h('div', { class: 'row' }, h('button', { type: 'button', class: 'btn btn-quiet', onclick: () => dlg.close() }, 'Cancel'), h('button', { type: 'submit', class: 'btn btn-primary' }, 'Save')),
    ),
  );
  dlg.showModal();
}

async function openScanner() {
  const dlg = $('scanner');
  dlg.replaceChildren();
  const video = h('video', { class: 'scan', playsinline: true, muted: true, 'aria-label': 'Camera view' });
  const status = h('p', { class: 'small', role: 'status' }, 'Starting the camera…');
  let stream = null;
  let stop = false;
  const close = () => {
    stop = true;
    stream?.getTracks().forEach((t) => t.stop());
    dlg.close();
  };
  dlg.append(h('h2', { id: 'scanner-title' }, 'Scan a deposit code'), video, status, h('div', { class: 'row' }, h('button', { type: 'button', class: 'btn btn-quiet', onclick: close }, 'Cancel')));
  dlg.addEventListener('close', () => ((stop = true), stream?.getTracks().forEach((t) => t.stop())), { once: true });
  dlg.showModal();
  try {
    stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
    video.srcObject = stream;
    await video.play();
    status.textContent = 'Point the camera at the QR code. The picture stays on this device.';
    const detector = new window.BarcodeDetector({ formats: ['qr_code'] });
    while (!stop) {
      const codes = await detector.detect(video).catch(() => []);
      if (codes.length) {
        close();
        openRequest(codes[0].rawValue);
        return;
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } catch (e) {
    status.textContent = `The camera did not start (${e.name || e}). Paste the request instead.`;
  }
}

// ------------------------------------------------------------------ render

let lastStep = null;
const STEP_WORDS = { ready: 'Ready.', blocked: 'A check failed.', 'deposit-sign': 'Waiting for your wallet.', 'deposit-wait': 'Sent. Waiting for the first block.', finalizing: 'Deposited. Final on Sepolia in up to about 20 minutes.', done: 'Deposit final on Sepolia.', failed: 'The deposit failed.' };

/** The tab title carries the wait, so a person who switches tabs still sees it. */
function docTitle(s) {
  const tail = 'Names Wallet bridge (Sepolia testnet)';
  if (s.step === 'done') return `Final: ${amt(s.request.amount)} ${sym(s)} · ${tail}`;
  if (s.tx?.blockNumber) {
    const f = s.finality || {};
    const mins = f.mode === 'tag' && f.finalized != null ? finalInMinutes(s.tx.blockNumber, f.finalized) : null;
    return `Deposited · ${mins ? `final in about ${mins} min` : 'final in up to about 20 min'} · ${tail}`;
  }
  return 'Bridge Sepolia ETH to Logos · Names Wallet (Sepolia testnet)';
}

let draftAsked = null;
function render(s) {
  if (s.request && !s.createdHere && ui.mode === 'make') ui.mode = 'request';
  // a connected wallet on Sepolia with nothing chosen yet: make the default draft (the demo account where offered)
  if (ui.mode === 'make' && s.account && s.walletChainId === SEPOLIA && !s.request && !s.draft && draftAsked !== s.account) {
    draftAsked = s.account;
    updateDraft();
  }
  renderCard(s);
  linkSent(s);
  document.title = docTitle(s);
  if (s.step !== lastStep) {
    if (lastStep !== null && STEP_WORDS[s.step]) {
      // not "Ready." while the can't-be-claimed-yet box still waits for its tick
      const held = s.step === 'ready' && $('unclaimable') && !$('unclaimable').hidden && !s.unclaimableAck;
      if (!held) say(s.step === 'ready' && s.data?.desk ? `${STEP_WORDS.ready} ${feeWords(s)}` : STEP_WORDS[s.step]);
      if (['blocked', 'done', 'failed', 'finalizing'].includes(s.step) && !document.querySelector('dialog[open]')) $('card-h')?.focus({ preventScroll: true });
    }
    lastStep = s.step;
  }
}

flow.subscribe(render);
WIDE.addEventListener('change', () => render(S()));
const remembered = store.get('logosdep.wallet');
const tried = new Set();
discoverWallets(window, (list) => {
  wallets = list;
  flow.setWallets(list);
  // A returning visitor's wallet reconnects without a prompt (eth_accounts only), and only
  // the wallet they chose last time.
  for (const w of list) {
    if (S().account || tried.has(w.id) || !remembered || (w.rdns || w.id) !== remembered) continue;
    tried.add(w.id);
    flow.restore(w.id);
  }
  render(S());
});
applyReadRpc();
fromHash();
window.addEventListener('hashchange', () => {
  // A link opened or pasted into this tab. (The page's own replaceState after sending never fires
  // hashchange.) It used to be ignored whenever this tab had started a deposit here, which a
  // connected wallet does at once (the empty draft): the code was dropped and the page stayed on
  // the make-a-deposit path, asking for an account. Now a code always takes over.
  const f = readFragment(location.hash);
  if (f.kind === 'code' && S().request && encodeRequest(S().request) === f.text) return; // already showing it
  fromHash();
  render(S());
  $('card-h')?.focus();
});
render(S());
