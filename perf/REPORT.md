# Rustboy Advance Performance, Weak-Hardware & Portability Report

**Workspace:** `crates/gba-core`, `crates/gba-frontend-desktop` (`rustboy-advance.exe`), `crates/gba-testrunner`  
**Final Binary:** `target\release\rustboy-advance.exe` (`build_time_utc: 2026-11-13T09:22:48Z`, `11,538,432` bytes, SHA256 `9f5f32383ca14b64ff8f22131249f0fba7aee56cb4dd3864b0ab81b8e022911a`, commit `fc25e98b72a0-dirty`, `0` compiler warnings)  
**Host Environment:** AMD Ryzen 9 8945HX (16C / 32T), `32,560 MB` RAM, Windows 11 (`10.0.26300`), display `2560 x 1600 @ 240 Hz` (`NVIDIA GeForce RTX 5070 Laptop GPU`), `rustc 1.99.1` (`x86_64-pc-windows-gnu`, `rust-lld`, LLVM `23.1.1`)  
**Release Profile (`Cargo.toml`):** `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`  
**Test ROMs (never committed):** `C:\Users\My Laptop\Downloads\Summon Night - Swordcraft Story (USA).gba` (read-only external ROM with deterministic `3,600f` replay `perf/replays/summon_night.txt` + `perf/replays/summon_night.sav`), `9` git-ignored stress ROMs + `13` `jsmolka/gba-tests` ROMs in `roms/homebrew/`, and embedded MIT/Apache `STRESS_SELF` (`stress_rom.rs`).

---

## 1. Quick Results (Phase 8 Final)

- **`8a` Fixed GPU `--selftest` Measurement (`2x -> 8x`, `0.081 ms` Fixed GPU Wait):**
  - Updated GPU `--selftest` to pace and scale **only** emulation + CPU filter/submit by `--slowdown N` (never GPU fence/`poll` wait) and use non-blocking `Maintain::Poll` during paced work timing, measuring isolated blocking `Maintain::Wait` separately (`avg = 0.081 ms`, `p95 = 0.098 ms`, `p99.5 = 0.112 ms`).
  - Auto-GPU `--selftest` (`RTX 5070 Vulkan`, `embedded:STRESS_SELF`) now measures `p99.5 = 1.764 ms` at `1x` and **`8x`** smooth & zero-over-budget ceiling (`12x` = `5.00%` FAIL at `p99.5 = 20.45 ms`), matching the headroom gap between `STRESS_SELF` (`0.888 ms` emu) and lighter *Summon Night* (`0.453 ms` emu, `13x` full-replay ceiling).
- **`8b` Real Desktop Window Vsync Runs (`4 x 5 min`) & `59.73 Hz` Refresh Fix:**
  - Pre-fix `eframe`/Vulkan (`perf/smoke-vulkan.csv.summary.json`) saturated the `240 Hz` swapchain into `[3v, 4v, 5v]` hold beating (`75.03%` repeated UI frames, **`66.67%` GBA cadence jitter `> 2 ms`**, `p99 = 21.599 ms`) due to unconditional `request_repaint()` + `egui`'s `predicted_dt` subtraction clamping `<= 16.67 ms` delays to `0`.
  - Fixed with a clamped monotonic `pacing_deadline`, `predicted_dt`-compensated `request_repaint_after`, and default `--sync-60hz` (`60.000 Hz`, absorbed by `+0.456%` audio DRC within `±1.0%`), reducing `300s` steady jitter `> 2 ms` to **`0.000%–0.090%`**, `> 2x` frames to **`0.000%`**, and audio underruns to **`0`** across all 4 modes (Vulkan & `--no-gpu`, Windowed & Fullscreen).
- **`8c` DX12 WARP Scaling Root Cause (`34 ms` vs `0.19 ms` No-GPU) & Auto-Selection:**
  - Confirmed DX12 WARP (`Microsoft Basic Render Driver`) already uses CPU filtering + `PlainTexturePresenter` (`fs_plain`); `--profile-sw` isolated its `4.46 -> 16.21 -> 34.06 ms` scaling to **`d3d10warp.dll` `write_texture` upload bandwidth** (`~24 µs/KB`: `153 KB` at `240x160` = `3.87 ms` vs `1.38 MB` at `720x480` = `33.12 ms`), whereas real `softbuffer` presents at **`0.185–0.190 ms`** (`0.149 ms` in tight XRGB8 blit loop).
  - Auto-selection now prefers the **No-GPU presenter (`winit + softbuffer`)** over any `DeviceType::Cpu` adapter unless startup micro-benchmarking proves that CPU adapter faster, logging both measured timings and the selection reason.
- **`8d` `softbuffer` Real-Window Scaling (`720x480`..`2560x1600`, `1x / 8x / 15x`):**
  - Optimized `no_gpu.rs` `blit_inner` (border-only clear, integer `copy_within` row reuse, separable cached horizontal scanlines). At **`1x`**, all sizes through `2560x1600` fit inside `16.743 ms` (`0.00%` over, worst `<= 5.12 ms`). At **`8x`**, `myboy-smooth` stops fitting above `720x480` (`1280x720` = `51.67%` over) and `none` stops fitting above `1280x720` (`1920x1080` = `29.17%` over). At **`15x`**, all sizes exceed `16.743 ms` (`720x480 none` = `17.50%` over).
- **`8e` & `8f` Simulated Zero-Adapter Fallback & Portable Binary Attribution:**
  - `--simulate-no-adapter` returns `0` adapters, falls back automatically to `winit + softbuffer`, passes `--selftest` (`8x` smooth / `6x` zero-over-budget), and runs a smooth live window (`8.2 s`, `0` repeats, `0` jitter, `0` underruns) while keeping `--no-gpu` intact.
  - Added `exe_sha256`, `git_commit`, and `build_time_utc` to all `--selftest` JSON files and text summaries.

