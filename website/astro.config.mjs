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
            { slug: 'docs/downloads' },
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
          label: 'API Reference',
          items: [
            { slug: 'docs/rust-docs' },
          ],
        },
        {
          label: 'Protocol',
          collapsed: true,
          items: [
            { slug: 'docs/protocol/overview' },
            { slug: 'docs/protocol/uuids' },
            { slug: 'docs/protocol/data-parsing' },
          ],
        },
        {
          label: 'Help & Resources',
          items: [
            { slug: 'docs/troubleshooting' },
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
        {
          tag: 'script',
          content: `
            // Back to top button
            document.addEventListener('DOMContentLoaded', function() {
              // Create back to top button
              const btn = document.createElement('button');
              btn.className = 'back-to-top';
              btn.setAttribute('aria-label', 'Back to top');
              btn.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 15l-6-6-6 6"/></svg>';
              document.body.appendChild(btn);

              // Show/hide based on scroll position
              let ticking = false;
              window.addEventListener('scroll', function() {
                if (!ticking) {
                  window.requestAnimationFrame(function() {
                    if (window.scrollY > 400) {
                      btn.classList.add('visible');
                    } else {
                      btn.classList.remove('visible');
                    }
                    ticking = false;
                  });
                  ticking = true;
                }
              });

              // Scroll to top on click
              btn.addEventListener('click', function() {
                window.scrollTo({ top: 0, behavior: 'smooth' });
              });
            });
          `,
        },
      ],
      tableOfContents: {
        minHeadingLevel: 2,
        maxHeadingLevel: 3,
      },
    }),
  ],
});

