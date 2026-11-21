use std::path::PathBuf;

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

struct CaptureFrontend {
    framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
}

impl Default for CaptureFrontend {
    fn default() -> Self {
        Self {
            framebuffer: [0; LCD_WIDTH * LCD_HEIGHT],
        }
    }
}

impl HostFrontend for CaptureFrontend {
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.framebuffer = *framebuffer_bgr555;
    }

    fn push_audio_samples(&mut self, _samples_interleaved_i16: &[i16]) {}

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

fn test_roms_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-roms")
        .join("jsmolka")
}

#[test]
fn jsmolka_gba_tests() {
    let base = test_roms_dir();
    if !base.exists() {
        eprintln!(
            "Skipping jsmolka_gba_tests: test-roms/jsmolka directory not found at {}",
            base.display()
        );
        return;
    }

    let roms = [
        "arm.gba",
        "thumb.gba",
        "memory.gba",
        "nes.gba",
        "bios.gba",
        "unsafe.gba",
        "ppu/hello.gba",
        "ppu/shades.gba",
        "ppu/stripes.gba",
        "save/none.gba",
        "save/sram.gba",
        "save/flash64.gba",
        "save/flash128.gba",
    ];

    let mut present_count = 0usize;
    let mut failures = Vec::new();

    for rel in roms {
        let path = base.join(rel);
        if !path.exists() {
            eprintln!("Skipping missing ROM: {}", path.display());
            continue;
        }
        present_count += 1;

        let rom = std::fs::read(&path).expect("read jsmolka test ROM");
        let mut core = GbaCore::new(None, rom);
        let mut frontend = CaptureFrontend::default();

        for _ in 0..118 {
            core.run_frame(&mut frontend);
        }
        let pc_118 = core.cpu.gpr[15];
        core.run_frame(&mut frontend);
        let pc_119 = core.cpu.gpr[15];
        core.run_frame(&mut frontend);
        let pc = core.cpu.gpr[15];

        // thumb.gba reports failure code in r7 (Thumb low reg); all other jsmolka tests report in r12.
        let status_reg = if rel == "thumb.gba" {
            core.cpu.gpr[7]
        } else {
            core.cpu.gpr[12]
        };
        let converged = pc == pc_118 && pc == pc_119;
        println!(
            "[jsmolka] {rel:<18} -> status={status_reg:#010X} ({status_reg}), pc={pc:#010X}, converged={converged}"
        );

        // Visual-only PPU demos don't follow the r12=0 / halt convention.
        let is_ppu_visual = rel.starts_with("ppu/");
        if !is_ppu_visual && (status_reg != 0 || !converged) {
            failures.push(format!(
                "{rel}: status={status_reg:#x}, converged={converged}, pc={pc:#010x}"
            ));
        }

        match rel {
            "ppu/hello.gba" => {
                let lit = frontend
                    .framebuffer
                    .iter()
                    .filter(|&&c| c == 0x7FFF)
                    .count();
                let dark = frontend
                    .framebuffer
                    .iter()
                    .filter(|&&c| c == 0x0000)
                    .count();
                assert!(
                    lit > 0 && dark > 0,
                    "ppu/hello.gba should render white glyphs on black backdrop (lit={lit}, dark={dark})"
                );
            }
            "ppu/shades.gba" => {
                for stripe in 0..15usize {
                    let expected = ((stripe as u16) * 2) << 10;
                    let actual = frontend.framebuffer[stripe * 16];
                    assert_eq!(
                        actual,
                        expected,
                        "ppu/shades.gba stripe {stripe} at x={} mismatch",
                        stripe * 16
                    );
                }
            }
            "ppu/stripes.gba" => {
                assert_eq!(
                    frontend.framebuffer[0], 0x560B,
                    "ppu/stripes tile 1 backdrop stripe"
                );
                assert_eq!(
                    frontend.framebuffer[8], 0x6290,
                    "ppu/stripes tile 0 green stripe"
                );
                assert_eq!(
                    frontend.framebuffer[16], 0x560B,
                    "ppu/stripes second tile 1 stripe"
                );
            }
            _ => {}
        }
    }

    if present_count == 0 {
        eprintln!("Skipping jsmolka_gba_tests: no .gba files present in test-roms/");
        return;
    }

    assert!(
        failures.is_empty(),
        "jsmolka test ROM failures: {failures:#?}"
    );
}
