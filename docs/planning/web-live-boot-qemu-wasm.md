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

## How it wires up (the real API, from ktock/qemu-wasm-sample)

The generated build is a MODULARIZE Emscripten module, and serial goes through
**xterm-pty** (a PTY bridge for xterm.js), NOT `Module.print`. Because our kernel
also *reads* input over serial (the serial console), xterm-pty makes the shell
**interactive** in the browser. The scaffold in this repo:

- `web/serve.py` (`make serve`) — the COOP/COEP dev server. **Verified working.**
- `make web` stages **`web/ziran.iso`** (packaged into the emulator's FS at build).
- **`web/build-qemu-wasm.sh`** — turnkey build: clones the patched QEMU, runs the
  two Docker builds, `emmake`s `qemu-system-x86_64` to WASM, packages QEMU's
  pc-bios **and** our ISO with `file_packager`, and stages
  `web/{out.js,out.wasm,out.worker.js,out.data,load.js}` + vendored xterm/xterm-pty.
- **`web/qemu-wasm.html`** — the dedicated live-boot page. Mirrors the sample's
  working wiring exactly (load.js → out.js MODULARIZE → xterm + xterm-pty →
  `Module.pty = slave`), but with our QEMU args (`-cdrom pack/ziran.iso -L pack/
  -nographic -accel tcg`). This is the page you open and iterate.
- `web/index.html` — a **"Boot the real kernel (~46 MB)"** button, hidden behind
  `QEMU_WASM_READY` (default `false`); when flipped on it opens `qemu-wasm.html`.

The Emscripten shape (see `qemu-wasm.html` for the real code):

```js
import initEmscriptenModule from './out.js';   // MODULARIZE default export
import './vendor/xterm.js'; import './vendor/xterm-pty.js';
window.Module.arguments = ['-nographic','-m','128M','-L','pack/',
                           '-cdrom','pack/ziran.iso','-accel','tcg,tb-size=500'];
const term = new Terminal(); term.open(el);
const { master, slave } = openpty(); term.loadAddon(master);
Module.pty = slave;                             // guest stdio <-> the terminal
await initEmscriptenModule(Module);             // load.js (script) ran first, preloading the FS
```

## Getting the ~46 MB assets — run the build script

No prebuilt exists. `web/build-qemu-wasm.sh` automates the
`ktock/qemu-wasm-sample` flow, trimmed to our ISO. **Heavy and slow** — it
compiles a patched QEMU (`ktock/qemu-wasm`, Wasm-backend patch tracked in that
repo's PR #21) to WebAssembly. On **Apple Silicon** the Emscripten Docker images
are x86_64 and run under emulation, so budget hours; a fast x86_64 Linux box is
far better. Needs `docker` (daemon up), `git`, `curl`, and `build/ziran.iso`.

```sh
make web                # stage build/ziran.iso (macOS: add the x86_64-elf- flags)
./web/build-qemu-wasm.sh
```

It stages `web/{out.js,out.wasm,out.worker.js,out.data,load.js,vendor/}` (all
gitignored). The script checks out the Wasm-backend branch itself
(`QEMU_WASM_BRANCH`, default `dev-wasm-j`; the code lives there, not on `master`)
and repoints zlib from the dead `zlib.net` pin to GitHub's mirror.

### Build on a native x86_64 Linux box (recommended)

**Do not build on Apple Silicon.** Observed there: the Emscripten toolchain images
are x86_64, so they run under emulation — zlib's `configure` alone took ~8 minutes,
and Docker's VM then died mid-build (`error reading from server: EOF`) under the
memory pressure of the parallel emulated dep builds. A **native x86_64 Linux host
with plenty of RAM fixes both**: the toolchain runs natively, and native Docker
uses host RAM directly (no Desktop VM cap). A slow CPU only makes it *longer*, not
*fail*.

Requirements: **x86_64** (`uname -m` → `x86_64`; ARM would emulate too), Docker
(daemon running — if building *inside* a container, it needs docker-in-docker or
the host docker socket), `git`, `curl`.

```sh
# deps (Debian/Ubuntu): docker.io git curl + the ISO toolchain, native names:
#   nasm binutils grub-pc-bin grub-common xorriso  + rustup
rustup target add x86_64-unknown-none
git clone <repo> ziran && cd ziran && git checkout claude/ziran-os-plan-qgobid
make web                     # native ld/objcopy/grub-mkrescue — NO x86_64-elf- prefixes on Linux
./web/build-qemu-wasm.sh     # long on a slow CPU, but it completes
```

(No Rust on the box? Skip `make web` and `scp` your Mac's `build/ziran.iso` over —
that ISO file is the only input the build needs.) Then either serve/deploy from the
box, or copy the assets back to a workstation:

```sh
rsync -a box:ziran/web/{out.js,out.wasm,out.worker.js,out.data,load.js,vendor/} web/
make serve   # open http://localhost:8000/qemu-wasm.html
```

## Finish it (in a browser — the part that needs eyes)

1. `./web/build-qemu-wasm.sh` (above) — produces the assets.
2. `make serve`, open **`http://localhost:8000/qemu-wasm.html`**.
3. It should boot to `ziran:/>`. Click the terminal and type `ps` / `ls` /
   `cd docs` / `cat filesystem.txt` — interactive, over real emulated serial.
4. Knobs if it doesn't: the QEMU args in `qemu-wasm.html` (`-machine`, `-accel`,
   whether the ISO wants `-cdrom` vs `-drive`), and the xterm-pty poll shim.
   `ktock/qemu-wasm-sample`'s `samples/module.js` + `samples/index.html` are the
   reference.
5. When it boots, set `QEMU_WASM_READY = true` in `index.html` so the tour's
   "Boot the real kernel" button appears and links here.
6. To host it, the server must send the two COOP/COEP headers (a Netlify/Cloudflare
   `_headers` file, or your own server) — plain GitHub Pages can't.

## Then — the realness follow-ons (only meaningful once live)

- The shell is interactive over serial (Milestone: serial console) — wire the
  page's keystrokes to QEMU stdin so visitors can type `ps`/`ls`/`cat` live.
- A register/state peek from the running guest (QEMU monitor over a second serial)
  — the one thing a reconstruction can't fake.
