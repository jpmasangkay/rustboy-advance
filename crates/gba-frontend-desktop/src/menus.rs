use std::path::PathBuf;

use egui::{self, RichText, Stroke};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::app::{RustboyApp, TEXT_MUTED};
use crate::capture::GifFpsMode;
use crate::config::{
    AppConfig, BezelStyle, ColorProfileChoice, DisplayFilter, DmgPaletteChoice, HotkeyAction,
    ViewportScale,
};
use crate::theme::{
    ACCENT_TEAL, ACCENT_TEAL_BG, COLOR_BORDER, COLOR_SURFACE, COLOR_TEXT, COLOR_TEXT_DISABLED,
    COLOR_TILE, RADIUS_DEFAULT, STROKE_BORDER, STROKE_TEAL,
};

const SPEED_PRESETS: [(f32, &str); 7] = [
    (0.5, "0.5x Slow Motion"),
    (1.0, "1.0x Normal"),
    (1.5, "1.5x Crisp"),
    (2.0, "2.0x Fast"),
    (3.0, "3.0x Turbo"),
    (4.0, "4.0x Ultra"),
    (0.0, "Unthrottled Turbo"),
];

const FAST_FORWARD_PRESETS: [(f32, &str); 4] = [
    (2.0, "2x Fast"),
    (3.0, "3x Turbo"),
    (4.0, "4x Ultra"),
    (0.0, "Unlimited (Unthrottled)"),
];

const VIDEO_LAYER_NAMES: [(u8, &str); 5] = [
    (0, "BG0 Layer"),
    (1, "BG1 Layer"),
    (2, "BG2 Layer"),
    (3, "BG3 Layer"),
    (4, "OBJ (Sprites) Layer"),
];

const AUDIO_CHANNEL_NAMES: [(u8, &str); 6] = [
    (0, "PSG Square 1"),
    (1, "PSG Square 2"),
    (2, "PSG Wave"),
    (3, "PSG Noise"),
    (4, "Direct Sound FIFO A"),
    (5, "Direct Sound FIFO B"),
];

fn toolbar_btn(ui: &mut egui::Ui, label: &str, active: bool, enabled: bool) -> egui::Response {
    let text_color = if !enabled {
        COLOR_TEXT_DISABLED
    } else if active {
        ACCENT_TEAL
    } else {
        COLOR_TEXT
    };
    let fill = if active {
        ACCENT_TEAL_BG
    } else if enabled {
        COLOR_TILE
    } else {
        COLOR_SURFACE
    };
    let border_stroke = if active { STROKE_TEAL } else { STROKE_BORDER };
    let btn = egui::Button::new(RichText::new(label).size(12.0).color(text_color))
        .fill(fill)
        .stroke(border_stroke)
        .corner_radius(RADIUS_DEFAULT);
    ui.add_enabled(enabled, btn.min_size(egui::vec2(0.0, 26.0)))
}

