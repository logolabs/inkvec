/**
 * The two full-window screens: Settings and About.
 *
 * Settings is six groups on one scrolling page, no tabs. Privacy is a paragraph rather
 * than a footnote, because it is a feature: it is the reason a brand person can put a
 * client's unreleased mark through this app at all.
 */

import { appMark, fill, h } from "../lib/dom";
import { APP_NAME, openExternal, pickFolder, WEB } from "../lib/platform";
import {
  api,
  events,
  type DenoiserStatus,
  type IntegrationStatus,
  type Prefs,
  type OnOpen,
  type Theme,
} from "../lib/ipc";
import { bytes, type Store } from "../lib/state";
import { closeOverlay, confirm, modal, openModal, toast } from "../components/overlays";
import { windowControls } from "../components/wincontrols";

export interface ScreenActions {
  applyPrefs(patch: Partial<Prefs>): void;
  close(): void;
}

export function createScreens(store: Store, act: ScreenActions): HTMLElement {
  const host = h("div");

  const render = () => {
    const st = store.state;
    if (!st.screen) {
      fill(host);
      host.style.display = "none";
      return;
    }
    host.style.display = "";
    fill(host, st.screen === "settings" ? settings(store, act) : about(store, act));
  };

  store.on(["screen", "prefs", "caps"], render);
  render();
  return host;
}

// ------------------------------------------------------------------ settings ---

