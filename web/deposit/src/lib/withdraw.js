// SPDX-License-Identifier: MIT OR Apache-2.0
// "Bridge to Ethereum": the two codes between the Names Wallet and this page.
//
// Withdraw-to code (this page makes it from the connected wallet; the wallet reads it):
//   payload = version(1)=1 | chainId(8, big-endian) | vault(20) | recipient(20) | relayFeeChoice(1)
//   code    = bech32m("logoswd", payload)
//   words   = the compare words of SHA-256("logos-bridge/withdraw-to/v1" | payload)
//
// Release code (the wallet makes it; this page sends `release` with the connected wallet):
//   payload = version(1)=1 | chainId(8) | vault(20) | root(32) | nullifier(32) | token(20)
//             | denomIndex(4, big-endian) | recipient(20) | relayer(20) | relayFee(32) | seal(260)
//   code    = bech32m("logosrel", payload)
//
// Both hold public facts only. Spec: the bridge's design note NAMES-SEND-BACK.md;
// vectors: ../../test/vectors/withdraw-v1.json (independent generator). Pure: no DOM, no network.

import { compareWords } from './compare-words.js';

export const WD_HRP = 'logoswd';
export const REL_HRP = 'logosrel';
export const VERSION = 1;
export const WITHDRAW_TAG = 'logos-bridge/withdraw-to/v1';
export const GROTH16_SELECTOR = '0xe71e6620';
export const MAX_FEE_CHOICE = 4;
const WD_LEN = 50;
const REL_LEN = 1 + 8 + 20 + 32 + 32 + 20 + 4 + 20 + 20 + 32 + 260;

const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const BECH32M_CONST = 0x2bc830a3;
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

export class CodeError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'CodeError';
    this.code = code;
  }
}

function polymod(values) {
  let chk = 1;
  for (const v of values) {
    const top = chk >>> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= GEN[i];
  }
  return chk >>> 0;
}

function hrpExpand(hrp) {
  const out = [];
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) >>> 5);
  out.push(0);
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) & 31);
  return out;
}

function convert(data, from, to, pad) {
  let acc = 0, bits = 0;
  const out = [], maxv = (1 << to) - 1;
  for (const v of data) {
    acc = (acc << from) | v;
    bits += from;
    while (bits >= to) {
      bits -= to;
      out.push((acc >>> bits) & maxv);
    }
    acc &= (1 << bits) - 1;
  }
  if (pad) {
    if (bits > 0) out.push((acc << (to - bits)) & maxv);
  } else if (bits >= from || ((acc << (to - bits)) & maxv) !== 0) {
    throw new CodeError('padding', 'The code has invalid padding bits.');
  }
  return out;
}

export function bech32mEncode(hrp, bytes) {
  const data = convert(bytes, 8, 5, true);
  const mod = polymod([...hrpExpand(hrp), ...data, 0, 0, 0, 0, 0, 0]) ^ BECH32M_CONST;
  const chk = [];
  for (let i = 0; i < 6; i++) chk.push((mod >>> (5 * (5 - i))) & 31);
  return hrp + '1' + [...data, ...chk].map((d) => CHARSET[d]).join('');
}

/** Strict decode (one case, no spaces, valid checksum, at most `max` characters). */
export function bech32mDecode(str, max) {
  if (typeof str !== 'string') throw new CodeError('type', 'A code is text.');
  if (str.length > max) throw new CodeError('length', 'The code is too long.');
  const lower = str.toLowerCase();
  if (str !== lower && str !== str.toUpperCase()) throw new CodeError('case', 'The code mixes upper and lower case letters.');
  const sep = lower.lastIndexOf('1');
  if (sep < 1 || sep + 7 > lower.length) throw new CodeError('format', 'This is not a code from the bridge.');
  const hrp = lower.slice(0, sep);
  const data = [];
  for (const ch of lower.slice(sep + 1)) {
    const d = CHARSET.indexOf(ch);
    if (d === -1) throw new CodeError('charset', `The code contains "${ch}", which no code can contain.`);
    data.push(d);
  }
  if (polymod([...hrpExpand(hrp), ...data]) !== BECH32M_CONST) {
    throw new CodeError('checksum', 'The code’s checksum does not match: it was mistyped or cut short. Copy it again.');
  }
  return { hrp, bytes: convert(data.slice(0, -6), 5, 8, false) };
}

