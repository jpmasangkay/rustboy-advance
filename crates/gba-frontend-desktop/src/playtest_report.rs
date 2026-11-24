//! Automatic per-game playtest telemetry and live `PLAYTEST_RESULTS.md` reporting.
//!
//! Tracks real gameplay sessions (GBA, GB, and GBC) with zero heap allocations on the
//! per-frame hot path, persists a bounded JSON ledger in `AppConfig::app_data_dir()`,
//! and renders a clean, privacy-safe Markdown report (`docs/PLAYTEST_RESULTS.md`) that
//! friends can inspect or share back.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gba_core::cartridge::{Cartridge, SaveType};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, write_atomic};
use crate::gb_bridge::GbSession;

/// Markdown report filename written under `<exe_dir>/docs/` (when present & writable)
/// or `<app_data_dir>/docs/`.
pub(crate) const PLAYTEST_REPORT_FILENAME: &str = "PLAYTEST_RESULTS.md";

/// Companion structured ledger filename stored inside `<app_data_dir>/`.
pub(crate) const PLAYTEST_LEDGER_FILENAME: &str = "playtest_results.json";

/// Bounded circular buffer capacity for per-frame timing percentiles (p95).
const MAX_TIMING_SAMPLES: usize = 2048;

/// Maximum distinct games persisted in the ledger / Markdown table.
const MAX_RECORDED_GAMES: usize = 512;

/// Minimum emulated frames (~3 s at 60 FPS) before assigning a full performance verdict.
const MIN_VERDICT_FRAMES: u64 = 180;

/// Minimum normal-speed frames (~2 s) before using measured FPS for slow/stutter classification.
const MIN_NORMAL_SPEED_FRAMES: u64 = 120;

/// Periodic background flush interval during continuous play so abrupt exits still leave results.
const PERIODIC_FLUSH_INTERVAL: Duration = Duration::from_secs(30);

/// Stride used when checking whether a framebuffer has non-uniform rendered pixels.
const VIDEO_ACTIVITY_PIXEL_STRIDE: usize = 197;

/// Frame cadence for probing video activity until the first non-uniform frame is observed.
const VIDEO_ACTIVITY_CHECK_INTERVAL_FRAMES: u64 = 30;

/// Clamped per-tick wall time delta so OS sleep/hibernate never inflates play duration.
const MAX_ACTIVE_TICK_SECS: f64 = 2.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PlaytestVerdict {
    Smooth,
    MinorStutter,
    Slow,
    BlankScreen,
    BriefLaunch,
}

impl PlaytestVerdict {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Smooth => "Smooth",
            Self::MinorStutter => "Minor Stutter",
            Self::Slow => "Slow",
            Self::BlankScreen => "Blank Screen",
            Self::BriefLaunch => "Brief Launch",
        }
    }

    pub(crate) const fn is_substantive(self) -> bool {
        !matches!(self, Self::BriefLaunch)
    }
}

/// Static metadata describing a loaded GBA / GB / GBC ROM (strictly non-PII: no local file paths).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GameSessionMeta {
    pub(crate) title: String,
    pub(crate) code: String,
    pub(crate) platform: String,
    pub(crate) crc32: u32,
    pub(crate) save_type: String,
    pub(crate) hardware_extras: String,
    pub(crate) bios_mode: String,
    pub(crate) real_bios_hint: Option<String>,
    pub(crate) renderer: String,
    pub(crate) filter: String,
}

impl GameSessionMeta {
    pub(crate) fn from_gba_core(
        cart: &Cartridge,
        crc32: u32,
        using_real_bios: bool,
        real_bios_hint: Option<String>,
        renderer: String,
        filter: String,
    ) -> Self {
        let mut extras = Vec::new();
        if cart.effective_rtc() {
            extras.push("RTC");
        }
        if cart.solar_enabled {
            extras.push("Solar");
        }
        if cart.tilt_enabled {
            extras.push("Tilt");
        }
        if cart.gyro_enabled {
            extras.push("Gyro");
        }
        if cart.rumble_enabled {
            extras.push("Rumble");
        }
        let hardware_extras = if extras.is_empty() {
            "Standard".to_string()
        } else {
            extras.join(" + ")
        };

        let raw_title = cart.game_title();
        let title = sanitize_display_text(if raw_title.trim().is_empty() {
            "Untitled GBA ROM"
        } else {
            raw_title.trim()
        });
        let raw_code = cart.game_code();
        let code = sanitize_display_text(if raw_code.trim().is_empty() {
            "----"
        } else {
            raw_code.trim()
        });

        Self {
            title,
            code,
            platform: "GBA".to_string(),
            crc32,
            save_type: format_gba_save_type(cart.effective_save_type()).to_string(),
            hardware_extras,
            bios_mode: if using_real_bios {
                "Real BIOS".to_string()
            } else {
                "HLE (built-in)".to_string()
            },
            real_bios_hint: real_bios_hint.map(|s| sanitize_display_text(&s)),
            renderer: sanitize_display_text(&renderer),
            filter: sanitize_display_text(&filter),
        }
    }

    pub(crate) fn from_gb_session(
        gb: &GbSession,
        fallback_title: &str,
        crc32: u32,
        renderer: String,
        filter: String,
    ) -> Self {
        let hdr_title = gb.core.title();
        let raw_title = if hdr_title.trim().is_empty() {
            fallback_title.trim()
        } else {
            hdr_title.trim()
        };
        let title = sanitize_display_text(if raw_title.is_empty() {
            "Untitled GB ROM"
        } else {
            raw_title
        });
        let code = sanitize_display_text(if gb.short_code.trim().is_empty() {
            "----"
        } else {
            gb.short_code.trim()
        });
        let hardware_extras = if gb.has_rtc() {
            "RTC".to_string()
        } else {
            "Standard".to_string()
        };
        Self {
            title,
            code,
            platform: gb.platform.badge().to_string(),
            crc32,
            save_type: sanitize_display_text(&gb.cart_type_label),
            hardware_extras,
            bios_mode: "Built-in Boot".to_string(),
            real_bios_hint: None,
            renderer: sanitize_display_text(&renderer),
            filter: sanitize_display_text(&filter),
        }
    }

    pub(crate) fn record_key(&self) -> String {
        format!("{}:{}:{:08X}", self.platform, self.code, self.crc32)
    }
}

