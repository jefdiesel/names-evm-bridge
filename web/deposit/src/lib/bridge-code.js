// SPDX-License-Identifier: MIT OR Apache-2.0
// The bridge code (`logosbr1…`): one whole bridge deployment, so the Names Wallet can add a new vault
// without a new build. Public facts only. The page shows the code of its live deployment with four
// words; the wallet shows the same four words after it has checked the code against both chains.
//
//   payload v1 (integers big-endian)
//     version(1)=1 | chainId(8) | vault(20) | vaultCreateBlock(8) | gate(20) | desk(20)
//     | cfgId(32) | lezChainId(32) | zoneId(32) | bridgeProgram(32) | configAccount(32)
//     | tokenProgram(32) | release(1: 0 never, 1 proof) | minReleaseDelay(8)
//     | genesisLezBlock(8) | genesisG2State(32) | updateImageId(32) | releaseImageId(32)
//     | flags(1: bit 0 the endpoint relays claims, bit 1 it may settle) | endpointLen(1) endpoint
//     | labelLen(1) label | tokenCount(1) { ethToken(20) | definition(32) | decimals(1)
//                                          | symbolLen(1) symbol | denomCount(1) | denom(16)… }
//   code   = bech32m("logosbr", payload)
//   words  = the compare words of SHA-256("logos-bridge/bridge-code/v1" | payload)
//
// A reader checks the layout in byte order, then that cfgId follows from its parts (the vault's own
// formula, BridgeVault.computeCfgId) and that configAccount is the bridge program's Config PDA.
// Spec: the bridge's design note NAMES-SEND-BACK.md §9; vectors ../../test/vectors/bridge-code-v1.json
// (an independent generator). Pure: no DOM, no network (SHA-256 is the browser's, so it is async).

import { bech32mEncode, bech32mDecode, CodeError, codeFromInput } from './withdraw.js';
import { base58Decode, base58Encode } from './create.js';
import { compareWords } from './compare-words.js';

export const BR_HRP = 'logosbr';
export const BR_VERSION = 1;
export const BRIDGE_CODE_TAG = 'logos-bridge/bridge-code/v1';
/** keccak256("RFP022/cfg/v1"), the vault's CFG_TAG. */
export const CFG_TAG = '0x26e962302b1dc9417c466ff56ef206a578cd054a0a099172cff8b80460ab5a45';
const PDA_PREFIX = '/LEE/v0.2/AccountId/PDA/';
const CONFIG_SEED = '/RFP022/bridge/config/v1/000000/';
const MAX_CODE = 2000;
const ENDPOINT_RE = /^https:\/\/[A-Za-z0-9.\-:/_]{4,92}$/;
const LABEL_RE = /^[A-Za-z0-9 .()-]{1,32}$/;
const SYMBOL_RE = /^[A-Za-z0-9]{1,8}$/;
const enc = new TextEncoder();
const dec = new TextDecoder();

const hx = (bytes) => '0x' + Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
const isZero = (bytes) => bytes.every((b) => b === 0);

function unhex(h, len, what) {
  const s = String(h ?? '').toLowerCase();
  if (!/^0x[0-9a-f]*$/.test(s) || s.length !== 2 + 2 * len) throw new CodeError('field', `${what} must be 0x and ${2 * len} hex digits.`);
  const out = new Uint8Array(len);
  for (let i = 0; i < len; i++) out[i] = parseInt(s.slice(2 + 2 * i, 4 + 2 * i), 16);
  return out;
}
function account(text, what) {
  let b;
  try {
    b = base58Decode(String(text ?? ''));
  } catch {
    throw new CodeError('field', `${what} is not a LEZ account id.`);
  }
  if (b.length !== 32) throw new CodeError('field', `${what} is not a LEZ account id.`);
  return b;
}
function be(v, n, what) {
  let x = BigInt(v);
  if (x < 0n) throw new CodeError('field', `${what} is negative.`);
  const out = new Uint8Array(n);
  for (let i = n - 1; i >= 0; i--) { out[i] = Number(x & 0xffn); x >>= 8n; }
  if (x !== 0n) throw new CodeError('field', `${what} is too large.`);
  return out;
}
const num = (bytes) => bytes.reduce((a, b) => (a << 8n) | BigInt(b), 0n);
const concat = (parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
};
async function sha256(...parts) {
  return new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', concat(parts)));
}

/** cfgId = SHA-256(CFG_TAG | u256(chainId) | vault | lezChainId | zoneId | bridgeProgram). */
export async function cfgIdOf(chainId, vault, lezChainId, zoneId, bridgeProgram) {
  return sha256(unhex(CFG_TAG, 32), be(chainId, 32, 'chain id'), vault, lezChainId, zoneId, bridgeProgram);
}

/** The bridge program's Config account: SHA-256("/LEE/v0.2/AccountId/PDA/" + 8 zero bytes | program | seed). */
export async function configAccountOf(bridgeProgram) {
  return sha256(enc.encode(PDA_PREFIX), new Uint8Array(8), bridgeProgram, enc.encode(CONFIG_SEED));
}

