use std::path::PathBuf;

use egui::{self, Color32, CornerRadius, RichText, Stroke};

use crate::app::{
    RustboyApp, ACCENT_TEAL, ACCENT_TEAL_BG, BG_CARD, BORDER_HAIRLINE, TEXT_MUTED,
};
use crate::config::{
    game_settings_key, AppConfig, ColorProfileChoice, RomSpecificSettings, SaveOverrideChoice,
};
use crate::library::RomLibraryEntry;
use crate::rom_loader::load_rom_bytes;

#[derive(Clone, Debug)]
pub(crate) struct GameSettingsTarget {
    pub(crate) title: String,
    pub(crate) code: String,
    pub(crate) crc32: u32,
    pub(crate) rom_path: Option<PathBuf>,
    pub(crate) error_message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CartridgeHardwareDefaults {
    pub(crate) solar_enabled: bool,
    pub(crate) tilt_enabled: bool,
    pub(crate) gyro_enabled: bool,
    pub(crate) rumble_enabled: bool,
}

const SPEED_OVERRIDE_PRESETS: &[(Option<f32>, &str)] = &[
    (None, "Global Default"),
    (Some(0.5), "0.5x Slow Motion"),
    (Some(1.0), "1.0x Normal"),
    (Some(1.5), "1.5x Crisp"),
    (Some(2.0), "2.0x Fast"),
    (Some(3.0), "3.0x Turbo"),
    (Some(4.0), "4.0x Ultra"),
    (Some(0.0), "Unthrottled Turbo"),
];

pub(crate) fn load_effective_settings(
    config: &AppConfig,
    code: &str,
    crc32: u32,
) -> RomSpecificSettings {
    if code.is_empty() {
        return RomSpecificSettings::default();
    }
    config
        .get_game_settings(code, crc32)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn save_game_settings_entry_in_memory(
    config: &mut AppConfig,
    code: &str,
    crc32: u32,
    settings: RomSpecificSettings,
) {
    if code.is_empty() {
        return;
    }
    let key = game_settings_key(code, crc32);
    if settings.is_empty() {
        config.per_rom.remove(&key);
    } else {
        config.per_rom.insert(key, settings);
    }
}

pub(crate) fn save_game_settings_entry(
    config: &mut AppConfig,
    code: &str,
    crc32: u32,
    settings: RomSpecificSettings,
) {
    save_game_settings_entry_in_memory(config, code, crc32, settings);
    config.save();
}

pub(crate) fn clear_game_override_fields_in_memory(
    config: &mut AppConfig,
    code: &str,
    crc32: u32,
) {
    if code.is_empty() {
        return;
    }
    let exact_key = game_settings_key(code, crc32);
    let current = load_effective_settings(config, code, crc32);
    let kept_cheats = current.cheats;

    if kept_cheats.is_empty() {
        config.per_rom.remove(&exact_key);
    } else {
        config.per_rom.insert(
            exact_key,
            RomSpecificSettings {
                cheats: kept_cheats,
                ..RomSpecificSettings::default()
            },
        );
    }

    // Clear override fields on legacy code-only entry as well so it doesn't shadow reset.
    if let Some(legacy) = config.per_rom.get_mut(code) {
        let cheats = std::mem::take(&mut legacy.cheats);
        *legacy = RomSpecificSettings {
            cheats,
            ..RomSpecificSettings::default()
        };
        if legacy.is_empty() {
            config.per_rom.remove(code);
        }
    }
}

pub(crate) fn clear_game_override_fields(config: &mut AppConfig, code: &str, crc32: u32) {
    clear_game_override_fields_in_memory(config, code, crc32);
    config.save();
}

impl RustboyApp {
    pub(crate) fn active_game_settings(&self) -> Option<&RomSpecificSettings> {
        if self.rom_code.is_empty() {
            return None;
        }
        self.config.get_game_settings(&self.rom_code, self.rom_crc32)
    }

    pub(crate) fn has_per_game_speed_override(&self) -> bool {
        self.active_game_settings()
            .and_then(|s| s.speed_multiplier)
            .is_some()
    }

    pub(crate) fn effective_color_profile(&self) -> ColorProfileChoice {
        self.active_game_settings()
            .and_then(|ov| ov.custom_color_profile)
            .unwrap_or(self.config.color_profile)
    }

    pub(crate) fn effective_speed_multiplier(&self) -> f32 {
        if self.tab_fast_forward {
            return self.config.fast_forward_speed;
        }
        self.session_speed_override
            .or_else(|| self.active_game_settings().and_then(|ov| ov.speed_multiplier))
            .unwrap_or(self.config.speed_multiplier)
    }

    pub(crate) fn effective_rumble_enabled(&self) -> bool {
        self.active_game_settings()
            .and_then(|ov| ov.rumble_enabled)
            .unwrap_or(self.config.rumble_enabled)
    }

    pub(crate) fn apply_active_game_settings_to_core(&mut self) {
        let ov = self.active_game_settings().cloned().unwrap_or_default();
        let hw = self.cart_hw_defaults;
        if let Some(core) = &mut self.core {
            core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
            core.bus.ppu.layer_enable_mask = self.config.layer_mask;
            core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
            core.bus.apu.master_volume_pct = self.config.master_volume;

            let save_ov = ov.save_override.and_then(SaveOverrideChoice::to_core);
            core.bus.cart.set_save_type_override(save_ov);
            core.bus.cart.rtc_override = ov.rtc_override;
            core.bus.cart.solar_enabled = ov.solar_enabled.unwrap_or(hw.solar_enabled);
            if let Some(lux) = ov.solar_lux {
                core.bus.cart.solar_lux = lux;
            }
            core.bus.cart.rumble_enabled = ov.rumble_enabled.unwrap_or(hw.rumble_enabled);
            core.bus.cart.tilt_enabled = ov.tilt_enabled.unwrap_or(hw.tilt_enabled);
            core.bus.cart.gyro_enabled = ov.gyro_enabled.unwrap_or(hw.gyro_enabled);
        }
        self.active_cheats = ov.cheats;
        self.sink.frame_dirty = true;
    }

    pub(crate) fn open_game_settings_for_active_rom(&mut self) {
        if self.core.is_none() || self.rom_code.is_empty() {
            self.notify_warn("Load a ROM first to edit active game settings");
            return;
        }
        self.game_settings_target = Some(GameSettingsTarget {
            title: self.rom_title.clone(),
            code: self.rom_code.clone(),
            crc32: self.rom_crc32,
            rom_path: self.rom_path.clone(),
            error_message: None,
        });
        self.show_game_settings = true;
    }

    pub(crate) fn open_game_settings_for_library_entry(&mut self, entry: &RomLibraryEntry) {
        self.open_game_settings_for_path(&entry.path, &entry.display_name, &entry.game_code);
    }

    pub(crate) fn open_game_settings_for_path(
        &mut self,
        path: &std::path::Path,
        title: &str,
        code: &str,
    ) {
        let target = match load_rom_bytes(path) {
            Ok(loaded) => {
                let crc32 = crc32fast::hash(&loaded.bytes);
                GameSettingsTarget {
                    title: title.to_string(),
                    code: code.to_string(),
                    crc32,
                    rom_path: Some(path.to_path_buf()),
                    error_message: None,
                }
            }
            Err(e) => GameSettingsTarget {
                title: title.to_string(),
                code: code.to_string(),
                crc32: 0,
                rom_path: Some(path.to_path_buf()),
                error_message: Some(e),
            },
        };
        self.game_settings_target = Some(target);
        self.show_game_settings = true;
    }

    pub(crate) fn draw_game_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_game_settings {
            return;
        }

        if self.game_settings_target.is_none() && self.core.is_some() && !self.rom_code.is_empty()
        {
            self.open_game_settings_for_active_rom();
        }

        let Some(target) = self.game_settings_target.clone() else {
            self.show_game_settings = false;
            return;
        };

        let mut open = self.show_game_settings;
        let is_active_rom = self.core.is_some()
            && self.rom_code == target.code
            && self.rom_crc32 == target.crc32;

        egui::Window::new("Per-Game Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(540.0)
            .default_height(500.0)
            .show(ctx, |ui| {
                // Header card
                egui::Frame::new()
                    .fill(BG_CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(&target.title)
                                        .strong()
                                        .size(15.0)
                                        .color(Color32::WHITE),
                                );
                                let key_lbl = if target.crc32 != 0 {
                                    game_settings_key(&target.code, target.crc32)
                                } else {
                                    target.code.clone()
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "Code: {}  •  CRC32: {:08X}  •  Key: {}",
                                        target.code, target.crc32, key_lbl
                                    ))
                                    .monospace()
                                    .small()
                                    .color(TEXT_MUTED),
                                );
                            });
                            if is_active_rom {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        egui::Frame::new()
                                            .fill(ACCENT_TEAL_BG)
                                            .stroke(Stroke::new(1.0_f32, ACCENT_TEAL))
                                            .corner_radius(CornerRadius::same(4))
                                            .inner_margin(egui::Margin::symmetric(8, 3))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new("Active ROM")
                                                        .small()
                                                        .strong()
                                                        .color(ACCENT_TEAL),
                                                );
                                            });
                                    },
                                );
                            }
                        });
                    });

                ui.add_space(8.0);

                if let Some(err) = &target.error_message {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(56, 24, 24))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(185, 65, 65)))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Unable to inspect ROM image for per-game key")
                                    .strong()
                                    .color(Color32::from_rgb(255, 160, 160)),
                            );
                            ui.label(
                                RichText::new(err)
                                    .small()
                                    .color(Color32::from_rgb(245, 205, 205)),
                            );
                            if let Some(path) = &target.rom_path {
                                ui.add_space(4.0);
                                if ui.small_button("Retry").clicked() {
                                    self.open_game_settings_for_path(
                                        path,
                                        &target.title,
                                        &target.code,
                                    );
                                }
                            }
                        });
                    return;
                }

                let mut ov = load_effective_settings(&self.config, &target.code, target.crc32);
                let mut changed = false;

                if has_no_non_cheat_overrides(&ov) {
                    egui::Frame::new()
                        .fill(BG_CARD)
                        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    "No overrides configured — inheriting global emulator defaults.",
                                )
                                .small()
                                .color(TEXT_MUTED),
                            );
                        });
                    ui.add_space(6.0);
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // Cartridge Save & Clock
                        egui::Frame::new()
                            .fill(BG_CARD)
                            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("Cartridge Save & RTC")
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                                ui.add_space(6.0);
                                egui::Grid::new("gs_cart_grid")
                                    .num_columns(2)
                                    .spacing([14.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Save Type:");
                                        let save_lbl = match ov.save_override {
                                            None => "Global / Auto-Detect",
                                            Some(c) => c.label(),
                                        };
                                        egui::ComboBox::from_id_salt("gs_save_type")
                                            .selected_text(save_lbl)
                                            .width(240.0)
                                            .show_ui(ui, |ui| {
                                                if ui
                                                    .selectable_value(
                                                        &mut ov.save_override,
                                                        None,
                                                        "Global / Auto-Detect",
                                                    )
                                                    .changed()
                                                {
                                                    changed = true;
                                                }
                                                for choice in SaveOverrideChoice::ALL {
                                                    if ui
                                                        .selectable_value(
                                                            &mut ov.save_override,
                                                            Some(choice),
                                                            choice.label(),
                                                        )
                                                        .changed()
                                                    {
                                                        changed = true;
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        ui.label("Real-Time Clock:");
                                        changed |= tri_state_combo(
                                            ui,
                                            "gs_rtc",
                                            &mut ov.rtc_override,
                                            "Auto-Detect",
                                            "Force Enabled",
                                            "Force Disabled",
                                        );
                                        ui.end_row();
                                    });
                            });

                        ui.add_space(8.0);

                        // Video & Speed
                        egui::Frame::new()
                            .fill(BG_CARD)
                            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("Display & Speed")
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                                ui.add_space(6.0);
                                egui::Grid::new("gs_av_grid")
                                    .num_columns(2)
                                    .spacing([14.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Color Profile:");
                                        let cp_lbl = match ov.custom_color_profile {
                                            None => format!(
                                                "Global Default ({})",
                                                self.config.color_profile.label()
                                            ),
                                            Some(cp) => cp.label().to_string(),
                                        };
                                        egui::ComboBox::from_id_salt("gs_color_profile")
                                            .selected_text(cp_lbl)
                                            .width(240.0)
                                            .show_ui(ui, |ui| {
                                                if ui
                                                    .selectable_value(
                                                        &mut ov.custom_color_profile,
                                                        None,
                                                        "Global Default",
                                                    )
                                                    .changed()
                                                {
                                                    changed = true;
                                                }
                                                for cp in ColorProfileChoice::ALL {
                                                    if ui
                                                        .selectable_value(
                                                            &mut ov.custom_color_profile,
                                                            Some(cp),
                                                            cp.label(),
                                                        )
                                                        .changed()
                                                    {
                                                        changed = true;
                                                    }
                                                }
                                            });
                                        ui.end_row();

                                        ui.label("Speed Multiplier:");
                                        let spd_lbl = speed_label(
                                            ov.speed_multiplier,
                                            self.config.speed_multiplier,
                                        );
                                        egui::ComboBox::from_id_salt("gs_speed")
                                            .selected_text(spd_lbl)
                                            .width(240.0)
                                            .show_ui(ui, |ui| {
                                                for &(val, lbl) in SPEED_OVERRIDE_PRESETS {
                                                    let selected = match (ov.speed_multiplier, val)
                                                    {
                                                        (None, None) => true,
                                                        (Some(a), Some(b)) => (a - b).abs() < 0.01,
                                                        _ => false,
                                                    };
                                                    if ui.selectable_label(selected, lbl).clicked() {
                                                        ov.speed_multiplier = val;
                                                        if is_active_rom {
                                                            self.session_speed_override = None;
                                                        }
                                                        changed = true;
                                                    }
                                                }
                                            });
                                        ui.end_row();
                                    });
                            });

                        ui.add_space(8.0);

                        // Sensors & Rumble
                        egui::Frame::new()
                            .fill(BG_CARD)
                            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("Cartridge Sensors & Haptics")
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                                ui.add_space(6.0);
                                egui::Grid::new("gs_sensor_grid")
                                    .num_columns(2)
                                    .spacing([14.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Rumble Haptics:");
                                        changed |= tri_state_combo(
                                            ui,
                                            "gs_rumble",
                                            &mut ov.rumble_enabled,
                                            "Global Default",
                                            "Enabled",
                                            "Disabled",
                                        );
                                        ui.end_row();

                                        ui.label("Boktai Solar Sensor:");
                                        changed |= tri_state_combo(
                                            ui,
                                            "gs_solar_en",
                                            &mut ov.solar_enabled,
                                            "Auto-Detect",
                                            "Force Enabled",
                                            "Force Disabled",
                                        );
                                        ui.end_row();

                                        ui.label("Solar Lux Override:");
                                        ui.horizontal(|ui| {
                                            let mut has_lux = ov.solar_lux.is_some();
                                            if ui.checkbox(&mut has_lux, "Custom").changed() {
                                                ov.solar_lux = if has_lux { Some(128) } else { None };
                                                changed = true;
                                            }
                                            if let Some(mut lux) = ov.solar_lux {
                                                let lvl = ((f32::from(lux) / 255.0) * 10.0).round()
                                                    as u8;
                                                if ui
                                                    .add(
                                                        egui::Slider::new(&mut lux, 0..=255)
                                                            .text(format!("Lv {lvl}/10")),
                                                    )
                                                    .changed()
                                                {
                                                    ov.solar_lux = Some(lux);
                                                    changed = true;
                                                }
                                            } else {
                                                ui.label(
                                                    RichText::new("Using cartridge level")
                                                        .small()
                                                        .color(TEXT_MUTED),
                                                );
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Warioware Gyroscope:");
                                        changed |= tri_state_combo(
                                            ui,
                                            "gs_gyro",
                                            &mut ov.gyro_enabled,
                                            "Auto-Detect",
                                            "Force Enabled",
                                            "Force Disabled",
                                        );
                                        ui.end_row();

                                        ui.label("Yoshi Tilt Sensor:");
                                        changed |= tri_state_combo(
                                            ui,
                                            "gs_tilt",
                                            &mut ov.tilt_enabled,
                                            "Auto-Detect",
                                            "Force Enabled",
                                            "Force Disabled",
                                        );
                                        ui.end_row();
                                    });
                            });
                    });

                if changed {
                    save_game_settings_entry(&mut self.config, &target.code, target.crc32, ov.clone());
                    if is_active_rom {
                        self.apply_active_game_settings_to_core();
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} cheat(s) attached", ov.cheats.len()))
                            .small()
                            .color(TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.show_game_settings = false;
                        }
                        let has_ov = !has_no_non_cheat_overrides(&ov);
                        if ui
                            .add_enabled(
                                has_ov,
                                egui::Button::new("Reset Overrides to Global"),
                            )
                            .clicked()
                        {
                            clear_game_override_fields(&mut self.config, &target.code, target.crc32);
                            if is_active_rom {
                                self.session_speed_override = None;
                                self.apply_active_game_settings_to_core();
                            }
                            self.notify("Per-game overrides reset to global defaults");
                        }
                    });
                });
            });

        if !open {
            self.show_game_settings = false;
        }
    }
}

