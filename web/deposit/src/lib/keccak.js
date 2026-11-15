// Keccak-256 (the pre-NIST padding Ethereum uses), written out so the page needs no
// library. Used only for function selectors, event topics and EIP-55 address checksums,
// all checked against Foundry's `cast` in test/abi.test.mjs. Lanes are BigInt: slow
// (well under a millisecond per hash here) but short enough to review line by line.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

const MASK = (1n << 64n) - 1n;

const RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];

// Rotation offsets r[x][y], indexed as x + 5y.
const ROT = [0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14];

const rotl = (v, n) => (n === 0 ? v : ((v << BigInt(n)) | (v >> BigInt(64 - n))) & MASK);

function keccakF(s) {
  const c = new Array(5);
  const b = new Array(25);
  for (let round = 0; round < 24; round++) {
    for (let x = 0; x < 5; x++) c[x] = s[x] ^ s[x + 5] ^ s[x + 10] ^ s[x + 15] ^ s[x + 20];
    for (let x = 0; x < 5; x++) {
      const d = c[(x + 4) % 5] ^ rotl(c[(x + 1) % 5], 1);
      for (let y = 0; y < 25; y += 5) s[x + y] ^= d;
    }
    for (let x = 0; x < 5; x++) {
      for (let y = 0; y < 5; y++) b[y + 5 * ((2 * x + 3 * y) % 5)] = rotl(s[x + 5 * y], ROT[x + 5 * y]);
    }
    for (let x = 0; x < 5; x++) {
      for (let y = 0; y < 25; y += 5) s[x + y] = b[x + y] ^ (~b[((x + 1) % 5) + y] & MASK & b[((x + 2) % 5) + y]);
    }
    s[0] ^= RC[round];
  }
}

/** keccak256 of bytes (Uint8Array or number[]). Returns a Uint8Array(32). */
export function keccak256(bytes) {
  const rate = 136;
  const msg = Array.from(bytes);
  msg.push(0x01);
  while (msg.length % rate !== 0) msg.push(0);
  msg[msg.length - 1] |= 0x80;
  const s = new Array(25).fill(0n);
  for (let off = 0; off < msg.length; off += rate) {
    for (let i = 0; i < rate / 8; i++) {
      let lane = 0n;
      for (let k = 7; k >= 0; k--) lane = (lane << 8n) | BigInt(msg[off + 8 * i + k]);
      s[i] ^= lane;
    }
    keccakF(s);
  }
  const out = new Uint8Array(32);
  for (let i = 0; i < 4; i++) {
    for (let k = 0; k < 8; k++) out[8 * i + k] = Number((s[i] >> BigInt(8 * k)) & 0xffn);
  }
  return out;
}

const enc = new TextEncoder();

/** keccak256 of a UTF-8 string, as 0x-hex. */
export function keccakText(text) {
  return '0x' + Array.from(keccak256(enc.encode(text)), (b) => b.toString(16).padStart(2, '0')).join('');
}

/** First 4 bytes of keccak256(signature), as 0x + 8 hex. */
export function selector(signature) {
  return keccakText(signature).slice(0, 10);
}

/** EIP-55 mixed-case checksum form of an address. */
export function checksumAddress(address) {
  const a = String(address).toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]{40}$/.test(a)) throw new Error('not an address');
  const h = keccakText(a).slice(2);
  let out = '0x';
  for (let i = 0; i < 40; i++) out += parseInt(h[i], 16) >= 8 ? a[i].toUpperCase() : a[i];
  return out;
}
