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
// Two things are fetched the first time the denoiser runs, and cached in the browser
// afterwards: ONNX Runtime Web (~28 MB with its WebGPU kernels) and the weights (~80 MB).
// The image is not one of them — it never leaves the machine, the same as the tracer.

// Pinned, both of them. A runtime that silently moved under the page would be a second,
// untested set of kernels for a network whose agreement with the native one is a measured
// claim; and `wasmPaths` must point at the dist folder of the very same version, or ONNX
// Runtime loads its JavaScript from one release and its WebAssembly from another.
const ORT_VERSION = "1.30.0";
const ORT_BASE = `https://cdn.jsdelivr.net/npm/onnxruntime-web@${ORT_VERSION}/dist/`;
const ORT_ENTRY = `${ORT_BASE}ort.webgpu.min.mjs`;

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
    ortPromise = import(/* @vite-ignore */ ORT_ENTRY).then((mod) => {
      const ort = mod.default ?? mod;
      // The WebGPU build still loads WebAssembly: the GPU kernels live in a wasm module
      // (`ort-wasm-simd-threaded.jsep.wasm`) and everything WebGPU has no kernel for falls
      // back to CPU inside the same session.
      ort.env.wasm.wasmPaths = ORT_BASE;
      // Threads need `SharedArrayBuffer`, which needs the cross-origin isolation the Space
      // asks for in its README. Where it is not granted this is one thread, and the CPU
      // fallback is correspondingly slower — the same trade the tracer makes next door.
      ort.env.wasm.numThreads = self.crossOriginIsolated
        ? Math.max(1, Math.min(navigator.hardwareConcurrency || 4, 8))
        : 1;
      ort.env.logLevel = "error";
      return ort;
    });
    ortPromise.catch(() => { ortPromise = null; });
  }
  return ortPromise;
}

const hex = (buf) =>
  [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

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
  let cache = null;
  try {
    cache = await caches.open(CACHE);
  } catch (e) {
    // Private windows and pages with site data blocked have no Cache Storage. Downloading
    // every time is worse than caching, and better than not working.
  }

  let bytes = null;
  const hit = cache && (await cache.match(url));
  if (hit) {
    bytes = new Uint8Array(await hit.arrayBuffer());
    onProgress?.({ received: bytes.length, total: bytes.length, cached: true });
  } else {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`the denoiser weights could not be fetched: HTTP ${res.status}`);
    const total = Number(res.headers.get("content-length")) || 0;
    const chunks = [];
    let received = 0;
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      received += value.length;
      onProgress?.({ received, total, cached: false });
    }
    bytes = new Uint8Array(received);
    let at = 0;
    for (const c of chunks) { bytes.set(c, at); at += c.length; }
  }

  const digest = hex(await crypto.subtle.digest("SHA-256", bytes));
  if (digest !== sha256) {
    if (cache) await cache.delete(url);
    throw new Error(
      `the denoiser weights failed verification: SHA-256 ${digest} is not ${sha256}`,
    );
  }
  if (cache && !hit) {
    // A failure here is a full disk or a quota, not a reason to refuse to denoise.
    try {
      const headers = { "content-type": "application/octet-stream" };
      await cache.put(url, new Response(bytes, { headers }));
    } catch (e) { /* the next visit downloads again */ }
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
      onStage?.("runtime");
      const ortMod = await ort();
      onStage?.("weights");
      const model = await weights(url, sha256, onProgress);
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
