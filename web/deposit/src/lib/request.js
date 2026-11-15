// Logos bridge deposit request, format v1.
//
// A deposit request is what a Logos wallet hands to an ordinary Ethereum wallet so that
// the Ethereum side can lock funds behind a commitment C without ever learning the
// deposit secret or the Logos destination. It carries only public facts:
//
//   version | EVM chain id | vault | token | denomination index | denomination amount | C
//
// encoded as bech32m (BIP-350) with the human-readable part "logosdep". The full
// specification, with the reasons for each choice, is in ../../README.md
// ("Deposit request format, v1"); test vectors are in ../../test/vectors/request-v1.json.
//
// This file is standalone on purpose: no imports, no dependencies, plain ES2020 with
// BigInt. It runs unchanged in browsers, Node and Deno, so the Logos side can copy it.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

export const HRP = 'logosdep';
export const VERSION = 1;
/** Longest request string accepted (a v1 request is 150 to 199 characters). */
export const MAX_LENGTH = 200;

const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const BECH32M_CONST = 0x2bc830a3;
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

/** An error with a stable machine-readable `code` (listed in the README and the vectors). */
export class RequestError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'RequestError';
    this.code = code;
  }
}

// ---------------------------------------------------------------- bech32m

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

function checksum(hrp, data) {
  const mod = polymod([...hrpExpand(hrp), ...data, 0, 0, 0, 0, 0, 0]) ^ BECH32M_CONST;
  const out = [];
  for (let i = 0; i < 6; i++) out.push((mod >>> (5 * (5 - i))) & 31);
  return out;
}

function convertBits(data, from, to, pad) {
  let acc = 0;
  let bits = 0;
  const out = [];
  const maxv = (1 << to) - 1;
  for (const value of data) {
    acc = (acc << from) | value;
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
    throw new RequestError('padding', 'The request has invalid padding bits.');
  }
  return out;
}

/** bech32m-encode bytes under `hrp`. Always lowercase. */
export function bech32mEncode(hrp, bytes) {
  const data = convertBits(bytes, 8, 5, true);
  const all = [...data, ...checksum(hrp, data)];
  return hrp + '1' + all.map((d) => CHARSET[d]).join('');
}

/**
 * Strict bech32m decode to 5-bit words: no whitespace, one case throughout, valid
 * checksum. Returns { hrp, words } (checksum removed).
 */
export function bech32mDecodeWords(str) {
  if (typeof str !== 'string') throw new RequestError('type', 'A request must be a string.');
  if (str.length > MAX_LENGTH) throw new RequestError('length', 'The request is too long.');
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    if (c < 33 || c > 126) throw new RequestError('charset', 'The request contains spaces or characters it cannot contain.');
  }
  const lower = str.toLowerCase();
  if (str !== lower && str !== str.toUpperCase()) {
    throw new RequestError('case', 'The request mixes upper and lower case letters.');
  }
  const sep = lower.lastIndexOf('1');
  if (sep < 1 || sep + 7 > lower.length) throw new RequestError('format', 'This is not a deposit request.');
  const hrp = lower.slice(0, sep);
  const data = [];
  for (const ch of lower.slice(sep + 1)) {
    const d = CHARSET.indexOf(ch);
    if (d === -1) throw new RequestError('charset', `The request contains "${ch}", which no request can contain.`);
    data.push(d);
  }
  if (polymod([...hrpExpand(hrp), ...data]) !== BECH32M_CONST) {
    throw new RequestError('checksum', 'The request’s checksum does not match: it was mistyped or cut short. Copy it again from your Logos wallet.');
  }
  return { hrp, words: data.slice(0, -6) };
}

/** Strict bech32m decode to bytes (padding bits must be zero). Returns { hrp, bytes }. */
export function bech32mDecode(str) {
  const { hrp, words } = bech32mDecodeWords(str);
  return { hrp, bytes: convertBits(words, 5, 8, false) };
}

// ---------------------------------------------------------------- helpers

const hexRe = /^0x[0-9a-fA-F]*$/;

function hexToBytes(hex, len, what) {
  if (typeof hex !== 'string' || !hexRe.test(hex) || hex.length !== 2 + 2 * len) {
    throw new RequestError('field', `${what} must be 0x followed by ${2 * len} hex digits.`);
  }
  const out = new Array(len);
  for (let i = 0; i < len; i++) out[i] = parseInt(hex.slice(2 + 2 * i, 4 + 2 * i), 16);
  return out;
}

function bytesToHex(bytes) {
  let s = '0x';
  for (const b of bytes) s += b.toString(16).padStart(2, '0');
  return s;
}

function toBigInt(v, what) {
  if (typeof v === 'bigint') return v;
  if (typeof v === 'number' && Number.isSafeInteger(v)) return BigInt(v);
  if (typeof v === 'string' && /^[0-9]+$/.test(v)) return BigInt(v);
  throw new RequestError('field', `${what} must be a non-negative integer.`);
}

const U64_MAX = (1n << 64n) - 1n;
const U256_MAX = (1n << 256n) - 1n;

function isZero(bytes) {
  return bytes.every((b) => b === 0);
}

// ---------------------------------------------------------------- v1 payload

/**
 * @typedef {Object} DepositRequest
 * @property {number} version      always 1
 * @property {bigint} chainId      EVM chain id, 1 .. 2^64-1
 * @property {string} vault        0x + 40 lowercase hex, never zero
 * @property {string} token        0x + 40 lowercase hex; the zero address means native ETH
 * @property {number} denomIndex   0 .. 255 (the vault allows at most 8)
 * @property {bigint} amount       the denomination in the token's smallest unit, > 0, < 2^256
 * @property {string} commitment   0x + 64 lowercase hex, never zero
 */

