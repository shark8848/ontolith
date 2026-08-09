// Ontolith console — Vite 8 frontend build.
// Dev: `npm run dev` (Vite on :5173) proxies /api to the backend server
// (`npm run dev:api` → node server.js on :8890). Prod: `npm run build`
// emits dist/ which server.js serves on CONSOLE_BIND.
import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    port: 5173,
    strictPort: false,
    proxy: {
      '/api': 'http://127.0.0.1:8890',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
});
