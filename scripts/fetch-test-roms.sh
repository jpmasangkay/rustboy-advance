#!/usr/bin/env bash
set -euo pipefail

FORCE=0
ROMS_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --force|-f) FORCE=1 ;;
    --roms-only) ROMS_ONLY=1 ;;
  esac
done

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
  curl -fsSL --retry 3 --retry-delay 2 --connect-timeout 15 --max-time 180 -o "$tmp" "$url"
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
  ppu/bgx/bgx.gba \
  ppu/dispcnt-latch/dispcnt-latch.gba \
  ppu/greenswap/greenswap.gba \
  ppu/ram-access-timing/ram-access-timing.gba \
  ppu/sprite-hmosaic/sprite-hmosaic.gba \
  ppu/status-irq-dma/status-irq-dma.gba \
  ppu/vram-mirror/vram-mirror.gba \
  timer/reload/reload.gba \
  timer/start-stop/start-stop.gba \
  timer/tick-before-reload/tick-before-reload.gba; do
  download_file "$NBA_BASE/$f" "$OUT_ROOT/nba-hw-test/$f"
done
if [[ "$ROMS_ONLY" -eq 0 ]]; then
  for f in \
    ppu/bgpd/expected.png \
    ppu/bgx/expected.png \
    ppu/dispcnt-latch/expected.png \
    ppu/greenswap/expected.png \
    ppu/sprite-hmosaic/expected.png; do
    download_file "$NBA_BASE/$f" "$OUT_ROOT/nba-hw-test/$f"
  done
fi

echo "==> Fetching Tonc prebuilt demos (tonc-bin.zip)"
TONC_DIR="$OUT_ROOT/tonc"
TONC_REQUIRED=(
  bm_modes.gba mos_demo.gba win_demo.gba bld_demo.gba obj_aff.gba obj_demo.gba
  m7_demo.gba prio_demo.gba tte_demo.gba irq_demo.gba tmr_demo.gba dma_demo.gba
)
TONC_MISSING=0
for rom in "${TONC_REQUIRED[@]}"; do
  if [[ ! -s "$TONC_DIR/$rom" ]]; then
    TONC_MISSING=1
    break
  fi
done
if [[ "$FORCE" -eq 1 || "$TONC_MISSING" -eq 1 ]]; then
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
  echo "  [cached] $TONC_DIR (all ${#TONC_REQUIRED[@]} manifest ROMs present)"
fi

CHECKSUM_FILE="$SCRIPT_DIR/test-roms.sha256"
if [[ -f "$CHECKSUM_FILE" ]]; then
  echo "==> Verifying SHA-256 checksums ($CHECKSUM_FILE)"
  if command -v sha256sum >/dev/null 2>&1; then
    SHA_CMD=(sha256sum)
  elif command -v shasum >/dev/null 2>&1; then
    SHA_CMD=(shasum -a 256)
  else
    echo "Error: neither sha256sum nor shasum found to verify ROM checksums" >&2
    exit 1
  fi
  VERIFIED=0
  while read -r expected relpath; do
    [[ -z "$expected" || "$expected" == \#* ]] && continue
    if [[ "$ROMS_ONLY" -eq 1 && "$relpath" == *.png ]]; then
      continue
    fi
    target="$OUT_ROOT/$relpath"
    if [[ ! -f "$target" ]]; then
      echo "Error: missing expected file $target" >&2
      exit 1
    fi
    actual="$("${SHA_CMD[@]}" "$target" | awk '{print $1}')"
    if [[ "$actual" != "$expected" ]]; then
      echo "Error: SHA-256 mismatch for $relpath (expected $expected, got $actual)" >&2
      exit 1
    fi
    VERIFIED=$((VERIFIED + 1))
  done < "$CHECKSUM_FILE"
  echo "  Verified $VERIFIED files against SHA-256 manifest."
fi

echo ""
echo "Note: mGBA suite (mgba-emu/suite) is source-only; see scripts/README.md for devkitARM build instructions (place at test-roms/mgba/suite.gba)."
echo "Note: AGS Aging Cartridge is proprietary; place user-owned ROM at test-roms/ags/ags.gba if desired."
echo "Done."
