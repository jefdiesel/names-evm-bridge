// BridgeVault and ERC-20 calls, return decoding, and revert decoding.
// Signatures follow vault/src/BridgeVault.sol; every selector and every calldata shape is
// checked against `cast` output in test/abi.test.mjs.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

import { encodeCall, words, asUint, asBool, asUintArray, asAddress, asBytes32, strip0x, decodeString } from './abi.js';
import { selector, keccakText } from './keccak.js';

export const ETH = '0x0000000000000000000000000000000000000000';

export const SIG = {
  deposit: 'deposit(address,uint32,bytes32)',
  depositETH: 'depositETH(uint32,bytes32)',
  tokenConfig: 'tokenConfig(address)',
  capState: 'capState(address)',
  globalCapState: 'globalCapState()',
  capWeight: 'capWeight(address)',
  globalDepositsFrozen: 'globalDepositsFrozen()',
  commitmentUsed: 'commitmentUsed(bytes32)',
  counters: 'counters(address,uint256)',
  depositCount: 'depositCount()',
  depositRoot: 'depositRoot()',
  cfgId: 'cfgId()',
  balanceOf: 'balanceOf(address)',
  allowance: 'allowance(address,address)',
  approve: 'approve(address,uint256)',
  decimals: 'decimals()',
  symbol: 'symbol()',
  // DepositDesk (vault/src/desk/DepositDesk.sol): the bridge fee in front of the vault
  depositETHWithFee: 'depositETHWithFee(uint32,bytes32,bytes32)',
  depositWithFee: 'depositWithFee(address,uint32,bytes32,bytes32)', // the deployed desks (v2 to v5): no cap
  depositWithFeeMax: 'depositWithFee(address,uint32,bytes32,bytes32,uint256)', // a desk with `deskMaxTotal`: reverts above maxTotal
  deskQuote: 'quote(address,uint32)',
  deskVault: 'vault()',
  deskFeeBps: 'feeBps()',
};

export const FEE_PAID_EVENT = 'FeePaid(address,bytes32,bytes32,uint32,uint64,uint256)';
export const FEE_PAID_TOPIC = keccakText(FEE_PAID_EVENT);
export const ZERO32 = '0x' + '0'.repeat(64);

export const DEPOSIT_EVENT = 'Deposit(address,uint32,bytes32,uint64,bytes32)';
export const DEPOSIT_TOPIC = keccakText(DEPOSIT_EVENT);

export const call = {
  deposit: (token, denomIndex, c) => encodeCall(SIG.deposit, [token, denomIndex, c]),
  depositETH: (denomIndex, c) => encodeCall(SIG.depositETH, [denomIndex, c]),
  tokenConfig: (token) => encodeCall(SIG.tokenConfig, [token]),
  capState: (token) => encodeCall(SIG.capState, [token]),
  globalCapState: () => encodeCall(SIG.globalCapState),
  capWeight: (token) => encodeCall(SIG.capWeight, [token]),
  globalDepositsFrozen: () => encodeCall(SIG.globalDepositsFrozen),
  commitmentUsed: (c) => encodeCall(SIG.commitmentUsed, [c]),
  counters: (token, i) => encodeCall(SIG.counters, [token, i]),
  depositCount: () => encodeCall(SIG.depositCount),
  depositRoot: () => encodeCall(SIG.depositRoot),
  cfgId: () => encodeCall(SIG.cfgId),
  balanceOf: (who) => encodeCall(SIG.balanceOf, [who]),
  allowance: (owner, spender) => encodeCall(SIG.allowance, [owner, spender]),
  approve: (spender, amount) => encodeCall(SIG.approve, [spender, amount]),
  decimals: () => encodeCall(SIG.decimals),
  symbol: () => encodeCall(SIG.symbol),
  depositETHWithFee: (denomIndex, c, pk) => encodeCall(SIG.depositETHWithFee, [denomIndex, c, pk]),
  depositWithFee: (token, denomIndex, c, pk) => encodeCall(SIG.depositWithFee, [token, denomIndex, c, pk]),
  depositWithFeeMax: (token, denomIndex, c, pk, maxTotal) => encodeCall(SIG.depositWithFeeMax, [token, denomIndex, c, pk, maxTotal]),
  deskQuote: (token, denomIndex) => encodeCall(SIG.deskQuote, [token, denomIndex]),
  deskVault: () => encodeCall(SIG.deskVault),
  deskFeeBps: () => encodeCall(SIG.deskFeeBps),
};

export const decode = {
  tokenConfig(data) {
    const w = words(data);
    return {
      registered: asBool(w[0]),
      depositsFrozen: asBool(w[1]),
      releasesFrozen: asBool(w[2]),
      decimals: Number(asUint(w[3])),
      releaseDelay: asUint(w[4]),
      denoms: asUintArray(w, 5),
      depositFees: asUintArray(w, 6),
      withdrawFees: asUintArray(w, 7),
      accruedFees: asUint(w[8]),
    };
  },
  capState(data) {
    const w = words(data);
    return { window: asUint(w[0]), depositCap: asUint(w[1]), releaseCap: asUint(w[2]), depUsed: asUint(w[3]), relUsed: asUint(w[4]) };
  },
  counters(data) {
    const w = words(data);
    return { deposits: asUint(w[0]), claims: asUint(w[1]), burns: asUint(w[2]), releases: asUint(w[3]) };
  },
  /** DepositDesk.quote: (denom, vaultFee, bridgeFee, total). */
  deskQuote(data) {
    const w = words(data);
    if (w.length !== 4) throw new Error('quote: expected 4 words');
    return { denom: asUint(w[0]), vaultFee: asUint(w[1]), bridgeFee: asUint(w[2]), total: asUint(w[3]) };
  },
  address: (data) => asAddress(words(data)[0]),
  uint: (data) => asUint(words(data)[0]),
  bool: (data) => asBool(words(data)[0]),
  bytes32: (data) => asBytes32(words(data)[0]),
  string: (data) => decodeString(data),
};

