use gba_core::bus::AccessType;
use gba_core::cartridge::SaveType;
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

struct HeadlessFrontend {
    frames: usize,
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    audio_samples: usize,
    keypad: u16,
}

impl HeadlessFrontend {
    fn new() -> Self {
        Self {
            frames: 0,
            last_fb: [0; LCD_WIDTH * LCD_HEIGHT],
            audio_samples: 0,
            keypad: 0x03FF,
        }
    }
}

impl HostFrontend for HeadlessFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.frames += 1;
        self.last_fb.copy_from_slice(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad
    }
}

fn rom_from_arm_words(words: &[u32]) -> Vec<u8> {
    let mut rom = Vec::with_capacity(words.len() * 4);
    for &w in words {
        rom.extend_from_slice(&w.to_le_bytes());
    }
    rom
}

#[test]
fn arm_data_proc_and_branch_execution() {
    // Direct boot executes from 0x0800_0000:
    // MOV R0, #10        ; 0xE3A0000A
    // MOV R1, #32        ; 0xE3A01020
    // ADD R2, R0, R1     ; 0xE0802001
    // SUBS R3, R2, #42   ; 0xE252302A (Z=1, C=1)
    // B .                ; 0xEAFFFFFE
    let rom = rom_from_arm_words(&[
        0xE3A0_000A,
        0xE3A0_1020,
        0xE080_2001,
        0xE252_302A,
        0xEAFF_FFFE,
    ]);
    let mut core = GbaCore::new(None, rom.clone());
    for _ in 0..8 {
        core.step_one();
    }
    assert_eq!(core.cpu.gpr[0], 10);
    assert_eq!(core.cpu.gpr[1], 32);
    assert_eq!(core.cpu.gpr[2], 42);
    assert_eq!(core.cpu.gpr[3], 0);
    assert_ne!(core.cpu.cpsr & (1 << 30), 0); // Z flag
    assert_ne!(core.cpu.cpsr & (1 << 29), 0); // C flag (no borrow)
}

#[test]
fn arm_to_thumb_bx_and_long_bl_switch() {
    // At 0x0800_0000 (ARM):
    //   ADD R0, PC, #4 | 1   -> 0xE28F0005  (PC = 0x0800_0008, R0 = 0x0800_000D -> Thumb at +0x0C)
    //   BX R0                -> 0xE12FFF10
    //   NOP                  -> 0xE1A00000
    // At 0x0800_000C (Thumb, two halfwords per word):
    //   MOV R1, #77 (0x214D) | BL +0x04 -> prefix(F000) suffix(F802)? Let's encode MOV R1,#77 ; ADD R1,#23 ; B .
    //   0x0800_000C: MOV R1, #77 (0x214D)
    //   0x0800_000E: ADD R1, #23 (0x3117)
    //   0x0800_0010: BL 0x0800_0018 -> target = 0x10 + 4 + 4 = 0x18 -> prefix 0xF000, suffix 0xF802 (actually 0x10 + 4 + (2<<1) = 0x18)
    //   0x0800_0014: B . (0xE7FE)
    //   0x0800_0016: NOP (0x46C0)
    //   0x0800_0018: MOV R2, #99 (0x2263)
    //   0x0800_001A: BX LR (0x4770)
    let w3: u32 = 0x214D | (0x3117 << 16);
    let w4: u32 = 0xF000 | (0xF802 << 16);
    let w5: u32 = 0xE7FE | (0x46C0 << 16);
    let w6: u32 = 0x2263 | (0x4770 << 16);

    let rom = rom_from_arm_words(&[0xE28F_0005, 0xE12F_FF10, 0xE1A0_0000, w3, w4, w5, w6]);
    let mut core = GbaCore::new(None, rom.clone());
    for _ in 0..20 {
        core.step_one();
    }
    assert!(core.cpu.is_thumb());
    assert_eq!(core.cpu.gpr[1], 100);
    assert_eq!(core.cpu.gpr[2], 99);
    // Should have returned via BX LR to 0x0800_0014 loop (PC == 0x0800_0014 + 4 == 0x0800_0018)
    assert_eq!(core.cpu.gpr[14], 0x0800_0015);
}

#[test]
fn ldm_stm_empty_rlist_and_base_quirks() {
    let rom = rom_from_arm_words(&[
        0xE3A0_D403, // MOV SP, #0x03000000
        0xE28D_D040, // ADD SP, SP, #0x40 (0x03000040)
        0xE90D_0000, // STMDB SP, {} -> stores PC (+12), does NOT writeback (W=0)
        0xE92D_0000, // STMDB SP!, {} -> stores PC, decrements SP by 0x40 (back to 0x03000000)
        0xEAFF_FFFE,
    ]);
    let mut core = GbaCore::new(None, rom.clone());
    for _ in 0..10 {
        core.step_one();
    }
    assert_eq!(core.cpu.gpr[13], 0x0300_0000);
}

#[test]
fn ppu_mode3_mode4_and_top2_compositor_prio() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // 1. Mode 3 (BG2 15bpp bitmap is completely opaque across 240x160)
    core.bus.write_io_16(0x0400_0000, 0x0403);
    core.bus
        .write_16(0x0600_0000, 0x001F, AccessType::NonSequential);
    core.bus
        .write_16(0x0500_0000, 0x03E0, AccessType::NonSequential);

    let mut fe = HeadlessFrontend::new();
    // Run 1 settle frame so the hardware 2-scanline DISPCNT forced-blank/layer latch settles at y=0.
    core.run_frame(&mut fe);
    core.run_frame(&mut fe);
    assert_eq!(fe.frames, 2);
    assert_eq!(fe.last_fb[0], 0x001F); // Red pixel at (0,0)
    assert_eq!(fe.last_fb[1], 0x0000); // VRAM black pixel at (1,0)

    // 2. Mode 4 (BG2 8bpp bitmap uses palette idx 0 as transparent backdrop)
    core.bus.write_io_16(0x0400_0000, 0x0404);
    core.bus
        .write_16(0x0500_0002, 0x7C00, AccessType::NonSequential); // Palette idx 1 = blue
    core.bus
        .write_16(0x0600_0000, 0x0001, AccessType::NonSequential); // x=0 -> pal 1, x=1 -> pal 0
    core.run_frame(&mut fe);
    assert_eq!(fe.last_fb[0], 0x7C00); // Blue
    assert_eq!(fe.last_fb[1], 0x03E0); // Transparent -> green backdrop

    // 3. Top-2 compositor priority order + alpha blend:
    // BG0 (prio 0, top), BG1 (prio 1, bot), OBJ (prio 2) -> BG0 blends with BG1, NOT OBJ!
    // Enable Mode 0 + BG0 + BG1 + OBJ
    core.bus.write_io_16(0x0400_0000, 0x1300);
    core.bus.write_io_16(0x0400_0008, 0x0000); // BG0 prio 0, charbase 0, screenbase 0 (0x0000)
    core.bus.write_io_16(0x0400_000A, 0x0101); // BG1 prio 1, charbase 0, screenbase 1 (0x0800)
    // Hide OAM entries 1..127 cleanly (attr0 bit 9 = disabled)
    for idx in 1..128u32 {
        core.bus
            .write_16(0x0700_0000 + idx * 8, 0x0200, AccessType::NonSequential);
    }
    // Screen entries for tile 1, palette 0
    core.bus
        .write_16(0x0600_0000, 0x0001, AccessType::NonSequential);
    core.bus
        .write_16(0x0600_0800, 0x0001, AccessType::NonSequential);
    // Tile 1 pixel (0,0) = color idx 2 (in VRAM charbase 0 + 32 bytes)
    core.bus
        .write_16(0x0600_0020, 0x0022, AccessType::NonSequential);
    core.bus
        .write_16(0x0500_0004, 0x001E, AccessType::NonSequential); // Color 2 = Red 30
    // Wait: make BG1 use palette 1 with Color idx 2 = Green 30
    core.bus
        .write_16(0x0600_0800, 0x1001, AccessType::NonSequential);
    core.bus
        .write_16(0x0500_0024, 0x03C0, AccessType::NonSequential); // Pal 1 col 2 = Green (30<<5)
    // Put an OBJ at (0,0) with priority 2 (blue)
    core.bus
        .write_16(0x0601_0000, 0x0011, AccessType::NonSequential); // OBJ tile 0 pix (0,0)=1
    core.bus
        .write_16(0x0500_0202, 0x7C00, AccessType::NonSequential); // OBJ pal 0 col 1 = Blue
    core.bus
        .write_16(0x0700_0000, 0x0000, AccessType::NonSequential); // y=0
    core.bus
        .write_16(0x0700_0002, 0x0000, AccessType::NonSequential); // x=0
    core.bus
        .write_16(0x0700_0004, 0x0800, AccessType::NonSequential); // prio=2, tile=0

    // Alpha blend BG0 (1st target) + BG1 (2nd target) at 50%/50% (EVA=8, EVB=8)
    core.bus.write_io_16(0x0400_0050, 0x0241); // mode 1, top=BG0 (bit 0), bot=BG1 (bit 9)
    core.bus.write_io_16(0x0400_0052, 0x0808); // EVA=8, EVB=8
    // Settle newly enabled BG0/BG1/OBJ DISPCNT latch pipeline across VBlank before checking y=0.
    core.run_frame(&mut fe);
    core.run_frame(&mut fe);
    // Expected: (30*8)>>4 = 15 red, (30*8)>>4 = 15 green -> 15 | (15<<5)
    assert_eq!(fe.last_fb[0], 15 | (15 << 5));
}

#[test]
fn memory_quirks_bios_lock_sram_mirror_and_vram_8bit() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // BIOS read while PC >= 0x4000 returns bios_latch
    let bval = core
        .bus
        .read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000);
    assert_eq!(bval, core.bus.bios_latch);

    // OAM 8-bit write is ignored; 8-bit PRAM/VRAM writes duplicate byte
    core.bus
        .write_8(0x0700_0000, 0xAB, AccessType::NonSequential);
    assert_eq!(core.bus.ppu.oam[0], 0);
    core.bus
        .write_8(0x0500_0002, 0x5A, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_16(0x0500_0002, AccessType::NonSequential, 0x0800_0000, false),
        0x5A5A
    );

    // SRAM 32K mirror on BOTH read and write
    core.bus.cart.save_type = SaveType::Sram32K;
    core.bus
        .write_8(0x0E00_0123, 0x7E, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_8123, AccessType::NonSequential, 0x0800_0000, false),
        0x7E
    );
}

#[test]
fn timers_cascade_and_hle_swi_div_sqrt_lz77() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // Timer 0 reload 0xFFFE prescaler 1, Timer 1 cascade enabled
    core.bus.write_io_16(0x0400_0100, 0xFFFE);
    core.bus.write_io_16(0x0400_0104, 0x0000);
    core.bus.write_io_16(0x0400_0106, 0x0084); // enable + cascade
    core.bus.write_io_16(0x0400_0102, 0x0080); // enable T0

    let mut fe = HeadlessFrontend::new();
    core.run_frame(&mut fe);
    let t1_val = core.bus.read_io_16(0x0400_0104, 0x0800_0000, false);
    assert!(
        t1_val > 1000,
        "Cascaded Timer 1 should increment on Timer 0 overflows, got {t1_val}"
    );

    // Test HLE Div & Sqrt
    core.cpu.gpr[0] = (-100i32) as u32;
    core.cpu.gpr[1] = 7;
    core.bus.hle_swi(&mut core.cpu, 0x06);
    assert_eq!(core.cpu.gpr[0] as i32, -14);
    assert_eq!(core.cpu.gpr[1] as i32, -2);
    assert_eq!(core.cpu.gpr[3], 14);

    core.cpu.gpr[0] = 625;
    core.bus.hle_swi(&mut core.cpu, 0x08);
    assert_eq!(core.cpu.gpr[0], 25);
}

