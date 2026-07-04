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
`load.js`) at that path. Steps to finalize once we know the built sizes:

1. `./web/build-qemu-wasm.sh` → assets in `web/`.
2. Create an R2 bucket, upload `out.{js,wasm,worker.js,data}` + `load.js`, set the
   CORP header, and map it to `https://YOURDOMAIN/qemu/` (or a subdomain).
3. In `qemu-wasm.html`, set the asset base to that path; keep `vendor/` (small)
   and `qemu-wasm.html` in the Pages deploy.
4. Flip `QEMU_WASM_READY = true` in `index.html` so the button appears.
5. Confirm `crossOriginIsolated === true` in the page console, then boot.

(Smaller alternative: if the built `out.wasm` comes in under 25 MiB, you can skip
R2 and `npx wrangler pages deploy web` the whole folder as a Direct Upload —
needs Node. We'll know the size when the build finishes.)

## Notes

- Auto-deploy: with Git-connected Pages, every push to the branch redeploys the
  tour. The heavy assets (R2) are updated out-of-band, only when rebuilt.
- Purging: `out.*` use fixed names with a 1-week cache (`web/_headers`); after a
  rebuild, purge the CF cache (or bump a `?v=` query) so clients refetch.
