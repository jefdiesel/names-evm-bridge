#!/usr/bin/env node
// Regenerate test/vectors/request-v1.json: valid requests, inputs the lenient parser must
// accept, and strings the strict decoder must refuse (with the error code). A Rust (or any
// other) implementation of the format should pass the same file.
//
// The commitments here are SHA-256 hashes of public labels. Nobody knows a deposit secret
// behind them, so a deposit made with one of these could never be claimed: they are for
// tests and examples only.

import { createHash } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { encodeRequest, requestToBytes, requestToJSON, HRP } from '../src/lib/request.js';

const sha = (s) => '0x' + createHash('sha256').update(s).digest('hex');
const VAULT = '0x8c8b2664de28e5287158db4db65d448638871b74';
const USDC = '0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e';
const ETH = '0x0000000000000000000000000000000000000000';

const valid = [
  { name: 'sepolia demo v2, 0.01 ETH (denomination 1 of 3)', req: { chainId: 11155111n, vault: VAULT, token: ETH, denomIndex: 0, amount: 10n ** 16n, commitment: sha('logos-deposit-request example 1: unclaimable') } },
  { name: 'sepolia demo v2, 100 dUSDC', req: { chainId: 11155111n, vault: VAULT, token: USDC, denomIndex: 0, amount: 100_000_000n, commitment: sha('logos-deposit-request example 2: unclaimable') } },
  { name: 'sepolia demo v2, 10,000 dUSDC (denomination 3 of 3)', req: { chainId: 11155111n, vault: VAULT, token: USDC, denomIndex: 2, amount: 10_000_000_000n, commitment: sha('logos-deposit-request example 3: unclaimable') } },
  { name: 'bounds: chain 1, index 255, amount 2^256-1', req: { chainId: 1n, vault: '0xffffffffffffffffffffffffffffffffffffffff', token: '0x00000000000000000000000000000000000000ff', denomIndex: 255, amount: (1n << 256n) - 1n, commitment: '0x' + 'ff'.repeat(32) } },
  { name: 'bounds: chain 2^64-1, amount 1, commitment 1', req: { chainId: (1n << 64n) - 1n, vault: '0x0000000000000000000000000000000000000001', token: ETH, denomIndex: 0, amount: 1n, commitment: '0x' + '00'.repeat(31) + '01' } },
];

// ---- a separate, minimal bech32/bech32m writer, so invalid strings can be crafted
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
function polymod(v) {
  let c = 1;
  for (const x of v) {
    const t = c >>> 25;
    c = ((c & 0x1ffffff) << 5) ^ x;
    for (let i = 0; i < 5; i++) if ((t >>> i) & 1) c ^= GEN[i];
  }
  return c >>> 0;
}
const expand = (h) => [...[...h].map((c) => c.charCodeAt(0) >>> 5), 0, ...[...h].map((c) => c.charCodeAt(0) & 31)];
function toWords(bytes) {
  const out = [];
  let acc = 0;
  let bits = 0;
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out.push((acc >>> bits) & 31);
    }
    acc &= (1 << bits) - 1;
  }
  if (bits) out.push((acc << (5 - bits)) & 31);
  return out;
}
function encodeWords(hrp, words, constant = 0x2bc830a3) {
  const mod = polymod([...expand(hrp), ...words, 0, 0, 0, 0, 0, 0]) ^ constant;
  const chk = [0, 1, 2, 3, 4, 5].map((i) => (mod >>> (5 * (5 - i))) & 31);
  return hrp + '1' + [...words, ...chk].map((d) => CHARSET[d]).join('');
}
const encBytes = (bytes, hrp = HRP, constant) => encodeWords(hrp, toWords(bytes), constant);

const base = valid[0].req;
const baseStr = encodeRequest(base);
const baseBytes = requestToBytes(base);
const flip = (s, i) => s.slice(0, i) + (s[i] === 'q' ? 'p' : 'q') + s.slice(i + 1);
const withPayload = (f) => encBytes(f([...baseBytes]));