> *Weak-Hardware & VM Scope Note:* Physical baseline-CPU (`Core 2` / non-AVX) Windows 10 VMs and bare-metal GPU-less VMs were not available on this host. Zero-adapter fallback was verified end-to-end via `--simulate-no-adapter` (both `--selftest` and live `8.2 s` window), `--no-gpu` (`--selftest`, `--bench-softbuffer`, two `300 s` desktop runs), and unit tests. All `--slowdown N` and software-adapter runs approximate weak hardware and do not replace testing on real low-end devices.

---

## 2. Final Verified Numbers (Phase 8 Build, `9f5f3238...`)

### 2.1 Verification & Accuracy Scorecard

| Check | Command | Final Observed Result | Status |
| :--- | :--- | :--- | :---: |
| **Release Build** | `cargo build --release --offline --workspace` | Finished in `1m 26s`, **`0` warnings, `0` errors** (`2026-11-13T09:22:48Z`, `commit fc25e98b72a0-dirty`, `11,538,432` B, `sha256: 9f5f3238...2911a`) | **PASS** |
| **Workspace Unit Tests** | `cargo test --offline --workspace` | **`163 passed, 0 failed, 0 ignored`** (`gb-core 12`, `gba-core 44`, `gba-debugger 8`, `gba-frontend-desktop 72`, `gba-frontend-web 6`, `gba-libretro 14`, `gba-testrunner 7`) | **PASS** |
| **Determinism Hashes** | Sequential `--headless --verify-hashes` across all `10` entries in `perf/verify_all_hashes.ps1` | **`10 / 10` PASS** (`core_vid`, `pres_vid`, `audio` CRC32 bit-identical across *Summon Night* `3,600f` + all `9` `600f` stress ROMs) | **PASS** |
| **`jsmolka/gba-tests`** | `cargo run --release --offline -p gba-testrunner -- --suite jsmolka` | **`13 passed, 0 failed, 0 skipped`** | **PASS** |
| **`--selftest` (`4` Modes)** | `rustboy-advance.exe --selftest` (`auto`, `--software-adapter`, `--no-gpu`, `--simulate-no-adapter`) | **`4 / 4` PASS** (`0` dropped work, `0` `>2x` spikes, `0` underruns) | **PASS** |
| **Real Window `300s` Runs** | `--perf-seconds 300 --perf-log ...` (Vulkan Windowed & Fullscreen, `--no-gpu` Windowed & Fullscreen) | **`4 / 4` PASS** (`0.000%` `>2x`, `0` underruns, `<= 0.090%` cadence jitter `>2 ms`) | **PASS** |
| **Simulated No-Adapter Window** | `--simulate-no-adapter --perf-seconds 8 --perf-log perf\sim-no-adapter.csv` | **`PASS`** (`8.2 s`, `0%` repeats, `0%` jitter, `0%` `>2x`, `0` underruns, audio active) | **PASS** |

### 2.2 Portable `--selftest` Across All 4 Modes (`8a`, `8e`, `8f`)

Measured on `embedded:STRESS_SELF (Mode0 4xBG + 128 Sprites + Alpha)` with `120` paced warmup frames discarded before collecting `600` paced frames (`Maintain::Poll` on GPU) and sweeping `N = 1, 2, 4, 6, 8, 12, 16, 24` (`300` frames/step):

| Mode & JSON Artifact | Chosen Adapter `[render_path]` | Adapters Seen | Calib (`Ratio`) | Headless `600f` (FPS / `Headroom` / `Emu`) | Warmup `120f` `p99.5` | Post-Warmup `600f` `Avg / p95 / p99.5 / Worst` (ms) | Isolated GPU Wait `Avg / p95 / p99.5` | Measured `Slowdown` Sweep (`--slowdown N`) | Smooth / Zero-Drop Ceiling | Rec. Filter (`Avg`) |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: | :---: | :--- | :---: | :--- |
| **Auto GPU** (`perf\selftest-20261113-094832.json`) | `RTX 5070 Laptop GPU` (`Vulkan / DiscreteGpu`) `[gpu-shader]` | `4` | `76.405 ms` (`0.99x`) | `926.1 FPS` / **`15.51x`** / `0.888 ms` (`1.080` tot) | `1.850 ms` | `1.137 / 1.526 / 1.764 / 1.934` | `0.081 / 0.098 / 0.112 ms` | `1x` `0%`, `2x` `0%`, `4x` `0%`, `6x` `0.33%`, `8x` `0.00%` (`13.82 ms`), `12x` `5.00%` (`20.45 ms`, **FAIL**) | **`8x` / `8x`** | `myboy-smooth` (`0.122 ms`) |
| **`--software-adapter`** (`perf\selftest-20261113-113329.json`) | `SwiftShader Device (Subzero)` (`Vulkan / Cpu`) `[software-cpu-plain]` | `4` | `78.217 ms` (`0.97x`) | `794.4 FPS` / **`13.30x`** / `0.844 ms` (`1.259` tot) | `1.741 ms` | `1.364 / 1.615 / 2.023 / 3.050` | `—` *(CPU+plain)* | `1x..8x` `0.00%` PASS (`8x` `15.28 ms`), `12x` `22.67%` / `22` ur (**FAIL**) | **`8x` / `8x`** | `lcd-grid` (`0.416 ms`) |
| **`--no-gpu`** (`perf\selftest-20261113-113423.json`) | `winit + softbuffer` (`NoGpu / CpuSoftware`) `[no-gpu-softbuffer]` | `4` | `79.764 ms` (`0.95x`) | `1031.3 FPS` / **`17.27x`** / `0.819 ms` (`0.970` tot) | `1.430 ms` | `1.066 / 1.366 / 1.540 / 2.038` | `—` *(softbuffer)* | `1x..8x` `0.00%` PASS (`8x` `12.14 ms`), `12x` `5.00%` / `0` ur (**FAIL**) | **`8x` / `8x`** | `myboy-smooth` (`0.160 ms`) |
| **`--simulate-no-adapter`** (`perf\selftest-20261113-113516.json`) | `winit + softbuffer` (`NoGpu / CpuFallback`) `[no-gpu-softbuffer]` | `0` *(sim.)* | `75.152 ms` (`1.01x`) | `1032.8 FPS` / **`17.29x`** / `0.818 ms` (`0.968` tot) | `1.697 ms` | `1.078 / 1.399 / 1.525 / 1.839` | `—` *(softbuffer)* | `1x..6x` `0.00%`, `8x` `0.33%` (`13.05 ms`, PASS), `12x` `5.33%` / `0` ur (**FAIL**) | **`8x` / `6x`** | `myboy-smooth` (`0.148 ms`) |

