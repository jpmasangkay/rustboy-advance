# gba-libretro — Rustboy Advance Libretro Core

`gba-libretro` exposes both `gba-core` (Game Boy Advance) and `gb-core` (Game Boy / Game Boy Color) through the `libretro.h` v1 C ABI so they can run inside RetroArch, Ludo, and other libretro frontends.

## Features

- **Video**: 240×160 (GBA, 3:2) and 160×144 (GB/GBC, 10:9) output at exact hardware refresh (`16777216 / 280896` ≈ 59.7275 Hz), preferring `RGB565` via a 32K `BGR555 -> RGB565` lookup table with automatic fallback to `0RGB1555`.
- **Audio**: Interleaved stereo signed 16-bit PCM streamed at `gba_core::SAMPLE_RATE_HZ`, handling partial `retro_audio_sample_batch` consumption cleanly.
- **BIOS**: Optional authentic 16 KiB `gba_bios.bin` loaded from the frontend's system directory (`RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY`), with automatic built-in HLE BIOS fallback.
- **Save Memory, RTC & RetroAchievements**:
  - Exposes battery-backed save memory (`RETRO_MEMORY_SAVE_RAM`), system RAM (`RETRO_MEMORY_SYSTEM_RAM`), and 48-byte MBC3 RTC persistence (`RETRO_MEMORY_RTC`).
  - Initializes GBA GPIO RTC and GB/GBC MBC3 RTC from the host clock on game load/reset.
  - Registers full `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` (GBA: EWRAM `0x02000000`, IWRAM `0x03000000`, ROM `0x08000000`, SRAM `0x0E000000`; GB/GBC: ROM `0x0000`, VRAM `0x8000`, SRAM `0xA000`, WRAM banks 0–1 `0xC000`/`0xD000`, CGB WRAM banks 2–7 `0x10000`, OAM `0xFE00`, HRAM `0xFF80`) and `RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS`.
- **Save States & Rewind**: Uncompressed length-prefixed `RBLS` serialized wrapper around `GbaCore::save_state_raw`/`load_state_raw` and `GbCore::save_state`/`load_state` for optimal RetroArch XOR-delta rewind compression, with unwind-safe memory-map buffer pinning across reset/unserialize.
- **Input & Core Options**:
  - Uses `RETRO_ENVIRONMENT_GET_INPUT_BITMASKS` when supported by the frontend for single-call Joypad polling.
  - `rustboy_color_correction`: `Off`, `GBA` (AGB-001 reflective TFT sRGB), `GBA SP` (AGS-101 backlit).
  - `rustboy_solar_sensor_level`: Boktai solar sensor lux level (`0%`–`100%`).
  - `rustboy_save_type`: `Auto`, `SRAM 32KB`, `Flash 64KB`, `Flash 128KB`, `EEPROM 512B`, `EEPROM 8KB`, `None`.
  - Input descriptors for RetroPad port 0 GBA/GB buttons and cartridge rumble via `RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE`.

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
| Linux | `librustboy_advance_libretro.so` | `rustboy_advance_libretro.so` |
| Windows | `rustboy_advance_libretro.dll` | `rustboy_advance_libretro.dll` |
| macOS | `librustboy_advance_libretro.dylib` | `rustboy_advance_libretro.dylib` |

## Installing in RetroArch

1. Copy the compiled dynamic library into your RetroArch `cores/` directory (`rustboy_advance_libretro.{so,dll,dylib}`, dropping any `lib` prefix on Linux/macOS to match the `.info` file).
2. Copy [`rustboy_advance_libretro.info`](rustboy_advance_libretro.info) into your RetroArch `info/` directory.
3. *(Optional)* Place a 16 KiB `gba_bios.bin` dump inside RetroArch's `system/` directory (`Settings -> Directory -> System/BIOS`). If omitted, the built-in HLE BIOS is used automatically.

You can also launch directly from the command line without installing into `cores/`:

```sh
retroarch -L target/release/librustboy_advance_libretro.so path/to/game.gba
```

## Running Tests

```sh
cargo test -p gba-libretro
```
