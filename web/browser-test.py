#!/usr/bin/env python3
"""End-to-end test of the real qemu-wasm live boot, in headless Chrome, with no
node/selenium/playwright — just the Chrome DevTools Protocol over a hand-rolled
WebSocket. Serves web/ with the COOP/COEP headers, boots web/qemu-wasm.html in
headless Chrome, waits for the guest to reach the `ziran:/>` prompt, types `ps`,
and checks the task list comes back.

    python3 web/browser-test.py          # exit 0 = booted to an interactive shell

Requires: Chrome (or set CHROME=/path/to/chrome), the built qemu-wasm assets in
web/ (./web/build-qemu-wasm.sh) + web/kernel-v86.bin (make web). This is the check
native `-kernel` can't do — it exercises the actual WASM/qemu-wasm path (it's what
would have caught the missing-romfile abort)."""
import base64, json, os, socket, struct, subprocess, sys, time, urllib.request

WEB = os.path.dirname(os.path.abspath(__file__))
PORT, DBG = 8071, 9231
CHROME = os.environ.get("CHROME") or next(
    (p for p in ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                 "/Applications/Chromium.app/Contents/MacOS/Chromium",
                 "google-chrome", "chromium", "chromium-browser"]
     if os.path.exists(p) or subprocess.run(["which", p], capture_output=True).returncode == 0),
    None)


class WS:  # minimal RFC6455 client (client frames are masked)
    def __init__(self, url):
        hostport, _, path = url[len("ws://"):].partition("/")
        host, port = hostport.split(":")
        self.s = socket.create_connection((host, int(port)), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\n"
                        f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
                        f"Sec-WebSocket-Version: 13\r\n\r\n").encode())
        self.buf = b""
        while b"\r\n\r\n" not in self.buf:
            self.buf += self.s.recv(4096)
        self.buf = self.buf.split(b"\r\n\r\n", 1)[1]

    def _rd(self, n):
        while len(self.buf) < n:
            self.buf += self.s.recv(65536)
        d, self.buf = self.buf[:n], self.buf[n:]
        return d

    def send(self, text):
        p = text.encode(); m = os.urandom(4); h = bytearray([0x81]); n = len(p)
        if n < 126: h.append(0x80 | n)
        elif n < 65536: h.append(0x80 | 126); h += struct.pack(">H", n)
        else: h.append(0x80 | 127); h += struct.pack(">Q", n)
        h += m
        self.s.sendall(bytes(h) + bytes(b ^ m[i % 4] for i, b in enumerate(p)))

    def recv(self):
        b0, b1 = self._rd(2); ln = b1 & 0x7f
        if ln == 126: ln = struct.unpack(">H", self._rd(2))[0]
        elif ln == 127: ln = struct.unpack(">Q", self._rd(8))[0]
        mask = self._rd(4) if (b1 & 0x80) else None
        p = self._rd(ln)
        if mask: p = bytes(c ^ mask[i % 4] for i, c in enumerate(p))
        return p.decode("utf-8", "replace")


class CDP:
    def __init__(self, ws_url):
        self.ws = WS(ws_url); self.id = 0

    def cmd(self, method, params=None):
        self.id += 1; i = self.id
        self.ws.send(json.dumps({"id": i, "method": method, "params": params or {}}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("id") == i:
                if "error" in m: raise RuntimeError(m["error"])
                return m.get("result", {})

    def eval(self, expr):
        r = self.cmd("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True})
        return r.get("result", {}).get("value")


def main():
    if not CHROME:
        print("SKIP: no Chrome/Chromium found (set CHROME=/path/to/chrome)"); return 0
    for f in ["out.js", "qemu-system-x86_64.wasm", "load.js", "kernel-v86.bin"]:
        if not os.path.exists(os.path.join(WEB, f)):
            print(f"SKIP: missing web/{f} — build the assets first (build-qemu-wasm.sh + make web)"); return 0

    os.system("rm -rf /tmp/chrome-ziran-test")  # fresh profile each run
    srv = subprocess.Popen([sys.executable, "serve.py", str(PORT)], cwd=WEB,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    chrome = subprocess.Popen([CHROME, "--headless=new", "--disable-gpu", "--no-sandbox",
        "--no-first-run", "--no-default-browser-check", f"--remote-debugging-port={DBG}",
        "--user-data-dir=/tmp/chrome-ziran-test", "--enable-features=SharedArrayBuffer",
        f"http://localhost:{PORT}/qemu-wasm.html"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(3)
        ws_url = None
        for _ in range(20):
            try:
                for t in json.load(urllib.request.urlopen(f"http://localhost:{DBG}/json/list", timeout=5)):
                    if t.get("type") == "page" and "qemu-wasm.html" in t.get("url", ""):
                        ws_url = t["webSocketDebuggerUrl"]; break
            except Exception: pass
            if ws_url: break
            time.sleep(1)
        if not ws_url:
            print("FAIL: could not attach to the page"); return 1
        cdp = CDP(ws_url); cdp.cmd("Runtime.enable")
        print("attached. crossOriginIsolated:", cdp.eval("crossOriginIsolated"))

        READ = ("(document.querySelector('.xterm-rows')||document.querySelector('.xterm-screen')||"
                "{innerText:''}).innerText")
        booted, last = False, ""
        for i in range(120):  # WASM compile + boot is slow in headless Chrome
            last = cdp.eval(READ) or ""
            if "ziran:/>" in last: booted = True; break
            if any(w in last.lower() for w in ("boot error", "romfile", "panic")):
                print(f"FAIL at ~{i*2}s — error in terminal:\n{last[-800:]}"); return 1
            time.sleep(2)
        if not booted:
            status = cdp.eval("document.getElementById('status')?document.getElementById('status').textContent:''")
            print(f"FAIL: never reached ziran:/> prompt (status={status!r}).\nlast terminal:\n{last[-800:]}")
            return 1
        print(f"booted to prompt.\n--- last screen ---\n{last[-500:]}")

        cdp.eval("(document.querySelector('.xterm-helper-textarea')||{focus:()=>{}}).focus()")
        cdp.cmd("Input.insertText", {"text": "ps"})
        time.sleep(0.4)
        cdp.cmd("Input.dispatchKeyEvent", {"type": "keyDown", "key": "Enter", "windowsVirtualKeyCode": 13})
        cdp.cmd("Input.dispatchKeyEvent", {"type": "char", "text": "\r"})
        cdp.cmd("Input.dispatchKeyEvent", {"type": "keyUp", "key": "Enter", "windowsVirtualKeyCode": 13})
        time.sleep(2)
        after = cdp.eval(READ) or ""
        ok = "tasks:" in after
        print(f"--- after `ps` ---\n{after[-400:]}")
        print(f"\nRESULT: booted={booted}  ps_responded={ok}")
        return 0 if ok else 2
    finally:
        chrome.terminate(); srv.terminate()


if __name__ == "__main__":
    sys.exit(main())
