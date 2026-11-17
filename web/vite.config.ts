import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import pkg from './package.json' with { type: 'json' };

// `npm run dev` proxies the API to a local sdrd (e.g. docker-compose.dev.yml).
export default defineConfig({
  plugins: [svelte()],
  // Shown on Settings → About.
  define: { __APP_VERSION__: JSON.stringify(pkg.version) },
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8080', ws: true, changeOrigin: false },
    },
  },
  build: { target: 'es2022', sourcemap: false },
});
