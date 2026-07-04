# Web live boot — the real path is qemu-wasm

Supersedes the v86 trampoline plan (`web-live-boot-trampoline.md`). That plan hit
a wall — v86 is a **32-bit** emulator with no long mode, so it can never run our
64-bit kernel. A 2026-07 re-survey (below) found the same is true of the other
lightweight browser emulators, and identified the **one** option that actually
works. This doc is the scaffold + the browser-only steps to finish it.

## The finding (2026-07 survey)

To boot our *bare-metal, 64-bit* ISO in a browser tab, the emulator must emulate a
full PC **and** support x86-64 long mode. That rules out almost everything:

| Emulator | 64-bit? | Verdict |
|---|---|---|
| **v86** | ❌ 32-bit only | Loads our MB1-trampolined boot code, then dies at `check_long_mode` (`ERR: L`). No EFER, no `CPUID 0x80000001`, no REX. |
| **Halfix** | ❌ 32-bit only | Same class as v86. |
| **Bochs-WASM** | ⚠️ incomplete | Long-mode support can't even deliver a `#GP` through the IDT — a 64-bit kernel dies on its first fault before printing anything. |
| bespoke (nanox, ax) | ✅ but purpose-built | Tiny x86-64 WASM cores, but they don't emulate the VGA/PIC/PIT/PS2 hardware our kernel drives. Not a drop-in. |
| **qemu-wasm** | ✅ **yes** | Real QEMU (`qemu-system-x86_64`) compiled to WASM via Emscripten. Boots our ISO **unchanged**. This is the path. |

Sources: <https://github.com/ktock/qemu-wasm>,
<https://github.com/ktock/qemu-wasm-sample>,
container2wasm <https://github.com/container2wasm/container2wasm>,
FOSDEM 2025 "Running QEMU Inside Browser" (NTT),
nanokrnl <https://www.msuiche.com/posts/nanokrnl-cold-boot-fast-boot/>.

## The cost (be honest up front)

- **~46 MB** artifact (`qemu-system-x86_64.wasm` + glue) — so it must be
  **lazy-loaded** behind an explicit "Boot the real kernel" button, never on page
  load. The page stays light; the scripted reconstruction remains the default.
- Needs **`SharedArrayBuffer`** (Emscripten pthreads), which the browser only
  exposes when the document is **cross-origin isolated** — i.e. served with
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`. `web/serve.py` / `make serve` set
  these; plain `python3 -m http.server` does not.
- **Hosting constraint:** plain GitHub Pages can't set those headers. Netlify,
  Cloudflare Pages, or any server you control can (a `_headers` file on
  Netlify/Cloudflare; `serve.py` locally). Note this before choosing where to
  publish.
- **Verification is browser-only** — none of the boot can be checked headlessly.

## How it wires up

The scaffold in this repo:

- `web/serve.py` (`make serve`) — the COOP/COEP dev server. **Verified working.**
- `make web` now also stages **`web/ziran.iso`** (what qemu-wasm boots), alongside
  the existing `kernel.bin`/`kernel-v86.bin`.
- `web/qemu-wasm-boot.js` — a self-contained integration module (the code you
  iterate on in the browser). It: builds the Emscripten `Module`, points QEMU at
  the ISO, and pipes the guest serial to a callback the page renders.
- `web/index.html` — a lazy-load **"Boot the real kernel (~46 MB)"** button, gated
  behind `QEMU_WASM_READY` (default `false`). It fetches the assets, runs
  `qemu-wasm-boot.js`, and — as the existing `realBoot()` already does — only
  flips the badge to **live** when the real `Ziran OS booted…` serial marker
  appears. Until then the page is the honest reconstruction.

The Emscripten shape (see `qemu-wasm-boot.js` for the real code):

```js
const Module = {
  arguments: [
    "-machine", "q35", "-m", "128M",
    "-cdrom", "ziran.iso",          // staged by `make web`, written into the FS in preRun
    "-serial", "stdio",             // Emscripten pipes guest COM1 -> Module.print
    "-nographic",
  ],
  print:    (line) => onSerialLine(line),   // <- the page renders these
  printErr: (line) => onSerialLine(line),
  locateFile: (p) => p,                     // assets sit next to index.html
  preRun: [() => Module.FS.writeFile("/ziran.iso", isoBytes)],  // isoBytes fetched first
};
// then load the generated glue:  <script src="qemu-system-x86_64.js"></script>
```

## Getting the ~46 MB assets

They are **not** committed (see `.gitignore`). Two ways to obtain
`qemu-system-x86_64.{js,wasm,worker.js,data}` and drop them in `web/`:

1. **Prebuilt** — check `ktock/qemu-wasm-sample`'s releases/CI artifacts for a
   ready `x86_64` build; copy the four files into `web/`.
2. **Build from source** — `ktock/qemu-wasm` ships a Dockerfile:
   `docker build` the `qemu-system-x86_64` Emscripten target, then extract the
   generated files. (This is the container2wasm toolchain; expect a long build.)

Confirm the exact asset filenames the build emits and, if they differ, update the
`ASSETS` list in `qemu-wasm-boot.js` and the `.gitignore` globs.

## Finish it (in a browser — the part that needs eyes)

1. Obtain the qemu-wasm assets into `web/` (above).
2. `make web` (stages `web/ziran.iso`), then `make serve`.
3. Open `http://localhost:8000`, click **"Boot the real kernel"**.
4. Iterate `qemu-wasm-boot.js` until the guest serial appears — the exact QEMU
   `-machine`/`-serial` flags, the FS path for the ISO, and how the glue exposes
   `print`/`FS` are the knobs. The `qemu-wasm-sample` `module.js` is the reference.
5. Success = the page sees `Ziran OS booted…` on serial and the badge flips to
   **live · real kernel** on its own. Then set `QEMU_WASM_READY = true`.
6. For hosting, add a Netlify/Cloudflare `_headers` file with the two COOP/COEP
   headers so the live boot works off localhost.

## Then — the realness follow-ons (only meaningful once live)

- The shell is interactive over serial (Milestone: serial console) — wire the
  page's keystrokes to QEMU stdin so visitors can type `ps`/`ls`/`cat` live.
- A register/state peek from the running guest (QEMU monitor over a second serial)
  — the one thing a reconstruction can't fake.
