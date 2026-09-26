import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri-friendly Vite configuration.
//
// - port 1420 is fixed and `strictPort` is on so the Tauri devUrl never silently drifts;
// - `clearScreen: false` keeps the Rust/cargo output visible in `tauri dev`;
// - the watcher ignores the Rust crate so `cargo` rebuilds do not retrigger the frontend;
// - no CDN, no remote fonts, no network at runtime: everything is bundled at build time.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_"],
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    hmr: {
      host: "127.0.0.1",
      port: 1421,
    },
    watch: {
      ignored: ["**/crates/**", "**/src-tauri/**", "**/sidecar/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
    minify: "esbuild",
    chunkSizeWarningLimit: 900,
  },
});
