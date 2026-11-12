#!/usr/bin/env bash
set -euo pipefail

FORCE=0
if [[ "${1:-}" == "--force" || "${1:-}" == "-f" ]]; then
  FORCE=1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_ROOT="$REPO_ROOT/test-roms"

JSMOLKA_SHA="a7113b67e63f83a9b321696ddd7042ccfad6c881"
FUZZARM_SHA="a675329cd57da48e3e406216ba2d79dd7e09ee20"
ARMWRESTLER_SHA="802e55ad61b421f4bbbc6f74f96aa4e5df4d630f"
NBA_HWTEST_SHA="fbc99140e06f083c0a47612467cfbb02470e56dc"

download_file() {
  local url="$1"
  local dest="$2"
  mkdir -p "$(dirname "$dest")"
  if [[ "$FORCE" -eq 0 && -s "$dest" ]]; then
    echo "  [cached] $dest"
    return 0
  fi
  echo "  [fetch]  $url -> $dest"
  local tmp="${dest}.tmp"
  rm -f "$tmp"
  curl -fsSL --retry 3 --retry-delay 2 --max-time 60 -o "$tmp" "$url"
  if [[ ! -s "$tmp" ]]; then
    echo "Error: downloaded empty file from $url" >&2
    rm -f "$tmp"
    return 1
  fi
  mv "$tmp" "$dest"
}

echo "==> Fetching jsmolka/gba-tests ($JSMOLKA_SHA)"
JSMOLKA_BASE="https://raw.githubusercontent.com/jsmolka/gba-tests/$JSMOLKA_SHA"
download_file "$JSMOLKA_BASE/arm/arm.gba"           "$OUT_ROOT/jsmolka/arm.gba"
download_file "$JSMOLKA_BASE/thumb/thumb.gba"       "$OUT_ROOT/jsmolka/thumb.gba"
download_file "$JSMOLKA_BASE/memory/memory.gba"     "$OUT_ROOT/jsmolka/memory.gba"
download_file "$JSMOLKA_BASE/nes/nes.gba"           "$OUT_ROOT/jsmolka/nes.gba"
download_file "$JSMOLKA_BASE/bios/bios.gba"         "$OUT_ROOT/jsmolka/bios.gba"
download_file "$JSMOLKA_BASE/unsafe/unsafe.gba"     "$OUT_ROOT/jsmolka/unsafe.gba"
download_file "$JSMOLKA_BASE/ppu/hello.gba"        "$OUT_ROOT/jsmolka/ppu/hello.gba"
download_file "$JSMOLKA_BASE/ppu/shades.gba"       "$OUT_ROOT/jsmolka/ppu/shades.gba"
download_file "$JSMOLKA_BASE/ppu/stripes.gba"      "$OUT_ROOT/jsmolka/ppu/stripes.gba"
download_file "$JSMOLKA_BASE/save/flash64.gba"     "$OUT_ROOT/jsmolka/save/flash64.gba"
download_file "$JSMOLKA_BASE/save/flash128.gba"    "$OUT_ROOT/jsmolka/save/flash128.gba"
download_file "$JSMOLKA_BASE/save/none.gba"        "$OUT_ROOT/jsmolka/save/none.gba"
download_file "$JSMOLKA_BASE/save/sram.gba"        "$OUT_ROOT/jsmolka/save/sram.gba"

echo "==> Fetching DenSinH/FuzzARM ($FUZZARM_SHA)"
FUZZARM_BASE="https://raw.githubusercontent.com/DenSinH/FuzzARM/$FUZZARM_SHA"
for f in FuzzARM.gba ARM_Any.gba ARM_DataProcessing.gba THUMB_Any.gba THUMB_DataProcessing.gba; do
  download_file "$FUZZARM_BASE/$f" "$OUT_ROOT/fuzzarm/$f"
done

echo "==> Fetching ARMWrestler ($ARMWRESTLER_SHA)"
download_file \
  "https://raw.githubusercontent.com/destoer/armwrestler-gba-fixed/$ARMWRESTLER_SHA/armwrestler-gba-fixed.gba" \
  "$OUT_ROOT/armwrestler/armwrestler-gba-fixed.gba"

echo "==> Fetching NanoBoyAdvance hw-test ($NBA_HWTEST_SHA)"
NBA_BASE="https://codeberg.org/nba-emu/hw-test/raw/commit/$NBA_HWTEST_SHA"
for f in \
  bus/128kb-boundary/128kb-boundary.gba \
  dma/burst-into-tears/burst-into-tears.gba \
  dma/force-nseq-access/force-nseq-access.gba \
  dma/latch/latch.gba \
  dma/start-delay/start-delay.gba \
  haltcnt/haltcnt.gba \
  irq/irq-delay/irq-delay.gba \
  ppu/bgpd/bgpd.gba \
  ppu/bgpd/expected.png \
  ppu/bgx/bgx.gba \
  ppu/bgx/expected.png \
  ppu/dispcnt-latch/dispcnt-latch.gba \
  ppu/dispcnt-latch/expected.png \
  ppu/greenswap/greenswap.gba \
  ppu/greenswap/expected.png \
  ppu/ram-access-timing/ram-access-timing.gba \
  ppu/sprite-hmosaic/sprite-hmosaic.gba \
  ppu/sprite-hmosaic/expected.png \
  ppu/status-irq-dma/status-irq-dma.gba \
  ppu/vram-mirror/vram-mirror.gba \
  timer/reload/reload.gba \
  timer/start-stop/start-stop.gba \
  timer/tick-before-reload/tick-before-reload.gba; do
  download_file "$NBA_BASE/$f" "$OUT_ROOT/nba-hw-test/$f"
done

echo "==> Fetching Tonc prebuilt demos (tonc-bin.zip)"
TONC_DIR="$OUT_ROOT/tonc"
if [[ "$FORCE" -eq 1 || ! -s "$TONC_DIR/win_demo.gba" ]]; then
  mkdir -p "$TONC_DIR"
  TONC_ZIP="$OUT_ROOT/tonc-bin.zip"
  download_file "https://www.coranac.com/files/tonc-bin.zip" "$TONC_ZIP"
  if command -v unzip >/dev/null 2>&1; then
    unzip -q -o -j "$TONC_ZIP" "bin/*.gba" -d "$TONC_DIR"
  else
    python3 - "$TONC_ZIP" "$TONC_DIR" <<'PY'
import os, sys, zipfile
zpath, outdir = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(zpath) as zf:
    for name in zf.namelist():
        if name.endswith(".gba"):
            dest = os.path.join(outdir, os.path.basename(name))
            with zf.open(name) as src, open(dest, "wb") as dst:
                dst.write(src.read())
PY
  fi
  rm -f "$TONC_ZIP"
else
  echo "  [cached] $TONC_DIR"
fi

echo ""
echo "Note: mGBA suite (mgba-emu/suite) is source-only; see scripts/README.md for devkitARM build instructions (place at test-roms/mgba/suite.gba)."
echo "Note: AGS Aging Cartridge is proprietary; place user-owned ROM at test-roms/ags/ags.gba if desired."
echo "Done."
