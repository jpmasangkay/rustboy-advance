# 🎮 Rustboy Advance
[![Ask DeepWiki](https://devin.ai/assets/askdeepwiki.png)](https://deepwiki.com/jpmasangkay/rustboy-advance)

### Cycle-Accurate Game Boy Advance & Game Boy Emulator — Rust Core, WebAssembly, Libretro & Modern Desktop Frontend

**Emulate. Debug. Rewind. Pure Rust.**

A high-performance, cycle-accurate Game Boy Advance (GBA) and Game Boy / Game Boy Color (GB/GBC) emulator, debugger, and TAS toolkit built entirely in Rust. Features an HLE/LLE ARM7TDMI core with pipelined instruction execution, scanline-accurate Mode 0–5 PPU with multi-layer alpha blending, comprehensive 6-channel APU synthesizer with Kaiser sinc resamplers, fluid real-time XOR-delta rewind, live disassembler/memory inspector, GDB remote stub on `:2345`, GLSL shader pipeline, pure-Rust Piccolo Lua 5.4 scripting, RetroAchievements integration, 2-player local split-screen & TCP Netplay link cable, and a sleek DuckStation-inspired game library interface.

[![Rust](https://img.shields.io/badge/Rust-1.85+-DEA584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/Edition-2024-blue?style=for-the-badge)](https://www.rust-lang.org)
[![WebAssembly](https://img.shields.io/badge/WebAssembly-WASM-654FF0?style=for-the-badge&logo=webassembly&logoColor=white)](https://webassembly.org)
[![egui](https://img.shields.io/badge/egui-0.31-4183C4?style=for-the-badge)](https://github.com/emilk/egui)
[![winit](https://img.shields.io/badge/winit-0.30-333333?style=for-the-badge)](https://github.com/rust-windowing/winit)
[![OpenGL](https://img.shields.io/badge/OpenGL-Glow-5586A4?style=for-the-badge&logo=opengl&logoColor=white)](https://github.com/grovesNL/glow)
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
- **GLSL shader pipeline** — hardware-accelerated GLSL 140 / OpenGL 3.3 shaders with preset hot-switching:
  - `Nearest` (pixel-crisp integer scaling)
  - `SharpBilinear` (anti-shimmer pixel scaling)
  - `LcdGrid` (authentic GBA LCD pixel matrix with subpixel boundaries)
  - `CrtScanlines` (classic CRT beam profile with phosphor glow)
  - `ScaleEdge` (smart edge smoothing)
  - `Frame Blending` (50/50 flicker fix for transparency effects)

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
| **gba-frontend-desktop** | Cross-platform native desktop application with egui, GLSL shaders, TAS movies, Lua, and Netplay |
| **gba-frontend-web** | WebAssembly (WASM) frontend bindings with AudioWorklet and WebGL presentation |
| **gba-libretro** | Libretro core bridge for RetroArch and RetroPie ecosystems with 32K color LUTs and rumble |
| **gba-testrunner** | Automated test harness executing JSMolka, FuzzARM, ARMWrestler, NanoBoyAdvance, and Tonc suites |
| **winit 0.30 & glutin 0.32** | Cross-platform window creation and OpenGL context management |
| **glow** | Modern, lightweight OpenGL bindings for accelerated framebuffer presentation and GLSL shaders |
| **egui 0.31** | Immediate-mode GUI library powering the ROM library, menu bars, HUD, and debugger windows |
| **cpal** | Low-latency cross-platform native audio streaming |
| **gilrs** | Cross-platform gamepad and controller input handling with vibration support |
| **piccolo** | Pure-Rust, allocation-safe Lua 5.4 scripting VM |

---

## 🚀 Getting Started

### Prerequisites

- [Rust SDK](https://rustup.rs/) (1.85 or later with `cargo`)
- A system supporting OpenGL 3.3+ (Windows, Linux, or macOS)
- GBA ROM files (`.gba`) or GB/GBC ROM files (`.gb`, `.gbc`)

### Installation & Build

```bash
# Clone the repository
git clone https://github.com/jpmasangkay/rustboy-advance.git
cd rustboy-advance

# Build the desktop release binary
cargo build --release -p gba-frontend-desktop

# Run the automated test suite across all test ROMs
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

| GBA Button | Primary Key | Alternate Key | Gamepad (XInput / DualShock) |
|------------|-------------|---------------|------------------------------|
| **D-Pad Up** | `Up Arrow` | `W` | D-Pad Up / Left Stick Up |
| **D-Pad Down** | `Down Arrow` | `S` | D-Pad Down / Left Stick Down |
| **D-Pad Left** | `Left Arrow` | `A` | D-Pad Left / Left Stick Left |
| **D-Pad Right** | `Right Arrow` | `D` | D-Pad Right / Left Stick Right |
| **Button A** | `Z` | `J` | Button A (Cross) |
| **Button B** | `X` | `K` | Button B (Circle) |
| **Left Shoulder (L)** | `Q` | `U` | Left Bumper (LB / L1) |
| **Right Shoulder (R)** | `E` | `I` | Right Bumper (RB / R1) |
| **Start** | `Enter` | `Return` | Start / Options |
| **Select** | `Backspace` | `Shift` | Back / Share |

### Hotkeys & Shortcuts

| Action | Shortcut |
|--------|----------|
| **Rewind Gameplay** | Hold `R` |
| **Fast Forward (Turbo)** | Hold `Tab` / Toggle `Space` |
| **Pause / Resume** | `P` / `Space` |
| **Toggle Fullscreen** | `F11` / Double-Click Viewport |
| **Visual Save State Manager** | `F6` |
| **Quick Save State (Slots 1–5)** | `F1` – `F5` |
| **Quick Load State (Slots 1–5)** | `Shift+F1` – `Shift+F5` |
| **Undo Last Save / Load Action** | `Ctrl+Z` |
| **Advance Single Frame** | `Ctrl+N` |
| **Hard Reset** | `Ctrl+R` |
| **Mute / Unmute Audio** | `Ctrl+M` |
| **Take Screenshot** | `F12` |
| **Toggle HUD & Status Bar** | `Ctrl+S` |
| **Memory Search** | `Ctrl+F` |
| **GDB Remote Stub** | `Ctrl+G` |
| **Lua Scripting Console** | `Ctrl+L` |
| **Record GIF Video** | `Ctrl+Shift+R` |
| **Record WAV Audio** | `Ctrl+Shift+A` |

---

## 👥 Author

Built with ❤️ by **John Paul Masangkay** ([@jpmasangkay](https://github.com/jpmasangkay))

---

## 👏 Acknowledgements

- **Martin Korth (GBATEK)** — The definitive Game Boy Advance technical reference
- **Vicki Pfau & the mGBA project** — Invaluable documentation and test suites for GBA edge cases
- **NanoBoyAdvance, JSMolka, and FuzzARM authors** — Outstanding automated hardware test ROM suites
- **The Rust Community** — For crates like `egui`, `winit`, `glow`, `cpal`, and `piccolo`

---

## 📄 License

This project is licensed under the Mozilla Public License 2.0 ([MPL-2.0](LICENSE)).
