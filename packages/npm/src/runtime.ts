// What the package needs to know about the platform it runs on, without importing
// anything platform-specific.
//
// There is one entry for every runtime. Node.js's modules are reached through
// `process.getBuiltinModule` (Node.js 20.16+, 22.3+; Bun and Deno have it too) rather than
// an `import "node:fs"`, so a browser bundler never sees a Node.js import to fail on, and a
// browser loading the files straight from a CDN needs no import map.

import type { WasmSource } from "./common.js";

type NodeProcess = {
  versions?: { node?: string };
  getBuiltinModule?: (id: string) => unknown;
};

function nodeProcess(): NodeProcess | undefined {
  const p = (globalThis as { process?: NodeProcess }).process;
  return p && typeof p.versions?.node === "string" ? p : undefined;
}

/** A Node.js built-in module, where the runtime offers them; else `undefined`. */
export function builtin<T>(id: string): T | undefined {
  return nodeProcess()?.getBuiltinModule?.(id) as T | undefined;
}

type Fs = { promises: { readFile(path: string | URL): Promise<Uint8Array> } };

/**
 * A WebAssembly source as the generated bindings can use it. Where there is a file system
 * (Node.js, Bun, Deno), `file:` URLs and plain paths are read into bytes, since `fetch`
 * does not read `file:` URLs everywhere. Everything else -- http(s) URLs, bytes, a compiled
 * module, a `Response` -- passes through, and the bindings fetch or instantiate it.
 */
export async function resolveSource(source: WasmSource): Promise<unknown> {
  const s = await source;
  const fs = builtin<Fs>("fs");
  if (!fs) return s;
  if (s instanceof URL) return s.protocol === "file:" ? fs.promises.readFile(s) : s;
  if (typeof s === "string" && !/^(https?|data|blob):/i.test(s)) {
    return fs.promises.readFile(s.startsWith("file:") ? new URL(s) : s);
  }
  return s;
}
