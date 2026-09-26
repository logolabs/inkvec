// The trained denoiser (the `--restore` pre-pass), run in the browser.
//
// Inkvec's denoiser is a 19.7-million-parameter U-Net exported to ONNX — `restorer.onnx`,
// the same file the command line's `restore-model` build feeds to ONNX Runtime. Running it
// here is therefore not a port: it is the same graph in a second runtime, ONNX Runtime Web,
// which is the same engine again with WebGPU and WebAssembly kernels in place of the native
// ones. Nothing about the network, its weights or the arithmetic around it is reimplemented
// in JavaScript. Measured against native ONNX Runtime on a 512-px JPEG, the WebAssembly
// kernels agree to 4.2e-7, and after the 8-bit quantisation the tracer reads, one channel of
// one pixel in 786,432 differs by one level.
//
// What is reimplemented here is nothing at all, and that is deliberate: the padding, the
// compositing, the crop, the quantisation and the extreme-snapping all live in
// `inkvec-restore` and reach this file through the WebAssembly tracer's `Intake` object
// (`denoiser_input` / `take_denoiser_output`). This module's whole job is to turn one
// Float32Array into another.
//
// Two things are fetched before the denoiser can run, and kept in the browser's Cache Storage
// afterwards: ONNX Runtime Web's WebAssembly (~26 MB; its two small JavaScript files ride the
// HTTP cache, jsDelivr marks them immutable) and the weights (~76 MB). `prefetch` fetches
// both ahead of need -- Inkvec Studio Lite starts it as soon as its engine has arrived -- and
// `load` fetches whatever is still missing. One download at a time per file however many ask,
// and one that breaks off resumes where it stopped (an HTTP range) when it is asked again.
// The image is not among them: it never leaves the machine, the same as the tracer.

// Pinned, both of them. A runtime that silently moved under the page would be a second,
// untested set of kernels for a network whose agreement with the native one is a measured
// claim; and `wasmPaths` must point at the dist folder of the very same version, or ONNX
// Runtime loads its JavaScript from one release and its WebAssembly from another.
const ORT_VERSION = "1.30.0";
const ORT_BASE = `https://cdn.jsdelivr.net/npm/onnxruntime-web@${ORT_VERSION}/dist/`;
const ORT_ENTRY = `${ORT_BASE}ort.webgpu.min.mjs`;
// What the WebGPU build loads at run time (1.30 builds its GPU and CPU kernels with
// asyncify): the WebAssembly, handed over as bytes from the cache (`env.wasm.wasmBinary`),
// and its loader, which only the HTTP cache keeps.
const ORT_WASM = `${ORT_BASE}ort-wasm-simd-threaded.asyncify.wasm`;
const ORT_LOADER = `${ORT_BASE}ort-wasm-simd-threaded.asyncify.mjs`;
// Its size, for progress: jsDelivr compresses it, so its Content-Length is not this.
const ORT_WASM_BYTES = 26781914;
const ORT_CACHE = `inkvec-ort-${ORT_VERSION}`;

// Where a verified copy of the weights lives between visits. Bump the suffix if the model
// the page asks for ever changes, so an old entry cannot be served for a new URL.
const CACHE = "inkvec-denoiser-v1";

// The export's own names for its input and output (`export_restorer_onnx.py`).
const INPUT = "image";
const OUTPUT = "restored";

let ortPromise = null;
let sessionPromise = null;
let backend = null;
// Where the weights came from, so a session that has to be rebuilt can read them back out
// of the cache instead of holding 80 MB alive for a fallback that usually never happens.
let source = null;

/** The execution provider the loaded session actually runs on, or null before it loads. */
export function activeBackend() {
  return backend;
}

