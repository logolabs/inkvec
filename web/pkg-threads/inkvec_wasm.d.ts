/* tslint:disable */
/* eslint-disable */

/**
 * A decoded raster, prepared for tracing and held across a denoiser round trip.
 *
 * [`prepare`] makes one, [`Intake::trace`] finishes the job, and in between the denoiser
 * exports hand the tensor out to the page's ONNX Runtime Web session and take its answer
 * back. With no denoising in between, `prepare(...).trace()` is exactly what [`trace`] does
 * — the same call, because [`trace`] *is* that pair.
 *
 * JavaScript owns it and must release it: `free()` after the last [`Intake::trace`], or
 * [`Intake::trace_once`] instead of that last trace, which consumes the intake and leaves
 * nothing to free. Not both -- `free()` on an intake `traceOnce` already took is a free of
 * a null pointer.
 */
export class Intake {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The tensor the denoiser network takes: composited onto white, planar, edge-padded.
     *
     * This is `inkvec_restore::network_input`, the same preparation the command line's
     * in-process backends do, so the browser is not a second recipe — only a second
     * runtime for the same `restorer.onnx`.
     */
    denoiser_input(): Float32Array;
    /**
     * How far this raster disagrees with `svg` where the trace claims a flat interior —
     * `inkvec_restore::decide`'s signal, and the one `--restore auto` decides on. Above
     * [`denoiser_threshold`] the input is treated as damaged. `undefined` when the SVG
     * could not be rendered or has no flat interior to measure.
     */
    residual(svg: string): number | undefined;
    /**
     * The raster as RGBA8, for showing the page what the denoiser did.
     */
    rgba8(): Uint8Array;
    /**
     * Take the network's output — the same `1 x 3 x height x width` planar layout — as the
     * raster to trace: cropped back to size, quantised to 256 levels, extremes snapped,
     * alpha carried through, all of it `inkvec_restore::network_output`.
     *
     * The trace that follows is then forced onto soft intake, as `--restore` forces it: a
     * denoised raster can look clean enough that the edge-width and ringing detectors no
     * longer open soft intake by themselves, and the network was validated with it open.
     */
    take_denoiser_output(chw: Float32Array): void;
    /**
     * Trace it, consuming the intake. `trace` when the answer is only needed once.
     */
    traceOnce(): string;
    /**
     * Trace it, without consuming: `auto` needs one trace before it can decide whether to
     * denoise, and keeps that trace when the input turns out to be undamaged.
     *
     * The clone this takes is the raster only. It is the price of asking for the same trace
     * twice, and it is small next to the trace.
     */
    trace(): string;
    /**
     * The size of the tensor [`Intake::denoiser_input`] returns, as `[width, height]`: the
     * raster's size rounded up to a multiple of 16, which is what the network's four 2x
     * downsamplings require. The tensor itself is `1 x 3 x height x width`, planar.
     */
    readonly denoiser_input_size: Uint32Array;
    /**
     * The height the tracer will see.
     */
    readonly height: number;
    /**
     * The width the tracer will see, after `max_dim` and any intake normalisation. This is
     * the size the denoiser runs at, which is the point of denoising here rather than
     * before the decode: the network must see the raster the tracer sees.
     */
    readonly width: number;
}

/**
 * The build target the contract fixtures key their SVG hashes by (`inkvec::build_target`):
 * `wasm32-unknown` for both builds.
 */
export function build_target(): string;

/**
 * Every option at its default, as a JSON object.
 */
export function default_options_json(): string;

/**
 * The SHA-256 the weights must hash to, so a page can refuse a truncated or tampered
 * download the way `inkvec_restore::pull_onnx_weights` refuses one.
 */
export function denoiser_model_sha256(): string;

/**
 * Where the denoiser weights come from: the same `restorer.onnx` the command line pulls,
 * from the same Hugging Face repository.
 */
export function denoiser_model_url(): string;

/**
 * The interior residual above which `auto` denoises, `inkvec_restore::Options`' default.
 */
export function denoiser_threshold(): number;

export function initThreadPool(num_threads: number): Promise<any>;

/**
 * The JSON Schema of the options: byte for byte `bindings/options.schema.json`.
 */
export function options_schema_json(): string;

/**
 * Decode an image and run the pipeline up to the point the denoiser would see it, for a
 * caller that wants to denoise it there. The arguments are [`trace`]'s.
 *
 * `trace(bytes, options)` is `prepare(bytes, options).traceOnce()`; the page calls this one
 * instead when it has a denoiser to run in between.
 */
