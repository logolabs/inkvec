// Loads the threaded build and starts its pool.
//
// wasm-bindgen-rayon starts the pool with the Web Worker API -- `new Worker(url, { type:
// "module" })`, `postMessage`, `addEventListener("message")`. Browsers, Deno and Bun have
// it. Node.js does not, but `worker_threads` does the same job with a different surface,
// so there this adapts one to the other, in two halves:
//
//   - here, a `Worker` class over `worker_threads.Worker`. It is put on `globalThis` only
//     for the synchronous call that starts the pool (every worker is constructed inside
//     it) and removed straight after, so the process is never left with a `Worker` global
//     that other code could mistake for a browser;
//   - `node-worker.js`, which each worker thread runs first: it gives that thread the
//     `self` / `postMessage` / `addEventListener` a Web Worker has, then imports the rayon
//     helper.
//
// The Node.js workers are unref'd: they park inside the WebAssembly waiting for work and
// would otherwise keep the process alive after the caller is done.
//
// Browsers get two checks before anything is fetched, because both fail late and badly
// otherwise. The pool's memory is a SharedArrayBuffer, which a browser only hands to a
// cross-origin isolated page; and the thread that calls `trace` waits while the pool works,
// which a browser's main thread is not allowed to do.

import * as glue from "../wasm/threads/inkvec_wasm.js";
import { InkvecError, type Glue, type WasmSource } from "./common.js";
import { builtin, resolveSource } from "./runtime.js";

type Listener = (event: { data: unknown }) => void;
type NodeThread = {
  postMessage(data: unknown): void;
  on(type: string, fn: (data: unknown) => void): void;
  off(type: string, fn: (data: unknown) => void): void;
  unref(): void;
};
type WorkerThreads = {
  Worker: new (url: URL, opts: { workerData: unknown }) => NodeThread;
};

const scope = globalThis as {
  Worker?: unknown;
  crossOriginIsolated?: boolean;
  document?: unknown;
  navigator?: { hardwareConcurrency?: number };
};

/** One worker per core, at most 16; in a browser one fewer, for the page. */
export function defaultThreads(): number {
  const os = builtin<{ availableParallelism?: () => number; cpus: () => unknown[] }>("os");
  if (os) return Math.max(1, Math.min(os.availableParallelism?.() ?? os.cpus().length, 16));
  return Math.max(1, Math.min(scope.navigator?.hardwareConcurrency ?? 4, 16) - 1);
}

export async function load(source: WasmSource | undefined, threads: number): Promise<Glue> {
  const threadsApi = scope.Worker === undefined ? builtin<WorkerThreads>("worker_threads") : undefined;
  if (!threadsApi) checkBrowser();

  const wasm = await resolveSource(
    source ?? new URL("../wasm/threads/inkvec_wasm_bg.wasm", import.meta.url),
  );
  await glue.default({ module_or_path: wasm as never });

  if (!threadsApi) {
    await glue.initThreadPool(threads);
    return glue as unknown as Glue;
  }
  const had = Object.prototype.hasOwnProperty.call(scope, "Worker");
  const previous = scope.Worker;
  scope.Worker = webWorkerOver(threadsApi);
  let started: Promise<unknown>;
  try {
    started = glue.initThreadPool(threads);
  } finally {
    if (had) scope.Worker = previous;
    else delete scope.Worker;
  }
  await started;
  return glue as unknown as Glue;
}

function checkBrowser(): void {
  if (scope.Worker === undefined) {
    throw new InkvecError("load_failed", "@logolabs/inkvec/threads needs Web Workers or Node.js worker_threads");
  }
  if (scope.crossOriginIsolated === false || typeof SharedArrayBuffer === "undefined") {
    throw new InkvecError(
      "load_failed",
      "@logolabs/inkvec/threads needs a cross-origin isolated page (serve it with " +
        "Cross-Origin-Opener-Policy: same-origin and Cross-Origin-Embedder-Policy: require-corp); " +
        "use @logolabs/inkvec where that is not possible",
    );
  }
  if (scope.document !== undefined) {
    throw new InkvecError(
      "load_failed",
      "@logolabs/inkvec/threads must run inside a Web Worker: the thread that calls trace " +
        "waits for the pool, and a browser's main thread is not allowed to wait",
    );
  }
}

/** The Web Worker surface the rayon helper uses, over a `worker_threads` Worker. */
function webWorkerOver(api: WorkerThreads) {
  const bootstrap = new URL("./node-worker.js", import.meta.url);
  return class WebWorker {
    #thread: NodeThread;
    #wrapped = new Map<Listener, (data: unknown) => void>();

    constructor(url: string | URL) {
      this.#thread = new api.Worker(bootstrap, { workerData: { url: String(url) } });
      this.#thread.unref();
    }

    postMessage(data: unknown): void {
      this.#thread.postMessage(data);
    }

    addEventListener(type: string, fn: Listener): void {
      const h = (data: unknown) => fn({ data });
      this.#wrapped.set(fn, h);
      this.#thread.on(type, h);
    }

    removeEventListener(type: string, fn: Listener): void {
      const h = this.#wrapped.get(fn);
      if (h) this.#thread.off(type, h);
      this.#wrapped.delete(fn);
    }
  };
}