*Hidden-window `softbuffer` present micro-timings (`120f`):* `--no-gpu` = `none 0.155 ms` (`p95 0.167`), `myboy-smooth 0.169 ms` (`p95 0.272`); `--simulate-no-adapter` = `none 0.151 ms` (`p95 0.161`), `myboy-smooth 0.158 ms` (`p95 0.196`).  
**Auto-GPU Ceiling vs. *Summon Night* `~13x` Explanation (`8a`):** Fixing GPU `--selftest` to use non-blocking `Maintain::Poll` and exclude fixed GPU wait (`0.081 ms` avg) from `--slowdown N` moved Auto-GPU from `2x` (`6.273 ms` `p99.5`) to **`8x`** (`1.764 ms` `p99.5` at `1x`). The remaining gap to Phase 6a's `13x` *Summon Night* ceiling is purely the workload difference (`STRESS_SELF` spends `0.888 ms` emu / `~1.08–1.14 ms` total, vs *Summon Night*'s `0.453 ms` emu, and `--selftest` tests discrete steps `8x, 12x, ...`: at `12x`, `12 * ~1.1 ms + tails` reaches `p99.5 = 20.45 ms` and `5.00%` over budget).

### 2.3 Real Desktop Window Vsync Runs (`300s`), A/B Check & `59.73 Hz` Refresh Analysis (`8b`)

Measured with `--perf-log` on live `eframe`/Vulkan and `winit + softbuffer` desktop windows running *Summon Night* (`myboy-smooth`, looping `perf/replays/summon_night.txt`, vsync on, `2560 x 1600 @ 240 Hz` panel):

| Run / Condition (`--perf-log`) | Duration / Steady Frames (Presented / GBA) | Wall Interval `p50 / p95 / p99 / Max` (ms) | GBA Cadence `p50 / p95 / p99 / Max` (ms) | `> 16.74 ms` / `> 17.5 ms` | `> 2x` (`33.49 ms`) | Cadence Jitter `> 2 ms` | Frame Repeats (`vsync_holds`) | Audio Underruns (Steady) | Emu / Present `Avg (p95)` (ms) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| **Pre-Fix Vulkan Windowed** (`perf\smoke-vulkan.csv`) **`[SUPERSEDED]`** | `6.0 s` (`961 / 240`) | `3.433 / 9.219 / 9.447 / 10.368` | `16.631 / 21.119 / 21.599 / 22.683` | `46.25%` / `33.33%` | `0.00%` | **`66.67%`** | **`75.03%`** (`[3v:80, 4v:80, 5v:80]`) | `0` | `0.638 (0.835)` / `0.022 (0.077)` |
| **Vulkan Windowed `300s`** (`perf\win-vulkan-windowed.csv`) | `300.0 s` (`17,881 / 17,881`) | `16.669 / 17.316 / 17.623 / 18.417` | `16.669 / 17.316 / 17.623 / 18.417` | `42.53%` / `2.00%` | `0.000%` | **`0.006%`** | **`0.00%`** (`[1v:17,881]`) | **`0`** | `1.068 (1.595)` / `0.080 (0.095)` |
| **Vulkan Fullscreen `300s`** (`perf\win-vulkan-fullscreen.csv`) | `300.0 s` (`17,907 / 17,881`) | `16.674 / 17.298 / 17.584 / 18.370` | `16.675 / 17.298 / 17.584 / 18.370` | `42.81%` / `1.74%` | `0.000%` | **`0.000%`** | **`0.15%`** (`[1v:17,872, 3v:1, 4v:8]`) | **`0`** | `1.064 (1.568)` / `0.081 (0.094)` |
| **`--no-gpu` Windowed `300s`** (`perf\win-nogpu-windowed.csv`) | `300.2 s` (`17,845 / 17,845`) | `16.710 / 17.129 / 17.462 / 21.857` | `16.710 / 17.129 / 17.462 / 21.857` | `40.49%` / `0.85%` | `0.000%` | **`0.090%`** | **`0.00%`** (`[1v:17,845]`) | **`0`** | `0.605 (0.861)` / `2.602 (2.966)` |
| **`--no-gpu` Fullscreen `300s`** (`perf\win-nogpu-fullscreen.csv`) | `300.2 s` (`17,846 / 17,846`) | `16.696 / 17.104 / 17.385 / 18.989` | `16.696 / 17.104 / 17.385 / 18.989` | `40.17%` / `0.55%` | `0.000%` | **`0.017%`** | **`0.00%`** (`[1v:17,846]`) | **`0`** | `0.597 (0.851)` / `1.907 (2.391)` |
| **A/B Vulkan `--no-sync-60hz`** (`perf\ab-vulkan-nosync60.csv`) | `60.0 s` (`3,466 / 3,466`) | `16.763 / 17.402 / 17.660 / 18.263` | `16.763 / 17.402 / 17.660 / 18.263` | `51.79%` / `2.71%` | `0.000%` | `0.000%` | `0.00%` (`[1v:3,466]`) | `0` | `1.080 (1.727)` / `0.079 (0.094)` |
| **A/B `--no-gpu --no-sync-60hz`** (`perf\ab-nogpu-nosync60.csv`) | `60.2 s` (`3,476 / 3,476`) | `16.769 / 17.433 / 17.733 / 18.250` | `16.769 / 17.433 / 17.733 / 18.250` | `52.36%` / `3.57%` | `0.000%` | `0.000%` | `0.00%` (`[1v:3,476]`) | `0` | `0.684 (1.165)` / `2.525 (2.830)` |

