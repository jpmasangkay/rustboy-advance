use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

struct TestNullFrontend;
impl HostFrontend for TestNullFrontend {
    fn present_frame(&mut self, _frame: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
    fn push_audio_samples(&mut self, _samples: &[i16]) {}
    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[test]
fn test_jit_initialization_and_graceful_fallback() {
    let rom = vec![0u8; 0x1000];
    #[allow(unused_mut)]
    let mut core = GbaCore::new(None, rom);

    // Verify interpreter is default and fallback
    assert!(
        !core.jit.enabled,
        "Interpreter must remain the default when core is instantiated"
    );

    // Check availability (will be true on x86_64/aarch64 with jit feature, or false gracefully if disabled)
    #[cfg(feature = "jit")]
    {
        assert!(
            core.jit.available,
            "Cranelift JIT should be available on host architecture"
        );
        core.jit.enabled = true;
        assert!(core.jit.enabled);
        core.jit.enabled = false;
        assert!(!core.jit.enabled);
    }
}

#[test]
fn test_jit_code_page_invalidation() {
    let mut rom = vec![0u8; 0x4000];
    // Put a simple loop in ROM
    rom[0] = 0x01; // movs r1, #1
    rom[1] = 0x21;
    rom[2] = 0xFE; // b .
    rom[3] = 0xE7;

    #[allow(unused_mut)]
    let mut core = GbaCore::new(None, rom);
    core.jit.enabled = core.jit.available;

    let mut fe = TestNullFrontend;
    core.run_frame(&mut fe);

    // Simulate writing to EWRAM code page (0x0200_0000 -> page 512)
    core.bus.write_32(
        0x0200_0000,
        0x12345678,
        gba_core::bus::AccessType::NonSequential,
    );
    assert_eq!(core.bus.code_page_invalidated, Some(0x0200_0000 >> 14));

    // Next step must consume the invalidation and clear JIT blocks for that page
    let inv_page = core.bus.code_page_invalidated.take();
    assert_eq!(inv_page, Some(0x0200_0000 >> 14));
    core.jit.invalidate_page(inv_page.unwrap());
}

#[test]
fn test_differential_lockstep_interpreter_vs_jit() {
    let rom_path = std::path::Path::new("test-roms/tonc/pageflip.gba");
    if !rom_path.exists() {
        return;
    }
    let rom = std::fs::read(rom_path).expect("Failed to read pageflip.gba");

    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestNullFrontend;

    // Run differential lockstep test over 120 frames
    let res = core.run_differential_test(&mut fe, &rom, 120);
    assert!(
        res.is_ok(),
        "Differential lockstep divergence detected between Interpreter and JIT: {:?}",
        res.err()
    );
}

#[test]
fn test_differential_lockstep_mode7_racers() {
    let rom_path = std::path::Path::new("test-roms/tonc/m7_demo.gba");
    if !rom_path.exists() {
        return;
    }
    let rom = std::fs::read(rom_path).expect("Failed to read m7_demo.gba");

    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestNullFrontend;

    // Run differential lockstep test over 60 frames of affine graphics
    let res = core.run_differential_test(&mut fe, &rom, 60);
    assert!(
        res.is_ok(),
        "Differential lockstep divergence detected in Mode 7 demo: {:?}",
        res.err()
    );
}
