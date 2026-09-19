/**
 * Inkvec: raster to SVG by minimum description length, compiled to WebAssembly.
 *
 * ```js
 * import { trace } from "@logolabs/inkvec";
 * const svg = await trace(pngBytes, { colors: 16 });
 * ```
 *
 * This entry is the single-threaded build, which runs anywhere WebAssembly does. The
 * threaded build is `@logolabs/inkvec/threads`.
 *
 * @module
 */

import { load } from "./load.js";
import { createTracer, type ImageInput, type RGBAImage, type WasmSource } from "./common.js";
import type { Options } from "./options.generated.js";
import { VERSION } from "./version.generated.js";

export { InkvecError } from "./common.js";
export type { ImageInput, InkvecErrorCode, RGBAImage, WasmSource } from "./common.js";
export type { Options } from "./options.generated.js";

const tracer = createTracer(load);

/**
 * Load the WebAssembly now instead of on the first trace.
 *
 * Optional: every other call loads it on first use. Call it early to take the download
 * off the first trace, or to say where the module is when the default location does not
 * work -- a CDN, a bundler that emits assets elsewhere, bytes you already hold:
 *
 * ```js
 * await init(new URL("/assets/inkvec.wasm", location.href));
 * ```
 *
 * The first call wins; later calls resolve once that load has finished. A load that fails
 * rejects with an `InkvecError` and is forgotten, so the next call tries again.
 *
 * @param source Where the `.wasm` comes from. Defaults to the file shipped in the
 *   package (`@logolabs/inkvec/inkvec.wasm`), fetched in browsers and read from disk in
 *   Node.js.
 */
export function init(source?: WasmSource): Promise<void> {
  return tracer.init(source);
}

/**
 * Trace an encoded image -- PNG, JPEG, WebP, GIF, BMP or TIFF -- to an SVG document.
 *
 * The work is synchronous inside the WebAssembly: seconds for a large logo. Call it from a
 * Web Worker in a browser, or a `worker_threads` Worker in a Node.js server, to keep the
 * thread you care about responsive.
 *
 * @param input The file's bytes: `Uint8Array` (a Node.js `Buffer` is one), `ArrayBuffer`,
 *   any typed array, or a `Blob`/`File`.
 * @param options Tracer options; anything left out takes the tracer's default. Unknown
 *   keys and out-of-range values are rejected by the tracer with an `InkvecError`.
 * @returns The SVG document as a string.
 */
export function trace(input: ImageInput, options?: Options): Promise<string> {
  return tracer.trace(input, options);
}

/**
 * Trace decoded pixels -- a canvas's `ImageData`, or any straight RGBA8 buffer.
 *
 * ```js
 * const img = ctx.getImageData(0, 0, w, h);
 * const svg = await traceRGBA(img, { colors: 16 });
 * // or: await traceRGBA(img.data, img.width, img.height, { colors: 16 });
 * ```
 *
 * Pixels decoded from a PNG trace to the same bytes as the PNG itself. Raw pixels carry
 * no container, so a JPEG's pixels are not treated as lossily compressed; pass the JPEG
 * file to {@link trace} to keep that.
 */
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

/**
 * Every option with the value the tracer uses when it is left out, read from the tracer
 * itself.
 */
export function defaults(): Promise<Required<Options>> {
  return tracer.defaults();
}

/**
 * The JSON Schema of the options object, generated from the tracer's Rust definition: the
 * same document the TypeScript types in this package were generated from.
 */
export function optionsSchema(): Promise<Record<string, unknown>> {
  return tracer.optionsSchema();
}

/** The tracer's version. Matches the Rust crates and the command-line tool. */
export function version(): string {
  return VERSION;
}