- **Why Pre-Fix Vulkan Stuttered & How It Was Fixed:** Calling `ctx.request_repaint()` unconditionally on Vulkan saturated the 3-image FIFO swapchain at `240 Hz` (`avg = 4.167 ms`) while `emu_thread` pushed new frames every `~16.67 ms`, producing alternating `[3v, 4v, 5v]` hold patterns (`[12.5, 16.7, 20.8] ms`, **`66.67%` jitter `> 2 ms`**, visible judder). Calling plain `ctx.request_repaint_after(delay)` also failed because `egui-0.31.1` subtracts `predicted_dt` (`~16.67 ms`), clamping GBA-frame delays to `Duration::ZERO`. Replacing this with a clamped monotonic `pacing_deadline`, `predicted_dt`-compensated `ctx.request_repaint_after(delay + pred)`, and unified default `--sync-60hz` (`60.000 Hz`) eliminated the beating (`0.000%–0.006%` jitter on Vulkan).
- **Matching `59.7275 Hz` to `60 Hz` / `240 Hz` Displays (`40%–43% > 16.74 ms` Explained):** On a `240 Hz` panel (`4.167 ms` vblank quantum), a native `59.7275 Hz` (`16.7427 ms`) loop lands cleanly on `4` vblanks (`16.67 ms`) except around slow phase slips; on a standard `60.000 Hz` (`16.667 ms`) display, running un-synced at `59.7275 Hz` slips one vblank beat (`1 / (60 - 59.7275) = 3.67 s`, `~220` frames) causing a periodic duplicate frame. Enabling **`--sync-60hz` by default** on both Vulkan and `--no-gpu` targets exact `60.000 Hz` (`16.667 ms`, audio rate smoothly shifted **`+0.456%`** within the existing `±1.0%` DRC clamp -> **`0` underruns**). Because `16.667 ms` sits only `0.076 ms` below the raw `16.743 ms` GBA constant, ordinary sub-millisecond OS/vblank jitter places `~40%–43%` in `[16.743, 17.5) ms` while true over-cadence frames (`> 17.5 ms = 0.55%–2.00%`, `> 2 ms` jitter `<= 0.09%`, `> 2x = 0.000%`) remain negligible. `--no-sync-60hz` remains available to preserve native `59.7275 Hz` (`p50 = 16.763–16.769 ms`).

### 2.4 Multi-Backend Filter Benchmark & DX12 WARP Root-Cause Diagnosis (`8c`)

**Display Filters (`perf\filters-20261113-094655.json`, *Summon Night* `600f` post-`1200f`-warmup):**

| Filter | `normal-gpu` RTX 5070 Vulkan (`Avg / Added / p99 ms`) | `software-vulkan` SwiftShader CPU+Plain (`Avg / Added / p99 ms`) | `dx12-warp` MS Basic Render CPU+Plain (`Avg / Added / p99 ms`) | `cpu-software` RGBA PostProc (`Avg / p99 ms`) | `no-gpu-xrgb8` CPU + XRGB8 (`Avg / p99 ms`) | `no-gpu-softbuffer` Real Present (`Avg / p95 ms`) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| `none` | `0.1169 / +0.0000 / 0.2942` | `0.6937 / +0.0000 / 1.1136` | `4.4609 / +0.0000 / 5.2816` | `0.0867 / 0.2819` | `0.1493 / 0.1591` | **`0.1900 / 0.2588`** |
| **`myboy-smooth`** *(default)* | **`0.1166 / +0.0000 / 0.2550`** | `0.8644 / +0.1708 / 1.2276` | `4.6784 / +0.2175 / 5.9248` | **`0.0915 / 0.3044`** | **`0.1493 / 0.1578`** | **`0.1848 / 0.2496`** |
| `color-correction` | `0.1159 / +0.0000 / 0.2898` | `0.6500 / +0.0000 / 0.9219` | `4.4684 / +0.0075 / 5.3573` | `0.0919 / 0.2747` | `0.1488 / 0.1598` | — |
| `bilinear` | `0.1178 / +0.0009 / 0.2962` | `1.0328 / +0.3391 / 1.4607` | `16.2108 / +11.7499 / 21.3656` | `0.1446 / 0.3607` | `0.7043 / 0.7193` | — |
| `lcd-grid` | `0.1167 / +0.0000 / 0.2917` | **`0.4187 / +0.0000 / 0.7729`** | `34.0560 / +29.5951 / 39.3177` | `0.3130 / 0.6670` | `0.4147 / 0.4292` | — |
| `crt-scanlines` | `0.1456 / +0.0287 / 0.3179` | `0.8327 / +0.1391 / 1.2392` | `15.5583 / +11.0974 / 18.3685` | `0.1304 / 0.1411` | `0.2600 / 0.2672` | — |
| `hq2x` | `0.1316 / +0.0147 / 0.3198` | `1.2897 / +0.5960 / 1.8634` | `16.4602 / +11.9993 / 19.3673` | `0.3526 / 0.3815` | `0.4827 / 0.5155` | — |
| `xbrz` | `0.1277 / +0.0108 / 0.2885` | `0.9073 / +0.2136 / 1.6387` | `35.6045 / +31.1436 / 41.7809` | `0.6696 / 0.7080` | `0.8150 / 1.0950` | — |

**WARP `--profile-sw` Diagnosis (`60f`):**
1. **Verified Path:** Both `SwiftShader Device (Subzero)` and `Microsoft Basic Render Driver` already take the **CPU filter + `PlainTexturePresenter`** path (`[CPU filter + plain texture]`), NOT WGSL shader effects (`[D] Empty = 0.0290 ms`, `[L] fs_plain Clear = 0.4342 ms`, `[E] fs_plain Load = 0.3851 ms` on DX12 WARP).
2. **Why DX12 WARP Scaled `4.46 -> 16.21 -> 34.06 ms`:** Output size changes CPU `VideoPostProcessor` dimensions (`240x160` `153 KB` for `none`/`myboy-smooth` vs `480x320` `614 KB` for `bilinear`/`crt`/`hq2x` vs `720x480` `1.38 MB` for `lcd-grid`/`xbrz`). On WARP (`d3d10warp.dll`), **`queue.write_texture` upload scales linearly at `~24 µs/KB`**:
   - `[G] write_tex 240x160 + Wait = 3.8703 ms` (`[H] write 240x160 + fs_plain = 4.3829 ms`), vs `[F] write_tex 720x480 + Wait = 33.1233 ms` (`[J] copy_tex 720x480 = 33.6602 ms`). (Even `[B] Clear+fs_near = 0.7017 ms` is tiny once uploaded, whereas `SwiftShader` uploads `720x480` in `0.1223 ms`!)
