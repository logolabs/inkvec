import { afterEach, describe, expect, it, vi } from "vitest";

import { Latest, TraceLoop, type Tier, type TraceHost } from "./traceflow";

/** A progress event: a generation and a sequence number to tell them apart in the log. */
interface P {
  generation: number;
  n: number;
}

/** A promise with its resolve and reject, for a start reply a test answers when it likes. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Let every pending promise continuation run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

/**
 * A loop over a host that behaves like the store in `main.ts`: `began` makes a generation the
 * one shown and tracing, and a finished, failed or cancelled trace stops tracing. Everything
 * the loop asks of it is written to `log`; every start waits in `starts` for its reply.
 */
function harness() {
  const log: string[] = [];
  const starts: { tier: Tier; fast: boolean; groups: string[]; reply: ReturnType<typeof deferred<number>> }[] = [];
  const shown = { generation: 0, tracing: false };
  const world = { source: true, groups: [] as string[] };
  const host: TraceHost<P, string, string[]> = {
    hasSource: () => world.source,
    groups: () => world.groups,
    start: (tier, fast) => {
      const reply = deferred<number>();
      starts.push({ tier, fast, groups: world.groups, reply });
      return reply.promise;
    },
    settleMs: () => 800,
    shown: () => ({ ...shown }),
    began: (generation, tier) => {
      shown.generation = generation;
      shown.tracing = true;
      log.push(`began ${generation} ${tier}`);
    },
    progressed: (p) => log.push(`progress ${p.generation}.${p.n}`),
    finished: (outcome, generation) => {
      shown.tracing = false;
      log.push(`finished ${generation} ${outcome}`);
    },
    failed: (e) => {
      shown.tracing = false;
      log.push(`failed ${e instanceof Error ? e.message : String(e)}`);
    },
    cancel: () => {
      shown.tracing = false;
      log.push("cancel");
    },
  };
  return { loop: new TraceLoop(host), host, log, starts, shown, world };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("TraceLoop: generations", () => {
  it("watches a trace once its generation comes back, and applies its progress and result", async () => {
    const { loop, log, starts } = harness();
    const done = loop.trace("final");
    expect(starts).toHaveLength(1);
    expect(log).toEqual([]);
    starts[0].reply.resolve(1);
    await done;
    loop.onProgress({ generation: 1, n: 1 });
    loop.onDone(1, "drawing");
    expect(log).toEqual(["began 1 final", "progress 1.1", "finished 1 drawing"]);
  });

  it("starts nothing while no image is open", async () => {
    const { loop, starts, world } = harness();
    world.source = false;
    await loop.trace("final");
    loop.controlChanged();
    expect(starts).toHaveLength(0);
  });

  it("ignores progress and results of an older generation once a newer one is watched", async () => {
    const { loop, log, starts } = harness();
    const first = loop.trace("draft");
    starts[0].reply.resolve(1);
    await first;
    const second = loop.trace("final");
    starts[1].reply.resolve(2);
    await second;
    loop.onProgress({ generation: 1, n: 1 });
    loop.onDone(1, "old");
    expect(log).toEqual(["began 1 draft", "began 2 final"]);
    loop.onDone(2, "new");
    expect(log[log.length - 1]).toBe("finished 2 new");
  });

  it("drops a result for the shown generation once it has stopped tracing (cancelled)", async () => {
    const { loop, log, starts } = harness();
    const t = loop.trace("final");
    starts[0].reply.resolve(1);
    await t;
    loop.cancel();
    loop.onDone(1, "late");
    expect(log).toEqual(["began 1 final", "cancel"]);
  });

  it("reports a start that fails, and code that throws while taking over, as a failure", async () => {
    const { loop, log, starts, host } = harness();
    const t = loop.trace("final");
    starts[0].reply.reject(new Error("engine gone"));
    await t;
    expect(log).toEqual(["failed engine gone"]);

    host.began = () => {
      throw new Error("listener broke");
    };
    const u = loop.trace("final");
    starts[1].reply.resolve(1);
    await u;
    expect(log[log.length - 1]).toBe("failed listener broke");
  });
});

describe("TraceLoop: start replies that cross", () => {
  it("never lets an older reply take over from a newer one", async () => {
    const { loop, log, starts, shown } = harness();
    const draft = loop.trace("draft");
    const final = loop.trace("final");
    // The backend answers the second command first.
    starts[1].reply.resolve(2);
    await final;
    starts[0].reply.resolve(1);
    await draft;
    expect(log).toEqual(["began 2 final"]);
    expect(shown.generation).toBe(2);
    // So the newer trace's result is still the one applied, not dropped as stale.
    loop.onDone(2, "final drawing");
    expect(log[log.length - 1]).toBe("finished 2 final drawing");
  });

  it("still takes a reply in order when they do not cross", async () => {
    const { loop, log, starts } = harness();
    const draft = loop.trace("draft");
    const final = loop.trace("final");
    starts[0].reply.resolve(1);
    starts[1].reply.resolve(2);
    await Promise.all([draft, final]);
    expect(log).toEqual(["began 1 draft", "began 2 final"]);
  });
});

describe("TraceLoop: a result before its start reply", () => {
  it("holds progress and the result, then applies them in order when the generation is known", async () => {
    const { loop, log, starts } = harness();
    const t = loop.trace("final");
    // A trace served from the cache: everything arrives before the reply.
    loop.onProgress({ generation: 1, n: 1 });
    loop.onProgress({ generation: 1, n: 2 });
    loop.onDone(1, "cached");
    expect(log).toEqual([]);
    starts[0].reply.resolve(1);
    await t;
    expect(log).toEqual(["began 1 final", "progress 1.1", "progress 1.2", "finished 1 cached"]);
  });

  it("discards what was held for an older generation when a newer one takes over", async () => {
    const { loop, log, starts } = harness();
    const a = loop.trace("draft");
    const b = loop.trace("final");
    // Generation 1's events beat both replies.
    loop.onProgress({ generation: 1, n: 1 });
    loop.onDone(1, "draft drawing");
    starts[1].reply.resolve(2);
    await b;
    starts[0].reply.resolve(1);
    await a;
    expect(log).toEqual(["began 2 final"]);
  });

  it("keeps what is held for a newer generation when an older one takes over", async () => {
    const { loop, log, starts } = harness();
    const a = loop.trace("draft");
    const b = loop.trace("final");
    loop.onDone(2, "final drawing");
    starts[0].reply.resolve(1);
    await a;
    expect(log).toEqual(["began 1 draft"]);
    starts[1].reply.resolve(2);
    await b;
    expect(log).toEqual(["began 1 draft", "began 2 final", "finished 2 final drawing"]);
  });
});

describe("TraceLoop: the settle timer and Cancel", () => {
  it("drafts at once and traces in full once the controls have been still", async () => {
    vi.useFakeTimers();
    const { loop, starts } = harness();
    loop.controlChanged();
    expect(starts.map((s) => s.tier)).toEqual(["draft"]);
    vi.advanceTimersByTime(799);
    expect(starts).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(starts.map((s) => s.tier)).toEqual(["draft", "final"]);
  });

  it("restarts the wait when a control moves again", () => {
    vi.useFakeTimers();
    const { loop, starts } = harness();
    loop.controlChanged();
    vi.advanceTimersByTime(500);
    loop.controlChanged();
    vi.advanceTimersByTime(500);
    expect(starts.map((s) => s.tier)).toEqual(["draft", "draft"]);
    vi.advanceTimersByTime(300);
    expect(starts.map((s) => s.tier)).toEqual(["draft", "draft", "final"]);
  });

  it("is not undone by the full trace a moved control had queued", async () => {
    vi.useFakeTimers();
    const { loop, log, starts } = harness();
    loop.controlChanged();
    starts[0].reply.resolve(1);
    await flush();
    loop.cancel();
    vi.advanceTimersByTime(5000);
    expect(starts).toHaveLength(1);
    expect(log).toEqual(["began 1 draft", "cancel"]);
  });

  it("drops the queued full trace for a caller that starts one itself", () => {
    vi.useFakeTimers();
    const { loop, starts } = harness();
    loop.controlChanged();
    loop.clearSettle();
    void loop.trace("final");
    vi.advanceTimersByTime(5000);
    expect(starts.map((s) => s.tier)).toEqual(["draft", "final"]);
  });

  it("stops watching and drops the queued trace when another image is opened", async () => {
    vi.useFakeTimers();
    const { loop, log, starts } = harness();
    loop.controlChanged();
    starts[0].reply.resolve(1);
    await flush();
    loop.detach();
    loop.onProgress({ generation: 1, n: 1 });
    vi.advanceTimersByTime(5000);
    expect(starts).toHaveLength(1);
    expect(log).toEqual(["began 1 draft"]);
  });

  it("lands the full trace, not the draft, when both finish", async () => {
    vi.useFakeTimers();
    const { loop, log, starts } = harness();
    loop.controlChanged();
    starts[0].reply.resolve(1);
    await flush();
    vi.advanceTimersByTime(800);
    starts[1].reply.resolve(2);
    await flush();
    loop.onDone(1, "draft drawing");
    loop.onDone(2, "full drawing");
    expect(log).toEqual(["began 1 draft", "began 2 final", "finished 2 full drawing"]);
  });
});

describe("TraceLoop: a Fast draft first for an opened image", () => {
  /** `open`'s promise, with whether it has settled yet. */
  function opened(loop: TraceLoop<P, string, string[]>, fastFirst: boolean) {
    const state = { settled: false };
    const promise = loop.open(fastFirst).then(() => {
      state.settled = true;
    });
    return { state, promise };
  }

  it("draws the Fast draft, then asks for the full trace the moment the draft lands", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    expect(starts.map((s) => [s.tier, s.fast])).toEqual([["draft", true]]);
    starts[0].reply.resolve(1);
    await flush();
    expect(loop.waitingForDraft).toBe(true);
    expect(o.state.settled).toBe(false);
    loop.onDone(1, "fast drawing");
    expect(starts.map((s) => [s.tier, s.fast])).toEqual([["draft", true], ["final", false]]);
    starts[1].reply.resolve(2);
    await o.promise;
    expect(log).toEqual(["began 1 draft", "finished 1 fast drawing", "began 2 final"]);
    expect(loop.waitingForDraft).toBe(false);
    loop.onDone(2, "full drawing");
    expect(log[log.length - 1]).toBe("finished 2 full drawing");
  });

  it("is the full trace alone without a draft", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, false);
    expect(starts.map((s) => [s.tier, s.fast])).toEqual([["final", false]]);
    starts[0].reply.resolve(1);
    await o.promise;
    expect(log).toEqual(["began 1 final"]);
  });

  it("asks for the full trace whatever the draft found, a failure or a flat image included", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    starts[0].reply.resolve(1);
    await flush();
    loop.onDone(1, "failed");
    expect(starts.map((s) => s.tier)).toEqual(["draft", "final"]);
    starts[1].reply.resolve(2);
    await o.promise;
    expect(log).toEqual(["began 1 draft", "finished 1 failed", "began 2 final"]);
  });

  it("takes a draft that lands before its start reply, and still follows it with the full trace", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    loop.onDone(1, "cached fast drawing");
    starts[0].reply.resolve(1);
    await flush();
    expect(starts.map((s) => s.tier)).toEqual(["draft", "final"]);
    starts[1].reply.resolve(2);
    await o.promise;
    expect(log).toEqual(["began 1 draft", "finished 1 cached fast drawing", "began 2 final"]);
  });

  it("does not follow a cancelled draft with the full trace", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    starts[0].reply.resolve(1);
    await flush();
    loop.cancel();
    await o.promise;
    loop.onDone(1, "late fast drawing");
    expect(starts).toHaveLength(1);
    expect(log).toEqual(["began 1 draft", "cancel"]);
  });

  it("gives way to a control moved during the draft: its own draft and full trace, no other", async () => {
    vi.useFakeTimers();
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    starts[0].reply.resolve(1);
    await flush();
    loop.controlChanged();
    await o.promise;
    starts[1].reply.resolve(2);
    await flush();
    loop.onDone(1, "fast drawing, too late");
    vi.advanceTimersByTime(800);
    expect(starts.map((s) => [s.tier, s.fast])).toEqual([["draft", true], ["draft", false], ["final", false]]);
    expect(log).toEqual(["began 1 draft", "began 2 draft"]);
  });

  it("gives way to a full trace asked for meanwhile, and to another image", async () => {
    const { loop, starts } = harness();
    const a = opened(loop, true);
    starts[0].reply.resolve(1);
    await flush();
    void loop.trace("final");
    await a.promise;
    starts[1].reply.resolve(2);
    await flush();
    loop.onDone(1, "fast drawing, too late");
    expect(starts.map((s) => s.tier)).toEqual(["draft", "final"]);

    const b = opened(loop, true);
    starts[2].reply.resolve(3);
    await flush();
    loop.detach();
    await b.promise;
    loop.onDone(3, "the old image's draft");
    expect(starts).toHaveLength(3);
  });

  it("releases its caller when the draft cannot start", async () => {
    const { loop, log, starts } = harness();
    const o = opened(loop, true);
    starts[0].reply.reject(new Error("engine gone"));
    await o.promise;
    expect(log).toEqual(["failed engine gone"]);
    expect(loop.waitingForDraft).toBe(false);
    expect(starts).toHaveLength(1);
  });
});

describe("TraceLoop: colour groups", () => {
  it("remembers the groups a trace was sent with, read before it started", async () => {
    const { loop, starts, world } = harness();
    world.groups = ["#000=#111"];
    const t = loop.trace("final");
    world.groups = ["changed while it ran"];
    starts[0].reply.resolve(1);
    await t;
    expect(loop.groupsFor(1)).toEqual(["#000=#111"]);
  });

  it("forgets the groups of a trace more than eight generations old", async () => {
    const { loop, starts } = harness();
    for (let g = 1; g <= 10; g++) {
      const t = loop.trace("draft");
      starts[g - 1].reply.resolve(g);
      await t;
    }
    expect(loop.groupsFor(1)).toBeUndefined();
    expect(loop.groupsFor(2)).toEqual([]);
    expect(loop.groupsFor(10)).toEqual([]);
  });
});

describe("Latest", () => {
  it("lets only the newest ticket land", () => {
    const latest = new Latest();
    const first = latest.take();
    const second = latest.take();
    expect(latest.isNewest(first)).toBe(false);
    expect(latest.isNewest(second)).toBe(true);
  });
});
