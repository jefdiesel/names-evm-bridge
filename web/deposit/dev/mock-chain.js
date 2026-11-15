// DEV ONLY, never deployed. An in-memory Ethereum chain with the BridgeVault deposit rules
// and an EIP-1193 wallet on top of it. Used by test/flow.test.mjs (headless) and by
// dev/harness.html (screenshots of every state without a real wallet).
//
// It follows BridgeVault.sol's deposit path: freeze, registration, denomination index,
// msg.value / transferFrom amount, commitmentUsed, per-token and global caps (as a simple
// used-so-far counter), counters, Deposit event. It is a test double, not an EVM.

import { selector } from '../src/lib/keccak.js';
import { DEPOSIT_TOPIC, FEE_PAID_TOPIC } from '../src/lib/vault.js';
import { DEPLOYMENTS } from '../src/lib/deployments.js';

const W = (v) => BigInt(v).toString(16).padStart(64, '0');
const A = (a) => a.toLowerCase().replace(/^0x/, '').padStart(64, '0');
const hex = (v) => '0x' + BigInt(v).toString(16);
const err = (sig) => Object.assign(new Error('execution reverted'), { code: 3, data: selector(sig) });

// The vault is the page's live deployment (DEPLOYMENTS[0]), so requests and drafts made against the
// mock resolve to the deployment the page uses.
export const ADDR = {
  vault: DEPLOYMENTS[0].vault,
  usdc: '0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e',
  eth: '0x0000000000000000000000000000000000000000',
  me: '0x5a1e0000000000000000000000000000000c0de5',
};

