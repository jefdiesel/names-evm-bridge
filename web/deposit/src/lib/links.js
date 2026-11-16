// Links the page and its about pages share. ONE place: tools/chrome.mjs writes these into every
// static page's `data-link="…"` anchors, and the app imports them.
//
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Where to get the Names Wallet test build for LEZ 0.4. The wallet session is building
 * /basecamp/test-build/; until that page is live this points at /basecamp/ (never a dead link).
 * Flip it here, then run tools/chrome.mjs and tools/manifest.mjs.
 */
export const WALLET_TEST_BUILD = '/basecamp/test-build/';

/** Where to report a problem with the bridge: its own address (forwards like the accessibility one; owner, 2026-11-15). */
export const REPORT_PROBLEM = 'mailto:bridge@nameswallet.app';

/** The bridge's own about page for testers (relative to the bridge page). */
export const TESTING_PAGE = 'about/testing/';
