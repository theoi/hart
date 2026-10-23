import { defineConfig } from 'vite';

export default defineConfig({
  // Relative asset URLs, so the site works at / locally and at /hart/ on Pages.
  base: './',
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    assetsInlineLimit: 0,
    reportCompressedSize: false,
  },
  worker: {
    format: 'es',
  },
});
