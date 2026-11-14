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
| **Tonc demos** | `coranac.com/files/tonc-bin.zip` (Jasper Vijn / cearn, MIT License) | 2013-03-28 archive | `test-roms/tonc/` |

## Licensing & Redistribution Notes

- All fetched GBA test ROMs live in `test-roms/` (gitignored) and are **never** checked into the repository or redistributed in release archives by default.
- **Tonc demos (`tonc-bin.zip`):** Authored by Jasper Vijn (`cearn`, Coranac). Per upstream `libtonc` clarification, the example code and built demos are MIT-licensed; if you ever bundle `tonc/*.gba` in a distribution (e.g. a `sample-roms/` folder), retain attribution (`Copyright (c) J Vijn`) and the MIT license text.
- **mGBA suite (`mgba-emu/suite`) & NanoBoyAdvance (`nba-emu/hw-test`):** Open-source hardware test suites (MPL-2.0 / MIT).
- **jsmolka/gba-tests, FuzzARM, ARMWrestler:** Open-source test suites downloaded from pinned commits above.

## Game Boy / Game Boy Color Suites (`gb-core`)

- `gb-core` integration tests (`crates/gb-core/tests/core_tests.rs`) expect Blargg (`cpu_instrs`, `instr_timing`, `mem_timing`, `mem_timing-2`) and Matt Currie's `dmg-acid2.gb` / `cgb-acid2.gbc` under `test-roms/gb/`.
- **Mooneye Test Suite (`Gekkio/mooneye-test-suite`):** The `mooneye_ld_bb_harness_and_optional_suite` test automatically discovers and runs any `*.gb` / `*.gbc` ROMs placed under `test-roms/gb/mooneye/` using the Mooneye `LD B, B` (`0x40`) breakpoint + Fibonacci pass (`B=3, C=5, D=8, E=13, H=21, L=34`) protocol. Build `mooneye-test-suite` from upstream releases and extract acceptance ROMs into `test-roms/gb/mooneye/`.

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

## Performance & Verification Scripts

The benchmark scripts resolve paths relative to the repository root and must write generated JSON outputs under `perf/` (which is gitignored). `run-phase3*.ps1` take a long time to complete, and `run-phase3.ps1` / `run-phase3b.ps1` modify the active Windows AC power plan via `powercfg` (restoring it on exit).

- `scripts/generate-stress-roms.ps1` — Regenerates the 3 git-ignored synthetic stress ROMs (`stress_aff_objwin.gba`, `stress_mode0_4bg_128spr.gba`, `stress_bitmap_345.gba`) into `roms/homebrew/` via `cargo run -p gba-testrunner --example gen_stress_roms` (pass `-IncludeTonc` to also copy fetched `test-roms/tonc/*.gba` demos).
- `scripts/verify-all-hashes.ps1` — Verifies presented video, core video, and core audio CRC32 hashes for 10 ROMs against `perf/baselines/` (auto-regenerates missing synthetic stress ROMs and falls back to `test-roms/tonc/` when `roms/homebrew/` lacks TONC demos).
- `scripts/run-phase3.ps1` — Runs legacy single-iteration weak-hardware benchmarks (`powercfg` throttling + Core-0 contention) on *Summon Night* and writes `perf/weak_hardware.json`.
- `scripts/run-phase3b.ps1` — Runs 5-iteration `powercfg` throttle validation, Core-0 contention, and `--slowdown` sweeps on *Summon Night* and writes `perf/phase3b_results.json`.
- `scripts/run-phase3c.ps1` — Runs realtime homebrew/stress baselines across 9 ROMs and a 5-iteration `--slowdown` sweep on 2 ROMs, writing `perf/phase3c_realtime.json` and `perf/phase3c_slowdown_sweep.json`.