/// Privacy-safe hardware & software specs of the device used for playtesting
/// (no hostnames, usernames, serial numbers, or local file paths).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DeviceSpecs {
    pub(crate) os_version: String,
    pub(crate) arch: String,
    pub(crate) cpu_model: String,
    pub(crate) physical_cores: usize,
    pub(crate) logical_threads: usize,
    pub(crate) total_ram_mb: u64,
    pub(crate) gpu_name: String,
    pub(crate) gpu_backend: String,
    pub(crate) gpu_device_type: String,
    pub(crate) gpu_driver: String,
    pub(crate) display_width: u32,
    pub(crate) display_height: u32,
    pub(crate) display_refresh_hz: Option<f64>,
    pub(crate) sync_60hz: bool,
    pub(crate) audio_sample_rate_hz: u32,
}

impl DeviceSpecs {
    pub(crate) fn collect_host_baseline() -> Self {
        let host = crate::bench::collect_host_system_info();
        Self {
            os_version: sanitize_display_text(&host.os_version),
            arch: std::env::consts::ARCH.to_string(),
            cpu_model: sanitize_display_text(&host.cpu_model),
            physical_cores: host.physical_cores,
            logical_threads: host.logical_threads,
            total_ram_mb: host.total_ram_mb,
            gpu_name: String::new(),
            gpu_backend: String::new(),
            gpu_device_type: String::new(),
            gpu_driver: String::new(),
            display_width: 0,
            display_height: 0,
            display_refresh_hz: None,
            sync_60hz: false,
            audio_sample_rate_hz: 0,
        }
    }

    pub(crate) fn gpu_detailed_summary(&self) -> String {
        let base = if !self.gpu_name.is_empty() {
            self.gpu_name.clone()
        } else if !self.gpu_backend.is_empty() {
            self.gpu_backend.clone()
        } else {
            "Unknown".to_string()
        };
        let mut details = Vec::new();
        if !self.gpu_backend.is_empty() && self.gpu_backend != base {
            details.push(self.gpu_backend.clone());
        }
        if !self.gpu_device_type.is_empty() {
            details.push(self.gpu_device_type.clone());
        }
        if !self.gpu_driver.is_empty() {
            details.push(format!("Driver: {}", self.gpu_driver));
        }
        if details.is_empty() {
            base
        } else {
            format!("{base} ({})", details.join(" · "))
        }
    }

    pub(crate) fn cpu_summary(&self) -> String {
        let model = if self.cpu_model.is_empty() {
            "Unknown CPU"
        } else {
            self.cpu_model.as_str()
        };
        if self.physical_cores > 0 && self.logical_threads > 0 {
            format!(
                "{model} ({}C / {}T)",
                self.physical_cores, self.logical_threads
            )
        } else if self.logical_threads > 0 {
            format!("{model} ({} threads)", self.logical_threads)
        } else {
            model.to_string()
        }
    }

    pub(crate) fn ram_summary(&self) -> String {
        if self.total_ram_mb == 0 {
            "Unknown".to_string()
        } else {
            let gb = (self.total_ram_mb as f64) / 1024.0;
            format!("{gb:.1} GB ({} MB)", self.total_ram_mb)
        }
    }

    pub(crate) fn display_summary(&self) -> String {
        let res = if self.display_width > 0 && self.display_height > 0 {
            format!("{}x{}", self.display_width, self.display_height)
        } else {
            "Unknown resolution".to_string()
        };
        let hz = match self.display_refresh_hz {
            Some(r) if r.is_finite() && r > 0.0 => format!(" @ {r:.1} Hz"),
            _ => String::new(),
        };
        let lock = if self.sync_60hz {
            " · 60 Hz lock: On"
        } else {
            " · 60 Hz lock: Off"
        };
        format!("{res}{hz}{lock}")
    }

    pub(crate) fn audio_summary(&self) -> String {
        if self.audio_sample_rate_hz > 0 {
            format!("{} Hz", self.audio_sample_rate_hz)
        } else {
            "Unknown".to_string()
        }
    }

    pub(crate) fn one_line_summary(&self) -> String {
        let os = if self.os_version.is_empty() {
            std::env::consts::OS.to_string()
        } else if !self.arch.is_empty() {
            format!("{} ({})", self.os_version, self.arch)
        } else {
            self.os_version.clone()
        };
        format!(
            "{os} · {} · {} RAM · GPU: {} · Display: {} · Audio: {}",
            self.cpu_summary(),
            self.ram_summary(),
            self.gpu_detailed_summary(),
            self.display_summary(),
            self.audio_summary(),
        )
    }

    fn is_populated(&self) -> bool {
        !self.os_version.is_empty() || !self.cpu_model.is_empty() || !self.gpu_name.is_empty()
    }
}

/// Live runtime GPU/display/audio environment update passed outside the hot frame loop.
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeDeviceUpdate<'a> {
    pub(crate) gpu_name: &'a str,
    pub(crate) gpu_backend: &'a str,
    pub(crate) gpu_device_type: &'a str,
    pub(crate) gpu_driver: &'a str,
    pub(crate) display_width: u32,
    pub(crate) display_height: u32,
    pub(crate) display_refresh_hz: Option<f64>,
    pub(crate) sync_60hz: bool,
    pub(crate) audio_sample_rate_hz: u32,
}

/// Persisted per-ROM playtest record serialized in `playtest_results.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlaytestGameRecord {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) code: String,
    pub(crate) platform: String,
    pub(crate) crc32: u32,
    pub(crate) save_type: String,
    pub(crate) hardware_extras: String,
    pub(crate) bios_mode: String,
    pub(crate) real_bios_hint: Option<String>,
    pub(crate) renderer: String,
    pub(crate) filter: String,
    pub(crate) verdict: PlaytestVerdict,
    pub(crate) sessions: u32,
    pub(crate) total_frames: u64,
    pub(crate) total_play_secs: u64,
    pub(crate) avg_fps: f32,
    pub(crate) avg_emu_ms: f32,
    pub(crate) p95_emu_ms: f32,
    pub(crate) avg_frame_ms: f32,
    pub(crate) p95_frame_ms: f32,
    pub(crate) audio_underruns: u64,
    pub(crate) frameskip_frames: u64,
    pub(crate) video_active: bool,
    pub(crate) save_written: bool,
    pub(crate) last_played_unix: u64,
    #[serde(default)]
    pub(crate) device: DeviceSpecs,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PlaytestLedger {
    version: u32,
    #[serde(default)]
    last_device: DeviceSpecs,
    records: Vec<PlaytestGameRecord>,
}

