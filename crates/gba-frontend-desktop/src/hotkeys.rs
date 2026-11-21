use egui::{self, Key};

use crate::app::RustboyApp;
use crate::config::{HotkeyAction, HotkeyChord};

pub(crate) fn modifiers_match(chord: &HotkeyChord, modifiers: egui::Modifiers) -> bool {
    let ctrl_down = modifiers.ctrl || modifiers.mac_cmd;
    ctrl_down == chord.ctrl && modifiers.shift == chord.shift && modifiers.alt == chord.alt
}

pub(crate) fn chord_matches_key_event(
    chord: &HotkeyChord,
    key: Key,
    modifiers: egui::Modifiers,
) -> bool {
    chord.egui_key() == Some(key) && modifiers_match(chord, modifiers)
}

pub(crate) fn is_chord_held(i: &egui::InputState, chord: &HotkeyChord) -> bool {
    let Some(key) = chord.egui_key() else {
        return false;
    };
    i.key_down(key) && modifiers_match(chord, i.modifiers)
}

fn is_hotkey_triggered(
    i: &egui::InputState,
    chord: Option<&HotkeyChord>,
    allow_repeat: bool,
) -> bool {
    let Some(chord) = chord else {
        return false;
    };
    i.events.iter().any(|ev| match ev {
        egui::Event::Key {
            key,
            pressed: true,
            repeat,
            modifiers,
            ..
        } => (allow_repeat || !*repeat) && chord_matches_key_event(chord, *key, *modifiers),
        _ => false,
    })
}

fn is_hotkey_just_pressed(i: &egui::InputState, chord: Option<&HotkeyChord>) -> bool {
    is_hotkey_triggered(i, chord, false)
}

