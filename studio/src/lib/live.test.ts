import { afterEach, describe, expect, it, vi } from "vitest";

import { applyProgress, clock, elapsedText, LOG_CAP, runningLabel, stamp, startClock, stepText, type LiveState } from "./live";

const empty = (): LiveState => ({ liveStages: [], liveNow: null, traceLog: [] });

describe("applyProgress", () => {
  it("opens a stage when it begins and closes it, with its time, when it ends", () => {
    let st = applyProgress(empty(), { entries: [{ kind: "begin", stage: "fit", what: "fitting curves", at: 10 }], step: null }, 1000);
    expect(st.liveNow).toEqual({ stage: "fit", what: "fitting curves", since: 1000, step: null });
    expect(st.traceLog).toEqual([{ at: 10, kind: "stage", stage: "fit", what: "fitting curves" }]);

    st = applyProgress(st, { entries: [{ kind: "end", stage: "fit", what: "fitting curves", ms: 42, at: 52 }], step: null }, 1042);
    expect(st.liveNow).toBeNull();
    expect(st.liveStages).toEqual([{ name: "fit", ms: 42 }]);
    expect(st.traceLog).toEqual([{ at: 10, kind: "stage", stage: "fit", what: "fitting curves", ms: 42, step: null }]);
  });

  it("adds up the times of internal stages that share one of the nine names", () => {
    let st = empty();
    st = applyProgress(st, { entries: [{ kind: "end", stage: "colour", what: "palette", ms: 5, at: 5 }], step: null }, 0);
    st = applyProgress(st, { entries: [{ kind: "end", stage: "colour", what: "merge", ms: 7, at: 12 }], step: null }, 0);
    expect(st.liveStages).toEqual([{ name: "colour", ms: 12 }]);
  });

  it("logs a stage that ended without a begin, dated from its start", () => {
    const st = applyProgress(empty(), { entries: [{ kind: "end", stage: "fit", what: "x", ms: 30, at: 20 }], step: null }, 0);
    expect(st.traceLog).toEqual([{ at: 0, kind: "stage", stage: "fit", what: "x", ms: 30 }]);
  });

  it("keeps the measurement out of the stage list but in the log", () => {
    const st = applyProgress(empty(), { entries: [{ kind: "end", stage: "measure", what: "dE00", ms: 9, at: 9 }], step: null }, 0);
    expect(st.liveStages).toEqual([]);
    expect(st.traceLog).toHaveLength(1);
  });

  it("logs a note as it comes", () => {
    const st = applyProgress(empty(), { entries: [{ kind: "note", stage: "colour", what: "palette", text: "8 inks", at: 3 }], step: null }, 0);
    expect(st.traceLog).toEqual([{ at: 3, kind: "note", stage: "colour", what: "palette", text: "8 inks" }]);
  });

  it("puts a loop's count on the running stage and its open log line", () => {
    const step = { stage: "fit", what: "fitting curves", unit: "boundaries fitted", done: 212, total: 530 };
    let st = applyProgress(empty(), { entries: [{ kind: "begin", stage: "fit", what: "fitting curves", at: 0 }], step: null }, 100);
    st = applyProgress(st, { entries: [], step }, 200);
    // Same stage: the running clock keeps its start.
    expect(st.liveNow).toEqual({ stage: "fit", what: "fitting curves", since: 100, step });
    expect(st.traceLog[0].step).toEqual(step);
    // A count for a stage nobody announced starts a running line of its own.
    const other = { ...step, what: "boundary solve", unit: "iterations" };
    st = applyProgress(st, { entries: [], step: other }, 300);
    expect(st.liveNow).toEqual({ stage: "fit", what: "boundary solve", since: 300, step: other });
  });

  it("never changes the state it was given", () => {
    const before = empty();
    const frozen = JSON.stringify(before);
    applyProgress(before, { entries: [{ kind: "begin", stage: "fit", what: "x", at: 0 }], step: null }, 0);
    expect(JSON.stringify(before)).toBe(frozen);
  });

  it("keeps only the last LOG_CAP lines", () => {
    const entries = Array.from({ length: LOG_CAP + 5 }, (_, i) => ({ kind: "note" as const, stage: "s", what: "w", text: String(i), at: i }));
    const st = applyProgress(empty(), { entries, step: null }, 0);
    expect(st.traceLog).toHaveLength(LOG_CAP);
    expect(st.traceLog[0].text).toBe("5");
  });
});

describe("formatting", () => {
  it("writes a loop's count with or without a known total", () => {
    expect(stepText({ stage: "s", what: "w", unit: "boundaries fitted", done: 212, total: 530 })).toBe("212 / 530 boundaries fitted");
    expect(stepText({ stage: "s", what: "w", unit: "merge rounds", done: 14, total: 0 })).toBe("merge rounds 14");
  });

  it("shows tenths of a second under a minute, then minutes and padded seconds", () => {
    expect([clock(-5), clock(0), clock(1234), clock(59_949)]).toEqual(["0.0 s", "0.0 s", "1.2 s", "59.9 s"]);
    expect([clock(60_000), clock(125_500), clock(3_600_000)]).toEqual(["1 min 00 s", "2 min 05 s", "60 min 00 s"]);
  });

  it("stamps a log line in seconds to the hundredth", () => {
    expect([stamp(1234), stamp(-1)]).toEqual(["1.23", "0.00"]);
  });

  it("names the running stage, or the last one that finished", () => {
    const step = { stage: "fit", what: "fitting", unit: "iterations", done: 7, total: 48 };
    expect(runningLabel({ liveNow: { stage: "fit", what: "fitting", since: 0, step }, liveStages: [] })).toBe("fit · 7 / 48 iterations");
    expect(runningLabel({ liveNow: { stage: "measure", what: "dE00", since: 0, step: null }, liveStages: [] })).toBe("measuring · dE00");
    expect(runningLabel({ liveNow: null, liveStages: [{ name: "colour", ms: 3 }] })).toBe("colour");
    expect(runningLabel({ liveNow: null, liveStages: [] })).toBe("starting");
  });
});

describe("startClock", () => {
  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("writes the elapsed counters ten times a second while tracing, and stops after", () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout", "performance"] });
    document.body.innerHTML = '<span data-live="elapsed"></span><span data-live="stage"></span>';
    const elapsed = document.querySelector<HTMLElement>('[data-live="elapsed"]')!;
    const stage = document.querySelector<HTMLElement>('[data-live="stage"]')!;
    let tracing = true;
    const started = performance.now();
    let stageSince: number | null = null;
    const sync = startClock(
      () => tracing,
      () => started,
      () => stageSince,
    );

    sync();
    expect(elapsed.textContent).toBe("0.0 s");
    expect(stage.textContent).toBe("");
    stageSince = performance.now();
    vi.advanceTimersByTime(1500);
    expect(elapsed.textContent).toBe("1.5 s");
    expect(stage.textContent).toBe("1.5 s");
    expect(elapsedText({ traceStarted: started })).toBe("1.5 s");

    tracing = false;
    sync();
    vi.advanceTimersByTime(2000);
    expect(elapsed.textContent).toBe("1.5 s");
  });
});
