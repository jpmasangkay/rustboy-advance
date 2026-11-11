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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct RomSpecificSettings {
    pub save_override: Option<SaveOverrideChoice>,
    pub rtc_override: Option<bool>,
    pub solar_lux: Option<u8>,
    pub cheats: Vec<CheatEntry>,
    pub custom_color_profile: Option<ColorProfileChoice>,
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
    pub dot_accurate_ppu: bool,
    pub master_volume: u8,
    pub muted: bool,
    pub speed_multiplier: f32,
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
            dot_accurate_ppu: false,
            master_volume: 100,
            muted: false,
            speed_multiplier: 1.0,
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

    pub fn load() -> Self {
        let path = Self::config_file_path();
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
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
}
