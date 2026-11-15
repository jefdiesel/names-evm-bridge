// The small part of the Solidity ABI this page needs: static arguments (address, uint,
// bool, bytes32) in, and static words, uint256[] and string out. Checked against
// `cast calldata` / `cast abi-decode` in test/abi.test.mjs.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

import { selector } from './keccak.js';

const U256 = (1n << 256n) - 1n;

export function strip0x(hex) {
  return String(hex).replace(/^0x/i, '');
}

function word(hex64) {
  if (!/^[0-9a-f]{64}$/.test(hex64)) throw new Error('bad ABI word');
  return hex64;
}

/** Encode one static value as a 32-byte word (hex, no 0x). */
export function encodeValue(type, value) {
  if (type === 'address') {
    const a = strip0x(value).toLowerCase();
    if (!/^[0-9a-f]{40}$/.test(a)) throw new Error(`not an address: ${value}`);
    return a.padStart(64, '0');
  }
  if (type === 'bool') return (value ? '1' : '0').padStart(64, '0');
  if (type === 'bytes32') {
    const b = strip0x(value).toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(b)) throw new Error(`not bytes32: ${value}`);
    return b;
  }
  const m = /^uint(\d+)$/.exec(type);
  if (m) {
    const bits = BigInt(m[1]);
    const v = BigInt(value);
    if (v < 0n || v >= 1n << bits) throw new Error(`${value} does not fit ${type}`);
    return v.toString(16).padStart(64, '0');
  }
  throw new Error(`unsupported ABI type ${type}`);
}

/**
 * Calldata for `name(types...)` with static arguments.
 * `fn` is the canonical signature, e.g. "deposit(address,uint32,bytes32)".
 */
export function encodeCall(fn, args = []) {
  const types = fn.slice(fn.indexOf('(') + 1, -1).split(',').filter(Boolean);
  if (types.length !== args.length) throw new Error(`${fn} takes ${types.length} arguments`);
  return selector(fn) + types.map((t, i) => encodeValue(t, args[i])).join('');
}

/** Split return data into 32-byte words (hex strings without 0x). */
export function words(data) {
  const h = strip0x(data).toLowerCase();
  if (h.length % 64 !== 0) throw new Error('return data is not whole words');
  const out = [];
  for (let i = 0; i < h.length; i += 64) out.push(word(h.slice(i, i + 64)));
  return out;
}

export const asUint = (w) => BigInt('0x' + w);
export const asBool = (w) => {
  const v = asUint(w);
  if (v > 1n) throw new Error('bad bool');
  return v === 1n;
};
export const asAddress = (w) => '0x' + w.slice(24);
export const asBytes32 = (w) => '0x' + w;

/** Read a uint256[] whose head word (at word index `at`) holds a byte offset. */
export function asUintArray(ws, at) {
  const off = Number(asUint(ws[at]));
  if (off % 32 !== 0) throw new Error('bad array offset');
  const start = off / 32;
  const len = Number(asUint(ws[start]));
  if (start + 1 + len > ws.length) throw new Error('array runs past the data');
  return ws.slice(start + 1, start + 1 + len).map(asUint);
}

/** Decode a `string` return value, or a legacy bytes32 symbol. */
export function decodeString(data) {
  const ws = words(data);
  if (ws.length === 1) {
    // bytes32-style (old tokens like MKR): trailing zeros trimmed
    const bytes = ws[0].match(/../g).map((b) => parseInt(b, 16)).filter((b) => b !== 0);
    return new TextDecoder().decode(new Uint8Array(bytes));
  }
  const off = Number(asUint(ws[0])) / 32;
  const len = Number(asUint(ws[off]));
  const hex = ws.slice(off + 1).join('').slice(0, len * 2);
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) bytes[i] = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return new TextDecoder().decode(bytes);
}

export function hexQuantity(v) {
  const b = BigInt(v);
  if (b < 0n || b > U256) throw new Error('bad quantity');
  return '0x' + b.toString(16);
}
