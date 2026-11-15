import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatUnits, parseUnits, shortHex, groupHex, aboutDuration } from '../src/lib/units.js';

test('formatUnits', () => {
  assert.equal(formatUnits(10n ** 16n, 18), '0.01');
  assert.equal(formatUnits(10n ** 17n, 18), '0.1');
  assert.equal(formatUnits(10n ** 18n, 18), '1');
  assert.equal(formatUnits(100_000_000n, 6), '100');
  assert.equal(formatUnits(1_000_000_000n, 6), '1,000');
  assert.equal(formatUnits(10_000_000_000n, 6), '10,000');
  assert.equal(formatUnits(1_234_567_890_123n, 6), '1,234,567.890123');
  assert.equal(formatUnits(1n, 18), '0.000000000000000001');
  assert.equal(formatUnits(0n, 18), '0');
  assert.equal(formatUnits(5n, 0), '5');
  assert.equal(formatUnits(1_000_000n, 6, { group: '' }), '1');
  assert.equal(formatUnits(12_345_000n, 3, { group: '' }), '12345');
  assert.equal(formatUnits((1n << 256n) - 1n, 18).length > 60, true);
});

test('formatUnits with rounding for display', () => {
  assert.equal(formatUnits(5_479_059_830_500_454_269n, 18, { maxFraction: 4 }), '5.4791');
  assert.equal(formatUnits(1_999_960_000_000_000_000n, 18, { maxFraction: 4 }), '2');
  assert.equal(formatUnits(9_999_960_000_000_000_000n, 18, { maxFraction: 4 }), '10');
  assert.equal(formatUnits(428_445_420_351_432n, 18, { maxFraction: 6 }), '0.000428');
  assert.equal(formatUnits(1_500_000n, 6, { maxFraction: 0 }), '2');
});

test('parseUnits', () => {
  assert.equal(parseUnits('0.01', 18), 10n ** 16n);
  assert.equal(parseUnits('1,000', 6), 1_000_000_000n);
  assert.equal(parseUnits('10000', 6), 10_000_000_000n);
  assert.equal(parseUnits('.5', 6), 500_000n);
  assert.equal(parseUnits('1.500000', 6), 1_500_000n);
  assert.throws(() => parseUnits('0.0000001', 6));
  assert.throws(() => parseUnits('abc', 6));
  assert.throws(() => parseUnits('', 6));
  for (const [v, d] of [[123456789n, 6], [10n ** 16n, 18], [1n, 18]]) {
    assert.equal(parseUnits(formatUnits(v, d), d), v);
  }
});

test('hex display helpers', () => {
  assert.equal(shortHex('0x8C8b2664De28e5287158db4DB65D448638871b74'), '0x8C8b…1b74');
  assert.equal(groupHex('0x8C8b2664'), '0x 8C8b 2664');
});

test('aboutDuration', () => {
  assert.equal(aboutDuration(5), 'about 10 seconds');
  assert.equal(aboutDuration(14 * 60), 'about 14 minutes');
  assert.equal(aboutDuration(60), 'about 60 seconds');
  assert.equal(aboutDuration(3 * 3600), 'about 3 hours');
});
