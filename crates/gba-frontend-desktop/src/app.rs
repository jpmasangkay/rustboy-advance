use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{self, TextureHandle, Vec2};
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gilrs::Gilrs;

use crate::audio::AudioOutput;
use crate::capture::CaptureController;
use crate::cheats::{CheatEntry, apply_cheats_to_core};
use crate::config::{AppConfig, DisplayFilter, SaveOverrideChoice};
use crate::debugger_ui::{DebugAction, DebuggerWindows};
use crate::filters::{QualityGovernor, VideoPostProcessor};
use crate::game_settings::{
    CartridgeHardwareDefaults, GameSettingsTarget, load_effective_settings,
    save_game_settings_entry,
};
use crate::gb_bridge::{GbSession, blit_gb_to_gba_fb, crop_gb_rgba_from_240x160};
use crate::headless::CliConfig;
use crate::input::{AutofireState, ControlsTab, PendingInputBind};
use crate::library::{LibraryScanResult, LibraryViewMode, RomLibraryEntry};
use crate::link::LinkManager;
use crate::movies::MovieController;
use crate::rewind::RewindBuffer;
use crate::rom_loader::{
    RomPlatform, apply_auto_patch_if_present, apply_patch_file, detect_rom_platform,
    is_patch_extension, load_rom_bytes,
};
use crate::savestates::SlotMetaEntry;
use crate::shaders::WgpuShaderPipeline;

pub(crate) const CONFIG_FILE: &str = "rustboy_library.cfg";
pub(crate) const GBA_FRAME_SECS: f64 = 280_896.0 / 16_777_216.0; // Exact GBA refresh ~16.7427ms (~59.7275 Hz)
#[allow(unused_imports)]
pub(crate) use crate::theme::*;

pub(crate) struct CoreSink {
    pub(crate) audio: AudioOutput,
    pub(crate) framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub(crate) prev_framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub(crate) has_prev_frame: bool,
    pub(crate) frame_seq: u64,
    pub(crate) keypad: u16,
    pub(crate) frame_dirty: bool,
    pub(crate) suppress_audio: bool,
    pub(crate) capture_audio: bool,
    pub(crate) captured_audio: Vec<i16>,
}

impl CoreSink {
    pub(crate) fn new(audio: AudioOutput) -> Self {
        Self {
            audio,
            framebuffer: [0; LCD_WIDTH * LCD_HEIGHT],
            prev_framebuffer: [0; LCD_WIDTH * LCD_HEIGHT],
            has_prev_frame: false,
            frame_seq: 0,
            keypad: 0x03FF,
            frame_dirty: false,
            suppress_audio: false,
            capture_audio: false,
            captured_audio: Vec::new(),
        }
    }

    pub(crate) fn push_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.prev_framebuffer.copy_from_slice(&self.framebuffer);
        self.framebuffer.copy_from_slice(fb);
        self.has_prev_frame = true;
        self.frame_seq = self.frame_seq.wrapping_add(1);
        self.frame_dirty = true;
    }

    pub(crate) fn clear_frames(&mut self) {
        self.framebuffer.fill(0);
        self.prev_framebuffer.fill(0);
        self.has_prev_frame = false;
        self.frame_seq = self.frame_seq.wrapping_add(1);
        self.frame_dirty = true;
    }
}

impl HostFrontend for CoreSink {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.push_frame(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        if !self.suppress_audio {
            self.audio.push_interleaved(samples);
        }
        if self.capture_audio {
            self.captured_audio.extend_from_slice(samples);
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
    // WAVE-2 HOOK: Core emulator handle (`core.bus.*` memory, PPU, APU, Link/SIO)
    // shared with Debugger, Lua scripting, RetroAchievements, and Netplay/Link Cable.
    pub(crate) core: Option<GbaCore>,
    pub(crate) gb: Option<GbSession>,
    pub(crate) link: LinkManager,
    pub(crate) sink: CoreSink,
    pub(crate) postproc: VideoPostProcessor,
    pub(crate) lcd_tex: Option<TextureHandle>,
    pub(crate) wgpu_shaders: Option<Arc<Mutex<WgpuShaderPipeline>>>,
    pub(crate) debugger: DebuggerWindows,
    pub(crate) gdb_stub: gba_debugger::GdbStub,
    pub(crate) temp_step_bp: Option<u32>,
    pub(crate) temp_step_target_depth: Option<usize>,
    pub(crate) gilrs: Option<Gilrs>,

    pub(crate) rom_path: Option<PathBuf>,
    pub(crate) sav_path: Option<PathBuf>,
    pub(crate) rom_title: String,
    pub(crate) rom_code: String,
    pub(crate) rom_crc32: u32,
    pub(crate) bios_bytes: Option<Vec<u8>>,
    pub(crate) active_cheats: Vec<CheatEntry>,
    pub(crate) cart_hw_defaults: CartridgeHardwareDefaults,
    pub(crate) session_speed_override: Option<f32>,
    pub(crate) show_game_settings: bool,
    pub(crate) game_settings_target: Option<GameSettingsTarget>,

    // ROM Library & Folder Scanner
    pub(crate) rom_folders: Vec<PathBuf>,
    pub(crate) library_entries: Vec<RomLibraryEntry>,
    pub(crate) library_scanning: bool,
    pub(crate) library_scan_rx: Option<std::sync::mpsc::Receiver<LibraryScanResult>>,
    pub(crate) library_scan_error: Option<String>,
    pub(crate) favorites: HashSet<PathBuf>,
    pub(crate) search_query: String,
    pub(crate) filter_favorites_only: bool,
    pub(crate) library_view_mode: LibraryViewMode,
    pub(crate) selected_rom_path: Option<PathBuf>,
    pub(crate) table_sort_column: crate::library::TableSortColumn,
    pub(crate) table_sort_descending: bool,
    pub(crate) cover_textures: HashMap<PathBuf, TextureHandle>,
    pub(crate) pending_cover_downloads: HashSet<PathBuf>,

    // Real-Time Rewind Buffer
    pub(crate) rewind_buffer: RewindBuffer,
    pub(crate) frame_counter: u64,
    pub(crate) rewinding: bool,
    pub(crate) show_rewind_settings: bool,

    pub(crate) paused: bool,
    pub(crate) tab_fast_forward: bool,
    pub(crate) is_fullscreen: bool,
    pub(crate) last_pressed_mask: u16,
    pub(crate) show_controls_dialog: bool,
    pub(crate) controls_tab: ControlsTab,
    pub(crate) awaiting_input_bind: Option<PendingInputBind>,
    pub(crate) autofire_state: AutofireState,
    pub(crate) base_pressed_mask: u16,
    pub(crate) active_turbo_buttons: (bool, bool),
    pub(crate) next_frame_due: Instant,
    pub(crate) last_step_time: Instant,
    pub(crate) time_accumulator: f64,
    pub(crate) upload_arcs: [std::sync::Arc<egui::ColorImage>; 3],
    pub(crate) upload_arc_idx: usize,
    pub(crate) host_audio_rate: u32,
    pub(crate) last_save_flush: Instant,
    pub(crate) last_fps_sample: Instant,
    pub(crate) frames_in_window: u32,
    pub(crate) measured_fps: f32,
    pub(crate) last_core_ms: f32,
    pub(crate) last_present_ms: f32,
    pub(crate) quality_governor: QualityGovernor,
    pub(crate) governor_adapter_name: String,
    pub(crate) governor_adapter_backend: String,
    pub(crate) governor_viewport_px: (u32, u32),
    pub(crate) governor_user_target: DisplayFilter,
    pub(crate) governor_cli_filter: bool,
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
    pub(crate) gilrs_rumble_effect: Option<gilrs::ff::Effect>,

    // TAS Input Movies & Media Capture
    pub(crate) movie: MovieController,
    pub(crate) capture: CaptureController,
    pub(crate) show_movies: bool,
    pub(crate) rewind_movie_frames: VecDeque<usize>,

    // Lua Scripting & RetroAchievements
    pub(crate) lua: crate::lua_engine::LuaEngine,
    pub(crate) ra: crate::retroachievements::RetroAchievementsManager,
    pub(crate) emu_error_rx: Option<std::sync::mpsc::Receiver<String>>,

    // Real-Window Perf Telemetry (--perf-log, --perf-seconds, --input-replay, --sync-60hz)
    pub(crate) perf_log_path: Option<PathBuf>,
    pub(crate) perf_logger: Option<crate::bench::DesktopPerfLogger>,
    pub(crate) perf_seconds: Option<f64>,
    pub(crate) perf_start_wall: Instant,
    pub(crate) cli_replay: Option<crate::bench::InputReplay>,
    pub(crate) cli_loop_replay: bool,
    pub(crate) cli_replay_frame: usize,
    pub(crate) sync_60hz: bool,
    pub(crate) display_refresh_hz: Option<f64>,
    pub(crate) pacing_deadline: Option<Instant>,
    pub(crate) last_step_frames: u32,
    pub(crate) last_step_emu_ms: f64,
}

impl RustboyApp {
    pub(crate) fn new(cli: CliConfig) -> Self {
        let mut config = AppConfig::load();
        if let Some(gov_flag) = cli.governor {
            config.auto_quality_governor = gov_flag;
        }
        let cli_filter_set = cli.filter.is_some();
        if let Some(f) = cli.filter {
            config.display_filter = f;
            config.shader_preset = f.default_shader_preset();
            config.scanline_grid = f == DisplayFilter::AuthenticGbaLcd;
        }
        let cli_replay = cli
            .input_replay
            .as_deref()
            .and_then(|p| match crate::bench::InputReplay::load(p) {
                Ok(r) => Some(r),
                Err(e) => {
                    log::error!("Failed to load CLI --input-replay: {e}");
                    None
                }
            });
        let cli_perf_log_path = cli.perf_log;
        let cli_perf_seconds = cli.perf_seconds;
        let cli_loop_replay = cli.loop_replay;
        let sync_decision =
            crate::display_timing::resolve_sync_decision(cli.sync_60hz, cli.simulate_refresh_hz);
        println!("[sync] {}", sync_decision.reason);
        let cli_sync_60hz = sync_decision.sync_60hz;
        let cli_display_refresh_hz = sync_decision.effective_refresh_hz;
        let cli_fullscreen = cli.fullscreen;
        let rewind_budget_mb = config.rewind_budget_mb;
        let audio = AudioOutput::start();
        audio.set_muted(config.muted);
        let host_audio_rate = audio.host_sample_rate();
        let sink = CoreSink::new(audio);
        let ra = crate::retroachievements::RetroAchievementsManager::new(&config);

        let blank_img = std::sync::Arc::new(egui::ColorImage::new(
            [LCD_WIDTH * 3, LCD_HEIGHT * 3],
            egui::Color32::BLACK,
        ));

        let bios_bytes = cli
            .bios_path
            .as_deref()
            .and_then(|p| fs::read(p).ok())
            .or_else(|| config.bios_path.as_deref().and_then(|p| fs::read(p).ok()));

        let scale_u32 = config.window_scale.clamp(1, 5) as u32;
        let governor_user_target = config.display_filter;
        let quality_governor =
            QualityGovernor::new(config.display_filter, config.auto_quality_governor);
        let mut app = Self {
            config,
            core: None,
            gb: None,
            link: LinkManager::new(),
            sink,
            postproc: VideoPostProcessor::new(),
            lcd_tex: None,
            wgpu_shaders: None,
            debugger: DebuggerWindows::default(),
            gdb_stub: gba_debugger::GdbStub::new(),
            temp_step_bp: None,
            temp_step_target_depth: None,
            gilrs: Gilrs::new().ok(),
            rom_path: None,
            sav_path: None,
            rom_title: String::new(),
            rom_code: String::new(),
            rom_crc32: 0,
            bios_bytes,
            active_cheats: Vec::new(),
            cart_hw_defaults: CartridgeHardwareDefaults::default(),
            session_speed_override: None,
            show_game_settings: false,
            game_settings_target: None,
            rom_folders: Vec::new(),
            library_entries: Vec::new(),
            library_scanning: false,
            library_scan_rx: None,
            library_scan_error: None,
            favorites: HashSet::new(),
            search_query: String::new(),
            filter_favorites_only: false,
            library_view_mode: LibraryViewMode::Table,
            selected_rom_path: None,
            table_sort_column: crate::library::TableSortColumn::Title,
            table_sort_descending: false,
            cover_textures: HashMap::new(),
            pending_cover_downloads: HashSet::new(),
            rewind_buffer: RewindBuffer::new(rewind_budget_mb),
            frame_counter: 0,
            rewinding: false,
            show_rewind_settings: false,
            paused: false,
            tab_fast_forward: false,
            is_fullscreen: cli_fullscreen,
            last_pressed_mask: 0,
            show_controls_dialog: false,
            controls_tab: ControlsTab::Keyboard,
            awaiting_input_bind: None,
            autofire_state: AutofireState::default(),
            base_pressed_mask: 0,
            active_turbo_buttons: (false, false),
            next_frame_due: Instant::now(),
            last_step_time: Instant::now(),
            time_accumulator: 0.0,
            upload_arcs: [blank_img.clone(), blank_img.clone(), blank_img],
            upload_arc_idx: 0,
            host_audio_rate,
            last_save_flush: Instant::now(),
            last_fps_sample: Instant::now(),
            frames_in_window: 0,
            measured_fps: 0.0,
            last_core_ms: 0.0,
            last_present_ms: 0.0,
            quality_governor,
            governor_adapter_name: String::from("Unknown"),
            governor_adapter_backend: String::from("Unknown"),
            governor_viewport_px: (LCD_WIDTH as u32 * scale_u32, LCD_HEIGHT as u32 * scale_u32),
            governor_user_target,
            governor_cli_filter: cli_filter_set,
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
            gilrs_rumble_effect: None,
            movie: MovieController::new(),
            capture: CaptureController::new(),
            show_movies: false,
            rewind_movie_frames: VecDeque::new(),
            lua: crate::lua_engine::LuaEngine::new(),
            ra,
            emu_error_rx: None,
            perf_log_path: cli_perf_log_path,
            perf_logger: None,
            perf_seconds: cli_perf_seconds,
            perf_start_wall: Instant::now(),
            cli_replay,
            cli_loop_replay,
            cli_replay_frame: 0,
            sync_60hz: cli_sync_60hz,
            display_refresh_hz: cli_display_refresh_hz,
            pacing_deadline: None,
            last_step_frames: 0,
            last_step_emu_ms: 0.0,
        };

        app.load_persistent_config();
        app.config.run_ahead_frames = app.config.run_ahead_frames.min(1);
        let measured_costs = app.postproc.measure_filter_costs(
            app.config.color_correction,
            app.config.lcd_ghosting,
            false,
            app.config.lcd_grid_intensity,
        );
        app.quality_governor.set_measured_costs(measured_costs);
        app.rescan_rom_folders();

        if let Some(rom_path) = cli.rom_path
            && let Err(e) = app.open_rom(&rom_path, cli.sav_path.as_deref(), None)
        {
            app.notify_error(&format!("Failed to load CLI ROM: {e}"));
        }
        app
    }

    pub(crate) fn is_ra_hardcore_active(&self) -> bool {
        self.ra.is_hardcore_active(&self.config)
    }

    pub(crate) fn notify(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Info,
            expires_at: Instant::now() + Duration::from_millis(3200),
        });
    }