const LEDGER_SCHEMA_VERSION: u32 = 2;

struct ActiveSession {
    meta: GameSessionMeta,
    session_counted_in_ledger: bool,
    dirty_since_flush: bool,
    total_frames: u64,
    normal_frames: u64,
    normal_wall_secs: f64,
    active_wall_secs: f64,
    last_active_tick: Option<Instant>,
    emu_ms_sum: f64,
    frame_ms_sum: f64,
    emu_samples_ms: Box<[f32; MAX_TIMING_SAMPLES]>,
    frame_samples_ms: Box<[f32; MAX_TIMING_SAMPLES]>,
    sample_len: usize,
    sample_cursor: usize,
    frameskip_frames: u64,
    last_seen_audio_underruns: u64,
    session_audio_underruns: u64,
    video_active: bool,
    save_written: bool,
    flushed_frames: u64,
    flushed_play_secs: u64,
    last_flush_at: Instant,
}

/// Per-frame telemetry snapshot passed from the desktop frontend loop (Copy, zero allocations).
#[derive(Copy, Clone, Debug)]
pub(crate) struct FrameSample {
    pub(crate) emu_ms: f32,
    pub(crate) present_ms: f32,
    pub(crate) normal_speed: bool,
    pub(crate) frameskipped: bool,
    pub(crate) save_dirty: bool,
    pub(crate) audio_underruns_now: u64,
}

impl ActiveSession {
    fn new(meta: GameSessionMeta, initial_audio_underruns: u64) -> Self {
        Self {
            meta,
            session_counted_in_ledger: false,
            dirty_since_flush: true,
            total_frames: 0,
            normal_frames: 0,
            normal_wall_secs: 0.0,
            active_wall_secs: 0.0,
            last_active_tick: Some(Instant::now()),
            emu_ms_sum: 0.0,
            frame_ms_sum: 0.0,
            emu_samples_ms: Box::new([0.0; MAX_TIMING_SAMPLES]),
            frame_samples_ms: Box::new([0.0; MAX_TIMING_SAMPLES]),
            sample_len: 0,
            sample_cursor: 0,
            frameskip_frames: 0,
            last_seen_audio_underruns: initial_audio_underruns,
            session_audio_underruns: 0,
            video_active: false,
            save_written: false,
            flushed_frames: 0,
            flushed_play_secs: 0,
            last_flush_at: Instant::now(),
        }
    }

    #[inline]
    fn record_frame(&mut self, sample: FrameSample, framebuffer: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        debug_assert!(self.sample_len <= MAX_TIMING_SAMPLES);
        debug_assert!(self.sample_cursor < MAX_TIMING_SAMPLES);

        let now = Instant::now();
        let tick_dt = match self.last_active_tick {
            Some(prev) => now
                .saturating_duration_since(prev)
                .as_secs_f64()
                .min(MAX_ACTIVE_TICK_SECS),
            None => 0.0,
        };
        self.last_active_tick = Some(now);
        self.active_wall_secs += tick_dt;
        self.total_frames = self.total_frames.saturating_add(1);
        self.dirty_since_flush = true;

        if sample.save_dirty {
            self.save_written = true;
        }

        // Accumulate audio underrun deltas even if the frontend resets the counter on unpause.
        if sample.audio_underruns_now >= self.last_seen_audio_underruns {
            self.session_audio_underruns = self
                .session_audio_underruns
                .saturating_add(sample.audio_underruns_now - self.last_seen_audio_underruns);
        } else {
            self.session_audio_underruns = self
                .session_audio_underruns
                .saturating_add(sample.audio_underruns_now);
        }
        self.last_seen_audio_underruns = sample.audio_underruns_now;

        if !self.video_active
            && (self.total_frames == 1
                || self
                    .total_frames
                    .is_multiple_of(VIDEO_ACTIVITY_CHECK_INTERVAL_FRAMES))
            && framebuffer_has_active_image(framebuffer)
        {
            self.video_active = true;
        }

        if sample.normal_speed {
            self.normal_frames = self.normal_frames.saturating_add(1);
            self.normal_wall_secs += tick_dt;
            if sample.frameskipped {
                self.frameskip_frames = self.frameskip_frames.saturating_add(1);
            }
            let clamped_emu = if sample.emu_ms.is_finite() && sample.emu_ms >= 0.0 {
                sample.emu_ms
            } else {
                0.0
            };
            let clamped_present = if sample.present_ms.is_finite() && sample.present_ms >= 0.0 {
                sample.present_ms
            } else {
                0.0
            };
            let total_ms = clamped_emu + clamped_present;
            self.emu_ms_sum += clamped_emu as f64;
            self.frame_ms_sum += total_ms as f64;

            self.emu_samples_ms[self.sample_cursor] = clamped_emu;
            self.frame_samples_ms[self.sample_cursor] = total_ms;
            self.sample_cursor = (self.sample_cursor + 1) % MAX_TIMING_SAMPLES;
            if self.sample_len < MAX_TIMING_SAMPLES {
                self.sample_len += 1;
            }
        }
    }

    fn pause_clock(&mut self) {
        self.last_active_tick = None;
    }

