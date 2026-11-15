// Making a deposit here, in the browser: the Logos account, the secret, the commitment and the
// claim file. No DOM, no network: WebCrypto only, so the same code runs in Node for the tests.
//
// The commitment is the bridge program's own (its deposit_commitment):
//   C = SHA-256("RFP022/dep" || k || dest(32) || token(20) || uint256(amount) || cfgId(32))
// with the bridge's two-level secret convention:
//   s = 32 random bytes (the master secret, never in any transaction)
//   k = SHA-256("RFP022/ck" || s) (the claim key: the bridge CLI's `claim --secret k`)
// test/vectors/commitment-cli.json holds commitments made by the real CLI for these rules.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const enc = new TextEncoder();

export class AccountError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

export function base58Decode(text) {
  let n = 0n;
  for (const ch of text) {
    const v = ALPHABET.indexOf(ch);
    if (v < 0) throw new AccountError('charset', `“${ch}” cannot be in a Logos account.`);
    n = n * 58n + BigInt(v);
  }
  const bytes = [];
  while (n > 0n) {
    bytes.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  for (const ch of text) {
    if (ch !== '1') break;
    bytes.unshift(0);
  }
  return Uint8Array.from(bytes);
}

export function base58Encode(bytes) {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let s = '';
  while (n > 0n) {
    s = ALPHABET[Number(n % 58n)] + s;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    s = '1' + s;
  }
  return s;
}

const EXAMPLE = 'Public/2if1EJVc…6HUC';

/**
 * A LEZ account as Names Wallet shows it (Receive LEZ and tokens → Your public account:
 * `Public/<id>`) or as the LEZ wallet prints it: base58 of 32 bytes, with or without the
 * `Public/` or `Private/` label. The bridge accepts public and private accounts alike; the
 * commitment binds the bare 32 bytes (the CLI's `--dest` takes the bare id only).
 * Returns { id (bare base58), bytes, label ('Public', 'Private' or '') }. Everything else is
 * refused with a plain explanation, never a character-level complaint: in particular a
 * 64-character hex key (a Logos Blockchain key) is NOT read as an account, because funds sent
 * to such a reading could never be claimed.
 */
export function parseLogosAccount(text) {
  const t = String(text ?? '').replace(/[\s\u200b-\u200d\ufeff]/g, '');
  if (!t) throw new AccountError('empty', 'Paste your LEZ account.');
  if (/^0x[0-9a-f]{40}$/i.test(t)) throw new AccountError('eth-address', 'That is an Ethereum address. Paste your LEZ account.');
  if (/^(0x)?[0-9a-f]{32,}…?$/i.test(t)) throw new AccountError('blockchain-key', `This looks like a Logos Blockchain key. The bridge sends to a LEZ account instead; it looks like ${EXAMPLE}.`);
  const m = t.match(/^(public|private)\/(.*)$/i);
  const label = m ? m[1][0].toUpperCase() + m[1].slice(1).toLowerCase() : '';
  const bare = m ? m[2] : t;
  if (!m && /^[a-z0-9][a-z0-9-]{0,30}(\.lez)?$/.test(bare) && bare.length < 32) throw new AccountError('name', 'Names are not supported here yet. Paste the LEZ account.');
  if (!/^[1-9A-HJ-NP-Za-km-z]+$/.test(bare)) throw new AccountError('unrecognised', `That doesn’t look like a LEZ account. It looks like ${EXAMPLE}.`);
  const bytes = base58Decode(bare);
  if (bytes.length !== 32) throw new AccountError('incomplete', 'That LEZ account looks incomplete. Copy it again from Names Wallet.');
  return { id: bare, bytes, label };
}

export const hexToBytes = (hex) => {
  const h = String(hex).replace(/^0x/i, '');
  if (h.length % 2 || /[^0-9a-f]/i.test(h)) throw new Error(`not hex: ${hex}`);
  return Uint8Array.from(h.match(/../g) || [], (b) => parseInt(b, 16));
};
export const bytesToHex = (b) => '0x' + [...b].map((x) => x.toString(16).padStart(2, '0')).join('');

export function u256(value) {
  const out = new Uint8Array(32);
  let v = BigInt(value);
  for (let i = 31; i >= 0; i--) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

const concat = (parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
};

export async function sha256(...parts) {
  return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', concat(parts)));
}

/** 32 fresh random bytes from the browser's cryptographic generator. */
export function newMasterSecret(random = (b) => globalThis.crypto.getRandomValues(b)) {
  return random(new Uint8Array(32));
}

export const claimKey = (s) => sha256(enc.encode('RFP022/ck'), s);

export function depositCommitment(k, dest, token, amount, cfgId) {
  return sha256(enc.encode('RFP022/dep'), k, dest, hexToBytes(token), u256(amount), hexToBytes(cfgId));
}

/** Everything a deposit made here needs, from a fresh secret: { s, k, commitment } as hex. */
export async function makeDeposit({ dest, token, amount, cfgId, random }) {
  const s = newMasterSecret(random);
  const k = await claimKey(s);
  const c = await depositCommitment(k, dest, token, amount, cfgId);
  return { s: bytesToHex(s), k: bytesToHex(k), commitment: bytesToHex(c) };
}

export const CLAIM_FILE_FORMAT = 'logos-bridge-claim';

export function claimFileName(commitment) {
  return `logos-bridge-claim-${String(commitment).slice(2, 10)}.json`;
}

/**
 * The claim file: the only copy of the secret. `deposit` is filled in once the deposit is on chain.
 * Optional, only when the deposit goes through the deposit desk: `bridgeFee` (desk, feeBps,
 * amount in the token's base units). Readers that do not know it ignore it; the format stays
 * version 1. (Files made between 2026-11-13 and the relayed claim may also carry `lezPublicKey`.)
 */
export function claimFile({ chainId, network, vault, cfgId, token, symbol, decimals, denomIndex, amount, amountText, account, s, k, commitment, createdAt, deposit = null, bridgeFee = null }) {
  return {
    format: CLAIM_FILE_FORMAT,
    version: 1,
    warning: 'Without this file the deposit cannot be claimed, and nobody can recover it for you. Whoever has it can claim the deposit: keep it private.',
    network,
    chainId: String(chainId),
    vault,
    cfgId,
    token: { address: token, symbol, decimals },
    denominationIndex: denomIndex,
    amount: String(amount),
    amountText,
    logosAccount: account,
    masterSecret: s,
    claimKey: k,
    commitment,
    createdAt,
    deposit: deposit ? { tx: deposit.tx, block: String(deposit.block), leafIndex: deposit.leafIndex == null ? null : String(deposit.leafIndex) } : null,
    ...(bridgeFee ? { bridgeFee: { desk: bridgeFee.desk, feeBps: String(bridgeFee.feeBps), amount: String(bridgeFee.amount) } } : {}),
  };
}

/** Checks a claim file's shape and that its commitment follows from its secret. */
export async function verifyClaimFile(f) {
  const need = ['format', 'version', 'chainId', 'vault', 'cfgId', 'token', 'denominationIndex', 'amount', 'logosAccount', 'masterSecret', 'claimKey', 'commitment', 'createdAt'];
  for (const key of need) if (f?.[key] === undefined) throw new Error(`claim file: missing ${key}`);
  if (f.format !== CLAIM_FILE_FORMAT || f.version !== 1) throw new Error('claim file: unknown format');
  const k = bytesToHex(await claimKey(hexToBytes(f.masterSecret)));
  if (k !== f.claimKey) throw new Error('claim file: claimKey does not follow from masterSecret');
  const { bytes } = parseLogosAccount(f.logosAccount);
  const c = bytesToHex(await depositCommitment(hexToBytes(k), bytes, f.token.address, BigInt(f.amount), f.cfgId));
  if (c !== f.commitment) throw new Error('claim file: commitment does not follow from the file');
  return true;
}
