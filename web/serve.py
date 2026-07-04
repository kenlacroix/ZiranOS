#!/usr/bin/env python3
"""Dev server for the Ziran OS web tool, with the cross-origin isolation headers
(COOP/COEP) that `SharedArrayBuffer` — and therefore the qemu-wasm live boot —
requires.

Plain `python3 -m http.server` does NOT set these headers, so the live 64-bit
boot (real QEMU compiled to WebAssembly; see
`docs/planning/web-live-boot-qemu-wasm.md`) fails with a `SharedArrayBuffer is
not defined` error. The scripted reconstruction works either way; only the live
boot needs cross-origin isolation.

    cd web && ./serve.py            # or, from the repo root:  make serve
    # then open http://localhost:8000

Pass a port as the first argument:  ./serve.py 9000
"""
import http.server
import os
import socketserver
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8000


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        # Cross-origin isolation — the browser only exposes SharedArrayBuffer (used
        # by qemu-wasm's pthreads) when the top-level document is isolated with
        # BOTH of these. CORP lets same-origin subresources (the .wasm/.data) load
        # under COEP: require-corp.
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        super().end_headers()

    def guess_type(self, path):
        # Emscripten's WASM must be served as application/wasm for streaming compile.
        if path.endswith(".wasm"):
            return "application/wasm"
        return super().guess_type(path)


if __name__ == "__main__":
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("", PORT), Handler) as httpd:
        print(f"Ziran OS web tool  ->  http://localhost:{PORT}")
        print("cross-origin isolation ON (COOP/COEP) — required for the live qemu-wasm boot")
        print("Ctrl-C to stop")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nbye")
