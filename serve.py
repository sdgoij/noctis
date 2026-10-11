"""Serve a directory over HTTP with caching disabled.

    python serve.py docs 8000

For local browser testing: every response carries `Cache-Control: no-store`, so a
rebuilt `docs/` (or a project's `dist/<profile>/`) is picked up on reload — no
hard refresh, no stale wasm.
"""

import http.server
import sys


class NoCacheHandler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, must-revalidate")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()


def main() -> None:
    directory = sys.argv[1] if len(sys.argv) > 1 else "docs"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 8000
    handler = lambda *args, **kwargs: NoCacheHandler(  # noqa: E731
        *args, directory=directory, **kwargs
    )
    with http.server.ThreadingHTTPServer(("", port), handler) as httpd:
        print(f"serving {directory}/ at http://localhost:{port}/ (no-store)")
        httpd.serve_forever()


if __name__ == "__main__":
    main()
