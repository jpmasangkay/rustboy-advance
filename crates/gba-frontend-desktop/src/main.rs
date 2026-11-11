mod app_host;
mod audio;
mod cheats;
mod config;
mod debugger_ui;
mod filters;
mod headless;

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{
    self, Align, Color32, CornerRadius, Key, Layout, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use gba_core::{encode_keyinput, GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gilrs::{Axis, Button, Gilrs};

use crate::audio::AudioOutput;
use crate::cheats::{apply_cheats_to_core, CheatEntry};
use crate::config::{AppConfig, GbaButton, RomSpecificSettings, SaveOverrideChoice};
use crate::debugger_ui::{DebugAction, DebuggerWindows};
use crate::filters::VideoPostProcessor;
use crate::headless::CliConfig;

const CONFIG_FILE: &str = "rustboy_library.cfg";
const GBA_FRAME_SECS: f64 = 280_896.0 / 16_777_216.0; // Exact GBA refresh ~16.7427ms (~59.7275 Hz)
const BG_APP: Color32 = Color32::from_rgb(13, 15, 19);
const BG_PANEL: Color32 = Color32::from_rgb(20, 23, 30);
const BG_CARD: Color32 = Color32::from_rgb(25, 29, 38);
const BORDER_HAIRLINE: Color32 = Color32::from_rgb(42, 48, 61);
const ACCENT_TEAL: Color32 = Color32::from_rgb(46, 196, 182);
const ACCENT_TEAL_BG: Color32 = Color32::from_rgb(26, 62, 62);
const TEXT_MUTED: Color32 = Color32::from_rgb(142, 152, 170);

#[derive(Clone, Debug)]
pub struct RomLibraryEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub internal_title: String,
    pub game_code: String,
    pub region: &'static str,
    pub size_mb: f32,
    pub has_battery_sav: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum LibraryViewMode {
    Table,
    Grid,
}

struct CoreSink {
    audio: AudioOutput,
    framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    keypad: u16,
    frame_dirty: bool,
    suppress_audio: bool,
}

impl CoreSink {
    fn new(audio: AudioOutput) -> Self {
        Self {
            audio,
            framebuffer: [0; LCD_WIDTH * LCD_HEIGHT],
            keypad: 0x03FF,
            frame_dirty: false,
            suppress_audio: false,
        }
    }
}

impl HostFrontend for CoreSink {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.framebuffer.copy_from_slice(fb);
        self.frame_dirty = true;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        if !self.suppress_audio {
            self.audio.push_interleaved(samples);
        }
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad
    }
}

struct ToastMessage {
    text: String,
    expires_at: Instant,
}

struct RustboyApp {
    config: AppConfig,
    core: Option<GbaCore>,
    sink: CoreSink,
    postproc: VideoPostProcessor,
    lcd_tex: Option<TextureHandle>,
    debugger: DebuggerWindows,
    gilrs: Option<Gilrs>,

    rom_path: Option<PathBuf>,
    sav_path: Option<PathBuf>,
    rom_title: String,
    rom_code: String,
    bios_bytes: Option<Vec<u8>>,
    active_cheats: Vec<CheatEntry>,

    // ROM Library & Folder Scanner
    rom_folders: Vec<PathBuf>,
    library_entries: Vec<RomLibraryEntry>,
    favorites: HashSet<PathBuf>,
    search_query: String,
    filter_favorites_only: bool,
    library_view_mode: LibraryViewMode,

    // Real-Time Rewind Buffer
    rewind_buffer: VecDeque<Vec<u8>>,
    frame_counter: u64,
    rewinding: bool,

    paused: bool,
    tab_fast_forward: bool,
    is_fullscreen: bool,
    last_pressed_mask: u16,
    next_frame_due: Instant,
    last_save_flush: Instant,
    last_fps_sample: Instant,
    frames_in_window: u32,
    measured_fps: f32,
    last_core_ms: f32,
    toast: Option<ToastMessage>,
    title_dirty: bool,
    pub(crate) quit_requested: bool,
}

impl RustboyApp {
    fn new(cli: CliConfig) -> Self {
        let config = AppConfig::load();
        let audio = AudioOutput::start();
        audio.set_muted(config.muted);
        let sink = CoreSink::new(audio);

        let bios_bytes = cli
            .bios_path
            .as_deref()
            .and_then(|p| fs::read(p).ok())
            .or_else(|| config.bios_path.as_deref().and_then(|p| fs::read(p).ok()));

        let mut app = Self {
            config,
            core: None,
            sink,
            postproc: VideoPostProcessor::new(),
            lcd_tex: None,
            debugger: DebuggerWindows::default(),
            gilrs: Gilrs::new().ok(),
            rom_path: None,
            sav_path: None,
            rom_title: String::new(),
            rom_code: String::new(),
            bios_bytes,
            active_cheats: Vec::new(),
            rom_folders: Vec::new(),
            library_entries: Vec::new(),
            favorites: HashSet::new(),
            search_query: String::new(),
            filter_favorites_only: false,
            library_view_mode: LibraryViewMode::Table,
            rewind_buffer: VecDeque::with_capacity(60),
            frame_counter: 0,
            rewinding: false,
            paused: false,
            tab_fast_forward: false,
            is_fullscreen: false,
            last_pressed_mask: 0,
            next_frame_due: Instant::now(),
            last_save_flush: Instant::now(),
            last_fps_sample: Instant::now(),
            frames_in_window: 0,
            measured_fps: 0.0,
            last_core_ms: 0.0,
            toast: None,
            title_dirty: true,
            quit_requested: false,
        };

        app.load_persistent_config();
        app.rescan_rom_folders();

        if let Some(rom_path) = cli.rom_path {
            if let Err(e) = app.open_rom(&rom_path, cli.sav_path.as_deref(), None) {
                app.notify(&format!("Failed to load CLI ROM: {e}"));
            }
        }
        app
    }

    fn notify(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            expires_at: Instant::now() + Duration::from_millis(3500),
        });
    }

    fn sync_window_title(&self, ctx: &egui::Context) {
        let title = if self.core.is_some() {
            format!("Rustboy Advance — {} ({})", self.rom_title, self.rom_code)
        } else {
            "Rustboy Advance".to_string()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn save_persistent_config(&mut self) {
        let mut lines = Vec::new();
        if let Some(bios) = &self.config.bios_path {
            lines.push(format!("BIOS={bios}"));
        }
        for folder in &self.rom_folders {
            lines.push(format!("FOLDER={}", folder.display()));
        }
        for fav in &self.favorites {
            lines.push(format!("FAV={}", fav.display()));
        }
        let _ = fs::write(CONFIG_FILE, lines.join("\n"));

        // Mirror to AppConfig
        self.config.rom_folders = self
            .rom_folders
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.config.favorites = self
            .favorites
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.config.library_view_mode = match self.library_view_mode {
            LibraryViewMode::Table => "Table".to_string(),
            LibraryViewMode::Grid => "Grid".to_string(),
        };
        self.config.save();
    }

    fn load_persistent_config(&mut self) {
        let mut loaded_folders = Vec::new();
        let mut loaded_favs = HashSet::new();

        let cfg_path = Path::new(CONFIG_FILE);
        let cfg_content = fs::read_to_string(cfg_path)
            .or_else(|_| fs::read_to_string(AppConfig::app_data_dir().join(CONFIG_FILE)));

        if let Ok(content) = cfg_content {
            for line in content.lines() {
                let line = line.trim();
                if let Some(p) = line.strip_prefix("BIOS=") {
                    let path = PathBuf::from(p);
                    if let Ok(b) = fs::read(&path) {
                        if b.len() >= 0x4000 {
                            self.bios_bytes = Some(b[..0x4000].to_vec());
                            self.config.bios_path = Some(p.to_string());
                        }
                    }
                } else if let Some(f) = line.strip_prefix("FOLDER=") {
                    let path = PathBuf::from(f);
                    if path.is_dir() && !loaded_folders.contains(&path) {
                        loaded_folders.push(path);
                    }
                } else if let Some(fav) = line.strip_prefix("FAV=") {
                    loaded_favs.insert(PathBuf::from(fav));
                }
            }
        }

        // Incorporate any paths persisted in AppConfig
        for f in &self.config.rom_folders {
            let p = PathBuf::from(f);
            if p.is_dir() && !loaded_folders.contains(&p) {
                loaded_folders.push(p);
            }
        }
        for fav in &self.config.favorites {
            loaded_favs.insert(PathBuf::from(fav));
        }

        if self.config.library_view_mode == "Grid" {
            self.library_view_mode = LibraryViewMode::Grid;
        } else {
            self.library_view_mode = LibraryViewMode::Table;
        }

        self.rom_folders = loaded_folders;
        self.favorites = loaded_favs;
    }

    fn scan_folder_recursive(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(ext.as_str(), "gba" | "agb" | "bin") {
                    out.push(p);
                }
            } else if p.is_dir() {
                Self::scan_folder_recursive(&p, depth + 1, out);
            }
        }
    }

    fn parse_gba_header(path: &Path) -> Option<RomLibraryEntry> {
        use std::io::Read;
        let mut file = fs::File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        let size_mb = (metadata.len() as f32) / (1024.0 * 1024.0);
        let mut header = [0u8; 0xC0];
        let bytes_read = file.read(&mut header).ok()?;

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let (internal_title, game_code, region) = if bytes_read >= 0xB0 {
            let title_raw = &header[0xA0..0xAC];
            let title_clean: String = title_raw
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { ' ' })
                .collect();
            let code_raw = &header[0xAC..0xB0];
            let code_clean: String = code_raw
                .iter()
                .map(|&c| if c.is_ascii_alphanumeric() { c as char } else { '-' })
                .collect();
            let reg = match code_raw.get(3).copied().unwrap_or(b' ') {
                b'E' | b'U' => "USA",
                b'P' | b'D' | b'F' | b'I' | b'S' => "EUR",
                b'J' => "JPN",
                b'A' => "World",
                _ => "Unk",
            };
            (title_clean.trim().to_string(), code_clean, reg)
        } else {
            (String::from("Homebrew"), String::from("HOME"), "Unk")
        };

        let has_battery_sav = path.with_extension("sav").exists();

        Some(RomLibraryEntry {
            path: path.to_path_buf(),
            display_name,
            internal_title,
            game_code,
            region,
            size_mb,
            has_battery_sav,
        })
    }

    fn rescan_rom_folders(&mut self) {
        self.library_entries.clear();
        let mut found_files = Vec::new();
        for folder in &self.rom_folders {
            Self::scan_folder_recursive(folder, 0, &mut found_files);
        }
        found_files.sort();
        found_files.dedup();

        for path in found_files {
            if let Some(entry) = Self::parse_gba_header(&path) {
                self.library_entries.push(entry);
            }
        }
        self.library_entries.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        });
    }

    fn add_rom_folder_dialog(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Select ROM Directory")
            .pick_folder()
        {
            if !self.rom_folders.contains(&folder) {
                self.rom_folders.push(folder);
                self.save_persistent_config();
                self.rescan_rom_folders();
                self.notify(&format!(
                    "Added folder — {} GBA title(s) in library",
                    self.library_entries.len()
                ));
            }
        }
    }

    fn apply_ips_patch_if_present(rom_path: &Path, rom: &mut Vec<u8>) -> bool {
        let ips_path = rom_path.with_extension("ips");
        let Ok(patch) = fs::read(&ips_path) else {
            return false;
        };
        if patch.len() < 8 || &patch[0..5] != b"PATCH" {
            return false;
        }
        let mut pos = 5;
        while pos + 3 <= patch.len() {
            if &patch[pos..pos + 3] == b"EOF" {
                if pos + 6 <= patch.len() {
                    let trunc_len = ((patch[pos + 3] as usize) << 16)
                        | ((patch[pos + 4] as usize) << 8)
                        | (patch[pos + 5] as usize);
                    rom.truncate(trunc_len);
                }
                return true;
            }
            if pos + 5 > patch.len() {
                break;
            }
            let offset = ((patch[pos] as usize) << 16)
                | ((patch[pos + 1] as usize) << 8)
                | (patch[pos + 2] as usize);
            let size = ((patch[pos + 3] as usize) << 8) | (patch[pos + 4] as usize);
            pos += 5;
            if size == 0 {
                // RLE record
                if pos + 3 > patch.len() {
                    break;
                }
                let rle_len = ((patch[pos] as usize) << 8) | (patch[pos + 1] as usize);
                let val = patch[pos + 2];
                pos += 3;
                if offset + rle_len > rom.len() {
                    rom.resize(offset + rle_len, 0xFF);
                }
                rom[offset..offset + rle_len].fill(val);
            } else {
                if pos + size > patch.len() {
                    break;
                }
                if offset + size > rom.len() {
                    rom.resize(offset + size, 0xFF);
                }
                rom[offset..offset + size].copy_from_slice(&patch[pos..pos + size]);
                pos += size;
            }
        }
        true
    }

    fn open_rom(
        &mut self,
        rom_path: &Path,
        custom_sav: Option<&Path>,
        ctx: Option<&egui::Context>,
    ) -> Result<(), String> {
        self.flush_battery_save();
        self.save_rom_specific_settings();

        let mut rom_bytes = fs::read(rom_path).map_err(|e| format!("Failed reading ROM: {e}"))?;
        let ips_applied = Self::apply_ips_patch_if_present(rom_path, &mut rom_bytes);

        let sav_path = custom_sav
            .map(PathBuf::from)
            .unwrap_or_else(|| rom_path.with_extension("sav"));

        let mut core = GbaCore::new(self.bios_bytes.as_deref(), rom_bytes);
        if let Ok(sav) = fs::read(&sav_path) {
            core.bus.cart.load_save_data(&sav);
        }

        let title = core.bus.cart.game_title();
        let code = core.bus.cart.game_code();

        // Apply saved overrides & cheats for this ROM if present, else global config settings
        core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
        core.bus.ppu.layer_enable_mask = self.config.layer_mask;
        core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
        core.bus.apu.master_volume_pct = self.config.master_volume;

        let rom_key = code.clone();
        if let Some(ov) = self.config.per_rom.get(&rom_key).cloned() {
            if let Some(sv) = ov.save_override {
                core.bus.cart.set_save_type_override(sv.to_core());
            }
            if ov.rtc_override.is_some() {
                core.bus.cart.rtc_override = ov.rtc_override;
            }
            if let Some(lux) = ov.solar_lux {
                core.bus.cart.solar_lux = lux;
            }
            self.active_cheats = ov.cheats;
        } else {
            self.active_cheats.clear();
        }

        self.core = Some(core);
        self.rom_path = Some(rom_path.to_path_buf());
        self.sav_path = Some(sav_path);
        self.rom_title = title.clone();
        self.rom_code = code.clone();
        self.paused = false;
        self.rewind_buffer.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.next_frame_due = Instant::now();
        self.frames_in_window = 0;
        self.last_fps_sample = Instant::now();
        self.postproc.prev_rgba.fill(0);
        self.sink.framebuffer.fill(0);
        self.sink.frame_dirty = true;

        self.config.push_recent(rom_path, title.clone(), code.clone());
        self.title_dirty = true;
        if let Some(c) = ctx {
            self.sync_window_title(c);
            self.title_dirty = false;
        }
        let patch_tag = if ips_applied { " (IPS Patched)" } else { "" };
        self.notify(&format!("Playing {title} [{code}]{patch_tag}"));
        Ok(())
    }

    fn close_rom(&mut self, ctx: &egui::Context) {
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.core = None;
        self.rom_path = None;
        self.sav_path = None;
        self.rom_title.clear();
        self.rom_code.clear();
        self.active_cheats.clear();
        self.paused = false;
        self.rewind_buffer.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.lcd_tex = None;
        self.rescan_rom_folders();
        self.sync_window_title(ctx);
        self.notify("Game stopped — Returned to ROM Library");
    }

    fn save_rom_specific_settings(&mut self) {
        let Some(core) = &self.core else { return };
        let key = self.rom_code.clone();
        if key.is_empty() {
            return;
        }
        let ov = RomSpecificSettings {
            save_override: Some(SaveOverrideChoice::from_core(
                core.bus.cart.save_type_override,
            )),
            rtc_override: core.bus.cart.rtc_override,
            solar_lux: Some(core.bus.cart.solar_lux),
            cheats: self.active_cheats.clone(),
        };
        self.config.per_rom.insert(key, ov);
        self.config.save();
    }

    fn flush_battery_save(&mut self) {
        let (Some(core), Some(sav_path)) = (&mut self.core, &self.sav_path) else {
            return;
        };
        if !core.bus.cart.save_dirty {
            return;
        }
        let data = core.bus.cart.export_save_data();
        if !data.is_empty() && fs::write(sav_path, data).is_ok() {
            core.bus.cart.save_dirty = false;
        }
    }

    fn load_custom_bios_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select GBA BIOS Binary")
            .add_filter("BIOS Image", &["bin", "gba", "rom"])
            .pick_file()
        {
            match fs::read(&path) {
                Ok(bytes) if bytes.len() >= 0x4000 => {
                    self.bios_bytes = Some(bytes[..0x4000].to_vec());
                    self.config.bios_path = Some(path.to_string_lossy().into_owned());
                    self.config.save();
                    if let Some(core) = &mut self.core {
                        core.load_bios(self.bios_bytes.as_deref());
                    }
                    self.notify("Loaded Custom 16KB GBA BIOS");
                }
                Ok(_) => self.notify("BIOS must be at least 16 KB (16,384 bytes)"),
                Err(e) => self.notify(&format!("Failed reading BIOS: {e}")),
            }
        }
    }

    fn clear_custom_bios(&mut self) {
        self.bios_bytes = None;
        self.config.bios_path = None;
        self.config.save();
        if let Some(core) = &mut self.core {
            core.load_bios(None);
        }
        self.notify("Switched back to built-in HLE BIOS");
    }

    fn slot_path(&self, slot: usize) -> Option<PathBuf> {
        let rom_path = self.rom_path.as_ref()?;
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(rom_path.with_file_name(format!("{stem}.slot{slot}.state")))
    }

    fn quick_save(&mut self, slot: usize) {
        let (Some(core), Some(path)) = (&self.core, self.slot_path(slot)) else {
            return;
        };
        let res = core
            .save_state()
            .map_err(|e| format!("{e}"))
            .and_then(|bytes| fs::write(&path, bytes).map_err(|e| format!("{e}")));
        match res {
            Ok(()) => self.notify(&format!("Saved State Slot {slot}")),
            Err(e) => self.notify(&format!("Save State failed: {e}")),
        }
    }

    fn quick_load(&mut self, slot: usize) {
        let Some(path) = self.slot_path(slot) else {
            return;
        };
        self.load_state_from_path(&path, &format!("Loaded State Slot {slot}"));
    }

    fn save_state_to_file(&mut self) {
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

    fn load_state_from_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Load Save State")
            .add_filter("Rustboy Save State", &["state", "gbas"])
            .pick_file()
        {
            self.load_state_from_path(&path, "Loaded State from file");
        }
    }

    fn load_state_from_path(&mut self, path: &Path, success_msg: &str) {
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

    fn import_battery_save(&mut self) {
        let Some(core) = &mut self.core else { return };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import Battery Save (.sav)")
            .add_filter("Battery Save", &["sav", "srm"])
            .pick_file()
        {
            match fs::read(&path) {
                Ok(bytes) => {
                    core.bus.cart.load_save_data(&bytes);
                    core.bus.cart.save_dirty = true;
                    self.flush_battery_save();
                    self.notify("Imported Battery Save");
                }
                Err(e) => self.notify(&format!("Import failed: {e}")),
            }
        }
    }

    fn export_battery_save(&mut self) {
        let Some(core) = &self.core else { return };
        let data = core.bus.cart.export_save_data();
        if data.is_empty() {
            self.notify("Current game reports no backup save memory");
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export Battery Save (.sav)")
            .add_filter("Battery Save", &["sav"])
            .set_file_name(format!("{}.sav", self.rom_title))
            .save_file()
        {
            match fs::write(&path, data) {
                Ok(()) => self.notify("Exported Battery Save"),
                Err(e) => self.notify(&format!("Export failed: {e}")),
            }
        }
    }

    fn capture_screenshot(&mut self) {
        if self.core.is_none() {
            return;
        }
        let save_path = if let Some(rom_path) = &self.rom_path {
            rom_path.with_extension("png")
        } else {
            let dir = AppConfig::screenshots_dir();
            let _ = fs::create_dir_all(&dir);
            dir.join(format!("{}.png", self.rom_title))
        };

        if let Some(parent) = save_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let res = image::save_buffer_with_format(
            &save_path,
            &self.postproc.curr_rgba,
            LCD_WIDTH as u32,
            LCD_HEIGHT as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        );

        match res {
            Ok(()) => self.notify(&format!("Screenshot saved: {}", save_path.display())),
            Err(e) => {
                let ppm_path = save_path.with_extension("ppm");
                let mut data = format!("P6\n{} {}\n255\n", LCD_WIDTH, LCD_HEIGHT).into_bytes();
                for chunk in self.postproc.curr_rgba.chunks_exact(4) {
                    data.push(chunk[0]);
                    data.push(chunk[1]);
                    data.push(chunk[2]);
                }
                if fs::write(&ppm_path, data).is_ok() {
                    self.notify(&format!("Screenshot saved: {}", ppm_path.display()));
                } else {
                    self.notify(&format!("Failed saving screenshot: {e}"));
                }
            }
        }
    }

    fn set_window_scale(&mut self, ctx: &egui::Context, scale: u8) {
        self.config.window_scale = scale.clamp(1, 5);
        self.config.save();
        let w = (LCD_WIDTH as f32) * (self.config.window_scale as f32) + 24.0;
        let h = (LCD_HEIGHT as f32) * (self.config.window_scale as f32) + 92.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        self.is_fullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(w, h)));
    }

    fn poll_inputs(&mut self, ctx: &egui::Context) -> u16 {
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

    fn connected_gamepad_names(&self) -> Vec<String> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        gilrs
            .gamepads()
            .filter(|(_, p)| p.is_connected())
            .map(|(_, p)| p.name().to_string())
            .collect()
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.debugger.awaiting_rebind.is_some() {
            return;
        }

        // Escape or Ctrl+W returns to library when a game is running
        let stop_game = ctx.input(|i| {
            (i.modifiers.ctrl && i.key_pressed(Key::W)) || i.key_pressed(Key::Escape)
        });
        if stop_game && self.core.is_some() {
            self.close_rom(ctx);
            return;
        }

        // Holding R in-game rewinds real-time
        self.rewinding = self.core.is_some()
            && ctx.input(|i| i.key_down(Key::R) && !i.modifiers.ctrl && !i.modifiers.alt);

        let open_rom = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::O));
        let step_frame = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::N));
        let hard_reset = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::R));
        let toggle_mute = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::M));
        let toggle_pause = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::Space));
        let screenshot = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::F12));
        let fullscreen = ctx.input(|i| !i.modifiers.any() && i.key_pressed(Key::F11));

        let fn_slot = ctx.input(|i| {
            let keys = [Key::F1, Key::F2, Key::F3, Key::F4, Key::F5];
            keys.iter()
                .position(|&k| i.key_pressed(k))
                .map(|idx| (idx + 1, i.modifiers.shift))
        });

        if open_rom {
            self.trigger_open_rom_dialog(ctx);
        }
        if step_frame {
            self.step_single_frame();
        }
        if hard_reset {
            if let Some(core) = &mut self.core {
                core.reset();
                self.notify("Hard Reset");
            }
        }
        if toggle_mute {
            self.config.muted = !self.config.muted;
            self.sink.audio.set_muted(self.config.muted);
            self.config.save();
            self.notify(if self.config.muted { "Audio Muted" } else { "Audio Unmuted" });
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
        }
        if let Some((slot, is_load)) = fn_slot {
            if is_load {
                self.quick_load(slot);
            } else {
                self.quick_save(slot);
            }
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if !self.paused {
            // Avoid spurious immediate re-hit if currently parked on a breakpoint
            if let Some(core) = &mut self.core {
                let pc = core.cpu.current_exec_pc();
                if self.debugger.breakpoints.contains(&pc) {
                    core.step_one();
                }
            }
            self.next_frame_due = Instant::now();
        }
    }

    fn step_single_frame(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        apply_cheats_to_core(core, &self.active_cheats);
        let _ = core.run_frame_with_breakpoints(&mut self.sink, &[]);
        self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
        self.sink.frame_dirty = true;
    }

    fn step_single_instruction(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        core.step_one();
        if core.bus.ppu.frame_ready {
            self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
            self.sink.frame_dirty = true;
        }
    }

    fn step_single_scanline(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        core.step_scanline();
        if core.bus.ppu.frame_ready {
            self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
            self.sink.frame_dirty = true;
        }
    }

    fn trigger_open_rom_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open Game Boy Advance ROM")
            .add_filter("GBA ROMs", &["gba", "agb", "bin"])
            .pick_file()
        {
            if let Err(e) = self.open_rom(&path, None, Some(ctx)) {
                self.notify(&e);
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            if path.is_dir() {
                if !self.rom_folders.contains(&path) {
                    self.rom_folders.push(path);
                    self.save_persistent_config();
                    self.rescan_rom_folders();
                    self.notify(&format!(
                        "Added ROM folder — {} GBA title(s) in library",
                        self.library_entries.len()
                    ));
                }
                continue;
            }
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "sav" || ext == "srm" {
                if let (Some(core), Ok(bytes)) = (&mut self.core, fs::read(&path)) {
                    core.bus.cart.load_save_data(&bytes);
                    core.bus.cart.save_dirty = true;
                    self.notify("Loaded dropped .sav file");
                }
            } else if ext == "state" || ext == "gbas" {
                self.load_state_from_path(&path, "Loaded dropped Save State");
            } else {
                if let Err(e) = self.open_rom(&path, None, Some(ctx)) {
                    self.notify(&e);
                }
                break;
            }
        }
    }

    fn advance_emulation(&mut self, ctx: &egui::Context) {
        if self.core.is_none() {
            return;
        }

        // Handle active real-time rewind
        if self.rewinding {
            if let Some(prev) = self.rewind_buffer.pop_back() {
                if let Some(core) = &mut self.core {
                    if core.load_state(&prev).is_ok() {
                        self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                        self.sink.frame_dirty = true;
                        self.notify("Rewinding ⏪");
                    }
                }
            } else {
                self.notify("Rewind buffer empty");
                self.rewinding = false;
            }
            ctx.request_repaint();
            return;
        }

        if self.paused {
            return;
        }

        self.sink.keypad = self.poll_inputs(ctx);
        let speed = if self.tab_fast_forward {
            0.0 // Unthrottled fast-forward while Tab held
        } else {
            self.config.speed_multiplier
        };
        let turbo = speed <= 0.0;
        let frame_interval = if turbo {
            Duration::from_millis(0)
        } else {
            Duration::from_secs_f64(GBA_FRAME_SECS / (speed as f64))
        };

        let now = Instant::now();
        let max_catchup = if turbo { 8 } else if speed > 1.5 { 4 } else { 2 };
        let mut executed = 0usize;

        while executed < max_catchup && (turbo || now >= self.next_frame_due) {
            if turbo && executed > 0 && now.elapsed() > Duration::from_millis(12) {
                break;
            }
            let start_t = Instant::now();
            let Some(core) = &mut self.core else { break };
            self.sink.suppress_audio = turbo && executed > 0;
            apply_cheats_to_core(core, &self.active_cheats);

            let bp_hit = core.run_frame_with_breakpoints(&mut self.sink, &self.debugger.breakpoints);
            self.last_core_ms = start_t.elapsed().as_secs_f32() * 1000.0;
            executed += 1;
            self.frames_in_window += 1;
            self.frame_counter += 1;

            // Push a rewind snapshot every 30 frames (~0.5s)
            if self.frame_counter % 30 == 0 {
                if let Ok(state) = core.save_state() {
                    if self.rewind_buffer.len() >= 60 {
                        self.rewind_buffer.pop_front();
                    }
                    self.rewind_buffer.push_back(state);
                }
            }

            if let Some(bp_pc) = bp_hit {
                self.paused = true;
                self.debugger.show_cpu = true;
                self.notify(&format!("Paused at Breakpoint 0x{bp_pc:08X}"));
                break;
            }

            if turbo {
                self.next_frame_due = Instant::now();
                if executed >= 4 {
                    break;
                }
            } else {
                self.next_frame_due += frame_interval;
                if now.saturating_duration_since(self.next_frame_due) > Duration::from_millis(60) {
                    self.next_frame_due = now + frame_interval;
                }
            }
        }
        self.sink.suppress_audio = false;

        if self.last_fps_sample.elapsed() >= Duration::from_millis(500) {
            let secs = self.last_fps_sample.elapsed().as_secs_f32();
            self.measured_fps = (self.frames_in_window as f32) / secs.max(0.001);
            self.frames_in_window = 0;
            self.last_fps_sample = Instant::now();
        }

        if self.last_save_flush.elapsed() >= Duration::from_secs(3) {
            self.flush_battery_save();
            self.last_save_flush = Instant::now();
        }

        if !self.paused {
            if turbo {
                ctx.request_repaint();
            } else {
                let wait = self
                    .next_frame_due
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(16));
                ctx.request_repaint_after(wait);
            }
        }
    }

    fn draw_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_menu_bar")
            .frame(egui::Frame::new().fill(BG_PANEL).inner_margin(4.0).stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE)))
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    let has_rom = self.core.is_some();

                    ui.menu_button("File", |ui| {
                        if ui.button("Open ROM... (Ctrl+O)").clicked() {
                            ui.close_menu();
                            self.trigger_open_rom_dialog(ctx);
                        }
                        if ui.button("Add ROM Folder...").clicked() {
                            ui.close_menu();
                            self.add_rom_folder_dialog();
                        }
                        if ui.button("Rescan ROM Folders").clicked() {
                            ui.close_menu();
                            self.rescan_rom_folders();
                        }
                        ui.menu_button("Open Recent ROM", |ui| {
                            if self.config.recent_roms.is_empty() {
                                ui.label(RichText::new("No recent ROMs").color(TEXT_MUTED));
                            } else {
                                let recent = self.config.recent_roms.clone();
                                for entry in recent {
                                    let lbl = format!("{} ({})", entry.title, entry.code);
                                    if ui.button(lbl).on_hover_text(&entry.path).clicked() {
                                        ui.close_menu();
                                        let p = PathBuf::from(&entry.path);
                                        if let Err(e) = self.open_rom(&p, None, Some(ctx)) {
                                            self.notify(&e);
                                        }
                                    }
                                }
                                ui.separator();
                                if ui.button("Clear Recent List").clicked() {
                                    self.config.recent_roms.clear();
                                    self.config.save();
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.separator();
                        if ui.button("Load Custom BIOS...").clicked() {
                            ui.close_menu();
                            self.load_custom_bios_dialog();
                        }
                        if ui
                            .add_enabled(self.bios_bytes.is_some(), egui::Button::new("Clear Custom BIOS (Use HLE)"))
                            .clicked()
                        {
                            ui.close_menu();
                            self.clear_custom_bios();
                        }
                        ui.separator();
                        ui.add_enabled_ui(has_rom, |ui| {
                            if ui
                                .button("⏹ Close Game / Return to Library (Esc / Ctrl+W)")
                                .clicked()
                            {
                                ui.close_menu();
                                self.close_rom(ctx);
                            }
                            ui.separator();
                            ui.menu_button("Save State", |ui| {
                                for s in 1..=5 {
                                    if ui.button(format!("Slot {s} (F{s})")).clicked() {
                                        ui.close_menu();
                                        self.quick_save(s);
                                    }
                                }
                                ui.separator();
                                if ui.button("Save State to File...").clicked() {
                                    ui.close_menu();
                                    self.save_state_to_file();
                                }
                            });
                            ui.menu_button("Load State", |ui| {
                                for s in 1..=5 {
                                    let exists = self.slot_path(s).is_some_and(|p| p.exists());
                                    let lbl = if exists {
                                        format!("Slot {s} (Shift+F{s}) [Saved]")
                                    } else {
                                        format!("Slot {s} (Shift+F{s})")
                                    };
                                    if ui.add_enabled(exists, egui::Button::new(lbl)).clicked() {
                                        ui.close_menu();
                                        self.quick_load(s);
                                    }
                                }
                                ui.separator();
                                if ui.button("Load State from File...").clicked() {
                                    ui.close_menu();
                                    self.load_state_from_file();
                                }
                            });
                            ui.separator();
                            if ui.button("Import Battery Save (.sav)...").clicked() {
                                ui.close_menu();
                                self.import_battery_save();
                            }
                            if ui.button("Export Battery Save (.sav)...").clicked() {
                                ui.close_menu();
                                self.export_battery_save();
                            }
                            if ui.button("Save Screenshot (F12)").clicked() {
                                ui.close_menu();
                                self.capture_screenshot();
                            }
                        });
                        ui.separator();
                        if ui.button("Exit (Alt+F4)").clicked() {
                            self.quit_requested = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button("Emulation", |ui| {
                        ui.add_enabled_ui(has_rom, |ui| {
                            let p_lbl = if self.paused { "Resume (Space)" } else { "Pause (Space)" };
                            if ui.button(p_lbl).clicked() {
                                self.toggle_pause();
                                ui.close_menu();
                            }
                            if ui.button("Frame Advance / Step 1 Frame (Ctrl+N)").clicked() {
                                self.step_single_frame();
                                ui.close_menu();
                            }
                            if ui.button("⏪ Rewind Step (Hold R)").clicked() {
                                if let Some(prev) = self.rewind_buffer.pop_back() {
                                    if let Some(core) = &mut self.core {
                                        if core.load_state(&prev).is_ok() {
                                            self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                                            self.sink.frame_dirty = true;
                                            self.notify("Rewound 1 step (0.5s)");
                                        }
                                    }
                                }
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Soft Reset (SWI 00h)").clicked() {
                                if let Some(core) = &mut self.core {
                                    core.soft_reset();
                                    self.notify("Soft Reset");
                                }
                                ui.close_menu();
                            }
                            if ui.button("Hard Reset (Ctrl+R)").clicked() {
                                if let Some(core) = &mut self.core {
                                    core.reset();
                                    self.notify("Hard Reset");
                                }
                                ui.close_menu();
                            }
                            if ui.button("⏹ Stop & Return to Library (Esc)").clicked() {
                                self.close_rom(ctx);
                                ui.close_menu();
                            }
                        });
                        ui.separator();
                        ui.menu_button("Speed Multiplier", |ui| {
                            for (mult, lbl) in [
                                (0.5, "0.5x Slow Motion"),
                                (1.0, "1.0x Normal"),
                                (2.0, "2.0x Fast"),
                                (4.0, "4.0x Ultra"),
                                (0.0, "Unthrottled Turbo"),
                            ] {
                                let selected = (self.config.speed_multiplier - mult).abs() < 0.01;
                                if ui.selectable_label(selected, lbl).clicked() {
                                    self.config.speed_multiplier = mult;
                                    self.config.save();
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new("Tip: Hold Tab anytime for Turbo").small().color(TEXT_MUTED));
                        });
                        ui.menu_button("PPU Accuracy", |ui| {
                            if ui
                                .radio(!self.config.dot_accurate_ppu, "Fast Scanline Renderer")
                                .clicked()
                            {
                                self.config.dot_accurate_ppu = false;
                                if let Some(core) = &mut self.core {
                                    core.bus.ppu.dot_accurate_mode = false;
                                }
                                self.config.save();
                                ui.close_menu();
                            }
                            if ui
                                .radio(self.config.dot_accurate_ppu, "Cycle-Accurate Per-Dot Renderer")
                                .clicked()
                            {
                                self.config.dot_accurate_ppu = true;
                                if let Some(core) = &mut self.core {
                                    core.bus.ppu.dot_accurate_mode = true;
                                }
                                self.config.save();
                                ui.close_menu();
                            }
                        });
                    });

                    ui.menu_button("Audio/Video", |ui| {
                        ui.menu_button("Window Scale", |ui| {
                            for s in 1..=5u8 {
                                if ui
                                    .selectable_label(
                                        self.config.window_scale == s,
                                        format!(
                                            "{s}x ({}x{})",
                                            LCD_WIDTH * usize::from(s),
                                            LCD_HEIGHT * usize::from(s)
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.set_window_scale(ctx, s);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            if ui.checkbox(&mut self.config.integer_scaling, "Integer Scaling Lock").changed() {
                                self.config.save();
                            }
                            if ui.selectable_label(self.is_fullscreen, "Fullscreen (F11)").clicked() {
                                self.is_fullscreen = !self.is_fullscreen;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
                                ui.close_menu();
                            }
                        });
                        ui.menu_button("Display Filters", |ui| {
                            if ui.checkbox(&mut self.config.color_correction, "AGB-001 sRGB Color Correction").changed() {
                                self.config.save();
                            }
                            if ui.checkbox(&mut self.config.lcd_ghosting, "LCD Interframe Ghosting").changed() {
                                self.config.save();
                            }
                            if ui.checkbox(&mut self.config.scanline_grid, "Subpixel Scanline Grid").changed() {
                                self.config.save();
                            }
                        });
                        ui.menu_button("Video Layers", |ui| {
                            for (bit, name) in [
                                (0, "BG0 Layer"),
                                (1, "BG1 Layer"),
                                (2, "BG2 Layer"),
                                (3, "BG3 Layer"),
                                (4, "OBJ (Sprites) Layer"),
                            ] {
                                let mut on = (self.config.layer_mask & (1 << bit)) != 0;
                                if ui.checkbox(&mut on, name).changed() {
                                    if on {
                                        self.config.layer_mask |= 1 << bit;
                                    } else {
                                        self.config.layer_mask &= !(1 << bit);
                                    }
                                    if let Some(core) = &mut self.core {
                                        core.bus.ppu.layer_enable_mask = self.config.layer_mask;
                                    }
                                    self.config.save();
                                }
                            }
                        });
                        ui.menu_button("Audio Channels", |ui| {
                            for (bit, name) in [
                                (0, "PSG Square 1"),
                                (1, "PSG Square 2"),
                                (2, "PSG Wave"),
                                (3, "PSG Noise"),
                                (4, "Direct Sound FIFO A"),
                                (5, "Direct Sound FIFO B"),
                            ] {
                                let mut on = (self.config.audio_channel_mask & (1 << bit)) != 0;
                                if ui.checkbox(&mut on, name).changed() {
                                    if on {
                                        self.config.audio_channel_mask |= 1 << bit;
                                    } else {
                                        self.config.audio_channel_mask &= !(1 << bit);
                                    }
                                    if let Some(core) = &mut self.core {
                                        core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
                                    }
                                    self.config.save();
                                }
                            }
                        });
                        ui.separator();
                        if ui.add(egui::Slider::new(&mut self.config.master_volume, 0..=100).text("Master Volume %")).changed() {
                            if let Some(core) = &mut self.core {
                                core.bus.apu.master_volume_pct = self.config.master_volume;
                            }
                            self.config.save();
                        }
                        if ui.checkbox(&mut self.config.muted, "Mute Audio (Ctrl+M)").changed() {
                            self.sink.audio.set_muted(self.config.muted);
                            self.config.save();
                        }
                    });

                    ui.menu_button("Tools", |ui| {
                        if ui.add_enabled(has_rom, egui::Button::new("Game Overrides & Sensors...")).clicked() {
                            self.debugger.show_overrides = true;
                            ui.close_menu();
                        }
                        if ui.button("Cheat Code Manager...").clicked() {
                            self.debugger.show_cheats = true;
                            ui.close_menu();
                        }
                        if ui.button("Controls & Keybindings...").clicked() {
                            self.debugger.show_controls = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Open Screenshots Folder").clicked() {
                            let dir = AppConfig::screenshots_dir();
                            let _ = fs::create_dir_all(&dir);
                            #[cfg(target_os = "windows")]
                            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("Debug / Views", |ui| {
                        ui.checkbox(&mut self.debugger.show_cpu, "ARM7TDMI CPU & Disassembler");
                        ui.checkbox(&mut self.debugger.show_hex, "Memory Hex Viewer");
                        ui.checkbox(&mut self.debugger.show_palette, "Palette Viewer");
                        ui.checkbox(&mut self.debugger.show_vram, "VRAM Tile Viewer");
                        ui.checkbox(&mut self.debugger.show_bg, "BG Map Viewer");
                        ui.checkbox(&mut self.debugger.show_oam, "OAM Sprite Viewer");
                        ui.checkbox(&mut self.debugger.show_audio, "Audio Channel Visualizer");
                        ui.separator();
                        if ui.button("About Rustboy Advance").clicked() {
                            self.debugger.show_about = true;
                            ui.close_menu();
                        }
                    });
                });
            });
    }

    fn draw_toolbar(&mut self, ctx: &egui::Context) {
        let toggle_btn = |ui: &mut egui::Ui, label: &str, active: bool| -> egui::Response {
            let mut btn = egui::Button::new(RichText::new(label).small());
            if active {
                btn = btn
                    .fill(ACCENT_TEAL_BG)
                    .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
            }
            ui.add(btn)
        };

        egui::TopBottomPanel::top("quick_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let has_rom = self.core.is_some();
                    if ui
                        .button(RichText::new("📂 Open ROM").small())
                        .on_hover_text("Open GBA ROM File (Ctrl+O)")
                        .clicked()
                    {
                        self.trigger_open_rom_dialog(ctx);
                    }
                    if ui
                        .button(RichText::new("📁 Add Folder").small())
                        .on_hover_text("Add ROM Directory to Library")
                        .clicked()
                    {
                        self.add_rom_folder_dialog();
                    }
                    if ui
                        .button(RichText::new("🔄 Refresh").small())
                        .on_hover_text("Rescan configured ROM folders")
                        .clicked()
                    {
                        self.rescan_rom_folders();
                    }

                    ui.separator();

                    ui.add_enabled_ui(has_rom, |ui| {
                        let play_lbl = if self.paused { "▶ Resume" } else { "⏸ Pause" };
                        if toggle_btn(ui, play_lbl, self.paused).clicked() {
                            self.toggle_pause();
                        }

                        // Prominent STOP button to exit running game back to ROM Library
                        let stop_btn = egui::Button::new(
                            RichText::new("⏹ Stop")
                                .color(Color32::from_rgb(255, 120, 120))
                                .small(),
                        );
                        if ui
                            .add(stop_btn)
                            .on_hover_text("Stop game and return to ROM Library (Esc / Ctrl+W)")
                            .clicked()
                        {
                            self.close_rom(ctx);
                        }

                        if ui
                            .button(RichText::new("⏭ Step").small())
                            .on_hover_text("Advance 1 Frame (Ctrl+N)")
                            .clicked()
                        {
                            self.step_single_frame();
                        }

                        let rewind_enabled = !self.rewind_buffer.is_empty();
                        let rew_btn = egui::Button::new(RichText::new("⏪ Rewind").small());
                        if ui
                            .add_enabled(rewind_enabled, rew_btn)
                            .on_hover_text("Rewind ~0.5s (or hold R key in-game)")
                            .clicked()
                        {
                            if let Some(prev) = self.rewind_buffer.pop_back() {
                                if let Some(core) = &mut self.core {
                                    if core.load_state(&prev).is_ok() {
                                        self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                                        self.sink.frame_dirty = true;
                                        self.notify("Rewound 1 step (0.5s)");
                                    }
                                }
                            }
                        }

                        if ui
                            .button(RichText::new("Reset").small())
                            .on_hover_text("Hard Reset (Ctrl+R)")
                            .clicked()
                        {
                            if let Some(core) = &mut self.core {
                                core.reset();
                                self.notify("Hard Reset");
                            }
                        }

                        let turbo_on = self.tab_fast_forward
                            || self.config.speed_multiplier > 1.5
                            || self.config.speed_multiplier <= 0.0;
                        if toggle_btn(ui, "FF >>", turbo_on)
                            .on_hover_text("Toggle 4x Fast-Forward (or Hold Tab)")
                            .clicked()
                        {
                            self.config.speed_multiplier =
                                if self.config.speed_multiplier > 1.5 { 1.0 } else { 4.0 };
                            self.config.save();
                        }

                        ui.separator();

                        if ui.button(RichText::new("💾 Save 1").small()).clicked() {
                            self.quick_save(1);
                        }
                        if ui.button(RichText::new("📂 Load 1").small()).clicked() {
                            self.quick_load(1);
                        }
                        if ui
                            .button(RichText::new("📷 Shot").small())
                            .on_hover_text("Save Screenshot alongside ROM (F12)")
                            .clicked()
                        {
                            self.capture_screenshot();
                        }

                        ui.separator();
                    });

                    if toggle_btn(
                        ui,
                        if self.config.muted { "Muted" } else { "Audio" },
                        self.config.muted,
                    )
                    .clicked()
                    {
                        self.config.muted = !self.config.muted;
                        self.sink.audio.set_muted(self.config.muted);
                        self.config.save();
                    }
                    if toggle_btn(ui, "sRGB", self.config.color_correction)
                        .on_hover_text("AGB-001 sRGB Color Correction")
                        .clicked()
                    {
                        self.config.color_correction = !self.config.color_correction;
                        self.config.save();
                    }
                    if toggle_btn(ui, "Grid", self.config.scanline_grid)
                        .on_hover_text("LCD Subpixel Scanline Grid")
                        .clicked()
                    {
                        self.config.scanline_grid = !self.config.scanline_grid;
                        self.config.save();
                    }

                    ui.separator();

                    if toggle_btn(ui, "CPU", self.debugger.show_cpu).clicked() {
                        self.debugger.show_cpu = !self.debugger.show_cpu;
                    }
                    if toggle_btn(ui, "Hex", self.debugger.show_hex).clicked() {
                        self.debugger.show_hex = !self.debugger.show_hex;
                    }
                    if toggle_btn(ui, "VRAM", self.debugger.show_vram).clicked() {
                        self.debugger.show_vram = !self.debugger.show_vram;
                    }
                    if toggle_btn(ui, "Audio", self.debugger.show_audio).clicked() {
                        self.debugger.show_audio = !self.debugger.show_audio;
                    }
                });
            });
    }

    fn draw_status_bar(&mut self, ctx: &egui::Context) {
        if let Some(t) = &self.toast {
            if Instant::now() > t.expires_at {
                self.toast = None;
            }
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
                        ui.label(
                            RichText::new(format!(
                                "{isa} | Mode {mode} | Save: {save_lbl} | Esc = Stop to Library"
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
                        let bios_lbl = if self.bios_bytes.is_some() {
                            "BIOS: Custom"
                        } else {
                            "BIOS: HLE"
                        };

                        if let Some(_core) = &self.core {
                            let speed_str = if self.paused {
                                "PAUSED".to_string()
                            } else if self.tab_fast_forward || self.config.speed_multiplier <= 0.0 {
                                "TURBO".to_string()
                            } else {
                                format!("{:.1}x", self.config.speed_multiplier)
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
                            ui.monospace(RichText::new(bios_lbl).small().color(TEXT_MUTED));
                        } else {
                            ui.monospace(RichText::new(bios_lbl).small().color(TEXT_MUTED));
                        }
                    });
                });
            });
    }

    fn draw_rom_library_browser(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let query = self.search_query.trim().to_lowercase();
        let filtered: Vec<RomLibraryEntry> = self
            .library_entries
            .iter()
            .filter(|e| {
                if self.filter_favorites_only && !self.favorites.contains(&e.path) {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                e.display_name.to_lowercase().contains(&query)
                    || e.internal_title.to_lowercase().contains(&query)
                    || e.game_code.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        // 1. Library Sub-Bar
        egui::Frame::new()
            .fill(BG_PANEL)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let total = self.library_entries.len();
                    let count_str = format!("{total} Title{}", if total == 1 { "" } else { "s" });
                    ui.label(RichText::new(count_str).strong());
                    if !query.is_empty() || self.filter_favorites_only {
                        ui.label(
                            RichText::new(format!("({} shown)", filtered.len()))
                                .small()
                                .color(TEXT_MUTED),
                        );
                    }

                    ui.separator();

                    if ui.button("📁 Add ROM Folder...").clicked() {
                        self.add_rom_folder_dialog();
                    }
                    if ui.button("🔄 Refresh").clicked() {
                        self.rescan_rom_folders();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let is_grid = self.library_view_mode == LibraryViewMode::Grid;
                        let is_table = self.library_view_mode == LibraryViewMode::Table;
                        if ui.selectable_label(is_grid, "⊞ Grid").clicked() {
                            self.library_view_mode = LibraryViewMode::Grid;
                            self.save_persistent_config();
                        }
                        if ui.selectable_label(is_table, "☰ Table").clicked() {
                            self.library_view_mode = LibraryViewMode::Table;
                            self.save_persistent_config();
                        }

                        ui.separator();

                        let fav_active = self.filter_favorites_only;
                        if ui
                            .selectable_label(fav_active, "★ Favorites")
                            .clicked()
                        {
                            self.filter_favorites_only = !self.filter_favorites_only;
                        }

                        ui.separator();

                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Search ROM library...")
                                .desired_width(190.0),
                        );
                    });
                });
            });

        ui.add_space(6.0);

        // 2. Empty States
        if self.rom_folders.is_empty() && filtered.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Game Directories Configured")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Add a folder containing your .gba ROMs to populate the game list, or open a single file.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let pad = (ui.available_width() - 320.0).max(0.0) * 0.5;
                        ui.add_space(pad);
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📁 Add Game Folder..."))
                            .clicked()
                        {
                            self.add_rom_folder_dialog();
                        }
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📂 Open ROM File..."))
                            .clicked()
                        {
                            self.trigger_open_rom_dialog(ctx);
                        }
                    });
                });
            });
            return;
        }

        if !self.rom_folders.is_empty() && self.library_entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Supported ROMs Found")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "The configured directories do not contain any .gba, .agb, or .bin ROM files.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let pad = (ui.available_width() - 320.0).max(0.0) * 0.5;
                        ui.add_space(pad);
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📁 Add Another Folder..."))
                            .clicked()
                        {
                            self.add_rom_folder_dialog();
                        }
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("🔄 Rescan Folders"))
                            .clicked()
                        {
                            self.rescan_rom_folders();
                        }
                    });
                });
            });
            return;
        }

        if !self.library_entries.is_empty() && filtered.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Matching ROMs Found")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("No games match the current search query or favorites filter.")
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(16.0);
                    if ui.button("Clear Search & Filters").clicked() {
                        self.search_query.clear();
                        self.filter_favorites_only = false;
                    }
                });
            });
            return;
        }

        // 3. Render Table or Grid View
        let mut launch_target: Option<PathBuf> = None;
        let mut open_overrides = false;
        let mut rescan_needed = false;

        match self.library_view_mode {
            LibraryViewMode::Table => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("rba_rom_table")
                            .striped(true)
                            .min_col_width(30.0)
                            .spacing([16.0, 7.0])
                            .show(ui, |ui| {
                                ui.strong("★");
                                ui.strong("Game Title (Filename)");
                                ui.strong("Internal Header");
                                ui.strong("Game ID");
                                ui.strong("Region");
                                ui.strong("Size");
                                ui.strong("Save File");
                                ui.strong("Action");
                                ui.end_row();

                                for entry in &filtered {
                                    let is_fav = self.favorites.contains(&entry.path);
                                    let star = if is_fav { "★" } else { "☆" };
                                    let star_color = if is_fav {
                                        Color32::from_rgb(255, 204, 0)
                                    } else {
                                        TEXT_MUTED
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(RichText::new(star).color(star_color))
                                                .frame(false),
                                        )
                                        .clicked()
                                    {
                                        if is_fav {
                                            self.favorites.remove(&entry.path);
                                        } else {
                                            self.favorites.insert(entry.path.clone());
                                        }
                                        self.save_persistent_config();
                                    }

                                    let resp = ui.selectable_label(
                                        false,
                                        RichText::new(&entry.display_name).strong(),
                                    );
                                    if resp.double_clicked() {
                                        launch_target = Some(entry.path.clone());
                                    }
                                    resp.context_menu(|ui| {
                                        if ui.button("▶ Play Game").clicked() {
                                            launch_target = Some(entry.path.clone());
                                            ui.close_menu();
                                        }
                                        if ui.button("📁 Open Save Folder").clicked() {
                                            if let Some(parent) = entry.path.parent() {
                                                #[cfg(target_os = "windows")]
                                                let _ = std::process::Command::new("explorer")
                                                    .arg(parent)
                                                    .spawn();
                                            }
                                            ui.close_menu();
                                        }
                                        if ui.button("⚙ Configure Game Overrides").clicked() {
                                            launch_target = Some(entry.path.clone());
                                            open_overrides = true;
                                            ui.close_menu();
                                        }
                                        if ui
                                            .button(if is_fav {
                                                "☆ Remove from Favorites"
                                            } else {
                                                "★ Add to Favorites"
                                            })
                                            .clicked()
                                        {
                                            if is_fav {
                                                self.favorites.remove(&entry.path);
                                            } else {
                                                self.favorites.insert(entry.path.clone());
                                            }
                                            self.save_persistent_config();
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("🗑 Remove Folder from Library").clicked() {
                                            if let Some(parent) = entry.path.parent() {
                                                self.rom_folders.retain(|f| f != parent);
                                                self.save_persistent_config();
                                                rescan_needed = true;
                                            }
                                            ui.close_menu();
                                        }
                                    });

                                    ui.monospace(
                                        RichText::new(&entry.internal_title).color(TEXT_MUTED),
                                    );
                                    ui.monospace(RichText::new(&entry.game_code).strong());
                                    ui.label(RichText::new(entry.region).color(match entry.region {
                                        "USA" => Color32::from_rgb(120, 180, 255),
                                        "EUR" => Color32::from_rgb(120, 220, 160),
                                        "JPN" => Color32::from_rgb(255, 140, 140),
                                        _ => TEXT_MUTED,
                                    }));
                                    ui.monospace(format!("{:.1} MB", entry.size_mb));

                                    if entry.has_battery_sav {
                                        ui.label(
                                            RichText::new("✔ .sav")
                                                .color(Color32::from_rgb(100, 220, 120)),
                                        );
                                    } else {
                                        ui.label(RichText::new("—").color(TEXT_MUTED));
                                    }

                                    if ui.button("▶ Play").clicked() {
                                        launch_target = Some(entry.path.clone());
                                    }

                                    ui.end_row();
                                }
                            });
                    });
            }
            LibraryViewMode::Grid => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for entry in &filtered {
                                let is_fav = self.favorites.contains(&entry.path);
                                let star = if is_fav { "★" } else { "☆" };
                                let star_col = if is_fav {
                                    Color32::from_rgb(255, 204, 0)
                                } else {
                                    TEXT_MUTED
                                };

                                let frame = egui::Frame::new()
                                    .fill(BG_CARD)
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        if is_fav {
                                            Color32::from_rgb(160, 140, 50)
                                        } else {
                                            BORDER_HAIRLINE
                                        },
                                    ))
                                    .corner_radius(CornerRadius::same(6))
                                    .inner_margin(12.0);

                                let card_resp = frame.show(ui, |ui| {
                                    ui.set_width(210.0);
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(star).color(star_col),
                                                    )
                                                    .frame(false),
                                                )
                                                .clicked()
                                            {
                                                if is_fav {
                                                    self.favorites.remove(&entry.path);
                                                } else {
                                                    self.favorites.insert(entry.path.clone());
                                                }
                                                self.save_persistent_config();
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&entry.display_name).strong(),
                                                )
                                                .truncate(),
                                            );
                                        });

                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.monospace(
                                                RichText::new(format!("[{}]", entry.game_code))
                                                    .small(),
                                            );
                                            ui.label(RichText::new("•").color(TEXT_MUTED).small());
                                            ui.label(
                                                RichText::new(entry.region)
                                                    .color(match entry.region {
                                                        "USA" => Color32::from_rgb(120, 180, 255),
                                                        "EUR" => Color32::from_rgb(120, 220, 160),
                                                        "JPN" => Color32::from_rgb(255, 140, 140),
                                                        _ => TEXT_MUTED,
                                                    })
                                                    .small(),
                                            );
                                            ui.label(RichText::new("•").color(TEXT_MUTED).small());
                                            ui.monospace(
                                                RichText::new(format!("{:.1} MB", entry.size_mb))
                                                    .small(),
                                            );
                                        });

                                        ui.label(
                                            RichText::new(format!("Header: {}", entry.internal_title))
                                                .small()
                                                .color(TEXT_MUTED),
                                        );

                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            if entry.has_battery_sav {
                                                ui.label(
                                                    RichText::new("✔ .sav")
                                                        .small()
                                                        .color(Color32::from_rgb(100, 220, 120)),
                                                );
                                            } else {
                                                ui.label(RichText::new("—").small().color(TEXT_MUTED));
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if ui.button("▶ Play").clicked() {
                                                        launch_target = Some(entry.path.clone());
                                                    }
                                                },
                                            );
                                        });
                                    });
                                });

                                if card_resp.response.interact(Sense::click()).double_clicked() {
                                    launch_target = Some(entry.path.clone());
                                }
                                card_resp.response.context_menu(|ui| {
                                    if ui.button("▶ Play Game").clicked() {
                                        launch_target = Some(entry.path.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button("📁 Open Save Folder").clicked() {
                                        if let Some(parent) = entry.path.parent() {
                                            #[cfg(target_os = "windows")]
                                            let _ = std::process::Command::new("explorer")
                                                .arg(parent)
                                                .spawn();
                                        }
                                        ui.close_menu();
                                    }
                                    if ui.button("⚙ Configure Game Overrides").clicked() {
                                        launch_target = Some(entry.path.clone());
                                        open_overrides = true;
                                        ui.close_menu();
                                    }
                                    if ui
                                        .button(if is_fav {
                                            "☆ Remove from Favorites"
                                        } else {
                                            "★ Add to Favorites"
                                        })
                                        .clicked()
                                    {
                                        if is_fav {
                                            self.favorites.remove(&entry.path);
                                        } else {
                                            self.favorites.insert(entry.path.clone());
                                        }
                                        self.save_persistent_config();
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button("🗑 Remove Folder from Library").clicked() {
                                        if let Some(parent) = entry.path.parent() {
                                            self.rom_folders.retain(|f| f != parent);
                                            self.save_persistent_config();
                                            rescan_needed = true;
                                        }
                                        ui.close_menu();
                                    }
                                });
                            }
                        });
                    });
            }
        }

        if rescan_needed {
            self.rescan_rom_folders();
        }

        if let Some(path) = launch_target {
            if let Err(e) = self.open_rom(&path, None, Some(ctx)) {
                self.notify(&e);
            }
            if open_overrides {
                self.debugger.show_overrides = true;
            }
        }
    }

    fn draw_active_viewport(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if self.sink.frame_dirty {
            let color_img = self.postproc.process_frame(
                &self.sink.framebuffer,
                self.config.color_correction,
                self.config.lcd_ghosting,
                self.config.scanline_grid,
            );
            let tex = self.lcd_tex.get_or_insert_with(|| {
                ctx.load_texture("gba_lcd", color_img.clone(), TextureOptions::NEAREST)
            });
            tex.set(color_img, TextureOptions::NEAREST);
            self.sink.frame_dirty = false;
        } else if self.lcd_tex.is_none() {
            let blank = self.postproc.process_frame(
                &self.sink.framebuffer,
                self.config.color_correction,
                false,
                false,
            );
            self.lcd_tex = Some(ctx.load_texture("gba_lcd", blank, TextureOptions::NEAREST));
        }

        let avail = ui.available_size();
        let disp_size = if self.config.integer_scaling {
            let factor = ((avail.x / LCD_WIDTH as f32)
                .floor()
                .min((avail.y / LCD_HEIGHT as f32).floor()))
            .max(1.0);
            Vec2::new(LCD_WIDTH as f32 * factor, LCD_HEIGHT as f32 * factor)
        } else {
            let scale = (avail.x / LCD_WIDTH as f32).min(avail.y / LCD_HEIGHT as f32).max(0.5);
            Vec2::new(LCD_WIDTH as f32 * scale, LCD_HEIGHT as f32 * scale)
        };

        ui.centered_and_justified(|ui| {
            if let Some(tex) = &self.lcd_tex {
                let (rect, _) = ui.allocate_exact_size(disp_size, Sense::hover());
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::ZERO,
                    Stroke::new(1.0_f32, BORDER_HAIRLINE),
                    egui::StrokeKind::Outside,
                );
            }
        });
    }
}

