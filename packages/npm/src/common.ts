// The part of the package that is the same everywhere: turning what the caller holds into
// bytes, loading the WebAssembly once, and handing both to the tracer.
//
// Nothing here knows an option by name. `Options` is generated from the Rust options
// schema (`src/options.generated.ts`), and the object the caller passes goes to Rust as
// JSON, where defaults are applied and every value is checked. An unknown key, a value of
// the wrong type or out of range, comes back as an `InkvecError` carrying Rust's message.

import type { Options } from "./options.generated.js";

/**
 * Where the WebAssembly comes from, for `init`: a URL (or a string holding
 * one), a `Response`, the module's bytes, an already compiled `WebAssembly.Module`, or a
 * promise of any of these. In Node.js, `file:` URLs and plain file paths are read from
 * disk.
 */
export type WasmSource =
  | string
  | URL
  | Request
  | Response
  | ArrayBuffer
  | ArrayBufferView
  | WebAssembly.Module
  | Promise<string | URL | Request | Response | ArrayBuffer | ArrayBufferView | WebAssembly.Module>;

/**
 * An encoded image: PNG, JPEG, WebP, GIF, BMP or TIFF. A Node.js `Buffer` is a
 * `Uint8Array` and is accepted as it is; a `Blob` or `File` is read with `arrayBuffer()`.
 */
export type ImageInput = Uint8Array | ArrayBuffer | ArrayBufferView | Blob;

/**
 * Decoded pixels: straight (not premultiplied) RGBA, 8 bits a channel, row-major, `width *
 * height * 4` bytes. This is exactly `ImageData`, from a canvas's `getImageData()`.
 */
export interface RGBAImage {
  /** `width * height * 4` bytes of straight RGBA. */
  data: Uint8ClampedArray | Uint8Array;
  /** Width in pixels. */
  width: number;
  /** Height in pixels. */
  height: number;
}

/**
 * What went wrong, as a stable string:
 *
 * - `"invalid_image"`: the input is not an image the tracer can decode, is not bytes at
 *   all, or raw pixels do not match the dimensions given;
 * - `"invalid_options"`: an option is unknown, of the wrong type or out of range;
 * - `"internal"`: the tracer itself failed -- worth a bug report;
 * - `"load_failed"`: the WebAssembly could not be fetched or instantiated.
 *
 * The first three are the Rust library's error kinds, shared by every Inkvec binding.
 */
export type InkvecErrorCode = "invalid_image" | "invalid_options" | "internal" | "load_failed";

/** Every error the package reports. `message` is the tracer's own wording. */
export class InkvecError extends Error {
  /** The kind of failure; see {@link InkvecErrorCode}. */
  readonly code: InkvecErrorCode;

  constructor(code: InkvecErrorCode, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "InkvecError";
    this.code = code;
  }
}

/** The generated bindings, as both WebAssembly builds export them. */
export interface Glue {
  trace_json(bytes: Uint8Array, options: string): string;
  trace_rgba_json(pixels: Uint8Array, width: number, height: number, options: string): string;
  default_options_json(): string;
  options_schema_json(): string;
  version(): string;
  threads_available(): boolean;
}

/** Instantiates a build's WebAssembly and resolves to its bindings. */
export type Loader = (source?: WasmSource) => Promise<Glue>;

/** The package's surface, bound to one build of the tracer. */
export interface Tracer {
  init(source?: WasmSource): Promise<void>;
  trace(input: ImageInput, options?: Options): Promise<string>;
  traceRGBA(
    pixels: Uint8ClampedArray | Uint8Array | RGBAImage,
    widthOrOptions?: number | Options,
    height?: number,
    options?: Options,
  ): Promise<string>;
  defaults(): Promise<Required<Options>>;
  optionsSchema(): Promise<Record<string, unknown>>;
  loaded(): Glue | undefined;
}

/**
 * A tracer over one build: the first call loads the WebAssembly, later calls reuse it. A
 * failed load is forgotten, so the next call tries again.
 */
