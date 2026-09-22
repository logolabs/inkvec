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
