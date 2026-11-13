use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui_wgpu::wgpu;
use gba_core::{
    GbaCore, HostFrontend, KEY_A, KEY_B, KEY_DOWN, KEY_L, KEY_LEFT, KEY_R, KEY_RIGHT, KEY_SELECT,
    KEY_START, KEY_UP, LCD_HEIGHT, LCD_WIDTH, encode_keyinput,
};
use serde::{Deserialize, Serialize};

use crate::audio::{AudioOutput, AudioProducer};
use crate::config::{AppConfig, ColorProfileChoice, DisplayFilter, ShaderPreset};
use crate::emu_thread::DrcPacer;
use crate::filters::{QualityGovernor, VideoPostProcessor};
use crate::headless::CliConfig;
use crate::no_gpu::{
    NoGpuCpuPresenter, SoftbufferSizeBenchRow, bench_real_softbuffer_present, bench_softbuffer_sizes,
};
use crate::shaders::{
    BenchWgpuContext, EnumeratedAdapterInfo, StartupBackendDecision, choose_desktop_backend,
};

/// NTSC-like GBA refresh rate (280,896 cycles / frame at 16,777,216 Hz).
pub const GBA_FPS: f64 = 16_777_216.0 / 280_896.0; // ~59.72750057 Hz
pub const FRAME_BUDGET_MS: f64 = 1000.0 / GBA_FPS; // ~16.7427056 ms
pub const TWO_X_BUDGET_MS: f64 = FRAME_BUDGET_MS * 2.0; // ~33.4854112 ms

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SceneRange {
    pub name: String,
    pub start_frame: usize,
    pub end_frame: usize,
}

#[derive(Clone, Debug)]
enum InputRule {
    Single {
        frame: usize,
        mask: u16,
    },
    Range {
        start: usize,
        end: usize,
        mask: u16,
    },
    Pulse {
        start: usize,
        end: usize,
        every: usize,
        hold: usize,
        mask: u16,
    },
}

/// Deterministic frame-accurate input and scene replay schedule.
#[derive(Clone, Debug, Default)]
pub struct InputReplay {
    pub source_path: Option<PathBuf>,
    pub scenes: Vec<SceneRange>,
    rules: Vec<InputRule>,
}

impl InputReplay {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("failed to read input replay {}: {e}", path.display()))?;
        let mut replay = Self::parse(&text)
            .map_err(|e| format!("failed to parse input replay {}: {e}", path.display()))?;
        replay.source_path = Some(path.to_path_buf());
        Ok(replay)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut replay = Self::default();
        for (line_idx, raw_line) in text.lines().enumerate() {
            let line_no = line_idx + 1;
            let trimmed = raw_line.split('#').next().unwrap_or("").trim();
            if trimmed.is_empty() {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            match parts[0].to_ascii_lowercase().as_str() {
                "scene" => {
                    if parts.len() != 3 {
                        return Err(format!(
                            "line {line_no}: expected `scene <name> <start>..<end>`"
                        ));
                    }
                    let (start_frame, end_frame) = parse_frame_range(parts[2], line_no)?;
                    replay.scenes.push(SceneRange {
                        name: parts[1].to_string(),
                        start_frame,
                        end_frame,
                    });
                }
                "frame" => {
                    if parts.len() != 3 {
                        return Err(format!("line {line_no}: expected `frame <N> <BUTTONS>`"));
                    }
                    let frame = parts[1]
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: invalid frame `{}`", parts[1]))?;
                    let mask = parse_buttons_mask(parts[2], line_no)?;
                    replay.rules.push(InputRule::Single { frame, mask });
                }
                "range" => {
                    if parts.len() != 3 {
                        return Err(format!(
                            "line {line_no}: expected `range <start>..<end> <BUTTONS>`"
                        ));
                    }
                    let (start, end) = parse_frame_range(parts[1], line_no)?;
                    let mask = parse_buttons_mask(parts[2], line_no)?;
                    replay.rules.push(InputRule::Range { start, end, mask });
                }
                "pulse" => {
                    if parts.len() != 5 {
                        return Err(format!(
                            "line {line_no}: expected `pulse <start>..<end> <every> <hold> <BUTTONS>`"
                        ));
                    }
                    let (start, end) = parse_frame_range(parts[1], line_no)?;
                    let every = parts[2]
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: invalid period `{}`", parts[2]))?;
                    if every == 0 {
                        return Err(format!("line {line_no}: pulse `every` must be >= 1"));
                    }
                    let hold = parts[3]
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_no}: invalid hold `{}`", parts[3]))?;
                    let mask = parse_buttons_mask(parts[4], line_no)?;
                    replay.rules.push(InputRule::Pulse {
                        start,
                        end,
                        every,
                        hold,
                        mask,
                    });
                }
                other => {
                    return Err(format!("line {line_no}: unknown directive `{other}`"));
                }
            }
        }
        Ok(replay)
    }

    #[inline]
    pub fn pressed_mask_at(&self, frame: usize) -> u16 {
        let mut mask = 0u16;
        for rule in &self.rules {
            match *rule {
                InputRule::Single { frame: f, mask: m } if f == frame => mask |= m,
                InputRule::Range {
                    start,
                    end,
                    mask: m,
                } if frame >= start && frame < end => {
                    mask |= m;
                }
                InputRule::Pulse {
                    start,
                    end,
                    every,
                    hold,
                    mask: m,
                } if frame >= start && frame < end && ((frame - start) % every) < hold => {
                    mask |= m;
                }
                _ => {}
            }
        }
        mask
    }

    #[inline]
    pub fn keyinput_at(&self, frame: usize) -> u16 {
        encode_keyinput(self.pressed_mask_at(frame))
    }

    pub fn scene_at(&self, frame: usize) -> &str {
        for scene in &self.scenes {
            if frame >= scene.start_frame && frame < scene.end_frame {
                return &scene.name;
            }
        }
        "default"
    }

    pub fn loop_period(&self) -> usize {
        let scene_max = self.scenes.iter().map(|s| s.end_frame).max().unwrap_or(0);
        let rule_max = self
            .rules
            .iter()
            .map(|r| match *r {
                InputRule::Single { frame, .. } => frame.saturating_add(1),
                InputRule::Range { end, .. } | InputRule::Pulse { end, .. } => end,
            })
            .max()
            .unwrap_or(0);
        scene_max.max(rule_max)
    }

    #[inline]
    pub fn effective_frame(&self, frame: usize, loop_replay: bool) -> usize {
        if loop_replay {
            let period = self.loop_period();
            if period > 0 {
                return frame % period;
            }
        }
        frame
    }
}

fn parse_frame_range(spec: &str, line_no: usize) -> Result<(usize, usize), String> {
    let mut parts = spec.split("..");
    let start_s = parts
        .next()
        .ok_or_else(|| format!("line {line_no}: invalid range `{spec}`"))?;
    let end_s = parts
        .next()
        .ok_or_else(|| format!("line {line_no}: expected `start..end` in `{spec}`"))?;
    if parts.next().is_some() {
        return Err(format!("line {line_no}: invalid range `{spec}`"));
    }
    let start = start_s
        .parse::<usize>()
        .map_err(|_| format!("line {line_no}: invalid start frame `{start_s}`"))?;
    let end = end_s
        .parse::<usize>()
        .map_err(|_| format!("line {line_no}: invalid end frame `{end_s}`"))?;
    if end < start {
        return Err(format!("line {line_no}: range end {end} < start {start}"));
    }
    Ok((start, end))
}

fn parse_buttons_mask(spec: &str, line_no: usize) -> Result<u16, String> {
    let mut mask = 0u16;
    for token in spec.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match token.to_ascii_uppercase().as_str() {
            "NONE" => {}
            "A" => mask |= KEY_A,
            "B" => mask |= KEY_B,
            "SELECT" => mask |= KEY_SELECT,
            "START" => mask |= KEY_START,
            "RIGHT" => mask |= KEY_RIGHT,
            "LEFT" => mask |= KEY_LEFT,
            "UP" => mask |= KEY_UP,
            "DOWN" => mask |= KEY_DOWN,
            "R" => mask |= KEY_R,
            "L" => mask |= KEY_L,
            other => return Err(format!("line {line_no}: unknown button `{other}`")),
        }
    }
    Ok(mask)
}

pub fn parse_display_filter(name: &str) -> Result<DisplayFilter, String> {
    match name.trim().to_ascii_lowercase().as_str() {
        "myboy-smooth" | "myboy" | "default" => Ok(DisplayFilter::MyBoySmooth),
        "sharp-1x" | "sharp1x" | "sharp" | "nearest" | "raw-nearest" | "none" => {
            Ok(DisplayFilter::Sharp1x)
        }
        "xbr-3x" | "xbr3x" | "xbr" | "xbrz" => Ok(DisplayFilter::XbrSmooth3x),
        "lcd-matrix" | "authentic-gba-lcd" | "lcd" | "gba-lcd" | "lcd-grid" => {
            Ok(DisplayFilter::AuthenticGbaLcd)
        }
        "soft-handheld" | "soft-lcd" | "soft" | "bilinear" => Ok(DisplayFilter::SoftHandheld),
        "scale2x" | "smooth-scale2x" | "epx" | "hq2x" => Ok(DisplayFilter::SmoothScale2x),
        "crt-scanlines" | "crt" | "scanlines" => Ok(DisplayFilter::CrtScanlines),
        other => Err(format!(
            "unknown filter `{other}` (expected: none, myboy-smooth, bilinear, lcd-grid, crt-scanlines, hq2x, xbrz)"
        )),
    }
}

pub fn filter_cli_slug(filter: DisplayFilter) -> &'static str {
    match filter {
        DisplayFilter::MyBoySmooth => "myboy-smooth",
        DisplayFilter::Sharp1x | DisplayFilter::RawNearest => "none",
        DisplayFilter::XbrSmooth3x => "xbrz",
        DisplayFilter::AuthenticGbaLcd => "lcd-grid",
        DisplayFilter::SoftHandheld => "bilinear",
        DisplayFilter::SmoothScale2x => "hq2x",
        DisplayFilter::CrtScanlines => "crt-scanlines",
    }
}

fn bench_shader_preset_for_filter(filter: DisplayFilter) -> ShaderPreset {
    match filter {
        DisplayFilter::Sharp1x | DisplayFilter::RawNearest => ShaderPreset::Nearest,
        DisplayFilter::MyBoySmooth | DisplayFilter::SoftHandheld => ShaderPreset::SharpBilinear,
        DisplayFilter::AuthenticGbaLcd => ShaderPreset::LcdGrid,
        DisplayFilter::CrtScanlines => ShaderPreset::CrtScanlines,
        DisplayFilter::SmoothScale2x | DisplayFilter::XbrSmooth3x => ShaderPreset::ScaleEdge,
    }
}

/// UTC timestamp `YYYYMMDD-HHMMSS` for deterministic artifact naming without external crates.
fn format_utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let hour = sod / 3_600;
    let min = (sod % 3_600) / 60;
    let sec = sod % 60;

    // Civil date from Unix days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    format!("{year:04}{m:02}{d:02}-{hour:02}{min:02}{sec:02}")
}

#[derive(Clone, Debug, Serialize)]
pub struct LatencySummary {
    pub frames: usize,
    pub total_ms: f64,
    pub avg_ms: f64,
    pub avg_fps: f64,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub worst_ms: f64,
    pub over_budget_frames: usize,
    pub over_2x_budget_frames: usize,
}

fn percentile_sorted(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

fn percentile_ms(samples_ms: &[f64], pct: f64) -> f64 {
    if samples_ms.is_empty() {
        return 0.0;
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    percentile_sorted(&sorted, pct)
}

fn measure_calibrate_ms() -> (f64, u64) {
    // Fixed single-thread CPU-bound integer + branchless arithmetic calibration loop.
    let start = Instant::now();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut acc: u64 = 0x517C_C1B7_2722_0A95;
    for i in 0..80_000_000u64 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407 ^ i);
        acc = acc.wrapping_add(state.rotate_left(17) ^ (state >> 23));
    }
    black_box(acc);
    (start.elapsed().as_secs_f64() * 1000.0, acc)
}

fn summarize_samples(samples_ms: &[f64]) -> LatencySummary {
    if samples_ms.is_empty() {
        return LatencySummary {
            frames: 0,
            total_ms: 0.0,
            avg_ms: 0.0,
            avg_fps: 0.0,
            min_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            worst_ms: 0.0,
            over_budget_frames: 0,
            over_2x_budget_frames: 0,
        };
    }

    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));

    let total_ms: f64 = samples_ms.iter().sum();
    let frames = samples_ms.len();
    let avg_ms = total_ms / frames as f64;
    let avg_fps = if avg_ms > 0.0 { 1000.0 / avg_ms } else { 0.0 };
    let over_budget_frames = samples_ms
        .iter()
        .filter(|&&ms| ms > FRAME_BUDGET_MS)
        .count();
    let over_2x_budget_frames = samples_ms
        .iter()
        .filter(|&&ms| ms > TWO_X_BUDGET_MS)
        .count();

    LatencySummary {
        frames,
        total_ms,
        avg_ms,
        avg_fps,
        min_ms: sorted[0],
        p50_ms: percentile_sorted(&sorted, 50.0),
        p95_ms: percentile_sorted(&sorted, 95.0),
        p99_ms: percentile_sorted(&sorted, 99.0),
        worst_ms: *sorted.last().unwrap_or(&0.0),
        over_budget_frames,
        over_2x_budget_frames,
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SubsystemMetrics {
    pub emu_total_ms: f64,
    pub cpu_ms: f64,
    pub bus_ms: f64,
    pub ppu_ms: f64,
    pub apu_ms: f64,
    pub dma_ms: f64,
    pub present_ms: f64,
    pub cpu_pct_of_total: f64,
    pub bus_pct_of_total: f64,
    pub ppu_pct_of_total: f64,
    pub apu_pct_of_total: f64,
    pub dma_pct_of_total: f64,
    pub present_pct_of_total: f64,
    pub raw_instrumented_cpu_ms: f64,
    pub raw_instrumented_bus_ms: f64,
    pub raw_instrumented_ppu_ms: f64,
    pub raw_instrumented_apu_ms: f64,
    pub raw_instrumented_dma_ms: f64,
}

#[derive(Clone, Debug, Default)]
struct SubsystemAccum {
    cpu: Duration,
    bus: Duration,
    ppu: Duration,
    apu: Duration,
    dma: Duration,
    fetch_cycles: u64,
    decode_cycles: u64,
    read_cycles: u64,
    write_cycles: u64,
    flush_cycles: u64,
    ppu_cycles: u64,
    apu_cycles: u64,
    dma_cycles: u64,
}

impl SubsystemAccum {
    fn add_timing(
        &mut self,
        timing: &gba_core::SubsystemTiming,
        profile: &gba_core::FunctionProfile,
    ) {
        self.cpu += timing.cpu_time;
        self.bus += timing.bus_time;
        self.ppu += timing.ppu_time;
        self.apu += timing.apu_time;
        self.dma += timing.dma_time;
        self.fetch_cycles += profile.fetch_cycles;
        self.decode_cycles += profile.decode_exec_cycles;
        self.read_cycles += profile.mem_read_cycles;
        self.write_cycles += profile.mem_write_cycles;
        self.flush_cycles += profile.branch_flush_cycles;
        self.ppu_cycles += profile.ppu_cycles;
        self.apu_cycles += profile.apu_cycles;
        self.dma_cycles += profile.dma_cycles;
    }

    fn into_metrics(
        self,
        uninstrumented_emu_ms: f64,
        uninstrumented_present_ms: f64,
    ) -> SubsystemMetrics {
        let raw_cpu = self.cpu.as_secs_f64() * 1000.0;
        let raw_bus = self.bus.as_secs_f64() * 1000.0;
        let raw_ppu = self.ppu.as_secs_f64() * 1000.0;
        let raw_apu = self.apu.as_secs_f64() * 1000.0;
        let raw_dma = self.dma.as_secs_f64() * 1000.0;
        let raw_sum = (raw_cpu + raw_bus + raw_ppu + raw_apu + raw_dma).max(1e-9);

        let scale = uninstrumented_emu_ms / raw_sum;
        let cpu_ms = raw_cpu * scale;
        let bus_ms = raw_bus * scale;
        let ppu_ms = raw_ppu * scale;
        let apu_ms = raw_apu * scale;
        let dma_ms = raw_dma * scale;

        let total_ms = (uninstrumented_emu_ms + uninstrumented_present_ms).max(1e-9);

        SubsystemMetrics {
            emu_total_ms: uninstrumented_emu_ms,
            cpu_ms,
            bus_ms,
            ppu_ms,
            apu_ms,
            dma_ms,
            present_ms: uninstrumented_present_ms,
            cpu_pct_of_total: (cpu_ms / total_ms) * 100.0,
            bus_pct_of_total: (bus_ms / total_ms) * 100.0,
            ppu_pct_of_total: (ppu_ms / total_ms) * 100.0,
            apu_pct_of_total: (apu_ms / total_ms) * 100.0,
            dma_pct_of_total: (dma_ms / total_ms) * 100.0,
            present_pct_of_total: (uninstrumented_present_ms / total_ms) * 100.0,
            raw_instrumented_cpu_ms: raw_cpu,
            raw_instrumented_bus_ms: raw_bus,
            raw_instrumented_ppu_ms: raw_ppu,
            raw_instrumented_apu_ms: raw_apu,
            raw_instrumented_dma_ms: raw_dma,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SceneBenchReport {
    pub scene: String,
    pub total: LatencySummary,
    pub emu_only: LatencySummary,
    pub present_only: LatencySummary,
    pub subsystems: SubsystemMetrics,
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchReportJson {
    pub schema_version: u32,
    pub timestamp_utc: String,
    pub rom_name: String,
    pub bios_mode: String,
    pub filter: String,
    pub target_fps: f64,
    pub frame_budget_ms: f64,
    pub two_x_budget_ms: f64,
    pub input_replay: Option<String>,
    pub build_profile: String,
    pub release_flags_verified: bool,
    pub overall: LatencySummary,
    pub emu_only: LatencySummary,
    pub present_only: LatencySummary,
    pub subsystems: SubsystemMetrics,
    pub scenes: Vec<SceneBenchReport>,
}

struct BenchFrontend<'a> {
    replay: &'a InputReplay,
    frame_idx: usize,
    filter: DisplayFilter,
    color_correction: bool,
    color_profile: ColorProfileChoice,
    frame_blending: bool,
    lcd_ghosting: bool,
    lcd_grid_intensity: f32,
    postproc: VideoPostProcessor,
    prev_fb: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
    has_prev: bool,
    last_present_dur: Duration,
    frames_presented: usize,
    audio_samples: usize,
}

impl<'a> BenchFrontend<'a> {
    fn new(replay: &'a InputReplay, filter: DisplayFilter) -> Self {
        let cfg = AppConfig::default();
        let mut postproc = VideoPostProcessor::new();
        postproc.prewarm(
            cfg.color_correction,
            cfg.lcd_ghosting,
            filter,
            false,
            cfg.lcd_grid_intensity,
        );
        Self {
            replay,
            frame_idx: 0,
            filter,
            color_correction: cfg.color_correction,
            color_profile: cfg.color_profile,
            frame_blending: cfg.frame_blending,
            lcd_ghosting: cfg.lcd_ghosting,
            lcd_grid_intensity: cfg.lcd_grid_intensity,
            postproc,
            prev_fb: vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice().try_into().unwrap(),
            has_prev: false,
            last_present_dur: Duration::ZERO,
            frames_presented: 0,
            audio_samples: 0,
        }
    }
}

impl<'a> HostFrontend for BenchFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        let t0 = Instant::now();
        let prev = self.has_prev.then_some(self.prev_fb.as_ref());
        self.postproc.sync_rgba_for_capture(
            fb,
            prev,
            self.frame_idx as u64,
            self.color_correction,
            self.color_profile,
            self.frame_blending,
            self.lcd_ghosting,
        );
        let img = self.postproc.process_display_filter(
            fb,
            self.color_correction,
            self.lcd_ghosting,
            self.filter,
            false,
            self.lcd_grid_intensity,
        );
        black_box(img.pixels.as_ptr());
        black_box(img.pixels.len());
        self.prev_fb.copy_from_slice(fb);
        self.has_prev = true;
        self.last_present_dur += t0.elapsed();
        self.frames_presented += 1;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        self.replay.keyinput_at(self.frame_idx)
    }
}

struct SubAttrFrontend<'a> {
    replay: &'a InputReplay,
    frame_idx: usize,
    frames_presented: usize,
}

impl<'a> HostFrontend for SubAttrFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        black_box(fb[0]);
        self.frames_presented += 1;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        black_box(samples.len());
    }

    fn poll_keypad(&mut self) -> u16 {
        self.replay.keyinput_at(self.frame_idx)
    }
}

