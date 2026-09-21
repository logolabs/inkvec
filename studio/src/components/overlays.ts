/**
 * Toasts, modals, popovers and tooltips.
 *
 * Modals are reserved for things the user asked for: a download, an update, a bulk paste.
 * Nothing here is timed, and nothing here sells.
 */

import { fill, h, icon } from "../lib/dom";

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

/** Close whatever overlay is open. */
export function closeOverlay(): void {
  fill(layer());
  document.removeEventListener("keydown", onEscape, true);
}

function onEscape(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.stopPropagation();
    closeOverlay();
  }
}

/**
 * Put `content` on screen over a scrim.
 *
 * Escape closes, clicking the scrim closes, and focus moves into the dialog so the
 * keyboard does not stay behind in the window underneath.
 */
export function openModal(content: HTMLElement): void {
  const scrim = h("div.scrim", {
    onmousedown: (e: MouseEvent) => {
      if (e.target === scrim) closeOverlay();
    },
  });
  content.setAttribute("role", "dialog");
  content.setAttribute("aria-modal", "true");
  scrim.append(content);
  fill(layer(), scrim);
  document.addEventListener("keydown", onEscape, true);
  (content.querySelector<HTMLElement>("button, input, [tabindex]") ?? content).focus();
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
 * Hover and keyboard focus both show it, because the advanced drawer's tooltips carry the
 * engine's own documentation and somebody driving the app from the keyboard needs it as
 * much as somebody with a mouse.
 */
export function tip(el: HTMLElement, text: string): HTMLElement {
  let node: HTMLElement | null = null;
  let timer = 0;

  const show = () => {
    if (node) return;
    node = h("div.tooltip", { role: "tooltip" }, text);
    document.body.append(node);
    const a = el.getBoundingClientRect();
    const b = node.getBoundingClientRect();
    const left = Math.min(Math.max(8, a.left), window.innerWidth - b.width - 8);
    const above = a.top - b.height - 8;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(above < 8 ? a.bottom + 8 : above)}px`;
  };
  const hide = () => {
    window.clearTimeout(timer);
    node?.remove();
    node = null;
  };

  el.addEventListener("pointerenter", () => {
    timer = window.setTimeout(show, 350);
  });
  el.addEventListener("pointerleave", hide);
  el.addEventListener("focus", show);
  el.addEventListener("blur", hide);
  return el;
}

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
