/**
 * The batch queue.
 *
 * It has to survive several hundred rows, so the list is virtualised: only the window
 * that is on screen is in the DOM, with a spacer above and below to keep the scrollbar
 * honest. Failures stay visible after the run — a red line that scrolled past an hour ago
 * is the same as no error at all — so they can be sorted to the top.
 */

import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { fill, h, icon } from "../lib/dom";
import { api, type BatchRow, type PresetId } from "../lib/ipc";
import { bytes, count, de00, duration, percent, type Store } from "../lib/state";
import { closeOverlay, openPopover, toast } from "../components/overlays";

const ROW_HEIGHT = 30;
/** Rows drawn above and below the visible window, so a fast scroll does not flash. */
const OVERSCAN = 8;

export function createBatch(store: Store): HTMLElement {
  const bar = h("div.batchbar");
  const head = h("div.batchhead.eyebrow");
  const viewport = h("div.batchviewport");
  const foot = h("div.batchfoot");
  const el = h(
    "section.stage",
    { style: { background: "var(--paper)" } },
    bar,
    head,
    viewport,
    foot,
  );

  const spacerTop = h("div");
  const rows = h("div");
  const spacerBottom = h("div");
  viewport.append(spacerTop, rows, spacerBottom);

  const ordered = (): BatchRow[] => {
    const b = store.state.batch;
    if (!b.failuresFirst) return b.rows;
    return [...b.rows].sort((x, y) => Number(y.state === "failed") - Number(x.state === "failed"));
  };

  /** An empty queue is an invitation, not an empty table. */
  const emptyQueue = () =>
    h(
      "div.firstrun",
      { style: { minHeight: "100%", background: "transparent" } },
      h(
        "div.drop",
        null,
        h("span.glyph", null, icon("folder", 32)),
        h(
          "div",
          { style: { display: "flex", flexDirection: "column", gap: "6px" } },
          h("span.headline", null, "Trace a whole folder"),
          h(
            "span.faint",
            { style: { fontSize: "13px", maxWidth: "46ch" } },
            "Every PNG, JPEG and WebP in it becomes an SVG in the output folder. A row that fails stays listed after the run.",
          ),
        ),
        h("button.btn.primary", { onclick: () => void chooseInput() }, "Choose a folder…"),
      ),
    );

  const drawRows = () => {
    const list = ordered();
    head.hidden = list.length === 0;
    if (!list.length) {
      spacerTop.style.height = "0px";
      spacerBottom.style.height = "0px";
      fill(rows, emptyQueue());
      return;
    }
    const top = viewport.scrollTop;
    const visible = Math.ceil(viewport.clientHeight / ROW_HEIGHT) + OVERSCAN * 2;
    const first = Math.max(0, Math.floor(top / ROW_HEIGHT) - OVERSCAN);
    const slice = list.slice(first, first + visible);

    spacerTop.style.height = `${first * ROW_HEIGHT}px`;
    spacerBottom.style.height = `${Math.max(0, (list.length - first - slice.length) * ROW_HEIGHT)}px`;
    fill(rows, ...slice.map((r) => rowEl(r, store, () => store.touch("batch"))));
  };

  viewport.addEventListener("scroll", drawRows, { passive: true });

  const pickFolder = async (): Promise<string | null> => {
    const picked = await open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  };

  const chooseInput = async () => {
    const folder = await pickFolder();
    if (!folder) return;
    try {
      const files = await api.batchScan(folder);
      const b = store.state.batch;
      b.folder = folder;
      b.outputDir ??= `${folder}${folder.includes("\\") ? "\\" : "/"}svg`;
      b.rows = files.map((path, id) => ({
        id,
        path,
        file: path.split(/[\\/]/).pop() ?? path,
        preset: b.preset,
        state: "queued",
        de00: null,
        coordinates: null,
        outBytes: null,
        destination: "",
        message: null,
        seconds: null,
      }));
      b.totals = null;
      store.touch("batch");
    } catch (e) {
      toast(String(e), { kind: "bad" });
    }
  };

  const start = async () => {
    const b = store.state.batch;
    if (!b.folder || !b.outputDir) {
      toast("Choose a folder of images and an output folder first.");
      return;
    }
    b.running = true;
    b.paused = false;
    store.touch("batch");
    try {
      await api.batchStart({
        files: b.rows.map((r) => r.path),
        preset: b.preset,
        overrides: b.rows.filter((r) => r.preset !== b.preset).map((r) => [r.id, r.preset] as [number, PresetId]),
        outputDir: b.outputDir,
        skipExisting: b.skipExisting,
      });
    } catch (e) {
      b.running = false;
      store.touch("batch");
      toast(String(e), { kind: "bad" });
    }
  };

  const renderBar = () => {
    const b = store.state.batch;
    const presets = store.state.caps?.presets ?? [];
    const chip = (label: string, onclick: () => void, title?: string) =>
      h("button.btn.compact", { onclick, title: title ?? null }, label);

    fill(
      bar,
      h(
        "select",
        {
          style: { height: "28px", padding: "0 8px", borderRadius: "7px", border: "1px solid var(--rule2)", background: "var(--card)", color: "var(--ink)", font: "400 12px var(--font-body)" },
          "aria-label": "Preset for all",
          onchange: (e: Event) => {
            const value = (e.target as HTMLSelectElement).value as PresetId;
            b.preset = value;
            for (const r of b.rows) if (r.state === "queued") r.preset = value;
            store.touch("batch");
          },
        },
        ...presets.map((p) => h("option", { value: p.id, selected: b.preset === p.id ? "" : null }, `Preset for all · ${p.name}`)),
      ),
      chip(b.folder ? `Input · ${b.folder}` : "Choose a folder…", () => void chooseInput()),
      chip(b.outputDir ? `Output · ${b.outputDir}` : "Output folder…", async () => {
        const picked = await pickFolder();
        if (picked) {
          b.outputDir = picked;
          store.touch("batch");
        }
      }),
      h(
        "button.btn.compact",
        {
          "aria-pressed": String(b.skipExisting),
          onclick: () => {
            b.skipExisting = !b.skipExisting;
            store.touch("batch");
          },
        },
        "Skip existing",
      ),
      h("div.spacer"),
      h(
        "button.btn.compact",
        {
          "aria-pressed": String(b.failuresFirst),
          disabled: !b.rows.some((r) => r.state === "failed"),
          onclick: () => {
            b.failuresFirst = !b.failuresFirst;
            store.touch("batch");
          },
        },
        "Failures first",
      ),
      b.running
        ? chip(b.paused ? "Resume" : "Pause", () => {
            b.paused = !b.paused;
            void api.batchPause(b.paused);
            store.touch("batch");
          })
        : null,
      b.running
        ? chip("Cancel", () => void api.batchCancel())
        : h(
            "button.btn.primary.compact",
            { disabled: !b.rows.length, onclick: () => void start() },
            "Start",
          ),
    );

    fill(
      head,
      h("span"),
      h("span", null, "File"),
      h("span", null, "Preset"),
      h("span", null, "Status"),
      h("span.right", null, "dE00"),
      h("span.right", null, "Coordinates"),
      h("span.right", null, "Output"),
      h("span", null, "Destination"),
    );
  };

  const renderFoot = () => {
    const b = store.state.batch;
    const t = b.totals;
    const done = t?.finished ?? 0;
    const total = t?.total ?? b.rows.length;
    const smaller = t && t.sourceBytes > 0 ? 1 - t.bytesWritten / t.sourceBytes : null;

    fill(
      foot,
      h(
        "div",
        { style: { flex: "1", maxWidth: "420px", display: "flex", flexDirection: "column", gap: "6px" } },
        h(
          "div",
          { style: { display: "flex", justifyContent: "space-between", color: "var(--faint)" } },
          h("span", null, total ? `${done} of ${total} done${t?.failed ? ` · ${t.failed} failed` : ""}` : "Nothing queued"),
          h("span", null, t?.remaining != null ? `${duration(t.remaining)} left` : ""),
        ),
        h("div.progress", null, h("i", { style: { width: percent(total ? done / total : 0) } })),
      ),
      t
        ? h(
            "span.faint",
            null,
            `mean dE00 ${de00(t.meanDe00)} · ${bytes(t.bytesWritten)} written${smaller !== null ? ` · ${percent(smaller)} smaller than source` : ""}`,
          )
        : null,
      h(
        "button.reset",
        {
          style: { marginLeft: "auto" },
          disabled: !b.rows.some((r) => r.state !== "queued"),
          onclick: async () => {
            const csv = await api.batchStatsCsv();
            const path = await save({ defaultPath: "stats.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
            if (!path) return;
            await api.saveBytes(path, [...new TextEncoder().encode(csv)]);
            toast(`Stats written to ${path}`, {
              kind: "good",
              action: { label: "Show in folder", run: () => void revealItemInDir(path) },
            });
          },
        },
        "Export stats.csv",
      ),
    );
  };

  const render = () => {
    renderBar();
    renderFoot();
    drawRows();
  };

  store.on(["batch", "caps"], render);
  render();
  return el;
}

function rowEl(r: BatchRow, store: Store, changed: () => void): HTMLElement {
  const stateClass = r.state === "failed" ? ".failed" : r.state === "running" ? ".running" : "";
  // A row that has already run cannot be re-presetted: the number beside it was measured
  // with the preset it says, and letting the two disagree would make the column a lie.
  const editable = r.state === "queued";
  const preset = h(
    `button.reset${editable ? "" : ".muted"}`,
    {
      style: { textAlign: "left", color: editable ? "var(--dim)" : "var(--muted)" },
      disabled: !editable,
      title: editable ? "Use a different preset for this file" : "Already run",
      onclick: () => openRowPreset(preset, r, store, changed),
    },
    presetName(r.preset),
  );
  return h(
    `div.batchrow${stateClass}`,
    { title: r.message ?? r.path },
    h("span", {
      style: {
        width: "14px",
        height: "14px",
        borderRadius: "4px",
        background: "var(--surface)",
        display: "block",
      },
    }),
    h("span.trunc.dim", null, r.file),
    preset,
    h(`span.state.${r.state}`, null, h("i"), r.state),
    h("span.right", { class: r.de00 === null ? "muted" : "dim" }, r.de00 === null ? "—" : de00(r.de00)),
    h("span.right.faint", null, r.coordinates === null ? "—" : count(r.coordinates)),
    h("span.right.faint", null, r.outBytes === null ? "—" : bytes(r.outBytes)),
    h("span.trunc.muted", null, r.message ?? r.destination),
  );
}

/** Choose a preset for one row. The queue-wide preset stays whatever it was. */
function openRowPreset(anchor: HTMLElement, row: BatchRow, store: Store, changed: () => void): void {
  const presets = store.state.caps?.presets ?? [];
  openPopover(
    anchor,
    h(
      "div.menu",
      null,
      ...presets.map((p) =>
        h(
          "button.item",
          {
            onclick: () => {
              const target = store.state.batch.rows.find((x) => x.id === row.id);
              if (target) target.preset = p.id;
              closeOverlay();
              changed();
            },
          },
          h("span", { style: { flex: "1" } }, p.name),
          h("span.when", null, p.id === row.preset ? "current" : p.subtitle),
        ),
      ),
    ),
  );
}

function presetName(id: PresetId): string {
  return id
    .split("-")
    .map((w, i) => (i === 0 ? w[0].toUpperCase() + w.slice(1) : w))
    .join(" ");
}
