// How a visitor arrives, and what the page says after the deposit on each path. DOM-free, so the
// tests read it (test/entry.test.mjs).
//
// Two ways in:
//   - a deposit code from the Names Wallet: `#logosdep1…` (or pasted / scanned). The wallet holds
//     the secret and offers the claim; this path never makes or mentions a claim file.
//   - a deposit made on this page: the secret is made in the tab and saved in the claim file.
//     After sending, the address bar gets `#made:logosdep1…`, so a reload knows the claim file
//     claims it.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/** What the fragment asks for: `{ kind: 'none' | 'paste' | 'settings' | 'code', madeHere, text }`. */
export function readFragment(fragment) {
  let f = String(fragment || '').replace(/^#/, '');
  try {
    f = decodeURIComponent(f);
  } catch {
    /* the decoder says what is wrong */
  }
  if (!f) return { kind: 'none', madeHere: false, text: '' };
  if (f === 'request') return { kind: 'paste', madeHere: false, text: '' };
  if (f === 'settings') return { kind: 'settings', madeHere: false, text: '' };
  const madeHere = f.startsWith('made:');
  return { kind: 'code', madeHere, text: madeHere ? f.slice(5) : f };
}

/** The fragment for a deposit made on this page, once sent. */
export function madeFragment(code) {
  return `#made:${code}`;
}

/** The commitment's first and last 6 hex digits, for the comparison with the wallet. */
export function compareDigits(commitment) {
  const hex = String(commitment).replace(/^0x/, '').toLowerCase();
  return { first: hex.slice(0, 6), last: hex.slice(-6) };
}

/** Which claim the page describes after a deposit: 'here' (made in this tab), 'file' (made here,
 * reopened from its link) or 'names' (a Names Wallet code: the wallet offers the claim). */
export function claimPath({ createdHere, madeHere }) {
  if (createdHere) return 'here';
  return madeHere ? 'file' : 'names';
}

/** The words after a deposit from a Names Wallet code (no claim file on this path). */
export function namesAfter(network) {
  return `Your Names Wallet will show this deposit and offer the claim, on ${network}. Nothing more to do on this page.`;
}

/**
 * Minutes until a deposit in `depositBlock` is final, with Sepolia's `finalized` block at
 * `finalizedBlock`: the same rule as the bridge module (rust-lib/src/receive.rs), so the page and
 * the Names Wallet say the same number. Finality moves an epoch (32 blocks of 12 s) at a time, so
 * whole epochs are counted (an upper bound), plus one minute. 0 once final.
 */
export function finalInMinutes(depositBlock, finalizedBlock) {
  const b = BigInt(depositBlock);
  const f = BigInt(finalizedBlock);
  if (b <= f) return 0;
  const epochs = (b - f + 31n) / 32n;
  return Number((epochs * 32n * 12n) / 60n) + 1;
}

/** What the Deposited screen says, by claim path: the wait first, then what to do once final. */
export function depositedWords(path, final) {
  const then = path === 'names' ? 'Then your Names Wallet offers the claim.' : 'Then claim with your file in the Names Wallet.';
  if (!final) return `It takes up to about 20 minutes to become final on Sepolia. ${then}`;
  return path === 'names' ? 'Final on Sepolia. Open your Names Wallet to claim.' : 'Final on Sepolia. Claim with your file in the Names Wallet.';
}