const hx = (bytes) => '0x' + bytes.map((b) => b.toString(16).padStart(2, '0')).join('');
function unhex(h, len, what) {
  if (typeof h !== 'string' || !/^0x[0-9a-fA-F]*$/.test(h) || h.length !== 2 + 2 * len) {
    throw new CodeError('field', `${what} must be 0x and ${2 * len} hex digits.`);
  }
  const out = [];
  for (let i = 0; i < len; i++) out.push(parseInt(h.slice(2 + 2 * i, 4 + 2 * i), 16));
  return out;
}
const beBytes = (v, n) => {
  const out = new Array(n).fill(0);
  let x = BigInt(v);
  for (let i = n - 1; i >= 0; i--) { out[i] = Number(x & 0xffn); x >>= 8n; }
  if (x !== 0n) throw new CodeError('field', 'a number is too large');
  return out;
};
const beNum = (bytes) => bytes.reduce((a, b) => (a << 8n) | BigInt(b), 0n);
const isZero = (h) => /^0x0*$/.test(h);

/** Lenient front door: trims whitespace, accepts a full link (the part after `#`). */
export function codeFromInput(input) {
  let s = String(input ?? '').replace(/[\s​‌‍﻿]+/g, '');
  const hash = s.indexOf('#');
  if (hash !== -1) s = s.slice(hash + 1);
  if (!s) throw new CodeError('empty', 'Nothing to read: paste the code.');
  return s;
}

// ---------------------------------------------------------------- withdraw-to

export function withdrawToBytes(w) {
  const chainId = BigInt(w.chainId);
  if (chainId === 0n || chainId >= 1n << 64n) throw new CodeError('chain', 'The chain id must be between 1 and 2^64-1.');
  const vault = unhex(String(w.vault).toLowerCase(), 20, 'vault');
  const recipient = unhex(String(w.recipient).toLowerCase(), 20, 'recipient');
  const fee = w.feeChoice ?? 0;
  if (!Number.isInteger(fee) || fee < 0 || fee > MAX_FEE_CHOICE) throw new CodeError('fee-choice', 'The relay-fee choice must be 0 to 4.');
  return [VERSION, ...beBytes(chainId, 8), ...vault, ...recipient, fee];
}

export function encodeWithdrawTo(w) {
  const b = withdrawToBytes(w);
  const n = withdrawToFromBytes(b); // the same checks a reader applies
  void n;
  return bech32mEncode(WD_HRP, b);
}

export function withdrawToFromBytes(b) {
  if (b[0] !== VERSION) throw new CodeError('version', `This code is version ${b[0]}; this page reads version ${VERSION}.`);
  if (b.length !== WD_LEN) throw new CodeError('length', 'The code has the wrong length.');
  const chainId = beNum(b.slice(1, 9));
  const vault = hx(b.slice(9, 29));
  const recipient = hx(b.slice(29, 49));
  const feeChoice = b[49];
  if (chainId === 0n) throw new CodeError('chain', 'The code names no chain.');
  if (isZero(vault)) throw new CodeError('vault', 'The code names no vault.');
  if (isZero(recipient)) throw new CodeError('recipient', 'The zero address can’t receive a release.');
  if (recipient === vault) throw new CodeError('recipient', 'The vault itself can’t receive a release.');
  if (feeChoice > MAX_FEE_CHOICE) throw new CodeError('fee-choice', 'The relay-fee choice must be 0 to 4.');
  return { version: VERSION, chainId, vault, recipient, feeChoice };
}