/** The payload bytes of `fields` (the shape decode gives back). Checks what can be checked offline. */
export async function bridgeCodePayload(f) {
  const parts = [];
  parts.push(Uint8Array.of(BR_VERSION), be(f.chain_id, 8, 'chain id'), unhex(f.vault, 20, 'vault'), be(f.vault_create_block, 8, 'vault block'));
  parts.push(unhex(f.gate, 20, 'gate'), unhex(f.desk, 20, 'desk'), unhex(f.cfg_id, 32, 'cfgId'), unhex(f.lez_chain_id, 32, 'LEZ chain id'), unhex(f.zone_id, 32, 'zone id'));
  parts.push(account(f.bridge_program, 'bridge program'), account(f.config_account, 'Config account'), account(f.token_program, 'token program'));
  const release = { never: 0, proof: 1 }[f.release];
  if (release === undefined) throw new CodeError('field', 'release is "never" or "proof".');
  parts.push(Uint8Array.of(release), be(f.min_release_delay, 8, 'release delay'), be(f.genesis_lez_block, 8, 'genesis block'));
  parts.push(unhex(f.genesis_g2_state, 32, 'genesis state'), unhex(f.update_image, 32, 'update image'), unhex(f.release_image, 32, 'release image'));
  parts.push(Uint8Array.of((f.relay ? 1 : 0) | (f.settle ? 2 : 0)));
  const e = enc.encode(String(f.endpoint ?? ''));
  const l = enc.encode(String(f.label ?? ''));
  if (e.length > 255 || l.length > 255) throw new CodeError('field', 'a text is too long.');
  parts.push(Uint8Array.of(e.length), e, Uint8Array.of(l.length), l);
  const tokens = Array.isArray(f.tokens) ? f.tokens : [];
  if (tokens.length > 255) throw new CodeError('tokens', 'too many tokens.');
  parts.push(Uint8Array.of(tokens.length));
  for (const t of tokens) {
    const s = enc.encode(String(t.symbol ?? ''));
    const denoms = Array.isArray(t.denoms) ? t.denoms : [];
    if (s.length > 255 || denoms.length > 255 || !Number.isInteger(t.decimals) || t.decimals < 0 || t.decimals > 255) throw new CodeError('tokens', 'a token field is out of range.');
    parts.push(unhex(t.eth_token, 20, 'token'), account(t.definition, 'token definition'), Uint8Array.of(t.decimals), Uint8Array.of(s.length), s, Uint8Array.of(denoms.length));
    for (const d of denoms) parts.push(be(d, 16, 'denomination'));
  }
  const p = concat(parts);
  await bridgeCodeFromBytes(p); // the same checks a reader applies
  return p;
}

/** The code of `fields`. */
export async function encodeBridgeCode(fields) {
  return bech32mEncode(BR_HRP, Array.from(await bridgeCodePayload(fields)));
}

