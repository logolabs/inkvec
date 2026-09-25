/**
 * The palette card: the inks of the trace, and colour groups — inks to draw as one.
 *
 * What it shows, top to bottom:
 *
 * - **Colour groups** the user made. Each is sent with every trace, and the engine merges
 *   the group's inks in the image before tracing, so the shapes between them really join
 *   (`inkvec_trace::regroup`). A group keeps showing the colours it was made of after the
 *   re-trace has merged them away, so it can be undone, and what the engine said about it.
 * - **Suggested groups**, proposed from the palette and never applied until accepted: near
 *   duplicates by default, or "simplify to N colours".
 * - **The inks**, flat or gradient, each draggable onto another to group them. The same can
 *   be done without a pointer: tick inks and press Merge.
 *
 * Hovering or focusing an ink, a group or a member singles its fills out in the vector pane
 * (`State.hoverFill`; the viewer draws it), so it is clear what is being merged.
 */


import { fill, h, icon } from "../lib/dom";
import { de00, percent, type State, type Store, type Suggestion } from "../lib/state";
import type { ColourGroup, Ink, Snap } from "../lib/ipc";
import { api } from "../lib/ipc";
import { copyText } from "../lib/platform";
import {
  groupSignature,
  inkFor,
  isGradient,
  memberOf,
  mergeReports,
  reportedMissing,
  sameMember,
  stopsOf,
  suggestGroups,
  swatchBackground,
  type MergeReport,
} from "../lib/colour";
import { closeOverlay, modal, openModal, openPopover, toast } from "./overlays";

export interface PaletteActions {
  /** Snap inks, each named by the colour it was traced as, in one rewrite of the drawing. */
  snap(snaps: Snap[]): void;
  /** Replace the colour groups; a new trace follows, draft then final. */
  setColourGroups(groups: ColourGroup[]): void;
}

// ------------------------------------------------------------- proposals ---

/**
 * The suggestions for the palette as it now stands.
 *
 * Suggestions the user has edited are kept as they are; the rest are proposed afresh from
 * the inks that are in no group and no kept suggestion, less any the user has dismissed.
 */
export function proposeGroups(st: State): Suggestion[] {
  const kept = (st.groupSuggestions ?? []).filter((g) => g.edited);
  const skip = [...st.colourGroups.flatMap((g) => g.members), ...kept.flatMap((g) => g.members)];
  const fresh = suggestGroups(st.palette, { count: st.simplifyTo, skip }).filter(
    (g) => !st.dismissedGroups.includes(groupSignature(g)),
  );
  return [...kept, ...fresh];
}

/** A suggestion as it applies now: only the members that are still inks of the palette. */
function liveSuggestion(g: Suggestion, palette: Ink[]): Suggestion | null {
  const members = g.members.filter((m) => inkFor(m, palette));
  if (members.length < 2) return null;
  // A target that names a member keeps naming the same member.
  let target = g.target;
  if (target?.startsWith("@")) {
    const named = g.members[Number(target.slice(1)) - 1];
    const at = named ? members.indexOf(named) : -1;
    target = at >= 0 ? `@${at + 1}` : null;
  }
  return { ...g, members, target };
}

/** The suggestions that still apply to the palette on screen, with their place in the stored list. */
export function liveSuggestions(st: State): { g: Suggestion; index: number }[] {
  return (st.groupSuggestions ?? [])
    .map((g, index) => ({ g: liveSuggestion(g, st.palette), index }))
    .filter((x): x is { g: Suggestion; index: number } => x.g !== null);
}

// ------------------------------------------------------------ group edits ---

/** `g` without its member `k`, with a target that named a later member following it. */
function withoutMember(g: ColourGroup, k: number): ColourGroup {
  let target = g.target;
  if (target?.startsWith("@")) {
    const n = Number(target.slice(1)) - 1;
    target = n === k ? null : n > k ? `@${n}` : target;
  }
  return { ...g, members: g.members.filter((_, i) => i !== k), target };
}

/** Every group and suggestion with `member` taken out of it. Groups of one are not groups. */
function takeOut<T extends ColourGroup>(list: T[], member: string, except = -1, mark = false): T[] {
  return list.map((g, i) => {
    if (i === except) return g;
    const k = g.members.findIndex((m) => sameMember(m, member));
    if (k < 0) return g;
    return { ...withoutMember(g, k), ...(mark ? { edited: true } : {}) } as T;
  });
}

