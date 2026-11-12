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
    let mut core = GbaCore::new(None, rom);
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
    let mut core = GbaCore::new(None, rom);
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
    let mut core = GbaCore::new(None, rom);
    for _ in 0..10 {
        core.step_one();
    }
    assert_eq!(core.cpu.gpr[13], 0x0300_0000);
}

#[test]
fn ppu_mode3_mode4_and_top2_compositor_prio() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);

    // 1. Mode 3 (BG2 15bpp bitmap is completely opaque across 240x160)
    core.bus.write_io_16(0x0400_0000, 0x0403);
    core.bus.write_16(0x0600_0000, 0x001F, AccessType::NonSequential);
    core.bus.write_16(0x0500_0000, 0x03E0, AccessType::NonSequential);

    let mut fe = HeadlessFrontend::new();
    core.run_frame(&mut fe);
    assert_eq!(fe.frames, 1);
    assert_eq!(fe.last_fb[0], 0x001F); // Red pixel at (0,0)
    assert_eq!(fe.last_fb[1], 0x0000); // VRAM black pixel at (1,0)

    // 2. Mode 4 (BG2 8bpp bitmap uses palette idx 0 as transparent backdrop)
    core.bus.write_io_16(0x0400_0000, 0x0404);
    core.bus.write_16(0x0500_0002, 0x7C00, AccessType::NonSequential); // Palette idx 1 = blue
    core.bus.write_16(0x0600_0000, 0x0001, AccessType::NonSequential); // x=0 -> pal 1, x=1 -> pal 0
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
        core.bus.write_16(0x0700_0000 + idx * 8, 0x0200, AccessType::NonSequential);
    }
    // Screen entries for tile 1, palette 0
    core.bus.write_16(0x0600_0000, 0x0001, AccessType::NonSequential);
    core.bus.write_16(0x0600_0800, 0x0001, AccessType::NonSequential);
    // Tile 1 pixel (0,0) = color idx 2 (in VRAM charbase 0 + 32 bytes)
    core.bus.write_16(0x0600_0020, 0x0022, AccessType::NonSequential);
    core.bus.write_16(0x0500_0004, 0x001E, AccessType::NonSequential); // Color 2 = Red 30
    // Wait: make BG1 use palette 1 with Color idx 2 = Green 30
    core.bus.write_16(0x0600_0800, 0x1001, AccessType::NonSequential);
    core.bus.write_16(0x0500_0024, 0x03C0, AccessType::NonSequential); // Pal 1 col 2 = Green (30<<5)
    // Put an OBJ at (0,0) with priority 2 (blue)
    core.bus.write_16(0x0601_0000, 0x0011, AccessType::NonSequential); // OBJ tile 0 pix (0,0)=1
    core.bus.write_16(0x0500_0202, 0x7C00, AccessType::NonSequential); // OBJ pal 0 col 1 = Blue
    core.bus.write_16(0x0700_0000, 0x0000, AccessType::NonSequential); // y=0
    core.bus.write_16(0x0700_0002, 0x0000, AccessType::NonSequential); // x=0
    core.bus.write_16(0x0700_0004, 0x0800, AccessType::NonSequential); // prio=2, tile=0

    // Alpha blend BG0 (1st target) + BG1 (2nd target) at 50%/50% (EVA=8, EVB=8)
    core.bus.write_io_16(0x0400_0050, 0x0241); // mode 1, top=BG0 (bit 0), bot=BG1 (bit 9)
    core.bus.write_io_16(0x0400_0052, 0x0808); // EVA=8, EVB=8
    core.run_frame(&mut fe);
    // Expected: (30*8)>>4 = 15 red, (30*8)>>4 = 15 green -> 15 | (15<<5)
    assert_eq!(fe.last_fb[0], 15 | (15 << 5));
}

#[test]
fn memory_quirks_bios_lock_sram_mirror_and_vram_8bit() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);

    // BIOS read while PC >= 0x4000 returns bios_latch
    let bval = core.bus.read_32(0x0000_0000, AccessType::NonSequential, 0x0800_0000);
    assert_eq!(bval, core.bus.bios_latch);

    // OAM 8-bit write is ignored; 8-bit PRAM/VRAM writes duplicate byte
    core.bus.write_8(0x0700_0000, 0xAB, AccessType::NonSequential);
    assert_eq!(core.bus.ppu.oam[0], 0);
    core.bus.write_8(0x0500_0002, 0x5A, AccessType::NonSequential);
    assert_eq!(core.bus.read_16(0x0500_0002, AccessType::NonSequential, 0x0800_0000, false), 0x5A5A);

    // SRAM 32K mirror on BOTH read and write
    core.bus.cart.save_type = SaveType::Sram32K;
    core.bus.write_8(0x0E00_0123, 0x7E, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_8123, AccessType::NonSequential, 0x0800_0000, false), 0x7E);
}

