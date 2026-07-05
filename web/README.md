# web/ — the learn-as-it-boots tutorial

A single, self-contained page (`index.html`) that boots the actual Ziran OS
kernel in the browser and teaches what each layer is doing, using a
**predict → observe → explain** flow. It's the project's answer to a real
tension: an OS you can *poke at and understand*, not just an artifact someone
(or some AI) handed you finished.

## Two modes, one file

- **Live mode** — loads the real `kernel.bin` into the [v86](https://github.com/copy/v86)
  PC emulator (via its `multiboot` option) and boots it for real. Keystrokes go
  to the actual kernel.
- **Simulated mode** — if v86 can't be reached (offline, or a sandbox that
  blocks the script/wasm), the page falls back to a faithful replay of the exact
  same boot output, and local typing still echoes. The lesson works either way.

The page picks live mode when it can and silently falls back otherwise, so it's
never broken — just sometimes replayed.

## Running it live locally

v86 needs the page served over HTTP (not `file://`), and needs the kernel image
plus v86's own files reachable. The quickest path:

```sh
make web          # builds kernel.bin and copies it into web/kernel.bin
cd web && python3 -m http.server 8000
# open http://localhost:8000
```

By default the page pulls `libv86.js`, the wasm, and the SeaBIOS/VGA-BIOS files
from a CDN (jsDelivr). To make it fully self-hosted (recommended for a real blog
embed, and required if your CSP blocks CDNs), download those four files and drop
them next to `index.html`, then edit the `ASSET` object near the top of the
`<script>` to point at the local copies:

```
build/libv86.js   -> web/libv86.js
build/v86.wasm    -> web/v86.wasm
bios/seabios.bin  -> web/seabios.bin
bios/vgabios.bin  -> web/vgabios.bin
```

## Editing the tutorial

Everything is in `index.html`:

- `SCREEN` — the exact lines the kernel prints (kept in sync with the real boot),
  each with an optional annotation and a link to the source file.
- `STEPS` — the guided tour: for each, a "watch for", a predict question with a
  hidden answer, an explanation, and links to real source + concept docs.
- `ROAD` — the milestone roadmap shown at the bottom.
- `ASSET`, `REPO`, `BRANCH` — where the kernel image, v86 files, and source live.

When a new milestone lands, add its lines to `SCREEN` and a step to `STEPS`. This
is part of the milestone checklist (`docs/MILESTONE_CHECKLIST.md`): the teaching
tool grows with the OS.

## Deploying

The site is live at <https://ziranos.pages.dev/> on **Cloudflare Pages**, deployed
with `make deploy-web` (or `make redeploy-web` to rebuild the browser kernel first).
Cloudflare — not GitHub Pages — because the live qemu-wasm boot needs the COOP/COEP
cross-origin-isolation headers set in `web/_headers`, which GitHub Pages can't send.
The whole `web/` folder is direct-uploaded (including the gitignored ~17 MB qemu-wasm
assets a git-connected deploy can't see). Full runbook: `docs/DEPLOY.md`.

The reconstruction-only tour (no live boot) is self-contained and would run on any
static host or as an `<iframe>` embed; only the live-boot page needs the isolation
headers. See `docs/DEPLOY.md` for the two-phase split.
