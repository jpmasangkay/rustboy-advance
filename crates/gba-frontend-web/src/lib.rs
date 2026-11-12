#![allow(clippy::new_without_default)]

extern crate alloc;

pub mod color;
pub mod emulator;
#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use color::ColorProfile;
pub use emulator::{BIOS_BYTES, MAX_ROM_BYTES, RGBA_FRAME_BYTES, WebEmulator, WebFrontend, crc32};

#[cfg(test)]
mod tests {
    use super::*;
    use gba_core::{
        GbaCore, HostFrontend, KEY_A, KEY_B, KEY_START, LCD_HEIGHT, LCD_WIDTH, SAMPLE_RATE_HZ,
        encode_keyinput,
    };

    fn make_mode3_test_rom(title: &str, code: &str, save_tag: Option<&[u8]>) -> Vec<u8> {
        // ARMv4T test program at 0x0800_0000:
        //   0x0800_0000: B +0xC0 (skip 192-byte header -> 0x0800_00C8) -> 0xEA000030
        // At 0x0800_00C8:
        //   MOV R0, #0x04000000  (I/O base)           -> 0xE3A00301
        //   MOV R1, #0x0400      (BG2 enable)         -> 0xE3A01B01
        //   ORR R1, R1, #0x03    (Mode 3)             -> 0xE3811003
        //   STRH R1, [R0]        (DISPCNT = 0x0403)   -> 0xE1C010B0
        //   MOV R2, #0x06000000  (VRAM base)          -> 0xE3A02406
        //   MOV R3, #0x001F      (BGR555 red)         -> 0xE3A0301F
        //   STRH R3, [R2]        (pixel (0,0) = red)  -> 0xE1C230B0
        //   B .                                       -> 0xEAFFFFFE
        let mut rom = vec![0u8; 0x200];
        // Branch over 0xC0 header: pc = 0x0800_0008, target = 0x0800_00C8 -> offset = 0xC0 / 4 = 0x30
        rom[0..4].copy_from_slice(&0xEA00_0030u32.to_le_bytes());
        rom[0xB2] = 0x96;

        let title_bytes = title.as_bytes();
        let copy_title = title_bytes.len().min(12);
        rom[0xA0..0xA0 + copy_title].copy_from_slice(&title_bytes[..copy_title]);

        let code_bytes = code.as_bytes();
        let copy_code = code_bytes.len().min(4);
        rom[0xAC..0xAC + copy_code].copy_from_slice(&code_bytes[..copy_code]);

        let code_words: [u32; 8] = [
            0xE3A0_0301,
            0xE3A0_1B01,
            0xE381_1003,
            0xE1C0_10B0,
            0xE3A0_2406,
            0xE3A0_301F,
            0xE1C2_30B0,
            0xEAFF_FFFE,
        ];
        for (i, w) in code_words.iter().enumerate() {
            let off = 0xC8 + i * 4;
            rom[off..off + 4].copy_from_slice(&w.to_le_bytes());
        }

        if let Some(tag) = save_tag {
            let off = 0x140;
            rom[off..off + tag.len()].copy_from_slice(tag);
        }
        rom
    }

    #[test]
    fn web_frontend_linear_and_color_profiles() {
        let mut fe = WebFrontend::new();
        let mut fb = [0u16; LCD_WIDTH * LCD_HEIGHT];
        fb[0] = 0x001F; // Red
        fb[1] = 0x03E0; // Green
        fb[2] = 0x7C00; // Blue
        fb[3] = 0x7FFF; // White

        fe.set_color_profile(ColorProfile::Linear);
        fe.present_frame(&fb);
        assert_eq!(&fe.rgba_frame[0..4], &[255, 0, 0, 255]);
        assert_eq!(&fe.rgba_frame[4..8], &[0, 255, 0, 255]);
        assert_eq!(&fe.rgba_frame[8..12], &[0, 0, 255, 255]);
        assert_eq!(&fe.rgba_frame[12..16], &[255, 255, 255, 255]);

        // AGB-001 keeps white white and cross-mixes primaries subtly with full alpha.
        fe.set_color_profile(ColorProfile::Agb001);
        fe.present_frame(&fb);
        assert_eq!(fe.rgba_frame[3], 255);
        assert!(fe.rgba_frame[0] > fe.rgba_frame[1]); // Red pixel stays red-dominant
        assert!(fe.rgba_frame[1] > 0); // Reflective cross-talk raises G channel slightly
        assert_eq!(&fe.rgba_frame[12..16], &[255, 255, 255, 255]);

        fe.set_color_profile(ColorProfile::Ags101);
        fe.present_frame(&fb);
        assert_eq!(fe.rgba_frame[3], 255);
        assert!(fe.rgba_frame[0] > 200);

        fe.set_color_profile(ColorProfile::Ags001);
        fe.present_frame(&fb);
        assert_eq!(fe.rgba_frame[3], 255);
        assert!(fe.rgba_frame[0] > 170);
    }