async function ort() {
  if (!ortPromise) {
    ortPromise = (async () => {
      const [mod, binary] = await Promise.all([import(/* @vite-ignore */ ORT_ENTRY), runtimeBinary()]);
      const ort = mod.default ?? mod;
      // The WebGPU build still loads WebAssembly: the GPU kernels live in a wasm module and
      // everything WebGPU has no kernel for falls back to CPU inside the same session. Its
      // bytes come from the cache; its loader from the same folder, the same version.
      ort.env.wasm.wasmPaths = ORT_BASE;
      if (binary) ort.env.wasm.wasmBinary = binary;
      // Threads need `SharedArrayBuffer`, which needs the cross-origin isolation the Space
      // asks for in its README. Where it is not granted this is one thread, and the CPU
      // fallback is correspondingly slower — the same trade the tracer makes next door.
      ort.env.wasm.numThreads = self.crossOriginIsolated
        ? Math.max(1, Math.min(navigator.hardwareConcurrency || 4, 8))
        : 1;
      ort.env.logLevel = "error";
      return ort;
    })();
    ortPromise.catch(() => { ortPromise = null; });
  }
  return ortPromise;
}

const hex = (buf) =>
  [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

async function openCache(name) {
  try {
    return await caches.open(name);
  } catch (e) {
    // Private windows and pages with site data blocked have no Cache Storage. Downloading
    // every time is worse than caching, and better than not working.
    return null;
  }
}

/** Bytes of one file already fetched in this page, by URL, while its download is incomplete. */
const partial = new Map();
/** Downloads in flight, by URL: a second caller joins the first instead of fetching again. */
const inflight = new Map();
let aborter = null;

/**
 * One file, streamed with progress. If an earlier attempt in this page broke off, the bytes it
 * got are kept and only the rest is asked for (`Range`); a server that answers with the whole
 * file instead simply starts it over.
 */
async function download(url, { onBytes, priority, knownBytes, signal }) {
  const have = partial.get(url) ?? { chunks: [], received: 0 };
  const headers = have.received > 0 ? { Range: `bytes=${have.received}-` } : undefined;
  // `no-store`: the bytes go into Cache Storage below, and a second copy in the HTTP cache
  // would only double what is written to disk while the page is starting.
  const res = await fetch(url, { headers, priority, signal, cache: "no-store" });
  if (!(res.ok || res.status === 206)) throw new Error(`HTTP ${res.status} for ${url.split("?")[0]}`);
  if (res.status !== 206) {
    have.chunks = [];
    have.received = 0;
  }
  partial.set(url, have);
  const length = Number(res.headers.get("content-length")) || 0;
  const encoded = Boolean(res.headers.get("content-encoding"));
  const total = knownBytes || (!encoded && length ? have.received + length : 0);
  onBytes(have.received, total);
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    have.chunks.push(value);
    have.received += value.length;
    onBytes(have.received, total);
  }
  const bytes = new Uint8Array(have.received);
  let at = 0;
  for (const c of have.chunks) { bytes.set(c, at); at += c.length; }
  partial.delete(url);
  return bytes;
}

/**
 * What is still to be fetched of `files` ([{ url, cache, sha256?, knownBytes? }]), fetched
 * together with one combined progress report, verified where a hash is known, and stored.
 * Returns each file's bytes (from the cache or the network), in order.
 */
async function ensure(files, { onProgress, priority = "auto", read = true } = {}) {
  const found = await Promise.all(
    files.map(async (f) => {
      const cache = await openCache(f.cache);
      const hit = cache && (await cache.match(f.url));
      return { ...f, cache, hit };
    }),
  );
  const missing = found.filter((f) => !f.hit);
  if (!missing.length) {
    onProgress?.({ received: 0, total: 0, cached: true });
    return read ? Promise.all(found.map(async (f) => new Uint8Array(await f.hit.arrayBuffer()))) : [];
  }
  const got = new Map();
  const totals = new Map();
  const report = () => {
    let received = 0;
    let total = 0;
    for (const f of missing) {
      received += got.get(f.url) ?? 0;
      total += totals.get(f.url) || f.knownBytes || 0;
    }
    onProgress?.({ received, total: Math.max(total, received), cached: false });
  };
  if (!aborter) aborter = new AbortController();
  const signal = aborter.signal;
  const fetched = await Promise.all(
    missing.map((f) => {
      let job = inflight.get(f.url);
      if (!job) {
        job = (async () => {
          const bytes = await download(f.url, {
            priority,
            knownBytes: f.knownBytes,
            signal,
            onBytes: (n, t) => {
              got.set(f.url, n);
              if (t) totals.set(f.url, t);
              report();
            },
          });
          if (f.sha256) {
            const digest = hex(await crypto.subtle.digest("SHA-256", bytes));
            if (digest !== f.sha256) {
              throw new Error(`the denoiser weights failed verification: SHA-256 ${digest} is not ${f.sha256}`);
            }
          }
          if (f.cache) {
            // A failure here is a full disk or a quota, not a reason to refuse to denoise.
            try {
              const headers = { "content-type": "application/octet-stream", "content-length": String(bytes.length) };
              await f.cache.put(f.url, new Response(bytes, { headers }));
            } catch (e) { /* the next visit downloads again */ }
          }
          return bytes;
        })();
        inflight.set(f.url, job);
        job.then(() => inflight.delete(f.url), () => inflight.delete(f.url));
      } else {
        // Joined a download another caller started: its progress is reported there, and
        // here the file counts as arriving all at once.
        totals.set(f.url, f.knownBytes || 0);
        job.then((b) => { got.set(f.url, b.length); totals.set(f.url, b.length); report(); }, () => {});
      }
      return job;
    }),
  ).finally(() => {
    if (!inflight.size) aborter = null;
  });
  if (!read) return [];
  const out = [];
  let k = 0;
  for (const f of found) out.push(f.hit ? new Uint8Array(await f.hit.arrayBuffer()) : fetched[k++]);
  return out;
}