    fn compute_metrics(&self) -> SessionSnapshot {
        let avg_emu_ms = if self.normal_frames > 0 {
            (self.emu_ms_sum / self.normal_frames as f64) as f32
        } else {
            0.0
        };
        let avg_frame_ms = if self.normal_frames > 0 {
            (self.frame_ms_sum / self.normal_frames as f64) as f32
        } else {
            0.0
        };
        let p95_emu_ms = percentile_95(&self.emu_samples_ms[..self.sample_len]);
        let p95_frame_ms = percentile_95(&self.frame_samples_ms[..self.sample_len]);
        let avg_fps = if self.normal_frames > 0 && self.normal_wall_secs >= 0.25 {
            ((self.normal_frames as f64) / self.normal_wall_secs).min(120.0) as f32
        } else if self.normal_frames > 0 && avg_frame_ms > 0.0 {
            (1000.0 / avg_frame_ms.max(16.7427)).min(60.0)
        } else {
            0.0
        };
        let verdict = classify_verdict(
            self.total_frames,
            self.normal_frames,
            avg_fps,
            p95_frame_ms,
            self.frameskip_frames,
            self.session_audio_underruns,
            self.video_active,
        );
        SessionSnapshot {
            verdict,
            avg_fps,
            avg_emu_ms,
            p95_emu_ms,
            avg_frame_ms,
            p95_frame_ms,
            audio_underruns: self.session_audio_underruns,
            frameskip_frames: self.frameskip_frames,
            video_active: self.video_active,
            save_written: self.save_written,
            play_secs: self.active_wall_secs.round() as u64,
            total_frames: self.total_frames,
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct SessionSnapshot {
    verdict: PlaytestVerdict,
    avg_fps: f32,
    avg_emu_ms: f32,
    p95_emu_ms: f32,
    avg_frame_ms: f32,
    p95_frame_ms: f32,
    audio_underruns: u64,
    frameskip_frames: u64,
    video_active: bool,
    save_written: bool,
    play_secs: u64,
    total_frames: u64,
}

/// Manages active playtest tracking and live Markdown report generation.
pub(crate) struct PlaytestTracker {
    enabled: bool,
    ledger_path: PathBuf,
    report_path: PathBuf,
    fallback_report_path: PathBuf,
    device_specs: DeviceSpecs,
    ledger: PlaytestLedger,
    active: Option<ActiveSession>,
}

impl PlaytestTracker {
    /// Creates a desktop tracker that writes to `<exe_dir>/docs/PLAYTEST_RESULTS.md` when a
    /// `docs/` directory sits next to the executable, falling back to `<app_data_dir>/docs/`.
    pub(crate) fn new(enabled: bool) -> Self {
        let ledger_path = resolve_ledger_path();
        let report_path = resolve_report_path();
        let fallback_report_path = AppConfig::app_data_dir()
            .join("docs")
            .join(PLAYTEST_REPORT_FILENAME);
        Self::with_paths(enabled, ledger_path, report_path, fallback_report_path)
    }

    pub(crate) fn with_paths(
        enabled: bool,
        ledger_path: PathBuf,
        report_path: PathBuf,
        fallback_report_path: PathBuf,
    ) -> Self {
        let device_specs = if enabled {
            DeviceSpecs::collect_host_baseline()
        } else {
            DeviceSpecs::default()
        };
        let mut ledger = if enabled {
            load_ledger(&ledger_path)
        } else {
            PlaytestLedger {
                version: LEDGER_SCHEMA_VERSION,
                last_device: DeviceSpecs::default(),
                records: Vec::new(),
            }
        };
        if enabled && !ledger.last_device.is_populated() {
            ledger.last_device = device_specs.clone();
        }
        Self {
            enabled,
            ledger_path,
            report_path,
            fallback_report_path,
            device_specs,
            ledger,
            active: None,
        }
    }

    /// Updates live GPU/display/audio specs detected by the desktop frontend.
    pub(crate) fn update_device_specs(&mut self, update: RuntimeDeviceUpdate<'_>) {
        if !update.gpu_name.is_empty() && update.gpu_name != "Unknown" {
            self.device_specs.gpu_name = sanitize_display_text(update.gpu_name);
        }
        if !update.gpu_backend.is_empty() && update.gpu_backend != "Unknown" {
            self.device_specs.gpu_backend = sanitize_display_text(update.gpu_backend);
        }
        if !update.gpu_device_type.is_empty() {
            self.device_specs.gpu_device_type = sanitize_display_text(update.gpu_device_type);
        }
        if !update.gpu_driver.is_empty() {
            self.device_specs.gpu_driver = sanitize_display_text(update.gpu_driver);
        }
        if update.display_width > 0 && update.display_height > 0 {
            self.device_specs.display_width = update.display_width;
            self.device_specs.display_height = update.display_height;
        }
        if let Some(hz) = update.display_refresh_hz
            && hz.is_finite()
            && hz > 0.0
        {
            self.device_specs.display_refresh_hz = Some(hz);
        }
        self.device_specs.sync_60hz = update.sync_60hz;
        if update.audio_sample_rate_hz > 0 {
            self.device_specs.audio_sample_rate_hz = update.audio_sample_rate_hz;
        }
        if self.enabled {
            self.ledger.last_device = self.device_specs.clone();
        }
    }

    /// Directory where `PLAYTEST_RESULTS.md` is written (for "Open Playtest Results Folder").
    pub(crate) fn report_dir(&self) -> PathBuf {
        let target =
            if self.report_path.is_file() || self.report_path.parent().is_some_and(Path::is_dir) {
                &self.report_path
            } else {
                &self.fallback_report_path
            };
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| AppConfig::app_data_dir().join("docs"))
    }

    /// Begins tracking a newly loaded game, flushing any previous active session first.
    pub(crate) fn begin_session(&mut self, meta: GameSessionMeta, current_audio_underruns: u64) {
        if !self.enabled {
            return;
        }
        self.end_session();
        self.active = Some(ActiveSession::new(meta, current_audio_underruns));
    }

    /// Updates live renderer/filter metadata (e.g. after wgpu init or filter switch).
    pub(crate) fn update_environment(
        &mut self,
        renderer: &str,
        filter: &str,
        save_type: Option<&str>,
    ) {
        let Some(active) = &mut self.active else {
            return;
        };
        if !renderer.is_empty() {
            active.meta.renderer = sanitize_display_text(renderer);
        }
        if !filter.is_empty() {
            active.meta.filter = sanitize_display_text(filter);
        }
        if let Some(st) = save_type
            && !st.is_empty()
        {
            active.meta.save_type = sanitize_display_text(st);
        }
    }

    /// Records a single executed emulation frame (zero heap allocations).
    #[inline]
    pub(crate) fn record_frame(
        &mut self,
        sample: FrameSample,
        framebuffer: &[u16; LCD_WIDTH * LCD_HEIGHT],
    ) {
        let Some(active) = &mut self.active else {
            return;
        };
        active.record_frame(sample, framebuffer);
    }

    /// Pauses wall-clock playtime tracking and flushes results if gameplay happened.
    pub(crate) fn on_pause(&mut self) {
        if let Some(active) = &mut self.active {
            active.pause_clock();
        }
        self.flush_if_needed(true);
    }

    /// Resumes wall-clock playtime tracking after unpausing.
    pub(crate) fn on_resume(&mut self, current_audio_underruns: u64) {
        if let Some(active) = &mut self.active {
            active.last_active_tick = Some(Instant::now());
            active.last_seen_audio_underruns = current_audio_underruns;
        }
    }

    /// Returns `true` when enough time has elapsed to perform a periodic flush.
    pub(crate) fn is_periodic_flush_due(&self) -> bool {
        self.enabled
            && self.active.as_ref().is_some_and(|a| {
                a.total_frames > 0
                    && a.dirty_since_flush
                    && a.last_flush_at.elapsed() >= PERIODIC_FLUSH_INTERVAL
            })
    }

    /// Periodically persists the active session every `PERIODIC_FLUSH_INTERVAL`.
    pub(crate) fn maybe_flush_periodic(&mut self) {
        self.flush_if_needed(false);
    }

    /// Flushes and closes the active game session (called on Close ROM, ROM switch, or Exit).
    pub(crate) fn end_session(&mut self) {
        if !self.enabled {
            self.active = None;
            return;
        }
        self.flush_if_needed(true);
        self.active = None;
    }

    /// Writes the current ledger + Markdown report to disk immediately (used on explicit user actions).
    pub(crate) fn flush_now(&mut self) {
        if !self.enabled {
            return;
        }
        self.flush_if_needed(true);
        if self.active.is_none() {
            self.write_outputs();
        }
    }

    fn flush_if_needed(&mut self, force: bool) {
        if !self.enabled {
            return;
        }
        let Some(active) = &mut self.active else {
            return;
        };
        if active.total_frames == 0 || !active.dirty_since_flush {
            return;
        }
        if !force && active.last_flush_at.elapsed() < PERIODIC_FLUSH_INTERVAL {
            return;
        }

        let snap = active.compute_metrics();
        let delta_frames = snap.total_frames.saturating_sub(active.flushed_frames);
        let delta_secs = snap.play_secs.saturating_sub(active.flushed_play_secs);
        let increment_session = !active.session_counted_in_ledger;
        let now_unix = now_unix_secs();

        merge_session_into_ledger(
            &mut self.ledger,
            &active.meta,
            &self.device_specs,
            snap,
            delta_frames,
            delta_secs,
            increment_session,
            now_unix,
        );

        active.flushed_frames = snap.total_frames;
        active.flushed_play_secs = snap.play_secs;
        active.session_counted_in_ledger = true;
        active.dirty_since_flush = false;
        active.last_flush_at = Instant::now();

        self.write_outputs();
    }

    fn write_outputs(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(&self.ledger)
            && let Err(err) = write_atomic(&self.ledger_path, &json)
        {
            log::warn!(
                "Failed writing playtest ledger {}: {err}",
                self.ledger_path.display()
            );
        }
        let report_device = if self.device_specs.is_populated() {
            &self.device_specs
        } else {
            &self.ledger.last_device
        };
        let md = render_markdown_report(&self.ledger.records, report_device, now_unix_secs());
        if let Err(primary_err) = write_atomic(&self.report_path, md.as_bytes()) {
            if self.fallback_report_path != self.report_path {
                if let Err(fb_err) = write_atomic(&self.fallback_report_path, md.as_bytes()) {
                    log::warn!(
                        "Failed writing playtest report ({} -> {primary_err}; fallback {} -> {fb_err})",
                        self.report_path.display(),
                        self.fallback_report_path.display()
                    );
                }
            } else {
                log::warn!(
                    "Failed writing playtest report {}: {primary_err}",
                    self.report_path.display()
                );
            }
        }
    }
}

pub(crate) fn resolve_ledger_path() -> PathBuf {
    AppConfig::app_data_dir().join(PLAYTEST_LEDGER_FILENAME)
}

pub(crate) fn resolve_report_path() -> PathBuf {
    #[cfg(test)]
    {
        AppConfig::app_data_dir()
            .join("docs")
            .join(PLAYTEST_REPORT_FILENAME)
    }
    #[cfg(not(test))]
    {
        if let Ok(exe) = std::env::current_exe()
            && let Some(exe_dir) = exe.parent()
        {
            let exe_docs = exe_dir.join("docs");
            if exe_docs.is_dir() {
                return exe_docs.join(PLAYTEST_REPORT_FILENAME);
            }
        }
        AppConfig::app_data_dir()
            .join("docs")
            .join(PLAYTEST_REPORT_FILENAME)
    }
}

fn load_ledger(path: &Path) -> PlaytestLedger {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<PlaytestLedger>(&text) {
            Ok(mut ledger) => {
                ledger.version = LEDGER_SCHEMA_VERSION;
                if ledger.records.len() > MAX_RECORDED_GAMES {
                    ledger.records.truncate(MAX_RECORDED_GAMES);
                }
                ledger
            }
            Err(_) => PlaytestLedger {
                version: LEDGER_SCHEMA_VERSION,
                last_device: DeviceSpecs::default(),
                records: Vec::new(),
            },
        },
        Err(_) => PlaytestLedger {
            version: LEDGER_SCHEMA_VERSION,
            last_device: DeviceSpecs::default(),
            records: Vec::new(),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn merge_session_into_ledger(
    ledger: &mut PlaytestLedger,
    meta: &GameSessionMeta,
    device: &DeviceSpecs,
    snap: SessionSnapshot,
    delta_frames: u64,
    delta_secs: u64,
    increment_session: bool,
    now_unix: u64,
) {
    ledger.last_device = device.clone();
    let key = meta.record_key();
    if let Some(existing) = ledger.records.iter_mut().find(|r| r.key == key) {
        if increment_session {
            existing.sessions = existing.sessions.saturating_add(1);
        }
        existing.total_frames = existing.total_frames.saturating_add(delta_frames);
        existing.total_play_secs = existing.total_play_secs.saturating_add(delta_secs);
        existing.save_written |= snap.save_written;
        existing.video_active |= snap.video_active;
        existing.last_played_unix = now_unix;
        existing.title = meta.title.clone();
        existing.save_type = meta.save_type.clone();
        existing.hardware_extras = meta.hardware_extras.clone();
        existing.real_bios_hint = meta.real_bios_hint.clone();

        // Preserve a substantive verdict/metrics if this new run was just an accidental <3s launch.
        if snap.verdict.is_substantive() || !existing.verdict.is_substantive() {
            existing.verdict = snap.verdict;
            existing.bios_mode = meta.bios_mode.clone();
            existing.renderer = meta.renderer.clone();
            existing.filter = meta.filter.clone();
            existing.avg_fps = snap.avg_fps;
            existing.avg_emu_ms = snap.avg_emu_ms;
            existing.p95_emu_ms = snap.p95_emu_ms;
            existing.avg_frame_ms = snap.avg_frame_ms;
            existing.p95_frame_ms = snap.p95_frame_ms;
            existing.audio_underruns = snap.audio_underruns;
            existing.frameskip_frames = snap.frameskip_frames;
            existing.device = device.clone();
        }
    } else {
        if ledger.records.len() >= MAX_RECORDED_GAMES {
            // Evict the oldest-played entry to respect the explicit upper bound.
            if let Some((oldest_idx, _)) = ledger
                .records
                .iter()
                .enumerate()
                .min_by_key(|(_, r)| r.last_played_unix)
            {
                ledger.records.swap_remove(oldest_idx);
            }
        }
        ledger.records.push(PlaytestGameRecord {
            key,
            title: meta.title.clone(),
            code: meta.code.clone(),
            platform: meta.platform.clone(),
            crc32: meta.crc32,
            save_type: meta.save_type.clone(),
            hardware_extras: meta.hardware_extras.clone(),
            bios_mode: meta.bios_mode.clone(),
            real_bios_hint: meta.real_bios_hint.clone(),
            renderer: meta.renderer.clone(),
            filter: meta.filter.clone(),
            verdict: snap.verdict,
            sessions: 1,
            total_frames: delta_frames,
            total_play_secs: delta_secs,
            avg_fps: snap.avg_fps,
            avg_emu_ms: snap.avg_emu_ms,
            p95_emu_ms: snap.p95_emu_ms,
            avg_frame_ms: snap.avg_frame_ms,
            p95_frame_ms: snap.p95_frame_ms,
            audio_underruns: snap.audio_underruns,
            frameskip_frames: snap.frameskip_frames,
            video_active: snap.video_active,
            save_written: snap.save_written,
            last_played_unix: now_unix,
            device: device.clone(),
        });
    }

    // Most recently played games appear first in the table.
    ledger.records.sort_by(|a, b| {
        b.last_played_unix
            .cmp(&a.last_played_unix)
            .then_with(|| a.title.cmp(&b.title))
    });
}

#[inline]
fn framebuffer_has_active_image(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> bool {
    let first = fb[0];
    let mut i = VIDEO_ACTIVITY_PIXEL_STRIDE;
    while i < fb.len() {
        if fb[i] != first {
            return true;
        }
        i += VIDEO_ACTIVITY_PIXEL_STRIDE;
    }
    // Also detect non-black / non-white uniform color fills (e.g. bright GBA splash).
    let masked = first & 0x7FFF;
    masked != 0x0000 && masked != 0x7FFF
}

fn percentile_95(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = Vec::with_capacity(samples.len());
    sorted.extend_from_slice(samples);
    sorted.sort_by(|a, b| a.total_cmp(b));
    let idx = ((sorted.len() - 1) * 95) / 100;
    debug_assert!(idx < sorted.len());
    sorted[idx]
}

fn classify_verdict(
    total_frames: u64,
    normal_frames: u64,
    avg_fps: f32,
    p95_frame_ms: f32,
    frameskip_frames: u64,
    audio_underruns: u64,
    video_active: bool,
) -> PlaytestVerdict {
    if total_frames < MIN_VERDICT_FRAMES {
        return PlaytestVerdict::BriefLaunch;
    }
    if !video_active {
        return PlaytestVerdict::BlankScreen;
    }
    let skip_ratio = if normal_frames > 0 {
        (frameskip_frames as f64) / (normal_frames as f64)
    } else {
        0.0
    };
    if (normal_frames >= MIN_NORMAL_SPEED_FRAMES && avg_fps > 0.0 && avg_fps < 50.0)
        || skip_ratio >= 0.25
    {
        return PlaytestVerdict::Slow;
    }
    if (normal_frames >= MIN_NORMAL_SPEED_FRAMES && avg_fps > 0.0 && avg_fps < 57.0)
        || audio_underruns >= 5
        || frameskip_frames > 0
        || (normal_frames >= MIN_NORMAL_SPEED_FRAMES && p95_frame_ms > 20.0)
    {
        return PlaytestVerdict::MinorStutter;
    }
    PlaytestVerdict::Smooth
}

/// Renders the complete `PLAYTEST_RESULTS.md` report from persisted game records and device specs.
pub(crate) fn render_markdown_report(
    records: &[PlaytestGameRecord],
    device: &DeviceSpecs,
    generated_unix: u64,
) -> String {
    let mut out = String::with_capacity(2560 + records.len() * 420);
    let version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let _ = writeln!(out, "# Rustboy Advance — Playtest Results");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "> Automatically updated whenever a game is played in Rustboy Advance."
    );
    let _ = writeln!(
        out,
        "> No personal folders, hostnames, or usernames are recorded — safe to share back."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **Emulator:** Rustboy Advance v{version} (`{os}` / `{arch}`)"
    );
    if generated_unix > 0 {
        let _ = writeln!(
            out,
            "- **Last Updated (UTC):** `{}`",
            unix_secs_to_iso8601_utc(generated_unix)
        );
    }

    if device.is_populated() {
        let os_display = if device.os_version.is_empty() {
            format!("{os} ({arch})")
        } else if !device.arch.is_empty() {
            format!("{} ({})", device.os_version, device.arch)
        } else {
            device.os_version.clone()
        };
        let _ = writeln!(out);
        let _ = writeln!(out, "## Device Specs");
        let _ = writeln!(out);
        let _ = writeln!(out, "| Component | Details |");
        let _ = writeln!(out, "| :--- | :--- |");
        let _ = writeln!(out, "| **OS** | {} |", escape_md_cell(&os_display));
        let _ = writeln!(
            out,
            "| **CPU** | {} |",
            escape_md_cell(&device.cpu_summary())
        );
        let _ = writeln!(
            out,
            "| **Memory** | {} |",
            escape_md_cell(&device.ram_summary())
        );
        let _ = writeln!(
            out,
            "| **GPU / Backend** | {} |",
            escape_md_cell(&device.gpu_detailed_summary())
        );
        let _ = writeln!(
            out,
            "| **Display** | {} |",
            escape_md_cell(&device.display_summary())
        );
        let _ = writeln!(
            out,
            "| **Audio** | {} |",
            escape_md_cell(&device.audio_summary())
        );
    }

    if records.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "- **Games Tested:** 0");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "_No games played yet. Launch any `.gba`, `.gb`, or `.gbc` ROM in `rustboy-advance.exe` and this file will automatically record your device specs, each game played, and how well it ran._"
        );
        return out;
    }

    let mut smooth = 0usize;
    let mut minor = 0usize;
    let mut slow = 0usize;
    let mut blank = 0usize;
    let mut brief = 0usize;
    for r in records {
        match r.verdict {
            PlaytestVerdict::Smooth => smooth += 1,
            PlaytestVerdict::MinorStutter => minor += 1,
            PlaytestVerdict::Slow => slow += 1,
            PlaytestVerdict::BlankScreen => blank += 1,
            PlaytestVerdict::BriefLaunch => brief += 1,
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **Games Tested:** {} ({smooth} Smooth, {minor} Minor Stutter, {slow} Slow, {blank} Blank Screen, {brief} Brief Launch)",
        records.len()
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Results Summary");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Game | System | Code | Result | Play Time | Avg FPS | Emu / Frame (p95) | Audio Underruns | Save | BIOS | Renderer | Notes |"
    );
    let _ = writeln!(
        out,
        "| :--- | :---: | :---: | :--- | :---: | :---: | :---: | :---: | :--- | :--- | :--- | :--- |"
    );

    for r in records {
        let fps_col = if r.avg_fps > 0.0 {
            format!("{:.1}", r.avg_fps)
        } else {
            "-".to_string()
        };
        let timing_col = if r.p95_frame_ms > 0.0 {
            format!("{:.2} / {:.2} ms", r.p95_emu_ms, r.p95_frame_ms)
        } else {
            "-".to_string()
        };
        let save_col = format_save_summary(r);
        let notes_col = format_summary_notes(r);
        let _ = writeln!(
            out,
            "| **{}** | {} | `{}` | **{}** | {} | {} | {} | {} | {} | {} | {} | {} |",
            escape_md_cell(&r.title),
            escape_md_cell(&r.platform),
            escape_md_cell(&r.code),
            r.verdict.label(),
            format_duration_short(r.total_play_secs),
            fps_col,
            timing_col,
            r.audio_underruns,
            escape_md_cell(&save_col),
            escape_md_cell(&r.bios_mode),
            escape_md_cell(&r.renderer),
            escape_md_cell(&notes_col),
        );
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "## Session Details");

    for r in records {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "### {} (`{}` · `0x{:08X}`)",
            escape_md_inline(&r.title),
            escape_md_inline(&r.code),
            r.crc32
        );
        let _ = writeln!(out, "- **Result:** {}", r.verdict.label());
        let _ = writeln!(
            out,
            "- **System / Hardware:** {} · Save: `{}` · Extras: `{}` · Battery Save Written: {}",
            escape_md_inline(&r.platform),
            escape_md_inline(&r.save_type),
            escape_md_inline(&r.hardware_extras),
            if r.save_written { "Yes" } else { "No" }
        );
        let _ = writeln!(out, "- **BIOS Mode:** {}", escape_md_inline(&r.bios_mode));
        if let Some(hint) = &r.real_bios_hint {
            let _ = writeln!(
                out,
                "- **BIOS Note:** Real 16 KB GBA BIOS recommended ({})",
                escape_md_inline(hint)
            );
        }
        let _ = writeln!(
            out,
            "- **Performance:** Avg `{:.1} FPS` · Core `{:.2} ms` avg / `{:.2} ms` p95 · Total Frame `{:.2} ms` avg / `{:.2} ms` p95",
            r.avg_fps, r.avg_emu_ms, r.p95_emu_ms, r.avg_frame_ms, r.p95_frame_ms
        );
        let _ = writeln!(
            out,
            "- **Stability:** `{}` audio underrun(s) · `{}` frameskipped frame(s) · Video active: {}",
            r.audio_underruns,
            r.frameskip_frames,
            if r.video_active {
                "Yes"
            } else {
                "No (uniform/blank screen)"
            }
        );
        let _ = writeln!(
            out,
            "- **Environment:** `{}` · Filter: `{}`",
            escape_md_inline(&r.renderer),
            escape_md_inline(&r.filter)
        );
        if r.device.is_populated() {
            let _ = writeln!(
                out,
                "- **Device Used:** {}",
                escape_md_inline(&r.device.one_line_summary())
            );
        }
        let _ = writeln!(
            out,
            "- **History:** `{}` session(s) · `{}` total play time · `{}` frames · Last played `{}`",
            r.sessions,
            format_duration_short(r.total_play_secs),
            r.total_frames,
            unix_secs_to_iso8601_utc(r.last_played_unix)
        );
    }

    out
}

pub(crate) fn format_gba_save_type(save_type: SaveType) -> &'static str {
    match save_type {
        SaveType::None => "None",
        SaveType::Sram32K => "SRAM 32K",
        SaveType::Flash64K => "Flash 64K",
        SaveType::Flash128K => "Flash 128K",
        SaveType::Eeprom512B => "EEPROM 512B",
        SaveType::Eeprom8K => "EEPROM 8K",
    }
}