// amount field of the base request starts at byte 50 (length) / 51 (value)
const invalid = [
  { name: 'one character changed', input: flip(baseStr, 40), error: 'checksum' },
  { name: 'two characters swapped', input: baseStr.slice(0, 30) + baseStr[31] + baseStr[30] + baseStr.slice(32), error: 'checksum' },
  { name: 'last character dropped', input: baseStr.slice(0, -1), error: 'checksum' },
  { name: 'bech32 (not bech32m) checksum', input: encBytes(baseBytes, HRP, 1), error: 'checksum' },
  { name: 'mixed case', input: baseStr.slice(0, 20) + baseStr.slice(20).toUpperCase(), error: 'case' },
  { name: 'other prefix', input: encBytes(baseBytes, 'logosdap'), error: 'hrp' },
  { name: 'character not in the bech32 set (b)', input: baseStr.slice(0, 30) + 'b' + baseStr.slice(31), error: 'charset' },
  { name: 'space inside (strict decoder)', input: baseStr.slice(0, 50) + ' ' + baseStr.slice(50), error: 'charset' },
  { name: 'too long', input: encBytes([...baseBytes, ...new Array(40).fill(0)]), error: 'length' },
  { name: 'version 2', input: withPayload((b) => ((b[0] = 2), b)), error: 'version' },
  { name: 'version 0', input: withPayload((b) => ((b[0] = 0), b)), error: 'version' },
  { name: 'one extra byte at the end', input: withPayload((b) => [...b, 0]), error: 'trailing' },
  { name: 'last commitment byte missing', input: withPayload((b) => b.slice(0, -1)), error: 'length' },
  { name: 'amount length 0', input: withPayload((b) => [...b.slice(0, 50), 0, ...b.slice(51 + b[50])]), error: 'amount' },
  { name: 'amount length 33', input: withPayload((b) => [...b.slice(0, 50), 33, ...b.slice(51)]), error: 'amount' },
  { name: 'amount with a leading zero byte', input: withPayload((b) => [...b.slice(0, 50), b[50] + 1, 0, ...b.slice(51)]), error: 'amount' },
  { name: 'zero commitment', input: withPayload((b) => [...b.slice(0, -32), ...new Array(32).fill(0)]), error: 'commitment' },
  { name: 'zero vault', input: withPayload((b) => [...b.slice(0, 9), ...new Array(20).fill(0), ...b.slice(29)]), error: 'vault' },
  { name: 'chain id 0', input: withPayload((b) => [b[0], 0, 0, 0, 0, 0, 0, 0, 0, ...b.slice(9)]), error: 'chain' },
  {
    name: 'non-zero padding bits',
    input: (() => {
      const bytes = requestToBytes(valid[1].req); // 87 bytes = 696 bits -> 140 words, 4 padding bits
      if ((bytes.length * 8) % 5 === 0) throw new Error('padding vector needs a payload with padding');
      const w = toWords(bytes);
      w[w.length - 1] |= 1;
      return encodeWords(HRP, w);
    })(),
    error: 'padding',
  },
];

const lenient = [
  { name: 'full link (request in the fragment)', input: `https://deposit.example/#${baseStr}`, expect: 0 },
  { name: 'link with request= in the fragment', input: `https://deposit.example/app/#request=${baseStr}`, expect: 0 },
  { name: 'surrounding whitespace and newline', input: `  ${baseStr}\n`, expect: 0 },
  { name: 'wrapped across lines by a chat app', input: baseStr.slice(0, 60) + '\n' + baseStr.slice(60, 120) + '\r\n' + baseStr.slice(120), expect: 0 },
  { name: 'all uppercase (as in a QR code)', input: baseStr.toUpperCase(), expect: 0 },
  { name: 'zero-width space pasted inside', input: baseStr.slice(0, 10) + '​' + baseStr.slice(10), expect: 0 },
];

const bip350 = {
  note: 'Official BIP-350 bech32m vectors: the checksum layer must accept these (hrp and 5-bit words only).',
  valid: ['A1LQFN3A', 'a1lqfn3a', 'abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryx', 'split1checkupstagehandshakeupstreamerranterredcaperredlc445v', '?1v759aa'],
  mutatedMustFail: ['a1lqfn3q', 'abcdef1l7aum6echk45nj3s0wdvt2fg8x9yrzpqzd3ryq'],
};

const file = {
  format: 'Logos bridge deposit request',
  version: 1,
  hrp: HRP,
  spec: 'web/deposit/README.md, section "Deposit request format, v1"',
  warning: 'The commitments below are SHA-256 hashes of public labels; no deposit secret exists for them. Never deposit to them.',
  valid: valid.map((v) => {
    const s = encodeRequest(v.req);
    return { name: v.name, request: requestToJSON(v.req), payloadHex: Buffer.from(requestToBytes(v.req)).toString('hex'), string: s, length: s.length };
  }),
  lenient: lenient.map((l) => ({ name: l.name, input: l.input, expect: encodeRequest(valid[l.expect].req) })),
  invalid: invalid.map((x) => ({ name: x.name, input: x.input, error: x.error })),
  bip350,
};

const out = fileURLToPath(new URL('../test/vectors/request-v1.json', import.meta.url));
writeFileSync(out, JSON.stringify(file, null, 2) + '\n');
console.log(`wrote ${out}: ${file.valid.length} valid, ${file.lenient.length} lenient, ${file.invalid.length} invalid`);
