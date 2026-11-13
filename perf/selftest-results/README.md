# Portable `--selftest` Instructions (No ROM / No Repo Required)

Run **Rustboy Advance** on any target Windows 10/11 PC to verify emulation speed, audio pacing, display filter timing, graphics adapters, and the **measured smooth slowdown ceiling** without copying any game ROMs, BIOS files, or source code.

## Quick Steps

1. Copy **only** `rustboy-advance.exe` (`target\release\rustboy-advance.exe`) to an empty folder on the target machine (for example `C:\temp\rba-test\rustboy-advance.exe`).
2. Open PowerShell or Command Prompt in that folder and run:

```powershell
.\rustboy-advance.exe --selftest
```

3. Optional modes to force software/no-GPU fallback paths:
   - Software CPU `wgpu` adapter (SwiftShader Vulkan or Windows DX12 WARP):
     ```powershell
     .\rustboy-advance.exe --selftest --software-adapter
     ```
   - Last-resort No-GPU presenter (`winit + softbuffer`, zero Vulkan/DX12/GL):
     ```powershell
     .\rustboy-advance.exe --selftest --no-gpu
     ```
4. Collect the generated JSON file from `.\perf\selftest-*.json` and send it back alongside the console summary.

## What `--selftest` Measures & What `PASS` Means

- **System & Adapter Inventory**: CPU model, physical cores / logical threads, total RAM, Windows build, and every GPU/software adapter enumerated by `wgpu` (`Vulkan`, `DX12`, `GL`) plus the chosen backend and reason.
- **Calibration Loop**: Single-thread integer calibration baseline compared against reference (`75.769 ms`).
- **600 Unthrottled Headless Frames**: Emulation + filter cost using the built-in MIT/Apache `STRESS_SELF` ROM (Mode 0, 4 BGs, 128 sprites, alpha blending).
- **Per-Filter Timing (`avg` / `p95` / `worst`)**: Every filter measured on the active adapter (plus OS `softbuffer` present cost for `none` vs `myboy-smooth` in `--no-gpu` mode).
- **Paced 59.73 Hz Run (`600f` after discarding first `120f` warm-up)**: Real audio DAC DRC pacing (`DrcPacer`), frame over-budget count, spikes `>2x` (`33.485 ms`), audio underruns, and presentation interval jitter.
- **Measured Slowdown Sweep (`N = 1, 2, 4, 6, 8, 12, 16, 24`, `300f` each)**: Real CPU-slowdown simulation (`(N-1)x` spin) stopping at the first failure, reporting both the highest `N` passing the smooth gate (`<0.5%` over-budget & `0` underruns) and highest `N` with `0` over-budget frames.

### Exact `PASS` Definition

`--selftest` reports **`PASS`** only when **all** of the following hold:
1. Embedded stress ROM renders `>= 16` distinct colors.
2. Emulation average frame time is `< 50%` of the `16.743 ms` GBA frame budget.
3. Post-warm-up (`120f` discarded) 600-frame paced run has `< 0.50%` frames over budget (`16.743 ms`), **`0`** spikes `> 2x` budget (`33.485 ms`), and **`0`** audio underruns.