const runtimeFile = () => ({ url: ORT_WASM, cache: ORT_CACHE, knownBytes: ORT_WASM_BYTES });
const weightsFile = (url, sha256) => ({ url, cache: CACHE, sha256 });

/** ONNX Runtime Web's WebAssembly, from the cache, or fetched and kept there. */
async function runtimeBinary() {
  const [bytes] = await ensure([runtimeFile()]);
  return bytes;
}

/**
 * Fetch everything the denoiser needs and keep it, without starting it: the weights
 * (verified against `sha256`) and ONNX Runtime Web. `priority` is the fetch priority hint
 * ("low" for a download nobody is waiting for yet). Resolves to `{ cached }`: whether
 * everything was already here. Nothing is read back out of the cache to answer that.
 */
export async function prefetch({ url, sha256, onProgress, priority = "low" } = {}) {
  let cached = true;
  await ensure([weightsFile(url, sha256), runtimeFile()], {
    priority,
    read: false,
    onProgress: (p) => {
      if (!p.cached) cached = false;
      onProgress?.(p);
    },
  });
  // The small loaders as well, so a later visit's first run needs nothing from the network.
  if (!cached) {
    await Promise.all(
      [ORT_ENTRY, ORT_LOADER].map((u) => fetch(u, { priority }).then((r) => r.arrayBuffer(), () => null)),
    );
  }
  return { cached };
}

/** Stop any download in flight; what arrived is kept, and the next request resumes it. */
export function abort() {
  aborter?.abort();
  aborter = null;
}

/**
 * The weights, from the cache if they are there and from Hugging Face if they are not,
 * verified either way against the SHA-256 the tracer reports.
 *
 * Verified either way on purpose: `inkvec_restore::pull_onnx_weights` re-checks a file it
 * finds already present rather than trusting it, because a half-written or tampered copy
 * would otherwise fail much later, deep inside the runtime, with an unrelated message. The
 * hash costs about a tenth of a second against the minute the download costs once.
 */
async function weights(url, sha256, onProgress) {
  // The runtime is fetched alongside, so the progress reported is for everything missing.
  const [bytes] = await ensure([weightsFile(url, sha256), runtimeFile()], { onProgress, priority: "high" });
  const digest = hex(await crypto.subtle.digest("SHA-256", bytes));
  if (digest !== sha256) {
    const cache = await openCache(CACHE);
    if (cache) await cache.delete(url);
    throw new Error(`the denoiser weights failed verification: SHA-256 ${digest} is not ${sha256}`);
  }
  return bytes;
}

async function build(ortMod, model, eps) {
  return ortMod.InferenceSession.create(model, {
    executionProviders: eps,
    graphOptimizationLevel: "all",
  });
}