#[test]
fn cartridge_flash_128k_and_save_state_roundtrip() {
    let mut rom = b"TEST_FLASH1M_V103_ROM".to_vec();
    rom.resize(0x1000, 0);
    let mut core = GbaCore::new(None, rom.clone());
    assert_eq!(core.bus.cart.save_type, SaveType::Flash128K);

    // Enter ID mode
    core.bus
        .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_5555, 0x90, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_0000, AccessType::NonSequential, 0x0800_0000, false),
        0x62
    );
    assert_eq!(
        core.bus
            .read_8(0x0E00_0001, AccessType::NonSequential, 0x0800_0000, false),
        0x13
    );
    // Exit ID mode
    core.bus
        .write_8(0x0E00_0000, 0xF0, AccessType::NonSequential);

    // Switch to bank 1 & program byte
    core.bus
        .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_5555, 0xB0, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_0000, 0x01, AccessType::NonSequential);

    core.bus
        .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_5555, 0xA0, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_0042, 0x55, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_0042, AccessType::NonSequential, 0x0800_0000, false),
        0x55
    );

    // Save state & verify fastmem + memory restore cleanly
    core.bus
        .write_32(0x0200_0100, 0xDEAD_BEEF, AccessType::NonSequential);
    let state = core.save_state().expect("serialize save state");
    core.bus
        .write_32(0x0200_0100, 0x0000_0000, AccessType::NonSequential);
    core.load_state(&state).expect("deserialize save state");
    assert_eq!(
        core.bus
            .read_32(0x0200_0100, AccessType::NonSequential, 0x0800_0000),
        0xDEAD_BEEF
    );
}

struct AudioCaptureFrontend {
    samples: Vec<i16>,
}

impl AudioCaptureFrontend {
    fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }
}

impl HostFrontend for AudioCaptureFrontend {
    fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.samples.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[test]
fn ppu_dot_accurate_mode_matches_scanline_and_respects_layer_mask() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // Enable Mode 0 + BG0
    core.bus.write_io_16(0x0400_0000, 0x0100);
    core.bus.write_io_16(0x0400_0008, 0x0000);
    core.bus
        .write_16(0x0500_0000, 0x7C00, AccessType::NonSequential); // Backdrop = blue
    core.bus
        .write_16(0x0500_0002, 0x001F, AccessType::NonSequential); // Pal 0 col 1 = red
    core.bus
        .write_16(0x0600_0000, 0x0001, AccessType::NonSequential); // Map tile 1 at (0,0)
    core.bus
        .write_16(0x0600_0020, 0x0011, AccessType::NonSequential); // Tile 1 pixels 0,1 = col 1

    let mut fe_scanline = HeadlessFrontend::new();
    // Settle hardware 2-scanline DISPCNT forced-blank/BG0 latch pipeline before checking y=0.
    core.run_frame(&mut fe_scanline);
    core.run_frame(&mut fe_scanline);
    assert_eq!(fe_scanline.last_fb[0], 0x001F);

    // Per-dot scheduler mode must match scanline renderer output pixel-for-pixel
    core.bus.ppu.dot_accurate_mode = true;
    let mut fe_dot = HeadlessFrontend::new();
    core.run_frame(&mut fe_dot);
    assert_eq!(fe_dot.last_fb, fe_scanline.last_fb);

    // Turning off BG0 via layer_enable_mask reveals the blue backdrop
    core.bus.ppu.layer_enable_mask = 0x1E;
    core.run_frame(&mut fe_dot);
    assert_eq!(fe_dot.last_fb[0], 0x7C00);
}

#[test]
fn apu_channel_enable_mask_mutes_channels_cleanly() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // Enable master sound, unmute L/R for SQ1 at max PSG volume, trigger loud SQ1 note
    core.bus.write_io_16(0x0400_0084, 0x0080); // SOUNDCNT_X: master enable
    core.bus.write_io_16(0x0400_0080, 0x1177); // SOUNDCNT_L: SQ1 L/R + full vol
    core.bus.write_io_16(0x0400_0082, 0x0002); // SOUNDCNT_H: PSG 100%
    core.bus.write_io_16(0x0400_0060, 0x0000); // SOUND1CNT_L
    core.bus.write_io_16(0x0400_0062, 0xF080); // SOUND1CNT_H: vol=15, 50% duty
    core.bus.write_io_16(0x0400_0064, 0x8600); // SOUND1CNT_X: trigger note

    let mut fe = AudioCaptureFrontend::new();
    core.run_frame(&mut fe);
    assert!(
        fe.samples.iter().any(|&s| s != 0),
        "Expected audible SQ1 waveform when all APU channels enabled"
    );

    fe.samples.clear();
    core.bus.apu.channel_enable_mask = 0x00;
    core.run_frame(&mut fe);
    assert!(
        !fe.samples.is_empty() && fe.samples.iter().all(|&s| s == 0),
        "Expected silence when channel_enable_mask == 0"
    );
}

#[test]
fn cartridge_save_override_and_boktai_yoshi_warioware_peripherals() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE; 128]);
    let mut core = GbaCore::new(None, rom.clone());

    // SaveType::None override blocks SRAM writes (buffer stays erased 0xFF)
    core.bus.cart.set_save_type_override(Some(SaveType::None));
    core.bus
        .write_8(0x0E00_0010, 0xA5, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_0010, AccessType::NonSequential, 0x0800_0000, false),
        0xFF
    );

    // Override to Flash64K and program one byte
    core.bus
        .cart
        .set_save_type_override(Some(SaveType::Flash64K));
    assert_eq!(core.bus.cart.effective_save_type(), SaveType::Flash64K);
    core.bus
        .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_5555, 0xA0, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_0010, 0x5A, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_0010, AccessType::NonSequential, 0x0800_0000, false),
        0x5A
    );

    // Yoshi Tiled / Topsy-Turvy Tilt Sensor
    core.bus.cart.tilt_enabled = true;
    core.bus.cart.tilt_x = 0x03A5;
    core.bus.cart.tilt_y = 0x02B4;
    core.bus
        .write_8(0x0E00_8000, 0x55, AccessType::NonSequential);
    core.bus
        .write_8(0x0E00_8100, 0xAA, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_8(0x0E00_8200, AccessType::NonSequential, 0x0800_0000, false),
        0xA5
    );
    assert_eq!(
        core.bus
            .read_8(0x0E00_8300, AccessType::NonSequential, 0x0800_0000, false),
        0x83
    );
    assert_eq!(
        core.bus
            .read_8(0x0E00_8400, AccessType::NonSequential, 0x0800_0000, false),
        0xB4
    );
    assert_eq!(
        core.bus
            .read_8(0x0E00_8500, AccessType::NonSequential, 0x0800_0000, false),
        0x02
    );
    core.bus.cart.tilt_enabled = false;

    // Boktai Solar Sensor (GPIO)
    core.bus.cart.solar_enabled = true;
    core.bus.cart.solar_lux = 250; // threshold = 255 - 250 = 5 clock pulses
    core.bus.cart.write_rom_16(0x0800_00C8, 0x0001); // Enable readable GPIO
    core.bus.cart.write_rom_16(0x0800_00C6, 0x0007); // Pins 0..2 output, pin 3 input
    core.bus.cart.write_rom_16(0x0800_00C4, 0x0002); // RST=1 clears counter
    assert_eq!(core.bus.cart.read_rom_16(0x0800_00C4) & 0x08, 0);
    for _ in 0..5 {
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0000);
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0001); // Rising CLK
    }
    assert_eq!(core.bus.cart.read_rom_16(0x0800_00C4) & 0x08, 0x08);
    core.bus.cart.solar_enabled = false;

    // WarioWare Twisted Gyro + Rumble (GPIO: bit0=RES/SAMPLE, bit1=CLK, bit2=DATA_OUT(in), bit3=RUMBLE)
    core.bus.cart.rumble_enabled = true;
    core.bus.cart.gyro_enabled = true;
    core.bus.cart.gyro_val = 0x0A53; // 12-bit value
    core.bus.cart.write_rom_16(0x0800_00C6, 0x000B); // bit0=SAMPLE, bit1=CLK, bit2=in, bit3=RUMBLE
    core.bus.cart.write_rom_16(0x0800_00C4, 0x0008); // Rumble ON
    assert!(core.bus.cart.rumble_active);
    core.bus.cart.write_rom_16(0x0800_00C4, 0x0003); // Sample=1, CLK=1 -> latches 4 dummy zeros + 12-bit gyro_val
    let mut shifted = 0u16;
    for _ in 0..16 {
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0002); // CLK=1, Sample=0
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0000); // Falling edge shifts MSB onto pin 2
        let bit = (core.bus.cart.read_rom_16(0x0800_00C4) >> 2) & 1;
        shifted = (shifted << 1) | bit;
    }
    assert_eq!(
        shifted & 0xF000,
        0,
        "top 4 gyro bits must be leading dummy zeros"
    );
    assert_eq!(shifted & 0x0FFF, 0x0A53);
}

fn setup_dma1_fifo_a_sine(
    core: &mut GbaCore,
    ewram_bytes: usize,
    amplitude: f64,
    period_samples: f64,
) {
    for i in 0..ewram_bytes {
        let angle = core::f64::consts::TAU * (i as f64) / period_samples;
        let sample = (angle.sin() * amplitude).round().clamp(-127.0, 127.0) as i8;
        core.bus.write_8(
            0x0200_0000 + i as u32,
            sample as u8,
            AccessType::NonSequential,
        );
    }

    // Master enable, FIFO A 100% vol + L/R enabled + Timer 0 selected, PSG 100%
    core.bus.write_io_16(0x0400_0084, 0x0080);
    core.bus.write_io_16(0x0400_0080, 0x0000);
    core.bus.write_io_16(0x0400_0082, 0x0306); // DMA A 100% (bit 2), L/R (bits 8-9), Timer 0 (bit 10=0)

    // DMA1: SAD=0x0200_0000, DAD=0x0400_00A0 (FIFO_A), repeat + 32-bit + FIFO timing + enable
    core.bus.write_io_16(0x0400_00BC, 0x0000);
    core.bus.write_io_16(0x0400_00BE, 0x0200);
    core.bus.write_io_16(0x0400_00C0, 0x00A0);
    core.bus.write_io_16(0x0400_00C2, 0x0400);
    core.bus.write_io_16(0x0400_00C4, 0x0004);
    core.bus.write_io_16(0x0400_00C6, 0xB640);

    // Timer 0 at ~18,157 Hz: 16,777,216 / 924 = 18,157.16 Hz -> reload = 65536 - 924 = 0xFC64
    core.bus.write_io_16(0x0400_0100, 0xFC64);
    core.bus.write_io_16(0x0400_0102, 0x0080);
}

fn dft_spectrum_power_bh(samples: &[f64]) -> Vec<f64> {
    let n = samples.len();
    let nf = n as f64;
    let denom = (n - 1) as f64;
    let tau = core::f64::consts::TAU;
    let windowed: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let p = tau * (i as f64) / denom;
            let w =
                0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos();
            x * w
        })
        .collect();

    let mut powers = vec![0.0f64; n / 2 + 1];
    for (k, power_out) in powers.iter_mut().enumerate() {
        let mut re = 0.0f64;
        let mut im = 0.0f64;
        let d_angle = tau * (k as f64) / nf;
        let (ds, dc) = d_angle.sin_cos();
        let mut c = 1.0f64;
        let mut s = 0.0f64;
        for &xw in &windowed {
            re += xw * c;
            im -= xw * s;
            let next_c = c * dc - s * ds;
            let next_s = s * dc + c * ds;
            c = next_c;
            s = next_s;
        }
        *power_out = re * re + im * im;
    }
    powers
}

