/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": `${import.meta.dirname}/src` },
  },
  clearScreen: false,
  // 最大的块是数据统计页按需加载的 echarts（已按需引入柱 / 折线 / 饼图，约 530 KB），不影响启动；主包约 430 KB
  build: { chunkSizeWarningLimit: 600 },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/.superpowers/**"] },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    include: ["src/**/*.test.{ts,tsx}"],
  },
}));
