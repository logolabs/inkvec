import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { focusables, heldCount, holdModal } from "./dialog";

/** The page as the app builds it: the app, the overlay layer and the toasts' live region. */
function page(): { app: HTMLElement; stage: HTMLElement; rail: HTMLElement; sheet: HTMLElement; opener: HTMLButtonElement; buttons: HTMLButtonElement[] } {
  document.body.innerHTML = `
    <div id="app">
      <header id="bar"><button id="open">Open</button></header>
      <div class="body">
        <main id="stage"><button id="zoom">Fit</button></main>
        <aside id="rail">
          <div id="foot"><button id="export">Export</button></div>
          <div class="sheet"><div id="sheet" role="dialog" aria-modal="true">
            <button id="a">Close</button><button id="b">SVG</button><button id="hidden" hidden>x</button><button id="c" disabled>Go</button><button id="d">Download</button>
          </div></div>
        </aside>
      </div>
    </div>
    <div id="overlays"></div>
    <div id="toasts" role="status"></div>`;
  const $ = (id: string) => document.getElementById(id) as HTMLElement;
  const opener = $("export") as HTMLButtonElement;
  opener.focus();
  return { app: $("app"), stage: $("stage"), rail: $("rail"), sheet: $("sheet"), opener, buttons: ["a", "b", "d"].map((id) => $(id) as HTMLButtonElement) };
}

/** A key pressed on whatever has focus, as the browser would deliver it. */
function press(key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  (document.activeElement ?? document.body).dispatchEvent(e);
  return e;
}

describe("holdModal: the WAI-ARIA modal dialog contract", () => {
  let releases: ((returnFocus?: boolean) => void)[] = [];
  const hold = (...args: Parameters<typeof holdModal>) => {
    const r = holdModal(...args);
    releases.push(r);
    return r;
  };
  beforeEach(() => {
    releases = [];
  });
  afterEach(() => {
    for (const r of releases.reverse()) r(false);
    expect(heldCount()).toBe(0);
  });

  it("lists only what Tab can reach: no disabled or hidden buttons", () => {
    const { sheet, buttons } = page();
    expect(focusables(sheet)).toEqual(buttons);
  });

  it("moves focus in on open, to the first focusable element or the one asked for", () => {
    const { sheet, buttons } = page();
    hold(sheet);
    expect(document.activeElement).toBe(buttons[0]);
    releases.pop()!(false);
    hold(sheet, { initial: () => buttons[1] });
    expect(document.activeElement).toBe(buttons[1]);
  });

  it("wraps Tab from the last element to the first and Shift+Tab from the first to the last", () => {
    const { sheet, buttons } = page();
    hold(sheet);
    buttons[2].focus();
    const tab = press("Tab");
    expect(tab.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(buttons[0]);
    const back = press("Tab", { shiftKey: true });
    expect(back.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(buttons[2]);
    // In the middle the browser's own order is right, so the press is left alone.
    buttons[0].focus();
    expect(press("Tab").defaultPrevented).toBe(false);
  });

  it("brings a Tab pressed outside the dialog back into it", () => {
    const { sheet, buttons } = page();
    hold(sheet);
    (document.activeElement as HTMLElement).blur();
    press("Tab");
    expect(document.activeElement).toBe(buttons[0]);
  });

  it("closes on Escape, and the window behind never hears it", () => {
    const { sheet } = page();
    const onEscape = vi.fn();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    hold(sheet, { onEscape });
    press("Escape");
    expect(onEscape).toHaveBeenCalledTimes(1);
    expect(behind).not.toHaveBeenCalled();
    window.removeEventListener("keydown", behind);
  });

  it("keeps single-key shortcuts inside, and lets Ctrl and Cmd shortcuts through", () => {
    const { sheet } = page();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    hold(sheet);
    press(" ");
    press("+");
    expect(behind).not.toHaveBeenCalled();
    press("o", { ctrlKey: true });
    press("o", { metaKey: true });
    expect(behind).toHaveBeenCalledTimes(2);
    window.removeEventListener("keydown", behind);
  });

  it("makes the rest of the app inert, never the toasts or the overlay layer, and undoes it on release", () => {
    const { sheet, stage, opener } = page();
    const release = hold(sheet);
    const bar = document.getElementById("bar")!;
    const foot = document.getElementById("foot")!;
    expect([bar.inert, stage.inert, foot.inert]).toEqual([true, true, true]);
    expect(document.getElementById("toasts")!.inert).toBeFalsy();
    expect(document.getElementById("overlays")!.inert).toBeFalsy();
    expect(sheet.inert).toBeFalsy();
    release();
    expect([bar.inert, stage.inert, foot.inert]).toEqual([false, false, false]);
    expect(document.activeElement).toBe(opener);
  });

  it("returns focus to the fallback when the opener has left the page", () => {
    const { sheet, opener } = page();
    const zoom = document.getElementById("zoom") as HTMLElement;
    const release = hold(sheet, { fallback: () => zoom });
    opener.remove();
    release();
    expect(document.activeElement).toBe(zoom);
  });

  it("stacks: a modal over the sheet hands it back with its own inert intact", () => {
    const { sheet, stage } = page();
    hold(sheet);
    const modal = document.createElement("div");
    modal.innerHTML = "<button id='ok'>OK</button>";
    document.getElementById("overlays")!.append(modal);
    const inner = hold(modal);
    const app = document.getElementById("app")!;
    expect(app.inert).toBe(true);
    expect(document.activeElement?.id).toBe("ok");
    inner();
    expect(app.inert).toBe(false);
    // The sheet's own share is still in place, and its trap is back on top.
    expect(stage.inert).toBe(true);
    expect(heldCount()).toBe(1);
  });

  it("reports a press outside the dialog, but not one inside it or on a toast", () => {
    const { sheet, stage, buttons } = page();
    const onOutside = vi.fn();
    hold(sheet, { onOutside });
    buttons[1].dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    document.getElementById("toasts")!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    expect(onOutside).not.toHaveBeenCalled();
    stage.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    expect(onOutside).toHaveBeenCalledTimes(1);
  });

  it("is released once however many times release is called", () => {
    const { sheet } = page();
    const release = hold(sheet);
    release();
    release();
    expect(heldCount()).toBe(0);
  });
});