    #[test]
    fn web_frontend_lcd_ghosting_blends_frames() {
        let mut fe = WebFrontend::new();
        fe.set_color_profile(ColorProfile::Linear);
        let black = [0u16; LCD_WIDTH * LCD_HEIGHT];
        let mut white = [0u16; LCD_WIDTH * LCD_HEIGHT];
        white[0] = 0x7FFF;

        fe.set_lcd_ghosting(true);
        fe.present_frame(&black);
        assert_eq!(&fe.rgba_frame[0..4], &[0, 0, 0, 255]);

        fe.present_frame(&white);
        // 255 * 166 / 256 = 165 (~65% blend from black prev frame)
        assert_eq!(fe.rgba_frame[0], 165);
        assert_eq!(fe.rgba_frame[1], 165);
        assert_eq!(fe.rgba_frame[2], 165);
        assert_eq!(fe.rgba_frame[3], 255);
    }

    #[test]
    fn emulator_validation_rejects_bad_roms_and_bios() {
        let mut emu = WebEmulator::new();
        assert!(!emu.rom_loaded());
        assert!(!emu.run_frame());
        assert_eq!(emu.sample_rate(), SAMPLE_RATE_HZ);

        assert!(emu.load_rom(&[]).is_err());
        assert!(emu.load_rom(&[1, 2, 3]).is_err());
        assert!(emu.load_rom(b"PK\x03\x04zipdata").is_err());
        assert!(emu.load_rom(b"\x89PNG\r\n\x1a\n1234").is_err());
        assert!(emu.load_rom(b"\x7FELF1234").is_err());
        assert!(emu.load_rom(b"%PDF-1.7").is_err());

        assert!(emu.load_bios(&[0u8; 1024]).is_err());
        assert!(!emu.bios_loaded());
    }

    #[test]
    fn emulator_runs_rom_produces_pixels_audio_and_metadata() {
        let rom = make_mode3_test_rom("RUSTBOYTEST", "RBAT", Some(b"SRAM_V110"));
        let mut emu = WebEmulator::new();
        emu.load_rom(&rom).expect("valid ROM loads");

        assert!(emu.rom_loaded());
        assert_eq!(emu.game_title(), "RUSTBOYTEST");
        assert_eq!(emu.game_code(), "RBAT");
        assert_eq!(emu.save_type_name(), "SRAM 32 KiB");
        assert_eq!(emu.rom_crc32(), crc32(&rom));
        assert_eq!(
            emu.storage_key(),
            format!("RBAT_{:08X}", crc32(&rom))
        );
        assert_eq!(emu.framebuffer().len(), RGBA_FRAME_BYTES);

        emu.set_keys(KEY_A | KEY_START);
        assert!(emu.run_frame());
        assert!(emu.run_frame());

        // Pixel (0,0) should now be red (255, 0, 0, 255) under Linear profile.
        let mut out = vec![0u8; RGBA_FRAME_BYTES];
        let copied = emu.copy_rgba_frame(&mut out);
        assert_eq!(copied, RGBA_FRAME_BYTES);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);

        // Audio buffer receives stereo PCM samples per frame and drains cleanly.
        let samples = emu.drain_audio();
        assert!(!samples.is_empty());
        assert_eq!(samples.len() % 2, 0);
        assert!(emu.audio_samples().is_empty());

        // Keypad input reached core KEYINPUT register (active-low).
        let core = emu.core().expect("core active");
        assert_eq!(core.bus.hw.keyinput, encode_keyinput(KEY_A | KEY_START));
    }

    #[test]
    fn emulator_save_state_and_sram_round_trip() {
        let rom = make_mode3_test_rom("SAVETEST", "SAVE", Some(b"SRAM_V110"));
        let mut emu = WebEmulator::new();
        emu.load_rom(&rom).expect("valid ROM");
        emu.run_frame();
        emu.run_frame();

        let state = emu.save_state().expect("save state succeeds");
        assert!(!state.is_empty());

        // Mutate keyinput & SRAM after saving state, then restore state.
        emu.set_keys(KEY_B);
        emu.run_frame();

        let mut sram_in = vec![0xAAu8; 0x8000];
        sram_in[0] = 0x5A;
        sram_in[0x7FFF] = 0xC3;
        emu.import_sram(&sram_in).expect("SRAM import succeeds");
        assert_eq!(emu.export_sram()[0], 0x5A);
        assert_eq!(emu.export_sram()[0x7FFF], 0xC3);

        // Reset preserves SRAM!
        assert!(emu.reset());
        assert_eq!(emu.export_sram()[0], 0x5A);

        // Load state restores pre-import RAM/VRAM/registers.
        emu.load_state(&state).expect("load state succeeds");
        assert_eq!(&emu.framebuffer()[0..4], &[255, 0, 0, 255]);
        assert!(emu.load_state(b"corrupt").is_err());

        // Speed clamping & BIOS lifecycle.
        emu.set_speed(99.0);
        assert_eq!(emu.speed(), 8.0);
        emu.set_speed(0.01);
        assert_eq!(emu.speed(), 0.25);
        emu.set_speed(f32::NAN);
        assert_eq!(emu.speed(), 0.25);

        let bios = vec![0u8; BIOS_BYTES];
        emu.load_bios(&bios).expect("16 KiB BIOS accepted");
        assert!(emu.bios_loaded());
        emu.clear_bios();
        assert!(!emu.bios_loaded());
    }

    #[test]
    fn web_frontend_step_frame_compat() {
        let rom = make_mode3_test_rom("COMPAT", "CMPT", None);
        let mut core = GbaCore::new(None, rom);
        let mut fe = WebFrontend::default();
        fe.set_pressed_keys(KEY_B);
        fe.step_frame(&mut core);
        assert_eq!(fe.poll_keypad(), encode_keyinput(KEY_B));
    }
}