3. **Selection Policy:** Auto-selection therefore **prefers No-GPU (`winit + softbuffer`, `0.185 ms`) over any `DeviceType::Cpu` adapter** unless a startup micro-benchmark proves that CPU adapter faster (`softbuffer` is `~25x–184x` faster than DX12 WARP and still `~3.8x–4.7x` faster than SwiftShader for `myboy-smooth`).

### 2.5 `softbuffer` Real-Window Size Scaling (`perf\softbuffer-20261113-094558.json`, `8d`)

Measured over `120` *Summon Night* frames per condition on a real `winit + softbuffer` window (`budget = 16.743 ms`):

| Resolution (`filter`) | `1x` Present (`avg`) | `1x` Total `Avg / p95 / Worst` | `1x` Over (`Fit?`) | `8x` Present (`avg`) | `8x` Total `Avg / p95 / Worst` | `8x` Over (`Fit?`) | `15x` Total `Avg / p95 / Worst` | `15x` Over (`Fit?`) |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |
| `720x480` (`none`) | `0.157 ms` | `1.026 / 1.173 / 1.413` | `0.00%` (**FIT**) | `1.187 ms` | `8.314 / 9.061 / 9.553` | `0.00%` (**FIT**) | `15.332 / 17.103 / 24.114` | `17.50%` (EXCEEDS) |
| `720x480` (`myboy-smooth`) | `0.167 ms` | `1.095 / 1.581 / 1.619` | `0.00%` (**FIT**) | `1.400 ms` | `9.361 / 12.807 / 13.854` | `0.00%` (**FIT**) | `18.000 / 25.283 / 31.767` | `44.17%` (EXCEEDS) |
| `1280x720` (`none`) | `0.237 ms` | `1.168 / 1.657 / 1.739` | `0.00%` (**FIT**) | `2.016 ms` | `9.757 / 13.511 / 14.660` | `0.00%` (**FIT**) | `20.632 / 30.648 / 41.444` | `61.67%` (EXCEEDS) |
| `1280x720` (`myboy-smooth`) | `1.043 ms` | `2.050 / 2.876 / 3.219` | `0.00%` (**FIT**) | `9.000 ms` | `17.808 / 23.382 / 25.185` | `51.67%` (EXCEEDS) | `33.100 / 44.103 / 49.125` | `100.00%` (EXCEEDS) |
| `1920x1080` (`none`) | `0.476 ms` | `1.487 / 2.052 / 3.143` | `0.00%` (**FIT**) | `6.479 ms` | `15.005 / 23.432 / 38.308` | `29.17%` (EXCEEDS) | `26.220 / 38.022 / 50.837` | `99.17%` (EXCEEDS) |
| `1920x1080` (`myboy-smooth`) | `1.881 ms` | `2.812 / 3.573 / 3.672` | `0.00%` (**FIT**) | `18.521 ms` | `27.551 / 34.976 / 40.770` | `100.00%` (EXCEEDS) | `52.521 / 67.323 / 74.487` | `100.00%` (EXCEEDS) |
| `2560x1600` (`none`) | `1.379 ms` | `2.594 / 4.331 / 5.101` | `0.00%` (**FIT**) | `10.494 ms` | `18.710 / 29.839 / 42.514` | `52.50%` (EXCEEDS) | `43.383 / 55.844 / 74.408` | `100.00%` (EXCEEDS) |
| `2560x1600` (`myboy-smooth`) | `1.262 ms` | `2.382 / 3.513 / 5.118` | `0.00%` (**FIT**) | `11.869 ms` | `20.780 / 30.425 / 37.530` | `75.00%` (EXCEEDS) | `41.205 / 61.068 / 70.914` | `100.00%` (EXCEEDS) |

*(Note: at `2560x1600`, `2400x1600` is an exact `10x` integer multiple of `240x160`, keeping `myboy-smooth` slightly faster at `1x` than unaligned `1920x1080`.)*  
**Budget Cutoff (`<= 16.743 ms`):** At **`1x`**, every tested resolution (`720x480`..`2560x1600`) fits with `0.00%` over-budget frames. At **`8x`**, `myboy-smooth` stops fitting above **`720x480`** (fails at `1280x720`, `51.67%` over) and `none` stops fitting above **`1280x720`** (fails at `1920x1080`, `29.17%` over). At **`15x`**, `softbuffer` exceeds `16.743 ms` at all tested sizes (`720x480` `none` = `17.50%` over).

### 2.6 Simulated Zero-Adapter Window Fallback (`8e`)

Running `--simulate-no-adapter --perf-seconds 8 --perf-log perf\sim-no-adapter.csv` logged automatic fallback along `[Vulkan -> DX12 -> GL -> No-GPU presenter (winit + softbuffer)]` (`NoGpu / CpuFallback`), opened a live window with active audio (`0` underruns), and achieved `8.2 s, 445 presented (370 steady) | Wall & GBA: p50 = 16.706, p95 = 17.038, p99 = 17.238, max = 17.788 ms | 0.000% >2x | 0.000% jitter >2ms | 0/370 (0.00%) repeats | emu avg 0.300 (p95 0.537) ms | present avg 2.554 (p95 2.897) ms`.

---

## 3. Methodology, Architecture & Portability