export function createMockChain(opts = {}) {
  const isVault = (a) => a === ADDR.vault || (opts.altVault && a === opts.altVault.toLowerCase());
  // opts.desk = { address, feeBps, vault? }: a DepositDesk (vault/src/desk/DepositDesk.sol) in front of the vault
  // (the same object, so a test can change feeBps between the checks and the send)
  const D = opts.desk || null;
  if (D) Object.assign(D, { feeBps: D.feeBps ?? 300n, vault: D.vault ?? ADDR.vault, address: D.address.toLowerCase() });
  const isDesk = (a) => !!D && a === D.address;
  const chainId = BigInt(opts.chainId ?? 11155111);
  const S = {
    head: BigInt(opts.head ?? 12125800),
    finalizedLag: BigInt(opts.finalizedLag ?? 70),
    finalizedSupported: opts.finalizedSupported ?? true,
    globalFrozen: false,
    tokens: {
      [ADDR.eth]: { registered: true, frozen: false, decimals: 18, denoms: [10n ** 16n, 10n ** 17n, 10n ** 18n], fees: [0n, 0n, 0n], cap: { window: 86400n, cap: 5n * 10n ** 18n, used: 10n ** 16n }, weight: 3_000_000_000n },
      [ADDR.usdc]: { registered: true, frozen: false, decimals: 6, denoms: [100_000_000n, 1_000_000_000n, 10_000_000_000n], fees: [0n, 0n, 0n], cap: { window: 86400n, cap: 50_000_000_000n, used: 1_300_000_000n }, weight: 10n ** 18n },
    },
    gcap: { window: 86400n, cap: 100_000_000_000n, used: 1_330_000_000n },
    used: new Set(),
    counters: new Map(), // `${token}:${i}` -> n
    leaves: 0n,
    eth: new Map([[ADDR.me, BigInt(opts.ethBalance ?? 2n * 10n ** 17n)]]),
    usdcBal: new Map([[ADDR.me, BigInt(opts.usdcBalance ?? 0n)]]),
    allowance: new Map(), // owner -> allowance for the vault
    deskAllowance: new Map(), // owner -> allowance for the desk
    deskFees: new Map(), // token -> bridge fees held by the desk
    logs: [],
    txs: new Map(),
    receipts: new Map(),
    pendingPolls: new Map(),
    blocks: new Map(),
    nonce: 0,
  };
  // seed: the five Sepolia v2 deposits (4 dUSDC, 1 ETH)
  for (const [tok, i] of [[ADDR.usdc, 0], [ADDR.usdc, 0], [ADDR.usdc, 1], [ADDR.usdc, 0], [ADDR.eth, 0]]) {
    const k = `${tok}:${i}`;
    S.counters.set(k, (S.counters.get(k) || 0n) + 1n);
    S.leaves++;
  }
  Object.assign(S, opts.state || {});

  const blockHash = (n) => {
    if (!S.blocks.has(n)) S.blocks.set(n, '0x' + W(0xb10c000000n + n));
    return S.blocks.get(n);
  };

  function checkDeposit(from, token, idx, c, value, { payer = true } = {}) {
    const t = S.tokens[token];
    if (S.globalFrozen || (t && t.frozen)) throw err('IsFrozen()');
    if (!t || !t.registered) throw err('NotRegistered()');
    if (idx >= t.denoms.length) throw err('BadDenomination()');
    const denom = t.denoms[idx];
    const total = denom + t.fees[idx];
    if (!payer) {
      /* forwarded by the desk, which holds exactly the vault's amount */
    } else if (token === ADDR.eth) {
      if (value !== total) throw err('WrongAmount()');
      if ((S.eth.get(from) || 0n) < value) throw Object.assign(new Error('insufficient funds for gas * price + value'), { code: -32000 });
    } else {
      const al = S.allowance.get(from) || 0n;
      if (al < total) throw Object.assign(new Error('execution reverted'), { code: 3, data: selector('ERC20InsufficientAllowance(address,uint256,uint256)') + A(ADDR.vault) + W(al) + W(total) });
      if ((S.usdcBal.get(from) || 0n) < total) throw Object.assign(new Error('execution reverted'), { code: 3, data: selector('ERC20InsufficientBalance(address,uint256,uint256)') + A(from) + W(S.usdcBal.get(from) || 0n) + W(total) });
    }
    if (S.used.has(c)) throw err('CommitmentUsed()');
    if (t.cap.window && t.cap.used + denom > t.cap.cap) throw err('CapExceeded()');
    return { t, denom, total };
  }

  function applyDeposit(from, token, idx, c, value, { payer = true } = {}) {
    const { t, denom, total } = checkDeposit(from, token, idx, c, value, { payer });
    if (!payer) {
      /* the desk paid */
    } else if (token === ADDR.eth) S.eth.set(from, S.eth.get(from) - total);
    else {
      S.usdcBal.set(from, S.usdcBal.get(from) - total);
      S.allowance.set(from, S.allowance.get(from) - total);
    }
    S.used.add(c);
    t.cap.used += denom;
    const k = `${token}:${idx}`;
    S.counters.set(k, (S.counters.get(k) || 0n) + 1n);
    const leaf = S.leaves++;
    return { address: opts.altVault ? opts.altVault.toLowerCase() : ADDR.vault, topics: [DEPOSIT_TOPIC, '0x' + A(token)], data: '0x' + W(idx) + c.slice(2) + W(leaf) + W(0xabcdefn + leaf) };
  }

  // ---- the desk: quote, then the payer's checks, then the vault's rules on the forwarded amount
  function deskQuote(token, idx) {
    const t = S.tokens[token] || { denoms: [], fees: [] };
    if (idx >= t.denoms.length) throw err('BadDenomination()');
    const denom = t.denoms[idx];
    const vaultFee = t.fees[idx];
    const bridgeFee = (denom * BigInt(D.feeBps)) / 10000n;
    return { denom, vaultFee, bridgeFee, total: denom + vaultFee + bridgeFee };
  }

  function deskCheck(from, token, idx, c, value) {
    if (token !== ADDR.eth && value) throw err('UnexpectedValue()');
    const q = deskQuote(token, idx);
    if (token === ADDR.eth) {
      if (value !== q.total) throw err('WrongAmount()');
      if ((S.eth.get(from) || 0n) < value) throw Object.assign(new Error('insufficient funds for gas * price + value'), { code: -32000 });
    } else {
      const al = S.deskAllowance.get(from) || 0n;
      if (al < q.total) throw Object.assign(new Error('execution reverted'), { code: 3, data: selector('ERC20InsufficientAllowance(address,uint256,uint256)') + A(D.address) + W(al) + W(q.total) });
      if ((S.usdcBal.get(from) || 0n) < q.total) throw Object.assign(new Error('execution reverted'), { code: 3, data: selector('ERC20InsufficientBalance(address,uint256,uint256)') + A(from) + W(S.usdcBal.get(from) || 0n) + W(q.total) });
    }
    checkDeposit(D.address, token, idx, c, 0n, { payer: false });
    return q;
  }

  function applyDeskDeposit(from, token, idx, c, value, pk) {
    const q = deskCheck(from, token, idx, c, value);
    if (token === ADDR.eth) S.eth.set(from, S.eth.get(from) - q.total);
    else {
      S.usdcBal.set(from, S.usdcBal.get(from) - q.total);
      S.deskAllowance.set(from, S.deskAllowance.get(from) - q.total);
    }
    S.deskFees.set(token, (S.deskFees.get(token) || 0n) + q.bridgeFee);
    const vaultLog = applyDeposit(D.address, token, idx, c, 0n, { payer: false });
    const leaf = BigInt('0x' + vaultLog.data.slice(2 + 128, 2 + 192));
    const feeLog = { address: D.address, topics: [FEE_PAID_TOPIC, '0x' + A(token), pk, c], data: '0x' + W(idx) + W(leaf) + W(q.bridgeFee) };
    return [vaultLog, feeLog];
  }

  function ethCall(tx) {
    const to = tx.to.toLowerCase();
    const d = tx.data.toLowerCase();
    const sel = d.slice(0, 10);
    const arg = (i) => d.slice(10 + 64 * i, 10 + 64 * (i + 1));
    const addr = (i) => '0x' + arg(i).slice(24);
    const from = (tx.from || '0x' + '0'.repeat(40)).toLowerCase();
    const is = (sig) => sel === selector(sig);
    if (isVault(to)) {
      if (is('tokenConfig(address)')) {
        const t = S.tokens[addr(0)] || { registered: false, frozen: false, decimals: 0, denoms: [], fees: [] };
        const n = t.denoms.length;
        const arr = (xs) => W(xs.length) + xs.map(W).join('');
        const head = 9 * 32;
        const o1 = head;
        const o2 = o1 + 32 * (n + 1);
        const o3 = o2 + 32 * (n + 1);
        return '0x' + W(t.registered ? 1 : 0) + W(t.frozen ? 1 : 0) + W(0) + W(t.decimals) + W(86400) + W(o1) + W(o2) + W(o3) + W(0) + arr(t.denoms) + arr(t.fees) + arr(t.fees.map(() => 0n));
      }
      if (is('capState(address)')) {
        const t = S.tokens[addr(0)];
        return '0x' + W(t ? t.cap.window : 0) + W(t ? t.cap.cap : 0) + W(t ? t.cap.cap : 0) + W(t ? t.cap.used : 0) + W(0);
      }
      if (is('globalCapState()')) return '0x' + W(S.gcap.window) + W(S.gcap.cap) + W(S.gcap.cap) + W(S.gcap.used) + W(0);
      if (is('capWeight(address)')) return '0x' + W(S.tokens[addr(0)]?.weight || 0n);
      if (is('globalDepositsFrozen()')) return '0x' + W(S.globalFrozen ? 1 : 0);
      if (is('commitmentUsed(bytes32)')) return '0x' + W(S.used.has('0x' + arg(0)) ? 1 : 0);
      if (is('counters(address,uint256)')) return '0x' + W(S.counters.get(`${addr(0)}:${Number(BigInt('0x' + arg(1)))}`) || 0n) + W(0) + W(0) + W(0);
      if (is('depositCount()')) return '0x' + W(S.leaves);
      if (is('cfgId()')) return S.cfgId || DEPLOYMENTS[0].cfgId;
      if (is('depositETH(uint32,bytes32)')) {
        checkDeposit(from, ADDR.eth, Number(BigInt('0x' + arg(0))), '0x' + arg(1), BigInt(tx.value || 0));
        return '0x' + W(S.leaves);
      }
      if (is('deposit(address,uint32,bytes32)')) {
        checkDeposit(from, addr(0), Number(BigInt('0x' + arg(1))), '0x' + arg(2), 0n);
        return '0x' + W(S.leaves);
      }
    }
    if (isDesk(to)) {
      if (is('vault()')) return '0x' + A(D.vault);
      if (is('feeBps()')) return '0x' + W(D.feeBps);
      if (is('quote(address,uint32)')) {
        const q = deskQuote(addr(0), Number(BigInt('0x' + arg(1))));
        return '0x' + W(q.denom) + W(q.vaultFee) + W(q.bridgeFee) + W(q.total);
      }
      if (is('depositETHWithFee(uint32,bytes32,bytes32)')) {
        deskCheck(from, ADDR.eth, Number(BigInt('0x' + arg(0))), '0x' + arg(1), BigInt(tx.value || 0));
        return '0x' + W(S.leaves);
      }
      if (is('depositWithFee(address,uint32,bytes32,bytes32)')) {
        deskCheck(from, addr(0), Number(BigInt('0x' + arg(1))), '0x' + arg(2), BigInt(tx.value || 0));
        return '0x' + W(S.leaves);
      }
    }
    if (to === ADDR.usdc) {
      if (is('balanceOf(address)')) return '0x' + W(S.usdcBal.get(addr(0)) || 0n);
      if (is('allowance(address,address)')) return '0x' + W((isDesk(addr(1)) ? S.deskAllowance : S.allowance).get(addr(0)) || 0n);
      if (is('approve(address,uint256)')) return '0x' + W(1);
      if (is('symbol()')) return '0x' + W(32) + W(5) + '6455534443'.padEnd(64, '0');
      if (is('decimals()')) return '0x' + W(6);
    }
    return '0x';
  }

  function mine(tx, logs) {
    S.head += 1n;
    const hash = '0x' + W(0x7e000000n + BigInt(++S.nonce) * 7919n);
    const receipt = { transactionHash: hash, blockNumber: hex(S.head), blockHash: blockHash(S.head), status: '0x1', gasUsed: hex(470_000n), effectiveGasPrice: hex(1_000_000_000n), logs: logs.map((l) => ({ ...l, transactionHash: hash, blockNumber: hex(S.head) })), from: tx.from };
    S.txs.set(hash, { hash, from: tx.from, to: tx.to, input: tx.data, blockNumber: hex(S.head) });
    S.receipts.set(hash, receipt);
    S.pendingPolls.set(hash, opts.receiptDelayPolls ?? 1);
    for (const l of receipt.logs) S.logs.push(l);
    return hash;
  }

  return { S, isVault, isDesk, applyDeskDeposit, chainId, timeOf: (n) => 1794540000n + (BigInt(n) - 12120000n) * 12n, ethCall, checkDeposit, applyDeposit, mine, blockHash, advance: (n = 1n) => (S.head += BigInt(n)) };
}

