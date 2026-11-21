//! Generates minimal, deterministic ARM7TDMI `.gba` stress ROMs for Phase 3C PPU benchmarking.
//!
//! Run with:
//!   cargo run -p gba-testrunner --example gen_stress_roms

use std::fs;
use std::path::Path;

use gba_testrunner::stress_roms::{
    gen_stress_aff_objwin, gen_stress_bitmap_345, gen_stress_mode0_4bg_128spr,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = Path::new("roms/homebrew");
    fs::create_dir_all(out_dir)?;

    let aff = gen_stress_aff_objwin();
    fs::write(out_dir.join("stress_aff_objwin.gba"), &aff)?;

    let m0 = gen_stress_mode0_4bg_128spr();
    fs::write(out_dir.join("stress_mode0_4bg_128spr.gba"), &m0)?;

    let bm = gen_stress_bitmap_345();
    fs::write(out_dir.join("stress_bitmap_345.gba"), &bm)?;

    println!(
        "Generated stress ROMs in {} ({} / {} / {} bytes)",
        out_dir.display(),
        aff.len(),
        m0.len(),
        bm.len()
    );
    Ok(())
}
