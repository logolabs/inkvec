/**
 * The trace loop's bookkeeping, with no DOM, store or backend in it: which trace the
 * interface is waiting for, what to do with each event the backend sends, and the timer that
 * turns a moved control into a full trace once the controls have been still.
 *
 * `main.ts` owns the loop and wires it to the store and to `lib/ipc.ts` through a
 * `TraceHost`; everything here can be driven by a test with promises it resolves in any
 * order it likes (`traceflow.test.ts`).
 *
 * **Generations.** Every trace the backend starts gets a number one higher than the last
 * (`start_trace` returns it), and every event it sends for that trace carries it. The
 * interface only ever shows the newest trace it has been handed, so each event is checked
 * against that number. Three races make this less simple than it sounds:
 *
 * - *Start replies that cross.* A draft and a full trace are started a moment apart; the
 *   backend may answer the second before the first. The older reply must not put its
 *   generation back in charge, or the newer trace's result would be dropped as stale and
 *   the interface would wait for ever. So `newest` only goes up, and a reply older than it
 *   is ignored.
 * - *A result before its start reply.* A trace served from the backend's cache can finish
 *   before the reply to the command that started it reaches the page. Its progress and its
 *   result are held (`early`), and applied, in the order they came, the moment the
 *   generation is known.
 * - *Cancel undone by a queued trace.* Moving a control starts a draft at once and arms a
 *   timer for the full trace. A Cancel that the timer then undoes 800 ms later is not a
 *   cancel, so Cancel disarms the timer too; so does opening another image.
 */

/** Which kind of trace: a small, quick one that keeps up with a moving control, or the full one. */
export type Tier = "draft" | "final";

/** The trace the interface is showing, as the store has it. */
export interface Shown {
  /** The generation the interface is waiting for or showing; 0 before the first trace. */
  generation: number;
  /** Whether a trace is running as far as the interface is concerned. */
  tracing: boolean;
}

/**
 * What the loop needs from the app: the backend's two commands and the store's reactions.
 *
 * `P` is a progress event, `O` an outcome and `G` the colour groups a trace was sent with.
 * The loop never looks inside any of them except for a progress event's generation.
 */
export interface TraceHost<P extends { generation: number }, O, G> {
  /** Whether an image is open; with none there is nothing to trace. */
  hasSource(): boolean;
  /** The colour groups the next trace is sent with, read before it is started. */
  groups(): G;
  /** Ask the backend for a trace; resolves to its generation, rejects if it cannot start. */
  start(tier: Tier): Promise<number>;
  /** How long the controls must be still before the full trace, in milliseconds. */
  settleMs(): number;
  /** The generation the store shows and whether it is tracing, for filtering a result. */
  shown(): Shown;
  /**
   * The backend has handed back `generation` and it is the newest: the interface now waits
   * for it. `askedAt` is `performance.now()` when the trace was asked for, before the reply.
   */
  began(generation: number, tier: Tier, askedAt: number): void;
  /** A progress event for the trace being watched. */
  progressed(p: P): void;
  /** The watched trace's outcome. */
  finished(outcome: O, generation: number): void;
  /** Starting the trace failed (or the code reacting to its start threw). */
  failed(error: unknown): void;
  /** Stop the backend's trace and put the interface back to the last result. */
  cancel(): void;
}

/** Events that arrived for a generation before its start reply did. */
interface Held<P, O> {
  progress: P[];
  done?: O;
}

/**
 * How many generations back the colour groups a trace was sent with are kept. A result
 * older than this is never shown, so its groups are never asked for.
 */
const GROUPS_KEPT = 8;

/**
 * The trace loop: generations, early events, the colour groups each trace was sent with,
 * and the settle timer. One per app.
 */
export class TraceLoop<P extends { generation: number }, O, G> {
  /** The generation whose stages are being collected; 0 for none (another image opened). */
  private watching = 0;
  /** The newest generation the backend has handed back. Never goes down. */
  private newest = 0;
  /** What arrived for a generation newer than `newest`, waiting for its start reply. */
  private readonly early = new Map<number, Held<P, O>>();
  /** The colour groups each recent trace was sent with, by generation. */
  private readonly groupsSent = new Map<number, G>();
  /** The pending full trace a moved control armed, if any. */
  private settleTimer: ReturnType<typeof setTimeout> | undefined = undefined;

