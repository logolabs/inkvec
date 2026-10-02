/**
 * Toasts, modals, popovers and tooltips.
 *
 * Modals are reserved for things the user asked for: a download, an update, a bulk paste.
 * Nothing here is timed, and nothing here sells.
 */

import { fill, h, icon } from "../lib/dom";
import { holdModal } from "./dialog";

const layer = () => document.getElementById("overlays") as HTMLElement;
const toastLayer = () => document.getElementById("toasts") as HTMLElement;

/** A toast: a fact and at most one thing to do about it. */
export function toast(
  text: string,
  options: { kind?: "good" | "bad" | "plain"; action?: { label: string; run: () => void } } = {},
): void {
  const kind = options.kind ?? "plain";
  const glyph = kind === "good" ? "checkCircle" : kind === "bad" ? "alert" : "info";
  const el = h(
    "div.toast",
    { role: "status" },
    h(`span.glyph.${kind}`, null, icon(glyph, 16)),
    h("span.text", null, text),
    options.action &&
      h(
        "button",
        {
          class: "reset",
          onclick: () => {
            options.action?.run();
            el.remove();
          },
        },
        options.action.label,
      ),
  );
  toastLayer().append(el);
  // Holds six seconds, then goes. Long enough to read a path, short enough not to
  // become furniture.
  window.setTimeout(() => el.remove(), 6000);
}

/** The modal on screen, held by `holdModal`; releasing it gives the window its focus back. */
let releaseModal: ((returnFocus?: boolean) => void) | null = null;
/** What had focus when a popover opened, to go back to when it closes. */
let popoverOpener: HTMLElement | null = null;

/** Close whatever overlay is open, and put the keyboard back where it was before it opened. */
export function closeOverlay(): void {
  const layerHadFocus = layer().contains(document.activeElement);
  fill(layer());
  document.removeEventListener("keydown", onEscape, true);
  const release = releaseModal;
  releaseModal = null;
  release?.();
  const opener = popoverOpener;
  popoverOpener = null;
  // A popover's item may already have moved focus on purpose (a file dialog, another
  // view); only focus left in the emptied layer, or on nothing, goes back to the button.
  if (opener?.isConnected && (layerHadFocus || document.activeElement === document.body)) opener.focus({ preventScroll: true });
}

function onEscape(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.stopPropagation();
    closeOverlay();
  }
}

/** Bumped per modal, so each one's heading gets an id of its own. */
let modalSeq = 0;

/**
 * Put `content` on screen over a scrim, as a modal dialog (`dialog.ts`): focus moves in,
 * Tab stays inside, Escape or a press on the scrim closes it, and focus goes back to what
 * had it. The dialog is named by its first heading when it has no name of its own.
 */
export function openModal(content: HTMLElement): void {
  // One modal at a time: the layer is about to be replaced, so the last one lets go first.
  releaseModal?.(false);
  releaseModal = null;
  const scrim = h("div.scrim", {
    onmousedown: (e: MouseEvent) => {
      if (e.target === scrim) closeOverlay();
    },
  });
  content.setAttribute("role", "dialog");
  content.setAttribute("aria-modal", "true");
  const heading = content.querySelector<HTMLElement>("h1, h2, h3");
  if (heading && !content.hasAttribute("aria-label") && !content.hasAttribute("aria-labelledby")) {
    heading.id ||= `modal-title-${++modalSeq}`;
    content.setAttribute("aria-labelledby", heading.id);
  }
  scrim.append(content);
  fill(layer(), scrim);
  document.removeEventListener("keydown", onEscape, true);
  releaseModal = holdModal(content, {
    initial: () => content.querySelector<HTMLElement>("button, input, select, textarea, [tabindex]:not([tabindex='-1'])"),
    onEscape: closeOverlay,
  });
}

/** A modal built from the usual parts. */
export function modal(
  title: string,
  body: (HTMLElement | string | null | false)[],
  actions: HTMLElement[] = [],
): HTMLElement {
  return h(
    "div.modal",
    { tabindex: "-1" },
    h("h2", null, title),
    ...body.filter(Boolean).map((b) => (typeof b === "string" ? h("p", null, b) : (b as HTMLElement))),
    actions.length ? h("div.actions", null, ...actions) : null,
  );
}

