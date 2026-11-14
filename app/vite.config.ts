import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The main window (Setup / Settings / Activity) is a Svelte app; the
// overlay and approval pages are plain files in public/ copied as they are.
export default defineConfig({
  plugins: [svelte()],
  publicDir: 'public',
  clearScreen: false,
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'safari15',
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