/** Check and normalise the fields of a request. Throws RequestError. */
export function normalizeRequest(r) {
  if (!r || typeof r !== 'object') throw new RequestError('field', 'A request must be an object.');
  const version = r.version === undefined ? VERSION : r.version;
  if (version !== VERSION) throw new RequestError('version', `Only version ${VERSION} requests exist.`);
  const chainId = toBigInt(r.chainId, 'chainId');
  if (chainId === 0n || chainId > U64_MAX) throw new RequestError('chain', 'The chain id must be between 1 and 2^64-1.');
  const vault = bytesToHex(hexToBytes(r.vault, 20, 'vault'));
  if (isZero(hexToBytes(vault, 20, 'vault'))) throw new RequestError('vault', 'The vault cannot be the zero address.');
  const token = bytesToHex(hexToBytes(r.token, 20, 'token'));
  const denomIndex = r.denomIndex;
  if (!Number.isInteger(denomIndex) || denomIndex < 0 || denomIndex > 255) {
    throw new RequestError('denom-index', 'The denomination index must be between 0 and 255.');
  }
  const amount = toBigInt(r.amount, 'amount');
  if (amount === 0n) throw new RequestError('amount', 'The amount cannot be zero.');
  if (amount > U256_MAX) throw new RequestError('amount', 'The amount does not fit in 256 bits.');
  const commitment = bytesToHex(hexToBytes(r.commitment, 32, 'commitment'));
  if (isZero(hexToBytes(commitment, 32, 'commitment'))) {
    throw new RequestError('commitment', 'The commitment cannot be all zeros.');
  }
  return { version, chainId, vault, token, denomIndex, amount, commitment };
}

/** The v1 payload bytes (before bech32m). */
export function requestToBytes(r) {
  const n = normalizeRequest(r);
  const out = [VERSION];
  for (let i = 7; i >= 0; i--) out.push(Number((n.chainId >> BigInt(8 * i)) & 0xffn));
  out.push(...hexToBytes(n.vault, 20, 'vault'));
  out.push(...hexToBytes(n.token, 20, 'token'));
  out.push(n.denomIndex);
  const amt = [];
  for (let v = n.amount; v > 0n; v >>= 8n) amt.unshift(Number(v & 0xffn));
  out.push(amt.length, ...amt);
  out.push(...hexToBytes(n.commitment, 32, 'commitment'));
  return out;
}

/** Parse v1 payload bytes. Throws RequestError. */
export function requestFromBytes(bytes) {
  if (bytes.length < 1) throw new RequestError('length', 'The request is empty.');
  if (bytes[0] !== VERSION) {
    throw new RequestError('version', `This request is version ${bytes[0]}; this page understands version ${VERSION} only.`);
  }
  if (bytes.length < 51) throw new RequestError('length', 'The request is too short.');
  let chainId = 0n;
  for (let i = 1; i <= 8; i++) chainId = (chainId << 8n) | BigInt(bytes[i]);
  const vault = bytesToHex(bytes.slice(9, 29));
  const token = bytesToHex(bytes.slice(29, 49));
  const denomIndex = bytes[49];
  const len = bytes[50];
  if (len < 1 || len > 32) throw new RequestError('amount', 'The request’s amount field is malformed.');
  if (bytes.length < 51 + len + 32) throw new RequestError('length', 'The request is too short.');
  if (bytes.length > 51 + len + 32) throw new RequestError('trailing', 'The request has extra data at the end.');
  const amt = bytes.slice(51, 51 + len);
  if (amt[0] === 0) throw new RequestError('amount', 'The request’s amount is not minimally encoded.');
  let amount = 0n;
  for (const b of amt) amount = (amount << 8n) | BigInt(b);
  const commitment = bytesToHex(bytes.slice(51 + len));
  return normalizeRequest({ version: VERSION, chainId, vault, token, denomIndex, amount, commitment });
}

/** Encode a request as its one canonical string (lowercase bech32m, hrp "logosdep"). */
export function encodeRequest(r) {
  return bech32mEncode(HRP, requestToBytes(r));
}

/** Strictly decode a request string (lowercase or all-uppercase, nothing around it). */
export function decodeRequest(str) {
  const { hrp, bytes } = bech32mDecode(str);
  if (hrp !== HRP) throw new RequestError('hrp', `This is a "${hrp}" string, not a Logos deposit request.`);
  return requestFromBytes(bytes);
}

/**
 * Lenient front door for anything a person pastes, scans or opens: trims whitespace and
 * line breaks a chat app may add, and accepts a full link (the request is the part after
 * `#`). Anything else must be the request string itself. Then decodes strictly.
 */
export function parseRequestInput(input) {
  if (typeof input !== 'string') throw new RequestError('type', 'A request must be text.');
  let s = input.replace(/[\s​‌‍﻿]+/g, '');
  const hash = s.indexOf('#');
  if (hash !== -1) s = s.slice(hash + 1);
  if (s.toLowerCase().startsWith('request=')) s = s.slice(8);
  if (s === '') throw new RequestError('empty', 'Nothing to read: paste a deposit request.');
  return decodeRequest(s);
}

/** A link that opens `pageUrl` with the request in the fragment (never the query string). */
export function requestUrl(pageUrl, r) {
  const base = String(pageUrl).split('#')[0];
  return `${base}#${encodeRequest(r)}`;
}

/** Plain JSON form (bigints as decimal strings), as used by the test vectors. */
export function requestToJSON(r) {
  const n = normalizeRequest(r);
  return { ...n, chainId: n.chainId.toString(), amount: n.amount.toString() };
}