export function prepare(bytes: Uint8Array, options: string): Intake;

/**
 * How editable an SVG is, as a JSON object of counts: `nodes`, `cubics`, `handles`,
 * `axisHandles`, `joins`, `smoothJoins`, `alignedNodes`. `inkvec_svgmin::structure`, the
 * same measurement the desktop app's editability card reports.
 */
export function structure_json(svg: string): string;

/**
 * Whether this is the threaded build, whose pool `initThreadPool` starts.
 *
 * How many workers to start is left to JavaScript: only the page knows whether it is
 * allowed to (`crossOriginIsolated`) and how many cores the visitor has agreed to give it.
 */
export function threads_available(): boolean;

/**
 * Trace image bytes to an SVG string, with the options as a JSON object --
 * [`trace_json`]'s options, and the same defaults for whatever is missing. The
 * difference is the route: this is `prepare(bytes, options).traceOnce()`, the pair the
 * page uses when it has a denoiser to run in between, so a page that never denoises and a
 * page that does trace the same way.
 */
export function trace(bytes: Uint8Array, options: string): string;

/**
 * Trace an encoded image (PNG, JPEG, WebP, GIF, BMP or TIFF) to an SVG string, with the
 * options as a JSON object -- `inkvec::trace` with `inkvec::Options::from_json(options)`.
 * Missing keys take their defaults; `""` and `"{}"` mean all defaults.
 */
export function trace_json(bytes: Uint8Array, options: string): string;

/**
 * Trace raw pixels -- straight RGBA8, row-major, exactly `width * height * 4` bytes, as in
 * `ImageData.data` -- to an SVG string. `inkvec::trace_rgba`; the options as for
 * [`trace_json`]. Byte-identical to [`trace_json`] on a PNG of the same pixels.
 */
export function trace_rgba_json(pixels: Uint8Array, width: number, height: number, options: string): string;

/**
 * The tracer's version, for the page footer.
 */
export function version(): string;

export class wbg_rayon_PoolBuilder {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    build(): void;
    numThreads(): number;
    receiver(): number;
}

export function wbg_rayon_start_worker(receiver: number): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly __wbg_intake_free: (a: number, b: number) => void;
    readonly __wbg_wbg_rayon_poolbuilder_free: (a: number, b: number) => void;
    readonly build_target: () => [number, number];
    readonly default_options_json: () => [number, number];
    readonly denoiser_model_sha256: () => [number, number];
    readonly denoiser_model_url: () => [number, number];
    readonly denoiser_threshold: () => number;
    readonly initThreadPool: (a: number) => any;
    readonly intake_denoiser_input: (a: number) => [number, number];
    readonly intake_denoiser_input_size: (a: number) => [number, number];
    readonly intake_height: (a: number) => number;
    readonly intake_residual: (a: number, b: number, c: number) => [number, number];
    readonly intake_rgba8: (a: number) => [number, number];
    readonly intake_take_denoiser_output: (a: number, b: number, c: number) => [number, number];
    readonly intake_trace: (a: number) => [number, number, number, number];
    readonly intake_traceOnce: (a: number) => [number, number, number, number];
    readonly intake_width: (a: number) => number;
    readonly options_schema_json: () => [number, number];
    readonly prepare: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly structure_json: (a: number, b: number) => [number, number];
    readonly threads_available: () => number;
    readonly trace: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly trace_json: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly trace_rgba_json: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number, number, number];
    readonly version: () => [number, number];
    readonly wbg_rayon_poolbuilder_build: (a: number) => void;
    readonly wbg_rayon_poolbuilder_numThreads: (a: number) => number;
    readonly wbg_rayon_poolbuilder_receiver: (a: number) => number;
    readonly wbg_rayon_start_worker: (a: number) => void;
    readonly memory: WebAssembly.Memory;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_thread_destroy: (a?: number, b?: number, c?: number) => void;
    readonly __wbindgen_start: (a: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number }} module - Passing `SyncInitInput` directly is deprecated.
 * @param {WebAssembly.Memory} memory - Deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number } | SyncInitInput, memory?: WebAssembly.Memory): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number }} module_or_path - Passing `InitInput` directly is deprecated.
 * @param {WebAssembly.Memory} memory - Deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number } | InitInput | Promise<InitInput>, memory?: WebAssembly.Memory): Promise<InitOutput>;
