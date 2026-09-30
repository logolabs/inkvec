import { describe, expect, it } from "vitest";

import { PREFS_KEY, readStoredPrefs, writeStoredPrefs } from "./prefstore";

/** A `Storage` in memory, as the tests need it. */
function memory(): Pick<Storage, "getItem" | "setItem"> & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
  };
}

const blocked = () => {
  throw new DOMException("The operation is insecure.", "SecurityError");
};

describe("the browser's preferences store", () => {
  it("round-trips the preferences through storage as JSON", () => {
    const store = memory();
    const prefs = { schema: 3, theme: "dark", trace: { precision: 0.2 }, saved: [] };
    expect(writeStoredPrefs(prefs, () => store)).toBe(true);
    expect(JSON.parse(store.data.get(PREFS_KEY)!)).toEqual(prefs);
    expect(readStoredPrefs(() => store)).toEqual(prefs);
  });

  it("reads nothing kept as null", () => {
    expect(readStoredPrefs(() => memory())).toBeNull();
  });

  it("reads a document that is not JSON as null rather than failing", () => {
    const store = memory();
    store.data.set(PREFS_KEY, "{not json");
    expect(readStoredPrefs(() => store)).toBeNull();
  });

  it("carries on when storage cannot even be reached", () => {
    expect(readStoredPrefs(blocked)).toBeNull();
    expect(writeStoredPrefs({ schema: 1 }, blocked)).toBe(false);
  });

  it("carries on when storage is full", () => {
    const full = {
      setItem: () => {
        throw new DOMException("Quota exceeded", "QuotaExceededError");
      },
    };
    expect(writeStoredPrefs({ schema: 1 }, () => full)).toBe(false);
  });

  it("uses the page's localStorage by default", () => {
    localStorage.removeItem(PREFS_KEY);
    expect(writeStoredPrefs({ schema: 7 })).toBe(true);
    expect(readStoredPrefs()).toEqual({ schema: 7 });
    localStorage.removeItem(PREFS_KEY);
  });
});
