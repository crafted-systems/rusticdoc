import type { APIRoute } from 'astro';
import { DETAILS, SITE, SUMMARY, pages, text, url } from '../llms';

export const GET: APIRoute = async () => {
  const parts = [`# rusticdoc`, `> ${SUMMARY}`, DETAILS];
  for (const [label, list] of await pages()) {
    if (!list.length) continue;
    parts.push(`## ${label}\n\n${list.map((p) => `- [${p.data.title}](${url(p)}): ${p.data.description ?? ''}`.trimEnd()).join('\n')}`);
  }
  parts.push(`## Optional\n\n- [Every page in one file](${SITE}/llms-full.txt): the documentation above as one Markdown file\n- [OpenAPI description](${SITE}/openapi.json): every endpoint, field and response of the HTTP API`);
  return text(`${parts.join('\n\n')}\n`);
};
