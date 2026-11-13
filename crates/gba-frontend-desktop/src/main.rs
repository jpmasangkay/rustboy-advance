// #![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

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
mod shaders;
mod stress_rom;
mod theme;

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
                | "--bench"
                | "--bench-realtime"
                | "--bench-filters"
                | "--bench-softbuffer"
                | "--throttled-test"
                | "--calibrate"
                | "--profile-sw"
                | "--selftest"
                | "--gpu-probe"
                | "--dump-hashes"
                | "--verify-hashes"
        )
    })
}

fn should_suppress_error_dialog() -> bool {
    is_cli_headless_invocation()
        || std::env::args().any(|a| a == "--perf-log" || a == "--perf-seconds")
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let msg = format!("{panic_info}");
        if !std::env::args().any(|a| a == "--gpu-probe") {
            let _ = std::fs::write("rustboy-advance-crash.log", &msg);
        }
        eprintln!("Rustboy Advance - Fatal Crash:\n{msg}");
        if !should_suppress_error_dialog() {
            rfd::MessageDialog::new()
                .set_title("Rustboy Advance - Fatal Crash")
                .set_description(&msg)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
    }));
}

fn show_startup_error(title: &str, description: &str) {
    let _ = std::fs::write("rustboy-advance-startup-error.log", description);
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
    install_panic_hook();

    let builder = std::thread::Builder::new()
        .name("rustboy-main".into())
        .stack_size(16 * 1024 * 1024);

    let handle = builder
        .spawn(real_main)
        .expect("failed to spawn main thread with 16MB stack");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
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

    if cli.no_gpu {
        println!(
            "[backend] Selected: {} — reason: forced via --no-gpu flag",
            crate::shaders::ADAPTER_FALLBACK_ORDER[3]
        );
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
        println!(
            "[backend] Selected: {} — reason: {reason}",
            crate::shaders::ADAPTER_FALLBACK_ORDER[3]
        );
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
    if let Err(err) = app_host::run_desktop("Rustboy Advance", init_w, init_h, cli) {
        log::warn!("wgpu desktop initialization unavailable ({err}); falling back to no-GPU presenter");
        println!(
            "[backend] Selected: {} — reason: wgpu fallback ({err})",
            crate::shaders::ADAPTER_FALLBACK_ORDER[3]
        );
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
