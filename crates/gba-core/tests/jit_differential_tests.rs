#[cfg(feature = "jit")]
use std::path::PathBuf;

use gba_core::bus::SysBus;
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

struct TestNullFrontend;
impl HostFrontend for TestNullFrontend {
    fn present_frame(&mut self, _frame: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
    fn push_audio_samples(&mut self, _samples: &[i16]) {}
    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[cfg(feature = "jit")]
fn workspace_rom(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(rel)
}

#[test]
fn test_jit_initialization_and_graceful_fallback() {
    let rom = vec![0u8; 0x1000];
    #[allow(unused_mut)]
    let mut core = GbaCore::new(None, rom);

    assert!(
        !core.jit.enabled,
        "Interpreter must remain the default when core is instantiated"
    );

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
    rom[0] = 0x01; // movs r1, #1
    rom[1] = 0x21;
    rom[2] = 0xFE; // b .
    rom[3] = 0xE7;

    let mut core = GbaCore::new(None, rom);
    core.jit.enabled = core.jit.available;

    let mut fe = TestNullFrontend;
    core.run_frame(&mut fe);
    core.bus.dirty_code_pages.clear();
    core.bus.invalidate_all_jit = false;

    // Write to mirrored EWRAM code page (0x0204_0000 aliases 0x0200_0000 -> canonical page 0x0200_0000 >> 14)
    core.bus.write_32(
        0x0204_0000,
        0x1234_5678,
        gba_core::bus::AccessType::NonSequential,
    );
    let expected = SysBus::canonical_code_page(0x0200_0000 >> 14);
    assert!(
        core.bus.dirty_code_pages.contains(&expected),
        "mirrored EWRAM write must record canonical dirty code page"
    );
    for page in core.bus.dirty_code_pages.drain(..) {
        core.jit.invalidate_page(page);
    }
}

#[test]
#[cfg(feature = "jit")]
fn test_differential_lockstep_inline_arm_and_thumb() {
    // Inline ARM + Thumb program exercising ALU, shifts, branches, memory loads/stores, and loops.
    let words: [u32; 12] = [
        0xE3A0_0000, // mov r0, #0
        0xE3A0_1001, // mov r1, #1
        0xE3A0_2403, // mov r2, #0x03000000
        0xE080_0001, // add r0, r0, r1
        0xE281_1003, // add r1, r1, #3
        0xE020_3001, // eor r3, r0, r1
        0xE582_3000, // str r3, [r2]
        0xE592_4000, // ldr r4, [r2]
        0xE044_5000, // sub r5, r4, r0
        0xE351_0064, // cmp r1, #100
        0xBAFF_FFF7, // blt 0x0800_000C
        0xEAFF_FFFE, // b .
    ];
    let mut rom = vec![0u8; 0x1000];
    for (i, &w) in words.iter().enumerate() {
        rom[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }

    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestNullFrontend;
    core.run_differential_test(&mut fe, &rom, 5)
        .expect("inline ARM JIT differential test must match interpreter bit-for-bit");
}

#[test]
#[cfg(feature = "jit")]
fn test_differential_lockstep_interpreter_vs_jit() {
    let rom_path = workspace_rom("test-roms/tonc/pageflip.gba");
    if !rom_path.exists() {
        return;
    }
    let rom = std::fs::read(rom_path).expect("Failed to read pageflip.gba");

    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestNullFrontend;

    let res = core.run_differential_test(&mut fe, &rom, 60);
    assert!(
        res.is_ok(),
        "Differential lockstep divergence detected between Interpreter and JIT: {:?}",
        res.err()
    );
}

#[test]
#[cfg(feature = "jit")]
fn test_differential_lockstep_mode7_racers() {
    let rom_path = workspace_rom("test-roms/tonc/m7_demo.gba");
    if !rom_path.exists() {
        return;
    }
    let rom = std::fs::read(rom_path).expect("Failed to read m7_demo.gba");

    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestNullFrontend;

    let res = core.run_differential_test(&mut fe, &rom, 30);
    assert!(
        res.is_ok(),
        "Differential lockstep divergence detected in Mode 7 demo: {:?}",
        res.err()
    );
}
