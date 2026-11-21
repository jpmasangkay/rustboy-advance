use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

static ATOMIC_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct ConfigWriteState {
    pending: Option<(PathBuf, Vec<u8>)>,
    writing: bool,
}

struct AsyncConfigWriter {
    state: Mutex<ConfigWriteState>,
    cv: Condvar,
}

impl AsyncConfigWriter {
    fn global() -> &'static Self {
        static WRITER: OnceLock<AsyncConfigWriter> = OnceLock::new();
        WRITER.get_or_init(|| {
            let writer = AsyncConfigWriter {
                state: Mutex::new(ConfigWriteState {
                    pending: None,
                    writing: false,
                }),
                cv: Condvar::new(),
            };
            let _ = std::thread::Builder::new()
                .name("config-saver".to_string())
                .spawn(|| {
                    let w = AsyncConfigWriter::global();
                    loop {
                        let next = {
                            let mut st = w.state.lock().unwrap_or_else(|e| e.into_inner());
                            while st.pending.is_none() {
                                st = w.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                            }
                            st.writing = true;
                            st.pending.take()
                        };
                        if let Some((path, data)) = next
                            && let Err(e) = write_atomic(&path, &data)
                        {
                            log::warn!(
                                "Failed to atomically save config to {}: {e}",
                                path.display()
                            );
                        }
                        let mut st = w.state.lock().unwrap_or_else(|e| e.into_inner());
                        st.writing = false;
                        if st.pending.is_none() {
                            w.cv.notify_all();
                        }
                    }
                });
            writer
        })
    }

    fn enqueue(&self, path: PathBuf, data: Vec<u8>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending = Some((path, data));
        self.cv.notify_all();
    }

    fn flush(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while st.pending.is_some() || st.writing {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn cancel_and_wait(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending = None;
        while st.writing {
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Atomically writes `data` to `path` by writing to a sibling temporary file,
/// flushing to disk with `sync_all`, and renaming into place.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let pid = std::process::id();
    let seq = ATOMIC_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp_path = path.with_file_name(format!(".{file_name}.{pid}.{seq}.tmp"));

    let write_res = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp_path)?;
        f.write_all(data)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            #[cfg(windows)]
            {
                // On Windows, `rename` can fail if destination exists and is temporarily locked;
                // retry once after removing destination if needed.
                if path.exists() {
                    let _ = fs::remove_file(path);
                    if fs::rename(&tmp_path, path).is_ok() {
                        return Ok(());
                    }
                }
            }
            let _ = fs::remove_file(&tmp_path);
            Err(rename_err)
        }
    }
}

use egui::Key;
use gba_core::cartridge::SaveType;
use serde::{Deserialize, Serialize};

use crate::cheats::CheatEntry;

#[cfg(not(test))]
const KEYRING_SERVICE: &str = "RustboyAdvance";
#[cfg(not(test))]
const KEYRING_RA_USER: &str = "retroachievements_token";

static PORTABLE_MODE: AtomicBool = AtomicBool::new(false);

/// Initializes portable mode (`--portable` CLI flag or `portable.txt` next to the executable).
/// Must be called early during startup before resolving configuration or log paths.
pub fn init_portable_mode() {
    let cli_portable = std::env::args_os().any(|a| a == "--portable");
    let marker_portable = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("portable.txt").is_file()))
        .unwrap_or(false);
    PORTABLE_MODE.store(cli_portable || marker_portable, Ordering::Relaxed);
}

/// Returns `true` when portable mode is active (`--portable` or `portable.txt`).
#[cfg_attr(test, allow(dead_code))]
pub fn is_portable_mode() -> bool {
    PORTABLE_MODE.load(Ordering::Relaxed)
}

#[cfg(test)]
static TEST_KEYRING_TOKEN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

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
            #[cfg(target_os = "macos")]
            parts.push("Cmd");
            #[cfg(not(target_os = "macos"))]
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
                if s.is_empty() { None } else { Some(s) }
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
pub enum ViewportScale {
    #[default]
    FitWindow,
    Handheld3x,
    Compact2x,
    Medium4x,
}

impl ViewportScale {
    pub const ALL: [Self; 4] = [
        Self::FitWindow,
        Self::Handheld3x,
        Self::Compact2x,
        Self::Medium4x,
    ];

