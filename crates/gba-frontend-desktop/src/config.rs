use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use egui::Key;
use gba_core::cartridge::SaveType;
use serde::{Deserialize, Serialize};

use crate::cheats::CheatEntry;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentRomEntry {
    pub path: String,
    pub title: String,
    pub code: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GbaButton {
    A,
    B,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
    R,
    L,
}

impl GbaButton {
    pub const ALL: [GbaButton; 10] = [
        GbaButton::Up,
        GbaButton::Down,
        GbaButton::Left,
        GbaButton::Right,
        GbaButton::A,
        GbaButton::B,
        GbaButton::L,
        GbaButton::R,
        GbaButton::Start,
        GbaButton::Select,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::A => "A Button",
            Self::B => "B Button",
            Self::Select => "Select",
            Self::Start => "Start",
            Self::Right => "D-Pad Right",
            Self::Left => "D-Pad Left",
            Self::Up => "D-Pad Up",
            Self::Down => "D-Pad Down",
            Self::R => "R Shoulder",
            Self::L => "L Shoulder",
        }
    }

    pub const fn mask(self) -> u16 {
        match self {
            Self::A => gba_core::KEY_A,
            Self::B => gba_core::KEY_B,
            Self::Select => gba_core::KEY_SELECT,
            Self::Start => gba_core::KEY_START,
            Self::Right => gba_core::KEY_RIGHT,
            Self::Left => gba_core::KEY_LEFT,
            Self::Up => gba_core::KEY_UP,
            Self::Down => gba_core::KEY_DOWN,
            Self::R => gba_core::KEY_R,
            Self::L => gba_core::KEY_L,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlTarget {
    Button(GbaButton),
    TurboA,
    TurboB,
}

impl ControlTarget {
    pub const ALL: [ControlTarget; 12] = [
        ControlTarget::Button(GbaButton::Up),
        ControlTarget::Button(GbaButton::Down),
        ControlTarget::Button(GbaButton::Left),
        ControlTarget::Button(GbaButton::Right),
        ControlTarget::Button(GbaButton::A),
        ControlTarget::Button(GbaButton::B),
        ControlTarget::Button(GbaButton::L),
        ControlTarget::Button(GbaButton::R),
        ControlTarget::Button(GbaButton::Start),
        ControlTarget::Button(GbaButton::Select),
        ControlTarget::TurboA,
        ControlTarget::TurboB,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Button(btn) => btn.label(),
            Self::TurboA => "Turbo A (Autofire)",
            Self::TurboB => "Turbo B (Autofire)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadButtonCode {
    South,
    East,
    North,
    West,
    C,
    Z,
    LeftTrigger,
    LeftTrigger2,
    RightTrigger,
    RightTrigger2,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

impl GamepadButtonCode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::South => "Face South (A/Cross)",
            Self::East => "Face East (B/Circle)",
            Self::North => "Face North (Y/Triangle)",
            Self::West => "Face West (X/Square)",
            Self::C => "Button C",
            Self::Z => "Button Z",
            Self::LeftTrigger => "LB / L1",
            Self::LeftTrigger2 => "LT / L2",
            Self::RightTrigger => "RB / R1",
            Self::RightTrigger2 => "RT / R2",
            Self::Select => "Back / Select",
            Self::Start => "Start / Options",
            Self::Mode => "Guide / Home",
            Self::LeftThumb => "L3 (Left Stick)",
            Self::RightThumb => "R3 (Right Stick)",
            Self::DPadUp => "D-Pad Up",
            Self::DPadDown => "D-Pad Down",
            Self::DPadLeft => "D-Pad Left",
            Self::DPadRight => "D-Pad Right",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadAxisCode {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    DPadX,
    DPadY,
    LeftZ,
    RightZ,
}

impl GamepadAxisCode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::LeftStickX => "Left Stick X",
            Self::LeftStickY => "Left Stick Y",
            Self::RightStickX => "Right Stick X",
            Self::RightStickY => "Right Stick Y",
            Self::DPadX => "D-Pad Axis X",
            Self::DPadY => "D-Pad Axis Y",
            Self::LeftZ => "Left Trigger Axis",
            Self::RightZ => "Right Trigger Axis",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadInput {
    Button(GamepadButtonCode),
    AxisNegative(GamepadAxisCode),
    AxisPositive(GamepadAxisCode),
}

impl GamepadInput {
    pub fn label(self) -> String {
        match self {
            Self::Button(b) => b.label().to_string(),
            Self::AxisNegative(a) => format!("{} (-)", a.label()),
            Self::AxisPositive(a) => format!("{} (+)", a.label()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GamepadBindings {
    pub a: Option<GamepadInput>,
    pub b: Option<GamepadInput>,
    pub select: Option<GamepadInput>,
    pub start: Option<GamepadInput>,
    pub right: Option<GamepadInput>,
    pub left: Option<GamepadInput>,
    pub up: Option<GamepadInput>,
    pub down: Option<GamepadInput>,
    pub r: Option<GamepadInput>,
    pub l: Option<GamepadInput>,
    pub turbo_a: Option<GamepadInput>,
    pub turbo_b: Option<GamepadInput>,
    pub left_stick_as_dpad: bool,
}

impl Default for GamepadBindings {
    fn default() -> Self {
        use GamepadButtonCode as B;
        Self {
            a: Some(GamepadInput::Button(B::East)),
            b: Some(GamepadInput::Button(B::South)),
            select: Some(GamepadInput::Button(B::Select)),
            start: Some(GamepadInput::Button(B::Start)),
            right: Some(GamepadInput::Button(B::DPadRight)),
            left: Some(GamepadInput::Button(B::DPadLeft)),
            up: Some(GamepadInput::Button(B::DPadUp)),
            down: Some(GamepadInput::Button(B::DPadDown)),
            r: Some(GamepadInput::Button(B::RightTrigger)),
            l: Some(GamepadInput::Button(B::LeftTrigger)),
            turbo_a: Some(GamepadInput::Button(B::North)),
            turbo_b: Some(GamepadInput::Button(B::West)),
            left_stick_as_dpad: true,
        }
    }
}

impl GamepadBindings {
    pub fn get(&self, target: ControlTarget) -> Option<GamepadInput> {
        match target {
            ControlTarget::Button(GbaButton::A) => self.a,
            ControlTarget::Button(GbaButton::B) => self.b,
            ControlTarget::Button(GbaButton::Select) => self.select,
            ControlTarget::Button(GbaButton::Start) => self.start,
            ControlTarget::Button(GbaButton::Right) => self.right,
            ControlTarget::Button(GbaButton::Left) => self.left,
            ControlTarget::Button(GbaButton::Up) => self.up,
            ControlTarget::Button(GbaButton::Down) => self.down,
            ControlTarget::Button(GbaButton::R) => self.r,
            ControlTarget::Button(GbaButton::L) => self.l,
            ControlTarget::TurboA => self.turbo_a,
            ControlTarget::TurboB => self.turbo_b,
        }
    }

    pub fn set(&mut self, target: ControlTarget, input: Option<GamepadInput>) {
        let slot = match target {
            ControlTarget::Button(GbaButton::A) => &mut self.a,
            ControlTarget::Button(GbaButton::B) => &mut self.b,
            ControlTarget::Button(GbaButton::Select) => &mut self.select,
            ControlTarget::Button(GbaButton::Start) => &mut self.start,
            ControlTarget::Button(GbaButton::Right) => &mut self.right,
            ControlTarget::Button(GbaButton::Left) => &mut self.left,
            ControlTarget::Button(GbaButton::Up) => &mut self.up,
            ControlTarget::Button(GbaButton::Down) => &mut self.down,
            ControlTarget::Button(GbaButton::R) => &mut self.r,
            ControlTarget::Button(GbaButton::L) => &mut self.l,
            ControlTarget::TurboA => &mut self.turbo_a,
            ControlTarget::TurboB => &mut self.turbo_b,
        };
        *slot = input;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutofireMode {
    #[default]
    Hold,
    Toggle,
}

impl AutofireMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hold => "Hold (while pressed)",
            Self::Toggle => "Toggle (press to latch)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyAction {
    SaveState,
    LoadState,
    Rewind,
    FastForward,
    Pause,
    Screenshot,
    FrameAdvance,
    HardReset,
    Mute,
    Fullscreen,
    StateManager,
}

impl HotkeyAction {
    pub const ALL: [HotkeyAction; 11] = [
        Self::Pause,
        Self::FastForward,
        Self::Rewind,
        Self::FrameAdvance,
        Self::SaveState,
        Self::LoadState,
        Self::StateManager,
        Self::Screenshot,
        Self::HardReset,
        Self::Mute,
        Self::Fullscreen,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::SaveState => "Quick Save (Slot 1)",
            Self::LoadState => "Quick Load (Slot 1)",
            Self::Rewind => "Rewind (Hold)",
            Self::FastForward => "Fast-Forward (Hold)",
            Self::Pause => "Pause / Resume",
            Self::Screenshot => "Take Screenshot",
            Self::FrameAdvance => "Frame Advance",
            Self::HardReset => "Hard Reset",
            Self::Mute => "Mute / Unmute Audio",
            Self::Fullscreen => "Toggle Fullscreen",
            Self::StateManager => "Save State Manager",
        }
    }

    pub const fn is_hold_action(self) -> bool {
        matches!(self, Self::Rewind | Self::FastForward)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HotkeyChord {
    pub key: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
}

impl HotkeyChord {
    pub fn simple(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    pub fn with_ctrl(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    pub fn with_shift(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ctrl: false,
            shift: true,
            alt: false,
        }
    }

    pub fn egui_key(&self) -> Option<Key> {
        parse_key_name(&self.key)
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyBindings {
    pub save_state: Option<HotkeyChord>,
    pub load_state: Option<HotkeyChord>,
    pub rewind: Option<HotkeyChord>,
    pub fast_forward: Option<HotkeyChord>,
    pub pause: Option<HotkeyChord>,
    pub screenshot: Option<HotkeyChord>,
    pub frame_advance: Option<HotkeyChord>,
    pub hard_reset: Option<HotkeyChord>,
    pub mute: Option<HotkeyChord>,
    pub fullscreen: Option<HotkeyChord>,
    pub state_manager: Option<HotkeyChord>,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            save_state: Some(HotkeyChord::simple("F1")),
            load_state: Some(HotkeyChord::with_shift("F1")),
            rewind: Some(HotkeyChord::simple("R")),
            fast_forward: Some(HotkeyChord::simple("Tab")),
            pause: Some(HotkeyChord::simple("Space")),
            screenshot: Some(HotkeyChord::simple("F12")),
            frame_advance: Some(HotkeyChord::with_ctrl("N")),
            hard_reset: Some(HotkeyChord::with_ctrl("R")),
            mute: Some(HotkeyChord::with_ctrl("M")),
            fullscreen: Some(HotkeyChord::simple("F11")),
            state_manager: Some(HotkeyChord::simple("F6")),
        }
    }
}

impl HotkeyBindings {
    pub fn get(&self, action: HotkeyAction) -> Option<&HotkeyChord> {
        match action {
            HotkeyAction::SaveState => self.save_state.as_ref(),
            HotkeyAction::LoadState => self.load_state.as_ref(),
            HotkeyAction::Rewind => self.rewind.as_ref(),
            HotkeyAction::FastForward => self.fast_forward.as_ref(),
            HotkeyAction::Pause => self.pause.as_ref(),
            HotkeyAction::Screenshot => self.screenshot.as_ref(),
            HotkeyAction::FrameAdvance => self.frame_advance.as_ref(),
            HotkeyAction::HardReset => self.hard_reset.as_ref(),
            HotkeyAction::Mute => self.mute.as_ref(),
            HotkeyAction::Fullscreen => self.fullscreen.as_ref(),
            HotkeyAction::StateManager => self.state_manager.as_ref(),
        }
    }

    pub fn set(&mut self, action: HotkeyAction, chord: Option<HotkeyChord>) {
        let slot = match action {
            HotkeyAction::SaveState => &mut self.save_state,
            HotkeyAction::LoadState => &mut self.load_state,
            HotkeyAction::Rewind => &mut self.rewind,
            HotkeyAction::FastForward => &mut self.fast_forward,
            HotkeyAction::Pause => &mut self.pause,
            HotkeyAction::Screenshot => &mut self.screenshot,
            HotkeyAction::FrameAdvance => &mut self.frame_advance,
            HotkeyAction::HardReset => &mut self.hard_reset,
            HotkeyAction::Mute => &mut self.mute,
            HotkeyAction::Fullscreen => &mut self.fullscreen,
            HotkeyAction::StateManager => &mut self.state_manager,
        };
        *slot = chord;
    }

    pub fn display(&self, action: HotkeyAction) -> String {
        self.get(action)
            .map(HotkeyChord::display)
            .unwrap_or_else(|| "Unbound".to_string())
    }
}

fn default_turbo_a() -> Option<String> {
    Some("V".to_string())
}

fn default_turbo_b() -> Option<String> {
    Some("C".to_string())
}

fn default_autofire_interval() -> u8 {
    4
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyBindings {
    pub a: String,
    pub b: String,
    pub select: String,
    pub start: String,
    pub right: String,
    pub left: String,
    pub up: String,
    pub down: String,
    pub r: String,
    pub l: String,
    #[serde(default = "default_turbo_a")]
    pub turbo_a: Option<String>,
    #[serde(default = "default_turbo_b")]
    pub turbo_b: Option<String>,
    #[serde(default)]
    pub autofire_mode: AutofireMode,
    #[serde(default = "default_autofire_interval")]
    pub autofire_interval_frames: u8,
    #[serde(default)]
    pub gamepad: GamepadBindings,
    #[serde(default)]
    pub hotkeys: HotkeyBindings,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            a: "Z".to_string(),
            b: "X".to_string(),
            select: "Backspace".to_string(),
            start: "Enter".to_string(),
            right: "ArrowRight".to_string(),
            left: "ArrowLeft".to_string(),
            up: "ArrowUp".to_string(),
            down: "ArrowDown".to_string(),
            r: "E".to_string(),
            l: "Q".to_string(),
            turbo_a: default_turbo_a(),
            turbo_b: default_turbo_b(),
            autofire_mode: AutofireMode::Hold,
            autofire_interval_frames: default_autofire_interval(),
            gamepad: GamepadBindings::default(),
            hotkeys: HotkeyBindings::default(),
        }
    }
}

impl KeyBindings {
    pub fn key_name(&self, btn: GbaButton) -> &str {
        match btn {
            GbaButton::A => &self.a,
            GbaButton::B => &self.b,
            GbaButton::Select => &self.select,
            GbaButton::Start => &self.start,
            GbaButton::Right => &self.right,
            GbaButton::Left => &self.left,
            GbaButton::Up => &self.up,
            GbaButton::Down => &self.down,
            GbaButton::R => &self.r,
            GbaButton::L => &self.l,
        }
    }

    pub fn set_key_name(&mut self, btn: GbaButton, name: String) {
        let slot = match btn {
            GbaButton::A => &mut self.a,
            GbaButton::B => &mut self.b,
            GbaButton::Select => &mut self.select,
            GbaButton::Start => &mut self.start,
            GbaButton::Right => &mut self.right,
            GbaButton::Left => &mut self.left,
            GbaButton::Up => &mut self.up,
            GbaButton::Down => &mut self.down,
            GbaButton::R => &mut self.r,
            GbaButton::L => &mut self.l,
        };
        *slot = name;
    }

    pub fn egui_key(&self, btn: GbaButton) -> Option<Key> {
        parse_key_name(self.key_name(btn))
    }

    pub fn target_key_name(&self, target: ControlTarget) -> Option<&str> {
        match target {
            ControlTarget::Button(btn) => {
                let s = self.key_name(btn);
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
            ControlTarget::TurboA => self.turbo_a.as_deref(),
            ControlTarget::TurboB => self.turbo_b.as_deref(),
        }
    }

    pub fn set_target_key_name(&mut self, target: ControlTarget, name: Option<String>) {
        match target {
            ControlTarget::Button(btn) => {
                self.set_key_name(btn, name.unwrap_or_default());
            }
            ControlTarget::TurboA => self.turbo_a = name,
            ControlTarget::TurboB => self.turbo_b = name,
        }
    }

    pub fn target_egui_key(&self, target: ControlTarget) -> Option<Key> {
        self.target_key_name(target).and_then(parse_key_name)
    }
}

pub fn format_key_name(key: Key) -> String {
    format!("{key:?}")
}

pub fn parse_key_name(name: &str) -> Option<Key> {
    Key::from_name(name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveOverrideChoice {
    Auto,
    None,
    Sram32K,
    Flash64K,
    Flash128K,
    Eeprom512B,
    Eeprom8K,
}

impl SaveOverrideChoice {
    pub const ALL: [SaveOverrideChoice; 7] = [
        Self::Auto,
        Self::None,
        Self::Sram32K,
        Self::Flash64K,
        Self::Flash128K,
        Self::Eeprom512B,
        Self::Eeprom8K,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto-Detect",
            Self::None => "None",
            Self::Sram32K => "SRAM 32 KB",
            Self::Flash64K => "Flash 64 KB",
            Self::Flash128K => "Flash 128 KB",
            Self::Eeprom512B => "EEPROM 512 B",
            Self::Eeprom8K => "EEPROM 8 KB",
        }
    }

    pub const fn to_core(self) -> Option<SaveType> {
        match self {
            Self::Auto => None,
            Self::None => Some(SaveType::None),
            Self::Sram32K => Some(SaveType::Sram32K),
            Self::Flash64K => Some(SaveType::Flash64K),
            Self::Flash128K => Some(SaveType::Flash128K),
            Self::Eeprom512B => Some(SaveType::Eeprom512B),
            Self::Eeprom8K => Some(SaveType::Eeprom8K),
        }
    }

    pub const fn from_core(ov: Option<SaveType>) -> Self {
        match ov {
            None => Self::Auto,
            Some(SaveType::None) => Self::None,
            Some(SaveType::Sram32K) => Self::Sram32K,
            Some(SaveType::Flash64K) => Self::Flash64K,
            Some(SaveType::Flash128K) => Self::Flash128K,
            Some(SaveType::Eeprom512B) => Self::Eeprom512B,
            Some(SaveType::Eeprom8K) => Self::Eeprom8K,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum DisplayFilter {
    AuthenticGbaLcd, // 3x (720x480) LCD Subpixel Dot-Matrix
    #[default]
    SoftHandheld,    // 2x (480x320) Sharp-Bilinear
    SmoothScale2x,   // 2x (480x320) EPX / Scale2x diagonal smoothing
    RawNearest,      // 1x (240x160) Raw square pixels
}

impl DisplayFilter {
    pub const ALL: [Self; 4] = [
        Self::AuthenticGbaLcd,
        Self::SoftHandheld,
        Self::SmoothScale2x,
        Self::RawNearest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::AuthenticGbaLcd => "LCD Matrix",
            Self::SoftHandheld => "Soft LCD",
            Self::SmoothScale2x => "Scale2x",
            Self::RawNearest => "Raw 1x",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::AuthenticGbaLcd => "Authentic GBA LCD (3x subpixel grid)",
            Self::SoftHandheld => "Soft Handheld (2x sharp-bilinear)",
            Self::SmoothScale2x => "Smooth Pixel Art (Scale2x / EPX)",
            Self::RawNearest => "Raw Pixelated (1x nearest-neighbor)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::AuthenticGbaLcd => Self::SoftHandheld,
            Self::SoftHandheld => Self::SmoothScale2x,
            Self::SmoothScale2x => Self::RawNearest,
            Self::RawNearest => Self::AuthenticGbaLcd,
        }
    }

    pub fn texture_options(self) -> egui::TextureOptions {
        match self {
            Self::AuthenticGbaLcd | Self::SoftHandheld => egui::TextureOptions::LINEAR,
            Self::SmoothScale2x | Self::RawNearest => egui::TextureOptions::NEAREST,
        }
    }

    pub fn target_dimensions(self) -> (usize, usize) {
        match self {
            Self::AuthenticGbaLcd => (gba_core::LCD_WIDTH * 3, gba_core::LCD_HEIGHT * 3),
            Self::SoftHandheld | Self::SmoothScale2x => {
                (gba_core::LCD_WIDTH * 2, gba_core::LCD_HEIGHT * 2)
            }
            Self::RawNearest => (gba_core::LCD_WIDTH, gba_core::LCD_HEIGHT),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ShaderPreset {
    Nearest,
    #[default]
    SharpBilinear,
    LcdGrid,
    CrtScanlines,
    ScaleEdge,
}

impl ShaderPreset {
    pub const COUNT: usize = 5;

    pub const ALL: [ShaderPreset; Self::COUNT] = [
        Self::Nearest,
        Self::SharpBilinear,
        Self::LcdGrid,
        Self::CrtScanlines,
        Self::ScaleEdge,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Nearest => 0,
            Self::SharpBilinear => 1,
            Self::LcdGrid => 2,
            Self::CrtScanlines => 3,
            Self::ScaleEdge => 4,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest (Pixel Perfect)",
            Self::SharpBilinear => "Sharp Bilinear (Anti-Shimmer)",
            Self::LcdGrid => "GBA LCD Grid (RGB Subpixels)",
            Self::CrtScanlines => "CRT Scanlines & Curvature",
            Self::ScaleEdge => "ScaleEdge (Edge Smoothing)",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest",
            Self::SharpBilinear => "Sharp",
            Self::LcdGrid => "LCD Grid",
            Self::CrtScanlines => "CRT",
            Self::ScaleEdge => "ScaleEdge",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorProfileChoice {
    Linear,
    #[default]
    Agb001,
    Ags101,
    Ags001,
}

impl ColorProfileChoice {
    pub const ALL: [ColorProfileChoice; 4] = [
        Self::Agb001,
        Self::Ags101,
        Self::Ags001,
        Self::Linear,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Agb001 => "AGB-001 (Reflective sRGB)",
            Self::Ags101 => "AGS-101 (Backlit Vibrant)",
            Self::Ags001 => "AGS-001 (Frontlit Pastel)",
            Self::Linear => "Raw Unfiltered",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DmgPaletteChoice {
    #[default]
    GbaColorized,
    Greyscale,
    ClassicGreen,
}

impl DmgPaletteChoice {
    pub fn to_core(self) -> gb_core::DmgPalette {
        match self {
            Self::Greyscale => gb_core::DmgPalette::Greyscale,
            Self::GbaColorized => gb_core::DmgPalette::GbaColorized,
            Self::ClassicGreen => gb_core::DmgPalette::ClassicGreen,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Greyscale => "Greyscale",
            Self::GbaColorized => "GBA Colorized",
            Self::ClassicGreen => "Classic Green",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BezelStyle {
    #[default]
    None,
    ClassicIndigo,
    OnyxStealth,
    PlatinumSp,
}

impl BezelStyle {
    pub const ALL: [BezelStyle; 4] = [
        Self::None,
        Self::ClassicIndigo,
        Self::OnyxStealth,
        Self::PlatinumSp,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None (Black Screen)",
            Self::ClassicIndigo => "Classic Indigo GBA",
            Self::OnyxStealth => "Onyx Stealth Bezel",
            Self::PlatinumSp => "Platinum SP Frame",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RomPlaytime {
    pub total_seconds: u64,
    pub last_played_timestamp: u64,
    pub launch_count: u32,
}

impl RomPlaytime {
    pub fn formatted_playtime(&self) -> String {
        let mins = self.total_seconds / 60;
        let hours = mins / 60;
        let rem_mins = mins % 60;
        if hours > 0 {
            format!("{hours}h {rem_mins}m")
        } else if mins > 0 {
            format!("{mins}m")
        } else if self.total_seconds > 0 {
            format!("{}s", self.total_seconds)
        } else {
            "-".to_string()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RomSpecificSettings {
    pub save_override: Option<SaveOverrideChoice>,
    pub rtc_override: Option<bool>,
    pub solar_lux: Option<u8>,
    pub cheats: Vec<CheatEntry>,
    pub custom_color_profile: Option<ColorProfileChoice>,
    pub speed_multiplier: Option<f32>,
    pub rumble_enabled: Option<bool>,
    pub solar_enabled: Option<bool>,
    pub tilt_enabled: Option<bool>,
    pub gyro_enabled: Option<bool>,
}

impl RomSpecificSettings {
    pub fn is_empty(&self) -> bool {
        self.save_override.is_none()
            && self.rtc_override.is_none()
            && self.solar_lux.is_none()
            && self.cheats.is_empty()
            && self.custom_color_profile.is_none()
            && self.speed_multiplier.is_none()
            && self.rumble_enabled.is_none()
            && self.solar_enabled.is_none()
            && self.tilt_enabled.is_none()
            && self.gyro_enabled.is_none()
    }
}

pub fn game_settings_key(code: &str, rom_crc32: u32) -> String {
    format!("{code}_{rom_crc32:08X}")
}

fn default_rewind_enabled() -> bool {
    true
}

fn default_rewind_budget_mb() -> u32 {
    64
}

fn default_rewind_interval() -> u8 {
    1
}

fn default_rewind_speed() -> u8 {
    1
}

fn default_fast_forward_speed() -> f32 {
    4.0
}

fn default_ra_enabled() -> bool {
    true
}

fn default_ra_rich_presence() -> bool {
    true
}

fn default_lcd_grid_intensity() -> f32 {
    0.08
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub recent_roms: Vec<RecentRomEntry>,
    pub bios_path: Option<String>,
    pub window_scale: u8,
    pub integer_scaling: bool,
    pub color_correction: bool,
    pub color_profile: ColorProfileChoice,
    pub bezel_style: BezelStyle,
    pub lcd_ghosting: bool,
    pub scanline_grid: bool,
    #[serde(default = "default_lcd_grid_intensity")]
    pub lcd_grid_intensity: f32,
    #[serde(default)]
    pub display_filter: DisplayFilter,
    #[serde(default)]
    pub shader_preset: ShaderPreset,
    #[serde(default)]
    pub frame_blending: bool,
    pub dot_accurate_ppu: bool,
    #[serde(default)]
    pub dmg_palette: DmgPaletteChoice,
    pub master_volume: u8,
    pub muted: bool,
    pub speed_multiplier: f32,
    #[serde(default = "default_fast_forward_speed")]
    pub fast_forward_speed: f32,
    pub layer_mask: u8,
    pub audio_channel_mask: u8,
    pub keybindings: KeyBindings,
    pub per_rom: BTreeMap<String, RomSpecificSettings>,
    pub rom_folders: Vec<String>,
    pub favorites: Vec<String>,
    pub library_view_mode: String,
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub show_menu_bar: bool,
    pub rumble_enabled: bool,
    pub pause_overlay_enabled: bool,
    pub playtimes: BTreeMap<String, RomPlaytime>,
    #[serde(default = "default_rewind_enabled")]
    pub rewind_enabled: bool,
    #[serde(default = "default_rewind_budget_mb")]
    pub rewind_budget_mb: u32,
    #[serde(default = "default_rewind_interval")]
    pub rewind_interval: u8,
    #[serde(default = "default_rewind_speed")]
    pub rewind_speed: u8,
    #[serde(default = "default_ra_enabled")]
    pub ra_enabled: bool,
    pub ra_hardcore: bool,
    #[serde(default = "default_ra_rich_presence")]
    pub ra_rich_presence: bool,
    pub ra_username: String,
    pub ra_token: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            recent_roms: Vec::new(),
            bios_path: None,
            window_scale: 3,
            integer_scaling: false,
            color_correction: true,
            color_profile: ColorProfileChoice::Agb001,
            bezel_style: BezelStyle::None,
            lcd_ghosting: false,
            scanline_grid: false,
            lcd_grid_intensity: 0.08,
            display_filter: DisplayFilter::SoftHandheld,
            shader_preset: ShaderPreset::SharpBilinear,
            frame_blending: false,
            dot_accurate_ppu: false,
            dmg_palette: DmgPaletteChoice::default(),
            master_volume: 100,
            muted: false,
            speed_multiplier: 1.0,
            fast_forward_speed: default_fast_forward_speed(),
            layer_mask: 0x1F,
            audio_channel_mask: 0x3F,
            keybindings: KeyBindings::default(),
            per_rom: BTreeMap::new(),
            rom_folders: Vec::new(),
            favorites: Vec::new(),
            library_view_mode: "Table".to_string(),
            show_toolbar: true,
            show_status_bar: true,
            show_menu_bar: true,
            rumble_enabled: true,
            pause_overlay_enabled: true,
            playtimes: BTreeMap::new(),
            rewind_enabled: default_rewind_enabled(),
            rewind_budget_mb: default_rewind_budget_mb(),
            rewind_interval: default_rewind_interval(),
            rewind_speed: default_rewind_speed(),
            ra_enabled: default_ra_enabled(),
            ra_hardcore: false,
            ra_rich_presence: default_ra_rich_presence(),
            ra_username: String::new(),
            ra_token: String::new(),
        }
    }
}

impl AppConfig {
    pub fn app_data_dir() -> PathBuf {
        #[cfg(target_os = "windows")]
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(target_os = "windows"))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("RustboyAdvance")
    }

    pub fn config_file_path() -> PathBuf {
        Self::app_data_dir().join("config.json")
    }

    pub fn screenshots_dir() -> PathBuf {
        Self::app_data_dir().join("screenshots")
    }

    pub fn from_json_str(raw: &str) -> Self {
        let mut cfg: Self = serde_json::from_str(raw).unwrap_or_default();
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            if val.get("display_filter").is_none() {
                if val.get("scanline_grid").is_some() || val.get("shader_preset").is_some() {
                    if cfg.scanline_grid || cfg.shader_preset == ShaderPreset::LcdGrid {
                        cfg.display_filter = DisplayFilter::AuthenticGbaLcd;
                    } else if cfg.shader_preset == ShaderPreset::SharpBilinear {
                        cfg.display_filter = DisplayFilter::SoftHandheld;
                    } else if cfg.shader_preset == ShaderPreset::ScaleEdge {
                        cfg.display_filter = DisplayFilter::SmoothScale2x;
                    } else if cfg.shader_preset == ShaderPreset::Nearest {
                        cfg.display_filter = DisplayFilter::RawNearest;
                    }
                }
            }
            if val.get("shader_preset").is_none() && cfg.scanline_grid {
                cfg.shader_preset = ShaderPreset::LcdGrid;
            }
            if val.get("lcd_grid_intensity").is_none() {
                cfg.lcd_grid_intensity = 0.08;
            }
        }
        cfg
    }

    pub fn effective_shader_preset(&self) -> ShaderPreset {
        if self.shader_preset == ShaderPreset::Nearest && self.scanline_grid {
            ShaderPreset::LcdGrid
        } else {
            self.shader_preset
        }
    }

    pub fn load() -> Self {
        let path = Self::config_file_path();
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::from_json_str(&raw)
    }

    pub fn save(&self) {
        let path = Self::config_file_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn push_recent(&mut self, path: &Path, title: String, code: String) {
        let path_str = path.to_string_lossy().into_owned();
        self.recent_roms.retain(|e| e.path != path_str);
        self.recent_roms.insert(
            0,
            RecentRomEntry {
                path: path_str,
                title,
                code,
            },
        );
        self.recent_roms.truncate(10);
        self.save();
    }

    pub fn get_game_settings(&self, code: &str, rom_crc32: u32) -> Option<&RomSpecificSettings> {
        let exact_key = game_settings_key(code, rom_crc32);
        self.per_rom
            .get(&exact_key)
            .or_else(|| self.per_rom.get(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playtime_and_settings() {
        let empty = RomPlaytime::default();
        assert_eq!(empty.formatted_playtime(), "-");

        let thirty_secs = RomPlaytime {
            total_seconds: 30,
            last_played_timestamp: 0,
            launch_count: 1,
        };
        assert_eq!(thirty_secs.formatted_playtime(), "30s");

        let five_mins = RomPlaytime {
            total_seconds: 310,
            last_played_timestamp: 0,
            launch_count: 2,
        };
        assert_eq!(five_mins.formatted_playtime(), "5m");

        let two_hours = RomPlaytime {
            total_seconds: 7325,
            last_played_timestamp: 0,
            launch_count: 5,
        };
        assert_eq!(two_hours.formatted_playtime(), "2h 2m");

        // Verify color profile labels
        for profile in ColorProfileChoice::ALL {
            assert!(!profile.label().is_empty());
        }

        // Verify bezel labels
        for bezel in BezelStyle::ALL {
            assert!(!bezel.label().is_empty());
        }
    }

    #[test]
    fn legacy_config_migrates_with_defaults() {
        let legacy_json = r#"{
            "window_scale": 4,
            "muted": true,
            "keybindings": {
                "a": "J",
                "b": "K",
                "select": "Backspace",
                "start": "Enter",
                "right": "D",
                "left": "A",
                "up": "W",
                "down": "S",
                "r": "I",
                "l": "U"
            }
        }"#;

        let parsed: AppConfig = serde_json::from_str(legacy_json).expect("legacy config parse");
        assert_eq!(parsed.window_scale, 4);
        assert!(parsed.muted);
        assert_eq!(parsed.keybindings.a, "J");
        assert_eq!(parsed.keybindings.turbo_a.as_deref(), Some("V"));
        assert_eq!(parsed.keybindings.turbo_b.as_deref(), Some("C"));
        assert_eq!(parsed.keybindings.autofire_interval_frames, 4);
        assert_eq!(parsed.keybindings.gamepad, GamepadBindings::default());
        assert_eq!(parsed.keybindings.hotkeys, HotkeyBindings::default());
        assert!(parsed.rewind_enabled);
        assert_eq!(parsed.rewind_budget_mb, 64);
        assert_eq!(parsed.rewind_interval, 1);
        assert_eq!(parsed.rewind_speed, 1);
        assert!((parsed.fast_forward_speed - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn shader_preset_roundtrip_and_legacy_scanline_grid_migration() {
        for filter in DisplayFilter::ALL {
            assert!(!filter.label().is_empty());
            assert!(!filter.description().is_empty());
            assert_eq!(filter.next().next().next().next(), filter);
            let json = serde_json::to_string(&filter).expect("serialize filter");
            let back: DisplayFilter = serde_json::from_str(&json).expect("deserialize filter");
            assert_eq!(back, filter);
        }

        for preset in ShaderPreset::ALL {
            assert!(!preset.label().is_empty());
            assert!(!preset.short_label().is_empty());
            let json = serde_json::to_string(&preset).expect("serialize preset");
            let back: ShaderPreset = serde_json::from_str(&json).expect("deserialize preset");
            assert_eq!(back, preset);
        }

        let legacy_grid_json = r#"{"scanline_grid": true}"#;
        let migrated = AppConfig::from_json_str(legacy_grid_json);
        assert!(migrated.scanline_grid);
        assert_eq!(migrated.shader_preset, ShaderPreset::LcdGrid);
        assert_eq!(migrated.effective_shader_preset(), ShaderPreset::LcdGrid);
        assert!(!migrated.frame_blending);

        let modern_json = r#"{"scanline_grid": true, "shader_preset": "CrtScanlines", "frame_blending": true}"#;
        let modern = AppConfig::from_json_str(modern_json);
        assert_eq!(modern.shader_preset, ShaderPreset::CrtScanlines);
        assert_eq!(modern.effective_shader_preset(), ShaderPreset::CrtScanlines);
        assert!(modern.frame_blending);
        assert_eq!(modern.display_filter, DisplayFilter::AuthenticGbaLcd);
        assert!((modern.lcd_grid_intensity - 0.08).abs() < 1e-4);

        let default_cfg = AppConfig::from_json_str("{}");
        assert_eq!(default_cfg.display_filter, DisplayFilter::SoftHandheld);
        assert!((default_cfg.lcd_grid_intensity - 0.08).abs() < 1e-4);
    }
}
