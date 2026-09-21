/**
 * The SVG Minimizer tab.
 *
 * A different emotional register entirely, and worth exploiting: tracing takes a second,
 * minifying takes about thirty milliseconds. This tab is instant and should feel it — the
 * result is the screen, it updates as you move the control, and there is no progress to
 * show because there is nothing to wait for.
 *
 * The tolerance control explains itself in its own label. That sentence *is* the
 * interface; the number is inside it, not in a tooltip hanging off it.
 */

import { open, save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { fill, h, icon } from "../lib/dom";
import { api } from "../lib/ipc";
import { bytes, count, de00, percent, seconds, type Store } from "../lib/state";
import { toast } from "../components/overlays";

export function createMinify(store: Store): HTMLElement {
  const before = h("div.pane", null, h("span.eyebrow.panelabel", null, "Before"));
  const after = h("div.pane", null, h("span.eyebrow.panelabel", null, "After"));
  const divider = h("div.wipehandle", { role: "separator", "aria-label": "Wipe" });
  const panes = h("div.panes", null, before, after, divider);
  const tools = h("div.viewertools");
  const stage = h("section.stage", null, tools, panes);

  // The same three arrangements as the Vectorize viewer, for the same reason: this tab's
  // whole claim is that nothing moved, and the only way to believe that is to look.
  let view: "side" | "wipe" | "ab" = "side";
  let wipe = 0.5;
  let flicked = false;

  const layout = () => {
    panes.classList.toggle("stacked", view !== "side");
    divider.style.display = view === "wipe" ? "" : "none";
    if (view === "side") {
      before.classList.remove("hidden");
      after.classList.remove("hidden");
      before.style.clipPath = "";
      after.style.clipPath = "";
    } else if (view === "wipe") {
      before.classList.remove("hidden");
      after.classList.remove("hidden");
      const pct = `${(wipe * 100).toFixed(2)}%`;
      divider.style.left = pct;
      before.style.clipPath = `inset(0 ${(100 - wipe * 100).toFixed(2)}% 0 0)`;
      after.style.clipPath = `inset(0 0 0 ${pct})`;
    } else {
      before.style.clipPath = "";
      after.style.clipPath = "";
      before.classList.toggle("hidden", !flicked);
      after.classList.toggle("hidden", flicked);
    }
  };

  divider.addEventListener("pointerdown", (e: PointerEvent) => {
    divider.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const r = panes.getBoundingClientRect();
      wipe = Math.min(0.98, Math.max(0.02, (m.clientX - r.left) / r.width));
      layout();
    };
    const up = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", up);
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", up);
  });

  window.addEventListener("keydown", (e) => {
    if (store.state.tab !== "minify" || store.state.screen || e.key !== " " || e.repeat) return;
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    flicked = true;
    layout();
  });
  window.addEventListener("keyup", (e) => {
    if (e.key === " " && flicked) {
      flicked = false;
      layout();
    }
  });

  const renderTools = () => {
    const m = store.state.minify;
    const r = m.result;
    fill(
      tools,
      h(
        "div.seg",
        null,
        ...(["side", "wipe", "ab"] as const).map((v) =>
          h(
            "button",
            {
              "aria-pressed": String(view === v),
              disabled: !m.before,
              onclick: () => {
                view = v;
                layout();
                renderTools();
              },
            },
            v === "side" ? "Side by side" : v === "wipe" ? "Wipe" : "A/B",
          ),
        ),
      ),
      h("span.muted", { style: { fontSize: "11.5px" } }, "hold ", h("kbd", null, "Space"), " to flick"),
      // The promise, stated where somebody is looking for a reason to doubt it.
      h(
        "span.muted",
        { style: { marginLeft: "auto", fontSize: "11.5px" } },
        r
          ? `Nothing moved more than ${m.settings.tolerancePx} px${
              r.differenceDe00 === null ? "" : ` · measured ${de00(r.differenceDe00)} dE00`
            }`
          : "",
      ),
    );
  };
  const rail = h("aside.minifyrail");
  const el = h("div", { style: { flex: "1", minHeight: "0", display: "flex" } }, stage, rail);

  let pending = 0;

  const run = async () => {
    const st = store.state.minify;
    if (!st.before) return;
    const token = ++pending;
    try {
      const result = await api.minify(st.before, st.settings);
      if (token !== pending) return;
      store.state.minify.result = result;
      store.state.minify.error = null;
    } catch (e) {
      if (token !== pending) return;
      store.state.minify.result = null;
      store.state.minify.error = String(e);
    }
    store.touch("minify");
  };

  const openSvg = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "SVG", extensions: ["svg"] }],
    });
    if (typeof picked !== "string") return;
    try {
      const text = await api.readTextFile(picked);
      store.state.minify.name = picked.split(/[\\/]/).pop() ?? picked;
      store.state.minify.before = text;
      store.state.minify.result = null;
      store.state.minify.error = null;
      store.touch("minify");
      await run();
    } catch (e) {
      toast(String(e), { kind: "bad" });
    }
  };

  /** Accept an SVG dropped anywhere in the window while this tab is showing. */
  const acceptDropped = async (path: string) => {
    if (!path.toLowerCase().endsWith(".svg")) return false;
    const text = await api.readTextFile(path);
    store.state.minify.name = path.split(/[\\/]/).pop() ?? path;
    store.state.minify.before = text;
    store.touch("minify");
    await run();
    return true;
  };
  (el as HTMLElement & { acceptDropped?: typeof acceptDropped }).acceptDropped = acceptDropped;

  const render = () => {
    const m = store.state.minify;
    const r = m.result;
    renderTools();
    layout();

    fill(
      before,
      h("span.eyebrow.panelabel", null, `Before${r ? ` · ${bytes(r.bytesBefore)}` : ""}`),
      m.before ? art(m.before) : null,
    );
    fill(
      after,
      h("span.eyebrow.panelabel", null, `After${r ? ` · ${bytes(r.bytesAfter)}` : ""}`),
      r ? art(r.svg) : null,
    );

    if (!m.before) {
      fill(
        rail,
        h(
          "div.railscroll",
          { style: { justifyContent: "center", alignItems: "center", textAlign: "center", gap: "16px" } },
          h("span", { style: { color: "var(--muted)", display: "flex" } }, icon("file", 28)),
          h("span.serif", { style: { fontSize: "20px" } }, "Open an SVG"),
          h(
            "span.faint",
            { style: { fontSize: "12.5px", lineHeight: "1.6", maxWidth: "30ch" } },
            "From Illustrator, Figma, a stock site — anything. It gets rewritten with the fewest segments that draw the same picture.",
          ),
          h("button.btn.primary", { onclick: openSvg }, "Open an SVG"),
        ),
      );
      return;
    }

    const saved = r ? 1 - r.bytesAfter / Math.max(1, r.bytesBefore) : 0;

    fill(
      rail,
      h(
        "div.railscroll",
        { style: { gap: "20px", padding: "22px 20px" } },
        m.error
          ? h(
              "div.card",
              null,
              h("span.eyebrow", null, "This file would not parse"),
              h("span.faint", { style: { fontSize: "12.5px", lineHeight: "1.6" } }, m.error),
            )
          : h(
              "div.minifyresult",
              null,
              h("span.eyebrow", null, r ? `Result · ${seconds((r.ms ?? 0) / 1000)}` : "Working"),
              h(
                "div",
                { style: { display: "flex", alignItems: "baseline", gap: "10px" } },
                h("span.figure", null, r ? percent(saved) : "—"),
                h("span.dim", { style: { fontSize: "14px", paddingBottom: "6px" } }, "smaller"),
              ),
              r
                ? h("span.faint.num", { style: { fontSize: "13px" } }, `${bytes(r.bytesBefore)} → ${bytes(r.bytesAfter)}`)
                : null,
            ),
        r
          ? h(
              "div.stats",
              null,
              ...(
                [
                  ["numbers before", count(r.numbersBefore)],
                  ["numbers after", count(r.numbersAfter)],
                  ["paths", `${r.pathsBefore} → ${r.pathsAfter}`],
                  ["segments", `${count(r.segmentsBefore)} → ${count(r.segmentsAfter)}`],
                  // The promise, measured the same way the tracer measures its own.
                  ["difference", r.differenceDe00 === null ? "—" : `${de00(r.differenceDe00)} dE00`],
                  ["kept as drawn", `${r.guarded} run${r.guarded === 1 ? "" : "s"}`],
                ] as [string, string][]
              ).map(([k, v]) => h("div", null, h("span.k", null, k), h("span.v", null, v))),
            )
          : null,
        toleranceControl(store, run),
        h(
          "div.card",
          null,
          h(
            "div",
            { style: { display: "flex", alignItems: "center", justifyContent: "space-between" } },
            h("span.dim", { style: { fontSize: "12.5px" } }, "Corner threshold"),
            h(
              "div",
              { style: { display: "flex", alignItems: "center", gap: "8px" } },
              h("input.slider", {
                type: "range",
                min: "5",
                max: "90",
                step: "1",
                style: { width: "120px" },
                value: String(m.settings.cornerDegrees),
                "aria-label": "Corner threshold in degrees",
                oninput: (e: Event) => {
                  m.settings.cornerDegrees = Number((e.target as HTMLInputElement).value);
                  store.touch("minify");
                  void run();
                },
              }),
              h("span.num.faint", { style: { fontSize: "12px", width: "34px" } }, `${m.settings.cornerDegrees}°`),
            ),
          ),
          h(
            "div",
            { style: { display: "flex", alignItems: "center", justifyContent: "space-between" } },
            h("span.dim", { style: { fontSize: "12.5px" } }, "Document cleanup"),
            h("button.switch", {
              role: "switch",
              "aria-checked": String(m.settings.documentCleanup),
              "aria-label": "Document cleanup",
              onclick: () => {
                m.settings.documentCleanup = !m.settings.documentCleanup;
                store.touch("minify");
                void run();
              },
            }),
          ),
          h(
            "span.muted",
            { style: { fontSize: "11.5px", lineHeight: "1.5" } },
            "Removes ids, groups, editor metadata and trailing zeros. Same geometry.",
          ),
        ),
        r && r.removed.length
          ? h(
              "div",
              { style: { display: "flex", flexDirection: "column", gap: "7px" } },
              h("span.eyebrow", null, "Removed"),
              ...r.removed.map((x) =>
                h(
                  "div",
                  { style: { display: "flex", alignItems: "baseline", gap: "10px", fontSize: "12px" } },
                  h("span.faint", { style: { flex: "1" } }, x.what),
                  h("span.dim.num", null, x.amount),
                ),
              ),
            )
          : null,
      ),
      h(
        "div.railfoot",
        null,
        h(
          "div.row",
          null,
          h(
            "button.btn.primary",
            {
              style: { flex: "1" },
              disabled: !r,
              onclick: async () => {
                if (!r) return;
                const stem = (m.name ?? "drawing").replace(/\.svg$/i, "");
                const path = await save({
                  defaultPath: `${stem}.min.svg`,
                  filters: [{ name: "SVG", extensions: ["svg"] }],
                });
                if (!path) return;
                await api.saveBytes(path, [...new TextEncoder().encode(r.svg)]);
                toast(`Saved to ${path}`, {
                  kind: "good",
                  action: { label: "Show in folder", run: () => void revealItemInDir(path) },
                });
              },
            },
            "Save SVG",
          ),
          h(
            "button.btn",
            {
              disabled: !r,
              onclick: async () => {
                if (!r) return;
                await writeText(r.svg);
                toast("SVG copied. Paste straight into Figma or Illustrator.");
              },
            },
            "Copy",
          ),
          h("button.btn", { onclick: openSvg }, "Open…"),
        ),
      ),
    );
  };

  store.on(["minify"], render);
  render();
  return el;
}

