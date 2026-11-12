import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';
import { mockApi } from './mock/plugin';

// `npm run dev`      proxies the API to the Go hub on 127.0.0.1:7470.
// `npm run dev:mock` serves fixtures from mock/ instead, so the UI runs with no backend.
const target = process.env.HUB_URL || 'http://127.0.0.1:7470';
const useMock = process.env.MOCK === '1';

export default defineConfig({
  base: './', // relative asset URLs: the page's <base href> decides the prefix
  plugins: [react(), ...(useMock ? [mockApi()] : [])],
  // The app builds the package from source (packages/hub-ui): one copy of the components, no prebuild step.
  resolve: { alias: { '@muthuishere/klark-hub-ui/style.css': path.resolve(__dirname, 'packages/hub-ui/style.css'), '@muthuishere/klark-hub-ui': path.resolve(__dirname, 'packages/hub-ui/src/index.ts') } },
  build: { outDir: path.resolve(__dirname, '../internal/ui/dist'), emptyOutDir: true },
  server: {
    proxy: useMock
      ? {}
      : Object.fromEntries(['/api', '/auth', '/agent', '/mock-adfs'].map((p) => [p, { target, changeOrigin: false }])),
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    css: false,
  },
});
