import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import starlightOpenAPI, { createOpenAPISidebarGroup } from 'starlight-openapi';
import { prepareApiSpec } from './scripts/api-spec.mjs';

// The HTTP API reference is built from the service's OpenAPI description. This
// placeholder marks where its pages go in the sidebar.
const httpApi = createOpenAPISidebarGroup();

export default defineConfig({
  site: 'https://rusticdoc.com',
  trailingSlash: 'always',
  devToolbar: { enabled: false },
  // Moved pages redirect in public/_redirects, not in Astro's `redirects`: on a
  // static build those become 200 pages with a canonical tag, which Google
  // reports as alternates instead of following them as moves.
  // Keep small fonts as files: the site's CSP has no data: in font-src.
  vite: { build: { assetsInlineLimit: 0 } },
  integrations: [
    starlight({
      title: 'rusticdoc',
      description: 'Keep document layout out of your code: fill Office, HTML and PDF templates from JSON, convert them with LibreOffice and finish them with PDF operations, in one self-hosted container.',
      logo: { light: './src/assets/logo-light.svg', dark: './src/assets/logo-dark.svg', alt: '' },
      favicon: '/favicon.svg',
      disable404Route: true,
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/crafted-systems/rusticdoc' }],
      editLink: { baseUrl: 'https://github.com/crafted-systems/rusticdoc/edit/main/site/' },
      lastUpdated: false,
      // Starlight sets og:title and og:description per page; the share image
      // and the pointers for agents are the same everywhere.
      head: [
        { tag: 'meta', attrs: { property: 'og:image', content: 'https://rusticdoc.com/og.png' } },
        { tag: 'meta', attrs: { property: 'og:image:width', content: '1200' } },
        { tag: 'meta', attrs: { property: 'og:image:height', content: '630' } },
        { tag: 'meta', attrs: { property: 'og:image:alt', content: 'rusticdoc: keep document layout out of your code, fill templates from JSON' } },
        { tag: 'meta', attrs: { name: 'twitter:image', content: 'https://rusticdoc.com/og.png' } },
        { tag: 'link', attrs: { rel: 'alternate', type: 'text/plain', href: '/llms.txt', title: 'Documentation for language models' } },
      ],
      routeMiddleware: './src/routeData.ts',
      components: {
        Header: './src/components/Header.astro',
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSwitch.astro',
        PageTitle: './src/components/PageTitle.astro',
      },
      customCss: ['./src/styles/fonts.css', './src/styles/tokens.css', './src/styles/docs.css'],
      // Code block settings live in ec.config.mjs: they use functions, which
      // Expressive Code reads only from there.
      expressiveCode: true,
      plugins: [
        starlightOpenAPI([
          {
            base: 'docs/reference/api',
            // The service's description with the site's additions: a base
            // URL and a checked curl example for every operation.
            schema: prepareApiSpec(),
            sidebar: {
              label: 'HTTP API',
              collapsed: true,
              group: httpApi,
              operations: { badges: true, labels: 'summary', sort: 'document' },
            },
          },
        ]),
      ],
      sidebar: [
        { label: 'Start here', items: [{ autogenerate: { directory: 'docs/start' } }] },
        { label: 'Use cases', items: [{ autogenerate: { directory: 'docs/use-cases' } }] },
        { label: 'Write templates', items: [{ autogenerate: { directory: 'docs/templates' } }] },
        { label: 'Use the API', items: [{ autogenerate: { directory: 'docs/guides' } }] },
        { label: 'Run it', items: [{ autogenerate: { directory: 'docs/operate' } }] },
        { label: 'Use with AI', items: [{ autogenerate: { directory: 'docs/ai' } }] },
        {
          label: 'Reference',
          items: [
            httpApi,
            { slug: 'docs/reference/configuration' },
            { slug: 'docs/reference/cli' },
            { slug: 'docs/reference/errors' },
          ],
        },
        { label: 'Compare', collapsed: true, items: [{ autogenerate: { directory: 'docs/compare' } }] },
        {
          label: 'Project',
          items: [{ autogenerate: { directory: 'docs/project' } }],
        },
      ],
    }),
    sitemap(),
  ],
});
