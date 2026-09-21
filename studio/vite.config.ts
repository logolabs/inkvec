import { defineConfig } from "vite";

// Tauri serves the frontend from a fixed port in development and from the bundled
// `dist` directory in a release build. `clearScreen: false` keeps the Rust compiler's
// output visible while `tauri dev` runs both halves in one terminal.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // The webviews Tauri v2 targets: WebKitGTK on Linux, WKWebView on macOS, WebView2
    // (Chromium) on Windows. All three are evergreen enough for es2021.
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
    chunkSizeWarningLimit: 900,
  },
});