function settings(store: Store, act: ScreenActions): HTMLElement {
  const p = store.state.prefs;
  const denoiser = store.state.caps?.denoiser;

  return h(
    "div.screen",
    null,
    h(
      "div.screenbar",
      { "data-tauri-drag-region": "" },
      h("span", { style: { fontSize: "12.5px", fontWeight: "600" } }, "Settings"),
      h("div.spacer"),
      h("button.btn.compact", { onclick: act.close }, "Done"),
      // This screen covers the app bar, so the window's own controls come with it.
      // Taking the chrome and then hiding it is how an app becomes unclosable.
      h("div.sep"),
      windowControls(),
    ),
    h(
      "div.screenbody",
      null,
      h(
        "div.settingsgrid",
        null,
        group("General", [
          // A browser downloads; there is no folder for Export to open.
          WEB
            ? null
            : row(
                "Default output folder",
                "Where Export opens first.",
                h(
                  "button.btn.compact",
                  {
                    onclick: async () => {
                      const picked = await pickFolder();
                      if (picked) act.applyPrefs({ outputFolder: picked });
                    },
                  },
                  p?.outputFolder ?? "Choose…",
                ),
              ),
          row(
            "Theme",
            "The light theme matches the dark one token for token.",
            select(
              [
                ["system", "System"],
                ["dark", "Dark"],
                ["light", "Light"],
              ],
              p?.theme ?? "system",
              (v) => act.applyPrefs({ theme: v as Theme }),
            ),
          ),
          row(
            "When an image is opened",
            "Auto always starts tracing at once. Ask shows a small card offering the Custom wizard beside it; Custom opens the wizard straight away.",
            select(
              [
                ["ask", "Ask"],
                ["auto", "Auto only"],
                ["custom", "Custom wizard"],
              ],
              p?.onOpen ?? "ask",
              (v) => act.applyPrefs({ onOpen: v as OnOpen }),
            ),
          ),
          row(
            "Language",
            "English only in this release. Nothing in the app is machine-translated, which is why there is one.",
            select([["en", "English"]], "en", () => {}),
          ),
        ]),

        group("Performance", [
          WEB
            ? row(
                "Threads",
                crossOriginIsolated
                  ? "The engine runs on a pool of workers, one per core the browser reports."
                  : "This page is not cross-origin isolated, so the engine runs on one core. Open it in its own tab for all of them.",
                h("span.muted.num", { style: { fontSize: "12.5px" } }, crossOriginIsolated ? `${navigator.hardwareConcurrency || 4}` : "1"),
              )
            : row(
                "Threads",
                `${navigator.hardwareConcurrency || 8} available. Fewer leaves room for other work. Takes effect at the next start.`,
                number(p?.threads ?? navigator.hardwareConcurrency ?? 8, 1, 256, (v) => act.applyPrefs({ threads: v })),
              ),
          row(
            "Draft resolution",
            "Used while you move a control.",
            number(p?.draftPx ?? 512, 128, 2048, (v) => act.applyPrefs({ draftPx: v }), "px"),
          ),
          row(
            "Draft time limit",
            "A draft that overruns is abandoned, not shown late.",
            number(p?.draftSeconds ?? 0.4, 0.05, 10, (v) => act.applyPrefs({ draftSeconds: v }), "s", 2),
          ),
          row(
            "Settle before the full trace",
            "How long the controls stay still before the full-resolution trace is queued.",
            number(p?.settleMs ?? 800, 100, 5000, (v) => act.applyPrefs({ settleMs: v }), "ms"),
          ),
        ]),

        group("Denoiser", denoiserRows(store, denoiser)),

        // A web page is always the version it is served as.
        WEB
          ? null
          : group("Updates", [
          row(
            "Check on start",
            "Sends your app version and operating system. Nothing else.",
            toggle(p?.checkUpdates ?? true, (v) => act.applyPrefs({ checkUpdates: v })),
          ),
          row(
            "Channel",
            "",
            select(
              [
                ["stable", "Stable"],
                ["prerelease", "Prerelease"],
              ],
              p?.channel ?? "stable",
              (v) => act.applyPrefs({ channel: v as Prefs["channel"] }),
            ),
          ),
          row(
            "Check now",
            "Asks once, immediately, whatever the setting above says.",
            h(
              "button.btn.compact",
              {
                onclick: async (e: Event) => {
                  const button = e.currentTarget as HTMLButtonElement;
                  button.disabled = true;
                  button.textContent = "Checking…";
                  try {
                    const update = await api.checkUpdate();
                    store.set({ update });
                    if (update.offline) {
                      // A missing network is not an error worth a dialog: the stage says
                      // so plainly and tracing carries on regardless.
                      store.set({ screen: null, stageState: { kind: "offline", message: offlineNote(update.offline) } });
                    } else if (update.newer) {
                      toast(`${update.latest} is available. The status strip has the link.`);
                    } else {
                      toast("This is the newest version.");
                    }
                  } catch (err) {
                    toast(String(err), { kind: "bad" });
                  } finally {
                    button.disabled = false;
                    button.textContent = "Check now";
                  }
                },
              },
              "Check now",
            ),
          ),
        ]),

        group("Advanced", [
          // The same engine, on the command line. The binary linked is the one shipped
          // beside the app, so a trace from the terminal and a trace from the window are
          // the same version.
          WEB
            ? null
            : integrationRow(
            "Add inkvec to PATH",
            "The same engine, on the command line.",
            () => api.cliStatus(),
            () => api.installCli(),
            () => api.removeCli(),
            (s) => (s.installed ? (s.path ?? "Installed") : "Not on the path"),
          ),
          WEB
            ? null
            : integrationRow(
            "Right-click menu",
            platformMenuHelp(store),
            () => api.contextMenuStatus(),
            () => api.installContextMenu(),
            () => api.removeContextMenu(),
            (s) => (s.installed ? "In the menu" : "Not added"),
          ),
          row(
            "Reset settings",
            "Puts every preference and the trace controls back to their defaults. Your files are untouched.",
            h(
              "button.btn.compact.danger",
              {
                onclick: () =>
                  confirm(
                    "Reset settings?",
                    "Every preference goes back to its default, and the recent list is cleared. Nothing you have traced or exported is affected.",
                    "Reset",
                    async () => {
                      const fresh = await api.resetPrefs();
                      store.set({ prefs: fresh, settings: fresh.trace });
                      toast("Settings reset.");
                    },
                    true,
                  ),
              },
              "Reset",
            ),
          ),
        ]),

        // Privacy gets room rather than a footnote. It is a feature.
        h(
          "div.privacycard",
          null,
          h("span.eyebrow", null, "Privacy"),
          h("span.line", null, WEB ? "Your image never leaves this browser." : "Your image never leaves this computer."),
          h(
            "p",
            { style: { margin: "0", fontSize: "12.5px", lineHeight: "1.65", color: "var(--faint)" } },
            WEB
              ? "Tracing runs in this tab, on your own processor, as WebAssembly. There is no account, no upload and no telemetry. The page fetches only itself and, if you ask for it, the denoiser's weights; your preferences are kept in this browser's storage."
              : "Tracing runs entirely on your own processor. There is no account, no upload and no telemetry. The only network request the app can make is the update check, which sends your app version and operating system and nothing else. Turn it off above and the app never contacts the network at all.",
          ),
        ),
      ),
    ),
  );
}

