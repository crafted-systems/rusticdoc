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
  // Security was a page of its own before it joined the docs. Status has left
  // the site for docs/STATUS.md in the repository, so its address goes to the
  // known limits.
  redirects: {
    '/security/': '/docs/project/security-overview/',
    '/status/': '/docs/project/known-limits/',
  },
  // Keep small fonts as files: the site's CSP has no data: in font-src.
  vite: { build: { assetsInlineLimit: 0 } },
  integrations: [
    starlight({
      title: 'rusticdoc',
      description: 'Fill Office and PDF templates from JSON, convert them with LibreOffice and finish them with PDF operations, in one self-hosted container.',
      logo: { light: './src/assets/logo-light.svg', dark: './src/assets/logo-dark.svg', alt: '' },
      favicon: '/favicon.svg',
      disable404Route: true,
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/vmosoti/rusticdoc' }],
      editLink: { baseUrl: 'https://github.com/vmosoti/rusticdoc/edit/main/site/' },
      lastUpdated: false,
      components: {
        Header: './src/components/Header.astro',
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSwitch.astro',
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
        { label: 'Write templates', items: [{ autogenerate: { directory: 'docs/templates' } }] },
        { label: 'Use the API', items: [{ autogenerate: { directory: 'docs/guides' } }] },
        { label: 'Run it', items: [{ autogenerate: { directory: 'docs/operate' } }] },
        {
          label: 'Reference',
          items: [
            httpApi,
            { slug: 'docs/reference/configuration' },
            { slug: 'docs/reference/cli' },
            { slug: 'docs/reference/errors' },
          ],
        },
        {
          label: 'Project',
          items: [{ autogenerate: { directory: 'docs/project' } }],
        },
      ],
    }),
    sitemap(),
  ],
});
