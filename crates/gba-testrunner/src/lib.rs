use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gba_core::{
    GbaCore, HostFrontend, KEY_A, KEY_B, KEY_DOWN, KEY_L, KEY_LEFT, KEY_R, KEY_RIGHT, KEY_SELECT,
    KEY_START, KEY_UP, LCD_HEIGHT, LCD_WIDTH, encode_keyinput,
};
use image::{ImageBuffer, Rgb, RgbImage};
use serde::{Deserialize, Serialize};

/// Embedded default test manifest compiled into the crate as a fallback.
pub const DEFAULT_MANIFEST_TOML: &str = include_str!("../manifest.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    /// Jsmolka-style register check: wait for PC to settle in ROM idle loop and verify register value.
    #[default]
    Register,
    /// Compare 64-bit FNV-1a framebuffer hash against a recorded `.hash` file in `golden/`.
    FbHash,
    /// Compare 240x160 framebuffer (at 5-bit BGR555 precision) against a recorded `.png` in `golden/`.
    Screenshot,
}

impl Criterion {
    pub fn label(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::FbHash => "fb_hash",
            Self::Screenshot => "screenshot",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputEvent {
    /// 0-based frame index at which keys begin being held.
    pub frame: usize,
    /// Number of consecutive frames to hold the key mask (default 2).
    #[serde(default = "default_input_duration")]
    pub duration: usize,
    /// Key names (`A`, `B`, `SELECT`, `START`, `RIGHT`, `LEFT`, `UP`, `DOWN`, `R`, `L`).
    pub keys: Vec<String>,
}

const fn default_input_duration() -> usize {
    2
}

const fn default_reg() -> usize {
    12
}

const fn default_min_frames() -> usize {
    5
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestEntry {
    pub id: String,
    pub suite: String,
    pub name: String,
    pub rom_path: String,
    pub frames: usize,
    #[serde(default)]
    pub criterion: Criterion,
    #[serde(default = "default_reg")]
    pub register: usize,
    #[serde(default)]
    pub expected_reg_value: u32,
    #[serde(default = "default_min_frames")]
    pub min_frames: usize,
    #[serde(default)]
    pub inputs: Vec<InputEvent>,
    /// Optional flag indicating a known current-core failure tracked for diagnosis (allows `cargo test` to report known baseline failures without masking regressions).
    #[serde(default)]
    pub known_failure: bool,
    /// Optional CRC32 hex baseline (`"0x..."`) of the known-bad output when no passing golden exists.
    #[serde(default)]
    pub known_bad_crc32: Option<String>,
    /// Marks interactive menu/visualizer ROMs that should not have automated goldens recorded by `--record`.
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub tests: Vec<TestEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Pass,
    Fail,
    SkipMissingRom,
    SkipMissingGolden,
}

impl TestOutcome {
    pub fn status_text(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::SkipMissingRom => "SKIP (missing ROM)",
            Self::SkipMissingGolden => "SKIP (missing golden)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestResult {
    pub id: String,
    pub suite: String,
    pub name: String,
    pub rom_path: String,
    pub criterion: Criterion,
    pub outcome: TestOutcome,
    pub known_failure: bool,
    pub frames_executed: usize,
    pub avg_frame_ms: f64,
    pub fb_hash: Option<u64>,
    pub fb_crc32: Option<u32>,
    pub notes: String,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub bios_path: Option<PathBuf>,
    pub golden_dir: PathBuf,
    pub output_dir: PathBuf,
    pub record: bool,
    pub dump_all: bool,
    pub filter: Option<String>,
    pub suite: Option<String>,
}

impl RunOptions {
    pub fn for_workspace(root: &Path) -> Self {
        Self {
            bios_path: std::env::var_os("GBA_BIOS_PATH").map(PathBuf::from),
            golden_dir: root.join("crates/gba-testrunner/golden"),
            output_dir: root.join("test_output"),
            record: false,
            dump_all: false,
            filter: None,
            suite: None,
        }
    }
}

/// Parses a `Manifest` from TOML text.
pub fn parse_manifest(toml_str: &str) -> Result<Manifest, String> {
    toml::from_str(toml_str).map_err(|e| format!("failed to parse manifest TOML: {e}"))
}

/// Loads `manifest.toml` from disk or falls back to the embedded default manifest.
pub fn load_manifest(path: Option<&Path>, root: &Path) -> Result<Manifest, String> {
    if let Some(p) = path {
        let content = fs::read_to_string(p)
            .map_err(|e| format!("failed to read manifest {}: {e}", p.display()))?;
        return parse_manifest(&content);
    }
    let disk_default = root.join("crates/gba-testrunner/manifest.toml");
    if disk_default.exists() {
        let content = fs::read_to_string(&disk_default)
            .map_err(|e| format!("failed to read manifest {}: {e}", disk_default.display()))?;
        return parse_manifest(&content);
    }
    parse_manifest(DEFAULT_MANIFEST_TOML)
}

/// Discovers the repository/workspace root directory by walking up from `CARGO_MANIFEST_DIR` or CWD.
pub fn discover_workspace_root() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = walk_up_for_root(&cwd)
    {
        return found;
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(found) = walk_up_for_root(&manifest_dir) {
        return found;
    }
    PathBuf::from(".")
}

fn walk_up_for_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join("Cargo.toml").exists() && dir.join("crates/gba-core").exists() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Parses a list of key names into a GBA active-high key bitmask (`KEY_*`).
pub fn parse_key_mask(keys: &[String]) -> Result<u16, String> {
    let mut mask = 0u16;
    for raw in keys {
        let bit = match raw.trim().to_ascii_uppercase().as_str() {
            "A" => KEY_A,
            "B" => KEY_B,
            "SELECT" => KEY_SELECT,
            "START" => KEY_START,
            "RIGHT" => KEY_RIGHT,
            "LEFT" => KEY_LEFT,
            "UP" => KEY_UP,
            "DOWN" => KEY_DOWN,
            "R" => KEY_R,
            "L" => KEY_L,
            other => return Err(format!("unknown GBA key name: {other}")),
        };
        mask |= bit;
    }
    Ok(mask)
}

struct ScriptedFrontend {
    key_schedule: Vec<u16>,
    current_frame: usize,
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    presented: bool,
}

impl ScriptedFrontend {
    fn new(total_frames: usize, inputs: &[InputEvent]) -> Result<Self, String> {
        let mut key_schedule = vec![0u16; total_frames + 1];
        for ev in inputs {
            let mask = parse_key_mask(&ev.keys)?;
            let end = ev.frame.saturating_add(ev.duration.max(1));
            for f in ev.frame..end {
                if f < key_schedule.len() {
                    key_schedule[f] |= mask;
                }
            }
        }
        Ok(Self {
            key_schedule,
            current_frame: 0,
            last_fb: [0x7FFF; LCD_WIDTH * LCD_HEIGHT],
            presented: false,
        })
    }
}

impl HostFrontend for ScriptedFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.last_fb.copy_from_slice(fb);
        self.presented = true;
    }

    fn push_audio_samples(&mut self, _samples: &[i16]) {}

    fn poll_keypad(&mut self) -> u16 {
        let mask = self
            .key_schedule
            .get(self.current_frame)
            .copied()
            .unwrap_or(0);
        encode_keyinput(mask)
    }
}

/// Computes a deterministic 64-bit FNV-1a hash of a 240x160 BGR555 framebuffer.
pub fn compute_fb_hash(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &px in fb {
        let bytes = px.to_le_bytes();
        for b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01B3);
        }
    }
    hash
}

