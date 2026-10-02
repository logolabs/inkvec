/**
 * Modal dialog behaviour, shared by the export sheet and the modals over a scrim.
 *
 * Method from: W3C WAI-ARIA Authoring Practices Guide, "Dialog (Modal) Pattern"
 * (https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/). Its keyboard contract is what
 * `holdModal` implements, step by step:
 *
 * 1. **Focus in.** When the dialog opens, focus moves to an element inside it: the one the
 *    caller names, else the first focusable element, else the dialog itself (`tabindex=-1`).
 * 2. **Tab trap.** Tab from the last focusable element goes to the first, Shift+Tab from the
 *    first goes to the last, and a Tab pressed while focus is outside the dialog (on the page
 *    body after a click on blank space) comes back into it.
 * 3. **Escape** closes it.
 * 4. **Focus return.** On close, focus goes back to the element that had it when the dialog
 *    opened (the Export button, the viewer button Ctrl+E was pressed on), if it is still on
 *    the page.
 *
 * Adapted, and why:
 * - The rest of the page is made `inert` (HTML Living Standard, "inert subtrees",
 *   https://html.spec.whatwg.org/multipage/interaction.html#inert-subtrees) rather than
 *   `aria-hidden`: inert takes the outside out of the accessibility tree *and* out of focus
 *   and pointer hit-testing, which is what `aria-modal="true"` promises a screen reader. The
 *   APG leaves the mechanism open; `inert` is the one the platform now provides. The
 *   live-region hosts (`#toasts`) and the overlay layer (`#overlays`) are never made inert:
 *   a toast must still be announced, and a modal opened from inside the dialog lives there.
 * - Keys pressed inside the dialog stop at the dialog, unless Ctrl or Cmd is held, so the
 *   window's single-key shortcuts (Space flicks the viewer, +/- zoom, Escape cancels a trace)
 *   never act on the app behind it. The shortcuts with a modifier, and the function keys
 *   (F1, the guide), still work.
 * - Dialogs stack: only the top one handles Tab and Escape, and each undoes only the `inert`
 *   it set itself, so a modal opened over the export sheet hands the sheet back intact.
 *
 * Not from the literature: the outside-pointer close (`onOutside`), because the APG pattern
 * is keyboard-only and says nothing about pointers. The export sheet has always closed on a
 * press over its backdrop; with the stage now inert, a press anywhere outside the sheet does
 * the same rather than nothing. See also: the HTML `<dialog>` element's `closedby="any"`
 * light dismiss (https://html.spec.whatwg.org/multipage/interactive-elements.html#the-dialog-element),
 * which behaves the same way but is not in every webview Tauri targets.
 */

/** What makes an element reachable by Tab, before visibility is checked. */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "iframe",
  "[tabindex]:not([tabindex='-1'])",
  "[contenteditable='true']",
].join(",");

/** The page-level hosts that stay live while a dialog is open (see the module comment). */
const NEVER_INERT = "#toasts, #overlays, .tooltip, script, style, link";

/**
 * The elements inside `root` that Tab can reach, in document order.
 *
 * Disabled, hidden (`[hidden]`, `display: none`, an inert subtree) and zero-size elements are
 * skipped, because the browser skips them too and a trap that wraps to an element the browser
 * will not focus leaves focus where it was. `checkVisibility` is the precise test where the
 * engine has it; otherwise an element with no layout boxes (`getClientRects` empty) is hidden.
 * Cost: one `querySelectorAll` and one visibility read per candidate, on a dialog of tens of
 * elements.
 */
export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => {
    if (el.closest("[hidden], [inert]")) return false;
    const check = (el as HTMLElement & { checkVisibility?: () => boolean }).checkVisibility;
    if (typeof check === "function") return check.call(el);
    return el.getClientRects().length > 0;
  });
}

/** How a dialog is held open; see `holdModal`. */
export interface HoldOptions {
  /** Where focus goes when the dialog opens; the first focusable element when absent. */
  initial?: () => HTMLElement | null | undefined;
  /** Escape was pressed while this was the top dialog. Not given: Escape is left alone. */
  onEscape?: () => void;
  /** A press landed outside the dialog (see the module comment). */
  onOutside?: () => void;
  /** Where focus goes on release when the opener is gone; the opener when absent. */
  fallback?: () => HTMLElement | null | undefined;
}

/** One dialog held open: its root, what it was opened with, and the `inert` it set. */
interface Held {
  root: HTMLElement;
  opts: HoldOptions;
  opener: HTMLElement | null;
  inerted: HTMLElement[];
}

/** The dialogs held open, bottom to top. Only the last handles Tab and Escape. */
const stack: Held[] = [];

