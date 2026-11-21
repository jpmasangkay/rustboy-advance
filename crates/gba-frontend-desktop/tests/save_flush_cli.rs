use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// Builds a valid minimal GBA ROM with an SRAM signature (`SRAM_V110`) and safe idle loop.
fn build_minimal_sram_rom() -> Vec<u8> {
    let mut rom = vec![0xFFu8; 4096];
    rom[0..4].copy_from_slice(&0xEA00_002Eu32.to_le_bytes()); // B 0xC0
    rom[0xA0..0xAC].copy_from_slice(b"SAVETEST\0\0\0\0");
    rom[0xAC..0xB0].copy_from_slice(b"BSVE");
    rom[0xB0..0xB2].copy_from_slice(b"01");
    rom[0xB2] = 0x96;
    let mut chk: u8 = 0;
    for &b in &rom[0xA0..0xBD] {
        chk = chk.wrapping_sub(b);
    }
    rom[0xBD] = chk.wrapping_sub(0x19);
    rom[0xC0..0xC4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes()); // B .
    rom[0x140..0x14C].copy_from_slice(b"SRAM_V110\0\0\0");
    rom
}

struct TempDirGuard {
    dir: PathBuf,
}

impl TempDirGuard {
    fn new(tag: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rustboy_cli_a0_{tag}_{}_{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp test dir");
        Self { dir }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn verify_cli_quit_mode(mode: &str) {
    let ws = TempDirGuard::new(mode);
    let rom_path = ws.dir.join("test_sram.gba");
    let sav_path = ws.dir.join("saves/test_sram.sav");
    fs::write(&rom_path, build_minimal_sram_rom()).expect("write ROM");
    fs::create_dir_all(sav_path.parent().unwrap()).expect("create saves dir");

    // Pre-populate .sav with identifiable bytes and backdated modification time.
    let mut initial_sav = vec![0x5Au8; 0x8000];
    initial_sav[..4].copy_from_slice(b"OLD!");
    {
        use std::io::Write;
        let mut f = fs::File::create(&sav_path).expect("create initial sav");
        f.write_all(&initial_sav).expect("write initial sav");
        f.sync_all().expect("sync initial sav");
        let old_time = SystemTime::now() - Duration::from_secs(30);
        let _ = f.set_modified(old_time);
    }
    let before_mtime = fs::metadata(&sav_path)
        .expect("read sav metadata")
        .modified()
        .expect("read sav mtime");

    let exe = env!("CARGO_BIN_EXE_rustboy-advance");
    let mut child = Command::new(exe)
        .env("APPDATA", &ws.dir)
        .env("XDG_CONFIG_HOME", &ws.dir)
        .env("HOME", &ws.dir)
        // Pass ROM path positionally to verify positional CLI launch opens the game.
        .arg(&rom_path)
        .arg("--sav")
        .arg(&sav_path)
        .arg("--verify-save-flush")
        .arg(mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rustboy-advance");

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(st) = child.try_wait().expect("poll child") {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("[{mode}] timed out waiting for CLI verification process");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let output = child.wait_with_output().expect("collect output");
    assert!(
        status.success(),
        "[{mode}] exited with {status}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let after_meta = fs::metadata(&sav_path).expect("read post-exit sav metadata");
    let after_mtime = after_meta.modified().expect("read post-exit sav mtime");
    assert!(
        after_mtime > before_mtime,
        "[{mode}] expected .sav timestamp ({after_mtime:?}) to advance past {before_mtime:?}"
    );

    let after_bytes = fs::read(&sav_path).expect("read post-exit sav bytes");
    assert_eq!(after_bytes.len(), 0x8000, "[{mode}] unexpected .sav size");
    assert_eq!(
        &after_bytes[..4],
        b"SVOK",
        "[{mode}] modified save memory did not flush to .sav on quit"
    );
    assert!(
        after_bytes[4..].iter().all(|&b| b == 0x5A),
        "[{mode}] pre-existing .sav contents were not preserved after load & flush"
    );
}

#[test]
fn cli_save_flush_all_quit_paths_and_positional_rom() {
    for mode in [
        "eframe-esc",
        "eframe-alt-f4",
        "eframe-close",
        "nogpu-esc",
        "nogpu-alt-f4",
        "nogpu-close",
    ] {
        verify_cli_quit_mode(mode);
    }
}

#[test]
fn cli_uninstall_cleanly_deletes_all_persistent_app_data() {
    let ws = TempDirGuard::new("uninstall");
    let app_data = ws.dir.join("RustboyAdvance");
    fs::create_dir_all(app_data.join("saves")).expect("create saves");
    fs::create_dir_all(app_data.join("states")).expect("create states");
    fs::create_dir_all(app_data.join("covers")).expect("create covers");
    fs::create_dir_all(app_data.join("screenshots")).expect("create screenshots");
    fs::write(app_data.join("config.json"), b"{\"window_scale\":3}").expect("write config");
    fs::write(app_data.join("rustboy_library.cfg"), b"[]").expect("write library");
    fs::write(app_data.join("saves/game.sav"), b"SAVDATA").expect("write sav");
    fs::write(app_data.join("states/game.slot1.state"), b"STATEDATA").expect("write state");
    fs::write(app_data.join("covers/game.png"), b"COVER").expect("write cover");
    fs::write(app_data.join("screenshots/shot.png"), b"SHOT").expect("write screenshot");

    let exe = env!("CARGO_BIN_EXE_rustboy-advance");
    let output = Command::new(exe)
        .env("APPDATA", &ws.dir)
        .env("XDG_CONFIG_HOME", &ws.dir)
        .env("HOME", &ws.dir)
        .arg("--uninstall")
        .arg("--yes")
        .output()
        .expect("run rustboy-advance --uninstall --yes");

    assert!(
        output.status.success(),
        "--uninstall exited with {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !app_data.exists(),
        "expected {} to be completely removed by --uninstall",
        app_data.display()
    );
}
