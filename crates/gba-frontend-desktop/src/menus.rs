use std::fs;
use std::path::PathBuf;

use egui::{self, RichText, Stroke};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::app::{RustboyApp, TEXT_MUTED};
use crate::capture::GifFpsMode;
use crate::config::{
    AppConfig, BezelStyle, ColorProfileChoice, DisplayFilter, DmgPaletteChoice, HotkeyAction,
    ShaderPreset, ViewportScale,
};

fn toolbar_btn(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let text_color = if !enabled {
        crate::theme::COLOR_TEXT_DISABLED
    } else if active {
        crate::theme::ACCENT_TEAL
    } else {
        crate::theme::COLOR_TEXT
    };

    let border_stroke = if active && enabled {
        Stroke::new(1.0_f32, crate::theme::ACCENT_TEAL)
    } else {
        Stroke::new(1.0_f32, crate::theme::COLOR_BORDER)
    };

    let btn = egui::Button::new(RichText::new(label).color(text_color))
        .fill(crate::theme::COLOR_SURFACE)
        .stroke(border_stroke)
        .corner_radius(crate::theme::RADIUS_DEFAULT);

    ui.add_enabled(enabled, btn.min_size(egui::vec2(0.0, 26.0)))
}

impl RustboyApp {
    // WAVE-2 HOOK: Top menu bar — wave-2 agents should add menu entries here for
    // GLSL Shaders, Lua Script Console, RetroAchievements, Movies, and Link Cable.
    pub(crate) fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_menu_bar")
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::COLOR_SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .stroke(Stroke::new(1.0_f32, crate::theme::COLOR_BORDER)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    let has_rom = self.has_active_game();

                    ui.menu_button("File", |ui| {
                        if ui.button("Open ROM... (Ctrl+O)").clicked() {
                            ui.close_menu();
                            self.trigger_open_rom_dialog(ctx);
                        }
                        if ui.button("Add ROM Folder...").clicked() {
                            ui.close_menu();
                            self.add_rom_folder_dialog();
                        }
                        if ui.button("Rescan ROM Folders").clicked() {
                            ui.close_menu();
                            self.rescan_rom_folders();
                        }
                        ui.menu_button("Open Recent ROM", |ui| {
                            if self.config.recent_roms.is_empty() {
                                ui.label(RichText::new("No recent ROMs").color(TEXT_MUTED));
                            } else {
                                let recent = self.config.recent_roms.clone();
                                for entry in recent {
                                    let lbl = format!("{} ({})", entry.title, entry.code);
                                    if ui.button(lbl).on_hover_text(&entry.path).clicked() {
                                        ui.close_menu();
                                        let p = PathBuf::from(&entry.path);
                                        if let Err(e) = self.open_rom(&p, None, Some(ctx)) {
                                            self.notify_error(&e);
                                        }
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear Recent List").clicked() {
                                    self.config.recent_roms.clear();
                                    self.config.save();
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.separator();
                        if ui.button("Load Custom BIOS...").clicked() {
                            ui.close_menu();
                            self.load_custom_bios_dialog();
                        }
                        if ui
                            .add_enabled(
                                self.bios_bytes.is_some(),
                                egui::Button::new("Clear Custom BIOS (Use HLE)"),
                            )
                            .clicked()
                        {
                            ui.close_menu();
                            self.clear_custom_bios();
                        }
                        ui.separator();
                        ui.add_enabled_ui(has_rom, |ui| {
                            if ui
                                .button("⏹ Close Game / Return to Library (Esc / Ctrl+W)")
                                .clicked()
                            {
                                ui.close_menu();
                                self.close_rom(ctx);
                            }
                            if ui.button("Load Patch (BPS / UPS / IPS)...").clicked() {
                                ui.close_menu();
                                self.load_patch_dialog(ctx);
                            }
                            ui.separator();
                            let hardcore = self.is_ra_hardcore_active();
                            ui.add_enabled_ui(!hardcore, |ui| {
                                ui.menu_button("Save State", |ui| {
                                    for s in 1..=5 {
                                        if ui.button(format!("Slot {s} (F{s})")).clicked() {
                                            ui.close_menu();
                                            self.quick_save(s);
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("🖼️ Save State Manager... (F6)").clicked() {
                                        ui.close_menu();
                                        self.show_state_manager = true;
                                    }
                                    if ui.button("↩ Undo State Action (Ctrl+Z)").clicked() {
                                        ui.close_menu();
                                        self.undo_last_state_action();
                                    }
                                    ui.separator();
                                    if ui.button("Save State to File...").clicked() {
                                        ui.close_menu();
                                        self.save_state_to_file();
                                    }
                                });
                                ui.menu_button("Load State", |ui| {
                                    for s in 1..=5 {
                                        let exists = self.slot_path(s).is_some_and(|p| p.exists());
                                        let lbl = if exists {
                                            format!("Slot {s} (Shift+F{s}) [Saved]")
                                        } else {
                                            format!("Slot {s} (Shift+F{s})")
                                        };
                                        if ui.add_enabled(exists, egui::Button::new(lbl)).clicked() {
                                            ui.close_menu();
                                            self.quick_load(s);
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("🖼️ Save State Manager... (F6)").clicked() {
                                        ui.close_menu();
                                        self.show_state_manager = true;
                                    }
                                    ui.separator();
                                    if ui.button("Load State from File...").clicked() {
                                        ui.close_menu();
                                        self.load_state_from_file();
                                    }
                                });
                            });
                            ui.separator();
                            if ui.button("Import Battery Save (.sav)...").clicked() {
                                ui.close_menu();
                                self.import_battery_save();
                            }
                            if ui.button("Export Battery Save (.sav)...").clicked() {
                                ui.close_menu();
                                self.export_battery_save();
                            }
                            if ui.button("Save Screenshot (F12)").clicked() {
                                ui.close_menu();
                                self.capture_screenshot();
                            }
                        });
                        ui.separator();
                        if ui.button("Exit (Alt+F4)").clicked() {
                            self.quit_requested = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button("Emulation", |ui| {
                        ui.add_enabled_ui(has_rom, |ui| {
                            let p_lbl = if self.paused {
                                "Resume (Space)"
                            } else {
                                "Pause (Space)"
                            };
                            if ui.button(p_lbl).clicked() {
                                self.toggle_pause();
                                ui.close_menu();
                            }
                            let hardcore = self.is_ra_hardcore_active();
                            if ui
                                .add_enabled(
                                    !hardcore,
                                    egui::Button::new("Frame Advance / Step 1 Frame (Ctrl+N)"),
                                )
                                .clicked()
                            {
                                self.step_single_frame(ctx);
                                ui.close_menu();
                            }
                            if ui
                                .add_enabled(
                                    !hardcore,
                                    egui::Button::new("⏪ Rewind Step (Hold R)"),
                                )
                                .clicked()
                            {
                                self.rewind_manual_with_movie_sync();
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Soft Reset (SWI 00h)").clicked() {
                                self.soft_reset();
                                ui.close_menu();
                            }
                            if ui.button("Hard Reset (Ctrl+R)").clicked() {
                                self.hard_reset();
                                ui.close_menu();
                            }
                            if ui.button("⏹ Stop & Return to Library (Esc)").clicked() {
                                self.close_rom(ctx);
                                ui.close_menu();
                            }
                        });
                        ui.separator();
                        ui.menu_button("Speed Multiplier", |ui| {
                            let active_speed = self.effective_speed_multiplier();
                            let ff_hotkey = self
                                .config
                                .keybindings
                                .hotkeys
                                .display(HotkeyAction::FastForward);
                            for (mult, lbl) in [
                                (0.5, "0.5x Slow Motion"),
                                (1.0, "1.0x Normal"),
                                (1.5, "1.5x Crisp"),
                                (2.0, "2.0x Fast"),
                                (3.0, "3.0x Turbo"),
                                (4.0, "4.0x Ultra"),
                                (0.0, "Unthrottled Turbo"),
                            ] {
                                let selected = (active_speed - mult).abs() < 0.01;
                                if ui.selectable_label(selected, lbl).clicked() {
                                    if self.has_per_game_speed_override() {
                                        self.session_speed_override = Some(mult);
                                    } else {
                                        self.session_speed_override = None;
                                        self.config.speed_multiplier = mult;
                                        self.config.save();
                                    }
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            ui.label(
                                RichText::new(format!("Hold {ff_hotkey} for Fast-Forward"))
                                    .small()
                                    .color(TEXT_MUTED),
                            );
                        });
                        ui.menu_button("Fast-Forward Speed", |ui| {
                            for (mult, lbl) in [
                                (2.0, "2x Fast"),
                                (3.0, "3x Turbo"),
                                (4.0, "4x Ultra"),
                                (0.0, "Unlimited (Unthrottled)"),
                            ] {
                                let selected = (self.config.fast_forward_speed - mult).abs() < 0.01;
                                if ui.selectable_label(selected, lbl).clicked() {
                                    self.config.fast_forward_speed = mult;
                                    self.config.save();
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui
                            .add_enabled(
                                !self.is_ra_hardcore_active(),
                                egui::Button::new("Rewind Settings..."),
                            )
                            .clicked()
                        {
                            self.show_rewind_settings = true;
                            ui.close_menu();
                        }
                        ui.menu_button("PPU Accuracy", |ui| {
                            if ui
                                .radio(!self.config.dot_accurate_ppu, "Fast Scanline Renderer")
                                .clicked()
                            {
                                self.config.dot_accurate_ppu = false;
                                if let Some(core) = &mut self.core {
                                    core.bus.ppu.dot_accurate_mode = false;
                                }
                                self.config.save();
                                ui.close_menu();
                            }
                            if ui
                                .radio(
                                    self.config.dot_accurate_ppu,
                                    "Cycle-Accurate Per-Dot Renderer",
                                )
                                .clicked()
                            {
                                self.config.dot_accurate_ppu = true;
                                if let Some(core) = &mut self.core {
                                    core.bus.ppu.dot_accurate_mode = true;
                                }
                                self.config.save();
                                ui.close_menu();
                            }
                        });
                    });

                    ui.menu_button("Audio/Video", |ui| {
                        ui.menu_button("Window Scale", |ui| {
                            for s in 1..=5u8 {
                                if ui
                                    .selectable_label(
                                        self.config.window_scale == s,
                                        format!(
                                            "{s}x ({}x{})",
                                            LCD_WIDTH * usize::from(s),
                                            LCD_HEIGHT * usize::from(s)
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.set_window_scale(ctx, s);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui
                                .checkbox(&mut self.config.integer_scaling, "Integer Scaling Lock")
                                .changed()
                            {
                                self.config.save();
                            }
                            if ui
                                .selectable_label(self.is_fullscreen, "Fullscreen (F11)")
                                .clicked()
                            {
                                self.is_fullscreen = !self.is_fullscreen;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                    self.is_fullscreen,
                                ));
                                ui.close_menu();
                            }
                        });
                        ui.menu_button(
                            format!("Viewport Size ({})", self.config.viewport_scale.toolbar_label()),
                            |ui| {
                                for scale in ViewportScale::ALL {
                                    if ui
                                        .selectable_label(
                                            self.config.viewport_scale == scale,
                                            scale.description(),
                                        )
                                        .clicked()
                                    {
                                        self.config.viewport_scale = scale;
                                        self.config.save();
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                        let active_shader = self.config.effective_shader_preset();
                        ui.menu_button(
                            format!("Shader Preset ({})", active_shader.short_label()),
                            |ui| {
                                for preset in ShaderPreset::ALL {
                                    if ui
                                        .selectable_label(active_shader == preset, preset.label())
                                        .clicked()
                                    {
                                        self.config.shader_preset = preset;
                                        self.config.scanline_grid = preset == ShaderPreset::LcdGrid;
                                        self.config.save();
                                        self.sink.frame_dirty = true;
                                        ui.close_menu();
                                    }
                                }
                            },
                        );
                        ui.menu_button("Display Filters", |ui| {
                            ui.menu_button(
                                format!("Filter Mode ({})", self.config.display_filter.label()),
                                |ui| {
                                    for filter in DisplayFilter::ALL {
                                        if ui
                                            .selectable_label(
                                                self.config.display_filter == filter,
                                                filter.description(),
                                            )
                                            .clicked()
                                        {
                                            self.config.display_filter = filter;
                                            self.config.shader_preset = match filter {
                                                DisplayFilter::XbrSmooth3x => ShaderPreset::Nearest,
                                                DisplayFilter::AuthenticGbaLcd => ShaderPreset::LcdGrid,
                                                DisplayFilter::SoftHandheld => ShaderPreset::SharpBilinear,
                                                DisplayFilter::SmoothScale2x => ShaderPreset::ScaleEdge,
                                                DisplayFilter::RawNearest => ShaderPreset::Nearest,
                                            };
                                            self.config.scanline_grid = filter == DisplayFilter::AuthenticGbaLcd;
                                            self.config.save();
                                            self.sink.frame_dirty = true;
                                            self.lcd_tex = None;
                                            ui.close_menu();
                                        }
                                    }
                                },
                            );
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("LCD Grid Intensity:");
                                let slider = ui.add(
                                    egui::Slider::new(&mut self.config.lcd_grid_intensity, 0.0..=0.25)
                                        .custom_formatter(|n, _| format!("{:.0}%", n * 100.0)),
                                );
                                if slider.changed() {
                                    self.config.save();
                                    self.sink.frame_dirty = true;
                                    self.lcd_tex = None;
                                }
                            });
                            ui.separator();
                            if ui
                                .checkbox(
                                    &mut self.config.color_correction,
                                    "Enable Color Correction",
                                )
                                .changed()
                            {
                                self.config.save();
                                self.sink.frame_dirty = true;
                            }
                            ui.menu_button("Color Profile Preset", |ui| {
                                for p in ColorProfileChoice::ALL {
                                    if ui
                                        .selectable_label(
                                            self.config.color_profile == p,
                                            p.label(),
                                        )
                                        .clicked()
                                    {
                                        self.config.color_profile = p;
                                        self.config.color_correction =
                                            p != ColorProfileChoice::Linear;
                                        self.config.save();
                                        self.sink.frame_dirty = true;
                                        ui.close_menu();
                                    }
                                }
                            });
                            ui.separator();
                            if ui
                                .checkbox(
                                    &mut self.config.frame_blending,
                                    "Frame Blending (Flicker Fix)",
                                )
                                .on_hover_text(
                                    "50/50 temporal blend between previous and current frame to smooth GBA flicker transparency",
                                )
                                .changed()
                            {
                                self.config.save();
                                self.sink.frame_dirty = true;
                            }
                            if ui
                                .checkbox(&mut self.config.lcd_ghosting, "LCD Interframe Ghosting")
                                .changed()
                            {
                                self.config.save();
                                self.sink.frame_dirty = true;
                            }
                        });
                        ui.menu_button("GBA Screen Bezel", |ui| {
                            for b in BezelStyle::ALL {
                                if ui
                                    .selectable_label(self.config.bezel_style == b, b.label())
                                    .clicked()
                                {
                                    self.config.bezel_style = b;
                                    self.config.save();
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button("DMG Color Palette", |ui| {
                            for p in [
                                DmgPaletteChoice::Greyscale,
                                DmgPaletteChoice::GbaColorized,
                                DmgPaletteChoice::ClassicGreen,
                            ] {
                                if ui
                                    .selectable_label(self.config.dmg_palette == p, p.label())
                                    .clicked()
                                {
                                    self.apply_dmg_palette(p);
                                    ui.close_menu();
                                }
                            }
                            if !self.is_gb_active() {
                                ui.separator();
                                ui.label(
                                    RichText::new("Applies to monochrome .gb titles")
                                        .small()
                                        .color(TEXT_MUTED),
                                );
                            }
                        });
                        ui.menu_button("UI Panels & Chrome", |ui| {
                            if ui
                                .checkbox(
                                    &mut self.config.show_toolbar,
                                    "Show Action Toolbar (Ctrl+T)",
                                )
                                .changed()
                            {
                                self.config.save();
                            }
                            if ui
                                .checkbox(
                                    &mut self.config.show_status_bar,
                                    "Show Status Bar (Ctrl+S)",
                                )
                                .changed()
                            {
                                self.config.save();
                            }
                            if ui
                                .checkbox(&mut self.config.show_menu_bar, "Show Menu Bar in Window")
                                .changed()
                            {
                                self.config.save();
                            }
                        });
                        ui.menu_button("Video Layers", |ui| {
                            for (bit, name) in [
                                (0, "BG0 Layer"),
                                (1, "BG1 Layer"),
                                (2, "BG2 Layer"),
                                (3, "BG3 Layer"),
                                (4, "OBJ (Sprites) Layer"),
                            ] {
                                let mut on = (self.config.layer_mask & (1 << bit)) != 0;
                                if ui.checkbox(&mut on, name).changed() {
                                    if on {
                                        self.config.layer_mask |= 1 << bit;
                                    } else {
                                        self.config.layer_mask &= !(1 << bit);
                                    }
                                    if let Some(core) = &mut self.core {
                                        core.bus.ppu.layer_enable_mask = self.config.layer_mask;
                                    }
                                    self.config.save();
                                }
                            }
                        });
                        ui.menu_button("Audio Channels", |ui| {
                            for (bit, name) in [
                                (0, "PSG Square 1"),
                                (1, "PSG Square 2"),
                                (2, "PSG Wave"),
                                (3, "PSG Noise"),
                                (4, "Direct Sound FIFO A"),
                                (5, "Direct Sound FIFO B"),
                            ] {
                                let mut on = (self.config.audio_channel_mask & (1 << bit)) != 0;
                                if ui.checkbox(&mut on, name).changed() {
                                    if on {
                                        self.config.audio_channel_mask |= 1 << bit;
                                    } else {
                                        self.config.audio_channel_mask &= !(1 << bit);
                                    }
                                    if let Some(core) = &mut self.core {
                                        core.bus.apu.channel_enable_mask =
                                            self.config.audio_channel_mask;
                                    }
                                    self.config.save();
                                }
                            }
                        });
                        ui.separator();
                        if ui
                            .add(
                                egui::Slider::new(&mut self.config.master_volume, 0..=100)
                                    .text("Master Volume %"),
                            )
                            .changed()
                        {
                            if let Some(core) = &mut self.core {
                                core.bus.apu.master_volume_pct = self.config.master_volume;
                            }
                            self.config.save();
                        }
                        if ui
                            .checkbox(&mut self.config.muted, "Mute Audio (Ctrl+M)")
                            .changed()
                        {
                            self.sink.audio.set_muted(self.config.muted);
                            self.config.save();
                        }
                    });

                    ui.menu_button("Tools", |ui| {
                        let hardcore = self.is_ra_hardcore_active();
                        if ui
                            .add_enabled(
                                !hardcore,
                                egui::Button::new("🖼️ Save State Manager... (F6)"),
                            )
                            .clicked()
                        {
                            self.show_state_manager = true;
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(
                                !hardcore,
                                egui::Button::new("↩ Undo State Action (Ctrl+Z)"),
                            )
                            .clicked()
                        {
                            self.undo_last_state_action();
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(has_rom, egui::Button::new("Game Settings..."))
                            .clicked()
                        {
                            self.open_game_settings_for_active_rom();
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(has_rom, egui::Button::new("Game Overrides & Sensors..."))
                            .clicked()
                        {
                            self.debugger.show_overrides = true;
                            ui.close_menu();
                        }
                        if ui.button("Memory Search / Cheat Finder...").clicked() {
                            self.debugger.mem_search.open = true;
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(!hardcore, egui::Button::new("Cheat Code Manager..."))
                            .clicked()
                        {
                            self.debugger.show_cheats = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Lua Scripting Console...").clicked() {
                            self.lua.show_console = true;
                            ui.close_menu();
                        }
                        if ui.button("RetroAchievements...").clicked() {
                            self.ra.show_window = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Controls & Keybindings...").clicked() {
                            self.show_controls_dialog = true;
                            ui.close_menu();
                        }
                        if ui.button("🔗 Link Cable / Netplay...").clicked() {
                            self.link.show_window = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        let movie_lbl = if self.movie.is_recording() {
                            format!("Input Movies (TAS)... [REC {}f]", self.movie.current_frame())
                        } else if self.movie.is_playing() {
                            format!(
                                "Input Movies (TAS)... [PLAY {}/{}]",
                                self.movie.current_frame(),
                                self.movie.total_frames()
                            )
                        } else {
                            "Input Movies (TAS)...".to_string()
                        };
                        if ui.button(movie_lbl).clicked() {
                            self.show_movies = true;
                            ui.close_menu();
                        }
                        ui.menu_button("Capture GIF / WAV", |ui| {
                            let gif_rec = self.capture.is_gif_recording();
                            let gif_lbl = if gif_rec {
                                format!("Stop GIF Recording ({}f)", self.capture.gif_frames())
                            } else {
                                "Start GIF Recording".to_string()
                            };
                            if ui
                                .add_enabled(has_rom || gif_rec, egui::Button::new(gif_lbl))
                                .clicked()
                            {
                                self.capture_toggle_gif();
                                ui.close_menu();
                            }
                            ui.add_enabled_ui(!gif_rec, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("GIF Rate:").small().color(TEXT_MUTED));
                                    ui.radio_value(
                                        &mut self.capture.gif_fps_mode,
                                        GifFpsMode::Fps30,
                                        "30fps",
                                    );
                                    ui.radio_value(
                                        &mut self.capture.gif_fps_mode,
                                        GifFpsMode::Fps60,
                                        "60fps",
                                    );
                                });
                            });
                            ui.separator();
                            let wav_rec = self.capture.is_wav_recording();
                            let wav_lbl = if wav_rec {
                                format!(
                                    "Stop WAV Audio Recording ({:.1}s)",
                                    self.capture.wav_duration_secs()
                                )
                            } else {
                                "Start WAV Audio Recording".to_string()
                            };
                            if ui
                                .add_enabled(has_rom || wav_rec, egui::Button::new(wav_lbl))
                                .clicked()
                            {
                                self.capture_toggle_wav();
                                ui.close_menu();
                            }
                        });
                        ui.separator();
                        if ui
                            .checkbox(&mut self.config.rumble_enabled, "Gamepad Haptic Rumble")
                            .changed()
                        {
                            self.config.save();
                        }
                        ui.separator();
                        if ui.button("Open Screenshots / Captures Folder").clicked() {
                            let dir = AppConfig::screenshots_dir();
                            let _ = fs::create_dir_all(&dir);
                            #[cfg(target_os = "windows")]
                            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Debug / Views", |ui| {
                        ui.checkbox(&mut self.debugger.show_cpu, "ARM7TDMI CPU & Disassembler");
                        ui.checkbox(&mut self.debugger.show_hex, "Memory Hex Viewer");
                        ui.checkbox(&mut self.debugger.show_io_regs, "I/O Register Inspector");
                        ui.checkbox(&mut self.debugger.mem_search.open, "Memory Search / Cheat Finder...");
                        ui.checkbox(&mut self.debugger.show_gdb, "GDB Remote Server...");
                        ui.checkbox(&mut self.debugger.show_palette, "Palette Viewer");
                        ui.checkbox(&mut self.debugger.show_vram, "VRAM Tile Viewer");
                        ui.checkbox(&mut self.debugger.show_bg, "BG Map Viewer");
                        ui.checkbox(&mut self.debugger.show_oam, "OAM Sprite Viewer");
                        ui.checkbox(&mut self.debugger.show_audio, "Audio Channel Visualizer");
                        ui.separator();
                        if ui.button("About Rustboy Advance").clicked() {
                            self.debugger.show_about = true;
                            ui.close_menu();
                        }
                    });
                });
            });
    }

    pub(crate) fn draw_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("quick_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::COLOR_SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(crate::theme::STROKE_BORDER),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let has_rom = self.has_active_game();

                    // Group 1: Open, Folder, Reload
                    if toolbar_btn(ui, "📂 Open", false, true)
                        .on_hover_text("Open GBA / GB / GBC ROM File (Ctrl+O)")
                        .clicked()
                    {
                        self.trigger_open_rom_dialog(ctx);
                    }
                    if toolbar_btn(ui, "📁 Folder", false, true)
                        .on_hover_text("Add ROM Directory to Library")
                        .clicked()
                    {
                        self.add_rom_folder_dialog();
                    }
                    if toolbar_btn(ui, "🔄 Reload", false, true)
                        .on_hover_text("Reload current ROM or rescan folders")
                        .clicked()
                    {
                        if let Some(path) = self.rom_path.clone() {
                            let _ = self.open_rom(&path, None, Some(ctx));
                        } else {
                            self.rescan_rom_folders();
                        }
                    }

                    ui.separator();

                    // Group 2: Pause, Stop, Step, Rewind, Reset, FF
                    let play_lbl = if self.paused { "▶ Resume" } else { "⏸ Pause" };
                    if toolbar_btn(ui, play_lbl, self.paused, has_rom)
                        .on_hover_text("Pause / Resume emulation (P / Space)")
                        .clicked()
                    {
                        self.toggle_pause();
                    }
                    if toolbar_btn(ui, "⏹ Stop", false, has_rom)
                        .on_hover_text("Stop game and return to ROM Library (Esc / Ctrl+W)")
                        .clicked()
                    {
                        self.close_rom(ctx);
                    }
                    let hardcore = self.is_ra_hardcore_active();
                    if toolbar_btn(ui, "⏭ Step", false, has_rom && !hardcore)
                        .on_hover_text("Advance 1 Frame (Ctrl+N)")
                        .clicked()
                    {
                        self.step_single_frame(ctx);
                    }
                    let rewind_enabled =
                        has_rom && !hardcore && self.config.rewind_enabled && !self.rewind_buffer.is_empty();
                    if toolbar_btn(ui, "⏪ Rewind", false, rewind_enabled)
                        .on_hover_text("Rewind ~0.5s (or hold R key in-game)")
                        .clicked()
                    {
                        self.rewind_manual_with_movie_sync();
                    }
                    if toolbar_btn(ui, "⟲ Reset", false, has_rom)
                        .on_hover_text("Hard Reset (Ctrl+R)")
                        .clicked()
                    {
                        self.hard_reset();
                    }
                    let eff_speed = self.effective_speed_multiplier();
                    let turbo_on = self.tab_fast_forward || eff_speed > 1.05 || eff_speed <= 0.0;
                    if toolbar_btn(ui, "⏩ FF", turbo_on, has_rom)
                        .on_hover_text("Toggle Fast-Forward (Tab / hotkey)")
                        .clicked()
                    {
                        let target_speed = if eff_speed > 1.05 || eff_speed <= 0.0 {
                            1.0
                        } else {
                            self.config.fast_forward_speed
                        };
                        if self.has_per_game_speed_override() {
                            self.session_speed_override = Some(target_speed);
                        } else {
                            self.session_speed_override = None;
                            self.config.speed_multiplier = target_speed;
                            self.config.save();
                        }
                    }

                    ui.separator();

                    // Group 3: S1, L1, States, Snap
                    if toolbar_btn(ui, "💾 S1", false, has_rom)
                        .on_hover_text("Quick Save Slot 1 (F5)")
                        .clicked()
                    {
                        self.quick_save(1);
                    }
                    if toolbar_btn(ui, "📂 L1", false, has_rom)
                        .on_hover_text("Quick Load Slot 1 (F7)")
                        .clicked()
                    {
                        self.quick_load(1);
                    }
                    if toolbar_btn(ui, "◈ States", self.show_state_manager, true)
                        .on_hover_text("Open Visual Save State Manager (F6)")
                        .clicked()
                    {
                        self.show_state_manager = !self.show_state_manager;
                    }
                    if toolbar_btn(ui, "📷 Snap", false, has_rom)
                        .on_hover_text("Save Screenshot alongside ROM (F12)")
                        .clicked()
                    {
                        self.capture_screenshot();
                    }

                    ui.separator();

                    // Group 4: Audio, sRGB, Filter: LCD Matrix
                    let audio_active = !self.config.muted;
                    if toolbar_btn(ui, "🔊 Audio", audio_active, true)
                        .on_hover_text("Toggle Audio Output (M)")
                        .clicked()
                    {
                        self.config.muted = !self.config.muted;
                        self.sink.audio.set_muted(self.config.muted);
                        self.config.save();
                    }
                    if toolbar_btn(ui, "🎨 sRGB", self.config.color_correction, true)
                        .on_hover_text("Color Correction")
                        .clicked()
                    {
                        self.config.color_correction = !self.config.color_correction;
                        self.config.save();
                        self.sink.frame_dirty = true;
                    }

                    let filter_label = format!("🎛 Filter: {}", self.config.display_filter.label());
                    let filter_resp = toolbar_btn(ui, &filter_label, false, true);
                    if filter_resp.clicked() {
                        self.config.display_filter = self.config.display_filter.next();
                        self.config.shader_preset = match self.config.display_filter {
                            DisplayFilter::XbrSmooth3x => ShaderPreset::Nearest,
                            DisplayFilter::AuthenticGbaLcd => ShaderPreset::LcdGrid,
                            DisplayFilter::SoftHandheld => ShaderPreset::SharpBilinear,
                            DisplayFilter::SmoothScale2x => ShaderPreset::ScaleEdge,
                            DisplayFilter::RawNearest => ShaderPreset::Nearest,
                        };
                        self.config.scanline_grid =
                            self.config.display_filter == DisplayFilter::AuthenticGbaLcd;
                        self.config.save();
                        self.sink.frame_dirty = true;
                        self.lcd_tex = None;
                    }
                    let filter_desc = self.config.display_filter.description();
                    let filter_resp = filter_resp.on_hover_text(format!(
                        "GBA Display Filter: {filter_desc}\n(Click to cycle, right-click for quick menu)"
                    ));
                    filter_resp.context_menu(|ui| {
                        for f in DisplayFilter::ALL {
                            if ui
                                .selectable_label(self.config.display_filter == f, f.description())
                                .clicked()
                            {
                                self.config.display_filter = f;
                                self.config.shader_preset = match f {
                                    DisplayFilter::XbrSmooth3x => ShaderPreset::Nearest,
                                    DisplayFilter::AuthenticGbaLcd => ShaderPreset::LcdGrid,
                                    DisplayFilter::SoftHandheld => ShaderPreset::SharpBilinear,
                                    DisplayFilter::SmoothScale2x => ShaderPreset::ScaleEdge,
                                    DisplayFilter::RawNearest => ShaderPreset::Nearest,
                                };
                                self.config.scanline_grid = f == DisplayFilter::AuthenticGbaLcd;
                                self.config.save();
                                self.sink.frame_dirty = true;
                                self.lcd_tex = None;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Grid Intensity:");
                            let slider = ui.add(
                                egui::Slider::new(&mut self.config.lcd_grid_intensity, 0.0..=0.25)
                                    .custom_formatter(|n, _| format!("{:.0}%", n * 100.0)),
                            );
                            if slider.changed() {
                                self.config.save();
                                self.sink.frame_dirty = true;
                                self.lcd_tex = None;
                            }
                        });
                    });

                    ui.separator();

                    // Group 5: CPU, Hex, VRAM, Audio, Link
                    if toolbar_btn(ui, "CPU", self.debugger.show_cpu, true).clicked() {
                        self.debugger.show_cpu = !self.debugger.show_cpu;
                    }
                    if toolbar_btn(ui, "Hex", self.debugger.show_hex, true).clicked() {
                        self.debugger.show_hex = !self.debugger.show_hex;
                    }
                    if toolbar_btn(ui, "VRAM", self.debugger.show_vram, true).clicked() {
                        self.debugger.show_vram = !self.debugger.show_vram;
                    }
                    if toolbar_btn(ui, "Audio", self.debugger.show_audio, true).clicked() {
                        self.debugger.show_audio = !self.debugger.show_audio;
                    }
                    let link_on = self.link.local_is_active()
                        || self.link.netplay.as_ref().is_some_and(|n| n.is_active());
                    if toolbar_btn(ui, "Link", self.link.show_window || link_on, true)
                        .on_hover_text("GBA Link Cable (Local 2P & TCP Netplay)")
                        .clicked()
                    {
                        self.link.show_window = !self.link.show_window;
                    }
                });
            });
    }
}