const real = <T extends ColourGroup>(list: T[]) => list.filter((g) => g.members.length > 1);

/** Where a dragged member came from. */
type From = { kind: "palette" } | { kind: "group" | "sugg"; index: number };

/**
 * Put `member` where it was dropped.
 *
 * Onto a group or a suggestion it joins it; onto an ink it makes a new group with that ink
 * (or joins the group that ink is already in); dropped anywhere else, a member dragged out
 * of a group leaves it. A member is only ever in one place, so it is taken out of wherever
 * else it was.
 */
function place(store: Store, act: PaletteActions, member: string, from: From, target: Element | null): void {
  const st = store.state;
  let groups = st.colourGroups.map((g) => ({ ...g, members: [...g.members] }));
  let suggs: Suggestion[] = (st.groupSuggestions ?? []).map((g) => ({ ...g, members: [...g.members] }));
  const spot = target?.closest<HTMLElement>("[data-drop]") ?? null;
  const kind = spot?.dataset.drop ?? null;
  const index = Number(spot?.dataset.index ?? -1);

  if ((kind === "group" || kind === "sugg") && from.kind === kind && from.index === index) return;
  if (kind === "ink" && sameMember(spot?.dataset.member ?? "", member)) return;

  if (kind === "group") {
    groups = takeOut(groups, member, index);
    suggs = takeOut(suggs, member, -1, true);
    if (!groups[index].members.some((m) => sameMember(m, member))) groups[index].members.push(member);
  } else if (kind === "sugg") {
    groups = takeOut(groups, member);
    suggs = takeOut(suggs, member, index, true);
    if (!suggs[index].members.some((m) => sameMember(m, member))) {
      suggs[index] = { ...suggs[index], members: [...suggs[index].members, member], edited: true };
    }
  } else if (kind === "ink") {
    const other = spot?.dataset.member ?? "";
    groups = takeOut(groups, member);
    suggs = takeOut(suggs, member, -1, true);
    const home = groups.findIndex((g) => g.members.some((m) => sameMember(m, other)));
    if (home >= 0) groups[home].members.push(member);
    else {
      suggs = takeOut(suggs, other, -1, true);
      groups.push({ members: [other, member], target: null });
    }
  } else if (from.kind === "palette") {
    return;
  } else if (from.kind === "group") {
    groups = takeOut(groups, member);
  } else {
    suggs = takeOut(suggs, member, -1, true);
  }

  const nextGroups = real(groups);
  store.set({ groupSuggestions: real(suggs), paletteSelection: [] });
  if (JSON.stringify(nextGroups) !== JSON.stringify(st.colourGroups)) act.setColourGroups(nextGroups);
}

/** The ticked inks, as one group: added to a group one of them is already in, or a new one. */
function mergeSelected(store: Store, act: PaletteActions): void {
  const st = store.state;
  const picked = st.paletteSelection;
  if (picked.length < 2) return;
  let groups = st.colourGroups.map((g) => ({ ...g, members: [...g.members] }));
  const home = groups.findIndex((g) => g.members.some((m) => picked.some((p) => sameMember(p, m))));
  let suggs = st.groupSuggestions ?? [];
  for (const m of picked) {
    groups = takeOut(groups, m, home);
    suggs = takeOut(suggs, m, -1, true);
  }
  if (home >= 0) {
    for (const m of picked) if (!groups[home].members.some((x) => sameMember(x, m))) groups[home].members.push(m);
  } else {
    groups.push({ members: [...picked], target: null });
  }
  store.set({ groupSuggestions: real(suggs), paletteSelection: [] });
  act.setColourGroups(real(groups));
}

// ------------------------------------------------------------------ drag ---

/** How far the pointer must travel before a press on a chip becomes a drag, in pixels. */
const DRAG_START_PX = 4;

/**
 * Make `el` draggable as `member`.
 *
 * Pointer events rather than HTML drag and drop: the desktop webview's own file-drop
 * handling takes over HTML5 drag events on Windows, so they would never reach the page.
 * A press that does not travel stays a click. The listeners live on the window for the
 * length of a drag, because the rail may be rebuilt under the pointer while it is held.
 */