  constructor(private readonly host: TraceHost<P, O, G>) {}

  /**
   * Start a trace and, once the backend has said which generation it is, make it the one
   * the interface waits for, unless a newer one has already taken over. Events that beat
   * the reply are applied then, in the order they came. Never rejects: a failure to start
   * goes to `host.failed`.
   */
  async trace(tier: Tier): Promise<void> {
    if (!this.host.hasSource()) return;
    // Counted from the moment it was asked for: that is the wait the user sees.
    const asked = performance.now();
    try {
      const groups = this.host.groups();
      const generation = await this.host.start(tier);
      // A newer trace was started meanwhile and has already taken over.
      if (generation < this.newest) return;
      this.newest = generation;
      this.groupsSent.set(generation, groups);
      for (const old of this.groupsSent.keys()) if (old < generation - GROUPS_KEPT) this.groupsSent.delete(old);
      this.watching = generation;
      this.host.began(generation, tier, asked);
      // Anything that beat the reply here, in the order it came. Anything held for an older
      // generation is now stale and goes.
      const held = this.early.get(generation);
      for (const g of this.early.keys()) if (g <= generation) this.early.delete(g);
      for (const p of held?.progress ?? []) this.progressed(p);
      if (held?.done) this.host.finished(held.done, generation);
    } catch (e) {
      this.host.failed(e);
    }
  }

  /** A `trace:progress` event from the backend. */
  onProgress(p: P): void {
    if (p.generation > this.newest) {
      this.hold(p.generation).progress.push(p);
      return;
    }
    this.progressed(p);
  }

  /**
   * A `trace:done` event from the backend. Held if its start reply has not come back yet;
   * applied only if it is the trace the interface is showing and still waiting for.
   */
  onDone(generation: number, outcome: O): void {
    if (generation > this.newest) {
      this.hold(generation).done = outcome;
      return;
    }
    const shown = this.host.shown();
    if (generation !== shown.generation || !shown.tracing) return;
    this.host.finished(outcome, generation);
  }

  /** A control moved: a draft now, and the full trace once the controls have been still. */
  controlChanged(): void {
    this.clearSettle();
    if (!this.host.hasSource()) return;
    void this.trace("draft");
    this.settleTimer = setTimeout(() => void this.trace("final"), this.host.settleMs());
  }

  /**
   * Stop the trace in flight and go back to the last result. The full trace a draft armed
   * goes too: a Cancel that a timer undoes 800 ms later is not a cancel.
   */
  cancel(): void {
    this.clearSettle();
    this.host.cancel();
  }

  /** Drop the full trace a moved control armed, for a caller about to start one itself. */
  clearSettle(): void {
    clearTimeout(this.settleTimer);
    this.settleTimer = undefined;
  }

  /**
   * Another image is being opened: whatever was tracing belongs to the one being replaced,
   * nothing it still sends is wanted, and nor is a full trace a draft of it had armed.
   */
  detach(): void {
    this.clearSettle();
    this.watching = 0;
  }

  /** The colour groups the trace `generation` was sent with, if it is recent enough to be kept. */
  groupsFor(generation: number): G | undefined {
    return this.groupsSent.get(generation);
  }

  /** Pass a progress event on if it belongs to the trace being watched. */
  private progressed(p: P): void {
    if (p.generation !== this.watching) return;
    this.host.progressed(p);
  }

  /** The events held for `generation`, created empty on first use. */
  private hold(generation: number): Held<P, O> {
    const held = this.early.get(generation) ?? { progress: [] };
    this.early.set(generation, held);
    return held;
  }
}

/**
 * Tickets for "only the newest of these asynchronous jobs may land": a job takes a ticket
 * when it starts and checks it when it finishes. Used for a finished trace whose snapped
 * colours are painted asynchronously, so a slow paint cannot overwrite a newer drawing.
 */
export class Latest {
  private last = 0;

  /** A ticket for a job starting now. It supersedes every ticket taken before it. */
  take(): number {
    return ++this.last;
  }

  /** Whether `ticket` is still the newest one taken. */
  isNewest(ticket: number): boolean {
    return ticket === this.last;
  }
}
