#!/usr/bin/env bash
# Build the qemu-wasm assets that let the browser boot the REAL 64-bit kernel,
# and stage them into web/. This compiles a patched QEMU to WebAssembly with
# Emscripten — it is HEAVY and SLOW (tens of minutes to hours), and on Apple
# Silicon the Emscripten Docker images are x86_64, so they run under emulation
# (slower still). Run it when you have time and a beefy machine; a fast x86_64
# Linux box is ideal.
#
# Prereqs: docker (daemon running), git, curl, and build/ziran.iso (`make web`).
# Based on ktock/qemu-wasm-sample's documented flow (README), trimmed to boot our
# bare-metal ISO instead of the sample's Linux guest.
#
#   ./web/build-qemu-wasm.sh
#
# Output (into web/, all gitignored): out.js, out.wasm, out.worker.js, out.data,
# load.js, and vendor/ (xterm + xterm-pty). Then:  make serve  and open the page.
set -euo pipefail
# The QEMU/emsdk Dockerfiles use BuildKit features (RUN <<EOF heredocs, COPY --link,
# --progress). Force BuildKit on — some docker.io installs still default to the
# legacy builder even with buildx present.
export DOCKER_BUILDKIT=1
cd "$(dirname "$0")/.."           # repo root
ROOT="$PWD"
WORK="${QEMU_WASM_WORK:-$ROOT/build/qemu-wasm}"
QEMU_REPO="${QEMU_REPO:-$WORK/qemu}"   # the patched QEMU (ktock/qemu-wasm)
# The Wasm backend lives on a dev branch, NOT master. dev-wasm-j carries the
# emsdk-wasm32-cross.docker the sample uses; sync-upstream-wasmdev is a fallback.
QEMU_WASM_BRANCH="${QEMU_WASM_BRANCH:-dev-wasm-j}"
mkdir -p "$WORK"

command -v docker >/dev/null || { echo "docker not found"; exit 1; }
docker info >/dev/null 2>&1     || { echo "docker daemon not running"; exit 1; }
[ -f "$ROOT/build/ziran.iso" ]  || { echo "build/ziran.iso missing — run 'make web' first"; exit 1; }

# 1. The patched QEMU source, on the Wasm-backend dev branch (not master).
if [ ! -d "$QEMU_REPO/.git" ]; then
  echo "==> cloning ktock/qemu-wasm ($QEMU_WASM_BRANCH) into $QEMU_REPO"
  git clone --depth 1 -b "$QEMU_WASM_BRANCH" https://github.com/ktock/qemu-wasm "$QEMU_REPO"
else
  echo "==> ensuring $QEMU_REPO is on $QEMU_WASM_BRANCH"
  git -C "$QEMU_REPO" fetch --depth 1 origin "$QEMU_WASM_BRANCH"
  git -C "$QEMU_REPO" checkout -f -B "$QEMU_WASM_BRANCH" FETCH_HEAD   # -f discards our patch below
fi
EMSDK_DF="$QEMU_REPO/tests/docker/dockerfiles/emsdk-wasm32-cross.docker"
[ -f "$EMSDK_DF" ] || {
  echo "emsdk-wasm32-cross.docker not found on $QEMU_WASM_BRANCH — try QEMU_WASM_BRANCH=sync-upstream-wasmdev"; exit 1; }

# Upstream's dockerfile fetches zlib from zlib.net, which DELETES old tarballs when
# it releases a new one (1.3.1 is now a 404 — they moved to 1.3.2). Repoint that one
# line at GitHub's permanent release mirror. Re-applied every run (checkout -f above
# reverts it), so it survives a re-clone.
sed -i.bak 's#https://zlib.net/zlib-$ZLIB_VERSION.tar.xz#https://github.com/madler/zlib/releases/download/v$ZLIB_VERSION/zlib-$ZLIB_VERSION.tar.xz#' "$EMSDK_DF"

# 2. The Emscripten cross-compile base image (emsdk + GLib/zlib/libffi/Pixman).
#    This Dockerfile ships inside the QEMU repo. THIS STEP IS THE LONG ONE.
echo "==> building build-qemu-base (Emscripten + cross-compiled deps) — slow"
docker buildx build --load --progress=plain -t build-qemu-base - \
  < "$QEMU_REPO/tests/docker/dockerfiles/emsdk-wasm32-cross.docker"

# 3. A minimal final image on top of the base: just add xterm-pty (the sample's
#    Dockerfile also builds a Linux guest we don't need, so we skip that).
echo "==> building build-qemu (adds xterm-pty)"
docker buildx build --load --progress=plain -t build-qemu - <<'DOCKER'
FROM build-qemu-base
WORKDIR /builddeps/
ENV EMCC_CFLAGS="--js-library=/builddeps/node_modules/xterm-pty/emscripten-pty.js"
RUN npm i xterm-pty@v0.10.1
WORKDIR /build/
CMD ["sleep", "infinity"]
DOCKER

# 4. Run it, mounting the patched QEMU source (ro) and our ISO.
docker rm -f build-qemu >/dev/null 2>&1 || true
echo "==> starting build container"
docker run --rm --init -d --name build-qemu \
  -v "$QEMU_REPO:/qemu/:ro" \
  -v "$ROOT/build/ziran.iso:/images/ziran.iso:ro" \
  build-qemu

# 5. Configure + compile QEMU to WASM (the other long step), then package QEMU's
#    pc-bios AND our ISO into the Emscripten virtual FS.
echo "==> compiling qemu-system-x86_64 to WASM + packaging pc-bios + ziran.iso"
docker exec build-qemu /bin/bash -euxc '
  cd /build
  emconfigure /qemu/configure --static --disable-tools --target-list=x86_64-softmmu
  emmake make -j"$(nproc)"
  mkdir -p pack
  cp -r /qemu/pc-bios/* pack/
  cp /images/ziran.iso pack/ziran.iso
  /emsdk/upstream/emscripten/tools/file_packager.py qemu-system-x86_64.data --preload pack > load.js
'

# 6. Copy the generated files into web/ (glue renamed out.js, per the sample).
echo "==> staging assets into web/"
docker cp build-qemu:/build/qemu-system-x86_64.js "$ROOT/web/out.js"
for f in qemu-system-x86_64.wasm qemu-system-x86_64.worker.js qemu-system-x86_64.data load.js; do
  docker cp "build-qemu:/build/$f" "$ROOT/web/"
done
docker rm -f build-qemu >/dev/null 2>&1 || true

# 7. Vendor xterm + xterm-pty locally (COEP: require-corp blocks cross-origin CDN
#    scripts, so self-host them next to the page).
echo "==> vendoring xterm + xterm-pty into web/vendor/"
mkdir -p "$ROOT/web/vendor"
curl -fsSL https://unpkg.com/xterm@5.3.0/lib/xterm.js          -o "$ROOT/web/vendor/xterm.js"
curl -fsSL https://unpkg.com/xterm@5.3.0/css/xterm.css         -o "$ROOT/web/vendor/xterm.css"
curl -fsSL https://unpkg.com/xterm-pty@0.10.1/index.js         -o "$ROOT/web/vendor/xterm-pty.js"

echo
echo "DONE. Staged: web/{out.js,out.wasm,out.worker.js,out.data,load.js,vendor/}"
echo "Next:  make serve   then open  http://localhost:8000/qemu-wasm.html"
echo "If it boots to a ziran:/> prompt, you have the real OS live in the browser."
