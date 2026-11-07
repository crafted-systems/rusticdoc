#!/usr/bin/env node
// Checks the built site: every internal link and anchor resolves.
// Run after `npm run build`.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const dist = join(dirname(fileURLToPath(import.meta.url)), '../dist');
const walk = (d) => readdirSync(d).flatMap((n) => {
  const p = join(d, n);
  return statSync(p).isDirectory() ? walk(p) : p.endsWith('.html') ? [p] : [];
});
const pages = walk(dist);
const decode = (s) => decodeURIComponent(s.replace(/&#x26;|&amp;/g, '&'));
const idsOf = new Map();
const ids = (file) => {
  if (!idsOf.has(file)) idsOf.set(file, new Set([...readFileSync(file, 'utf8').matchAll(/\sid="([^"]+)"/g)].map((m) => m[1])));
  return idsOf.get(file);
};
function target(path) {
  if (path.endsWith('/')) return join(dist, path, 'index.html');
  const direct = join(dist, path);
  if (existsSync(direct) && statSync(direct).isFile()) return direct;
  return null;
}
const problems = [];
let checked = 0;
for (const page of pages) {
  const html = readFileSync(page, 'utf8');
  const where = relative(dist, page);
  for (const m of html.matchAll(/<(?:a|link)\s[^>]*href="([^"]+)"/g)) {
    const raw = m[1];
    if (/^(https?:|mailto:|data:)/.test(raw)) continue;
    checked++;
    const [pathPart, hash] = raw.split('#');
    const base = '/' + relative(dist, dirname(page)).replace(/\\/g, '/') + '/';
    const path = pathPart === '' ? base.replace(/^\/\//, '/') : new URL(pathPart, `https://x${base.replace(/^\/\//, '/')}`).pathname;
    const file = target(decode(path));
    if (!file || !existsSync(file)) { problems.push(`${where}: ${raw} does not exist`); continue; }
    if (hash && file.endsWith('.html') && !ids(file).has(decode(hash))) problems.push(`${where}: ${raw} has no such anchor`);
  }
}
for (const p of problems) console.error(p);
console.log(`${pages.length} pages, ${checked} internal links, ${problems.length} problems`);
process.exit(problems.length ? 1 : 0);
