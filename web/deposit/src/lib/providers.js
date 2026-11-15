// EIP-1193 plumbing: wallet discovery (EIP-6963, then the legacy window.ethereum), an
// optional read-only HTTP JSON-RPC provider, and typed helpers over `request`.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Find injected wallets for the whole life of the page. Calls `onChange(list)` whenever the
 * list changes. Each entry: { id, name, icon (a data:image URI or ''), rdns, provider }.
 *
 * - EIP-6963: listens for `eip6963:announceProvider` all the time (a wallet may announce late)
 *   and asks with `eip6963:requestProvider` at once.
 * - Legacy `window.ethereum` (EIP-1193), for wallets and in-app browsers that predate 6963:
 *   looked for at 0.4 s, 1.5 s and 4 s and on MetaMask's `ethereum#initialized` event, since
 *   some inject late. It is shown only when no announced wallet is the same provider.
 */
export function discoverWallets(win, onChange, { legacyAt = [400, 1500, 4000] } = {}) {
  const found = new Map();
  let legacy = null;
  const list = () => {
    const out = [...found.values()];
    if (legacy && !out.some((w) => w.provider === legacy.provider)) out.push(legacy);
    return out;
  };
  let last = '';
  const emit = () => {
    const l = list();
    const key = l.map((w) => w.id).join('|');
    if (key === last) return;
    last = key;
    onChange(l);
  };
  const onAnnounce = (event) => {
    const d = event.detail;
    if (!d || !d.provider || typeof d.provider.request !== 'function' || !d.info) return;
    const id = String(d.info.uuid || d.info.rdns || d.info.name);
    const icon = typeof d.info.icon === 'string' && /^data:image\/(png|svg\+xml|webp|jpeg|gif)[;,]/i.test(d.info.icon) ? d.info.icon : '';
    found.set(id, { id, name: String(d.info.name || 'Browser wallet').slice(0, 40), icon, rdns: String(d.info.rdns || '').slice(0, 100), provider: d.provider });
    emit();
  };
  const lookLegacy = () => {
    const eth = win.ethereum;
    if (legacy || !eth || typeof eth.request !== 'function') return;
    const name = eth.isRabby ? 'Rabby' : eth.isCoinbaseWallet ? 'Coinbase Wallet' : eth.isMetaMask ? 'MetaMask' : 'Browser wallet';
    legacy = { id: 'legacy', name, icon: '', rdns: '', provider: eth };
    emit();
  };
  win.addEventListener('eip6963:announceProvider', onAnnounce);
  win.addEventListener('ethereum#initialized', lookLegacy, { once: true });
  win.dispatchEvent(new Event('eip6963:requestProvider'));
  const timers = legacyAt.map((ms) => win.setTimeout(lookLegacy, ms));
  return {
    list,
    /** Ask again (for a chooser opened later). */
    refresh() {
      win.dispatchEvent(new Event('eip6963:requestProvider'));
      lookLegacy();
    },
    stop() {
      win.removeEventListener('eip6963:announceProvider', onAnnounce);
      win.removeEventListener('ethereum#initialized', lookLegacy);
      timers.forEach((t) => win.clearTimeout(t));
    },
  };
}

/** A read-only EIP-1193-shaped provider over HTTP JSON-RPC. */
export function httpProvider(url, fetchImpl = globalThis.fetch) {
  let id = 0;
  return {
    isReadOnly: true,
    url,
    async request({ method, params = [] }) {
      let res;
      try {
        res = await fetchImpl(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: ++id, method, params }),
        credentials: 'omit',
        referrerPolicy: 'no-referrer',
        cache: 'no-store',
        });
      } catch (e) {
        throw Object.assign(new Error(`could not reach it (${e.message || e})`), { code: -32603, transport: true });
      }
      if (!res.ok) throw Object.assign(new Error(`HTTP ${res.status}`), { code: -32603, transport: true, status: res.status });
      const body = await res.json();
      if (body.error) throw Object.assign(new Error(body.error.message || 'RPC error'), { code: body.error.code, data: body.error.data });
      return body.result;
    },
    on() {},
    removeListener() {},
  };
}

/**
 * A failure to reach the chain (as opposed to the chain's answer, e.g. a revert): network
 * errors, HTTP errors, timeouts, and a wallet whose own RPC is broken (MetaMask reports those
 * as -32603 "Request failed with status code 404"). Reverts carry data and are never this.
 */
export function isTransportError(e) {
  if (!e) return false;
  if (e.transport) return true;
  const msg = String(e.message || e);
  const hasData = typeof e.data === 'string' || typeof e.data?.data === 'string' || typeof e.error?.data === 'string';
  if (hasData) return false;
  return e.code === -32603 || /status code \d{3}|HTTP \d{3}|failed to fetch|network|timed? ?out|ECONN|ENOTFOUND|could not reach/i.test(msg);
}

