"""Serve Inkvec Studio Lite locally the way the Hugging Face Space serves it.

    python studio/web/serve.py [directory] [--port 8931]

The directory defaults to studio/dist-web (what `npm run build:web` writes). The two
cross-origin isolation headers are the point: without them the browser refuses the
SharedArrayBuffer the threaded engine and the denoiser bridge need, and the page falls back
to one core and no denoiser. The Space sends the same headers from its README's
`custom_headers`.
"""

from __future__ import annotations

import argparse
import functools
import http.server
import pathlib

HEADERS = {
    "Cross-Origin-Opener-Policy": "same-origin",
    "Cross-Origin-Embedder-Policy": "require-corp",
    "Cross-Origin-Resource-Policy": "cross-origin",
    # A local server for testing: always the build on disk, never a stale copy.
    "Cache-Control": "no-cache",
}


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
        ".mjs": "text/javascript",
        ".svg": "image/svg+xml",
        ".md": "text/markdown; charset=utf-8",
    }

    def end_headers(self) -> None:
        for k, v in HEADERS.items():
            self.send_header(k, v)
        super().end_headers()

    def log_message(self, format: str, *args) -> None:  # noqa: A002 - the base class's name
        pass


def main() -> None:
    here = pathlib.Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("directory", nargs="?", default=str(here.parent / "dist-web"))
    ap.add_argument("--port", type=int, default=8931)
    a = ap.parse_args()
    handler = functools.partial(Handler, directory=a.directory)
    with http.server.ThreadingHTTPServer(("127.0.0.1", a.port), handler) as srv:
        print(f"Inkvec Studio Lite on http://127.0.0.1:{a.port}/ (from {a.directory})")
        srv.serve_forever()


if __name__ == "__main__":
    main()