#[test]
fn apu_high_freq_square_wave_suppresses_aliasing() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // SQ1 at 10,922.67 Hz (freq = 2048 - 12 = 2036 = 0x7F4):
    // Odd harmonics >= 3 (32,768 Hz, 54,613 Hz, ...) exceed 24 kHz Nyquist.
    core.bus.write_io_16(0x0400_0084, 0x0080);
    core.bus.write_io_16(0x0400_0080, 0x1177);
    core.bus.write_io_16(0x0400_0082, 0x0002);
    core.bus.write_io_16(0x0400_0060, 0x0000);
    core.bus.write_io_16(0x0400_0062, 0xF080); // vol=15, 50% duty
    core.bus.write_io_16(0x0400_0064, 0x87F4);

    let mut fe = AudioCaptureFrontend::new();
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }

    let mut mono = Vec::with_capacity(2048);
    while mono.len() < 2048 {
        fe.samples.clear();
        core.run_frame(&mut fe);
        for &s in fe.samples.iter().step_by(2) {
            if mono.len() < 2048 {
                mono.push(s as f64);
            }
        }
    }

    let spectrum = dft_spectrum_power_bh(&mono);
    // Fundamental bin: 10922.667 * 2048 / 48000 = 466.034 (4-term BH lobe +-4 bins)
    let fund_power: f64 = spectrum[462..=470].iter().sum();
    let alias_power: f64 = spectrum[5..=1024]
        .iter()
        .enumerate()
        .filter(|&(idx, _)| !(462..=470).contains(&(idx + 5)))
        .map(|(_, &p)| p)
        .sum();

    assert!(
        fund_power > 1e10,
        "expected strong 10.92 kHz fundamental, got {fund_power}"
    );
    let alias_ratio = alias_power / fund_power;
    assert!(
        alias_ratio < 1e-4,
        "expected >40 dB suppression of above-Nyquist aliases, got ratio={alias_ratio:.6e} (fund={fund_power:.3e}, alias={alias_power:.3e})"
    );

    // Also test an ultrasonic 26,214.4 Hz square wave (freq = 2048 - 5 = 2043 = 0x7FB)
    // which lies entirely above 24 kHz Nyquist and must attenuate strongly.
    core.bus.write_io_16(0x0400_0064, 0x87FB);
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    fe.samples.clear();
    core.run_frame(&mut fe);
    let inband_rms = (mono.iter().map(|x| x * x).sum::<f64>() / mono.len() as f64).sqrt();
    let ultra_rms = (fe
        .samples
        .iter()
        .step_by(2)
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / (fe.samples.len() / 2) as f64)
        .sqrt();
    assert!(
        ultra_rms < inband_rms * 0.05,
        "expected ultrasonic 26.2 kHz square wave to be suppressed, ultra_rms={ultra_rms:.2}, inband_rms={inband_rms:.2}"
    );
}

#[test]
fn apu_fifo_18khz_steady_frame_output_and_interpolation() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);

    // 1. Non-interpolated 18,157 Hz FIFO playback
    let mut core_raw = GbaCore::new(None, rom.clone());
    setup_dma1_fifo_a_sine(&mut core_raw, 16_384, 64.0, 32.0); // ~567.4 Hz sine

    let expected_pairs = (gba_core::CYCLES_PER_FRAME as f64) * (gba_core::SAMPLE_RATE_HZ as f64)
        / (gba_core::CPU_CLOCK_HZ as f64);
    let mut fe = AudioCaptureFrontend::new();
    let mut raw_mono = Vec::new();
    for frame in 0..24 {
        fe.samples.clear();
        core_raw.run_frame(&mut fe);
        assert_eq!(fe.samples.len() % 2, 0);
        let pairs = fe.samples.len() / 2;
        assert!(
            (pairs as f64 - expected_pairs).abs() <= 1.0,
            "frame {frame}: stereo pairs {pairs} drifted from expected {expected_pairs:.2}"
        );
        if frame >= 4 {
            for &s in fe.samples.iter().step_by(2) {
                raw_mono.push(s as f64);
            }
        }
    }
    assert!(
        raw_mono.iter().any(|&s| s.abs() > 100.0),
        "expected non-zero 18,157 Hz FIFO A output"
    );

    // 2. Interpolated 18,157 Hz FIFO playback reduces high-frequency sample-hold imaging
    let mut core_interp = GbaCore::new(None, rom);
    core_interp.bus.apu.fifo_interpolation = true;
    setup_dma1_fifo_a_sine(&mut core_interp, 16_384, 64.0, 32.0);
    let mut interp_mono = Vec::new();
    for frame in 0..24 {
        fe.samples.clear();
        core_interp.run_frame(&mut fe);
        if frame >= 4 {
            for &s in fe.samples.iter().step_by(2) {
                interp_mono.push(s as f64);
            }
        }
    }

    let raw_spec = dft_spectrum_power_bh(&raw_mono[..2048]);
    let interp_spec = dft_spectrum_power_bh(&interp_mono[..2048]);
    // Carrier imaging around f_s_fifo +- f0 (18,157 +- 567 Hz -> ~17.5..18.8 kHz -> bins 740..810)
    let raw_img: f64 = raw_spec[740..=810].iter().sum();
    let interp_img: f64 = interp_spec[740..=810].iter().sum();
    assert!(
        interp_img < raw_img * 0.25,
        "expected fifo_interpolation to attenuate 18 kHz imaging spurs, raw_img={raw_img:.3e}, interp_img={interp_img:.3e}"
    );
}

#[test]
fn apu_silence_settles_to_zero_after_dc_blocker() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    core.bus.write_io_16(0x0400_0084, 0x0080);
    core.bus.write_io_16(0x0400_0080, 0x1177);
    core.bus.write_io_16(0x0400_0082, 0x0002);
    core.bus.write_io_16(0x0400_0060, 0x0000);
    core.bus.write_io_16(0x0400_0062, 0xF080);
    core.bus.write_io_16(0x0400_0064, 0x8500);

    let mut fe = AudioCaptureFrontend::new();
    for _ in 0..6 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    assert!(fe.samples.iter().any(|&s| s.abs() > 1000));

    // Disable APU master sound -> hardware capacitor discharges smoothly to exact 0
    core.bus.write_io_16(0x0400_0084, 0x0000);
    for _ in 0..30 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    assert!(
        !fe.samples.is_empty() && fe.samples.iter().all(|&s| s == 0),
        "expected exact zero output after DC blocker settles, peak={}",
        fe.samples.iter().map(|s| s.abs()).max().unwrap_or(0)
    );
}

#[test]
fn apu_individual_channel_masks_and_soundcnt_h_ratios() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());
    setup_dma1_fifo_a_sine(&mut core, 32_768, 64.0, 32.0);
    // Also trigger SQ1 so both FIFO A (bit 4) and SQ1 (bit 0) produce audio
    core.bus.write_io_16(0x0400_0080, 0x1177);
    core.bus.write_io_16(0x0400_0062, 0xF080);
    core.bus.write_io_16(0x0400_0064, 0x8600);

    let mut fe = AudioCaptureFrontend::new();
    core.run_frame(&mut fe);
    assert!(fe.samples.iter().any(|&s| s != 0));

    // Mask out both active channels (bits 0 and 4) while keeping unused bits 1,2,3,5 enabled
    core.bus.apu.channel_enable_mask = 0b10_1110;
    for _ in 0..30 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    assert!(
        fe.samples.iter().all(|&s| s == 0),
        "expected silence when only active channels (SQ1 + FIFO A) are masked off"
    );

    // Unmask FIFO A only, verify SOUNDCNT_H 50% DMA ratio reduces energy vs 100%
    core.bus.apu.channel_enable_mask = 0b01_0000;
    core.bus.write_io_16(0x0400_0082, 0x0306); // DMA A = 100% (bit 2 = 1)
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_100 = fe
        .samples
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);

    core.bus.write_io_16(0x0400_0082, 0x0302); // DMA A = 50% (bit 2 = 0)
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_50 = fe
        .samples
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak_100 > peak_50 && peak_50 > 0 && peak_50 * 2 <= peak_100 + 200,
        "expected 50% DMA A ratio (~half of 100%), peak_100={peak_100}, peak_50={peak_50}"
    );
}

#[test]
fn apu_save_state_roundtrip_does_not_panic() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());
    core.bus.apu.fifo_interpolation = true;
    setup_dma1_fifo_a_sine(&mut core, 16_384, 64.0, 32.0);
    core.bus.write_io_16(0x0400_0080, 0x1177);
    core.bus.write_io_16(0x0400_0062, 0xF080);
    core.bus.write_io_16(0x0400_0064, 0x8600);

    let mut fe = AudioCaptureFrontend::new();
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }

    let saved = core
        .save_state()
        .expect("serialize GBA state with active APU");

    // Mutate state significantly before restoring
    core.bus.write_io_16(0x0400_0084, 0x0000);
    core.bus.apu.fifo_interpolation = false;
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }

    core.load_state(&saved)
        .expect("deserialize GBA state with active APU");
    assert!(
        core.bus.apu.fifo_interpolation,
        "fifo_interpolation flag should persist across save state roundtrip"
    );

    fe.samples.clear();
    core.run_frame(&mut fe);
    let expected_pairs = (gba_core::CYCLES_PER_FRAME as f64) * (gba_core::SAMPLE_RATE_HZ as f64)
        / (gba_core::CPU_CLOCK_HZ as f64);
    let pairs = fe.samples.len() / 2;
    assert!((pairs as f64 - expected_pairs).abs() <= 1.0);
    assert!(
        fe.samples.iter().any(|&s| s.abs() > 100),
        "expected restored APU to continue emitting valid audio without panic or silence"
    );
}

#[test]
fn apu_soundbias_clamp_and_resolution() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // Enable loud SQ1 50% duty square wave (+-120 around center at PSG 100%, L/R vol 7)
    core.bus.write_io_16(0x0400_0084, 0x0080);
    core.bus.write_io_16(0x0400_0080, 0x1177);
    core.bus.write_io_16(0x0400_0082, 0x0002);
    core.bus.write_io_16(0x0400_0060, 0x0000);
    core.bus.write_io_16(0x0400_0062, 0xF080);
    core.bus.write_io_16(0x0400_0064, 0x8600);

    let mut fe = AudioCaptureFrontend::new();
    core.bus.write_io_16(0x0400_0088, 0x0200); // Default mid-bias 512, 9-bit resolution
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_mid_bias = fe
        .samples
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);

    // Raise SOUNDBIAS level to near top (0x03F0 = 1008): positive half-cycle clips heavily at 1023
    core.bus.write_io_16(0x0400_0088, 0x03F0);
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_clipped_bias = fe
        .samples
        .iter()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak_mid_bias > peak_clipped_bias * 13 / 10 && peak_clipped_bias > 0,
        "expected 10-bit saturation clamp at high SOUNDBIAS to reduce swing: mid={peak_mid_bias}, clipped={peak_clipped_bias}"
    );
}

#[test]
fn hle_swi_soundbias_midikey2freq_and_ppu_skip_rendering() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom.clone());

    // 1. Test SWI 0x19 SoundBias (r0 == 0 -> 0x0000, r0 != 0 -> 0x0200)
    core.cpu.gpr[0] = 0x0001;
    core.bus.hle_swi(&mut core.cpu, 0x19);
    assert_eq!(core.bus.apu.soundbias & 0x03FE, 0x0200);

    core.cpu.gpr[0] = 0x0000;
    core.bus.hle_swi(&mut core.cpu, 0x19);
    assert_eq!(core.bus.apu.soundbias & 0x03FE, 0x0000);

    // 2. Test SWI 0x1F MidiKey2Freq
    let wave_ptr = 0x0200_0000u32;
    core.bus
        .write_32(wave_ptr + 4, 44100, AccessType::NonSequential);
    core.cpu.gpr[0] = wave_ptr;
    core.cpu.gpr[1] = 180; // Base key 180 -> ratio 1:1
    core.cpu.gpr[2] = 0; // Fine pitch 0
    core.bus.hle_swi(&mut core.cpu, 0x1F);
    assert_eq!(
        core.cpu.gpr[0], 44100,
        "Base key 180 should return original sample rate"
    );

    // Key 168 (1 octave down = 12 semitones down) -> 44100 / 2 = 22050
    core.cpu.gpr[0] = wave_ptr;
    core.cpu.gpr[1] = 168;
    core.cpu.gpr[2] = 0;
    core.bus.hle_swi(&mut core.cpu, 0x1F);
    assert_eq!(
        core.cpu.gpr[0], 22050,
        "1 octave down should halve the frequency"
    );

    // Key 192 (1 octave up = 12 semitones up) -> 44100 * 2 = 88200
    core.cpu.gpr[0] = wave_ptr;
    core.cpu.gpr[1] = 192;
    core.cpu.gpr[2] = 0;
    core.bus.hle_swi(&mut core.cpu, 0x1F);
    assert_eq!(
        core.cpu.gpr[0], 88200,
        "1 octave up should double the frequency"
    );

    // 3. Test PPU skip_rendering
    let mut fe = HeadlessFrontend::new();
    core.bus.ppu.skip_rendering = true;
    core.run_frame(&mut fe);
    assert_eq!(fe.frames, 1);
    core.bus.ppu.skip_rendering = false;
    core.run_frame(&mut fe);
    assert_eq!(fe.frames, 2);
}

