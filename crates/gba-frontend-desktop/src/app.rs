use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{self, Color32, CornerRadius, Stroke, TextureHandle, Vec2};
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gilrs::Gilrs;

use crate::audio::AudioOutput;
use crate::cheats::{apply_cheats_to_core, CheatEntry};
use crate::config::{AppConfig, SaveOverrideChoice};
use crate::debugger_ui::{DebugAction, DebuggerWindows};
use crate::filters::VideoPostProcessor;
use crate::headless::CliConfig;
use crate::library::{LibraryViewMode, RomLibraryEntry};
use crate::rewind::RewindBuffer;
use crate::savestates::SlotMetaEntry;

pub(crate) const CONFIG_FILE: &str = "rustboy_library.cfg";
pub(crate) const GBA_FRAME_SECS: f64 = 280_896.0 / 16_777_216.0; // Exact GBA refresh ~16.7427ms (~59.7275 Hz)
pub(crate) const BG_APP: Color32 = Color32::from_rgb(13, 15, 19);
pub(crate) const BG_PANEL: Color32 = Color32::from_rgb(20, 23, 30);
pub(crate) const BG_CARD: Color32 = Color32::from_rgb(25, 29, 38);
pub(crate) const BORDER_HAIRLINE: Color32 = Color32::from_rgb(42, 48, 61);
pub(crate) const ACCENT_TEAL: Color32 = Color32::from_rgb(46, 196, 182);
pub(crate) const ACCENT_TEAL_BG: Color32 = Color32::from_rgb(26, 62, 62);
pub(crate) const TEXT_MUTED: Color32 = Color32::from_rgb(142, 152, 170);

pub(crate) struct CoreSink {
    pub(crate) audio: AudioOutput,
    pub(crate) framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub(crate) keypad: u16,
    pub(crate) frame_dirty: bool,
    pub(crate) suppress_audio: bool,
}

