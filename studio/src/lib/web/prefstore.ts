/**
 * Where Inkvec Studio Lite keeps its preferences: one JSON document in the page's
 * `localStorage`. The desktop app keeps them in a file instead (`src-tauri`); both go
 * through the same sanitising code in the core before the interface sees them.
 *
 * Storage can fail in ways a desktop file rarely does: a private window, a blocked or full
 * store, a page inside a sandboxed frame, a document someone edited by hand. None of those
 * may stop the Studio from starting or from working, so both functions here swallow the
 * failure and say so in their result. Used by `web/engine.ts`; tested in `prefstore.test.ts`.
 */

/** The `localStorage` key the preferences are kept under. */
export const PREFS_KEY = "inkvec-studio-lite:prefs";

/**
 * The preferences this browser kept, parsed but not yet sanitised, or `null` when there are
 * none or they cannot be read (storage blocked, or not valid JSON). `storage` is called
 * inside the guard because merely reading `window.localStorage` throws where storage is
 * blocked.
 */
export function readStoredPrefs(storage: () => Pick<Storage, "getItem"> = () => localStorage): unknown {
  try {
    const text = storage().getItem(PREFS_KEY);
    return text ? JSON.parse(text) : null;
  } catch {
    return null;
  }
}

/**
 * Keep `prefs` in this browser. Returns false when they could not be written (storage full
 * or blocked): the session still works, it just starts fresh next time.
 */
export function writeStoredPrefs(prefs: unknown, storage: () => Pick<Storage, "setItem"> = () => localStorage): boolean {
  try {
    storage().setItem(PREFS_KEY, JSON.stringify(prefs));
    return true;
  } catch {
    return false;
  }
}
