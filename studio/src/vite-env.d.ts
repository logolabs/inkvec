/// <reference types="vite/client" />

// `?raw` gives the file's text. One asset uses it: the mark, generated from
// `web/logo.svg` so the app cannot draw a version of it the rest of the project has
// retired. Declared here because `tsc` does not read Vite's resolver.
declare module "*.svg?raw" {
  const contents: string;
  export default contents;
}

/** The app's version, from package.json, substituted at build time by `vite.config.ts`. */
declare const __APP_VERSION__: string;

/**
 * True in the browser build, Inkvec Studio Lite (`vite build --mode web`), false in the
 * desktop app. A constant, so the other build's code is dropped from each bundle.
 */
declare const __INKVEC_WEB__: boolean;

/** The browser build's cache token for its WebAssembly, so a new build is never served stale. */
declare const __INKVEC_WASM_TOKEN__: string;

/**
 * The browser build's two WebAssembly modules' sizes in bytes (`pkg`, `pkg-threads`), for the
 * loading screen's bar; empty in development. Written by `scripts/build-web.mjs`.
 */
declare const __INKVEC_WASM_BYTES__: Record<string, number>;