    pub(crate) fn notify_warn(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Warning,
            expires_at: Instant::now() + Duration::from_millis(3800),
        });
    }

    pub(crate) fn current_governor_profile_key(&self) -> String {
        AppConfig::governor_profile_key(
            &self.governor_adapter_name,
            &self.governor_adapter_backend,
            self.governor_viewport_px.0,
            self.governor_viewport_px.1,
            self.config.window_scale,
        )
    }

    pub(crate) fn set_display_filter_from_user(&mut self, filter: DisplayFilter) {
        self.config.display_filter = filter;
        self.config.shader_preset = filter.default_shader_preset();
        self.config.scanline_grid = filter == DisplayFilter::AuthenticGbaLcd;
        self.governor_user_target = filter;
        self.quality_governor.set_user_target(filter);
        let key = self.current_governor_profile_key();
        self.config.set_settled_filter(key, filter);
        self.config.save();
        self.sink.frame_dirty = true;
        self.lcd_tex = None;
    }

    pub(crate) fn apply_quality_governor_sample(&mut self, work_ms: f64) {
        self.quality_governor
            .set_enabled(self.config.auto_quality_governor);
        self.quality_governor
            .sync_external_filter(self.config.display_filter);
        if let Some(tr) = self.quality_governor.record_frame(work_ms) {
            self.config.display_filter = tr.to;
            self.config.shader_preset = tr.to.default_shader_preset();
            self.config.scanline_grid = tr.to == DisplayFilter::AuthenticGbaLcd;
            let key = self.current_governor_profile_key();
            self.config.set_settled_filter(key, tr.to);
            self.config.save();
            self.sink.frame_dirty = true;
            self.lcd_tex = None;
            let verb = match tr.kind {
                crate::filters::GovernorTransitionKind::StepDown => "switched",
                crate::filters::GovernorTransitionKind::StepUp => "recovered",
                crate::filters::GovernorTransitionKind::CapResolution => "capped output to 720x480",
                crate::filters::GovernorTransitionKind::UncapResolution => "uncapped output resolution",
            };
            self.notify_warn(&format!(
                "Quality Governor: {verb} {} -> {} (p95 {:.1} ms)",
                tr.from.label(),
                tr.to.label(),
                tr.rolling_p95_ms
            ));
        }
    }

    pub(crate) fn notify_error(&mut self, msg: &str) {
        self.toast = Some(ToastMessage {
            text: msg.to_string(),
            level: ToastLevel::Error,
            expires_at: Instant::now() + Duration::from_millis(4500),
        });
    }

    pub(crate) fn sync_window_title(&self, ctx: &egui::Context) {
        let title = if self.has_active_game() {
            format!(
                "Rustboy Advance — {} [{}] ({})",
                self.rom_title,
                self.active_platform_badge(),
                self.rom_code
            )
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
                    if let Ok(b) = fs::read(&path)
                        && b.len() >= 0x4000
                    {
                        self.bios_bytes = Some(b[..0x4000].to_vec());
                        self.config.bios_path = Some(p.to_string());
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
        self.open_rom_with_patch(rom_path, custom_sav, None, ctx)
    }

    pub(crate) fn open_rom_with_patch(
        &mut self,
        rom_path: &Path,
        custom_sav: Option<&Path>,
        explicit_patch: Option<&Path>,
        ctx: Option<&egui::Context>,
    ) -> Result<(), String> {
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.stop_all_captures(true);
        self.movie.reset_for_rom_change();
        self.link.disconnect_all();

        let mut loaded = load_rom_bytes(rom_path)?;
        let patch_applied = if let Some(patch_path) = explicit_patch {
            Some(apply_patch_file(patch_path, &mut loaded.bytes)?)
        } else {
            apply_auto_patch_if_present(rom_path, &mut loaded.bytes)?
        };
        let rom_bytes = loaded.bytes;
        let archive_badge = loaded.source_kind.badge();
        let archive_inner = loaded.inner_name;

        let sav_path = custom_sav
            .map(PathBuf::from)
            .unwrap_or_else(|| rom_path.with_extension("sav"));

        let platform = detect_rom_platform(rom_path, archive_inner.as_deref(), &rom_bytes);
        if matches!(platform, RomPlatform::Gb | RomPlatform::Gbc) {
            self.open_gb_rom_loaded(
                rom_path,
                &sav_path,
                rom_bytes,
                archive_badge,
                archive_inner,
                patch_applied,
                ctx,
            );
            return Ok(());
        }

        self.gb = None;
        let rom_crc32 = crc32fast::hash(&rom_bytes);
        let mut core = GbaCore::new(self.bios_bytes.as_deref(), rom_bytes.clone());
        if let Ok(sav) = fs::read(&sav_path) {
            core.bus.cart.load_save_data(&sav);
        }

        let title = core.bus.cart.game_title();
        let code = core.bus.cart.game_code();
        self.cart_hw_defaults = CartridgeHardwareDefaults {
            solar_enabled: core.bus.cart.solar_enabled,
            tilt_enabled: core.bus.cart.tilt_enabled,
            gyro_enabled: core.bus.cart.gyro_enabled,
            rumble_enabled: core.bus.cart.rumble_enabled,
        };

        self.core = Some(core);
        self.sink.framebuffer.fill(0);
        self.finalize_rom_session(
            rom_path,
            &sav_path,
            title.clone(),
            code.clone(),
            rom_crc32,
            ctx,
        );
        self.apply_active_game_settings_to_core();
        self.ra.on_rom_loaded(&self.config, &rom_bytes, &title);
        self.lua.on_rom_changed(&title, &code);

        let patch_tag = match patch_applied {
            Some(fmt) => format!(" ({} Patched)", fmt.label()),
            None => String::new(),
        };
        let archive_tag = match (archive_badge, archive_inner.as_deref()) {
            (Some(kind), Some(inner)) => format!(" [{kind}: {inner}]"),
            (Some(kind), None) => format!(" [{kind}]"),
            _ => String::new(),
        };
        self.notify(&format!("Playing {title} [{code}]{archive_tag}{patch_tag}"));
        Ok(())
    }

    pub(crate) fn finalize_rom_session(
        &mut self,
        rom_path: &Path,
        sav_path: &Path,
        title: String,
        code: String,
        rom_crc32: u32,
        ctx: Option<&egui::Context>,
    ) {
        self.rom_path = Some(rom_path.to_path_buf());
        self.sav_path = Some(sav_path.to_path_buf());
        self.rom_title = title.clone();
        self.rom_code = code.clone();
        self.rom_crc32 = rom_crc32;
        self.session_speed_override = None;
        self.show_game_settings = false;
        self.game_settings_target = None;
        self.paused = false;
        self.show_pause_overlay = false;
        self.show_state_manager = false;
        self.rewind_buffer
            .set_budget_mb(self.config.rewind_budget_mb);
        self.rewind_buffer.clear();
        self.rewind_movie_frames.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.autofire_state.clear_latches();
        self.base_pressed_mask = 0;
        self.active_turbo_buttons = (false, false);
        self.next_frame_due = Instant::now();
        self.last_step_time = Instant::now();
        self.time_accumulator = 0.0;
        self.frames_in_window = 0;
        self.last_fps_sample = Instant::now();
        self.postproc.reset();
        self.sink.clear_frames();
        self.sink.audio.reset_underruns();
        self.slot_thumbnails = [None, None, None, None, None];
        self.slot_meta = [None, None, None, None, None];
        self.state_manager_error = None;
        self.playtime_timer = Instant::now();

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let pt = self.config.playtimes.entry(code.clone()).or_default();
        pt.launch_count += 1;
        pt.last_played_timestamp = now_ts;

        self.config.push_recent(rom_path, title, code);
        self.title_dirty = true;
        if let Some(c) = ctx {
            self.sync_window_title(c);
            self.title_dirty = false;
        }
    }

    pub(crate) fn close_rom(&mut self, ctx: &egui::Context) {
        self.flush_gb_battery_save(true);
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.stop_all_captures(true);
        self.movie.reset_for_rom_change();
        self.rewind_movie_frames.clear();
        self.link.disconnect_all();
        self.core = None;
        self.gb = None;
        self.rom_path = None;
        self.sav_path = None;
        self.rom_title.clear();
        self.rom_code.clear();
        self.rom_crc32 = 0;
        self.active_cheats.clear();
        self.cart_hw_defaults = CartridgeHardwareDefaults::default();
        self.session_speed_override = None;
        self.show_game_settings = false;
        self.game_settings_target = None;
        self.slot_thumbnails = [None, None, None, None, None];
        self.slot_meta = [None, None, None, None, None];
        self.state_manager_error = None;
        self.paused = false;
        self.show_pause_overlay = false;
        self.show_state_manager = false;
        self.rewind_buffer
            .set_budget_mb(self.config.rewind_budget_mb);
        self.rewind_buffer.clear();
        self.rewind_movie_frames.clear();
        self.frame_counter = 0;
        self.rewinding = false;
        self.autofire_state.clear_latches();
        self.base_pressed_mask = 0;
        self.active_turbo_buttons = (false, false);
        self.last_step_time = Instant::now();
        self.time_accumulator = 0.0;
        self.lcd_tex = None;
        self.postproc.reset();
        self.sink.clear_frames();
        self.rescan_rom_folders();
        self.sync_window_title(ctx);
        self.notify("Game stopped — Returned to ROM Library");
    }

    pub(crate) fn save_rom_specific_settings(&mut self) {
        let Some(core) = &self.core else { return };
        if self.rom_code.is_empty() {
            return;
        }
        let mut ov = load_effective_settings(&self.config, &self.rom_code, self.rom_crc32);
        if ov.save_override.is_some() || core.bus.cart.save_type_override.is_some() {
            ov.save_override = core
                .bus
                .cart
                .save_type_override
                .map(|st| SaveOverrideChoice::from_core(Some(st)));
        }
        if ov.rtc_override.is_some() || core.bus.cart.rtc_override.is_some() {
            ov.rtc_override = core.bus.cart.rtc_override;
        }
        if ov.solar_lux.is_some() || core.bus.cart.solar_enabled {
            ov.solar_lux = Some(core.bus.cart.solar_lux);
        }
        ov.cheats = self.active_cheats.clone();
        save_game_settings_entry(&mut self.config, &self.rom_code, self.rom_crc32, ov);
    }

    pub(crate) fn flush_battery_save(&mut self) {
        if let Some(local) = &mut self.link.local {
            local.flush_p2_battery_save();
        }
        if self.gb.is_some() {
            self.flush_gb_battery_save(false);
            return;
        }
        let (Some(core), Some(sav_path)) = (&mut self.core, &self.sav_path) else {
            return;
        };
        if !core.bus.cart.save_dirty {
            return;
        }
        let data = core.bus.cart.export_save_data().to_vec();
        if !data.is_empty() {
            core.bus.cart.save_dirty = false;
            let path = sav_path.clone();
            std::thread::spawn(move || {
                let _ = fs::write(path, data);
            });
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
        if self.gb.is_some() {
            self.import_gb_battery_save();
            return;
        }
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
        if self.gb.is_some() {
            self.export_gb_battery_save();
            return;
        }
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
        if !self.has_active_game() {
            return;
        }
        if self.sink.frame_dirty {
            let color_profile = self.effective_color_profile();
            let _ = self.postproc.process_frame(
                &self.sink.framebuffer,
                self.config.color_correction,
                color_profile,
                self.config.lcd_ghosting,
                self.config.scanline_grid,
            );
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

        let color_profile = self.effective_color_profile();
        self.postproc.sync_rgba_for_capture(
            &self.sink.framebuffer,
            self.sink
                .has_prev_frame
                .then_some(&self.sink.prev_framebuffer),
            self.sink.frame_seq,
            self.config.color_correction,
            color_profile,
            self.config.frame_blending,
            self.config.lcd_ghosting,
        );
        let (rgba, w, h) = if self.is_gb_active() {
            (
                crop_gb_rgba_from_240x160(&self.postproc.curr_rgba),
                gb_core::LCD_WIDTH as u32,
                gb_core::LCD_HEIGHT as u32,
            )
        } else {
            (
                self.postproc.curr_rgba.to_vec(),
                LCD_WIDTH as u32,
                LCD_HEIGHT as u32,
            )
        };

        let res = image::save_buffer_with_format(
            &save_path,
            &rgba,
            w,
            h,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        );

        match res {
            Ok(()) => self.notify(&format!("Screenshot saved: {}", save_path.display())),
            Err(e) => {
                let ppm_path = save_path.with_extension("ppm");
                let mut data = format!("P6\n{w} {h}\n255\n").into_bytes();
                for chunk in rgba.as_chunks::<4>().0 {
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
            self.rewind_movie_frames.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.last_step_time = Instant::now();
            self.time_accumulator = 0.0;
            self.next_frame_due = Instant::now();
            if self.movie.is_active() {
                self.movie.stop();
                self.notify("Soft Reset — Active TAS Movie stopped");
            } else {
                self.notify("Soft Reset");
            }
        } else if let Some(gb) = &mut self.gb {
            gb.core.reset();
            gb.core.set_dmg_palette(self.config.dmg_palette.to_core());
            self.postproc.reset();
            self.sink.clear_frames();
            blit_gb_to_gba_fb(gb.core.framebuffer(), &mut self.sink.framebuffer);
            self.sink.frame_dirty = true;
            self.rewind_buffer.clear();
            self.rewind_movie_frames.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.last_step_time = Instant::now();
            self.time_accumulator = 0.0;
            self.next_frame_due = Instant::now();
            if self.movie.is_active() {
                self.movie.stop();
                self.notify("Reset GB/GBC Core — Active TAS Movie stopped");
            } else {
                self.notify("Reset GB/GBC Core");
            }
        }
    }

    pub(crate) fn hard_reset(&mut self) {
        if let Some(core) = &mut self.core {
            let link_on = self.link.local_is_active()
                || self.link.netplay.as_ref().is_some_and(|n| n.is_connected());
            core.reset();
            core.set_link_connected(link_on);
            self.rewind_buffer.clear();
            self.rewind_movie_frames.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.postproc.reset();
            self.sink.clear_frames();
            self.last_step_time = Instant::now();
            self.time_accumulator = 0.0;
            self.next_frame_due = Instant::now();
            self.apply_active_game_settings_to_core();
            if self.movie.is_active() {
                self.movie.stop();
                self.notify("Hard Reset — Active TAS Movie stopped");
            } else {
                self.notify("Hard Reset");
            }
        } else if let Some(gb) = &mut self.gb {
            gb.core.reset();
            gb.core.set_dmg_palette(self.config.dmg_palette.to_core());
            self.postproc.reset();
            self.sink.clear_frames();
            blit_gb_to_gba_fb(gb.core.framebuffer(), &mut self.sink.framebuffer);
            self.sink.frame_dirty = true;
            self.rewind_buffer.clear();
            self.rewind_movie_frames.clear();
            self.frame_counter = 0;
            self.rewinding = false;
            self.last_step_time = Instant::now();
            self.time_accumulator = 0.0;
            self.next_frame_due = Instant::now();
            if self.movie.is_active() {
                self.movie.stop();
                self.notify("Hard Reset GB/GBC — Active TAS Movie stopped");
            } else {
                self.notify("Hard Reset GB/GBC");
            }
        }
    }

    pub(crate) fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if !self.paused {
            self.temp_step_bp = None;
            self.temp_step_target_depth = None;
            let ctrl = self.build_debug_control();
            // Avoid spurious immediate re-hit if currently parked on a breakpoint
            if let Some(core) = &mut self.core {
                let pc = core.cpu.current_exec_pc();
                if ctrl.breakpoints.contains(&pc) {
                    self.debugger.call_stack.step_one_with_tracker(core, &ctrl);
                }
            }
            self.last_step_time = Instant::now();
            self.time_accumulator = 0.0;
            self.next_frame_due = Instant::now();
            self.sink.audio.reset_underruns();
        }
    }

    pub(crate) fn step_single_frame(&mut self, ctx: &egui::Context) {
        if !self.has_active_game() {
            return;
        }
        self.paused = true;
        self.show_pause_overlay = false;
        let _ = self.poll_live_inputs(ctx);
        let live_keypad = self.effective_live_keypad_for_frame();
        self.sink.keypad = self.movie.step_frame(live_keypad);
        let p2_kp = if self.link.local_is_active() {
            self.poll_p2_inputs(ctx)
        } else {
            0x03FF
        };
        self.sink.capture_audio = self.capture.is_wav_recording();
        self.sink.captured_audio.clear();
        let color_profile = self.effective_color_profile();
        let color_corr = self.config.color_correction;
        let movie_frame_for_rewind = self.movie.current_frame();

        if self.config.rewind_enabled {
            let interval = self.config.rewind_interval.clamp(1, 4) as u64;
            if self.frame_counter.is_multiple_of(interval)
                && let Ok(state) = self.save_active_core_state_raw()
            {
                record_rewind_snapshot(
                    &mut self.rewind_buffer,
                    &mut self.rewind_movie_frames,
                    state,
                    movie_frame_for_rewind,
                );
            }
        }

        if let Some(core) = &mut self.core {
            apply_cheats_to_core(core, &self.active_cheats);
            if let Some(local) = &mut self.link.local {
                local.step_frame(core, &mut self.sink, p2_kp);
            } else if let Some(net) = &mut self.link.netplay {
                net.step_frame(core, &mut self.sink);
            } else {
                let _ = core.run_frame_with_breakpoints(&mut self.sink, &[]);
            }
            self.frame_counter += 1;
        } else if self.gb.is_some() {
            self.step_gb_frame(self.sink.keypad, false);
            self.frame_counter += 1;
        }

        self.flush_frame_captures(true, color_corr, color_profile);
        if let Some(msg) = self.movie.take_notice() {
            self.notify(&msg);
        }
    }

    pub(crate) fn step_single_instruction(&mut self) {
        self.temp_step_bp = None;
        self.temp_step_target_depth = None;
        let ctrl = self.build_debug_control();
        let mut stop_hit = None;
        if let Some(core) = &mut self.core {
            self.paused = true;
            stop_hit = self.debugger.call_stack.step_one_with_tracker(core, &ctrl);
            if self.debugger.show_io_regs {
                self.debugger.poll_io_changes(core);
            }
            if core.bus.ppu.frame_ready {
                self.sink.push_frame(&core.bus.ppu.framebuffer);
                core.bus.ppu.frame_ready = false;
            }
        }
        if let Some(reason) = stop_hit {
            self.handle_stop_reason(reason);
        }
    }

    pub(crate) fn step_over(&mut self) {
        let Some(core) = &self.core else { return };
        let pc = core.cpu.current_exec_pc();
        let thumb = core.cpu.is_thumb();
        let call_ret = gba_debugger::is_call_instruction(core, pc, thumb);
        let Some(ret_addr) = call_ret else {
            self.step_single_instruction();
            return;
        };

        let base_depth = self.debugger.call_stack.frames().len();
        let ctrl = self.build_debug_control();
        if let Some(core) = &mut self.core {
            // Advance past call start (and any breakpoint parked on current PC) before running to return addr.
            self.debugger.call_stack.step_one_with_tracker(core, &ctrl);
        }
        self.temp_step_bp = Some(ret_addr & !1);
        self.temp_step_target_depth = Some(base_depth);
        self.paused = false;
        self.next_frame_due = Instant::now();
    }

    pub(crate) fn step_out(&mut self) {
        let Some(core) = &self.core else { return };
        let Some(ret_addr) = gba_debugger::step_out_return_addr(core, &self.debugger.call_stack)
        else {
            self.notify("Step Out: no return address available");
            return;
        };

        let target_depth = self.debugger.call_stack.frames().len().saturating_sub(1);
        let ctrl = self.build_debug_control();
        if let Some(core) = &mut self.core {
            let pc = core.cpu.current_exec_pc();
            if ctrl.breakpoints.contains(&pc) {
                self.debugger.call_stack.step_one_with_tracker(core, &ctrl);
            }
        }
        self.temp_step_bp = Some(ret_addr & !1);
        self.temp_step_target_depth = Some(target_depth);
        self.paused = false;
        self.next_frame_due = Instant::now();
    }

    fn build_debug_control(&self) -> gba_core::DebugControl {
        let base = gba_core::DebugControl {
            breakpoints: self.debugger.breakpoints.clone(),
            watchpoints: self.debugger.watchpoints.clone(),
        };
        let mut merged = gba_core::DebugControl::default();
        self.gdb_stub.merged_debug_control(&base, &mut merged);
        if let Some(temp_bp) = self.temp_step_bp
            && !merged.breakpoints.contains(&temp_bp)
        {
            merged.breakpoints.push(temp_bp);
        }
        merged
    }

    fn handle_stop_reason(&mut self, reason: gba_core::StopReason) {
        use gba_core::StopReason;

        let hit_temp = matches!(
            reason,
            StopReason::Breakpoint(pc) if self.temp_step_bp == Some(pc & !1)
        );
        if hit_temp {
            self.temp_step_bp = None;
            if let Some(target_depth) = self.temp_step_target_depth.take() {
                // Keep call stack aligned after running at frame speed during Step Over/Out.
                self.debugger.call_stack.truncate(target_depth);
            }
        } else {
            self.temp_step_bp = None;
            self.temp_step_target_depth = None;
        }

        self.paused = true;
        self.debugger.show_cpu = true;
        if self.gdb_stub.is_client_connected() && !hit_temp {
            self.gdb_stub.notify_stop(&reason);
        }

        match reason {
            StopReason::Breakpoint(pc) => {
                if hit_temp {
                    self.notify(&format!("Stepped to 0x{pc:08X}"));
                } else {
                    self.notify(&format!("Paused at Breakpoint 0x{pc:08X}"));
                }
            }
            StopReason::Watchpoint {
                addr,
                pc,
                write,
                value,
            } => {
                self.debugger.last_watchpoint_hit = Some(reason);
                let rw = if write { "WRITE" } else { "READ" };
                self.notify(&format!(
                    "Watchpoint [{rw}] 0x{addr:08X} = 0x{value:X} @ 0x{pc:08X}"
                ));
            }
            StopReason::FrameComplete => {}
        }
    }

    fn poll_gdb_server(&mut self) {
        if !self.gdb_stub.is_listening() && !self.debugger.show_gdb {
            return;
        }
        let user_ctrl = gba_core::DebugControl {
            breakpoints: self.debugger.breakpoints.clone(),
            watchpoints: self.debugger.watchpoints.clone(),
        };
        let mut pending_notice: Option<String> = None;
        let mut stop_hit = None;
        if let Some(core) = &mut self.core {
            let outcome = self.gdb_stub.poll_network(core, &user_ctrl);
            if let Some(peer) = outcome.client_connected {
                pending_notice = Some(format!("GDB client connected ({peer})"));
            } else if outcome.client_disconnected {
                pending_notice = Some(String::from("GDB client disconnected"));
            }
            if outcome.kill_requested {
                self.paused = true;
                pending_notice = Some(String::from("GDB target killed (paused)"));
            } else if let Some(pause) = outcome.pause_request {
                if pause {
                    self.paused = true;
                } else {
                    // Resuming from GDB continue: step past current PC if parked on a breakpoint.
                    let mut merged = gba_core::DebugControl::default();
                    self.gdb_stub.merged_debug_control(&user_ctrl, &mut merged);
                    let pc = core.cpu.current_exec_pc();
                    if merged.breakpoints.contains(&pc) {
                        self.debugger
                            .call_stack
                            .step_one_with_tracker(core, &merged);
                    }
                    self.paused = false;
                    self.last_step_time = Instant::now();
                    self.time_accumulator = 0.0;
                    self.next_frame_due = Instant::now();
                }
            }
            if let Some(reason) = outcome.stop_reason {
                stop_hit = Some(reason);
            }
            if let Some(err) = outcome.last_error {
                self.debugger.gdb_status.last_error = Some(err);
            }
        }
        if let Some(msg) = pending_notice {
            self.notify(&msg);
        }
        if let Some(reason) = stop_hit {
            self.handle_stop_reason(reason);
        }
        let (bps, wps) = self.gdb_stub.session.breakpoint_counts();
        self.debugger.gdb_status.listening = self.gdb_stub.is_listening();
        self.debugger.gdb_status.port = self.gdb_stub.port();
        self.debugger.gdb_status.client_addr = self.gdb_stub.client_addr().map(str::to_owned);
        self.debugger.gdb_status.breakpoints = bps;
        self.debugger.gdb_status.watchpoints = wps;
    }

    pub(crate) fn step_single_scanline(&mut self) {
        let Some(core) = &mut self.core else { return };
        self.paused = true;
        core.step_scanline();
        if core.bus.ppu.frame_ready {
            self.sink.push_frame(&core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
        }
    }

    pub(crate) fn trigger_open_rom_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open GBA / Game Boy ROM")
            .add_filter(
                "GBA / GB / GBC ROMs & Archives",
                &["gba", "agb", "bin", "gb", "gbc", "zip", "7z"],
            )
            .pick_file()
            && let Err(e) = self.open_rom(&path, None, Some(ctx))
        {
            self.notify_error(&e);
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
                    self.notify("Added ROM directory — Scanning library...");
                }
                continue;
            }
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "sav" || ext == "srm" {
                if let Ok(bytes) = fs::read(&path) {
                    if let Some(core) = &mut self.core {
                        core.bus.cart.load_save_data(&bytes);
                        core.bus.cart.save_dirty = true;
                        self.notify("Loaded dropped .sav file");
                    } else if let Some(gb) = &mut self.gb {
                        gb.load_sav_with_rtc(&bytes);
                        self.flush_gb_battery_save(true);
                        self.notify("Loaded dropped GB/GBC .sav file");
                    }
                }
            } else if ext == "state" || ext == "gbas" {
                self.load_state_from_path(&path, "Loaded dropped Save State");
            } else if is_patch_extension(&path) {
                if let Some(base_rom) = self.rom_path.clone() {
                    if let Err(e) =
                        self.open_rom_with_patch(&base_rom, None, Some(&path), Some(ctx))
                    {
                        self.notify_error(&e);
                    }
                } else {
                    self.notify_error("Load a ROM first before dropping a .bps/.ups/.ips patch");
                }
            } else {
                if let Err(e) = self.open_rom(&path, None, Some(ctx)) {
                    self.notify_error(&e);
                }
                break;
            }
        }
    }

    // WAVE-2 HOOK: Per-frame emulation loop — Lua `onframe`, RetroAchievements frame tick,
    // Movie frame advance, and Link Cable / SIO sync integrate inside this cadence loop.
    pub(crate) fn advance_emulation(&mut self, ctx: &egui::Context) {
        if let Some(net) = &mut self.link.netplay {
            net.poll_events();
        }
        if !self.has_active_game() {
            return;
        }

        let now = Instant::now();
        let dt = now
            .duration_since(self.last_step_time)
            .as_secs_f64()
            .min(0.05);
        self.last_step_time = now;

        // Handle active real-time rewind paced at GBA frame cadence
        if self.rewinding {
            self.sink.suppress_audio = true;
            self.sink.capture_audio = false;
            self.sink.captured_audio.clear();
            self.time_accumulator = 0.0;
            if now >= self.next_frame_due {
                let prev_len = self.rewind_buffer.len();
                self.step_rewind_frame();
                self.sync_movie_after_rewind(prev_len);
                self.next_frame_due = now + Duration::from_secs_f64(GBA_FRAME_SECS);
            }
            ctx.request_repaint();
            return;
        }

        if self.paused {
            self.time_accumulator = 0.0;
            self.pacing_deadline = None;
            return;
        }

        self.sink.keypad = self.poll_inputs(ctx);
        let p2_kp = if self.link.local_is_active() {
            self.poll_p2_inputs(ctx)
        } else {
            0x03FF
        };
        let speed = self.effective_speed_multiplier();
        let turbo = speed <= 0.0;
        let is_normal_speed = !turbo && (speed - 1.0).abs() < 0.01;
        let target_frame_time = if turbo {
            0.0
        } else if is_normal_speed && self.sync_60hz {
            // Audio DRC (+/-1.0%) absorbs +0.456% (59.7275 -> 60.0 Hz) so frames
            // align cleanly with 60/120/240 Hz display refresh intervals.
            1.0 / 60.0
        } else {
            1.0 / (59.72750057 * (speed as f64).max(0.1))
        };

        let mut max_catchup = if turbo {
            16
        } else if is_normal_speed {
            1
        } else {
            (speed.ceil() as usize + 1).clamp(2, 8)
        };
        let mut paced_wake_at: Option<Instant> = None;

        if !turbo && is_normal_speed && self.core.is_some() {
            let step_now = Instant::now();
            let step_period = Duration::from_secs_f64(target_frame_time);
            let early_margin = Duration::from_micros(3200);
            let wake_lead = Duration::from_micros(2000);
            let mut deadline = *self.pacing_deadline.get_or_insert(step_now);
            if step_now > deadline + step_period.saturating_mul(2) {
                deadline = step_now;
            }
            let next_deadline = if step_now + early_margin >= deadline {
                self.time_accumulator = target_frame_time;
                let min_dl = step_now + step_period + Duration::from_micros(500);
                let max_dl = step_now + step_period + Duration::from_micros(2500);
                let clamped = (deadline + step_period).clamp(min_dl, max_dl);
                self.pacing_deadline = Some(clamped);
                clamped
            } else {
                self.time_accumulator = 0.0;
                max_catchup = 0;
                deadline
            };
            paced_wake_at = Some(
                next_deadline
                    .checked_sub(wake_lead)
                    .unwrap_or(next_deadline),
            );
        } else {
            self.pacing_deadline = None;
            if !turbo {
                self.time_accumulator += dt;
                // Bound accumulator to prevent multi-frame catchup burst after window moves/menus
                if self.time_accumulator > target_frame_time * 2.0 {
                    self.time_accumulator = target_frame_time * 1.5;
                }
            }
        }

        let mut executed = 0usize;

        let execution_threshold = target_frame_time;

        while executed < max_catchup && (turbo || self.time_accumulator >= execution_threshold) {
            if turbo && executed > 0 && now.elapsed() > Duration::from_millis(12) {
                break;
            }
            if !turbo {
                self.time_accumulator = (self.time_accumulator - target_frame_time).max(0.0);
            }
            let start_t = Instant::now();
            let max_skip = (self.config.max_frameskip as usize).clamp(1, 8);
            let should_skip_render = (turbo && executed + 1 < max_catchup)
                || (self.config.auto_frameskip
                    && executed < max_skip
                    && self.time_accumulator > target_frame_time);

            if let Some(core) = &mut self.core {
                core.bus.ppu.skip_rendering = should_skip_render;
            }

            let live_keypad = if let Some(ref replay) = self.cli_replay {
                let eff = replay.effective_frame(self.cli_replay_frame, self.cli_loop_replay);
                self.cli_replay_frame += 1;
                replay.keyinput_at(eff)
            } else {
                self.effective_live_keypad_for_frame()
            };
            self.sink.keypad = self.movie.step_frame(live_keypad);
            self.sink.capture_audio = self.capture.is_wav_recording();
            self.sink.captured_audio.clear();
            let color_profile = self.effective_color_profile();
            let color_corr = self.config.color_correction;
            let movie_frame_for_rewind = self.movie.current_frame();

            if !self.has_active_game() {
                break;
            }

            // Capture rewind snapshot before advancing frame so rewind restores exact pre-frame state
            if self.config.rewind_enabled {
                let interval = self.config.rewind_interval.clamp(1, 4) as u64;
                if self.frame_counter.is_multiple_of(interval)
                    && let Ok(state) = self.save_active_core_state_raw()
                {
                    record_rewind_snapshot(
                        &mut self.rewind_buffer,
                        &mut self.rewind_movie_frames,
                        state,
                        movie_frame_for_rewind,
                    );
                }
            }

            let suppress_audio = (turbo || speed > 1.5) && executed > 0;
            self.sink.suppress_audio = suppress_audio;
            let mut stop_reason = gba_core::StopReason::FrameComplete;
            let ctrl = self.build_debug_control();
            let hardcore = self.is_ra_hardcore_active();
            let run_ahead_budget_ok =
                self.last_core_ms <= 6.5 && self.time_accumulator <= target_frame_time * 1.1;
            let run_ahead_eligible = self.config.run_ahead_frames.min(1) > 0
                && run_ahead_budget_ok
                && !turbo
                && !should_skip_render
                && self.link.local.is_none()
                && self.link.netplay.is_none()
                && ctrl.breakpoints.is_empty()
                && ctrl.watchpoints.is_empty();
            let saved_prev_fb = run_ahead_eligible
                .then(|| (self.sink.framebuffer.to_vec(), self.sink.has_prev_frame));

            if let Some(core) = &mut self.core {
                self.ra.tick_frame(core, &self.config);
                self.lua.run_frame(
                    core,
                    self.frame_counter,
                    &self.rom_title,
                    &self.rom_code,
                    self.base_pressed_mask,
                    hardcore,
                );
                apply_cheats_to_core(core, &self.active_cheats);
                if let Some(local) = &mut self.link.local {
                    local.step_frame(core, &mut self.sink, p2_kp);
                } else if let Some(net) = &mut self.link.netplay {
                    net.step_frame(core, &mut self.sink);
                } else {
                    stop_reason = core.run_frame_debug(&mut self.sink, &ctrl);
                    if self.debugger.show_io_regs {
                        self.debugger.poll_io_changes(core);
                    }
                }
            } else if self.gb.is_some() {
                self.step_gb_frame(self.sink.keypad, suppress_audio);
            }

            if let Some((prev_fb, had_prev)) = saved_prev_fb
                && matches!(stop_reason, gba_core::StopReason::FrameComplete)
            {
                self.apply_speculative_run_ahead_frame(&prev_fb, had_prev);
            }

            self.last_core_ms = start_t.elapsed().as_secs_f32() * 1000.0;
            self.last_step_frames += 1;
            self.last_step_emu_ms += self.last_core_ms as f64;
            executed += 1;
            self.frames_in_window += 1;
            self.frame_counter += 1;

            let frame_complete = matches!(stop_reason, gba_core::StopReason::FrameComplete);
            self.flush_frame_captures(frame_complete, color_corr, color_profile);
            if frame_complete && !turbo && !should_skip_render {
                let work_ms = self.last_core_ms as f64 + self.last_present_ms as f64;
                self.apply_quality_governor_sample(work_ms);
            }

            if !matches!(stop_reason, gba_core::StopReason::FrameComplete) {
                self.handle_stop_reason(stop_reason);
                break;
            }

            if turbo {
                self.last_step_time = Instant::now();
                self.time_accumulator = 0.0;
                self.next_frame_due = Instant::now();
            } else {
                self.next_frame_due = now + Duration::from_secs_f64(target_frame_time);
            }
        }
        if let Some(core) = &mut self.core {
            core.bus.ppu.skip_rendering = false;
        }
        self.sink.suppress_audio = false;
        self.sink.capture_audio = false;
        if let Some(msg) = self.movie.take_notice() {
            self.notify(&msg);
        }

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
        if self.has_active_game()
            && !self.paused
            && self.playtime_timer.elapsed() >= Duration::from_secs(1)
        {
            if !self.rom_code.is_empty() {
                let entry = self
                    .config
                    .playtimes
                    .entry(self.rom_code.clone())
                    .or_default();
                entry.total_seconds += self.playtime_timer.elapsed().as_secs();
            }
            self.playtime_timer = Instant::now();
        }

        if self.effective_rumble_enabled() {
            let active = if let Some(core) = &self.core {
                core.bus.cart.rumble_active
            } else if let Some(gb) = &self.gb {
                gb.core.is_rumble_active()
            } else {
                false
            };
            if active != self.rumble_in_progress {
                self.rumble_in_progress = active;
                if let Some(gilrs) = &mut self.gilrs {
                    if active {
                        if self.gilrs_rumble_effect.is_none() {
                            let support_ff = gilrs
                                .gamepads()
                                .filter_map(
                                    |(id, gp)| if gp.is_ff_supported() { Some(id) } else { None },
                                )
                                .collect::<Vec<_>>();
                            if !support_ff.is_empty() {
                                let duration = gilrs::ff::Ticks::from_ms(120);
                                if let Ok(eff) = gilrs::ff::EffectBuilder::new()
                                    .add_effect(gilrs::ff::BaseEffect {
                                        kind: gilrs::ff::BaseEffectType::Strong {
                                            magnitude: 45_000,
                                        },
                                        scheduling: gilrs::ff::Replay {
                                            play_for: duration,
                                            with_delay: duration,
                                            ..Default::default()
                                        },
                                        envelope: Default::default(),
                                    })
                                    .gamepads(&support_ff)
                                    .finish(gilrs)
                                {
                                    let _ = eff.play();
                                    self.gilrs_rumble_effect = Some(eff);
                                }
                            }
                        } else if let Some(eff) = &self.gilrs_rumble_effect {
                            let _ = eff.play();
                        }
                    } else if let Some(eff) = &self.gilrs_rumble_effect {
                        let _ = eff.stop();
                    }
                }
            }
        } else if self.rumble_in_progress {
            self.rumble_in_progress = false;
            if let Some(eff) = &self.gilrs_rumble_effect {
                let _ = eff.stop();
            }
        }

        if !self.paused {
            match paced_wake_at
                .map(|wake_at| wake_at.saturating_duration_since(Instant::now()))
            {
                Some(delay) if delay >= Duration::from_micros(500) => {
                    // egui::Context::request_repaint_after subtracts predicted_dt (default 1/60s),
                    // which clamps delays <= 16.67ms to ZERO; compensate so the requested timer holds.
                    let pred = Duration::try_from_secs_f32(ctx.input(|i| i.predicted_dt))
                        .unwrap_or_default();
                    ctx.request_repaint_after(delay + pred);
                }
                _ => ctx.request_repaint(),
            }
        }
    }
}

impl RustboyApp {
    pub fn update(&mut self, ctx: &egui::Context) {
        self.last_step_frames = 0;
        self.last_step_emu_ms = 0.0;
        if self.perf_logger.is_none()
            && let Some(path) = self.perf_log_path.take()
        {
            let tag = if self.sync_60hz {
                "eframe-fallback-sync60"
            } else {
                "eframe-fallback-nosync60"
            };
            self.perf_logger = crate::bench::DesktopPerfLogger::create_with_timing(
                &path,
                tag,
                self.display_refresh_hz,
                self.sync_60hz,
            )
            .ok();
            self.perf_start_wall = Instant::now();
        }
        let vp_size = ctx.screen_rect().size();
        let ppp = ctx.pixels_per_point();
        let vp_w = (vp_size.x * ppp).round() as u32;
        let vp_h = (vp_size.y * ppp).round() as u32;
        if vp_w > 0 && vp_h > 0 {
            self.governor_viewport_px = (vp_w, vp_h);
        }
        let emu_errors: Vec<String> = if let Some(rx) = &self.emu_error_rx {
            let mut errs = Vec::new();
            while let Ok(err) = rx.try_recv() {
                errs.push(err);
            }
            errs
        } else {
            Vec::new()
        };
        for err in emu_errors {
            self.notify_error(&err);
        }
        self.poll_library_scan(ctx);
        self.pump_gamepad_events(ctx);
        self.poll_gdb_server();
        if self.ra.poll_events(&mut self.config) {
            ctx.request_repaint();
        }
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

        let cursor_should_hide = self.has_active_game()
            && !self.paused
            && !self.show_pause_overlay
            && !self.show_state_manager
            && !self.link.show_window
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

        let present_t0 = Instant::now();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG_APP))
            .show(ctx, |ui| {
                if self.has_active_game() {
                    self.draw_active_viewport(ctx, ui);
                } else {
                    self.draw_rom_library_browser(ctx, ui);
                }
            });
        let ui_present_ms = present_t0.elapsed().as_secs_f64() * 1000.0;

        // Always render floating OSD toast on top so feedback is never lost
        self.draw_floating_osd_toast(ctx);

        // Visual Save State Manager, Rewind Settings, Controls, Movies & Per-Game Settings dialogs
        self.draw_visual_state_manager(ctx);
        self.draw_rewind_settings_dialog(ctx);
        self.draw_controls_dialog(ctx);
        self.draw_game_settings_dialog(ctx);
        self.draw_movies_panel(ctx);
        self.draw_link_window(ctx);
        let hardcore = self.is_ra_hardcore_active();
        self.lua
            .show_lua_console(ctx, self.core.is_some(), hardcore);
        self.ra.show_retroachievements_window(ctx, &mut self.config);

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
            DebugAction::StepOver => self.step_over(),
            DebugAction::StepOut => self.step_out(),
            DebugAction::StepScanline => self.step_single_scanline(),
            DebugAction::StepFrame => self.step_single_frame(ctx),
            DebugAction::HardReset => {
                self.temp_step_bp = None;
                self.temp_step_target_depth = None;
                self.debugger.call_stack.clear();
                self.debugger.last_watchpoint_hit = None;
                self.debugger.io_prev_snapshot.clear();
                self.hard_reset();
            }
            DebugAction::ConfigChanged => {
                self.save_rom_specific_settings();
                self.config.save();
            }
            DebugAction::AddCheat(entry) => {
                let name = entry.name.clone();
                self.active_cheats.push(entry);
                self.notify(&format!("Added cheat: {name}"));
            }
            DebugAction::GdbStart(port) => match self.gdb_stub.start(port) {
                Ok(bound) => {
                    self.debugger.gdb_status.listening = true;
                    self.debugger.gdb_status.port = bound;
                    self.debugger.gdb_status.last_error = None;
                    self.notify(&format!("GDB Server listening on 127.0.0.1:{bound}"));
                }
                Err(err) => {
                    self.debugger.gdb_status.listening = false;
                    self.debugger.gdb_status.last_error =
                        Some(format!("Failed to bind 127.0.0.1:{port}: {err}"));
                }
            },
            DebugAction::GdbStop => {
                self.gdb_stub.stop();
                self.debugger.gdb_status.listening = false;
                self.debugger.gdb_status.client_addr = None;
                self.debugger.gdb_status.breakpoints = 0;
                self.debugger.gdb_status.watchpoints = 0;
                self.notify("GDB Server stopped");
            }
            DebugAction::None => {}
        }

        // Refresh LCD immediately when Color Correction / Grid / Ghosting toggles while paused
        if self.paused && self.has_active_game() {
            self.sink.frame_dirty = true;
        }

        let has_game = self.has_active_game();
        let underruns_now = self.sink.audio.underruns();
        let filter_label = self.config.display_filter.label();
        let (vw, vh) = self.governor_viewport_px;
        let is_fs = self.is_fullscreen;
        let emu_ms = self.last_step_emu_ms;
        let step_frames = self.last_step_frames;
        let logged_present_ms = if step_frames > 0 && self.last_present_ms > 0.0 {
            self.last_present_ms as f64
        } else {
            ui_present_ms
        };
        if has_game && let Some(ref mut logger) = self.perf_logger {
            logger.record_present(
                emu_ms,
                logged_present_ms,
                underruns_now,
                filter_label,
                step_frames,
                vw,
                vh,
                is_fs,
            );
        }
        if let Some(limit_s) = self.perf_seconds
            && self.perf_start_wall.elapsed().as_secs_f64() >= limit_s
        {
            if let Some(ref mut logger) = self.perf_logger {
                logger.finish_and_print();
            }
            self.quit_requested = true;
        }
    }

    pub fn init_wgpu(&mut self, rs: &eframe::egui_wgpu::RenderState) {
        let info = rs.adapter.get_info();
        self.governor_adapter_name = info.name.clone();
        self.governor_adapter_backend = format!("{:?}", info.backend);
        if let Some(path) = self.perf_log_path.take() {
            let tag = format!(
                "eframe-wgpu-{:?}{}{}",
                info.backend,
                if info.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu {
                    "-cpu"
                } else {
                    ""
                },
                if self.sync_60hz {
                    "-sync60"
                } else {
                    "-nosync60"
                }
            );
            self.perf_logger = crate::bench::DesktopPerfLogger::create_with_timing(
                &path,
                &tag,
                self.display_refresh_hz,
                self.sync_60hz,
            )
            .ok();
            self.perf_start_wall = Instant::now();
        } else {
            self.perf_start_wall = Instant::now();
        }
        if info.device_type == eframe::egui_wgpu::wgpu::DeviceType::Cpu {
            log::info!(
                "Software CPU wgpu adapter detected ({} / {:?}); running filters on CPU post-processor with plain texture upload",
                info.name,
                info.backend
            );
            let measured_costs = crate::shaders::measure_cpu_adapter_filter_costs(
                &rs.device,
                &rs.queue,
                &mut self.postproc,
                self.config.color_correction,
                self.config.lcd_ghosting,
                self.config.lcd_grid_intensity,
            );
            self.quality_governor.set_measured_costs(measured_costs);
            self.wgpu_shaders = None;
        } else {
            match WgpuShaderPipeline::new(
                Arc::new(rs.device.clone()),
                Arc::new(rs.queue.clone()),
                rs.target_format,
            ) {
                Ok(pipeline) => {
                    log::info!("WGPU/WGSL post-processing shader pipeline initialized successfully");
                    self.wgpu_shaders = Some(Arc::new(Mutex::new(pipeline)));
                }
                Err(err) => {
                    log::warn!("WGPU/WGSL shader pipeline unavailable, using CPU fallback: {err}");
                    self.wgpu_shaders = None;
                }
            }
        }

        let key = self.current_governor_profile_key();
        if !self.governor_cli_filter
            && self.config.auto_quality_governor
            && let Some(settled) = self.config.settled_filter_for(&key)
        {
            self.quality_governor.set_active_from_persisted(settled);
            let active = self.quality_governor.active_filter();
            self.config.display_filter = active;
            self.config.shader_preset = active.default_shader_preset();
            self.config.scanline_grid = active == DisplayFilter::AuthenticGbaLcd;
            log::info!(
                "Restored settled governor filter `{}` for profile `{key}`",
                active.label()
            );
        }
    }

    pub fn on_exit(&mut self) {
        if let Some(ref mut logger) = self.perf_logger {
            logger.finish_and_print();
        }
        self.stop_all_captures(false);
        self.ra.on_rom_closed();
        self.lua.on_rom_changed("", "");
        self.flush_gb_battery_save(true);
        self.flush_battery_save();
        self.save_rom_specific_settings();
        self.link.disconnect_all();
        self.save_persistent_config();
        let key = self.current_governor_profile_key();
        self.config
            .set_settled_filter(key, self.quality_governor.active_filter());
        self.config.display_filter = self.governor_user_target;
        self.config.save();
    }

    pub(crate) fn sync_movie_after_rewind(&mut self, prev_rewind_len: usize) {
        let new_len = self.rewind_buffer.len();
        let mut last_popped_frame = None;
        while self.rewind_movie_frames.len() > new_len {
            last_popped_frame = self.rewind_movie_frames.pop_back();
        }
        if new_len < prev_rewind_len {
            if let Some(core) = &self.core {
                self.movie.on_state_loaded_at(core, last_popped_frame);
            }
            if let Some(msg) = self.movie.take_notice() {
                self.notify(&msg);
            }
        }
    }

    pub(crate) fn rewind_manual_with_movie_sync(&mut self) {
        let prev_len = self.rewind_buffer.len();
        self.step_rewind_manual();
        self.sync_movie_after_rewind(prev_len);
    }

    pub(crate) fn capture_toggle_gif(&mut self) {
        if self.capture.is_gif_recording() {
            match self.capture.stop_gif() {
                Ok(Some(sum)) => {
                    let fname = capture_path_name(sum.path.as_deref(), "capture.gif");
                    self.notify(&format!(
                        "GIF saved ({} frames): {}",
                        sum.frames_written, fname
                    ));
                }
                Ok(None) => {}
                Err(err) => self.notify(&format!("GIF finalize failed: {err}")),
            }
            return;
        }
        if self.core.is_none() {
            self.notify("Open a ROM before starting GIF capture");
            return;
        }
        let title = if self.rom_title.is_empty() {
            "rustboy_capture"
        } else {
            &self.rom_title
        };
        match self.capture.start_gif(title, None) {
            Ok(path) => {
                let fname = capture_path_name(Some(&path), "capture.gif");
                self.notify(&format!(
                    "GIF Recording started ({}): {}",
                    self.capture.gif_fps_mode.label(),
                    fname
                ));
            }
            Err(err) => self.notify(&format!("GIF start failed: {err}")),
        }
    }

    pub(crate) fn capture_toggle_wav(&mut self) {
        if self.capture.is_wav_recording() {
            self.sink.capture_audio = false;
            self.sink.captured_audio.clear();
            match self.capture.stop_wav() {
                Ok(Some(sum)) => {
                    let fname = capture_path_name(sum.path.as_deref(), "capture.wav");
                    self.notify(&format!(
                        "WAV saved ({:.1}s): {}",
                        sum.duration_secs(),
                        fname
                    ));
                }
                Ok(None) => {}
                Err(err) => self.notify(&format!("WAV finalize failed: {err}")),
            }
            return;
        }
        if self.core.is_none() {
            self.notify("Open a ROM before starting WAV capture");
            return;
        }
        let title = if self.rom_title.is_empty() {
            "rustboy_capture"
        } else {
            &self.rom_title
        };
        match self.capture.start_wav(title, None) {
            Ok(path) => {
                self.sink.captured_audio.clear();
                self.sink.capture_audio = true;
                let fname = capture_path_name(Some(&path), "capture.wav");
                self.notify(&format!("WAV Recording started: {fname}"));
            }
            Err(err) => self.notify(&format!("WAV start failed: {err}")),
        }
    }

    fn flush_frame_captures(
        &mut self,
        video_ready: bool,
        color_correction: bool,
        color_profile: crate::config::ColorProfileChoice,
    ) {
        if self.capture.is_wav_recording() && !self.sink.captured_audio.is_empty() {
            let res = self.capture.push_audio_samples(&self.sink.captured_audio);
            self.sink.captured_audio.clear();
            if let Err(err) = res {
                self.sink.capture_audio = false;
                let _ = self.capture.stop_wav();
                self.notify(&format!("WAV recording error: {err}"));
            }
        }
        if video_ready
            && self.capture.is_gif_recording()
            && let Err(err) = self.capture.push_video_frame(
                &self.sink.framebuffer,
                color_correction,
                color_profile,
            )
        {
            let _ = self.capture.stop_gif();
            self.notify(&format!("GIF recording error: {err}"));
        }
    }

    pub(crate) fn stop_all_captures(&mut self, notify_user: bool) {
        self.sink.capture_audio = false;
        self.sink.captured_audio.clear();
        let (gif_res, wav_res) = self.capture.stop_all();
        if !notify_user {
            return;
        }
        match (gif_res, wav_res) {
            (Ok(Some(g)), Ok(Some(w))) => {
                self.notify(&format!(
                    "Saved GIF ({} frames) & WAV ({:.1}s)",
                    g.frames_written,
                    w.duration_secs()
                ));
            }
            (gif, wav) => {
                match gif {
                    Ok(Some(sum)) => {
                        let fname = capture_path_name(sum.path.as_deref(), "capture.gif");
                        self.notify(&format!(
                            "GIF saved ({} frames): {}",
                            sum.frames_written, fname
                        ));
                    }
                    Err(e) => self.notify(&format!("GIF save error: {e}")),
                    Ok(None) => {}
                }
                match wav {
                    Ok(Some(sum)) => {
                        let fname = capture_path_name(sum.path.as_deref(), "capture.wav");
                        self.notify(&format!(
                            "WAV saved ({:.1}s): {}",
                            sum.duration_secs(),
                            fname
                        ));
                    }
                    Err(e) => self.notify(&format!("WAV save error: {e}")),
                    Ok(None) => {}
                }
            }
        }
    }

    /// Executes 1 speculative frame ahead to shave 1 frame of internal game input latency,
    /// presenting the speculative video frame while keeping audio and persistent state at the
    /// true end-of-frame boundary.
    pub(crate) fn apply_speculative_run_ahead_frame(
        &mut self,
        prev_displayed_fb: &[u16],
        prev_has_frame: bool,
    ) {
        let prev_suppress = self.sink.suppress_audio;
        let prev_capture = self.sink.capture_audio;
        self.sink.suppress_audio = true;
        self.sink.capture_audio = false;

        if let Some(core) = &mut self.core {
            let _ = core.speculate_run_ahead_frame(&mut self.sink);
            core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
            core.bus.ppu.layer_enable_mask = self.config.layer_mask;
            core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
            core.bus.apu.master_volume_pct = self.config.master_volume;
        } else if self.gb.is_some()
            && let Ok(snapshot) = self.save_active_core_state_raw()
        {
            self.step_gb_frame(self.sink.keypad, true);
            let _ = self.restore_active_core_state_raw(&snapshot, false);
        }

        self.sink.suppress_audio = prev_suppress;
        self.sink.capture_audio = prev_capture;
        if prev_displayed_fb.len() == self.sink.prev_framebuffer.len() {
            self.sink
                .prev_framebuffer
                .copy_from_slice(prev_displayed_fb);
        }
        self.sink.has_prev_frame = prev_has_frame;
    }
}