impl RustboyApp {
    // WAVE-2 HOOK: Top menu bar — wave-2 agents should add menu entries here for
    // GLSL Shaders, Lua Script Console, RetroAchievements, Movies, and Link Cable.
    pub(crate) fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_menu_bar")
            .frame(
                egui::Frame::new()
                    .fill(COLOR_SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .stroke(Stroke::new(1.0_f32, COLOR_BORDER)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    let has_rom = self.has_active_game();
                    let dev_tools = self.config.developer_tools;

                    ui.menu_button("File", |ui| {
                        self.draw_menu_file(ctx, ui, has_rom, dev_tools);
                    });
                    ui.menu_button("Play", |ui| {
                        self.draw_menu_play(ctx, ui, has_rom, dev_tools);
                    });
                    ui.menu_button("Screen & Sound", |ui| {
                        self.draw_menu_screen_and_sound(ctx, ui, dev_tools);
                    });
                    ui.menu_button("Tools", |ui| {
                        self.draw_menu_tools(ui, has_rom, dev_tools);
                    });
                    ui.menu_button("Help", |ui| {
                        self.draw_menu_help(ui);
                    });
                });
            });
    }

    fn draw_menu_file(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        has_rom: bool,
        dev_tools: bool,
    ) {
        let snap_key = self.config.keybindings.hotkeys.display(HotkeyAction::Screenshot);
        let state_mgr_key = self
            .config
            .keybindings
            .hotkeys
            .display(HotkeyAction::StateManager);

        if ui.button("Open ROM... (Ctrl+O)").clicked() {
            ui.close_menu();
            self.trigger_open_rom_dialog(ctx);
        }
        if ui
            .button("Add Game Folder...")
            .on_hover_text("Add a folder to scan for GBA / GB / GBC games")
            .clicked()
        {
            ui.close_menu();
            self.add_rom_folder_dialog();
        }
        if ui
            .button("Refresh Library")
            .on_hover_text("Rescan ROM folders for added or removed files")
            .clicked()
        {
            ui.close_menu();
            self.rescan_rom_folders();
        }

        ui.menu_button("Open Recent", |ui| {
            if self.config.recent_roms.is_empty() {
                ui.label(RichText::new("No recent games").small().color(TEXT_MUTED));
            } else {
                let recents = self.config.recent_roms.clone();
                for entry in recents {
                    let lbl = if entry.code.is_empty() {
                        entry.title.clone()
                    } else {
                        format!("{} ({})", entry.title, entry.code)
                    };
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

        if dev_tools {
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
        }

        ui.separator();
        ui.add_enabled_ui(has_rom, |ui| {
            if ui.button("Close Game / Return to Library (Ctrl+W)").clicked() {
                ui.close_menu();
                self.close_rom(ctx);
            }
            if ui
                .button("Apply ROM Patch...")
                .on_hover_text("Load Patch (BPS / UPS / IPS)")
                .clicked()
            {
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
                    if ui
                        .button(format!("Save State Manager... ({state_mgr_key})"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.show_state_manager = true;
                    }
                    if ui.button("Undo State Action (Ctrl+Z)").clicked() {
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
                        let exists = self.existing_slot_path(s).is_some();
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
                    if ui
                        .button(format!("Save State Manager... ({state_mgr_key})"))
                        .clicked()
                    {
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
            if ui
                .button("Import Game Save...")
                .on_hover_text("Import Battery Save (.sav)")
                .clicked()
            {
                ui.close_menu();
                self.import_battery_save();
            }
            if ui
                .button("Export Game Save...")
                .on_hover_text("Export Battery Save (.sav)")
                .clicked()
            {
                ui.close_menu();
                self.export_battery_save();
            }
            if ui.button(format!("Save Screenshot ({snap_key})")).clicked() {
                ui.close_menu();
                self.capture_screenshot();
            }
        });

        ui.separator();
        if ui.button("Settings...").clicked() {
            self.show_settings = true;
            ui.close_menu();
        }
        if ui.button("Exit (Alt+F4)").clicked() {
            self.quit_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn draw_menu_play(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        has_rom: bool,
        dev_tools: bool,
    ) {
        let hotkeys = &self.config.keybindings.hotkeys;
        let pause_key = hotkeys.display(HotkeyAction::Pause);
        let adv_key = hotkeys.display(HotkeyAction::FrameAdvance);
        let rewind_key = hotkeys.display(HotkeyAction::Rewind);
        let reset_key = hotkeys.display(HotkeyAction::HardReset);
        let ff_hotkey = hotkeys.display(HotkeyAction::FastForward);

        ui.add_enabled_ui(has_rom, |ui| {
            let p_lbl = if self.paused {
                format!("Resume ({pause_key})")
            } else {
                format!("Pause ({pause_key})")
            };
            if ui.button(p_lbl).clicked() {
                self.toggle_pause();
                ui.close_menu();
            }

            let hardcore = self.is_ra_hardcore_active();
            if dev_tools
                && ui
                    .add_enabled(
                        !hardcore,
                        egui::Button::new(format!("Step 1 Frame ({adv_key})")),
                    )
                    .on_hover_text("Frame Advance — advance emulation by one video frame")
                    .clicked()
            {
                self.step_single_frame(ctx);
                ui.close_menu();
            }

            if ui
                .add_enabled(
                    !hardcore && self.config.rewind_enabled,
                    egui::Button::new(format!("Rewind Step (Hold {rewind_key})")),
                )
                .clicked()
            {
                self.rewind_manual_with_movie_sync();
                ui.close_menu();
            }

            ui.separator();
            if ui
                .button(format!("Restart game ({reset_key})"))
                .on_hover_text("Hard Reset — restart the console from boot")
                .clicked()
            {
                self.hard_reset();
                ui.close_menu();
            }

            if dev_tools
                && ui
                    .button("Soft Reset (SWI 00h)")
                    .on_hover_text("Software Reset via BIOS SWI 00h (may skip cartridge header init)")
                    .clicked()
            {
                self.soft_reset();
                ui.close_menu();
            }

            if ui.button("Stop & Return to Library (Ctrl+W)").clicked() {
                self.close_rom(ctx);
                ui.close_menu();
            }
        });

        ui.separator();
        ui.menu_button("Game Speed", |ui| {
            let active_speed = self.effective_speed_multiplier();
            for (mult, lbl) in SPEED_PRESETS {
                let selected = (active_speed - mult).abs() < 0.05;
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
                RichText::new(format!("Tip: Hold Tab (or {ff_hotkey}) for Fast-Forward"))
                    .small()
                    .color(TEXT_MUTED),
            );
        });

        ui.menu_button("Fast-Forward Speed", |ui| {
            for (mult, lbl) in FAST_FORWARD_PRESETS {
                let selected = (self.config.fast_forward_speed - mult).abs() < 0.05;
                if ui.selectable_label(selected, lbl).clicked() {
                    self.config.fast_forward_speed = mult;
                    self.config.save();
                    ui.close_menu();
                }
            }
        });

        ui.separator();
        let mut rewind = self.config.rewind_enabled;
        if ui.checkbox(&mut rewind, "Enable Rewind").changed() {
            self.config.rewind_enabled = rewind;
            if !rewind {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
            }
            self.config.save();
        }
        if ui
            .add_enabled(
                self.config.rewind_enabled && !self.is_ra_hardcore_active(),
                egui::Button::new("Rewind Settings..."),
            )
            .clicked()
        {
            self.show_rewind_settings = true;
            ui.close_menu();
        }

        ui.menu_button("Reduce Input Lag", |ui| {
            if ui
                .radio(self.config.run_ahead_frames == 0, "Off (Default)")
                .on_hover_text("Run-Ahead Latency: 0 frames")
                .clicked()
            {
                self.config.run_ahead_frames = 0;
                self.config.save();
            }
            if ui
                .radio(
                    self.config.run_ahead_frames == 1,
                    "1 Frame Lower Latency (-16.7ms)",
                )
                .on_hover_text("Run-Ahead Latency: 1 frame (uses more CPU)")
                .clicked()
            {
                self.config.run_ahead_frames = 1;
                self.config.save();
            }
        })
        .response
        .on_hover_text("Run-Ahead Latency: predict frames to trim internal game input delay");

        ui.menu_button("Graphics Accuracy", |ui| {
            if ui
                .radio(!self.config.dot_accurate_ppu, "Fast (Scanline)")
                .on_hover_text("Fast Scanline Renderer — best speed for most games")
                .clicked()
            {
                self.config.dot_accurate_ppu = false;
                if let Some(core) = &mut self.core {
                    core.bus.ppu.dot_accurate_mode = false;
                }
                self.config.save();
            }
            if ui
                .radio(self.config.dot_accurate_ppu, "High (Per-Dot Accurate)")
                .on_hover_text("Cycle-Accurate Per-Dot PPU — fixes rare mid-scanline effects")
                .clicked()
            {
                self.config.dot_accurate_ppu = true;
                if let Some(core) = &mut self.core {
                    core.bus.ppu.dot_accurate_mode = true;
                }
                self.config.save();
            }
        })
        .response
        .on_hover_text("PPU Accuracy mode");

        if ui
            .checkbox(&mut self.config.auto_frameskip, "Skip Frames If Slow")
            .on_hover_text("Auto Frame-Skip: drop rendered video frames on slow hardware to keep sound smooth")
            .clicked()
        {
            self.config.save();
        }
    }

    fn draw_menu_screen_and_sound(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        dev_tools: bool,
    ) {
        let fs_key = self
            .config
            .keybindings
            .hotkeys
            .display(HotkeyAction::Fullscreen);
        let mute_key = self.config.keybindings.hotkeys.display(HotkeyAction::Mute);

        ui.menu_button("Window Size", |ui| {
            for s in 1..=6u8 {
                if ui
                    .selectable_label(
                        self.config.window_scale == s,
                        format!("{s}x ({}x{})", LCD_WIDTH * usize::from(s), LCD_HEIGHT * usize::from(s)),
                    )
                    .clicked()
                {
                    self.set_window_scale(ctx, s);
                    ui.close_menu();
                }
            }
            ui.separator();
            if ui
                .checkbox(&mut self.config.integer_scaling, "Lock Integer Scaling")
                .changed()
            {
                self.config.save();
            }
            if ui
                .selectable_label(self.is_fullscreen, format!("Fullscreen ({fs_key})"))
                .clicked()
            {
                self.is_fullscreen = !self.is_fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                ui.close_menu();
            }
        })
        .response
        .on_hover_text("Window Scale and Fullscreen toggle");

        ui.menu_button(
            format!("Screen Fill ({})", self.config.viewport_scale.label()),
            |ui| {
                for scale in ViewportScale::ALL {
                    if ui
                        .selectable_label(self.config.viewport_scale == scale, scale.description())
                        .clicked()
                    {
                        self.config.viewport_scale = scale;
                        self.config.save();
                        ui.close_menu();
                    }
                }
            },
        )
        .response
        .on_hover_text("Viewport Size inside the window");

        ui.menu_button("Display Filters", |ui| {
            ui.menu_button(
                format!("Filter: {}", self.config.display_filter.label()),
                |ui| {
                    for filter in DisplayFilter::ALL {
                        if ui
                            .selectable_label(
                                self.config.display_filter == filter,
                                filter.description(),
                            )
                            .clicked()
                        {
                            self.set_display_filter_from_user(filter);
                            ui.close_menu();
                        }
                    }
                },
            );
            if ui
                .checkbox(
                    &mut self.config.auto_quality_governor,
                    "Auto Performance Tuning",
                )
                .on_hover_text("Auto Quality Governor: step down heavy GPU filters automatically if frames drop")
                .clicked()
            {
                self.quality_governor
                    .set_enabled(self.config.auto_quality_governor);
                self.config.save();
            }
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
                .checkbox(&mut self.config.color_correction, "Accurate Screen Colors")
                .on_hover_text("Color Correction (sRGB / GBA LCD gamma profile)")
                .changed()
            {
                self.config.save();
                self.sink.frame_dirty = true;
            }
            ui.menu_button("Color Profile", |ui| {
                for p in ColorProfileChoice::ALL {
                    if ui
                        .selectable_label(self.config.color_profile == p, p.label())
                        .clicked()
                    {
                        self.config.color_profile = p;
                        self.config.color_correction = p != ColorProfileChoice::Linear;
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
                    "Reduce Transparency Flicker",
                )
                .on_hover_text("Frame Blending: blend consecutive frames for GBA flicker shadows")
                .changed()
            {
                self.config.save();
                self.sink.frame_dirty = true;
            }
            if ui
                .checkbox(&mut self.config.lcd_ghosting, "Original Screen Blur")
                .on_hover_text("LCD Interframe Ghosting")
                .changed()
            {
                self.config.save();
                self.sink.frame_dirty = true;
            }
        });

        ui.menu_button("Screen Border", |ui| {
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
        })
        .response
        .on_hover_text("GBA Screen Bezel frame around gameplay");

        ui.menu_button("Original Game Boy Colors", |ui| {
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
        })
        .response
        .on_hover_text("DMG Color Palette for monochrome Game Boy games");

        ui.separator();
        ui.menu_button("Interface Panels", |ui| {
            if ui
                .checkbox(&mut self.config.show_toolbar, "Show Quick Toolbar")
                .changed()
            {
                self.config.save();
            }
            if ui
                .checkbox(&mut self.config.show_status_bar, "Show Bottom Status Bar")
                .changed()
            {
                self.config.save();
            }
            if ui
                .checkbox(
                    &mut self.config.show_menu_bar,
                    "Show Menu Bar (Ctrl+Shift+M)",
                )
                .changed()
            {
                self.config.save();
                if !self.config.show_menu_bar {
                    self.notify("Menu bar hidden — press Ctrl+Shift+M to restore");
                }
            }
        });

        if dev_tools {
            ui.menu_button("Video Layers", |ui| {
                for (bit, name) in VIDEO_LAYER_NAMES {
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
                for (bit, name) in AUDIO_CHANNEL_NAMES {
                    let mut on = (self.config.audio_channel_mask & (1 << bit)) != 0;
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            self.config.audio_channel_mask |= 1 << bit;
                        } else {
                            self.config.audio_channel_mask &= !(1 << bit);
                        }
                        if let Some(core) = &mut self.core {
                            core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
                        }
                        self.config.save();
                    }
                }
            });
        }

        ui.separator();
        if ui
            .add(egui::Slider::new(&mut self.config.master_volume, 0..=100).text("Volume %"))
            .changed()
        {
            self.sync_audio_volume();
            self.config.save();
        }
        if ui
            .checkbox(&mut self.config.muted, format!("Mute Audio ({mute_key})"))
            .changed()
        {
            self.sync_audio_volume();
            self.config.save();
        }
    }

    fn draw_menu_tools(&mut self, ui: &mut egui::Ui, has_rom: bool, dev_tools: bool) {
        let hardcore = self.is_ra_hardcore_active();
        let state_mgr_key = self
            .config
            .keybindings
            .hotkeys
            .display(HotkeyAction::StateManager);

        if ui
            .add_enabled(
                !hardcore,
                egui::Button::new(format!("Save State Manager... ({state_mgr_key})")),
            )
            .clicked()
        {
            self.show_state_manager = true;
            ui.close_menu();
        }
        if ui
            .add_enabled(
                has_rom && !hardcore,
                egui::Button::new("Undo State Action (Ctrl+Z)"),
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
            .add_enabled(
                !hardcore,
                egui::Button::new("Cheat Code Manager..."),
            )
            .clicked()
        {
            self.debugger.show_cheats = true;
            ui.close_menu();
        }
        if ui.button("Controls & Keybindings...").clicked() {
            self.show_controls_dialog = true;
            ui.close_menu();
        }
        if ui.button("Settings...").clicked() {
            self.show_settings = true;
            ui.close_menu();
        }

        ui.separator();
        if ui.button("RetroAchievements...").clicked() {
            self.ra.show_window = true;
            ui.close_menu();
        }
        let link_active = self.link.local_is_active()
            || self.link.netplay.as_ref().is_some_and(|n| n.is_active());
        let link_lbl = if link_active {
            "Link Cable / Netplay... [Active]"
        } else {
            "Link Cable / Netplay..."
        };
        if ui.button(link_lbl).clicked() {
            self.link.show_window = true;
            ui.close_menu();
        }

        ui.separator();
        ui.menu_button("Capture GIF / Audio", |ui| {
            let gif_rec = self.capture.is_gif_recording();
            let gif_lbl = if gif_rec {
                format!("Stop GIF Recording ({}f)", self.capture.gif_frames())
            } else {
                "Record Animated GIF (Start/Stop)".to_string()
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
                    ui.radio_value(&mut self.capture.gif_fps_mode, GifFpsMode::Fps30, "30 fps");
                    ui.radio_value(&mut self.capture.gif_fps_mode, GifFpsMode::Fps60, "60 fps");
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
                "Record Audio (.wav) (Start/Stop)".to_string()
            };
            if ui
                .add_enabled(has_rom || wav_rec, egui::Button::new(wav_lbl))
                .clicked()
            {
                self.capture_toggle_wav();
                ui.close_menu();
            }
        });

        if ui
            .checkbox(&mut self.config.rumble_enabled, "Gamepad Haptic Rumble")
            .changed()
        {
            self.config.save();
        }
        if ui.button("Open Screenshots Folder").clicked() {
            let dir = AppConfig::screenshots_dir();
            self.open_directory_in_file_manager(&dir);
            ui.close_menu();
        }

        if dev_tools {
            ui.separator();
            ui.label(RichText::new("Developer Tools").small().color(TEXT_MUTED));
            ui.checkbox(&mut self.debugger.show_cpu, "ARM7TDMI CPU & Disassembler");
            ui.checkbox(&mut self.debugger.show_hex, "Memory Hex Viewer");
            ui.checkbox(&mut self.debugger.show_io_regs, "I/O Register Inspector");
            ui.checkbox(
                &mut self.debugger.mem_search.open,
                "Memory Search / Cheat Finder...",
            );
            ui.checkbox(&mut self.debugger.show_palette, "Palette Viewer (PRAM)");
            ui.checkbox(&mut self.debugger.show_vram, "VRAM Tile Viewer");
            ui.checkbox(&mut self.debugger.show_bg, "BG Map Viewer");
            ui.checkbox(&mut self.debugger.show_oam, "OAM Sprite Viewer");
            ui.checkbox(&mut self.debugger.show_audio, "Audio Channel Visualizer");
            ui.checkbox(&mut self.debugger.show_gdb, "GDB Remote Server...");
            if ui.button("Lua Scripting Console...").clicked() {
                self.lua.show_console = true;
                ui.close_menu();
            }
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
            if ui
                .add_enabled(has_rom, egui::Button::new("Hardware Overrides & Sensors..."))
                .clicked()
            {
                self.debugger.show_overrides = true;
                ui.close_menu();
            }
        }
    }

    fn draw_menu_help(&mut self, ui: &mut egui::Ui) {
        if ui.button("Controls Cheat-Sheet...").clicked() {
            self.debugger.show_controls = true;
            ui.close_menu();
        }
        if ui.button("Customize Controls...").clicked() {
            self.show_controls_dialog = true;
            ui.close_menu();
        }
        if ui
            .button("Run Performance Check...")
            .on_hover_text("Benchmark GPU adapter and timing stability (--selftest)")
            .clicked()
        {
            self.start_performance_check();
            ui.close_menu();
        }
        if ui.button("Open Config & Logs Folder").clicked() {
            self.open_config_and_logs_folder();
            ui.close_menu();
        }
        ui.separator();
        if ui.button("About Rustboy Advance").clicked() {
            self.debugger.show_about = true;
            ui.close_menu();
        }
    }

    pub(crate) fn draw_toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("quick_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(COLOR_SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(STROKE_BORDER),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let has_rom = self.has_active_game();
                    let hardcore = self.is_ra_hardcore_active();
                    let hotkeys = &self.config.keybindings.hotkeys;
                    let pause_key = hotkeys.display(HotkeyAction::Pause);
                    let reset_key = hotkeys.display(HotkeyAction::HardReset);
                    let ff_key = hotkeys.display(HotkeyAction::FastForward);
                    let save_key = hotkeys.display(HotkeyAction::SaveState);
                    let load_key = hotkeys.display(HotkeyAction::LoadState);
                    let snap_key = hotkeys.display(HotkeyAction::Screenshot);
                    let mute_key = hotkeys.display(HotkeyAction::Mute);

                    // 1. Open
                    if toolbar_btn(ui, "+ Open", false, true)
                        .on_hover_text("Open GBA / GB / GBC ROM File (Ctrl+O)")
                        .clicked()
                    {
                        self.trigger_open_rom_dialog(ctx);
                    }

                    ui.separator();

                    // 2. Pause / Resume
                    let play_lbl = if self.paused { "▶ Resume" } else { "⏸ Pause" };
                    if toolbar_btn(ui, play_lbl, self.paused, has_rom)
                        .on_hover_text(format!("Pause / Resume ({pause_key})"))
                        .clicked()
                    {
                        self.toggle_pause();
                    }

                    // 3. Restart game (Hard Reset)
                    if toolbar_btn(ui, "⟲ Reset", false, has_rom)
                        .on_hover_text(format!("Restart game / Hard Reset ({reset_key})"))
                        .clicked()
                    {
                        self.hard_reset();
                    }

                    // 4. Fast-Forward
                    let eff_speed = self.effective_speed_multiplier();
                    let turbo_on = self.tab_fast_forward || eff_speed > 1.05 || eff_speed <= 0.0;
                    if toolbar_btn(ui, "⏩ Fast", turbo_on, has_rom)
                        .on_hover_text(format!("Toggle Fast-Forward ({ff_key})"))
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

                    // 5. Quick Save
                    if toolbar_btn(ui, "💾 Save", false, has_rom && !hardcore)
                        .on_hover_text(format!("Quick Save ({save_key})"))
                        .clicked()
                    {
                        self.quick_save(1);
                    }

                    // 6. Quick Load
                    if toolbar_btn(ui, "↓ Load", false, has_rom && !hardcore)
                        .on_hover_text(format!("Quick Load ({load_key})"))
                        .clicked()
                    {
                        self.quick_load(1);
                    }

                    // 7. Screenshot
                    if toolbar_btn(ui, "📷 Shot", false, has_rom)
                        .on_hover_text(format!("Save Screenshot ({snap_key})"))
                        .clicked()
                    {
                        self.capture_screenshot();
                    }

                    ui.separator();

                    // 8. Volume popup & quick-mute
                    let vol_label = if self.config.muted || self.config.master_volume == 0 {
                        "🔇 Muted".to_string()
                    } else {
                        format!("🔊 {}%", self.config.master_volume)
                    };
                    let vol_resp = toolbar_btn(
                        ui,
                        &vol_label,
                        self.show_volume_popup,
                        true,
                    );
                    self.volume_button_rect = Some(vol_resp.rect);
                    if vol_resp.clicked() {
                        self.show_volume_popup = !self.show_volume_popup;
                        self.volume_popup_anchor =
                            Some(egui::pos2(vol_resp.rect.center().x, vol_resp.rect.max.y));
                    }
                    if vol_resp.secondary_clicked() {
                        self.config.muted = !self.config.muted;
                        self.sync_audio_volume();
                        self.config.save();
                    }
                    vol_resp.on_hover_text(format!(
                        "Volume ({}%) — Click for slider, right-click or {mute_key} to mute",
                        self.config.master_volume
                    ));

                    // 9. Settings
                    if toolbar_btn(ui, "☰ Settings", self.show_settings, true)
                        .on_hover_text("Open Settings (interface, audio, controls, developer tools)")
                        .clicked()
                    {
                        self.show_settings = !self.show_settings;
                    }
                });
            });
    }
}
