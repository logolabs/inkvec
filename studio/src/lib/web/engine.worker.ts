/**
 * Inkvec Studio Lite's backend: the Studio core (`studio/wasm`) in a Web Worker.
 *
 * The desktop app runs each command on a thread of its own and reports long work as
 * events; this worker is the browser's one such thread. It takes one job at a time from the
 * page (`engine.ts` decides the order: palette and export ahead of traces, traces ahead of
 * wizard previews, stale ones dropped) and answers it synchronously, streaming a trace's
 * stages as it goes. The pipeline's own parallelism is rayon's, on a pool of nested workers
 * where the page is cross-origin isolated.
 *
 * Two builds sit side by side, as on the old Space: `pkg-threads/` (atomics, shared memory,
 * a rayon pool) where `crossOriginIsolated` is true, `pkg/` (one core) where it is not. Same
 * source, same f64, same output.
 */

/// <reference lib="webworker" />

interface StudioWasm {
  capabilities(supported: boolean, installed: boolean, bytes: number | undefined): string;
  open_bytes(bytes: Uint8Array, name: string | undefined): string;
  trace(
    request: string,
    draftPx: number,
    draftSeconds: number,
    generation: number,
    onStage: (name: string, ms: number) => void,
    isCurrent: (generation: number) => boolean,
  ): string | undefined;
  preview(request: string, draftPx: number, draftSeconds: number): string;
  call(cmd: string, args: string): string;
  export_files(request: string, zip: boolean): { name: string; data: Uint8Array }[];
  close(): void;
}

interface WasmModule {
  default(init: { module_or_path: URL | Response }): Promise<{ memory?: WebAssembly.Memory }>;
  Studio: new () => StudioWasm;
  initThreadPool?: (n: number) => Promise<void>;
  threads_available(): boolean;
  version(): string;
  enable_denoiser(): void;
  denoiser_model_url(): string;
  denoiser_model_sha256(): string;
  denoiser_model_repo(): string;
}

type Init = {
  type: "init";
  base: string;
  token: string;
  /** The page's current trace generation, shared so a retired trace skips its measurement. */
  generations: SharedArrayBuffer | null;
  denoiserPort: MessagePort | null;
  /** Each module's size in bytes (`pkg`, `pkg-threads`), known at build time; see `withProgress`. */
  wasmBytes: Record<string, number>;
};

type Job = { type: "job"; id: number; op: string; [k: string]: unknown };

const scope = self as unknown as DedicatedWorkerGlobalScope & {
  inkvecDenoiseSync?: (input: Float32Array, width: number, height: number) => Float32Array;
};

let mod: WasmModule | null = null;
/** The module's linear memory: it only grows, so its size is the session's high-water mark. */
let memory: WebAssembly.Memory | null = null;
let studio: StudioWasm | null = null;
let generations: Int32Array | null = null;
let denoiserPort: MessagePort | null = null;

/**
 * The module's bytes, counted as they arrive, for the loading screen's bar. The response
 * is re-wrapped rather than read whole, so compilation still streams alongside the download.
 * The total is the size the build recorded: a host that compresses the file sends a
 * Content-Length for the compressed bytes, not for what the stream yields.
 */
function withProgress(res: Response, known: number | undefined): Response {
  if (!res.ok || !res.body || typeof TransformStream === "undefined") return res;
  const total = known || Number(res.headers.get("content-length")) || 0;
  let got = 0;
  let last = 0;
  const counted = res.body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, ctl) {
        got += chunk.byteLength;
        const now = performance.now();
        if (now - last > 40) {
          last = now;
          scope.postMessage({ type: "loading", got, total });
        }
        ctl.enqueue(chunk);
      },
      flush() {
        scope.postMessage({ type: "loading", got, total: Math.max(total, got), done: true });
      },
    }),
  );
  return new Response(counted, { status: res.status, headers: { "content-type": "application/wasm" } });
}

async function init(m: Init): Promise<void> {
  const isolated = typeof crossOriginIsolated !== "undefined" && crossOriginIsolated;
  const dir = isolated ? "pkg-threads" : "pkg";
  const q = m.token ? `?v=${m.token}` : "";
  const js = new URL(`${dir}/inkvec_studio_wasm.js${q}`, m.base).href;
  // The module's bytes are asked for at once, alongside its JavaScript, not after it.
  const wasm = fetch(new URL(`${dir}/inkvec_studio_wasm_bg.wasm${q}`, m.base));
  // Handled where it is awaited, below; this only keeps a failure there from being reported
  // as unhandled while the JavaScript is still arriving.
  wasm.catch(() => undefined);
  mod = (await import(/* @vite-ignore */ js)) as WasmModule;
  const exports = await mod.default({ module_or_path: withProgress(await wasm, m.wasmBytes?.[dir]) });
  memory = exports.memory ?? null;
  // Compiled and instantiated: the page can start what waits on the engine's bytes being in
  // (the denoiser's download) while the thread pool starts.
  scope.postMessage({
    type: "instantiated",
    denoiserUrl: mod.denoiser_model_url(),
    denoiserSha256: mod.denoiser_model_sha256(),
  });
  let threads = 1;
  let poolError: string | null = null;
  if (isolated && mod.initThreadPool) {
    // One worker per core, less the one this thread already is. If the pool will not start
    // (nested workers refused, memory), rayon runs everything here instead: slower, same bytes.
    const n = Math.max(1, Math.min(navigator.hardwareConcurrency || 4, 16) - 1);
    try {
      await mod.initThreadPool(n);
      threads = n + 1;
    } catch (e) {
      poolError = String((e as Error)?.message ?? e);
    }
  }
  generations = m.generations ? new Int32Array(m.generations) : null;
  denoiserPort = m.denoiserPort;
  if (denoiserPort && isolated) {
    installDenoiseBridge(denoiserPort);
    mod.enable_denoiser();
  }
  studio = new mod.Studio();
  scope.postMessage({
    type: "ready",
    threads,
    poolError,
    isolated,
    version: mod.version(),
    denoiserUrl: mod.denoiser_model_url(),
    denoiserSha256: mod.denoiser_model_sha256(),
    denoiserRepo: mod.denoiser_model_repo(),
  });
}

