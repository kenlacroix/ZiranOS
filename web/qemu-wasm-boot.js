// web/qemu-wasm-boot.js — the live-boot integration (qemu-wasm, ~46 MB, lazy-loaded).
//
// This runs REAL QEMU (qemu-system-x86_64) compiled to WebAssembly and boots
// web/ziran.iso — the actual 64-bit kernel, not a scripted replay. It is verified
// ONLY in a browser; the exact QEMU flags, the FS path, and how the generated glue
// exposes `print`/`FS` are the knobs to iterate on against ktock/qemu-wasm-sample's
// module.js. See docs/planning/web-live-boot-qemu-wasm.md.
//
// Usage (from index.html, after lazy-loading this file):
//   ZiranQemuWasm.boot({ onLine, onStatus, onError });
(function () {
  "use strict";

  // The Emscripten build's generated glue. It pulls in the .wasm / .worker.js /
  // .data itself. Confirm the exact filename your build emits.
  var GLUE = "qemu-system-x86_64.js";

  // QEMU command line. `-serial stdio` makes Emscripten pipe the guest's COM1 to
  // Module.print; `-nographic` keeps it headless (we render the serial ourselves).
  function qemuArgs() {
    return [
      "-machine", "q35",
      "-m", "128M",
      "-cdrom", "ziran.iso",
      "-serial", "stdio",
      "-nographic",
      "-no-reboot",
    ];
  }

  // Boot the real kernel. opts: { onLine(str), onStatus(str), onError(err) }.
  function boot(opts) {
    opts = opts || {};
    var onLine = opts.onLine || function () {};
    var onStatus = opts.onStatus || function () {};
    var onError = opts.onError || function (e) { console.error(e); };

    // qemu-wasm uses pthreads -> SharedArrayBuffer -> the page must be
    // cross-origin isolated (COOP/COEP). `make serve` sets those headers.
    if (!self.crossOriginIsolated) {
      onError(new Error(
        "Not cross-origin isolated — SharedArrayBuffer is unavailable, so qemu-wasm " +
        "cannot start. Serve with COOP/COEP headers: run `make serve` (not `python3 " +
        "-m http.server`)."
      ));
      return;
    }

    onStatus("fetching the disk image…");
    fetch("ziran.iso").then(function (r) {
      if (!r.ok) throw new Error("could not fetch ziran.iso — run `make web` first");
      return r.arrayBuffer();
    }).then(function (isoBuf) {
      var iso = new Uint8Array(isoBuf);

      // Emscripten's print tends to hand back whole lines; printErr can hand back
      // fragments. Buffer to newline boundaries so the page gets clean lines.
      var pending = "";
      function feed(chunk) {
        if (chunk == null) return;
        pending += "" + chunk;
        var nl;
        while ((nl = pending.indexOf("\n")) >= 0) {
          onLine(pending.slice(0, nl));
          pending = pending.slice(nl + 1);
        }
      }

      // The generated glue (GLUE) reads this global `Module`.
      var Module = {
        arguments: qemuArgs(),
        print: feed,
        printErr: feed,
        locateFile: function (path) { return path; }, // assets sit beside index.html
        preRun: [function () {
          // Put the ISO where `-cdrom ziran.iso` will find it, in Emscripten's FS.
          try {
            Module.FS.writeFile("ziran.iso", iso);
          } catch (e) {
            onError(new Error("FS.writeFile(ziran.iso) failed: " + e));
          }
        }],
        setStatus: function (t) { if (t) onStatus(t); },
        onAbort: function (what) { onError(new Error("qemu-wasm aborted: " + what)); },
      };
      self.Module = Module;

      onStatus("loading the emulator (~46 MB, one-time)…");
      var s = document.createElement("script");
      s.src = GLUE;
      s.onerror = function () {
        onError(new Error(
          "could not load " + GLUE + " — the qemu-wasm assets aren't in web/. " +
          "See docs/planning/web-live-boot-qemu-wasm.md for how to build/obtain them."
        ));
      };
      document.body.appendChild(s);
    }).catch(onError);
  }

  self.ZiranQemuWasm = { boot: boot };
})();
