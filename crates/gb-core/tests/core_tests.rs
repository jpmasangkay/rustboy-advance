use std::fs;
use std::path::{Path, PathBuf};

use gb_core::{
    CYCLES_PER_FRAME, CartridgeHeader, DmgPalette, GbConfig, GbCore, GbHostFrontend, GbModel,
    KEY_A, KEY_DOWN, KEY_RIGHT, KEY_START, LCD_HEIGHT, LCD_WIDTH, encode_keyinput,
};
use image::{ImageBuffer, Rgb};

struct TestFrontend {
    frames: usize,
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    audio_samples: Vec<i16>,
    keypad: u16,
}

impl TestFrontend {
    fn new() -> Self {
        Self {
            frames: 0,
            last_fb: [0; LCD_WIDTH * LCD_HEIGHT],
            audio_samples: Vec::new(),
            keypad: 0x03FF,
        }
    }
}

impl GbHostFrontend for TestFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.frames += 1;
        self.last_fb.copy_from_slice(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_samples.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad
    }
}

fn make_test_rom(
    program: &[u8],
    title: &str,
    cgb_flag: u8,
    cart_type: u8,
    rom_banks: usize,
    ram_code: u8,
) -> Vec<u8> {
    let size = (rom_banks.max(2)) * 0x4000;
    let mut rom = vec![0x00; size];
    // Entry point at 0x0100: JP 0x0150
    rom[0x0100] = 0xC3;
    rom[0x0101] = 0x50;
    rom[0x0102] = 0x01;

    let title_bytes = title.as_bytes();
    let max_title = if cgb_flag == 0x80 || cgb_flag == 0xC0 {
        15
    } else {
        16
    };
    let copy_len = title_bytes.len().min(max_title);
    rom[0x0134..0x0134 + copy_len].copy_from_slice(&title_bytes[..copy_len]);
    if cgb_flag != 0 {
        rom[0x0143] = cgb_flag;
    }
    rom[0x0147] = cart_type;
    let rom_code = (rom_banks.max(2) / 2).trailing_zeros() as u8;
    rom[0x0148] = rom_code;
    rom[0x0149] = ram_code;

    let mut chk = 0u8;
    for b in &rom[0x0134..=0x014C] {
        chk = chk.wrapping_sub(*b).wrapping_sub(1);
    }
    rom[0x014D] = chk;

    let prog_end = (0x0150 + program.len()).min(rom.len());
    rom[0x0150..prog_end].copy_from_slice(&program[..prog_end - 0x0150]);
    rom
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn bgr555_to_rgb8(c: u16) -> [u8; 3] {
    let r5 = (c & 0x1F) as u8;
    let g5 = ((c >> 5) & 0x1F) as u8;
    let b5 = ((c >> 10) & 0x1F) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g5 << 3) | (g5 >> 2),
        (b5 << 3) | (b5 >> 2),
    ]
}