/**
 * An EIP-1193 wallet over a mock chain. `behaviour` lets a test or the harness script
 * the person: { rejectConnect, rejectNext: Set(['approve','deposit','switch']), switchError }.
 */
export function createMockWallet(chain, { walletChainId = 1n, behaviour = {}, onSend } = {}) {
  const listeners = new Map();
  let current = BigInt(walletChainId);
  let connected = false;
  const emit = (ev, v) => (listeners.get(ev) || []).forEach((fn) => fn(v));
  const { S } = chain;
  const reject = () => Object.assign(new Error('User rejected the request.'), { code: 4001 });
  const calls = [];
  const added = [];
  const flaky = new Map();

  const provider = {
    isMockWallet: true,
    calls,
    added,
    /** Test hook: the wallet emits an EIP-1193 event (accountsChanged, chainChanged, disconnect). */
    emit(ev, v) {
      if (ev === 'chainChanged') current = BigInt(v);
      emit(ev, v);
    },
    async request({ method, params = [] }) {
      calls.push(method);
      if (behaviour.brokenRpc && !['eth_chainId', 'eth_accounts', 'eth_requestAccounts', 'wallet_switchEthereumChain', 'wallet_addEthereumChain'].includes(method)) {
        // a wallet whose own Sepolia RPC is gone (rpc.sepolia.org answering 404, 2026-11-13)
        if (!(method === 'eth_blockNumber' && behaviour.healOnAdd && added.length)) throw Object.assign(new Error('Request failed with status code 404'), { code: -32603 });
      }
      const left = flaky.get(method) || 0;
      if (left > 0) {
        flaky.set(method, left - 1);
        throw Object.assign(new Error('upstream timeout'), { code: -32603 });
      }
      switch (method) {
        case 'eth_chainId':
          return hex(current);
        case 'eth_accounts':
          return (connected || behaviour.alreadyConnected) && !behaviour.locked ? [ADDR.me] : [];
        case 'eth_requestAccounts':
          if (behaviour.rejectConnect) throw reject();
          if (behaviour.locked) return [];
          connected = true;
          return [ADDR.me];
        case 'wallet_addEthereumChain': {
          if (behaviour.rejectNext?.has('add')) {
            behaviour.rejectNext.delete('add');
            throw reject();
          }
          added.push(params[0]);
          behaviour.switchError = undefined;
          current = BigInt(params[0].chainId);
          emit('chainChanged', hex(current));
          return null;
        }
        case 'wallet_switchEthereumChain': {
          if (behaviour.rejectNext?.has('switch')) {
            behaviour.rejectNext.delete('switch');
            throw reject();
          }
          if (behaviour.switchError) throw Object.assign(new Error('Unrecognized chain'), { code: behaviour.switchError });
          current = BigInt(params[0].chainId);
          emit('chainChanged', hex(current));
          return null;
        }
      }
      if (current !== chain.chainId) {
        // a wallet on the wrong network answers reads from that other network
        if (method === 'eth_call') return '0x';
        if (method === 'eth_getCode') return '0x';
      }
      switch (method) {
        case 'eth_blockNumber':
          return hex(S.head);
        case 'eth_gasPrice':
          return hex(1_000_000_000n);
        case 'eth_getBalance':
          return hex(S.eth.get(params[0].toLowerCase()) || 0n);
        case 'eth_getCode':
          return chain.isVault(params[0].toLowerCase()) || chain.isDesk(params[0].toLowerCase()) || params[0].toLowerCase() === ADDR.usdc ? '0x6080' : '0x';
        case 'eth_call':
          return chain.ethCall(params[0]);
        case 'eth_estimateGas':
          chain.ethCall(params[0]);
          return hex(params[0].data.startsWith(selector('approve(address,uint256)')) ? 46_000n : 470_000n);
        case 'eth_getBlockByNumber': {
          const tag = params[0];
          if (tag === 'finalized') {
            if (!S.finalizedSupported) throw Object.assign(new Error('unknown block tag'), { code: -32602 });
            S.head += 8n; // time passes between polls
            const f = S.head - S.finalizedLag;
            return { number: hex(f), hash: chain.blockHash(f), timestamp: hex(chain.timeOf(f)) };
          }
          const n = tag === 'latest' ? S.head : BigInt(tag);
          return { number: hex(n), hash: chain.blockHash(n), timestamp: hex(chain.timeOf(n)) };
        }
        case 'eth_getTransactionReceipt': {
          const left = S.pendingPolls.get(params[0]) || 0;
          if (left > 0) {
            S.pendingPolls.set(params[0], left - 1);
            return null;
          }
          return S.receipts.get(params[0]) || null;
        }
        case 'eth_getTransactionByHash':
          return S.txs.get(params[0]) || null;
        case 'eth_getLogs': {
          const f = params[0];
          return S.logs.filter((l) => chain.isVault(f.address.toLowerCase()) && l.topics[0] === f.topics[0] && (!f.topics[1] || l.topics[1] === f.topics[1]));
        }
        case 'eth_sendTransaction': {
          if (behaviour.sendRpcFailsOnce) {
            // the wallet's own RPC fails while it prepares the transaction (seen live with MetaMask)
            behaviour.sendRpcFailsOnce = false;
            throw Object.assign(new Error('Request failed with status code 404'), { code: -32603 });
          }
          const tx = { ...params[0], from: params[0].from.toLowerCase(), to: params[0].to.toLowerCase() };
          const isApprove = tx.data.startsWith(selector('approve(address,uint256)'));
          const kind = isApprove ? 'approve' : 'deposit';
          if (behaviour.hangSend) return new Promise(() => {});
          if (behaviour.rejectNext?.has(kind)) {
            behaviour.rejectNext.delete(kind);
            throw reject();
          }
          onSend?.(kind, tx);
          if (isApprove) {
            const amount = BigInt('0x' + tx.data.slice(74, 138));
            const spender = '0x' + tx.data.slice(34, 74);
            (chain.isDesk(spender) ? S.deskAllowance : S.allowance).set(tx.from, amount);
            return chain.mine(tx, []);
          }
          if (behaviour.revertOnChain) {
            const h = chain.mine(tx, []);
            S.receipts.get(h).status = '0x0';
            return h;
          }
          const d = tx.data;
          let log;
          if (chain.isDesk(tx.to)) {
            const w = (i) => d.slice(10 + 64 * i, 10 + 64 * (i + 1));
            const logs = d.startsWith(selector('depositETHWithFee(uint32,bytes32,bytes32)'))
              ? chain.applyDeskDeposit(tx.from, ADDR.eth, Number(BigInt('0x' + w(0))), '0x' + w(1), BigInt(tx.value || 0), '0x' + w(2))
              : chain.applyDeskDeposit(tx.from, '0x' + w(0).slice(24), Number(BigInt('0x' + w(1))), '0x' + w(2), BigInt(tx.value || 0), '0x' + w(3));
            return chain.mine(tx, logs);
          }
          if (d.startsWith(selector('depositETH(uint32,bytes32)'))) {
            log = chain.applyDeposit(tx.from, ADDR.eth, Number(BigInt('0x' + d.slice(10, 74))), '0x' + d.slice(74, 138), BigInt(tx.value || 0));
          } else {
            log = chain.applyDeposit(tx.from, '0x' + d.slice(34, 74), Number(BigInt('0x' + d.slice(74, 138))), '0x' + d.slice(138, 202), 0n);
          }
          return chain.mine(tx, [log]);
        }
      }
      throw Object.assign(new Error(`mock wallet: ${method} not supported`), { code: 4200 });
    },
    /** Test hook: the wallet changes network without emitting chainChanged. */
    setChainSilently(id) {
      current = BigInt(id);
    },
    /** Test hook: the next `n` reads of `method` fail like a flaky RPC. */
    failNext(method, n = 1) {
      flaky.set(method, n);
    },
    on(ev, fn) {
      if (!listeners.has(ev)) listeners.set(ev, []);
      listeners.get(ev).push(fn);
    },
    removeListener(ev, fn) {
      listeners.set(ev, (listeners.get(ev) || []).filter((f) => f !== fn));
    },
  };
  return provider;
}
