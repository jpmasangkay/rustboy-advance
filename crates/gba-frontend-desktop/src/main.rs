mod app;
mod app_host;
mod audio;
mod cheats;
mod config;
mod debugger_ui;
mod filters;
mod game_settings;
mod headless;
mod hotkeys;
mod hud;
mod input;
mod library;
mod lua_engine;
mod menus;
mod rewind;
mod rom_loader;
mod savestates;

use gba_core::{LCD_HEIGHT, LCD_WIDTH};

pub(crate) use app::{configure_obsidian_style, RustboyApp};
use config::AppConfig;
use headless::CliConfig;

fn main() {
    let cli = match CliConfig::parse() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Argument error: {e}");
            std::process::exit(2);
        }
    };

    if cli.headless {
        if let Err(e) = headless::run_headless(&cli) {
            eprintln!("Error: {e}");
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
        eprintln!("Desktop frontend error: {err}");
        std::process::exit(1);
    }
}
