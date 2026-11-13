# 🎮 Rustboy Advance
[![Ask DeepWiki](https://devin.ai/assets/askdeepwiki.png)](https://deepwiki.com/jpmasangkay/rustboy-advance)

### Cycle-Accurate Game Boy Advance & Game Boy Emulator — Rust Core, WebAssembly, Libretro & Modern Desktop Frontend

**Emulate. Debug. Rewind. Pure Rust.**

A high-performance, cycle-accurate Game Boy Advance (GBA) and Game Boy / Game Boy Color (GB/GBC) emulator, debugger, and TAS toolkit built entirely in Rust. Features an HLE/LLE ARM7TDMI core with pipelined instruction execution, scanline-accurate Mode 0–5 PPU with multi-layer alpha blending, comprehensive 6-channel APU synthesizer with Kaiser sinc resamplers, fluid real-time XOR-delta rewind, live disassembler/memory inspector, GDB remote stub on `:2345`, WGSL (`wgpu`) shader pipeline with automatic `winit`/`softbuffer` No-GPU fallback, pure-Rust Piccolo Lua 5.4 scripting, RetroAchievements integration, 2-player local split-screen & TCP Netplay link cable, and a sleek DuckStation-inspired game library interface.

[![Rust](https://img.shields.io/badge/Rust-1.85+-DEA584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/Edition-2024-blue?style=for-the-badge)](https://www.rust-lang.org)
[![WebAssembly](https://img.shields.io/badge/WebAssembly-WASM-654FF0?style=for-the-badge&logo=webassembly&logoColor=white)](https://webassembly.org)
[![egui](https://img.shields.io/badge/egui-0.31-4183C4?style=for-the-badge)](https://github.com/emilk/egui)
[![winit](https://img.shields.io/badge/winit-0.30-333333?style=for-the-badge)](https://github.com/rust-windowing/winit)
[![wgpu](https://img.shields.io/badge/wgpu-24.0-4E8EE9?style=for-the-badge)](https://wgpu.rs)
[![License: MPL-2.0](https://img.shields.io/badge/License-MPL--2.0-yellow.svg?style=for-the-badge)](LICENSE)

🖥️ **Repository:** [github.com/jpmasangkay/rustboy-advance](https://github.com/jpmasangkay/rustboy-advance)

---

## ✨ Features

### 🕹️ CPU & Core Emulation (ARM7TDMI & SM83)

- **ARMv4T instruction set** — full 32-bit ARM and 16-bit Thumb instruction execution with exact 3-stage fetch-decode-execute pipelining, verified against FuzzARM, ARMWrestler, and JSMolka suites.
- **Fastmem translation** — zero-overhead direct page-table pointer mapping for EWRAM, IWRAM, and GamePak ROM, with accurate open bus fallback.
- **Cycle-accurate DMA** — 4 independent DMA channels (0–3) supporting immediate, VBlank, HBlank, and Sound FIFO burst transfers.
- **Dual BIOS support** — built-in high-performance High-Level Emulation (HLE) BIOS implementing all SWI routines (Div, Sqrt, CpuSet, CpuFastSet, LZ77, Huffman, RL, IntrWait, VBlankIntrWait), plus seamless Low-Level Emulation (LLE) using authentic BIOS dumps.
- **Game Boy & Game Boy Color (`gb-core`)** — dedicated `#![no_std]` + alloc DMG and CGB core with SM83 CPU, MBC1/2/3(+RTC)/5(+rumble), PPU with STAT blocking, HDMA, and 48 kHz APU.

### 🎨 PPU & Video Pipeline

- **All display modes** — Mode 0, 1, 2 (tiled text & affine backgrounds) and Mode 3, 4, 5 (bitmap frame-buffer modes with page flipping and VRAM unmapped mirroring).
- **128 hardware sprites (OBJs)** — rotation/scaling (affine matrix transformations), mosaic effects, double-size flags, and semi-transparent priority arbitration.
- **Full color compositor** — multi-layer alpha blending, brightness increase/decrease, windowing (Win0, Win1, WinOBJ), and GREENSWAP support.
- **WGSL (`wgpu`) shader pipeline & No-GPU fallback** — hardware-accelerated WGSL shaders (`Vulkan` → `DX12` → `GL`) with GPU startup probe guard, adaptive `QualityGovernor` scaling, and automatic `winit` + `softbuffer` No-GPU fallback (capped at `720×480` via Win32 `StretchDIBits` on Windows):
  - **Shader presets:** `Nearest`, `SharpBilinear`, `LcdGrid`, `CrtScanlines`, and `ScaleEdge`
  - **Display filters & color LUTs:** `MyBoy Smooth`, `Sharp 1x`, `xBR 3x`, `LCD Matrix`, `Soft LCD`, `Scale2x`, and `CRT Scanlines` (`AGB-001`, `AGS-101`, `AGS-001`, `Linear`)
  - **Frame Blending & LCD Ghosting** (50/50 transparency flicker fix and inter-frame persistence)

### 🔊 APU & Sound Synthesis

- **Bandlimited Kaiser-sinc synthesis** — stereo Blip-buffer with 48 kHz / 32,768 frame sequencer for anti-aliased square and wave outputs.
- **Complete 6-channel audio** — Tone 1 (pitch sweep), Tone 2 (square wave), Wave Memory (arbitrary 4-bit sample playback), Noise generator, and Direct Sound A/B FIFO DMA audio.
- **High-fidelity audio post-processing** — 16 Hz DC high-pass blocker and 8-tap Blackman-sinc fractional resampler.
- **Low-latency streaming** — cross-platform real-time audio playback powered by `cpal` with ringbuffer synchronization.

### 💾 Cartridge, Save Hardware & Soft-Patching

- **Intelligent backup detection & GameDB** — auto-detects and supports SRAM (32KB), Flash (64KB/128KB with Panasonic & Sanyo device IDs), and EEPROM (512B/8KB) with bundled overrides for over 30 classic titles.
- **Real-Time Clock (RTC)** — cycle-accurate MBC3-style RTC emulation for day/night cycles (*Pokémon Emerald/Ruby/Sapphire*).
- **Specialized sensor peripherals** — solar sensor (*Boktai*), tilt sensor (*Yoshi's Universal Gravitation*), gyro sensor (*WarioWare: Twisted!*), and rumble feedback.
- **Universal soft-patching & archives** — automatically parses and applies **BPS**, **UPS**, and **IPS** patches on the fly, and reads ROMs directly from `.zip` and `.7z` archives.

### ⏱️ Rewind, TAS & Media Capture

- **XOR-Delta circular state rewind** — memory-efficient rolling circular buffer capturing XOR deltas second-by-second with fluid 60 FPS rewind.
- **RBM TAS input movies** — record and replay frame-accurate input movies (`.rbm`) with embedded LZ4 state headers and rerecord tracking.
- **Media recording** — in-app lossless **WAV** audio capture and animated **GIF** video recording.

### 🌐 Link Cable & Netplay

- **Local split-screen 2-player** — run two GBA instances side-by-side with P1 and P2 independent controls and synced serial communication.
- **TCP Netplay bridge** — peer-to-peer remote link cable emulation over TCP sockets supporting SIO Normal 8-bit, Normal 32-bit, and Multiplayer modes.

### 📜 Scripting & RetroAchievements

- **Pure-Rust Piccolo Lua 5.4 engine** — run custom Lua scripts with memory read/write hooks, joypad input injection, and on-screen GUI drawing (text, boxes, circles).
- **RetroAchievements integration** — authentic game identification via MD5 hashing, `MemAddr` trigger evaluation, hardcore mode anti-cheat gating, rich presence strings, and unlock toast banners.

### 🔍 Advanced Debugger Suite

- **GDB Remote Serial Protocol (RSP)** — integrated GDB server on `localhost:2345` for source-level debugging with `gdb-multiarch` or Ghidra.
- **Execution controls** — step-into, step-over, step-out, run to cursor, and instruction breakpoints.
- **Memory search** — scan EWRAM, IWRAM, and VRAM with Exact, Greater, Less, or Changed comparisons across Integer, Float, and Hex types, with address value freezing.
- **Callstack backtrace & IO inspector** — tracks subroutine calls and returns across ARM and Thumb modes with bitfield breakdown of all memory-mapped IO registers.

---

## 🛠️ Tech Stack & Workspace Crates

| Package / Crate | Purpose |
|-----------------|---------|
| **Rust 2024 Edition (MSRV 1.85)** | High-performance systems programming language providing bare-metal speed and memory safety |
| **gba-core** | Decoupled, `no_std`-ready core emulation engine (ARM7TDMI, PPU, APU, Bus, Cartridge, Timers, DMA) |
| **gb-core** | Dedicated `#![no_std]` + alloc Game Boy & Game Boy Color emulation core (SM83, MBC, PPU, APU) |
| **gba-debugger** | Disassembler for ARM and Thumb instruction sets, breakpoint engine, memory inspector, GDB stub |
| **gba-frontend-desktop** | Cross-platform native desktop application with `eframe`/`egui`, WGSL (`wgpu`) shaders, `softbuffer` No-GPU fallback, `.rbm` TAS movies, Lua, and Netplay |
| **gba-frontend-web** | WebAssembly (WASM) frontend bindings paired with Canvas 2D pixel presentation and AudioWorklet audio |
| **gba-libretro** | Libretro core bridge for RetroArch and RetroPie ecosystems with 32K color LUTs and rumble |
| **gba-testrunner** | Automated test harness executing JSMolka, FuzzARM, ARMWrestler, NanoBoyAdvance, and Tonc suites |
| **winit 0.30 & softbuffer 0.4** | Cross-platform window management and software framebuffer presentation for `--no-gpu` fallback |
| **wgpu 24.0** | Hardware-accelerated GPU presentation (`Vulkan`, `DX12`, `GL`) and WGSL post-processing shaders |
| **egui 0.31** | Immediate-mode GUI library powering the ROM library, menu bars, HUD, and debugger windows |
| **cpal** | Low-latency cross-platform native audio streaming |
| **gilrs** | Cross-platform gamepad and controller input handling with vibration support |
| **piccolo** | Pure-Rust, allocation-safe Lua 5.4 scripting VM |

---

## 🚀 Getting Started

### Prerequisites

- [Rust SDK](https://rustup.rs/) (1.85 or later with `cargo`)
- Windows, Linux, or macOS (Vulkan, DX12, Metal, or OpenGL GPU recommended; automatic `winit` + `softbuffer` `--no-gpu` CPU fallback supported)
- GBA ROM files (`.gba`) or GB/GBC ROM files (`.gb`, `.gbc`)

### Installation & Build

```bash
# Clone the repository
git clone https://github.com/jpmasangkay/rustboy-advance.git
cd rustboy-advance

# Build the desktop release binary
cargo build --release -p gba-frontend-desktop

# Fetch public test ROMs (first time only), then run the automated test suite
./scripts/fetch-test-roms.sh   # or .\scripts\fetch-test-roms.ps1 on Windows
cargo run -p gba-testrunner -- --report TEST_RESULTS.md --fail-on-unexpected
```

### Running the Desktop App

```bash
# Launch the desktop UI with ROM library browser
cargo run --release -p gba-frontend-desktop

# Launch directly into a specific GBA or GB/GBC ROM
cargo run --release -p gba-frontend-desktop -- "path/to/game.gba"

# Start GDB remote debugging server on localhost:2345
cargo run --release -p gba-frontend-desktop -- "game.gba" --gdb 2345

# Run headless for 120 frames and output framebuffer CRC32 checksum
cargo run --release -p gba-frontend-desktop -- "game.gba" --headless --frames 120 --fb-crc32
```

### WebAssembly (WASM) Web Frontend

```bash
# Build the WASM package
cargo build --target wasm32-unknown-unknown --release -p gba-frontend-web
wasm-bindgen --target web --out-dir web/pkg target/wasm32-unknown-unknown/release/gba_frontend_web.wasm

# Start local server and open in browser
node web/server.js
# Navigate to http://localhost:8080
```

---

## 🎮 Controls

### Default Keyboard Mapping

| GBA Button | Player 1 Default | Player 2 (Local Link) |
|------------|------------------|-----------------------|
| **D-Pad Up** | `ArrowUp` | `I` |
| **D-Pad Down** | `ArrowDown` | `K` |
| **D-Pad Left** | `ArrowLeft` | `J` |
| **D-Pad Right** | `ArrowRight` | `L` |
| **Button A** | `Z` | `O` |
| **Button B** | `X` | `U` |
| **Left Shoulder (L)** | `Q` | `7` |
| **Right Shoulder (R)** | `E` | `8` |
| **Start** | `Enter` | `N` |
| **Select** | `Backspace` | `M` |
| **Turbo A / Turbo B** | `V` / `C` | — |

Gamepad controllers (`gilrs`) are supported out of the box and all keyboard/gamepad bindings can be remapped in the UI under **Input Settings**.

### Hotkeys & Shortcuts

| Action | Shortcut |
|--------|----------|
| **Open ROM** | `Ctrl+O` |
| **Close ROM / Return to Library** | `Ctrl+W` |
| **Pause Overlay / Exit Fullscreen** | `Esc` |
| **Pause / Resume** | `Space` |
| **Rewind Gameplay** | Hold `R` |
| **Fast Forward (Turbo)** | Hold `Tab` |
| **Toggle Fullscreen** | `F11` |
| **Visual Save State Manager** | `F6` |
| **Quick Save State (Slots 1–5)** | `F1` (configurable), `F2`–`F5` |
| **Quick Load State (Slots 1–5)** | `Shift+F1` (configurable), `Shift+F2`–`F5` |
| **Undo Last Save / Load Action** | `Ctrl+Z` |
| **Advance Single Frame** | `Ctrl+N` |
| **Hard Reset** | `Ctrl+R` |
| **Mute / Unmute Audio** | `Ctrl+M` |
| **Take Screenshot** | `F12` |
| **Toggle Action Toolbar** | `Ctrl+T` |
| **Toggle Status Bar** | `Ctrl+S` |
| **Boktai Solar Sensor (- / +)** | `[` / `]` |

---

## 👥 Author

Built with ❤️ by **John Paul Masangkay** ([@jpmasangkay](https://github.com/jpmasangkay))

---

## 👏 Acknowledgements

- **Martin Korth (GBATEK)** — The definitive Game Boy Advance technical reference
- **Vicki Pfau & the mGBA project** — Invaluable documentation and test suites for GBA edge cases
- **NanoBoyAdvance, JSMolka, and FuzzARM authors** — Outstanding automated hardware test ROM suites
- **The Rust Community** — For crates like `egui`, `winit`, `wgpu`, `softbuffer`, `cpal`, and `piccolo`

---

## 📄 License

This project is licensed under the Mozilla Public License 2.0 ([MPL-2.0](LICENSE) — see the [official text](https://www.mozilla.org/en-US/MPL/2.0/)).