struct SceneSampleBucket {
    name: String,
    total_ms: Vec<f64>,
    emu_ms: Vec<f64>,
    present_ms: Vec<f64>,
    accum: SubsystemAccum,
}

impl SceneSampleBucket {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            total_ms: Vec::new(),
            emu_ms: Vec::new(),
            present_ms: Vec::new(),
            accum: SubsystemAccum::default(),
        }
    }
}

struct LoadedRomInputs {
    rom: Vec<u8>,
    bios: Option<Vec<u8>>,
    sav: Option<Vec<u8>>,
    rom_name: String,
}

fn load_rom_and_bios(cfg: &CliConfig) -> Result<LoadedRomInputs, String> {
    let rom_path = cfg
        .rom_path
        .as_ref()
        .ok_or("a ROM path is required for `--bench` / `--bench-realtime`")?;
    let rom = fs::read(rom_path)
        .map_err(|e| format!("failed to read ROM {}: {e}", rom_path.display()))?;
    let bios = match &cfg.bios_path {
        Some(p) => {
            Some(fs::read(p).map_err(|e| format!("failed to read BIOS {}: {e}", p.display()))?)
        }
        None => None,
    };
    let sav = match &cfg.sav_path {
        Some(p) if p.exists() => {
            Some(fs::read(p).map_err(|e| format!("failed to read save {}: {e}", p.display()))?)
        }
        _ => None,
    };
    let rom_name = rom_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.gba".to_string());
    Ok(LoadedRomInputs {
        rom,
        bios,
        sav,
        rom_name,
    })
}

fn load_replay(cfg: &CliConfig) -> Result<InputReplay, String> {
    match &cfg.input_replay {
        Some(p) => InputReplay::load(p),
        None => Ok(InputReplay::default()),
    }
}

fn get_or_create_bucket<'a>(
    buckets: &'a mut Vec<SceneSampleBucket>,
    scene: &str,
) -> &'a mut SceneSampleBucket {
    if let Some(pos) = buckets.iter().position(|b| b.name == scene) {
        return &mut buckets[pos];
    }
    buckets.push(SceneSampleBucket::new(scene));
    buckets.last_mut().expect("just pushed")
}

/// Checks workspace Cargo.toml for release optimization settings.
pub fn verify_release_profile_settings() -> (bool, String) {
    let candidates = [Path::new("Cargo.toml"), Path::new("../../Cargo.toml")];
    for p in candidates {
        if let Ok(text) = fs::read_to_string(p) {
            let has_opt3 = text.contains("opt-level = 3");
            let has_lto = text.contains("lto = \"fat\"") || text.contains("lto = true");
            let has_cgu1 = text.contains("codegen-units = 1");
            let ok = has_opt3 && has_lto && has_cgu1;
            let detail = format!(
                "Cargo.toml ({}): opt-level=3 ({has_opt3}), lto=fat ({has_lto}), codegen-units=1 ({has_cgu1})",
                p.display()
            );
            return (ok, detail);
        }
    }
    (
        false,
        "Cargo.toml not found in working directory".to_string(),
    )
}

pub fn run_bench(cfg: &CliConfig) -> Result<(), String> {
    let LoadedRomInputs {
        rom,
        bios,
        sav,
        rom_name,
    } = load_rom_and_bios(cfg)?;
    let replay = load_replay(cfg)?;
    let filter = cfg
        .filter
        .unwrap_or_else(|| AppConfig::default().display_filter);
    let frames = cfg.frames.max(1);
    let ts = format_utc_timestamp();

    fs::create_dir_all(&cfg.perf_dir)
        .map_err(|e| format!("failed to create perf dir {}: {e}", cfg.perf_dir.display()))?;

    // Pass 1: Uninstrumented wall-clock run (`core.run_frame` + `VideoPostProcessor` present)
    let mut core1 = GbaCore::new(bios.as_deref(), rom.clone());
    if let Some(bytes) = &sav {
        core1.bus.cart.load_save_data(bytes);
    }
    let mut front1 = BenchFrontend::new(&replay, filter);

    let mut all_total_ms = Vec::with_capacity(frames);
    let mut all_emu_ms = Vec::with_capacity(frames);
    let mut all_present_ms = Vec::with_capacity(frames);
    let mut scene_buckets: Vec<SceneSampleBucket> = replay
        .scenes
        .iter()
        .map(|s| SceneSampleBucket::new(&s.name))
        .collect();

    for frame_idx in 0..frames {
        front1.frame_idx = frame_idx;
        front1.last_present_dur = Duration::ZERO;
        let t0 = Instant::now();
        core1.run_frame(&mut front1);
        let total_dur = t0.elapsed();

        let present_ms = front1.last_present_dur.as_secs_f64() * 1000.0;
        let total_ms = total_dur.as_secs_f64() * 1000.0;
        let emu_ms = (total_ms - present_ms).max(0.0);

        all_total_ms.push(total_ms);
        all_emu_ms.push(emu_ms);
        all_present_ms.push(present_ms);

        let scene_name = replay.scene_at(frame_idx);
        let bucket = get_or_create_bucket(&mut scene_buckets, scene_name);
        bucket.total_ms.push(total_ms);
        bucket.emu_ms.push(emu_ms);
        bucket.present_ms.push(present_ms);
    }

    // Pass 2: Deterministic replay through `core.run_frame_bench` for subsystem attribution
    let mut core2 = GbaCore::new(bios.as_deref(), rom);
    if let Some(bytes) = &sav {
        core2.bus.cart.load_save_data(bytes);
    }
    let mut front2 = SubAttrFrontend {
        replay: &replay,
        frame_idx: 0,
        frames_presented: 0,
    };
    let mut total_accum = SubsystemAccum::default();

    for frame_idx in 0..frames {
        front2.frame_idx = frame_idx;
        let (timing, profile) = core2.run_frame_bench(&mut front2);
        total_accum.add_timing(&timing, &profile);
        let scene_name = replay.scene_at(frame_idx);
        let bucket = get_or_create_bucket(&mut scene_buckets, scene_name);
        bucket.accum.add_timing(&timing, &profile);
    }

    let overall_summary = summarize_samples(&all_total_ms);
    let emu_summary = summarize_samples(&all_emu_ms);
    let present_summary = summarize_samples(&all_present_ms);
    let overall_subsystems =
        total_accum.into_metrics(emu_summary.total_ms, present_summary.total_ms);

    let mut scene_reports = Vec::new();
    for bucket in scene_buckets {
        if bucket.total_ms.is_empty() {
            continue;
        }
        let total = summarize_samples(&bucket.total_ms);
        let emu_only = summarize_samples(&bucket.emu_ms);
        let present_only = summarize_samples(&bucket.present_ms);
        let subsystems = bucket
            .accum
            .into_metrics(emu_only.total_ms, present_only.total_ms);
        scene_reports.push(SceneBenchReport {
            scene: bucket.name,
            total,
            emu_only,
            present_only,
            subsystems,
        });
    }

    let (release_verified, release_detail) = verify_release_profile_settings();
    let build_profile = if cfg!(debug_assertions) {
        "debug".to_string()
    } else {
        "release".to_string()
    };

    let report = BenchReportJson {
        schema_version: 1,
        timestamp_utc: ts.clone(),
        rom_name: rom_name.clone(),
        bios_mode: if bios.is_some() {
            "custom-bios".to_string()
        } else {
            "hle-bios".to_string()
        },
        filter: filter_cli_slug(filter).to_string(),
        target_fps: GBA_FPS,
        frame_budget_ms: FRAME_BUDGET_MS,
        two_x_budget_ms: TWO_X_BUDGET_MS,
        input_replay: cfg
            .input_replay
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        build_profile: build_profile.clone(),
        release_flags_verified: release_verified && build_profile == "release",
        overall: overall_summary.clone(),
        emu_only: emu_summary,
        present_only: present_summary,
        subsystems: overall_subsystems,
        scenes: scene_reports,
    };

    let json_path = cfg.perf_dir.join(format!("bench-{ts}.json"));
    let json_body = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("failed to serialize bench JSON: {e}"))?;
    fs::write(&json_path, &json_body)
        .map_err(|e| format!("failed to write {}: {e}", json_path.display()))?;

    let report_md_path = cfg.perf_dir.join("REPORT.md");
    let md = render_phase1_markdown(&report, &json_path, &release_detail);
    fs::write(&report_md_path, md)
        .map_err(|e| format!("failed to write {}: {e}", report_md_path.display()))?;

    println!("Benchmark complete ({frames} frames, {rom_name})");
    println!(
        "  Avg FPS: {:.2} | Avg: {:.3} ms | p50: {:.3} ms | p95: {:.3} ms | p99: {:.3} ms | Worst: {:.3} ms",
        report.overall.avg_fps,
        report.overall.avg_ms,
        report.overall.p50_ms,
        report.overall.p95_ms,
        report.overall.p99_ms,
        report.overall.worst_ms,
    );
    println!("  JSON: {}", json_path.display());
    println!("  Report: {}", report_md_path.display());

    if cfg.dump_hashes.is_some() || cfg.verify_hashes.is_some() {
        run_hash_pass(cfg)?;
    }

    Ok(())
}

fn render_phase1_markdown(
    report: &BenchReportJson,
    json_path: &Path,
    release_detail: &str,
) -> String {
    let mut md = String::new();
    md.push_str("# Rustboy Advance Performance Report\n\n");
    md.push_str("## Execution Plan\n\n");
    md.push_str("1. **Phase 1 — Measure (Completed):** Headless unthrottled `--bench` (3600 frames) and paced `--bench-realtime` (`16.74 ms` budget) with deterministic replay covering boot, title, menus, overworld scrolling, and battle effects.\n");
    md.push_str("2. **Phase 2 — Filters:** Measure every display filter individually; confirm the cheapest clean default filter and keep heavy filters optional.\n");
    md.push_str("3. **Phase 3 — Weak-Hardware Simulation:** Re-test under 1–2 core `ProcessorAffinity`, lowered process priority, WARP/GL adapter, and reduced window scale (explicitly noting this approximates rather than replaces real low-end devices).\n");
    md.push_str("4. **Phase 4 — Optimize:** Profile-guided, one-change-at-a-time optimizations verified by re-running benchmarks while preserving emulation accuracy and identical filter output.\n\n");

    md.push_str("## Phase 1 — Baseline Measurement\n\n");
    md.push_str(&format!(
        "- **Timestamp (UTC):** `{}`\n",
        report.timestamp_utc
    ));
    md.push_str(&format!(
        "- **ROM:** `{}` ({})\n",
        report.rom_name, report.bios_mode
    ));
    md.push_str(&format!(
        "- **Build Profile:** `{}` (`{}`)\n",
        report.build_profile, release_detail
    ));
    md.push_str(&format!(
        "- **Display Filter (`present`):** `{}`\n",
        report.filter
    ));
    md.push_str(&format!(
        "- **Input Replay:** `{}`\n",
        report.input_replay.as_deref().unwrap_or("none")
    ));
    md.push_str(&format!(
        "- **Target Budget:** `{:.4} ms/frame` ({:.4} FPS), `2x` spike threshold = `{:.4} ms`\n",
        report.frame_budget_ms, report.target_fps, report.two_x_budget_ms
    ));
    md.push_str(&format!("- **Raw JSON:** `{}`\n\n", json_path.display()));

    md.push_str("### Overall Frame Time Summary (Pass 1 Uninstrumented CPU + Present)\n\n");
    md.push_str("| Scope | Frames | Avg FPS | Avg (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Worst (ms) | >16.74 ms | >33.49 ms (2x) |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for (label, s) in [
        ("Total (Emu + Present)", &report.overall),
        ("Emulation Only", &report.emu_only),
        ("Present Only", &report.present_only),
    ] {
        md.push_str(&format!(
            "| {} | {} | {:.2} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {} | {} |\n",
            label,
            s.frames,
            s.avg_fps,
            s.avg_ms,
            s.p50_ms,
            s.p95_ms,
            s.p99_ms,
            s.worst_ms,
            s.over_budget_frames,
            s.over_2x_budget_frames
        ));
    }

    let n = report.overall.frames.max(1) as f64;
    let s = &report.subsystems;
    md.push_str("\n### Subsystem Breakdown (Pass 2 Attributed to Uninstrumented Wall Time)\n\n");
    md.push_str("| Subsystem | Total Time (ms) | Avg / Frame (ms) | Share of Frame (%) | Raw Instrumented (ms) |\n");
    md.push_str("|---|---:|---:|---:|---:|\n");
    md.push_str(&format!(
        "| CPU | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        s.cpu_ms,
        s.cpu_ms / n,
        s.cpu_pct_of_total,
        s.raw_instrumented_cpu_ms
    ));
    md.push_str(&format!(
        "| Bus | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        s.bus_ms,
        s.bus_ms / n,
        s.bus_pct_of_total,
        s.raw_instrumented_bus_ms
    ));
    md.push_str(&format!(
        "| PPU | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        s.ppu_ms,
        s.ppu_ms / n,
        s.ppu_pct_of_total,
        s.raw_instrumented_ppu_ms
    ));
    md.push_str(&format!(
        "| APU | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        s.apu_ms,
        s.apu_ms / n,
        s.apu_pct_of_total,
        s.raw_instrumented_apu_ms
    ));
    md.push_str(&format!(
        "| DMA | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        s.dma_ms,
        s.dma_ms / n,
        s.dma_pct_of_total,
        s.raw_instrumented_dma_ms
    ));
    md.push_str(&format!(
        "| Present (`{}`) | {:.3} | {:.4} | {:.2}% | {:.3} |\n",
        report.filter,
        s.present_ms,
        s.present_ms / n,
        s.present_pct_of_total,
        s.present_ms
    ));

    if !report.scenes.is_empty() {
        md.push_str("\n### Per-Scene Breakdown\n\n");
        md.push_str("| Scene | Frames | Avg FPS | Avg (ms) | p50 (ms) | p95 (ms) | p99 (ms) | Worst (ms) | CPU % | Bus % | PPU % | APU % | DMA % | Present % |\n");
        md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
        for sc in &report.scenes {
            md.push_str(&format!(
                "| `{}` | {} | {:.2} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% |\n",
                sc.scene,
                sc.total.frames,
                sc.total.avg_fps,
                sc.total.avg_ms,
                sc.total.p50_ms,
                sc.total.p95_ms,
                sc.total.p99_ms,
                sc.total.worst_ms,
                sc.subsystems.cpu_pct_of_total,
                sc.subsystems.bus_pct_of_total,
                sc.subsystems.ppu_pct_of_total,
                sc.subsystems.apu_pct_of_total,
                sc.subsystems.dma_pct_of_total,
                sc.subsystems.present_pct_of_total,
            ));
        }
    }

    md
}

struct RealtimeFrontend<'a> {
    replay: &'a InputReplay,
    loop_replay: bool,
    frame_idx: usize,
    filter: DisplayFilter,
    color_correction: bool,
    color_profile: ColorProfileChoice,
    frame_blending: bool,
    lcd_ghosting: bool,
    lcd_grid_intensity: f32,
    postproc: VideoPostProcessor,
    software_wgpu: Option<BenchWgpuContext>,
    no_gpu_blitter: Option<crate::no_gpu::NoGpuCpuPresenter>,
    prev_fb: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
    has_prev: bool,
    audio: &'a mut AudioProducer,
    last_present_dur: Duration,
    last_gpu_submit_dur: Duration,
    async_gpu_submit: bool,
    last_present_wall: Option<Instant>,
    presentation_intervals: Vec<f64>,
    cadence_jitter_frames: usize,
    skip_present_this_frame: bool,
    skipped_presents: usize,
}

impl<'a> RealtimeFrontend<'a> {
    fn new(
        replay: &'a InputReplay,
        loop_replay: bool,
        filter: DisplayFilter,
        color_correction: bool,
        software_wgpu: Option<BenchWgpuContext>,
        no_gpu_blitter: Option<crate::no_gpu::NoGpuCpuPresenter>,
        audio: &'a mut AudioProducer,
    ) -> Self {
        let cfg = AppConfig::default();
        let mut postproc = VideoPostProcessor::new();
        postproc.prewarm(
            color_correction,
            cfg.lcd_ghosting,
            filter,
            false,
            cfg.lcd_grid_intensity,
        );
        Self {
            replay,
            loop_replay,
            frame_idx: 0,
            filter,
            color_correction,
            color_profile: cfg.color_profile,
            frame_blending: cfg.frame_blending,
            lcd_ghosting: cfg.lcd_ghosting,
            lcd_grid_intensity: cfg.lcd_grid_intensity,
            postproc,
            software_wgpu,
            no_gpu_blitter,
            prev_fb: vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice().try_into().unwrap(),
            has_prev: false,
            audio,
            last_present_dur: Duration::ZERO,
            last_gpu_submit_dur: Duration::ZERO,
            async_gpu_submit: false,
            last_present_wall: None,
            presentation_intervals: Vec::new(),
            cadence_jitter_frames: 0,
            skip_present_this_frame: false,
            skipped_presents: 0,
        }
    }

    fn drain_wgpu(&self) {
        if let Some(ref ctx) = self.software_wgpu {
            ctx.drain_gpu();
        }
    }
}

