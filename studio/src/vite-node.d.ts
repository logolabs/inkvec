// The two Node functions `vite.config.ts` uses, declared rather than pulling in @types/node
// for them (as `process` is declared there): the browser build reads its loading screen's
// files and hashes its one inline script for the content policy.
declare module "node:fs" {
  export function readFileSync(path: URL | string, encoding: "utf8"): string;
}
declare module "node:crypto" {
  interface Hash {
    update(data: string): Hash;
    digest(encoding: "base64" | "hex"): string;
  }
  export function createHash(algorithm: "sha256"): Hash;
}