### 3.1 Adapter Discovery, Fallback Order & No-GPU Presenter (`shaders.rs`, `no_gpu.rs`, `app_host.rs`, `main.rs`, `8c`, `8e`)
- **Enforced Fallback Chain (`shaders::ADAPTER_FALLBACK_ORDER`):** **`Vulkan -> DX12 -> GL -> No-GPU presenter (winit + softbuffer)`**.
  - Normal `select_preferred_adapter` scores `Vulkan > Dx12 > Gl`, prefers hardware GPUs (`DiscreteGpu > IntegratedGpu > VirtualGpu > Other > Cpu`). If only a `DeviceType::Cpu` adapter exists, startup benchmarks it against `NoGpuCpuPresenter` (`winit + softbuffer`) and automatically picks No-GPU unless the `Cpu` adapter measures faster (`log` prints both timings and the reason).
  - `--software-adapter` (`discover_software_cpu_adapter`) selects CPU adapters explicitly in `Vulkan -> DX12 -> GL` order (preferring `SwiftShader Device (Subzero)` when installed, or `Microsoft Basic Render Driver` DX12 WARP on Windows 10+). Both `Cpu` adapters run filters on CPU (`VideoPostProcessor`) and present via `PlainTexturePresenter`.
  - If `--no-gpu` is passed, `--simulate-no-adapter` is set, OR `0` hardware/faster adapters initialize, `run_no_gpu_desktop` launches a pure `winit 0.30 + softbuffer 0.4` XRGB8 window using `NoGpuCpuPresenter` (full input, drag-and-drop/`Ctrl+O`, `.sav`, CPAL audio, `--sync-60hz` DRC, `QualityGovernor`, and `--perf-log`).

### 3.2 Quality Governor Recovery & Persistence (`7c`, `filters.rs`, `config.rs`, `app.rs`, `no_gpu.rs`)
- Startup `prewarm()` micro-times all 7 distinct filter costs on the active backend (`VideoPostProcessor`, `BenchWgpuContext`, or `NoGpuCpuPresenter`) and sorts ascending.
- **Step-Down:** Rolling `120f` window (`min 60f`); if `p95 > 70%` (`11.72 ms`) persists for `120` frames (`saturating_sub(1)` hysteresis on quiet frames), jumps straight to the costliest strictly cheaper filter with `est_base + cost <= 50%` (`8.37 ms`), sets `cooldown = 180f`, and starts a **`18,000f` (~5-minute) upward lockout**.
- **Conservative Step-Up Recovery:** Once `up_lockout_remaining == 0` and `active` is below `user_target`, if rolling `p95 < 35%` (`< 5.86 ms`) holds continuously for **`3,600f` (~60s)** AND `est_base + next_costlier_cost < 50%` (`< 8.37 ms`), steps up **one** tier toward `user_target`.
- **Anti-Flap Backoff:** If overload (`p95 > 70%`) returns within **`1,800f` (~30s)** of a step-up, `QualityGovernor` steps back down immediately (without waiting another `120f`) and **doubles `current_lockout_frames`** (`5m -> 10m -> 20m -> ...`). Changing `user_target` manually resets backoff.
- **Persistence:** `AppConfig::governor_settled_filters` stores settled filters keyed by `governor_profile_key(adapter, resolution_bucket)` across sessions (`720p`, `1080p`, `1440p`, `4k+`).
- **Unit Coverage (`filters::tests` 8/8, `config::tests`):** `quality_governor_jumps_directly_under_50_pct_budget_with_hysteresis_and_override` (transient hiccup + sustained overload + lockout + override), `quality_governor_respects_backend_order_and_never_picks_costlier_filter`, `quality_governor_recovery_and_doubled_lockout_on_flapping`, and `governor_settled_filter_persists_per_adapter_and_resolution`.

### 3.3 Portability Verification (`7d`)
- **Minimum Supported Windows OS:** Official `rustc 1.99.1` `x86_64-pc-windows-gnu` tier guarantees require **Windows 10** (or Windows Server 2016+) or later.
- **Baseline `x86-64` Codegen:** `.cargo/config.toml` sets only `rustflags = ["-C", "linker=rust-lld"]`. `rustc --print cfg --target x86_64-pc-windows-gnu` enables strictly baseline features (`cmpxchg16b`, `fxsr`, `sse`, `sse2`, `sse3`) with zero `target-cpu=native`, AVX, or FMA instructions.
- **Static DLL Imports (`objdump -p target\release\rustboy-advance.exe`):** Imports **only stock Windows OS DLLs** (`api-ms-win-core-synch-l1-2-0.dll`, `bcryptprimitives.dll`, `shlwapi.dll`, `gdi32.dll`, `oleaut32.dll`, `shell32.dll`, `user32.dll`, `d3dcompiler_47.dll`, `dwmapi.dll`, `imm32.dll`, `ole32.dll`, `opengl32.dll`, `uxtheme.dll`, `ws2_32.dll`, `ADVAPI32.dll`, `bcrypt.dll`, `KERNEL32.dll`, `msvcrt.dll`, `ntdll.dll`, `WINMM.dll`). Zero VC++ redistributable (`VCRUNTIME*.dll`), zero external MinGW runtime DLLs (`libgcc`/`libstdc++`/`winpthread`), and zero static Vulkan/DX12 loader dependencies (`vulkan-1.dll`, `d3d12.dll`, `dxgi.dll`, `avrt.dll` are loaded dynamically at runtime with `--no-gpu` fallback if absent).

---

## 4. History Appendix (Phases 1–6)

> Numbers in this appendix were captured on earlier pre-Phase-7 builds (`fc25e98` through Phase 6e) and are retained chronologically for traceability. Tables/rows superseded by Phase 6b/7 architectural fixes are explicitly marked **`[SUPERSEDED]`**.