/** Anchor a popover to an element, kept inside the window. */
export function openPopover(anchor: HTMLElement, content: HTMLElement): void {
  releaseModal?.(false);
  releaseModal = null;
  popoverOpener = anchor;
  const box = h("div.popover", null, content);
  const scrim = h("div", {
    style: { position: "fixed", inset: "0", zIndex: "44" },
    onmousedown: closeOverlay,
  });
  fill(layer(), scrim, box);
  document.addEventListener("keydown", onEscape, true);

  const a = anchor.getBoundingClientRect();
  const b = box.getBoundingClientRect();
  const margin = 8;
  const left = Math.min(Math.max(margin, a.left), window.innerWidth - b.width - margin);
  const below = a.bottom + 6;
  const top = below + b.height + margin > window.innerHeight ? a.top - b.height - 6 : below;
  box.style.left = `${Math.round(left)}px`;
  box.style.top = `${Math.round(Math.max(margin, top))}px`;
  (box.querySelector<HTMLElement>("button, input, [tabindex]") ?? box).focus?.();
}

/**
 * Attach a tooltip to an element.
 *
 * Hover and keyboard focus both show it, because the Tune tab's tooltips carry the
 * engine's own documentation and somebody driving the app from the keyboard needs it as
 * much as somebody with a mouse.
 *
 * There is only ever one, and it cannot outlive what it describes. The rail is rebuilt
 * whenever a control or a trace changes, so the label a tooltip belongs to is routinely
 * removed from the page while the pointer is still over it — and a removed element never
 * fires `pointerleave` or `blur`, which is how tooltips used to be left stuck on screen. A
 * tooltip therefore checks, while it is showing, that its label is still in the page and
 * still hovered or keyboard-focused, and goes the moment that stops being true. A click
 * dismisses it too: pressing the thing it explains is the answer to "what is this".
 */
let showing: { node: HTMLElement; el: HTMLElement } | null = null;
let watchdog = 0;

function hideTip(): void {
  window.cancelAnimationFrame(watchdog);
  showing?.node.remove();
  showing = null;
}

function watch(): void {
  const t = showing;
  if (!t) return;
  const held = t.el.isConnected && (t.el.matches(":hover") || t.el.matches(":focus-visible"));
  if (!held) {
    hideTip();
    return;
  }
  watchdog = window.requestAnimationFrame(watch);
}

/**
 * Give `el` a tooltip of `text`: shown 350 ms into a hover, or at once on keyboard focus
 * (not on focus from a click), above `el` or below it when there is no room, kept inside the
 * window. It goes when the pointer or focus leaves, on a press, or on Escape. Returns `el`,
 * so a label can be wrapped where it is built.
 */
export function tip(el: HTMLElement, text: string): HTMLElement {
  let timer = 0;

  const show = () => {
    // The label may have been rebuilt away during the delay; a tooltip for an element that
    // is not on the page has no position, and would land in the corner.
    if (!el.isConnected || showing?.el === el) return;
    hideTip();
    const node = h("div.tooltip", { role: "tooltip" }, text);
    document.body.append(node);
    const a = el.getBoundingClientRect();
    const b = node.getBoundingClientRect();
    const left = Math.min(Math.max(8, a.left), window.innerWidth - b.width - 8);
    const above = a.top - b.height - 8;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(above < 8 ? a.bottom + 8 : above)}px`;
    showing = { node, el };
    watchdog = window.requestAnimationFrame(watch);
  };
  const hide = () => {
    window.clearTimeout(timer);
    if (showing?.el === el) hideTip();
  };

  el.addEventListener("pointerenter", () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(show, 350);
  });
  el.addEventListener("pointerleave", hide);
  el.addEventListener("pointerdown", hide);
  // Focus from a click is not a request for a tooltip; focus from the keyboard is.
  el.addEventListener("focus", () => {
    if (el.matches(":focus-visible")) show();
  });
  el.addEventListener("blur", hide);
  return el;
}

// Escape closes a tooltip before it does anything else.
document.addEventListener(
  "keydown",
  (e) => {
    if (e.key === "Escape" && showing) hideTip();
  },
  true,
);
window.addEventListener("blur", hideTip);

/** A confirmation with a named consequence, for the few things that destroy something. */
export function confirm(
  title: string,
  body: string,
  confirmLabel: string,
  run: () => void,
  danger = false,
): void {
  openModal(
    modal(title, [body], [
      h("button.btn", { onclick: closeOverlay }, "Cancel"),
      h(
        `button.btn.primary${danger ? ".danger" : ""}`,
        {
          onclick: () => {
            closeOverlay();
            run();
          },
        },
        confirmLabel,
      ),
    ]),
  );
}
