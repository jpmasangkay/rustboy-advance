use egui::{self, Color32, CornerRadius, Key, RichText, Stroke};
use gba_core::encode_keyinput;
use gilrs::{Axis, Button, EventType};

use crate::app::{
    RustboyApp, ACCENT_TEAL, ACCENT_TEAL_BG, BG_CARD, BORDER_HAIRLINE, TEXT_MUTED,
};
use crate::config::{
    format_key_name, parse_key_name, AutofireMode, ControlTarget, GamepadAxisCode, GamepadBindings,
    GamepadButtonCode, GamepadInput, GbaButton, HotkeyAction, HotkeyBindings, HotkeyChord,
    KeyBindings,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlsTab {
    Keyboard,
    Gamepad,
    Autofire,
    Hotkeys,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingInputBind {
    Keyboard(ControlTarget),
    Gamepad(ControlTarget),
    Hotkey(HotkeyAction),
}

impl PendingInputBind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Keyboard(t) | Self::Gamepad(t) => t.label(),
            Self::Hotkey(a) => a.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindingConflict {
    pub(crate) target_a: String,
    pub(crate) target_b: String,
    pub(crate) binding_desc: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AutofireState {
    pub(crate) turbo_a_latched: bool,
    pub(crate) turbo_b_latched: bool,
    prev_turbo_a_down: bool,
    prev_turbo_b_down: bool,
}

impl AutofireState {
    pub(crate) fn clear_latches(&mut self) {
        self.turbo_a_latched = false;
        self.turbo_b_latched = false;
        self.prev_turbo_a_down = false;
        self.prev_turbo_b_down = false;
    }

    pub(crate) fn update_latches(
        &mut self,
        raw_turbo_a: bool,
        raw_turbo_b: bool,
        mode: AutofireMode,
    ) -> (bool, bool) {
        let rising_a = raw_turbo_a && !self.prev_turbo_a_down;
        let rising_b = raw_turbo_b && !self.prev_turbo_b_down;
        self.prev_turbo_a_down = raw_turbo_a;
        self.prev_turbo_b_down = raw_turbo_b;

        match mode {
            AutofireMode::Hold => (raw_turbo_a, raw_turbo_b),
            AutofireMode::Toggle => {
                if rising_a {
                    self.turbo_a_latched = !self.turbo_a_latched;
                }
                if rising_b {
                    self.turbo_b_latched = !self.turbo_b_latched;
                }
                (self.turbo_a_latched, self.turbo_b_latched)
            }
        }
    }
}

/// Computes the GBA key bitmask contribution of Turbo A / Turbo B on `frame_counter`.
///
/// `interval_frames` is the full square-wave period in GBA frames (clamped to `2..=16`),
/// holding the button active for the first half of each cycle and released for the second half.
pub(crate) fn autofire_pulse_mask(
    active_a: bool,
    active_b: bool,
    frame_counter: u64,
    interval_frames: u8,
) -> u16 {
    let period = u64::from(interval_frames.clamp(2, 16));
    let half = (period / 2).max(1);
    let high = (frame_counter % period) < half;
    if !high {
        return 0;
    }
    let mut mask = 0u16;
    if active_a {
        mask |= GbaButton::A.mask();
    }
    if active_b {
        mask |= GbaButton::B.mask();
    }
    mask
}

pub(crate) fn detect_keyboard_conflicts(bindings: &KeyBindings) -> Vec<BindingConflict> {
    let mut conflicts = Vec::new();
    let targets = ControlTarget::ALL;
    for (i, &ta) in targets.iter().enumerate() {
        let Some(ka) = bindings.target_egui_key(ta) else {
            continue;
        };
        for &tb in &targets[i + 1..] {
            if bindings.target_egui_key(tb) == Some(ka) {
                conflicts.push(BindingConflict {
                    target_a: ta.label().to_string(),
                    target_b: tb.label().to_string(),
                    binding_desc: format_key_name(ka),
                });
            }
        }
        // Warn if an unmodified hotkey uses the same key as a playable keyboard control.
        for &action in &HotkeyAction::ALL {
            if let Some(chord) = bindings.hotkeys.get(action)
                && !chord.ctrl
                && !chord.shift
                && !chord.alt
                && chord.egui_key() == Some(ka)
            {
                conflicts.push(BindingConflict {
                    target_a: ta.label().to_string(),
                    target_b: format!("Hotkey: {}", action.label()),
                    binding_desc: format_key_name(ka),
                });
            }
        }
    }
    conflicts
}

pub(crate) fn detect_gamepad_conflicts(bindings: &GamepadBindings) -> Vec<BindingConflict> {
    let mut conflicts = Vec::new();
    let targets = ControlTarget::ALL;
    for (i, &ta) in targets.iter().enumerate() {
        let Some(ia) = bindings.get(ta) else {
            continue;
        };
        for &tb in &targets[i + 1..] {
            if bindings.get(tb) == Some(ia) {
                conflicts.push(BindingConflict {
                    target_a: ta.label().to_string(),
                    target_b: tb.label().to_string(),
                    binding_desc: ia.label(),
                });
            }
        }
    }
    conflicts
}

pub(crate) fn detect_hotkey_conflicts(hotkeys: &HotkeyBindings) -> Vec<BindingConflict> {
    let mut conflicts = Vec::new();
    let actions = HotkeyAction::ALL;
    for (i, &aa) in actions.iter().enumerate() {
        let Some(ca) = hotkeys.get(aa) else {
            continue;
        };
        let norm_a = (ca.egui_key(), ca.ctrl, ca.shift, ca.alt);
        for &ab in &actions[i + 1..] {
            let Some(cb) = hotkeys.get(ab) else {
                continue;
            };
            let norm_b = (cb.egui_key(), cb.ctrl, cb.shift, cb.alt);
            let same = match (norm_a.0, norm_b.0) {
                (Some(ka), Some(kb)) => {
                    ka == kb && ca.ctrl == cb.ctrl && ca.shift == cb.shift && ca.alt == cb.alt
                }
                _ => {
                    ca.key.eq_ignore_ascii_case(&cb.key)
                        && ca.ctrl == cb.ctrl
                        && ca.shift == cb.shift
                        && ca.alt == cb.alt
                }
            };
            if same {
                conflicts.push(BindingConflict {
                    target_a: aa.label().to_string(),
                    target_b: ab.label().to_string(),
                    binding_desc: ca.display(),
                });
            }
        }
    }
    conflicts
}

fn gilrs_button_to_code(btn: Button) -> Option<GamepadButtonCode> {
    match btn {
        Button::South => Some(GamepadButtonCode::South),
        Button::East => Some(GamepadButtonCode::East),
        Button::North => Some(GamepadButtonCode::North),
        Button::West => Some(GamepadButtonCode::West),
        Button::C => Some(GamepadButtonCode::C),
        Button::Z => Some(GamepadButtonCode::Z),
        Button::LeftTrigger => Some(GamepadButtonCode::LeftTrigger),
        Button::LeftTrigger2 => Some(GamepadButtonCode::LeftTrigger2),
        Button::RightTrigger => Some(GamepadButtonCode::RightTrigger),
        Button::RightTrigger2 => Some(GamepadButtonCode::RightTrigger2),
        Button::Select => Some(GamepadButtonCode::Select),
        Button::Start => Some(GamepadButtonCode::Start),
        Button::Mode => Some(GamepadButtonCode::Mode),
        Button::LeftThumb => Some(GamepadButtonCode::LeftThumb),
        Button::RightThumb => Some(GamepadButtonCode::RightThumb),
        Button::DPadUp => Some(GamepadButtonCode::DPadUp),
        Button::DPadDown => Some(GamepadButtonCode::DPadDown),
        Button::DPadLeft => Some(GamepadButtonCode::DPadLeft),
        Button::DPadRight => Some(GamepadButtonCode::DPadRight),
        Button::Unknown => None,
    }
}

fn code_to_gilrs_button(code: GamepadButtonCode) -> Button {
    match code {
        GamepadButtonCode::South => Button::South,
        GamepadButtonCode::East => Button::East,
        GamepadButtonCode::North => Button::North,
        GamepadButtonCode::West => Button::West,
        GamepadButtonCode::C => Button::C,
        GamepadButtonCode::Z => Button::Z,
        GamepadButtonCode::LeftTrigger => Button::LeftTrigger,
        GamepadButtonCode::LeftTrigger2 => Button::LeftTrigger2,
        GamepadButtonCode::RightTrigger => Button::RightTrigger,
        GamepadButtonCode::RightTrigger2 => Button::RightTrigger2,
        GamepadButtonCode::Select => Button::Select,
        GamepadButtonCode::Start => Button::Start,
        GamepadButtonCode::Mode => Button::Mode,
        GamepadButtonCode::LeftThumb => Button::LeftThumb,
        GamepadButtonCode::RightThumb => Button::RightThumb,
        GamepadButtonCode::DPadUp => Button::DPadUp,
        GamepadButtonCode::DPadDown => Button::DPadDown,
        GamepadButtonCode::DPadLeft => Button::DPadLeft,
        GamepadButtonCode::DPadRight => Button::DPadRight,
    }
}

fn axis_to_code(axis: Axis) -> Option<GamepadAxisCode> {
    match axis {
        Axis::LeftStickX => Some(GamepadAxisCode::LeftStickX),
        Axis::LeftStickY => Some(GamepadAxisCode::LeftStickY),
        Axis::RightStickX => Some(GamepadAxisCode::RightStickX),
        Axis::RightStickY => Some(GamepadAxisCode::RightStickY),
        Axis::DPadX => Some(GamepadAxisCode::DPadX),
        Axis::DPadY => Some(GamepadAxisCode::DPadY),
        Axis::LeftZ => Some(GamepadAxisCode::LeftZ),
        Axis::RightZ => Some(GamepadAxisCode::RightZ),
        Axis::Unknown => None,
    }
}

fn code_to_axis(code: GamepadAxisCode) -> Axis {
    match code {
        GamepadAxisCode::LeftStickX => Axis::LeftStickX,
        GamepadAxisCode::LeftStickY => Axis::LeftStickY,
        GamepadAxisCode::RightStickX => Axis::RightStickX,
        GamepadAxisCode::RightStickY => Axis::RightStickY,
        GamepadAxisCode::DPadX => Axis::DPadX,
        GamepadAxisCode::DPadY => Axis::DPadY,
        GamepadAxisCode::LeftZ => Axis::LeftZ,
        GamepadAxisCode::RightZ => Axis::RightZ,
    }
}

fn is_gamepad_input_active(gamepad: &gilrs::Gamepad<'_>, input: GamepadInput) -> bool {
    const AXIS_THRESHOLD: f32 = 0.45;
    match input {
        GamepadInput::Button(code) => gamepad.is_pressed(code_to_gilrs_button(code)),
        GamepadInput::AxisNegative(code) => gamepad
            .axis_data(code_to_axis(code))
            .is_some_and(|d| d.value() < -AXIS_THRESHOLD),
        GamepadInput::AxisPositive(code) => gamepad
            .axis_data(code_to_axis(code))
            .is_some_and(|d| d.value() > AXIS_THRESHOLD),
    }
}

impl RustboyApp {
    /// Drains pending `gilrs` events once per UI frame and handles active gamepad bind capture.
    pub(crate) fn pump_gamepad_events(&mut self, ctx: &egui::Context) {
        let mut captured_input: Option<GamepadInput> = None;
        let awaiting_gamepad = matches!(self.awaiting_input_bind, Some(PendingInputBind::Gamepad(_)));

        if let Some(gilrs) = &mut self.gilrs {
            while let Some(ev) = gilrs.next_event() {
                if awaiting_gamepad && captured_input.is_none() {
                    match ev.event {
                        EventType::ButtonPressed(btn, _) => {
                            if let Some(code) = gilrs_button_to_code(btn) {
                                captured_input = Some(GamepadInput::Button(code));
                            }
                        }
                        EventType::AxisChanged(axis, value, _) if value.abs() >= 0.65 => {
                            if let Some(code) = axis_to_code(axis) {
                                captured_input = Some(if value < 0.0 {
                                    GamepadInput::AxisNegative(code)
                                } else {
                                    GamepadInput::AxisPositive(code)
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if let (Some(PendingInputBind::Gamepad(target)), Some(inp)) =
            (self.awaiting_input_bind, captured_input)
        {
            self.config.keybindings.gamepad.set(target, Some(inp));
            self.awaiting_input_bind = None;
            self.autofire_state.clear_latches();
            self.config.save();
            self.notify(&format!("Bound {} to {}", target.label(), inp.label()));
            ctx.request_repaint();
        }
    }

    // WAVE-2 HOOK: Input polling entrypoint — Lua scripts (`joypad.get` / `joypad.set`)
    // and Movie playback/recording can inspect or override the active-low `u16` keyinput here.
    pub(crate) fn poll_live_inputs(&mut self, ctx: &egui::Context) -> u16 {
        let mut mask = 0u16;
        let mut raw_turbo_a = false;
        let mut raw_turbo_b = false;

        let allow_kbd = !ctx.wants_keyboard_input()
            && self.debugger.awaiting_rebind.is_none()
            && self.awaiting_input_bind.is_none();

        if allow_kbd {
            ctx.input(|i| {
                for btn in GbaButton::ALL {
                    if let Some(k) = self.config.keybindings.egui_key(btn)
                        && i.key_down(k)
                    {
                        mask |= btn.mask();
                    }
                }
                if let Some(k) = self.config.keybindings.target_egui_key(ControlTarget::TurboA)
                    && i.key_down(k)
                {
                    raw_turbo_a = true;
                }
                if let Some(k) = self.config.keybindings.target_egui_key(ControlTarget::TurboB)
                    && i.key_down(k)
                {
                    raw_turbo_b = true;
                }
            });
        }

        if self.awaiting_input_bind.is_none()
            && let Some(gilrs) = &self.gilrs
        {
            let gp_cfg = &self.config.keybindings.gamepad;
            for (_id, gamepad) in gilrs.gamepads() {
                if !gamepad.is_connected() {
                    continue;
                }
                for btn in GbaButton::ALL {
                    if let Some(binding) = gp_cfg.get(ControlTarget::Button(btn))
                        && is_gamepad_input_active(&gamepad, binding)
                    {
                        mask |= btn.mask();
                    }
                }
                if let Some(binding) = gp_cfg.get(ControlTarget::TurboA)
                    && is_gamepad_input_active(&gamepad, binding)
                {
                    raw_turbo_a = true;
                }
                if let Some(binding) = gp_cfg.get(ControlTarget::TurboB)
                    && is_gamepad_input_active(&gamepad, binding)
                {
                    raw_turbo_b = true;
                }

                if gp_cfg.left_stick_as_dpad {
                    let lx = gamepad
                        .axis_data(Axis::LeftStickX)
                        .map(|d| d.value())
                        .unwrap_or(0.0);
                    let ly = gamepad
                        .axis_data(Axis::LeftStickY)
                        .map(|d| d.value())
                        .unwrap_or(0.0);
                    if lx < -0.4 {
                        mask |= GbaButton::Left.mask();
                    } else if lx > 0.4 {
                        mask |= GbaButton::Right.mask();
                    }
                    if ly > 0.4 {
                        mask |= GbaButton::Up.mask();
                    } else if ly < -0.4 {
                        mask |= GbaButton::Down.mask();
                    }
                }
            }
        }

        self.base_pressed_mask = mask;
        let (active_a, active_b) = self.autofire_state.update_latches(
            raw_turbo_a,
            raw_turbo_b,
            self.config.keybindings.autofire_mode,
        );
        self.active_turbo_buttons = (active_a, active_b);

        let effective_mask = mask
            | autofire_pulse_mask(
                active_a,
                active_b,
                self.frame_counter,
                self.config.keybindings.autofire_interval_frames,
            );
        self.last_pressed_mask = effective_mask;
        encode_keyinput(effective_mask)
    }

    pub(crate) fn poll_inputs(&mut self, ctx: &egui::Context) -> u16 {
        let live = self.poll_live_inputs(ctx);
        if let Some(movie_key) = self.movie.peek_playback_keyinput() {
            self.last_pressed_mask = (!movie_key) & 0x03FF;
            movie_key
        } else {
            live
        }
    }

    pub(crate) fn effective_live_keypad_for_frame(&mut self) -> u16 {
        let effective_mask = self.base_pressed_mask
            | autofire_pulse_mask(
                self.active_turbo_buttons.0,
                self.active_turbo_buttons.1,
                self.frame_counter,
                self.config.keybindings.autofire_interval_frames,
            );
        self.last_pressed_mask = effective_mask;
        encode_keyinput(effective_mask)
    }

    pub(crate) fn connected_gamepad_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Some(gilrs) = &self.gilrs {
            for (_id, gp) in gilrs.gamepads() {
                if gp.is_connected() {
                    names.push(gp.name().to_string());
                }
            }
        }
        names
    }

    fn process_pending_keyboard_capture(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.awaiting_input_bind else {
            return;
        };
        if matches!(pending, PendingInputBind::Gamepad(_)) {
            ctx.request_repaint();
            return;
        }

        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };

            match pending {
                PendingInputBind::Keyboard(target) => {
                    if key == Key::Escape {
                        self.awaiting_input_bind = None;
                        return;
                    }
                    let key_name = format_key_name(key);
                    self.config
                        .keybindings
                        .set_target_key_name(target, Some(key_name.clone()));
                    self.awaiting_input_bind = None;
                    self.autofire_state.clear_latches();
                    self.config.save();
                    self.notify(&format!("Bound {} to {key_name}", target.label()));
                    return;
                }
                PendingInputBind::Hotkey(action) => {
                    if key == Key::Escape
                        && !modifiers.ctrl
                        && !modifiers.mac_cmd
                        && !modifiers.shift
                        && !modifiers.alt
                    {
                        self.awaiting_input_bind = None;
                        return;
                    }
                    let chord = HotkeyChord {
                        key: format_key_name(key),
                        ctrl: modifiers.ctrl || modifiers.mac_cmd,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                    };
                    let desc = chord.display();
                    self.config.keybindings.hotkeys.set(action, Some(chord));
                    self.awaiting_input_bind = None;
                    self.config.save();
                    self.notify(&format!("Bound hotkey '{}' to {desc}", action.label()));
                    return;
                }
                PendingInputBind::Gamepad(_) => {}
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn draw_controls_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_controls_dialog {
            if self.awaiting_input_bind.is_some() {
                self.awaiting_input_bind = None;
            }
            return;
        }

        self.process_pending_keyboard_capture(ctx);

        let mut open = self.show_controls_dialog;
        let kbd_conflicts = detect_keyboard_conflicts(&self.config.keybindings);
        let gp_conflicts = detect_gamepad_conflicts(&self.config.keybindings.gamepad);
        let hk_conflicts = detect_hotkey_conflicts(&self.config.keybindings.hotkeys);
        let pads = self.connected_gamepad_names();

        egui::Window::new("Controls & Keybindings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(660.0)
            .default_height(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let tab_btn = |ui: &mut egui::Ui, current: &mut ControlsTab, tab: ControlsTab, lbl: &str| {
                        let active = *current == tab;
                        let mut btn = egui::Button::new(RichText::new(lbl).small());
                        if active {
                            btn = btn
                                .fill(ACCENT_TEAL_BG)
                                .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
                        }
                        if ui.add(btn).clicked() {
                            *current = tab;
                        }
                    };
                    tab_btn(
                        ui,
                        &mut self.controls_tab,
                        ControlsTab::Keyboard,
                        &conflict_tab_label("Keyboard", kbd_conflicts.len()),
                    );
                    tab_btn(
                        ui,
                        &mut self.controls_tab,
                        ControlsTab::Gamepad,
                        &conflict_tab_label("Gamepad", gp_conflicts.len()),
                    );
                    tab_btn(
                        ui,
                        &mut self.controls_tab,
                        ControlsTab::Autofire,
                        "Autofire / Turbo",
                    );
                    tab_btn(
                        ui,
                        &mut self.controls_tab,
                        ControlsTab::Hotkeys,
                        &conflict_tab_label("Hotkeys", hk_conflicts.len()),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("Reset All Defaults").small()).clicked() {
                            self.config.keybindings = KeyBindings::default();
                            self.awaiting_input_bind = None;
                            self.autofire_state.clear_latches();
                            self.config.save();
                            self.notify("All controls, autofire, and hotkeys reset to defaults");
                        }
                        if ui.button(RichText::new("Reset Tab").small()).clicked() {
                            self.reset_active_controls_tab();
                        }
                    });
                });

                ui.add_space(6.0);

                // Listening / Error / Conflict notice strip
                if let Some(pending) = self.awaiting_input_bind {
                    let hint = match pending {
                        PendingInputBind::Gamepad(_) => {
                            "Press any gamepad button or push an analog stick / trigger (click Cancel to abort)"
                        }
                        PendingInputBind::Hotkey(_) => {
                            "Press desired modifier + key combination (Esc to cancel)"
                        }
                        PendingInputBind::Keyboard(_) => "Press any keyboard key (Esc to cancel)",
                    };
                    egui::Frame::new()
                        .fill(ACCENT_TEAL_BG)
                        .stroke(Stroke::new(1.0_f32, ACCENT_TEAL))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("Listening for {}: ", pending.label()))
                                        .strong()
                                        .color(ACCENT_TEAL),
                                );
                                ui.label(RichText::new(hint).small().color(Color32::WHITE));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Cancel").clicked() {
                                            self.awaiting_input_bind = None;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                } else {
                    let active_conflicts: &[BindingConflict] = match self.controls_tab {
                        ControlsTab::Keyboard => &kbd_conflicts,
                        ControlsTab::Gamepad => &gp_conflicts,
                        ControlsTab::Hotkeys => &hk_conflicts,
                        ControlsTab::Autofire => &[],
                    };
                    if !active_conflicts.is_empty() {
                        draw_conflict_banner(ui, active_conflicts);
                        ui.add_space(6.0);
                    }
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.controls_tab {
                        ControlsTab::Keyboard => {
                            self.draw_keyboard_tab(ui, &kbd_conflicts);
                        }
                        ControlsTab::Gamepad => {
                            self.draw_gamepad_tab(ui, &pads, &gp_conflicts);
                        }
                        ControlsTab::Autofire => {
                            self.draw_autofire_tab(ui);
                        }
                        ControlsTab::Hotkeys => {
                            self.draw_hotkeys_tab(ui, &hk_conflicts);
                        }
                    });
            });

        self.show_controls_dialog = open;
    }

    fn reset_active_controls_tab(&mut self) {
        let def = KeyBindings::default();
        self.awaiting_input_bind = None;
        match self.controls_tab {
            ControlsTab::Keyboard => {
                for target in ControlTarget::ALL {
                    self.config
                        .keybindings
                        .set_target_key_name(
                            target,
                            def.target_key_name(target).map(str::to_string),
                        );
                }
                self.notify("Keyboard controls reset to defaults");
            }
            ControlsTab::Gamepad => {
                self.config.keybindings.gamepad = def.gamepad;
                self.notify("Gamepad bindings reset to defaults");
            }
            ControlsTab::Autofire => {
                self.config.keybindings.autofire_mode = def.autofire_mode;
                self.config.keybindings.autofire_interval_frames = def.autofire_interval_frames;
                self.config.keybindings.turbo_a = def.turbo_a;
                self.config.keybindings.turbo_b = def.turbo_b;
                self.autofire_state.clear_latches();
                self.notify("Autofire settings reset to defaults");
            }
            ControlsTab::Hotkeys => {
                self.config.keybindings.hotkeys = def.hotkeys;
                self.notify("Hotkeys reset to defaults");
            }
        }
        self.config.save();
    }

    fn draw_keyboard_tab(&mut self, ui: &mut egui::Ui, conflicts: &[BindingConflict]) {
        ui.label(
            RichText::new("Assign keyboard keys for GBA buttons and Turbo autofire triggers.")
                .small()
                .color(TEXT_MUTED),
        );
        ui.add_space(6.0);

        let all_unbound = ControlTarget::ALL
            .iter()
            .all(|&t| self.config.keybindings.target_key_name(t).is_none());
        if all_unbound {
            draw_empty_state_card(
                ui,
                "All keyboard bindings are currently cleared.",
                "Click Bind on any control below or use Reset Tab to restore defaults.",
            );
            ui.add_space(6.0);
        }

        egui::Grid::new("keyboard_bindings_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Control").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("Key").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("Rebind").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("").small());
                ui.end_row();

                for target in ControlTarget::ALL {
                    let is_capturing =
                        self.awaiting_input_bind == Some(PendingInputBind::Keyboard(target));
                    let raw_name = self.config.keybindings.target_key_name(target);
                    let valid_key = raw_name.and_then(parse_key_name).is_some();
                    let conflicted = conflicts
                        .iter()
                        .any(|c| c.target_a == target.label() || c.target_b == target.label());

                    ui.label(RichText::new(target.label()).color(Color32::WHITE));

                    let pill_text = if is_capturing {
                        "Press key...".to_string()
                    } else if let Some(name) = raw_name {
                        if valid_key {
                            name.to_string()
                        } else {
                            format!("{name} (Unknown)")
                        }
                    } else {
                        "Unbound".to_string()
                    };
                    draw_binding_pill(ui, &pill_text, is_capturing, conflicted, raw_name.is_none());

                    let bind_lbl = if is_capturing { "Listening..." } else { "Bind" };
                    if ui
                        .add_sized([92.0, 22.0], egui::Button::new(RichText::new(bind_lbl).small()))
                        .clicked()
                    {
                        self.awaiting_input_bind = if is_capturing {
                            None
                        } else {
                            Some(PendingInputBind::Keyboard(target))
                        };
                    }

                    if ui
                        .add_enabled(
                            raw_name.is_some(),
                            egui::Button::new(RichText::new("Clear").small()),
                        )
                        .clicked()
                    {
                        self.config.keybindings.set_target_key_name(target, None);
                        if is_capturing {
                            self.awaiting_input_bind = None;
                        }
                        self.config.save();
                    }
                    ui.end_row();
                }
            });
    }

    fn draw_gamepad_tab(
        &mut self,
        ui: &mut egui::Ui,
        pads: &[String],
        conflicts: &[BindingConflict],
    ) {
        egui::Frame::new()
            .fill(BG_CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                if pads.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(TEXT_MUTED).small());
                        ui.label(
                            RichText::new(
                                "No gamepad detected — connect a USB/Bluetooth controller to capture buttons.",
                            )
                            .small()
                            .color(TEXT_MUTED),
                        );
                    });
                } else {
                    for name in pads {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("●").color(ACCENT_TEAL).small());
                            ui.label(
                                RichText::new(format!("Connected: {name}"))
                                    .small()
                                    .color(Color32::WHITE),
                            );
                        });
                    }
                }
            });

        ui.add_space(6.0);
        if ui
            .checkbox(
                &mut self.config.keybindings.gamepad.left_stick_as_dpad,
                "Mirror Left Analog Stick onto GBA D-Pad",
            )
            .changed()
        {
            self.config.save();
        }
        ui.add_space(6.0);

        let all_unbound = ControlTarget::ALL
            .iter()
            .all(|&t| self.config.keybindings.gamepad.get(t).is_none());
        if all_unbound {
            draw_empty_state_card(
                ui,
                "No gamepad inputs are mapped.",
                "Click Bind beside any button to listen for a gamepad press, or click Reset Tab.",
            );
            ui.add_space(6.0);
        }

        egui::Grid::new("gamepad_bindings_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Control").strong().small().color(TEXT_MUTED));
                ui.label(
                    RichText::new("Gamepad Input")
                        .strong()
                        .small()
                        .color(TEXT_MUTED),
                );
                ui.label(RichText::new("Rebind").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("").small());
                ui.end_row();

                for target in ControlTarget::ALL {
                    let is_capturing =
                        self.awaiting_input_bind == Some(PendingInputBind::Gamepad(target));
                    let current = self.config.keybindings.gamepad.get(target);
                    let conflicted = conflicts
                        .iter()
                        .any(|c| c.target_a == target.label() || c.target_b == target.label());

                    ui.label(RichText::new(target.label()).color(Color32::WHITE));

                    let pill_text = if is_capturing {
                        "Press button / stick...".to_string()
                    } else if let Some(inp) = current {
                        inp.label()
                    } else {
                        "Unbound".to_string()
                    };
                    draw_binding_pill(ui, &pill_text, is_capturing, conflicted, current.is_none());

                    let bind_lbl = if is_capturing { "Listening..." } else { "Bind" };
                    if ui
                        .add_sized([92.0, 22.0], egui::Button::new(RichText::new(bind_lbl).small()))
                        .clicked()
                    {
                        self.awaiting_input_bind = if is_capturing {
                            None
                        } else {
                            Some(PendingInputBind::Gamepad(target))
                        };
                    }

                    if ui
                        .add_enabled(
                            current.is_some(),
                            egui::Button::new(RichText::new("Clear").small()),
                        )
                        .clicked()
                    {
                        self.config.keybindings.gamepad.set(target, None);
                        if is_capturing {
                            self.awaiting_input_bind = None;
                        }
                        self.config.save();
                    }
                    ui.end_row();
                }
            });
    }

    fn draw_autofire_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Configure Turbo A and Turbo B pulse cadence and activation behavior.")
                .small()
                .color(TEXT_MUTED),
        );
        ui.add_space(8.0);

        egui::Frame::new()
            .fill(BG_CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new("Trigger Mode").strong().color(Color32::WHITE));
                ui.add_space(4.0);
                for mode in [AutofireMode::Hold, AutofireMode::Toggle] {
                    if ui
                        .radio(self.config.keybindings.autofire_mode == mode, mode.label())
                        .clicked()
                    {
                        self.config.keybindings.autofire_mode = mode;
                        self.autofire_state.clear_latches();
                        self.config.save();
                    }
                }

                ui.add_space(10.0);
                ui.label(RichText::new("Pulse Cadence").strong().color(Color32::WHITE));
                ui.add_space(4.0);
                let mut interval = self.config.keybindings.autofire_interval_frames.clamp(2, 16);
                let hz = 59.7275 / (f32::from(interval));
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut interval, 2..=16)
                                .text("frames per cycle")
                                .integer(),
                        )
                        .changed()
                    {
                        self.config.keybindings.autofire_interval_frames = interval;
                        self.config.save();
                    }
                    ui.label(
                        RichText::new(format!("({hz:.1} presses/sec)"))
                            .small()
                            .color(ACCENT_TEAL),
                    );
                });
            });

        ui.add_space(8.0);

        let kbd_a = self.config.keybindings.target_key_name(ControlTarget::TurboA);
        let kbd_b = self.config.keybindings.target_key_name(ControlTarget::TurboB);
        let gp_a = self.config.keybindings.gamepad.get(ControlTarget::TurboA);
        let gp_b = self.config.keybindings.gamepad.get(ControlTarget::TurboB);

        if kbd_a.is_none() && kbd_b.is_none() && gp_a.is_none() && gp_b.is_none() {
            draw_empty_state_card(
                ui,
                "No Turbo A or Turbo B bindings are configured.",
                "Bind Turbo A / Turbo B on the Keyboard or Gamepad tab to trigger autofire.",
            );
            ui.add_space(8.0);
        }

        egui::Frame::new()
            .fill(BG_CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Active Bindings & Toggle Status")
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(4.0);
                egui::Grid::new("autofire_summary_grid")
                    .num_columns(3)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Target").small().color(TEXT_MUTED));
                        ui.label(RichText::new("Keyboard").small().color(TEXT_MUTED));
                        ui.label(RichText::new("Gamepad").small().color(TEXT_MUTED));
                        ui.end_row();

                        ui.label("Turbo A");
                        ui.label(kbd_a.unwrap_or("Unbound"));
                        ui.label(
                            gp_a.map(GamepadInput::label)
                                .unwrap_or_else(|| "Unbound".to_string()),
                        );
                        ui.end_row();

                        ui.label("Turbo B");
                        ui.label(kbd_b.unwrap_or("Unbound"));
                        ui.label(
                            gp_b.map(GamepadInput::label)
                                .unwrap_or_else(|| "Unbound".to_string()),
                        );
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.small_button("Edit Keyboard Turbo").clicked() {
                        self.controls_tab = ControlsTab::Keyboard;
                    }
                    if ui.small_button("Edit Gamepad Turbo").clicked() {
                        self.controls_tab = ControlsTab::Gamepad;
                    }
                    if self.config.keybindings.autofire_mode == AutofireMode::Toggle {
                        let a_lbl = if self.autofire_state.turbo_a_latched {
                            "A: ON"
                        } else {
                            "A: OFF"
                        };
                        let b_lbl = if self.autofire_state.turbo_b_latched {
                            "B: ON"
                        } else {
                            "B: OFF"
                        };
                        ui.label(RichText::new(format!("Latched [{a_lbl} | {b_lbl}]")).small());
                        if ui.small_button("Clear Latches").clicked() {
                            self.autofire_state.clear_latches();
                        }
                    }
                });
            });
    }

    fn draw_hotkeys_tab(&mut self, ui: &mut egui::Ui, conflicts: &[BindingConflict]) {
        ui.label(
            RichText::new(
                "Customize emulator hotkeys. Supports Ctrl, Shift, and Alt key combinations.",
            )
            .small()
            .color(TEXT_MUTED),
        );
        ui.add_space(6.0);

        let all_unbound = HotkeyAction::ALL
            .iter()
            .all(|&a| self.config.keybindings.hotkeys.get(a).is_none());
        if all_unbound {
            draw_empty_state_card(
                ui,
                "All primary hotkeys are unbound.",
                "Click Bind next to an action or Reset Tab to restore defaults.",
            );
            ui.add_space(6.0);
        }

        egui::Grid::new("hotkey_bindings_grid")
            .num_columns(4)
            .spacing([10.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("Action").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("Shortcut").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("Rebind").strong().small().color(TEXT_MUTED));
                ui.label(RichText::new("").small());
                ui.end_row();

                for action in HotkeyAction::ALL {
                    let is_capturing =
                        self.awaiting_input_bind == Some(PendingInputBind::Hotkey(action));
                    let current = self.config.keybindings.hotkeys.get(action).cloned();
                    let conflicted = conflicts
                        .iter()
                        .any(|c| c.target_a == action.label() || c.target_b == action.label());

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(action.label()).color(Color32::WHITE));
                        if action.is_hold_action() {
                            ui.label(RichText::new("[Hold]").small().color(TEXT_MUTED));
                        }
                    });

                    let pill_text = if is_capturing {
                        "Press shortcut...".to_string()
                    } else if let Some(chord) = &current {
                        if chord.egui_key().is_some() {
                            chord.display()
                        } else {
                            format!("{} (Unknown)", chord.display())
                        }
                    } else {
                        "Unbound".to_string()
                    };
                    draw_binding_pill(ui, &pill_text, is_capturing, conflicted, current.is_none());

                    let bind_lbl = if is_capturing { "Listening..." } else { "Bind" };
                    if ui
                        .add_sized([92.0, 22.0], egui::Button::new(RichText::new(bind_lbl).small()))
                        .clicked()
                    {
                        self.awaiting_input_bind = if is_capturing {
                            None
                        } else {
                            Some(PendingInputBind::Hotkey(action))
                        };
                    }

                    if ui
                        .add_enabled(
                            current.is_some(),
                            egui::Button::new(RichText::new("Clear").small()),
                        )
                        .clicked()
                    {
                        self.config.keybindings.hotkeys.set(action, None);
                        if is_capturing {
                            self.awaiting_input_bind = None;
                        }
                        self.config.save();
                    }
                    ui.end_row();
                }
            });
    }
}