impl<'a> HostFrontend for RealtimeFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        if self.skip_present_this_frame {
            self.prev_fb.copy_from_slice(fb);
            self.has_prev = true;
            self.skipped_presents += 1;
            return;
        }
        let now = Instant::now();
        if let Some(prev) = self.last_present_wall {
            let dt = (now - prev).as_secs_f64() * 1000.0;
            self.presentation_intervals.push(dt);
            if (dt - FRAME_BUDGET_MS).abs() > 2.0 {
                self.cadence_jitter_frames += 1;
            }
        }
        self.last_present_wall = Some(now);

        let t0 = Instant::now();
        if let Some(ref mut ctx) = self.software_wgpu {
            if ctx.is_cpu_adapter() {
                if self.frame_blending {
                    let prev = self.has_prev.then_some(self.prev_fb.as_ref());
                    self.postproc.sync_rgba_for_capture(
                        fb,
                        prev,
                        self.frame_idx as u64,
                        self.color_correction,
                        self.color_profile,
                        self.frame_blending,
                        self.lcd_ghosting,
                    );
                }
                let img = self.postproc.process_display_filter(
                    fb,
                    self.color_correction,
                    self.lcd_ghosting,
                    self.filter,
                    false,
                    self.lcd_grid_intensity,
                );
                let linear = self.filter.texture_options() == egui::TextureOptions::LINEAR;
                ctx.present_plain_rgba_sync(img.as_raw(), img.size[0] as u32, img.size[1] as u32, linear);
            } else {
                let preset = bench_shader_preset_for_filter(self.filter);
                let color_profile_id = if self.color_correction { 1 } else { 0 };
                if self.async_gpu_submit {
                    let submit_ms = ctx.present_frame_async(
                        fb,
                        &self.prev_fb,
                        self.has_prev,
                        preset,
                        color_profile_id,
                    );
                    self.last_gpu_submit_dur += Duration::from_secs_f64(submit_ms / 1000.0);
                } else {
                    ctx.present_frame_sync(fb, &self.prev_fb, self.has_prev, preset, color_profile_id);
                }
            }
        } else {
            if self.frame_blending {
                let prev = self.has_prev.then_some(self.prev_fb.as_ref());
                self.postproc.sync_rgba_for_capture(
                    fb,
                    prev,
                    self.frame_idx as u64,
                    self.color_correction,
                    self.color_profile,
                    self.frame_blending,
                    self.lcd_ghosting,
                );
            }
            let img = self.postproc.process_display_filter(
                fb,
                self.color_correction,
                self.lcd_ghosting,
                self.filter,
                false,
                self.lcd_grid_intensity,
            );
            if let Some(ref mut blitter) = self.no_gpu_blitter {
                let linear = self.filter.texture_options() == egui::TextureOptions::LINEAR;
                let xrgb = blitter.present_rgba_to_xrgb(
                    &img.pixels,
                    img.size[0],
                    img.size[1],
                    LCD_WIDTH * 3,
                    LCD_HEIGHT * 3,
                    linear,
                );
                black_box(xrgb.as_ptr());
                black_box(xrgb.len());
            } else {
                black_box(img.pixels.as_ptr());
                black_box(img.pixels.len());
            }
        }
        self.prev_fb.copy_from_slice(fb);
        self.has_prev = true;
        self.last_present_dur += t0.elapsed();
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio.push_interleaved(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        let eff = self.replay.effective_frame(self.frame_idx, self.loop_replay);
        self.replay.keyinput_at(eff)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RealtimeEvent {
    pub frame: usize,
    pub elapsed_s: f64,
    pub scene: String,
    pub kind: String,
    pub work_ms: f64,
    pub interval_ms: f64,
    pub underrun_delta: u64,
    pub audio_occupancy_pct: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct RealtimeFilterCostJson {
    pub filter: String,
    pub cost_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RealtimeReportJson {
    pub schema_version: u32,
    pub timestamp_utc: String,
    pub rom_name: String,
    pub filter: String,
    pub frames: usize,
    pub slowdown: usize,
    pub wall_duration_s: f64,
    pub audio_device_available: bool,
    pub host_sample_rate_hz: u32,
    pub target_fps: f64,
    pub frame_budget_ms: f64,
    pub two_x_budget_ms: f64,
    pub dropped_frames_work: usize,
    pub dropped_frames_deadline: usize,
    pub frames_over_2x_work: usize,
    pub frames_over_2x_interval: usize,
    pub total_audio_underruns: u64,
    pub work_summary: LatencySummary,
    pub work_post_warmup: LatencySummary,
    pub present_summary: LatencySummary,
    pub interval_summary: LatencySummary,
    pub presentation_interval_summary: LatencySummary,
    pub cadence_jitter_frames: usize,
    pub log_path: String,
    pub events: Vec<RealtimeEvent>,
    pub initial_filter: String,
    pub final_filter: String,
    pub present_backend: String,
    pub emu_priority_raised: bool,
    pub emu_mmcss_active: bool,
    pub emu_thread_priority: i32,
    pub governor_enabled: bool,
    pub governor_transitions: usize,
    pub governor_settled_frame: Option<usize>,
    pub governor_settled_s: Option<f64>,
    pub underruns_during_settling: u64,
    pub underruns_post_settle: u64,
    pub measured_filter_costs: Vec<RealtimeFilterCostJson>,
    pub work_over_budget_frames: usize,
    pub over_budget_pct: f64,
    pub work_over_budget_post_warmup_frames: usize,
    pub over_budget_post_warmup_pct: f64,
    pub warmup_frames: usize,
}

pub fn run_bench_realtime(cfg: &CliConfig) -> Result<(), String> {
    let LoadedRomInputs {
        rom,
        bios,
        sav,
        rom_name,
    } = load_rom_and_bios(cfg)?;
    let replay = load_replay(cfg)?;
    let filter = cfg
        .filter
        .unwrap_or_else(|| AppConfig::default().display_filter);
    let color_correction = if cfg.filter_color_correction {
        true
    } else {
        AppConfig::default().color_correction
    };
    let initial_filter_slug = if cfg.filter_color_correction {
        "color-correction".to_string()
    } else {
        filter_cli_slug(filter).to_string()
    };
    let frames = cfg.frames.max(1);
    let warmup_frames = cfg.warmup_frames;
    let slowdown = cfg.slowdown.max(1);
    let governor_enabled = cfg.governor.unwrap_or(false);
    let emu_priority_raised = !cfg.no_emu_priority;
    let _emu_priority_guard =
        emu_priority_raised.then(crate::audio::win_mmcss::MmcssGuard::enter_games);
    let emu_mmcss_active = _emu_priority_guard
        .as_ref()
        .is_some_and(|g| g.is_mmcss_active());
    let emu_thread_priority = crate::audio::win_mmcss::current_thread_priority();

    let mut core = Box::new(GbaCore::new(bios.as_deref(), rom));
    if let Some(bytes) = &sav {
        core.bus.cart.load_save_data(bytes);
    }

    let mut warmup_last_fb: Box<[u16; LCD_WIDTH * LCD_HEIGHT]> = vec![0u16; LCD_WIDTH * LCD_HEIGHT]
        .into_boxed_slice()
        .try_into()
        .unwrap();
    let mut warmup_has_prev = false;
    if warmup_frames > 0 {
        struct WarmupFrontend<'a> {
            replay: &'a InputReplay,
            loop_replay: bool,
            frame_idx: usize,
            last_fb: &'a mut [u16; LCD_WIDTH * LCD_HEIGHT],
            has_frame: &'a mut bool,
        }
        impl<'a> HostFrontend for WarmupFrontend<'a> {
            fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
                self.last_fb.copy_from_slice(fb);
                *self.has_frame = true;
            }
            fn push_audio_samples(&mut self, _samples: &[i16]) {}
            fn poll_keypad(&mut self) -> u16 {
                let eff = self.replay.effective_frame(self.frame_idx, self.loop_replay);
                self.replay.keyinput_at(eff)
            }
        }
        let mut wf = WarmupFrontend {
            replay: &replay,
            loop_replay: cfg.loop_replay,
            frame_idx: 0,
            last_fb: &mut warmup_last_fb,
            has_frame: &mut warmup_has_prev,
        };
        for w in 0..warmup_frames {
            wf.frame_idx = w;
            core.run_frame(&mut wf);
        }
    }

    let mut software_wgpu = if cfg.software_adapter && !cfg.no_gpu {
        let mut ctx = BenchWgpuContext::init(true, LCD_WIDTH as u32 * 3, LCD_HEIGHT as u32 * 3)?;
        if ctx.is_cpu_adapter() {
            for mult in [1u32, 2u32, 3u32] {
                let w = LCD_WIDTH as u32 * mult;
                let h = LCD_HEIGHT as u32 * mult;
                let blank_rgba = vec![0u8; (w * h * 4) as usize];
                ctx.present_plain_rgba_sync(&blank_rgba, w, h, false);
                ctx.present_plain_rgba_sync(&blank_rgba, w, h, true);
            }
        } else {
            let blank: Box<[u16; LCD_WIDTH * LCD_HEIGHT]> =
                vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice().try_into().unwrap();
            for &preset in &[
                ShaderPreset::Nearest,
                ShaderPreset::SharpBilinear,
                ShaderPreset::LcdGrid,
                ShaderPreset::CrtScanlines,
                ShaderPreset::ScaleEdge,
            ] {
                ctx.present_frame_sync(&blank, &blank, false, preset, 0);
            }
        }
        Some(ctx)
    } else {
        None
    };
    let mut no_gpu_blitter = if cfg.no_gpu {
        Some(crate::no_gpu::NoGpuCpuPresenter::new(LCD_WIDTH * 3, LCD_HEIGHT * 3))
    } else {
        None
    };
    let present_backend = match (&software_wgpu, &no_gpu_blitter) {
        (Some(ctx), _) if ctx.is_cpu_adapter() => format!(
            "software-cpu-plain: {} ({} / {})",
            ctx.adapter_name, ctx.adapter_backend, ctx.adapter_device_type
        ),
        (Some(ctx), _) => format!(
            "software-vulkan: {} ({} / {})",
            ctx.adapter_name, ctx.adapter_backend, ctx.adapter_device_type
        ),
        (None, Some(_)) => "no-gpu (NoGpuCpuPresenter XRGB8 / softbuffer CPU fallback)".to_string(),
        (None, None) => "cpu-software (VideoPostProcessor)".to_string(),
    };

    let mut measure_postproc = VideoPostProcessor::new();
    let mut measured_filter_costs = if let Some(ref mut ctx) = software_wgpu {
        ctx.measure_filter_costs(
            &mut measure_postproc,
            color_correction,
            false,
            AppConfig::default().lcd_grid_intensity,
        )
    } else if let Some(ref mut blitter) = no_gpu_blitter {
        blitter.measure_filter_costs(
            &mut measure_postproc,
            color_correction,
            false,
            AppConfig::default().lcd_grid_intensity,
            LCD_WIDTH * 3,
            LCD_HEIGHT * 3,
        )
    } else {
        measure_postproc.measure_filter_costs(
            color_correction,
            false,
            false,
            AppConfig::default().lcd_grid_intensity,
        )
    };
    if slowdown > 1 {
        for entry in &mut measured_filter_costs {
            entry.cost_ms *= slowdown as f64;
        }
        crate::filters::sort_filter_costs(&mut measured_filter_costs);
    }
    let measured_costs_summary = measured_filter_costs
        .iter()
        .map(|c| format!("{}:{:.3}ms", filter_cli_slug(c.filter), c.cost_ms))
        .collect::<Vec<_>>()
        .join(", ");

    let ts = format_utc_timestamp();

    fs::create_dir_all(&cfg.perf_dir)
        .map_err(|e| format!("failed to create perf dir {}: {e}", cfg.perf_dir.display()))?;

    let log_path = cfg.perf_dir.join(format!("realtime-{ts}.log"));
    let json_path = cfg.perf_dir.join(format!("realtime-{ts}.json"));
    let log_file = File::create(&log_path)
        .map_err(|e| format!("failed to create {}: {e}", log_path.display()))?;
    let mut log = BufWriter::new(log_file);

    let mut audio_out = AudioOutput::start();
    let has_audio_device = audio_out.has_device();
    let host_rate = audio_out.host_sample_rate();
    let capacity = audio_out.capacity();
    audio_out.reset_underruns();
    let mut producer = audio_out
        .take_producer()
        .ok_or("failed to acquire AudioProducer for --bench-realtime")?;

    writeln!(
        log,
        "# Rustboy Advance --bench-realtime log ({ts} UTC)\n\
         # rom={rom_name} filter={initial_filter_slug} backend=\"{present_backend}\" frames={frames} warmup_frames={warmup_frames} slowdown={slowdown} loop_replay={} emu_priority_raised={emu_priority_raised} mmcss={emu_mmcss_active} prio={emu_thread_priority} governor={governor_enabled} audio_device={has_audio_device} host_rate={host_rate}Hz\n\
         # measured_filter_costs=[{measured_costs_summary}]\n\
         # budget_ms={FRAME_BUDGET_MS:.4} two_x_budget_ms={TWO_X_BUDGET_MS:.4}",
        cfg.loop_replay,
    )
    .map_err(|e| format!("log write error: {e}"))?;
    log.flush().map_err(|e| format!("log flush error: {e}"))?;

    let mut frontend = RealtimeFrontend::new(
        &replay,
        cfg.loop_replay,
        filter,
        color_correction,
        software_wgpu,
        no_gpu_blitter,
        &mut producer,
    );
    if warmup_has_prev {
        frontend.prev_fb.copy_from_slice(&*warmup_last_fb);
        frontend.has_prev = true;
    }
    let mut governor = QualityGovernor::with_measured_costs(
        filter,
        governor_enabled,
        measured_filter_costs.clone(),
    );
    let mut governor_transitions = 0usize;
    let mut governor_settled_frame: Option<usize> = None;
    let mut governor_settled_s: Option<f64> = None;
    let mut underruns_during_settling = 0u64;
    let mut pacer = DrcPacer::new(capacity);

    let mut work_ms_samples = Vec::with_capacity(frames);
    let mut present_ms_samples = Vec::with_capacity(frames);
    let mut interval_ms_samples = Vec::with_capacity(frames);
    let mut events: Vec<RealtimeEvent> = Vec::new();

    let mut dropped_work = 0usize;
    let mut dropped_deadline = 0usize;
    let mut over_2x_work = 0usize;
    let mut over_2x_interval = 0usize;
    let mut prev_underruns = audio_out.underruns();
    let mut next_ideal_deadline = Instant::now();
    let ideal_period = Duration::from_secs_f64(1.0 / GBA_FPS);
    let run_start = Instant::now();
    let mut consecutive_present_skips = 0usize;
    let mut prev_work_ms = 0.0f64;

    for frame_idx in 0..frames {
        let abs_frame = warmup_frames + frame_idx;
        let tick_start = Instant::now();
        frontend.frame_idx = abs_frame;
        frontend.last_present_dur = Duration::ZERO;
        // Skip presentation filtering (never emulation!) only after a severe (>2x budget) work stall.
        // Do not skip on minor 17-23ms deadline drifts, because skipping intentionally creates a
        // >33.5ms presentation gap even when every frame completes within 2x budget.
        let overload_or_behind = frame_idx > 0 && prev_work_ms > TWO_X_BUDGET_MS;
        if overload_or_behind && consecutive_present_skips < 1 {
            frontend.skip_present_this_frame = true;
            consecutive_present_skips += 1;
        } else {
            frontend.skip_present_this_frame = false;
            consecutive_present_skips = 0;
        }

        core.run_frame(&mut frontend);
        let raw_work_dur = tick_start.elapsed();
        if slowdown > 1 {
            let target_work_dur = raw_work_dur.mul_f64(slowdown as f64);
            while tick_start.elapsed() < target_work_dur {
                std::hint::spin_loop();
            }
        }
        let work_dur = tick_start.elapsed();
        let present_ms = frontend.last_present_dur.as_secs_f64() * 1000.0;
        let work_ms = work_dur.as_secs_f64() * 1000.0;
        prev_work_ms = work_ms;

        next_ideal_deadline += ideal_period;
        let now_after_work = Instant::now();
        let missed_deadline = now_after_work > next_ideal_deadline + ideal_period;
        if missed_deadline {
            dropped_deadline += 1;
            // Re-anchor ideal deadline when falling behind so a single stall does not cascade infinitely.
            next_ideal_deadline = now_after_work;
        }

        let occ = frontend.audio.occupied_len();
        pacer.set_target(frontend.audio.target_fill());
        pacer.pace(occ, has_audio_device);
        let after_pace = Instant::now();
        if !missed_deadline
            && now_after_work <= next_ideal_deadline
            && after_pace > next_ideal_deadline
            && after_pace.saturating_duration_since(next_ideal_deadline)
                <= Duration::from_millis(2)
        {
            // Keep open-loop deadline tracking aligned to sub-ms DAC clock drift when pacing cleanly.
            next_ideal_deadline = after_pace;
        }

        let interval_ms = tick_start.elapsed().as_secs_f64() * 1000.0;
        let underruns_now = audio_out.underruns();
        let underrun_delta = underruns_now.saturating_sub(prev_underruns);
        prev_underruns = underruns_now;

        let occ_pct = audio_out.occupancy_percent();
        let elapsed_s = run_start.elapsed().as_secs_f64();
        let eff_frame = replay.effective_frame(abs_frame, cfg.loop_replay);
        let scene = replay.scene_at(eff_frame).to_string();

        if let Some(tr) = governor.record_frame(work_ms) {
            frontend.filter = tr.to;
            if frontend
                .software_wgpu
                .as_ref()
                .is_none_or(|ctx| ctx.is_cpu_adapter())
            {
                frontend.postproc.prewarm(
                    frontend.color_correction,
                    frontend.lcd_ghosting,
                    tr.to,
                    false,
                    frontend.lcd_grid_intensity,
                );
            }
            governor_transitions += 1;
            governor_settled_frame = Some(frame_idx);
            governor_settled_s = Some(elapsed_s);
            underruns_during_settling = underruns_now;
            let action = match tr.kind {
                crate::filters::GovernorTransitionKind::StepDown => "GOVERNOR_STEP_DOWN",
                crate::filters::GovernorTransitionKind::StepUp => "GOVERNOR_STEP_UP",
            };
            let from_slug = filter_cli_slug(tr.from);
            let to_slug = filter_cli_slug(tr.to);
            println!(
                "  [NOTICE] frame={frame_idx} ({elapsed_s:.3}s): Quality Governor {action} {from_slug} -> {to_slug} (rolling_p95={:.3} ms, underruns_during_settling={underruns_during_settling})",
                tr.rolling_p95_ms
            );
            writeln!(
                log,
                "[{elapsed_s:7.3}s] frame={frame_idx:5} scene={scene} kind={action} from={from_slug} to={to_slug} rolling_p95_ms={:.3} underruns_during_settling={underruns_during_settling}",
                tr.rolling_p95_ms
            )
            .map_err(|e| format!("log write error: {e}"))?;
            events.push(RealtimeEvent {
                frame: frame_idx,
                elapsed_s,
                scene: scene.clone(),
                kind: format!("{action}:{from_slug}->{to_slug}"),
                work_ms,
                interval_ms,
                underrun_delta,
                audio_occupancy_pct: occ_pct,
            });
        }

        work_ms_samples.push(work_ms);
        present_ms_samples.push(present_ms);
        interval_ms_samples.push(interval_ms);

        let is_work_drop = work_ms > FRAME_BUDGET_MS;
        let is_2x_work = work_ms > TWO_X_BUDGET_MS;
        let is_2x_interval = interval_ms > TWO_X_BUDGET_MS;

        if is_work_drop {
            dropped_work += 1;
        }
        if is_2x_work {
            over_2x_work += 1;
        }
        if is_2x_interval {
            over_2x_interval += 1;
        }

        if is_2x_work || is_2x_interval || is_work_drop || missed_deadline || underrun_delta > 0 {
            let mut flags = Vec::new();
            if is_2x_work {
                flags.push("OVER_2X_WORK");
            } else if is_work_drop {
                flags.push("DROPPED_WORK");
            }
            if is_2x_interval {
                flags.push("OVER_2X_INTERVAL");
            }
            if missed_deadline {
                flags.push("DEADLINE_DROP");
            }
            if underrun_delta > 0 {
                flags.push("AUDIO_UNDERRUN");
            }
            let kind = flags.join("|");
            writeln!(
                log,
                "[{elapsed_s:7.3}s] frame={frame_idx:5} scene={scene} kind={kind} work_ms={work_ms:.3} present_ms={present_ms:.3} interval_ms={interval_ms:.3} underrun_delta={underrun_delta} audio_occ={occ_pct:.1}%"
            )
            .map_err(|e| format!("log write error: {e}"))?;
            events.push(RealtimeEvent {
                frame: frame_idx,
                elapsed_s,
                scene: scene.clone(),
                kind,
                work_ms,
                interval_ms,
                underrun_delta,
                audio_occupancy_pct: occ_pct,
            });
        }

        if (frame_idx + 1) % 300 == 0 || frame_idx + 1 == frames {
            writeln!(
                log,
                "[{elapsed_s:7.3}s] HEARTBEAT frame={}/{} scene={scene} work_ms={work_ms:.3} interval_ms={interval_ms:.3} dropped_work={dropped_work} dropped_deadline={dropped_deadline} over_2x_work={over_2x_work} over_2x_interval={over_2x_interval} underruns={underruns_now} audio_occ={occ_pct:.1}%",
                frame_idx + 1,
                frames,
            )
            .map_err(|e| format!("log write error: {e}"))?;
            log.flush().map_err(|e| format!("log flush error: {e}"))?;
        }
    }

    let wall_duration_s = run_start.elapsed().as_secs_f64();
    let total_audio_underruns = audio_out.underruns();
    let work_summary = summarize_samples(&work_ms_samples);
    let work_post_warmup = if work_ms_samples.len() > 1 {
        summarize_samples(&work_ms_samples[1..])
    } else {
        work_summary.clone()
    };
    let present_summary = summarize_samples(&present_ms_samples);
    let interval_summary = summarize_samples(&interval_ms_samples);
    let presentation_interval_summary = summarize_samples(&frontend.presentation_intervals);
    let cadence_jitter_frames = frontend.cadence_jitter_frames;
    let active_filter = if governor_enabled {
        governor.active_filter()
    } else {
        frontend.filter
    };
    let final_filter_slug = if cfg.filter_color_correction && active_filter == filter {
        "color-correction".to_string()
    } else {
        filter_cli_slug(active_filter).to_string()
    };
    let underruns_post_settle = total_audio_underruns.saturating_sub(underruns_during_settling);
    let work_over_budget_frames = dropped_work;
    let over_budget_pct = (work_over_budget_frames as f64 / frames as f64) * 100.0;
    let work_over_budget_post_warmup_frames = work_ms_samples
        .iter()
        .skip(1)
        .filter(|&&w| w > FRAME_BUDGET_MS)
        .count();
    let over_budget_post_warmup_pct = if frames > 1 {
        (work_over_budget_post_warmup_frames as f64 / (frames - 1) as f64) * 100.0
    } else {
        0.0
    };

    let settle_s_str = governor_settled_s
        .map(|s| format!("{s:.3}s"))
        .unwrap_or_else(|| "none".to_string());
    writeln!(
        log,
        "# SUMMARY wall_s={wall_duration_s:.3} frames={frames} dropped_work={dropped_work} dropped_deadline={dropped_deadline} skipped_presents={} over_2x_work={over_2x_work} over_2x_interval={over_2x_interval} audio_underruns={total_audio_underruns} underruns_during_settling={underruns_during_settling} underruns_post_settle={underruns_post_settle} over_budget_pct={over_budget_pct:.3}% over_budget_post_warmup_pct={over_budget_post_warmup_pct:.3}% initial_filter={initial_filter_slug} final_filter={final_filter_slug} governor_transitions={governor_transitions} governor_settled_s={settle_s_str} work_avg_ms={:.4} work_p99_ms={:.4} work_worst_ms={:.4} work_post_warmup_worst_ms={:.4} interval_p99_ms={:.4} interval_worst_ms={:.4} present_interval_p99_ms={:.4} present_interval_max_ms={:.4} cadence_jitter_frames={cadence_jitter_frames}",
        frontend.skipped_presents,
        work_summary.avg_ms,
        work_summary.p99_ms,
        work_summary.worst_ms,
        work_post_warmup.worst_ms,
        interval_summary.p99_ms,
        interval_summary.worst_ms,
        presentation_interval_summary.p99_ms,
        presentation_interval_summary.worst_ms,
    )
    .map_err(|e| format!("log write error: {e}"))?;
    log.flush().map_err(|e| format!("log flush error: {e}"))?;

    let measured_filter_costs_json = governor
        .measured_costs()
        .iter()
        .map(|c| RealtimeFilterCostJson {
            filter: filter_cli_slug(c.filter).to_string(),
            cost_ms: c.cost_ms,
        })
        .collect();

    let rt_json = RealtimeReportJson {
        schema_version: 2,
        timestamp_utc: ts,
        rom_name,
        filter: initial_filter_slug.clone(),
        frames,
        slowdown,
        wall_duration_s,
        audio_device_available: has_audio_device,
        host_sample_rate_hz: host_rate,
        target_fps: GBA_FPS,
        frame_budget_ms: FRAME_BUDGET_MS,
        two_x_budget_ms: TWO_X_BUDGET_MS,
        dropped_frames_work: dropped_work,
        dropped_frames_deadline: dropped_deadline,
        frames_over_2x_work: over_2x_work,
        frames_over_2x_interval: over_2x_interval,
        total_audio_underruns,
        work_summary: work_summary.clone(),
        work_post_warmup: work_post_warmup.clone(),
        present_summary,
        interval_summary,
        presentation_interval_summary: presentation_interval_summary.clone(),
        cadence_jitter_frames,
        log_path: log_path.to_string_lossy().to_string(),
        events,
        initial_filter: initial_filter_slug,
        final_filter: final_filter_slug.clone(),
        present_backend: present_backend.clone(),
        emu_priority_raised,
        emu_mmcss_active,
        emu_thread_priority,
        governor_enabled,
        governor_transitions,
        governor_settled_frame,
        governor_settled_s,
        underruns_during_settling,
        underruns_post_settle,
        measured_filter_costs: measured_filter_costs_json,
        work_over_budget_frames,
        over_budget_pct,
        work_over_budget_post_warmup_frames,
        over_budget_post_warmup_pct,
        warmup_frames,
    };

    let json_body = serde_json::to_string_pretty(&rt_json)
        .map_err(|e| format!("failed to serialize realtime JSON: {e}"))?;
    fs::write(&json_path, json_body)
        .map_err(|e| format!("failed to write {}: {e}", json_path.display()))?;

    println!(
        "Realtime benchmark complete: {frames} frames in {wall_duration_s:.2}s | filter={final_filter_slug} backend=\"{present_backend}\" | dropped_work={dropped_work} ({over_budget_pct:.3}% over 16.74ms, post-warmup {over_budget_post_warmup_pct:.3}%) dropped_deadline={dropped_deadline} over_2x_work={over_2x_work} over_2x_interval={over_2x_interval} underruns={total_audio_underruns} governor_transitions={governor_transitions}"
    );
    if governor_enabled {
        println!(
            "  Governor: settled={settle_s_str} (frame={:?}) | underruns_during_settling={underruns_during_settling} | underruns_post_settle={underruns_post_settle} | measured_costs=[{measured_costs_summary}]",
            governor_settled_frame
        );
    }
    println!(
        "  Work (all):         avg={:.3} ms | p50={:.3} ms | p95={:.3} ms | p99={:.3} ms | worst={:.3} ms",
        work_summary.avg_ms, work_summary.p50_ms, work_summary.p95_ms, work_summary.p99_ms, work_summary.worst_ms,
    );
    println!(
        "  Work (post-warmup): avg={:.3} ms | p50={:.3} ms | p95={:.3} ms | p99={:.3} ms | worst={:.3} ms",
        work_post_warmup.avg_ms, work_post_warmup.p50_ms, work_post_warmup.p95_ms, work_post_warmup.p99_ms, work_post_warmup.worst_ms,
    );
    println!(
        "  Present-Interval:   avg={:.3} ms | p50={:.3} ms | p95={:.3} ms | p99={:.3} ms | max={:.3} ms | jitter(>2ms off 16.74ms)={cadence_jitter_frames}",
        presentation_interval_summary.avg_ms,
        presentation_interval_summary.p50_ms,
        presentation_interval_summary.p95_ms,
        presentation_interval_summary.p99_ms,
        presentation_interval_summary.worst_ms,
    );
    println!("  Log: {}", log_path.display());
    println!("  JSON: {}", json_path.display());
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct FilterBenchRow {
    pub filter: String,
    pub condition: String,
    pub adapter: String,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub worst_ms: f64,
    pub added_cost_ms: f64,
    pub fps: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FiltersReportJson {
    pub schema_version: u32,
    pub timestamp_utc: String,
    pub rom_name: String,
    pub frames_tested: usize,
    pub normal_adapter: String,
    pub fallback_adapter: String,
    pub rows: Vec<FilterBenchRow>,
}

struct FilterBenchDef {
    slug: &'static str,
    shader_preset: ShaderPreset,
    color_profile_id: i32,
    cpu_filter: DisplayFilter,
    cpu_color_correction: bool,
}

const BENCH_FILTERS: [FilterBenchDef; 8] = [
    FilterBenchDef {
        slug: "none",
        shader_preset: ShaderPreset::Nearest,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::Sharp1x,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "myboy-smooth",
        shader_preset: ShaderPreset::SharpBilinear,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::MyBoySmooth,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "bilinear",
        shader_preset: ShaderPreset::SharpBilinear,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::SoftHandheld,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "color-correction",
        shader_preset: ShaderPreset::Nearest,
        color_profile_id: 1, // Agb001 sRGB LUT
        cpu_filter: DisplayFilter::Sharp1x,
        cpu_color_correction: true,
    },
    FilterBenchDef {
        slug: "lcd-grid",
        shader_preset: ShaderPreset::LcdGrid,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::AuthenticGbaLcd,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "crt-scanlines",
        shader_preset: ShaderPreset::CrtScanlines,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::CrtScanlines,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "hq2x",
        shader_preset: ShaderPreset::ScaleEdge,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::SmoothScale2x,
        cpu_color_correction: false,
    },
    FilterBenchDef {
        slug: "xbrz",
        shader_preset: ShaderPreset::ScaleEdge,
        color_profile_id: 0,
        cpu_filter: DisplayFilter::XbrSmooth3x,
        cpu_color_correction: false,
    },
];

struct FrameCaptureFrontend<'a> {
    replay: &'a InputReplay,
    frame_idx: usize,
    frames: Vec<[u16; LCD_WIDTH * LCD_HEIGHT]>,
}

impl<'a> HostFrontend for FrameCaptureFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.frames.push(*fb);
    }

    fn push_audio_samples(&mut self, _samples: &[i16]) {}

    fn poll_keypad(&mut self) -> u16 {
        self.replay.keyinput_at(self.frame_idx)
    }
}

