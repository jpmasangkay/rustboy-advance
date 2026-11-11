# 🎮 Rustboy Advance
[![Ask DeepWiki](https://devin.ai/assets/askdeepwiki.png)](https://deepwiki.com/jpmasangkay/rustboy-advance)

### Cycle-Accurate Game Boy Advance Emulator — Rust Core, WebAssembly & Modern Desktop Frontend

**Emulate. Debug. Rewind. Pure Rust.**

A high-performance, cycle-accurate Game Boy Advance (GBA) emulator and debugger built entirely in Rust. Features an HLE/LLE ARM7TDMI core with pipelined instruction execution, scanline-accurate Mode 0–5 PPU with multi-layer alpha blending, comprehensive 6-channel APU synthesizer, fluid real-time rewind, live disassembler/memory inspector, and a sleek DuckStation-inspired game library interface.

[![Rust](https://img.shields.io/badge/Rust-1.75+-DEA584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![WebAssembly](https://img.shields.io/badge/WebAssembly-WASM-654FF0?style=for-the-badge&logo=webassembly&logoColor=white)](https://webassembly.org)
[![egui](https://img.shields.io/badge/egui-0.31-4183C4?style=for-the-badge)](https://github.com/emilk/egui)
[![winit](https://img.shields.io/badge/winit-0.30-333333?style=for-the-badge)](https://github.com/rust-windowing/winit)
[![OpenGL](https://img.shields.io/badge/OpenGL-Glow-5586A4?style=for-the-badge&logo=opengl&logoColor=white)](https://github.com/grovesNL/glow)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg?style=for-the-badge)](LICENSE)

🖥️ **Repository:** [github.com/jpmasangkay/rustboy-advance](https://github.com/jpmasangkay/rustboy-advance)

---

## ✨ Features

### 🕹️ CPU & Core Emulation (ARM7TDMI)

- **ARMv4T instruction set** — full 32-bit ARM and 16-bit Thumb instruction execution with exact 3-stage fetch-decode-execute pipelining
- **Fastmem translation** — zero-overhead direct page-table pointer mapping for EWRAM, IWRAM, VRAM, and GamePak ROM
- **Cycle-accurate DMA** — 4 independent DMA channels (0–3) supporting immediate, VBlank, HBlank, and Sound FIFO burst transfers
- **Dual BIOS support** — built-in high-performance High-Level Emulation (HLE) BIOS implementing all SWI routines (Div, Sqrt, CpuSet, CpuFastSet, LZ77, Huffman, RL, IntrWait, VBlankIntrWait), plus seamless Low-Level Emulation (LLE) using authentic BIOS dumps

### 🎨 PPU & Video Pipeline

- **All display modes** — Mode 0, 1, 2 (tiled text & affine backgrounds) and Mode 3, 4, 5 (bitmap frame-buffer modes with page flipping)
- **128 hardware sprites (OBJs)** — rotation/scaling (affine matrix transformations), mosaic effects, double-size flags, and semi-transparent priority arbitration
- **Full color compositor** — multi-layer alpha blending, brightness increase/decrease, and complex windowing (Win0, Win1, WinOBJ)
- **Visual post-processing** — CRT scanlines, LCD pixel grid, sRGB color correction, and crisp integer aspect-ratio scaling

### 🔊 APU & Sound Synthesis

- **Complete 6-channel audio synthesis** — Tone 1 (pitch sweep), Tone 2 (square wave), Wave Memory (arbitrary 4-bit sample playback), Noise generator, and Direct Sound A/B FIFO DMA audio
- **Low-latency streaming** — cross-platform real-time audio playback powered by `cpal` with ringbuffer synchronization
- **Mixer control** — individual per-channel mute/solo toggles, stereo panning, and master volume attenuation

### 💾 Cartridge & Save Hardware

- **Intelligent backup detection** — auto-detects and supports SRAM (32KB), Flash (64KB/128KB), and EEPROM (512B/8KB) with manual override support
- **Real-Time Clock (RTC)** — cycle-accurate MBC3-style RTC emulation for day/night cycles (Pokémon Ruby/Sapphire/Emerald)
- **Specialized sensor peripherals** — solar sensor (*Boktai*), tilt sensor (*Yoshi's Universal Gravitation*), gyro sensor (*WarioWare: Twisted!*), and rumble feedback
- **IPS soft-patching** — automatically applies `.ips` ROM translation and enhancement patches on the fly without modifying base ROMs

### ⏱️ Rewind, States & Cheats

- **Fluid 60 FPS real-time rewind** — hold `R` at any point during gameplay to smoothly wind back time second-by-second
- **Instant state serialization** — lightning-fast, atomic save state compression powered by `postcard`
- **Cheat engine** — real-time Gameshark, Action Replay, and Codebreaker cheat code parsing and bus-level injection

### 🖥️ Modern Desktop Interface & Library Manager

- **DuckStation-style ROM library browser** — grid/list view with automatic ROM header parsing (Game Title, Game Code, Maker Code, Save Type, File Size) and search filter
- **Modern dark theme** — designed with 1px hairline borders, layered surface contrast, and tactile micro-interactions
- **In-game status bar & HUD** — live display of CPU mode (ARM/Thumb), PPU mode, scanline timing, save type, frame render time, and FPS counter
- **Integrated debugger suite** — CPU step/pause, ARM/Thumb live disassembler, memory hex viewer, IO register inspector, and VRAM tilemap explorer

---

## 🛠️ Tech Stack

| Package / Crate | Purpose |
|-----------------|---------|
| **Rust 2021 Edition** | High-performance systems programming language providing bare-metal speed and memory safety |
| **gba-core** | Decoupled, `no_std`-ready core emulation engine (CPU, PPU, APU, Bus, Cartridge, Timers, DMA) |
| **gba-debugger** | Disassembler for ARM and Thumb instruction sets, breakpoint engine, memory inspector |
| **gba-frontend-desktop** | Cross-platform native desktop application |
| **gba-frontend-web** | WebAssembly (WASM) frontend bindings for in-browser emulation |
| **gba-libretro** | Libretro core bridge for RetroArch and RetroPie ecosystems |
| **winit 0.30** | Cross-platform window creation and OS event loop handling |
| **glutin 0.32** | OpenGL context management and platform initialization |
| **glow** | Modern, lightweight OpenGL bindings for accelerated framebuffer presentation and GLSL shaders |
| **egui 0.31** | Immediate-mode GUI library powering the ROM library, menu bars, HUD, and debugger windows |
| **cpal** | Low-latency cross-platform native audio streaming |
| **gilrs** | Cross-platform gamepad and controller input handling with vibration support |
| **rfd** | Native system file dialogs for ROM loading and BIOS selection |
| **postcard & serde** | Ultra-compact, zero-overhead binary serialization for save states and config |

---

## 🚀 Getting Started

### Prerequisites

- [Rust SDK](https://rustup.rs/) (1.75 or later with `cargo`)
- A system supporting OpenGL 3.3+ (Windows, Linux, or macOS)
- GBA ROM files (`.gba`) — legally obtained game backups or homebrew

### Installation

```bash
# Clone the repository
git clone https://github.com/jpmasangkay/rustboy-advance.git
cd rustboy-advance

# Build the release binary
cargo build --release -p gba-frontend-desktop
```

### Running the App

```bash
# Launch the desktop UI with ROM library browser
cargo run --release -p gba-frontend-desktop

# Launch directly into a specific ROM
cargo run --release -p gba-frontend-desktop -- "path/to/game.gba"
```

### Headless & Testing Options

```bash
# Run headless for 120 frames and output framebuffer CRC32 checksum
cargo run --release -p gba-frontend-desktop -- "game.gba" --headless --frames 120 --fb-crc32

# Force low-level emulation with official BIOS dump
cargo run --release -p gba-frontend-desktop -- "game.gba" --bios "path/to/gba_bios.bin" --lle
```

---

## 🎮 Controls

### Default Keyboard Mapping

| GBA Button | Keyboard Key | Gamepad (XInput / DualShock) |
|------------|--------------|------------------------------|
| **D-Pad Up** | `Up Arrow` / `W` | D-Pad Up / Left Stick Up |
| **D-Pad Down** | `Down Arrow` / `S` | D-Pad Down / Left Stick Down |
| **D-Pad Left** | `Left Arrow` / `A` | D-Pad Left / Left Stick Left |
| **D-Pad Right** | `Right Arrow` / `D` | D-Pad Right / Left Stick Right |
| **Button A** | `Z` / `J` | Button A (Cross) |
| **Button B** | `X` / `K` | Button B (Circle) |
| **Left Shoulder (L)** | `Q` / `U` | Left Bumper (LB / L1) |
| **Right Shoulder (R)** | `E` / `I` | Right Bumper (RB / R1) |
| **Start** | `Enter` | Start / Options |
| **Select** | `Backspace` / `Shift` | Back / Share |

### Hotkeys & Shortcuts

| Action | Shortcut |
|--------|----------|
| **Rewind Gameplay** | Hold `R` |
| **Fast Forward (2x/4x)** | Hold `Space` / `Tab` |
| **Pause / Resume** | `P` |
| **Step Single Frame** | `F` |
| **Stop Emulation** | `Esc` / `Ctrl+W` |
| **Quick Save (Slot 1)** | `F5` |
| **Quick Load (Slot 1)** | `F9` |
| **Take Screenshot** | `F12` |
| **Toggle Fullscreen** | `F11` / `Alt+Enter` |

---

## 📁 Project Structure

```
rustboy-advance/
├── Cargo.toml                       # Workspace manifest configuration
├── Cargo.lock                       # Pinned dependency lockfile
├── .gitignore                       # Clean repository exclusions
├── README.md                        # Documentation & quickstart guide
└── crates/
    ├── gba-core/                    # Hardware emulation engine (no_std ready)
    │   ├── src/
    │   │   ├── lib.rs               # GbaCore top-level interface & scheduler
    │   │   ├── cpu.rs               # ARM7TDMI 3-stage pipeline & interpreter
    │   │   ├── bus.rs               # Memory map, fastmem, waitstates, open bus
    │   │   ├── ppu.rs               # Backgrounds, OBJs, scanline render, blending
    │   │   ├── apu.rs               # PSG Tone/Wave/Noise, DirectSound A/B FIFOs
    │   │   ├── cartridge.rs         # ROM parsing, SRAM/Flash/EEPROM, RTC, sensors
    │   │   ├── hw.rs                # Timers, DMA channels 0-3, IRQ controller, HLE BIOS
    │   │   └── serde_helpers.rs     # State serialization helpers
    │   └── tests/
    │       └── core_tests.rs        # 10 comprehensive hardware regression tests
    ├── gba-debugger/                # Disassembly and debugging tools
    │   ├── src/
    │   │   └── lib.rs               # ARM and Thumb instruction disassemblers
    │   └── tests/                   # Disassembly unit tests
    ├── gba-frontend-desktop/        # Native desktop GUI frontend (egui + glow)
    │   └── src/
    │       ├── main.rs              # App entry point & winit/glutin event loop
    │       ├── app_host.rs          # Emulation frame loop, ROM library, HUD & UI
    │       ├── debugger_ui.rs       # Disassembly, hex viewer, IO register panels
    │       ├── filters.rs           # CRT scanlines & LCD grid shader post-processing
    │       ├── cheats.rs            # Gameshark & Codebreaker cheat code parsing
    │       ├── audio.rs             # Ringbuffer & cpal output stream
    │       ├── config.rs            # Application configuration persistence
    │       └── headless.rs          # Headless benchmark & test runner
    ├── gba-frontend-web/            # WebAssembly bindings for browser emulation
    │   └── src/lib.rs
    └── gba-libretro/                # Libretro core integration
        └── src/lib.rs
```

---

## 🧠 How It Works

Rustboy Advance implements the GBA architecture through synchronized subsystems designed for accuracy and high throughput:

1. **Memory & Fastmem** — The memory bus translates addresses using a page table mapping 16KB memory chunks directly to raw host slices for EWRAM, IWRAM, and ROM, completely bypassing switch-case lookups on standard sequential reads. Open bus behavior and accurate waitstate timings (Waitstates 0, 1, 2) are faithfully reproduced.

2. **3-Stage Pipelined CPU** — The ARM7TDMI executes instructions through a realistic 3-stage pipeline (Fetch, Decode, Execute). Pipeline flushes, Thumb/ARM mode interworking (`BX`, `BLX`), and banked register preservation during interrupt service routines guarantee exact CPU state transitions.

3. **Scanline & Dot Compositor** — The PPU evaluates scanlines by calculating background priority layers (BG0–BG3), object priority layers (OBJ0–OBJ3), mosaic, and window containment before running the target-1 and target-2 alpha blending formula:
   $$\text{Color} = \min(31, \frac{A \cdot EVA + B \cdot EVB}{16})$$

4. **Circular State Rewind** — A rolling circular buffer captures complete snapshots every frame. When holding the rewind hotkey (`R`), the emulator steps backward through the buffer, rewinding both gameplay video and audio synchronously.

5. **DirectSound DMA & APU Synchronization** — Audio timer overflows trigger DMA channels 1 and 2 in DirectSound mode, feeding 32-sample circular FIFOs. The APU mixes PWM outputs alongside the 4 classic Game Boy channels into a single unified stream passed to `cpal`.

---

## 👥 Author

Built with ❤️ by **John Paul Masangkay** ([@jpmasangkay](https://github.com/jpmasangkay))

---

## 👏 Acknowledgements

- **Martin Korth (GBATEK)** — The definitive Game Boy Advance technical reference
- **Vicki Pfau & the mGBA project** — Invaluable documentation and test suites for GBA edge cases
- **The Rust Community** — For crates like `egui`, `winit`, `glow`, and `cpal` that make modern native desktop development a delight

---
