#!/usr/bin/env node
// Write the site's shared header, footer and asset links (tools/site-chrome.html) into
// every deployable page, and stamp this page's own app.css and src/app.js with ?v=<version>,
// where the version is the SHA-256 of app.css and every file under src/ (so a redeploy is never
// hidden by a cached file). Run before tools/manifest.mjs. Pages mark the blocks with
// <!-- site:assets -->…<!-- /site:assets -->, <!-- site:header -->…, <!-- site:footer -->….
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WALLET_TEST_BUILD, REPORT_PROBLEM } from '../src/lib/links.js';

const root = fileURLToPath(new URL('..', import.meta.url));
const tpl = readFileSync(join(root, 'tools/site-chrome.html'), 'utf8');
const block = (id) => tpl.match(new RegExp(`<template id="${id}">\\n?([\\s\\S]*?)</template>`))[1].replace(/\n$/, '');
const walk = (d) => readdirSync(d).flatMap((f) => (statSync(join(d, f)).isDirectory() ? walk(join(d, f)) : [join(d, f)]));
const versioned = ['app.css', ...walk(join(root, 'src')).map((f) => relative(root, f))].sort();
const h = createHash('sha256');
for (const f of versioned) h.update(f + '\0').update(readFileSync(join(root, f)));
export const VERSION = h.digest('hex').slice(0, 12);

// dev/harness.html is not deployed, but gets the same chrome so its screenshots show the real page
const pages = ['index.html', '404.html', 'dev/harness.html', ...walk(join(root, 'about')).map((f) => relative(root, f)).filter((f) => f.endsWith('.html'))];
for (const p of pages) {
  let s = readFileSync(join(root, p), 'utf8');
  for (const [name, id] of [['assets', 'ASSETS'], ['header', 'HEADER'], ['footer', 'FOOTER']]) {
    const re = new RegExp(`<!-- site:${name} -->[\\s\\S]*?<!-- /site:${name} -->`);
    if (!re.test(s)) throw new Error(`${p}: no <!-- site:${name} --> block`);
    let b = block(id);
    // the deposit page's skip link names what it skips to (partials/README.md, rule 3)
    if (id === 'HEADER' && (p === 'index.html' || p === 'dev/harness.html')) b = b.replace('>Skip to content</a>', '>Skip to the deposit</a>');
    s = s.replace(re, `<!-- site:${name} -->\n${b}\n<!-- /site:${name} -->`);
  }
  s = s.replace(/(href|src)="((?:\.\.\/)*(?:\/bridge\/)?(?:app\.css|src\/app\.js))(?:\?v=[0-9a-f]+)?"/g, (m, attr, path) => `${attr}="${path}?v=${VERSION}"`);
  // the shared links (src/lib/links.js): every static anchor marked for one gets its href from there
  s = s.replace(/<a href="[^"]*" data-link="wallet-test-build"/g, `<a href="${WALLET_TEST_BUILD}" data-link="wallet-test-build"`);
  s = s.replace(/<a href="[^"]*" data-link="report-problem"/g, `<a href="${REPORT_PROBLEM}" data-link="report-problem"`);
  writeFileSync(join(root, p), s);
}
console.log(`chrome written to ${pages.length} pages; version ${VERSION}`);