fn fnv1a_u16_slice(data: &[u16]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &v in data {
        for b in v.to_le_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn save_fb_png(path: &Path, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut img = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(LCD_WIDTH as u32, LCD_HEIGHT as u32);
    for y in 0..LCD_HEIGHT {
        for x in 0..LCD_WIDTH {
            img.put_pixel(
                x as u32,
                y as u32,
                Rgb(bgr555_to_rgb8(fb[y * LCD_WIDTH + x])),
            );
        }
    }
    img.save(path).expect("failed to save test screenshot");
}

#[test]
fn header_parsing_and_post_boot_gba_mode() {
    let dmg_rom = make_test_rom(&[0x18, 0xFE], "TETRIS", 0x00, 0x00, 2, 0x00);
    let dmg_hdr = CartridgeHeader::parse(&dmg_rom);
    assert_eq!(dmg_hdr.title, "TETRIS");
    assert!(!dmg_hdr.supports_cgb);
    assert!(dmg_hdr.header_checksum_valid);

    let dmg = GbCore::new(dmg_rom, None, GbModel::Auto);
    assert!(!dmg.is_cgb());
    assert_eq!(dmg.title(), "TETRIS");
    assert_eq!(dmg.cpu.a, 0x01);
    assert_eq!(dmg.cpu.pc, 0x0100);

    let cgb_rom = make_test_rom(&[0x18, 0xFE], "ZELDA", 0xC0, 0x1B, 4, 0x03);
    let core_gba = GbCore::new(cgb_rom.clone(), None, GbModel::Auto);
    assert!(core_gba.is_cgb());
    assert!(core_gba.gba_mode());
    assert_eq!(core_gba.cpu.a, 0x11);
    assert_eq!(core_gba.cpu.b & 0x01, 0x01);

    let core_gbc = GbCore::new_with_config(
        cgb_rom,
        None,
        GbConfig {
            model: GbModel::Auto,
            gba_mode: false,
            ..GbConfig::default()
        },
    );
    assert!(core_gbc.is_cgb());
    assert!(!core_gbc.gba_mode());
    assert_eq!(core_gbc.cpu.a, 0x11);
    assert_eq!(core_gbc.cpu.b & 0x01, 0x00);
}

#[test]
fn mbc1_mbc2_mbc3_rtc_and_mbc5_rumble() {
    // MBC1 mode 1 banking
    let mut rom1 = make_test_rom(&[0x00], "MBC1TEST", 0x00, 0x03, 64, 0x03);
    // Mark bank 0x20 (for mode 1 0x0000 window) and bank 0x23 (for 0x4000 window)
    rom1[0x20 * 0x4000] = 0xAA;
    rom1[0x23 * 0x4000] = 0x55;
    let mut c1 = GbCore::new(rom1, None, GbModel::Dmg);
    c1.bus.write8_immediate(0x0000, 0x0A); // enable RAM
    c1.bus.write8_immediate(0x2000, 0x03); // lo bank 3
    c1.bus.write8_immediate(0x4000, 0x01); // hi bank 1 -> 0x20 / 0x23
    c1.bus.write8_immediate(0x6000, 0x01); // mode 1
    assert_eq!(c1.peek8(0x0000), 0xAA);
    assert_eq!(c1.peek8(0x4000), 0x55);
    c1.bus.write8_immediate(0xA000, 0x42);
    assert_eq!(c1.peek8(0xA000), 0x42);

    // MBC2 4-bit RAM
    let rom2 = make_test_rom(&[0x00], "MBC2TEST", 0x00, 0x06, 4, 0x00);
    let mut c2 = GbCore::new(rom2, None, GbModel::Dmg);
    c2.bus.write8_immediate(0x0000, 0x0A); // bit 8 clear -> enable RAM
    c2.bus.write8_immediate(0xA000, 0xAB);
    assert_eq!(c2.peek8(0xA000), 0xFB);
    assert_eq!(c2.battery_ram().len(), 512);

    // MBC3 + RTC + 48-byte save trailer
    let rom3 = make_test_rom(&[0x00], "POKEMON", 0x80, 0x10, 4, 0x03);
    let mut c3 = GbCore::new(rom3.clone(), None, GbModel::Auto);
    c3.bus.write8_immediate(0x0000, 0x0A);
    c3.bus.write8_immediate(0xA000, 0x77);
    c3.bus.write8_immediate(0x4000, 0x08); // RTC sec
    c3.bus.write8_immediate(0xA000, 12);
    c3.advance_rtc_seconds(5);
    c3.bus.write8_immediate(0x6000, 0x00);
    c3.bus.write8_immediate(0x6000, 0x01);
    assert_eq!(c3.bus.read8_immediate(0xA000), 17);
    let sav = c3.battery_ram();
    assert_eq!(sav.len(), 0x8000 + 48);

    let mut c3_loaded = GbCore::new(rom3, None, GbModel::Auto);
    c3_loaded.load_battery_ram(&sav);
    c3_loaded.bus.write8_immediate(0x0000, 0x0A);
    c3_loaded.bus.write8_immediate(0x4000, 0x08);
    assert_eq!(c3_loaded.bus.read8_immediate(0xA000), 17);
    c3_loaded.bus.write8_immediate(0x4000, 0x00);
    assert_eq!(c3_loaded.peek8(0xA000), 0x77);

    // MBC5 + Rumble + bank 0 selectable at 0x4000
    let mut rom5 = make_test_rom(&[0x00], "RUMBLE", 0x80, 0x1E, 4, 0x03);
    rom5[0x0005] = 0x99;
    let mut c5 = GbCore::new(rom5, None, GbModel::Auto);
    c5.bus.write8_immediate(0x2000, 0x00);
    assert_eq!(c5.peek8(0x4005), 0x99);
    assert!(!c5.is_rumble_active());
    c5.bus.write8_immediate(0x4000, 0x08);
    assert!(c5.is_rumble_active());
}

#[test]
fn cpu_ei_delay_halt_bug_and_cgb_speed_switch() {
    // EI delay + EI/DI cancellation + HALT bug
    // 0x0150:
    //   FB       ; EI
    //   F3       ; DI (cancels EI!)
    //   00       ; NOP
    //   76       ; HALT (with IF=0x01, IE=0x01, IME=0 -> triggers HALT bug!)
    //   3C       ; INC A (executed TWICE due to HALT bug!)
    //   18 FE    ; JR .
    let prog = [0xFB, 0xF3, 0x00, 0x76, 0x3C, 0x18, 0xFE];
    let rom = make_test_rom(&prog, "CPUTEST", 0x80, 0x00, 2, 0x00);
    let mut core = GbCore::new(rom, None, GbModel::Auto);
    core.bus.ie_flag = 0x01;
    core.bus.if_flag = 0x01;
    let start_a = core.cpu.a; // 0x11
    for _ in 0..7 {
        core.step_one();
    }
    assert!(!core.cpu.ime);
    assert_eq!(core.cpu.a, start_a.wrapping_add(2));

    // CGB double-speed switch via KEY1 + STOP
    // 0x0150:
    //   3E 01    ; LD A, 0x01
    //   E0 4D    ; LDH (KEY1), A
    //   10 00    ; STOP
    //   18 FE    ; JR .
    let speed_prog = [0x3E, 0x01, 0xE0, 0x4D, 0x10, 0x00, 0x18, 0xFE];
    let srom = make_test_rom(&speed_prog, "SPEED", 0x80, 0x00, 2, 0x00);
    let mut sc = GbCore::new(srom, None, GbModel::Cgb);
    assert!(!sc.bus.double_speed);
    for _ in 0..4 {
        sc.step_one();
    }
    assert!(sc.bus.double_speed);
    assert!(!sc.cpu.stopped);
    let mut fe = TestFrontend::new();
    let dots_before = sc.bus.total_normal_dots;
    sc.run_frame(&mut fe);
    assert!(sc.bus.total_normal_dots - dots_before >= CYCLES_PER_FRAME);
}

#[test]
fn keypad_gba_layout_save_state_peek_poke_and_apu_rate() {
    let rom = make_test_rom(&[0x00, 0x18, 0xFD], "APUTEST", 0x00, 0x00, 2, 0x00);
    let mut core = GbCore::new(rom, None, GbModel::Dmg);
    let mut fe = TestFrontend::new();
    fe.keypad = encode_keyinput(KEY_A | KEY_START | KEY_RIGHT | KEY_DOWN);

    core.run_frame(&mut fe);
    assert_eq!(fe.frames, 1);
    // 70224 / 4194304 * 48000 = ~803.65 stereo samples => ~1606-1608 i16 values
    assert!((1590..=1620).contains(&fe.audio_samples.len()));

    // Verify GBA keypad mapping into P1 (0xFF00)
    core.bus.write8_immediate(0xFF00, 0x20); // select D-pad (bit 4 == 0)
    assert_eq!(core.peek8(0xFF00) & 0x0F, 0x0F & !(0x01 | 0x08)); // Right + Down
    core.bus.write8_immediate(0xFF00, 0x10); // select Action (bit 5 == 0)
    assert_eq!(core.peek8(0xFF00) & 0x0F, 0x0F & !(0x01 | 0x08)); // A + Start

    // Side-effect-free peek/poke
    core.poke8(0xC000, 0x5A);
    assert_eq!(core.peek8(0xC000), 0x5A);

    // Save state excludes ROM and restores cleanly
    let big_rom = make_test_rom(&[0x18, 0xFE], "BIGSAVE", 0x80, 0x1B, 64, 0x03);
    let rom_len = big_rom.len();
    let mut big_core = GbCore::new(big_rom, None, GbModel::Auto);
    big_core.poke8(0xD010, 0xBE);
    let state = big_core.save_state().expect("save_state failed");
    assert!(
        state.len() < rom_len / 4 && state.len() < 200 * 1024,
        "state unexpectedly large: {} (rom_len: {})",
        state.len(),
        rom_len
    );
    big_core.poke8(0xD010, 0x00);
    big_core.load_state(&state).expect("load_state failed");
    assert_eq!(big_core.peek8(0xD010), 0xBE);

    // Configurable sample rate + DMG palette
    core.set_sample_rate(44_100);
    assert_eq!(core.sample_rate(), 44_100);
    core.set_dmg_palette(DmgPalette::GbaColorized);
    assert_eq!(core.dmg_palette(), DmgPalette::GbaColorized);
}

fn run_blargg_serial_test(rel_path: &str, model: GbModel, max_frames: usize) {
    let rom_path = workspace_root().join("test-roms").join("gb").join(rel_path);
    if !rom_path.exists() {
        eprintln!("Skipping missing test ROM: {}", rom_path.display());
        return;
    }
    let rom = fs::read(&rom_path).expect("failed to read test ROM");
    let mut core = GbCore::new(rom, None, model);
    let mut fe = TestFrontend::new();

    for frame in 0..max_frames {
        core.run_frame(&mut fe);
        let text = core.serial_output_str();
        if text.contains("Passed") {
            eprintln!(
                "[{rel_path} @ {model:?}] passed in {} frames:\n{}",
                frame + 1,
                text.trim()
            );
            return;
        }
        assert!(
            !text.contains("Failed"),
            "{rel_path} failed! Serial output:\n{text}"
        );
    }

    let text = core.serial_output_str();
    panic!("{rel_path} timed out after {max_frames} frames. Serial output:\n{text}");
}

fn run_blargg_cart_ram_test(rel_path: &str, model: GbModel, max_frames: usize) {
    let rom_path = workspace_root().join("test-roms").join("gb").join(rel_path);
    if !rom_path.exists() {
        eprintln!("Skipping missing test ROM: {}", rom_path.display());
        return;
    }
    let rom = fs::read(&rom_path).expect("failed to read test ROM");
    let mut core = GbCore::new(rom, None, model);
    let mut fe = TestFrontend::new();

    let mut saw_magic = false;
    for frame in 0..max_frames {
        core.run_frame(&mut fe);
        let ram = &core.bus.cart.ram;
        if ram.len() >= 16 && ram[1] == 0xDE && ram[2] == 0xB0 && ram[3] == 0x61 {
            saw_magic = true;
            let status = ram[0];
            if status != 0x80 {
                let end = ram[4..]
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(ram.len() - 4);
                let text = String::from_utf8_lossy(&ram[4..4 + end]).into_owned();
                assert_eq!(
                    status, 0x00,
                    "{rel_path} failed with status 0x{status:02X}! Output:\n{text}"
                );
                assert!(
                    text.contains("Passed"),
                    "{rel_path} unexpected output:\n{text}"
                );
                eprintln!(
                    "[{rel_path} @ {model:?}] passed in {} frames:\n{}",
                    frame + 1,
                    text.trim()
                );
                return;
            }
        }
    }
    panic!("{rel_path} timed out after {max_frames} frames (saw_magic={saw_magic})");
}

#[test]
fn blargg_cpu_instrs() {
    run_blargg_serial_test("cpu_instrs/cpu_instrs.gb", GbModel::Auto, 4000);
}

#[test]
fn blargg_cpu_instrs_dmg() {
    run_blargg_serial_test("cpu_instrs/cpu_instrs.gb", GbModel::Dmg, 4000);
}

#[test]
fn blargg_instr_timing() {
    run_blargg_serial_test("instr_timing/instr_timing.gb", GbModel::Auto, 800);
    run_blargg_serial_test("instr_timing/instr_timing.gb", GbModel::Dmg, 800);
}

#[test]
fn blargg_mem_timing() {
    run_blargg_serial_test("mem_timing/mem_timing.gb", GbModel::Auto, 800);
    run_blargg_serial_test("mem_timing/mem_timing.gb", GbModel::Dmg, 800);
}

#[test]
fn blargg_mem_timing_2() {
    run_blargg_cart_ram_test("mem_timing-2/mem_timing.gb", GbModel::Auto, 800);
    run_blargg_cart_ram_test("mem_timing-2/mem_timing.gb", GbModel::Dmg, 800);
}

#[test]
fn dmg_acid2_visual_and_reference() {
    let root = workspace_root();
    let rom_path = root.join("test-roms/gb/dmg-acid2.gb");
    if !rom_path.exists() {
        eprintln!("Skipping missing test ROM: {}", rom_path.display());
        return;
    }

    let rom = fs::read(&rom_path).expect("failed to read dmg-acid2.gb");
    let mut core = GbCore::new(rom, None, GbModel::Dmg);
    let mut fe = TestFrontend::new();
    for _ in 0..120 {
        core.run_frame(&mut fe);
    }

    let out_png = root.join("test_output/dmg-acid2.png");
    save_fb_png(&out_png, &fe.last_fb);

    let hash = fnv1a_u16_slice(&fe.last_fb);
    eprintln!("dmg-acid2 framebuffer FNV-1a hash: 0x{hash:016X}");

    let ref_path = root.join("test-roms/gb/dmg-acid2-reference.png");
    if ref_path.exists() {
        let ref_img = image::open(&ref_path)
            .expect("failed to decode dmg-acid2-reference.png")
            .to_rgb8();
        assert_eq!(ref_img.dimensions(), (LCD_WIDTH as u32, LCD_HEIGHT as u32));

        let shade_of_bgr555 = |c: u16| -> u8 {
            match c {
                0x7FFF => 0,
                0x56B5 => 1,
                0x294A => 2,
                0x0000 => 3,
                _ => 255,
            }
        };
        let shade_of_ref = |p: &Rgb<u8>| -> u8 {
            match p.0[0] {
                0xFF => 0,
                0xAA => 1,
                0x55 => 2,
                0x00 => 3,
                other => panic!("unexpected reference greyscale value: {other}"),
            }
        };

        let mut mismatches = 0usize;
        for y in 0..LCD_HEIGHT {
            for x in 0..LCD_WIDTH {
                let got = shade_of_bgr555(fe.last_fb[y * LCD_WIDTH + x]);
                let exp = shade_of_ref(ref_img.get_pixel(x as u32, y as u32));
                if got != exp {
                    mismatches += 1;
                }
            }
        }
        assert_eq!(
            mismatches,
            0,
            "dmg-acid2 had {mismatches} mismatched pixels vs reference (dumped {})",
            out_png.display()
        );
    }
}

#[test]
fn cgb_acid2_visual_and_reference() {
    let root = workspace_root();
    let rom_path = root.join("test-roms/gb/cgb-acid2.gbc");
    if !rom_path.exists() {
        eprintln!("Skipping missing test ROM: {}", rom_path.display());
        return;
    }

    let rom = fs::read(&rom_path).expect("failed to read cgb-acid2.gbc");
    let mut core = GbCore::new(rom, None, GbModel::Auto);
    let mut fe = TestFrontend::new();
    for _ in 0..120 {
        core.run_frame(&mut fe);
    }

    let out_png = root.join("test_output/cgb-acid2.png");
    save_fb_png(&out_png, &fe.last_fb);

    let hash = fnv1a_u16_slice(&fe.last_fb);
    eprintln!("cgb-acid2 framebuffer FNV-1a hash: 0x{hash:016X}");

    let ref_path = root.join("test-roms/gb/cgb-acid2-reference.png");
    if ref_path.exists() {
        let ref_img = image::open(&ref_path)
            .expect("failed to decode cgb-acid2-reference.png")
            .to_rgb8();
        assert_eq!(ref_img.dimensions(), (LCD_WIDTH as u32, LCD_HEIGHT as u32));

        let mut mismatches = 0usize;
        for y in 0..LCD_HEIGHT {
            for x in 0..LCD_WIDTH {
                let got = bgr555_to_rgb8(fe.last_fb[y * LCD_WIDTH + x]);
                let exp = ref_img.get_pixel(x as u32, y as u32).0;
                if got != exp {
                    mismatches += 1;
                }
            }
        }
        assert_eq!(
            mismatches,
            0,
            "cgb-acid2 had {mismatches} mismatched pixels vs reference (dumped {})",
            out_png.display()
        );
    }
}

#[test]
fn cgb_banking_hdma_stat_blocking_apu_and_reset() {
    let mut rom = make_test_rom(&[0x18, 0xFE], "CGBSYS", 0xC0, 0x1B, 4, 0x03);
    // Source data at 0x2000 for GDMA/HDMA
    for i in 0..32u8 {
        rom[0x2000 + (i as usize)] = i.wrapping_mul(3).wrapping_add(1);
    }
    let mut core = GbCore::new(rom, None, GbModel::Auto);

    // WRAM SVBK banking (0 maps to 1, banks 1..=7 independent)
    core.bus.write8_immediate(0xFF70, 0x00);
    core.bus.write8_immediate(0xD000, 0x11);
    core.bus.write8_immediate(0xFF70, 0x01);
    assert_eq!(core.peek8(0xD000), 0x11);
    core.bus.write8_immediate(0xFF70, 0x05);
    core.bus.write8_immediate(0xD000, 0x55);
    assert_eq!(core.peek8(0xD000), 0x55);
    core.bus.write8_immediate(0xFF70, 0x01);
    assert_eq!(core.peek8(0xD000), 0x11);

    // VRAM VBK banking + CGB palette auto-increment
    core.bus.write8_immediate(0xFF4F, 0x01);
    core.bus.write8_immediate(0x8000, 0xA5);
    core.bus.write8_immediate(0xFF4F, 0x00);
    core.bus.write8_immediate(0x8000, 0x5A);
    assert_eq!(core.peek8(0x8000), 0x5A);
    core.bus.write8_immediate(0xFF4F, 0x01);
    assert_eq!(core.peek8(0x8000), 0xA5);

    core.bus.write8_immediate(0xFF68, 0x80); // BCPS index 0 + auto-inc
    core.bus.write8_immediate(0xFF69, 0x34);
    core.bus.write8_immediate(0xFF69, 0x12);
    assert_eq!(core.peek8(0xFF68) & 0x3F, 2);

    // GDMA: copy 16 bytes from 0x2000 to VRAM 0x8100 (bank 1)
    core.bus.write8_immediate(0xFF51, 0x20);
    core.bus.write8_immediate(0xFF52, 0x00);
    core.bus.write8_immediate(0xFF53, 0x81);
    core.bus.write8_immediate(0xFF54, 0x00);
    core.bus.write8_immediate(0xFF55, 0x00); // 1 block GDMA
    assert_eq!(core.peek8(0x8100), 1);
    assert_eq!(core.peek8(0x810F), 15 * 3 + 1);
    assert_eq!(core.peek8(0xFF55), 0xFF);

    // STAT blocking: LY==LYC already true at LY=0, LYC=0
    core.bus.if_flag = 0;
    core.bus.write8_immediate(0xFF45, 0x00); // LYC=0
    core.bus.write8_immediate(0xFF41, 0x40); // LYC=LY enabled -> rising edge sets IF.1
    assert_ne!(core.bus.if_flag & 0x02, 0);
    core.bus.if_flag = 0;
    // Enabling Mode 2 while LYC=LY is still true keeps STAT line high (no new rising edge!)
    core.bus.write8_immediate(0xFF41, 0x60);
    assert_eq!(core.bus.if_flag & 0x02, 0);

    // APU tone produces non-zero audio and NR52=0 mutes
    core.bus.write8_immediate(0xFF26, 0x80); // NR52 on
    core.bus.write8_immediate(0xFF24, 0x77);
    core.bus.write8_immediate(0xFF25, 0xFF);
    core.bus.write8_immediate(0xFF11, 0x80); // 50% duty
    core.bus.write8_immediate(0xFF12, 0xF0); // vol 15
    core.bus.write8_immediate(0xFF13, 0x00);
    core.bus.write8_immediate(0xFF14, 0x87); // trigger high freq
    let mut fe = TestFrontend::new();
    core.run_frame(&mut fe);
    assert!(fe.audio_samples.iter().any(|&s| s != 0));
    core.bus.write8_immediate(0xFF26, 0x00); // power off
    assert_eq!(core.peek8(0xFF24), 0x00);

    // Reset preserves battery RAM & config while resetting CPU/WRAM
    core.bus.write8_immediate(0x0000, 0x0A);
    core.bus.write8_immediate(0xA000, 0xCA);
    core.reset();
    assert_eq!(core.cpu.pc, 0x0100);
    assert!(core.is_cgb());
    core.bus.write8_immediate(0x0000, 0x0A);
    assert_eq!(core.peek8(0xA000), 0xCA);
}