impl CoreSink {
    pub(crate) fn new(audio: AudioOutput) -> Self {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToastLevel {
    Info,
    Warning,
    Error,
}

pub(crate) struct ToastMessage {
    pub(crate) text: String,
    pub(crate) level: ToastLevel,
    pub(crate) expires_at: Instant,
}

pub(crate) struct RustboyApp {
    pub(crate) config: AppConfig,
    pub(crate) core: Option<GbaCore>,
    pub(crate) sink: CoreSink,
    pub(crate) postproc: VideoPostProcessor,
    pub(crate) lcd_tex: Option<TextureHandle>,
    pub(crate) debugger: DebuggerWindows,
    pub(crate) gilrs: Option<Gilrs>,

    pub(crate) rom_path: Option<PathBuf>,
    pub(crate) sav_path: Option<PathBuf>,
    pub(crate) rom_title: String,
    pub(crate) rom_code: String,
    pub(crate) rom_crc32: u32,
    pub(crate) bios_bytes: Option<Vec<u8>>,
    pub(crate) active_cheats: Vec<CheatEntry>,

    // ROM Library & Folder Scanner
    pub(crate) rom_folders: Vec<PathBuf>,
    pub(crate) library_entries: Vec<RomLibraryEntry>,
    pub(crate) favorites: HashSet<PathBuf>,
    pub(crate) search_query: String,
    pub(crate) filter_favorites_only: bool,
    pub(crate) library_view_mode: LibraryViewMode,

    // Real-Time Rewind Buffer
    pub(crate) rewind_buffer: RewindBuffer,
    pub(crate) frame_counter: u64,
    pub(crate) rewinding: bool,
    pub(crate) show_rewind_settings: bool,

    pub(crate) paused: bool,
    pub(crate) tab_fast_forward: bool,
    pub(crate) is_fullscreen: bool,
    pub(crate) last_pressed_mask: u16,
    pub(crate) next_frame_due: Instant,
    pub(crate) last_save_flush: Instant,
    pub(crate) last_fps_sample: Instant,
    pub(crate) frames_in_window: u32,
    pub(crate) measured_fps: f32,
    pub(crate) last_core_ms: f32,
    pub(crate) toast: Option<ToastMessage>,
    pub(crate) title_dirty: bool,
    pub(crate) quit_requested: bool,

    // Immersive Controls, Overlays & States
    pub(crate) last_mouse_activity: Instant,
    pub(crate) show_pause_overlay: bool,
    pub(crate) show_state_manager: bool,
    pub(crate) undo_state_buffer: Option<Vec<u8>>,
    pub(crate) slot_thumbnails: [Option<TextureHandle>; 5],
    pub(crate) slot_meta: [Option<SlotMetaEntry>; 5],
    pub(crate) state_manager_error: Option<String>,
    pub(crate) playtime_timer: Instant,
    pub(crate) rumble_in_progress: bool,
}

impl RustboyApp {
    pub(crate) fn new(cli: CliConfig) -> Self {
        let config = AppConfig::load();
        let rewind_budget_mb = config.rewind_budget_mb;
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
            rom_crc32: 0,
            bios_bytes,
            active_cheats: Vec::new(),
            rom_folders: Vec::new(),
            library_entries: Vec::new(),
            favorites: HashSet::new(),
            search_query: String::new(),
            filter_favorites_only: false,
            library_view_mode: LibraryViewMode::Table,
            rewind_buffer: RewindBuffer::new(rewind_budget_mb),
            frame_counter: 0,
            rewinding: false,
            show_rewind_settings: false,
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
            last_mouse_activity: Instant::now(),
            show_pause_overlay: false,
            show_state_manager: false,
            undo_state_buffer: None,
            slot_thumbnails: [None, None, None, None, None],
            slot_meta: [None, None, None, None, None],
            state_manager_error: None,
            playtime_timer: Instant::now(),
            rumble_in_progress: false,
        };

        app.load_persistent_config();
        app.rescan_rom_folders();

        if let Some(rom_path) = cli.rom_path {
            if let Err(e) = app.open_rom(&rom_path, cli.sav_path.as_deref(), None) {
                app.notify_error(&format!("Failed to load CLI ROM: {e}"));
            }
        }
        app
    }

    pub(crate) fn notify(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Info,
            expires_at: Instant::now() + Duration::from_millis(3200),
        });
    }

    #[allow(dead_code)]
    pub(crate) fn notify_warn(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Warning,
            expires_at: Instant::now() + Duration::from_millis(3800),
        });
    }

    pub(crate) fn notify_error(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Error,
            expires_at: Instant::now() + Duration::from_millis(4500),
        });
    }

    pub(crate) fn sync_window_title(&self, ctx: &egui::Context) {
        let title = if self.core.is_some() {
            format!("Rustboy Advance — {} ({})", self.rom_title, self.rom_code)
        } else {
            "Rustboy Advance".to_string()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    pub(crate) fn save_persistent_config(&mut self) {
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

    pub(crate) fn load_persistent_config(&mut self) {
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

    pub(crate) fn open_rom(
        &mut self,
        rom_path: &Path,
        custom_sav: Option<&Path>,
        ctx: Option<&egui::Context>,
    ) -> Result<(), String> {
        self.flush_battery_save();
        self.save_rom_specific_settings();

        let mut rom_bytes = fs::read(rom_path).map_err(|e| format!("Failed reading ROM: {e}"))?;
        let ips_applied = Self::apply_ips_patch_if_present(rom_path, &mut rom_bytes);
        let rom_crc32 = crc32fast::hash(&rom_bytes);

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
            if let Some(cp) = ov.custom_color_profile {
                self.config.color_profile = cp;
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
        self.rom_crc32 = rom_crc32;
        self.paused = false;
        self.show_pause_overlay = false;
        self.show_state_manager = false;
        self.rewind_buffer.set_budget_mb(self.config.rewind_budget_mb);
        self.rewind_buffer.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.next_frame_due = Instant::now();
        self.frames_in_window = 0;
        self.last_fps_sample = Instant::now();
        self.postproc.prev_rgba.fill(0);
        self.sink.framebuffer.fill(0);
        self.sink.frame_dirty = true;
        self.slot_thumbnails = [None, None, None, None, None];
        self.slot_meta = [None, None, None, None, None];
        self.state_manager_error = None;
        self.playtime_timer = Instant::now();

        // Track per-game launch count and last played timestamp
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let pt = self.config.playtimes.entry(code.clone()).or_default();
        pt.launch_count += 1;
        pt.last_played_timestamp = now_ts;

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

    pub(crate) fn close_rom(&mut self, ctx: &egui::Context) {
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.core = None;
        self.rom_path = None;
        self.sav_path = None;
        self.rom_title.clear();
        self.rom_code.clear();
        self.rom_crc32 = 0;
        self.active_cheats.clear();
        self.slot_thumbnails = [None, None, None, None, None];
        self.slot_meta = [None, None, None, None, None];
        self.state_manager_error = None;
        self.paused = false;
        self.show_pause_overlay = false;
        self.show_state_manager = false;
        self.rewind_buffer.set_budget_mb(self.config.rewind_budget_mb);
        self.rewind_buffer.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.lcd_tex = None;
        self.rescan_rom_folders();
        self.sync_window_title(ctx);
        self.notify("Game stopped — Returned to ROM Library");
    }

    pub(crate) fn save_rom_specific_settings(&mut self) {
        let Some(core) = &self.core else { return };
        let key = self.rom_code.clone();
        if key.is_empty() {
            return;
        }
        let mut ov = self.config.per_rom.get(&key).cloned().unwrap_or_default();
        ov.save_override = Some(SaveOverrideChoice::from_core(
            core.bus.cart.save_type_override,
        ));
        ov.rtc_override = core.bus.cart.rtc_override;
        ov.solar_lux = Some(core.bus.cart.solar_lux);
        ov.cheats = self.active_cheats.clone();
        ov.custom_color_profile = Some(self.config.color_profile);
        self.config.per_rom.insert(key, ov);
        self.config.save();
    }

    pub(crate) fn flush_battery_save(&mut self) {
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

    pub(crate) fn load_custom_bios_dialog(&mut self) {
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

    pub(crate) fn clear_custom_bios(&mut self) {
        self.bios_bytes = None;
        self.config.bios_path = None;
        self.config.save();
        if let Some(core) = &mut self.core {
            core.load_bios(None);
        }
        self.notify("Switched back to built-in HLE BIOS");
    }

    pub(crate) fn import_battery_save(&mut self) {
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

    pub(crate) fn export_battery_save(&mut self) {
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

    pub(crate) fn capture_screenshot(&mut self) {
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

    pub(crate) fn set_window_scale(&mut self, ctx: &egui::Context, scale: u8) {
        self.config.window_scale = scale.clamp(1, 5);
        self.config.save();
        let w = (LCD_WIDTH as f32) * (self.config.window_scale as f32) + 24.0;
        let h = (LCD_HEIGHT as f32) * (self.config.window_scale as f32) + 92.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        self.is_fullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(w, h)));
    }

    pub(crate) fn soft_reset(&mut self) {
        if let Some(core) = &mut self.core {
            core.soft_reset();
            self.rewind_buffer.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.next_frame_due = Instant::now();
            self.notify("Soft Reset");
        }
    }

    pub(crate) fn hard_reset(&mut self) {
        if let Some(core) = &mut self.core {
            core.reset();
            self.rewind_buffer.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.sink.framebuffer.fill(0);
            self.sink.frame_dirty = true;
            self.next_frame_due = Instant::now();
            self.notify("Hard Reset");
        }
    }

    pub(crate) fn toggle_pause(&mut self) {
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

    pub(crate) fn step_single_frame(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        if self.config.rewind_enabled {
            let interval = self.config.rewind_interval.clamp(1, 4) as u64;
            if self.frame_counter.is_multiple_of(interval)
                && let Ok(state) = core.save_state()
            {
                self.rewind_buffer.push(state);
            }
        }
        apply_cheats_to_core(core, &self.active_cheats);
        let _ = core.run_frame_with_breakpoints(&mut self.sink, &[]);
        self.frame_counter += 1;
        self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
        self.sink.frame_dirty = true;
    }

    pub(crate) fn step_single_instruction(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        core.step_one();
        if core.bus.ppu.frame_ready {
            self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
            self.sink.frame_dirty = true;
        }
    }

    pub(crate) fn step_single_scanline(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        core.step_scanline();
        if core.bus.ppu.frame_ready {
            self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
            self.sink.frame_dirty = true;
        }
    }

    pub(crate) fn trigger_open_rom_dialog(&mut self, ctx: &egui::Context) {
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

    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
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

    pub(crate) fn advance_emulation(&mut self, ctx: &egui::Context) {
        if self.core.is_none() {
            return;
        }

        // Handle active real-time rewind paced at GBA frame cadence
        if self.rewinding {
            self.sink.suppress_audio = true;
            let now = Instant::now();
            if now >= self.next_frame_due {
                self.step_rewind_frame();
                self.next_frame_due = now + Duration::from_secs_f64(GBA_FRAME_SECS);
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

            // Capture rewind snapshot before advancing frame so rewind restores exact pre-frame state
            if self.config.rewind_enabled {
                let interval = self.config.rewind_interval.clamp(1, 4) as u64;
                if self.frame_counter.is_multiple_of(interval)
                    && let Ok(state) = core.save_state()
                {
                    self.rewind_buffer.push(state);
                }
            }

            self.sink.suppress_audio = turbo && executed > 0;
            apply_cheats_to_core(core, &self.active_cheats);

            let bp_hit = core.run_frame_with_breakpoints(&mut self.sink, &self.debugger.breakpoints);
            self.last_core_ms = start_t.elapsed().as_secs_f32() * 1000.0;
            executed += 1;
            self.frames_in_window += 1;
            self.frame_counter += 1;

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

        // Track per-game active gameplay time
        if self.core.is_some() && !self.paused {
            if self.playtime_timer.elapsed() >= Duration::from_secs(1) {
                if !self.rom_code.is_empty() {
                    let entry = self.config.playtimes.entry(self.rom_code.clone()).or_default();
                    entry.total_seconds += self.playtime_timer.elapsed().as_secs();
                }
                self.playtime_timer = Instant::now();
            }
        }

        if self.config.rumble_enabled {
            if let Some(core) = &self.core {
                self.rumble_in_progress = core.bus.cart.rumble_active;
            }
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

        // Auto-hide mouse cursor after 2.2s of inactivity during gameplay
        let mouse_moved = ctx.input(|i| i.pointer.delta() != Vec2::ZERO);
        let now = Instant::now();
        if mouse_moved {
            self.last_mouse_activity = now;
        }
        let pointer_y = ctx.input(|i| i.pointer.latest_pos().map(|p| p.y).unwrap_or(999.0));
        let hover_near_top = pointer_y <= 36.0;

        let cursor_should_hide = self.core.is_some()
            && !self.paused
            && !self.show_pause_overlay
            && !self.show_state_manager
            && now.duration_since(self.last_mouse_activity) > Duration::from_millis(2200);

        if cursor_should_hide {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        // Fullscreen Chrome Auto-Hide:
        // In fullscreen:
        //   - Status bar is completely hidden (telemetry rendered via floating OSD pill)
        //   - Top menu & toolbar slide into view when cursor hovers near top edge
        // In windowed mode:
        //   - Toggled via Ctrl+T / Ctrl+S / Ctrl+M
        if self.is_fullscreen {
            if hover_near_top {
                self.draw_menu_bar(ctx);
                if self.config.show_toolbar {
                    self.draw_toolbar(ctx);
                }
            }
        } else {
            if self.config.show_menu_bar {
                self.draw_menu_bar(ctx);
            }
            if self.config.show_toolbar {
                self.draw_toolbar(ctx);
            }
            if self.config.show_status_bar {
                self.draw_status_bar(ctx);
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_APP))
            .show(ctx, |ui| {
                if self.core.is_some() {
                    self.draw_active_viewport(ctx, ui);
                } else {
                    self.draw_rom_library_browser(ctx, ui);
                }
            });

        // Always render floating OSD toast on top so feedback is never lost
        self.draw_floating_osd_toast(ctx);

        // Visual Save State Manager & Rewind Settings dialogs
        self.draw_visual_state_manager(ctx);
        self.draw_rewind_settings_dialog(ctx);

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
            DebugAction::HardReset => self.hard_reset(),
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