/// Computes the same per-halfword CRC32 used by `gba-frontend-desktop --fb-crc32`.
pub fn compute_fb_crc32(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &px in fb {
        crc ^= px as u32;
        for _ in 0..16 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Converts a GBA BGR555 framebuffer to an RGB888 image using bit replication (`(c << 3) | (c >> 2)`).
pub fn fb_to_rgb_image(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> RgbImage {
    let mut img = ImageBuffer::new(LCD_WIDTH as u32, LCD_HEIGHT as u32);
    for (idx, &px) in fb.iter().enumerate() {
        let x = (idx % LCD_WIDTH) as u32;
        let y = (idx / LCD_WIDTH) as u32;
        let r5 = (px & 0x1F) as u8;
        let g5 = ((px >> 5) & 0x1F) as u8;
        let b5 = ((px >> 10) & 0x1F) as u8;
        let r = (r5 << 3) | (r5 >> 2);
        let g = (g5 << 3) | (g5 >> 2);
        let b = (b5 << 3) | (b5 >> 2);
        img.put_pixel(x, y, Rgb([r, g, b]));
    }
    img
}

/// Saves a GBA framebuffer as a 240x160 RGB888 PNG.
pub fn save_fb_png(path: &Path, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
    }
    let img = fb_to_rgb_image(fb);
    img.save(path)
        .map_err(|e| format!("failed to write PNG {}: {e}", path.display()))
}

/// Coordinate and RGB555 values `(x, y, got_bgr555, expected_bgr555)` of the first differing pixel.
pub type PixelDiffCoord = (u32, u32, u16, u16);

/// Result of comparing a GBA framebuffer against a golden PNG file.
pub type PngDiffResult = Result<(usize, Option<PixelDiffCoord>), String>;

/// Compares a GBA BGR555 framebuffer against a golden PNG at 5-bit BGR555 precision.
/// Returns `Ok((0, None))` on exact match, or `Ok((diff_pixels, first_diff))` when pixels differ.
pub fn compare_fb_with_png(
    fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
    golden_path: &Path,
) -> PngDiffResult {
    let img = image::open(golden_path)
        .map_err(|e| format!("failed to read golden PNG {}: {e}", golden_path.display()))?
        .to_rgb8();
    if img.width() != LCD_WIDTH as u32 || img.height() != LCD_HEIGHT as u32 {
        return Err(format!(
            "golden PNG {} has dimensions {}x{}, expected {}x{}",
            golden_path.display(),
            img.width(),
            img.height(),
            LCD_WIDTH,
            LCD_HEIGHT
        ));
    }

    let mut diff_count = 0usize;
    let mut first_diff = None;
    for y in 0..LCD_HEIGHT {
        for x in 0..LCD_WIDTH {
            let px = fb[y * LCD_WIDTH + x] & 0x7FFF;
            let p = img.get_pixel(x as u32, y as u32);
            let er = ((p[0] >> 3) & 0x1F) as u16;
            let eg = ((p[1] >> 3) & 0x1F) as u16;
            let eb = ((p[2] >> 3) & 0x1F) as u16;
            let expected_bgr555 = er | (eg << 5) | (eb << 10);
            if px != expected_bgr555 {
                diff_count += 1;
                if first_diff.is_none() {
                    first_diff = Some((x as u32, y as u32, px, expected_bgr555));
                }
            }
        }
    }
    Ok((diff_count, first_diff))
}

fn count_distinct_colors(fb: &[u16; LCD_WIDTH * LCD_HEIGHT], limit: usize) -> usize {
    let mut seen = HashSet::with_capacity(limit);
    for &px in fb {
        seen.insert(px & 0x7FFF);
        if seen.len() >= limit {
            break;
        }
    }
    seen.len()
}

/// Runs the filtered manifest suite and returns all test results.
pub fn run_suite(
    root: &Path,
    manifest: &Manifest,
    opts: &RunOptions,
) -> Result<Vec<TestResult>, String> {
    let bios = match &opts.bios_path {
        Some(p) => {
            let bytes =
                fs::read(p).map_err(|e| format!("failed to read BIOS {}: {e}", p.display()))?;
            Some(bytes)
        }
        None => None,
    };

    let mut results = Vec::new();
    for entry in &manifest.tests {
        if let Some(suite_filter) = &opts.suite
            && !entry.suite.eq_ignore_ascii_case(suite_filter)
        {
            continue;
        }
        if let Some(substr) = &opts.filter {
            let needle = substr.to_ascii_lowercase();
            if !entry.id.to_ascii_lowercase().contains(&needle)
                && !entry.name.to_ascii_lowercase().contains(&needle)
                && !entry.suite.to_ascii_lowercase().contains(&needle)
            {
                continue;
            }
        }
        results.push(run_single_test(root, entry, bios.as_deref(), opts));
    }
    Ok(results)
}

/// Executes a single manifest test entry.
pub fn run_single_test(
    root: &Path,
    entry: &TestEntry,
    bios: Option<&[u8]>,
    opts: &RunOptions,
) -> TestResult {
    let rom_abs = root.join(&entry.rom_path);
    if !rom_abs.exists() {
        return TestResult {
            id: entry.id.clone(),
            suite: entry.suite.clone(),
            name: entry.name.clone(),
            rom_path: entry.rom_path.clone(),
            criterion: entry.criterion,
            outcome: TestOutcome::SkipMissingRom,
            known_failure: entry.known_failure,
            frames_executed: 0,
            avg_frame_ms: 0.0,
            fb_hash: None,
            fb_crc32: None,
            notes: if entry.notes.is_empty() {
                "ROM not present".to_string()
            } else {
                entry.notes.clone()
            },
        };
    }

    let rom = match fs::read(&rom_abs) {
        Ok(b) => b,
        Err(e) => {
            return TestResult {
                id: entry.id.clone(),
                suite: entry.suite.clone(),
                name: entry.name.clone(),
                rom_path: entry.rom_path.clone(),
                criterion: entry.criterion,
                outcome: TestOutcome::Fail,
                known_failure: entry.known_failure,
                frames_executed: 0,
                avg_frame_ms: 0.0,
                fb_hash: None,
                fb_crc32: None,
                notes: format!("failed to read ROM: {e}"),
            };
        }
    };

    let mut frontend = match ScriptedFrontend::new(entry.frames, &entry.inputs) {
        Ok(fe) => fe,
        Err(e) => {
            return TestResult {
                id: entry.id.clone(),
                suite: entry.suite.clone(),
                name: entry.name.clone(),
                rom_path: entry.rom_path.clone(),
                criterion: entry.criterion,
                outcome: TestOutcome::Fail,
                known_failure: entry.known_failure,
                frames_executed: 0,
                avg_frame_ms: 0.0,
                fb_hash: None,
                fb_crc32: None,
                notes: format!("invalid input script: {e}"),
            };
        }
    };

    let mut core = GbaCore::new(bios, rom);
    let mut frames_executed = 0usize;
    let mut last_pc = u32::MAX;
    let mut stable_pc_frames = 0usize;
    let mut settled_in_loop = false;

    let started = Instant::now();
    for frame in 0..entry.frames {
        frontend.current_frame = frame;
        core.run_frame(&mut frontend);
        frames_executed += 1;

        if entry.criterion == Criterion::Register {
            let pc = core.cpu.current_exec_pc();
            if pc == last_pc && pc >= 0x0200_0000 {
                stable_pc_frames += 1;
            } else {
                last_pc = pc;
                stable_pc_frames = 1;
            }
            let cur_fb = if frontend.presented {
                &frontend.last_fb
            } else {
                &*core.bus.ppu.framebuffer
            };
            if frame + 1 >= entry.min_frames
                && stable_pc_frames >= 3
                && count_distinct_colors(cur_fb, 2) >= 2
            {
                settled_in_loop = true;
                break;
            }
        }
    }
    if entry.criterion == Criterion::Register && !settled_in_loop {
        settled_in_loop = stable_pc_frames >= 3;
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let avg_frame_ms = if frames_executed > 0 {
        elapsed_ms / (frames_executed as f64)
    } else {
        0.0
    };

    let fb = if frontend.presented {
        &frontend.last_fb
    } else {
        &*core.bus.ppu.framebuffer
    };
    let fb_hash = compute_fb_hash(fb);
    let fb_crc32 = compute_fb_crc32(fb);
    let observed = ObservedFrame {
        fb,
        hash: fb_hash,
        crc32: fb_crc32,
    };

    let (outcome, notes) = evaluate_entry(
        entry,
        &core,
        &observed,
        settled_in_loop,
        frames_executed,
        opts,
    );

    if opts.dump_all || outcome == TestOutcome::Fail {
        let dump_path = opts.output_dir.join(format!("{}.png", entry.id));
        if let Err(e) = save_fb_png(&dump_path, fb) {
            log_write_err(&dump_path, &e);
        }
    }

    TestResult {
        id: entry.id.clone(),
        suite: entry.suite.clone(),
        name: entry.name.clone(),
        rom_path: entry.rom_path.clone(),
        criterion: entry.criterion,
        outcome,
        known_failure: entry.known_failure,
        frames_executed,
        avg_frame_ms,
        fb_hash: Some(fb_hash),
        fb_crc32: Some(fb_crc32),
        notes,
    }
}

fn log_write_err(path: &Path, err: &str) {
    eprintln!("warning: could not save {}: {err}", path.display());
}

struct ObservedFrame<'a> {
    fb: &'a [u16; LCD_WIDTH * LCD_HEIGHT],
    hash: u64,
    crc32: u32,
}

fn evaluate_entry(
    entry: &TestEntry,
    core: &GbaCore,
    observed: &ObservedFrame<'_>,
    settled_in_loop: bool,
    frames_executed: usize,
    opts: &RunOptions,
) -> (TestOutcome, String) {
    let fb = observed.fb;
    let fb_hash = observed.hash;
    let fb_crc32 = observed.crc32;
    match entry.criterion {
        Criterion::Register => {
            let reg_idx = entry.register.min(15);
            let reg_val = core.cpu.gpr[reg_idx];
            let pc = core.cpu.current_exec_pc();
            let distinct = count_distinct_colors(fb, 2);

            if !settled_in_loop {
                return (
                    TestOutcome::Fail,
                    format!(
                        "did not reach idle loop within {frames_executed} frames (PC=0x{pc:08X}, r{reg_idx}={reg_val})"
                    ),
                );
            }
            if reg_val != entry.expected_reg_value {
                return (
                    TestOutcome::Fail,
                    format!(
                        "failed test #{reg_val} (r{reg_idx}={reg_val}, expected {}, PC=0x{pc:08X})",
                        entry.expected_reg_value
                    ),
                );
            }
            if distinct < 2 {
                return (
                    TestOutcome::Fail,
                    format!(
                        "PC settled at 0x{pc:08X} with r{reg_idx}={reg_val}, but screen is blank (possible early hang)"
                    ),
                );
            }
            (
                TestOutcome::Pass,
                format!("r{reg_idx}={reg_val} at idle PC=0x{pc:08X} ({frames_executed}f)"),
            )
        }
        Criterion::FbHash => {
            let hash_path = opts.golden_dir.join(format!("{}.hash", entry.id));
            if opts.record && !entry.known_failure && !entry.interactive {
                if let Err(e) = write_golden_hash(&hash_path, fb_hash) {
                    return (TestOutcome::Fail, e);
                }
                return (
                    TestOutcome::Pass,
                    format!("recorded hash 0x{fb_hash:016x} (crc32=0x{fb_crc32:08x})"),
                );
            }
            let expected_str = match fs::read_to_string(&hash_path) {
                Ok(s) => s,
                Err(_) => {
                    return evaluate_missing_golden(
                        entry,
                        fb_hash,
                        fb_crc32,
                        "golden hash",
                        &hash_path,
                    );
                }
            };
            let trimmed = expected_str.trim().trim_start_matches("0x");
            match u64::from_str_radix(trimmed, 16) {
                Ok(expected) if expected == fb_hash => (
                    TestOutcome::Pass,
                    format!("hash 0x{fb_hash:016x} (crc32=0x{fb_crc32:08x})"),
                ),
                Ok(expected) => (
                    TestOutcome::Fail,
                    format!("hash mismatch: got 0x{fb_hash:016x}, expected 0x{expected:016x}"),
                ),
                Err(e) => (
                    TestOutcome::Fail,
                    format!("invalid golden hash in {}: {e}", hash_path.display()),
                ),
            }
        }
        Criterion::Screenshot => {
            let png_path = opts.golden_dir.join(format!("{}.png", entry.id));
            if opts.record && !entry.known_failure && !entry.interactive {
                if let Err(e) = save_fb_png(&png_path, fb) {
                    return (TestOutcome::Fail, e);
                }
                return (
                    TestOutcome::Pass,
                    format!("recorded golden PNG (crc32=0x{fb_crc32:08x})"),
                );
            }
            if !png_path.exists() {
                return evaluate_missing_golden(entry, fb_hash, fb_crc32, "golden PNG", &png_path);
            }
            match compare_fb_with_png(fb, &png_path) {
                Ok((0, _)) => (
                    TestOutcome::Pass,
                    format!("pixel-exact match (crc32=0x{fb_crc32:08x})"),
                ),
                Ok((diffs, first)) => {
                    let detail = match first {
                        Some((x, y, got, exp)) => {
                            format!(
                                " first diff at ({x},{y}): got 0x{got:04X}, expected 0x{exp:04X}"
                            )
                        }
                        None => String::new(),
                    };
                    (
                        TestOutcome::Fail,
                        format!("{diffs} pixels differ vs golden PNG;{detail}"),
                    )
                }
                Err(e) => (TestOutcome::Fail, e),
            }
        }
    }
}

fn evaluate_missing_golden(
    entry: &TestEntry,
    fb_hash: u64,
    fb_crc32: u32,
    kind_desc: &str,
    missing_path: &Path,
) -> (TestOutcome, String) {
    if entry.known_failure {
        let base_note = if entry.notes.is_empty() {
            "known failure (no passing golden recorded)"
        } else {
            entry.notes.as_str()
        };
        if let Some(ref bad_hex) = entry.known_bad_crc32 {
            let trimmed = bad_hex.trim().trim_start_matches("0x");
            return match u32::from_str_radix(trimmed, 16) {
                Ok(exp_bad) if exp_bad == fb_crc32 => (
                    TestOutcome::Fail,
                    format!("{base_note} (crc32=0x{fb_crc32:08x})"),
                ),
                Ok(exp_bad) => (
                    TestOutcome::Fail,
                    format!(
                        "output changed from known-bad baseline 0x{exp_bad:08x} to 0x{fb_crc32:08x} ({base_note})"
                    ),
                ),
                Err(_) => (
                    TestOutcome::Fail,
                    format!(
                        "invalid known_bad_crc32 `{bad_hex}` (observed crc32=0x{fb_crc32:08x})"
                    ),
                ),
            };
        }
        return (
            TestOutcome::Fail,
            format!("{base_note} (crc32=0x{fb_crc32:08x})"),
        );
    }

    let suffix = if entry.notes.is_empty() {
        String::new()
    } else {
        format!(" — {}", entry.notes)
    };
    let rel_golden = missing_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| format!("golden/{n}"))
        .unwrap_or_else(|| missing_path.display().to_string());
    (
        TestOutcome::SkipMissingGolden,
        format!(
            "missing {kind_desc} `{rel_golden}` (observed 0x{fb_hash:016x}, crc32=0x{fb_crc32:08x}){suffix}"
        ),
    )
}

fn write_golden_hash(path: &Path, hash: u64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create golden dir {}: {e}", parent.display()))?;
    }
    fs::write(path, format!("0x{hash:016x}\n"))
        .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Counts outcomes in a slice of `TestResult`s.
#[derive(Debug, Clone, Copy, Default)]
pub struct SummaryCounts {
    pub passed: usize,
    pub failed: usize,
    pub skipped_rom: usize,
    pub skipped_golden: usize,
}

pub fn summarize_results(results: &[TestResult]) -> SummaryCounts {
    let mut s = SummaryCounts::default();
    for r in results {
        match r.outcome {
            TestOutcome::Pass => s.passed += 1,
            TestOutcome::Fail => s.failed += 1,
            TestOutcome::SkipMissingRom => s.skipped_rom += 1,
            TestOutcome::SkipMissingGolden => s.skipped_golden += 1,
        }
    }
    s
}

/// Renders the test results table in Markdown format.
pub fn render_markdown_table(results: &[TestResult]) -> String {
    let mut out = String::new();
    out.push_str("| ROM | Criterion | Status | Notes | Frame Time (ms) |\n");
    out.push_str("|-----|-----------|--------|-------|-----------------|\n");
    for r in results {
        let frame_ms = if r.frames_executed > 0 {
            format!("{:.3}", r.avg_frame_ms)
        } else {
            "-".to_string()
        };
        let notes = r.notes.replace('|', "\\|").replace('\n', " ");
        out.push_str(&format!(
            "| `{}` | `{}` | **{}** | {} | {} |\n",
            r.name,
            r.criterion.label(),
            r.outcome.status_text(),
            notes,
            frame_ms
        ));
    }
    out
}

/// Renders a full `TEST_RESULTS.md` document (preserving any existing `## Failure Diagnosis` section if `--preserve-diagnosis` is used, or generating a clean report).
pub fn render_markdown_report(results: &[TestResult], bios_label: &str) -> String {
    let counts = summarize_results(results);
    let total = results.len();
    let mut out = String::new();
    out.push_str("# Test ROM Results\n\n");
    out.push_str(&format!(
        "- **BIOS mode:** {bios_label}\n\
         - **Summary:** {} passed, {} failed, {} skipped (missing ROM), {} skipped (missing golden) out of {} total\n\n",
        counts.passed, counts.failed, counts.skipped_rom, counts.skipped_golden, total
    ));
    out.push_str("## Results Table\n\n");
    out.push_str(&render_markdown_table(results));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_mask_combines_buttons() {
        let mask = parse_key_mask(&["start".to_string(), "A".to_string(), "Down".to_string()])
            .expect("valid keys");
        assert_eq!(mask, KEY_START | KEY_A | KEY_DOWN);
        assert_eq!(encode_keyinput(mask), (!mask) & 0x03FF);
    }

    #[test]
    fn parse_key_mask_rejects_unknown_button() {
        assert!(parse_key_mask(&["TURBO".to_string()]).is_err());
    }

    #[test]
    fn embedded_manifest_parses_cleanly() {
        let manifest = parse_manifest(DEFAULT_MANIFEST_TOML).expect("valid embedded manifest");
        assert!(!manifest.tests.is_empty());
        let mut ids = HashSet::new();
        for t in &manifest.tests {
            assert!(ids.insert(&t.id), "duplicate manifest id: {}", t.id);
        }
    }

    #[test]
    fn missing_rom_reports_skip_not_fail() {
        let root = discover_workspace_root();
        let mut opts = RunOptions::for_workspace(&root);
        opts.output_dir = std::env::temp_dir().join("gba_testrunner_unit_out");
        let entry = TestEntry {
            id: "nonexistent_test".to_string(),
            suite: "unit".to_string(),
            name: "does/not/exist.gba".to_string(),
            rom_path: "test-roms/__nonexistent__/missing.gba".to_string(),
            frames: 10,
            criterion: Criterion::Register,
            register: 12,
            expected_reg_value: 0,
            min_frames: 2,
            inputs: vec![],
            known_failure: false,
            known_bad_crc32: None,
            interactive: false,
            notes: String::new(),
        };
        let res = run_single_test(&root, &entry, None, &opts);
        assert_eq!(res.outcome, TestOutcome::SkipMissingRom);
    }

    #[test]
    fn missing_golden_with_known_failure_reports_fail() {
        let fb = [0u16; LCD_WIDTH * LCD_HEIGHT];
        let hash = compute_fb_hash(&fb);
        let crc = compute_fb_crc32(&fb);
        let entry = TestEntry {
            id: "unit_known_fail".to_string(),
            suite: "unit".to_string(),
            name: "unit/known_fail.gba".to_string(),
            rom_path: "unused.gba".to_string(),
            frames: 1,
            criterion: Criterion::Screenshot,
            register: 12,
            expected_reg_value: 0,
            min_frames: 1,
            inputs: vec![],
            known_failure: true,
            known_bad_crc32: Some(format!("0x{crc:08x}")),
            interactive: false,
            notes: "expected subtest failure".to_string(),
        };
        let missing = std::env::temp_dir().join("__nonexistent_golden__.png");
        let (outcome, notes) = evaluate_missing_golden(&entry, hash, crc, "golden PNG", &missing);
        assert_eq!(outcome, TestOutcome::Fail);
        assert!(notes.contains("expected subtest failure"));
        assert!(notes.contains(&format!("0x{crc:08x}")));
    }

    #[test]
    fn fb_hash_and_png_roundtrip_match_at_5bit_precision() {
        let mut fb = [0u16; LCD_WIDTH * LCD_HEIGHT];
        for (i, px) in fb.iter_mut().enumerate() {
            let r = (i % 32) as u16;
            let g = ((i / 32) % 32) as u16;
            let b = ((i / 1024) % 32) as u16;
            *px = r | (g << 5) | (b << 10);
        }
        let hash1 = compute_fb_hash(&fb);
        let hash2 = compute_fb_hash(&fb);
        assert_eq!(hash1, hash2);

        let tmp_png = std::env::temp_dir().join("gba_testrunner_roundtrip.png");
        save_fb_png(&tmp_png, &fb).expect("save png");
        let (diffs, first) = compare_fb_with_png(&fb, &tmp_png).expect("compare png");
        let _ = fs::remove_file(&tmp_png);
        assert_eq!(diffs, 0, "unexpected diff: {first:?}");
    }
}
