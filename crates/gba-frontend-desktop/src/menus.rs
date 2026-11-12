use std::fs;
use std::path::PathBuf;

use egui::{self, Color32, RichText, Stroke};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::app::{
    RustboyApp, ACCENT_TEAL, ACCENT_TEAL_BG, BG_PANEL, BORDER_HAIRLINE, TEXT_MUTED,
};
use crate::config::{AppConfig, BezelStyle, ColorProfileChoice, HotkeyAction};

impl RustboyApp {
    // WAVE-2 HOOK: Top menu bar — wave-2 agents should add menu entries here for
    // GLSL Shaders, Lua Script Console, RetroAchievements, Movies, and Link Cable.
    pub(crate) fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_menu_bar")
            .frame(
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .inner_margin(4.0)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    let has_rom = self.core.is_some();

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
                            if ui.button("Frame Advance / Step 1 Frame (Ctrl+N)").clicked() {
                                self.step_single_frame(ctx);
                                ui.close_menu();
                            }
                            if ui.button("⏪ Rewind Step (Hold R)").clicked() {
                                self.step_rewind_manual();
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
                        if ui.button("Rewind Settings...").clicked() {
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
                        ui.menu_button("Display Filters", |ui| {
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
                                .checkbox(&mut self.config.lcd_ghosting, "LCD Interframe Ghosting")
                                .changed()
                            {
                                self.config.save();
                            }
                            if ui
                                .checkbox(
                                    &mut self.config.scanline_grid,
                                    "Subpixel Scanline Grid",
                                )
                                .changed()
                            {
                                self.config.save();
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
                        if ui.button("🖼️ Save State Manager... (F6)").clicked() {
                            self.show_state_manager = true;
                            ui.close_menu();
                        }
                        if ui.button("↩ Undo State Action (Ctrl+Z)").clicked() {
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
                        if ui.button("Cheat Code Manager...").clicked() {
                            self.debugger.show_cheats = true;
                            ui.close_menu();
                        }
                        if ui.button("Controls & Keybindings...").clicked() {
                            self.show_controls_dialog = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .checkbox(&mut self.config.rumble_enabled, "Gamepad Haptic Rumble")
                            .changed()
                        {
                            self.config.save();
                        }
                        ui.separator();
                        if ui.button("Open Screenshots Folder").clicked() {
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
        let toggle_btn = |ui: &mut egui::Ui, label: &str, active: bool| -> egui::Response {
            let mut btn = egui::Button::new(RichText::new(label).small());
            if active {
                btn = btn
                    .fill(ACCENT_TEAL_BG)
                    .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
            }
            ui.add(btn)
        };

        egui::TopBottomPanel::top("quick_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let has_rom = self.core.is_some();
                    if ui
                        .button(RichText::new("📂 Open ROM").small())
                        .on_hover_text("Open GBA ROM File (Ctrl+O)")
                        .clicked()
                    {
                        self.trigger_open_rom_dialog(ctx);
                    }
                    if ui
                        .button(RichText::new("📁 Add Folder").small())
                        .on_hover_text("Add ROM Directory to Library")
                        .clicked()
                    {
                        self.add_rom_folder_dialog();
                    }
                    if ui
                        .button(RichText::new("🔄 Refresh").small())
                        .on_hover_text("Rescan configured ROM folders")
                        .clicked()
                    {
                        self.rescan_rom_folders();
                    }

                    ui.separator();

                    ui.add_enabled_ui(has_rom, |ui| {
                        let play_lbl = if self.paused {
                            "▶ Resume"
                        } else {
                            "⏸ Pause"
                        };
                        if toggle_btn(ui, play_lbl, self.paused).clicked() {
                            self.toggle_pause();
                        }

                        // Prominent STOP button to exit running game back to ROM Library
                        let stop_btn = egui::Button::new(
                            RichText::new("⏹ Stop")
                                .color(Color32::from_rgb(255, 120, 120))
                                .small(),
                        );
                        if ui
                            .add(stop_btn)
                            .on_hover_text("Stop game and return to ROM Library (Esc / Ctrl+W)")
                            .clicked()
                        {
                            self.close_rom(ctx);
                        }

                        if ui
                            .button(RichText::new("⏭ Step").small())
                            .on_hover_text("Advance 1 Frame (Ctrl+N)")
                            .clicked()
                        {
                            self.step_single_frame(ctx);
                        }

                        let rewind_enabled =
                            self.config.rewind_enabled && !self.rewind_buffer.is_empty();
                        let rew_btn = egui::Button::new(RichText::new("⏪ Rewind").small());
                        if ui
                            .add_enabled(rewind_enabled, rew_btn)
                            .on_hover_text("Rewind ~0.5s (or hold R key in-game)")
                            .clicked()
                        {
                            self.step_rewind_manual();
                        }

                        if ui
                            .button(RichText::new("Reset").small())
                            .on_hover_text("Hard Reset (Ctrl+R)")
                            .clicked()
                        {
                            self.hard_reset();
                        }

                        let eff_speed = self.effective_speed_multiplier();
                        let turbo_on = self.tab_fast_forward || eff_speed > 1.05 || eff_speed <= 0.0;
                        let ff_hotkey = self
                            .config
                            .keybindings
                            .hotkeys
                            .display(HotkeyAction::FastForward);
                        let ff_label = if self.config.fast_forward_speed <= 0.0 {
                            "Unlimited".to_string()
                        } else {
                            format!("{:.0}x", self.config.fast_forward_speed)
                        };
                        if toggle_btn(ui, "FF >>", turbo_on)
                            .on_hover_text(format!(
                                "Toggle {ff_label} Fast-Forward (or Hold {ff_hotkey})"
                            ))
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

                        if ui.button(RichText::new("💾 Save 1").small()).clicked() {
                            self.quick_save(1);
                        }
                        if ui.button(RichText::new("📂 Load 1").small()).clicked() {
                            self.quick_load(1);
                        }
                        if ui
                            .button(RichText::new("🖼️ States").small())
                            .on_hover_text("Open Visual Save State Manager (F6)")
                            .clicked()
                        {
                            self.show_state_manager = true;
                        }
                        if self.undo_state_buffer.is_some() {
                            if ui
                                .button(RichText::new("↩ Undo").small())
                                .on_hover_text("Undo Last Save/Load (Ctrl+Z)")
                                .clicked()
                            {
                                self.undo_last_state_action();
                            }
                        }
                        if let Some(core) = &self.core {
                            if core.bus.cart.solar_enabled {
                                let lvl = ((core.bus.cart.solar_lux as f32 / 255.0) * 10.0).round()
                                    as u8;
                                ui.label(
                                    RichText::new(format!("☀️ {lvl}/10"))
                                        .small()
                                        .color(ACCENT_TEAL),
                                )
                                .on_hover_text("Boktai Solar Sensor Intensity (Hotkey: [ / ])");
                            }
                        }
                        if ui
                            .button(RichText::new("📷 Shot").small())
                            .on_hover_text("Save Screenshot alongside ROM (F12)")
                            .clicked()
                        {
                            self.capture_screenshot();
                        }

                        ui.separator();
                    });

                    if toggle_btn(
                        ui,
                        if self.config.muted { "Muted" } else { "Audio" },
                        self.config.muted,
                    )
                    .clicked()
                    {
                        self.config.muted = !self.config.muted;
                        self.sink.audio.set_muted(self.config.muted);
                        self.config.save();
                    }
                    if toggle_btn(ui, "sRGB", self.config.color_correction)
                        .on_hover_text("AGB-001 sRGB Color Correction")
                        .clicked()
                    {
                        self.config.color_correction = !self.config.color_correction;
                        self.config.save();
                    }
                    if toggle_btn(ui, "Grid", self.config.scanline_grid)
                        .on_hover_text("LCD Subpixel Scanline Grid")
                        .clicked()
                    {
                        self.config.scanline_grid = !self.config.scanline_grid;
                        self.config.save();
                    }

                    ui.separator();

                    if toggle_btn(ui, "CPU", self.debugger.show_cpu).clicked() {
                        self.debugger.show_cpu = !self.debugger.show_cpu;
                    }
                    if toggle_btn(ui, "Hex", self.debugger.show_hex).clicked() {
                        self.debugger.show_hex = !self.debugger.show_hex;
                    }
                    if toggle_btn(ui, "VRAM", self.debugger.show_vram).clicked() {
                        self.debugger.show_vram = !self.debugger.show_vram;
                    }
                    if toggle_btn(ui, "Audio", self.debugger.show_audio).clicked() {
                        self.debugger.show_audio = !self.debugger.show_audio;
                    }
                });
            });
    }
}