fn print_sw_profile(label: &str, ctx: &mut BenchWgpuContext, frames: usize) {
    let p = ctx.profile_swiftshader_none(frames);
    println!(
        "{label} ({}x{}) micro-profile ({} frames avg ms):",
        ctx.width, ctx.height, frames.max(20)
    );
    println!(
        "  Adapter: {} ({} / {})",
        ctx.adapter_name, ctx.adapter_backend, ctx.adapter_device_type
    );
    println!(
        "  [A] Full present_frame_sync total:       {:.4} ms",
        p.a_total_sync_ms
    );
    println!(
        "      A1 stage_frames (curr+prev):         {:.4} ms",
        p.a1_stage_ms
    );
    println!(
        "      A2 write_texture (curr 240x160):     {:.4} ms",
        p.a2_flush_curr_ms
    );
    println!(
        "      A3 write_texture (prev 240x160):     {:.4} ms",
        p.a3_flush_prev_ms
    );
    println!(
        "      A4 write_buffer + encode (720x480):  {:.4} ms",
        p.a4_encode_ms
    );
    println!(
        "      A5 submit + poll(Wait):              {:.4} ms",
        p.a5_submit_poll_ms
    );
    println!(
        "  [B] bench-filters 'none' (Clear+fs_near):{:.4} ms",
        p.b_bench_filters_none_ms
    );
    println!(
        "  [C] LoadOp::Load + fs_nearest (720x480): {:.4} ms",
        p.c_loadop_load_ms
    );
    println!(
        "  [D] Empty submit + poll(Wait):           {:.4} ms",
        p.d_empty_submit_poll_ms
    );
    println!(
        "  [E] fs_plain (LoadOp::Load, 720x480):    {:.4} ms",
        p.e_plain_shader_720p_ms
    );
    println!(
        "  [F] write_texture 720x480 + poll(Wait):  {:.4} ms",
        p.f_write_tex_720p_sync_ms
    );
    println!(
        "  [G] write_texture 240x160 + poll(Wait):  {:.4} ms",
        p.g_write_tex_240p_sync_ms
    );
    println!(
        "  [H] write 240x160 + fs_plain 720x480:    {:.4} ms",
        p.h_write_240_plus_plain_720_ms
    );
    println!(
        "  [I] copy_texture 240x160 -> 240x160:     {:.4} ms",
        p.i_copy_tex_240_sync_ms
    );
    println!(
        "  [J] copy_texture 720x480 -> 720x480:     {:.4} ms",
        p.j_copy_tex_720_sync_ms
    );
    println!(
        "  [K] write_buf+copy 720x480 + poll(Wait): {:.4} ms",
        p.k_write_buf_copy_720p_ms
    );
    println!(
        "  [L] fs_plain (LoadOp::Clear, 720x480):   {:.4} ms",
        p.l_plain_clear_720p_ms
    );
}

pub fn run_profile_swiftshader(frames: usize) -> Result<(), String> {
    let mut ctx = BenchWgpuContext::init(true, LCD_WIDTH as u32 * 3, LCD_HEIGHT as u32 * 3)?;
    print_sw_profile("Primary Software CPU", &mut ctx, frames);
    if ctx.adapter_backend != "Dx12"
        && let Ok(mut warp) = BenchWgpuContext::init_with_backends(
            wgpu::Backends::DX12,
            true,
            LCD_WIDTH as u32 * 3,
            LCD_HEIGHT as u32 * 3,
        )
    {
        println!();
        print_sw_profile("DX12 WARP Software", &mut warp, frames);
    }
    Ok(())
}

pub fn run_bench_filters(cfg: &CliConfig) -> Result<(), String> {
    let LoadedRomInputs {
        rom,
        bios,
        sav,
        rom_name,
    } = load_rom_and_bios(cfg)?;
    let replay = load_replay(cfg)?;
    let frames = cfg.frames.min(600).max(120);
    let ts = format_utc_timestamp();

    fs::create_dir_all(&cfg.perf_dir)
        .map_err(|e| format!("failed to create perf dir {}: {e}", cfg.perf_dir.display()))?;

    println!("Pre-capturing {frames} real emulation frames for filter benchmark...");
    let mut core = Box::new(GbaCore::new(bios.as_deref(), rom));
    if let Some(bytes) = &sav {
        core.bus.cart.load_save_data(bytes);
    }
    let mut capture_front = FrameCaptureFrontend {
        replay: &replay,
        frame_idx: 0,
        frames: Vec::with_capacity(frames),
    };

    for frame_idx in 0..frames {
        capture_front.frame_idx = frame_idx;
        core.run_frame(&mut capture_front);
    }
    let captured_frames = capture_front.frames;
    println!("Captured {} frames successfully.", captured_frames.len());

    println!("Step 1: probing software CPU adapters (Vulkan / DX12 WARP) & normal GPU adapter...");
    // Register/probe the CPU software Vulkan ICD first so the Vulkan loader
    // sees both hardware GPU and SwiftShader/Lavapipe ICDs in this process.
    let software_ctx = BenchWgpuContext::init(true, LCD_WIDTH as u32 * 3, LCD_HEIGHT as u32 * 3);
    let warp_ctx = if !software_ctx
        .as_ref()
        .is_ok_and(|c| c.adapter_backend == "Dx12")
    {
        BenchWgpuContext::init_with_backends(
            wgpu::Backends::DX12,
            true,
            LCD_WIDTH as u32 * 3,
            LCD_HEIGHT as u32 * 3,
        )
        .ok()
    } else {
        None
    };
    let normal_ctx = BenchWgpuContext::init(false, LCD_WIDTH as u32 * 3, LCD_HEIGHT as u32 * 3);

    let normal_name = match &normal_ctx {
        Ok(c) => format!("{} ({} / {})", c.adapter_name, c.adapter_backend, c.adapter_device_type),
        Err(e) => format!("Unavailable ({e})"),
    };
    let fallback_name = match &software_ctx {
        Ok(c) => format!("{} ({} / {})", c.adapter_name, c.adapter_backend, c.adapter_device_type),
        Err(e) => format!("Unavailable ({e})"),
    };

    println!("Target Adapters:");
    println!("  Normal Adapter (GPU):    {normal_name}");
    println!("  Software CPU Adapter:    {fallback_name}");
    if let Some(ref w) = warp_ctx {
        println!(
            "  DX12 WARP Adapter:       {} ({} / {})",
            w.adapter_name, w.adapter_backend, w.adapter_device_type
        );
    }

    let mut all_rows: Vec<FilterBenchRow> = Vec::new();

    if let Ok(mut ctx) = normal_ctx {
        all_rows.extend(bench_filter_rows_wgpu(&mut ctx, &captured_frames, "normal-gpu", true));
    }

    match software_ctx {
        Ok(mut ctx) => {
            all_rows.extend(bench_filter_rows_wgpu(
                &mut ctx,
                &captured_frames,
                "software-vulkan",
                true,
            ));
        }
        Err(err) => {
            println!("  [WARN] True CPU software GPU adapter unavailable: {err}");
        }
    }

    if let Some(mut ctx) = warp_ctx {
        all_rows.extend(bench_filter_rows_wgpu(
            &mut ctx,
            &captured_frames,
            "dx12-warp",
            true,
        ));
    }

    // Condition 4: CPU pure-software fallback rasterizer + no-GPU XRGB8 blitter
    all_rows.extend(bench_filter_rows_cpu(&captured_frames, "cpu-software", true));
    all_rows.extend(bench_filter_rows_no_gpu(
        &captured_frames,
        "no-gpu-xrgb8",
        true,
    ));

    if let Ok(sb_timings) = bench_real_softbuffer_present(frames.min(180)) {
        println!("\n=== Real Windowed winit+softbuffer Present (none vs myboy-smooth) ===");
        for (slug, avg_ms, p95_ms) in sb_timings {
            println!("  [no-gpu-softbuffer] {slug:<16} | avg={avg_ms:.4} ms | p95={p95_ms:.4} ms");
        }
    }

    let report_json = FiltersReportJson {
        schema_version: 1,
        timestamp_utc: ts.clone(),
        rom_name,
        frames_tested: captured_frames.len(),
        normal_adapter: normal_name,
        fallback_adapter: fallback_name,
        rows: all_rows,
    };

    let json_path = cfg.perf_dir.join(format!("filters-{ts}.json"));
    let json_body = serde_json::to_string_pretty(&report_json)
        .map_err(|e| format!("failed to serialize filters JSON: {e}"))?;
    fs::write(&json_path, json_body)
        .map_err(|e| format!("failed to write {}: {e}", json_path.display()))?;

    println!("\nFilters benchmark complete! JSON written to: {}", json_path.display());
    Ok(())
}

