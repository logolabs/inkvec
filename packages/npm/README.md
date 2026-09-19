# @logolabs/inkvec

Inkvec turns raster logos, icons and illustrations into compact, accurate SVG. This package
is the tracer compiled to WebAssembly, for browsers, Node.js, Deno and Bun: a PNG, JPEG,
WebP, GIF, BMP or TIFF (or raw RGBA pixels) in, an SVG string out. It runs the same
pipeline as the `inkvec` command line with the same defaults. Output is byte-identical
across runtimes and between this package's two builds; it can differ from a native build
in small details, because WebAssembly takes its floating-point functions from a different
math library.

- A boundary sits where the anti-aliasing says it is, to a fraction of a pixel.
- A circle is written as `<circle>`, a smooth ramp as a real gradient.
- The number of coordinates is chosen by minimum description length, not by a tolerance
  you have to guess.

Project page, method and benchmarks: <https://github.com/logolabs/inkvec>. Try it in the
browser: <https://huggingface.co/spaces/logolabs/inkvec>.

## Install

```sh
npm install @logolabs/inkvec
```

ES modules only. Node.js 20.16 or later (`require()` of the package works where Node.js
supports requiring ES modules: 20.19+, 22.12+).

## Use

### Node.js

```js
import { readFile, writeFile } from "node:fs/promises";
import { trace } from "@logolabs/inkvec";

const svg = await trace(await readFile("logo.png"), { colors: 16 });
await writeFile("logo.svg", svg);
```

### Browser

A trace is seconds of synchronous work for a large logo, so run it in a Web Worker:

```js
// worker.js -- new Worker(new URL("./worker.js", import.meta.url), { type: "module" })
import { trace } from "@logolabs/inkvec";

onmessage = async ({ data: file }) => {
  // `file` is a File or Blob, e.g. from <input type="file">
  postMessage(await trace(file, { cutout: true }));
};
```

From a canvas, trace the pixels directly:

```js
import { traceRGBA } from "@logolabs/inkvec";

const image = ctx.getImageData(0, 0, canvas.width, canvas.height);
const svg = await traceRGBA(image);
```

### Bundlers and CDNs

The package finds its `.wasm` with `new URL("…", import.meta.url)`, which Vite, webpack 5,
Rollup, esbuild (with an asset loader) and Parcel turn into an emitted asset, so no setup is
needed. Where the file ends up somewhere else -- a CDN, a custom asset pipeline -- say where
before the first trace:

```js
import { init } from "@logolabs/inkvec";
import wasmUrl from "@logolabs/inkvec/inkvec.wasm?url"; // Vite

await init(wasmUrl);
```

`init` also takes the module's bytes or a compiled `WebAssembly.Module` (Cloudflare
Workers: `import wasm from "@logolabs/inkvec/inkvec.wasm"; await init(wasm);`).

Without a bundler the files load as they are, e.g.
`import { trace } from "https://cdn.jsdelivr.net/npm/@logolabs/inkvec/dist/index.js"`.

## API

```ts
function trace(input: Uint8Array | ArrayBuffer | ArrayBufferView | Blob, options?: Options): Promise<string>;
function traceRGBA(image: { data: Uint8ClampedArray | Uint8Array; width: number; height: number }, options?: Options): Promise<string>;
function traceRGBA(pixels: Uint8ClampedArray | Uint8Array, width: number, height: number, options?: Options): Promise<string>;
function init(source?: string | URL | Request | Response | BufferSource | WebAssembly.Module): Promise<void>;
function defaults(): Promise<Required<Options>>;
function optionsSchema(): Promise<object>;
function version(): string;
class InkvecError extends Error { code: "invalid_image" | "invalid_options" | "internal" | "load_failed" }
```

- **`trace`** traces an encoded image. A Node.js `Buffer` is a `Uint8Array`.
- **`traceRGBA`** traces decoded pixels: straight (not premultiplied) RGBA, 8 bits a
  channel, `width * height * 4` bytes -- an `ImageData`. The pixels of a PNG trace to the
  same bytes as the PNG. Raw pixels have no container, so a JPEG's pixels are not treated
  as lossily compressed; pass the JPEG file to `trace` to keep that.
- **`init`** loads the WebAssembly now, optionally from a source of your choosing. It is
  optional: the first trace loads it. The first load wins; a failed load can be retried.
- **`defaults`** and **`optionsSchema`** read every option's default and the options' JSON
  Schema from the tracer itself.
- Every failure is an `InkvecError`. `code` is the kind: `invalid_image` (not an image the
  tracer can decode, not bytes, or pixels that do not match the size given),
  `invalid_options` (an unknown option, a wrong type or a value out of range),
  `internal` (the tracer failed; please report it) or `load_failed`.

The output is deterministic: the same input and options give the same bytes, in every
runtime and on both builds, as long as `time_budget` is 0 (the default). The cross-language
contract fixtures (`bindings/contract/` in the repository) pin those bytes.

## Options

Every option is optional. Names and meanings are the Rust library's (`inkvec::Options`);
the table and the TypeScript types are generated from its schema, and the tracer checks
every value -- an unknown name is an error, not a silent no-op.

