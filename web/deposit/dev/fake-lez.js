// DEV ONLY, never deployed. A fake LEZ 0.4 RPC holding LNS name records, for dev/harness.html
// (the LNS scenarios). Same record and profile encoding as test/lns.test.mjs's fake chain, with the
// page's real derivation (src/lib/lnsderive.js: Argon2id), so the page runs exactly as on the network.
import { nameRecordAccount, base58Decode, LNS_PROGRAM, CLOCK_ACCOUNT, CLOCK_PROGRAM } from '../src/lib/lns.js';
import { deriveName } from '../src/lib/lnsderive.js';

const utf8 = new TextEncoder();
const u32 = (n) => Uint8Array.of(n, n >>> 8, n >>> 16, n >>> 24);
const u64 = (n) => { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(n), true); return b; };
const cat = (...p) => { const o = new Uint8Array(p.reduce((n, x) => n + x.length, 0)); let i = 0; for (const x of p) { o.set(x, i); i += x.length; } return o; };
const str = (s) => cat(u32(utf8.encode(s).length), utf8.encode(s));
function encodeProfile({ name, account = null, npk = null, vpk = new Uint8Array(0), texts = {} }) {
  const e = Object.entries(texts);
  const raw = cat(str(name), account ? cat(Uint8Array.of(1), base58Decode(account)) : Uint8Array.of(0),
    npk ? cat(Uint8Array.of(1), npk) : Uint8Array.of(0), u32(vpk.length), vpk, u32(e.length), ...e.flatMap(([k, v]) => [str(k), str(v)]));
  const padded = new Uint8Array(Math.ceil(raw.length / 256) * 256); padded.set(raw); return padded;
}
async function seal(key, plain) {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const k = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, ['encrypt']);
  return cat(nonce, new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv: nonce, additionalData: utf8.encode('logos-names/v5/record') }, k, plain)));
}
const record = ({ sealed, expires = 2n ** 64n - 1n }) => cat(Uint8Array.of(6), new Uint8Array(32).fill(9), u64(expires), u32(sealed.length), sealed, Uint8Array.of(0));

export class FakeLez {
  constructor() { this.accounts = new Map(); this.block = 6300; this.time = 1_794_700_000_000; this.channel = '2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374'; }
  /** A name's record: `profile` as in LNS SPEC (account: base58 public account, or none). */
  async put(name, profile) {
    const { id, key } = await deriveName(name);
    const bytes = record({ sealed: await seal(key, encodeProfile({ name, ...profile })) });
    this.accounts.set(await nameRecordAccount(id), { nonce: 0, data: { actor_states: { [LNS_PROGRAM]: Array.from(bytes) } } });
    this.block += 1;
  }
  fetch = async (url, init) => {
    const calls = JSON.parse(init.body);
    const answers = calls.map(({ id, method, params }) => {
      if (method === 'getChannelId') return { jsonrpc: '2.0', id, result: this.channel };
      if (method === 'getAccount' && params[0] === CLOCK_ACCOUNT) return { jsonrpc: '2.0', id, result: { nonce: 0, data: { actor_states: { [CLOCK_PROGRAM]: Array.from(cat(u64(this.block), u64(this.time))) } } } };
      if (method === 'getAccount') return { jsonrpc: '2.0', id, result: this.accounts.get(params[0]) ?? null };
      return { jsonrpc: '2.0', id, error: { code: -32601, message: 'method not served here' } };
    });
    return new Response(JSON.stringify(answers), { status: 200, headers: { 'content-type': 'application/json' } });
  };
}
