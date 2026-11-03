import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;
const testPort = process.env.LOCAL_AGENT_TEST_PORT;
const testing = /^\d+$/.test(testPort ?? "");

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],
  cacheDir: testing ? `.work/vite-test-cache-${testPort}` : undefined,
  // Archived evidence is not an entry point. Keep cold dependency scans on
  // the shipped app and, during tests, the current interaction fixtures.
  optimizeDeps: {
    entries: testing ? ["index.html", "tests/frontend/*.html"] : ["index.html"],
  },
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    // Test fixtures own their state; a dev-server reload must not reset it.
    hmr: testing
      ? false
      : host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
    watch: {
      // Generated Rust artifacts, capacity fixtures and archived evidence are
      // never frontend sources. Their writes must not trigger reloads or scans.
      ignored: [
        "**/src-tauri/**",
        "**/target/**",
        "**/.work/**",
        "**/output/**",
      ],
    },
  },
}));