fn bench_filter_rows_wgpu(
    ctx: &mut BenchWgpuContext,
    captured_frames: &[[u16; LCD_WIDTH * LCD_HEIGHT]],
    cond_name: &str,
    verbose: bool,
) -> Vec<FilterBenchRow> {
    let adapter_desc = if ctx.is_cpu_adapter() {
        format!(
            "{} ({} / {}) [CPU filter + plain texture]",
            ctx.adapter_name, ctx.adapter_backend, ctx.adapter_device_type
        )
    } else {
        format!(
            "{} ({} / {})",
            ctx.adapter_name, ctx.adapter_backend, ctx.adapter_device_type
        )
    };
    if verbose {
        println!("\n=== Benchmarking Condition: {cond_name} on {adapter_desc} ===");
    }

    let mut rows = Vec::with_capacity(BENCH_FILTERS.len());
    let mut none_avg = 0.0;

    if ctx.is_cpu_adapter() {
        let mut postproc = VideoPostProcessor::new();
        for def in &BENCH_FILTERS {
            postproc.prewarm(def.cpu_color_correction, false, def.cpu_filter, false, 0.08);
            let linear = def.cpu_filter.texture_options() == egui::TextureOptions::LINEAR;

            for i in 0..5.min(captured_frames.len()) {
                let img = postproc.process_display_filter(
                    &captured_frames[i],
                    def.cpu_color_correction,
                    false,
                    def.cpu_filter,
                    false,
                    0.08,
                );
                ctx.present_plain_rgba_sync(
                    img.as_raw(),
                    img.size[0] as u32,
                    img.size[1] as u32,
                    linear,
                );
            }

            let mut samples = Vec::with_capacity(captured_frames.len());
            for fb in captured_frames {
                let t0 = Instant::now();
                let img = postproc.process_display_filter(
                    fb,
                    def.cpu_color_correction,
                    false,
                    def.cpu_filter,
                    false,
                    0.08,
                );
                ctx.present_plain_rgba_sync(
                    img.as_raw(),
                    img.size[0] as u32,
                    img.size[1] as u32,
                    linear,
                );
                samples.push(t0.elapsed().as_secs_f64() * 1000.0);
            }

            let summary = summarize_samples(&samples);
            if def.slug == "none" {
                none_avg = summary.avg_ms;
            }
            let added_cost_ms = (summary.avg_ms - none_avg).max(0.0);
            if verbose {
                println!(
                    "  [{cond_name}] {:<16} | avg={:.4} ms | p50={:.4} ms | p95={:.4} ms | p99={:.4} ms | worst={:.4} ms | added={:+.4} ms | fps={:.1}",
                    def.slug,
                    summary.avg_ms,
                    summary.p50_ms,
                    summary.p95_ms,
                    summary.p99_ms,
                    summary.worst_ms,
                    added_cost_ms,
                    summary.avg_fps
                );
            }
            rows.push(FilterBenchRow {
                filter: def.slug.to_string(),
                condition: cond_name.to_string(),
                adapter: adapter_desc.clone(),
                avg_ms: summary.avg_ms,
                p50_ms: summary.p50_ms,
                p95_ms: summary.p95_ms,
                p99_ms: summary.p99_ms,
                worst_ms: summary.worst_ms,
                added_cost_ms,
                fps: summary.avg_fps,
            });
        }
    } else {
        for def in &BENCH_FILTERS {
            for i in 0..5.min(captured_frames.len()) {
                let fb = &captured_frames[i];
                ctx.pipeline.stage_frames(fb, fb, false);
                ctx.pipeline.flush_uploads();
                let mut enc = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                ctx.pipeline.render_to_texture_view(
                    &mut enc,
                    &ctx.output_view,
                    def.shader_preset,
                    def.color_profile_id,
                    ctx.width,
                    ctx.height,
                );
                ctx.queue.submit(std::iter::once(enc.finish()));
                ctx.device.poll(wgpu::Maintain::Wait);
            }

            let mut samples = Vec::with_capacity(captured_frames.len());
            for i in 0..captured_frames.len() {
                let curr = &captured_frames[i];
                let prev = if i > 0 { &captured_frames[i - 1] } else { curr };
                ctx.pipeline.stage_frames(curr, prev, i > 0);
                ctx.pipeline.flush_uploads();

                let t0 = Instant::now();
                let mut enc = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                ctx.pipeline.render_to_texture_view(
                    &mut enc,
                    &ctx.output_view,
                    def.shader_preset,
                    def.color_profile_id,
                    ctx.width,
                    ctx.height,
                );
                ctx.queue.submit(std::iter::once(enc.finish()));
                ctx.device.poll(wgpu::Maintain::Wait);
                samples.push(t0.elapsed().as_secs_f64() * 1000.0);
            }

            let summary = summarize_samples(&samples);
            if def.slug == "none" {
                none_avg = summary.avg_ms;
            }
            let added_cost_ms = (summary.avg_ms - none_avg).max(0.0);
            if verbose {
                println!(
                    "  [{cond_name}] {:<16} | avg={:.4} ms | p50={:.4} ms | p95={:.4} ms | p99={:.4} ms | worst={:.4} ms | added={:+.4} ms | fps={:.1}",
                    def.slug,
                    summary.avg_ms,
                    summary.p50_ms,
                    summary.p95_ms,
                    summary.p99_ms,
                    summary.worst_ms,
                    added_cost_ms,
                    summary.avg_fps
                );
            }
            rows.push(FilterBenchRow {
                filter: def.slug.to_string(),
                condition: cond_name.to_string(),
                adapter: adapter_desc.clone(),
                avg_ms: summary.avg_ms,
                p50_ms: summary.p50_ms,
                p95_ms: summary.p95_ms,
                p99_ms: summary.p99_ms,
                worst_ms: summary.worst_ms,
                added_cost_ms,
                fps: summary.avg_fps,
            });
        }
    }

    rows
}

fn bench_filter_rows_cpu(
    captured_frames: &[[u16; LCD_WIDTH * LCD_HEIGHT]],
    cond_name: &str,
    verbose: bool,
) -> Vec<FilterBenchRow> {
    if verbose {
        println!("\n=== Benchmarking Condition: cpu-software-fallback ===");
    }
    let mut rows = Vec::with_capacity(BENCH_FILTERS.len());
    let mut none_avg_cpu = 0.0;
    let mut postproc = VideoPostProcessor::new();

    for def in &BENCH_FILTERS {
        postproc.prewarm(def.cpu_color_correction, false, def.cpu_filter, false, 0.08);
        for i in 0..5.min(captured_frames.len()) {
            let img = postproc.process_display_filter(
                &captured_frames[i],
                def.cpu_color_correction,
                false,
                def.cpu_filter,
                false,
                0.08,
            );
            black_box(img.pixels.as_ptr());
        }

        let mut samples = Vec::with_capacity(captured_frames.len());
        for fb in captured_frames {
            let t0 = Instant::now();
            let img = postproc.process_display_filter(
                fb,
                def.cpu_color_correction,
                false,
                def.cpu_filter,
                false,
                0.08,
            );
            black_box(img.pixels.as_ptr());
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }

        let summary = summarize_samples(&samples);
        if def.slug == "none" {
            none_avg_cpu = summary.avg_ms;
        }
        let added_cost_ms = (summary.avg_ms - none_avg_cpu).max(0.0);
        if verbose {
            println!(
                "  [{cond_name}] {:<16} | avg={:.4} ms | p50={:.4} ms | p95={:.4} ms | p99={:.4} ms | worst={:.4} ms | added={:+.4} ms | fps={:.1}",
                def.slug,
                summary.avg_ms,
                summary.p50_ms,
                summary.p95_ms,
                summary.p99_ms,
                summary.worst_ms,
                added_cost_ms,
                summary.avg_fps
            );
        }
        rows.push(FilterBenchRow {
            filter: def.slug.to_string(),
            condition: cond_name.to_string(),
            adapter: "Host CPU (VideoPostProcessor)".to_string(),
            avg_ms: summary.avg_ms,
            p50_ms: summary.p50_ms,
            p95_ms: summary.p95_ms,
            p99_ms: summary.p99_ms,
            worst_ms: summary.worst_ms,
            added_cost_ms,
            fps: summary.avg_fps,
        });
    }
    rows
}

/// Helper used by `--headless --dump-scene-pngs <dir>` to verify visual scene progression.
pub fn save_rgb_png_from_bgr555(
    fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
    path: &Path,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create dir {}: {e}", parent.display()))?;
    }
    let mut rgb = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 3];
    for (i, &c) in fb_bgr555.iter().enumerate() {
        let r5 = (c & 0x1F) as u8;
        let g5 = ((c >> 5) & 0x1F) as u8;
        let b5 = ((c >> 10) & 0x1F) as u8;
        rgb[i * 3] = (r5 << 3) | (r5 >> 2);
        rgb[i * 3 + 1] = (g5 << 3) | (g5 >> 2);
        rgb[i * 3 + 2] = (b5 << 3) | (b5 >> 2);
    }
    image::save_buffer_with_format(
        path,
        &rgb,
        LCD_WIDTH as u32,
        LCD_HEIGHT as u32,
        image::ColorType::Rgb8,
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("PNG write failed ({}): {e}", path.display()))
}

pub fn run_calibrate() -> Result<(), String> {
    let (elapsed_ms, acc) = measure_calibrate_ms();
    println!("CALIBRATION_MS={elapsed_ms:.3} CHECKSUM=0x{acc:016x}");
    Ok(())
}

/// Reference machine idle calibration loop baseline (Ryzen test host, 3-run median).
pub const SELFTEST_REFERENCE_CALIB_MS: f64 = 75.769;
pub const SELFTEST_PASS_DEFINITION: &str = "emulation avg < 50% of 16.743ms frame budget, >=16 distinct colors rendered, and paced post-120f-warmup run achieves <0.5% frames over budget, 0 spikes >2x budget (33.485ms), and 0 audio underruns";

#[derive(Clone, Debug, Serialize)]
pub struct HostSystemInfo {
    pub cpu_model: String,
    pub physical_cores: usize,
    pub logical_threads: usize,
    pub total_ram_mb: u64,
    pub os_version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SoftbufferPresentSample {
    pub filter: String,
    pub avg_ms: f64,
    pub p95_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelftestSweepPoint {
    pub slowdown_x: usize,
    pub frames: usize,
    pub work_avg_ms: f64,
    pub work_p95_ms: f64,
    pub work_p995_ms: f64,
    pub work_worst_ms: f64,
    pub over_budget_frames: usize,
    pub over_budget_pct: f64,
    pub over_2x_work: usize,
    pub over_2x_interval: usize,
    pub audio_underruns: u64,
    pub present_p99_ms: f64,
    pub present_max_ms: f64,
    pub cadence_jitter_frames: usize,
    pub smooth_passed: bool,
    pub zero_over_budget: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelftestReportJson {
    pub schema_version: u32,
    pub timestamp_utc: String,
    pub exe_sha256: String,
    pub git_commit: String,
    pub build_time_utc: String,
    pub host_info: HostSystemInfo,
    pub rom_name: String,
    pub available_adapters: Vec<EnumeratedAdapterInfo>,
    pub adapter_name: String,
    pub adapter_backend: String,
    pub adapter_device_type: String,
    pub render_path: String,
    pub chosen_adapter_reason: String,
    pub fallback_order: Vec<String>,
    pub adapter_fallback_reason: Option<String>,
    pub backend_cpu_adapter_ms: Option<f64>,
    pub backend_no_gpu_ms: Option<f64>,
    pub calibration_ms: f64,
    pub reference_calib_ms: f64,
    pub calibration_ratio_vs_ref: f64,
    pub calibration_checksum: String,
    pub headless_frames: usize,
    pub headless_summary: LatencySummary,
    pub headless_emu_avg_ms: f64,
    pub measured_headroom: f64,
    pub paced_warmup_discarded: usize,
    pub paced_frames: usize,
    pub paced_warmup_p995_ms: f64,
    pub paced_work_summary: LatencySummary,
    pub paced_p995_ms: f64,
    pub paced_dropped_work: usize,
    pub paced_over_budget_pct: f64,
    pub paced_over_2x_work: usize,
    pub paced_over_2x_interval: usize,
    pub paced_audio_underruns: u64,
    pub paced_present_interval_summary: LatencySummary,
    pub paced_cadence_jitter_frames: usize,
    pub fixed_gpu_wait_avg_ms: f64,
    pub fixed_gpu_wait_p95_ms: f64,
    pub fixed_gpu_wait_p995_ms: f64,
    pub slowdown_sweep: Vec<SelftestSweepPoint>,
    pub measured_smooth_ceiling_x: usize,
    pub measured_zero_over_budget_x: usize,
    pub recommended_filter: String,
    pub recommended_filter_reason: String,
    pub pass_definition: String,
    pub pacing_diagnosis: String,
    pub softbuffer_present_timings: Vec<SoftbufferPresentSample>,
    pub passed: bool,
    pub fail_reasons: Vec<String>,
    pub filter_timings: Vec<FilterBenchRow>,
}

fn collect_host_system_info() -> HostSystemInfo {
    let logical_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct OsVersionInfoW {
            dw_os_version_info_size: u32,
            dw_major_version: u32,
            dw_minor_version: u32,
            dw_build_number: u32,
            dw_platform_id: u32,
            sz_csd_version: [u16; 128],
        }
        #[repr(C)]
        struct MemoryStatusEx {
            dw_length: u32,
            dw_memory_load: u32,
            ull_total_phys: u64,
            ull_avail_phys: u64,
            ull_total_page_file: u64,
            ull_avail_page_file: u64,
            ull_total_virtual: u64,
            ull_avail_virtual: u64,
            ull_avail_extended_virtual: u64,
        }
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct SystemLogicalProcessorInfo {
            processor_mask: usize,
            relationship: u32,
            reserved: [u64; 2],
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn RtlGetVersion(lp_version_info: *mut OsVersionInfoW) -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
            fn GetLogicalProcessorInformation(
                buffer: *mut SystemLogicalProcessorInfo,
                returned_length: *mut u32,
            ) -> i32;
        }
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn RegGetValueW(
                hkey: isize,
                lp_sub_key: *const u16,
                lp_value: *const u16,
                dw_flags: u32,
                pdw_type: *mut u32,
                pv_data: *mut core::ffi::c_void,
                pcb_data: *mut u32,
            ) -> i32;
        }

        let mut os_version = String::from("Windows (unknown build)");
        unsafe {
            let mut ver = OsVersionInfoW {
                dw_os_version_info_size: std::mem::size_of::<OsVersionInfoW>() as u32,
                dw_major_version: 0,
                dw_minor_version: 0,
                dw_build_number: 0,
                dw_platform_id: 0,
                sz_csd_version: [0; 128],
            };
            if RtlGetVersion(&mut ver) == 0 {
                let edition = if ver.dw_major_version >= 10 && ver.dw_build_number >= 22_000 {
                    "Windows 11"
                } else if ver.dw_major_version >= 10 {
                    "Windows 10"
                } else {
                    "Windows"
                };
                os_version = format!(
                    "{edition} ({}.{}.{})",
                    ver.dw_major_version, ver.dw_minor_version, ver.dw_build_number
                );
            }
        }

        let mut total_ram_mb = 0u64;
        unsafe {
            let mut mem = MemoryStatusEx {
                dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
                dw_memory_load: 0,
                ull_total_phys: 0,
                ull_avail_phys: 0,
                ull_total_page_file: 0,
                ull_avail_page_file: 0,
                ull_total_virtual: 0,
                ull_avail_virtual: 0,
                ull_avail_extended_virtual: 0,
            };
            if GlobalMemoryStatusEx(&mut mem) != 0 {
                total_ram_mb = mem.ull_total_phys / (1024 * 1024);
            }
        }

        let mut physical_cores = 0usize;
        unsafe {
            let mut len = 0u32;
            let _ = GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len);
            let elem_size = std::mem::size_of::<SystemLogicalProcessorInfo>();
            if len as usize >= elem_size {
                let count = len as usize / elem_size;
                let mut buf = vec![
                    SystemLogicalProcessorInfo {
                        processor_mask: 0,
                        relationship: 0,
                        reserved: [0; 2],
                    };
                    count
                ];
                if GetLogicalProcessorInformation(buf.as_mut_ptr(), &mut len) != 0 {
                    let valid = (len as usize / elem_size).min(buf.len());
                    physical_cores = buf[..valid].iter().filter(|e| e.relationship == 0).count();
                }
            }
        }
        if physical_cores == 0 {
            physical_cores = logical_threads;
        }

        let mut cpu_model = std::env::var("PROCESSOR_IDENTIFIER")
            .unwrap_or_else(|_| "x86_64 Windows CPU".to_string());
        unsafe {
            let subkey: Vec<u16> = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0"
                .encode_utf16()
                .collect();
            let val_name: Vec<u16> = "ProcessorNameString\0".encode_utf16().collect();
            let mut buf = [0u16; 256];
            let mut cb = (buf.len() * 2) as u32;
            let hklm: isize = (0x8000_0002u32 as i32) as isize;
            const RRF_RT_REG_SZ: u32 = 0x0000_0002;
            let status = RegGetValueW(
                hklm,
                subkey.as_ptr(),
                val_name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                &mut cb,
            );
            if status == 0 && cb >= 2 {
                let chars = (cb as usize / 2).min(buf.len());
                let raw = String::from_utf16_lossy(&buf[..chars]);
                let trimmed = raw.trim_end_matches('\0').trim();
                if !trimmed.is_empty() {
                    cpu_model = trimmed.to_string();
                }
            }
        }

        HostSystemInfo {
            cpu_model,
            physical_cores,
            logical_threads,
            total_ram_mb,
            os_version,
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        HostSystemInfo {
            cpu_model: std::env::consts::ARCH.to_string(),
            physical_cores: logical_threads,
            logical_threads,
            total_ram_mb: 0,
            os_version: std::env::consts::OS.to_string(),
        }
    }
}

fn bench_filter_rows_no_gpu(
    captured_frames: &[[u16; LCD_WIDTH * LCD_HEIGHT]],
    cond_name: &str,
    verbose: bool,
) -> Vec<FilterBenchRow> {
    let mut rows = Vec::with_capacity(BENCH_FILTERS.len());
    let mut postproc = VideoPostProcessor::new();
    let mut blitter = NoGpuCpuPresenter::new(LCD_WIDTH * 3, LCD_HEIGHT * 3);
    let mut none_avg = 0.0;

    if verbose {
        println!("\n=== Benchmarking Condition: {cond_name} (CPU filter + XRGB8 720x480 blit) ===");
    }
    for def in &BENCH_FILTERS {
        postproc.prewarm(def.cpu_color_correction, false, def.cpu_filter, false, 0.08);
        let linear = def.cpu_filter.texture_options() == egui::TextureOptions::LINEAR;
        for i in 0..5.min(captured_frames.len()) {
            let img = postproc.process_display_filter(
                &captured_frames[i],
                def.cpu_color_correction,
                false,
                def.cpu_filter,
                false,
                0.08,
            );
            let _ = blitter.present_rgba_to_xrgb(
                &img.pixels,
                img.size[0],
                img.size[1],
                LCD_WIDTH * 3,
                LCD_HEIGHT * 3,
                linear,
            );
        }

        let mut samples = Vec::with_capacity(captured_frames.len());
        for fb in captured_frames {
            let t0 = Instant::now();
            let img = postproc.process_display_filter(
                fb,
                def.cpu_color_correction,
                false,
                def.cpu_filter,
                false,
                0.08,
            );
            let xrgb = blitter.present_rgba_to_xrgb(
                &img.pixels,
                img.size[0],
                img.size[1],
                LCD_WIDTH * 3,
                LCD_HEIGHT * 3,
                linear,
            );
            black_box(xrgb.as_ptr());
            black_box(xrgb.len());
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }

        let summary = summarize_samples(&samples);
        if def.slug == "none" {
            none_avg = summary.avg_ms;
        }
        let added_cost_ms = (summary.avg_ms - none_avg).max(0.0);
        if verbose {
            println!(
                "  [{cond_name}] {:<16} | avg={:.4} ms | p50={:.4} ms | p95={:.4} ms | p99={:.4} ms | worst={:.4} ms | added={:+.4} ms | fps={:.1}",
                def.slug,
                summary.avg_ms,
                summary.p50_ms,
                summary.p95_ms,
                summary.p99_ms,
                summary.worst_ms,
                added_cost_ms,
                summary.avg_fps
            );
        }
        rows.push(FilterBenchRow {
            filter: def.slug.to_string(),
            condition: cond_name.to_string(),
            adapter: "winit + softbuffer CPU XRGB8 blitter (no wgpu)".to_string(),
            avg_ms: summary.avg_ms,
            p50_ms: summary.p50_ms,
            p95_ms: summary.p95_ms,
            p99_ms: summary.p99_ms,
            worst_ms: summary.worst_ms,
            added_cost_ms,
            fps: summary.avg_fps,
        });
    }
    rows
}

fn find_bench_filter_def(slug: &str) -> &'static FilterBenchDef {
    BENCH_FILTERS
        .iter()
        .find(|d| d.slug == slug)
        .unwrap_or(&BENCH_FILTERS[0])
}

