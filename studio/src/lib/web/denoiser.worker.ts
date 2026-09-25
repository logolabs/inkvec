/**
 * The denoiser, in a worker of its own: `restorer.onnx` in ONNX Runtime Web (WebGPU where
 * the GPU is real, its WebAssembly kernels where not), loaded by the old Space's
 * `denoise.js`, unchanged.
 *
 * Two callers. The page downloads the weights here on request (Settings' "Download the
 * denoiser"), with progress; and the engine worker, in the middle of a trace, posts a
 * tensor down a `MessagePort` and blocks on a shared buffer until this worker writes the
 * network's answer into it. Everything around the network — composite, pad, crop,
 * quantise, snap, the `auto` decision — stays in the tracer.
 */

/// <reference lib="webworker" />

interface DenoiseModule {
  load(o: {
    url: string;
    sha256: string;
    onProgress?: (p: { received: number; total: number; cached: boolean }) => void;
    onStage?: (s: string) => void;
  }): Promise<{ backend: string }>;
  run(input: Float32Array, width: number, height: number): Promise<Float32Array>;
  activeBackend(): string | null;
}

const scope = self as unknown as DedicatedWorkerGlobalScope;

let base = "";
let token = "";
let url = "";
let sha256 = "";
let module: Promise<DenoiseModule> | null = null;

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

async function load(report: boolean): Promise<string> {
  const d = await denoise();
  let last = 0;
  const { backend } = await d.load({
    url,
    sha256,
    onProgress: report
      ? (p) => {
          const now = performance.now();
          if (p.cached || p.received === p.total || now - last > 200) {
            last = now;
            scope.postMessage({ type: "progress", got: p.received, total: p.total || null });
          }
        }
      : undefined,
  });
  return backend;
}

async function runOnce(m: { input: Float32Array; width: number; height: number; shared: SharedArrayBuffer }) {
  const head = new Int32Array(m.shared, 0, 2);
  try {
    await load(false);
    const out = await (await denoise()).run(m.input, m.width, m.height);
    new Float32Array(m.shared, 8, out.length).set(out);
    Atomics.store(head, 0, 1);
  } catch (e) {
    const text = new TextEncoder().encode(String((e as Error)?.message ?? e)).slice(0, m.shared.byteLength - 8);
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
  if (m.type === "init" || m.type === "port") {
    // The engine's requests arrive on their own port, so a download in progress here never
    // queues behind them or they behind it. A replacement engine sends a new one.
    (m.port as MessagePort).onmessage = (ev: MessageEvent) => {
      if (ev.data?.type === "run") void runOnce(ev.data);
    };
    return;
  }
  if (m.type === "download") {
    load(true).then(
      (backend) => scope.postMessage({ type: "done", ok: true, backend }),
      (err) => scope.postMessage({ type: "done", ok: false, message: String((err as Error)?.message ?? err) }),
    );
  }
};

// A module, so its names are its own and not the page's globals.
export {};