function draggable(el: HTMLElement, store: Store, act: PaletteActions, member: string, from: From, radial: boolean): void {
  el.addEventListener("pointerdown", (down: PointerEvent) => {
    if (down.button !== 0) return;
    const x0 = down.clientX;
    const y0 = down.clientY;
    let ghost: HTMLElement | null = null;
    let over: Element | null = null;

    const mark = (spot: Element | null) => {
      if (spot === over) return;
      over?.classList.remove("dropping");
      over = spot;
      over?.classList.add("dropping");
    };
    const move = (m: PointerEvent) => {
      if (!ghost) {
        if (Math.hypot(m.clientX - x0, m.clientY - y0) < DRAG_START_PX) return;
        ghost = h("div.dragghost", null, h("span.swatch", null, h("i", { style: { background: swatchBackground(member, radial) } })));
        document.body.append(ghost);
        document.body.classList.add("dragging-ink");
      }
      ghost.style.left = `${m.clientX + 8}px`;
      ghost.style.top = `${m.clientY + 8}px`;
      const under = document.elementFromPoint(m.clientX, m.clientY);
      mark(under?.closest("[data-drop]") ?? null);
    };
    const up = (u: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      if (!ghost) return;
      ghost.remove();
      document.body.classList.remove("dragging-ink");
      mark(null);
      // The press was a drag, not a click: a click that follows it in the same gesture is
      // swallowed, and the guard is gone before anything else can be clicked.
      const swallow = (c: Event) => c.stopPropagation();
      window.addEventListener("click", swallow, true);
      window.setTimeout(() => window.removeEventListener("click", swallow, true), 0);
      if (u.type === "pointercancel") return;
      place(store, act, member, from, document.elementFromPoint(u.clientX, u.clientY));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  });
}

// ----------------------------------------------------------------- hover ---

/** The paint values a member stands for in the drawing now. */
function keysOf(member: string, palette: Ink[]): string[] {
  return inkFor(member, palette)?.keys ?? [];
}

/**
 * Keep `State.hoverFill` on whatever `[data-fill]` element inside `container` is hovered or
 * focused. Delegated, because the rail is rebuilt whole and its elements come and go; and
 * checked every frame while it is set, because an element removed from under a still
 * pointer never says it was left.
 */
export function wirePaletteHover(container: HTMLElement, store: Store): void {
  let frame = 0;
  const current = (): string[] | null => {
    // The innermost hovered one: a member chip inside a group card, not the card.
    let el: HTMLElement | null = [...container.querySelectorAll<HTMLElement>("[data-fill]:hover")].pop() ?? null;
    if (!el) {
      // Keyboard focus counts; the focus a click leaves behind does not.
      const a = document.activeElement;
      if (a instanceof HTMLElement && container.contains(a) && a.matches(":focus-visible")) {
        el = a.closest<HTMLElement>("[data-fill]");
      }
    }
    const keys = el ? (JSON.parse(el.dataset.fill ?? "[]") as string[]) : [];
    return keys.length ? keys : null;
  };
  const update = () => {
    const keys = current();
    const was = store.state.hoverFill;
    if ((keys ?? []).join("\n") !== (was ?? []).join("\n")) store.set({ hoverFill: keys });
    window.cancelAnimationFrame(frame);
    if (keys) frame = window.requestAnimationFrame(update);
  };
  for (const type of ["pointerover", "pointerout", "focusin", "focusout"]) {
    container.addEventListener(type, () => window.requestAnimationFrame(update));
  }
}

// ------------------------------------------------------------------ card ---

export function paletteCard(store: Store, act: PaletteActions): HTMLElement {
  const st = store.state;
  // Each suggestion as it applies to this palette, with its place in the stored list.
  const suggestions = liveSuggestions(st);
  const flats = st.palette.filter((i) => i.kind !== "gradient");
  const selected = st.paletteSelection;

  return h(
    "div.card.palettecard",
    null,
    h(
      "div.cardhead",
      null,
      h(
        "span.eyebrow",
        { style: { display: "flex", alignItems: "center", gap: "6px" } },
        icon("palette", 12),
        `Palette · ${st.palette.length} ink${st.palette.length === 1 ? "" : "s"}`,
      ),
      h("button.reset", { disabled: !flats.length, onclick: () => pastePalette(store, act) }, "Paste brand palette"),
    ),
    st.colourGroups.length ? groupsBlock(store, act) : null,
    st.palette.length ? suggestionsBlock(store, act, suggestions) : null,
    h(
      "div.inklist",
      { "data-drop": "list", "aria-label": "Inks" },
      ...st.palette.map((ink) => inkRow(store, act, ink)),
    ),
    selected.length
      ? h(
          "div.mergebar",
          null,
          h("span.muted", null, `${selected.length} ticked`),
          h(
            "button.btn.compact",
            {
              "data-ctl": "merge-ticked",
              disabled: selected.length < 2,
              title: "Draw the ticked inks as one: a new group, or the group one of them is already in",
              onclick: () => mergeSelected(store, act),
            },
            "Merge",
          ),
          h("button.reset", { onclick: () => store.set({ paletteSelection: [] }) }, "Clear"),
        )
      : null,
    st.palette.length
      ? h(
          "div",
          { style: { display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: "10px" } },
          h(
            "span.muted",
            { style: { fontSize: "11px", lineHeight: "1.45" } },
            "Snapping rewrites fills only — no re-trace. Grouping re-traces, so the shapes join.",
          ),
          h(
            "button.reset",
            {
              style: { fontSize: "11px", flex: "none" },
              title: "Custom properties, one per flat ink, in canvas-share order",
              onclick: () => void copyPaletteCss(flats),
            },
            "Copy all as CSS",
          ),
        )
      : h("span.muted", { style: { fontSize: "11px" } }, "Trace an image to see its inks."),
  );
}

/** A small preview of one member: a colour chip, or its stops as a ramp. */
function chipSwatch(member: string, radial = false): HTMLElement {
  return h(
    `span.swatch.mini${isGradient(member) ? ".ramp" : ""}`,
    null,
    h("i", { style: { background: swatchBackground(member, radial) } }),
  );
}

function memberLabel(member: string): string {
  const stops = stopsOf(member);
  return stops.length > 1 ? `${stops.length} stops` : stops[0].toUpperCase();
}

// --------------------------------------------------------------- groups ---

function groupsBlock(store: Store, act: PaletteActions): HTMLElement {
  const st = store.state;
  // The engine's lines belong to the groups the drawing on screen was traced with; while a
  // re-trace for changed groups is on its way they would describe the wrong groups.
  const current = JSON.stringify(st.resultGroups) === JSON.stringify(st.colourGroups);
  const reports = current && st.result ? mergeReports(st.result.engineLog) : [];
  return h(
    "div.cgroups",
    null,
    h(
      "div.cgroupshead",
      null,
      h("span.eyebrow", null, `Colour groups · ${st.colourGroups.length}`),
      h(
        "button.reset",
        {
          title: "Trace every ink on its own again",
          onclick: () => act.setColourGroups([]),
        },
        "Clear groups",
      ),
    ),
    ...st.colourGroups.map((g, i) => groupCard(store, act, g, i, "group", reports[i] ?? null)),
  );
}

/** What a group becomes: its preview and the words for it. */
function outcome(g: ColourGroup, palette: Ink[]): { member: string; label: string } {
  if (g.target?.startsWith("#")) return { member: g.target.toLowerCase(), label: "custom colour" };
  if (g.target?.startsWith("@")) {
    const m = g.members[Number(g.target.slice(1)) - 1] ?? g.members[0];
    return { member: m, label: isGradient(m) ? "extends this gradient" : "becomes this" };
  }
  // Auto: the member covering the most of the image, as the engine picks it.
  let best = g.members[0];
  let share = -1;
  for (const m of g.members) {
    const s = inkFor(m, palette)?.share ?? 0;
    if (s > share) {
      share = s;
      best = m;
    }
  }
  return { member: best, label: "most-used" };
}

function groupCard(
  store: Store,
  act: PaletteActions,
  g: ColourGroup,
  index: number,
  kind: "group" | "sugg",
  report: MergeReport | null,
): HTMLElement {
  const st = store.state;
  const palette = st.palette;
  const radial = (m: string) => inkFor(m, palette)?.gradient === "radial";
  const result = outcome(g, palette);
  const allKeys = [...new Set([...g.members, result.member].flatMap((m) => keysOf(m, palette)))];

  const edit = (next: ColourGroup) => {
    if (kind === "group") {
      const groups = st.colourGroups.map((x, i) => (i === index ? next : x));
      act.setColourGroups(real(groups));
    } else {
      const suggs = (st.groupSuggestions ?? []).map((x, i) => (i === index ? { ...next, edited: true } : x));
      store.set({ groupSuggestions: real(suggs) });
    }
  };
  // A suggestion is edited through its live form, so its index is the stored one.
  const unmatched = (m: string) => reportedMissing(report, m);

  // A member the re-trace merged away is drawn now as what its group became.
  const drawnAs = (m: string) => {
    const own = keysOf(m, palette);
    return own.length || kind === "sugg" ? own : keysOf(result.member, palette);
  };
  const chips = g.members.map((m, k) => {
    const isTarget = g.target === `@${k + 1}`;
    const chip = h(
      `div.cchip${isTarget ? ".target" : ""}${unmatched(m) ? ".unmatched" : ""}`,
      { "data-fill": JSON.stringify(drawnAs(m)) },
      h(
        "button.cchipmain",
        {
          // The rail is rebuilt after every change; this is how focus finds its way back.
          "data-ctl": `${kind}${index}:${m}`,
          "aria-pressed": String(isTarget),
          title: isTarget
            ? "The group becomes this. Press again for the most-used colour."
            : isGradient(m)
              ? "Make the group this gradient: it is extended over the other members"
              : "Make the group this colour",
          onclick: () => edit({ ...g, target: isTarget ? null : `@${k + 1}` }),
        },
        chipSwatch(m, radial(m)),
        h("span.num", null, memberLabel(m)),
      ),
      h(
        "button.cchipx",
        {
          "data-ctl": `${kind}${index}:${m}:x`,
          "aria-label": `Take ${memberLabel(m)} out of the group`,
          title: "Take it out of the group",
          onclick: () => edit(withoutMember(g, k)),
        },
        icon("x", 11),
      ),
    );
    draggable(chip, store, act, m, { kind, index }, radial(m));
    return chip;
  });

  const custom = h(
    "button.reset",
    {
      title: "Paint the whole group one colour of your choosing",
      onclick: (e: Event) => pickColour(e.currentTarget as HTMLElement, result.member, (hex) => edit({ ...g, target: hex })),
    },
    "Custom…",
  );

  return h(
    `div.cgroup${kind === "sugg" ? ".suggested" : ""}`,
    { "data-drop": kind, "data-index": String(index), "data-fill": JSON.stringify(allKeys) },
    h("div.cchips", null, ...chips),
    h(
      "div.cresult",
      null,
      h("span.arrow", { "aria-hidden": "true" }, "→"),
      h(
        "span.cresultchip",
        { "data-fill": JSON.stringify(keysOf(result.member, palette)) },
        chipSwatch(result.member, radial(result.member)),
        h("span.num", null, memberLabel(result.member)),
      ),
      h("span.muted", null, result.label),
      h("span", { style: { flex: "1" } }),
      custom,
    ),
    kind === "sugg"
      ? h(
          "div.cactions",
          null,
          h(
            "button.btn.compact.primary",
            {
              onclick: () => {
                const rest = (st.groupSuggestions ?? []).filter((_, i) => i !== index);
                store.set({ groupSuggestions: rest });
                act.setColourGroups([...st.colourGroups, { members: g.members, target: g.target }]);
              },
            },
            "Accept",
          ),
          h(
            "button.btn.compact",
            {
              onclick: () =>
                store.set({
                  groupSuggestions: (st.groupSuggestions ?? []).filter((_, i) => i !== index),
                  dismissedGroups: [...st.dismissedGroups, groupSignature(g)],
                }),
            },
            "Dismiss",
          ),
        )
      : [
          reportLine(report),
          h(
            "div.cactions",
            null,
            h("span", { style: { flex: "1" } }),
            g.target
              ? h(
                  "button.reset",
                  { title: "Become the member covering the most of the image", onclick: () => edit({ ...g, target: null }) },
                  "Most-used",
                )
              : null,
            h(
              "button.reset",
              {
                title: "Trace these inks on their own again",
                onclick: () => act.setColourGroups(st.colourGroups.filter((_, i) => i !== index)),
              },
              "Ungroup",
            ),
          ),
        ],
  );
}

/** What the engine said about a group, and a warning when a colour matched nothing. */
function reportLine(report: MergeReport | null): HTMLElement {
  if (!report) return h("span.muted.creport", null, "Merged on the next trace.");
  if (report.leftAlone) {
    return h(
      "span.creport.warn",
      { title: report.line },
      icon("alert", 12),
      "Left alone: fewer than two of these colours are in the image now.",
    );
  }
  if (report.unmatched.length) {
    const named = report.unmatched.map((u) => (isGradient(u) ? "a gradient" : u.toUpperCase())).join(", ");
    return h(
      "span.creport.warn",
      { title: `${report.line}; not found: ${report.unmatched.join(", ")}` },
      icon("alert", 12),
      `${named} ${report.unmatched.length === 1 ? "is" : "are"} no longer in the image; the rest merged.`,
    );
  }
  const said = report.line.replace(/ -> /, " → ").replace(/#[0-9a-f]{6}/gi, (x) => x.toUpperCase());
  return h("span.muted.creport", { title: report.line }, said);
}

// ----------------------------------------------------------- suggestions ---

function suggestionsBlock(store: Store, act: PaletteActions, live: { g: Suggestion; index: number }[]): HTMLElement {
  const st = store.state;
  const inks = st.palette.length;
  const setCount = (n: number | null) => {
    const simplifyTo = n === null ? null : Math.max(1, Math.min(inks, Math.round(n)));
    const kept = (st.groupSuggestions ?? []).filter((g) => g.edited);
    store.set({ simplifyTo, groupSuggestions: proposeGroups({ ...st, simplifyTo, groupSuggestions: kept }) });
  };
  const field = h("input.numberfield.simplify", {
    type: "text",
    inputmode: "numeric",
    value: String(st.simplifyTo ?? inks),
    "aria-label": "Simplify the palette to this many colours",
    onchange: (e: Event) => {
      const n = Number((e.target as HTMLInputElement).value);
      if (Number.isFinite(n) && n >= 1) setCount(n >= inks ? null : n);
    },
  }) as HTMLInputElement;

  return h(
    "div.cgroups.suggestions",
    null,
    h(
      "div.cgroupshead",
      null,
      h("span.eyebrow", null, live.length ? `Suggested · ${live.length}` : "Suggested"),
      live.length > 1
        ? h(
            "button.reset",
            {
              title: "Accept every suggestion below",
              onclick: () => {
                store.set({ groupSuggestions: [] });
                act.setColourGroups([...st.colourGroups, ...live.map(({ g }) => ({ members: g.members, target: g.target }))]);
              },
            },
            "Accept all",
          )
        : null,
    ),
    h(
      "div.simplifyrow",
      null,
      h("span.muted", null, "Colours"),
      h("button.btn.compact.icon", { "aria-label": "One colour fewer", disabled: (st.simplifyTo ?? inks) <= 1, onclick: () => setCount((st.simplifyTo ?? inks) - 1) }, icon("minus", 12)),
      field,
      h(
        "button.btn.compact.icon",
        { "aria-label": "One colour more", disabled: st.simplifyTo === null, onclick: () => setCount((st.simplifyTo ?? inks) + 1 >= inks ? null : (st.simplifyTo ?? inks) + 1) },
        "+",
      ),
      h(
        "span.muted",
        { style: { fontSize: "11px" } },
        st.simplifyTo === null ? `of ${inks} · near duplicates` : `of ${inks} · merging down`,
      ),
    ),
    ...(live.length
      ? live.map(({ g, index }) => groupCard(store, act, g, index, "sugg", null))
      : [h("span.muted", { style: { fontSize: "11px" } }, st.simplifyTo === null ? "No near-duplicate inks: nothing to propose." : "Nothing more to propose at this count.")]),
  );
}

// ------------------------------------------------------------------ inks ---

function inkRow(store: Store, act: PaletteActions, ink: Ink): HTMLElement {
  const st = store.state;
  const member = memberOf(ink);
  const gradient = ink.kind === "gradient";
  const snapped = ink.snappedDe00 !== null;
  const picked = st.paletteSelection.includes(member);
  const grouped = st.colourGroups.some((g) => g.members.some((m) => sameMember(m, member)));

  const main = h(
    "button.inkmain",
    {
      "data-ctl": `ink:${member}`,
      title: gradient ? `${ink.gradient ?? "linear"} gradient · ${ink.stops.map((s) => s.toUpperCase()).join(" → ")}` : "Snap this ink to a colour of your choosing",
      // Not `disabled`: a disabled button swallows the pointer events the row's drag needs.
      "aria-disabled": gradient ? "true" : null,
      onclick: (e: Event) => {
        if (!gradient) openSnap(e.currentTarget as HTMLElement, ink, act);
      },
    },
    h(
      `span.swatch${snapped ? ".snapped" : ""}${gradient ? ".ramp" : ""}`,
      null,
      h("i", { style: { background: gradient ? swatchBackground(member, ink.gradient === "radial") : ink.hex } }),
    ),
    h("span.hex", null, gradient ? (ink.gradient === "radial" ? "radial" : "linear") : ink.hex.toUpperCase()),
    h("span.share", null, percent(ink.share)),
    h(
      `span.snap${snapped ? ".on" : ""}`,
      null,
      gradient ? `${ink.stops.length} stops` : snapped ? `snapped · ΔE ${de00(ink.snappedDe00)}` : "snap to…",
    ),
  );
  const toggle = () =>
    store.set({
      paletteSelection: picked ? st.paletteSelection.filter((m) => m !== member) : [...st.paletteSelection, member],
    });
  const row = h(
    `div.ink${grouped ? ".grouped" : ""}`,
    { "data-drop": "ink", "data-member": member, "data-fill": JSON.stringify(ink.keys) },
    h(
      "button.inkpick",
      {
        "data-ctl": `pick:${member}`,
        role: "checkbox",
        "aria-checked": String(picked),
        "aria-label": `Tick ${gradient ? "this gradient" : ink.hex.toUpperCase()} to merge it`,
        title: "Tick inks, then Merge to draw them as one. Or drag one onto another.",
        onclick: toggle,
        // Space ticks a tick box. Elsewhere in the Vectorize tab it is held to flick to the
        // source, and that handler, on the window, would otherwise swallow the press.
        onkeydown: (e: KeyboardEvent) => {
          if (e.key !== " ") return;
          e.preventDefault();
          e.stopPropagation();
          if (!e.repeat) toggle();
        },
      },
      picked ? icon("check", 11) : null,
    ),
    main,
  );
  draggable(row, store, act, member, { kind: "palette" }, ink.gradient === "radial");
  return row;
}

/** A colour picker in a popover: a native picker and a hex field. */
function pickColour(anchor: HTMLElement, start: string, done: (hex: string) => void): void {
  const first = stopsOf(start)[0] ?? "#000000";
  const field = h("input.numberfield", {
    type: "text",
    value: first.toUpperCase(),
    spellcheck: "false",
    style: { width: "100%", textAlign: "left", height: "30px", padding: "0 9px" },
  }) as HTMLInputElement;
  const picker = h("input", {
    type: "color",
    value: first,
    style: { width: "100%", height: "60px", border: "none", background: "none", padding: "0" },
    oninput: () => {
      field.value = (picker as HTMLInputElement).value.toUpperCase();
    },
  });
  const commit = () => {
    const v = field.value.trim().toLowerCase();
    const hex = v.startsWith("#") ? v : `#${v}`;
    if (!/^#[0-9a-f]{6}$/.test(hex)) {
      toast(`${field.value} is not a colour; write it as #RRGGBB.`, { kind: "bad" });
      return;
    }
    done(hex);
    closeOverlay();
  };
  openPopover(
    anchor,
    h(
      "div",
      { style: { width: "260px", padding: "12px", display: "flex", flexDirection: "column", gap: "11px" } },
      picker,
      h("span.muted", { style: { fontSize: "11px" } }, "The group becomes"),
      h(
        "div",
        { style: { display: "flex", gap: "6px" } },
        field,
        h(
          "button.btn.compact",
          {
            style: { background: "var(--accent)", borderColor: "var(--accent)", color: "var(--accent-ink)", fontWeight: "600" },
            onclick: commit,
          },
          "Use",
        ),
      ),
    ),
  );
  field.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") commit();
  });
}