fn recommend_selftest_filter(
    is_cpu_adapter: bool,
    has_wgpu: bool,
    no_gpu: bool,
    emu_avg_ms: f64,
    rows: &[FilterBenchRow],
) -> (String, String) {
    let half_budget = FRAME_BUDGET_MS * 0.50;
    let cost_for = |slug: &str| -> Option<f64> {
        rows.iter().find(|r| r.filter == slug).map(|r| r.avg_ms)
    };
    let cheapest_row = rows.iter().min_by(|a, b| a.avg_ms.total_cmp(&b.avg_ms));
    let cheapest_slug = cheapest_row
        .map(|r| r.filter.clone())
        .unwrap_or_else(|| "none".to_string());
    let cheapest_cost = cheapest_row.map(|r| r.avg_ms).unwrap_or(0.0);

    if no_gpu || !has_wgpu {
        if let Some(myboy) = cost_for("myboy-smooth")
            && let Some(none) = cost_for("none")
            && myboy <= none + 0.15
            && emu_avg_ms + myboy <= half_budget
        {
            return (
                "myboy-smooth".to_string(),
                format!(
                    "No-GPU softbuffer path: myboy-smooth ({myboy:.3} ms) is within 0.15 ms of none ({none:.3} ms) and keeps emu+filter ({:.3} ms) under 50% budget ({half_budget:.2} ms)",
                    emu_avg_ms + myboy
                ),
            );
        }
        return (
            cheapest_slug.clone(),
            format!(
                "No-GPU path: picked cheapest measured CPU filter `{cheapest_slug}` ({cheapest_cost:.3} ms) for max headroom"
            ),
        );
    }

    if is_cpu_adapter {
        for cand in ["lcd-grid", "none", "myboy-smooth"] {
            if cand == cheapest_slug
                && let Some(c) = cost_for(cand)
                && emu_avg_ms + c <= half_budget
            {
                return (
                    cand.to_string(),
                    format!(
                        "Software CPU wgpu adapter: `{cand}` is the fastest measured filter ({c:.3} ms) and stays well under 50% budget ({:.3} <= {half_budget:.2} ms)",
                        emu_avg_ms + c
                    ),
                );
            }
        }
        return (
            cheapest_slug.clone(),
            format!(
                "Software CPU wgpu adapter: picked fastest measured filter `{cheapest_slug}` ({cheapest_cost:.3} ms)"
            ),
        );
    }

    if let Some(c) = cost_for("myboy-smooth")
        && emu_avg_ms + c <= half_budget
        && c <= 2.0
    {
        let none_ms = cost_for("none").unwrap_or(0.0);
        return (
            "myboy-smooth".to_string(),
            format!(
                "Hardware GPU shader path: myboy-smooth ({c:.3} ms, +{:.3} ms over none) provides clean interpolation while keeping total work ({:.3} ms) far under 50% budget ({half_budget:.2} ms)",
                (c - none_ms).max(0.0),
                emu_avg_ms + c
            ),
        );
    }
    (
        cheapest_slug.clone(),
        format!(
            "Hardware GPU fallback: picked fastest measured filter `{cheapest_slug}` ({cheapest_cost:.3} ms) to keep total work minimal"
        ),
    )
}

struct SelftestPacedMetrics {
    warmup_discarded: usize,
    warmup_p995_ms: f64,
    #[allow(dead_code)]
    warmup_poll_p995_ms: f64,
    poll_wait_avg_ms: f64,
    poll_wait_p995_ms: f64,
    work_summary: LatencySummary,
    p995_ms: f64,
    dropped_work: usize,
    over_budget_pct: f64,
    over_2x_work: usize,
    over_2x_interval: usize,
    audio_underruns: u64,
    present_interval_summary: LatencySummary,
    cadence_jitter_frames: usize,
}

fn run_selftest_paced_window<'a>(
    core: &mut GbaCore,
    frontend: &mut RealtimeFrontend<'a>,
    audio_out: &mut AudioOutput,
    pacer: &mut DrcPacer,
    has_audio_device: bool,
    warmup_frames: usize,
    measured_frames: usize,
    slowdown: usize,
    start_frame_idx: &mut usize,
) -> SelftestPacedMetrics {
    let ideal_period = Duration::from_secs_f64(1.0 / GBA_FPS);
    let mut next_ideal_deadline = Instant::now();
    let mut warmup_work_ms = Vec::with_capacity(warmup_frames);
    let mut warmup_poll_ms = Vec::with_capacity(warmup_frames);
    let mut work_ms_samples = Vec::with_capacity(measured_frames);
    let mut poll_ms_samples = Vec::with_capacity(measured_frames);
    let mut dropped_work = 0usize;
    let mut over_2x_work = 0usize;
    let mut over_2x_interval = 0usize;
    let mut prev_work_ms = 0.0;
    let mut consecutive_skips = 0usize;
    let total_iters = warmup_frames + measured_frames;
    let slowdown = slowdown.max(1);

    frontend.drain_wgpu();
    for step in 0..total_iters {
        if step == warmup_frames {
            frontend.presentation_intervals.clear();
            frontend.cadence_jitter_frames = 0;
            frontend.last_present_wall = None;
            audio_out.reset_underruns();
            next_ideal_deadline = Instant::now();
            prev_work_ms = 0.0;
            consecutive_skips = 0;
        }
        let tick_start = Instant::now();
        frontend.frame_idx = *start_frame_idx;
        *start_frame_idx += 1;
        frontend.last_present_dur = Duration::ZERO;
        frontend.last_gpu_submit_dur = Duration::ZERO;
        if step > 0 && prev_work_ms > TWO_X_BUDGET_MS && consecutive_skips < 1 {
            frontend.skip_present_this_frame = true;
            consecutive_skips += 1;
        } else {
            frontend.skip_present_this_frame = false;
            consecutive_skips = 0;
        }
        core.run_frame(frontend);
        let raw_work_dur = tick_start.elapsed();
        let frame_poll_wait_ms = frontend
            .software_wgpu
            .as_ref()
            .map(|ctx| ctx.last_poll_wait_ms)
            .unwrap_or(0.0);
        // Apply --slowdown N only to CPU emulation + CPU filter/submit work; never multiply
        // GPU fence/poll wait time.
        let poll_wait_dur = Duration::from_secs_f64(frame_poll_wait_ms.max(0.0) / 1000.0);
        let cpu_work_dur = raw_work_dur.saturating_sub(poll_wait_dur);
        if slowdown > 1 {
            let target_dur = cpu_work_dur.mul_f64(slowdown as f64) + poll_wait_dur;
            while tick_start.elapsed() < target_dur {
                std::hint::spin_loop();
            }
        }
        let work_ms = tick_start.elapsed().as_secs_f64() * 1000.0;
        prev_work_ms = work_ms;

        next_ideal_deadline += ideal_period;
        let now_after_work = Instant::now();
        let missed = now_after_work > next_ideal_deadline + ideal_period;
        if missed {
            next_ideal_deadline = now_after_work;
        }
        let occ = frontend.audio.occupied_len();
        pacer.set_target(frontend.audio.target_fill());
        pacer.pace(occ, has_audio_device);
        let after_pace = Instant::now();
        if !missed
            && now_after_work <= next_ideal_deadline
            && after_pace > next_ideal_deadline
            && after_pace.saturating_duration_since(next_ideal_deadline) <= Duration::from_millis(2)
        {
            next_ideal_deadline = after_pace;
        }

        let interval_ms = tick_start.elapsed().as_secs_f64() * 1000.0;
        if step < warmup_frames {
            warmup_work_ms.push(work_ms);
            warmup_poll_ms.push(frame_poll_wait_ms);
        } else {
            if work_ms > FRAME_BUDGET_MS {
                dropped_work += 1;
            }
            if work_ms > TWO_X_BUDGET_MS {
                over_2x_work += 1;
            }
            if interval_ms > TWO_X_BUDGET_MS {
                over_2x_interval += 1;
            }
            work_ms_samples.push(work_ms);
            poll_ms_samples.push(frame_poll_wait_ms);
        }
    }

    let warmup_p995_ms = if warmup_work_ms.is_empty() {
        0.0
    } else {
        percentile_ms(&warmup_work_ms, 99.5)
    };
    let warmup_poll_p995_ms = if warmup_poll_ms.is_empty() {
        0.0
    } else {
        percentile_ms(&warmup_poll_ms, 99.5)
    };
    let poll_wait_avg_ms = if poll_ms_samples.is_empty() {
        0.0
    } else {
        poll_ms_samples.iter().sum::<f64>() / poll_ms_samples.len() as f64
    };
    let poll_wait_p995_ms = if poll_ms_samples.is_empty() {
        0.0
    } else {
        percentile_ms(&poll_ms_samples, 99.5)
    };
    let work_summary = summarize_samples(&work_ms_samples);
    let p995_ms = percentile_ms(&work_ms_samples, 99.5);
    let over_budget_pct = (dropped_work as f64 / measured_frames.max(1) as f64) * 100.0;
    let present_interval_summary = summarize_samples(&frontend.presentation_intervals);
    frontend.drain_wgpu();
    SelftestPacedMetrics {
        warmup_discarded: warmup_frames,
        warmup_p995_ms,
        warmup_poll_p995_ms,
        poll_wait_avg_ms,
        poll_wait_p995_ms,
        work_summary,
        p995_ms,
        dropped_work,
        over_budget_pct,
        over_2x_work,
        over_2x_interval,
        audio_underruns: audio_out.underruns(),
        present_interval_summary,
        cadence_jitter_frames: frontend.cadence_jitter_frames,
    }
}