function denoiserRows(store: Store, d: DenoiserStatus | undefined) {
  if (!d) return [row("Status", "", h("span.muted", null, "—"))];

  if (!d.supported) {
    return [
      row(
        "Status",
        WEB
          ? "The denoiser needs a cross-origin isolated page, which this one is not. Open the app in its own tab to use it."
          : "This build does not include the restorer, so there is nothing to download. The release builds for Windows, Linux and Apple-silicon macOS do.",
        h("span.muted", { style: { fontSize: "12.5px" } }, "Not in this build"),
      ),
    ];
  }

  return [
    row(
      "Status",
      "Optional. Improves traces from photographs and screenshots.",
      h(
        "span",
        { style: { fontSize: "12.5px", color: d.installed ? "var(--good)" : "var(--muted)" } },
        d.installed ? `Installed · ${bytes(d.bytes ?? 0)}` : "Not installed",
      ),
    ),
    row(
      d.installed ? "Remove" : "Download",
      d.installed
        ? (d.path ?? (WEB ? "Kept in this browser's storage." : ""))
        : WEB
          ? "Downloaded once into this browser's storage, and run in this tab on your GPU where it has one. The model comes down, your image never goes up."
          : "Downloaded once and stored on this machine. Everything still runs locally — the model comes down, your image never goes up.",
      d.installed
        ? h(
            "button.btn.compact.danger",
            {
              onclick: async () => {
                const status = await api.denoiserRemove();
                const caps = store.state.caps;
                if (caps) store.set({ caps: { ...caps, denoiser: status } });
                toast("Denoiser removed.");
              },
            },
            "Remove",
          )
        : h("button.btn.compact", { onclick: () => openDenoiserModal(store) }, "Download"),
    ),
    row("Verified against", d.repo, h("span.mono.muted", { style: { fontSize: "11px" } }, `${d.sha256.slice(0, 16)}…`)),
  ];
}

/**
 * The denoiser download, explained honestly before anything is fetched.
 *
 * Cancel here is a real cancel, unlike a trace's: a download has somewhere to stop.
 */