#[test]
fn timers_cascade_and_hle_swi_div_sqrt_lz77() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);

    // Timer 0 reload 0xFFFE prescaler 1, Timer 1 cascade enabled
    core.bus.write_io_16(0x0400_0100, 0xFFFE);
    core.bus.write_io_16(0x0400_0104, 0x0000);
    core.bus.write_io_16(0x0400_0106, 0x0084); // enable + cascade
    core.bus.write_io_16(0x0400_0102, 0x0080); // enable T0

    let mut fe = HeadlessFrontend::new();
    core.run_frame(&mut fe);
    let t1_val = core.bus.read_io_16(0x0400_0104, 0x0800_0000, false);
    assert!(t1_val > 1000, "Cascaded Timer 1 should increment on Timer 0 overflows, got {t1_val}");

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
    let mut core = GbaCore::new(None, rom);
    assert_eq!(core.bus.cart.save_type, SaveType::Flash128K);

    // Enter ID mode
    core.bus.write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus.write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus.write_8(0x0E00_5555, 0x90, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_0000, AccessType::NonSequential, 0x0800_0000, false), 0x62);
    assert_eq!(core.bus.read_8(0x0E00_0001, AccessType::NonSequential, 0x0800_0000, false), 0x13);
    // Exit ID mode
    core.bus.write_8(0x0E00_0000, 0xF0, AccessType::NonSequential);

    // Switch to bank 1 & program byte
    core.bus.write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus.write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus.write_8(0x0E00_5555, 0xB0, AccessType::NonSequential);
    core.bus.write_8(0x0E00_0000, 0x01, AccessType::NonSequential);

    core.bus.write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus.write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus.write_8(0x0E00_5555, 0xA0, AccessType::NonSequential);
    core.bus.write_8(0x0E00_0042, 0x55, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_0042, AccessType::NonSequential, 0x0800_0000, false), 0x55);

    // Save state & verify fastmem + memory restore cleanly
    core.bus.write_32(0x0200_0100, 0xDEAD_BEEF, AccessType::NonSequential);
    let state = core.save_state().expect("serialize save state");
    core.bus.write_32(0x0200_0100, 0x0000_0000, AccessType::NonSequential);
    core.load_state(&state).expect("deserialize save state");
    assert_eq!(core.bus.read_32(0x0200_0100, AccessType::NonSequential, 0x0800_0000), 0xDEAD_BEEF);
}

struct AudioCaptureFrontend {
    samples: Vec<i16>,
}

impl AudioCaptureFrontend {
    fn new() -> Self {
        Self { samples: Vec::new() }
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
    let mut core = GbaCore::new(None, rom);

    // Enable Mode 0 + BG0
    core.bus.write_io_16(0x0400_0000, 0x0100);
    core.bus.write_io_16(0x0400_0008, 0x0000);
    core.bus.write_16(0x0500_0000, 0x7C00, AccessType::NonSequential); // Backdrop = blue
    core.bus.write_16(0x0500_0002, 0x001F, AccessType::NonSequential); // Pal 0 col 1 = red
    core.bus.write_16(0x0600_0000, 0x0001, AccessType::NonSequential); // Map tile 1 at (0,0)
    core.bus.write_16(0x0600_0020, 0x0011, AccessType::NonSequential); // Tile 1 pixels 0,1 = col 1

    let mut fe_scanline = HeadlessFrontend::new();
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
    let mut core = GbaCore::new(None, rom);

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
    let mut core = GbaCore::new(None, rom);

    // SaveType::None override blocks SRAM writes (buffer stays erased 0xFF)
    core.bus.cart.set_save_type_override(Some(SaveType::None));
    core.bus.write_8(0x0E00_0010, 0xA5, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_0010, AccessType::NonSequential, 0x0800_0000, false), 0xFF);