impl RustboyApp {
    pub fn update(&mut self, ctx: &egui::Context) {
        if self.title_dirty {
            self.sync_window_title(ctx);
            self.title_dirty = false;
        }
        self.handle_dropped_files(ctx);
        self.handle_global_shortcuts(ctx);
        self.advance_emulation(ctx);

        self.draw_menu_bar(ctx);
        self.draw_toolbar(ctx);
        self.draw_status_bar(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_APP))
            .show(ctx, |ui| {
                if self.core.is_some() {
                    self.draw_active_viewport(ctx, ui);
                } else {
                    self.draw_rom_library_browser(ctx, ui);
                }
            });

        // Present dockable/floating Debugger & Tool windows
        let pads = self.connected_gamepad_names();
        let dbg_act = self.debugger.show_all(
            ctx,
            self.core.as_mut(),
            self.paused,
            self.config.color_correction,
            &mut self.active_cheats,
            &mut self.config.keybindings,
            &pads,
        );
        match dbg_act {
            DebugAction::TogglePause => self.toggle_pause(),
            DebugAction::StepInstruction => self.step_single_instruction(),
            DebugAction::StepScanline => self.step_single_scanline(),
            DebugAction::StepFrame => self.step_single_frame(),
            DebugAction::HardReset => {
                if let Some(core) = &mut self.core {
                    core.reset();
                    self.notify("Hard Reset");
                }
            }
            DebugAction::ConfigChanged => {
                self.save_rom_specific_settings();
                self.config.save();
            }
            DebugAction::None => {}
        }

