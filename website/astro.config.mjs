import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://cameronrye.github.io',
  base: '/aranet',
  integrations: [
    starlight({
      title: 'Aranet',
      description: 'A complete Rust ecosystem for Aranet environmental sensors - CO₂, temperature, humidity, pressure, radon, and radiation monitoring.',
      logo: {
        dark: './src/assets/aranet-logo-dark.svg',
        light: './src/assets/aranet-logo-light.svg',
        replacesTitle: true,
      },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/cameronrye/aranet' },
      ],
      editLink: {
        baseUrl: 'https://github.com/cameronrye/aranet/edit/main/website/',
      },
      lastUpdated: true,
      customCss: ['./src/styles/custom.css'],
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { slug: 'docs/getting-started' },
            { slug: 'docs/installation' },
            { slug: 'docs/quick-start' },
          ],
        },
        {
          label: 'CLI Reference',
          items: [
            { slug: 'docs/cli/overview' },
            { slug: 'docs/cli/commands' },
          ],
        },
        {
          label: 'Protocol',
          items: [
            { slug: 'docs/protocol/overview' },
            { slug: 'docs/protocol/uuids' },
            { slug: 'docs/protocol/data-parsing' },
          ],
        },
        {
          label: 'API Reference',
          items: [
            { slug: 'docs/rust-docs' },
          ],
        },
        {
          label: 'Resources',
          items: [
            { slug: 'docs/changelog' },
            { slug: 'docs/roadmap' },
          ],
        },
      ],
      head: [
        {
          tag: 'meta',
          attrs: { property: 'og:image', content: '/aranet/og-image.png' },
        },
        {
          tag: 'meta',
          attrs: { property: 'og:type', content: 'website' },
        },
      ],
      tableOfContents: {
        minHeadingLevel: 2,
        maxHeadingLevel: 3,
      },
    }),
  ],
});

