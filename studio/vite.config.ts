import { defineConfig, type Plugin } from "vite";

import { version } from "./package.json";

// Node's own; declared rather than pulling in @types/node for one read.
declare const process: { env: Record<string, string | undefined> };

// Two builds of one interface.
//
// The default is the desktop app, Inkvec Studio: Tauri serves the frontend from a fixed port
// in development and from the bundled `dist` directory in a release build.
// `clearScreen: false` keeps the Rust compiler's output visible while `tauri dev` runs both
// halves in one terminal.
//
// `--mode web` is the browser build, Inkvec Studio Lite (`npm run build:web`, via
// `scripts/build-web.mjs`, which first stages the WebAssembly, the denoiser loader, the
// samples and the notices into `web/public`). It is a static site for the Hugging Face
// Space: relative URLs, one page, and the cross-origin isolation headers the threaded
// engine needs on the dev server too.
/**
 * The page as the browser build serves it: its own name, a loading state for the seconds
 * the WebAssembly takes to arrive (the desktop has its splash window for that), and a
 * content policy for a web page rather than a webview -- the denoiser's runtime and weights
 * come from jsDelivr and Hugging Face, and only when asked for.
 */
function webPage(): Plugin {
  const csp = [
    "default-src 'self'",
    "script-src 'self' 'wasm-unsafe-eval' https://cdn.jsdelivr.net",
    "worker-src 'self' blob:",
    "img-src 'self' blob: data:",
    "style-src 'self' 'unsafe-inline'",
    "font-src 'self'",
    "connect-src 'self' https://huggingface.co https://*.huggingface.co https://*.hf.co https://cdn.jsdelivr.net",
  ].join("; ");
  return {
    name: "inkvec-web-page",
    transformIndexHtml(html, ctx) {
      // The splash page is shared with the desktop as it is; only the app's page changes.
      if (!ctx.path.endsWith("index.html")) return html;
      return html
        .replace(/<title>[^<]*<\/title>/, "<title>Inkvec Studio Lite</title>")
        .replace(/content="default-src[^"]*"/, `content="${csp}"`)
        .replace(
          "<!-- A desktop app in a webview: no remote origins, no inline script, no eval. -->",
          "<!-- A web page: its own origin, plus the denoiser's runtime and weights when asked for. -->",
        )
        .replace(
          '<div id="app"></div>',
          // The loading screen is the desktop's splash window itself, shown as a card over the
          // page until the engine is ready and its animation has played (see lib/web/chrome.ts).
          '<div id="boot" class="boot"><iframe src="./splash.html" title="Inkvec Studio Lite is starting"></iframe></div>\n    <div id="app"></div>',
        )
        .replace(
          '<meta name="viewport" content="width=device-width, initial-scale=1.0" />',
          '<meta name="viewport" content="width=device-width, initial-scale=1.0" />\n    <meta name="description" content="Turn a raster logo into an exact SVG, in your browser. Nothing is uploaded." />\n    <link rel="icon" href="./favicon.svg" type="image/svg+xml" />',
        );
    },
  };
}

export default defineConfig(({ mode }) => {
  const web = mode === "web";
  const isolation = {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "cross-origin",
  };
  return {
    clearScreen: false,
    plugins: web ? [webPage()] : [],
    base: web ? "./" : "/",
    publicDir: web ? "web/public" : "public",
    // The splash window says which version this is, and it should not have to ask the
    // backend to do it: it has to paint before anything else is ready.
    define: {
      __APP_VERSION__: JSON.stringify(version),
      __INKVEC_WEB__: JSON.stringify(web),
      __INKVEC_WASM_TOKEN__: JSON.stringify(process.env.INKVEC_WASM_TOKEN ?? ""),
    },
    server: {
      port: web ? 1430 : 1420,
      strictPort: true,
      watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
      // The Showcase screen bundles the Space's gallery data, which lives beside the page
      // that also reads it (`web/showcase.json`, written by `tools/showcase_data.py`).
      fs: { allow: [".", "../web/showcase.json"] },
      headers: web ? isolation : undefined,
    },
    preview: { headers: web ? isolation : undefined },
    envPrefix: ["VITE_", "TAURI_"],
    worker: { format: "es" as const },
    build: {
      // The webviews Tauri v2 targets: WebKitGTK on Linux, WKWebView on macOS, WebView2
      // (Chromium) on Windows. All three are evergreen enough for es2021, and so is every
      // browser that can run the threaded WebAssembly the web build needs.
      target: "es2021",
      minify: "esbuild" as const,
      sourcemap: false,
      chunkSizeWarningLimit: 900,
      // The browser build is the Space's studio/ folder; its root is the presentation page.
      outDir: web ? "dist-web/studio" : "dist",
      emptyOutDir: true,
      rollupOptions: {
        // Both builds have both pages: the browser build shows the splash as its loading
        // screen, inside the app's page.
        input: { main: "index.html", splash: "splash.html" } as Record<string, string>,
      },
    },
  };
});
