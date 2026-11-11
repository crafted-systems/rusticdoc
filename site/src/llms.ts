// The docs as plain Markdown for language models, at /llms.txt (an index) and
// /llms-full.txt (every page), as https://llmstxt.org/ describes. Pages come in
// sidebar order, and site links become absolute so they work out of context.
import { getCollection, type CollectionEntry } from 'astro:content';
import { VERSION } from './site';

export const SITE = 'https://rusticdoc.com';

// The sidebar's sections, in its order.
const SECTIONS: Array<[string, string]> = [
  ['start', 'Start here'],
  ['templates', 'Write templates'],
  ['guides', 'Use the API'],
  ['operate', 'Run it'],
  ['ai', 'Use with AI'],
  ['reference', 'Reference'],
  ['project', 'Project'],
];

export const SUMMARY = `rusticdoc is a self-hosted document service written in Rust, at version ${VERSION}. It fills Office (.docx, .xlsx, .pptx, .odt, .ods, .odp), text, HTML and PDF form templates from JSON, converts the result with LibreOffice or Fulgur, and finishes it with PDF operations, in one multipart HTTP request or from the \`rusticdoc\` CLI.`;

export const DETAILS = `Things to know before writing templates or calls:

- Templates use a Jinja-style syntax evaluated by minijinja: \`{{ customer.name }}\`, \`{% for %}\`, \`{% if %}\` and filters. It is not Carbone's \`{d.field}\` or docxtemplater's \`{#items}\` syntax.
- A missing value stops the render with an error instead of printing nothing. \`POST /v1/templates/validate\` lists every problem in a template against sample data.
- \`image\` takes base64 or a data URL and never fetches a URL. HTML is laid out by Fulgur with no browser, no JavaScript and no network.
- There is no hosted service: every call goes to the reader's own deployment, usually on port 9027, with an \`X-Api-Key\` header when credentials are configured.
- Agent skills for writing templates and calling the API are in the repository's \`skills/\` folder: https://github.com/crafted-systems/rusticdoc/tree/main/skills`;

type Page = CollectionEntry<'docs'>;

export async function pages(): Promise<Array<[string, Page[]]>> {
  const all = await getCollection('docs', (p) => p.id.startsWith('docs/') && p.id.split('/').length === 3);
  const order = (p: Page) => p.data.sidebar?.order ?? Number.MAX_SAFE_INTEGER;
  return SECTIONS.map(([dir, label]) => [
    label,
    all
      .filter((p) => p.id.split('/')[1] === dir)
      .sort((a, b) => order(a) - order(b) || a.data.title.localeCompare(b.data.title)),
  ]);
}

export const url = (p: Page) => `${SITE}/${p.id}/`;

// A page's Markdown with absolute links and without the code fence options only
// the site reads.
export function markdown(p: Page): string {
  return (p.body ?? '')
    .replace(/^([ \t]*```[a-z]*)[ \t].*$/gm, '$1')
    // Starlight asides, :::note[Title] … :::, as block quotes.
    .replace(/^:::(\w+)(?:\[(.*?)\])?\n([\s\S]*?)\n:::$/gm, (_, kind: string, label: string | undefined, body: string) => {
      const heading = label ?? kind[0].toUpperCase() + kind.slice(1);
      return `> **${heading}**\n>\n${body.split('\n').map((l) => (l ? `> ${l}` : '>')).join('\n')}`;
    })
    .replace(/\]\((\/[^)\s]*)\)/g, (_, target) => `](${SITE}${target})`)
    .trim();
}

export function text(body: string): Response {
  return new Response(body, { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
}