fn capture_path_name<'a>(path: Option<&'a Path>, fallback: &'a str) -> &'a str {
    path.and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
}

fn record_rewind_snapshot(
    rewind_buffer: &mut RewindBuffer,
    rewind_movie_frames: &mut VecDeque<usize>,
    state: Vec<u8>,
    movie_frame: usize,
) {
    rewind_buffer.push(state);
    rewind_movie_frames.push_back(movie_frame);
    while rewind_movie_frames.len() > rewind_buffer.len() {
        rewind_movie_frames.pop_front();
    }
}

pub(crate) fn configure_obsidian_style(ctx: &egui::Context) {
    crate::theme::configure_theme(ctx);
}

#[cfg(test)]
fn step_gba_frame_with_optional_run_ahead(
    core: &mut GbaCore,
    sink: &mut CoreSink,
    run_ahead_frames: u8,
) {
    let saved_prev_fb = sink.framebuffer;
    let saved_has_prev = sink.has_prev_frame;

    let _ = core.run_frame_debug(sink, &gba_core::DebugControl::default());

    if run_ahead_frames > 0 {
        let prev_suppress = sink.suppress_audio;
        let prev_capture = sink.capture_audio;
        sink.suppress_audio = true;
        sink.capture_audio = false;

        let _ = core.speculate_run_ahead_frame(sink);

        sink.suppress_audio = prev_suppress;
        sink.capture_audio = prev_capture;
        sink.prev_framebuffer.copy_from_slice(&saved_prev_fb);
        sink.has_prev_frame = saved_has_prev;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_from_words(words: &[u32]) -> Vec<u8> {
        let mut rom = Vec::with_capacity(words.len() * 4);
        for &w in words {
            rom.extend_from_slice(&w.to_le_bytes());
        }
        rom
    }

    #[test]
    fn test_run_ahead_shaves_one_frame_latency_without_duplicating_audio() {
        // Mode 3 ROM with 1-frame internal input latency:
        // R7 = 0x001F (red) initially; once per frame at VCOUNT == 100 (before scanline 120),
        // writes R7 to VRAM pixel (0, 120) at 0x0600_E100, reads KEYINPUT, and updates R7 for the
        // NEXT frame (green 0x03E0 if KEY_A pressed else red).
        let rom = rom_from_words(&[
            0xE3A0_0301, // 0x00: MOV R0, #0x04000000
            0xE3A0_1B01, // 0x04: MOV R1, #0x0400
            0xE381_1003, // 0x08: ORR R1, R1, #3 -> 0x0403 (Mode 3, BG2)
            0xE1C0_10B0, // 0x0C: STRH R1, [R0]
            0xE3A0_4406, // 0x10: MOV R4, #0x06000000 (VRAM)
            0xE284_4CE1, // 0x14: ADD R4, R4, #0xE100 (pixel (0, 120))
            0xE280_5C01, // 0x18: ADD R5, R0, #0x100
            0xE285_5030, // 0x1C: ADD R5, R5, #0x30 (0x04000130 KEYINPUT)
            0xE280_6006, // 0x20: ADD R6, R0, #6 (0x04000006 VCOUNT)
            0xE3A0_701F, // 0x24: MOV R7, #0x001F (red)
            // wait_line (0x28):
            0xE1D6_80B0, // 0x28: LDRH R8, [R6]
            0xE358_0064, // 0x2C: CMP R8, #100
            0x1AFF_FFFC, // 0x30: BNE wait_line
            0xE1C4_70B0, // 0x34: STRH R7, [R4] (draw latched color)
            0xE1D5_80B0, // 0x38: LDRH R8, [R5] (poll KEYINPUT for NEXT frame)
            0xE318_0001, // 0x3C: TST R8, #1 (KEY_A active-low)
            0x03A0_7E3E, // 0x40: MOVEQ R7, #0x03E0 (green)
            0x13A0_701F, // 0x44: MOVNE R7, #0x001F (red)
            // leave_line (0x48):
            0xE1D6_80B0, // 0x48: LDRH R8, [R6]
            0xE358_0064, // 0x4C: CMP R8, #100
            0x0AFF_FFFC, // 0x50: BEQ leave_line
            0xEAFF_FFF3, // 0x54: B wait_line (0x54 + 8 - 13*4 = 0x28)
        ]);
        let pixel_idx = 120 * LCD_WIDTH;

        // 1) Baseline without run-ahead: pressing KEY_A on frame 2 still draws RED on frame 2.
        let mut core_off = GbaCore::new(None, rom.clone());
        let mut sink_off = CoreSink::new(AudioOutput::dummy_for_tests());
        sink_off.capture_audio = true;
        sink_off.keypad = 0x03FF;
        step_gba_frame_with_optional_run_ahead(&mut core_off, &mut sink_off, 0);
        assert_eq!(sink_off.framebuffer[pixel_idx], 0x001F);

        sink_off.captured_audio.clear();
        sink_off.keypad = gba_core::encode_keyinput(gba_core::KEY_A);
        step_gba_frame_with_optional_run_ahead(&mut core_off, &mut sink_off, 0);
        let single_frame_audio_len = sink_off.captured_audio.len();
        assert!(single_frame_audio_len > 0);
        assert_eq!(sink_off.framebuffer[pixel_idx], 0x001F); // Still delayed by 1 frame

        // 2) With 1-frame Run-Ahead: pressing KEY_A on frame 2 immediately presents GREEN!
        let mut core_on = GbaCore::new(None, rom);
        let mut sink_on = CoreSink::new(AudioOutput::dummy_for_tests());
        sink_on.capture_audio = true;
        sink_on.keypad = 0x03FF;
        step_gba_frame_with_optional_run_ahead(&mut core_on, &mut sink_on, 1);

        sink_on.captured_audio.clear();
        sink_on.keypad = gba_core::encode_keyinput(gba_core::KEY_A);
        step_gba_frame_with_optional_run_ahead(&mut core_on, &mut sink_on, 1);

        assert_eq!(sink_on.framebuffer[pixel_idx], 0x03E0);
        // Speculative frame must NOT duplicate audio samples.
        assert_eq!(sink_on.captured_audio.len(), single_frame_audio_len);
        // Persistent core state must match exact 2-frame cycle boundary of core_off.
        assert_eq!(
            core_on.bus.scheduler.current_cycle,
            core_off.bus.scheduler.current_cycle
        );
    }
}