impl RustboyApp {
    pub(crate) fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| {
            i.modifiers.alt && !i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(Key::F4)
        }) {
            self.handle_alt_f4_quit();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if ctx.input(|i| {
            let cmd = i.modifiers.ctrl || i.modifiers.mac_cmd;
            cmd && i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::M)
        }) {
            self.config.show_menu_bar = !self.config.show_menu_bar;
            self.config.save();
            self.notify(if self.config.show_menu_bar {
                "Menu Bar Shown (Ctrl+Shift+M)"
            } else {
                "Menu Bar Hidden — press F10 or Ctrl+Shift+M to restore"
            });
        }

        if ctx.wants_keyboard_input() || self.awaiting_input_bind.is_some() {
            self.tab_fast_forward = false;
            self.rewinding = false;
            return;
        }

        // Smart Escape key handling:
        // 1. Close the top-most open dialog / popup (never loses focus silently)
        // 2. Exit Fullscreen back to Windowed mode
        // 3. Toggle the in-game Pause Overlay Menu (flushing .sav synchronously)
        if ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::Escape))
            && self.handle_escape_key(ctx)
        {
            return;
        }

        // Ctrl/Cmd+W returns to library when a game is running
        let stop_game = ctx.input(|i| {
            let cmd = i.modifiers.ctrl || i.modifiers.mac_cmd;
            cmd && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::W)
        });
        if stop_game && self.has_active_game() {
            self.request_close_rom(ctx);
            return;
        }

        let hotkeys = self.config.keybindings.hotkeys.clone();
        let was_rewinding = self.rewinding;
        let hardcore = self.is_ra_hardcore_active();

        let (
            rewind_held,
            rewind_pressed,
            ff_held,
            pause_trig,
            frame_adv_trig,
            save_trig,
            load_trig,
            state_mgr_trig,
            screenshot_trig,
            hard_reset_trig,
            mute_trig,
            fullscreen_trig,
            open_rom,
            toggle_toolbar,
            toggle_status_bar,
            toggle_menu_bar,
            undo_state,
            fn_slot,
            open_bracket,
            close_bracket,
        ) = ctx.input(|i| {
            let cmd = i.modifiers.ctrl || i.modifiers.mac_cmd;
            let rw_chord = hotkeys.get(HotkeyAction::Rewind);
            let ff_chord = hotkeys.get(HotkeyAction::FastForward);

            let is_bound_to_configured_hotkey = |key: Key, shift: bool| -> bool {
                HotkeyAction::ALL.iter().any(|&action| {
                    hotkeys.get(action).is_some_and(|c| {
                        !c.ctrl && !c.alt && c.shift == shift && c.egui_key() == Some(key)
                    })
                })
            };

            let extra_slot = [Key::F2, Key::F3, Key::F4, Key::F5]
                .iter()
                .enumerate()
                .find_map(|(offset, &k)| {
                    if !cmd
                        && !i.modifiers.alt
                        && i.key_pressed(k)
                        && !is_bound_to_configured_hotkey(k, i.modifiers.shift)
                    {
                        Some((offset + 2, i.modifiers.shift))
                    } else {
                        None
                    }
                });

            let menu_toggle = !i.modifiers.any() && i.key_pressed(Key::F10);

            (
                rw_chord.is_some_and(|c| is_chord_held(i, c)),
                is_hotkey_just_pressed(i, rw_chord),
                ff_chord.is_some_and(|c| is_chord_held(i, c)),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::Pause), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::FrameAdvance), true),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::SaveState), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::LoadState), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::StateManager), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::Screenshot), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::HardReset), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::Mute), false),
                is_hotkey_triggered(i, hotkeys.get(HotkeyAction::Fullscreen), false),
                cmd && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::O),
                cmd && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::T),
                cmd && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::S),
                menu_toggle,
                cmd && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::Z),
                extra_slot,
                !i.modifiers.any() && i.key_pressed(Key::OpenBracket),
                !i.modifiers.any() && i.key_pressed(Key::CloseBracket),
            )
        });

        self.tab_fast_forward = ff_held;

        if self.has_active_game() && rewind_held {
            if hardcore {
                if !was_rewinding && rewind_pressed {
                    self.notify_warn("Rewind is disabled in RetroAchievements Hardcore Mode");
                }
                self.rewinding = false;
            } else if !self.config.rewind_enabled {
                if !was_rewinding && rewind_pressed {
                    self.notify("Rewind is disabled in Emulation -> Rewind Settings");
                }
                self.rewinding = false;
            } else if self.rewind_buffer.is_empty() {
                if !was_rewinding && rewind_pressed {
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

        // Boktai Solar Sensor hotkeys: '[' / ']'
        if let Some(core) = &mut self.core
            && core.bus.cart.solar_enabled
        {
            if open_bracket {
                core.bus.cart.solar_lux = core.bus.cart.solar_lux.saturating_sub(25);
                let lvl = ((core.bus.cart.solar_lux as f32 / 255.0) * 10.0).round() as u8;
                self.notify(&format!("☀️ Solar Sunlight: Level {lvl}/10"));
            } else if close_bracket {
                core.bus.cart.solar_lux = core.bus.cart.solar_lux.saturating_add(25);
                let lvl = ((core.bus.cart.solar_lux as f32 / 255.0) * 10.0).round() as u8;
                self.notify(&format!("☀️ Solar Sunlight: Level {lvl}/10"));
            }
        }

        if toggle_toolbar {
            self.config.show_toolbar = !self.config.show_toolbar;
            self.config.save();
            self.notify(if self.config.show_toolbar {
                "Toolbar Shown"
            } else {
                "Toolbar Hidden"
            });
        }
        if toggle_status_bar {
            self.config.show_status_bar = !self.config.show_status_bar;
            self.config.save();
            self.notify(if self.config.show_status_bar {
                "Status Bar Shown"
            } else {
                "Status Bar Hidden"
            });
        }
        if toggle_menu_bar {
            self.config.show_menu_bar = !self.config.show_menu_bar;
            self.config.save();
            self.notify(if self.config.show_menu_bar {
                "Menu Bar Shown"
            } else {
                "Menu Bar Hidden (F10 or Ctrl/Cmd+Shift+M to restore)"
            });
        }
        if undo_state {
            self.undo_last_state_action();
        }
        if state_mgr_trig {
            if hardcore {
                self.notify_warn(
                    "Save State Manager is disabled in RetroAchievements Hardcore Mode",
                );
            } else {
                self.show_state_manager = !self.show_state_manager;
            }
        }
        if open_rom {
            self.trigger_open_rom_dialog(ctx);
        }
        if frame_adv_trig {
            if hardcore {
                self.notify_warn("Frame Advance is disabled in RetroAchievements Hardcore Mode");
            } else {
                self.step_single_frame(ctx);
            }
        }
        if hard_reset_trig {
            self.request_hard_reset();
        }
        if mute_trig {
            self.config.muted = !self.config.muted;
            self.sink.audio.set_muted(self.config.muted);
            self.config.save();
            self.notify(if self.config.muted {
                "Audio Muted"
            } else {
                "Audio Unmuted"
            });
        }
        if pause_trig && self.has_active_game() {
            self.toggle_pause();
        }
        if screenshot_trig {
            self.capture_screenshot();
        }
        if fullscreen_trig {
            self.is_fullscreen = !self.is_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
            self.notify(if self.is_fullscreen {
                "Entered Fullscreen"
            } else {
                "Exited Fullscreen"
            });
        }
        if save_trig {
            self.quick_save(1);
        }
        if load_trig {
            self.quick_load(1);
        }
        if let Some((slot, is_load)) = fn_slot {
            if is_load {
                self.quick_load(slot);
            } else {
                self.quick_save(slot);
            }
        }
    }

    /// Processes Escape precedence: top dialog -> fullscreen -> pause overlay.
    pub(crate) fn handle_escape_key(&mut self, ctx: &egui::Context) -> bool {
        if self.close_top_dialog_on_escape(ctx) {
            return true;
        }
        if self.is_fullscreen {
            self.is_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.notify("Exited Fullscreen");
            return true;
        }
        self.pause_and_flush_on_escape()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::CliConfig;
    use std::mem::ManuallyDrop;

    #[test]
    fn test_hotkey_chord_requires_exact_modifiers() {
        let bare_r = HotkeyChord::simple("R");
        let ctrl_r = HotkeyChord::with_ctrl("R");
        let shift_f1 = HotkeyChord::with_shift("F1");

        assert!(chord_matches_key_event(
            &bare_r,
            Key::R,
            egui::Modifiers::NONE
        ));
        assert!(!chord_matches_key_event(
            &bare_r,
            Key::R,
            egui::Modifiers::CTRL
        ));
        assert!(chord_matches_key_event(
            &ctrl_r,
            Key::R,
            egui::Modifiers::CTRL
        ));
        assert!(!chord_matches_key_event(
            &ctrl_r,
            Key::R,
            egui::Modifiers::SHIFT
        ));
        assert!(chord_matches_key_event(
            &shift_f1,
            Key::F1,
            egui::Modifiers::SHIFT
        ));
        assert!(!chord_matches_key_event(
            &shift_f1,
            Key::F1,
            egui::Modifiers::NONE
        ));
    }

    #[test]
    fn test_escape_precedence_dialog_then_fullscreen_then_pause() {
        let mut app = ManuallyDrop::new(RustboyApp::new(
            CliConfig::default(),
            crate::config::AppConfig::default(),
        ));
        app.core = Some(gba_core::GbaCore::new(None, vec![0u8; 256]));
        app.sav_path = None;
        app.config.pause_overlay_enabled = true;
        app.is_fullscreen = true;
        app.show_settings = true;
        app.paused = false;
        app.show_pause_overlay = false;

        let ctx = egui::Context::default();
        let esc_input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };

        // 1st Esc: closes dialog first, keeps fullscreen and running state
        let _ = ctx.run(esc_input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(!app.show_settings);
        assert!(app.is_fullscreen);
        assert!(!app.show_pause_overlay);
        assert!(!app.paused);

        // 2nd Esc: exits fullscreen, keeps running state
        let _ = ctx.run(esc_input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(!app.is_fullscreen);
        assert!(!app.show_pause_overlay);
        assert!(!app.paused);

        // 3rd Esc: toggles pause overlay on
        let _ = ctx.run(esc_input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(app.show_pause_overlay);
        assert!(app.paused);

        // 4th Esc: resumes gameplay
        let _ = ctx.run(esc_input, |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(!app.show_pause_overlay);
        assert!(!app.paused);
    }

    #[test]
    fn test_escape_cancels_pending_confirm_before_fullscreen_or_pause() {
        let mut app = ManuallyDrop::new(RustboyApp::new(
            CliConfig::default(),
            crate::config::AppConfig::default(),
        ));
        app.core = Some(gba_core::GbaCore::new(None, vec![0u8; 256]));
        app.config.pause_overlay_enabled = true;
        app.is_fullscreen = true;
        app.paused = false;
        app.show_pause_overlay = false;
        app.pending_confirm_action = Some(crate::app::PendingConfirmAction::HardReset);

        let ctx = egui::Context::default();
        let esc_input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };

        // 1st Esc closes confirmation dialog only
        let _ = ctx.run(esc_input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(app.pending_confirm_action.is_none());
        assert!(app.is_fullscreen);
        assert!(!app.show_pause_overlay);

        // 2nd Esc exits fullscreen only
        let _ = ctx.run(esc_input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(!app.is_fullscreen);
        assert!(!app.show_pause_overlay);

        // 3rd Esc opens pause overlay
        let _ = ctx.run(esc_input, |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(app.show_pause_overlay);
        assert!(app.paused);
    }

    #[test]
    fn test_ctrl_shift_m_toggles_menu_bar_once_not_twice() {
        let mut app = ManuallyDrop::new(RustboyApp::new(
            CliConfig::default(),
            crate::config::AppConfig::default(),
        ));
        app.config.show_menu_bar = true;

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            events: vec![egui::Event::Key {
                key: Key::M,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        };

        let _ = ctx.run(input.clone(), |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(
            !app.config.show_menu_bar,
            "Ctrl+Shift+M must toggle menu bar off in one press"
        );

        let _ = ctx.run(input, |ctx| {
            app.handle_global_shortcuts(ctx);
        });
        assert!(
            app.config.show_menu_bar,
            "second Ctrl+Shift+M must restore menu bar"
        );
    }
}
