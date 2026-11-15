// Amounts: exact BigInt arithmetic only, never floating point.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Format `value` (smallest units) with `decimals`. Trailing fractional zeros are dropped
 * and the integer part is grouped in thousands with `group` (default ",").
 * formatUnits(10000000000000000n, 18) === "0.01"; formatUnits(10000000000n, 6) === "10,000".
 */
export function formatUnits(value, decimals, { group = ',', maxFraction } = {}) {
  let v = BigInt(value);
  const neg = v < 0n;
  if (neg) v = -v;
  const d = BigInt(decimals);
  const base = 10n ** d;
  const int = (v / base).toString();
  let frac = decimals > 0 ? (v % base).toString().padStart(decimals, '0') : '';
  if (maxFraction !== undefined && frac.length > maxFraction) {
    // round half up at maxFraction digits (display only)
    const keep = BigInt(frac.slice(0, maxFraction) || '0');
    const next = Number(frac[maxFraction]);
    const bumped = next >= 5 ? keep + 1n : keep;
    if (maxFraction === 0) {
      return (neg ? '-' : '') + groupInt((BigInt(int) + (next >= 5 ? 1n : 0n)).toString(), group);
    }
    let s = bumped.toString().padStart(maxFraction, '0');
    if (s.length > maxFraction) {
      return formatUnits((neg ? -1n : 1n) * (BigInt(int) + 1n) * base, decimals, { group, maxFraction });
    }
    frac = s;
  }
  frac = frac.replace(/0+$/, '');
  return (neg ? '-' : '') + groupInt(int, group) + (frac ? '.' + frac : '');
}

function groupInt(int, group) {
  if (!group) return int;
  return int.replace(/\B(?=(\d{3})+(?!\d))/g, group);
}

/** Parse a decimal string ("0.01", "1,000") into smallest units. Throws on excess precision. */
export function parseUnits(text, decimals) {
  const s = String(text).replace(/[,_\s]/g, '');
  const m = /^(\d*)(?:\.(\d*))?$/.exec(s);
  if (!m || (m[1] === '' && (m[2] === undefined || m[2] === ''))) throw new Error(`not a number: ${text}`);
  const frac = (m[2] || '').replace(/0+$/, '');
  if (frac.length > decimals) throw new Error(`${text} has more than ${decimals} decimals`);
  return BigInt(m[1] || '0') * 10n ** BigInt(decimals) + BigInt(frac.padEnd(decimals, '0') || '0');
}

/** Shorten a hex string for display: 0x8C8b…1b74. */
export function shortHex(hex, head = 6, tail = 4) {
  const s = String(hex);
  return s.length <= head + tail + 1 ? s : `${s.slice(0, head)}…${s.slice(-tail)}`;
}

/** Group hex digits in fours for side-by-side comparison: 0x 8C8b 2664 De28 … */
export function groupHex(hex) {
  const s = String(hex).replace(/^0x/, '');
  return '0x ' + s.match(/.{1,4}/g).join(' ');
}

/** "about 14 minutes", "about 40 seconds" */
export function aboutDuration(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 90) return `about ${Math.max(10, Math.round(s / 10) * 10)} seconds`;
  const m = Math.round(s / 60);
  if (m < 90) return `about ${m} minute${m === 1 ? '' : 's'}`;
  const h = Math.round(m / 6) / 10;
  return `about ${h} hours`;
}
