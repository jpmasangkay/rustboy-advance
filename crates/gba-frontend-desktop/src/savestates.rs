use std::fs;
use std::path::{Path, PathBuf};

use egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, Layout, RichText, Sense, Stroke,
    TextureOptions, Vec2,
};

use crate::app::{RustboyApp, ACCENT_TEAL, BG_CARD, BG_PANEL, BORDER_HAIRLINE, TEXT_MUTED};

impl RustboyApp {
    pub(crate) fn slot_path(&self, slot: usize) -> Option<PathBuf> {
        let rom_path = self.rom_path.as_ref()?;
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(rom_path.with_file_name(format!("{stem}.slot{slot}.state")))
    }

    pub(crate) fn slot_thumb_path(&self, slot: usize) -> Option<PathBuf> {
        let rom_path = self.rom_path.as_ref()?;
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(rom_path.with_file_name(format!("{stem}.slot{slot}.png")))
    }

    pub(crate) fn quick_save(&mut self, slot: usize) {
        let (Some(core), Some(path)) = (&self.core, self.slot_path(slot)) else {
            return;
        };
        // Backup existing state for undo
        if let Ok(old_bytes) = fs::read(&path) {
            self.undo_state_buffer = Some(old_bytes);
        }
        let res = core
            .save_state()
            .map_err(|e| format!("{e}"))
            .and_then(|bytes| fs::write(&path, bytes).map_err(|e| format!("{e}")));
        match res {
            Ok(()) => {
                if let Some(tp) = self.slot_thumb_path(slot) {
                    let _ = self.postproc.save_thumbnail_png(&tp);
                    if (1..=5).contains(&slot) {
                        self.slot_thumbnails[slot - 1] = None;
                    }
                }
                self.notify(&format!("Saved State Slot {slot}"));
            }
            Err(e) => self.notify(&format!("Save State failed: {e}")),
        }
    }

    pub(crate) fn quick_load(&mut self, slot: usize) {
        let Some(path) = self.slot_path(slot) else {
            return;
        };
        // Backup current core state before loading for instant undo
        if let Some(core) = &self.core {
            if let Ok(cur_bytes) = core.save_state() {
                self.undo_state_buffer = Some(cur_bytes);
            }
        }
        self.load_state_from_path(&path, &format!("Loaded State Slot {slot}"));
    }

    pub(crate) fn undo_last_state_action(&mut self) {
        let (Some(core), Some(prev_bytes)) = (&mut self.core, self.undo_state_buffer.take()) else {
            self.notify("No save/load action to undo");
            return;
        };
        let cur = core.save_state().ok();
        match core.load_state(&prev_bytes) {
            Ok(()) => {
                self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                self.sink.frame_dirty = true;
                self.undo_state_buffer = cur;
                self.notify("Undo: Reverted state to previous checkpoint");
            }
            Err(e) => self.notify(&format!("Undo state load failed: {e}")),
        }
    }

    pub(crate) fn save_state_to_file(&mut self) {
        let Some(core) = &self.core else { return };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save State File")
            .add_filter("Rustboy Save State", &["state", "gbas"])
            .set_file_name(format!("{}.state", self.rom_title))
            .save_file()
        {
            match core.save_state() {
                Ok(bytes) => match fs::write(&path, bytes) {
                    Ok(()) => self.notify("Saved State to file"),
                    Err(e) => self.notify(&format!("Failed writing state: {e}")),
                },
                Err(e) => self.notify(&format!("State serialize failed: {e}")),
            }
        }
    }

