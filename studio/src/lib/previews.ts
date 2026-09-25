/**
 * The wizard's preview traces: small drafts of this image under settings nobody has chosen
 * yet, one at a time.
 *
 * Every trace goes through one slot in the backend, and the drawing on screen always comes
 * first: a preview waits behind any trace the viewer is waiting for, and never retires it
 * (`start_preview` in `lib.rs`). So the queue lives here and sends one preview at a time —
 * the next only when the last has answered — which keeps a slider moved meanwhile at most
 * one small draft behind, and lets a queue that is no longer wanted be dropped without
 * anything to cancel in the backend. What was traced is kept by settings for the image, so
 * going back to a step shows its previews at once.
 */

import { api, events, type Outcome, type Report, type Settings } from "./ipc";

export type Preview =
  | { state: "queued" }
  | { state: "running" }
  | { state: "done"; svg: string; url: string; report: Report }
  | { state: "failed"; message: string };

export interface PreviewJob {
  key: string;
  settings: Settings;
}

/**
 * Two settings that draw the same draft have the same key. A draft never runs the denoiser
 * (`Settings::draft`), so "Clean up damage" is not part of it; key order is fixed so two
 * objects spelt differently compare equal.
 */
export function previewKey(settings: Settings): string {
  const plain: Record<string, unknown> = { ...settings, cleanUpDamage: "off" };
  return JSON.stringify(Object.keys(plain).sort().map((k) => [k, plain[k]]));
}

export class Previews {
  private results = new Map<string, Preview>();
  private queue: PreviewJob[] = [];
  private running: { key: string; generation: number | null; epoch: number } | null = null;
  /** Bumped when the image changes: nothing asked for before then is wanted after. */
  private epoch = 0;
  /** Results that arrived before their generation did (the command's reply is a hop behind). */
  private early = new Map<number, Outcome>();
  private listeners = new Set<() => void>();

  constructor() {
    void events.previewDone(({ generation, outcome }) => this.done(generation, outcome));
  }

  /** What is known about `key`: nothing yet, queued, running, or its result. */
  get(key: string): Preview | undefined {
    return this.results.get(key);
  }

  /** Call `fn` whenever a preview changes state; returns an unsubscribe. */
  on(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /**
   * Trace these, in this order, replacing whatever was still waiting. Anything already
   * traced or running is not asked for again.
   */
  want(jobs: PreviewJob[]): void {
    // Asked for again by a render that a preview landing caused: nothing new, nothing to say.
    const seen = new Set<string>();
    const missing = jobs.filter((j) => {
      if (seen.has(j.key)) return false;
      seen.add(j.key);
      const known = this.results.get(j.key)?.state;
      return known !== "done" && known !== "running";
    });
    if (missing.map((j) => j.key).join("\n") === this.queue.map((j) => j.key).join("\n")) return;
    for (const job of this.queue) {
      if (this.results.get(job.key)?.state === "queued") this.results.delete(job.key);
    }
    this.queue = missing;
    for (const job of this.queue) this.results.set(job.key, { state: "queued" });
    this.emit();
    this.next();
  }

  /** Stop sending what is still waiting; the one running finishes and is kept. */
  pause(): void {
    this.want([]);
  }

  /** A new image: forget everything, and retire whatever the backend still has. */
  reset(): void {
    this.epoch++;
    for (const p of this.results.values()) if (p.state === "done") URL.revokeObjectURL(p.url);
    this.results.clear();
    this.queue = [];
    this.early.clear();
    if (this.running) void api.cancelPreviews().catch(() => {});
    this.running = null;
    this.emit();
  }

  /** How many are queued or running, for a progress line. */
  pending(): number {
    return this.queue.length + (this.running ? 1 : 0);
  }

  private next(): void {
    if (this.running) return;
    const job = this.queue.shift();
    if (!job) return;
    const epoch = this.epoch;
    this.running = { key: job.key, generation: null, epoch };
    this.results.set(job.key, { state: "running" });
    this.emit();
    api.startPreview(job.settings).then(
      (generation) => {
        if (this.running?.epoch !== epoch || this.running.key !== job.key) return;
        this.running.generation = generation;
        const early = this.early.get(generation);
        if (early) {
          this.early.delete(generation);
          this.done(generation, early);
        }
      },
      (e) => {
        if (this.running?.epoch !== epoch) return;
        this.finish(job.key, { state: "failed", message: String(e) });
      },
    );
  }

  private done(generation: number, outcome: Outcome): void {
    const run = this.running;
    if (!run) return;
    if (run.generation === null) {
      // The event beat the command's reply; hold it until the generation is known.
      this.early.set(generation, outcome);
      return;
    }
    if (run.generation !== generation) return;
    if (outcome.state === "traced") {
      const url = URL.createObjectURL(new Blob([outcome.svg], { type: "image/svg+xml" }));
      this.finish(run.key, { state: "done", svg: outcome.svg, url, report: outcome.report });
    } else {
      const message = "message" in outcome ? outcome.message : outcome.state === "flat" ? "one flat colour" : "out of memory";
      this.finish(run.key, { state: "failed", message });
    }
  }

  private finish(key: string, result: Preview): void {
    this.results.set(key, result);
    this.running = null;
    this.emit();
    this.next();
  }

  private emit(): void {
    for (const fn of [...this.listeners]) fn();
  }
}