        // Refresh LCD immediately when Color Correction / Grid / Ghosting toggles while paused
        if self.paused && self.core.is_some() {
            self.sink.frame_dirty = true;
        }
    }

    pub fn on_exit(&mut self) {
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.save_persistent_config();
        self.config.save();
    }
}

pub(crate) fn configure_obsidian_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.extreme_bg_color = BG_APP;
    visuals.faint_bg_color = BG_CARD;
    visuals.window_corner_radius = CornerRadius::same(4);
    visuals.menu_corner_radius = CornerRadius::same(4);
    visuals.window_stroke = Stroke::new(1.0_f32, BORDER_HAIRLINE);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER_HAIRLINE);
    visuals.widgets.inactive.bg_fill = BG_CARD;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER_HAIRLINE);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_TEAL);
    visuals.selection.bg_fill = ACCENT_TEAL_BG;
    visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT_TEAL);
    ctx.set_visuals(visuals);
}

fn main() {
    let cli = match CliConfig::parse() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Argument error: {e}");
            std::process::exit(2);
        }
    };

    if cli.headless {
        if let Err(e) = headless::run_headless(&cli) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let initial_config = AppConfig::load();
    let scale = initial_config.window_scale.clamp(1, 5) as f32;
    let init_w = ((LCD_WIDTH as f32) * scale + 24.0).max(1020.0);
    let init_h = ((LCD_HEIGHT as f32) * scale + 92.0).max(680.0);

    if let Err(err) = app_host::run_desktop("Rustboy Advance", init_w, init_h, RustboyApp::new(cli)) {
        eprintln!("Desktop frontend error: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gba_header_parsing() {
        let mut rom = vec![0u8; 512];
        // Title at 0xA0..0xAC
        let title_bytes = b"POKEMON EMER";
        rom[0xA0..0xAC].copy_from_slice(title_bytes);
        // Game code at 0xAC..0xB0
        let code_bytes = b"BPEE";
        rom[0xAC..0xB0].copy_from_slice(code_bytes);

        let temp_dir = std::env::temp_dir().join("rba_test_header");
        let _ = fs::create_dir_all(&temp_dir);
        let rom_path = temp_dir.join("Pokemon Emerald.gba");
        fs::write(&rom_path, &rom).unwrap();

        let parsed = RustboyApp::parse_gba_header(&rom_path).expect("header parse failed");
        assert_eq!(parsed.display_name, "Pokemon Emerald");
        assert_eq!(parsed.internal_title, "POKEMON EMER");
        assert_eq!(parsed.game_code, "BPEE");
        assert_eq!(parsed.region, "USA");
        assert!(!parsed.has_battery_sav);

        // Test with .sav present
        let sav_path = temp_dir.join("Pokemon Emerald.sav");
        fs::write(&sav_path, b"SAVEDATA").unwrap();
        let parsed_with_sav = RustboyApp::parse_gba_header(&rom_path).unwrap();
        assert!(parsed_with_sav.has_battery_sav);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ips_soft_patching() {
        let temp_dir = std::env::temp_dir().join("rba_test_ips");
        let _ = fs::create_dir_all(&temp_dir);
        let rom_path = temp_dir.join("test_game.gba");
        let ips_path = temp_dir.join("test_game.ips");

        let mut original_rom = vec![0x00; 64];

        // Construct IPS patch: "PATCH", offset 0x000004 (3 bytes), size 0x0003 (2 bytes), data [0xAA, 0xBB, 0xCC],
        // followed by RLE: offset 0x000010 (3 bytes), size 0x0000 (2 bytes), rle_len 0x0004 (2 bytes), val 0x77,
        // then "EOF"
        let mut patch = Vec::new();
        patch.extend_from_slice(b"PATCH");
        // Standard chunk: offset 4, len 3
        patch.extend_from_slice(&[0x00, 0x00, 0x04]);
        patch.extend_from_slice(&[0x00, 0x03]);
        patch.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        // RLE chunk: offset 16, len 0 -> rle_len 4, val 0x77
        patch.extend_from_slice(&[0x00, 0x00, 0x10]);
        patch.extend_from_slice(&[0x00, 0x00]);
        patch.extend_from_slice(&[0x00, 0x04, 0x77]);
        // Terminator
        patch.extend_from_slice(b"EOF");

        fs::write(&ips_path, &patch).unwrap();

        let applied = RustboyApp::apply_ips_patch_if_present(&rom_path, &mut original_rom);
        assert!(applied);
        assert_eq!(&original_rom[4..7], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(&original_rom[16..20], &[0x77, 0x77, 0x77, 0x77]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_rewind_buffer_limit() {
        let mut buffer: VecDeque<Vec<u8>> = VecDeque::with_capacity(60);
        for i in 0..80 {
            if buffer.len() >= 60 {
                buffer.pop_front();
            }
            buffer.push_back(vec![i as u8]);
        }
        assert_eq!(buffer.len(), 60);
        assert_eq!(buffer.front(), Some(&vec![20u8]));
        assert_eq!(buffer.back(), Some(&vec![79u8]));

        let popped = buffer.pop_back();
        assert_eq!(popped, Some(vec![79u8]));
        assert_eq!(buffer.len(), 59);
    }
}
