/**
 * What differs between the two apps this interface is: Inkvec Studio on the desktop and
 * Inkvec Studio Lite in a browser tab.
 *
 * Everything the interface asks of its host that is not a Studio command goes through
 * here: choosing files, saving them, the clipboard, links, the window. On the desktop each
 * is the Tauri plugin it always was; in the browser, the platform's own equivalent — a file
 * input for a dialog, a download for a write, `navigator.clipboard`, a new tab. What a tab
 * genuinely cannot do (reveal a file in its folder, pick a folder to write into) is said
 * here once, so a view hides it rather than fakes it.
 *
 * `__INKVEC_WEB__` is a build-time constant (`vite.config.ts`), so each build carries only
 * its own half: the desktop bundle has no web backend in it and the web bundle never
 * reaches a Tauri plugin.
 */

import { open, save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";

import { api } from "./ipc";
import { download } from "./web/files";

export { download };

/** True in the browser build. */
export const WEB: boolean = __INKVEC_WEB__;

/** The app's name as this build is called: the browser build is the Lite one. */
export const APP_NAME = WEB ? "Inkvec Studio Lite" : "Inkvec Studio";

/** Where the desktop app is downloaded, and why the browser build recommends it. */
export const DESKTOP_URL = "https://github.com/logolabs/inkvec/releases";
export const DESKTOP_WHY =
  "Inkvec Studio for Windows, macOS and Linux: the native engine is faster, it traces whole folders in one batch, has no browser memory limit (traces up to 16384 px), and works offline.";

/** A file the user chose or dropped: a path on the desktop, the file itself in a browser. */
export interface Picked {
  name: string;
  path: string | null;
  file: File | null;
}

export interface Filter {
  name: string;
  extensions: string[];
}

/** The last part of a path, whichever separator it uses. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** A picked path, as the desktop's dialogs and drops report them. */
export function pickedPath(path: string): Picked {
  return { name: baseName(path), path, file: null };
}

/** A picked file, as a browser's input, drop or paste hands it over. */
export function pickedFile(file: File): Picked {
  return { name: file.name || "pasted image", path: null, file };
}

/** Ask for one file (or several) to open. Empty when the user cancels. */
export async function pickFiles(filters: Filter[], multiple = false): Promise<Picked[]> {
  if (WEB) {
    return new Promise((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.multiple = multiple;
      input.accept = filters.flatMap((f) => f.extensions.map((e) => `.${e}`)).join(",");
      input.style.display = "none";
      let settled = false;
      const done = (files: Picked[]) => {
        if (settled) return;
        settled = true;
        input.remove();
        resolve(files);
      };
      input.addEventListener("change", () => done([...(input.files ?? [])].map(pickedFile)));
      input.addEventListener("cancel", () => done([]));
      document.body.append(input);
      input.click();
    });
  }
  const picked = (await open({ multiple, filters })) as string | string[] | null;
  if (typeof picked === "string") return [pickedPath(picked)];
  if (Array.isArray(picked)) return picked.map(pickedPath);
  return [];
}

/** The text of a picked file (an SVG, for Minify and Fabricate). */
export async function readPickedText(p: Picked): Promise<string> {
  if (p.file) return p.file.text();
  if (p.path) return api.readTextFile(p.path);
  throw new Error("nothing to read");
}

/** Whether this build can choose a folder and write into it (the desktop can; a tab cannot). */
export const CAN_PICK_FOLDER = !WEB;

/** Ask for a folder. Always null in a browser, which has no such dialog. */
export async function pickFolder(current: string | null = null, title?: string): Promise<string | null> {
  if (WEB) return null;
  const picked = await open({ directory: true, multiple: false, defaultPath: current ?? undefined, title });
  return typeof picked === "string" ? picked : null;
}

/** Where a saved file went: a path on the desktop; `null` in a browser, where it downloaded. */
export interface Saved {
  path: string | null;
  name: string;
}

/**
 * Save one file. The desktop asks where with its save dialog; a browser downloads it under
 * `name`, into wherever the browser keeps downloads. Returns null when the user cancels.
 */
export async function saveFile(name: string, data: Uint8Array | string, filters: Filter[]): Promise<Saved | null> {
  const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
  if (WEB) {
    download(name, bytes);
    return { path: null, name };
  }
  const path = await save({ defaultPath: name, filters });
  if (!path) return null;
  await api.saveBytes(path, [...bytes]);
  return { path, name: baseName(path) };
}

/**
 * Save several files together. The desktop asks for a folder and writes each one into it;
 * a browser downloads one `.zip` named `zipName` holding them all.
 */
export async function saveFiles(
  files: { name: string; data: Uint8Array | string }[],
  zipName: string,
  title?: string,
): Promise<{ folder: string | null; first: string | null } | null> {
  const enc = new TextEncoder();
  const asBytes = (d: Uint8Array | string) => (typeof d === "string" ? enc.encode(d) : d);
  if (WEB) {
    if (files.length === 1) {
      download(files[0].name, asBytes(files[0].data));
    } else {
      const { zipStore } = await import("./web/zip");
      download(zipName, zipStore(files.map((f) => ({ name: f.name, data: asBytes(f.data) }))));
    }
    return { folder: null, first: null };
  }
  const folder = await pickFolder(null, title);
  if (!folder) return null;
  const sep = folder.includes("\\") ? "\\" : "/";
  for (const f of files) await api.saveBytes(`${folder}${sep}${f.name}`, [...asBytes(f.data)]);
  return { folder, first: files.length ? `${folder}${sep}${files[0].name}` : null };
}

/** Show a written file in the system's file manager. A browser cannot; the caller hides it. */
export const CAN_REVEAL = !WEB;

export function reveal(path: string): void {
  if (!WEB) void revealItemInDir(path);
}

/** Put text on the clipboard. */
export async function copyText(text: string): Promise<void> {
  if (WEB) await navigator.clipboard.writeText(text);
  else await writeText(text);
}

/** Open a link outside the app: the system browser on the desktop, a new tab in a browser. */
export async function openExternal(url: string): Promise<void> {
  if (WEB) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  await openUrl(url);
}

/** What a "saved" toast offers: "Show in folder" where there is one. */
export function revealAction(path: string | null): { label: string; run: () => void } | undefined {
  return path && CAN_REVEAL ? { label: "Show in folder", run: () => reveal(path) } : undefined;
}
