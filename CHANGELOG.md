# Changelog

All notable changes to **Rustboy Advance** are documented in this file.

## [0.1.0] - 2026-11-14

### Accuracy, Test Suites & Compatibility
- Clarified timing terminology across documentation as **cycle-aware** (replacing unsubstantiated "cycle-accurate" claims), including HLE BIOS approximations and real-BIOS guidance.
- Added automated regression and accuracy test coverage across `gba-testrunner` and workspace crates:
  - `jsmolka/gba-tests`, `FuzzARM`, `ARMWrestler`, `NanoBoyAdvance hw-test`, `Tonc` smoke tests, and optional `mGBA` / `AGS` / `Blargg` / `acid2` / `Mooneye` suites.
  - Added HLE BIOS edge-case tests (BIOS open-bus reads, SWI timing, `SoundBias`) and dedicated tests for `WAITCNT` prefetch, DMA timing boundaries, and 4-stage timer cascade (`crates/gba-core/tests/core_tests.rs`).
  - Published exact automated pass/fail/skip breakdown in [`TEST_RESULTS.md`](docs/TEST_RESULTS.md) (`43 passed, 9 failed, 2 skipped (missing ROM), 2 skipped (missing golden)`).
- Expanded [`crates/gba-core/src/gamedb.rs`](crates/gba-core/src/gamedb.rs) with per-game save-type / RTC / hardware-sensor overrides (`GAME_DB`) and `REAL_BIOS_FALLBACK_GAMES` diagnostics (`GbaCore::recommends_real_bios()`), documented across 60+ commercial titles in [`COMPATIBILITY.md`](docs/COMPATIBILITY.md).
- Cited Jasper Vijn (`cearn`) MIT License terms for `tonc-bin.zip` in [`scripts/README.md`](scripts/README.md) and [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

### Windows Polish, Storage & Privacy
- Enabled `#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]` for release desktop binaries on Windows so GUI launches do not spawn an extra console window, while calling `AttachConsole(ATTACH_PARENT_PROCESS)` for CLI/headless/help invocations (`crates/gba-frontend-desktop/src/main.rs`).
- Unified persistent application data storage (`config.json`, `rustboy_library.cfg`, `saves/`, `states/`, `covers/`, `screenshots/`, crash & startup logs) under `%APPDATA%\RustboyAdvance` on Windows (`$XDG_CONFIG_HOME/RustboyAdvance` or `~/.config/RustboyAdvance` on Unix) by default, with explicit portable mode via `--portable` or placing `portable.txt` next to the executable, plus read-fallback for legacy sibling saves and save states.
- Moved `ra_token` out of plaintext `config.json` into the OS credential store via `keyring` (`Windows Credential Manager` / macOS Keychain / Linux Secret Service & keyutils) with automatic legacy config migration (`crates/gba-frontend-desktop/src/config.rs`).
- Enforced strict opt-in defaults (`false`) for RetroAchievements (`ra_enabled`) and Libretro box-art downloads (`auto_download_covers`).
- Renamed legacy display filter labels and slugs (`MyBoy Smooth` / `myboy-smooth` -> `Smooth Bilinear` / `smooth-bilinear`).
- Verified binary `RBAS` v1 versioned save-state headers (`gba-core` & `savestates.rs`) and per-game settings overrides keyed by game code and ROM CRC32 (`game_settings.rs`).

### Cross-Platform, Frontends & Release Packaging
- Verified `no_std + alloc` zero-host-I/O core builds (`gba-core --no-default-features`, `gb-core`, `gba-debugger --no-default-features`), the Libretro core (`gba-libretro`), and the WebAssembly frontend (`gba-frontend-web` + `web/`).
- Updated `.cargo/config.toml` (`linker = "rust-lld"` for `x86_64-pc-windows-gnu`) so cross-target checks such as `wasm32-unknown-unknown` link host proc-macros/build scripts cleanly.
- Updated [`.github/workflows/ci.yml`](.github/workflows/ci.yml) with `no_std`, `wasm32-unknown-unknown`, `aarch64-linux-android`, `aarch64-apple-ios`, `aarch64-unknown-linux-gnu`, and `--remap-path-prefix` / GUI-subsystem verification.
- Added [`.github/workflows/release.yml`](.github/workflows/release.yml) for multi-platform release artifacts (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `wasm32` web bundle) with path-prefix stripping, forward-slash archive creation (`tar.exe -a` / `tar` / `zip`), and `SHA256SUMS.txt` checksums.
- Added [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) listing all statically linked Rust crates, embedded UI fonts, test ROM suites, and MPL-2.0 source notices.

