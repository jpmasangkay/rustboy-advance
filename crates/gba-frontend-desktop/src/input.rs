use egui::{self, Key};
use gba_core::encode_keyinput;
use gilrs::{Axis, Button};

use crate::app::RustboyApp;
use crate::config::GbaButton;

impl RustboyApp {
    pub(crate) fn poll_inputs(&mut self, ctx: &egui::Context) -> u16 {
        let mut mask = 0u16;
        let allow_kbd = !ctx.wants_keyboard_input() && self.debugger.awaiting_rebind.is_none();

        if allow_kbd {
            ctx.input(|i| {
                for btn in GbaButton::ALL {
                    if let Some(k) = self.config.keybindings.egui_key(btn) {
                        if i.key_down(k) {
                            mask |= btn.mask();
                        }
                    }
                }
                self.tab_fast_forward = i.key_down(Key::Tab);
            });
        } else {
            self.tab_fast_forward = false;
        }

        if let Some(gilrs) = &mut self.gilrs {
            while gilrs.next_event().is_some() {}
            for (_id, pad) in gilrs.gamepads() {
                if !pad.is_connected() {
                    continue;
                }
                if pad.is_pressed(Button::East) || pad.is_pressed(Button::North) {
                    mask |= gba_core::KEY_A;
                }
                if pad.is_pressed(Button::South) || pad.is_pressed(Button::West) {
                    mask |= gba_core::KEY_B;
                }
                if pad.is_pressed(Button::Select) {
                    mask |= gba_core::KEY_SELECT;
                }
                if pad.is_pressed(Button::Start) {
                    mask |= gba_core::KEY_START;
                }
                if pad.is_pressed(Button::DPadRight)
                    || pad.value(Axis::LeftStickX) > 0.4
                {
                    mask |= gba_core::KEY_RIGHT;
                }
                if pad.is_pressed(Button::DPadLeft)
                    || pad.value(Axis::LeftStickX) < -0.4
                {
                    mask |= gba_core::KEY_LEFT;
                }
                if pad.is_pressed(Button::DPadUp)
                    || pad.value(Axis::LeftStickY) > 0.4
                {
                    mask |= gba_core::KEY_UP;
                }
                if pad.is_pressed(Button::DPadDown)
                    || pad.value(Axis::LeftStickY) < -0.4
                {
                    mask |= gba_core::KEY_DOWN;
                }
                if pad.is_pressed(Button::RightTrigger) || pad.is_pressed(Button::RightTrigger2) {
                    mask |= gba_core::KEY_R;
                }
                if pad.is_pressed(Button::LeftTrigger) || pad.is_pressed(Button::LeftTrigger2) {
                    mask |= gba_core::KEY_L;
                }
            }
        }

        self.last_pressed_mask = mask;
        encode_keyinput(mask)
    }

    pub(crate) fn connected_gamepad_names(&self) -> Vec<String> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        gilrs
            .gamepads()
            .filter(|(_, p)| p.is_connected())
            .map(|(_, p)| p.name().to_string())
            .collect()
    }
}
