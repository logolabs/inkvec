/**
 * "Auto chose": the line that says what the automatic trace used and what it noticed, with
 * the alternatives one click away.
 *
 * Shown in the chooser card over the stage while that is up, and at the top of the Result
 * tab after. It never changes a trace on its own: every button here is an ordinary preset
 * or colour-group change that the Tune tab shows as changed, and "Back to Auto" puts the
 * controls back where the automatic trace had them.
 */

import { h } from "../lib/dom";
import type { ColourGroup } from "../lib/ipc";
import { groupSignature } from "../lib/colour";
import { type Store } from "../lib/state";
import { autoSuggestions, changedControls, describeAuto, type AutoSuggestion } from "../lib/suggest";
import { liveSuggestions } from "./palette";

export interface AutoChoseActions {
  setPreset(id: string): void;
  setColourGroups(groups: ColourGroup[]): void;
  /** Put the controls back where the automatic trace had them; a new trace follows. */
  backToAuto(): void;
}

/** What Auto's own trace suggests, read from its report rather than whatever is on screen now. */
export function suggestionsOf(store: Store): AutoSuggestion[] {
  const st = store.state;
  const auto = st.auto;
  if (!auto?.report) return [];
  return autoSuggestions({
    source: st.source,
    report: auto.report,
    palette: auto.palette,
    losses: auto.losses,
    facts: st.facts,
    settings: auto.settings,
    preset: auto.preset,
  });
}

/** Whether the controls have moved away from what Auto traced with. */
export function movedFromAuto(store: Store): boolean {
  const st = store.state;
  if (!st.auto) return false;
  return st.preset !== st.auto.preset || changedControls(st.caps, st.settings, st.auto.settings).length > 0;
}

/** Whether there is anything worth a note: an alternative, a colour group, or a change to undo. */
export function hasAutoNews(store: Store): boolean {
  const st = store.state;
  if (!st.auto?.report) return false;
  return (
    movedFromAuto(store) ||
    liveSuggestions(st).length > 0 ||
    suggestionsOf(store).length > 0
  );
}

/**
 * The block itself. `compact` drops the sentence about the settings, for the rail, where
 * the Tune tab says the same thing.
 */
export function autoChose(store: Store, act: AutoChoseActions, compact = false): HTMLElement | null {
  const st = store.state;
  const auto = st.auto;
  if (!auto?.report) return null;
  // Read against Auto's own settings: an alternative already taken is not suggested again,
  // and one that is taken is shown as the change it is.
  const suggestions = suggestionsOf(store);
  const groups = liveSuggestions(st);
  const moved = movedFromAuto(store);
  const current = st.caps?.presets.find((p) => p.id === st.preset);

  const rows: HTMLElement[] = suggestions.map((s) => {
    const taken = st.preset === s.preset;
    return h(
      "div.autorow",
      null,
      h(
        "div.autotext",
        null,
        h("span.autotitle", null, s.title),
        h("span.muted.num", null, s.evidence),
      ),
      taken
        ? h("span.autotaken", null, "chosen")
        : h(
            "button.btn.compact",
            {
              "data-ctl": `auto-try:${s.id}`,
              title: `Switch to ${s.action}. An ordinary preset: Back to Auto undoes it.`,
              onclick: () => act.setPreset(s.preset),
            },
            `Try ${s.action}`,
          ),
    );
  });

  if (groups.length) {
    const members = groups.reduce((a, { g }) => a + g.members.length, 0);
    rows.push(
      h(
        "div.autorow",
        null,
        h(
          "div.autotext",
          null,
          h("span.autotitle", null, `${groups.length} colour group${groups.length === 1 ? "" : "s"} suggested`),
          h("span.muted.num", null, `${members} inks that look like the same colour measured twice`),
        ),
        h(
          "div.autobtns",
          null,
          h(
            "button.btn.compact",
            {
              "data-ctl": "auto-groups-accept",
              title: "Draw each suggested group as one colour; the trace runs again so the shapes join",
              onclick: () => {
                store.set({ groupSuggestions: [] });
                act.setColourGroups([...st.colourGroups, ...groups.map(({ g }) => ({ members: g.members, target: g.target }))]);
              },
            },
            "Accept",
          ),
          h(
            "button.reset",
            {
              "data-ctl": "auto-groups-dismiss",
              onclick: () =>
                store.set({
                  groupSuggestions: [],
                  dismissedGroups: [...st.dismissedGroups, ...groups.map(({ g }) => groupSignature(g))],
                }),
            },
            "Dismiss",
          ),
        ),
      ),
    );
  }

  return h(
    "div.autochose",
    null,
    compact ? null : h("span.autolead", null, `Auto traced with ${describeAuto(st.caps, auto.settings, auto.preset)}.`),
    ...rows,
    !rows.length && !compact ? h("span.muted", null, "Nothing in its report suggests another preset.") : null,
    moved
      ? h(
          "div.autorow.moved",
          null,
          h(
            "div.autotext",
            null,
            h("span.autotitle", null, current ? `Now: ${current.name}` : "Now: your own settings"),
            h("span.muted", null, "An ordinary setting, shown under Tune"),
          ),
          h("button.reset", { "data-ctl": "auto-back", onclick: act.backToAuto }, "Back to Auto"),
        )
      : null,
  );
}
