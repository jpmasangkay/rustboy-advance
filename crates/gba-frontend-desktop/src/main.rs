#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod app;
mod app_host;
mod audio;
mod capture;
mod cheats;
mod config;
mod debugger_ui;
mod emu_thread;
mod filters;
mod game_settings;
mod gb_bridge;
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
mod retroachievements;
mod rewind;
mod rom_loader;
mod savestates;
mod shaders;
mod theme;

use gba_core::{LCD_HEIGHT, LCD_WIDTH};

pub(crate) use app::{configure_obsidian_style, RustboyApp};
pub use config::{DisplayFilter, ViewportScale};
pub use filters::FrontendBridge;
pub use theme::*;
use config::AppConfig;
use headless::CliConfig;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let msg = format!("{panic_info}");
        let _ = std::fs::write("rustboy-advance-crash.log", &msg);
        rfd::MessageDialog::new()
            .set_title("Rustboy Advance - Fatal Crash")
            .set_description(&msg)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }));
}

fn show_startup_error(title: &str, description: &str) {
    let _ = std::fs::write("rustboy-advance-startup-error.log", description);
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(description)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn main() {
    install_panic_hook();

    let cli = match CliConfig::parse() {
        Ok(c) => c,
        Err(e) => {
            show_startup_error("Argument Error", &format!("Failed to parse command-line arguments:\n{e}"));
            std::process::exit(2);
        }
    };

    if cli.headless {
        if let Err(e) = headless::run_headless(&cli) {
            show_startup_error("Headless Error", &format!("Error executing headless session:\n{e}"));
            std::process::exit(1);
        }
        return;
    }

    let initial_config = AppConfig::load();
    let scale = initial_config.window_scale.clamp(1, 5) as f32;
    let init_w = ((LCD_WIDTH as f32) * scale + 24.0).max(1020.0);
    let init_h = ((LCD_HEIGHT as f32) * scale + 92.0).max(680.0);

    if let Err(err) = app_host::run_desktop("Rustboy Advance", init_w, init_h, RustboyApp::new(cli))
    {
        show_startup_error("Desktop Frontend Error", &format!("Desktop initialization error:\n{err}"));
        std::process::exit(1);
    }
}
