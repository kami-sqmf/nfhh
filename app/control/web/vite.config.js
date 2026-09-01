import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import Icons from 'unplugin-icons/vite'

// 產物直接放進 ../static，由 Rust 端用 rust-embed 打進單一執行檔。
// 開發時 /api 代理到本機跑的控制平面。
export default defineConfig({
  // 圖標在**建置時**內聯成 Svelte 元件，不打 Iconify 的 API。
  // 面板是單一 binary 自帶所有資產：執行時外連會洩漏使用狀況、離線就壞掉，
  // 也跟嚴格 CSP 衝突。@iconify-json/* 只是 devDependency，
  // 最後進 bundle 的只有實際 import 到的那幾個。
  plugins: [svelte(), tailwindcss(), Icons({ compiler: 'svelte' })],
  build: {
    outDir: '../static',
    emptyOutDir: true,
    // 面板是單頁、單使用者的內部工具，切 chunk 只會多幾次往返
    rollupOptions: { output: { codeSplitting: false } },
  },
  server: {
    proxy: { '/api': 'http://127.0.0.1:8081' },
  },
})
