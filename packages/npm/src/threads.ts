/**
 * The threaded build of Inkvec: the same tracer and byte-identical output, with its
 * parallel stages spread over a pool of workers.
 *
 * ```js
 * import { init, trace } from "@logolabs/inkvec/threads";
 * await init();               // starts the pool
 * const svg = await trace(bytes);
 * ```
 *
 * In a browser it needs a cross-origin isolated page (`Cross-Origin-Opener-Policy:
 * same-origin`, `Cross-Origin-Embedder-Policy: require-corp`) and must run inside a Web
 * Worker, because the calling thread waits while the pool works. In Node.js the pool is
 * `worker_threads`. Everything else is as in the main entry, `@logolabs/inkvec`.
 *
 * @module
 */

import { defaultThreads, load } from "./load-threads.js";
import { createTracer, type ImageInput, type RGBAImage, type WasmSource } from "./common.js";
import type { Options } from "./options.generated.js";
import { VERSION } from "./version.generated.js";

export { InkvecError } from "./common.js";
export type { ImageInput, InkvecErrorCode, RGBAImage, WasmSource } from "./common.js";
export type { Options } from "./options.generated.js";

let requested: number | undefined;
let started = 0;

const tracer = createTracer(async (source) => {
  const n = requested ?? defaultThreads();
  const glue = await load(source, n);
  started = n;
  return glue;
});

/**
 * Load the threaded WebAssembly and start its pool. Optional -- the first trace does it
 * with the defaults -- but it is where the pool size is chosen, and it rejects with a
 * clear `InkvecError` where threads cannot run (a page that is not cross-origin
 * isolated, a browser main thread).
 *
 * A pool is started once per process or worker; later calls resolve once the first has
 * finished and ignore their arguments.
 *
 * @param source Where the `.wasm` comes from; defaults to the threaded module shipped in
 *   the package (`@logolabs/inkvec/threads/inkvec.wasm`). See the main entry's `init`.
 * @param threads Pool size. Defaults to one per core, at most 16 (in a browser, one less,
 *   for the page).
 */
export function init(source?: WasmSource, threads?: number): Promise<void> {
  if (threads !== undefined && (!Number.isInteger(threads) || threads < 1)) {
    return Promise.reject(new RangeError(`threads must be a positive integer, got ${threads}`));
  }
  if (!tracer.loaded()) requested = threads;
  return tracer.init(source);
}

/** The pool's size once started, else 0. */
export function threadCount(): number {
  return started;
}

/** As `trace` in `@logolabs/inkvec`, on the pool. */
export function trace(input: ImageInput, options?: Options): Promise<string> {
  return tracer.trace(input, options);
}

/** As `traceRGBA` in `@logolabs/inkvec`, on the pool. */
export function traceRGBA(image: RGBAImage, options?: Options): Promise<string>;
export function traceRGBA(
  pixels: Uint8ClampedArray | Uint8Array,
  width: number,
  height: number,
  options?: Options,
): Promise<string>;
export function traceRGBA(
  pixels: Uint8ClampedArray | Uint8Array | RGBAImage,
  widthOrOptions?: number | Options,
  height?: number,
  options?: Options,
): Promise<string> {
  return tracer.traceRGBA(pixels, widthOrOptions, height, options);
}

/** As `defaults` in `@logolabs/inkvec`. */
export function defaults(): Promise<Required<Options>> {
  return tracer.defaults();
}

/** As `optionsSchema` in `@logolabs/inkvec`. */
export function optionsSchema(): Promise<Record<string, unknown>> {
  return tracer.optionsSchema();
}

/** The tracer's version. */
export function version(): string {
  return VERSION;
}
