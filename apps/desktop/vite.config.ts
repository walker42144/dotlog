import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 固定端口，避免 devUrl 与真实端口不一致。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] }
  },
  build: {
    target: "chrome120",
    sourcemap: false
  }
});