<!-- inkvec:options:begin -->
| Option | Type | Default | Range | Meaning |
|---|---|---|---|---|
| `precision` | number | `0.1` | > 0 | Sets the description-length cost of a coordinate, in pixels: lambda = ln(extent / precision). Smaller values buy more detail with more coordinates. It does not set the digits written; coordinates are always written at 2 decimals. |
| `min_area` | number | `2.0` | > 0 | Discard features smaller than this area, in square pixels. |
| `colors` | integer | `64` | >= 1 and <= 4096 | Maximum palette size. |
| `merge` | number | `0.035` | >= 0 | OKLab distance below which two colours are treated as one ink. |
| `max_dim` | integer | `2048` | - | Inputs larger than this on their longer side, in pixels, are traced at this size and the SVG is written at the original size. Trace time grows with the pixel count. 0 means no cap. |
| `time_budget` | number | `0.0` | >= 0 | Advisory wall-clock budget, in seconds; 0 means none. Gradient-band merging stops at 60% of it and the boundary solve gets 25%; the output is still a correct trace, with more fills or a less polished outline. A nonzero budget makes the output depend on machine speed and load, so it is no longer reproducible. |
| `margin` | number | `0.0` | >= 0 | Transparent margin around the output, as a fraction of the larger side. The viewBox grows; the geometry does not move. |
| `no_background` | bool | `false` | - | Knock the background out: the face that covers the whole canvas is not painted, so the artwork sits on transparency. |
| `minify` | bool | `false` | - | No ids or groups, no trailing zeros. Same geometry, typically about a tenth smaller. |
| `cutout` | bool | `false` | - | Carry the input's transparency into the SVG: a face the source drew transparent becomes a hole, one drawn at a single opacity keeps it as fill-opacity, and white artwork on a transparent ground survives. Changes nothing for an opaque input. Off by default because over white it opens faint seams along shared edges; use it for artwork that will sit on anything but white. |
| `content_units` | bool | `false` | - | Scale the fit tolerances with the raster, so a large, simple drawing gets the parameter count of a small one. Trades fidelity for parsimony: small squares can come back as circles and thin rings broken. |
| `harmonize` | bool | `true` | - | Shape harmonization (on by default): marks that repeat across the drawing are redrawn from one consensus geometry per cluster, which saves parameters. The known cost is fidelity on fine-line art: on hairlines, thin rings and small rounded details the consensus can displace thin lines by about a pixel (on the 246-icon screen set mean dE00 0.151 off vs 0.299 on). Set it to false for such artwork. |
| `harmonize_threshold` | number | `0.92` | >= 0 and <= 1 | Shape-equivalence threshold for harmonization: the outline similarity (IoU after affine normalisation) above which two marks count as the same shape. |
<!-- inkvec:options:end -->

### Transparency

For artwork that will sit on anything but white, use `cutout: true`: a region the source
drew transparent becomes a hole instead of a patch of the matte colour, a region drawn at
one opacity keeps it as `fill-opacity`, and white artwork on a transparent ground survives.
It changes nothing for an opaque image. `no_background: true` additionally leaves out the
region that covers the whole canvas, so an opaque logo on a flat background comes back on
transparency.

### Shape harmonization

On by default, as on the command line. Marks that repeat across a drawing -- a run of
identical tabs, segmented rings, tiled glyphs -- are redrawn from one consensus shape per
cluster, which saves parameters. The cost is fidelity on fine-line art: on hairlines, thin
rings and small rounded details the consensus can move a thin line by about a pixel (on
the project's 246-icon screen set, mean colour error dE00 0.151 with it off, 0.299 with it
on). For such artwork pass `harmonize: false`.

## Threads

`@logolabs/inkvec/threads` is the same tracer built with WebAssembly threads. It produces
byte-identical output and spreads the parallel stages over a pool of workers.

```js
import { init, trace, threadCount } from "@logolabs/inkvec/threads";

await init();          // or init(undefined, 8) for a pool of 8
const svg = await trace(bytes);
```

- **Browsers.** The pool's memory is a `SharedArrayBuffer`, which a browser only gives to a
  cross-origin isolated page. Serve the page with

  ```
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  ```

  and check `crossOriginIsolated` before choosing this entry. It must also run inside a Web
  Worker: the calling thread waits while the pool works, which a browser's main thread may
  not do. Where either is not possible, use `@logolabs/inkvec`; `init` rejects with an
  `InkvecError` that says which condition failed.
- **Node.js.** The pool runs on `worker_threads`; nothing extra is needed. The workers do not
  keep the process alive.
- **Deno, Bun.** These use their own Web Worker implementation. The contract cases pass
  on both builds under Deno 2.9 and Bun 1.4.

The default pool is one worker per core, at most 16 (one fewer in a browser).

## Speed

Measured in Node.js 22 on an 8-core Ryzen 7 5800X that was busy with other work (so
upper bounds), default options:

| Input | `@logolabs/inkvec` | `@logolabs/inkvec/threads` (16 workers) |
|---|---|---|
| 96 px icon | 0.8-0.9 s | 1.4-1.7 s |
| 512-768 px logos (the demo's samples) | 8-14 s | 2.8-3.9 s |

The threaded build pays off from a few hundred pixels up; on small icons its coordination
costs more than it saves. The native command line is several times faster than either.

Trace time grows with the pixel count. `max_dim` (default 2048) caps the size that is
traced; the SVG is still written at the input's size. The web demo traces at 1024.

## Limits

- The optional neural pre-passes of the command line -- the trained restorer (`--restore`)
  and the super-resolution pre-pass (`--sr`) -- are not in this package: they need model
  weights and an ML runtime. For heavily compressed input, clean it first or use the
  command line.
- A trace runs synchronously inside the WebAssembly and cannot be cancelled; `time_budget`
  bounds it (and makes the output depend on machine speed).
- WebAssembly memory is 32-bit: at most 4 GiB per instance. `max_dim` keeps typical inputs
  far below that.
- Built for graphic artwork -- logos, icons, illustrations, diagrams. Photographs trace to a
  large number of flat regions.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