/** A short, plain description of a transport failure: "error 404", "no answer". */
export function failureWords(e) {
  const m = String(e?.message || e).match(/(?:status code|HTTP) (\d{3})/);
  if (m) return `error ${m[1]}`;
  if (/timed? ?out/i.test(String(e?.message))) return 'no answer in time';
  return 'no answer';
}

/**
 * Reads through the first provider that answers: each entry is { name, provider }. A revert or
 * other real answer is returned as is; only transport failures move on to the next. When all
 * fail, the error says which failed how, in plain words, with the raw texts in `raw`.
 */
export function fallbackProvider(entries) {
  return {
    async request(args) {
      const fails = [];
      for (const { name, provider } of entries) {
        try {
          return await provider.request(args);
        } catch (e) {
          if (!isTransportError(e)) throw e;
          fails.push({ name, e });
        }
      }
      const plain = fails.map((f) => `${f.name} (${failureWords(f.e)})`).join(' and ');
      throw Object.assign(new Error(`${plain} did not answer`), { code: -32603, transport: true, plain, raw: fails.map((f) => `${f.name}: ${f.e.message || f.e}`).join('; ') });
    },
    on() {},
    removeListener() {},
  };
}

/** Typed helpers over any EIP-1193 provider. */
export function rpc(provider) {
  const req = (method, params = []) => provider.request({ method, params });
  return {
    request: req,
    chainId: async () => BigInt(await req('eth_chainId')),
    accounts: async () => (await req('eth_accounts')).map((a) => a.toLowerCase()),
    requestAccounts: async () => (await req('eth_requestAccounts')).map((a) => a.toLowerCase()),
    call: (to, data, { from, value, block = 'latest' } = {}) => {
      const tx = { to, data };
      if (from) tx.from = from;
      if (value !== undefined && value !== 0n) tx.value = '0x' + BigInt(value).toString(16);
      return req('eth_call', [tx, block]);
    },
    estimateGas: async (tx) => BigInt(await req('eth_estimateGas', [tx])),
    gasPrice: async () => BigInt(await req('eth_gasPrice')),
    balance: async (who) => BigInt(await req('eth_getBalance', [who, 'latest'])),
    code: (addr) => req('eth_getCode', [addr, 'latest']),
    blockNumber: async () => BigInt(await req('eth_blockNumber')),
    block: (tag) => req('eth_getBlockByNumber', [typeof tag === 'bigint' ? '0x' + tag.toString(16) : tag, false]),
    receipt: (hash) => req('eth_getTransactionReceipt', [hash]),
    transaction: (hash) => req('eth_getTransactionByHash', [hash]),
    logs: (filter) => req('eth_getLogs', [filter]),
    send: (tx) => req('eth_sendTransaction', [tx]),
  };
}

/** True if an EIP-1193 error means the person declined in the wallet. */
export function isUserRejection(err) {
  if (!err) return false;
  if (err.code === 4001 || err.code === 'ACTION_REJECTED') return true;
  const msg = String(err.message || '').toLowerCase();
  return msg.includes('user rejected') || msg.includes('user denied') || msg.includes('rejected by user');
}

/**
 * eth_getLogs over [from, to], newest first, stopping early once `found(logs)` returns a
 * value. Nodes cap the block range (thirdweb: 1,000 blocks), so it tries the whole range,
 * then 10,000-block chunks, then 1,000-block chunks, and gives up after `maxCalls`.
 * Returns { value, logs, complete }.
 */
export async function scanLogs(r, filter, from, to, { found = () => undefined, maxCalls = 120 } = {}) {
  const q = (a, b) => r.logs({ ...filter, fromBlock: '0x' + a.toString(16), toBlock: '0x' + b.toString(16) });
  try {
    const logs = await q(from, to);
    return { value: found(logs), logs, complete: true };
  } catch {
    /* range too large for this node */
  }
  for (const size of [10000n, 1000n]) {
    const all = [];
    let calls = 0;
    let ok = true;
    for (let hi = to; hi >= from; hi -= size) {
      const lo = hi - size + 1n > from ? hi - size + 1n : from;
      if (++calls > maxCalls) return { value: undefined, logs: all, complete: false };
      let logs;
      try {
        logs = await q(lo, hi);
      } catch {
        ok = false;
        break;
      }
      all.unshift(...logs);
      const v = found(logs);
      if (v !== undefined) return { value: v, logs: all, complete: false };
    }
    if (ok) return { value: undefined, logs: all, complete: true };
  }
  throw new Error('the node refused every log range');
}
