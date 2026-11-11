#!/usr/bin/env node
// Keeps the agent skills in skills/ in step with the docs and the version.
//   node scripts/generate-skills.mjs           regenerate
//   node scripts/generate-skills.mjs --check   fail if anything is stale
// Each skill's references/ folder is copied from the docs pages below, with the
// front matter turned into a heading and site links made to work outside the
// site. The version each SKILL.md names, in its metadata and in image tags,
// follows the workspace version in Cargo.toml. Shared scripts are copied from
// the skill that owns them, because a host installs each skill folder alone.
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(site, '..');
const docs = join(site, 'src/content/docs/docs');
const skills = join(repo, 'skills');
const check = process.argv.includes('--check');
const SITE = 'https://rusticdoc.com';

// The docs pages each skill carries, as <section>/<page>.
const REFERENCES = {
  'rusticdoc-templates': [
    'templates/syntax', 'templates/filters', 'templates/conditions', 'templates/calculations',
    'templates/office', 'templates/rich-content', 'templates/pdf-templates', 'templates/translations',
    'templates/batch', 'templates/validate', 'guides/html',
  ],
  'rusticdoc-api': [
    'guides/pipeline', 'guides/pdf-operations', 'guides/convert', 'guides/stored-templates',
    'guides/background-jobs', 'guides/webhooks', 'guides/authentication', 'guides/fonts',
    'reference/errors', 'reference/cli',
  ],
};

// Files one skill owns and the others carry a copy of: [owner, path, copies].
const SHARED = [
  ['rusticdoc-templates', 'scripts/detect.sh', ['rusticdoc-api']],
  ['rusticdoc-templates', 'scripts/call.sh', ['rusticdoc-api']],
];

const version = readFileSync(join(repo, 'Cargo.toml'), 'utf8').match(/^version = "([^"]+)"/m)?.[1];
if (!version) throw new Error('no workspace version in Cargo.toml');

const stale = [];
function write(path, text) {
  if (existsSync(path) && readFileSync(path, 'utf8') === text) return;
  if (check) {
    stale.push(relative(repo, path));
    return;
  }
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
}

// A docs page as a reference file: the title and description as its opening,
// links to pages in the same skill as relative links, and every other site
// link as an absolute one. Code fence options only the site reads are dropped.
function reference(page, pages) {
  const source = readFileSync(join(docs, `${page}.md`), 'utf8');
  const match = source.match(/^---\n([\s\S]*?)\n---\n/);
  if (!match) throw new Error(`${page}.md has no front matter`);
  const field = (name) => match[1].match(new RegExp(`^${name}: (.*)$`, 'm'))?.[1]?.trim();
  const title = field('title');
  const description = field('description');
  let body = source.slice(match[0].length);
  body = body.replace(/^([ \t]*```[a-z]*)[ \t].*$/gm, '$1');
  // Starlight asides, :::note[Title] … :::, as block quotes.
  body = body.replace(/^:::(\w+)(?:\[(.*?)\])?\n([\s\S]*?)\n:::$/gm, (_, kind, label, text) => {
    const heading = label ?? kind[0].toUpperCase() + kind.slice(1);
    return `> **${heading}**\n>\n${text.split('\n').map((l) => (l ? `> ${l}` : '>')).join('\n')}`;
  });
  body = body.replace(/\]\((\/[^)\s]*)\)/g, (_, target) => {
    const [path, anchor] = target.split('#');
    const name = path.replace(/^\/docs\//, '').replace(/\/$/, '');
    if (pages.includes(name)) return `](${name.split('/').pop()}.md${anchor ? `#${anchor}` : ''})`;
    return `](${SITE}${target})`;
  });
  const notice = `<!-- Generated from site/src/content/docs/docs/${page}.md by site/scripts/generate-skills.mjs. Do not edit. -->`;
  return `# ${title}\n\n${notice}\n\n${description}\n${body.startsWith('\n') ? '' : '\n'}${body}`;
}

for (const [skill, pages] of Object.entries(REFERENCES)) {
  const folder = join(skills, skill, 'references');
  const wanted = new Set(pages.map((p) => `${p.split('/').pop()}.md`));
  for (const page of pages) write(join(folder, `${page.split('/').pop()}.md`), reference(page, pages));
  // A page dropped from the list leaves no file behind.
  if (existsSync(folder)) {
    for (const file of readdirSync(folder)) {
      if (wanted.has(file)) continue;
      if (check) stale.push(relative(repo, join(folder, file)));
      else rmSync(join(folder, file));
    }
  }
}

// The tag of every image a file names follows the workspace version.
const tagged = (text) => text.replace(/(ghcr\.io\/crafted-systems\/rusticdoc:)\d+\.\d+\.\d+/g, `$1${version}`);

for (const [owner, path, copies] of SHARED) {
  const text = tagged(readFileSync(join(skills, owner, path), 'utf8'));
  for (const skill of [owner, ...copies]) write(join(skills, skill, path), text);
}

// Each SKILL.md names its version as `version: "x.y.z"` in its metadata.
for (const skill of Object.keys(REFERENCES)) {
  const path = join(skills, skill, 'SKILL.md');
  const text = readFileSync(path, 'utf8');
  if (!/^ {2}version: "[^"]*"$/m.test(text)) throw new Error(`${skill}/SKILL.md has no metadata version`);
  write(path, tagged(text.replace(/^( {2}version: )"[^"]*"$/m, `$1"${version}"`)));
}

if (check && stale.length) {
  console.error(`Out of date, run make agent-docs:\n${stale.join('\n')}`);
  process.exit(1);
}
console.log(check ? 'skills are current' : 'skills written');