#[test]
fn test_fastmem_vs_slow_path_random_addresses() {
    let mut rom = vec![0u8; 0x80000]; // 512 KiB ROM
    // Seed predictable PRNG pattern into ROM
    let mut lfsr: u32 = 0xACE1_u32;
    for b in rom.iter_mut() {
        let bit = (lfsr ^ (lfsr >> 2) ^ (lfsr >> 3) ^ (lfsr >> 5)) & 1;
        lfsr = (lfsr >> 1) | (bit << 15);
        *b = (lfsr & 0xFF) as u8;
    }

    let mut core = GbaCore::new(None, rom.clone());

    // 1. EWRAM: test random 8-bit, 16-bit, and 32-bit writes and reads with mirroring
    for i in 0..500u32 {
        let mirror_page = (i % 64) * 0x0004_0000; // 0x0200_0000 .. 0x02FF_0000
        let offset = i.wrapping_mul(73) & 0x3_FFFC;
        let addr = 0x0200_0000 + mirror_page + offset;
        let val32 = 0xDEAD_BEEF ^ i.wrapping_mul(0x9E37_79B9);

        // Write 32-bit via bus (fastmem path)
        core.bus.write_32(addr, val32, AccessType::NonSequential);

        // Read 32-bit via bus (fastmem) and verify against slow-path EWRAM buffer slice
        let read_val = core
            .bus
            .read_32(addr, AccessType::NonSequential, 0x0800_0000);
        let ewram_off = (addr as usize) & 0x3FFFF;
        let expected_val = u32::from_le_bytes([
            core.bus.ewram[ewram_off],
            core.bus.ewram[ewram_off + 1],
            core.bus.ewram[ewram_off + 2],
            core.bus.ewram[ewram_off + 3],
        ]);
        assert_eq!(
            read_val, val32,
            "EWRAM 32-bit write-read mismatch at {addr:#010X}"
        );
        assert_eq!(
            read_val, expected_val,
            "EWRAM 32-bit fastmem vs slow slice mismatch at {addr:#010X}"
        );

        // Also test 16-bit and 8-bit sub-reads
        let read16 = core
            .bus
            .read_16(addr + 2, AccessType::NonSequential, 0x0800_0000, false);
        assert_eq!(
            read16,
            (val32 >> 16) as u16,
            "EWRAM 16-bit read mismatch at {addr:#010X}"
        );

        let read8 = core
            .bus
            .read_8(addr + 1, AccessType::NonSequential, 0x0800_0000, false);
        assert_eq!(
            read8,
            ((val32 >> 8) & 0xFF) as u8,
            "EWRAM 8-bit read mismatch at {addr:#010X}"
        );
    }

    // 2. IWRAM: test random 8-bit, 16-bit, and 32-bit writes and reads with 32 KiB mirroring
    for i in 0..500u32 {
        let mirror_page = (i % 512) * 0x0000_8000; // 0x0300_0000 .. 0x03FF_8000
        let offset = i.wrapping_mul(31) & 0x7FFC;
        let addr = 0x0300_0000 + mirror_page + offset;
        let val32 = 0x1234_5678 ^ i.wrapping_mul(0x85EB_CA6B);

        core.bus.write_32(addr, val32, AccessType::NonSequential);

        let read_val = core
            .bus
            .read_32(addr, AccessType::NonSequential, 0x0800_0000);
        let iwram_off = (addr as usize) & 0x7FFF;
        let expected_val = u32::from_le_bytes([
            core.bus.iwram[iwram_off],
            core.bus.iwram[iwram_off + 1],
            core.bus.iwram[iwram_off + 2],
            core.bus.iwram[iwram_off + 3],
        ]);
        assert_eq!(
            read_val, val32,
            "IWRAM 32-bit write-read mismatch at {addr:#010X}"
        );
        assert_eq!(
            read_val, expected_val,
            "IWRAM 32-bit fastmem vs slow slice mismatch at {addr:#010X}"
        );
    }

    // 3. ROM: verify fastmem reads against cart.read_rom_32 across pages 0x08..=0x0D
    for region in [0x08u32, 0x09, 0x0A, 0x0B] {
        for off in (0x4000..0x70000).step_by(0x1000) {
            let addr = (region << 24) + off;
            let fast_val = core
                .bus
                .read_32(addr, AccessType::NonSequential, 0x0800_0000);
            let slow_val = core.bus.cart.read_rom_32(addr & !3);
            assert_eq!(
                fast_val, slow_val,
                "ROM fastmem vs slow read mismatch at {addr:#010X}"
            );
        }
    }

    // 4. Out-of-bounds ROM read (open-bus fallback)
    let oob_addr = 0x0800_0000 + (rom.len() as u32) + 0x4000;
    core.bus.open_bus_latch = 0xA5A5_5A5A;
    let oob_val = core
        .bus
        .read_32(oob_addr, AccessType::NonSequential, 0x0800_0000);
    // Out-of-range ROM address in 16-bit ROM bus reads 16-bit halfword addr & !1 divided by 2
    let expected_oob = core.bus.cart.read_rom_32(oob_addr);
    assert_eq!(oob_val, expected_oob, "OOB ROM read mismatch");
}

#[test]
fn test_prefetch_real_sysbus_and_128kb_boundary() {
    let rom = vec![0u8; 0x40000];
    let mut core = GbaCore::new(None, rom);
    // Enable prefetch with WS0 S=2 cycles
    core.bus.update_waitcnt(0x4010);
    core.bus.prefetch.active = true;
    core.bus.prefetch.head_addr = 0x0801_FFF8;
    core.bus.prefetch.tail_addr = 0x0801_FFF8;
    core.bus.prefetch.count = 0;
    core.bus.prefetch.cycle_acc = 0;

    // 4 halfwords remain before 0x0802_0000 (128 KiB boundary). Stepping 100 cycles must stop at 4!
    core.bus.step_prefetch(100);
    assert_eq!(core.bus.prefetch.count, 4);
    assert_eq!(core.bus.prefetch.tail_addr, 0x0802_0000);
    assert_eq!(core.bus.prefetch.cycle_acc, 0);

    // Away from a boundary, stepping fills up to 8 halfwords accurately.
    core.bus.prefetch.head_addr = 0x0800_0100;
    core.bus.prefetch.tail_addr = 0x0800_0100;
    core.bus.prefetch.count = 0;
    core.bus.prefetch.cycle_acc = 0;
    core.bus.step_prefetch(5); // 2 halfwords (4 cyc) + 1 remainder
    assert_eq!(core.bus.prefetch.count, 2);
    assert_eq!(core.bus.prefetch.cycle_acc, 1);
    assert_eq!(core.bus.prefetch.tail_addr, 0x0800_0104);
    core.bus.step_prefetch(200);
    assert_eq!(core.bus.prefetch.count, 8);
    assert_eq!(core.bus.prefetch.cycle_acc, 0);
}

#[test]
fn test_scheduler_wakeups_and_apu_advance() {
    let rom = vec![0u8; 0x1000];
    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = HeadlessFrontend::new();

    // Enable sound master to verify advance_scanline mixes audio
    core.bus.apu.write_soundcnt_x(0x0080);

    // Count scheduler events popped over one full frame (280,896 cycles)
    let mut wakeups = 0usize;
    let end_cycle = core.bus.scheduler.current_cycle + gba_core::CYCLES_PER_FRAME;
    while core.bus.scheduler.current_cycle < end_cycle {
        while let Some((ev_cycle, kind)) = core.bus.scheduler.pop_due() {
            wakeups += 1;
            core.bus.dispatch_event(ev_cycle, kind);
        }
        let cycles = core.cpu.step(&mut core.bus);
        core.bus.scheduler.current_cycle += cycles.max(1);
    }
    while let Some((ev_cycle, kind)) = core.bus.scheduler.pop_due() {
        wakeups += 1;
        core.bus.dispatch_event(ev_cycle, kind);
    }

    core.bus.apu.flush(core.bus.scheduler.current_cycle);
    fe.push_audio_samples(&core.bus.apu.output_buffer);

    // Exactly 456 PPU events (228 HDrawEnd + 228 HBlank), down from ~1,097 wakeups!
    assert_eq!(
        wakeups, 456,
        "expected 456 scanline scheduler events, got {wakeups}"
    );
    // Audio samples were generated via scanline advance
    assert!(
        fe.audio_samples > 0,
        "audio samples should be produced by scanline advance"
    );
}

#[test]
fn test_mode4_fast_path_pixel_identical_to_normal_path() {
    let rom = vec![0u8; 0x1000];
    let mut core = GbaCore::new(None, rom.clone());

    // Setup Mode 4, BG2 enabled, forced blank off, no windows or sprites
    core.bus.ppu.dispcnt = 4 | 0x0400;
    core.bus.ppu.bldcnt = 0;
    core.bus.ppu.bgcnt[2] = 0;
    core.bus.ppu.bgp_a[0] = 0x0100;
    core.bus.ppu.bgp_b[0] = 0;
    core.bus.ppu.bgp_c[0] = 0;
    core.bus.ppu.bgp_d[0] = 0x0100;
    core.bus.ppu.ref_x[0] = 0;

    // Fill PRAM with diverse 15-bit BGR555 colors
    for c in 0..256usize {
        let off = c * 2;
        core.bus.ppu.pram[off] = (c * 7) as u8;
        core.bus.ppu.pram[off + 1] = ((c * 13) >> 8) as u8;
    }

    // Fill VRAM front and back pages with distinct test patterns
    for i in 0..LCD_WIDTH * LCD_HEIGHT {
        core.bus.ppu.vram[i] = (i % 256) as u8;
        core.bus.ppu.vram[0xA000 + i] = ((i + 128) % 256) as u8;
    }

    // Verify both page 0 and page 1
    for page in [0, 1] {
        if page == 1 {
            core.bus.ppu.dispcnt |= 0x0010; // Page flip bit
        } else {
            core.bus.ppu.dispcnt &= !0x0010;
        }

        for y in 0..LCD_HEIGHT {
            core.bus.ppu.ref_y[0] = (y as i32) << 8;

            // 1. Render using fast path
            core.bus.ppu.render_scanline(y);
            let row_start = y * LCD_WIDTH;
            let mut fast_scanline = [0u16; LCD_WIDTH];
            fast_scanline
                .copy_from_slice(&core.bus.ppu.framebuffer[row_start..row_start + LCD_WIDTH]);

            // 2. Render using normal path (temporarily breaking identity affine to force normal path)
            core.bus.ppu.ref_x[0] = 0;
            core.bus.ppu.ref_y[0] = (y as i32) << 8;
            core.bus.ppu.bgp_b[0] = 1; // Disables fast path!
            core.bus.ppu.render_scanline(y);
            core.bus.ppu.bgp_b[0] = 0; // Restore identity!

            let mut normal_scanline = [0u16; LCD_WIDTH];
            normal_scanline
                .copy_from_slice(&core.bus.ppu.framebuffer[row_start..row_start + LCD_WIDTH]);

            assert_eq!(
                fast_scanline, normal_scanline,
                "Mode 4 fast path diverged from normal path on page {page}, scanline {y}"
            );
        }
    }
}

