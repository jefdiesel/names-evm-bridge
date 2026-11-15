// derive(name) for src/lib/lns.js: the name's master key by Argon2id (LNS SPEC §3: salt
// "logos-names/v5", version 0x13, 64 MiB, 3 passes, 1 lane, 32 bytes), then id and key by SHA-256.
// About half a second and 64 MiB per name in plain JavaScript. The page's CSP has worker-src 'none',
// so it runs on the main thread; asyncTick hands control back to the page every 50 ms, so typing and
// the "Checking…" state stay live. Nothing is kept: the caller may memoise in memory only.
// SPDX-License-Identifier: MIT OR Apache-2.0
import { argon2idAsync } from './vendor/argon2id.js';

const utf8 = new TextEncoder();
const PARAMS = { t: 3, m: 65536, p: 1, dkLen: 32, version: 0x13, asyncTick: 50 };

async function sha256(...parts) {
  const all = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    all.set(p, o);
    o += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest('SHA-256', all));
}

/** `{ master, id, key }` of a bare, valid name (32-byte Uint8Arrays). */
export async function deriveName(name) {
  const master = await argon2idAsync(utf8.encode(name), utf8.encode('logos-names/v5'), PARAMS);
  return { master, id: await sha256(utf8.encode('logos-names/v5/id'), master), key: await sha256(utf8.encode('logos-names/v5/key'), master) };
}
