// The blog as an RSS 2.0 feed, built with the site. Each item carries the
// post's summary and links to the post.
import type { APIRoute } from 'astro';
import { postPath, posts } from '../../blog';
import { SITE } from '../../seo';

const escape = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export const GET: APIRoute = async () => {
  const all = await posts();
  const items = all.map((p) => {
    const url = new URL(postPath(p), SITE).href;
    return `    <item>
      <title>${escape(p.data.title)}</title>
      <link>${url}</link>
      <guid isPermaLink="true">${url}</guid>
      <description>${escape(p.data.description)}</description>
      <pubDate>${p.data.date.toUTCString()}</pubDate>
    </item>`;
  });
  const updated = all[0]?.data.date ?? new Date(0);
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>rusticdoc blog</title>
    <link>${SITE}/blog/</link>
    <atom:link href="${SITE}/blog/rss.xml" rel="self" type="application/rss+xml" />
    <description>News about rusticdoc: what each release brings, how it works, and why it works that way.</description>
    <language>en</language>
    <lastBuildDate>${updated.toUTCString()}</lastBuildDate>
${items.join('\n')}
  </channel>
</rss>
`;
  return new Response(xml, { headers: { 'Content-Type': 'application/rss+xml; charset=utf-8' } });
};
