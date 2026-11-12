use egui::{self, Key};

use crate::app::RustboyApp;

impl RustboyApp {
    pub(crate) fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.debugger.awaiting_rebind.is_some() {
            return;
        }

        // Smart Escape key handling:
        // In Fullscreen: Escape always exits Fullscreen back to Windowed mode.
        // In Windowed mode: Escape toggles the in-game Pause Overlay Menu!
        if ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::Escape)) {
            if self.is_fullscreen {
                self.is_fullscreen = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                self.notify("Exited Fullscreen");
                return;
            } else if self.core.is_some() {
                self.show_pause_overlay = !self.show_pause_overlay;
                self.paused = self.show_pause_overlay;
                return;
            }
        }

        // Ctrl+W returns to library when a game is running
        let stop_game = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::W));
        if stop_game && self.core.is_some() {
            self.close_rom(ctx);
            return;
        }

        // Holding R in-game rewinds real-time
        let was_rewinding = self.rewinding;
        let rewind_held = self.core.is_some()
            && ctx.input(|i| i.key_down(Key::R) && !i.modifiers.ctrl && !i.modifiers.alt);
        if rewind_held {
            if !self.config.rewind_enabled {
                if !was_rewinding && ctx.input(|i| i.key_pressed(Key::R)) {
                    self.notify("Rewind is disabled in Emulation -> Rewind Settings");
                }
                self.rewinding = false;
            } else if self.rewind_buffer.is_empty() {
                if !was_rewinding && ctx.input(|i| i.key_pressed(Key::R)) {
                    self.notify("Rewind buffer empty");
                }
                self.rewinding = false;
            } else {
                if !was_rewinding {
                    self.next_frame_due = std::time::Instant::now();
                }
                self.rewinding = true;
            }
        } else {
            self.rewinding = false;
            if was_rewinding {
                self.next_frame_due = std::time::Instant::now();
            }
        }

        let open_rom = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::O));
        let step_frame = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::N));
        let hard_reset = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::R));
        let toggle_mute = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::M));
        let toggle_pause = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::Space));
        let screenshot = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::F12));
        let fullscreen = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::F11));
        let toggle_toolbar = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::T));
        let toggle_status_bar = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::S));
        let undo_state = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::Z));
        let toggle_state_manager = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::F6));

        let fn_slot = ctx.input(|i| {
            let keys = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5];
            keys.iter()
                .position(|&k| i.key_pressed(k))
                .map(|idx| (idx + 1, i.modifiers.shift))
        });

        // Boktai Solar Sensor hotkeys: '[' / ']'
        let solar_delta = if let Some(core) = &self.core {
            if core.bus.cart.solar_enabled {
                if ctx.input(|i| i.key_pressed(Key::OpenBracket)) {
                    Some(-25i16)
                } else if ctx.input(|i| i.key_pressed(Key::CloseBracket)) {
                    Some(25i16)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(delta) = solar_delta {
            if let Some(core) = &mut self.core {
                if delta < 0 {
                    core.bus.cart.solar_lux = core.bus.cart.solar_lux.saturating_sub(25);
                } else {
                    core.bus.cart.solar_lux = core.bus.cart.solar_lux.saturating_add(25);
                }
                let lvl = ((core.bus.cart.solar_lux as f32 / 255.0) * 10.0).round() as u8;
                self.notify(&format!("☀️ Solar Sunlight: Level {lvl}/10"));
            }
        }

        if toggle_toolbar {
            self.config.show_toolbar = !self.config.show_toolbar;
            self.config.save();
            self.notify(if self.config.show_toolbar {
                "Toolbar Shown (Ctrl+T)"
            } else {
                "Toolbar Hidden (Ctrl+T)"
            });
        }
        if toggle_status_bar {
            self.config.show_status_bar = !self.config.show_status_bar;
            self.config.save();
            self.notify(if self.config.show_status_bar {
                "Status Bar Shown (Ctrl+S)"
            } else {
                "Status Bar Hidden (Ctrl+S)"
            });
        }
        if undo_state {
            self.undo_last_state_action();
        }
        if toggle_state_manager {
            self.show_state_manager = !self.show_state_manager;
        }

        if open_rom {
            self.trigger_open_rom_dialog(ctx);
        }
        if step_frame {
            self.step_single_frame();
        }
        if hard_reset {
            self.hard_reset();
        }
        if toggle_mute {
            self.config.muted = !self.config.muted;
            self.sink.audio.set_muted(self.config.muted);
            self.config.save();
            self.notify(if self.config.muted {
                "Audio Muted"
            } else {
                "Audio Unmuted"
            });
        }
        if toggle_pause && self.core.is_some() {
            self.toggle_pause();
        }
        if screenshot {
            self.capture_screenshot();
        }
        if fullscreen {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
            self.notify(if self.is_fullscreen {
                "Entered Fullscreen"
            } else {
                "Exited Fullscreen"
            });
        }
        if let Some((slot, is_load)) = fn_slot {
            if is_load {
                self.quick_load(slot);
            } else {
                self.quick_save(slot);
            }
        }
    }
}