### 4.1 Phase 1 — Baseline Measurement & Diagnostic Fixes (`Fix A / Fix B`)
- **`Fix A` (`[SUPERSEDED]` nested timer bug -> clean subsystem attribution):** Initial Phase 1 (`perf/bench-20261112-174845.json`, **`[SUPERSEDED]`**) misattributed CPU at `0.0005 ms/frame` (`0.0568%`) due to overlapping `Bus`/`PPU`/`APU`/`DMA` timers. Fixing timer nesting normalized Phase 1 *Summon Night* shares cleanly to `100.0%`: `CPU 21.09%`, `PPU 42.95%`, `Bus 14.94%`, `DMA 5.85%`, `APU 3.20%`, `Present 11.97%`.
- **`Fix B` (`prewarm()`, `worst` vs `p99`, & presentation intervals):** Prewarming pipelines before frame 0 removed an `18.923 ms` frame-0 cold-start shader spike (`bench` worst `19.8168 -> 11.4455 ms`), and separated `worst` (`2.1878 ms`) from `p99` (`1.6371 ms`) while tracking consecutive presentation intervals and `>2 ms` cadence jitter.

| Stage (`Summon Night` `3,600f`, `myboy-smooth`) | Avg Frame (ms) | Avg FPS | Emu Only (ms) | Worst (ms) | `1x` Realtime Underruns / `>2x` / Jitter |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Phase 1 Initial (`174845`) **`[SUPERSEDED]`** | `0.9285` | `1,077.0` | `0.8170` | `19.8168` | `0` / `0` / *(frame 0 drop `18.955 ms`)* |
| Post-Fix A/B (`184113`) **`[SUPERSEDED]`** | `0.8846` | `1,130.4` | `0.7787` | `11.4455` | `0` / `0` / `4` |
| Pre-4b Baseline (`phase4b_base`) **`[SUPERSEDED]`** | `0.9546` | `1,047.6` | `0.8478` | `—` | `—` |
| **Post-Phase-4 Headless (`phase4_final_bench`)** | **`0.5484`** | **`1,823.5`** | **`0.4534`** | **`1.3057`** | **`0` / `0` / `0`** (`pres_int p99 = 17.087, max = 17.666`) |

### 4.2 Phase 2–3C — Powercfg Calibration, Core 0 Contention & Stress Suite
- **`powercfg` Proof (`[SUPERSEDED]` failed throttle -> `--slowdown N`):** Even with `PROCTHROTTLEMIN`, `PROCTHROTTLEMAX`, and `PERFBOOSTMODE=0`, capping max state to `25%`, `15%`, or `5%` left single-thread calibration at `163–165 ms` (`99.0%` perf) on Ryzen ACPI CPPC and was marked `NOT APPLIED`. Real `--slowdown N` busy-spin replaced it, and the host power plan was restored.
- **Core 0 Contention (`5 x 720f` *Summon Night*, Core 0 Affinity, Phase 5e `MMCSS "Games"` + `ABOVE_NORMAL`):**
  - Core 0 + `1` or `2` competing PowerShell busy loops improved from **`0/5` FAIL** (`97–361` underruns) to **`5/5` PASS (`0` underruns, `0` `>2x` spikes)**.
  - Core 0 + `4` busy workers achieved **`5/5` `0` underruns**, **`4/5` `0` `>2x` spikes**, and 1 MMCSS `20%` SystemResponsiveness wakeup outlier (`perf/realtime-20261113-055325.json`, `14` `>2x` intervals, worst `48.63 ms`, `0` underruns).
- **Phase 3C `9` Stress ROMs (`600f` each, `myboy-smooth`, Pre- vs Post-Phase 4 headless):** `stress_mode0_4bg_128spr` (`0.8525 -> 0.6627 ms`, `1,509.1 FPS`, `25.27x`), `m7_ex` (`0.8384 -> 0.6299 ms`), `mos_demo` (`0.7809 -> 0.5457 ms`), `bld_demo` (`0.8615 -> 0.5391 ms`), `stress_aff_objwin` (`0.6994 -> 0.5294 ms`), `stress_bitmap_345` (`0.5351 -> 0.3994 ms`), `win_demo` (`0.4906 -> 0.2829 ms`), `oacombo` (`0.3899 -> 0.2174 ms`), `bm_modes` (`0.2665 -> 0.2027 ms`); all `9/9` passed `1x` realtime with `0` underruns and `0` spikes.

### 4.3 Phase 4 Optimization Progression & 10-Minute (`36,000f`) Soaks
- **Phase 4a (Audio & Pacing):** `VideoPostProcessor::prewarm`, `DrcPacer` (`timeBeginPeriod(1)` + waitable timer), dedicated `"gba-audio"` MMCSS `"Pro Audio"` thread, `32,768`-sample adaptive ring (`4,096 -> 16,384` target fill) + `±1.0%` sinc DRC, and `>2x`-only present-skip lifted the `--slowdown` `2x`-spike gate from `1x` to **`10x`**.
- **Phase 4b (PPU, Bus & Core):** Layer/window skip + `compose_span` (`4b.1`), sprite X-clipping (`4b.2`), regular BG nibble/span (`4b.3`), affine wraparound (`4b.4`), packed `bg_lines` (`4b.5a`), `ObjPixel` `6B -> 4B` (`4b.5c`), `run_frame_fast` + `fastmem` + `waitstate_table` + `code_page_ptr` (`4b.6`), and DRC drift/skip tuning (`4b.7–4b.9`) lifted the `2x`-spike/`0`-underrun gate across all 4 swept ROMs (`stress_mode0`, `m7_ex`, `bld_demo`, `summon_night` `5x720f` + full `3,600f` *Summon Night*) to **`15x`** (with `20x` failing on audio ring drain). Four regressive experiments (`4b.5b`, two bus experiments, `4b.8`, `4b.8b`) were measured and reverted.
- **Three Uninterrupted 10-Minute Soaks (`36,000f` each, `cpu-software` `myboy-smooth`):**
  - *Summon Night* `1x` (`perf/realtime-20261113-060122.json`, `603.38 s`): `0` drops, `0` `>2x`, `0` underruns, `0` jitter (`work avg = 0.728 / p99 = 1.203 / worst = 2.782 ms`, `pres_int max = 17.810 ms`).
  - *Summon Night* `10x` (`perf/realtime-20261113-061138.json`, `603.07 s`): `0` drops, `0` `>2x`, `0` underruns, `0` jitter (`work avg = 7.168 / p99 = 11.911 / worst = 16.587 ms`).
  - `stress_mode0_4bg_128spr` `10x` (`perf/realtime-20261113-062158.json`, `603.05 s`): `0` drops, `0` `>2x`, `0` underruns, `0` jitter (`work avg = 7.650 / p99 = 12.135 / worst = 16.487 ms`).

