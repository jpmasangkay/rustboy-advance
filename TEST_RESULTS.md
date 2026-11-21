# Test ROM Results

- **BIOS mode:** HLE (built-in)
- **Summary:** 58 passed, 0 failed, 2 skipped (missing ROM), 2 skipped (missing golden) out of 62 total

## Results Table

| ROM | Criterion | Status | Notes | Frame Time (ms) |
|-----|-----------|--------|-------|-----------------|
| `jsmolka/arm.gba` | `register` | **PASS** | r12=0 at idle PC=0x08001EC4 (5f) | 0.108 |
| `jsmolka/thumb.gba` | `register` | **PASS** | r7=0 at idle PC=0x08000AAC (5f) | 0.085 |
| `jsmolka/memory.gba` | `register` | **PASS** | r12=0 at idle PC=0x080004C8 (5f) | 0.082 |
| `jsmolka/nes.gba` | `register` | **PASS** | r12=0 at idle PC=0x06014248 (5f) | 0.901 |
| `jsmolka/bios.gba` | `register` | **PASS** | r12=0 at idle PC=0x080003C0 (5f) | 0.096 |
| `jsmolka/unsafe.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000304 (5f) | 0.198 |
| `jsmolka/save/none.gba` | `register` | **PASS** | r12=0 at idle PC=0x080002A8 (5f) | 0.096 |
| `jsmolka/save/sram.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000470 (5f) | 0.084 |
| `jsmolka/save/flash64.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000AC8 (74f) | 0.391 |
| `jsmolka/save/flash128.gba` | `register` | **PASS** | r12=0 at idle PC=0x08000C4C (74f) | 0.394 |
| `jsmolka/ppu/hello.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1862be60) | 0.039 |
| `jsmolka/ppu/shades.gba` | `fb_hash` | **PASS** | hash 0x06da45bf84412325 (crc32=0x1a9c4c92) | 0.084 |
| `jsmolka/ppu/stripes.gba` | `fb_hash` | **PASS** | hash 0xb804d1be99fc7525 (crc32=0x05166c70) | 0.073 |
| `fuzzarm/FuzzARM.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.122 |
| `fuzzarm/ARM_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.122 |
| `fuzzarm/ARM_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.122 |
| `fuzzarm/THUMB_Any.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.122 |
| `fuzzarm/THUMB_DataProcessing.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x472273a5) | 0.123 |
| `armwrestler/arm-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0x77b5e53b) | 0.239 |
| `armwrestler/arm-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x3fbfcf52) | 0.249 |
| `armwrestler/arm-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0xbd7dfb8b) | 0.259 |
| `armwrestler/thumb-alu` | `screenshot` | **PASS** | pixel-exact match (crc32=0xfa0784c1) | 0.319 |
| `armwrestler/thumb-ldr-str` | `screenshot` | **PASS** | pixel-exact match (crc32=0x1a59d72e) | 0.305 |
| `armwrestler/thumb-ldm-stm` | `screenshot` | **PASS** | pixel-exact match (crc32=0x18495aad) | 0.266 |
| `armwrestler/arm-alu-p2` | `screenshot` | **PASS** | pixel-exact match (crc32=0xef6ea51e) | 0.239 |
| `armwrestler/arm-ldr-str-p2` | `screenshot` | **PASS** | pixel-exact match (crc32=0xc33ccfcb) | 0.248 |
| `nba-hw-test/bus/128kb-boundary.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x6fa1b5e1) | 0.124 |
| `nba-hw-test/dma/burst-into-tears.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x8dfda7cf) | 0.105 |
| `nba-hw-test/dma/force-nseq-access.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xa7a13dba) | 0.103 |
| `nba-hw-test/dma/latch.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xcd7838df) | 0.105 |
| `nba-hw-test/dma/start-delay.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x5d6665a9) | 0.102 |
| `nba-hw-test/haltcnt.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x299f3de9) | 0.119 |
| `nba-hw-test/irq/irq-delay.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x15694d29) | 0.105 |
| `nba-hw-test/ppu/bgpd.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xb53e230f) | 1.271 |
| `nba-hw-test/ppu/bgx.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xac8283e1) | 1.340 |
| `nba-hw-test/ppu/dispcnt-latch.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xdba487b8) | 0.321 |
| `nba-hw-test/ppu/greenswap.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xa054cf49) | 0.744 |
| `nba-hw-test/ppu/ram-access-timing.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_ram_access_timing.png` (observed 0x52a00a16cb1ce049, crc32=0x7f87eab3) — Interactive mode-selection hardware test (no automated pass/fail screen) | 0.378 |
| `nba-hw-test/ppu/sprite-hmosaic.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0x04a0afe4) | 0.311 |
| `nba-hw-test/ppu/status-irq-dma.gba` | `screenshot` | **SKIP (missing golden)** | missing golden PNG `golden/nba_ppu_status_irq_dma.png` (observed 0x70602ed9184b6557, crc32=0x15185496) — Interactive raster/menu hardware test (no automated pass/fail screen) | 0.376 |
| `nba-hw-test/ppu/vram-mirror.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xaf8aa1ec) | 0.754 |
| `nba-hw-test/timer/reload.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xf4b00510) | 0.113 |
| `nba-hw-test/timer/start-stop.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xbe3e25d5) | 0.104 |
| `nba-hw-test/timer/tick-before-reload.gba` | `screenshot` | **PASS** | pixel-exact match (crc32=0xce07d33a) | 0.104 |
| `tonc/bm_modes.gba` | `fb_hash` | **PASS** | hash 0x961b0d921dc3abf6 (crc32=0xcf60ed03) | 0.168 |
| `tonc/mos_demo.gba` | `fb_hash` | **PASS** | hash 0x62d60e0245b1448d (crc32=0x38ab65c8) | 0.290 |
| `tonc/win_demo.gba` | `fb_hash` | **PASS** | hash 0x646144f68f7bca7e (crc32=0x457181d4) | 0.227 |
| `tonc/bld_demo.gba` | `fb_hash` | **PASS** | hash 0x6edb60f4d6745eb0 (crc32=0xf33ed441) | 0.353 |
| `tonc/obj_aff.gba` | `fb_hash` | **PASS** | hash 0xaffdef941e408537 (crc32=0x4d59dde2) | 0.334 |
| `tonc/obj_demo.gba` | `fb_hash` | **PASS** | hash 0xf5f7eb2443b88837 (crc32=0x68dc09d6) | 0.139 |
| `tonc/m7_demo.gba` | `fb_hash` | **PASS** | hash 0xb91cab92fe7965cf (crc32=0x893a5b88) | 0.357 |
| `tonc/prio_demo.gba` | `fb_hash` | **PASS** | hash 0xfd0522e59d004833 (crc32=0x38097e6d) | 0.421 |
| `tonc/tte_demo.gba` | `fb_hash` | **PASS** | hash 0x79a60db2a397d56c (crc32=0x8be71ff8) | 0.164 |
| `tonc/irq_demo.gba` | `fb_hash` | **PASS** | hash 0x2b7dce9043cb4c63 (crc32=0xf8556dcc) | 0.358 |
| `tonc/tmr_demo.gba` | `fb_hash` | **PASS** | hash 0x6dbbc2cecf358dd5 (crc32=0x6b6de4b0) | 0.109 |
| `tonc/dma_demo.gba` | `fb_hash` | **PASS** | hash 0x9c4c8f185b506e2f (crc32=0x4f581c12) | 0.322 |
| `tonc/bld_demo.gba (alpha blend)` | `fb_hash` | **PASS** | hash 0xb06c9fc1c2c0f642 (crc32=0x2230fa19) | 0.374 |
| `tonc/mos_demo.gba (bg+obj mosaic)` | `fb_hash` | **PASS** | hash 0xc6c9e4d84d6dae21 (crc32=0x87aad2d3) | 0.336 |
| `tonc/bm_modes.gba (mode 4)` | `fb_hash` | **PASS** | hash 0xc3de10bb5a256575 (crc32=0x5923cf51) | 0.116 |
| `tonc/obj_aff.gba (rotated)` | `fb_hash` | **PASS** | hash 0x64970e5aa2ed6912 (crc32=0x6ad7f0b3) | 0.335 |
| `mgba/suite.gba (all)` | `screenshot` | **SKIP (missing ROM)** | Requires devkitARM-built suite.gba (see scripts/README.md) | - |
| `ags/ags.gba` | `screenshot` | **SKIP (missing ROM)** | Proprietary Nintendo AGS Aging Cartridge (user-supplied) | - |
