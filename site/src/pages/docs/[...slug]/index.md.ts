// Every docs page as Markdown, at its own address plus index.md, for agents
// and for the Copy as Markdown button. The HTML page links here with
// <link rel="alternate" type="text/markdown">.
import type { APIRoute, GetStaticPaths } from 'astro';
import { getCollection, type CollectionEntry } from 'astro:content';
import { markdown, url } from '../../../llms';

export const getStaticPaths: GetStaticPaths = async () => {
  const docs = await getCollection('docs', (p) => p.id === 'docs' || p.id.startsWith('docs/'));
  return docs.map((page) => ({
    params: { slug: page.id === 'docs' ? undefined : page.id.slice('docs/'.length) },
    props: { page },
  }));
};

export const GET: APIRoute<{ page: CollectionEntry<'docs'> }> = ({ props: { page } }) => {
  const { title, description } = page.data;
  const head = [`# ${title}`, description ? `> ${description}` : '', `Source: ${url(page)}`].filter(Boolean);
  return new Response(`${head.join('\n\n')}\n\n${markdown(page)}\n`, {
    headers: { 'Content-Type': 'text/markdown; charset=utf-8' },
  });
};