export function openDenoiserModal(store: Store): void {
  const d = store.state.caps?.denoiser;
  if (!d) return;

  const bar = h("div.progress", null, h("i", { style: { width: "0%" } }));
  const line = h("span.faint.num", { style: { fontSize: "12.5px" } }, "");
  const body = h("div.modal", { tabindex: "-1" });
  let downloading = false;

  const render = () => {
    fill(
      body,
      h("h2", null, downloading ? "Downloading the denoiser" : "Download the denoiser"),
      downloading
        ? h("div", { style: { display: "flex", flexDirection: "column", gap: "10px" } }, bar, line)
        : h(
            "p",
            null,
            WEB
              ? "Downloaded once (about 80 MB, plus ONNX Runtime Web) and kept in this browser. It repairs JPEG and screenshot damage before tracing, which usually halves the colour error on photographed logos. It runs in this tab, on your GPU where there is one — the model comes down, your image never goes up."
              : "Downloaded once and stored on this machine. It repairs JPEG and screenshot damage before tracing, which usually halves the colour error on photographed logos. Everything still runs locally — the model comes down, your image never goes up.",
          ),
      h(
        "div.stats",
        null,
        h("div", null, h("span.k", null, "from"), h("span.v", { style: { fontSize: "12px" } }, d.repo)),
        h("div", null, h("span.k", null, "adds per trace"), h("span.v", null, WEB ? "seconds" : "~0.6 s")),
      ),
      h(
        "span.muted",
        { style: { fontSize: "11.5px", lineHeight: "1.5" } },
        `SHA-256 verified on arrival against ${d.sha256.slice(0, 16)}… · removable in Settings.`,
      ),
      h(
        "div.actions",
        null,
        h(
          "button.btn",
          {
            onclick: () => {
              if (downloading) void api.denoiserCancel();
              closeOverlay();
            },
          },
          downloading ? "Cancel download" : "Not now",
        ),
        downloading
          ? null
          : h(
              "button.btn.primary",
              {
                onclick: () => {
                  downloading = true;
                  render();
                  void api.denoiserDownload();
                },
              },
              "Download",
            ),
      ),
    );
  };

  // Progress and completion are events, so the modal can be left open or closed.
  void (async () => {
    const stopProgress = await events.denoiserProgress(({ got, total }) => {
      const pct = total ? (got / total) * 100 : 0;
      (bar.firstElementChild as HTMLElement).style.width = `${pct.toFixed(1)}%`;
      line.textContent = total
        ? `${bytes(got)} of ${bytes(total)} · you can keep tracing`
        : `${bytes(got)} · you can keep tracing`;
    });
    const stopDone = await events.denoiserDone(({ ok, message, status }) => {
      const caps = store.state.caps;
      if (caps) store.set({ caps: { ...caps, denoiser: status } });
      stopProgress();
      stopDone();
      closeOverlay();
      if (ok) toast("Denoiser installed and verified.", { kind: "good" });
      else if (message !== "cancelled") toast(message ?? "The download did not finish.", { kind: "bad" });
    });
  })();

  openModal(body);
  render();
}

// --------------------------------------------------------------------- about ---

