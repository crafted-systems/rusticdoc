import type { APIRoute } from 'astro';
import { DETAILS, SUMMARY, markdown, pages, text, url } from '../llms';

export const GET: APIRoute = async () => {
  const parts = [`# rusticdoc`, `> ${SUMMARY}`, DETAILS];
  for (const [, list] of await pages()) {
    for (const p of list) {
      const description = p.data.description ? `\n\n${p.data.description}` : '';
      parts.push(`---\n\n# ${p.data.title}\n\nSource: ${url(p)}${description}\n\n${markdown(p)}`);
    }
  }
  return text(`${parts.join('\n\n')}\n`);
};