fn conflict_tab_label(name: &str, conflict_count: usize) -> String {
    if conflict_count == 0 {
        name.to_string()
    } else {
        format!("{name} ({conflict_count} conflict)")
    }
}

fn draw_binding_pill(
    ui: &mut egui::Ui,
    text: &str,
    capturing: bool,
    conflicted: bool,
    unbound: bool,
) {
    let (fill, stroke, txt_color) = if capturing {
        (ACCENT_TEAL_BG, ACCENT_TEAL, ACCENT_TEAL)
    } else if conflicted {
        (
            Color32::from_rgb(54, 41, 20),
            Color32::from_rgb(220, 155, 65),
            Color32::from_rgb(248, 198, 110),
        )
    } else if unbound {
        (BG_CARD, BORDER_HAIRLINE, TEXT_MUTED)
    } else {
        (BG_CARD, BORDER_HAIRLINE, Color32::WHITE)
    };

    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, stroke))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.set_min_width(150.0);
            ui.label(RichText::new(text).monospace().small().color(txt_color));
        });
}

fn draw_conflict_banner(ui: &mut egui::Ui, conflicts: &[BindingConflict]) {
    egui::Frame::new()
        .fill(Color32::from_rgb(48, 36, 18))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(185, 130, 52)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "Binding conflict detected ({} overlap{}):",
                    conflicts.len(),
                    if conflicts.len() == 1 { "" } else { "s" }
                ))
                .strong()
                .small()
                .color(Color32::from_rgb(245, 190, 95)),
            );
            for c in conflicts.iter().take(3) {
                ui.label(
                    RichText::new(format!(
                        "• '{}' shares {} with '{}'",
                        c.target_a, c.binding_desc, c.target_b
                    ))
                    .small()
                    .color(Color32::from_rgb(235, 215, 175)),
                );
            }
            if conflicts.len() > 3 {
                ui.label(
                    RichText::new(format!("• ...and {} more", conflicts.len() - 3))
                        .small()
                        .color(TEXT_MUTED),
                );
            }
        });
}