function about(store: Store, act: ScreenActions): HTMLElement {
  const caps = store.state.caps;
  const notices = h("pre", null, "Loading the notices…");
  void api
    .thirdPartyNotices()
    .then((text) => {
      notices.textContent = text;
    })
    .catch(() => {
      notices.textContent =
        "The bundled notices could not be read. They are also published at github.com/logolabs/inkvec in docs/THIRD_PARTY.md.";
    });

  const link = (label: string, href: string) =>
    h("a", { href: "#", onclick: (e: Event) => { e.preventDefault(); void openExternal(href); } }, label);

  return h(
    "div.screen",
    null,
    h(
      "div.screenbar",
      { "data-tauri-drag-region": "" },
      h("span", { style: { fontSize: "12.5px", fontWeight: "600" } }, "About"),
      h("div.spacer"),
      h("button.btn.compact", { onclick: act.close }, "Done"),
      // This screen covers the app bar, so the window's own controls come with it.
      // Taking the chrome and then hiding it is how an app becomes unclosable.
      h("div.sep"),
      windowControls(),
    ),
    h(
      "div.screenbody",
      null,
      h(
        "div.about",
        null,
        h(
          "div.aboutleft",
          null,
          h("span.mark", null, appMark(72)),
          h(
            "div",
            { style: { display: "flex", flexDirection: "column", gap: "8px" } },
            h("span.serif", { style: { fontSize: "40px", lineHeight: "1" } }, APP_NAME),
            h(
              "span.faint.num",
              { style: { fontSize: "13px" } },
              `Version ${caps?.version ?? "—"} · engine Inkvec ${caps?.engineVersion ?? "—"} · Apache-2.0`,
            ),
            h("span.muted.mono", { style: { fontSize: "11px" } }, `build target ${caps?.buildTarget ?? "—"}`),
          ),
          // One honest line about what LogoLabs does. Not the pricing.
          h(
            "p",
            { style: { margin: "0", fontSize: "13.5px", lineHeight: "1.7", color: "var(--dim)", maxWidth: "380px" } },
            "Made by LogoLabs, an AI lab working on brand assets and logo creation. Tracing reconstructs a picture; drawing one is a different job, and that is the one we do.",
          ),
          WEB
            ? h(
                "p",
                { style: { margin: "0", fontSize: "13px", lineHeight: "1.7", color: "var(--faint)", maxWidth: "380px" } },
                "This is the browser edition. The desktop app, Inkvec Studio, is the same interface with folders of images traced in one go, the command line, the right-click menu and traces up to 16384 px.",
              )
            : null,
          h(
            "div",
            { style: { display: "flex", gap: "18px", fontSize: "13px", flexWrap: "wrap" } },
            link("GitHub", "https://github.com/logolabs/inkvec"),
            link("Benchmark results", "https://github.com/logolabs/inkvec#benchmark"),
            WEB
              ? link("Inkvec Studio for the desktop", "https://github.com/logolabs/inkvec/releases")
              : link("Inkvec Studio Lite, in the browser", "https://huggingface.co/spaces/Logolabs/inkvec"),
            link("logolabs.org", "https://logolabs.org"),
          ),
        ),
        h(
          "div.aboutright",
          null,
          // Obliged to carry it, and it happens to be a credibility asset, so it gets
          // a decent treatment rather than fine print.
          h(
            "div.credit",
            null,
            h("span.eyebrow", { style: { color: "var(--gold)" } }, "Compute acknowledgement"),
            h(
              "p",
              { style: { margin: "0", fontSize: "13px", lineHeight: "1.7", color: "var(--dim)" } },
              "The denoiser was trained on resources provided by EuroHPC JU and NAISS. Training ran on the Arrhenius GPU supercomputer at NAISS, Sweden, under EuroHPC project EHPC-AIF-2026PG01-907.",
            ),
            h(
              "div",
              { style: { display: "flex", gap: "10px", marginTop: "4px" } },
              h("span.btn.compact", { style: { cursor: "default" } }, "EuroHPC JU"),
              h("span.btn.compact", { style: { cursor: "default" } }, "NAISS"),
            ),
          ),
          h(
            "div.notices",
            null,
            h(
              "div.head",
              null,
              h("span", null, "Licence and third-party notices"),
              h("button.reset", { onclick: () => showFullNotices(notices.textContent ?? "") }, "Open"),
            ),
            notices,
          ),
          h("span.muted", { style: { fontSize: "12px" } }, WEB ? "Your image never leaves this browser." : "Your image never leaves this computer."),
        ),
      ),
    ),
  );
}

function showFullNotices(text: string): void {
  openModal(
    modal(
      "Licence and third-party notices",
      [
        h("pre", {
          style: {
            margin: "0",
            maxHeight: "60vh",
            overflow: "auto",
            font: "11.5px/1.7 var(--font-mono)",
            color: "var(--faint)",
            whiteSpace: "pre-wrap",
            userSelect: "text",
          },
        }, text),
      ],
      [h("button.btn", { onclick: closeOverlay }, "Close")],
    ),
  );
}

/**
 * The second sentence of the "no network" card.
 *
 * The reason the check failed is a transport error — a DNS name, a proxy, a refused
 * connection — and pasting one into a card helps nobody. What the user can act on is
 * whether the check will happen again, so that is what this says; the raw reason is kept
 * only when it is short enough to be a hint rather than a stack trace.
 */
function offlineNote(reason: string): string {
  const short = reason.length <= 80 ? ` (${reason})` : "";
  return `The update check will run next time you are online${short}.`;
}

