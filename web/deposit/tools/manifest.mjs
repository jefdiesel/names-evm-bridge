#!/usr/bin/env node
// Write MANIFEST.sha256: the SHA-256 of every file a host serves (the deployable set). Anyone
// can compare a hosted copy against it (`shasum -a 256 -c MANIFEST.sha256` in a download of
// the site; about/verify/ says how). The deployable set is the page, its 404 page, the about/
// pages and src/. Not in it: dev/, test/, tools/, README.md (developer notes), _headers (the
// host reads it; nameswallet.app keeps its own /bridge/* lines) and assets/ (a local stand-in
// for the site's /assets/tokens.css).
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const files = ['index.html', '404.html', 'app.css'];
const walk = (d) => readdirSync(d).flatMap((f) => (statSync(join(d, f)).isDirectory() ? walk(join(d, f)) : [join(d, f)]));
for (const dir of ['src', 'about']) files.push(...walk(join(root, dir)).map((f) => relative(root, f)));
files.sort();
let total = 0;
const lines = files.map((f) => {
  const b = readFileSync(join(root, f));
  total += b.length;
  return `${createHash('sha256').update(b).digest('hex')}  ${f}`;
});
writeFileSync(join(root, 'MANIFEST.sha256'), lines.join('\n') + '\n');
console.log(lines.join('\n'));
console.log(`${files.length} files, ${total} bytes`);