    // Override to Flash64K and program one byte
    core.bus.cart.set_save_type_override(Some(SaveType::Flash64K));
    assert_eq!(core.bus.cart.effective_save_type(), SaveType::Flash64K);
    core.bus.write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
    core.bus.write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
    core.bus.write_8(0x0E00_5555, 0xA0, AccessType::NonSequential);
    core.bus.write_8(0x0E00_0010, 0x5A, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_0010, AccessType::NonSequential, 0x0800_0000, false), 0x5A);

    // Yoshi Tiled / Topsy-Turvy Tilt Sensor
    core.bus.cart.tilt_enabled = true;
    core.bus.cart.tilt_x = 0x03A5;
    core.bus.cart.tilt_y = 0x02B4;
    core.bus.write_8(0x0E00_8000, 0x55, AccessType::NonSequential);
    core.bus.write_8(0x0E00_8100, 0xAA, AccessType::NonSequential);
    assert_eq!(core.bus.read_8(0x0E00_8200, AccessType::NonSequential, 0x0800_0000, false), 0xA5);
    assert_eq!(core.bus.read_8(0x0E00_8300, AccessType::NonSequential, 0x0800_0000, false), 0x83);
    assert_eq!(core.bus.read_8(0x0E00_8400, AccessType::NonSequential, 0x0800_0000, false), 0xB4);
    assert_eq!(core.bus.read_8(0x0E00_8500, AccessType::NonSequential, 0x0800_0000, false), 0x02);
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

    // WarioWare Twisted Gyro + Rumble (GPIO)
    core.bus.cart.rumble_enabled = true;
    core.bus.cart.gyro_enabled = true;
    core.bus.cart.gyro_val = 0x0A53; // 12-bit value
    core.bus.cart.write_rom_16(0x0800_00C6, 0x000B); // bit0=CLK, bit1=SAMPLE, bit2=in, bit3=RUMBLE
    core.bus.cart.write_rom_16(0x0800_00C4, 0x0008); // Rumble ON
    assert!(core.bus.cart.rumble_active);
    core.bus.cart.write_rom_16(0x0800_00C4, 0x0003); // Sample=1, CLK=1 -> latches gyro_val << 4
    let mut shifted = 0u16;
    for _ in 0..12 {
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0001); // CLK=1, Sample=0
        core.bus.cart.write_rom_16(0x0800_00C4, 0x0000); // Falling edge shifts MSB onto pin 2
        let bit = (core.bus.cart.read_rom_16(0x0800_00C4) >> 2) & 1;
        shifted = (shifted << 1) | bit;
    }
    assert_eq!(shifted, 0x0A53);
}

fn setup_dma1_fifo_a_sine(core: &mut GbaCore, ewram_bytes: usize, amplitude: f64, period_samples: f64) {
    for i in 0..ewram_bytes {
        let angle = core::f64::consts::TAU * (i as f64) / period_samples;
        let sample = (angle.sin() * amplitude).round().clamp(-127.0, 127.0) as i8;
        core.bus
            .write_8(0x0200_0000 + i as u32, sample as u8, AccessType::NonSequential);
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
            let w = 0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos() - 0.01168 * (3.0 * p).cos();
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
    let mut core = GbaCore::new(None, rom);

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
        for pair in fe.samples.chunks_exact(2) {
            if mono.len() < 2048 {
                mono.push(pair[0] as f64);
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

    assert!(fund_power > 1e10, "expected strong 10.92 kHz fundamental, got {fund_power}");
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

    let expected_pairs =
        (gba_core::CYCLES_PER_FRAME as f64) * (gba_core::SAMPLE_RATE_HZ as f64)
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
            for pair in fe.samples.chunks_exact(2) {
                raw_mono.push(pair[0] as f64);
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
            for pair in fe.samples.chunks_exact(2) {
                interp_mono.push(pair[0] as f64);
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
    let mut core = GbaCore::new(None, rom);

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
    let mut core = GbaCore::new(None, rom);
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
    let peak_100 = fe.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);

    core.bus.write_io_16(0x0400_0082, 0x0302); // DMA A = 50% (bit 2 = 0)
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_50 = fe.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(
        peak_100 > peak_50 && peak_50 > 0 && peak_50 * 2 <= peak_100 + 200,
        "expected 50% DMA A ratio (~half of 100%), peak_100={peak_100}, peak_50={peak_50}"
    );
}

#[test]
fn apu_save_state_roundtrip_does_not_panic() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);
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

    let saved = core.save_state().expect("serialize GBA state with active APU");

    // Mutate state significantly before restoring
    core.bus.write_io_16(0x0400_0084, 0x0000);
    core.bus.apu.fifo_interpolation = false;
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }

    core.load_state(&saved).expect("deserialize GBA state with active APU");
    assert!(
        core.bus.apu.fifo_interpolation,
        "fifo_interpolation flag should persist across save state roundtrip"
    );

    fe.samples.clear();
    core.run_frame(&mut fe);
    let expected_pairs =
        (gba_core::CYCLES_PER_FRAME as f64) * (gba_core::SAMPLE_RATE_HZ as f64)
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
    let mut core = GbaCore::new(None, rom);

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
    let peak_mid_bias = fe.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);

    // Raise SOUNDBIAS level to near top (0x03F0 = 1008): positive half-cycle clips heavily at 1023
    core.bus.write_io_16(0x0400_0088, 0x03F0);
    for _ in 0..4 {
        fe.samples.clear();
        core.run_frame(&mut fe);
    }
    let peak_clipped_bias = fe.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(
        peak_mid_bias > peak_clipped_bias * 13 / 10 && peak_clipped_bias > 0,
        "expected 10-bit saturation clamp at high SOUNDBIAS to reduce swing: mid={peak_mid_bias}, clipped={peak_clipped_bias}"
    );
}