/** What the right-click row's help line says, which differs per platform. */
function platformMenuHelp(store: Store): string {
  switch (store.state.caps?.platform) {
    case "windows":
      return '"Vectorize with Inkvec" in Explorer, for you only. Nothing machine-wide.';
    case "macos":
      return "macOS already offers Inkvec Studio under Finder's Open With.";
    default:
      return '"Vectorize with Inkvec" in your file manager, for you only.';
  }
}

/**
 * A Settings row backed by something that is either in place or not.
 *
 * It asks the backend what the truth is rather than remembering what it did, because both
 * of these live outside the app — a symlink and a registry key — and either can be removed
 * by something else between one visit to this screen and the next.
 */
function integrationRow(
  label: string,
  help: string,
  read: () => Promise<IntegrationStatus>,
  add: () => Promise<IntegrationStatus>,
  drop: () => Promise<IntegrationStatus>,
  describe: (s: IntegrationStatus) => string,
): HTMLElement {
  const control = h("span.muted", { style: { fontSize: "12.5px" } }, "…");
  const detail = h("span.help");
  const container = row(label, help, control);
  container.querySelector(".about")?.append(detail);

  const paint = (status: IntegrationStatus) => {
    detail.textContent = status.available ? describe(status) : (status.note ?? "");
    if (!status.available) {
      fill(control, h("span.muted", { style: { fontSize: "12.5px" } }, "Not available"));
      return;
    }
    fill(
      control,
      h(
        `button.btn.compact${status.installed ? ".danger" : ""}`,
        {
          onclick: async () => {
            fill(control, h("span.muted", { style: { fontSize: "12.5px" } }, "Working…"));
            try {
              paint(status.installed ? await drop() : await add());
              toast(status.installed ? `${label}: removed.` : `${label}: added.`);
            } catch (e) {
              paint(status);
              toast(String(e), { kind: "bad" });
            }
          },
        },
        status.installed ? "Remove" : "Add",
      ),
    );
  };

  void read().then(paint).catch(() => {
    fill(control, h("span.muted", { style: { fontSize: "12.5px" } }, "Unknown"));
  });
  return container;
}

// ------------------------------------------------------------------ fragments ---

/** A group of rows; a row that is `null` is one this build does not have. */
function group(name: string, rows: (HTMLElement | null)[]): HTMLElement {
  return h("div.settinggroup", null, h("div.head.eyebrow", null, name), ...rows);
}

function row(label: string, help: string, control: HTMLElement): HTMLElement {
  return h(
    "div.settingrow",
    null,
    h(
      "div.about",
      null,
      h("span.label", null, label),
      help ? h("span.help", null, help) : null,
    ),
    h("div.control", null, control),
  );
}

function select(options: [string, string][], value: string, onchange: (v: string) => void): HTMLElement {
  return h(
    "select",
    { onchange: (e: Event) => onchange((e.target as HTMLSelectElement).value) },
    ...options.map(([v, label]) => h("option", { value: v, selected: v === value ? "" : null }, label)),
  );
}

function number(
  value: number,
  min: number,
  max: number,
  onchange: (v: number) => void,
  unit = "",
  decimals = 0,
): HTMLElement {
  return h(
    "div",
    { style: { display: "flex", alignItems: "center", gap: "6px" } },
    h("input", {
      type: "number",
      value: String(decimals ? value.toFixed(decimals) : Math.round(value)),
      min: String(min),
      max: String(max),
      step: decimals ? String(10 ** -decimals) : "1",
      style: { width: "90px" },
      onchange: (e: Event) => {
        const n = Number((e.target as HTMLInputElement).value);
        if (Number.isFinite(n)) onchange(Math.min(max, Math.max(min, n)));
      },
    }),
    unit ? h("span.muted", { style: { fontSize: "11.5px" } }, unit) : null,
  );
}

function toggle(value: boolean, onchange: (v: boolean) => void): HTMLElement {
  const sw = h("button.switch", {
    role: "switch",
    "aria-checked": String(value),
    onclick: () => onchange(!value),
  });
  return sw;
}