### 4.4 Phase 5–6 Historical Notes (`[SUPERSEDED]`)
- **Test Count Audit (`145 -> 151 -> 157 -> 160 -> 162 -> 163`):** Zero tests were ever removed or weakened (`"171"` mentioned earlier in chat was an addition slip for `151`).
- **Phase 5b Fast-Path Accuracy Fixes (Permanent in Core):** Fixed IWRAM/EWRAM mirrored code-page invalidation (`bus.rs`/`hw.rs`), EEPROM fastmem page exclusion & `>16 MiB` auto-detect (`bus.rs`/`cartridge.rs`/`lib.rs`), and `load_state` cache coherence (`lib.rs`), adding `+4` targeted unit tests in `core_tests.rs`.
- **Phase 5 (`[SUPERSEDED]`) `SwiftShader` Fragment Shaders vs. Phase 6b Fix:** In Phase 5 (`perf/filters-20261113-053152.json`, **`[SUPERSEDED]`**), `SwiftShader` executed WGSL fragment shaders on CPU (`none = 3.0337 ms`, `myboy-smooth = 14.0157 ms`, `lcd-grid = 14.8609 ms`, `xbrz = 46.7109 ms`, `crt-scanlines = 73.1722 ms`), failing `6/8` filters at `1x`. **Phase 6b** moved `DeviceType::Cpu` filtering onto CPU `VideoPostProcessor` + `PlainTexturePresenter` (`fs_plain` + `LoadOp::Load`), dropping Phase 6b `SwiftShader` `lcd-grid` to `0.4111 ms` and `none` to `0.5235 ms` and passing all `8/8` filters with `0.000%` over-budget at `1x` (`perf/phase6b_sw_filters.csv`; see Section `2.4` for current Phase 8 filter numbers).
- **Phase 5f (`[SUPERSEDED]` `1,200f` prefix) vs. Phase 6a Full-Scene Smooth Ceilings (`cpu-software`, `myboy-smooth`):** Phase 5f's `14x` *Summon Night* ceiling only covered frames `0..1200` (`boot`–`menus`, **`[SUPERSEDED]`**). Phase 6a (`perf/phase6a_exact_bounds.csv`, `perf/phase6a_battle_stress.csv`) measured the full `3,600f` replay (**`13x`** `<0.5%` / `8x` zero-drop), isolated `1,000f` `battle` (**`14x`** `<0.5%` / `12x` zero-drop), and `1,200f` `stress_mode0_4bg_128spr` (**`13x`** `<0.5%` / `10x` zero-drop).
- **Phase 6c Measured-Cost Governor First-Settle Runs (`perf/phase6c/`, `900f` each):** `xbrz -> myboy-smooth` in **`3.071 s`** (`0`/`0` underruns) on CPU `15x` boot; `xbrz -> none` in **`3.264 s`** (`65` settle / `1` post-settle underruns) on CPU `15x` battle; `hq2x -> lcd-grid` in **`3.155 s`** (`13`/`0` underruns) on SwiftShader `10x` boot; `xbrz -> lcd-grid` in **`3.061 s`** (`5`/`0` underruns) on SwiftShader `10x` battle. Phase 7c extended this with `5m` step-up lockout, `60s < 35%` single-tier recovery, doubled flapping lockout, and per-`(adapter, resolution)` persistence.
- **Phase 6d–6e (`[SUPERSEDED]` Offline Blockers & Estimated Ceilings):** Phase 6d (`[SUPERSEDED]`) noted DX12 was blocked offline by uncached `gpu-allocator`/`range-alloc`; Phase 7b fetched both and enabled DX12 WARP. Phase 6e (`[SUPERSEDED]`) estimated `--selftest` ceiling via `16.74 / p99.5` (`~2x` GPU / `~8x` SwiftShader) without discarding `120f` warmup; Phase 7a replaced this with `120f` warmup discard and measured `N = 1..24` sweeps.

### 4.5 Phase 7 Historical Baseline (`[SUPERSEDED]` by Phase 8)
- **Phase 7 Build (`E41FC2E8F...`, `UTC 2026-11-13T08:17:27Z`, `[SUPERSEDED]`):** Passed `162` workspace unit tests, `10/10` determinism hashes, `13/13` `jsmolka/gba-tests`, full `gba-testrunner --allow-failures` (`43 passed, 9 failed` pre-existing `nba-hw-test` timing/raster tests, `2` skipped missing ROMs, `2` skipped missing golden), and verified Windows power plan restored to `Balanced`.
- **Phase 7 GPU `--selftest` (`perf/selftest-20261113-081812.json`, `[SUPERSEDED]` by `8a`):** Used blocking `device.poll(Maintain::Wait)` inside the timed/scaled paced loop, measuring `poll_wait p99.5 = 5.335 ms` (`p99.5 total = 6.273 ms`) and scaling it by `N` (`2x` smooth ceiling, `4x` FAIL at `24.20 ms`). Replacing this with async `Maintain::Poll` + isolated fixed GPU wait in Phase 8 lifted Auto-GPU `--selftest` to `8x` (`Section 2.2`).
- **Phase 7 Multi-Backend Filter Run (`perf/filters-20261113-082023.json`, `[SUPERSEDED]` by `8c`/`8d`):** Prior to Phase 8 `no_gpu.rs` `blit_inner` scanline/border optimizations, hidden-window `softbuffer` measured `none = 0.2839 ms (p95 = 0.3928)`, `myboy-smooth = 0.2878 ms (p95 = 0.3901)`, `bilinear = 2.4450 ms` on `no-gpu-xrgb8`; Phase 8 reduced these to `0.1900 / 0.1848 ms` real present (`0.1493 ms` in `no-gpu-xrgb8`, `0.7043 ms` `bilinear`, `Section 2.4`).
