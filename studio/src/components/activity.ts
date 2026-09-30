/**
 * The activity log: bottom right of the stage, what the engine is doing, line by line.
 *
 * One line per stage (its start time, what it does, and how long it took once it has
 * finished, or its loop count and its own running clock while it runs) and one per note the
 * engine made on the way ("8 inks"). Compact and collapsible; it keeps the last trace's log
 * once the trace has landed, with how long it took, until the next one starts.
 */

import { fill, h } from "../lib/dom";
import { clock, stamp, stepText, type LogLine } from "../lib/live";
import type { Store } from "../lib/state";

/** The most lines drawn at once; the log keeps more, the newest are what matter. */
const SHOWN = 160;

/**
 * The activity log's panel, built once: a header that folds it open or shut and the lines of
 * the trace in flight (or the last one), redrawn from `State.traceLog` as it changes.
 */
export function createActivity(store: Store): HTMLElement {
  const el = h("aside.activity", { "aria-label": "Engine activity", "aria-live": "off" });
  const head = h("button.activityhead", {
    type: "button",
    "data-ctl": "activity-toggle",
    onclick: () => store.set({ logOpen: !store.state.logOpen }),
  });
  const body = h("div.activitybody", { role: "log" });
  el.append(head, body);

  const line = (l: LogLine, running: boolean) => {
    if (l.kind === "note") {
      return h("div.activityline.note", null, h("span.t", null, stamp(l.at)), h("span.w", null, l.text ?? ""));
    }
    const detail = running && l.step ? ` · ${stepText(l.step)}` : "";
    return h(
      `div.activityline${running ? ".running" : ".done"}`,
      null,
      h("span.t", null, stamp(l.at)),
      h("span.w", null, `${l.what}${detail}`),
      running
        ? h("span.r.num", { "data-live": "stage" })
        : h("span.r.num", null, l.ms === undefined ? "" : clock(l.ms)),
    );
  };

  const render = () => {
    const st = store.state;
    const show = st.tab === "vectorize" && Boolean(st.source) && !st.screen && (st.tracing || st.traceLog.length > 0);
    el.hidden = !show;
    if (!show) return;
    el.classList.toggle("open", st.logOpen);
    el.classList.toggle("busy", st.tracing);
    const ended = st.traceEnded && !st.tracing ? clock(st.traceEnded - st.traceStarted) : null;
    fill(
      head,
      st.tracing ? h("span.pulse") : null,
      h("span.title", null, "Engine log"),
      st.tracing
        ? h("span.num.muted", { "data-live": "elapsed" }, clock(performance.now() - st.traceStarted))
        : ended
          ? h("span.num.muted", null, ended)
          : null,
      h("span.chev", null, st.logOpen ? "Hide" : "Show"),
    );
    if (!st.logOpen) {
      fill(body);
      return;
    }
    // Stick to the bottom unless the reader has scrolled up to look at something.
    const pinned = body.scrollHeight - body.scrollTop - body.clientHeight < 24;
    const lines = st.traceLog.slice(-SHOWN);
    let openAt = -1;
    if (st.tracing) for (let i = lines.length - 1; i >= 0 && openAt < 0; i--) if (lines[i].kind === "stage" && lines[i].ms === undefined) openAt = i;
    fill(body, ...lines.map((l, i) => line(l, i === openAt)));
    if (pinned) body.scrollTop = body.scrollHeight;
  };

  store.on(["traceLog", "tracing", "logOpen", "source", "tab", "screen", "liveNow"], render);
  render();
  return el;
}
