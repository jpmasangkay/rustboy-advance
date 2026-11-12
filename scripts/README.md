# Test ROM Acquisition Guide

This directory contains helper scripts to download freely-redistributable GBA hardware test ROMs into `test-roms/` (gitignored).

## Quick Start

- **Windows (PowerShell):**
  ```powershell
  ./scripts/fetch-test-roms.ps1
  ```
- **Linux / macOS (Bash):**
  ```bash
  ./scripts/fetch-test-roms.sh
  ```

Pass `--force` (or `-Force` in PowerShell) to re-download existing files.

## Included Suites (Pinned Commits)

| Suite | Source | Pinned Revision | Destination |
|-------|--------|-----------------|-------------|
| **jsmolka/gba-tests** | `github.com/jsmolka/gba-tests` | `a7113b67e63f83a9b321696ddd7042ccfad6c881` | `test-roms/jsmolka/` |
| **FuzzARM** | `github.com/DenSinH/FuzzARM` | `a675329cd57da48e3e406216ba2d79dd7e09ee20` | `test-roms/fuzzarm/` |
| **ARMWrestler (fixed)** | `github.com/destoer/armwrestler-gba-fixed` | `802e55ad61b421f4bbbc6f74f96aa4e5df4d630f` | `test-roms/armwrestler/` |
| **NanoBoyAdvance hw-test** | `codeberg.org/nba-emu/hw-test` | `fbc99140e06f083c0a47612467cfbb02470e56dc` | `test-roms/nba-hw-test/` |
| **Tonc demos** | `coranac.com/files/tonc-bin.zip` | 2013-03-28 archive | `test-roms/tonc/` |

## Optional / User-Supplied Suites

### mGBA Test Suite (`mgba-emu/suite`)

`mgba-emu/suite` does not publish prebuilt binaries on GitHub Releases; only source is hosted (pinned reference commit: `e6942030d25ffe3ba76c72b73a86da073ec857cc`). Build `suite.gba` using **devkitARM** (either natively or via Docker):

```bash
git clone https://github.com/mgba-emu/suite.git
cd suite
git checkout e6942030d25ffe3ba76c72b73a86da073ec857cc
# Option A: Docker (uses devkitpro/devkitarm)
docker run --rm -v "$PWD:/root/suite" -w /root/suite devkitpro/devkitarm make
# Option B: Local devkitPro installation (with DEVKITPRO & DEVKITARM set)
make
```

Then copy `suite.gba` to `test-roms/mgba/suite.gba`.

### AGS Aging Cartridge

The official Nintendo **AGS Aging Cartridge** is commercial/proprietary software and is **never** downloaded by these scripts. If you own a legal backup, place it at:

```text
test-roms/ags/ags.gba
```

When present, `gba-testrunner` will automatically execute its manifest entry; when absent, it cleanly reports `SKIP (missing ROM)`.
