/** Files leaving a browser tab: every "write" in the browser build is a download. */

/** Hand `bytes` to the browser as a download called `name`. */
export function download(name: string, bytes: Uint8Array | Blob, type = "application/octet-stream"): void {
  const blob = bytes instanceof Blob ? bytes : new Blob([bytes as BlobPart], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.style.display = "none";
  document.body.append(a);
  a.click();
  a.remove();
  // Long enough for the browser to have taken the file; a URL revoked mid-download fails it.
  window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
