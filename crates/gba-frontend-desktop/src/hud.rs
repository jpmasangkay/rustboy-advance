use std::time::Instant;

use egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, RichText, Sense, Stroke,
    Vec2,
};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::app::{RustboyApp, ACCENT_TEAL, BG_CARD, BG_PANEL, BORDER_HAIRLINE, TEXT_MUTED};
use crate::config::{BezelStyle, ShaderPreset};
use crate::gb_bridge::{GB_VIEW_OFFSET_X, GB_VIEW_OFFSET_Y};
use crate::rom_loader::RomPlatform;
use crate::shaders::{
    color_profile_uniform, create_viewport_paint_callback, ShaderRenderParams,
};

impl RustboyApp {
    pub(crate) fn draw_status_bar(&mut self, ctx: &egui::Context) {
        if let Some(t) = &self.toast
            && Instant::now() > t.expires_at
        {
            self.toast = None;
        }

        egui::TopBottomPanel::bottom("telemetry_status_bar")
            .frame(
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Left: Toast / Status
                    if let Some(t) = &self.toast {
                        ui.label(RichText::new(&t.text).small().color(ACCENT_TEAL));
                    } else if let Some(core) = &self.core {
                        let isa = if core.cpu.is_thumb() { "THUMB" } else { "ARM" };
                        let mode = core.bus.ppu.dispcnt & 7;
                        let save_lbl = format!("{:?}", core.bus.cart.effective_save_type());
                        let link_sfx = self
                            .link
                            .status_badge()
                            .map(|b| format!(" | {b}"))
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(format!(
                                "{isa} | Mode {mode} | Save: {save_lbl}{link_sfx} | Esc = Stop to Library"
                            ))
                            .small()
                            .color(TEXT_MUTED),
                        );
                    } else if let Some(gb) = &self.gb {
                        ui.label(
                            RichText::new(format!(
                                "{} | Cart: {} | Esc = Stop to Library",
                                gb.platform.badge(),
                                gb.cart_type_label
                            ))
                            .small()
                            .color(TEXT_MUTED),
                        );
                    } else {
                        let n_games = self.library_entries.len();
                        let n_folders = self.rom_folders.len();
                        ui.label(
                            RichText::new(format!(
                                "{n_games} Game{} | {n_folders} Folder{}",
                                if n_games == 1 { "" } else { "s" },
                                if n_folders == 1 { "" } else { "s" }
                            ))
                            .small()
                            .color(TEXT_MUTED),
                        );
                    }

                    // Right-to-left telemetry badges
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let bios_lbl = if let Some(gb) = &self.gb {
                            if gb.platform == RomPlatform::Gbc {
                                "CGB Mode".to_string()
                            } else {
                                format!("DMG: {}", self.config.dmg_palette.label())
                            }
                        } else if self.bios_bytes.is_some() {
                            "BIOS: Custom".to_string()
                        } else {
                            "BIOS: HLE".to_string()
                        };

                        if self.has_active_game() {
                            let eff_speed = self.effective_speed_multiplier();
                            let speed_str = if self.paused {
                                "PAUSED".to_string()
                            } else if eff_speed <= 0.0 {
                                "UNLIMITED".to_string()
                            } else {
                                format!("{eff_speed:.1}x")
                            };
                            ui.monospace(RichText::new(speed_str).small().color(ACCENT_TEAL));
                            ui.separator();
                            ui.monospace(
                                RichText::new(format!(
                                    "{:.1} FPS ({:.2} ms)",
                                    self.measured_fps, self.last_core_ms
                                ))
                                .small(),
                            );
                            ui.separator();
                            if let Some(link_badge) = self.link.status_badge() {
                                ui.monospace(RichText::new(link_badge).small().color(ACCENT_TEAL));
                                ui.separator();
                            }
                            ui.monospace(RichText::new(bios_lbl).small().color(TEXT_MUTED));
                        } else {
                            ui.monospace(RichText::new(bios_lbl).small().color(TEXT_MUTED));
                        }

                        if self.capture.is_wav_recording() {
                            ui.separator();
                            draw_status_pill(
                                ui,
                                &format!("WAV {:.1}s", self.capture.wav_duration_secs()),
                                Color32::from_rgb(245, 180, 66),
                                Color32::from_rgb(130, 88, 32),
                                Color32::from_rgb(36, 26, 16),
                            );
                        }
                        if self.capture.is_gif_recording() {
                            ui.separator();
                            draw_status_pill(
                                ui,
                                &format!("GIF {}f", self.capture.gif_frames()),
                                Color32::from_rgb(248, 113, 113),
                                Color32::from_rgb(138, 46, 52),
                                Color32::from_rgb(38, 18, 22),
                            );
                        }
                        if self.movie.is_recording() {
                            ui.separator();
                            draw_status_pill(
                                ui,
                                &format!(
                                    "REC {}f (RR:{})",
                                    self.movie.current_frame(),
                                    self.movie.rerecord_count()
                                ),
                                Color32::from_rgb(248, 113, 113),
                                Color32::from_rgb(138, 46, 52),
                                Color32::from_rgb(38, 18, 22),
                            );
                        } else if self.movie.is_playing() {
                            ui.separator();
                            let mode = if self.movie.read_only() { "RO" } else { "RW" };
                            draw_status_pill(
                                ui,
                                &format!(
                                    "PLAY {}/{} [{}]",
                                    self.movie.current_frame(),
                                    self.movie.total_frames(),
                                    mode
                                ),
                                ACCENT_TEAL,
                                Color32::from_rgb(36, 108, 104),
                                Color32::from_rgb(16, 34, 38),
                            );
                        }
                    });
                });
            });
    }

    pub(crate) fn draw_bezel_frame(&self, ui: &mut egui::Ui, screen_rect: egui::Rect, avail: Vec2) {
        let (chassis_color, gba_label, badge_color) = match self.config.bezel_style {
            BezelStyle::None => return,
            BezelStyle::ClassicIndigo => (
                Color32::from_rgb(44, 48, 86),
                "GAME BOY ADVANCE",
                Color32::from_rgb(180, 185, 210),
            ),
            BezelStyle::OnyxStealth => (
                Color32::from_rgb(22, 24, 30),
                "GAME BOY ADVANCE",
                Color32::from_rgb(140, 145, 160),
            ),
            BezelStyle::PlatinumSp => (
                Color32::from_rgb(175, 180, 190),
                "GAME BOY ADVANCE SP",
                Color32::from_rgb(40, 45, 55),
            ),
        };
        let bezel_label = match &self.gb {
            Some(gb) if gb.platform == RomPlatform::Gbc => "GAME BOY COLOR",
            Some(_) => "GAME BOY",
            None => gba_label,
        };

        let pad_x = ((avail.x - screen_rect.width()) * 0.45).clamp(24.0, 90.0);
        let pad_y = ((avail.y - screen_rect.height()) * 0.45).clamp(20.0, 50.0);
        let bezel_rect = screen_rect.expand2(Vec2::new(pad_x, pad_y));

        // Outer handheld chassis frame
        ui.painter()
            .rect_filled(bezel_rect, CornerRadius::same(14), chassis_color);
        ui.painter().rect_stroke(
            bezel_rect,
            CornerRadius::same(14),
            Stroke::new(1.5_f32, Color32::from_rgb(60, 65, 80)),
            egui::StrokeKind::Outside,
        );

        // Inner dark bezel surround around LCD
        let inner_bevel = screen_rect.expand(6.0);
        ui.painter()
            .rect_filled(inner_bevel, CornerRadius::same(4), Color32::from_rgb(10, 12, 16));
        ui.painter().rect_stroke(
            inner_bevel,
            CornerRadius::same(4),
            Stroke::new(1.0_f32, Color32::from_rgb(28, 32, 42)),
            egui::StrokeKind::Outside,
        );

        // Power LED indicator
        let led_pos = egui::Pos2::new(bezel_rect.right() - 24.0, bezel_rect.top() + 16.0);
        let led_color = if self.rewinding {
            Color32::from_rgb(255, 180, 0)
        } else if self.paused {
            Color32::from_rgb(255, 80, 80)
        } else {
            Color32::from_rgb(50, 220, 100)
        };
        ui.painter().circle_filled(led_pos, 4.0, led_color);
        ui.painter().circle_stroke(
            led_pos,
            4.0,
            Stroke::new(1.0_f32, Color32::from_black_alpha(150)),
        );

        // GBA Badge centered below screen
        if pad_y >= 26.0 {
            let label_pos = egui::Pos2::new(
                screen_rect.center().x,
                screen_rect.bottom() + (pad_y * 0.45).max(12.0),
            );
            ui.painter().text(
                label_pos,
                Align2::CENTER_CENTER,
                bezel_label,
                egui::FontId::monospace(13.0),
                badge_color,
            );
        }
    }

    pub(crate) fn draw_pause_overlay(&mut self, ui: &mut egui::Ui, disp_rect: egui::Rect) {
        if !self.show_pause_overlay {
            return;
        }
        ui.painter()
            .rect_filled(disp_rect, CornerRadius::ZERO, Color32::from_black_alpha(170));

        let card_w = 280.0;
        let card_h = 280.0;
        let card_rect = egui::Rect::from_center_size(disp_rect.center(), Vec2::new(card_w, card_h));

        ui.painter()
            .rect_filled(card_rect, CornerRadius::same(10), BG_CARD);
        ui.painter().rect_stroke(
            card_rect,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, BORDER_HAIRLINE),
            egui::StrokeKind::Outside,
        );

        let mut resume = false;
        let mut quick_save = false;
        let mut quick_load = false;
        let mut open_states = false;
        let mut hard_reset = false;
        let mut close_game = false;

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(card_rect.shrink(16.0)), |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(
                    RichText::new("PAUSED")
                        .strong()
                        .color(ACCENT_TEAL)
                        .size(18.0),
                );
                ui.label(RichText::new(&self.rom_title).color(Color32::WHITE).strong());
                ui.add_space(8.0);

                if ui
                    .add_sized([card_w - 40.0, 30.0], egui::Button::new("▶  Resume Emulation"))
                    .clicked()
                {
                    resume = true;
                }
                ui.add_space(4.0);
                let hardcore = self.is_ra_hardcore_active();
                if ui
                    .add_enabled(
                        !hardcore,
                        egui::Button::new("💾 Quick Save (Slot 1)").min_size(Vec2::new(card_w - 40.0, 26.0)),
                    )
                    .on_disabled_hover_text("Disabled in RetroAchievements Hardcore Mode")
                    .clicked()
                {
                    quick_save = true;
                }
                ui.add_space(4.0);
                if ui
                    .add_enabled(
                        !hardcore,
                        egui::Button::new("📂 Quick Load (Slot 1)").min_size(Vec2::new(card_w - 40.0, 26.0)),
                    )
                    .on_disabled_hover_text("Disabled in RetroAchievements Hardcore Mode")
                    .clicked()
                {
                    quick_load = true;
                }
                ui.add_space(4.0);
                if ui
                    .add_enabled(
                        !hardcore,
                        egui::Button::new("🖼️ Save State Manager (F6)")
                            .min_size(Vec2::new(card_w - 40.0, 26.0)),
                    )
                    .on_disabled_hover_text("Disabled in RetroAchievements Hardcore Mode")
                    .clicked()
                {
                    open_states = true;
                }
                ui.add_space(4.0);
                if ui
                    .add_sized([card_w - 40.0, 26.0], egui::Button::new("🔄 Hard Reset (Ctrl+R)"))
                    .clicked()
                {
                    hard_reset = true;
                }
                ui.add_space(4.0);
                if ui
                    .add_sized(
                        [card_w - 40.0, 26.0],
                        egui::Button::new(
                            RichText::new("⏹ Return to Library")
                                .color(Color32::from_rgb(255, 120, 120)),
                        ),
                    )
                    .clicked()
                {
                    close_game = true;
                }
            });
        });

        if resume {
            self.show_pause_overlay = false;
            self.paused = false;
            self.next_frame_due = Instant::now();
        }
        if quick_save {
            self.quick_save(1);
        }
        if quick_load {
            self.quick_load(1);
        }
        if open_states {
            self.show_state_manager = true;
        }
        if hard_reset {
            self.hard_reset();
            self.show_pause_overlay = false;
            self.paused = false;
        }
        if close_game {
            self.show_pause_overlay = false;
            self.close_rom(ui.ctx());
        }
    }

    pub(crate) fn draw_floating_osd_toast(&self, ctx: &egui::Context) {
        let Some(t) = &self.toast else { return };
        let now = Instant::now();
        if now > t.expires_at {
            return;
        }
        let remaining_ms = t.expires_at.saturating_duration_since(now).as_millis() as f32;
        let alpha = (remaining_ms / 350.0).clamp(0.0, 1.0);
        let (dot_color, border_color) = match t.level {
            crate::app::ToastLevel::Info => (ACCENT_TEAL, BORDER_HAIRLINE),
            crate::app::ToastLevel::Warning => (
                Color32::from_rgb(240, 180, 90),
                Color32::from_rgb(130, 95, 45),
            ),
            crate::app::ToastLevel::Error => (
                Color32::from_rgb(235, 95, 95),
                Color32::from_rgb(150, 52, 52),
            ),
        };
        let pos_y = if self.is_fullscreen { 20.0 } else { 76.0 };
        egui::Area::new(egui::Id::new("floating_osd_toast"))
            .anchor(egui::Align2::LEFT_TOP, [20.0, pos_y])
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(230))
                    .stroke(Stroke::new(1.0_f32, border_color))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("●").color(dot_color).small());
                            ui.label(RichText::new(&t.text).color(Color32::WHITE).strong());
                        });
                    });
            });
    }

    // WAVE-2 HOOK: Active viewport post-processing & HUD overlay rendering.
    // GLSL/WGSL shader passes plug in alongside `self.postproc.process_frame`, and
    // RetroAchievements unlock banners / Lua script canvas overlays draw over `disp_rect`.
    pub(crate) fn draw_active_viewport(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let color_correction = self.config.color_correction;
        let color_profile = self.effective_color_profile();
        let shader_preset = self.config.effective_shader_preset();
        let frame_blending = self.config.frame_blending;
        let lcd_ghosting = self.config.lcd_ghosting;
        let use_gl_shader = !self.is_gb_active()
            && !self.link.split_view_active()
            && self.config.shader_preset == ShaderPreset::CrtScanlines;
        let mut gl_pipeline_arc = None;

        if use_gl_shader
            && let Some(pipeline_arc) = &self.gl_shaders
            && let Ok(mut pipeline) = pipeline_arc.lock()
        {
            if pipeline.is_preset_available(shader_preset) {
                if self.sink.frame_dirty || pipeline.needs_initial_upload() {
                    pipeline.stage_frames(
                        &self.sink.framebuffer,
                        &self.sink.prev_framebuffer,
                        self.sink.has_prev_frame,
                    );
                    self.postproc.sync_rgba_for_capture(
                        &self.sink.framebuffer,
                        self.sink
                            .has_prev_frame
                            .then_some(&self.sink.prev_framebuffer),
                        self.sink.frame_seq,
                        color_correction,
                        color_profile,
                        frame_blending,
                        lcd_ghosting,
                    );
                    self.sink.frame_dirty = false;
                }
                gl_pipeline_arc = Some(pipeline_arc.clone());
            } else {
                pipeline.log_fallback_warning_once(&format!(
                    "GLSL shader preset {:?} unavailable, falling back to CPU post-processor",
                    shader_preset
                ));
            }
        }

        if gl_pipeline_arc.is_none() {
            let tex_options = self.config.display_filter.texture_options();
            if self.sink.frame_dirty || self.lcd_tex.is_none() {
                self.postproc.sync_rgba_for_capture(
                    &self.sink.framebuffer,
                    self.sink
                        .has_prev_frame
                        .then_some(&self.sink.prev_framebuffer),
                    self.sink.frame_seq,
                    color_correction,
                    color_profile,
                    frame_blending,
                    lcd_ghosting,
                );
                let green_swap = self
                    .core
                    .as_ref()
                    .map(|c| (c.bus.ppu.greenswap & 1) != 0)
                    .unwrap_or(false);
                let color_img = self.postproc.process_display_filter(
                    &self.sink.framebuffer,
                    color_correction,
                    lcd_ghosting,
                    self.config.display_filter,
                    green_swap,
                );
                if let Some(tex) = &mut self.lcd_tex {
                    if tex.size() == color_img.size {
                        tex.set(color_img.clone(), tex_options);
                    } else {
                        *tex = ctx.load_texture("gba_lcd", color_img.clone(), tex_options);
                    }
                } else {
                    self.lcd_tex = Some(ctx.load_texture("gba_lcd", color_img.clone(), tex_options));
                }
                self.sink.frame_dirty = false;
            }
        }

        let avail = ui.available_size();
        let mut toggle_fullscreen = false;
        let render_params = ShaderRenderParams {
            preset: shader_preset,
            color_profile_id: color_profile_uniform(color_correction, color_profile),
            frame_blend: if frame_blending && self.sink.has_prev_frame {
                0.5
            } else {
                0.0
            },
            lcd_ghosting: if lcd_ghosting && !frame_blending {
                0.35
            } else {
                0.0
            },
        };

        if self.link.split_view_active() {
            if let Some(local) = &mut self.link.local {
                let tex_options = self.config.display_filter.texture_options();
                let p2_img = local.p2_postproc.process_display_filter(
                    &local.p2_framebuffer,
                    color_correction,
                    lcd_ghosting,
                    self.config.display_filter,
                    false,
                );
                let p2_tex = local.p2_lcd_tex.get_or_insert_with(|| {
                    ctx.load_texture("gba_p2_lcd", p2_img.clone(), tex_options)
                });
                p2_tex.set(p2_img.clone(), tex_options);
            }

            let gap = 14.0;
            let header_h = 22.0;
            let half_w = ((avail.x - gap - 16.0) * 0.5).max(120.0);
            let half_h = (avail.y - header_h - 20.0).max(80.0);
            let panel_size = if self.config.integer_scaling {
                let factor = ((half_w / LCD_WIDTH as f32)
                    .floor()
                    .min((half_h / LCD_HEIGHT as f32).floor()))
                .max(1.0);
                Vec2::new(LCD_WIDTH as f32 * factor, LCD_HEIGHT as f32 * factor)
            } else {
                let scale = (half_w / LCD_WIDTH as f32)
                    .min(half_h / LCD_HEIGHT as f32)
                    .max(0.5);
                Vec2::new(LCD_WIDTH as f32 * scale, LCD_HEIGHT as f32 * scale)
            };
            let full_uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0));
            let total_w = panel_size.x * 2.0 + gap;
            let total_h = panel_size.y + header_h;

            let split_ids = if let (Some(tex1), Some(local)) = (&self.lcd_tex, &self.link.local) {
                local
                    .p2_lcd_tex
                    .as_ref()
                    .map(|tex2| (tex1.id(), tex2.id(), local.p2_rom_code.clone()))
            } else {
                None
            };

            ui.centered_and_justified(|ui| {
                if let Some((tex1_id, tex2_id, p2_code)) = split_ids.as_ref() {
                    let mut p1_rect = egui::Rect::NOTHING;
                    ui.allocate_ui(Vec2::new(total_w, total_h), |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;

                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("P1 (Master)")
                                        .small()
                                        .strong()
                                        .color(ACCENT_TEAL),
                                );
                                let (r1, resp1) = ui.allocate_exact_size(panel_size, Sense::click());
                                p1_rect = r1;
                                if resp1.double_clicked() {
                                    toggle_fullscreen = true;
                                }
                                ui.painter().image(*tex1_id, r1, full_uv, Color32::WHITE);
                                ui.painter().rect_stroke(
                                    r1,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                    egui::StrokeKind::Outside,
                                );
                            });

                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(format!("P2 (Slave) — {p2_code}"))
                                        .small()
                                        .strong()
                                        .color(TEXT_MUTED),
                                );
                                let (r2, resp2) = ui.allocate_exact_size(panel_size, Sense::click());
                                if resp2.double_clicked() {
                                    toggle_fullscreen = true;
                                }
                                ui.painter().image(*tex2_id, r2, full_uv, Color32::WHITE);
                                ui.painter().rect_stroke(
                                    r2,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                    egui::StrokeKind::Outside,
                                );
                            });
                        });
                    });
                    self.draw_pause_overlay(ui, p1_rect);
                    self.draw_viewport_recording_pill(ui, p1_rect);
                }
            });
        } else {
            let (ref_w, ref_h, uv) = if self.is_gb_active() {
                let u0 = GB_VIEW_OFFSET_X as f32 / LCD_WIDTH as f32;
                let v0 = GB_VIEW_OFFSET_Y as f32 / LCD_HEIGHT as f32;
                let u1 = (GB_VIEW_OFFSET_X + gb_core::LCD_WIDTH) as f32 / LCD_WIDTH as f32;
                let v1 = (GB_VIEW_OFFSET_Y + gb_core::LCD_HEIGHT) as f32 / LCD_HEIGHT as f32;
                (
                    gb_core::LCD_WIDTH as f32,
                    gb_core::LCD_HEIGHT as f32,
                    egui::Rect::from_min_max(egui::Pos2::new(u0, v0), egui::Pos2::new(u1, v1)),
                )
            } else {
                (
                    LCD_WIDTH as f32,
                    LCD_HEIGHT as f32,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                )
            };

            let disp_size = if self.config.integer_scaling {
                let factor = ((avail.x / ref_w).floor().min((avail.y / ref_h).floor())).max(1.0);
                Vec2::new(ref_w * factor, ref_h * factor)
            } else {
                let scale = (avail.x / ref_w).min(avail.y / ref_h).max(0.5);
                Vec2::new(ref_w * scale, ref_h * scale)
            };

            ui.centered_and_justified(|ui| {
                let has_surface = gl_pipeline_arc.is_some() || self.lcd_tex.is_some();
                if has_surface {
                    let (rect, resp) = ui.allocate_exact_size(disp_size, Sense::click());

                    if resp.double_clicked() {
                        toggle_fullscreen = true;
                    }

                    self.draw_bezel_frame(ui, rect, avail);

                    if let Some(pipeline_arc) = gl_pipeline_arc.take() {
                        let cb = create_viewport_paint_callback(pipeline_arc, rect, render_params);
                        ui.painter().add(egui::Shape::Callback(cb));
                    } else if let Some(tex) = &self.lcd_tex {
                        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
                    }
                    ui.painter().rect_stroke(
                        rect,
                        CornerRadius::ZERO,
                        Stroke::new(1.0_f32, BORDER_HAIRLINE),
                        egui::StrokeKind::Outside,
                    );

                    self.draw_pause_overlay(ui, rect);
                    self.draw_viewport_recording_pill(ui, rect);

                    // Render Lua `gui.rect` / `gui.text` HUD overlays scaled to the GBA viewport
                    let overlay_painter = ui.painter_at(rect);
                    let sx = rect.width() / LCD_WIDTH as f32;
                    let sy = rect.height() / LCD_HEIGHT as f32;
                    let text_scale = sx.min(sy).max(1.0);
                    self.lua.with_overlay_commands(|cmds| {
                        for cmd in cmds {
                            match cmd {
                                crate::lua_engine::GuiDrawCmd::Rect {
                                    x,
                                    y,
                                    w,
                                    h,
                                    color,
                                    fill,
                                } => {
                                    let r = egui::Rect::from_min_size(
                                        egui::Pos2::new(rect.left() + x * sx, rect.top() + y * sy),
                                        Vec2::new(w * sx, h * sy),
                                    );
                                    if let Some(f) = fill {
                                        overlay_painter.rect_filled(
                                            r,
                                            CornerRadius::ZERO,
                                            Color32::from_rgba_unmultiplied(f[0], f[1], f[2], f[3]),
                                        );
                                    }
                                    overlay_painter.rect_stroke(
                                        r,
                                        CornerRadius::ZERO,
                                        Stroke::new(
                                            1.0_f32,
                                            Color32::from_rgba_unmultiplied(
                                                color[0], color[1], color[2], color[3],
                                            ),
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                crate::lua_engine::GuiDrawCmd::Text { x, y, text, color } => {
                                    overlay_painter.text(
                                        egui::Pos2::new(rect.left() + x * sx, rect.top() + y * sy),
                                        egui::Align2::LEFT_TOP,
                                        text,
                                        egui::FontId::monospace((8.0 * text_scale).clamp(10.0, 20.0)),
                                        Color32::from_rgba_unmultiplied(
                                            color[0], color[1], color[2], color[3],
                                        ),
                                    );
                                }
                            }
                        }
                    });

                    // Render RetroAchievements Achievement Unlocked slide-in toast banner
                    if let Some(toast) = self.ra.current_unlock_banner() {
                        crate::retroachievements::draw_unlock_toast_banner(
                            &ui.painter_at(rect),
                            rect,
                            toast,
                        );
                        ctx.request_repaint();
                    }
                }
            });
        }

        if toggle_fullscreen {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
            self.notify(if self.is_fullscreen {
                "Entered Fullscreen"
            } else {
                "Exited Fullscreen"
            });
        }
    }

    fn draw_viewport_recording_pill(&self, ui: &egui::Ui, screen_rect: egui::Rect) {
        let mut segments: Vec<(String, Color32)> = Vec::new();
        if self.movie.is_recording() {
            segments.push((
                format!("REC {}f", self.movie.current_frame()),
                Color32::from_rgb(248, 113, 113),
            ));
        } else if self.movie.is_playing() {
            segments.push((
                format!(
                    "PLAY {}/{}",
                    self.movie.current_frame(),
                    self.movie.total_frames()
                ),
                ACCENT_TEAL,
            ));
        }
        if self.capture.is_gif_recording() {
            segments.push((
                format!("GIF {}f", self.capture.gif_frames()),
                Color32::from_rgb(248, 113, 113),
            ));
        }
        if self.capture.is_wav_recording() {
            segments.push((
                format!("WAV {:.1}s", self.capture.wav_duration_secs()),
                Color32::from_rgb(245, 180, 66),
            ));
        }
        if segments.is_empty() {
            return;
        }

        let label = segments
            .iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        let accent = segments[0].1;
        let galley = ui.painter().layout_no_wrap(
            label,
            egui::FontId::monospace(10.5),
            Color32::from_rgb(236, 240, 248),
        );
        let pad = Vec2::new(7.0, 3.0);
        let dot_w = 10.0;
        let box_size = Vec2::new(galley.size().x + pad.x * 2.0 + dot_w, galley.size().y + pad.y * 2.0);
        let pill_rect = egui::Rect::from_min_size(
            egui::pos2(
                screen_rect.right() - box_size.x - 8.0,
                screen_rect.top() + 8.0,
            ),
            box_size,
        );
        ui.painter().rect_filled(
            pill_rect,
            CornerRadius::same(3),
            Color32::from_rgba_unmultiplied(15, 18, 25, 215),
        );
        ui.painter().rect_stroke(
            pill_rect,
            CornerRadius::same(3),
            Stroke::new(1.0_f32, BORDER_HAIRLINE),
            egui::StrokeKind::Inside,
        );
        let dot_center = egui::pos2(pill_rect.left() + pad.x + 3.0, pill_rect.center().y);
        ui.painter().circle_filled(dot_center, 3.0, accent);
        ui.painter().galley(
            egui::pos2(pill_rect.left() + pad.x + dot_w, pill_rect.top() + pad.y),
            galley,
            Color32::WHITE,
        );
    }
}

fn draw_status_pill(ui: &mut egui::Ui, label: &str, fg: Color32, border: Color32, bg: Color32) {
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 6.0), Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 2.8, fg);
                ui.monospace(RichText::new(label).small().color(fg));
            });
        });
}