/**
 * The tolerance control: a sentence with two numbers in it.
 *
 * "Nothing moves more than 0.1 px when the drawing is 1024 px wide" is a complete
 * statement of what the minifier guarantees, and it is a better control than a slider
 * labelled "tolerance" could ever be.
 */
function toleranceControl(store: Store, run: () => void): HTMLElement {
  const m = store.state.minify;
  const field = (value: number, accent: boolean, apply: (n: number) => void) =>
    h("input", {
      class: `inlinefield${accent ? " accent" : ""}`,
      type: "text",
      inputmode: "decimal",
      value: String(value),
      onchange: (e: Event) => {
        const n = Number((e.target as HTMLInputElement).value);
        if (Number.isFinite(n) && n > 0) {
          apply(n);
          store.touch("minify");
          run();
        }
      },
    });

  return h(
    "div.card",
    null,
    h(
      "span.sentence",
      null,
      "Nothing moves more than ",
      field(m.settings.tolerancePx, true, (n) => (m.settings.tolerancePx = n)),
      " px when the drawing is ",
      field(m.settings.judgePx, false, (n) => (m.settings.judgePx = n)),
      " px wide.",
    ),
    h("input.slider", {
      type: "range",
      min: "0",
      max: "1000",
      step: "1",
      // 0–1 px on a square curve: the useful range is the first tenth of it.
      value: String(Math.round(Math.sqrt(Math.min(1, m.settings.tolerancePx)) * 1000)),
      "aria-label": "Tolerance",
      oninput: (e: Event) => {
        const p = Number((e.target as HTMLInputElement).value) / 1000;
        m.settings.tolerancePx = Number((p * p).toFixed(3));
        store.touch("minify");
        run();
      },
    }),
    h(
      "div.stops",
      null,
      h("span", null, "exact"),
      h("span", null, "gentle"),
      h("span", null, "aggressive"),
    ),
  );
}

/**
 * One pane's drawing, parsed rather than injected.
 *
 * `.centred` because this tab has no pan and zoom: the Vectorize viewer pins its artwork
 * to the pane's origin so the overlay can share one transform with it, and without the
 * modifier these two drawings would inherit that and sit in the corner.
 */
function art(svg: string): HTMLElement {
  const wrap = h("div.art.centred");
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  if (root && root.nodeName === "svg") {
    root.setAttribute("width", "320");
    root.setAttribute("height", "320");
    wrap.append(document.importNode(root, true));
  }
  return wrap;
}
