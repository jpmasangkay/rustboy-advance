use std::collections::HashSet;

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gba_testrunner::stress_roms::{
    gen_stress_aff_objwin, gen_stress_bitmap_345, gen_stress_mode0_4bg_128spr,
};
use gba_testrunner::{
    RunOptions, discover_workspace_root, load_manifest, run_suite, summarize_results,
};

#[test]
fn run_manifest_suite_skips_missing_roms() {
    let root = discover_workspace_root();
    let manifest = load_manifest(None, &root).expect("load default manifest");
    let opts = RunOptions::for_workspace(&root);
    let results = run_suite(&root, &manifest, &opts).expect("execute test suite");
    assert_eq!(results.len(), manifest.tests.len());

    let counts = summarize_results(&results);
    let mut unexpected_failures = Vec::new();
    for res in &results {
        if res.is_unexpected_failure() {
            unexpected_failures.push(format!("{} ({}): {}", res.id, res.name, res.notes));
        }
    }

    eprintln!(
        "gba-testrunner summary: {} passed, {} failed, {} skipped (missing ROM), {} skipped (missing golden)",
        counts.passed, counts.failed, counts.skipped_rom, counts.skipped_golden
    );

    assert!(
        unexpected_failures.is_empty(),
        "Unexpected test ROM failures or known-bad CRC32 drift:\n{}",
        unexpected_failures.join("\n")
    );
}

struct CaptureFrontend {
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    presented: usize,
}

impl HostFrontend for CaptureFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.last_fb.copy_from_slice(fb);
        self.presented += 1;
    }

    fn push_audio_samples(&mut self, _samples: &[i16]) {}

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[test]
fn procedural_stress_roms_render_animated_frames() {
    let cases: [(&str, Vec<u8>); 3] = [
        ("stress_aff_objwin", gen_stress_aff_objwin()),
        ("stress_mode0_4bg_128spr", gen_stress_mode0_4bg_128spr()),
        ("stress_bitmap_345", gen_stress_bitmap_345()),
    ];

    for (name, rom) in cases {
        let mut core = Box::new(GbaCore::new(None, rom));
        let mut fe = CaptureFrontend {
            last_fb: [0; LCD_WIDTH * LCD_HEIGHT],
            presented: 0,
        };
        for _ in 0..4 {
            core.run_frame(&mut fe);
        }
        let early = fe.last_fb;
        for _ in 0..6 {
            core.run_frame(&mut fe);
        }
        let later = fe.last_fb;
        assert_eq!(fe.presented, 10, "{name}: expected 10 presented frames");
        let colors: HashSet<u16> = later.iter().copied().collect();
        assert!(
            colors.len() >= 16,
            "{name}: expected rich multi-color render, got {} colors",
            colors.len()
        );
        assert_ne!(
            early, later,
            "{name}: expected animated scene to advance between frame 4 and 10"
        );
    }
}