fn format_save_summary(r: &PlaytestGameRecord) -> String {
    let mut tags = Vec::new();
    if r.hardware_extras != "Standard" && !r.hardware_extras.is_empty() {
        tags.push(r.hardware_extras.as_str());
    }
    if r.save_written {
        tags.push("saved");
    }
    if tags.is_empty() {
        r.save_type.clone()
    } else {
        format!("{} ({})", r.save_type, tags.join(", "))
    }
}

fn format_summary_notes(r: &PlaytestGameRecord) -> String {
    let mut notes = Vec::new();
    if r.verdict == PlaytestVerdict::BlankScreen {
        notes.push("Blank/uniform screen — try real BIOS or save override".to_string());
    } else if r.verdict == PlaytestVerdict::BriefLaunch {
        notes.push(format!("Short run ({} frames)", r.total_frames));
    }
    if r.frameskip_frames > 0 {
        notes.push(format!("Frameskip: {}", r.frameskip_frames));
    }
    if let Some(hint) = &r.real_bios_hint
        && r.bios_mode.starts_with("HLE")
    {
        notes.push(format!("Real BIOS recommended ({hint})"));
    }
    if notes.is_empty() {
        format!("{} · {} frames", r.filter, r.total_frames)
    } else {
        notes.join("; ")
    }
}