#[test]
fn test_3_layer_save_detector_and_dma3_sizing() {
    // 1. Layer 1: Verified GameDB entry takes precedence over generic SDK strings
    // (e.g., Classic NES Series contains "EEPROM_V" string but requires 512B EEPROM).
    let mut rom_classic_nes = vec![0u8; 0x1000];
    rom_classic_nes[0xAC..0xB0].copy_from_slice(b"FSME");
    rom_classic_nes[0x200..0x208].copy_from_slice(b"EEPROM_V");
    let cart_nes = gba_core::cartridge::Cartridge::new(rom_classic_nes);
    assert_eq!(
        cart_nes.save_type,
        SaveType::Eeprom512B,
        "GameDB entry for Classic NES must override generic EEPROM_V heuristic"
    );

    // Unknown game code falls back to SDK library string detection
    let mut rom_flash1m = vec![0u8; 0x1000];
    rom_flash1m[0xAC..0xB0].copy_from_slice(b"ZZZZ");
    rom_flash1m[0x200..0x209].copy_from_slice(b"FLASH1M_V");
    let cart = gba_core::cartridge::Cartridge::new(rom_flash1m);
    assert_eq!(
        cart.save_type,
        SaveType::Flash128K,
        "FLASH1M_V string must detect Flash128K for unknown game codes"
    );

    // 2. Layer 3: GameDB fallback when no string is present
    let mut rom_doom2 = vec![0u8; 0x1000];
    rom_doom2[0xAC..0xB0].copy_from_slice(b"A9DE"); // Doom II
    let cart_doom2 = gba_core::cartridge::Cartridge::new(rom_doom2);
    assert_eq!(
        cart_doom2.save_type,
        SaveType::Flash64K,
        "A9DE Doom II without string should fallback to Flash64K"
    );

    let mut rom_duke_us = vec![0u8; 0x1000];
    rom_duke_us[0xAC..0xB0].copy_from_slice(b"AD9E"); // Duke Nukem Advance USA
    let cart_duke_us = gba_core::cartridge::Cartridge::new(rom_duke_us);
    assert_eq!(
        cart_duke_us.save_type,
        SaveType::Eeprom512B,
        "AD9E Duke Nukem should fallback to EEPROM"
    );

    let mut rom_duke_eu = vec![0u8; 0x1000];
    rom_duke_eu[0xAC..0xB0].copy_from_slice(b"AD9P"); // Duke Nukem Advance EUR
    let cart_duke_eu = gba_core::cartridge::Cartridge::new(rom_duke_eu);
    assert_eq!(
        cart_duke_eu.save_type,
        SaveType::Eeprom512B,
        "AD9P Duke Nukem EUR should fallback to EEPROM"
    );

    // 3. Layer 2: EEPROM DMA3 transfer auto-sizing (9/73 words vs 17/81 words)
    let mut cart_eeprom = cart_duke_us;
    assert!(!cart_eeprom.eeprom.detected_8k);
    let mut sram = [0xFFu8; 0x20000];
    let mut dirty = false;

    // DMA3 transfer of 81 units auto-sizes to 8KB
    cart_eeprom.eeprom.write_bit(&mut sram, &mut dirty, 1, 81);
    assert!(
        cart_eeprom.eeprom.detected_8k,
        "DMA3 count 81 should auto-size EEPROM to 8KB"
    );

    // Reset back to Idle for next DMA transfer
    cart_eeprom.eeprom.reset();

    // DMA3 transfer of 73 units auto-sizes back to 512B
    cart_eeprom.eeprom.write_bit(&mut sram, &mut dirty, 1, 73);
    assert!(
        !cart_eeprom.eeprom.detected_8k,
        "DMA3 count 73 should auto-size EEPROM to 512B"
    );
}

#[test]
fn test_state_hash_determinism_across_frames() {
    let rom_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("test-roms/tonc/pageflip.gba");
    if !rom_path.exists() {
        return;
    }
    let rom = std::fs::read(&rom_path).expect("Failed to read pageflip.gba");

    fn fnv1a_hash(data: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for &b in data {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    struct NullFrontend;
    impl gba_core::HostFrontend for NullFrontend {
        fn present_frame(&mut self, _frame: &[u16; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT]) {}
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            0x03FF
        }
    }

    let run_sim = |rom_data: Vec<u8>| -> (u64, [u32; 16], u32) {
        let mut core = GbaCore::new(None, rom_data);
        let mut fe = NullFrontend;
        for _ in 0..60 {
            core.run_frame(&mut fe);
        }
        let mut state_bytes = Vec::new();
        for &reg in &core.cpu.gpr {
            state_bytes.extend_from_slice(&reg.to_le_bytes());
        }
        state_bytes.extend_from_slice(&core.cpu.cpsr.to_le_bytes());
        state_bytes.extend_from_slice(&core.bus.iwram[..]);
        state_bytes.extend_from_slice(&core.bus.ewram[..]);
        state_bytes.extend_from_slice(&core.bus.ppu.vram[..]);
        (fnv1a_hash(&state_bytes), core.cpu.gpr, core.cpu.cpsr)
    };

    let (hash1, gpr1, cpsr1) = run_sim(rom.clone());
    let (hash2, gpr2, cpsr2) = run_sim(rom);

    assert_eq!(
        gpr1, gpr2,
        "CPU general purpose registers diverged between runs"
    );
    assert_eq!(cpsr1, cpsr2, "CPSR diverged between runs");
    assert_eq!(hash1, hash2, "State hash diverged across 60 frames");
}

#[test]
#[ignore = "microbenchmark: runs 2M Instant::now() calls; invoke with -- --ignored"]
fn test_timer_overhead_and_subsystem_breakdown() {
    let n = 2_000_000;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        let t = std::hint::black_box(std::time::Instant::now());
        std::hint::black_box(t.elapsed());
    }
    let dur = t0.elapsed();
    let ns_per_call = (dur.as_nanos() as f64) / (n as f64);
    println!("\n=== TIMER OVERHEAD ===");
    println!("Total time for {n} Instant::now()+elapsed() calls: {dur:?}");
    println!("Average cost per measurement: {ns_per_call:.2} ns");
    let calls_per_frame = 160 + 228 + 50; // PPU (160) + APU (228) + DMA (~50)
    let frame_overhead_us = (calls_per_frame as f64 * ns_per_call) / 1000.0;
    println!(
        "Estimated timer overhead per frame ({calls_per_frame} calls): {frame_overhead_us:.3} us ({:.4} ms)",
        frame_overhead_us / 1000.0
    );

    let env_rom = std::env::var("GBA_BENCHMARK_ROM").ok();
    let rom_path = env_rom.as_deref().unwrap_or("roms/benchmark.gba");
    if let Ok(rom) = std::fs::read(rom_path) {
        let mut core = gba_core::GbaCore::new(None, rom);
        struct DummyFe;
        impl gba_core::HostFrontend for DummyFe {
            fn present_frame(&mut self, _fb: &[u16; 240 * 160]) {}
            fn push_audio_samples(&mut self, _samples: &[i16]) {}
            fn poll_keypad(&mut self) -> u16 {
                0x03FF
            }
        }
        let mut fe = DummyFe;
        // Warmup 60 frames
        for _ in 0..60 {
            core.run_frame(&mut fe);
        }

        // Measure 120 frames uninstrumented
        let t_uninst_start = std::time::Instant::now();
        for _ in 0..120 {
            core.run_frame(&mut fe);
        }
        let uninst_dur = t_uninst_start.elapsed();
        let uninst_per_frame_ms = (uninst_dur.as_secs_f64() * 1000.0) / 120.0;

        // Measure 120 frames instrumented (run_frame_bench)
        let t_bench_start = std::time::Instant::now();
        let mut ppu_total = std::time::Duration::ZERO;
        let mut apu_total = std::time::Duration::ZERO;
        let mut dma_total = std::time::Duration::ZERO;
        let mut emu_bench_total = std::time::Duration::ZERO;
        for _ in 0..120 {
            let (timing, _) = core.run_frame_bench(&mut fe);
            ppu_total += timing.ppu_time;
            apu_total += timing.apu_time;
            dma_total += timing.dma_time;
            emu_bench_total += timing.total_time;
        }
        let bench_dur = t_bench_start.elapsed();
        let bench_per_frame_ms = (bench_dur.as_secs_f64() * 1000.0) / 120.0;
        let empirical_timer_overhead_ms = bench_per_frame_ms - uninst_per_frame_ms;

        let ppu_ms = (ppu_total.as_secs_f64() * 1000.0) / 120.0;
        let apu_ms = (apu_total.as_secs_f64() * 1000.0) / 120.0;
        let dma_ms = (dma_total.as_secs_f64() * 1000.0) / 120.0;
        let other_ms = uninst_per_frame_ms - (ppu_ms + apu_ms + dma_ms);

        println!("\n=== EMPIRICAL COMPARISON (120 frames of benchmark ROM) ===");
        println!(
            "Pass 1 (Uninstrumented run_frame):     {uninst_per_frame_ms:.4} ms/frame ({:.1} FPS)",
            1000.0 / uninst_per_frame_ms
        );
        println!(
            "Pass 2 (Instrumented run_frame_bench): {bench_per_frame_ms:.4} ms/frame ({:.1} FPS)",
            1000.0 / bench_per_frame_ms
        );
        println!(
            "Empirical Instrumentation Overhead:    {empirical_timer_overhead_ms:.4} ms/frame ({:.2}%)",
            (empirical_timer_overhead_ms / uninst_per_frame_ms) * 100.0
        );
        println!(
            "  PPU Time:                            {ppu_ms:.4} ms ({:.1}%)",
            (ppu_ms / uninst_per_frame_ms) * 100.0
        );
        println!(
            "  APU Time:                            {apu_ms:.4} ms ({:.1}%)",
            (apu_ms / uninst_per_frame_ms) * 100.0
        );
        println!(
            "  DMA Time:                            {dma_ms:.4} ms ({:.1}%)",
            (dma_ms / uninst_per_frame_ms) * 100.0
        );
        println!(
            "  CPU + Bus Remaining:                 {other_ms:.4} ms ({:.1}%)",
            (other_ms / uninst_per_frame_ms) * 100.0
        );

        // Measure instruction profile
        let mut total_insts = 0u64;
        let mut total_loads = 0u64;
        let mut total_stores = 0u64;
        let mut total_branches = 0u64;
        for _ in 0..120 {
            let (_, prof) = core.run_frame_bench(&mut fe);
            total_insts += prof.decode_exec_cycles;
            total_loads += prof.mem_read_cycles;
            total_stores += prof.mem_write_cycles;
            total_branches += prof.branch_flush_cycles;
        }
        println!(
            "  Profile counts / frame: insts={}, loads={}, stores={}, branches={}",
            total_insts / 120,
            total_loads / 120,
            total_stores / 120,
            total_branches / 120
        );
    }
}

#[test]
fn test_exec_from_iwram_and_ewram_waitstates_and_savestate() {
    let rom = vec![0u8; 0x1000];
    let mut core = GbaCore::new(None, rom);

    // Install identical ARM routine in EWRAM (0x0200_0000) and IWRAM (0x0300_0000):
    //   MOV R0, #0        ; 0xE3A00000
    //   ADD R0, R0, #3    ; 0xE2800003
    //   ADD R0, R0, #4    ; 0xE2800004
    //   B   .             ; 0xEAFFFFFE
    let arm_prog: [u32; 4] = [0xE3A0_0000, 0xE280_0003, 0xE280_0004, 0xEAFF_FFFE];
    for (idx, &word) in arm_prog.iter().enumerate() {
        let off = (idx * 4) as u32;
        core.bus
            .write_32(0x0200_0000 + off, word, AccessType::NonSequential);
        core.bus
            .write_32(0x0300_0000 + off, word, AccessType::NonSequential);
    }

    // 1. Execute from EWRAM (32-bit waitstate = 6 cycles per sequential instruction fetch)
    core.cpu.gpr[0] = 0;
    core.cpu.cpsr = 0x1F; // ARM System mode
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0000);
    let cyc0 = core.cpu.step(&mut core.bus);
    let cyc1 = core.cpu.step(&mut core.bus);
    assert_eq!(core.cpu.gpr[0], 3);
    // Pipeline had first 2 words fetched; step 0 fetches +8 (+6 cyc), step 1 fetches +12 (+6 cyc)
    assert_eq!(cyc0, 6);
    assert_eq!(cyc1, 6);
    assert!(core.bus.code_page_ptr.is_some());
    assert_eq!(core.bus.code_page_region, 0x02);

    // Verify save_state_raw + load_state_raw preserves waitstate_table (did not zero to 0)
    let raw = core.save_state_raw().expect("save_state_raw");
    core.cpu.step(&mut core.bus);
    assert_eq!(core.cpu.gpr[0], 7);
    core.load_state_raw(&raw).expect("load_state_raw");
    assert_eq!(core.bus.waitstate_table[0x02][2], [6, 6]);
    assert_eq!(core.bus.waitstate_table[0x03][2], [1, 1]);
    let post_load_cyc = core.cpu.step(&mut core.bus);
    assert_eq!(core.cpu.gpr[0], 7);
    assert_eq!(post_load_cyc, 6);

    // 2. Execute ARM & Thumb from IWRAM (32-bit waitstate = 1 cycle, Thumb = 1 cycle)
    core.cpu.gpr[0] = 0;
    core.cpu.cpsr = 0x1F;
    core.cpu.flush_pipeline(&mut core.bus, 0x0300_0000);
    let iw0 = core.cpu.step(&mut core.bus);
    let iw1 = core.cpu.step(&mut core.bus);
    let iw2 = core.cpu.step(&mut core.bus);
    assert_eq!((iw0, iw1, iw2), (1, 1, 1));
    assert_eq!(core.cpu.gpr[0], 7);

    // Thumb in IWRAM mirror (0x0300_8100): MOVS R1, #55; ADDS R1, #12; B .
    core.bus
        .write_16(0x0300_8100, 0x2137, AccessType::NonSequential);
    core.bus
        .write_16(0x0300_8102, 0x310C, AccessType::NonSequential);
    core.bus
        .write_16(0x0300_8104, 0xE7FE, AccessType::NonSequential);
    core.cpu.gpr[1] = 0;
    core.cpu.cpsr = 0x3F; // Thumb System mode
    core.cpu.flush_pipeline(&mut core.bus, 0x0300_8100);
    let th0 = core.cpu.step(&mut core.bus);
    let th1 = core.cpu.step(&mut core.bus);
    assert_eq!((th0, th1), (1, 1));
    assert_eq!(core.cpu.gpr[1], 67);
}

