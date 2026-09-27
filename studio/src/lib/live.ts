/**
 * What a running trace is doing, as the engine says it: the reducer behind the live stage
 * list, the running-stage line and the activity log, and the one timer that keeps the
 * elapsed counters moving.
 *
 * The engine reports in `trace:progress` events (the core's `trace::live`): a stage began,
 * a stage ended after so many milliseconds, a note ("8 inks"), and how far the running
 * stage's loop has got ("212 of 530 boundaries fitted"). Nothing here is invented. The one
 * thing the interface adds is the clock: elapsed time counts up from the moment the trace
 * was asked for, ten times a second, written straight into the elements that show it, so a
 * long stage never looks frozen and the rest of the interface is not rebuilt to do it.
 */

import type { Stage } from "./ipc";
import type { State } from "./state";

/** One entry of a `trace:progress` event. `at` is milliseconds since the trace started. */
export type LiveEntry =
  | { kind: "begin"; stage: string; what: string; at: number }
  | { kind: "end"; stage: string; what: string; ms: number; at: number }
  | { kind: "note"; stage: string; what: string; text: string; at: number };

/** How far the running stage's loop has got; `total` is 0 when the loop cannot know. */
export interface LiveStep {
  stage: string;
  what: string;
  unit: string;
  done: number;
  total: number;
}

/** A `trace:progress` event. */
export interface TraceProgress {
  generation: number;
  entries: LiveEntry[];
  step: LiveStep | null;
}

/** What the engine is doing right now. */
export interface LiveNow {
  /** The interface's stage name (one of the nine, or `measure`). */
  stage: string;
  /** The few words the core gives it: "fitting curves". */
  what: string;
  /** When it started, on this page's clock (`performance.now()`). */
  since: number;
  step: LiveStep | null;
}

/** One line of the activity log. */
export interface LogLine {
  /** Milliseconds since the trace started, on the engine's clock. */
  at: number;
  kind: "stage" | "note";
  stage: string;
  what: string;
  /** A stage's duration once it has ended; absent while it runs. */
  ms?: number;
  /** A running stage's loop count, while it runs. */
  step?: LiveStep | null;
  /** A note's text. */
  text?: string;
}

/** The part of the interface's state this reducer owns. */
export interface LiveState {
  liveStages: Stage[];
  liveNow: LiveNow | null;
  traceLog: LogLine[];
}

/** The most lines a log keeps; a trace writes a few dozen. */
export const LOG_CAP = 400;

/**
 * Fold one `trace:progress` event into the live state. `now` is `performance.now()` when it
 * arrived. Returns new arrays and objects, never mutated ones, so a store sees the change.
 */
export function applyProgress(state: LiveState, event: Pick<TraceProgress, "entries" | "step">, now: number): LiveState {
  let liveStages = state.liveStages;
  let liveNow = state.liveNow;
  const log = [...state.traceLog];
  const openLine = (what: string) => {
    for (let i = log.length - 1; i >= 0; i--) {
      const l = log[i];
      if (l.kind === "stage") return l.what === what && l.ms === undefined ? i : -1;
    }
    return -1;
  };

  for (const e of event.entries) {
    if (e.kind === "begin") {
      liveNow = { stage: e.stage, what: e.what, since: now, step: null };
      log.push({ at: e.at, kind: "stage", stage: e.stage, what: e.what });
    } else if (e.kind === "end") {
      // The nine names fold several internal stages each, so their times add up, as the
      // stage list has always shown them. The measurement is not one of the nine.
      if (e.stage !== "measure") {
        const at = liveStages.findIndex((s) => s.name === e.stage);
        liveStages =
          at >= 0
            ? liveStages.map((s, i) => (i === at ? { ...s, ms: s.ms + e.ms } : s))
            : [...liveStages, { name: e.stage, ms: e.ms }];
      }
      const i = openLine(e.what);
      if (i >= 0) log[i] = { ...log[i], ms: e.ms, step: log[i].step ?? null };
      else log.push({ at: Math.max(0, e.at - e.ms), kind: "stage", stage: e.stage, what: e.what, ms: e.ms });
      if (liveNow && liveNow.what === e.what) liveNow = null;
    } else {
      log.push({ at: e.at, kind: "note", stage: e.stage, what: e.what, text: e.text });
    }
  }

  const step = event.step;
  if (step) {
    if (!liveNow || liveNow.what !== step.what) liveNow = { stage: step.stage, what: step.what, since: now, step };
    else liveNow = { ...liveNow, step };
    const i = openLine(step.what);
    if (i >= 0) log[i] = { ...log[i], step };
  }

  return {
    liveStages,
    liveNow,
    traceLog: log.length > LOG_CAP ? log.slice(log.length - LOG_CAP) : log,
  };
}

/** "212 / 530 boundaries fitted", "merge rounds 14". */
export function stepText(step: LiveStep): string {
  return step.total > 0 ? `${step.done} / ${step.total} ${step.unit}` : `${step.unit} ${step.done}`;
}

/** Elapsed time as the counters show it: tenths under a minute, then minutes and seconds. */
export function clock(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  return `${m} min ${String(Math.floor(s % 60)).padStart(2, "0")} s`;
}

/** A log line's timestamp: seconds since the trace started, to the hundredth. */
export function stamp(ms: number): string {
  return `${(Math.max(0, ms) / 1000).toFixed(2)}`;
}

/**
 * Keep every live counter on the page moving while a trace runs.
 *
 * Elements carrying `data-live="elapsed"` show the time since `started()`; elements with
 * `data-live="stage"` the time the running stage has taken. Written in place ten times a
 * second, so nothing is rebuilt for it and a button under the pointer stays the same button.
 */
export function startClock(tracing: () => boolean, started: () => number, stageSince: () => number | null): () => void {
  let timer = 0;
  const tick = () => {
    const now = performance.now();
    const t0 = started();
    const s0 = stageSince();
    for (const el of document.querySelectorAll<HTMLElement>('[data-live="elapsed"]')) {
      el.textContent = clock(now - t0);
    }
    for (const el of document.querySelectorAll<HTMLElement>('[data-live="stage"]')) {
      el.textContent = s0 === null ? "" : clock(now - s0);
    }
  };
  const sync = () => {
    if (tracing() && !timer) {
      tick();
      timer = window.setInterval(tick, 100);
    } else if (!tracing() && timer) {
      window.clearInterval(timer);
      timer = 0;
    }
  };
  return () => {
    sync();
    if (timer) tick();
  };
}

/** The running stage in a few words, for the one-line readouts: "boundary solve · 7 / 48 iterations". */
export function runningLabel(st: Pick<State, "liveNow" | "liveStages">): string {
  const now = st.liveNow;
  if (now) {
    const name = now.stage === "measure" ? "measuring" : now.stage;
    return now.step ? `${name} · ${stepText(now.step)}` : `${name} · ${now.what}`;
  }
  return st.liveStages[st.liveStages.length - 1]?.name ?? "starting";
}

/** Time since the trace in flight was asked for, as the counters show it. */
export function elapsedText(st: Pick<State, "traceStarted">): string {
  return clock(performance.now() - st.traceStarted);
}