fn format_duration_short(total_secs: u64) -> String {
    let hrs = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hrs > 0 {
        format!("{hrs}h {mins:02}m {secs:02}s")
    } else if mins > 0 {
        format!("{mins}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

fn sanitize_display_text(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn escape_md_cell(s: &str) -> String {
    sanitize_display_text(s).replace('|', "\\|")
}

fn escape_md_inline(s: &str) -> String {
    sanitize_display_text(s).replace('`', "'")
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Converts a Unix epoch timestamp (seconds) into a UTC ISO-8601 string (`YYYY-MM-DDTHH:MM:SSZ`)
/// using Howard Hinnant's civil-from-days integer algorithm.
pub(crate) fn unix_secs_to_iso8601_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = y + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta(title: &str, code: &str, crc32: u32) -> GameSessionMeta {
        GameSessionMeta {
            title: title.to_string(),
            code: code.to_string(),
            platform: "GBA".to_string(),
            crc32,
            save_type: "Flash 128K".to_string(),
            hardware_extras: "RTC".to_string(),
            bios_mode: "HLE (built-in)".to_string(),
            real_bios_hint: None,
            renderer: "Vulkan (Test GPU)".to_string(),
            filter: "Sharp 1x".to_string(),
        }
    }

    #[test]
    fn utc_iso8601_formats_known_epochs_and_leap_day() {
        assert_eq!(unix_secs_to_iso8601_utc(0), "1970-01-01T00:00:00Z");
        // 2024-02-29T12:34:56Z
        assert_eq!(
            unix_secs_to_iso8601_utc(1_709_210_096),
            "2024-02-29T12:34:56Z"
        );
        // 2026-11-21T14:58:00Z
        assert_eq!(
            unix_secs_to_iso8601_utc(1_795_273_080),
            "2026-11-21T14:58:00Z"
        );
    }

    #[test]
    fn verdict_classification_covers_all_outcomes() {
        assert_eq!(
            classify_verdict(60, 60, 59.7, 1.5, 0, 0, true),
            PlaytestVerdict::BriefLaunch
        );
        assert_eq!(
            classify_verdict(240, 240, 59.7, 1.5, 0, 0, false),
            PlaytestVerdict::BlankScreen
        );
        assert_eq!(
            classify_verdict(300, 300, 42.0, 24.0, 0, 0, true),
            PlaytestVerdict::Slow
        );
        assert_eq!(
            classify_verdict(300, 300, 59.5, 4.0, 100, 0, true),
            PlaytestVerdict::Slow
        );
        assert_eq!(
            classify_verdict(300, 300, 55.0, 15.0, 0, 0, true),
            PlaytestVerdict::MinorStutter
        );
        assert_eq!(
            classify_verdict(300, 300, 59.7, 4.0, 0, 6, true),
            PlaytestVerdict::MinorStutter
        );
        assert_eq!(
            classify_verdict(300, 300, 59.7, 2.1, 0, 0, true),
            PlaytestVerdict::Smooth
        );
    }

    #[test]
    fn tracker_writes_markdown_and_merges_repeat_sessions() {
        let base = std::env::temp_dir().join(format!(
            "rustboy_playtest_report_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let ledger_path = base.join("playtest_results.json");
        let report_path = base.join("docs").join(PLAYTEST_REPORT_FILENAME);
        let fallback_path = base.join("fallback").join(PLAYTEST_REPORT_FILENAME);

        let mut active_fb = [0u16; LCD_WIDTH * LCD_HEIGHT];
        for px in &mut active_fb[800..1600] {
            *px = 0x03E0; // non-uniform green band
        }

        let mut tracker = PlaytestTracker::with_paths(
            true,
            ledger_path.clone(),
            report_path.clone(),
            fallback_path.clone(),
        );
        tracker.update_device_specs(RuntimeDeviceUpdate {
            gpu_name: "Test RTX GPU",
            gpu_backend: "Vulkan",
            gpu_device_type: "DiscreteGpu",
            gpu_driver: "572.0",
            display_width: 1920,
            display_height: 1080,
            display_refresh_hz: Some(144.0),
            sync_60hz: false,
            audio_sample_rate_hz: 48_000,
        });
        let meta = sample_meta("POKEMON EMER | TEST", "BPEE", 0x1F1C_08FB);
        tracker.begin_session(meta.clone(), 0);
        for _ in 0..240 {
            tracker.record_frame(
                FrameSample {
                    emu_ms: 0.7,
                    present_ms: 0.8,
                    normal_speed: true,
                    frameskipped: false,
                    save_dirty: true,
                    audio_underruns_now: 0,
                },
                &active_fb,
            );
        }
        tracker.end_session();

        let md1 = fs::read_to_string(&report_path).expect("report written");
        assert!(md1.contains("## Device Specs"));
        assert!(md1.contains("Test RTX GPU (Vulkan · DiscreteGpu · Driver: 572.0)"));
        assert!(md1.contains("1920x1080 @ 144.0 Hz"));
        assert!(md1.contains("48000 Hz"));
        assert!(md1.contains("- **Device Used:**"));
        assert!(md1.contains("POKEMON EMER \\| TEST"));
        assert!(md1.contains("**Smooth**"));
        assert!(md1.contains("Flash 128K (RTC, saved)"));
        assert!(md1.contains("0x1F1C08FB"));

        // Reload in a fresh tracker and run a tiny <3s brief launch: should keep Smooth verdict
        // while incrementing session count to 2.
        let mut tracker2 = PlaytestTracker::with_paths(
            true,
            ledger_path.clone(),
            report_path.clone(),
            fallback_path,
        );
        tracker2.begin_session(meta, 0);
        for _ in 0..10 {
            tracker2.record_frame(
                FrameSample {
                    emu_ms: 0.7,
                    present_ms: 0.8,
                    normal_speed: true,
                    frameskipped: false,
                    save_dirty: false,
                    audio_underruns_now: 0,
                },
                &active_fb,
            );
        }
        tracker2.end_session();

        let reloaded = load_ledger(&ledger_path);
        assert_eq!(reloaded.records.len(), 1);
        assert_eq!(reloaded.records[0].sessions, 2);
        assert_eq!(reloaded.records[0].total_frames, 250);
        assert_eq!(reloaded.records[0].verdict, PlaytestVerdict::Smooth);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_report_renders_helpful_placeholder() {
        let md = render_markdown_report(&[], &DeviceSpecs::default(), 0);
        assert!(md.contains("No games played yet."));
        assert!(md.contains("Games Tested:** 0"));
    }
}