pub fn run_selftest(cfg: &CliConfig) -> Result<(), String> {
    let rom = crate::stress_rom::build_stress_mode0_4bg_128spr_rom();
    let rom_name = "embedded:STRESS_SELF (Mode0 4xBG + 128 Sprites + Alpha)".to_string();
    let ts = format_utc_timestamp();
    let host_info = collect_host_system_info();

    fs::create_dir_all(&cfg.perf_dir)
        .map_err(|e| format!("failed to create perf dir {}: {e}", cfg.perf_dir.display()))?;

    // 1. Calibration loop (1 warmup + 3 timed median)
    let _ = measure_calibrate_ms();
    let mut cal_samples = [0.0f64; 3];
    let mut cal_checksum = 0u64;
    for slot in &mut cal_samples {
        let (ms, sum) = measure_calibrate_ms();
        *slot = ms;
        cal_checksum = sum;
    }
    cal_samples.sort_by(f64::total_cmp);
    let calibration_ms = cal_samples[1];
    let calibration_ratio_vs_ref = if calibration_ms > 0.0 {
        SELFTEST_REFERENCE_CALIB_MS / calibration_ms
    } else {
        0.0
    };

    // 2. Select backend via automatic startup policy (`Vulkan -> DX12 -> GL -> No-GPU`),
    // preferring No-GPU over DeviceType::Cpu unless startup timing proves otherwise.
    let decision: StartupBackendDecision = choose_desktop_backend(
        cfg.simulate_no_adapter,
        cfg.no_gpu,
        cfg.software_adapter,
        DisplayFilter::MyBoySmooth,
        false,
        false,
        0.08,
    );
    let available_adapters = decision.available_adapters.clone();
    let mut wgpu_probe: Result<BenchWgpuContext, String> = if decision.use_wgpu {
        BenchWgpuContext::init_with_options(
            wgpu::Backends::all(),
            cfg.software_adapter || decision.is_cpu_adapter,
            cfg.simulate_no_adapter,
            LCD_WIDTH as u32 * 3,
            LCD_HEIGHT as u32 * 3,
        )
    } else {
        Err(decision
            .fallback_reason
            .clone()
            .unwrap_or_else(|| decision.reason.clone()))
    };

    let (
        adapter_name,
        adapter_backend,
        adapter_device_type,
        is_cpu_adapter,
        render_path,
        adapter_fallback_reason,
        chosen_adapter_reason,
    ) = match &wgpu_probe {
        Ok(ctx) => {
            let rp = if ctx.is_cpu_adapter() {
                "software-cpu-plain".to_string()
            } else {
                "gpu-shader".to_string()
            };
            (
                ctx.adapter_name.clone(),
                ctx.adapter_backend.clone(),
                ctx.adapter_device_type.clone(),
                ctx.is_cpu_adapter(),
                rp,
                decision.fallback_reason.clone(),
                decision.reason.clone(),
            )
        }
        Err(_) => (
            decision.adapter_name.clone(),
            decision.adapter_backend.clone(),
            decision.adapter_device_type.clone(),
            true,
            "no-gpu-softbuffer".to_string(),
            decision
                .fallback_reason
                .clone()
                .or_else(|| Some(decision.reason.clone())),
            decision.reason.clone(),
        ),
    };

    // 3. Pre-capture 60 animated frames from embedded stress ROM and time every filter
    let replay = InputReplay::default();
    let mut cap_core = Box::new(GbaCore::new(None, rom.clone()));
    let mut cap_fe = FrameCaptureFrontend {
        replay: &replay,
        frame_idx: 0,
        frames: Vec::with_capacity(70),
    };
    for i in 0..70 {
        cap_fe.frame_idx = i;
        cap_core.run_frame(&mut cap_fe);
    }
    let sample_frames = cap_fe.frames[10..70].to_vec();

    let mut filter_timings = match &mut wgpu_probe {
        Ok(ctx) => bench_filter_rows_wgpu(ctx, &sample_frames, &render_path, false),
        Err(_) => bench_filter_rows_no_gpu(&sample_frames, &render_path, false),
    };
    filter_timings.sort_by(|a, b| a.avg_ms.total_cmp(&b.avg_ms));

    // Also measure real windowed OS softbuffer present cost (`none` vs `myboy-smooth`) when running on the no-GPU path.
    let softbuffer_present_timings = if wgpu_probe.is_err() {
        bench_real_softbuffer_present(120)
            .unwrap_or_default()
            .into_iter()
            .map(|(filter, avg_ms, p95_ms)| SoftbufferPresentSample {
                filter,
                avg_ms,
                p95_ms,
            })
            .collect()
    } else {
        Vec::new()
    };

    let (prelim_slug, _) =
        recommend_selftest_filter(is_cpu_adapter, wgpu_probe.is_ok(), cfg.no_gpu, 1.0, &filter_timings);
    let prelim_def = find_bench_filter_def(&prelim_slug);

    // 4. 600 headless unthrottled frames
    const SELFTEST_HEADLESS_FRAMES: usize = 600;
    const SELFTEST_PACED_WARMUP_FRAMES: usize = 120;
    const SELFTEST_PACED_FRAMES: usize = 600;
    const SELFTEST_SWEEP_FRAMES: usize = 300;
    const SELFTEST_SWEEP_NS: [usize; 8] = [1, 2, 4, 6, 8, 12, 16, 24];

    struct HeadlessSelftestFrontend<'a> {
        filter: DisplayFilter,
        color_correction: bool,
        postproc: VideoPostProcessor,
        wgpu_ctx: &'a mut Option<BenchWgpuContext>,
        no_gpu_blitter: Option<NoGpuCpuPresenter>,
        prev_fb: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
        has_prev: bool,
        last_fb: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
        last_present_ms: f64,
    }
    impl<'a> HostFrontend for HeadlessSelftestFrontend<'a> {
        fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
            let t0 = Instant::now();
            if let Some(ctx) = self.wgpu_ctx.as_mut() {
                if ctx.is_cpu_adapter() {
                    let img = self.postproc.process_display_filter(
                        fb,
                        self.color_correction,
                        false,
                        self.filter,
                        false,
                        0.08,
                    );
                    let linear = self.filter.texture_options() == egui::TextureOptions::LINEAR;
                    ctx.present_plain_rgba_sync(
                        img.as_raw(),
                        img.size[0] as u32,
                        img.size[1] as u32,
                        linear,
                    );
                } else {
                    let preset = bench_shader_preset_for_filter(self.filter);
                    let cp_id = if self.color_correction { 1 } else { 0 };
                    ctx.present_frame_sync(fb, &self.prev_fb, self.has_prev, preset, cp_id);
                }
            } else {
                let img = self.postproc.process_display_filter(
                    fb,
                    self.color_correction,
                    false,
                    self.filter,
                    false,
                    0.08,
                );
                if let Some(ref mut blitter) = self.no_gpu_blitter {
                    let linear = self.filter.texture_options() == egui::TextureOptions::LINEAR;
                    let xrgb = blitter.present_rgba_to_xrgb(
                        &img.pixels,
                        img.size[0],
                        img.size[1],
                        LCD_WIDTH * 3,
                        LCD_HEIGHT * 3,
                        linear,
                    );
                    black_box(xrgb.as_ptr());
                } else {
                    black_box(img.pixels.as_ptr());
                }
            }
            self.prev_fb.copy_from_slice(fb);
            self.last_fb.copy_from_slice(fb);
            self.has_prev = true;
            self.last_present_ms += t0.elapsed().as_secs_f64() * 1000.0;
        }
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            0x03FF
        }
    }

    let mut wgpu_opt = wgpu_probe.ok();
    let has_wgpu = wgpu_opt.is_some();
    let mut headless_postproc = VideoPostProcessor::new();
    headless_postproc.prewarm(
        prelim_def.cpu_color_correction,
        false,
        prelim_def.cpu_filter,
        false,
        0.08,
    );
    let mut headless_fe = HeadlessSelftestFrontend {
        filter: prelim_def.cpu_filter,
        color_correction: prelim_def.cpu_color_correction,
        postproc: headless_postproc,
        wgpu_ctx: &mut wgpu_opt,
        no_gpu_blitter: if !has_wgpu {
            Some(NoGpuCpuPresenter::new(LCD_WIDTH * 3, LCD_HEIGHT * 3))
        } else {
            None
        },
        prev_fb: vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice().try_into().unwrap(),
        has_prev: false,
        last_fb: vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice().try_into().unwrap(),
        last_present_ms: 0.0,
    };
    let mut headless_core = Box::new(GbaCore::new(None, rom.clone()));
    let mut headless_total_ms = Vec::with_capacity(SELFTEST_HEADLESS_FRAMES);
    let mut headless_emu_ms = Vec::with_capacity(SELFTEST_HEADLESS_FRAMES);
    for _ in 0..SELFTEST_HEADLESS_FRAMES {
        headless_fe.last_present_ms = 0.0;
        let t0 = Instant::now();
        headless_core.run_frame(&mut headless_fe);
        let tot = t0.elapsed().as_secs_f64() * 1000.0;
        let emu = (tot - headless_fe.last_present_ms).max(0.0);
        headless_total_ms.push(tot);
        headless_emu_ms.push(emu);
    }
    let distinct_colors: std::collections::HashSet<u16> =
        headless_fe.last_fb.iter().copied().collect();
    let rendered_ok = distinct_colors.len() >= 16;

    let headless_summary = summarize_samples(&headless_total_ms);
    let emu_summary = summarize_samples(&headless_emu_ms);
    let measured_headroom = headless_summary.avg_fps / GBA_FPS;

    let (recommended_filter, recommended_filter_reason) = recommend_selftest_filter(
        is_cpu_adapter,
        has_wgpu,
        cfg.no_gpu,
        emu_summary.avg_ms,
        &filter_timings,
    );
    let rec_def = find_bench_filter_def(&recommended_filter);

    let (fixed_gpu_wait_avg_ms, fixed_gpu_wait_p95_ms, fixed_gpu_wait_p995_ms) =
        if let Some(ref mut ctx) = wgpu_opt
            && !ctx.is_cpu_adapter()
        {
            let preset = bench_shader_preset_for_filter(rec_def.cpu_filter);
            let cp_id = if rec_def.cpu_color_correction { 1 } else { 0 };
            ctx.measure_gpu_wait_latency_ms(preset, cp_id, 120)
        } else {
            (0.0, 0.0, 0.0)
        };

    // 5. Paced run (discard first 120 warmup frames for GPU clock ramp / shader warmup, then measure 600 frames)
    let _emu_prio = crate::audio::win_mmcss::MmcssGuard::enter_games();
    let mut audio_out = AudioOutput::start();
    let has_audio_device = audio_out.has_device();
    let capacity = audio_out.capacity();
    audio_out.reset_underruns();
    let mut producer = audio_out
        .take_producer()
        .ok_or("failed to acquire AudioProducer for --selftest")?;

    let mut paced_frontend = RealtimeFrontend::new(
        &replay,
        false,
        rec_def.cpu_filter,
        rec_def.cpu_color_correction,
        wgpu_opt,
        if !has_wgpu {
            Some(NoGpuCpuPresenter::new(LCD_WIDTH * 3, LCD_HEIGHT * 3))
        } else {
            None
        },
        &mut producer,
    );
    paced_frontend.async_gpu_submit = has_wgpu && !is_cpu_adapter;
    let mut pacer = DrcPacer::new(capacity);
    let mut paced_core = Box::new(GbaCore::new(None, rom));
    let mut sweep_frame_idx = 0usize;

    let base_paced = run_selftest_paced_window(
        &mut paced_core,
        &mut paced_frontend,
        &mut audio_out,
        &mut pacer,
        has_audio_device,
        SELFTEST_PACED_WARMUP_FRAMES,
        SELFTEST_PACED_FRAMES,
        1,
        &mut sweep_frame_idx,
    );

    // 6. Measured slowdown sweep N = 1, 2, 4, 6, 8, 12, 16, 24 (300 frames each after warmup, stop on first failure)
    let mut slowdown_sweep = Vec::with_capacity(SELFTEST_SWEEP_NS.len());
    let mut measured_smooth_ceiling_x = 0usize;
    let mut measured_zero_over_budget_x = 0usize;
    for &n in &SELFTEST_SWEEP_NS {
        let pt = run_selftest_paced_window(
            &mut paced_core,
            &mut paced_frontend,
            &mut audio_out,
            &mut pacer,
            has_audio_device,
            0,
            SELFTEST_SWEEP_FRAMES,
            n,
            &mut sweep_frame_idx,
        );
        let smooth_passed = pt.over_budget_pct < 0.50 && pt.audio_underruns == 0;
        let zero_over_budget = pt.dropped_work == 0 && pt.audio_underruns == 0;
        if smooth_passed {
            measured_smooth_ceiling_x = n;
        }
        if zero_over_budget {
            measured_zero_over_budget_x = n;
        }
        slowdown_sweep.push(SelftestSweepPoint {
            slowdown_x: n,
            frames: SELFTEST_SWEEP_FRAMES,
            work_avg_ms: pt.work_summary.avg_ms,
            work_p95_ms: pt.work_summary.p95_ms,
            work_p995_ms: pt.p995_ms,
            work_worst_ms: pt.work_summary.worst_ms,
            over_budget_frames: pt.dropped_work,
            over_budget_pct: pt.over_budget_pct,
            over_2x_work: pt.over_2x_work,
            over_2x_interval: pt.over_2x_interval,
            audio_underruns: pt.audio_underruns,
            present_p99_ms: pt.present_interval_summary.p99_ms,
            present_max_ms: pt.present_interval_summary.worst_ms,
            cadence_jitter_frames: pt.cadence_jitter_frames,
            smooth_passed,
            zero_over_budget,
        });
        if !smooth_passed {
            break;
        }
    }

    let pacing_diagnosis = if has_wgpu && !is_cpu_adapter {
        format!(
            "120f warmup p99.5={:.3} ms -> post-warmup 600f non-blocking async submit (Maintain::Poll) p99.5={:.3} ms (avg={:.3} ms, headless emu avg={:.3} ms). Isolated blocking GPU wait (Maintain::Wait) is fixed at avg={fixed_gpu_wait_avg_ms:.3} ms (p95={fixed_gpu_wait_p95_ms:.3} ms, p99.5={fixed_gpu_wait_p995_ms:.3} ms) per frame and is NEVER multiplied by --slowdown N.",
            base_paced.warmup_p995_ms,
            base_paced.p995_ms,
            base_paced.work_summary.avg_ms,
            emu_summary.avg_ms
        )
    } else {
        format!(
            "120f warmup p99.5={:.3} ms -> post-warmup 600f p99.5={:.3} ms (avg={:.3} ms, poll_wait avg={:.3} ms, p99.5={:.3} ms; headless total avg={:.3} ms, emu avg={:.3} ms).",
            base_paced.warmup_p995_ms,
            base_paced.p995_ms,
            base_paced.work_summary.avg_ms,
            base_paced.poll_wait_avg_ms,
            base_paced.poll_wait_p995_ms,
            headless_summary.avg_ms,
            emu_summary.avg_ms
        )
    };

    let exe_sha256 = current_exe_sha256();
    let git_commit = build_git_commit();
    let build_time_utc = build_time_utc();

    let mut fail_reasons = Vec::new();
    if !rendered_ok {
        fail_reasons.push(format!(
            "stress ROM rendered only {} colors (expected >=16)",
            distinct_colors.len()
        ));
    }
    if emu_summary.avg_ms >= FRAME_BUDGET_MS * 0.50 {
        fail_reasons.push(format!(
            "emulation avg {:.3} ms >= 50% budget ({:.3} ms)",
            emu_summary.avg_ms,
            FRAME_BUDGET_MS * 0.50
        ));
    }
    if base_paced.over_budget_pct >= 0.50 {
        fail_reasons.push(format!(
            "paced over-budget {:.3}% >= 0.50% smooth gate",
            base_paced.over_budget_pct
        ));
    }
    if base_paced.over_2x_work > 0 || base_paced.over_2x_interval > 0 {
        fail_reasons.push(format!(
            "spikes >2x budget (work={}, interval={})",
            base_paced.over_2x_work, base_paced.over_2x_interval
        ));
    }
    if base_paced.audio_underruns > 0 {
        fail_reasons.push(format!("audio underruns={}", base_paced.audio_underruns));
    }
    let passed = fail_reasons.is_empty();

    let fallback_order = crate::shaders::ADAPTER_FALLBACK_ORDER
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let report = SelftestReportJson {
        schema_version: 3,
        timestamp_utc: ts.clone(),
        exe_sha256: exe_sha256.clone(),
        git_commit: git_commit.clone(),
        build_time_utc: build_time_utc.clone(),
        host_info: host_info.clone(),
        rom_name: rom_name.clone(),
        available_adapters: available_adapters.clone(),
        adapter_name: adapter_name.clone(),
        adapter_backend: adapter_backend.clone(),
        adapter_device_type: adapter_device_type.clone(),
        render_path: render_path.clone(),
        chosen_adapter_reason: chosen_adapter_reason.clone(),
        fallback_order,
        adapter_fallback_reason,
        backend_cpu_adapter_ms: decision.cpu_adapter_ms,
        backend_no_gpu_ms: decision.no_gpu_ms,
        calibration_ms,
        reference_calib_ms: SELFTEST_REFERENCE_CALIB_MS,
        calibration_ratio_vs_ref,
        calibration_checksum: format!("0x{cal_checksum:016x}"),
        headless_frames: SELFTEST_HEADLESS_FRAMES,
        headless_summary: headless_summary.clone(),
        headless_emu_avg_ms: emu_summary.avg_ms,
        measured_headroom,
        paced_warmup_discarded: base_paced.warmup_discarded,
        paced_frames: SELFTEST_PACED_FRAMES,
        paced_warmup_p995_ms: base_paced.warmup_p995_ms,
        paced_work_summary: base_paced.work_summary.clone(),
        paced_p995_ms: base_paced.p995_ms,
        paced_dropped_work: base_paced.dropped_work,
        paced_over_budget_pct: base_paced.over_budget_pct,
        paced_over_2x_work: base_paced.over_2x_work,
        paced_over_2x_interval: base_paced.over_2x_interval,
        paced_audio_underruns: base_paced.audio_underruns,
        paced_present_interval_summary: base_paced.present_interval_summary.clone(),
        paced_cadence_jitter_frames: base_paced.cadence_jitter_frames,
        fixed_gpu_wait_avg_ms,
        fixed_gpu_wait_p95_ms,
        fixed_gpu_wait_p995_ms,
        slowdown_sweep: slowdown_sweep.clone(),
        measured_smooth_ceiling_x,
        measured_zero_over_budget_x,
        recommended_filter: recommended_filter.clone(),
        recommended_filter_reason: recommended_filter_reason.clone(),
        pass_definition: SELFTEST_PASS_DEFINITION.to_string(),
        pacing_diagnosis: pacing_diagnosis.clone(),
        softbuffer_present_timings: softbuffer_present_timings.clone(),
        passed,
        fail_reasons: fail_reasons.clone(),
        filter_timings: filter_timings.clone(),
    };

    let json_path = cfg.perf_dir.join(format!("selftest-{ts}.json"));
    let json_body = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("failed to serialize selftest JSON: {e}"))?;
    fs::write(&json_path, json_body)
        .map_err(|e| format!("failed to write {}: {e}", json_path.display()))?;

    let status_str = if passed { "PASS" } else { "FAIL" };
    let filter_line = filter_timings
        .iter()
        .map(|f| format!("{}:{:.3}ms", f.filter, f.avg_ms))
        .collect::<Vec<_>>()
        .join(", ");
    let sweep_line = slowdown_sweep
        .iter()
        .map(|s| {
            format!(
                "{}x(p99.5={:.2}ms,over={:.2}%,{})",
                s.slowdown_x,
                s.work_p995_ms,
                s.over_budget_pct,
                if s.smooth_passed { "PASS" } else { "FAIL" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    println!("=== RUSTBOY ADVANCE PORTABLE --SELFTEST ({status_str}) ===");
    println!(
        "  Build:             commit={git_commit} | built={build_time_utc} | exe_sha256={exe_sha256}"
    );
    println!(
        "  Host:              {} ({}c/{}t) | RAM={} MB | OS={}",
        host_info.cpu_model,
        host_info.physical_cores,
        host_info.logical_threads,
        host_info.total_ram_mb,
        host_info.os_version
    );
    println!("  Embedded ROM:      {rom_name}");
    println!("  Enumerated GPUs:   {} adapter(s) visible to wgpu", available_adapters.len());
    for (i, ad) in available_adapters.iter().enumerate() {
        let drv = if ad.driver_info.is_empty() {
            ad.driver.as_str()
        } else {
            ad.driver_info.as_str()
        };
        println!(
            "    [{}] {} ({} / {}) — driver: {} {}",
            i + 1,
            ad.name,
            ad.backend,
            ad.device_type,
            ad.driver,
            if drv == ad.driver { "" } else { drv }
        );
    }
    println!(
        "  Chosen Adapter:    {adapter_name} ({adapter_backend} / {adapter_device_type}) [{render_path}]"
    );
    println!("  Selection Reason:  {chosen_adapter_reason}");
    println!(
        "  Calibration:       {calibration_ms:.3} ms (ref {SELFTEST_REFERENCE_CALIB_MS:.3} ms, ratio vs ref = {calibration_ratio_vs_ref:.2}x)"
    );
    println!(
        "  Headless (600f):   {:.1} FPS | headroom = {measured_headroom:.2}x | emu_avg={:.3} ms | total avg={:.3} p95={:.3} worst={:.3} ms",
        headless_summary.avg_fps,
        emu_summary.avg_ms,
        headless_summary.avg_ms,
        headless_summary.p95_ms,
        headless_summary.worst_ms
    );
    println!(
        "  Paced (600f +120w): work avg={:.3} p95={:.3} p99.5={:.3} worst={:.3} ms | warmup120_p99.5={:.3} ms | over_budget={} ({:.3}%) | >2x=({}/{}) | underruns={} | present_p99={:.3} max={:.3} jitter={}",
        base_paced.work_summary.avg_ms,
        base_paced.work_summary.p95_ms,
        base_paced.p995_ms,
        base_paced.work_summary.worst_ms,
        base_paced.warmup_p995_ms,
        base_paced.dropped_work,
        base_paced.over_budget_pct,
        base_paced.over_2x_work,
        base_paced.over_2x_interval,
        base_paced.audio_underruns,
        base_paced.present_interval_summary.p99_ms,
        base_paced.present_interval_summary.worst_ms,
        base_paced.cadence_jitter_frames
    );
    if has_wgpu && !is_cpu_adapter {
        println!(
            "  Fixed GPU Wait:    avg={fixed_gpu_wait_avg_ms:.3} ms | p95={fixed_gpu_wait_p95_ms:.3} ms | p99.5={fixed_gpu_wait_p995_ms:.3} ms (unscaled hardware round-trip)"
        );
    }
    println!("  Filter Timings:    [{filter_line}]");
    if !softbuffer_present_timings.is_empty() {
        let sb_line = softbuffer_present_timings
            .iter()
            .map(|s| format!("{}:avg={:.3}ms/p95={:.3}ms", s.filter, s.avg_ms, s.p95_ms))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  Softbuffer Present:[{sb_line}]");
    }
    println!("  Slowdown Sweep:    [{sweep_line}]");
    println!(
        "  Measured Ceiling:  smooth (<0.5% over, 0 underruns) = {measured_smooth_ceiling_x}x | zero-over-budget = {measured_zero_over_budget_x}x"
    );
    println!("  Recommended Filter: {recommended_filter} — {recommended_filter_reason}");
    println!("  Pacing Notes:      {pacing_diagnosis}");
    println!("  PASS Definition:   {SELFTEST_PASS_DEFINITION}");
    if !fail_reasons.is_empty() {
        println!("  Fail Reasons:      {}", fail_reasons.join("; "));
    }
    println!("  JSON:              {}", json_path.display());

    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameHashesArchive {
    pub rom_name: String,
    pub filter: String,
    pub frames: usize,
    pub overall_core_video_crc32: u32,
    pub overall_presented_video_crc32: u32,
    pub overall_audio_crc32: u32,
    pub core_video_hashes: Vec<u32>,
    pub presented_video_hashes: Vec<u32>,
    pub audio_hashes: Vec<u32>,
}

struct HashCollectorFrontend<'a> {
    inner: BenchFrontend<'a>,
    frame_idx: usize,
    cur_core_video_hash: u32,
    cur_presented_video_hash: u32,
    cur_audio_hasher: crc32fast::Hasher,
    overall_core_video: crc32fast::Hasher,
    overall_presented_video: crc32fast::Hasher,
    overall_audio: crc32fast::Hasher,
}

impl<'a> HashCollectorFrontend<'a> {
    fn new(replay: &'a InputReplay, filter: DisplayFilter) -> Self {
        Self {
            inner: BenchFrontend::new(replay, filter),
            frame_idx: 0,
            cur_core_video_hash: 0,
            cur_presented_video_hash: 0,
            cur_audio_hasher: crc32fast::Hasher::new(),
            overall_core_video: crc32fast::Hasher::new(),
            overall_presented_video: crc32fast::Hasher::new(),
            overall_audio: crc32fast::Hasher::new(),
        }
    }

    fn reset_frame(&mut self, frame_idx: usize) {
        self.frame_idx = frame_idx;
        self.inner.frame_idx = frame_idx;
        self.cur_audio_hasher = crc32fast::Hasher::new();
    }

    fn take_frame_hashes(&mut self) -> (u32, u32, u32) {
        let a_hash = std::mem::take(&mut self.cur_audio_hasher).finalize();
        (self.cur_core_video_hash, self.cur_presented_video_hash, a_hash)
    }
}

impl<'a> HostFrontend for HashCollectorFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        let mut core_h = crc32fast::Hasher::new();
        for &px in fb.iter() {
            core_h.update(&px.to_le_bytes());
            self.overall_core_video.update(&px.to_le_bytes());
        }
        self.cur_core_video_hash = core_h.finalize();

        let prev = self.inner.has_prev.then_some(self.inner.prev_fb.as_ref());
        self.inner.postproc.sync_rgba_for_capture(
            fb,
            prev,
            self.frame_idx as u64,
            self.inner.color_correction,
            self.inner.color_profile,
            self.inner.frame_blending,
            self.inner.lcd_ghosting,
        );
        let img = self.inner.postproc.process_display_filter(
            fb,
            self.inner.color_correction,
            self.inner.lcd_ghosting,
            self.inner.filter,
            false,
            self.inner.lcd_grid_intensity,
        );
        let mut pres_h = crc32fast::Hasher::new();
        for c in &img.pixels {
            let rgba = c.to_array();
            pres_h.update(&rgba);
            self.overall_presented_video.update(&rgba);
        }
        self.cur_presented_video_hash = pres_h.finalize();

        self.inner.prev_fb.copy_from_slice(fb);
        self.inner.has_prev = true;
        self.inner.frames_presented += 1;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        for &s in samples {
            let b = s.to_le_bytes();
            self.cur_audio_hasher.update(&b);
            self.overall_audio.update(&b);
        }
        self.inner.audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        self.inner.replay.keyinput_at(self.frame_idx)
    }
}

pub fn run_hash_pass(cfg: &CliConfig) -> Result<(), String> {
    let LoadedRomInputs {
        rom,
        bios,
        sav,
        rom_name,
    } = load_rom_and_bios(cfg)?;
    let replay = load_replay(cfg)?;
    let filter = cfg
        .filter
        .unwrap_or_else(|| AppConfig::default().display_filter);
    let frames = cfg.frames.max(1);

    let mut core = Box::new(GbaCore::new(bios.as_deref(), rom));
    if let Some(bytes) = &sav {
        core.bus.cart.load_save_data(bytes);
    }

    let mut fe = HashCollectorFrontend::new(&replay, filter);
    let mut core_video_hashes = Vec::with_capacity(frames);
    let mut presented_video_hashes = Vec::with_capacity(frames);
    let mut audio_hashes = Vec::with_capacity(frames);

    for frame_idx in 0..frames {
        fe.reset_frame(frame_idx);
        core.run_frame(&mut fe);
        let (cv, pv, ah) = fe.take_frame_hashes();
        core_video_hashes.push(cv);
        presented_video_hashes.push(pv);
        audio_hashes.push(ah);
    }

    let archive = FrameHashesArchive {
        rom_name: rom_name.clone(),
        filter: filter_cli_slug(filter).to_string(),
        frames,
        overall_core_video_crc32: fe.overall_core_video.finalize(),
        overall_presented_video_crc32: fe.overall_presented_video.finalize(),
        overall_audio_crc32: fe.overall_audio.finalize(),
        core_video_hashes,
        presented_video_hashes,
        audio_hashes,
    };

    if let Some(out_path) = &cfg.dump_hashes {
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create dir {}: {e}", parent.display()))?;
            }
        }
        let body = serde_json::to_string(&archive)
            .map_err(|e| format!("failed to serialize hash archive: {e}"))?;
        fs::write(out_path, body)
            .map_err(|e| format!("failed to write {}: {e}", out_path.display()))?;
        println!(
            "HASHES_DUMPED path={} frames={} core_vid=0x{:08x} pres_vid=0x{:08x} audio=0x{:08x}",
            out_path.display(),
            frames,
            archive.overall_core_video_crc32,
            archive.overall_presented_video_crc32,
            archive.overall_audio_crc32,
        );
    }

    if let Some(ref_path) = &cfg.verify_hashes {
        let raw = fs::read_to_string(ref_path)
            .map_err(|e| format!("failed to read {}: {e}", ref_path.display()))?;
        let expected: FrameHashesArchive = serde_json::from_str(&raw)
            .map_err(|e| format!("failed to parse {}: {e}", ref_path.display()))?;
        if expected.frames != archive.frames {
            return Err(format!(
                "hash frame count mismatch: expected {}, got {}",
                expected.frames, archive.frames
            ));
        }
        for i in 0..frames {
            if expected.core_video_hashes[i] != archive.core_video_hashes[i] {
                return Err(format!(
                    "core video hash mismatch at frame {i}: expected 0x{:08x}, got 0x{:08x}",
                    expected.core_video_hashes[i], archive.core_video_hashes[i]
                ));
            }
            if expected.presented_video_hashes[i] != archive.presented_video_hashes[i] {
                return Err(format!(
                    "presented video hash mismatch at frame {i}: expected 0x{:08x}, got 0x{:08x}",
                    expected.presented_video_hashes[i], archive.presented_video_hashes[i]
                ));
            }
            if expected.audio_hashes[i] != archive.audio_hashes[i] {
                return Err(format!(
                    "audio hash mismatch at frame {i}: expected 0x{:08x}, got 0x{:08x}",
                    expected.audio_hashes[i], archive.audio_hashes[i]
                ));
            }
        }
        println!(
            "HASHES_VERIFIED path={} frames={} core_vid=0x{:08x} pres_vid=0x{:08x} audio=0x{:08x}",
            ref_path.display(),
            frames,
            archive.overall_core_video_crc32,
            archive.overall_presented_video_crc32,
            archive.overall_audio_crc32,
        );
    }

    Ok(())
}

