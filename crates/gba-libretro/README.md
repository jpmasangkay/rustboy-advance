# gba-libretro — Rustboy Advance Libretro Core

`gba-libretro` exposes the `gba-core` Game Boy Advance emulator through the `libretro.h` v1 C ABI so it can run inside RetroArch, Ludo, and other libretro frontends.

## Features

- **Video**: 240×160 output at 59.7275 FPS (3:2 aspect ratio), preferring `RGB565` via a 32K `BGR555 -> RGB565` lookup table with automatic fallback to `0RGB1555`.
- **Audio**: Interleaved stereo signed 16-bit PCM streamed at `gba_core::SAMPLE_RATE_HZ`.
- **BIOS**: Optional authentic 16 KiB `gba_bios.bin` loaded from the frontend's system directory (`RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY`), with automatic built-in HLE BIOS fallback.
- **Save Memory & RetroAchievements**: Exposes cartridge backup memory (`RETRO_MEMORY_SAVE_RAM`) and EWRAM (`RETRO_MEMORY_SYSTEM_RAM`), plus `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` (EWRAM `0x02000000`, IWRAM `0x03000000`, ROM `0x08000000`, SRAM `0x0E000000`) and `RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS`.
- **Save States**: Length-prefixed `RBLS` serialized wrapper around `gba_core::GbaCore::save_state`/`load_state` with a safe buffer upper bound while keeping memory-map backing buffers pinned across reset/unserialize.
- **Core Options & Peripherals**:
  - `rustboy_color_correction`: `Off`, `GBA` (AGB-001 reflective TFT sRGB), `GBA SP` (AGS-101 backlit).
  - `rustboy_solar_sensor_level`: Boktai solar sensor lux level (`0%`–`100%`).
  - `rustboy_save_type`: `Auto`, `SRAM 32KB`, `Flash 64KB`, `Flash 128KB`, `EEPROM 512B`, `EEPROM 8KB`, `None`.
  - Input descriptors for RetroPad port 0 GBA buttons and cartridge rumble via `RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE`.

## Building

Build the shared library (`cdylib`) from the workspace root:

```sh
cargo build -p gba-libretro
```

For optimized gameplay builds:

```sh
cargo build -p gba-libretro --release
```

Cargo outputs the dynamic library in `target/<profile>/`:

| OS | Built Artifact | RetroArch Core Filename |
|---|---|---|
| Linux | `libgba_libretro.so` | `rustboy_advance_libretro.so` |
| Windows | `gba_libretro.dll` | `rustboy_advance_libretro.dll` |
| macOS | `libgba_libretro.dylib` | `rustboy_advance_libretro.dylib` |

## Installing in RetroArch

1. Copy the compiled dynamic library into your RetroArch `cores/` directory, renaming it to `rustboy_advance_libretro.{so,dll,dylib}` so it matches the info file.
2. Copy [`rustboy_advance_libretro.info`](rustboy_advance_libretro.info) into your RetroArch `info/` directory.
3. *(Optional)* Place a 16 KiB `gba_bios.bin` dump inside RetroArch's `system/` directory (`Settings -> Directory -> System/BIOS`). If omitted, the built-in HLE BIOS is used automatically.

You can also launch directly from the command line without installing into `cores/`:

```sh
retroarch -L target/release/rustboy_advance_libretro.so path/to/game.gba
```

## Running Tests

```sh
cargo test -p gba-libretro
```