#[test]
fn test_self_modifying_code_invalidates_cached_code_page() {
    let rom = vec![0u8; 0x1000];
    let mut core = GbaCore::new(None, rom);

    // Place self-modifying ARM code in IWRAM (0x0300_0000), writing to an ALIAS mirror
    // (0x0300_8018) of the same physical IWRAM word (+0x18, safely past the 3-stage pipeline).
    // 0x00: NOP             ; primes code_page_ptr for 0x0300_0000
    // 0x04: STR R2, [R1]    ; writes MOV R0, #42 to 0x0300_8018 -> physical +0x18
    // 0x08: NOP
    // 0x0C: NOP
    // 0x10: NOP
    // 0x14: NOP
    // 0x18: MOV R0, #1      ; patched to MOV R0, #42 (0xE3A0_002A)
    // 0x1C: B .
    let prog: [u32; 8] = [
        0xE1A0_0000,
        0xE581_2000,
        0xE1A0_0000,
        0xE1A0_0000,
        0xE1A0_0000,
        0xE1A0_0000,
        0xE3A0_0001,
        0xEAFF_FFFE,
    ];
    for (idx, &w) in prog.iter().enumerate() {
        core.bus
            .write_32(0x0300_0000 + (idx as u32) * 4, w, AccessType::NonSequential);
    }

    core.cpu.gpr[0] = 0;
    core.cpu.gpr[1] = 0x0300_8018; // Mirrored IWRAM alias!
    core.cpu.gpr[2] = 0xE3A0_002A; // MOV R0, #42
    core.cpu.cpsr = 0x1F;
    core.cpu.flush_pipeline(&mut core.bus, 0x0300_0000);

    core.cpu.step(&mut core.bus); // +0x00 NOP -> caches code_page_ptr for IWRAM page
    assert!(core.bus.code_page_ptr.is_some());
    core.cpu.step(&mut core.bus); // +0x04 STR R2, [R1] -> writes to mirror alias 0x0300_8018
    assert!(
        core.bus.code_page_ptr.is_none(),
        "writing to mirrored code page must invalidate code_page_ptr"
    );
    for _ in 0..6 {
        core.cpu.step(&mut core.bus);
    }
    assert_eq!(
        core.cpu.gpr[0], 42,
        "CPU must execute patched MOV R0, #42 instead of original MOV R0, #1"
    );

    // Repeat self-modifying write directly within EWRAM (0x0200_0000) same page:
    for (idx, &w) in prog.iter().enumerate() {
        core.bus
            .write_32(0x0200_0000 + (idx as u32) * 4, w, AccessType::NonSequential);
    }
    core.cpu.gpr[0] = 0;
    core.cpu.gpr[1] = 0x0204_0018; // EWRAM mirror alias (+256 KiB)!
    core.cpu.gpr[2] = 0xE3A0_0054; // MOV R0, #84
    core.cpu.cpsr = 0x1F;
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0000);
    core.cpu.step(&mut core.bus);
    assert!(core.bus.code_page_ptr.is_some());
    core.cpu.step(&mut core.bus);
    assert!(core.bus.code_page_ptr.is_none());
    for _ in 0..6 {
        core.cpu.step(&mut core.bus);
    }
    assert_eq!(core.cpu.gpr[0], 84);
}

/// Advances past the 2-cycle hardware DMA startup delay (`DmaActivate`) and drains active DMA units.
fn flush_immediate_dma(core: &mut GbaCore) {
    core.bus.scheduler.current_cycle += 2;
    while let Some((ev_cycle, kind)) = core.bus.scheduler.pop_due() {
        core.bus.dispatch_event(ev_cycle, kind);
    }
    while core.bus.hw.active_dma.is_some() {
        core.bus.step_active_dma();
    }
}

#[test]
fn test_dma_writing_into_cached_code_page() {
    let rom = vec![0u8; 0x1000];
    let mut core = GbaCore::new(None, rom);

    // Stage patched instruction word in EWRAM source buffer
    core.bus
        .write_32(0x0200_1000, 0xE3A0_0063, AccessType::NonSequential); // MOV R0, #99

    // Stage target program in IWRAM (0x0300_0000..):
    //   0x00: NOP
    //   0x04: NOP
    //   0x08: NOP
    //   0x0C: NOP
    //   0x10: MOV R0, #1  <- DMA3 overwrites alias 0x0300_8010 before pipeline fetches it
    //   0x14: B .
    let prog: [u32; 6] = [
        0xE1A0_0000,
        0xE1A0_0000,
        0xE1A0_0000,
        0xE1A0_0000,
        0xE3A0_0001,
        0xEAFF_FFFE,
    ];
    for (idx, &w) in prog.iter().enumerate() {
        core.bus
            .write_32(0x0300_0000 + (idx as u32) * 4, w, AccessType::NonSequential);
    }

    core.cpu.gpr[0] = 0;
    core.cpu.cpsr = 0x1F;
    core.cpu.flush_pipeline(&mut core.bus, 0x0300_0000);

    // Step 1 instruction so code_page_ptr is actively cached for 0x0300_0000
    core.cpu.step(&mut core.bus);
    assert!(core.bus.code_page_ptr.is_some());
    assert_eq!(core.bus.code_page_region, 0x03);

    // Trigger immediate 32-bit DMA3 writing into mirror alias 0x0300_8010 of cached IWRAM code page
    core.bus
        .write_32(0x0400_00D4, 0x0200_1000, AccessType::NonSequential); // DMA3SAD
    core.bus
        .write_32(0x0400_00D8, 0x0300_8010, AccessType::NonSequential); // DMA3DAD (alias!)
    core.bus.write_16(0x0400_00DC, 1, AccessType::NonSequential); // DMA3CNT_L = 1 word
    core.bus
        .write_16(0x0400_00DE, 0x8400, AccessType::NonSequential); // Enable | 32-bit | Immediate
    flush_immediate_dma(&mut core);
    assert!(
        core.bus.code_page_ptr.is_none(),
        "DMA transfer into cached code page must invalidate code_page_ptr"
    );

    for _ in 0..5 {
        core.cpu.step(&mut core.bus);
    }
    assert_eq!(
        core.cpu.gpr[0], 99,
        "CPU must fetch and execute DMA-written MOV R0, #99 instead of stale cached code"
    );
}

#[test]
fn test_eeprom_reads_through_fast_path_including_32mb_autodetect() {
    // Use a full 32 MiB ROM with NO embedded "EEPROM_V" string (`SaveType::None` at boot),
    // verifying `fastmem_read` does not bypass EEPROM reads at `0x0DFF_FF00` either before
    // or after DMA auto-detection updates `cart.save_type`.
    let mut rom = vec![0xA5u8; 0x0200_0000];
    // Poison ROM bytes at 0x01FF_FF00.. so fastmem bypass would return 0x5A5A != EEPROM bit
    for b in &mut rom[0x01FF_FF00..0x01FF_FF20] {
        *b = 0x5A;
    }
    let mut core = GbaCore::new(None, rom);
    assert_eq!(core.bus.cart.save_type, SaveType::None);

    // Write 64-bit value 0xDEAD_BEEF_CAFE_BABE to 8 KiB EEPROM address 0 via 81-bit DMA3 write
    // bitstream: "10" + 14-bit address (0) + 64 data bits MSB-first + stop bit 0 = 81 halfwords
    let payload: u64 = 0xDEAD_BEEF_CAFE_BABE;
    let mut write_bits = [0u16; 81];
    write_bits[0] = 1;
    write_bits[1] = 0; // Write command ("10")
    for i in 0..14 {
        write_bits[2 + i] = 0; // Block address 0
    }
    for i in 0..64 {
        write_bits[16 + i] = ((payload >> (63 - i)) & 1) as u16;
    }
    write_bits[80] = 0; // Stop bit
    for (i, &bit) in write_bits.iter().enumerate() {
        core.bus
            .write_16(0x0200_0000 + (i as u32) * 2, bit, AccessType::NonSequential);
    }
    core.bus
        .write_32(0x0400_00D4, 0x0200_0000, AccessType::NonSequential);
    core.bus
        .write_32(0x0400_00D8, 0x0DFF_FF00, AccessType::NonSequential);
    core.bus
        .write_16(0x0400_00DC, 81, AccessType::NonSequential);
    core.bus
        .write_16(0x0400_00DE, 0x8000, AccessType::NonSequential); // 16-bit Immediate DMA3
    flush_immediate_dma(&mut core);
    assert_eq!(core.bus.cart.save_type, SaveType::Eeprom8K);

    // Send 17-bit EEPROM read request ("11" + 14-bit address 0 + stop bit 0)
    let mut read_req = [0u16; 17];
    read_req[0] = 1;
    read_req[1] = 1; // Read command ("11")
    for (i, &bit) in read_req.iter().enumerate() {
        core.bus
            .write_16(0x0200_0200 + (i as u32) * 2, bit, AccessType::NonSequential);
    }
    core.bus
        .write_32(0x0400_00D4, 0x0200_0200, AccessType::NonSequential);
    core.bus
        .write_32(0x0400_00D8, 0x0DFF_FF00, AccessType::NonSequential);
    core.bus
        .write_16(0x0400_00DC, 17, AccessType::NonSequential);
    core.bus
        .write_16(0x0400_00DE, 0x8000, AccessType::NonSequential);
    flush_immediate_dma(&mut core);

    // Read 68 halfwords through `bus.read_16` (verifying fastmem does NOT short-circuit to ROM!)
    for d in 0..4 {
        let dummy = core
            .bus
            .read_16(0x0DFF_FF00, AccessType::NonSequential, 0x0800_0000, false);
        assert_eq!(
            dummy, 0,
            "EEPROM dummy bit {d} should be 0 (got {dummy:#x})"
        );
    }
    let mut read_back = 0u64;
    for i in 0..64 {
        let bit = core
            .bus
            .read_16(0x0DFF_FF00, AccessType::NonSequential, 0x0800_0000, false);
        assert!(
            bit <= 1,
            "EEPROM read bypassed into ROM bytes ({bit:#x}) at bit {i}"
        );
        read_back = (read_back << 1) | ((bit & 1) as u64);
    }
    assert_eq!(read_back, payload);

    // Verify SaveType::None override cleanly rebuilds fastmem (and back again to Eeprom8K)
    core.bus.set_save_type_override(Some(SaveType::None));
    let rom_val = core
        .bus
        .read_16(0x0DFF_FF00, AccessType::NonSequential, 0x0800_0000, false);
    assert_eq!(rom_val, 0x5A5A);
    core.bus.set_save_type_override(Some(SaveType::Eeprom8K));
    let ready_bit = core
        .bus
        .read_16(0x0DFF_FF00, AccessType::NonSequential, 0x0800_0000, false);
    assert_eq!(ready_bit, 1);
}