fn draw_empty_state_card(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong().small().color(Color32::WHITE));
            ui.label(RichText::new(subtitle).small().color(TEXT_MUTED));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conflict_detection_across_keyboard_gamepad_and_hotkeys() {
        let mut kb = KeyBindings::default();
        assert!(detect_keyboard_conflicts(&kb).is_empty());
        assert!(detect_gamepad_conflicts(&kb.gamepad).is_empty());
        assert!(detect_hotkey_conflicts(&kb.hotkeys).is_empty());

        // Keyboard duplicate between A and B
        kb.set_target_key_name(ControlTarget::Button(GbaButton::B), Some("Z".to_string()));
        let kbd_conflicts = detect_keyboard_conflicts(&kb);
        assert!(kbd_conflicts.iter().any(|c| c.binding_desc == "Z"));

        // Bare hotkey colliding with keyboard control
        kb.set_target_key_name(
            ControlTarget::Button(GbaButton::Select),
            Some("R".to_string()),
        );
        let kbd_conflicts = detect_keyboard_conflicts(&kb);
        assert!(
            kbd_conflicts
                .iter()
                .any(|c| c.binding_desc == "R" && c.target_b.contains("Rewind"))
        );

        // Gamepad duplicate
        kb.gamepad
            .set(ControlTarget::TurboA, kb.gamepad.get(ControlTarget::Button(GbaButton::A)));
        let gp_conflicts = detect_gamepad_conflicts(&kb.gamepad);
        assert_eq!(gp_conflicts.len(), 1);

        // Hotkey chord duplicate
        kb.hotkeys
            .set(HotkeyAction::Screenshot, kb.hotkeys.get(HotkeyAction::Pause).cloned());
        let hk_conflicts = detect_hotkey_conflicts(&kb.hotkeys);
        assert_eq!(hk_conflicts.len(), 1);
    }

    #[test]
    fn test_autofire_hold_toggle_and_pulse_cadence() {
        let mut state = AutofireState::default();

        // Hold mode tracks raw press directly
        assert_eq!(state.update_latches(true, false, AutofireMode::Hold), (true, false));
        assert_eq!(state.update_latches(false, false, AutofireMode::Hold), (false, false));

        // Toggle mode flips cleanly on rising edges only
        assert_eq!(state.update_latches(true, false, AutofireMode::Toggle), (true, false));
        assert_eq!(state.update_latches(true, false, AutofireMode::Toggle), (true, false));
        assert_eq!(state.update_latches(false, false, AutofireMode::Toggle), (true, false));
        assert_eq!(state.update_latches(true, true, AutofireMode::Toggle), (false, true));

        // 4-frame cadence -> 2 frames high, 2 frames low
        assert_eq!(autofire_pulse_mask(true, false, 0, 4), gba_core::KEY_A);
        assert_eq!(autofire_pulse_mask(true, false, 1, 4), gba_core::KEY_A);
        assert_eq!(autofire_pulse_mask(true, false, 2, 4), 0);
        assert_eq!(autofire_pulse_mask(true, true, 3, 4), 0);
        assert_eq!(
            autofire_pulse_mask(true, true, 4, 4),
            gba_core::KEY_A | gba_core::KEY_B
        );
    }
}
