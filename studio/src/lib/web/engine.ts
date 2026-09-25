/**
 * Inkvec Studio Lite's backend, as the interface sees it: the same commands and events the
 * desktop app's Tauri backend answers (`src-tauri/src/lib.rs`), answered in the browser.
 *
 * The Studio's work is done by its shared core compiled to WebAssembly, in a worker
 * (`engine.worker.ts`). This module is the part of `lib.rs` that is about running things
 * rather than doing them, rewritten for a tab:
 *
 * - **Order.** The desktop runs every command on a thread of its own and one trace at a time
 *   through its scheduler. Here there is one worker, so the queue lives on this side: quick
 *   commands (palette, export, minify, fabricate) go first, then the trace the viewer waits
 *   for, then wizard previews; a trace or preview that a newer one has replaced before it
 *   started is dropped, and a Fabricate request overtaken by a newer one stands down, as on
 *   the desktop.
 * - **Generations.** `start_trace` returns its generation at once and the result arrives as
 *   `trace:done`; a retired generation's result is never delivered. The current generation
 *   is also shared with the worker, so a drawing nobody waits for any more is not measured.
 * - **What a tab has instead of a computer.** Preferences in `localStorage`, sanitised by the
 *   core; files as downloads; the samples and notices fetched from the site; the denoiser
 *   downloaded into the browser's cache and run by ONNX Runtime Web in a worker of its own.
 * - **What a tab does not have**: batch folders, the command-line install, the context menu
 *   and the update check answer as unavailable, and the interface hides them.
 */

import type { UnlistenFn } from "@tauri-apps/api/event";

import { download } from "./files";

type Priority = 0 | 1 | 2;

interface Job {
  id: number;
  op: string;
  priority: Priority;
  /** Jobs of one kind that replace each other: a newer one drops an older one not yet started. */
  kind?: "trace" | "preview" | "fab";
  payload: Record<string, unknown>;
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

const PREFS_KEY = "inkvec-studio-lite:prefs";
/** Where `denoise.js` keeps the verified weights (its `CACHE`). */
const DENOISER_CACHE = "inkvec-denoiser-v1";
const UNAVAILABLE = "Not available in the browser. Inkvec Studio, the desktop app, has it.";

/** The samples the first-run screen offers, as the desktop's `list_samples` names them. */
const SAMPLES: [string, string][] = [
  ["flat-logo.png", "Flat logo"],
  ["icon-64.png", "Icon, 64 px"],
  ["crest-filigree.png", "Crest, filigree"],
  ["signature-bw.png", "Signature, B&W"],
];

type Listener = (payload: unknown) => void;

class WebBackend {
  private worker: Worker | null = null;
  private ready: Promise<{ threads: number; isolated: boolean; denoiserUrl: string; denoiserSha256: string; denoiserRepo: string }> | null = null;
  private queue: Job[] = [];
  private running: Job | null = null;
  private nextId = 1;
  private listeners = new Map<string, Set<Listener>>();

  private generation = 0;
  private previewGeneration = 0;
  private shared: Int32Array | null = null;

  /** The open image's bytes, to reopen it in a fresh worker after a crash. */
  private source: { bytes: Uint8Array; name: string | null } | null = null;

  private prefs: Record<string, unknown> | null = null;
  private prefsTimer = 0;

  private denoiser: Worker | null = null;
  private denoiserPort: MessagePort | null = null;
  private downloading = false;

  /** The worker's thread count and isolation, once it has loaded; for About and the tests. */
  info: { threads: number; isolated: boolean; version: string } | null = null;
  /** The WebAssembly memory after the last job and how long the worker spent on it. */
  last: { op: string; ms: number; mem: number | null } | null = null;

  // ------------------------------------------------------------------ the worker ---

  private base(): string {
    return new URL("./", document.baseURI).href;
  }

