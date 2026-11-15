// SPDX-License-Identifier: MIT OR Apache-2.0
// Four words for comparing a deposit's commitment by eye. The Names Wallet shows them beside the
// commitment's first and last 6 hex digits, and so can this page; the person checks they match.
//
//   h       = SHA-256( ASCII "logos-bridge/compare/v1" | the 32 commitment bytes )
//   indexes = the first 44 bits of h, big-endian, as four 11-bit numbers
//             (bits 0-10, 11-21, 22-32, 33-43)
//   words   = the BIP-39 English word at each index (index 0 = "abandon")
//   text    = the words joined with " · " (space, U+00B7, space)
//
// The wallet's core module computes the same (its reply's `compare.words` / `compare.words_text`).
// Both are checked against test/vectors/compare-words-v1.json, made by an independent generator.
// Pure: no DOM, no network. SHA-256 is the browser's (crypto.subtle), so compareWords is async.

import { BIP39_ENGLISH } from './vendor/bip39-english.js';

export const COMPARE_TAG = 'logos-bridge/compare/v1';
export const WORDS_SEPARATOR = ' · ';

/** A commitment as 32 bytes, from `0x`-prefixed (or bare) hex, or a Uint8Array(32). */
function commitmentBytes(commitment) {
  if (commitment instanceof Uint8Array) {
    if (commitment.length !== 32) throw new Error('a commitment is 32 bytes');
    return commitment;
  }
  const hex = String(commitment).trim().replace(/^0x/i, '');
  if (!/^[0-9a-fA-F]{64}$/.test(hex)) throw new Error('a commitment is 64 hex digits');
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) out[i] = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/** The four 11-bit indexes from the first 6 bytes of h (byte by byte: no 32-bit overflow). */
export function compareIndexes(h) {
  return [
    (h[0] << 3) | (h[1] >> 5),
    ((h[1] & 0x1f) << 6) | (h[2] >> 2),
    ((h[2] & 0x03) << 9) | (h[3] << 1) | (h[4] >> 7),
    ((h[4] & 0x7f) << 4) | (h[5] >> 4),
  ];
}

/**
 * The compare words of a commitment: `{ indexes, words, words_text }`.
 * `commitment` is hex (`0x…`, 64 digits) or a Uint8Array(32).
 */
export async function compareWords(commitment) {
  const c = commitmentBytes(commitment);
  const tag = new TextEncoder().encode(COMPARE_TAG);
  const msg = new Uint8Array(tag.length + 32);
  msg.set(tag, 0);
  msg.set(c, tag.length);
  const h = new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', msg));
  const indexes = compareIndexes(h);
  const words = indexes.map((i) => BIP39_ENGLISH[i]);
  return { indexes, words, words_text: words.join(WORDS_SEPARATOR) };
}
