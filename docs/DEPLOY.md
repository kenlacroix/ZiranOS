# Deploying the web tool (Cloudflare Pages)

The browser teaching tool in `web/` is the project's public face. Host it on
**Cloudflare Pages** — not GitHub Pages — for one concrete reason: Pages can set
the **COOP/COEP** cross-origin-isolation headers the live qemu-wasm boot needs
(via `web/_headers`), which GitHub Pages cannot.

There are two phases, and **phase 1 is fully unblocked right now** (it doesn't
need the 46 MB build):

## Phase 1 — the tour, live (do this now)

The tour (manifesto, ELI5, guided reconstruction) is self-contained committed
HTML/JS. It runs in **reconstruction mode** with zero external assets, so a plain
static deploy of `web/` just works.

**Account-side steps (yours):**

1. **Domain** — register one (Cloudflare Registrar is at-cost, or any registrar),
   and add the zone to your Cloudflare account (point the nameservers at CF).
2. **Cloudflare Pages → Create project → Connect to Git**, pick this repo/branch.
   Build settings:
   - Framework preset: **None**
   - Build command: *(empty)*
   - **Build output directory: `web`**
   Deploy. You get a `*.pages.dev` URL serving the tour.
3. **Custom domain** — Pages project → *Custom domains* → add your domain. CF
   wires the DNS + TLS automatically.
4. **Verify the headers** are live (they're what phase 2 needs):
   ```sh
   curl -sI https://YOURDOMAIN/ | grep -i cross-origin
   # -> Cross-Origin-Opener-Policy: same-origin
   #    Cross-Origin-Embedder-Policy: require-corp
   ```
   `web/_headers` applies these site-wide (safe — the tour loads no cross-origin
   resources).

That's the project piece publicly live. The "Boot the real kernel" button stays
hidden (`QEMU_WASM_READY = false`) until phase 2.

## Phase 2 — the live boot assets (after `build-qemu-wasm.sh`)

The qemu-wasm assets (`out.wasm`, `out.data`, …) are big and **gitignored**, so
they are NOT in the git deploy. Two gotchas decide how to host them:

- **Cloudflare Pages caps files at 25 MiB.** `out.wasm` (and `out.data`, which
  packs the 16 MB ISO + BIOS) will likely exceed that — so they **can't** live in
  the Pages deploy directly.
- They must load **same-origin** (or send CORP) to satisfy COEP.

So host the big assets on **Cloudflare R2** (object storage, no per-file cap) and
serve them on the same domain via a route/custom domain, with
`Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`) on the bucket. Then
point `web/qemu-wasm.html`'s asset URLs (`out.js`, `out.wasm`, `out.data`,
`load.js`) at that path. `qemu-wasm.html` is already parameterized for this: it
reads an **`ASSET_BASE`** (default `./` for local `make serve`), overridable per
visit with `?assets=https://host/path/`. Steps:

1. `./web/build-qemu-wasm.sh` → `out.{js,wasm,worker.js,data}` + `load.js` in `web/`.
2. **Upload to R2:** create a bucket, then
   ```sh
   R2_BUCKET=ziran-os-assets ./web/deploy-r2.sh     # wrangler, or set RCLONE_REMOTE=r2:
   ```
   It sets the right content types (`out.wasm` → `application/wasm`).
3. **Map the bucket to your domain, same-origin** — this is the important bit.
   Attach a **custom domain / route** so the bucket serves at
   `https://YOURDOMAIN/qemu/` (a Pages/Workers route or R2 custom domain on the
   same apex). Same-origin means the COEP page can load the assets **without**
   CORP/CORS fuss. (If you must serve them cross-origin, add
   `Cross-Origin-Resource-Policy: cross-origin` **and** CORS `Access-Control-Allow-Origin`
   on the bucket, or COEP will block them.)
4. **Point the page at them:** set `ASSET_BASE = '/qemu/'` in `web/qemu-wasm.html`
   (same-origin path) — or leave it `./` and link the button with
   `qemu-wasm.html?assets=/qemu/`. Keep `vendor/`, `qemu-wasm.html`, `index.html`,
   `replay.html`, `ziran-session.cast` in the Pages deploy.
5. Flip **`QEMU_WASM_READY = true`** in `index.html` so the tour's button appears.
6. Load it, confirm `crossOriginIsolated === true` in the console, and boot to
   `ziran:/>`.

**Chosen path (the build landed under 25 MiB — no R2 needed).** The built
`qemu-system-x86_64.wasm` is ~15.5 MiB and the live-boot page loads the kernel via
`-kernel /kernel-v86.bin` (not the 16 MiB ISO), so every asset is under Pages'
25 MiB per-file cap. Skip R2 and **direct-upload the whole `web/` folder** — which
includes the gitignored assets a git-connected deploy can't see (that is exactly
why `/qemu-wasm` renders text but never boots the kernel):

```sh
./web/build-qemu-wasm.sh        # once, to produce the assets (heavy; see that script)
make deploy-web                 # = npx wrangler pages deploy web --project-name=ziranos
```

`make deploy-web` needs Node and a one-time `wrangler login`; override the project
name with `CF_PAGES_PROJECT=yourname`. **Caveat:** a later `git push` to a
git-connected Pages project redeploys *without* the assets (they're gitignored), so
after any push you want live, re-run `make deploy-web` — or make Pages
direct-upload-only so the two don't fight.

## Notes

- Auto-deploy: with Git-connected Pages, every push to the branch redeploys the
  tour. The heavy assets (R2) are updated out-of-band, only when rebuilt.
- Purging: `out.*` use fixed names with a 1-week cache (`web/_headers`); after a
  rebuild, purge the CF cache (or bump a `?v=` query) so clients refetch.