export function createTracer(load: Loader): Tracer {
  let pending: Promise<Glue> | undefined;
  let glue: Glue | undefined;

  const ready = (source?: WasmSource): Promise<Glue> => {
    if (!pending) {
      pending = load(source).then(
        (g) => (glue = g),
        (e: unknown) => {
          pending = undefined;
          throw e instanceof InkvecError
            ? e
            : new InkvecError("load_failed", `could not load the inkvec WebAssembly: ${message(e)}`, {
                cause: e,
              });
        },
      );
    }
    return pending;
  };

  return {
    async init(source) {
      await ready(source);
    },

    async trace(input, options) {
      const bytes = await toBytes(input);
      const json = optionsJson(options);
      const g = await ready();
      return call(() => g.trace_json(bytes, json));
    },

    async traceRGBA(pixels, widthOrOptions, height, options) {
      let data: Uint8Array;
      let w: unknown;
      let h: unknown;
      let opts: Options | undefined;
      if (isRGBAImage(pixels)) {
        data = asBytes(pixels.data, "data");
        w = pixels.width;
        h = pixels.height;
        opts = widthOrOptions as Options | undefined;
      } else {
        data = asBytes(pixels, "pixels");
        w = widthOrOptions;
        h = height;
        opts = options;
      }
      checkDimension(w, "width");
      checkDimension(h, "height");
      const json = optionsJson(opts);
      const g = await ready();
      return call(() => g.trace_rgba_json(data, w, h, json));
    },

    async defaults() {
      const g = await ready();
      return JSON.parse(call(() => g.default_options_json()));
    },

    async optionsSchema() {
      const g = await ready();
      return JSON.parse(call(() => g.options_schema_json()));
    },

    loaded() {
      return glue;
    },
  };
}

/** The options object as the JSON Rust reads. Only its shape is checked here. */
function optionsJson(options: unknown): string {
  if (options === undefined || options === null) return "{}";
  if (typeof options !== "object" || Array.isArray(options)) {
    throw new InkvecError("invalid_options", `options must be an object, got ${describe(options)}`);
  }
  return JSON.stringify(options);
}

/**
 * Run a synchronous binding, turning whatever it throws into an `InkvecError`. The bindings
 * throw `Error`s whose `code` is the Rust error kind; anything else -- a trap, a panic
 * message -- is `internal`.
 */
function call<T>(f: () => T): T {
  try {
    return f();
  } catch (e) {
    const code = (e as { code?: unknown } | null)?.code;
    throw new InkvecError(
      code === "invalid_image" || code === "invalid_options" ? code : "internal",
      message(e),
      { cause: e },
    );
  }
}

async function toBytes(input: unknown): Promise<Uint8Array> {
  if (input instanceof Uint8Array) return input;
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (typeof SharedArrayBuffer !== "undefined" && input instanceof SharedArrayBuffer) {
    return new Uint8Array(input);
  }
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  if (typeof Blob !== "undefined" && input instanceof Blob) {
    return new Uint8Array(await input.arrayBuffer());
  }
  if (typeof input === "string") {
    throw new InkvecError(
      "invalid_image",
      "trace takes the image's bytes, not a path or URL; read the file first " +
        "(fs.readFile in Node.js, fetch(...).then(r => r.arrayBuffer()) in a browser)",
    );
  }
  throw new InkvecError(
    "invalid_image",
    `trace takes a Uint8Array, ArrayBuffer, typed array or Blob, got ${describe(input)}`,
  );
}

function asBytes(pixels: unknown, what: string): Uint8Array {
  if (pixels instanceof Uint8Array) return pixels;
  if (pixels instanceof Uint8ClampedArray) {
    return new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength);
  }
  throw new InkvecError(
    "invalid_image",
    `traceRGBA ${what} must be a Uint8ClampedArray or Uint8Array of RGBA bytes, got ${describe(pixels)}`,
  );
}

function isRGBAImage(x: unknown): x is RGBAImage {
  return (
    typeof x === "object" &&
    x !== null &&
    !ArrayBuffer.isView(x) &&
    "data" in x &&
    "width" in x &&
    "height" in x
  );
}

function checkDimension(v: unknown, what: string): asserts v is number {
  if (typeof v !== "number" || !Number.isInteger(v) || v <= 0 || v > 0xffffffff) {
    throw new InkvecError(
      "invalid_image",
      `traceRGBA ${what} must be a positive integer, got ${describe(v)}`,
    );
  }
}

function describe(x: unknown): string {
  if (x === null) return "null";
  if (Array.isArray(x)) return "an array";
  if (typeof x === "object") return (x as object).constructor?.name ?? "an object";
  return `${typeof x} ${String(x)}`;
}

function message(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
