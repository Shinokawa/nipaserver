import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// nipaserver WebUI（docs/05-webui设计.md §7）
// 开发期：/api 代理到本机 nipa-server；构建产物落 dist/（后续 rust-embed 进服务器）。
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:11810',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
