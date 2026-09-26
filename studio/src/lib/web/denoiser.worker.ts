/**
 * The denoiser, in a worker of its own: `restorer.onnx` in ONNX Runtime Web (WebGPU where
 * the GPU is real, its WebAssembly kernels where not), loaded by the old Space's
 * `denoise.js`.
 *
 * Two callers. The page: it has the weights and the runtime fetched and kept (`prefetch`,
 * started in the background as soon as the engine has arrived, or `download` when asked
 * for), then has the session built (`prepare`) once a trace wants it, with progress for
 * both. And the engine worker, in the middle of a trace, posts a tensor down a `MessagePort`
 * and blocks on a shared buffer until this worker writes the network's answer into it. The
 * page only lets a trace ask once the session is built, so that wait is the network's run
 * and never a download. Everything around the network — composite, pad, crop, quantise,
 * snap, the `auto` decision — stays in the tracer.
 *
 * To the page: `progress` (bytes), `stored` (everything is in the cache), `preparing`,
 * `loaded`, `failed` with what failed, and `ran` after each run a trace asked for.
 */

/// <reference lib="webworker" />

interface Progress {
  received: number;
  total: number;
  cached: boolean;
}

interface DenoiseModule {
  prefetch(o: {
    url: string;
    sha256: string;
    priority?: "low" | "high" | "auto";
    onProgress?: (p: Progress) => void;
  }): Promise<{ cached: boolean }>;
  abort(): void;
  load(o: { url: string; sha256: string; onProgress?: (p: Progress) => void }): Promise<{ backend: string }>;
  run(input: Float32Array, width: number, height: number): Promise<Float32Array>;
  activeBackend(): string | null;
}

const scope = self as unknown as DedicatedWorkerGlobalScope;

let base = "";
let token = "";
let url = "";
let sha256 = "";
let module: Promise<DenoiseModule> | null = null;
/** The fetch in flight or done, shared by every caller: one download however many ask. */
let storing: Promise<{ cached: boolean }> | null = null;
let preparing: Promise<string> | null = null;

function denoise(): Promise<DenoiseModule> {
  if (!module) {
    module = import(/* @vite-ignore */ new URL(`denoise.js${token ? `?v=${token}` : ""}`, base).href) as Promise<DenoiseModule>;
    // A failed fetch is not kept: the next request tries again rather than failing for the
    // rest of the session.
    module.catch(() => {
      module = null;
    });
  }
  return module;
}

const message = (e: unknown) => String((e as Error)?.message ?? e);

/** Fetch and keep the weights and the runtime, reporting bytes at most five times a second. */
function store(priority: "low" | "high"): Promise<{ cached: boolean }> {
  if (!storing) {
    let last = 0;
    storing = (async () => {
      const d = await denoise();
      return d.prefetch({
        url,
        sha256,
        priority,
        onProgress: (p) => {
          if (p.cached) return;
          const now = performance.now();
          if (p.received === p.total || now - last > 200) {
            last = now;
            scope.postMessage({ type: "progress", got: p.received, total: p.total || null });
          }
        },
      });
    })();
    // A failure is reported by whoever asked; the next ask starts again (and resumes).
    storing.catch(() => {
      storing = null;
    });
  }
  return storing;
}

/** The session, built once everything is stored. */
function prepare(): Promise<string> {
  if (!preparing) {
    preparing = (async () => {
      const { cached } = await store("high");
      scope.postMessage({ type: "stored", cached });
      scope.postMessage({ type: "preparing" });
      const { backend } = await (await denoise()).load({ url, sha256 });
      return backend;
    })();
    preparing.catch(() => {
      preparing = null;
    });
  }
  return preparing;
}

async function runOnce(m: { input: Float32Array; width: number; height: number; shared: SharedArrayBuffer }) {
  const head = new Int32Array(m.shared, 0, 2);
  try {
    await prepare();
    const out = await (await denoise()).run(m.input, m.width, m.height);
    new Float32Array(m.shared, 8, out.length).set(out);
    Atomics.store(head, 0, 1);
    scope.postMessage({ type: "ran", width: m.width, height: m.height });
  } catch (e) {
    const text = new TextEncoder().encode(message(e)).slice(0, m.shared.byteLength - 8);
    new Uint8Array(m.shared, 8, text.length).set(text);
    Atomics.store(head, 1, text.length);
    Atomics.store(head, 0, 2);
  }
  Atomics.notify(head, 0);
}

scope.onmessage = (e: MessageEvent) => {
  const m = e.data;
  if (m.type === "init") {
    ({ base, token, url, sha256 } = m);
  }
  if ((m.type === "init" || m.type === "port") && m.port) {
    // The engine's requests arrive on their own port, so a download in progress here never
    // queues behind them or they behind it. A replacement engine sends a new one.
    (m.port as MessagePort).onmessage = (ev: MessageEvent) => {
      if (ev.data?.type === "run") void runOnce(ev.data);
    };
    return;
  }
  // Every request is answered, including one for something already done.
  if (m.type === "prefetch" || m.type === "download") {
    store(m.type === "prefetch" ? "low" : "high").then(
      ({ cached }) => scope.postMessage({ type: "stored", cached }),
      (err) => scope.postMessage({ type: "failed", during: "download", message: message(err) }),
    );
  }
  if (m.type === "prepare") {
    prepare().then(
      (backend) => scope.postMessage({ type: "loaded", backend }),
      (err) => scope.postMessage({ type: "failed", during: storing ? "prepare" : "download", message: message(err) }),
    );
  }
  if (m.type === "abort") {
    void module?.then((d) => d.abort());
  }
  // The stored copy was removed (Settings): the next request fetches again. A session
  // already running keeps running for the rest of this page.
  if (m.type === "forget") {
    storing = null;
  }
};

// A module, so its names are its own and not the page's globals.
export {};