/** Reads a payload: `fields` (the shape encode takes) or a CodeError with a stable `code`. */
export async function bridgeCodeFromBytes(bytes) {
  const b = bytes instanceof Uint8Array ? bytes : Uint8Array.from(bytes);
  if (b.length === 0 || b[0] !== BR_VERSION) throw new CodeError('version', `This bridge code is version ${b[0]}; this page reads version ${BR_VERSION}.`);
  let at = 1;
  const take = (n) => {
    if (at + n > b.length) throw new CodeError('short', 'The bridge code ends too early: copy it again, whole.');
    const s = b.slice(at, at + n);
    at += n;
    return s;
  };
  const u8 = () => take(1)[0];
  const chainId = num(take(8));
  if (chainId === 0n) throw new CodeError('chain', 'The bridge code names no chain.');
  const vault = take(20);
  if (isZero(vault)) throw new CodeError('field', 'The bridge code names no vault.');
  const vaultCreateBlock = num(take(8));
  const gate = take(20);
  const desk = take(20);
  const cfgId = take(32);
  const lezChainId = take(32);
  const zoneId = take(32);
  const bridgeProgram = take(32);
  const configAccount = take(32);
  const tokenProgram = take(32);
  const release = u8();
  if (release > 1) throw new CodeError('field', 'The bridge code’s release kind is unknown.');
  const minReleaseDelay = num(take(8));
  const genesisLezBlock = num(take(8));
  const genesisG2State = take(32);
  const updateImage = take(32);
  const releaseImage = take(32);
  const flags = u8();
  if (flags & ~3) throw new CodeError('field', 'The bridge code has a flag this page doesn’t know.');
  const endpoint = dec.decode(take(u8()));
  if (!ENDPOINT_RE.test(endpoint)) throw new CodeError('endpoint', 'The bridge code’s network address is not an https address.');
  const label = dec.decode(take(u8()));
  if (!LABEL_RE.test(label)) throw new CodeError('label', 'The bridge code’s name has characters a name can’t have.');
  const count = u8();
  if (count < 1 || count > 4) throw new CodeError('tokens', 'A bridge code lists 1 to 4 tokens.');
  const tokens = [];
  for (let i = 0; i < count; i++) {
    const ethToken = hx(take(20));
    const definition = base58Encode(take(32));
    const decimals = u8();
    const symbol = dec.decode(take(u8()));
    const n = u8();
    if (n < 1 || n > 8) throw new CodeError('tokens', 'A token has 1 to 8 denominations.');
    const denoms = [];
    for (let j = 0; j < n; j++) denoms.push(num(take(16)));
    if (decimals > 36 || !SYMBOL_RE.test(symbol)) throw new CodeError('tokens', 'A token’s decimals or symbol are out of range.');
    if (denoms.some((d, j) => d === 0n || (j > 0 && d <= denoms[j - 1]))) throw new CodeError('tokens', 'A token’s denominations must be above 0 and increasing.');
    if (tokens.some((t) => t.eth_token === ethToken || t.definition === definition)) throw new CodeError('tokens', 'A token is listed twice.');
    tokens.push({ eth_token: ethToken, definition, decimals, symbol, denoms: denoms.map(String) });
  }
  if (at !== b.length) throw new CodeError('trailing', 'The bridge code has bytes after its last token.');
  const want = await cfgIdOf(chainId, vault, lezChainId, zoneId, bridgeProgram);
  if (hx(want) !== hx(cfgId)) throw new CodeError('cfg_id', 'The bridge code’s cfgId doesn’t follow from its vault, chain and bridge program: it was changed or made wrong.');
  if (hx(await configAccountOf(bridgeProgram)) !== hx(configAccount)) throw new CodeError('config_account', 'The bridge code’s Config account is not its bridge program’s.');
  return {
    chain_id: Number(chainId), vault: hx(vault), vault_create_block: Number(vaultCreateBlock), gate: hx(gate), desk: hx(desk),
    cfg_id: hx(cfgId), lez_chain_id: hx(lezChainId), zone_id: hx(zoneId), bridge_program: base58Encode(bridgeProgram),
    config_account: base58Encode(configAccount), token_program: base58Encode(tokenProgram), release: release ? 'proof' : 'never',
    min_release_delay: Number(minReleaseDelay), genesis_lez_block: Number(genesisLezBlock), genesis_g2_state: hx(genesisG2State),
    update_image: hx(updateImage), release_image: hx(releaseImage), endpoint, relay: !!(flags & 1), settle: !!(flags & 2), label, tokens,
  };
}

/** Reads a pasted code (or a link with it after `#`): `{ fields, payload }`. */
export async function decodeBridgeCode(input) {
  const { hrp, bytes } = bech32mDecode(codeFromInput(input), MAX_CODE);
  if (hrp !== BR_HRP) throw new CodeError('hrp', `This is a "${hrp}" code, not a bridge code.`);
  const payload = Uint8Array.from(bytes);
  return { fields: await bridgeCodeFromBytes(payload), payload };
}

/** `{ indexes, words, words_text }` of a payload: the compare words of SHA-256(tag | payload). */
export async function bridgeCodeWords(payload) {
  return compareWords(await sha256(enc.encode(BRIDGE_CODE_TAG), payload));
}

/** The fields of a deployment entry of deployments.js (null for one without a `bridge` block). */
export function bridgeFieldsOf(d) {
  const x = d?.bridge;
  if (!x) return null;
  const tokens = Object.entries(d.tokens || {}).map(([eth, t]) => {
    const def = x.definitions?.[eth];
    return def ? { eth_token: eth.toLowerCase(), definition: def, decimals: t.decimals, symbol: t.symbol, denoms: t.denoms } : null;
  }).filter(Boolean);
  return {
    chain_id: d.chainId, vault: d.vault.toLowerCase(), vault_create_block: d.deployBlock, gate: x.gate.toLowerCase(),
    desk: (d.desk || '0x' + '00'.repeat(20)).toLowerCase(), cfg_id: d.cfgId, lez_chain_id: x.lezChainId, zone_id: x.zoneId,
    bridge_program: x.bridgeProgram, config_account: x.configAccount, token_program: x.tokenProgram, release: x.release,
    min_release_delay: x.minReleaseDelay, genesis_lez_block: x.genesisLezBlock, genesis_g2_state: x.genesisG2State,
    update_image: x.updateImage, release_image: x.releaseImage, endpoint: x.endpoint, relay: x.relay, settle: x.settle,
    label: x.label, tokens,
  };
}

/** `{ code, words, words_text, fields }` for a deployment entry, or null. */
export async function bridgeCodeOf(d) {
  const fields = bridgeFieldsOf(d);
  if (!fields) return null;
  const payload = await bridgeCodePayload(fields);
  const w = await bridgeCodeWords(payload);
  return { code: bech32mEncode(BR_HRP, Array.from(payload)), words: w.words, words_text: w.words_text, fields };
}
