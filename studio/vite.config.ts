import { defineConfig } from "vite";

import { version } from "./package.json";

// Tauri serves the frontend from a fixed port in development and from the bundled
// `dist` directory in a release build. `clearScreen: false` keeps the Rust compiler's
// output visible while `tauri dev` runs both halves in one terminal.
export default defineConfig({
  clearScreen: false,
  // The splash window says which version this is, and it should not have to ask the
  // backend to do it: it has to paint before anything else is ready.
  define: { __APP_VERSION__: JSON.stringify(version) },
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
    rollupOptions: {
      // Two pages: the app, and the splash window that covers its start-up.
      input: {
        main: "index.html",
        splash: "splash.html",
      },
    },
  },
});