/** The colour-snap popover: a picker, a "snap to" field, and what it would cost. */
function openSnap(anchor: HTMLElement, ink: Ink, act: PaletteActions): void {
  const field = h("input.numberfield", {
    type: "text",
    value: ink.hex.toUpperCase(),
    spellcheck: "false",
    style: { width: "100%", textAlign: "left", height: "30px", padding: "0 9px" },
  }) as HTMLInputElement;
  const picker = h("input", {
    type: "color",
    value: ink.hex,
    style: { width: "100%", height: "60px", border: "none", background: "none", padding: "0" },
    oninput: () => {
      field.value = (picker as HTMLInputElement).value.toUpperCase();
    },
  });
  const note = h(
    "span",
    { style: { fontSize: "11.5px", color: "var(--gold)" } },
    "You are overriding a measurement.",
  );

  openPopover(
    anchor,
    h(
      "div",
      { style: { width: "300px", padding: "12px", display: "flex", flexDirection: "column", gap: "11px" } },
      picker,
      h(
        "div",
        { style: { display: "flex", gap: "8px", alignItems: "center" } },
        h("span.swatch", null, h("i", { style: { background: ink.traced } })),
        h(
          "div",
          { style: { display: "flex", flexDirection: "column" } },
          h("span.muted", { style: { fontSize: "11px" } }, "traced"),
          h("span.num", { style: { fontSize: "12.5px" } }, `${ink.traced.toUpperCase()} · ${percent(ink.share)} of canvas`),
        ),
      ),
      h("span.muted", { style: { fontSize: "11px" } }, "Snap to"),
      h(
        "div",
        { style: { display: "flex", gap: "6px" } },
        field,
        h(
          "button.btn.compact",
          {
            style: { background: "var(--accent)", borderColor: "var(--accent)", color: "var(--accent-ink)", fontWeight: "600" },
            onclick: () => {
              act.snap([{ from: ink.traced, to: field.value.trim().toLowerCase() }]);
              closeOverlay();
            },
          },
          "Snap",
        ),
      ),
      note,
    ),
  );
}

