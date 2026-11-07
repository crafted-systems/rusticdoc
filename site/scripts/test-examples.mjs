#!/usr/bin/env node
// Runs every shell example marked `test` in the docs against a real service.
//   node scripts/test-examples.mjs [path filter]
// Starts IMAGE (default rusticdoc:site) on port 9027 unless NO_START is set.
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const docs = join(root, 'src/content/docs/docs');
const filter = process.argv[2] ?? '';
const NAME = 'rusticdoc-site-examples';
const start = !process.env.NO_START;
const image = process.env.IMAGE ?? 'rusticdoc:site';

const docker = (...args) => execFileSync('docker', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : /\.mdx?$/.test(name) ? [path] : [];
  });
}

function blocks(path) {
  const text = readFileSync(path, 'utf8');
  const found = [];
  for (const m of text.matchAll(/^```(?:bash|sh)([^\n]*)\n([\s\S]*?)^```/gm)) {
    const meta = m[1];
    if (!/(^|\s)test(=fail)?(\s|$)/.test(meta)) continue;
    found.push({
      code: m[2],
      mustFail: /test=fail/.test(meta),
      line: text.slice(0, m.index).split('\n').length,
    });
  }
  return found;
}

function waitReady() {
  for (let i = 0; i < 60; i++) {
    if (spawnSync('curl', ['-sf', 'http://localhost:9027/ready'], { stdio: 'ignore' }).status === 0) return;
    spawnSync('sleep', ['1']);
  }
  throw new Error('the service did not become ready');
}

if (start) {
  spawnSync('docker', ['rm', '-f', NAME], { stdio: 'ignore' });
  docker('run', '-d', '--name', NAME, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
    '--tmpfs', '/tmp', '--tmpfs', '/home/rusticdoc:uid=1001,gid=1001',
    '-e', 'RUSTICDOC_STORE=file:///tmp/store', '-p', '127.0.0.1:9027:9027', image);
}
waitReady();

// A font from the image, so the repository carries none.
const fixtures = mkdtempSync(join(tmpdir(), 'rusticdoc-fixtures-'));
cpSync(join(root, 'samples/fixtures'), fixtures, { recursive: true });
cpSync(join(root, 'public/samples/invoice-template.docx'), join(fixtures, 'invoice.docx'));
cpSync(join(root, 'samples/invoice.json'), join(fixtures, 'invoice.json'));
const container = process.env.CONTAINER ?? NAME;
const fontPath = docker('exec', container, 'sh', '-c', 'find / -name "Caladea-Regular.ttf" 2>/dev/null | head -1').trim();
docker('cp', `${container}:${fontPath}`, join(fixtures, 'font.ttf'));
execFileSync('zip', ['-qj', join(fixtures, 'brand-fonts.zip'), join(fixtures, 'font.ttf')]);

const prelude = `set -euo pipefail
curl() { command curl -sS --fail-with-body "$@"; }
export KEY=example-key ADMIN_KEY=example-admin-key
`;

let passed = 0;
const failures = [];
for (const path of walk(docs).sort()) {
  const rel = relative(docs, path);
  if (!rel.includes(filter)) continue;
  for (const block of blocks(path)) {
    const dir = mkdtempSync(join(tmpdir(), 'rusticdoc-example-'));
    cpSync(fixtures, dir, { recursive: true });
    const run = spawnSync('bash', ['-c', prelude + block.code], { cwd: dir, encoding: 'utf8', timeout: 120_000 });
    const ok = block.mustFail ? run.status !== 0 : run.status === 0;
    if (ok) passed++;
    else failures.push({ where: `${rel}:${block.line}`, status: run.status, out: (run.stdout + run.stderr).slice(-800) });
    rmSync(dir, { recursive: true, force: true });
  }
}
rmSync(fixtures, { recursive: true, force: true });
if (start) spawnSync('docker', ['rm', '-f', NAME], { stdio: 'ignore' });

for (const f of failures) console.error(`FAIL ${f.where} (exit ${f.status})\n${f.out}\n`);
console.log(`${passed} passed, ${failures.length} failed`);
process.exit(failures.length ? 1 : 0);