/**
 * The pipeline is synchronous and ONNX Runtime Web is not, so the tensor goes to the
 * denoiser's own worker and this thread waits on a shared buffer until the answer is
 * written into it. A worker may block (`Atomics.wait`); the page's main thread never does.
 */
function installDenoiseBridge(port: MessagePort): void {
  scope.inkvecDenoiseSync = (input, width, height) => {
    const floats = input.length;
    // [status, error length] then the output floats; the error text reuses the float area.
    const shared = new SharedArrayBuffer(8 + Math.max(floats * 4, 4096));
    const head = new Int32Array(shared, 0, 2);
    port.postMessage({ type: "run", input: input.slice(), width, height, shared });
    // Ten minutes: a CPU fallback on a large image is slow, but never this slow.
    const woke = Atomics.wait(head, 0, 0, 600_000);
    if (woke === "timed-out") throw new Error("the denoiser did not answer in ten minutes");
    if (Atomics.load(head, 0) !== 1) {
      const text = new TextDecoder().decode(new Uint8Array(shared, 8, Atomics.load(head, 1)).slice());
      throw new Error(text || "the denoiser failed");
    }
    return new Float32Array(shared, 8, floats).slice();
  };
}

function isCurrent(generation: number): boolean {
  return generations ? Atomics.load(generations, 0) === generation : true;
}

function run(job: Job): unknown {
  const s = studio;
  if (!s) throw new Error("the engine is not loaded");
  switch (job.op) {
    case "capabilities":
      return JSON.parse(
        s.capabilities(Boolean(job.supported), Boolean(job.installed), (job.bytes as number | null) ?? undefined),
      );
    case "open_bytes":
      return JSON.parse(s.open_bytes(job.bytes as Uint8Array, (job.name as string | null) ?? undefined));
    case "trace": {
      const generation = job.generation as number;
      const out = s.trace(
        JSON.stringify(job.request),
        job.draftPx as number,
        job.draftSeconds as number,
        generation,
        (name, ms) => scope.postMessage({ type: "stage", generation, name, ms }),
        isCurrent,
      );
      return out === undefined ? null : JSON.parse(out);
    }
    case "preview":
      return JSON.parse(s.preview(JSON.stringify(job.request), job.draftPx as number, job.draftSeconds as number));
    case "export": {
      const files = s.export_files(JSON.stringify({ request: job.request }), Boolean(job.zip));
      return files.map((f) => ({ name: f.name, data: f.data }));
    }
    case "close":
      s.close();
      return null;
    default:
      return JSON.parse(s.call(job.op, JSON.stringify(job.args ?? {})));
  }
}

/** A trap inside WebAssembly leaves the instance unusable; the page starts a fresh one. */
function isTrap(e: unknown): boolean {
  return (
    e instanceof WebAssembly.RuntimeError ||
    (e instanceof RangeError && /memory|allocation/i.test(e.message)) ||
    /unreachable|out of memory|memory access out of bounds/i.test(String((e as Error)?.message ?? e))
  );
}

let chain: Promise<void> = Promise.resolve();

scope.onmessage = (e: MessageEvent<Init | Job>) => {
  const m = e.data;
  chain = chain.then(async () => {
    if (m.type === "init") {
      try {
        await init(m);
      } catch (err) {
        scope.postMessage({ type: "loadfail", error: String((err as Error)?.stack ?? err) });
      }
      return;
    }
    const started = performance.now();
    try {
      const value = run(m);
      const transfer: Transferable[] = [];
      if (m.op === "export") for (const f of value as { data: Uint8Array }[]) transfer.push(f.data.buffer);
      scope.postMessage({ type: "reply", id: m.id, ok: true, value, ms: performance.now() - started, mem: memory?.buffer.byteLength ?? null }, transfer);
    } catch (err) {
      const message = String((err as Error)?.message ?? err);
      scope.postMessage({ type: "reply", id: m.id, ok: false, error: message, crashed: isTrap(err) });
    }
  });
};

// A module, so its names are its own and not the page's globals.
export {};