  private start(): Promise<{ threads: number; isolated: boolean; denoiserUrl: string; denoiserSha256: string; denoiserRepo: string }> {
    const isolated = typeof crossOriginIsolated !== "undefined" && crossOriginIsolated;
    if (isolated && !this.shared) this.shared = new Int32Array(new SharedArrayBuffer(4));
    const worker = new Worker(new URL("./engine.worker.ts", import.meta.url), { type: "module", name: "inkvec-engine" });
    this.worker = worker;

    let engineEnd: MessagePort | null = null;
    if (isolated) {
      const channel = new MessageChannel();
      engineEnd = channel.port1;
      this.denoiserPort = channel.port2;
    }

    this.ready = new Promise((resolve, reject) => {
      worker.onmessage = (e: MessageEvent) => {
        const m = e.data;
        if (m.type === "ready") {
          this.info = { threads: m.threads, isolated: m.isolated, version: m.version };
          if (m.poolError) console.warn("inkvec: thread pool did not start:", m.poolError);
          if (isolated) this.startDenoiser(m.denoiserUrl, m.denoiserSha256);
          resolve(m);
        } else if (m.type === "loadfail") {
          reject(new Error(`The engine could not load: ${m.error}`));
        } else if (m.type === "stage") {
          if (m.generation === this.generation) this.emit("trace:stage", { generation: m.generation, name: m.name, ms: m.ms });
        } else if (m.type === "reply") {
          this.finish(m);
        }
      };
      worker.onerror = (e) => {
        // A load error before `ready`; after it, a crash is reported through `reply`.
        reject(new Error(`The engine stopped: ${e.message || "unknown error"}`));
        this.crashed("The engine stopped unexpectedly.");
      };
    });
    const transfer: Transferable[] = engineEnd ? [engineEnd] : [];
    worker.postMessage(
      {
        type: "init",
        base: this.base(),
        token: __INKVEC_WASM_TOKEN__,
        generations: this.shared ? this.shared.buffer : null,
        denoiserPort: engineEnd,
      },
      transfer,
    );
    return this.ready;
  }

  private whenReady() {
    return this.ready ?? this.start();
  }

  /** The worker trapped (a panic, memory): start a fresh one and put the image back. */
  private crashed(message: string): void {
    const running = this.running;
    this.running = null;
    this.worker?.terminate();
    this.worker = null;
    this.ready = null;
    if (running) running.reject(new Error(message));
    const reopen = this.source;
    void this.whenReady().then(async () => {
      if (reopen) {
        await this.enqueue("open_bytes", 0, { bytes: reopen.bytes.slice(), name: reopen.name }).catch(() => {});
      }
      this.pump();
    });
  }

  private finish(m: { id: number; ok: boolean; value?: unknown; error?: string; crashed?: boolean; ms?: number; mem?: number | null }): void {
    const job = this.running;
    if (!job || job.id !== m.id) return;
    this.last = { op: job.op, ms: m.ms ?? 0, mem: m.mem ?? null };
    this.running = null;
    if (m.ok) job.resolve(m.value);
    else if (m.crashed) {
      this.crashed(m.error ?? "The engine stopped.");
      job.reject(new Error(m.error ?? "The engine stopped."));
      return;
    } else job.reject(new Error(m.error ?? "failed"));
    this.pump();
  }

