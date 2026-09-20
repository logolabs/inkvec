/* @ts-self-types="./inkvec_wasm.d.ts" */

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
    static __wrap(ptr) {
        const obj = Object.create(Intake.prototype);
        obj.__wbg_ptr = ptr;
        IntakeFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        IntakeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_intake_free(ptr, 0);
    }
    /**
     * The tensor the denoiser network takes: composited onto white, planar, edge-padded.
     *
     * This is `inkvec_restore::network_input`, the same preparation the command line's
     * in-process backends do, so the browser is not a second recipe — only a second
     * runtime for the same `restorer.onnx`.
     * @returns {Float32Array}
     */
    denoiser_input() {
        const ret = wasm.intake_denoiser_input(this.__wbg_ptr);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * The size of the tensor [`Intake::denoiser_input`] returns, as `[width, height]`: the
     * raster's size rounded up to a multiple of 16, which is what the network's four 2x
     * downsamplings require. The tensor itself is `1 x 3 x height x width`, planar.
     * @returns {Uint32Array}
     */
    get denoiser_input_size() {
        const ret = wasm.intake_denoiser_input_size(this.__wbg_ptr);
        var v1 = getArrayU32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * The height the tracer will see.
     * @returns {number}
     */
    get height() {
        const ret = wasm.intake_height(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * How far this raster disagrees with `svg` where the trace claims a flat interior —
     * `inkvec_restore::decide`'s signal, and the one `--restore auto` decides on. Above
     * [`denoiser_threshold`] the input is treated as damaged. `undefined` when the SVG
     * could not be rendered or has no flat interior to measure.
     * @param {string} svg
     * @returns {number | undefined}
     */
    residual(svg) {
        const ptr0 = passStringToWasm0(svg, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.intake_residual(this.__wbg_ptr, ptr0, len0);
        return ret[0] === 0 ? undefined : ret[1];
    }
    /**
     * The raster as RGBA8, for showing the page what the denoiser did.
     * @returns {Uint8Array}
     */
    rgba8() {
        const ret = wasm.intake_rgba8(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Take the network's output — the same `1 x 3 x height x width` planar layout — as the
     * raster to trace: cropped back to size, quantised to 256 levels, extremes snapped,
     * alpha carried through, all of it `inkvec_restore::network_output`.
     *
     * The trace that follows is then forced onto soft intake, as `--restore` forces it: a
     * denoised raster can look clean enough that the edge-width and ringing detectors no
     * longer open soft intake by themselves, and the network was validated with it open.
     * @param {Float32Array} chw
     */
    take_denoiser_output(chw) {
        const ptr0 = passArrayF32ToWasm0(chw, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.intake_take_denoiser_output(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Trace it, consuming the intake. `trace` when the answer is only needed once.
     * @returns {string}
     */
    traceOnce() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr = this.__destroy_into_raw();
            const ret = wasm.intake_traceOnce(ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Trace it, without consuming: `auto` needs one trace before it can decide whether to
     * denoise, and keeps that trace when the input turns out to be undamaged.
     *
     * The clone this takes is the raster only. It is the price of asking for the same trace
     * twice, and it is small next to the trace.
     * @returns {string}
     */
    trace() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.intake_trace(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * The width the tracer will see, after `max_dim` and any intake normalisation. This is
     * the size the denoiser runs at, which is the point of denoising here rather than
     * before the decode: the network must see the raster the tracer sees.
     * @returns {number}
     */
    get width() {
        const ret = wasm.intake_width(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) Intake.prototype[Symbol.dispose] = Intake.prototype.free;

/**
 * The build target the contract fixtures key their SVG hashes by (`inkvec::build_target`):
 * `wasm32-unknown` for both builds.
 * @returns {string}
 */
export function build_target() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.build_target();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Every option at its default, as a JSON object.
 * @returns {string}
 */
export function default_options_json() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.default_options_json();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * The SHA-256 the weights must hash to, so a page can refuse a truncated or tampered
 * download the way `inkvec_restore::pull_onnx_weights` refuses one.
 * @returns {string}
 */
export function denoiser_model_sha256() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.denoiser_model_sha256();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Where the denoiser weights come from: the same `restorer.onnx` the command line pulls,
 * from the same Hugging Face repository.
 * @returns {string}
 */
export function denoiser_model_url() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.denoiser_model_url();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * The interior residual above which `auto` denoises, `inkvec_restore::Options`' default.
 * @returns {number}
 */
export function denoiser_threshold() {
    const ret = wasm.denoiser_threshold();
    return ret;
}

/**
 * The JSON Schema of the options: byte for byte `bindings/options.schema.json`.
 * @returns {string}
 */
export function options_schema_json() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.options_schema_json();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Decode an image and run the pipeline up to the point the denoiser would see it, for a
 * caller that wants to denoise it there. The arguments are [`trace`]'s.
 *
 * `trace(bytes, ...)` is `prepare(bytes, ...).traceOnce()`; the page calls this one instead
 * when it has a denoiser to run in between.
 * @param {Uint8Array} bytes
 * @param {number} precision
 * @param {number} min_area
 * @param {number} colors
 * @param {number} merge
 * @param {number} max_dim
 * @param {number} time_budget
 * @param {boolean} no_background
 * @param {boolean} minify
 * @param {number} margin
 * @param {boolean} content_units
 * @param {boolean} cutout
 * @returns {Intake}
 */
export function prepare(bytes, precision, min_area, colors, merge, max_dim, time_budget, no_background, minify, margin, content_units, cutout) {
    const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.prepare(ptr0, len0, precision, min_area, colors, merge, max_dim, time_budget, no_background, minify, margin, content_units, cutout);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Intake.__wrap(ret[0]);
}

/**
 * Whether this is the threaded build, whose pool `initThreadPool` starts.
 *
 * How many workers to start is left to JavaScript: only the page knows whether it is
 * allowed to (`crossOriginIsolated`) and how many cores the visitor has agreed to give it.
 * @returns {boolean}
 */
export function threads_available() {
    const ret = wasm.threads_available();
    return ret !== 0;
}

/**
 * Trace image bytes to an SVG string.
 *
 * `precision`, `min_area`, `colors`, `merge` are the tracer's quality knobs (pass the
 * defaults 0.1, 2, 64, 0.035 when unsure; the merge default is
 * `inkvec_trace::color::DEFAULT_MERGE_DISTANCE`); `max_dim` and `time_budget` bound the
 * work; `no_background`, `minify`, `margin`, `content_units` shape the output. `max_dim = 0`
 * means no cap. `cutout` is `--cutout`: an input's transparency is carried into the SVG
 * (holes stay holes, a flat wash keeps its opacity); it changes nothing for an opaque
 * input.
 * @param {Uint8Array} bytes
 * @param {number} precision
 * @param {number} min_area
 * @param {number} colors
 * @param {number} merge
 * @param {number} max_dim
 * @param {number} time_budget
 * @param {boolean} no_background
 * @param {boolean} minify
 * @param {number} margin
 * @param {boolean} content_units
 * @param {boolean} cutout
 * @returns {string}
 */
export function trace(bytes, precision, min_area, colors, merge, max_dim, time_budget, no_background, minify, margin, content_units, cutout) {
    let deferred3_0;
    let deferred3_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.trace(ptr0, len0, precision, min_area, colors, merge, max_dim, time_budget, no_background, minify, margin, content_units, cutout);
        var ptr2 = ret[0];
        var len2 = ret[1];
        if (ret[3]) {
            ptr2 = 0; len2 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Trace an encoded image (PNG, JPEG, WebP, GIF, BMP or TIFF) to an SVG string, with the
 * options as a JSON object -- `inkvec::trace` with `inkvec::Options::from_json(options)`.
 * Missing keys take their defaults; `""` and `"{}"` mean all defaults.
 * @param {Uint8Array} bytes
 * @param {string} options
 * @returns {string}
 */
export function trace_json(bytes, options) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(options, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.trace_json(ptr0, len0, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Trace raw pixels -- straight RGBA8, row-major, exactly `width * height * 4` bytes, as in
 * `ImageData.data` -- to an SVG string. `inkvec::trace_rgba`; the options as for
 * [`trace_json`]. Byte-identical to [`trace_json`] on a PNG of the same pixels.
 * @param {Uint8Array} pixels
 * @param {number} width
 * @param {number} height
 * @param {string} options
 * @returns {string}
 */
export function trace_rgba_json(pixels, width, height, options) {
    let deferred4_0;
    let deferred4_1;
    try {
        const ptr0 = passArray8ToWasm0(pixels, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(options, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        const ret = wasm.trace_rgba_json(ptr0, len0, width, height, ptr1, len1);
        var ptr3 = ret[0];
        var len3 = ret[1];
        if (ret[3]) {
            ptr3 = 0; len3 = 0;
            throw takeFromExternrefTable0(ret[2]);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
    }
}

/**
 * The tracer's version, for the page footer.
 * @returns {string}
 */
export function version() {
    let deferred1_0;
    let deferred1_1;
    try {
        const ret = wasm.version();
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
    } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
    }
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_is_undefined_8c687d0b90d5b524: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_throw_5d9e815e6fdf150f: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_error_757e9472f8410341: function(arg0, arg1) {
            let deferred0_0;
            let deferred0_1;
            try {
                deferred0_0 = arg0;
                deferred0_1 = arg1;
                console.error(getStringFromWasm0(arg0, arg1));
            } finally {
                wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
            }
        },
        __wbg_new_227d7c05414eb861: function() {
            const ret = new Error();
            return ret;
        },
        __wbg_new_a32a1ab6c6655abe: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_now_e7c6795a7f81e10f: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_performance_3fcf6e32a7e1ed0a: function(arg0) {
            const ret = arg0.performance;
            return ret;
        },
        __wbg_set_a377297433dfea63: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_stack_3b0d974bbf31e44f: function(arg0, arg1) {
            const ret = arg1.stack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_static_accessor_GLOBAL_8eb4cd83130a11a0: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_1e7044f654e934db: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_d8b50611246a6d92: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_fd0bc376bf0f8b42: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbindgen_generic_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./inkvec_wasm_bg.js": import0,
    };
}

const IntakeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_intake_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('inkvec_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
