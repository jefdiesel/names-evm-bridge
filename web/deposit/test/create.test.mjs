// The deposit made in the browser: Logos account parsing, the secret, the commitment (against
// commitments made by the real bridge CLI) and the claim file.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { parseLogosAccount, base58Encode, claimKey, depositCommitment, hexToBytes, bytesToHex, makeDeposit, claimFile, verifyClaimFile, claimFileName } from '../src/lib/create.js';

const V = JSON.parse(readFileSync(new URL('./vectors/commitment-cli.json', import.meta.url)));

test('commitments are byte-identical to the bridge CLI (ETH and the 6-decimal token, public and private accounts)', async () => {
  assert.equal(V.cases.length, 4);
  for (const v of V.cases) {
    const k = await claimKey(hexToBytes(v.s));
    assert.equal(bytesToHex(k), v.k, `${v.name}: k = SHA-256("RFP022/ck" || s)`);
    const c = await depositCommitment(k, parseLogosAccount(v.dest).bytes, v.token, BigInt(v.denom), v.cfgId);
    assert.equal(bytesToHex(c), v.commitment, v.name);
  }
});

test('LEZ accounts: base58 of exactly 32 bytes, Public/ and Private/ labels and spaces tolerated', () => {
  const id = '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f';
  assert.equal(parseLogosAccount(id).bytes.length, 32);
  assert.deepEqual([parseLogosAccount(` Private/${id}\n`).id, parseLogosAccount(` Private/${id}\n`).label], [id, 'Private']);
  assert.deepEqual([parseLogosAccount(`Public/${id}`).id, parseLogosAccount(`Public/${id}`).label], [id, 'Public']);
  assert.equal(base58Encode(parseLogosAccount(id).bytes), id);
  const lead = V.cases.find((c) => c.name.includes('leading zero')).dest;
  assert.equal(parseLogosAccount(lead).bytes[0], 0);
});

test('LEZ accounts: every wrong kind of input gets a plain answer, never a character complaint', () => {
  const id = '6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f';
  // the user's own paste (2026-11-13): a 64-character hex Logos Blockchain key
  const userKey = '7a6223abde07c0c278d203973fd233ab3e47b2bfa53c8ed3d74' + '0123456789abc';
  assert.equal(userKey.length, 64);
  const cases = [
    ['', 'empty'],
    ['   ', 'empty'],
    [userKey, 'blockchain-key'],
    ['0x' + userKey, 'blockchain-key'],
    [userKey.toUpperCase(), 'blockchain-key'],
    ['7a6223abde07c0c278d203973fd233ab3e47b2bfa53c8ed3d74…', 'blockchain-key'],
    ['0x5a1e0000000000000000000000000000000c0de5', 'eth-address'],
    ['alice', 'name'],
    ['alice.lez', 'name'],
    [id.slice(0, -3), 'incomplete'],
    [id + 'z', 'incomplete'],
    [`Public/${id.slice(0, 20)}`, 'incomplete'],
    [id.replace('N', '0'), 'unrecognised'],
    ['Public/not an account!', 'unrecognised'],
  ];
  for (const [bad, code] of cases) {
    assert.throws(() => parseLogosAccount(bad), (e) => e.code === code && !/cannot be in/.test(e.message), `${JSON.stringify(bad)} -> ${code}`);
  }
  assert.throws(() => parseLogosAccount(userKey), /looks like a Logos Blockchain key\. The bridge sends to a LEZ account instead/);
});

test('every deposit gets a fresh 32-byte secret', async () => {
  const dest = parseLogosAccount('6NAoZKXE14LJGeeASmpzpwvsuDLUDB2r6v2a2dXa4f4f').bytes;
  const a = await makeDeposit({ dest, token: '0x' + '0'.repeat(40), amount: 10n ** 16n, cfgId: V.cases[0].cfgId });
  const b = await makeDeposit({ dest, token: '0x' + '0'.repeat(40), amount: 10n ** 16n, cfgId: V.cases[0].cfgId });
  assert.match(a.s, /^0x[0-9a-f]{64}$/);
  assert.notEqual(a.s, b.s);
  assert.notEqual(a.commitment, b.commitment);
});

test('claim file: schema, name, and the secret, key and commitment agree', async () => {
  const v = V.cases[1];
  const f = claimFile({ chainId: 11155111n, network: 'Sepolia', vault: '0x8C8b2664De28e5287158db4DB65D448638871b74', cfgId: v.cfgId, token: v.token, symbol: 'dUSDC', decimals: 6, denomIndex: 0, amount: BigInt(v.denom), amountText: '100 dUSDC', account: v.dest, s: v.s, k: v.k, commitment: v.commitment, createdAt: '2026-11-13T00:00:00.000Z' });
  assert.equal(f.format, 'logos-bridge-claim');
  assert.equal(f.version, 1);
  assert.equal(f.deposit, null);
  assert.equal(f.chainId, '11155111');
  assert.equal(await verifyClaimFile(JSON.parse(JSON.stringify(f))), true);
  assert.equal(claimFileName(v.commitment), `logos-bridge-claim-${v.commitment.slice(2, 10)}.json`);
  await assert.rejects(verifyClaimFile({ ...f, claimKey: V.cases[0].k }), /claimKey/);
  await assert.rejects(verifyClaimFile({ ...f, commitment: V.cases[0].commitment }), /commitment/);
  const done = claimFile({ ...f, token: v.token, symbol: 'dUSDC', decimals: 6, denomIndex: 0, account: v.dest, s: v.s, k: v.k, deposit: { tx: '0x' + 'ab'.repeat(32), block: 12125801n, leafIndex: 5n } });
  assert.deepEqual(done.deposit, { tx: '0x' + 'ab'.repeat(32), block: '12125801', leafIndex: '5' });
});

test('claim file content is stable for a given input (the format a later claim reads)', () => {
  const v = V.cases[0];
  const f = claimFile({ chainId: 11155111n, network: 'Sepolia', vault: '0x8C8b2664De28e5287158db4DB65D448638871b74', cfgId: v.cfgId, token: v.token, symbol: 'ETH', decimals: 18, denomIndex: 0, amount: BigInt(v.denom), amountText: '0.01 Sepolia ETH', account: v.dest, s: v.s, k: v.k, commitment: v.commitment, createdAt: '2026-11-13T00:00:00.000Z' });
  assert.deepEqual(Object.keys(f), ['format', 'version', 'warning', 'network', 'chainId', 'vault', 'cfgId', 'token', 'denominationIndex', 'amount', 'amountText', 'logosAccount', 'masterSecret', 'claimKey', 'commitment', 'createdAt', 'deposit']);
  assert.equal(createHash('sha256').update(JSON.stringify(f)).digest('hex').slice(0, 16), EXPECTED_FILE_HASH);
});
const EXPECTED_FILE_HASH = 'a0d458e3a52389b0'; // claimFile() for vector 0, unchanged since the claim file was introduced