fn build_git_commit() -> String {
    option_env!("RBA_GIT_COMMIT")
        .unwrap_or("unknown")
        .to_string()
}

fn build_time_utc() -> String {
    option_env!("RBA_BUILD_TIME_UTC")
        .unwrap_or("unknown")
        .to_string()
}

/// FIPS 180-4 SHA-256 over an arbitrary byte slice (pure Rust, zero external crates).
pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, delta) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(delta);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn current_exe_sha256() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "unknown".to_string();
    };
    let Ok(bytes) = fs::read(&exe) else {
        return "unknown".to_string();
    };
    let digest = sha256_bytes(&bytes);
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopPerfSummaryJson {
    pub backend: String,
    pub duration_s: f64,
    pub total_presented_frames: usize,
    pub steady_presented_frames: usize,
    pub steady_gba_frames: usize,
    pub repeated_frames_steady: usize,
    pub repeat_pct_steady: f64,
    pub multi_step_frames_steady: usize,
    pub underruns_session_total: u64,
    pub underruns_steady: u64,
    pub wall_interval_summary: LatencySummary,
    pub wall_over_16_74_pct: f64,
    pub wall_over_2x_pct: f64,
    pub gba_interval_summary: LatencySummary,
    pub gba_over_budget_pct: f64,
    pub gba_over_17_5ms_pct: f64,
    pub gba_over_2x_pct: f64,
    pub gba_jitter_2ms_pct: f64,
    pub emu_summary: LatencySummary,
    pub present_summary: LatencySummary,
    pub vsync_hold_histogram: std::collections::BTreeMap<u32, usize>,
    pub final_filter: String,
    pub csv_path: String,
}

/// Real-window per-present frame telemetry logger used by `--perf-log <file>` on both
/// `eframe`/wgpu and `--no-gpu` desktop frontends.
pub struct DesktopPerfLogger {
    csv_path: PathBuf,
    writer: BufWriter<File>,
    backend: String,
    start_wall: Instant,
    last_wall: Option<Instant>,
    last_gba_step_wall: Option<Instant>,
    frame_idx: usize,
    initial_underruns: Option<u64>,
    last_underruns_total: u64,
    underruns_steady: u64,
    steady_presented_frames: usize,
    steady_gba_frames: usize,
    repeated_frames_steady: usize,
    multi_step_frames_steady: usize,
    cur_vsync_hold: u32,
    vsync_hold_hist: std::collections::BTreeMap<u32, usize>,
    wall_intervals_steady: Vec<f64>,
    gba_intervals_steady: Vec<f64>,
    emu_ms_steady: Vec<f64>,
    present_ms_steady: Vec<f64>,
    last_filter: String,
    finished: bool,
}

impl DesktopPerfLogger {
    pub fn create(path: &Path, backend: &str) -> Result<Self, String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create perf log dir {}: {e}", parent.display()))?;
        }
        let file = File::create(path)
            .map_err(|e| format!("failed to create perf log {}: {e}", path.display()))?;
        let mut writer = BufWriter::new(file);
        writeln!(
            writer,
            "frame_idx,wall_interval_ms,emu_ms,present_ms,underruns_total,underruns_delta,filter,frames_stepped,frame_repeated,gba_interval_ms,window_w,window_h,fullscreen,backend"
        )
        .map_err(|e| format!("failed to write perf log header: {e}"))?;

        Ok(Self {
            csv_path: path.to_path_buf(),
            writer,
            backend: backend.to_string(),
            start_wall: Instant::now(),
            last_wall: None,
            last_gba_step_wall: None,
            frame_idx: 0,
            initial_underruns: None,
            last_underruns_total: 0,
            underruns_steady: 0,
            steady_presented_frames: 0,
            steady_gba_frames: 0,
            repeated_frames_steady: 0,
            multi_step_frames_steady: 0,
            cur_vsync_hold: 0,
            vsync_hold_hist: std::collections::BTreeMap::new(),
            wall_intervals_steady: Vec::with_capacity(65_536),
            gba_intervals_steady: Vec::with_capacity(20_480),
            emu_ms_steady: Vec::with_capacity(20_480),
            present_ms_steady: Vec::with_capacity(65_536),
            last_filter: String::from("Unknown"),
            finished: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_present(
        &mut self,
        emu_ms: f64,
        present_ms: f64,
        underruns_total: u64,
        filter: &str,
        frames_stepped: u32,
        window_w: u32,
        window_h: u32,
        fullscreen: bool,
    ) {
        let now = Instant::now();
        let elapsed_s = now.duration_since(self.start_wall).as_secs_f64();
        let steady = elapsed_s >= 2.0 && self.frame_idx >= 10;

        let wall_interval_ms = self
            .last_wall
            .map(|prev| now.duration_since(prev).as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        if self.last_wall.is_some() && steady {
            self.wall_intervals_steady.push(wall_interval_ms);
            self.steady_presented_frames += 1;
            self.present_ms_steady.push(present_ms);
        }
        self.last_wall = Some(now);

        if self.initial_underruns.is_none() {
            self.initial_underruns = Some(underruns_total);
            self.last_underruns_total = underruns_total;
        }
        let underruns_delta = underruns_total.saturating_sub(self.last_underruns_total);
        self.last_underruns_total = underruns_total;
        if steady {
            self.underruns_steady += underruns_delta;
        }

        let frame_repeated = frames_stepped == 0;
        let gba_interval_ms = if frames_stepped > 0 {
            if steady && self.cur_vsync_hold > 0 {
                *self.vsync_hold_hist.entry(self.cur_vsync_hold).or_insert(0) += 1;
            }
            self.cur_vsync_hold = 1;
            let dt = self
                .last_gba_step_wall
                .map(|prev| now.duration_since(prev).as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            if self.last_gba_step_wall.is_some() && steady {
                self.gba_intervals_steady.push(dt);
                self.steady_gba_frames += 1;
                self.emu_ms_steady.push(emu_ms);
                if frames_stepped > 1 {
                    self.multi_step_frames_steady += 1;
                }
            }
            self.last_gba_step_wall = Some(now);
            dt
        } else {
            self.cur_vsync_hold += 1;
            if steady {
                self.repeated_frames_steady += 1;
            }
            0.0
        };

        self.last_filter.clear();
        self.last_filter.push_str(filter);

        let _ = writeln!(
            self.writer,
            "{},{wall_interval_ms:.4},{emu_ms:.4},{present_ms:.4},{underruns_total},{underruns_delta},\"{}\",{frames_stepped},{},{gba_interval_ms:.4},{window_w},{window_h},{},\"{}\"",
            self.frame_idx,
            filter.replace('"', "\"\""),
            if frame_repeated { 1 } else { 0 },
            if fullscreen { 1 } else { 0 },
            self.backend.replace('"', "\"\""),
        );
        self.frame_idx += 1;
        if self.frame_idx.is_multiple_of(120) {
            let _ = self.writer.flush();
        }
    }

    pub fn finish_and_print(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let _ = self.writer.flush();

        let wall_summary = summarize_samples(&self.wall_intervals_steady);
        let wall_n = self.wall_intervals_steady.len().max(1) as f64;
        let wall_over_16_74_pct = (self
            .wall_intervals_steady
            .iter()
            .filter(|&&dt| dt > FRAME_BUDGET_MS)
            .count() as f64
            / wall_n)
            * 100.0;
        let wall_over_2x_pct = (self
            .wall_intervals_steady
            .iter()
            .filter(|&&dt| dt > TWO_X_BUDGET_MS)
            .count() as f64
            / wall_n)
            * 100.0;

        let gba_summary = summarize_samples(&self.gba_intervals_steady);
        let gba_n = self.gba_intervals_steady.len().max(1) as f64;
        let gba_over_budget_pct = (self
            .gba_intervals_steady
            .iter()
            .filter(|&&dt| dt > FRAME_BUDGET_MS)
            .count() as f64
            / gba_n)
            * 100.0;
        let gba_over_17_5ms_pct = (self
            .gba_intervals_steady
            .iter()
            .filter(|&&dt| dt > 17.5)
            .count() as f64
            / gba_n)
            * 100.0;
        let gba_over_2x_pct = (self
            .gba_intervals_steady
            .iter()
            .filter(|&&dt| dt > TWO_X_BUDGET_MS)
            .count() as f64
            / gba_n)
            * 100.0;
        let gba_jitter_2ms_pct = (self
            .gba_intervals_steady
            .iter()
            .filter(|&&dt| (dt - FRAME_BUDGET_MS).abs() > 2.0)
            .count() as f64
            / gba_n)
            * 100.0;

        let emu_summary = summarize_samples(&self.emu_ms_steady);
        let present_summary = summarize_samples(&self.present_ms_steady);
        let underruns_session_total = self
            .last_underruns_total
            .saturating_sub(self.initial_underruns.unwrap_or(self.last_underruns_total));
        let repeat_pct_steady = if self.steady_presented_frames > 0 {
            (self.repeated_frames_steady as f64 / self.steady_presented_frames as f64) * 100.0
        } else {
            0.0
        };
        let duration_s = self.start_wall.elapsed().as_secs_f64();

        let summary = DesktopPerfSummaryJson {
            backend: self.backend.clone(),
            duration_s,
            total_presented_frames: self.frame_idx,
            steady_presented_frames: self.steady_presented_frames,
            steady_gba_frames: self.steady_gba_frames,
            repeated_frames_steady: self.repeated_frames_steady,
            repeat_pct_steady,
            multi_step_frames_steady: self.multi_step_frames_steady,
            underruns_session_total,
            underruns_steady: self.underruns_steady,
            wall_interval_summary: wall_summary.clone(),
            wall_over_16_74_pct,
            wall_over_2x_pct,
            gba_interval_summary: gba_summary.clone(),
            gba_over_budget_pct,
            gba_over_17_5ms_pct,
            gba_over_2x_pct,
            gba_jitter_2ms_pct,
            emu_summary: emu_summary.clone(),
            present_summary: present_summary.clone(),
            vsync_hold_histogram: self.vsync_hold_hist.clone(),
            final_filter: self.last_filter.clone(),
            csv_path: self.csv_path.display().to_string(),
        };

        let summary_path = PathBuf::from(format!("{}.summary.json", self.csv_path.display()));
        if let Ok(body) = serde_json::to_string_pretty(&summary) {
            let _ = fs::write(&summary_path, body);
        }

        let hist_str = self
            .vsync_hold_hist
            .iter()
            .map(|(k, v)| format!("{k}v:{v}"))
            .collect::<Vec<_>>()
            .join(", ");

        println!(
            "=== DESKTOP PERF LOG SUMMARY [{}] ({:.1}s, {} presented, {} steady GBA frames) ===",
            self.backend, duration_s, self.frame_idx, self.steady_gba_frames
        );
        println!(
            "  Wall Interval:   p50={:.3} p95={:.3} p99={:.3} max={:.3} ms | >16.74ms={:.3}% | >2x={:.3}%",
            wall_summary.p50_ms,
            wall_summary.p95_ms,
            wall_summary.p99_ms,
            wall_summary.worst_ms,
            wall_over_16_74_pct,
            wall_over_2x_pct
        );
        println!(
            "  GBA Cadence:     p50={:.3} p95={:.3} p99={:.3} max={:.3} ms | >16.74ms={:.3}% | >17.5ms={:.3}% | >2x={:.3}% | jitter>2ms={:.3}%",
            gba_summary.p50_ms,
            gba_summary.p95_ms,
            gba_summary.p99_ms,
            gba_summary.worst_ms,
            gba_over_budget_pct,
            gba_over_17_5ms_pct,
            gba_over_2x_pct,
            gba_jitter_2ms_pct
        );
        println!(
            "  Repeats & Audio: repeats={}/{} ({:.2}%) | multi_step={} | vsync_holds=[{}] | underruns={} (steady={})",
            self.repeated_frames_steady,
            self.steady_presented_frames,
            repeat_pct_steady,
            self.multi_step_frames_steady,
            hist_str,
            underruns_session_total,
            self.underruns_steady
        );
        println!(
            "  Work Breakdown:  emu avg={:.3} p95={:.3} ms | present avg={:.3} p95={:.3} ms | filter={}",
            emu_summary.avg_ms,
            emu_summary.p95_ms,
            present_summary.avg_ms,
            present_summary.p95_ms,
            self.last_filter
        );
        println!("  CSV / JSON:      {} / {}", self.csv_path.display(), summary_path.display());
    }
}

impl Drop for DesktopPerfLogger {
    fn drop(&mut self) {
        self.finish_and_print();
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SoftbufferSizeReportJson {
    pub schema_version: u32,
    pub timestamp_utc: String,
    pub exe_sha256: String,
    pub git_commit: String,
    pub build_time_utc: String,
    pub frames_per_condition: usize,
    pub budget_ms: f64,
    pub rows: Vec<SoftbufferSizeBenchRow>,
}

/// Phase 8d: Time real `winit + softbuffer` across `720x480`, `1280x720`, `1920x1080`, and
/// monitor native fullscreen with `none` and `myboy-smooth` at `1x`, `--slowdown 8`, and `--slowdown 15`.
pub fn run_bench_softbuffer(cfg: &CliConfig) -> Result<(), String> {
    let frames = if cfg.frames_explicit {
        cfg.frames.max(30)
    } else {
        120
    };
    let ts = format_utc_timestamp();
    fs::create_dir_all(&cfg.perf_dir)
        .map_err(|e| format!("failed to create perf dir {}: {e}", cfg.perf_dir.display()))?;

    let rows = bench_softbuffer_sizes(frames, true)?;
    println!("=== NO-GPU SOFTBUFFER REAL-WINDOW SIZE SWEEP ({frames} frames/condition, budget {FRAME_BUDGET_MS:.2} ms) ===");
    for r in &rows {
        let verdict = if r.fits_budget { "FIT" } else { "EXCEEDS" };
        println!(
            "  [{:<11} {:>4}x{:<4}] {:<12} {:>2}x | present={:.3} p95={:.3} ms | total avg={:.3} p95={:.3} p99={:.3} worst={:.3} ms | over={:.2}% [{verdict}]",
            r.resolution_label,
            r.width,
            r.height,
            r.filter,
            r.slowdown_x,
            r.present_only_avg_ms,
            r.present_only_p95_ms,
            r.total_work_avg_ms,
            r.total_work_p95_ms,
            r.total_work_p99_ms,
            r.total_work_worst_ms,
            r.over_budget_pct,
        );
    }

    let report = SoftbufferSizeReportJson {
        schema_version: 1,
        timestamp_utc: ts.clone(),
        exe_sha256: current_exe_sha256(),
        git_commit: build_git_commit(),
        build_time_utc: build_time_utc(),
        frames_per_condition: frames,
        budget_ms: FRAME_BUDGET_MS,
        rows,
    };
    let json_path = cfg.perf_dir.join(format!("softbuffer-{ts}.json"));
    let body = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("failed to serialize softbuffer JSON: {e}"))?;
    fs::write(&json_path, body)
        .map_err(|e| format!("failed to write {}: {e}", json_path.display()))?;
    println!("  JSON: {}", json_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_parses_scenes_and_buttons() {
        let script = "\
# comment
scene boot 0..100
scene title 100..200
frame 5 START
range 10..15 A+UP
pulse 100..120 10 2 A
";
        let replay = InputReplay::parse(script).expect("valid script");
        assert_eq!(replay.scene_at(0), "boot");
        assert_eq!(replay.scene_at(100), "title");
        assert_eq!(replay.scene_at(250), "default");
        assert_eq!(replay.pressed_mask_at(4), 0);
        assert_eq!(replay.pressed_mask_at(5), KEY_START);
        assert_eq!(replay.pressed_mask_at(12), KEY_A | KEY_UP);
        assert_eq!(replay.pressed_mask_at(100), KEY_A);
        assert_eq!(replay.pressed_mask_at(101), KEY_A);
        assert_eq!(replay.pressed_mask_at(102), 0);
        assert_eq!(replay.pressed_mask_at(110), KEY_A);
    }

    #[test]
    fn summarize_samples_computes_percentiles() {
        let samples: Vec<f64> = (1..=100).map(|v| v as f64).collect();
        let s = summarize_samples(&samples);
        assert_eq!(s.frames, 100);
        assert!((s.min_ms - 1.0).abs() < 1e-9);
        assert!((s.p50_ms - 50.0).abs() < 1e-9);
        assert!((s.p95_ms - 95.0).abs() < 1e-9);
        assert!((s.p99_ms - 99.0).abs() < 1e-9);
        assert!((s.worst_ms - 100.0).abs() < 1e-9);
    }

    #[test]
    fn sha256_matches_nist_vectors() {
        let to_hex = |d: [u8; 32]| -> String {
            d.iter().map(|b| format!("{b:02x}")).collect()
        };
        assert_eq!(
            to_hex(sha256_bytes(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            to_hex(sha256_bytes(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