export function decodeWithdrawTo(str) {
  const { hrp, bytes } = bech32mDecode(str, 120);
  if (hrp !== WD_HRP) throw new CodeError('hrp', `This is a "${hrp}" code, not a withdraw-to code.`);
  return withdrawToFromBytes(bytes);
}

/** The digest the four words are taken over: SHA-256(tag | payload). */
export async function withdrawDigest(w) {
  const tag = new TextEncoder().encode(WITHDRAW_TAG);
  const p = withdrawToBytes(w);
  const msg = new Uint8Array(tag.length + p.length);
  msg.set(tag, 0);
  msg.set(p, tag.length);
  return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', msg));
}

/** `{ indexes, words, words_text }` of a withdraw-to code's fields. */
export async function withdrawWords(w) {
  return compareWords(await withdrawDigest(w));
}

// ---------------------------------------------------------------- release

export function releaseToBytes(r) {
  const seal = unhex(String(r.seal).toLowerCase(), 260, 'seal');
  return [
    VERSION, ...beBytes(r.chainId, 8), ...unhex(String(r.vault).toLowerCase(), 20, 'vault'),
    ...unhex(String(r.root).toLowerCase(), 32, 'root'), ...unhex(String(r.nullifier).toLowerCase(), 32, 'nullifier'),
    ...unhex(String(r.token).toLowerCase(), 20, 'token'), ...beBytes(r.denomIndex, 4),
    ...unhex(String(r.recipient).toLowerCase(), 20, 'recipient'), ...unhex(String(r.relayer).toLowerCase(), 20, 'relayer'),
    ...beBytes(r.relayFee, 32), ...seal,
  ];
}

export function releaseFromBytes(b) {
  if (b[0] !== VERSION) throw new CodeError('version', `This code is version ${b[0]}; this page reads version ${VERSION}.`);
  if (b.length !== REL_LEN) throw new CodeError('length', 'The release code has the wrong length.');
  let at = 1;
  const take = (n) => { const s = b.slice(at, at + n); at += n; return s; };
  const r = {
    version: VERSION,
    chainId: beNum(take(8)),
    vault: hx(take(20)),
    root: hx(take(32)),
    nullifier: hx(take(32)),
    token: hx(take(20)),
    denomIndex: Number(beNum(take(4))),
    recipient: hx(take(20)),
    relayer: hx(take(20)),
    relayFee: beNum(take(32)),
    seal: hx(take(260)),
  };
  if (r.chainId === 0n) throw new CodeError('chain', 'The code names no chain.');
  if (isZero(r.vault)) throw new CodeError('vault', 'The code names no vault.');
  if (isZero(r.recipient) || r.recipient === r.vault) throw new CodeError('recipient', 'This release has no usable recipient.');
  if (r.relayFee !== 0n && (isZero(r.relayer) || r.relayer === r.vault)) throw new CodeError('relayer', 'A relay fee needs a relayer.');
  if (!r.seal.startsWith(GROTH16_SELECTOR)) throw new CodeError('seal', 'The proof is not for RISC Zero’s Groth16 verifier 3.1.0.');
  return r;
}

export function encodeRelease(r) {
  const b = releaseToBytes(r);
  releaseFromBytes(b);
  return bech32mEncode(REL_HRP, b);
}

export function decodeRelease(str) {
  const { hrp, bytes } = bech32mDecode(str, 800);
  if (hrp !== REL_HRP) throw new CodeError('hrp', `This is a "${hrp}" code, not a release code.`);
  return releaseFromBytes(bytes);
}

/** What kind of code a pasted text is: 'withdraw', 'release', 'deposit' or null. */
export function codeKind(input) {
  const s = String(input ?? '').toLowerCase();
  const at = s.lastIndexOf('#');
  const t = (at === -1 ? s : s.slice(at + 1)).trim();
  if (t.startsWith(WD_HRP + '1')) return 'withdraw';
  if (t.startsWith(REL_HRP + '1')) return 'release';
  if (t.startsWith('logosdep1')) return 'deposit';
  return null;
}