/** Decode a Deposit log ({topics, data}) or return null if it is not one. */
export function decodeDepositLog(log) {
  if (!log || !log.topics || log.topics[0]?.toLowerCase() !== DEPOSIT_TOPIC) return null;
  const w = words(log.data);
  return {
    token: asAddress(strip0x(log.topics[1]).toLowerCase()),
    denomIndex: Number(asUint(w[0])),
    commitment: asBytes32(w[1]),
    leafIndex: asUint(w[2]),
    root: asBytes32(w[3]),
  };
}

// ---------------------------------------------------------------- errors

/**
 * Every custom error the deposit path can hit (vault, OpenZeppelin SafeERC20 / ERC20 /
 * ReentrancyGuard), plus Solidity's Error(string) and Panic(uint256). Messages are
 * written for the person depositing: what happened, whether anything was spent, what to
 * do. They never mention any other deposit.
 */
const ERRORS = [
  ['IsFrozen()', 'frozen', 'Deposits are paused for this token or for the whole vault. The operator can lift the pause; try again later.'],
  ['NotRegistered()', 'not-registered', 'This vault does not accept this token. Ask your Logos wallet for a request for a supported token.'],
  ['BadDenomination()', 'bad-denomination', 'This vault has no such amount for this token. The request does not match the vault; make a new one in your Logos wallet.'],
  ['WrongAmount()', 'wrong-amount', 'The amount sent did not match the deposit plus its fees (the bridge fee may have just changed; check again), or the token takes a fee on transfer, which cannot be bridged.'],
  ['CapExceeded()', 'cap-exceeded', 'The vault’s deposit limit for the current period is reached. The limit frees up over time; try again later or use a smaller denomination.'],
  ['CommitmentUsed()', 'commitment-used', 'This commitment has already been deposited. A commitment can be used once; make a new request in your Logos wallet.'],
  ['UnexpectedValue()', 'unexpected-value', 'The transaction sent ETH where none was expected.'],
  ['ReentrancyGuardReentrantCall()', 'reentrancy', 'The vault refused a nested call.'],
  ['SafeERC20FailedOperation(address)', 'token-transfer-failed', 'The token refused the transfer. Check your token balance and that the vault is approved for the full amount.'],
  ['ERC20InsufficientBalance(address,uint256,uint256)', 'insufficient-balance', 'Your token balance is lower than the deposit amount.'],
  ['ERC20InsufficientAllowance(address,uint256,uint256)', 'insufficient-allowance', 'The vault (or the deposit desk) is not approved to take this amount yet. Approve it first.'],
  ['ERC20InvalidApprover(address)', 'invalid-approver', 'The token refused the approval.'],
  ['ERC20InvalidSpender(address)', 'invalid-spender', 'The token refused the approval.'],
];

export const ERROR_TABLE = ERRORS.map(([sig, code, message]) => ({ sig, code, message, selector: selector(sig) }));
const BY_SELECTOR = new Map(ERROR_TABLE.map((e) => [e.selector, e]));

const PANICS = { 0x01: 'assertion failed', 0x11: 'arithmetic overflow', 0x12: 'division by zero', 0x32: 'array index out of bounds' };

/**
 * Turn revert data into { code, message, sig? }. Unknown data gives code "unknown".
 */
export function decodeRevert(data) {
  const hex = typeof data === 'string' ? data.toLowerCase() : '';
  if (!/^0x[0-9a-f]*$/.test(hex) || hex.length < 10) {
    return { code: 'unknown', message: 'The transaction would fail, and the node gave no reason.' };
  }
  const sel = hex.slice(0, 10);
  const known = BY_SELECTOR.get(sel);
  if (known) return { code: known.code, message: known.message, sig: known.sig };
  if (sel === '0x08c379a0') {
    try {
      return { code: 'revert-string', message: `The contract refused: “${decodeString('0x' + hex.slice(10))}”.` };
    } catch {
      /* fall through */
    }
  }
  if (sel === '0x4e487b71' && hex.length >= 74) {
    const n = Number(BigInt('0x' + hex.slice(10, 74)));
    return { code: 'panic', message: `The contract stopped with an internal error (${PANICS[n] || 'panic 0x' + n.toString(16)}).` };
  }
  return { code: 'unknown', message: `The transaction would fail with an error this page does not know (${sel}).` };
}

/**
 * Wallets put revert data in different places (error.data, error.data.data,
 * error.data.originalError.data, error.cause...). Find the first hex string that looks
 * like revert data.
 */
export function extractRevertData(err, depth = 0) {
  if (err == null || depth > 5) return null;
  if (typeof err === 'string') return /^0x[0-9a-fA-F]{8,}$/.test(err) ? err : null;
  if (typeof err !== 'object') return null;
  for (const key of ['data', 'originalError', 'cause', 'error', 'info']) {
    const found = extractRevertData(err[key], depth + 1);
    if (found) return found;
  }
  const m = typeof err.message === 'string' && /(0x[0-9a-fA-F]{8,})/.exec(err.message);
  if (m && BY_SELECTOR.has(m[1].slice(0, 10).toLowerCase())) return m[1];
  return null;
}
