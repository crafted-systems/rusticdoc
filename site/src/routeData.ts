// Starlight route middleware: what every docs page adds to its <head> for
// search engines and agents.
//   - a fuller <title> and og:title where the page sets `seoTitle`
//   - a link to the page as Markdown, served by pages/docs/[...slug]/index.md.ts
//   - breadcrumbs as schema.org JSON-LD
import { defineRouteMiddleware } from '@astrojs/starlight/route-data';
import { SITE, breadcrumbs, graph } from './seo';
import { hasMarkdown } from './llms';

// Pages that come from the OpenAPI plugin have no frontmatter to set
// `seoTitle` in.
const GENERATED_TITLES: Record<string, string> = {
  '/docs/reference/api/': 'HTTP API reference',
};

export const onRequest = defineRouteMiddleware(async (context) => {
  const route = context.locals.starlightRoute;
  const { entry, head } = route;
  const path = context.url.pathname;

  const seoTitle = entry.data.seoTitle ?? GENERATED_TITLES[path];
  if (seoTitle) {
    for (const tag of head) {
      if (tag.tag === 'title') tag.content = `${seoTitle} | rusticdoc`;
      if (tag.tag === 'meta' && tag.attrs?.property === 'og:title') tag.attrs.content = seoTitle;
    }
  }

  // Only pages written in Markdown have a Markdown version.
  if (await hasMarkdown(entry.id)) {
    head.push({ tag: 'link', attrs: { rel: 'alternate', type: 'text/markdown', href: `${path}index.md` }, content: '' });
  }

  const trail = [
    { name: 'rusticdoc', url: `${SITE}/` },
    { name: 'Documentation', url: `${SITE}/docs/` },
  ];
  if (path !== '/docs/') trail.push({ name: entry.data.title, url: new URL(path, SITE).href });
  head.push({ tag: 'script', attrs: { type: 'application/ld+json' }, content: graph(breadcrumbs(trail)) });
});
