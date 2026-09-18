// The trace runs here so the page stays responsive: a 768-px logo is seconds of work and
// on the main thread that would freeze the controls.
//
// Two builds of the same tracer sit beside each other, and which one loads is decided by
// whether the browser will give us threads. `pkg-threads/` is compiled with the wasm
// atomics feature and starts a rayon pool; the pipeline is already parallel — one
// independent dynamic program per boundary, which is 81% of the single-threaded time — so
// it needs no other change to use it. That build only runs on a cross-origin isolated page,
// because the pool is a `SharedArrayBuffer`. The Space asks for isolation with
// `custom_headers` in its README; anywhere that does not grant it, `pkg/` loads instead and
// behaves exactly as it always has. Same source, same f64, same output, one core.
//
// The `?v=` token is on every URL deliberately. The generated loader resolves the wasm
// against `import.meta.url` and URL resolution drops the base's query, so a token on the
// import alone busts the JavaScript and never the WebAssembly — the wrong way round, since
// the bindings rarely change and the wasm changes on every build. Bump it when `pkg/` or
// `pkg-threads/` is rebuilt.
const V = "v=9";

const isolated = typeof crossOriginIsolated !== "undefined" && crossOriginIsolated;

// One worker per core, less the one this thread is already using. `hardwareConcurrency`
// is what the browser is willing to admit to; on machines that report nothing, four is a
// safe guess that still pays.
const POOL = Math.max(1, Math.min(navigator.hardwareConcurrency || 4, 16) - 1);

async function load() {
  const dir = isolated ? "./pkg-threads" : "./pkg";
  stage = "import"; postMessage({ type: "stage", stage });
  const mod = await import(`${dir}/inkvec_wasm.js?${V}`);
  stage = "instantiate"; postMessage({ type: "stage", stage });
  await mod.default({ module_or_path: new URL(`${dir}/inkvec_wasm_bg.wasm?${V}`, import.meta.url) });
  stage = "pool"; postMessage({ type: "stage", stage });
  let threads = 0;
  if (isolated && mod.initThreadPool) {
    // If the pool cannot start — nested workers refused, memory limits — say so and carry
    // on. Rayon runs its work on the calling thread when it has no pool, so the trace is
    // still correct, just no faster than the fallback would have been.
    try {
      await mod.initThreadPool(POOL);
      threads = POOL;
      stage = "ready";
    } catch (e) {
      poolError = String((e && e.stack) || e);
      console.warn("inkvec: thread pool did not start, running on one core:", e);
    }
  }
  return { mod, threads };
}

let poolError = null;
let stage = "start";

const ready = load().then(({ mod, threads }) => {
  postMessage({ type: "ready", version: mod.version(), threads, poolError });
  return mod;
});
ready.catch((e) => postMessage({ type: "loadfail", stage, error: String((e && e.stack) || e) }));

onmessage = async (e) => {
  const mod = await ready;
  const { id, bytes, o } = e.data;
  const t0 = performance.now();
  try {
    const svg = mod.trace(bytes, o.precision, o.min_area, o.colors, o.merge, o.max_dim,
                          o.time_budget, o.no_background, o.minify, o.margin, o.content_units,
                          o.cutout);
    postMessage({ type: "done", id, svg, ms: performance.now() - t0 });
  } catch (err) {
    postMessage({ type: "error", id, error: String(err) });
  }
};
