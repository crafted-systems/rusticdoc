// Moves a built site under a base path, for hosting at
// https://<user>.github.io/<repo>/ rather than at a domain's root.
//
//   node scripts/rebase.mjs /rusticdoc
//
// Astro already puts its own assets and Starlight its own links under the
// base. Pages, components and Markdown also write links as root paths, such as
// href="/docs/start/quickstart/", so this prefixes every root path in the
// built HTML (href, src, action and meta refresh) that does not already start
// with the base. Protocol-relative URLs (//host) and absolute URLs are left
// alone. The CNAME goes too, since the domain it names is not this host.
import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const base = (process.argv[2] ?? '').replace(/\/+$/, '');
if (!/^\/[A-Za-z0-9._-]+(\/[A-Za-z0-9._-]+)*$/.test(base)) {
  console.error('usage: node scripts/rebase.mjs /base-path');
  process.exit(2);
}
const dist = new URL('../dist/', import.meta.url).pathname;
const escaped = base.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
const attribute = new RegExp(`(\\s(?:href|src|action)=["'])/(?!/|${escaped.slice(1)}(?:/|["']))`, 'g');
const refresh = new RegExp(`(content=["'][^"']*?url=)/(?!/|${escaped.slice(1)}(?:/|["']))`, 'gi');

let files = 0;
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path);
    else if (name.endsWith('.html')) {
      const html = readFileSync(path, 'utf8');
      const out = html.replace(attribute, `$1${base}/`).replace(refresh, `$1${base}/`);
      if (out !== html) {
        writeFileSync(path, out);
        files += 1;
      }
    }
  }
};
walk(dist);
rmSync(join(dist, 'CNAME'), { force: true });
console.log(`rebased ${files} pages under ${base}`);