    pub fn toolbar_label(self) -> &'static str {
        match self {
            Self::FitWindow => "Size: Fit",
            Self::Handheld3x => "Size: Handheld 3x",
            Self::Compact2x => "Size: Compact 2x",
            Self::Medium4x => "Size: Medium 4x",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FitWindow => "Fit",
            Self::Handheld3x => "Handheld 3x",
            Self::Compact2x => "Compact 2x",
            Self::Medium4x => "Medium 4x",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::FitWindow => "Fit Window (Stretches to fill panel, preserving 3:2 aspect ratio)",
            Self::Handheld3x => "Handheld 3x (720x480 centered, phone-sized physical dimension)",
            Self::Compact2x => "Compact 2x (480x320 centered, true GBA-SP physical screen size)",
            Self::Medium4x => "Medium 4x (960x640 centered, balanced desktop size)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::FitWindow => Self::Handheld3x,
            Self::Handheld3x => Self::Compact2x,
            Self::Compact2x => Self::Medium4x,
            Self::Medium4x => Self::FitWindow,
        }
    }

    pub fn scale_factor(self) -> Option<f32> {
        match self {
            Self::FitWindow => None,
            Self::Handheld3x => Some(3.0),
            Self::Compact2x => Some(2.0),
            Self::Medium4x => Some(4.0),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum DisplayFilter {
    #[default]
    #[serde(alias = "MyBoySmooth")]
    SmoothBilinear, // 1x (240x160) Native Linear ("Smooth Bilinear")
    Sharp1x,         // 1x (240x160) Native Nearest ("Sharp 1x")
    XbrSmooth3x,     // 3x (720x480) xBR 3x edge smoothing (Laptop HD)
    AuthenticGbaLcd, // 3x (720x480) LCD Subpixel Dot-Matrix
    SoftHandheld,    // 2x (480x320) Sharp-Bilinear
    SmoothScale2x,   // 2x (480x320) EPX / Scale2x diagonal smoothing
    CrtScanlines,    // 2x (480x320) CRT Scanlines
    RawNearest,      // Backward compatibility alias for Sharp 1x
}

impl DisplayFilter {
    pub const ALL: [Self; 7] = [
        Self::SmoothBilinear,
        Self::Sharp1x,
        Self::XbrSmooth3x,
        Self::AuthenticGbaLcd,
        Self::SoftHandheld,
        Self::SmoothScale2x,
        Self::CrtScanlines,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::SmoothBilinear => "Smooth Bilinear",
            Self::Sharp1x | Self::RawNearest => "Sharp 1x",
            Self::XbrSmooth3x => "xBR 3x",
            Self::AuthenticGbaLcd => "LCD Matrix",
            Self::SoftHandheld => "Soft LCD",
            Self::SmoothScale2x => "Scale2x",
            Self::CrtScanlines => "CRT Scanlines",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::SmoothBilinear => "Smooth Bilinear (240x160 native linear)",
            Self::Sharp1x | Self::RawNearest => "Sharp 1x (240x160 native nearest)",
            Self::XbrSmooth3x => "xBR 3x (Laptop HD)",
            Self::AuthenticGbaLcd => "LCD Matrix (3x subpixel grid)",
            Self::SoftHandheld => "Soft Handheld (2x sharp-bilinear)",
            Self::SmoothScale2x => "Smooth Pixel Art (Scale2x / EPX)",
            Self::CrtScanlines => "CRT Scanlines (2x with scanline beam gaps)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::SmoothBilinear => Self::Sharp1x,
            Self::Sharp1x | Self::RawNearest => Self::XbrSmooth3x,
            Self::XbrSmooth3x => Self::AuthenticGbaLcd,
            Self::AuthenticGbaLcd => Self::SoftHandheld,
            Self::SoftHandheld => Self::SmoothScale2x,
            Self::SmoothScale2x => Self::CrtScanlines,
            Self::CrtScanlines => Self::SmoothBilinear,
        }
    }

    pub fn texture_options(self) -> egui::TextureOptions {
        match self {
            Self::SmoothBilinear => egui::TextureOptions::LINEAR,
            Self::Sharp1x | Self::RawNearest => egui::TextureOptions::NEAREST,
            Self::XbrSmooth3x | Self::AuthenticGbaLcd | Self::SoftHandheld => {
                egui::TextureOptions::LINEAR
            }
            Self::SmoothScale2x | Self::CrtScanlines => egui::TextureOptions::NEAREST,
        }
    }

    pub fn target_dimensions(self) -> (usize, usize) {
        match self {
            Self::SmoothBilinear | Self::Sharp1x | Self::RawNearest => {
                (gba_core::LCD_WIDTH, gba_core::LCD_HEIGHT)
            }
            Self::XbrSmooth3x | Self::AuthenticGbaLcd => {
                (gba_core::LCD_WIDTH * 3, gba_core::LCD_HEIGHT * 3)
            }
            Self::SoftHandheld | Self::SmoothScale2x | Self::CrtScanlines => {
                (gba_core::LCD_WIDTH * 2, gba_core::LCD_HEIGHT * 2)
            }
        }
    }

    pub const fn default_shader_preset(self) -> ShaderPreset {
        match self {
            Self::SmoothBilinear | Self::SoftHandheld => ShaderPreset::SharpBilinear,
            Self::Sharp1x | Self::RawNearest | Self::XbrSmooth3x => ShaderPreset::Nearest,
            Self::AuthenticGbaLcd => ShaderPreset::LcdGrid,
            Self::SmoothScale2x => ShaderPreset::ScaleEdge,
            Self::CrtScanlines => ShaderPreset::CrtScanlines,
        }
    }

    pub const fn quality_tier(self) -> u8 {
        match self {
            Self::Sharp1x | Self::RawNearest => 0,
            Self::SmoothBilinear => 1,
            Self::SoftHandheld => 2,
            Self::CrtScanlines | Self::AuthenticGbaLcd => 3,
            Self::SmoothScale2x => 4,
            Self::XbrSmooth3x => 5,
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
    pub const ALL: [ColorProfileChoice; 4] =
        [Self::Agb001, Self::Ags101, Self::Ags001, Self::Linear];

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
    #[allow(dead_code)]
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
    false
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
    false
}

fn default_ra_rich_presence() -> bool {
    true
}

fn default_lcd_grid_intensity() -> f32 {
    0.08
}

fn default_auto_frameskip() -> bool {
    false
}

fn default_max_frameskip() -> u8 {
    3
}

fn default_run_ahead_frames() -> u8 {
    0
}

fn default_auto_quality_governor() -> bool {
    true
}

fn default_legacy_consecutive_failures() -> u32 {
    2
}

fn default_ui_scale() -> f32 {
    1.0
}

fn default_analog_deadzone() -> f32 {
    0.40
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedGpuAdapterEntry {
    pub adapter_name: String,
    pub backend: String,
    pub device_type: String,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
    pub failed_stage: String,
    pub reason: String,
    pub recorded_utc: String,
    #[serde(default = "default_legacy_consecutive_failures")]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub recorded_epoch_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedGpuAdapterEntry {
    pub adapter_name: String,
    pub backend: String,
    pub device_type: String,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
    #[serde(default)]
    pub verified_epoch_secs: u64,
}

fn deserialize_tolerant_filter_map<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, DisplayFilter>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(raw) = Option::<BTreeMap<String, serde_json::Value>>::deserialize(deserializer)?
    else {
        return Ok(BTreeMap::new());
    };
    Ok(raw
        .into_iter()
        .filter_map(|(key, value)| {
            DisplayFilter::deserialize(value)
                .ok()
                .map(|filter| (key, filter))
        })
        .collect())
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
    pub viewport_scale: ViewportScale,
    #[serde(default)]
    pub display_filter: DisplayFilter,
    #[serde(default = "default_auto_quality_governor")]
    pub auto_quality_governor: bool,
    #[serde(default, deserialize_with = "deserialize_tolerant_filter_map")]
    pub governor_settled_filters: BTreeMap<String, DisplayFilter>,
    #[serde(default)]
    pub blocked_gpu_adapters: BTreeMap<String, BlockedGpuAdapterEntry>,
    #[serde(default)]
    pub verified_gpu_adapters: BTreeMap<String, VerifiedGpuAdapterEntry>,
    #[serde(default)]
    pub shader_preset: ShaderPreset,
    #[serde(default)]
    pub frame_blending: bool,
    pub dot_accurate_ppu: bool,
    #[serde(default = "default_auto_frameskip")]
    pub auto_frameskip: bool,
    #[serde(default = "default_max_frameskip")]
    pub max_frameskip: u8,
    #[serde(default = "default_run_ahead_frames")]
    pub run_ahead_frames: u8,
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
    #[serde(default)]
    pub developer_tools: bool,
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
    #[serde(default, skip_serializing)]
    pub ra_token: String,
    #[serde(default)]
    pub auto_download_covers: bool,
    #[serde(default = "default_welcome_completed")]
    pub welcome_completed: bool,
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    #[serde(default = "default_analog_deadzone")]
    pub analog_deadzone: f32,
}

fn default_welcome_completed() -> bool {
    true
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
            viewport_scale: ViewportScale::FitWindow,
            display_filter: DisplayFilter::SmoothBilinear,
            auto_quality_governor: default_auto_quality_governor(),
            governor_settled_filters: BTreeMap::new(),
            blocked_gpu_adapters: BTreeMap::new(),
            verified_gpu_adapters: BTreeMap::new(),
            shader_preset: ShaderPreset::SharpBilinear,
            frame_blending: false,
            dot_accurate_ppu: false,
            auto_frameskip: default_auto_frameskip(),
            max_frameskip: default_max_frameskip(),
            run_ahead_frames: default_run_ahead_frames(),
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
            developer_tools: false,
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
            auto_download_covers: false,
            welcome_completed: true,
            ui_scale: default_ui_scale(),
            analog_deadzone: default_analog_deadzone(),
        }
    }
}

impl AppConfig {
    /// Returns the OS user configuration/data directory (`%APPDATA%\RustboyAdvance` on Windows,
    /// `$XDG_CONFIG_HOME/RustboyAdvance` or `~/.config/RustboyAdvance` on Unix), or the directory
    /// beside the executable when portable mode (`--portable` or `portable.txt`) is enabled.
    pub fn app_data_dir() -> PathBuf {
        #[cfg(test)]
        {
            std::env::temp_dir()
                .join(format!("rustboy_unit_tests_{}", std::process::id()))
                .join("RustboyAdvance")
        }
        #[cfg(not(test))]
        {
            if is_portable_mode()
                && let Ok(exe) = std::env::current_exe()
                && let Some(dir) = exe.parent()
            {
                return dir.to_path_buf();
            }
            Self::default_app_data_dir()
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub fn default_app_data_dir() -> PathBuf {
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

    pub fn library_config_path() -> PathBuf {
        Self::app_data_dir().join("rustboy_library.cfg")
    }

    pub fn saves_dir() -> PathBuf {
        Self::app_data_dir().join("saves")
    }

    pub fn states_dir() -> PathBuf {
        Self::app_data_dir().join("states")
    }

    pub fn covers_dir() -> PathBuf {
        Self::app_data_dir().join("covers")
    }

    pub fn screenshots_dir() -> PathBuf {
        Self::app_data_dir().join("screenshots")
    }

    pub fn crash_log_path() -> PathBuf {
        Self::app_data_dir().join("rustboy-advance-crash.log")
    }

    pub fn startup_error_log_path() -> PathBuf {
        Self::app_data_dir().join("rustboy-advance-startup-error.log")
    }

    /// Canonical battery save write path (`<data_dir>/saves/<rom_stem>.sav`).
    pub fn resolve_sav_path_for_rom(rom_path: &Path) -> PathBuf {
        let stem = rom_path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("save");
        Self::saves_dir().join(format!("{stem}.sav"))
    }

    /// Locates an existing `.sav` file, checking canonical `<data_dir>/saves/<stem>.sav` first and
    /// falling back to legacy sibling `<rom_dir>/<stem>.sav`.
    pub fn find_existing_sav_path_for_rom(rom_path: &Path) -> Option<PathBuf> {
        let primary = Self::resolve_sav_path_for_rom(rom_path);
        if primary.is_file() {
            return Some(primary);
        }
        let legacy = rom_path.with_extension("sav");
        if legacy.is_file() {
            return Some(legacy);
        }
        None
    }

    /// Canonical save-state slot write path (`<data_dir>/states/<rom_stem>.slot<slot>.state`).
    pub fn resolve_state_slot_path(rom_path: &Path, slot: usize) -> Option<PathBuf> {
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(Self::states_dir().join(format!("{stem}.slot{slot}.state")))
    }

    /// Locates an existing save-state slot, checking `<data_dir>/states/` first then legacy
    /// ROM-adjacent `<rom_dir>/<stem>.slot<slot>.state`.
    pub fn find_existing_state_slot_path(rom_path: &Path, slot: usize) -> Option<PathBuf> {
        let primary = Self::resolve_state_slot_path(rom_path, slot)?;
        if primary.is_file() {
            return Some(primary);
        }
        let stem = rom_path.file_stem()?.to_string_lossy();
        let legacy = rom_path.with_file_name(format!("{stem}.slot{slot}.state"));
        if legacy.is_file() {
            return Some(legacy);
        }
        None
    }

    /// Canonical save-state thumbnail path (`<data_dir>/states/<rom_stem>.slot<slot>.png`).
    pub fn resolve_state_thumb_path(rom_path: &Path, slot: usize) -> Option<PathBuf> {
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(Self::states_dir().join(format!("{stem}.slot{slot}.png")))
    }

    pub fn store_ra_token(token: &str) {
        #[cfg(test)]
        {
            if let Ok(mut guard) = TEST_KEYRING_TOKEN.lock() {
                *guard = (!token.is_empty()).then(|| token.to_string());
            }
        }
        #[cfg(not(test))]
        {
            if token.is_empty() {
                Self::clear_ra_token();
                return;
            }
            match keyring::Entry::new(KEYRING_SERVICE, KEYRING_RA_USER) {
                Ok(entry) => {
                    if let Err(err) = entry.set_password(token) {
                        log::warn!("Failed storing RetroAchievements token in OS keyring: {err}");
                    }
                }
                Err(err) => {
                    log::warn!("Failed opening OS keyring entry for RetroAchievements: {err}");
                }
            }
        }
    }

    pub fn load_ra_token() -> String {
        #[cfg(test)]
        {
            TEST_KEYRING_TOKEN
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_default()
        }
        #[cfg(not(test))]
        {
            match keyring::Entry::new(KEYRING_SERVICE, KEYRING_RA_USER) {
                Ok(entry) => entry.get_password().unwrap_or_default(),
                Err(_) => String::new(),
            }
        }
    }

    pub fn clear_ra_token() {
        #[cfg(test)]
        {
            if let Ok(mut guard) = TEST_KEYRING_TOKEN.lock() {
                *guard = None;
            }
        }
        #[cfg(not(test))]
        {
            if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, KEYRING_RA_USER) {
                let _ = entry.delete_credential();
            }
        }
    }

    fn try_from_json_str(raw: &str) -> Option<Self> {
        let mut cfg: Self = match serde_json::from_str(raw) {
            Ok(parsed) => parsed,
            Err(_) => {
                // Upgrade configs whose `display_filter` used an unknown legacy variant name.
                let mut map =
                    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(raw).ok()?;
                map.remove("display_filter");
                serde_json::from_value(serde_json::Value::Object(map)).ok()?
            }
        };
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            if val.get("display_filter").is_none()
                && (val.get("scanline_grid").is_some() || val.get("shader_preset").is_some())
            {
                if cfg.scanline_grid || cfg.shader_preset == ShaderPreset::LcdGrid {
                    cfg.display_filter = DisplayFilter::AuthenticGbaLcd;
                } else if cfg.shader_preset == ShaderPreset::SharpBilinear {
                    cfg.display_filter = DisplayFilter::SoftHandheld;
                } else if cfg.shader_preset == ShaderPreset::ScaleEdge {
                    cfg.display_filter = DisplayFilter::SmoothScale2x;
                } else if cfg.shader_preset == ShaderPreset::Nearest {
                    cfg.display_filter = DisplayFilter::Sharp1x;
                }
            }
            if val.get("shader_preset").is_none() && cfg.scanline_grid {
                cfg.shader_preset = ShaderPreset::LcdGrid;
            }
            if val.get("lcd_grid_intensity").is_none() {
                cfg.lcd_grid_intensity = 0.08;
            }
        }
        if cfg.display_filter == DisplayFilter::RawNearest {
            cfg.display_filter = DisplayFilter::Sharp1x;
        }
        cfg.ui_scale = cfg.ui_scale.clamp(0.8, 2.0);
        cfg.analog_deadzone = cfg.analog_deadzone.clamp(0.10, 0.85);
        Some(cfg)
    }

    #[cfg(test)]
    pub fn from_json_str(raw: &str) -> Self {
        Self::try_from_json_str(raw).unwrap_or_default()
    }

    fn load_from_path(path: &Path) -> Self {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Self {
                    welcome_completed: false,
                    ..Self::default()
                };
            }
            Err(_) => return Self::default(),
        };
        let Some(mut cfg) = Self::try_from_json_str(&raw) else {
            let bak = path.with_extension("json.bak");
            if let Err(e) = fs::write(&bak, &raw) {
                log::warn!(
                    "failed to write corrupt config backup {}: {e}",
                    bak.display()
                );
            } else {
                log::warn!(
                    "failed to parse {}, backed up to {}",
                    path.display(),
                    bak.display()
                );
            }
            return Self::default();
        };
        // Migrate legacy plaintext `ra_token` from config.json into the OS credential store.
        if !cfg.ra_token.is_empty() {
            Self::store_ra_token(&cfg.ra_token);
            cfg.save();
        } else if !cfg.ra_username.is_empty() {
            cfg.ra_token = Self::load_ra_token();
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
        Self::load_from_path(&Self::config_file_path())
    }

    /// Enqueues an atomic config write on a dedicated background thread (coalescing rapid UI edits)
    /// so clicking menus, toggling checkboxes, or dragging sliders never blocks gameplay on `fsync`.
    pub fn save(&self) {
        let path = Self::config_file_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            AsyncConfigWriter::global().enqueue(path, json.into_bytes());
        }
    }

    /// Blocks until any queued background config write has finished syncing to disk.
    /// Call during application shutdown so no final preference changes are lost.
    pub fn flush_pending_saves() {
        AsyncConfigWriter::global().flush();
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

    /// Maps an output viewport resolution + configured scale into a deterministic bucket string
    /// for per-`(adapter, resolution)` governor filter persistence.
    pub fn resolution_bucket(width: u32, height: u32, window_scale: u8) -> String {
        if width == 0 || height == 0 {
            let s = window_scale.clamp(1, 5) as u32;
            return format!("{}x{}", 240 * s, 160 * s);
        }
        if width.is_multiple_of(240)
            && height.is_multiple_of(160)
            && (width / 240) == (height / 160)
        {
            return format!("{width}x{height}");
        }
        match height {
            0..=240 => "240p".to_string(),
            241..=360 => "360p".to_string(),
            361..=540 => "480p".to_string(),
            541..=720 => "720p".to_string(),
            721..=1080 => "1080p".to_string(),
            _ => "1440p+".to_string(),
        }
    }

    pub fn governor_profile_key(
        adapter_name: &str,
        adapter_backend: &str,
        width: u32,
        height: u32,
        window_scale: u8,
    ) -> String {
        format!(
            "{}|{}|{}",
            adapter_backend.trim(),
            adapter_name.trim(),
            Self::resolution_bucket(width, height, window_scale)
        )
    }

    pub fn settled_filter_for(&self, key: &str) -> Option<DisplayFilter> {
        self.governor_settled_filters.get(key).copied()
    }

    pub fn set_settled_filter(&mut self, key: String, filter: DisplayFilter) {
        self.governor_settled_filters.insert(key, filter);
    }

    pub fn gpu_adapter_block_key(
        adapter_name: &str,
        backend: &str,
        vendor: u32,
        device: u32,
    ) -> String {
        format!(
            "{}|{}|{vendor:#06x}|{device:#06x}",
            backend.trim(),
            adapter_name.trim()
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn blocked_gpu_adapter(&self, key: &str) -> Option<&BlockedGpuAdapterEntry> {
        self.blocked_gpu_adapters
            .get(key)
            .or_else(|| self.blocked_gpu_adapters.get("adapter-enum"))
    }

    pub fn record_blocked_gpu_adapter(&mut self, key: String, entry: BlockedGpuAdapterEntry) {
        self.blocked_gpu_adapters.insert(key, entry);
    }

    pub fn clear_blocked_gpu_adapters(&mut self) -> usize {
        let n = self.blocked_gpu_adapters.len();
        self.blocked_gpu_adapters.clear();
        self.verified_gpu_adapters.clear();
        n
    }

    pub fn verified_gpu_adapter(
        &self,
        key: &str,
        driver: &str,
        driver_info: &str,
    ) -> Option<&VerifiedGpuAdapterEntry> {
        self.verified_gpu_adapters
            .get(key)
            .filter(|e| e.driver == driver && e.driver_info == driver_info)
    }

    pub fn record_verified_gpu_adapter(&mut self, key: String, entry: VerifiedGpuAdapterEntry) {
        self.verified_gpu_adapters.insert(key, entry);
    }

    /// Cleanly removes all persistent emulator configuration, library cache, battery saves,
    /// save states, box-art covers, screenshots, crash logs, and OS keyring credentials
    /// inside `Self::app_data_dir()`.
    pub fn uninstall_all_data() -> Result<Vec<PathBuf>, String> {
        AsyncConfigWriter::global().cancel_and_wait();
        Self::clear_ra_token();
        Self::uninstall_data_at(&Self::app_data_dir(), is_portable_mode())
    }

    /// Removes all emulator-managed files and directories under `data_dir`.
    /// When `portable_only_managed_items` is false and `data_dir` ends with `"RustboyAdvance"`,
    /// removes `data_dir` entirely after clearing managed subdirectories.
    pub fn uninstall_data_at(
        data_dir: &Path,
        portable_only_managed_items: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let mut removed = Vec::new();
        let managed_files = [
            data_dir.join("config.json"),
            data_dir.join("config.json.bak"),
            data_dir.join("rustboy_library.cfg"),
            data_dir.join("rustboy-advance-crash.log"),
            data_dir.join("rustboy-advance-startup-error.log"),
        ];
        let managed_dirs = [
            data_dir.join("saves"),
            data_dir.join("states"),
            data_dir.join("covers"),
            data_dir.join("screenshots"),
        ];

        for file in &managed_files {
            if file.is_file() {
                fs::remove_file(file)
                    .map_err(|e| format!("Failed removing {}: {e}", file.display()))?;
                removed.push(file.clone());
            }
        }
        for dir in &managed_dirs {
            if dir.is_dir() {
                fs::remove_dir_all(dir)
                    .map_err(|e| format!("Failed removing directory {}: {e}", dir.display()))?;
                removed.push(dir.clone());
            }
        }

        if !portable_only_managed_items
            && data_dir.is_dir()
            && data_dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case("RustboyAdvance"))
        {
            fs::remove_dir_all(data_dir)
                .map_err(|e| format!("Failed removing {}: {e}", data_dir.display()))?;
            if !removed.contains(&data_dir.to_path_buf()) {
                removed.push(data_dir.to_path_buf());
            }
        }

        Ok(removed)
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
        assert!(!parsed.rewind_enabled);
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
            let mut cycled = filter;
            for _ in 0..DisplayFilter::ALL.len() {
                cycled = cycled.next();
            }
            assert_eq!(cycled, filter);
            let json = serde_json::to_string(&filter).expect("serialize filter");
            let back: DisplayFilter = serde_json::from_str(&json).expect("deserialize filter");
            assert_eq!(back, filter);
        }

        for scale in ViewportScale::ALL {
            assert!(!scale.toolbar_label().is_empty());
            assert!(!scale.description().is_empty());
            assert_eq!(scale.next().next().next().next(), scale);
            let json = serde_json::to_string(&scale).expect("serialize viewport scale");
            let back: ViewportScale =
                serde_json::from_str(&json).expect("deserialize viewport scale");
            assert_eq!(back, scale);
        }

        let legacy_grid_json = r#"{"scanline_grid": true}"#;
        let migrated = AppConfig::from_json_str(legacy_grid_json);
        assert!(migrated.scanline_grid);
        assert_eq!(migrated.shader_preset, ShaderPreset::LcdGrid);
        assert_eq!(migrated.effective_shader_preset(), ShaderPreset::LcdGrid);
        assert!(!migrated.frame_blending);

        let modern_json =
            r#"{"scanline_grid": true, "shader_preset": "CrtScanlines", "frame_blending": true}"#;
        let modern = AppConfig::from_json_str(modern_json);
        assert_eq!(modern.shader_preset, ShaderPreset::CrtScanlines);
        assert_eq!(modern.effective_shader_preset(), ShaderPreset::CrtScanlines);
        assert!(modern.frame_blending);
        assert_eq!(modern.display_filter, DisplayFilter::AuthenticGbaLcd);
        assert!((modern.lcd_grid_intensity - 0.08).abs() < 1e-4);

        let default_cfg = AppConfig::from_json_str("{}");
        assert_eq!(default_cfg.display_filter, DisplayFilter::SmoothBilinear);
        assert_eq!(default_cfg.viewport_scale, ViewportScale::FitWindow);
        assert!((default_cfg.lcd_grid_intensity - 0.08).abs() < 1e-4);
        assert!(!default_cfg.ra_enabled);
        assert!(!default_cfg.auto_download_covers);
    }

    #[test]
    fn viewport_scale_toolbar_labels_and_dimensions() {
        assert_eq!(ViewportScale::FitWindow.toolbar_label(), "Size: Fit");
        assert_eq!(
            ViewportScale::Handheld3x.toolbar_label(),
            "Size: Handheld 3x"
        );
        assert_eq!(ViewportScale::Compact2x.toolbar_label(), "Size: Compact 2x");
        assert_eq!(ViewportScale::Medium4x.toolbar_label(), "Size: Medium 4x");

        // Cycle test: Fit -> Handheld 3x -> Compact 2x -> Medium 4x -> Fit
        assert_eq!(ViewportScale::FitWindow.next(), ViewportScale::Handheld3x);
        assert_eq!(ViewportScale::Handheld3x.next(), ViewportScale::Compact2x);
        assert_eq!(ViewportScale::Compact2x.next(), ViewportScale::Medium4x);
        assert_eq!(ViewportScale::Medium4x.next(), ViewportScale::FitWindow);

        // Physical dimension multipliers for GBA (240x160):
        let (w, h) = (gba_core::LCD_WIDTH as f32, gba_core::LCD_HEIGHT as f32);
        assert_eq!(ViewportScale::FitWindow.scale_factor(), None);
        assert_eq!(ViewportScale::Handheld3x.scale_factor(), Some(3.0));
        assert_eq!(ViewportScale::Compact2x.scale_factor(), Some(2.0));
        assert_eq!(ViewportScale::Medium4x.scale_factor(), Some(4.0));

        assert_eq!((w * 3.0, h * 3.0), (720.0, 480.0));
        assert_eq!((w * 2.0, h * 2.0), (480.0, 320.0));
        assert_eq!((w * 4.0, h * 4.0), (960.0, 640.0));
    }

    #[test]
    fn governor_settled_filter_persists_per_adapter_and_resolution() {
        let mut cfg = AppConfig::default();
        let k1 = AppConfig::governor_profile_key("RTX 5070", "Vulkan", 720, 480, 3);
        let k2 = AppConfig::governor_profile_key("RTX 5070", "Vulkan", 1920, 1080, 3);
        let k3 = AppConfig::governor_profile_key("softbuffer", "NoGpu", 720, 480, 3);
        assert_eq!(k1, "Vulkan|RTX 5070|720x480");
        assert_eq!(k2, "Vulkan|RTX 5070|1080p");
        assert_eq!(k3, "NoGpu|softbuffer|720x480");

        cfg.set_settled_filter(k1.clone(), DisplayFilter::SmoothBilinear);
        cfg.set_settled_filter(k2.clone(), DisplayFilter::Sharp1x);
        let raw = serde_json::to_string(&cfg).expect("serialize config");
        let back = AppConfig::from_json_str(&raw);
        assert_eq!(
            back.settled_filter_for(&k1),
            Some(DisplayFilter::SmoothBilinear)
        );
        assert_eq!(back.settled_filter_for(&k2), Some(DisplayFilter::Sharp1x));
        assert_eq!(back.settled_filter_for(&k3), None);
    }

    #[test]
    fn gpu_adapter_entries_roundtrip_and_queries() {
        let mut cfg = AppConfig::default();
        let key = AppConfig::gpu_adapter_block_key("Test GPU", "Vulkan", 0x10de, 0x2800);
        cfg.record_blocked_gpu_adapter(
            key.clone(),
            BlockedGpuAdapterEntry {
                adapter_name: "Test GPU".to_string(),
                backend: "Vulkan".to_string(),
                device_type: "DiscreteGpu".to_string(),
                vendor: 0x10de,
                device: 0x2800,
                driver: "nvidia".to_string(),
                driver_info: "560.1".to_string(),
                failed_stage: "device".to_string(),
                reason: "test".to_string(),
                recorded_utc: "unix:1700000000".to_string(),
                consecutive_failures: 1,
                recorded_epoch_secs: 1_700_000_000,
            },
        );
        assert_eq!(
            cfg.blocked_gpu_adapter(&key)
                .map(|e| e.consecutive_failures),
            Some(1)
        );

        cfg.record_verified_gpu_adapter(
            key.clone(),
            VerifiedGpuAdapterEntry {
                adapter_name: "Test GPU".to_string(),
                backend: "Vulkan".to_string(),
                device_type: "DiscreteGpu".to_string(),
                vendor: 0x10de,
                device: 0x2800,
                driver: "nvidia".to_string(),
                driver_info: "560.1".to_string(),
                verified_epoch_secs: 1_700_000_050,
            },
        );
        assert!(cfg.verified_gpu_adapter(&key, "nvidia", "560.1").is_some());
        assert!(cfg.verified_gpu_adapter(&key, "nvidia", "560.2").is_none());

        assert_eq!(cfg.clear_blocked_gpu_adapters(), 1);
        assert!(cfg.blocked_gpu_adapter(&key).is_none());
        assert!(cfg.verified_gpu_adapter(&key, "nvidia", "560.1").is_none());
    }

    #[test]
    fn ra_token_is_excluded_from_json_and_paths_target_data_dirs() {
        AppConfig::clear_ra_token();
        let cfg = AppConfig {
            ra_username: "player1".to_string(),
            ra_token: "super_secret_token".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(
            !json.contains("super_secret_token"),
            "ra_token must never be serialized to config.json"
        );
        assert!(!json.contains("\"ra_token\""));

        // Legacy plaintext migration path
        let legacy_raw = r#"{"ra_username":"player1","ra_token":"legacy_tok_99"}"#;
        let migrated = AppConfig::from_json_str(legacy_raw);
        assert_eq!(migrated.ra_token, "legacy_tok_99");
        AppConfig::store_ra_token(&migrated.ra_token);
        assert_eq!(AppConfig::load_ra_token(), "legacy_tok_99");
        AppConfig::clear_ra_token();
        assert!(AppConfig::load_ra_token().is_empty());

        let fake_rom = PathBuf::from("roms/GoldenSun.gba");
        let sav = AppConfig::resolve_sav_path_for_rom(&fake_rom);
        assert_eq!(
            sav.parent().unwrap().file_name().unwrap(),
            "saves",
            "default battery save path should live in AppData/portable saves/ directory"
        );
        assert_eq!(sav.file_name().unwrap(), "GoldenSun.sav");

        let slot = AppConfig::resolve_state_slot_path(&fake_rom, 2).expect("slot path");
        assert_eq!(
            slot.parent().unwrap().file_name().unwrap(),
            "states",
            "default save-state path should live in AppData/portable states/ directory"
        );
        assert_eq!(slot.file_name().unwrap(), "GoldenSun.slot2.state");
    }

    #[test]
    fn legacy_v010_filter_names_and_corrupt_backup_preserve_settings() {
        let v010_json = r#"{
            "window_scale": 4,
            "bios_path": "/roms/gba_bios.bin",
            "integer_scaling": true,
            "color_correction": false,
            "display_filter": "MyBoySmooth",
            "governor_settled_filters": {
                "Vulkan|RTX 5070|720x480": "MyBoySmooth",
                "Vulkan|RTX 5070|1080p": "RemovedFutureFilter",
                "NoGpu|softbuffer|720x480": "Sharp1x"
            }
        }"#;
        let parsed = AppConfig::from_json_str(v010_json);
        assert_eq!(parsed.display_filter, DisplayFilter::SmoothBilinear);
        assert_eq!(parsed.window_scale, 4);
        assert_eq!(parsed.bios_path.as_deref(), Some("/roms/gba_bios.bin"));
        assert!(parsed.integer_scaling);
        assert!(!parsed.color_correction);
        assert_eq!(
            parsed.settled_filter_for("Vulkan|RTX 5070|720x480"),
            Some(DisplayFilter::SmoothBilinear)
        );
        assert_eq!(parsed.settled_filter_for("Vulkan|RTX 5070|1080p"), None);
        assert_eq!(
            parsed.settled_filter_for("NoGpu|softbuffer|720x480"),
            Some(DisplayFilter::Sharp1x)
        );

        assert_eq!(
            crate::bench::parse_display_filter("myboy-smooth"),
            Ok(DisplayFilter::SmoothBilinear)
        );
        assert_eq!(
            crate::bench::parse_display_filter("myboy"),
            Ok(DisplayFilter::SmoothBilinear)
        );

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rustboy_cfg_backup_{}_{nonce}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        let cfg_path = dir.join("config.json");
        let broken = "{ not valid json";
        fs::write(&cfg_path, broken).expect("write broken config");
        let fallback = AppConfig::load_from_path(&cfg_path);
        assert_eq!(fallback.display_filter, DisplayFilter::SmoothBilinear);
        assert_eq!(
            fs::read_to_string(dir.join("config.json.bak")).expect("read backup"),
            broken
        );

        // First-run (missing config file) defaults to showing Welcome Card without touching existing files
        let missing_cfg = dir.join("fresh_config.json");
        let fresh = AppConfig::load_from_path(&missing_cfg);
        assert!(!fresh.welcome_completed);

        // Existing legacy config without `welcome_completed` field preserves false=completed default (true)
        let legacy = AppConfig::from_json_str(v010_json);
        assert!(legacy.welcome_completed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn async_config_writer_coalesces_and_flushes_atomically() {
        let dir = std::env::temp_dir().join(format!(
            "rustboy_async_cfg_test_{}_{}",
            std::process::id(),
            ATOMIC_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = dir.join("config.json");
        let writer = AsyncConfigWriter::global();
        for i in 0..8u32 {
            writer.enqueue(
                path.clone(),
                format!(r#"{{"master_volume":{i}}}"#).into_bytes(),
            );
        }
        AppConfig::flush_pending_saves();
        let raw = fs::read_to_string(&path).expect("flushed config file");
        let parsed = AppConfig::from_json_str(&raw);
        assert_eq!(parsed.master_volume, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_data_at_removes_all_managed_files_dirs_and_clears_ra_token() {
        let dir = std::env::temp_dir()
            .join(format!(
                "rustboy_uninstall_test_{}_{}",
                std::process::id(),
                ATOMIC_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ))
            .join("RustboyAdvance");
        fs::create_dir_all(dir.join("saves")).expect("create saves");
        fs::create_dir_all(dir.join("states")).expect("create states");
        fs::create_dir_all(dir.join("covers")).expect("create covers");
        fs::create_dir_all(dir.join("screenshots")).expect("create screenshots");
        fs::write(dir.join("config.json"), b"{}").expect("write config");
        fs::write(dir.join("rustboy_library.cfg"), b"[]").expect("write library");
        fs::write(dir.join("saves/test.sav"), b"SAVE").expect("write save");
        fs::write(dir.join("states/test.slot1.state"), b"STATE").expect("write state");
        fs::write(dir.join("covers/test.png"), b"PNG").expect("write cover");
        fs::write(dir.join("screenshots/shot.png"), b"SHOT").expect("write screenshot");

        AppConfig::store_ra_token("tok_uninstall_check");
        assert_eq!(AppConfig::load_ra_token(), "tok_uninstall_check");
        AppConfig::clear_ra_token();
        let removed = AppConfig::uninstall_data_at(&dir, false).expect("uninstall data");
        assert!(!removed.is_empty());
        assert!(!dir.exists(), "RustboyAdvance data directory must be deleted");
        assert!(AppConfig::load_ra_token().is_empty());
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }
}
