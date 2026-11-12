# Test ROM Results

- **BIOS mode:** HLE (built-in)
- **Summary:** 37 passed, 15 failed, 2 skipped (missing ROM), 2 skipped (missing golden) out of 56 total

## Results Table

| ROM | Criterion | Status | Notes | Frame Time (ms) |
|-----|-----------|--------|-------|-----------------|
| `jsmolka/arm.gba` | `register` | **PASS** | r12=0 at idle PC=0x08001EC4 (5f) | 7.305 |
| `jsmolka/thumb.gba` | `register` | **PASS** | r7=0 at idle PC=0x08000AAC (5f) | 7.222 |
| `jsmolka/memory.gba` | `register` | **PASS** | r12=0 at idle PC=0x080004C8 (5f) | 7.207 |
| `jsmolka/nes.gba` | `register` | **PASS** | r12=0 at idle PC=0x06014248 (5f) | 10.554 |
| `jsmolka/bios.gba` | `register` | **PASS** | r12=0 at idle PC=0x080003C0 (5f) | 7.267 |
| `jsmolka/unsafe.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000304 (5f) | 8.688 |
| `jsmolka/save/none.gba` | `register` | **PASS** | r12=0 at idle PC=0x080002A8 (5f) | 7.249 |
| `jsmolka/save/sram.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000470 (5f) | 7.186 |
| `jsmolka/save/flash64.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000AC8 (71f) | 13.995 |
| `jsmolka/save/flash128.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000C4C (71f) | 14.652 |
| `jsmolka/ppu/hello.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1862be60) | 6.326 |
| `jsmolka/ppu/shades.gba` | `fb_hash` | **PASS** | hash 0x06da45bf84412325 (crc32=0x1a9c4c92) | 7.740 |
| `jsmolka/ppu/stripes.gba` | `fb_hash` | **PASS** | hash 0xb804d1be99fc7525 (crc32=0x05166c70) | 7.466 |
| `fuzzarm/FuzzARM.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 8.186 |
| `fuzzarm/ARM_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 8.464 |
| `fuzzarm/ARM_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 8.934 |
| `fuzzarm/THUMB_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 8.860 |
| `fuzzarm/THUMB_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 8.856 |
| `armwrestler/arm-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0x77b5e53b) | 9.981 |
| `armwrestler/arm-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x3fbfcf52) | 11.131 |
| `armwrestler/arm-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0xbd7dfb8b) | 11.489 |
| `armwrestler/thumb-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0xfa0784c1) | 11.151 |
| `armwrestler/thumb-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1a59d72e) | 11.159 |
| `armwrestler/thumb-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0x18495aad) | 10.551 |
| `nba-hw-test/bus/128kb-boundary.gba` | `screenshot` | **FAIL** | 13/16 pass; DMA DEC 07FFFFF8 (68 vs 63) & 0801FFF0/0803FFF0 (68 vs 70) fail (crc32=0xef43f2c5) | 8.771 |
| `nba-hw-test/dma/burst-into-tears.gba` | `screenshot` | **FAIL** | 0/3 pass (1ST: 0x2e vs 0xea00, 2ND: 0xea00 vs 0xff24, DMA TIME: 43 vs 41) (crc32=0x53e7d0fe) | 8.061 |
| `nba-hw-test/dma/force-nseq-access.gba` | `screenshot` | **FAIL** | 0/2 pass (EWRAM DMA: 78 vs 88, ROM DMA: 78 vs 88) (crc32=0x0aeb8290) | 7.908 |
| `nba-hw-test/dma/latch.gba` | `screenshot` | **FAIL** | 2/4 pass (DMA LATCH 1: 0xc0de vs 0xdead, DMA LATCH 2: 0xc0de vs 0x1234) (crc32=0xcf7649a4) | 8.377 |
| `nba-hw-test/dma/start-delay.gba` | `screenshot` | **FAIL** | 0/1 pass (IMM: 18 vs 20) (crc32=0xcd81f685) | 7.714 |
| `nba-hw-test/haltcnt.gba` | `screenshot` | **FAIL** | 0/6 pass (HALTCNT DIRECT/CPUSET/POSTFLG/TIME IWRAM & ROM) (crc32=0x39cd150e) | 8.198 |
| `nba-hw-test/irq/irq-delay.gba` | `screenshot` | **FAIL** | 0/3 pass (IWRAM: 95 vs 92, EWRAM: 115 vs 112, ROM: 115 vs 120) (crc32=0x4fc84cc5) | 7.908 |
| `nba-hw-test/ppu/bgpd.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (3 linear grey ramps instead of 2 ramps) (crc32=0x41b13174) | 14.500 |
| `nba-hw-test/ppu/bgx.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (solid green; missing center vertical white line) (crc32=0xd5370a26) | 13.391 |
| `nba-hw-test/ppu/dispcnt-latch.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (full-height BCDEF/DEFGH; missing scanline latch delay) (crc32=0x7ccd9bca) | 9.555 |
| `nba-hw-test/ppu/greenswap.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (uniform RGB stripes; GREENSWAP swap unimplemented) (crc32=0x9a420c77) | 11.585 |
| `nba-hw-test/ppu/ram-access-timing.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_ram_access_timing.png` (observed 0x52a00a16cb1ce049, crc32=0x7f87eab3) — Interactive mode-selection hardware test (no automated pass/fail screen) | 11.928 |
| `nba-hw-test/ppu/sprite-hmosaic.gba` | `fb_hash` | **FAIL** | Visual mismatch vs expected.png (rows 9-14 horizontal mosaic stretch differs) (crc32=0x99822121) | 9.044 |
| `nba-hw-test/ppu/status-irq-dma.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_status_irq_dma.png` (observed 0x70602ed9184b6557, crc32=0x15185496) — Interactive raster/menu hardware test (no automated pass/fail screen) | 11.754 |
| `nba-hw-test/ppu/vram-mirror.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xaf8aa1ec) | 15.620 |
| `nba-hw-test/timer/reload.gba` | `screenshot` | **FAIL** | 4/7 pass (FFF8 32 7: 0xdeae vs 0xfff9; FFFF 16/32 0: 0xdeb5 vs 0xdeb4) (crc32=0x56386c33) | 8.168 |
| `nba-hw-test/timer/start-stop.gba` | `screenshot` | **FAIL** | 1/2 pass (2ND: 7 vs 8) (crc32=0x29380c6f) | 7.687 |
| `nba-hw-test/timer/tick-before-reload.gba` | `screenshot` | **FAIL** | 0/2 pass (TM1CNT_L: 0 vs 1, IF: 0 vs 8) (crc32=0xf9bb7091) | 7.998 |
| `tonc/bm_modes.gba` | `fb_hash` | **PASS** | hash 0x961b0d921dc3abf6 (crc32=0xcf60ed03) | 8.117 |
| `tonc/mos_demo.gba` | `fb_hash` | **PASS** | hash 0x62d60e0245b1448d (crc32=0x38ab65c8) | 11.240 |
| `tonc/win_demo.gba` | `fb_hash` | **PASS** | hash 0x646144f68f7bca7e (crc32=0x457181d4) | 11.054 |
| `tonc/bld_demo.gba` | `fb_hash` | **PASS** | hash 0x6edb60f4d6745eb0 (crc32=0xf33ed441) | 13.384 |
| `tonc/obj_aff.gba` | `fb_hash` | **PASS** | hash 0xaffdef941e408537 (crc32=0x4d59dde2) | 11.632 |
| `tonc/obj_demo.gba` | `fb_hash` | **PASS** | hash 0xf5f7eb2443b88837 (crc32=0x68dc09d6) | 7.576 |
| `tonc/m7_demo.gba` | `fb_hash` | **PASS** | hash 0xb91cab92fe7965cf (crc32=0x893a5b88) | 8.930 |
| `tonc/prio_demo.gba` | `fb_hash` | **PASS** | hash 0xfd0522e59d004833 (crc32=0x38097e6d) | 12.149 |
| `tonc/tte_demo.gba` | `fb_hash` | **PASS** | hash 0x79a60db2a397d56c (crc32=0x8be71ff8) | 6.916 |
| `tonc/irq_demo.gba` | `fb_hash` | **PASS** | hash 0x2b7dce9043cb4c63 (crc32=0xf8556dcc) | 9.052 |
| `tonc/tmr_demo.gba` | `fb_hash` | **PASS** | hash 0x6dbbc2cecf358dd5 (crc32=0x6b6de4b0) | 7.124 |
| `tonc/dma_demo.gba` | `fb_hash` | **PASS** | hash 0x9c4c8f185b506e2f (crc32=0x4f581c12) | 10.718 |
| `mgba/suite.gba (all)` | `screenshot` | **SKIP (missing ROM)** | Requires devkitARM-built suite.gba (see scripts/README.md) | - |
| `ags/ags.gba` | `screenshot` | **SKIP (missing ROM)** | Proprietary Nintendo AGS Aging Cartridge (user-supplied) | - |

## Failure Diagnosis

All 17 baseline failures below were reproduced on `main` (`HLE` BIOS) and inspected via `test_output/<id>.png`, upstream test source/reference images, and `crates/gba-core/src`.

### 1. ARMWrestler (`armwrestler/arm-ldm-stm`) — 8/12 pass

- **Observed:** `LDMIB!`, `LDMIA!`, `LDMDB!`, `LDMDA!`, `STMIB!`, `STMIA!`, `STMDB!`, and `STMDA!` pass (`OK`), while all four user-bank `LDM` instructions with writeback (`LDMIBS!`, `LDMIAS!`, `LDMDBS!`, `LDMDBA!`) report red `BAD Rn` ([`test_output/armwrestler_arm_ldm_stm.png`](test_output/armwrestler_arm_ldm_stm.png)).
- **Code pointer:** [`arm_block_transfer`](crates/gba-core/src/cpu.rs#L1104) at [`crates/gba-core/src/cpu.rs:1195`](crates/gba-core/src/cpu.rs#L1195) (`if writeback && !user_bank && (rlist & (1 << rn)) == 0 && rn != 15`). The `!user_bank` guard unconditionally suppresses base writeback whenever bit 22 (`S`) is set and `r15` is absent from `rlist`.

### 2. Bus & DMA Timing / Open-Bus Latch (`nba-hw-test`)

- **`nba-hw-test/bus/128kb-boundary.gba` (13/16 pass):** `LDM` (`0801FFF0..0803FFF8`), all `DMA INC` cases, and `DMA DEC 080003F8`/`0801FFF8`/`0803FFF8` pass, while `DMA DEC 07FFFFF8: FAIL 68 (expected: 63)`, `DMA DEC 0801FFF0: FAIL 68 (expected: 70)`, and `DMA DEC 0803FFF0: FAIL 68 (expected: 70)` fail.
  - **Code pointers:** [`SysBus::step_active_dma`](crates/gba-core/src/hw.rs#L264) at [`crates/gba-core/src/hw.rs:286-288`](crates/gba-core/src/hw.rs#L286-L288) forces `src_step = +unit_bytes` for all `0x08..=0x0D` GamePak sources without decrementing address/timing across `0x08000000 -> 0x07FFFFFC` or backward 128 KiB crossings; [`SysBus::rom_fetch_cycles`](crates/gba-core/src/bus.rs#L231-L240) at `bus.rs:234` only triggers forced-NSEQ when `(addr & 0x1FFFF) == 0`.
- **`nba-hw-test/dma/burst-into-tears.gba` (0/3 pass):** `1ST VALUE: FAIL 0x2e (expected: 0xea00)`, `2ND VALUE: FAIL 0xea00 (expected: 0xff24)`, `DMA TIME: FAIL 43 (expected: 41)`.
  - **Code pointers:** [`SysBus::step_active_dma`](crates/gba-core/src/hw.rs#L264-L388) and [`SysBus::read_16_dma`](crates/gba-core/src/bus.rs#L838-L841) execute each unit transfer independently without modelling the GamePak burst counter/open-bus carry across `0x07FFFFFE -> 0x08000000` and N/S burst timing.
- **`nba-hw-test/dma/force-nseq-access.gba` (0/2 pass):** `EWRAM DMA: FAIL 78 (expected: 88)`, `ROM DMA: FAIL 78 (expected: 88)`.
  - **Code pointers:** [`SysBus::read_open_bus_32`](crates/gba-core/src/bus.rs#L363-L369), [`crates/gba-core/src/hw.rs:327,343`](crates/gba-core/src/hw.rs#L327), and [`GbaCore::step_one`](crates/gba-core/src/lib.rs#L288-L292) / [`GbaCore::run_frame_with_breakpoints`](crates/gba-core/src/lib.rs#L356-L360): `last_access_was_dma` is only consulted for open-bus reads, never to force the CPU's first post-DMA fetch/access to `AccessType::NonSequential`.
- **`nba-hw-test/dma/latch.gba` (2/4 pass):** `DMA LATCH 0: PASS 0xc0de` and `BUS LATCH: PASS 0x46c046c0` pass, while `DMA LATCH 1: FAIL 0xc0de (expected: 0xdead)` and `DMA LATCH 2: FAIL 0xc0de (expected: 0x1234)` fail.
  - **Code pointers:** [`crates/gba-core/src/bus.rs:122`](crates/gba-core/src/bus.rs#L122) stores a single shared `dma_latch: u32` on `SysBus` instead of per-channel latches on `DmaChannel` (causing DMA1 to clobber DMA2's latch). Additionally, in [`crates/gba-core/src/hw.rs:333-342`](crates/gba-core/src/hw.rs#L333-L342), 16-bit unmapped reads (`< 0x0200_0000`) shift by `(s_addr & 2) * 8` instead of destination alignment, and line 342 overwrites `self.dma_latch` even when reading below `0x0200_0000`.
- **`nba-hw-test/dma/start-delay.gba` (0/1 pass):** `IMM: FAIL 18 (expected: 20)`.
  - **Code pointers:** [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L849-L854) schedules immediate DMA via `EventType::DmaActivate` at `current_cycle + 2` without full bus-arbiter synchronization latency relative to timer reads (2 cycles short).

### 3. `HALTCNT`, `POSTFLG`, `CpuSet`, & IRQ Delay (`nba-hw-test`)

- **`nba-hw-test/haltcnt.gba` (0/6 pass):** `HALTCNT DIRECT: FAIL 266 (expected: 12)`, `HALTCNT CPUSET: FAIL 44 (expected: 4155)`, `POSTFLG: FAIL 0 (expected: 1)`, `HALTCNT CPUSET DMA: FAIL 56 (expected: 4154)`, `HALTCNT TIME IWRAM: FAIL 21 (expected: 125)`, `HALTCNT TIME ROM: FAIL 133 (expected: 249)`.
  - **Code pointers:**
    1. [`SysBus::write_io_8`](crates/gba-core/src/hw.rs#L561-L570): 8-bit writes to `0x0400_0301` (`HALTCNT`) set `self.hw.halt_requested = true` unconditionally without BIOS-PC protection (`pc < 0x4000`), and `0x0400_0300` allows unguarded writes to `POSTFLG`.
    2. [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L939-L944): 16-bit writes to `0x0400_0300` unconditionally clobber `self.hw.postflg = val as u8` (setting `POSTFLG` to `0`) and ignore the `HALTCNT` high byte (`val >> 8`).
    3. [`SysBus::hle_swi`](crates/gba-core/src/hw.rs#L1115-L1147) (`SWI 0x0B` `CpuSet`): runs in zero emulated cycles rather than billing sequential read/write cycles; [`GbaCore`](crates/gba-core/src/lib.rs#L294-L306) wake-from-halt cycle timing also diverges on `IWRAM`/`ROM`.
- **`nba-hw-test/irq/irq-delay.gba` (0/3 pass):** `IRQ TIME IWRAM: FAIL 95 (expected: 92)`, `IRQ TIME EWRAM: FAIL 115 (expected: 112)`, `IRQ TIME ROM: FAIL 115 (expected: 120)`.
  - **Code pointers:** [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L932-L938) only sets `irq_sync_delay = 1` on `IME` `0 -> 1` transitions, and [`GbaCore`](crates/gba-core/src/lib.rs#L308-L323) (`cpu.trigger_irq` + flat `+ 3` cycles) does not model continuous interrupt synchronizer delay or memory waitstate differences in the HLE BIOS IRQ entry/exit trampoline across IWRAM, EWRAM, and ROM.

### 4. Timers (`nba-hw-test/timer/*`)

- **`nba-hw-test/timer/reload.gba` (4/7 pass):** fails `FFF8 32 7: FAIL 0xdeae (expected: 0xfff9)`, `FFFF 16 0: FAIL 0xdeb5 (expected: 0xdeb4)`, and `FFFF 32 0: FAIL 0xdeb5 (expected: 0xdeb4)`.
  - **Code pointers:** [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L884-L886) and [`reschedule_timer` / `handle_timer_overflow`](crates/gba-core/src/hw.rs#L391-L420): writes to `TMxCNT_L` update `timers[ch].reload` immediately instead of deferring reload visibility by 1 cycle across overflow/enable boundaries.
- **`nba-hw-test/timer/start-stop.gba` (1/2 pass):** `1ST: PASS 3`, `2ND: FAIL 7 (expected: 8)`.
  - **Code pointers:** [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L898-L905): clearing bit 7 (`TMxCNT_H`) cancels `TimerOverflow` and freezes `counter = cur` immediately without the hardware 1-cycle stop latency.
- **`nba-hw-test/timer/tick-before-reload.gba` (0/2 pass):** `TM1CNT_L: FAIL 0 (expected: 1)`, `IF: FAIL 0 (expected: 8)`.
  - **Code pointers:** [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L894-L897): `0 -> 1` enable immediately overwrites `counter = reload` and skips to `now_cycle + 2`, suppressing the pre-reload tick on the cycle before the reload value latches in.

### 5. PPU Timing, Latches, Mosaic, GREENSWAP, & VRAM Mirroring (`nba-hw-test/ppu/*`)

- **`nba-hw-test/ppu/bgpd.gba`:** renders 3 sharp linear greyscale ramps (`test_output/nba_ppu_bgpd.png`) instead of 2 ramps (`expected.png`).
  - **Code pointers:** [`Ppu::step_affine_refs_scanline`](crates/gba-core/src/ppu.rs#L205-L210) and [`SysBus::step_active_dma`](crates/gba-core/src/hw.rs#L264) HBlank/scanline ordering miss hardware `BG2PD` DMA timing relative to affine reference stepping.
- **`nba-hw-test/ppu/bgx.gba`:** renders solid green (`test_output/nba_ppu_bgx.png`), missing the center vertical white line in `expected.png`.
  - **Code pointers:** [`Ppu::set_bgx`](crates/gba-core/src/ppu.rs#L159-L163) and [`Ppu::render_dot_range`](crates/gba-core/src/ppu.rs#L229): mid-scanline `BG2X` DMA writes do not reproduce hardware reference latching (`ref_x`) vs scanline dot progress.
- **`nba-hw-test/ppu/dispcnt-latch.gba`:** draws full-height `BCDEF...` and `DEFGH...` glyph rows (`test_output/nba_ppu_dispcnt_latch.png`) instead of clipping top glyph rows (`expected.png`).
  - **Code pointers:** [`Ppu::begin_scanline`](crates/gba-core/src/ppu.rs#L212-L227) & `render_dot_range` evaluate `dispcnt` `BGx_ON` enable bits immediately instead of modelling the 3-scanline GBA BG-enable latch pipeline delay.
- **`nba-hw-test/ppu/greenswap.gba`:** draws uniform vertical RGB stripes (`test_output/nba_ppu_greenswap.png`) rather than odd/even green-channel swapping (`expected.png`).
  - **Code pointers:** [`Ppu::greenswap`](crates/gba-core/src/ppu.rs#L50), [`SysBus::write_io_16`](crates/gba-core/src/hw.rs#L704) (`0x0400_0002`), and [`Ppu::compose_scanline`](crates/gba-core/src/ppu.rs#L708): `greenswap` is stored and readable/writable, but `compose_scanline` never reads `self.greenswap`.
- **`nba-hw-test/ppu/sprite-hmosaic.gba`:** differs across rows 9-14 where adjacent mosaic/non-mosaic sprites overlap (`test_output/nba_ppu_sprite_hmosaic.png` vs `expected.png`).
  - **Code pointers:** [`Ppu::render_sprites`](crates/gba-core/src/ppu.rs#L611-L616) computes `(sx - (sx % mos_h)) as i32 - spr_x` per OAM entry instead of maintaining hardware's OAM horizontal mosaic latch state across adjacent sprites and priority changes.
- **`nba-hw-test/ppu/vram-mirror.gba` (7/10 pass):** `M0` (`5/5`) and `M3 14000h`/`1C000h` pass, while `M3 10000h: FAIL 0xdeadbeef (expected: 0xabad1dea)`, `M3 18000h: FAIL 0xdeadbeef (expected: 0x0)`, and `M3 CRC32: FAIL 0x99b0679a (expected: 0x5b156ce9)` fail.
  - **Code pointers:** [`Ppu::mirror_vram`](crates/gba-core/src/ppu.rs#L185-L192) unconditionally wraps `0x18000..=0x1FFFF` onto `0x10000..=0x17FFF` (`masked - 0x8000`) regardless of video mode. In bitmap modes 3-5 (`(dispcnt & 7) >= 3`), `0x06018000..0x0601BFFF` must ignore writes and return `0` on read rather than aliasing `0x06010000..0x06013FFF` ([`crates/gba-core/src/bus.rs`](crates/gba-core/src/bus.rs#L171) VRAM access & fastmem paths also need bitmap-mode bounds).

### 6. Skipped Suites (`SKIP`)

- `nba-hw-test/ppu/ram-access-timing.gba` & `nba-hw-test/ppu/status-irq-dma.gba` (`SKIP (missing golden)`): interactive menu/cycle-stepping visual diagnostic tools with no automated pass/fail detection or `expected.png` reference.
- `mgba/suite.gba` (`SKIP (missing ROM)`): `mgba-emu/suite` is distributed as source only; build `suite.gba` with `devkitARM` per [`scripts/README.md`](scripts/README.md) and place at `test-roms/mgba/suite.gba`.
- `ags/ags.gba` (`SKIP (missing ROM)`): commercial/proprietary Nintendo AGS Aging Cartridge; not downloaded automatically. Place a user-owned ROM at `test-roms/ags/ags.gba` to execute it.
