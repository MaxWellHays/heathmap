import { defineConfig } from 'vite';

// Served from https://<user>.github.io/heathmap/ on GitHub Pages.
export default defineConfig({
  base: '/heathmap/',
  build: { chunkSizeWarningLimit: 1500 }, // MapLibre alone is ~1 MB
});
