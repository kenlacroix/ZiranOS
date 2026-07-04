#!/usr/bin/env bash
# Upload the qemu-wasm live-boot assets to a Cloudflare R2 bucket.
#
# The big payloads (out.wasm, out.data, ...) exceed Cloudflare Pages' 25 MiB
# per-file limit, so they can't ship in the Pages deploy — host them on R2 and
# point the page at them with  ?assets=https://your-asset-host/  (or set
# ASSET_BASE in qemu-wasm.html). See docs/DEPLOY.md for mapping R2 to the same
# domain (so it's same-origin and needs no CORP/CORS gymnastics under COEP).
#
#   R2_BUCKET=ziran-os-assets ./web/deploy-r2.sh          # via wrangler
#   R2_BUCKET=ziran-os-assets RCLONE_REMOTE=r2: ./web/deploy-r2.sh   # via rclone
set -euo pipefail
cd "$(dirname "$0")"                       # web/
BUCKET="${R2_BUCKET:?set R2_BUCKET (e.g. R2_BUCKET=ziran-os-assets)}"
PREFIX="${R2_PREFIX:-}"                     # optional key prefix, e.g. qemu/
# Emscripten bakes the qemu-system-x86_64.* names into out.js, so keep them.
FILES=(out.js qemu-system-x86_64.wasm qemu-system-x86_64.worker.js qemu-system-x86_64.data load.js)

for f in "${FILES[@]}"; do
  [ -f "$f" ] || { echo "missing web/$f — run ./web/build-qemu-wasm.sh first"; exit 1; }
done

content_type() {
  case "$1" in
    *.js)   echo "text/javascript" ;;
    *.wasm) echo "application/wasm" ;;
    *)      echo "application/octet-stream" ;;   # .data and anything else
  esac
}

if command -v wrangler >/dev/null 2>&1; then
  for f in "${FILES[@]}"; do
    echo "wrangler put: $f -> $BUCKET/$PREFIX$f"
    wrangler r2 object put "$BUCKET/$PREFIX$f" --file="$f" --content-type="$(content_type "$f")"
  done
elif command -v rclone >/dev/null 2>&1; then
  : "${RCLONE_REMOTE:?set RCLONE_REMOTE (your rclone R2 remote, e.g. RCLONE_REMOTE=r2:)}"
  for f in "${FILES[@]}"; do
    echo "rclone copy: $f -> ${RCLONE_REMOTE}${BUCKET}/${PREFIX}${f}"
    rclone copyto "$f" "${RCLONE_REMOTE}${BUCKET}/${PREFIX}${f}" --s3-no-check-bucket
  done
else
  echo "Need 'wrangler' (npm i -g wrangler) or 'rclone', or upload via the R2 dashboard:"
  printf '  web/%s\n' "${FILES[@]}"
  exit 1
fi

echo
echo "Uploaded ${#FILES[@]} files to r2://$BUCKET/$PREFIX"
echo "Next: map the bucket to a URL and set qemu-wasm.html's ASSET_BASE (or ?assets=)."
echo "Verify content-types: out.wasm=application/wasm, *.js=text/javascript."
