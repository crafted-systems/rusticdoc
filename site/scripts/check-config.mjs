#!/usr/bin/env node
// Every RUSTICDOC_* variable the code names has to be in the configuration
// reference, and the reference must not name one the code doesn't.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const walk = (d) => readdirSync(d).flatMap((n) => {
  const p = join(d, n);
  return statSync(p).isDirectory() ? (n === 'tests' ? [] : walk(p)) : p.endsWith('.rs') ? [p] : [];
});
const names = (text) => new Set(text.match(/\bRUSTICDOC_[A-Z0-9_]*[A-Z0-9]\b/g) ?? []);
// Settings in from_vars are read through a closure that adds the prefix at run time.
const read = (p) => {
  const text = readFileSync(p, 'utf8');
  const start = text.indexOf('vars(&format!("RUSTICDOC_{k}"))');
  const end = text.indexOf('\n    }\n', start);
  const prefixed = start < 0 ? '' : text.slice(start, end);
  return [...names(text), ...[...prefixed.matchAll(/\bget\("([A-Z0-9_]+)"\)/g)].map((m) => `RUSTICDOC_${m[1]}`)];
};
const code = new Set(walk(join(root, '../crates')).flatMap(read));
code.add('RUSTICDOC_WRITE_FIXTURES'); // named on the page as test-only
const page = names(readFileSync(join(root, 'src/content/docs/docs/reference/configuration.md'), 'utf8'));
const missing = [...code].filter((n) => !page.has(n)).sort();
const unknown = [...page].filter((n) => !code.has(n)).sort();
if (missing.length) console.error(`In the code but not the reference:\n  ${missing.join('\n  ')}`);
if (unknown.length) console.error(`In the reference but not the code:\n  ${unknown.join('\n  ')}`);
console.log(`${code.size} variables in the code, ${page.size} in the reference`);
process.exit(missing.length || unknown.length ? 1 : 0);
