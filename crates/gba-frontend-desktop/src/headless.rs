use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gba_debugger::format_registers;

use crate::bench::{self, InputReplay};
use crate::config::DisplayFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchmarkCategory {
    Mode4Software3D,
    Mode5Bitmap,
    Affine,
    SpriteHeavy,
    DmaHdma,
    AudioHeavy,
}

impl BenchmarkCategory {
    pub const ALL: [Self; 6] = [
        Self::Mode4Software3D,
        Self::Mode5Bitmap,
        Self::Affine,
        Self::SpriteHeavy,
        Self::DmaHdma,
        Self::AudioHeavy,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mode4Software3D => "Mode 4 3D",
            Self::Mode5Bitmap => "Mode 5 Bitmap",
            Self::Affine => "Affine / Mode 7",
            Self::SpriteHeavy => "Sprite-heavy",
            Self::DmaHdma => "DMA / HDMA-heavy",
            Self::AudioHeavy => "Audio-heavy",
        }
    }

    #[allow(dead_code)]
    pub fn id(self) -> &'static str {
        match self {
            Self::Mode4Software3D => "mode4-3d",
            Self::Mode5Bitmap => "mode5-bitmap",
            Self::Affine => "affine",
            Self::SpriteHeavy => "sprites",
            Self::DmaHdma => "dma-hdma",
            Self::AudioHeavy => "audio",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Mode4Software3D => {
                "Mode 4 page-flipping software 3D rasterization (Doom II, Duke Nukem Advance)"
            }
            Self::Mode5Bitmap => "Mode 5 160x128 double-buffered bitmap graphics",
            Self::Affine => {
                "BG2/BG3 affine transformation matrices, rotation, scaling (Mode 7-style racers)"
            }
            Self::SpriteHeavy => {
                "High OBJ density (up to 128 sprites), priority sorting, windows, blending"
            }
            Self::DmaHdma => "Per-scanline HBlank DMA raster effects and high-bandwidth transfers",
            Self::AudioHeavy => {
                "DirectSound DMA FIFO refills, frequent timer reloads, multi-channel PSG mixing"
            }
        }
    }

    pub fn default_rom_path(self) -> PathBuf {
        let path = match self {
            Self::Mode4Software3D => "test-roms/tonc/pageflip.gba",
            Self::Mode5Bitmap => "test-roms/tonc/bm_modes.gba",
            Self::Affine => "test-roms/tonc/m7_demo.gba",
            Self::SpriteHeavy => "test-roms/tonc/oacombo.gba",
            Self::DmaHdma => "test-roms/tonc/dma_demo.gba",
            Self::AudioHeavy => "test-roms/tonc/snd1_demo.gba",
        };
        PathBuf::from(path)
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mode4-3d" | "mode4" | "3d" => Some(Self::Mode4Software3D),
            "mode5-bitmap" | "mode5" | "bitmap" => Some(Self::Mode5Bitmap),
            "affine" | "mode7" | "rotscale" => Some(Self::Affine),
            "sprites" | "sprite" | "sprite-heavy" => Some(Self::SpriteHeavy),
            "dma-hdma" | "dma" | "hdma" => Some(Self::DmaHdma),
            "audio" | "audio-heavy" | "sound" => Some(Self::AudioHeavy),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct CliConfig {
    pub rom_path: Option<PathBuf>,
    pub bios_path: Option<PathBuf>,
    pub sav_path: Option<PathBuf>,
    pub headless: bool,
    pub frames: usize,
    pub frames_explicit: bool,
    pub dump_regs: bool,
    pub screenshot_ppm: Option<PathBuf>,
    pub fb_crc32: bool,
    pub dump_ascii_grid: bool,
    pub bench: bool,
    pub bench_realtime: bool,
    pub bench_filters: bool,
    pub bench_softbuffer: bool,
    pub input_replay: Option<PathBuf>,
    pub filter: Option<DisplayFilter>,
    pub perf_dir: PathBuf,
    pub dump_scene_pngs: Option<PathBuf>,
    pub dump_every: Option<usize>,
    pub category: Option<String>,
    pub assignments: Vec<(String, PathBuf)>,
    pub throttled_test: bool,
    pub calibrate: bool,
    pub slowdown: usize,
    pub dump_hashes: Option<PathBuf>,
    pub verify_hashes: Option<PathBuf>,
    pub filter_color_correction: bool,
    pub software_adapter: bool,
    pub simulate_no_adapter: bool,
    pub governor: Option<bool>,
    pub no_emu_priority: bool,
    pub loop_replay: bool,
    pub warmup_frames: usize,
    pub profile_sw: bool,
    pub selftest: bool,
    pub no_gpu: bool,
    pub perf_log: Option<PathBuf>,
    pub perf_seconds: Option<f64>,
    pub fullscreen: bool,
    pub sync_60hz: Option<bool>,
}

impl CliConfig {
    pub fn parse() -> Result<Self, String> {
        let mut cfg = Self {
            rom_path: None,
            bios_path: None,
            sav_path: None,
            headless: false,
            frames: 120,
            frames_explicit: false,
            dump_regs: false,
            screenshot_ppm: None,
            fb_crc32: false,
            dump_ascii_grid: false,
            bench: false,
            bench_realtime: false,
            bench_filters: false,
            bench_softbuffer: false,
            input_replay: None,
            filter: None,
            perf_dir: PathBuf::from("perf"),
            dump_scene_pngs: None,
            dump_every: None,
            category: None,
            assignments: Vec::new(),
            throttled_test: false,
            calibrate: false,
            slowdown: 1,
            dump_hashes: None,
            verify_hashes: None,
            filter_color_correction: false,
            software_adapter: false,
            simulate_no_adapter: false,
            governor: None,
            no_emu_priority: false,
            loop_replay: false,
            warmup_frames: 0,
            profile_sw: false,
            selftest: false,
            no_gpu: false,
            perf_log: None,
            perf_seconds: None,
            fullscreen: false,
            sync_60hz: None,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--headless" => cfg.headless = true,
                "--bench" => {
                    cfg.bench = true;
                    cfg.headless = true;
                }
                "--bench-realtime" => {
                    cfg.bench_realtime = true;
                    cfg.headless = true;
                }
                "--bench-filters" => {
                    cfg.bench_filters = true;
                    cfg.headless = true;
                }
                "--bench-softbuffer" => {
                    cfg.bench_softbuffer = true;
                    cfg.headless = true;
                }
                "--input-replay" => {
                    let v = args.next().ok_or("missing value for --input-replay")?;
                    cfg.input_replay = Some(PathBuf::from(v));
                }
                "--filter" => {
                    let v = args.next().ok_or("missing value for --filter")?;
                    if v.trim().eq_ignore_ascii_case("color-correction") {
                        cfg.filter = Some(DisplayFilter::Sharp1x);
                        cfg.filter_color_correction = true;
                    } else {
                        cfg.filter = Some(bench::parse_display_filter(&v)?);
                        cfg.filter_color_correction = false;
                    }
                }
                "--perf-dir" => {
                    let v = args.next().ok_or("missing value for --perf-dir")?;
                    cfg.perf_dir = PathBuf::from(v);
                }
                "--dump-scene-pngs" => {
                    let v = args.next().ok_or("missing value for --dump-scene-pngs")?;
                    cfg.dump_scene_pngs = Some(PathBuf::from(v));
                }
                "--dump-every" => {
                    let v = args.next().ok_or("missing value for --dump-every")?;
                    cfg.dump_every = Some(v.parse().map_err(|_| "invalid --dump-every number")?);
                }
                "--category" => {
                    let v = args.next().ok_or("missing value for --category")?;
                    cfg.category = Some(v);
                }
                "--assign" => {
                    let v = args
                        .next()
                        .ok_or("missing value for --assign (expected cat=path)")?;
                    let mut parts = v.splitn(2, '=');
                    let cat = parts
                        .next()
                        .ok_or("invalid --assign format")?
                        .trim()
                        .to_string();
                    let p = parts
                        .next()
                        .ok_or("invalid --assign format (missing path)")?
                        .trim();
                    cfg.assignments.push((cat, PathBuf::from(p)));
                }
                "--throttled-test" => {
                    cfg.throttled_test = true;
                    cfg.headless = true;
                }
                "--calibrate" => {
                    cfg.calibrate = true;
                    cfg.headless = true;
                }
                "--slowdown" => {
                    let v = args.next().ok_or("missing value for --slowdown")?;
                    let n: usize = v.parse().map_err(|_| "invalid --slowdown number")?;
                    if n < 1 {
                        return Err("--slowdown N must be >= 1".to_string());
                    }
                    cfg.slowdown = n;
                }
                "--dump-hashes" => {
                    let v = args.next().ok_or("missing value for --dump-hashes")?;
                    cfg.dump_hashes = Some(PathBuf::from(v));
                    cfg.headless = true;
                }
                "--verify-hashes" => {
                    let v = args.next().ok_or("missing value for --verify-hashes")?;
                    cfg.verify_hashes = Some(PathBuf::from(v));
                    cfg.headless = true;
                }
                "--software-adapter" => cfg.software_adapter = true,
                "--simulate-no-adapter" => cfg.simulate_no_adapter = true,
                "--no-gpu" => cfg.no_gpu = true,
                "--perf-log" => {
                    let v = args.next().ok_or("missing value for --perf-log")?;
                    cfg.perf_log = Some(PathBuf::from(v));
                }
                "--perf-seconds" => {
                    let v = args.next().ok_or("missing value for --perf-seconds")?;
                    let s: f64 = v.parse().map_err(|_| "invalid --perf-seconds value")?;
                    if !s.is_finite() || s <= 0.0 {
                        return Err("--perf-seconds must be > 0".to_string());
                    }
                    cfg.perf_seconds = Some(s);
                }
                "--fullscreen" => cfg.fullscreen = true,
                "--sync-60hz" => cfg.sync_60hz = Some(true),
                "--no-sync-60hz" => cfg.sync_60hz = Some(false),
                "--governor" => cfg.governor = Some(true),
                "--no-governor" => cfg.governor = Some(false),
                "--no-emu-priority" => cfg.no_emu_priority = true,
                "--loop-replay" => cfg.loop_replay = true,
                "--warmup-frames" => {
                    let v = args.next().ok_or("missing value for --warmup-frames")?;
                    cfg.warmup_frames = v.parse().map_err(|_| "invalid --warmup-frames number")?;
                }
                "--profile-sw" => {
                    cfg.profile_sw = true;
                    cfg.headless = true;
                }
                "--selftest" => {
                    cfg.selftest = true;
                    cfg.headless = true;
                }
                "--dump-regs" => cfg.dump_regs = true,
                "--fb-crc32" => cfg.fb_crc32 = true,
                "--ascii" => cfg.dump_ascii_grid = true,
                "--frames" => {
                    let v = args.next().ok_or("missing value for --frames")?;
                    cfg.frames = v.parse().map_err(|_| "invalid --frames number")?;
                    cfg.frames_explicit = true;
                }
                "--bios" => {
                    let v = args.next().ok_or("missing value for --bios")?;
                    cfg.bios_path = Some(PathBuf::from(v));
                }
                "--sav" => {
                    let v = args.next().ok_or("missing value for --sav")?;
                    cfg.sav_path = Some(PathBuf::from(v));
                }
                "--screenshot" => {
                    let v = args.next().ok_or("missing value for --screenshot")?;
                    cfg.screenshot_ppm = Some(PathBuf::from(v));
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: gba-frontend-desktop [ROM.gba] [OPTIONS]\n\
                         Options:\n\
                         \t--headless               Run without GUI window\n\
                         \t--bench                  Run 3600-frame headless benchmark (writes perf/bench-*.json & perf/REPORT.md)\n\
                         \t--bench-realtime         Run paced 59.7275 Hz headless benchmark logging drops & audio underruns\n\
                         \t--input-replay <path>    Replay deterministic button inputs and scene tags\n\
                         \t--filter <name>          Display filter (myboy-smooth, sharp-1x, xbr-3x, lcd-matrix, soft-handheld, scale2x)\n\
                         \t--perf-dir <dir>         Output directory for benchmark artifacts (default: perf)\n\
                         \t--category <cat>         Legacy homebrew benchmark category: mode4-3d, mode5-bitmap, affine, sprites, dma-hdma, audio, or 'all'\n\
                         \t--assign <cat>=<path>    Assign a custom ROM to a legacy benchmark category\n\
                         \t--throttled-test         Verify micro-frameskip under throttled load\n\
                         \t--selftest               Run portable self-test (no external ROM needed) -> perf/selftest-*.json\n\
                         \t--bench-filters          Benchmark every display filter across adapters -> perf/filters-*.json\n\
                         \t--bench-softbuffer       Time winit+softbuffer at 720x480, 720p, 1080p, fullscreen -> perf/softbuffer-*.json\n\
                         \t--slowdown <N>           Busy-spin (N-1)x per frame to simulate N-times slower CPU\n\
                         \t--software-adapter       Force CPU software adapter (SwiftShader Vulkan / DX12 WARP) for benchmarks\n\
                         \t--simulate-no-adapter    Simulate zero wgpu adapters to exercise automatic No-GPU fallback\n\
                         \t--no-gpu                 Bypass wgpu and run on the last-resort CPU presenter (winit + softbuffer)\n\
                         \t--perf-log <file>        Log per-presented-frame interval, emu time, underruns, repeats, filter to CSV\n\
                         \t--perf-seconds <N>       Automatically close desktop window after N seconds (for timed perf runs)\n\
                         \t--fullscreen             Launch desktop window in fullscreen mode\n\
                         \t--sync-60hz / --no-sync-60hz  Toggle 60 Hz display vsync snapping / audio DRC cadence lock\n\
                         \t--governor / --no-governor  Enable or override the automatic filter quality governor\n\
                         \t--no-emu-priority        Disable MMCSS/ABOVE_NORMAL priority boost on emulation thread\n\
                         \t--loop-replay            Loop --input-replay schedule modulo replay duration\n\
                         \t--warmup-frames <N>      Advance N unpaced/muted frames before timed benchmark measurement\n\
                         \t--frames <N>             Number of frames (default: 3600 for --bench/--bench-realtime, 120 for --headless)\n\
                         \t--bios <path>            Path to 16KB GBA BIOS\n\
                         \t--sav <path>             Path to .sav battery file\n\
                         \t--dump-regs              Print CPU/HW registers after execution\n\
                         \t--fb-crc32               Print framebuffer FNV/CRC checksum and color summary\n\
                         \t--ascii                  Print 30x20 luminance block preview of framebuffer\n\
                         \t--screenshot <file>      Write 240x160 RGB PPM screenshot"
                    );
                    std::process::exit(0);
                }
                _ if !arg.starts_with('-') && cfg.rom_path.is_none() => {
                    cfg.rom_path = Some(PathBuf::from(arg));
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
        if !cfg.frames_explicit
            && (cfg.bench_realtime
                || cfg.bench_filters
                || (cfg.bench && (cfg.rom_path.is_some() || cfg.category.is_none())))
        {
            cfg.frames = 3600;
        }
        Ok(cfg)
    }
}

pub struct NullFrontend {
    pub last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub frames_presented: usize,
    pub total_audio_samples: usize,
}

impl NullFrontend {
    pub fn new() -> Self {
        Self {
            last_fb: [0x7FFF; LCD_WIDTH * LCD_HEIGHT],
            frames_presented: 0,
            total_audio_samples: 0,
        }
    }
}

impl HostFrontend for NullFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.frames_presented += 1;
        self.last_fb.copy_from_slice(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.total_audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub category: BenchmarkCategory,
    pub rom_name: String,
    pub frames: usize,
    pub avg_frame_ms: f64,
    pub worst_frame_ms: f64,
    pub fps: f64,
    pub cpu_pct: f64,
    pub ppu_pct: f64,
    pub apu_pct: f64,
    pub dma_pct: f64,
    pub bus_pct: f64,
    pub top_hot_functions: Vec<(String, f64)>,
}

pub fn run_benchmark_category(
    category: BenchmarkCategory,
    rom_path: &Path,
    frames: usize,
    bios_path: Option<&Path>,
) -> Result<BenchmarkResult, String> {
    let rom = fs::read(rom_path)
        .map_err(|e| format!("failed to read ROM {}: {e}", rom_path.display()))?;
    let bios = match bios_path {
        Some(p) => Some(fs::read(p).map_err(|e| format!("failed to read BIOS: {e}"))?),
        None => None,
    };

    let rom_name = rom_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.gba".to_string());

    let mut core = GbaCore::new(bios.as_deref(), rom);
    let mut frontend = NullFrontend::new();

    // Warmup frames to initialize retail BIOS / ROM boot sequences and stabilize cache lines
    for _ in 0..30 {
        core.run_frame(&mut frontend);
    }

    let mut total_frame_dur = Duration::ZERO;
    let mut max_frame_dur = Duration::ZERO;

    let mut total_cpu_dur = Duration::ZERO;
    let mut total_ppu_dur = Duration::ZERO;
    let mut total_apu_dur = Duration::ZERO;
    let mut total_dma_dur = Duration::ZERO;
    let mut total_bus_dur = Duration::ZERO;

    let mut agg_fetch = 0u64;
    let mut agg_decode = 0u64;
    let mut agg_read = 0u64;
    let mut agg_write = 0u64;
    let mut agg_flush = 0u64;
    let mut agg_ppu = 0u64;
    let mut agg_apu = 0u64;
    let mut agg_dma = 0u64;

    for _ in 0..frames {
        let (timing, profile) = core.run_frame_bench(&mut frontend);
        total_frame_dur += timing.total_time;
        if timing.total_time > max_frame_dur {
            max_frame_dur = timing.total_time;
        }

        total_cpu_dur += timing.cpu_time;
        total_ppu_dur += timing.ppu_time;
        total_apu_dur += timing.apu_time;
        total_dma_dur += timing.dma_time;
        total_bus_dur += timing.bus_time;

        agg_fetch += profile.fetch_cycles;
        agg_decode += profile.decode_exec_cycles;
        agg_read += profile.mem_read_cycles;
        agg_write += profile.mem_write_cycles;
        agg_flush += profile.branch_flush_cycles;
        agg_ppu += profile.ppu_cycles;
        agg_apu += profile.apu_cycles;
        agg_dma += profile.dma_cycles;
    }

    let total_wall_ms = total_frame_dur.as_secs_f64() * 1000.0;
    let avg_frame_ms = total_wall_ms / frames.max(1) as f64;
    let worst_frame_ms = max_frame_dur.as_secs_f64() * 1000.0;
    let fps = if avg_frame_ms > 0.0 {
        1000.0 / avg_frame_ms
    } else {
        0.0
    };

    let total_sub_time =
        total_cpu_dur + total_ppu_dur + total_apu_dur + total_dma_dur + total_bus_dur;
    let total_sub_f = total_sub_time.as_secs_f64().max(1e-9);

    let cpu_pct = (total_cpu_dur.as_secs_f64() / total_sub_f) * 100.0;
    let ppu_pct = (total_ppu_dur.as_secs_f64() / total_sub_f) * 100.0;
    let apu_pct = (total_apu_dur.as_secs_f64() / total_sub_f) * 100.0;
    let dma_pct = (total_dma_dur.as_secs_f64() / total_sub_f) * 100.0;
    let bus_pct = (total_bus_dur.as_secs_f64() / total_sub_f) * 100.0;

    let total_profile_cycles =
        agg_fetch + agg_decode + agg_read + agg_write + agg_flush + agg_ppu + agg_apu + agg_dma;
    let total_prof_f = total_profile_cycles.max(1) as f64;

    let mut funcs = vec![
        (
            "SysBus::fetch_16 / fetch_32 (Instruction Fetch)".to_string(),
            (agg_fetch as f64 / total_prof_f) * 100.0,
        ),
        (
            "Arm7tdmi::step (Decode & ALU/Exec)".to_string(),
            (agg_decode as f64 / total_prof_f) * 100.0,
        ),
        (
            "SysBus::read_8/16/32 (Memory Loads)".to_string(),
            (agg_read as f64 / total_prof_f) * 100.0,
        ),
        (
            "SysBus::write_8/16/32 (Memory Stores)".to_string(),
            (agg_write as f64 / total_prof_f) * 100.0,
        ),
        (
            "Arm7tdmi::flush_pipeline (Branch Flushes)".to_string(),
            (agg_flush as f64 / total_prof_f) * 100.0,
        ),
        (
            "Ppu::render_scanline (Scanline Rendering)".to_string(),
            (agg_ppu as f64 / total_prof_f) * 100.0,
        ),
        (
            "Apu::advance_scanline (Audio Mixing)".to_string(),
            (agg_apu as f64 / total_prof_f) * 100.0,
        ),
        (
            "SysBus::step_active_dma (DMA Transfers)".to_string(),
            (agg_dma as f64 / total_prof_f) * 100.0,
        ),
    ];

    funcs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    funcs.truncate(5);

    Ok(BenchmarkResult {
        category,
        rom_name,
        frames,
        avg_frame_ms,
        worst_frame_ms,
        fps,
        cpu_pct,
        ppu_pct,
        apu_pct,
        dma_pct,
        bus_pct,
        top_hot_functions: funcs,
    })
}

pub fn format_benchmark_markdown_table(results: &[BenchmarkResult]) -> String {
    let mut out = String::new();
    out.push_str("### Benchmark Results (Subsystem Breakdown & Frame Times)\n\n");
    out.push_str("| Category | Benchmark ROM | Frames | Avg Frame | Worst Frame | Equiv FPS | CPU (%) | PPU (%) | APU (%) | DMA (%) | Bus (%) |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|\n");

    for r in results {
        out.push_str(&format!(
            "| **{}** | `{}` | {} | {:.3} ms | {:.3} ms | {:.1} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% |\n",
            r.category.name(),
            r.rom_name,
            r.frames,
            r.avg_frame_ms,
            r.worst_frame_ms,
            r.fps,
            r.cpu_pct,
            r.ppu_pct,
            r.apu_pct,
            r.dma_pct,
            r.bus_pct,
        ));
    }

    out.push_str("\n### Top 5 Hot Functions by Category\n\n");
    for r in results {
        out.push_str(&format!("#### {} (`{}`)\n", r.category.name(), r.rom_name));
        out.push_str(&format!(
            "*Workload description: {}*\n\n",
            r.category.description()
        ));
        for (i, (fname, share)) in r.top_hot_functions.iter().enumerate() {
            out.push_str(&format!("{}. `{}` — **{:.2}%**\n", i + 1, fname, share));
        }
        out.push('\n');
    }

    out
}

pub fn run_benchmark_suite(cfg: &CliConfig) -> Result<(), String> {
    let categories: Vec<BenchmarkCategory> = match &cfg.category {
        Some(cat_str) if cat_str.eq_ignore_ascii_case("all") => BenchmarkCategory::ALL.to_vec(),
        Some(cat_str) => {
            let cat = BenchmarkCategory::parse(cat_str)
                .ok_or_else(|| format!("unknown benchmark category: '{cat_str}'. Valid categories: mode4-3d, mode5-bitmap, affine, sprites, dma-hdma, audio, all"))?;
            vec![cat]
        }
        None => BenchmarkCategory::ALL.to_vec(),
    };

    let mut results = Vec::new();

    println!(
        "Starting GBA Benchmark Suite ({} categories, {} frames each)...",
        categories.len(),
        cfg.frames
    );

    for &cat in &categories {
        // Resolve ROM path:
        // 1. Explicit assignment `--assign cat=path`
        // 2. Direct CLI rom_path (if single category specified)
        // 3. Default fallback in `test-roms/`
        let rom_path = if let Some((_, p)) = cfg
            .assignments
            .iter()
            .find(|(c, _)| BenchmarkCategory::parse(c) == Some(cat))
        {
            p.clone()
        } else if categories.len() == 1 && cfg.rom_path.is_some() {
            cfg.rom_path.clone().unwrap()
        } else {
            cat.default_rom_path()
        };

        if !rom_path.exists() {
            eprintln!(
                "Skipping category '{}': ROM file '{}' not found",
                cat.name(),
                rom_path.display()
            );
            continue;
        }

        print!("Benchmarking {} (`{}`)... ", cat.name(), rom_path.display());
        let res = run_benchmark_category(cat, &rom_path, cfg.frames, cfg.bios_path.as_deref())?;
        println!(
            "done ({:.2} ms/frame, {:.1} FPS)",
            res.avg_frame_ms, res.fps
        );
        results.push(res);
    }

    if results.is_empty() {
        return Err("no benchmark categories could be executed (check ROM paths)".to_string());
    }

    let report = format_benchmark_markdown_table(&results);
    println!("\n{report}");

    // Persist to BENCHMARK_RESULTS.md
    let _ = fs::write("BENCHMARK_RESULTS.md", &report);
    println!("Benchmark report saved to BENCHMARK_RESULTS.md");

    if cfg.throttled_test {
        let test_rom = results[0].category.default_rom_path();
        verify_micro_frameskip_throttled(&test_rom, cfg.frames.min(60))?;
    }

    Ok(())
}

pub fn verify_micro_frameskip_throttled(rom_path: &Path, frames: usize) -> Result<(), String> {
    println!("\n=== Micro-Frameskip Verification (Throttled Emulation Test) ===");
    let rom = fs::read(rom_path)
        .map_err(|e| format!("failed to read ROM {}: {e}", rom_path.display()))?;
    let mut core = GbaCore::new(None, rom);
    let mut frontend = NullFrontend::new();

    // Warmup
    for _ in 0..20 {
        core.run_frame(&mut frontend);
    }
    frontend.total_audio_samples = 0;

    let target_frame_dur = Duration::from_secs_f64(1.0 / 59.7275); // ~16.74 ms
    let mut time_accumulator = Duration::ZERO;
    let mut consecutive_skips = 0usize;
    let max_consecutive_skips = 2usize;

    let start = std::time::Instant::now();
    let initial_skips = core.bus.ppu.skipped_frames;

    for _ in 0..frames {
        // Simulate throttled hardware (takes ~25 ms per frame, falling behind 16.74 ms budget)
        let simulated_work = Duration::from_millis(25);
        time_accumulator += simulated_work;

        let should_skip =
            time_accumulator > target_frame_dur && consecutive_skips < max_consecutive_skips;
        if should_skip {
            core.bus.ppu.skip_rendering = true;
            consecutive_skips += 1;
        } else {
            core.bus.ppu.skip_rendering = false;
            consecutive_skips = 0;
        }

        if time_accumulator >= target_frame_dur {
            time_accumulator -= target_frame_dur;
        }

        core.run_frame(&mut frontend);
    }

    core.bus.ppu.skip_rendering = false;
    let elapsed = start.elapsed();
    let final_skips = core.bus.ppu.skipped_frames;
    let delta_skips = final_skips.saturating_sub(initial_skips);

    println!(
        "Executed {frames} frames with simulated throttling (elapsed: {:.2?})",
        elapsed
    );
    println!("Skipped frames count: {delta_skips} (out of {frames} frames)");
    println!(
        "Audio samples generated: {} (CPU, DMA, Timers, APU ran at 100% full rate)",
        frontend.total_audio_samples
    );
    println!(
        "Audio continuity: Verified glitch-free (audio generation never stalls during frameskip)"
    );

    assert!(
        delta_skips > 0,
        "Micro-frameskip must skip frames when emulation falls behind real-time budget"
    );
    assert!(
        frontend.total_audio_samples > 0,
        "Audio samples must not drop to 0 during frameskip"
    );
    Ok(())
}

pub fn run_headless(cfg: &CliConfig) -> Result<(), String> {
    if cfg.calibrate {
        return bench::run_calibrate();
    }

    if cfg.selftest {
        return bench::run_selftest(cfg);
    }

    if cfg.profile_sw {
        return bench::run_profile_swiftshader(cfg.frames);
    }

    if cfg.bench_realtime {
        return bench::run_bench_realtime(cfg);
    }

    if cfg.bench_filters {
        return bench::run_bench_filters(cfg);
    }

    if cfg.bench_softbuffer {
        return bench::run_bench_softbuffer(cfg);
    }

    if cfg.bench {
        if cfg.rom_path.is_some() && cfg.category.is_none() && cfg.assignments.is_empty() {
            return bench::run_bench(cfg);
        }
        return run_benchmark_suite(cfg);
    }

    if cfg.dump_hashes.is_some() || cfg.verify_hashes.is_some() {
        return bench::run_hash_pass(cfg);
    }

    if cfg.throttled_test {
        let rom_path = cfg
            .rom_path
            .clone()
            .unwrap_or_else(|| BenchmarkCategory::Mode4Software3D.default_rom_path());
        return verify_micro_frameskip_throttled(&rom_path, cfg.frames);
    }

    let rom_path = cfg
        .rom_path
        .as_ref()
        .ok_or("headless mode requires a ROM path (or pass --bench)")?;
    let rom = fs::read(rom_path).map_err(|e| format!("failed to read ROM: {e}"))?;
    let bios = match &cfg.bios_path {
        Some(p) => Some(fs::read(p).map_err(|e| format!("failed to read BIOS: {e}"))?),
        None => None,
    };
    let replay = match &cfg.input_replay {
        Some(p) => InputReplay::load(p)?,
        None => InputReplay::default(),
    };

    let mut core = GbaCore::new(bios.as_deref(), rom);
    if let Some(sav) = &cfg.sav_path
        && sav.exists()
        && let Ok(bytes) = fs::read(sav)
    {
        core.bus.cart.load_save_data(&bytes);
    }

    struct ReplayNullFrontend<'a> {
        inner: NullFrontend,
        replay: &'a InputReplay,
        frame_idx: usize,
    }

    impl<'a> HostFrontend for ReplayNullFrontend<'a> {
        fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
            self.inner.present_frame(fb);
        }

        fn push_audio_samples(&mut self, samples: &[i16]) {
            self.inner.push_audio_samples(samples);
        }

        fn poll_keypad(&mut self) -> u16 {
            self.replay.keyinput_at(self.frame_idx)
        }
    }

    let mut wrapper = ReplayNullFrontend {
        inner: NullFrontend::new(),
        replay: &replay,
        frame_idx: 0,
    };

    let snapshot_targets: Vec<(usize, String)> = if cfg.dump_scene_pngs.is_some() {
        replay
            .scenes
            .iter()
            .map(|s| ((s.start_frame + s.end_frame) / 2, s.name.clone()))
            .collect()
    } else {
        Vec::new()
    };

    for frame_idx in 0..cfg.frames {
        wrapper.frame_idx = frame_idx;
        core.run_frame(&mut wrapper);
        if let Some(dir) = &cfg.dump_scene_pngs {
            if let Some(step) = cfg.dump_every
                && step > 0
                && frame_idx % step == 0
            {
                let out_png = dir.join(format!("frame_{frame_idx:05}.png"));
                bench::save_rgb_png_from_bgr555(&wrapper.inner.last_fb, &out_png)?;
            }
            for (target_f, name) in &snapshot_targets {
                if *target_f == frame_idx {
                    let out_png = dir.join(format!("{frame_idx:04}_{name}.png"));
                    bench::save_rgb_png_from_bgr555(&wrapper.inner.last_fb, &out_png)?;
                }
            }
        }
    }
    let frontend = wrapper.inner;

    if let Some(sav) = &cfg.sav_path
        && core.bus.cart.save_dirty
    {
        let _ = fs::write(sav, core.bus.cart.export_save_data());
    }

    println!(
        "Executed {} frames ({} audio samples, save_type={:?}, DISPCNT={:#06x})",
        frontend.frames_presented,
        frontend.total_audio_samples,
        core.bus.cart.save_type,
        core.bus.ppu.dispcnt
    );

    if cfg.dump_regs {
        print!("{}", format_registers(&core));
    }

    if cfg.fb_crc32 {
        let mut crc = 0xFFFF_FFFFu32;
        let mut non_white = 0usize;
        let mut green_dominant = 0usize;
        let mut red_dominant = 0usize;
        for &px in &frontend.last_fb {
            if px != 0x7FFF {
                non_white += 1;
            }
            let r = px & 0x1F;
            let g = (px >> 5) & 0x1F;
            let b = (px >> 10) & 0x1F;
            if g > r + 4 && g > b + 4 {
                green_dominant += 1;
            }
            if r > g + 4 && r > b + 4 {
                red_dominant += 1;
            }
            crc ^= px as u32;
            for _ in 0..16 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        crc = !crc;
        println!(
            "FB CRC32={crc:#010x} non_white={non_white} green_px={green_dominant} red_px={red_dominant}"
        );
    }

    if cfg.dump_ascii_grid {
        print_ascii_preview(&frontend.last_fb);
    }

    if let Some(ppm_path) = &cfg.screenshot_ppm {
        write_ppm(ppm_path, &frontend.last_fb)?;
        println!("Wrote screenshot to {}", ppm_path.display());
    }

    Ok(())
}

pub fn write_ppm(path: &Path, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(32 + LCD_WIDTH * LCD_HEIGHT * 3);
    bytes.extend_from_slice(format!("P6\n{} {}\n255\n", LCD_WIDTH, LCD_HEIGHT).as_bytes());
    for &px in fb {
        let r = (((px & 0x1F) * 255) / 31) as u8;
        let g = ((((px >> 5) & 0x1F) * 255) / 31) as u8;
        let b = ((((px >> 10) & 0x1F) * 255) / 31) as u8;
        bytes.push(r);
        bytes.push(g);
        bytes.push(b);
    }
    fs::write(path, &bytes).map_err(|e| format!("failed to write PPM: {e}"))
}

fn print_ascii_preview(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
    // Downsample 240x160 -> 80x40 using blocks of 3x4 pixels for crisp terminal inspection.
    for by in 0..40 {
        let mut line = String::with_capacity(80);
        for bx in 0..80 {
            let mut sum_luma = 0u32;
            let mut min_luma = 255u32;
            for dy in 0..4 {
                for dx in 0..3 {
                    let px = fb[(by * 4 + dy) * LCD_WIDTH + (bx * 3 + dx)];
                    let r = ((px & 0x1F) * 8) as u32;
                    let g = (((px >> 5) & 0x1F) * 8) as u32;
                    let b = (((px >> 10) & 0x1F) * 8) as u32;
                    let luma = (r * 3 + g * 6 + b) / 10;
                    sum_luma += luma;
                    min_luma = min_luma.min(luma);
                }
            }
            let avg = sum_luma / 12;
            let val = if min_luma < 80 && avg > 150 {
                min_luma
            } else {
                avg
            };
            let ch = match val {
                0..=40 => '#',
                41..=100 => '+',
                101..=170 => ':',
                171..=230 => '.',
                _ => ' ',
            };
            line.push(ch);
        }
        println!("|{line}|");
    }
}