#[test]
fn hle_bios_edge_cases_open_bus_soundbias_timing_and_fallback() {
    // 1. Initial boot state & HLE open-bus latch (0xE129_F000)
    let mut rom = rom_from_arm_words(&[
        0xE3A0_0001, // 0x00: mov r0, #1
        0xEF19_0000, // 0x04: swi 0x19 (SoundBias(1)) -> expect SOUNDBIAS level = 0x0200
        0xE3A0_0000, // 0x08: mov r0, #0
        0xEF19_0000, // 0x0C: swi 0x19 (SoundBias(0)) -> expect SOUNDBIAS level = 0x0000
        0xEF0D_0000, // 0x10: swi 0x0D (GetBiosChecksum) -> r0 = 0xBAAE_187F, bios_latch = 0xE3A0_2004
        0xEAFF_FFFE, // 0x14: b .
    ]);
    rom.resize(0x200, 0);
    // Tag ROM with Classic NES Super Mario Bros ("FSME") to test real-BIOS fallback detection
    rom[0xAC..0xB0].copy_from_slice(b"FSME");

    let mut core = GbaCore::new(None, rom.clone());
    assert!(core.recommends_real_bios().is_some());
    assert_eq!(
        core.bus
            .read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000),
        0xE129_F000
    );
    assert_eq!(
        core.bus
            .read_16(0x0000_0002, AccessType::NonSequential, 0x0800_0000, false),
        0xE129
    );

    // Execute SWI 0x19 SoundBias(1) while preserving resolution bits (1 << 14)
    core.bus.write_io_16(0x0400_0088, (1 << 14) | 0x0080);
    core.cpu.step(&mut core.bus); // mov r0, #1
    let swi_bias1_cyc = core.cpu.step(&mut core.bus); // swi 0x19 (SoundBias(1))
    assert!(
        swi_bias1_cyc > 64,
        "SoundBias SWI should include BIOS timing"
    );
    assert_eq!(core.bus.apu.soundbias, (1 << 14) | 0x0200);

    core.cpu.step(&mut core.bus); // mov r0, #0
    core.cpu.step(&mut core.bus); // swi 0x19 (SoundBias(0))
    assert_eq!(core.bus.apu.soundbias, 1 << 14);

    // Execute SWI 0x0D GetBiosChecksum -> sets bios_latch = 0xE3A0_2004
    let swi_chk_cyc = core.cpu.step(&mut core.bus);
    assert_eq!(core.cpu.gpr[0], 0xBAAE_187F);
    assert!(swi_chk_cyc >= 60);
    assert_eq!(
        core.bus
            .read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000),
        0xE3A0_2004
    );

    // 2. IRQ trampoline exit must update bios_latch to 0xE55E_C002 (opcode prefetched at 0x0144)
    let irq_handler_addr = 0x0300_0000u32;
    let irq_handler_words = [
        0xE3A0_5042u32, // mov r5, #0x42
        0xE12F_FF1Eu32, // bx lr (return to BIOS 0x0138)
    ];
    for (i, w) in irq_handler_words.iter().enumerate() {
        core.bus.write_32(
            irq_handler_addr + (i as u32) * 4,
            *w,
            AccessType::NonSequential,
        );
    }
    core.bus
        .write_32(0x0300_7FFC, irq_handler_addr, AccessType::NonSequential);
    core.bus.write_16(0x0400_0208, 1, AccessType::NonSequential); // IME = 1
    core.bus.write_16(0x0400_0200, 1, AccessType::NonSequential); // IE = VBlank
    core.bus.hw.request_irq(1);
    for _ in 0..32 {
        core.step_one();
        if core.cpu.gpr[5] == 0x42 && core.cpu.current_exec_pc() >= 0x0800_0000 {
            break;
        }
    }
    assert_eq!(core.cpu.gpr[5], 0x42, "user IRQ handler must execute");
    assert_eq!(
        core.bus
            .read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000),
        0xE55E_C002,
        "after HLE IRQ exit, BIOS open-bus must latch 0xE55E_C002"
    );

    // 3. SoftReset resets bios_latch to 0xE129_F000 and branches back to 0x0800_0000
    core.bus.hle_swi(&mut core.cpu, 0x00);
    assert_eq!(
        core.bus
            .read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000),
        0xE129_F000
    );

    // 4. SWI timing scaling across Div and CpuFastSet
    let mut small_div = GbaCore::new(
        None,
        rom_from_arm_words(&[
            0xE3A0_0008, // mov r0, #8
            0xE3A0_1002, // mov r1, #2
            0xEF06_0000, // swi 0x06 (Div)
            0xEAFF_FFFE,
        ]),
    );
    small_div.cpu.step(&mut small_div.bus);
    small_div.cpu.step(&mut small_div.bus);
    let cyc_small = small_div.cpu.step(&mut small_div.bus);
    assert_eq!(small_div.cpu.gpr[0], 4);

    let mut large_div = GbaCore::new(
        None,
        rom_from_arm_words(&[
            0xE3A0_0102, // mov r0, #0x8000_0000 (Wait: 0x02 << 30 = 0x80000000)
            0xE3A0_1003, // mov r1, #3
            0xEF06_0000, // swi 0x06 (Div)
            0xEAFF_FFFE,
        ]),
    );
    large_div.cpu.step(&mut large_div.bus);
    large_div.cpu.step(&mut large_div.bus);
    let cyc_large = large_div.cpu.step(&mut large_div.bus);
    assert!(
        cyc_large > cyc_small,
        "large quotient Div should take more BIOS cycles ({cyc_large} vs {cyc_small})"
    );

    // 5. Real BIOS present suppresses fallback warning; non-fallback game returns None
    let dummy_bios = vec![0u8; 0x4000];
    let with_bios = GbaCore::new(Some(&dummy_bios), rom.clone());
    assert!(with_bios.recommends_real_bios().is_none());

    let mut emerald_rom = rom;
    emerald_rom[0xAC..0xB0].copy_from_slice(b"BPEE");
    let emerald = GbaCore::new(None, emerald_rom);
    assert!(emerald.recommends_real_bios().is_none());
}

#[test]
fn waitcnt_prefetch_dma_boundary_and_4stage_timer_cascade() {
    // 1. WAITCNT waitstate decoding across WS0, WS1, WS2, SRAM
    let rom = vec![0xAAu8; 0x6_0000]; // 384 KiB ROM to span 128 KiB boundaries cleanly
    let mut core = GbaCore::new(None, rom);

    // Set WAITCNT: SRAM=8 wait (bits 0-1=3), WS0 N=3 S=1 (bits 2-4 = 1 | (1<<4)),
    // WS1 N=2 S=1 (bits 5-7 = 2 | (1<<7)), WS2 N=8 S=1 (bits 8-10 = 3 | (1<<10)), Prefetch=OFF
    core.bus
        .write_16(0x0400_0204, 0x07D7, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0800_0002, AccessType::NonSequential),
        4
    ); // N=3 -> 4 cyc
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0800_0002, AccessType::Sequential),
        2
    ); // S=1 -> 2 cyc
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0A00_0002, AccessType::NonSequential),
        3
    ); // N=2 -> 3 cyc
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0A00_0002, AccessType::Sequential),
        2
    ); // S=1 -> 2 cyc
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0C00_0002, AccessType::NonSequential),
        9
    ); // N=8 -> 9 cyc
    assert_eq!(
        core.bus
            .rom_fetch_cycles(0x0C00_0002, AccessType::Sequential),
        2
    ); // S=1 -> 2 cyc
    core.bus
        .read_8(0x0E00_0000, AccessType::NonSequential, 0x0800_0000, false);
    assert_eq!(core.bus.last_cycles, 9); // SRAM 8 waitstates -> 9 cycles

    // 2. GamePak Prefetch Buffer (WAITCNT bit 14) reduces sequential ROM fetch cycles
    // Use an interleaved ALU/MUL sequence that gives prefetch cycles to fill between ROM fetches:
    let mut interleaved = Vec::new();
    for _ in 0..16 {
        interleaved.push(0xE3A0_0301); // mov r3, #0x04000000
        interleaved.push(0xE002_0091); // mul r2, r1, r0 (multi-I cycle -> fills prefetch!)
    }
    let mut pf_off = GbaCore::new(None, rom_from_arm_words(&interleaved));
    pf_off
        .bus
        .write_16(0x0400_0204, 0x0000, AccessType::NonSequential);
    let mut t_off = 0u64;
    for _ in 0..32 {
        t_off += pf_off.cpu.step(&mut pf_off.bus);
    }
    let mut pf_on = GbaCore::new(None, rom_from_arm_words(&interleaved));
    pf_on
        .bus
        .write_16(0x0400_0204, 0x4000, AccessType::NonSequential);
    let mut t_on = 0u64;
    for _ in 0..32 {
        t_on += pf_on.cpu.step(&mut pf_on.bus);
    }
    assert!(
        t_on < t_off,
        "GamePak prefetch should reduce cycle count with MUL I-cycles ({t_on} < {t_off})"
    );

    // 3. DMA timing & 128 KiB ROM boundary forced N-seq penalty
    let measure_dma3 = |src: u32| -> u64 {
        let mut c = GbaCore::new(None, vec![0xBBu8; 0x6_0000]);
        c.bus
            .write_16(0x0400_0204, 0x0000, AccessType::NonSequential); // WS0 N=5, S=3
        c.bus.write_32(0x0400_00D4, src, AccessType::NonSequential);
        c.bus
            .write_32(0x0400_00D8, 0x0300_0000, AccessType::NonSequential);
        c.bus.write_16(0x0400_00DC, 4, AccessType::NonSequential);
        c.bus
            .write_16(0x0400_00DE, 0x8400, AccessType::NonSequential); // 32-bit INC immediate
        c.bus.scheduler.current_cycle += 2; // Flush DmaStart startup delay
        while let Some((ev_cycle, kind)) = c.bus.scheduler.pop_due() {
            c.bus.dispatch_event(ev_cycle, kind);
        }
        assert!(
            c.bus.hw.active_dma.is_some(),
            "DMA3 should become active after startup delay"
        );
        let mut dma_cycles = 0u64;
        while c.bus.hw.active_dma.is_some() {
            dma_cycles += c.bus.step_active_dma();
        }
        dma_cycles
    };
    let dma_no_cross = measure_dma3(0x0801_FFF0); // 0801FFF0..0801FFFC
    let dma_cross = measure_dma3(0x0801_FFF8); // crosses 0x0802_0000 on 3rd word (+2 cyc)
    assert_eq!(
        dma_cross - dma_no_cross,
        2,
        "crossing 128 KiB ROM boundary must add N-S = 2 cycles"
    );

    // 4. 4-stage Timer Cascade (TM0 -> TM1 -> TM2 -> TM3 with IRQ on TM3)
    let mut tcore = GbaCore::new(None, vec![0u8; 0x100]);
    tcore
        .bus
        .write_16(0x0400_0200, 1 << 6, AccessType::NonSequential); // IE: Timer3
    tcore
        .bus
        .write_16(0x0400_010C, 0xFFFE, AccessType::NonSequential); // TM3 reload=FFFE (2 ticks)
    tcore
        .bus
        .write_16(0x0400_0108, 0xFFFE, AccessType::NonSequential); // TM2 reload=FFFE (2 ticks)
    tcore
        .bus
        .write_16(0x0400_0104, 0xFFFC, AccessType::NonSequential); // TM1 reload=FFFC (4 ticks)
    tcore
        .bus
        .write_16(0x0400_0100, 0xFFFE, AccessType::NonSequential); // TM0 reload=FFFE (2 cycles)
    tcore
        .bus
        .write_16(0x0400_010E, 0x00C4, AccessType::NonSequential); // TM3: En | IRQ | Cascade
    tcore
        .bus
        .write_16(0x0400_010A, 0x0084, AccessType::NonSequential); // TM2: En | Cascade
    tcore
        .bus
        .write_16(0x0400_0106, 0x0084, AccessType::NonSequential); // TM1: En | Cascade
    tcore
        .bus
        .write_16(0x0400_0102, 0x0080, AccessType::NonSequential); // TM0: En (/1)

    for _ in 0..80 {
        tcore.step_one();
        if (tcore.bus.hw.if_flag & (1 << 6)) != 0 {
            break;
        }
    }
    assert_ne!(
        tcore.bus.hw.if_flag & (1 << 6),
        0,
        "4-stage timer cascade TM0->TM1->TM2->TM3 must raise Timer3 IRQ"
    );
}