    pub(crate) fn load_state_from_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Load Save State")
            .add_filter("Rustboy Save State", &["state", "gbas"])
            .pick_file()
        {
            self.load_state_from_path(&path, "Loaded State from file");
        }
    }

    pub(crate) fn load_state_from_path(&mut self, path: &Path, success_msg: &str) {
        let Some(core) = &mut self.core else { return };
        match fs::read(path) {
            Ok(bytes) => match core.load_state(&bytes) {
                Ok(()) => {
                    core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
                    core.bus.ppu.layer_enable_mask = self.config.layer_mask;
                    core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
                    core.bus.apu.master_volume_pct = self.config.master_volume;
                    self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                    self.sink.frame_dirty = true;
                    self.notify(success_msg);
                }
                Err(e) => self.notify(&format!("Invalid save state: {e}")),
            },
            Err(e) => self.notify(&format!("State not found: {e}")),
        }
    }

    pub(crate) fn draw_visual_state_manager(&mut self, ctx: &egui::Context) {
        if !self.show_state_manager {
            return;
        }
        let mut open = self.show_state_manager;
        let mut do_undo = false;
        let mut save_slot: Option<usize> = None;
        let mut load_slot: Option<usize> = None;
        let mut delete_slot: Option<usize> = None;

        egui::Window::new("🖼️ Visual Save State Manager")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(BG_CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Game: {} ({})", self.rom_title, self.rom_code))
                            .color(TEXT_MUTED)
                            .small(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("↩ Undo State Action (Ctrl+Z)").clicked() {
                            do_undo = true;
                        }
                    });
                });
                ui.separator();

                ui.horizontal(|ui| {
                    for slot in 1..=5 {
                        let state_file = self.slot_path(slot);
                        let exists = state_file.as_ref().is_some_and(|p| p.exists());
                        let thumb_file = self.slot_thumb_path(slot);

                        ui.vertical(|ui| {
                            ui.add_sized(
                                [120.0, 20.0],
                                egui::Label::new(
                                    RichText::new(format!("Slot {slot}"))
                                        .strong()
                                        .color(if exists { ACCENT_TEAL } else { TEXT_MUTED }),
                                ),
                            );

                            if exists && self.slot_thumbnails[slot - 1].is_none() {
                                if let Some(tpath) = &thumb_file {
                                    if let Ok(img) = image::open(tpath) {
                                        let rgba = img.to_rgba8();
                                        let (w, h) = rgba.dimensions();
                                        let cimg = ColorImage::from_rgba_unmultiplied(
                                            [w as usize, h as usize],
                                            &rgba.into_raw(),
                                        );
                                        self.slot_thumbnails[slot - 1] = Some(ctx.load_texture(
                                            format!("slot_thumb_{slot}"),
                                            cimg,
                                            TextureOptions::LINEAR,
                                        ));
                                    }
                                }
                            }

                            let thumb_size = Vec2::new(120.0, 80.0);
                            if let Some(tex) = &self.slot_thumbnails[slot - 1] {
                                let (rect, _) = ui.allocate_exact_size(thumb_size, Sense::hover());
                                ui.painter().image(
                                    tex.id(),
                                    rect,
                                    egui::Rect::from_min_max(
                                        egui::Pos2::ZERO,
                                        egui::Pos2::new(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    CornerRadius::same(4),
                                    Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                    egui::StrokeKind::Outside,
                                );
                            } else {
                                let (rect, _) = ui.allocate_exact_size(thumb_size, Sense::hover());
                                ui.painter()
                                    .rect_filled(rect, CornerRadius::same(4), BG_PANEL);
                                ui.painter().rect_stroke(
                                    rect,
                                    CornerRadius::same(4),
                                    Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                    egui::StrokeKind::Outside,
                                );
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    if exists { "[No Thumbnail]" } else { "[Empty Slot]" },
                                    egui::FontId::proportional(12.0),
                                    TEXT_MUTED,
                                );
                            }

                            let status_lbl = if exists {
                                if let Some(sf) = &state_file {
                                    if let Ok(meta) = sf.metadata() {
                                        if let Ok(mod_time) = meta.modified() {
                                            if let Ok(el) = mod_time.elapsed() {
                                                if el.as_secs() < 60 {
                                                    "Saved: Just now".to_string()
                                                } else if el.as_secs() < 3600 {
                                                    format!("Saved: {}m ago", el.as_secs() / 60)
                                                } else {
                                                    format!("Saved: {}h ago", el.as_secs() / 3600)
                                                }
                                            } else {
                                                "Saved".to_string()
                                            }
                                        } else {
                                            "Saved".to_string()
                                        }
                                    } else {
                                        "Saved".to_string()
                                    }
                                } else {
                                    "Saved".to_string()
                                }
                            } else {
                                "Available".to_string()
                            };
                            ui.label(RichText::new(status_lbl).small().color(TEXT_MUTED));

                            ui.horizontal(|ui| {
                                if ui
                                    .add_sized(
                                        [55.0, 22.0],
                                        egui::Button::new(RichText::new("Save").small()),
                                    )
                                    .on_hover_text(format!("Quick Save to Slot {slot} (F{slot})"))
                                    .clicked()
                                {
                                    save_slot = Some(slot);
                                }
                                if ui
                                    .add_enabled(
                                        exists,
                                        egui::Button::new(RichText::new("Load").small()),
                                    )
                                    .on_hover_text(format!(
                                        "Quick Load from Slot {slot} (Shift+F{slot})"
                                    ))
                                    .clicked()
                                {
                                    load_slot = Some(slot);
                                }
                            });

                            if exists {
                                if ui
                                    .button(
                                        RichText::new("🗑 Delete")
                                            .small()
                                            .color(Color32::from_rgb(255, 100, 100)),
                                    )
                                    .clicked()
                                {
                                    delete_slot = Some(slot);
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                });
            });

        self.show_state_manager = open;
        if do_undo {
            self.undo_last_state_action();
        }
        if let Some(s) = save_slot {
            self.quick_save(s);
        }
        if let Some(s) = load_slot {
            self.quick_load(s);
        }
        if let Some(s) = delete_slot {
            if let Some(sp) = self.slot_path(s) {
                let _ = fs::remove_file(sp);
            }
            if let Some(tp) = self.slot_thumb_path(s) {
                let _ = fs::remove_file(tp);
            }
            self.slot_thumbnails[s - 1] = None;
            self.notify(&format!("Deleted Slot {s}"));
        }
    }
}
