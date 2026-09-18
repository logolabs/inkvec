/* tslint:disable */
/* eslint-disable */

/**
 * How many workers the pool should start, from the browser's own estimate.
 *
 * Returned rather than decided here: the pool is started from JavaScript, and only the
 * page knows whether it is allowed to (`crossOriginIsolated`) and how many cores the
 * visitor has agreed to give it.
 */
export function threads_available(): boolean;

/**
 * Trace image bytes to an SVG string.
 *
 * `precision`, `min_area`, `colors`, `merge` are the tracer's quality knobs (pass the
 * defaults 0.1, 2, 64, 0.055 when unsure); `max_dim` and `time_budget` bound the work;
 * `no_background`, `minify`, `margin`, `content_units` shape the output.
 */
export function trace(bytes: Uint8Array, precision: number, min_area: number, colors: number, merge: number, max_dim: number, time_budget: number, no_background: boolean, minify: boolean, margin: number, content_units: boolean): string;

/**
 * The tracer's version, for the page footer.
 */
export function version(): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly threads_available: () => number;
    readonly trace: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => [number, number, number, number];
    readonly version: () => [number, number];
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
