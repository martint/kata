import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte({ hot: !process.env.VITEST })],
  // Svelte 5 publishes separate `server` and `browser` (default) entry
  // points. Vitest's default `node` conditions land on the server
  // build, which throws `lifecycle_function_unavailable: mount(...)`
  // the moment a component test tries to render anything. Pin the
  // `browser` condition globally so component tests get the real
  // client runtime.
  resolve: {
    conditions: ['browser'],
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:7878',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: true,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    setupFiles: ['./vitest.setup.ts'],
  },
});
