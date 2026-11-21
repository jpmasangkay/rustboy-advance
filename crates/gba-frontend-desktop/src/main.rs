#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod app;
mod app_host;
mod audio;
mod bench;
mod capture;
mod cheats;
mod config;
mod debugger_ui;
mod display_timing;
mod emu_thread;
mod filters;
mod game_settings;
mod gb_bridge;
mod gpu_probe;
mod headless;
mod hotkeys;
mod hud;
mod icon;
mod input;
mod library;
mod link;
mod lua_engine;
mod mem_search;
mod menus;
mod movies;
mod no_gpu;
mod retroachievements;
mod rewind;
mod rom_loader;
mod savestates;
mod settings_ui;
mod shaders;
mod stress_rom;
mod theme;
#[cfg(test)]
mod ui_snapshots;

use gba_core::{LCD_HEIGHT, LCD_WIDTH};

pub(crate) use app::{RustboyApp, configure_obsidian_style};
use config::AppConfig;
pub use config::{DisplayFilter, ViewportScale};
pub use filters::FrontendBridge;
use headless::CliConfig;
pub use theme::*;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn is_cli_headless_invocation() -> bool {
    std::env::args().any(|a| {
        matches!(
            a.as_str(),
            "--headless"
                | "--help"
                | "-h"
                | "--version"
                | "-V"
                | "--bench"
                | "--bench-realtime"
                | "--bench-filters"
                | "--bench-softbuffer"
                | "--throttled-test"
                | "--calibrate"
                | "--profile-sw"
                | "--selftest"
                | "--scan-library"
                | "--verify-save-flush"
                | "--verify-perf-check"
                | "--gpu-probe"
                | "--dump-hashes"
                | "--verify-hashes"
                | "--uninstall"
        )
    })
}

/// On Windows release builds compiled with `windows_subsystem = "windows"`, re-attach the parent
/// terminal console when CLI/headless flags are passed so stdout/stderr remain visible.
#[cfg(windows)]
fn attach_parent_console_if_cli() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    if is_cli_headless_invocation() {
        unsafe extern "system" {
            fn AttachConsole(dw_process_id: u32) -> i32;
        }
        // Best effort: succeeds when launched from cmd/PowerShell; returns 0 if no parent console.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

#[cfg(not(windows))]
fn attach_parent_console_if_cli() {}

fn should_suppress_error_dialog() -> bool {
    is_cli_headless_invocation()
        || std::env::args().any(|a| a == "--perf-log" || a == "--perf-seconds")
}

struct DesktopLogger;

impl log::Log for DesktopLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let target = metadata.target();
        if target.starts_with("rustboy_advance")
            || target.starts_with("gba_")
            || target.starts_with("gb_")
        {
            metadata.level() <= log::max_level()
        } else {
            metadata.level() <= log::Level::Warn && metadata.level() <= log::max_level()
        }
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

static DESKTOP_LOGGER: DesktopLogger = DesktopLogger;
static MAIN_THREAD_ID: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

fn install_logger() {
    let max_level = match std::env::var("RUST_LOG")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "trace" => log::LevelFilter::Trace,
        "debug" => log::LevelFilter::Debug,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        "off" => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };
    let _ = log::set_logger(&DESKTOP_LOGGER);
    log::set_max_level(max_level);
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let msg = format!("{panic_info}");
        if !std::env::args().any(|a| a == "--gpu-probe") {
            let log_path = AppConfig::crash_log_path();
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(log_path, &msg);
        }
        eprintln!("Rustboy Advance - Fatal Crash:\n{msg}");
        // Only open a modal OS dialog from the UI main thread; calling rfd/Cocoa/Win32 UI
        // APIs from a background audio/worker thread can abort or deadlock the process.
        let on_main_thread = MAIN_THREAD_ID
            .get()
            .is_some_and(|&id| id == std::thread::current().id());
        if on_main_thread && !should_suppress_error_dialog() {
            rfd::MessageDialog::new()
                .set_title("Rustboy Advance - Fatal Crash")
                .set_description(&msg)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
    }));
}

