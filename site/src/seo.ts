// Structured data (schema.org JSON-LD) for search engines and agents. The
// marketing pages and the docs both build theirs from here, so the facts about
// the project live in one place.
import { REPO, VERSION } from './site';

export const SITE = 'https://rusticdoc.com';
export const OG_IMAGE = `${SITE}/og.png`;
export const OG_IMAGE_ALT = 'rusticdoc: keep document layout out of your code, fill templates from JSON';

const ORG_ID = `${SITE}/#organization`;
const SITE_ID = `${SITE}/#website`;
const APP_ID = `${SITE}/#software`;

export const organization = {
  '@type': 'Organization',
  '@id': ORG_ID,
  name: 'Crafted Systems',
  url: 'https://crafted.co.ke/',
  sameAs: ['https://github.com/crafted-systems/', 'https://www.linkedin.com/company/crafted-systems'],
};

export const website = {
  '@type': 'WebSite',
  '@id': SITE_ID,
  name: 'rusticdoc',
  url: `${SITE}/`,
  publisher: { '@id': ORG_ID },
  inLanguage: 'en',
};

export const software = {
  '@type': 'SoftwareApplication',
  '@id': APP_ID,
  name: 'rusticdoc',
  description:
    'A self-hosted document service that fills Office (DOCX, XLSX, PPTX, ODT, ODS, ODP), HTML and PDF form templates from JSON, converts them with LibreOffice and finishes them with PDF operations. The layout lives in the template file, not in application code.',
  url: `${SITE}/`,
  applicationCategory: 'DeveloperApplication',
  operatingSystem: 'Linux (container image for amd64 and arm64)',
  softwareVersion: VERSION,
  license: 'https://www.apache.org/licenses/LICENSE-2.0',
  isAccessibleForFree: true,
  offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
  downloadUrl: `${REPO}/pkgs/container/rusticdoc`,
  softwareHelp: `${SITE}/docs/`,
  releaseNotes: `${SITE}/docs/project/changelog/`,
  codeRepository: REPO,
  programmingLanguage: 'Rust',
  author: { '@id': ORG_ID },
  publisher: { '@id': ORG_ID },
  image: OG_IMAGE,
};

export function faqPage(items: Array<{ q: string; a: string }>) {
  return {
    '@type': 'FAQPage',
    mainEntity: items.map((f) => ({
      '@type': 'Question',
      name: f.q,
      acceptedAnswer: { '@type': 'Answer', text: f.a },
    })),
  };
}

export function breadcrumbs(trail: Array<{ name: string; url: string }>) {
  return {
    '@type': 'BreadcrumbList',
    itemListElement: trail.map((t, i) => ({ '@type': 'ListItem', position: i + 1, name: t.name, item: t.url })),
  };
}

/** One `<script type="application/ld+json">` body for a page's nodes. */
export function graph(...nodes: object[]): string {
  // Escape `<` so no string in the data can close the script element.
  return JSON.stringify({ '@context': 'https://schema.org', '@graph': nodes }).replace(/</g, '\\u003c');
}
