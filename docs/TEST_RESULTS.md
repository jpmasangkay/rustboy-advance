# Test ROM Results

- **BIOS mode:** HLE (built-in)
- **Summary:** 43 passed, 9 failed, 2 skipped (missing ROM), 2 skipped (missing golden) out of 56 total

## Results Table

| ROM | Criterion | Status | Notes | Frame Time (ms) |
|-----|-----------|--------|-------|-----------------|
| `jsmolka/arm.gba` | `register` | **PASS** | r12=0 at idle PC=0x08001EC4 (5f) | 0.146 |
| `jsmolka/thumb.gba` | `register` | **PASS** | r7=0 at idle PC=0x08000AAC (5f) | 0.117 |
| `jsmolka/memory.gba` | `register` | **PASS** | r12=0 at idle PC=0x080004C8 (5f) | 0.115 |
| `jsmolka/nes.gba` | `register` | **PASS** | r12=0 at idle PC=0x06014248 (5f) | 0.958 |
| `jsmolka/bios.gba` | `register` | **PASS** | r12=0 at idle PC=0x080003C0 (5f) | 0.127 |
| `jsmolka/unsafe.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000304 (5f) | 0.317 |
| `jsmolka/save/none.gba` | `register` | **PASS** | r12=0 at idle PC=0x080002A8 (5f) | 0.118 |
| `jsmolka/save/sram.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000470 (5f) | 0.115 |
| `jsmolka/save/flash64.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000AC8 (71f) | 0.800 |
| `jsmolka/save/flash128.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000C4C (71f) | 0.788 |
| `jsmolka/ppu/hello.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1862be60) | 0.058 |
| `jsmolka/ppu/shades.gba` | `fb_hash` | **PASS** | hash 0x06da45bf84412325 (crc32=0x1a9c4c92) | 0.107 |
| `jsmolka/ppu/stripes.gba` | `fb_hash` | **PASS** | hash 0xb804d1be99fc7525 (crc32=0x05166c70) | 0.096 |
| `fuzzarm/FuzzARM.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.214 |
| `fuzzarm/ARM_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.220 |
| `fuzzarm/ARM_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.220 |
| `fuzzarm/THUMB_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.221 |
| `fuzzarm/THUMB_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.222 |
| `armwrestler/arm-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0x77b5e53b) | 0.398 |
| `armwrestler/arm-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x3fbfcf52) | 0.476 |
| `armwrestler/arm-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0xbd7dfb8b) | 0.440 |
| `armwrestler/thumb-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0xfa0784c1) | 0.485 |
| `armwrestler/thumb-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1a59d72e) | 0.473 |
| `armwrestler/thumb-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0x18495aad) | 0.428 |
| `nba-hw-test/bus/128kb-boundary.gba` | `screenshot` | **FAIL** | 13/16 pass; DMA DEC 07FFFFF8 (68 vs 63) & 0801FFF0/0803FFF0 (68 vs 70) fail (crc32=0x8e9c5c44) | 0.171 |
| `nba-hw-test/dma/burst-into-tears.gba` | `screenshot` | **FAIL** | 0/3 pass (1ST: 0x2e vs 0xea00, 2ND: 0xea00 vs 0xff24, DMA TIME: 43 vs 41) (crc32=0x53e7d0fe) | 0.148 |
| `nba-hw-test/dma/force-nseq-access.gba` | `screenshot` | **FAIL** | 0/2 pass (EWRAM DMA: 78 vs 88, ROM DMA: 78 vs 88) (crc32=0x72dcf1e2) | 0.146 |
| `nba-hw-test/dma/latch.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xcd7838df) | 0.148 |
| `nba-hw-test/dma/start-delay.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x5d6665a9) | 0.142 |
| `nba-hw-test/haltcnt.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xe763cfdb) | 0.166 |
| `nba-hw-test/irq/irq-delay.gba` | `screenshot` | **FAIL** | output changed from known-bad baseline 0x4fc84cc5 to 0x73b6a3c4 (0/3 pass (IWRAM: 95 vs 92, EWRAM: 115 vs 112, ROM: 115 vs 120)) | 0.149 |
| `nba-hw-test/ppu/bgpd.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (3 linear grey ramps instead of 2 ramps) (crc32=0x41b13174) | 1.253 |
| `nba-hw-test/ppu/bgx.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (solid green; missing center vertical white line) (crc32=0xd5370a26) | 1.294 |
| `nba-hw-test/ppu/dispcnt-latch.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (full-height BCDEF/DEFGH; missing scanline latch delay) (crc32=0x7ccd9bca) | 0.427 |
| `nba-hw-test/ppu/greenswap.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (uniform RGB stripes; GREENSWAP swap unimplemented) (crc32=0x9a420c77) | 0.848 |
| `nba-hw-test/ppu/ram-access-timing.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_ram_access_timing.png` (observed 0x52a00a16cb1ce049, crc32=0x7f87eab3) — Interactive mode-selection hardware test (no automated pass/fail screen) | 0.628 |
| `nba-hw-test/ppu/sprite-hmosaic.gba` | `fb_hash` | **FAIL** | output changed from known-bad baseline 0x99822121 to 0x04a0afe4 (Visual mismatch vs expected.png (rows 9-14 horizontal mosaic stretch differs)) | 0.363 |
| `nba-hw-test/ppu/status-irq-dma.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_status_irq_dma.png` (observed 0x70602ed9184b6557, crc32=0x15185496) — Interactive raster/menu hardware test (no automated pass/fail screen) | 0.613 |
| `nba-hw-test/ppu/vram-mirror.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xaf8aa1ec) | 1.234 |
| `nba-hw-test/timer/reload.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xf4b00510) | 0.155 |
| `nba-hw-test/timer/start-stop.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xbe3e25d5) | 0.144 |
| `nba-hw-test/timer/tick-before-reload.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xce07d33a) | 0.145 |
| `tonc/bm_modes.gba` | `fb_hash` | **PASS** | hash 0x961b0d921dc3abf6 (crc32=0xcf60ed03) | 0.222 |
| `tonc/mos_demo.gba` | `fb_hash` | **PASS** | hash 0x62d60e0245b1448d (crc32=0x38ab65c8) | 0.409 |
| `tonc/win_demo.gba` | `fb_hash` | **PASS** | hash 0x646144f68f7bca7e (crc32=0x457181d4) | 0.284 |
| `tonc/bld_demo.gba` | `fb_hash` | **PASS** | hash 0x6edb60f4d6745eb0 (crc32=0xf33ed441) | 0.492 |
| `tonc/obj_aff.gba` | `fb_hash` | **PASS** | hash 0xaffdef941e408537 (crc32=0x4d59dde2) | 0.472 |
| `tonc/obj_demo.gba` | `fb_hash` | **PASS** | hash 0xf5f7eb2443b88837 (crc32=0x68dc09d6) | 0.164 |
| `tonc/m7_demo.gba` | `fb_hash` | **PASS** | hash 0xb91cab92fe7965cf (crc32=0x893a5b88) | 0.463 |
| `tonc/prio_demo.gba` | `fb_hash` | **PASS** | hash 0xfd0522e59d004833 (crc32=0x38097e6d) | 0.607 |
| `tonc/tte_demo.gba` | `fb_hash` | **PASS** | hash 0x79a60db2a397d56c (crc32=0x8be71ff8) | 0.200 |
| `tonc/irq_demo.gba` | `fb_hash` | **PASS** | hash 0x2b7dce9043cb4c63 (crc32=0xf8556dcc) | 0.468 |
| `tonc/tmr_demo.gba` | `fb_hash` | **PASS** | hash 0x6dbbc2cecf358dd5 (crc32=0x6b6de4b0) | 0.146 |
| `tonc/dma_demo.gba` | `fb_hash` | **PASS** | hash 0x9c4c8f185b506e2f (crc32=0x4f581c12) | 0.434 |
| `mgba/suite.gba (all)` | `screenshot` | **SKIP (missing ROM)** | Requires devkitARM-built suite.gba (see scripts/README.md) | - |
| `ags/ags.gba` | `screenshot` | **SKIP (missing ROM)** | Proprietary Nintendo AGS Aging Cartridge (user-supplied) | - |

## Failure Diagnosis

Summary counts above are generated by `cargo run -p gba-testrunner -- --report docs/TEST_RESULTS.md`. Previously failing suites that now pass (`armwrestler/arm-ldm-stm`, `nba-hw-test/dma/latch.gba`, `nba-hw-test/dma/start-delay.gba`, `nba-hw-test/haltcnt.gba`, `nba-hw-test/ppu/vram-mirror.gba`, `nba-hw-test/timer/*`) are reflected in the table above.

### Remaining Known Failures (9)

1. **Bus & DMA Timing (`nba-hw-test`)**
   - **`nba-hw-test/bus/128kb-boundary.gba` (13/16 pass):** Forward `INC` and `LDM` 128 KiB boundary cases pass; decrementing DMA across `0x08000000 -> 0x07FFFFFC` and backward 128 KiB crossings (`0801FFF0`, `0803FFF0`) diverge by 2-5 cycles (`SysBus::step_active_dma` in `crates/gba-core/src/hw.rs`, `SysBus::rom_fetch_cycles` in `crates/gba-core/src/bus.rs`).
   - **`nba-hw-test/dma/burst-into-tears.gba` (0/3 pass):** Models individual unit DMA transfers rather than GamePak ROM burst open-bus carry across `0x07FFFFFE -> 0x08000000` (`1ST: 0x2e vs 0xea00`, `2ND: 0xea00 vs 0xff24`, `DMA TIME: 43 vs 41`).
   - **`nba-hw-test/dma/force-nseq-access.gba` (0/2 pass):** CPU access immediately following DMA transfer does not yet force a Non-Sequential (`N`) cycle on EWRAM/ROM (`78 vs 88`).

2. **Interrupt Synchronizer Delay (`nba-hw-test/irq/irq-delay.gba`)**
   - **`0/3` pass (`IWRAM: 95 vs 92`, `EWRAM: 115 vs 112`, `ROM: 115 vs 120`):** HLE interrupt dispatch adds a fixed trampoline latency rather than full hardware IRQ synchronizer + pipeline waitstate coupling (`crates/gba-core/src/lib.rs`, `crates/gba-core/src/hw.rs`).

3. **PPU Mid-Scanline Latches, Mosaic & `GREENSWAP` (`nba-hw-test/ppu/*`)**
   - **`nba-hw-test/ppu/bgpd.gba`:** Renders 3 linear greyscale ramps instead of 2 due to HBlank DMA `BG2PD` vs affine reference step ordering.
   - **`nba-hw-test/ppu/bgx.gba`:** Renders solid green (missing vertical center white line) on mid-scanline `BG2X` DMA writes.
   - **`nba-hw-test/ppu/dispcnt-latch.gba`:** Evaluates `DISPCNT` `BGx_ON` bits immediately rather than modelling the 3-scanline hardware enable latch pipeline.
   - **`nba-hw-test/ppu/greenswap.gba`:** Scanline PPU compositor does not apply `GREENSWAP` (`0x0400_0002`), whereas the desktop post-processor handles the green-swap flag on CPU/shader paths.
   - **`nba-hw-test/ppu/sprite-hmosaic.gba`:** Rows 9-14 horizontal sprite mosaic stretch differs from hardware OAM horizontal mosaic latch state when mosaic and non-mosaic sprites overlap.

### Skipped Suites (4)

- **`nba-hw-test/ppu/ram-access-timing.gba` & `nba-hw-test/ppu/status-irq-dma.gba` (`SKIP (missing golden)`):** Interactive hardware diagnostic tools with menu controls and no single non-interactive pass/fail golden screen.
- **`mgba/suite.gba` (`SKIP (missing ROM)`):** `mgba-emu/suite` is distributed as source only; build `suite.gba` with `devkitARM` per [`scripts/README.md`](../scripts/README.md) and place at `test-roms/mgba/suite.gba`.
- **`ags/ags.gba` (`SKIP (missing ROM)`):** Proprietary Nintendo AGS Aging Cartridge; never downloaded automatically. Place a legally dumped copy at `test-roms/ags/ags.gba` to enable execution.