fn show_startup_error(title: &str, description: &str) {
    let log_path = AppConfig::startup_error_log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(log_path, description);
    eprintln!("{title}: {description}");
    if !should_suppress_error_dialog() {
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(description)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

fn main() {
    let _ = MAIN_THREAD_ID.set(std::thread::current().id());
    attach_parent_console_if_cli();
    config::init_portable_mode();
    install_logger();
    install_panic_hook();
    // Run directly on the OS main thread so Cocoa/macOS, Wayland, X11, and Win32
    // event loops all initialize legally (Windows stack size is configured in build.rs).
    real_main();
}

fn real_main() {
    let cli = match CliConfig::parse() {
        Ok(c) => c,
        Err(e) => {
            show_startup_error(
                "Argument Error",
                &format!("Failed to parse command-line arguments:\n{e}"),
            );
            std::process::exit(2);
        }
    };

    if cli.uninstall {
        let data_dir = AppConfig::app_data_dir();
        if !cli.yes {
            eprintln!(
                "Refusing to delete all persistent Rustboy Advance data in {} without confirmation.",
                data_dir.display()
            );
            eprintln!(
                "This permanently removes config, battery saves, save states, covers, and credentials."
            );
            eprintln!("Re-run with `--uninstall --yes` to confirm.");
            std::process::exit(2);
        }
        match AppConfig::uninstall_all_data() {
            Ok(removed) => {
                println!(
                    "Rustboy Advance: cleanly uninstalled persistent data from {}",
                    data_dir.display()
                );
                for path in &removed {
                    println!("  removed: {}", path.display());
                }
                return;
            }
            Err(err) => {
                eprintln!("Rustboy Advance uninstall failed: {err}");
                std::process::exit(1);
            }
        }
    }

    if let Some(mode) = cli.verify_save_flush.clone() {
        let res = if mode.starts_with("nogpu-") {
            no_gpu::verify_nogpu_save_flush_cli(cli, &mode)
        } else {
            app::verify_eframe_save_flush_cli(cli, &mode)
        };
        if let Err(e) = res {
            eprintln!("Save-flush verification failed ({mode}): {e}");
            std::process::exit(1);
        }
        println!("Save-flush verification passed ({mode})");
        return;
    }

    if cli.headless {
        if let Err(e) = headless::run_headless(&cli) {
            show_startup_error(
                "Headless Error",
                &format!("Error executing headless session:\n{e}"),
            );
            std::process::exit(1);
        }
        return;
    }

    let mut initial_config = AppConfig::load();
    let scale = initial_config.window_scale.clamp(1, 5) as f32;
    let init_w = ((LCD_WIDTH as f32) * scale + 24.0).max(1020.0);
    let init_h = ((LCD_HEIGHT as f32) * scale + 92.0).max(680.0);

    let no_gpu_label = crate::shaders::ADAPTER_FALLBACK_ORDER
        .last()
        .copied()
        .unwrap_or("No-GPU presenter (winit + softbuffer)");

    if cli.no_gpu {
        println!("[backend] Selected: {no_gpu_label} — reason: forced via --no-gpu flag");
        if let Err(err) = no_gpu::run_no_gpu_desktop(
            "Rustboy Advance",
            init_w,
            init_h,
            cli,
            "forced via --no-gpu flag",
        ) {
            show_startup_error(
                "No-GPU Frontend Error",
                &format!("No-GPU initialization error:\n{err}"),
            );
            std::process::exit(1);
        }
        return;
    }

    if !cli.simulate_no_adapter
        && let gpu_probe::GpuGuardOutcome::UseNoGpu { reason } =
            gpu_probe::ensure_gpu_startup_safe(&cli, &mut initial_config)
    {
        println!("[backend] Selected: {no_gpu_label} — reason: {reason}");
        if let Err(no_gpu_err) =
            no_gpu::run_no_gpu_desktop("Rustboy Advance", init_w, init_h, cli, &reason)
        {
            show_startup_error(
                "No-GPU Frontend Error",
                &format!("GPU guard fallback reason:\n{reason}\n\nNo-GPU error:\n{no_gpu_err}"),
            );
            std::process::exit(1);
        }
        return;
    }

    let cli_fallback = cli.clone();
    if let Err(err) = app_host::run_desktop("Rustboy Advance", init_w, init_h, cli, initial_config)
    {
        log::warn!(
            "wgpu desktop initialization unavailable ({err}); falling back to no-GPU presenter"
        );
        println!("[backend] Selected: {no_gpu_label} — reason: wgpu fallback ({err})");
        if let Err(no_gpu_err) =
            no_gpu::run_no_gpu_desktop("Rustboy Advance", init_w, init_h, cli_fallback, &err)
        {
            show_startup_error(
                "Desktop Frontend Error",
                &format!("wgpu error:\n{err}\n\nNo-GPU fallback error:\n{no_gpu_err}"),
            );
            std::process::exit(1);
        }
    }
}