/**
 * Every element that is a sibling of `root` or of one of its ancestors, up to `<body>`: the
 * smallest set whose `inert` leaves `root` (and only `root`, plus the never-inert hosts)
 * reachable. Linear in the depth of `root` times the siblings at each level, a few dozen
 * elements in this app.
 */
function outside(root: HTMLElement): HTMLElement[] {
  const out: HTMLElement[] = [];
  for (let node: HTMLElement | null = root; node && node !== document.body; node = node.parentElement) {
    const parent: HTMLElement | null = node.parentElement;
    if (!parent) break;
    for (const sib of parent.children) {
      if (sib !== node && sib instanceof HTMLElement && !sib.matches(NEVER_INERT)) out.push(sib);
    }
  }
  return out;
}

/** Tab and Escape for the top dialog, wherever focus is; on the document, in capture. */
function onKey(e: KeyboardEvent): void {
  const top = stack[stack.length - 1];
  if (!top) return;
  if (e.key === "Escape" && top.opts.onEscape) {
    e.preventDefault();
    e.stopPropagation();
    top.opts.onEscape();
    return;
  }
  if (e.key !== "Tab") return;
  const list = focusables(top.root);
  const active = document.activeElement as HTMLElement | null;
  const inside = Boolean(active && top.root.contains(active));
  if (!list.length) {
    // Nothing to move to: focus stays on the dialog itself.
    e.preventDefault();
    top.root.focus();
    return;
  }
  const first = list[0];
  const last = list[list.length - 1];
  // The wrap. Within the list the browser's own order is already right, so only the two ends
  // (and a Tab from outside, which would otherwise start from the page's top) are handled.
  if (!inside || (e.shiftKey && active === first) || (!e.shiftKey && active === last)) {
    e.preventDefault();
    (e.shiftKey ? last : first).focus();
  }
}

/** A press outside the top dialog; on the document, in capture, before anything acts on it. */
function onPointer(e: PointerEvent): void {
  const top = stack[stack.length - 1];
  const target = e.target as Element | null;
  if (!top?.opts.onOutside || !target || top.root.contains(target)) return;
  // A toast's action and a tooltip are not "outside" in any sense the user means.
  if (target.closest?.(NEVER_INERT)) return;
  top.opts.onOutside();
}

/**
 * Single-key shortcuts stop at the dialog; see the module comment. Bubble phase, on the root.
 * Modified keys and the function keys go on to the window: F1 over Settings still opens the
 * guide at the Settings page.
 */
function stopAtDialog(e: KeyboardEvent): void {
  if (e.ctrlKey || e.metaKey || e.key === "Tab" || /^F\d{1,2}$/.test(e.key)) return;
  e.stopPropagation();
}

/**
 * Hold `root` open as a modal dialog until the returned function is called: the four steps
 * in the module comment. The caller gives `root` its role, `aria-modal` and name; this sets
 * `tabindex="-1"` on it if it has none, so it can hold focus itself.
 *
 * The returned `release(returnFocus = true)` undoes the `inert` this dialog set, stops its
 * key and pointer handling, and puts focus back on the opener (or `fallback`). It is safe to
 * call more than once. Releasing a dialog that is not the top one removes it from the stack
 * all the same; the one above keeps its trap.
 */
export function holdModal(root: HTMLElement, opts: HoldOptions = {}): (returnFocus?: boolean) => void {
  const active = document.activeElement;
  const opener = active instanceof HTMLElement && active !== document.body ? active : null;
  if (!root.hasAttribute("tabindex")) root.setAttribute("tabindex", "-1");
  // Only elements that were not already inert are recorded, so a dialog over a dialog undoes
  // exactly its own share and the one underneath stays as it was.
  const inerted = outside(root).filter((el) => !el.inert);
  for (const el of inerted) el.inert = true;
  const held: Held = { root, opts, opener, inerted };
  if (!stack.length) {
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onPointer, true);
  }
  stack.push(held);
  root.addEventListener("keydown", stopAtDialog);

  const target = opts.initial?.() ?? focusables(root)[0] ?? root;
  target.focus({ preventScroll: true });

  let released = false;
  return (returnFocus = true) => {
    if (released) return;
    released = true;
    const at = stack.indexOf(held);
    if (at >= 0) stack.splice(at, 1);
    if (!stack.length) {
      document.removeEventListener("keydown", onKey, true);
      document.removeEventListener("pointerdown", onPointer, true);
    }
    root.removeEventListener("keydown", stopAtDialog);
    for (const el of inerted) el.inert = false;
    if (!returnFocus) return;
    const back = opener?.isConnected ? opener : opts.fallback?.();
    back?.focus({ preventScroll: true });
  };
}

/** How many dialogs are held open now; for the tests. */
export function heldCount(): number {
  return stack.length;
}
