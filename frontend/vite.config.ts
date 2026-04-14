import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:8765',
      '/slash': 'http://127.0.0.1:8765',
      '/llms.txt': 'http://127.0.0.1:8765',
    },
  },
});
