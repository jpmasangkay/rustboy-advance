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
        if ctx.wants_keyboard_input()
            || self.debugger.awaiting_rebind.is_some()
            || self.awaiting_input_bind.is_some()
        {
            self.tab_fast_forward = false;
            self.rewinding = false;
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
        let stop_game = ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::W));
        if stop_game && self.core.is_some() {
            self.close_rom(ctx);
            return;
        }

        let hotkeys = self.config.keybindings.hotkeys.clone();
        let was_rewinding = self.rewinding;

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
            undo_state,
            fn_slot,
            open_bracket,
            close_bracket,
        ) = ctx.input(|i| {
            let rw_chord = hotkeys.get(HotkeyAction::Rewind);
            let ff_chord = hotkeys.get(HotkeyAction::FastForward);

            let extra_slot = [Key::F2, Key::F3, Key::F4, Key::F5]
                .iter()
                .enumerate()
                .find_map(|(offset, &k)| {
                    if !i.modifiers.ctrl && !i.modifiers.alt && i.key_pressed(k) {
                        Some((offset + 2, i.modifiers.shift))
                    } else {
                        None
                    }
                });

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
                i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::O),
                i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::T),
                i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::S),
                i.modifiers.ctrl && !i.modifiers.shift && !i.modifiers.alt && i.key_pressed(Key::Z),
                extra_slot,
                !i.modifiers.any() && i.key_pressed(Key::OpenBracket),
                !i.modifiers.any() && i.key_pressed(Key::CloseBracket),
            )
        });

        self.tab_fast_forward = ff_held;

        if self.core.is_some() && rewind_held {
            if !self.config.rewind_enabled {
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
        if state_mgr_trig {
            self.show_state_manager = !self.show_state_manager;
        }
        if open_rom {
            self.trigger_open_rom_dialog(ctx);
        }
        if frame_adv_trig {
            self.step_single_frame(ctx);
        }
        if hard_reset_trig {
            self.hard_reset();
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
        if pause_trig && self.core.is_some() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_chord_requires_exact_modifiers() {
        let bare_r = HotkeyChord::simple("R");
        let ctrl_r = HotkeyChord::with_ctrl("R");
        let shift_f1 = HotkeyChord::with_shift("F1");

        assert!(chord_matches_key_event(&bare_r, Key::R, egui::Modifiers::NONE));
        assert!(!chord_matches_key_event(&bare_r, Key::R, egui::Modifiers::CTRL));
        assert!(chord_matches_key_event(&ctrl_r, Key::R, egui::Modifiers::CTRL));
        assert!(!chord_matches_key_event(&ctrl_r, Key::R, egui::Modifiers::SHIFT));
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
}
