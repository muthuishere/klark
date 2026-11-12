import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';

// Library build (build.mjs runs it): ESM only, React and xterm left to the app.
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: path.resolve(__dirname, 'release/dist'),
    emptyOutDir: true,
    sourcemap: true,
    minify: false,
    lib: { entry: path.resolve(__dirname, 'src/index.ts'), formats: ['es'], fileName: () => 'index.js' },
    rollupOptions: { external: [/^react($|\/)/, /^react-dom($|\/)/, /^@xterm\//] },
  },
  test: { environment: 'jsdom', globals: true, setupFiles: ['test/setup.ts'], include: ['test/**/*.test.{ts,tsx}'], css: false },
});