fn has_no_non_cheat_overrides(ov: &RomSpecificSettings) -> bool {
    ov.save_override.is_none()
        && ov.rtc_override.is_none()
        && ov.solar_lux.is_none()
        && ov.custom_color_profile.is_none()
        && ov.speed_multiplier.is_none()
        && ov.rumble_enabled.is_none()
        && ov.solar_enabled.is_none()
        && ov.tilt_enabled.is_none()
        && ov.gyro_enabled.is_none()
}

fn speed_label(override_speed: Option<f32>, global_speed: f32) -> String {
    match override_speed {
        None => {
            if global_speed <= 0.0 {
                "Global Default (Unthrottled)".to_string()
            } else {
                format!("Global Default ({global_speed:.1}x)")
            }
        }
        Some(v) if v <= 0.0 => "Unthrottled Turbo".to_string(),
        Some(v) => format!("{v:.1}x"),
    }
}

fn tri_state_combo(
    ui: &mut egui::Ui,
    id: &str,
    val: &mut Option<bool>,
    none_lbl: &str,
    true_lbl: &str,
    false_lbl: &str,
) -> bool {
    let mut changed = false;
    let selected_lbl = match *val {
        None => none_lbl,
        Some(true) => true_lbl,
        Some(false) => false_lbl,
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected_lbl)
        .width(240.0)
        .show_ui(ui, |ui| {
            if ui.selectable_value(val, None, none_lbl).changed() {
                changed = true;
            }
            if ui.selectable_value(val, Some(true), true_lbl).changed() {
                changed = true;
            }
            if ui.selectable_value(val, Some(false), false_lbl).changed() {
                changed = true;
            }
        });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_rom_settings_keyed_by_code_and_crc_with_legacy_fallback() {
        let mut cfg = AppConfig::default();

        // Legacy code-only entry applies as fallback when no CRC entry exists
        let legacy_ov = RomSpecificSettings {
            solar_lux: Some(200),
            ..RomSpecificSettings::default()
        };
        cfg.per_rom.insert("U3IE".to_string(), legacy_ov);
        assert_eq!(load_effective_settings(&cfg, "U3IE", 0x11223344).solar_lux, Some(200));

        // Exact code+CRC entry isolates ROM revisions/hacks with same game code
        let rev_a = RomSpecificSettings {
            speed_multiplier: Some(2.0),
            custom_color_profile: Some(ColorProfileChoice::Ags101),
            ..RomSpecificSettings::default()
        };
        let key_a = game_settings_key("BPEE", 0xAABBCCDD);
        cfg.per_rom.insert(key_a, rev_a);

        let rev_b = RomSpecificSettings {
            speed_multiplier: Some(1.0),
            rumble_enabled: Some(false),
            ..RomSpecificSettings::default()
        };
        let key_b = game_settings_key("BPEE", 0x99887766);
        cfg.per_rom.insert(key_b, rev_b);

        let loaded_a = load_effective_settings(&cfg, "BPEE", 0xAABBCCDD);
        let loaded_b = load_effective_settings(&cfg, "BPEE", 0x99887766);
        assert_eq!(loaded_a.speed_multiplier, Some(2.0));
        assert_eq!(loaded_a.custom_color_profile, Some(ColorProfileChoice::Ags101));
        assert_eq!(loaded_b.speed_multiplier, Some(1.0));
        assert_eq!(loaded_b.rumble_enabled, Some(false));

        // Resetting overrides deletes empty entry without affecting global AppConfig
        cfg.color_profile = ColorProfileChoice::Agb001;
        cfg.speed_multiplier = 1.0;
        clear_game_override_fields_in_memory(&mut cfg, "U3IE", 0x11223344);
        assert!(load_effective_settings(&cfg, "U3IE", 0x11223344).is_empty());
        assert_eq!(cfg.color_profile, ColorProfileChoice::Agb001);
        assert_eq!(cfg.speed_multiplier, 1.0);
    }
}