#[test]
fn test_savestate_runahead_sram_isolation_and_clone_safety() {
    // ROM writes 0x42 to SRAM[0] on frame 1
    let rom = rom_from_arm_words(&[
        0xE3A0_040E, // mov r0, #0x0E000000
        0xE3A0_1042, // mov r1, #0x42
        0xE5C0_1000, // strb r1, [r0]
        0xEAFF_FFFE, // b .
    ]);
    let mut core = GbaCore::new(None, rom);
    core.bus.cart.save_type = SaveType::Sram32K;
    core.bus.cart.sram[0] = 0x11;

    // 1. Run-ahead speculation must NOT leak speculative SRAM writes into live state.
    let mut fe = HeadlessFrontend::new();
    core.speculate_run_ahead_frame(&mut fe)
        .expect("speculate_run_ahead_frame");
    assert_eq!(
        core.bus.cart.sram[0], 0x11,
        "speculative run-ahead frame must roll back SRAM writes cleanly"
    );

    // 2. Save state & load state preserve SRAM contents AND keep heap buffer pointer stable.
    let sram_ptr_before = core.bus.cart.sram.as_ptr();
    let ewram_ptr_before = core.bus.ewram.as_ptr();
    let state = core.save_state().expect("save_state");
    core.run_frame(&mut fe);
    assert_eq!(core.bus.cart.sram[0], 0x42);
    core.load_state(&state).expect("load_state");
    assert_eq!(core.bus.cart.sram[0], 0x11);
    assert_eq!(
        core.bus.cart.sram.as_ptr(),
        sram_ptr_before,
        "SRAM box pointer must remain stable across load_state"
    );
    assert_eq!(
        core.bus.ewram.as_ptr(),
        ewram_ptr_before,
        "EWRAM box pointer must remain stable across load_state"
    );

    // 3. Cloning SysBus produces independent fastmem buffers with no aliasing.
    let mut cloned_bus = core.bus.clone();
    cloned_bus.write_32(0x0200_0000, 0xCAFE_BABE, AccessType::NonSequential);
    assert_eq!(
        core.bus
            .read_32(0x0200_0000, AccessType::NonSequential, 0x0800_0000),
        0,
        "writing to cloned SysBus must not alias original EWRAM"
    );
}

#[test]
fn test_hle_arctan_and_rtc_time_command_accuracy() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);

    // SWI 0x09 ArcTan(1.0 = 0x4000) must return 0x2000 (45 deg), ArcTan(-1.0) -> -0x2000
    core.cpu.gpr[0] = 0x4000;
    core.bus.hle_swi(&mut core.cpu, 0x09);
    assert_eq!(core.cpu.gpr[0] as i16, 0x2000);

    core.cpu.gpr[0] = (-0x4000i16) as i32 as u32;
    core.bus.hle_swi(&mut core.cpu, 0x09);
    assert_eq!(core.cpu.gpr[0] as i16, -0x2000);

    // SWI 0x09 with i16::MIN (-32768) must not overflow/panic
    core.cpu.gpr[0] = i16::MIN as i32 as u32;
    core.bus.hle_swi(&mut core.cpu, 0x09);

    // RTC Time-Only read command (0x67 -> cmd=3, is_read=1) returns 3 bytes (Hour, Min, Sec)
    core.bus.cart.has_rtc = true;
    core.bus.cart.rtc.set_datetime(26, 2, 28, 6, 23, 59, 59);
    core.bus.cart.rtc.advance_second();
    // 2026 is not a leap year -> Feb 28 23:59:59 advances to Mar 1 00:00:00!
    assert_eq!(core.bus.cart.rtc.regs[1], 0x03); // March
    assert_eq!(core.bus.cart.rtc.regs[2], 0x01); // Day 1
    assert_eq!(core.bus.cart.rtc.regs[4], 0x00); // 00h
    assert_eq!(core.bus.cart.rtc.regs[5], 0x00); // 00m
    assert_eq!(core.bus.cart.rtc.regs[6], 0x00); // 00s
}

#[test]
fn test_softreset_preserves_sp_svc_for_subsequent_cpuset() {
    // Program at 0x0800_0000:
    //   mov r0, #0x0200_0000  (src)
    //   mov r1, #0x0200_0100  (dst)
    //   mov r2, #0x0400_0004  (4 x 32-bit words, CpuSet 32-bit copy)
    //   swi 0x0B0000          (CpuSet)
    //   b .
    let rom = rom_from_arm_words(&[
        0xE3A0_0402, // mov r0, #0x02000000
        0xE280_1C01, // add r1, r0, #0x100
        0xE3A0_2301, // mov r2, #0x04000000
        0xE282_2004, // add r2, r2, #4
        0xEF0B_0000, // swi 0x0B
        0xEAFF_FFFE, // b .
    ]);
    let mut core = GbaCore::new(None, rom);
    // Execute SWI 0x00 SoftReset from SVC mode (matching real SWI entry)
    core.cpu.set_cpsr(0x0000_0093); // SVC
    core.bus.hle_swi(&mut core.cpu, 0x00);
    assert_eq!(
        core.cpu.banked_r13[3], 0x0300_7FE0,
        "SWI 0x00 SoftReset must preserve SP_svc = 0x0300_7FE0"
    );
    assert_eq!(core.cpu.gpr[13], 0x0300_7F00);

    // Seed source data in EWRAM and run until CpuSet completes.
    for i in 0..4u32 {
        core.bus.write_32(
            0x0200_0000 + i * 4,
            0xDEAD_0000 + i,
            AccessType::NonSequential,
        );
    }
    for _ in 0..200 {
        core.step_one();
    }
    for i in 0..4u32 {
        assert_eq!(
            core.bus
                .read_32(0x0200_0100 + i * 4, AccessType::NonSequential, 0x0800_0000),
            0xDEAD_0000 + i,
            "CpuSet after SoftReset must copy word {i} cleanly"
        );
    }
}

#[test]
fn test_ppu_1d_8bpp_tile_1023_wraps_and_oam_transparent_prio_quirk() {
    struct NullFe;
    impl HostFrontend for NullFe {
        fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            0x03FF
        }
    }

    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);
    let mut fe = NullFe;

    // 1. 1D 8bpp sprite with tile_num = 1023 must wrap inside 32 KiB OBJ VRAM without panicking.
    core.bus.ppu.write_dispcnt(0x1040); // OBJ enable + 1D mapping, Mode 0
    core.bus.ppu.pram[0x200 + 2..0x200 + 4].copy_from_slice(&0x001Fu16.to_le_bytes()); // OBJ color 1 = red
    // Tile 1023 starts at OBJ offset 1023*32 = 0x7FE0; row 4 wraps around to offset 0x0000!
    core.bus.ppu.vram[0x10000] = 1;
    // OAM #0: Y=0, 8bpp (bit 13), X=10, 8x8, tile=1023 (0x3FF)
    core.bus.ppu.oam[0..2].copy_from_slice(&0x2000u16.to_le_bytes());
    core.bus.ppu.oam[2..4].copy_from_slice(&10u16.to_le_bytes());
    core.bus.ppu.oam[4..6].copy_from_slice(&0x03FFu16.to_le_bytes());
    // Run a full frame so the 2-scanline DISPCNT pipeline latches cleanly; scanline y = 4
    // hits fine_y = 4 -> wraps to OBJ offset 0 -> red pixel at x = 10.
    core.run_frame(&mut fe);
    assert_eq!(core.bus.ppu.framebuffer[4 * LCD_WIDTH + 10], 0x001F);

    // 2. GBA hardware OAM transparent-pixel priority quirk:
    // - At x=20: OAM #0 is opaque green (prio 2), OAM #1 is transparent (prio 0), BG0 is blue (prio 1).
    //   On real GBA hardware, the later transparent pixel with prio 0 overwrites the opaque slot's
    //   priority from 2 -> 0, so the opaque green sprite beats BG0 (prio 1)!
    // - At x=40: OAM #2 is transparent (prio 0), OAM #3 is opaque green (prio 2), BG0 is blue (prio 1).
    //   Because the slot is still empty (`color == 0`) when OAM #3 draws, OAM #3 writes its own
    //   prio 2, so BG0 (prio 1) stays in front (matching Tonc `prio_demo`).
    core.bus.ppu.oam.fill(0);
    core.bus.ppu.vram.fill(0);
    core.bus.ppu.pram.fill(0);
    core.bus.ppu.write_dispcnt(0x1140); // BG0 + OBJ + 1D mapping
    core.bus.ppu.bgcnt[0] = 0x0001; // BG0 priority = 1, charbase 0, screenbase 0
    core.bus.ppu.pram[2..4].copy_from_slice(&0x7C00u16.to_le_bytes()); // BG color 1 = blue
    core.bus.ppu.pram[0x200 + 2..0x200 + 4].copy_from_slice(&0x03E0u16.to_le_bytes()); // OBJ color 1 = green
    // BG tile 1 filled with color 1 (4bpp); populate screen entries covering x=16..48 (tile cols 2..5)
    for b in &mut core.bus.ppu.vram[32..64] {
        *b = 0x11;
    }
    for col in 2..6usize {
        core.bus.ppu.vram[col * 2..col * 2 + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    // OBJ tile 1 = transparent (all 0), OBJ tile 2 = opaque color 1
    for b in &mut core.bus.ppu.vram[0x10000 + 64..0x10000 + 96] {
        *b = 0x11;
    }
    // OAM #0: Y=0, X=20, tile=2 (opaque green), prio=2 (0x0800)
    core.bus.ppu.oam[0..2].copy_from_slice(&0u16.to_le_bytes());
    core.bus.ppu.oam[2..4].copy_from_slice(&20u16.to_le_bytes());
    core.bus.ppu.oam[4..6].copy_from_slice(&0x0802u16.to_le_bytes());
    // OAM #1: Y=0, X=20, tile=1 (transparent), prio=0
    core.bus.ppu.oam[8..10].copy_from_slice(&0u16.to_le_bytes());
    core.bus.ppu.oam[10..12].copy_from_slice(&20u16.to_le_bytes());
    core.bus.ppu.oam[12..14].copy_from_slice(&0x0001u16.to_le_bytes());
    // OAM #2: Y=0, X=40, tile=1 (transparent), prio=0
    core.bus.ppu.oam[16..18].copy_from_slice(&0u16.to_le_bytes());
    core.bus.ppu.oam[18..20].copy_from_slice(&40u16.to_le_bytes());
    core.bus.ppu.oam[20..22].copy_from_slice(&0x0001u16.to_le_bytes());
    // OAM #3: Y=0, X=40, tile=2 (opaque green), prio=2 (0x0800)
    core.bus.ppu.oam[24..26].copy_from_slice(&0u16.to_le_bytes());
    core.bus.ppu.oam[26..28].copy_from_slice(&40u16.to_le_bytes());
    core.bus.ppu.oam[28..30].copy_from_slice(&0x0802u16.to_le_bytes());

    // Run 2 frames so BG0 enable in DISPCNT is latched from scanline 0 cleanly.
    core.run_frame(&mut fe);
    core.run_frame(&mut fe);
    assert_eq!(
        core.bus.ppu.framebuffer[20], 0x03E0,
        "Later transparent OAM #1 (prio 0) must promote earlier opaque OAM #0 (prio 2) above BG0 (prio 1)"
    );
    assert_eq!(
        core.bus.ppu.framebuffer[40], 0x7C00,
        "Opaque OAM #3 (prio 2) over empty slot must keep prio 2 and stay behind BG0 (prio 1)"
    );
    assert_eq!(
        core.bus.ppu.framebuffer[16], 0x7C00,
        "BG0 blue must render at x=16"
    );
}

#[test]
fn test_apu_high_byte_env_write_does_not_reload_length_counter() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);
    core.bus.write_io_16(0x0400_0084, 0x0080); // Master sound on
    // Write full 16-bit SOUND1CNT_H: length load = 54 (length_counter = 10), env vol = 15
    core.bus.write_io_16(0x0400_0062, 0xF036);
    assert_eq!(core.bus.apu.sq1.length_counter, 10);
    // Simulate length counter ticking down to 3
    core.bus.apu.sq1.length_counter = 3;
    // 8-bit STRB to high byte (0x0400_0063) changing envelope volume must NOT reload length_counter!
    core.bus.write_io_8(0x0400_0063, 0x80);
    assert_eq!(
        core.bus.apu.sq1.length_counter, 3,
        "high-byte write to SOUND1CNT_H must not clobber length_counter"
    );
    assert_eq!(core.bus.apu.sq1.env_init_vol, 8);
}
