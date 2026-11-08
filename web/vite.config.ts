import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// `npm run dev` proxies the API to a local sdrd (e.g. docker-compose.dev.yml).
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': { target: 'http://localhost:8080', ws: true, changeOrigin: false },
    },
  },
  build: { target: 'es2022', sourcemap: false },
});