  private enqueue(op: string, priority: Priority, payload: Record<string, unknown>, kind?: Job["kind"]): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (kind) {
        // Latest wins: an older job of this kind that has not started is dropped.
        for (const old of this.queue.filter((j) => j.kind === kind)) {
          old.reject(new Error("superseded by a newer request"));
        }
        this.queue = this.queue.filter((j) => j.kind !== kind);
      }
      this.queue.push({ id: this.nextId++, op, priority, kind, payload, resolve, reject });
      this.queue.sort((a, b) => a.priority - b.priority || a.id - b.id);
      void this.whenReady().then(
        () => this.pump(),
        (e) => {
          for (const j of this.queue) j.reject(e);
          this.queue = [];
        },
      );
    });
  }

  private pump(): void {
    if (this.running || !this.worker || !this.info) return;
    const job = this.queue.shift();
    if (!job) return;
    // A trace or preview retired while it waited is not worth starting.
    if (job.op === "trace" && job.payload.generation !== this.generation) {
      job.resolve(null);
      return this.pump();
    }
    if (job.op === "preview" && job.payload.generation !== this.previewGeneration) {
      job.resolve(null);
      return this.pump();
    }
    this.running = job;
    const transfer: Transferable[] = [];
    if (job.op === "open_bytes") transfer.push((job.payload.bytes as Uint8Array).buffer);
    this.worker.postMessage({ type: "job", id: job.id, op: job.op, ...job.payload }, transfer);
  }

  // ------------------------------------------------------------------- events ---

  private emit(event: string, payload: unknown): void {
    for (const fn of this.listeners.get(event) ?? []) fn(payload);
  }

  async listen<T>(event: string, fn: (e: { payload: T }) => void): Promise<UnlistenFn> {
    const wrapped: Listener = (payload) => fn({ payload: payload as T });
    let set = this.listeners.get(event);
    if (!set) this.listeners.set(event, (set = new Set()));
    set.add(wrapped);
    return () => void set?.delete(wrapped);
  }

  // ----------------------------------------------------------------- commands ---

  async invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    return (await this.dispatch(cmd, args)) as T;
  }

  private async dispatch(cmd: string, a: Record<string, unknown>): Promise<unknown> {
    switch (cmd) {
      case "capabilities": {
        const den = await this.denoiserStatus();
        return this.enqueue("capabilities", 0, { supported: den.supported, installed: den.installed, bytes: den.bytes });
      }
      case "startup_progress":
        return null;
      case "app_ready":
        document.getElementById("boot")?.remove();
        return null;

      case "open_path":
        throw new Error("A browser opens files you choose or drop, not paths.");
      case "open_bytes": {
        const raw = a.bytes as Uint8Array | number[];
        const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
        return this.open(bytes, (a.name as string | null) ?? null);
      }
      case "open_sample": {
        const name = String(a.name);
        if (!SAMPLES.some(([f]) => f === name)) throw new Error(`${name} is not one of the bundled samples`);
        const res = await fetch(new URL(`samples/${name}`, this.base()));
        if (!res.ok) throw new Error(`cannot fetch the sample ${name}: HTTP ${res.status}`);
        return this.open(new Uint8Array(await res.arrayBuffer()), name);
      }
      case "list_samples":
        return SAMPLES.map(([file, label]) => ({ file, label, preview: new URL(`samples/${file}`, this.base()).href }));
      case "launch_path":
        return null;

      case "start_trace": {
        const request = a.request as { settings: Record<string, unknown>; tier: string };
        const generation = this.bump();
        this.rememberTrace(request.settings);
        const p = await this.loadPrefs();
        this.enqueue(
          "trace",
          1,
          { request, draftPx: p.draftPx ?? 512, draftSeconds: p.draftSeconds ?? 0.4, generation },
          "trace",
        ).then(
          (outcome) => {
            if (outcome && generation === this.generation) this.emit("trace:done", { generation, outcome });
          },
          (e: Error) => {
            if (generation === this.generation && !/superseded/.test(e.message)) {
              this.emit("trace:done", { generation, outcome: { state: "failed", message: e.message } });
            }
          },
        );
        return generation;
      }
      case "cancel_trace":
        this.bump();
        this.queue = this.queue.filter((j) => (j.kind === "trace" ? (j.resolve(null), false) : true));
        return null;
      case "start_preview": {
        const request = a.request;
        const generation = ++this.previewGeneration;
        const p = await this.loadPrefs();
        this.enqueue(
          "preview",
          2,
          { request, draftPx: p.draftPx ?? 512, draftSeconds: p.draftSeconds ?? 0.4, generation },
          "preview",
        ).then(
          (outcome) => {
            if (outcome && generation === this.previewGeneration) this.emit("preview:done", { generation, outcome });
          },
          (e: Error) => {
            if (generation === this.previewGeneration && !/superseded/.test(e.message)) {
              this.emit("preview:done", { generation, outcome: { state: "failed", message: e.message } });
            }
          },
        );
        return generation;
      }
      case "cancel_previews":
        this.previewGeneration++;
        this.queue = this.queue.filter((j) => (j.kind === "preview" ? (j.resolve(null), false) : true));
        return null;

      case "source_facts":
      case "trace_bands":
      case "snap_inks":
      case "match_palette":
      case "plan_export":
      case "minify_svg":
      case "fab_analyze":
        return this.enqueue(cmd, 0, { args: a });
      case "fab_prepare":
        return this.enqueue(cmd, 0, { args: a }, "fab");

      case "write_export": {
        const request = a.request as { svg: string };
        const files = (await this.enqueue("export", 0, { request, zip: true })) as { name: string; data: Uint8Array }[];
        const stem = (this.source?.name ?? "image").replace(/\.[^.]+$/, "") || "image";
        if (files.length === 1 && files[0].name) {
          download(files[0].name, files[0].data);
          return [files[0].name];
        }
        const name = `${stem}-inkvec.zip`;
        if (files[0]) download(name, files[0].data, "application/zip");
        return [name];
      }
      case "read_text_file":
        throw new Error("A browser reads files you choose or drop, not paths.");
      case "save_bytes": {
        const path = String(a.path);
        download(path.split(/[\\/]/).pop() || "download", new Uint8Array(a.bytes as number[]));
        return null;
      }

      case "batch_scan":
      case "batch_start":
      case "batch_stats_csv":
        throw new Error(UNAVAILABLE);
      case "batch_pause":
      case "batch_cancel":
        return null;

      case "denoiser_status":
        return this.denoiserStatus();
      case "denoiser_download":
        return this.downloadDenoiser();
      case "denoiser_cancel":
        // The fetch runs to its end inside ONNX Runtime's loader and lands in the cache;
        // what cancelling does is stop waiting for it, as the desktop's cancel does.
        if (this.downloading) {
          this.downloading = false;
          this.emit("denoiser:done", { ok: false, message: "Cancelled.", status: await this.denoiserStatus() });
        }
        return null;
      case "denoiser_remove":
        try {
          await caches.delete(DENOISER_CACHE);
        } catch {
          // No Cache Storage (a private window): there was nothing kept to remove.
        }
        return this.denoiserStatus();

      case "load_prefs":
        return this.loadPrefs();
      case "save_prefs": {
        const clean = (await this.enqueue("sanitise_prefs", 0, { args: { prefs: a.prefs } })) as Record<string, unknown>;
        this.prefs = clean;
        this.storePrefs();
        return clean;
      }
      case "reset_prefs": {
        try {
          localStorage.removeItem(PREFS_KEY);
        } catch {
          // Storage blocked: nothing was kept.
        }
        this.prefs = (await this.enqueue("default_prefs", 0, { args: {} })) as Record<string, unknown>;
        return this.prefs;
      }

      case "third_party_notices": {
        const parts = await Promise.all(
          ["STUDIO_THIRD_PARTY.md", "THIRD_PARTY.md"].map((n) =>
            fetch(new URL(n, this.base())).then((r) => (r.ok ? r.text() : null), () => null),
          ),
        );
        const got = parts.filter((p): p is string => Boolean(p));
        if (!got.length) throw new Error("the notices could not be fetched");
        return got.join("\n\n\n");
      }
      case "check_update":
        return {
          latest: null,
          newer: false,
          url: "https://github.com/logolabs/inkvec/releases",
          offline: "a web page is always the version it is served as",
        };

      case "cli_status":
      case "context_menu_status":
      case "install_cli":
      case "remove_cli":
      case "install_context_menu":
      case "remove_context_menu":
        return { available: false, installed: false, path: null, note: UNAVAILABLE };
    }
    throw new Error(`${cmd} is not something the browser build does`);
  }

  private bump(): number {
    this.generation++;
    if (this.shared) Atomics.store(this.shared, 0, this.generation);
    return this.generation;
  }

  private async open(bytes: Uint8Array, name: string | null): Promise<unknown> {
    // Kept for a crash, so a fresh worker can be handed the same image.
    this.source = { bytes: bytes.slice(), name };
    this.bump();
    this.queue = this.queue.filter((j) => (j.kind ? (j.resolve(null), false) : true));
    return this.enqueue("open_bytes", 0, { bytes, name });
  }

  // -------------------------------------------------------------- preferences ---

  private async loadPrefs(): Promise<Record<string, unknown>> {
    if (this.prefs) return this.prefs;
    let stored: unknown = null;
    try {
      const text = localStorage.getItem(PREFS_KEY);
      stored = text ? JSON.parse(text) : null;
    } catch {
      stored = null;
    }
    this.prefs = (await (stored
      ? this.enqueue("sanitise_prefs", 0, { args: { prefs: stored } })
      : this.enqueue("default_prefs", 0, { args: {} }))) as Record<string, unknown>;
    return this.prefs;
  }

  /** The desktop remembers the settings a trace asked for; so does the tab. */
  private rememberTrace(settings: Record<string, unknown>): void {
    if (!this.prefs) return;
    const keep = { ...settings };
    delete keep.colourGroups;
    this.prefs = { ...this.prefs, trace: keep };
    window.clearTimeout(this.prefsTimer);
    this.prefsTimer = window.setTimeout(() => this.storePrefs(), 1000);
  }

  private storePrefs(): void {
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify(this.prefs));
    } catch {
      // Storage full or blocked: the session still works, it just starts fresh next time.
    }
  }

  // ------------------------------------------------------------------ denoiser ---

  private startDenoiser(url: string, sha256: string): void {
    if (this.denoiser || !this.denoiserPort) return;
    const w = new Worker(new URL("./denoiser.worker.ts", import.meta.url), { type: "module", name: "inkvec-denoiser" });
    this.denoiser = w;
    w.onmessage = async (e: MessageEvent) => {
      const m = e.data;
      if (!this.downloading) return;
      if (m.type === "progress") this.emit("denoiser:progress", { got: m.got, total: m.total });
      if (m.type === "done") {
        this.downloading = false;
        this.emit("denoiser:done", { ok: m.ok, message: m.message, status: await this.denoiserStatus() });
      }
    };
    w.postMessage(
      { type: "init", base: this.base(), token: __INKVEC_WASM_TOKEN__, url, sha256, port: this.denoiserPort },
      [this.denoiserPort],
    );
  }

  private async denoiserStatus() {
    const ready = await this.whenReady();
    let installed = false;
    let bytes: number | null = null;
    try {
      const hit = await (await caches.open(DENOISER_CACHE)).match(ready.denoiserUrl);
      if (hit) {
        installed = true;
        bytes = Number(hit.headers.get("content-length")) || (await hit.blob()).size;
      }
    } catch {
      // No Cache Storage: never installed, downloaded again each visit.
    }
    return {
      // The trace waits for the network on a shared buffer, which needs isolation.
      supported: ready.isolated,
      installed,
      path: null,
      bytes,
      repo: ready.denoiserRepo,
      sha256: ready.denoiserSha256,
    };
  }

  private async downloadDenoiser(): Promise<null> {
    await this.whenReady();
    if (!this.denoiser) throw new Error("The denoiser needs a cross-origin isolated page.");
    this.downloading = true;
    this.denoiser.postMessage({ type: "download" });
    return null;
  }
}

let backend: WebBackend | null = null;

/** The one backend of this tab. */
export function webBackend(): WebBackend {
  if (!backend) {
    backend = new WebBackend();
    // For the smoke test and anyone curious in the console: the backend, its thread count
    // and the last job's time and memory. Nothing in the app reads it.
    (globalThis as { __inkvecStudioLite?: WebBackend }).__inkvecStudioLite = backend;
  }
  return backend;
}
