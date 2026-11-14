use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let written =
        gba_testrunner::stress_roms::write_synthetic_stress_roms(Path::new("roms/homebrew"))?;
    println!(
        "Generated stress ROMs in roms/homebrew ({} / {} / {} bytes)",
        written[0].1, written[1].1, written[2].1
    );
    Ok(())
}
