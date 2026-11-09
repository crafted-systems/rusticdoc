import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

export default defineConfig({
  site: 'https://rusticdoc.com',
  trailingSlash: 'always',
  devToolbar: { enabled: false },
  // Security and Status were pages of their own before they joined the docs.
  redirects: {
    '/security/': '/docs/project/security-overview/',
    '/status/': '/docs/project/status/',
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
      sidebar: [
        { label: 'Start here', items: [{ autogenerate: { directory: 'docs/start' } }] },
        { label: 'Write templates', items: [{ autogenerate: { directory: 'docs/templates' } }] },
        { label: 'Use the API', items: [{ autogenerate: { directory: 'docs/guides' } }] },
        { label: 'Run it', items: [{ autogenerate: { directory: 'docs/operate' } }] },
        {
          label: 'Reference',
          items: [
            { label: 'HTTP API', collapsed: true, items: [{ autogenerate: { directory: 'docs/reference/api' } }] },
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