/**
 * The palette as CSS custom properties.
 *
 * Named by position rather than by colour, because `--ink-1` survives a re-trace that
 * moves the hue and `--dark-green` does not. The share goes in a comment: it is the
 * reason the order is what it is, and it is the first thing you want when deciding which
 * of four inks is the brand colour.
 */
async function copyPaletteCss(palette: Ink[]): Promise<void> {
  const body = palette
    .map((ink, i) => `  --ink-${i + 1}: ${ink.hex.toLowerCase()}; /* ${percent(ink.share)} of canvas */`)
    .join("\n");
  try {
    await copyText(`:root {\n${body}\n}\n`);
    toast(`${palette.length} ink${palette.length === 1 ? "" : "s"} copied as CSS.`, { kind: "good" });
  } catch (e) {
    toast(String(e), { kind: "bad" });
  }
}

/** Paste a brand palette and see what each match would cost before committing. */
function pastePalette(store: Store, act: PaletteActions): void {
  // Snapping rewrites flat fills; a gradient has no one colour to snap.
  const flats = store.state.palette.filter((i) => i.kind !== "gradient");
  const area = h("textarea", {
    rows: "5",
    spellcheck: "false",
    placeholder: "#12443E\n#E9B24C\nrgb(207, 198, 180)",
    style: {
      width: "100%",
      border: "1px solid var(--rule2)",
      borderRadius: "var(--radius)",
      background: "var(--paper)",
      color: "var(--ink)",
      padding: "10px 12px",
      font: "12px/1.8 var(--font-mono)",
      resize: "vertical",
    },
  }) as HTMLTextAreaElement;
  const preview = h("div", { style: { display: "flex", flexDirection: "column", gap: "7px" } });
  let matches: { from: string; to: string; de00: number }[] = [];

  const update = async () => {
    matches = await api.matchPalette(flats.map((i) => i.traced), area.value);
    fill(
      preview,
      ...matches.map((m) =>
        h(
          "div",
          { style: { display: "flex", alignItems: "center", gap: "9px", fontSize: "12px" }, class: "num" },
          h("span.swatch", { style: { width: "20px", height: "20px" } }, h("i", { style: { background: m.from } })),
          h("span.muted", null, m.from.toUpperCase()),
          h("span.muted", null, "→"),
          h("span.swatch", { style: { width: "20px", height: "20px" } }, h("i", { style: { background: m.to } })),
          h("span.dim", null, m.to.toUpperCase()),
          h(
            "span",
            { style: { marginLeft: "auto", color: m.de00 < 0.5 ? "var(--good)" : "var(--gold)" } },
            m.de00 < 0.05 ? "exact" : `${de00(m.de00)} dE00`,
          ),
        ),
      ),
    );
  };
  area.addEventListener("input", () => void update());

  openModal(
    modal(
      "Paste a brand palette",
      [
        area,
        h("span.muted", { style: { fontSize: "11.5px" } }, "Hex, RGB or CSS variables, one per line or separated by commas. We match each to the nearest traced ink."),
        preview,
      ],
      [
        h("button.btn", { onclick: closeOverlay }, "Cancel"),
        h(
          "button.btn.primary",
          {
            onclick: () => {
              act.snap(matches.map((m) => ({ from: m.from, to: m.to })));
              closeOverlay();
            },
          },
          `Snap ${flats.length} inks`,
        ),
      ],
    ),
  );
}