/**
 * Whether WebGPU here is real hardware, which is not the same question as whether
 * `navigator.gpu` exists.
 *
 * A browser with no usable GPU normally answers `requestAdapter()` with null, and that is the
 * end of it. It can also hand out a **software adapter** — WebGPU implemented on the CPU,
 * SwiftShader in Chrome, lavapipe or llvmpipe through Mesa. Running a 19.7-million-parameter
 * convolutional network through one is the worst of both worlds: CPU arithmetic with a GPU
 * abstraction in the way, where ONNX Runtime's own WebAssembly kernels are SIMD and, on an
 * isolated page, threaded. Measured in a headless Chromium whose only adapter was SwiftShader
 * (`vendor: "google", architecture: "swiftshader"`): the 512-px pass that the WebAssembly
 * kernels finish in about eight seconds there had still not returned ten minutes in.
 *
 * `isFallbackAdapter` is the spec's own name for this and is checked first, but Chrome does
 * not set it for SwiftShader, so the adapter's reported architecture is checked too.
 */
const SOFTWARE_ADAPTERS = /swiftshader|lavapipe|llvmpipe|software|warp/i;

async function realGpu() {
  if (!navigator.gpu) return false;
  try {
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
    if (!adapter) return false;
    const info = adapter.info ?? {};
    const named = `${info.architecture ?? ""} ${info.device ?? ""} ${info.description ?? ""}`;
    if (adapter.isFallbackAdapter || SOFTWARE_ADAPTERS.test(named)) {
      const what = named.trim() || "fallback";
      console.info(`inkvec: WebGPU here is software (${what}); using the CPU kernels`);
      return false;
    }
    return true;
  } catch (e) {
    return false;
  }
}

/**
 * Load the denoiser, once per page. `url` and `sha256` come from the tracer
 * (`denoiser_model_url()` / `denoiser_model_sha256()`) so that the page cannot end up
 * checking a different model than the one the build was made against.
 *
 * Returns `{ backend }`. WebGPU is asked for a session before it is believed: an adapter can
 * exist and still refuse this graph, and the only honest way to report which kernels ran is
 * to have watched one of them start.
 */
export async function load({ url, sha256, onProgress, onStage } = {}) {
  if (!sessionPromise) {
    source = { url, sha256 };
    sessionPromise = (async () => {
      // The weights first: that fetches whatever is missing of both files, with progress,
      // so the runtime after it comes out of the cache.
      onStage?.("weights");
      const model = await weights(url, sha256, onProgress);
      onStage?.("runtime");
      const ortMod = await ort();
      onStage?.("session");
      if (await realGpu()) {
        try {
          const s = await build(ortMod, model, ["webgpu"]);
          backend = "webgpu";
          return s;
        } catch (e) {
          console.warn("inkvec: WebGPU would not take the denoiser, using the CPU kernels:", e);
        }
      }
      const s = await build(ortMod, model, ["wasm"]);
      backend = "wasm";
      return s;
    })();
    sessionPromise.catch(() => { sessionPromise = null; backend = null; });
  }
  await sessionPromise;
  return { backend };
}

/**
 * Run the network. `input` is what `Intake.denoiser_input()` returned and `[width, height]`
 * is `Intake.denoiser_input_size()`; the result goes straight back to
 * `Intake.take_denoiser_output()`.
 *
 * A WebGPU session that fails mid-run — a lost device, a buffer it cannot allocate for a
 * large image — is rebuilt on the CPU kernels once and the run retried, because the
 * alternative is telling someone their image is untraceable when it is only too big for
 * their GPU.
 */
export async function run(input, width, height) {
  const session = await sessionPromise;
  if (!session) throw new Error("the denoiser is not loaded");
  const ortMod = await ort();
  const dims = [1, 3, height, width];

  const once = async (s) => {
    const out = await s.run({ [INPUT]: new ortMod.Tensor("float32", input, dims) });
    const t = out[OUTPUT];
    if (!t) throw new Error(`the denoiser returned no "${OUTPUT}" tensor`);
    return t.data;
  };

  try {
    return await once(session);
  } catch (e) {
    if (backend !== "webgpu" || !source) throw e;
    console.warn("inkvec: the WebGPU denoiser run failed, retrying on the CPU kernels:", e);
    // The weights are in the cache by now, so this reads them back rather than downloading
    // them again.
    const model = await weights(source.url, source.sha256);
    sessionPromise = build(ortMod, model, ["wasm"]);
    backend = "wasm";
    return once(await sessionPromise);
  }
}
