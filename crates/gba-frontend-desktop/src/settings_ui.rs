use crate::app::{PendingConfirmAction, RustboyApp};
use crate::config::{AppConfig, HotkeyAction};
use crate::theme::{ACCENT_TEAL, COLOR_TEXT, COLOR_TEXT_SECONDARY, COLOR_TILE, STROKE_BORDER};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const SELFTEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Parsed summary and full report produced by `rustboy-advance --selftest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PerfCheckReport {
    pub(crate) passed: bool,
    pub(crate) exit_success: bool,
    pub(crate) elapsed_ms: u64,
    pub(crate) status_label: String,
    pub(crate) build_info: Option<String>,
    pub(crate) enumerated_adapters: Vec<String>,
    pub(crate) chosen_adapter: Option<String>,
    pub(crate) calibration: Option<String>,
    pub(crate) headless: Option<String>,
    pub(crate) paced: Option<String>,
    pub(crate) fixed_gpu_wait: Option<String>,
    pub(crate) slowdown_sweep: Option<String>,
    pub(crate) measured_ceiling: Option<String>,
    pub(crate) recommended_filter: Option<String>,
    pub(crate) fail_reasons: Option<String>,
    pub(crate) json_path: Option<String>,
    pub(crate) full_json: Option<String>,
    pub(crate) raw_output: String,
    pub(crate) error_message: Option<String>,
}

impl PerfCheckReport {
    pub(crate) fn verdict_headline(&self) -> &'static str {
        if !self.passed {
            return "May stutter: try Sharp Pixels";
        }
        let rec_lower = self
            .recommended_filter
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let rec_sharp = rec_lower.starts_with("sharp1x")
            || rec_lower.starts_with("raw-nearest")
            || rec_lower.starts_with("none");
        let zero_ceiling = self
            .measured_ceiling
            .as_deref()
            .and_then(|s| s.split('=').nth(1))
            .and_then(|rhs| {
                rhs.trim().strip_suffix('x').or_else(|| {
                    rhs.split_whitespace()
                        .next()
                        .and_then(|t| t.strip_suffix('x'))
                })
            })
            .and_then(|num| num.trim().parse::<u32>().ok())
            .is_some_and(|x| x == 0);
        if rec_sharp || zero_ceiling {
            "May stutter: try Sharp Pixels"
        } else {
            "Runs smoothly on this PC"
        }
    }

    pub(crate) fn plain_summary_line(&self) -> String {
        if let Some(ref rec) = self.recommended_filter {
            let filter_id = rec
                .split(['—', '-'])
                .next()
                .map(str::trim)
                .unwrap_or("")
                .trim();
            let friendly_filter = match filter_id.to_ascii_lowercase().as_str() {
                "smooth-bilinear" | "bilinear" => "Soft & Balanced",
                "sharp1x" | "raw-nearest" | "none" => "Sharp Pixels",
                "authentic-gba-lcd" | "lcd-grid" => "Authentic GBA LCD",
                "smooth-scale2x" | "hq2x" | "xbrz" => "Smooth Scale2x / Crisp HD",
                _ => filter_id,
            };
            let headroom = self
                .measured_ceiling
                .as_deref()
                .and_then(|c| c.split('=').nth(1))
                .and_then(|rhs| rhs.split('|').next())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match (friendly_filter.is_empty(), headroom) {
                (false, Some(h)) => format!(
                    "Recommended screen look: {friendly_filter} ({h} smooth speed headroom)"
                ),
                (false, None) => format!("Recommended screen look: {friendly_filter}"),
                (true, Some(h)) => format!("Measured smooth headroom: {h}"),
                (true, None) => "Performance check complete.".to_string(),
            }
        } else if let Some(ref reasons) = self.fail_reasons {
            reasons.clone()
        } else if let Some(ref err) = self.error_message {
            err.clone()
        } else {
            "Check completed. Open Details below for adapter and benchmark timings.".to_string()
        }
    }

    pub(crate) fn export_json(&self) -> String {
        if let Some(ref json) = self.full_json {
            return json.clone();
        }
        let build = self.build_info.as_deref().unwrap_or("");
        let extract_kv = |key: &str| -> String {
            build
                .split('|')
                .filter_map(|part| part.trim().strip_prefix(key))
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let payload = serde_json::json!({
            "passed": self.passed,
            "status_label": self.status_label,
            "elapsed_ms": self.elapsed_ms,
            "git_commit": extract_kv("commit="),
            "build_time_utc": extract_kv("built="),
            "exe_sha256": extract_kv("exe_sha256="),
            "available_adapters": self.enumerated_adapters,
            "chosen_adapter": self.chosen_adapter,
            "calibration_ratio_vs_ref": self.calibration.as_deref().and_then(|c| {
                c.split("ratio vs ref =")
                    .nth(1)
                    .and_then(|s| s.split('x').next())
                    .and_then(|n| n.trim().parse::<f64>().ok())
            }),
            "calibration": self.calibration,
            "headless": self.headless,
            "paced": self.paced,
            "fixed_gpu_wait": self.fixed_gpu_wait,
            "slowdown_sweep": self.slowdown_sweep,
            "measured_smooth_ceiling_x": self.measured_ceiling.as_deref().and_then(|c| {
                c.split('=')
                    .nth(1)
                    .and_then(|s| s.split('x').next())
                    .and_then(|n| n.trim().parse::<usize>().ok())
            }),
            "measured_zero_over_budget_x": self.measured_ceiling.as_deref().and_then(|c| {
                c.split("zero-over-budget =")
                    .nth(1)
                    .and_then(|s| s.split('x').next())
                    .and_then(|n| n.trim().parse::<usize>().ok())
            }),
            "measured_ceiling": self.measured_ceiling,
            "recommended_filter": self.recommended_filter,
            "fail_reasons": self.fail_reasons,
        });
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| self.raw_output.clone())
    }
}

/// Asynchronous execution state for `Help -> Run performance check`.
#[derive(Default)]
pub(crate) enum PerfCheckState {
    #[default]
    Idle,
    Running {
        started_at: Instant,
        cancel: Arc<AtomicBool>,
        rx: Receiver<PerfCheckReport>,
    },
    Finished(Box<PerfCheckReport>),
}

/// Parses `--selftest` stdout/stderr output into a structured summary + full raw report.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_selftest_report(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
    elapsed_ms: u64,
) -> PerfCheckReport {
    parse_selftest_report_with_json(stdout, stderr, None, exit_success, elapsed_ms)
}

/// Parses `--selftest` stdout/stderr and optional verbatim `SelftestReportJson` file contents.
pub(crate) fn parse_selftest_report_with_json(
    stdout: &str,
    stderr: &str,
    full_json_override: Option<String>,
    exit_success: bool,
    elapsed_ms: u64,
) -> PerfCheckReport {
    let mut header_pass = None;
    let mut build_info = None;
    let mut enumerated_adapters = Vec::new();
    let mut in_enumerated_gpus = false;
    let mut chosen_adapter = None;
    let mut calibration = None;
    let mut headless = None;
    let mut paced = None;
    let mut fixed_gpu_wait = None;
    let mut slowdown_sweep = None;
    let mut measured_ceiling = None;
    let mut recommended_filter = None;
    let mut fail_reasons = None;
    let mut json_path: Option<String> = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if in_enumerated_gpus {
            if line.starts_with('[') {
                enumerated_adapters.push(line.to_string());
                continue;
            }
            in_enumerated_gpus = false;
        }
        if line.contains("--SELFTEST (PASS)") {
            header_pass = Some(true);
        } else if line.contains("--SELFTEST (FAIL)") {
            header_pass = Some(false);
        } else if let Some(val) = line.strip_prefix("Build:") {
            measured_filter_assign(&mut build_info, val);
        } else if line.starts_with("Enumerated GPUs:") {
            in_enumerated_gpus = true;
        } else if let Some(val) = line.strip_prefix("Chosen Adapter:") {
            chosen_adapter = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Calibration:") {
            calibration = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Headless (600f):") {
            headless = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Paced (600f +120w):") {
            paced = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Fixed GPU Wait:") {
            measured_filter_assign(&mut fixed_gpu_wait, val);
        } else if let Some(val) = line.strip_prefix("Slowdown Sweep:") {
            measured_filter_assign(&mut slowdown_sweep, val);
        } else if let Some(val) = line.strip_prefix("Measured Ceiling:") {
            measured_filter_assign(&mut measured_ceiling, val);
        } else if let Some(val) = line.strip_prefix("Recommended Filter:") {
            recommended_filter = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Fail Reasons:") {
            fail_reasons = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("JSON:") {
            measured_filter_assign(&mut json_path, val);
        }
    }

    let full_json = full_json_override.or_else(|| {
        let p = json_path.as_deref()?;
        if p.starts_with('{') {
            Some(p.to_string())
        } else {
            std::fs::read_to_string(p).ok()
        }
    });

    let passed = exit_success && header_pass.unwrap_or(false) && fail_reasons.is_none();
    let status_label = if passed {
        "PASS".to_string()
    } else if !exit_success {
        "FAILED".to_string()
    } else {
        "FAIL".to_string()
    };

    let raw_output = match (stdout.trim(), stderr.trim()) {
        ("", "") => "No output captured from --selftest.".to_string(),
        (out, "") => out.to_string(),
        ("", err) => format!("stderr:\n{err}"),
        (out, err) => format!("{out}\n\n--- stderr ---\n{err}"),
    };

    PerfCheckReport {
        passed,
        exit_success,
        elapsed_ms,
        status_label,
        build_info,
        enumerated_adapters,
        chosen_adapter,
        calibration,
        headless,
        paced,
        fixed_gpu_wait,
        slowdown_sweep,
        measured_ceiling,
        recommended_filter,
        fail_reasons,
        json_path,
        full_json,
        raw_output,
        error_message: None,
    }
}

fn measured_filter_assign(slot: &mut Option<String>, val: &str) {
    let trimmed = val.trim();
    if !trimmed.is_empty() {
        *slot = Some(trimmed.to_string());
    }
}

fn spawn_selftest_child(exe_path: PathBuf) -> (Arc<AtomicBool>, Receiver<PerfCheckReport>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let started = Instant::now();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_perf_dir =
            std::env::temp_dir().join(format!("rustboy_perf_check_{}_{nonce}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_perf_dir);

        let mut cmd = Command::new(&exe_path);
        cmd.arg("--selftest")
            .arg("--perf-dir")
            .arg(&temp_perf_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&temp_perf_dir);
                let _ = tx.send(PerfCheckReport {
                    passed: false,
                    exit_success: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    status_label: "ERROR".to_string(),
                    build_info: None,
                    enumerated_adapters: Vec::new(),
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    fixed_gpu_wait: None,
                    slowdown_sweep: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
                    json_path: None,
                    full_json: None,
                    raw_output: format!("Failed to start performance check: {err}"),
                    error_message: Some(format!("Failed to launch --selftest: {err}")),
                });
                return;
            }
        };

        let stdout_handle = child.stdout.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = pipe.read_to_string(&mut buf);
                buf
            })
        });
        let stderr_handle = child.stderr.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = pipe.read_to_string(&mut buf);
                buf
            })
        });

        let mut exit_success = false;
        let mut abort_reason: Option<String> = None;

        loop {
            if cancel_worker.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                abort_reason = Some("Performance check cancelled.".to_string());
                break;
            }
            if started.elapsed() >= SELFTEST_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                abort_reason = Some(format!(
                    "Performance check timed out after {} seconds.",
                    SELFTEST_TIMEOUT.as_secs()
                ));
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_success = status.success();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    abort_reason = Some(format!("Failed waiting for child process: {err}"));
                    break;
                }
            }
        }

        let stdout = stdout_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stderr = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let full_json = std::fs::read_dir(&temp_perf_dir).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                })
                .and_then(|e| std::fs::read_to_string(e.path()).ok())
        });
        let _ = std::fs::remove_dir_all(&temp_perf_dir);

        let mut report =
            parse_selftest_report_with_json(&stdout, &stderr, full_json, exit_success, elapsed_ms);
        if let Some(reason) = abort_reason {
            report.passed = false;
            report.exit_success = false;
            report.status_label = if cancel_worker.load(Ordering::Relaxed) {
                "CANCELLED".to_string()
            } else {
                "TIMEOUT".to_string()
            };
            report.error_message = Some(reason);
        }

        let _ = tx.send(report);
    });

    (cancel, rx)
}

impl RustboyApp {
    /// Opens a directory in the OS file manager, creating it if needed.
    pub(crate) fn open_directory_in_file_manager(&mut self, dir: &std::path::Path) {
        if let Err(err) = std::fs::create_dir_all(dir) {
            self.notify_error(&format!("Failed to prepare folder: {err}"));
            return;
        }

        let mut cmd;
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd = Command::new("explorer");
            cmd.arg(dir).creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(target_os = "macos")]
        {
            cmd = Command::new("open");
            cmd.arg(dir);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            cmd = Command::new("xdg-open");
            cmd.arg(dir);
        }

        match cmd.spawn() {
            Ok(_) => self.notify(&format!("Opened folder: {}", dir.display())),
            Err(err) => self.notify_error(&format!("Could not open folder: {err}")),
        }
    }

    /// Applies mute and master volume config to audio sink and active GBA APU.
    pub(crate) fn sync_audio_volume(&mut self) {
        self.sink.audio.set_muted(self.config.muted);
        if let Some(ref mut core) = self.core {
            core.bus.apu.master_volume_pct = self.config.master_volume;
        }
    }

    /// Opens the configuration, saves, states, and crash/perf logs directory in the file manager.
    pub(crate) fn open_config_and_logs_folder(&mut self) {
        let dir = AppConfig::app_data_dir();
        self.open_directory_in_file_manager(&dir);
    }

    /// Spawns `--selftest` in a child process and opens the Performance Check modal.
    pub(crate) fn start_performance_check(&mut self) {
        self.cancel_perf_check();
        self.show_perf_check = true;
        match std::env::current_exe() {
            Ok(exe_path) => {
                let (cancel, rx) = spawn_selftest_child(exe_path);
                self.perf_check = PerfCheckState::Running {
                    started_at: Instant::now(),
                    cancel,
                    rx,
                };
            }
            Err(err) => {
                self.perf_check = PerfCheckState::Finished(Box::new(PerfCheckReport {
                    passed: false,
                    exit_success: false,
                    elapsed_ms: 0,
                    status_label: "ERROR".to_string(),
                    build_info: None,
                    enumerated_adapters: Vec::new(),
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    fixed_gpu_wait: None,
                    slowdown_sweep: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
                    json_path: None,
                    full_json: None,
                    raw_output: format!("Failed to locate executable: {err}"),
                    error_message: Some(format!("Failed to locate executable: {err}")),
                }));
            }
        }
    }

    /// Signals any active `--selftest` child process to terminate cleanly.
    pub(crate) fn cancel_perf_check(&mut self) {
        if let PerfCheckState::Running { cancel, .. } = &self.perf_check {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Polls the background performance-check channel and wakes the UI while running.
    pub(crate) fn poll_perf_check(&mut self, ctx: &egui::Context) {
        let finished_report = match &self.perf_check {
            PerfCheckState::Running { rx, .. } => match rx.try_recv() {
                Ok(report) => Some(report),
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    None
                }
                Err(TryRecvError::Disconnected) => Some(PerfCheckReport {
                    passed: false,
                    exit_success: false,
                    elapsed_ms: 0,
                    status_label: "ERROR".to_string(),
                    build_info: None,
                    enumerated_adapters: Vec::new(),
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    fixed_gpu_wait: None,
                    slowdown_sweep: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
                    json_path: None,
                    full_json: None,
                    raw_output: "Performance check worker terminated unexpectedly.".to_string(),
                    error_message: Some(
                        "Performance check worker terminated unexpectedly.".to_string(),
                    ),
                }),
            },
            _ => None,
        };

        if let Some(report) = finished_report {
            self.perf_check = PerfCheckState::Finished(Box::new(report));
            ctx.request_repaint();
        }
    }

    /// Closes the top-most open UI popup or dialog on Esc. Returns true if one was closed.
    pub(crate) fn close_top_dialog_on_escape(&mut self, ctx: &egui::Context) -> bool {
        if self.pending_confirm_action.take().is_some() {
            return true;
        }
        if self.show_volume_popup {
            self.show_volume_popup = false;
            return true;
        }

        // Check active egui Middle-layer order from top to bottom so clicked/focused windows win.
        let mut middle_layers: Vec<egui::Id> = ctx.memory(|m| {
            m.layer_ids()
                .filter(|layer| layer.order == egui::Order::Middle)
                .map(|layer| layer.id)
                .collect()
        });
        middle_layers.reverse();

        let mut windows = self.dialog_entries_mut();
        for layer_id in middle_layers {
            if let Some((title, open)) = windows
                .iter_mut()
                .find(|(title, is_open)| **is_open && egui::Id::new(*title) == layer_id)
            {
                let closed_title = *title;
                **open = false;
                if closed_title == "Performance Check" {
                    self.cancel_perf_check();
                }
                return true;
            }
        }

        // Fallback in reverse draw order when no layer order is recorded yet (e.g. first frame).
        for (title, open) in windows.into_iter().rev() {
            if *open {
                *open = false;
                if title == "Performance Check" {
                    self.cancel_perf_check();
                }
                return true;
            }
        }

        false
    }

    fn dialog_entries_mut(&mut self) -> Vec<(&'static str, &mut bool)> {
        vec![
            // Earlier drawn -> bottom-most in fallback order
            ("Visual Save State Manager", &mut self.show_state_manager),
            ("Rewind Settings", &mut self.show_rewind_settings),
            ("Controls & Keybindings", &mut self.show_controls_dialog),
            ("Per-Game Settings", &mut self.show_game_settings),
            ("Input Movies (TAS)", &mut self.show_movies),
            ("GBA Link Cable / Netplay", &mut self.link.show_window),
            ("Lua Scripting Console", &mut self.lua.show_console),
            ("RetroAchievements", &mut self.ra.show_window),
            ("ARM7TDMI CPU & Disassembler", &mut self.debugger.show_cpu),
            ("Memory Hex Viewer", &mut self.debugger.show_hex),
            ("Palette Viewer (PRAM)", &mut self.debugger.show_palette),
            ("VRAM Tile Viewer", &mut self.debugger.show_vram),
            ("BG Map Viewer", &mut self.debugger.show_bg),
            ("OAM Sprite Viewer", &mut self.debugger.show_oam),
            ("Audio Channel Visualizer", &mut self.debugger.show_audio),
            (
                "Hardware & Game Overrides",
                &mut self.debugger.show_overrides,
            ),
            ("Cheat Code Manager", &mut self.debugger.show_cheats),
            ("Controls Cheat-Sheet", &mut self.debugger.show_controls),
            ("About Rustboy Advance", &mut self.debugger.show_about),
            (
                "I/O Register Inspector & Change Log",
                &mut self.debugger.show_io_regs,
            ),
            ("GDB Remote Server", &mut self.debugger.show_gdb),
            (
                "Memory Search / Cheat Finder",
                &mut self.debugger.mem_search.open,
            ),
            ("Settings", &mut self.show_settings),
            ("Performance Check", &mut self.show_perf_check),
        ]
    }

    /// Renders the toolbar Volume & Mute quick popup near the Volume toolbar button.
    pub(crate) fn draw_volume_popup(&mut self, ctx: &egui::Context) {
        if !self.show_volume_popup {
            return;
        }

        let screen = ctx.screen_rect();
        let popup_w = 176.0_f32;
        let default_pos = self
            .volume_popup_anchor
            .map(|p| {
                egui::pos2(
                    (p.x - 64.0).clamp(8.0, (screen.max.x - (popup_w + 20.0)).max(8.0)),
                    (p.y + 4.0).clamp(8.0, (screen.max.y - 110.0).max(8.0)),
                )
            })
            .unwrap_or_else(|| egui::pos2((screen.max.x - (popup_w + 28.0)).max(8.0), 60.0));

        let mute_key = self.config.keybindings.hotkeys.display(HotkeyAction::Mute);
        let resp = egui::Area::new(egui::Id::new("toolbar_volume_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(default_pos)
            .show(ctx, |ui| {
                ui.set_max_width(popup_w + 20.0);
                egui::Frame::window(ui.style())
                    .fill(COLOR_TILE)
                    .stroke(STROKE_BORDER)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(popup_w);
                        ui.set_max_width(popup_w);
                        ui.horizontal(|ui| {
                            ui.strong("Sound");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(mute_key)
                                            .small()
                                            .color(COLOR_TEXT_SECONDARY),
                                    );
                                },
                            );
                        });
                        ui.separator();
                        if ui.checkbox(&mut self.config.muted, "Mute audio").changed() {
                            self.sync_audio_volume();
                            self.config.save();
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().slider_width = 96.0;
                            let slider = crate::theme::add_slider(
                                ui,
                                !self.config.muted,
                                egui::Slider::new(&mut self.config.master_volume, 0..=100)
                                    .suffix("%"),
                            );
                            if slider.changed() {
                                if self.config.master_volume > 0 && self.config.muted {
                                    self.config.muted = false;
                                }
                                self.sync_audio_volume();
                                self.config.save();
                            }
                        });
                    });
            });

        if ctx.input(|i| i.pointer.any_pressed())
            && !resp.response.rect.contains(
                ctx.input(|i| i.pointer.interact_pos())
                    .unwrap_or(egui::Pos2::ZERO),
            )
            && !self.volume_button_rect.is_some_and(|r| {
                r.contains(
                    ctx.input(|i| i.pointer.interact_pos())
                        .unwrap_or(egui::Pos2::ZERO),
                )
            })
        {
            self.show_volume_popup = false;
        }
    }

    /// Renders the centralized Settings modal with the Developer tools gate.
    pub(crate) fn draw_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }

        let mut open = self.show_settings;
        let mut config_dirty = false;
        let mut open_controls = false;
        let mut open_rewind = false;
        let mut open_folder = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .default_size([440.0, 390.0])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.strong("Interface");
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(COLOR_TILE)
                    .stroke(STROKE_BORDER)
                    .corner_radius(2)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if ui
                            .checkbox(&mut self.config.show_menu_bar, "Show menu bar")
                            .changed()
                        {
                            config_dirty = true;
                        }
                        ui.label(
                            egui::RichText::new("Tip: Press Ctrl+Shift+M to show or hide the menu bar anytime.")
                                .small()
                                .color(COLOR_TEXT_SECONDARY),
                        );
                        if ui
                            .checkbox(&mut self.config.show_toolbar, "Show toolbar")
                            .changed()
                        {
                            config_dirty = true;
                        }
                        if ui
                            .checkbox(&mut self.config.show_status_bar, "Show status bar")
                            .changed()
                        {
                            config_dirty = true;
                        }
                        if ui
                            .checkbox(
                                &mut self.config.pause_overlay_enabled,
                                "Show pause menu when pressing Esc in-game",
                            )
                            .changed()
                        {
                            config_dirty = true;
                        }
                        if ui
                            .checkbox(
                                &mut self.config.auto_download_covers,
                                "Automatically download game box art",
                            )
                            .changed()
                        {
                            config_dirty = true;
                        }
                    });

                ui.add_space(8.0);
                ui.strong("Sound & Controls");
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(COLOR_TILE)
                    .stroke(STROKE_BORDER)
                    .corner_radius(2)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut self.config.muted, "Mute audio").changed() {
                                self.sync_audio_volume();
                                config_dirty = true;
                            }
                            ui.add_space(8.0);
                            if crate::theme::add_slider(
                                ui,
                                !self.config.muted,
                                egui::Slider::new(&mut self.config.master_volume, 0..=100)
                                    .text("Volume")
                                    .suffix("%"),
                            )
                            .changed()
                            {
                                self.sync_audio_volume();
                                config_dirty = true;
                            }
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("Controls & Keybindings...").clicked() {
                                open_controls = true;
                            }
                            if ui.button("Rewind Settings...").clicked() {
                                open_rewind = true;
                            }
                        });
                    });

                ui.add_space(8.0);
                ui.strong("Advanced");
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(COLOR_TILE)
                    .stroke(STROKE_BORDER)
                    .corner_radius(2)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if ui
                            .checkbox(&mut self.config.developer_tools, "Developer tools")
                            .changed()
                        {
                            config_dirty = true;
                        }
                        ui.label(
                            egui::RichText::new(
                                "Unlocks CPU, Hex, VRAM, Palette, BG/OAM, I/O Inspector, Audio Visualizer, GDB, Lua, Input Movies, Memory Search, Custom BIOS, Soft Reset, and technical status details (kHz, DRC, BIOS).",
                            )
                            .small()
                            .color(COLOR_TEXT_SECONDARY),
                        );
                        ui.add_space(6.0);
                        if ui.button("Open Config & Logs Folder").clicked() {
                            open_folder = true;
                        }
                    });
            });

        self.show_settings = open;
        if config_dirty {
            self.config.save();
        }
        if open_controls {
            self.show_controls_dialog = true;
        }
        if open_rewind {
            self.show_rewind_settings = true;
        }
        if open_folder {
            self.open_config_and_logs_folder();
        }
    }

    /// Renders the Help -> Run performance check dialog with Copy and Save buttons.
    pub(crate) fn draw_perf_check_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_perf_check {
            return;
        }

        let mut open = self.show_perf_check;
        let mut close_clicked = false;
        let mut cancel_clicked = false;
        let mut rerun_clicked = false;
        let mut copy_text_payload: Option<String> = None;
        let mut save_text_payload: Option<String> = None;

        egui::Window::new("Performance Check")
            .open(&mut open)
            .default_size([540.0, 420.0])
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| match &self.perf_check {
                PerfCheckState::Idle => {
                    ui.label("Run the built-in portability and timing check (--selftest).");
                    if ui.button("Start Performance Check").clicked() {
                        rerun_clicked = true;
                    }
                }
                PerfCheckState::Running { started_at, .. } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.strong("Running performance check (--selftest)...");
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Elapsed: {:.1}s — benchmarking GPU adapter and 600-frame CPU/pacing loops.",
                            started_at.elapsed().as_secs_f32()
                        ))
                        .color(COLOR_TEXT_SECONDARY),
                    );
                    ui.add_space(10.0);
                    if ui.button("Cancel").clicked() {
                        cancel_clicked = true;
                    }
                }
                PerfCheckState::Finished(report) => {
                    let verdict = report.verdict_headline();
                    let badge_color = if verdict == "Runs smoothly on this PC" {
                        ACCENT_TEAL
                    } else {
                        egui::Color32::from_rgb(230, 175, 85)
                    };
                    egui::Frame::new()
                        .fill(COLOR_TILE)
                        .stroke(STROKE_BORDER)
                        .corner_radius(2)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(verdict)
                                        .size(15.0)
                                        .strong()
                                        .color(badge_color),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} · {:.2}s",
                                                report.status_label,
                                                report.elapsed_ms as f32 / 1000.0
                                            ))
                                            .small()
                                            .color(COLOR_TEXT_SECONDARY),
                                        );
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(report.plain_summary_line()).color(COLOR_TEXT),
                            );

                            if let Some(ref err) = report.error_message {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(err)
                                        .color(egui::Color32::from_rgb(220, 95, 95)),
                                );
                            }
                        });

                    ui.add_space(8.0);
                    egui::CollapsingHeader::new("Details")
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(COLOR_TILE)
                                .stroke(STROKE_BORDER)
                                .corner_radius(2)
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    let adapters_joined = if report.enumerated_adapters.is_empty() {
                                        None
                                    } else {
                                        Some(report.enumerated_adapters.join("\n"))
                                    };
                                    egui::Grid::new("perf_check_summary_grid")
                                        .num_columns(2)
                                        .spacing([12.0, 4.0])
                                        .show(ui, |ui| {
                                            let rows: [(&str, Option<&str>); 11] = [
                                                ("Build", report.build_info.as_deref()),
                                                ("Adapter", report.chosen_adapter.as_deref()),
                                                ("Enumerated GPUs", adapters_joined.as_deref()),
                                                ("Calibration", report.calibration.as_deref()),
                                                ("Headless", report.headless.as_deref()),
                                                ("Paced", report.paced.as_deref()),
                                                ("Fixed GPU Wait", report.fixed_gpu_wait.as_deref()),
                                                ("Slowdown Sweep", report.slowdown_sweep.as_deref()),
                                                ("Measured Ceiling", report.measured_ceiling.as_deref()),
                                                ("Recommended Filter", report.recommended_filter.as_deref()),
                                                ("Issues", report.fail_reasons.as_deref()),
                                            ];
                                            for (label, value) in rows {
                                                if let Some(val) = value {
                                                    ui.label(
                                                        egui::RichText::new(label)
                                                            .color(COLOR_TEXT_SECONDARY),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(val).color(COLOR_TEXT),
                                                    );
                                                    ui.end_row();
                                                }
                                            }
                                        });
                                });

                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new("Full --selftest Output")
                                    .small()
                                    .color(COLOR_TEXT_SECONDARY),
                            );
                            egui::Frame::new()
                                .fill(COLOR_TILE)
                                .stroke(STROKE_BORDER)
                                .corner_radius(2)
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .max_height(150.0)
                                        .show(ui, |ui| {
                                            ui.add(
                                                egui::TextEdit::multiline(
                                                    &mut report.raw_output.as_str(),
                                                )
                                                .font(egui::TextStyle::Monospace)
                                                .desired_width(f32::INFINITY)
                                                .frame(false),
                                            );
                                        });
                                });
                        });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let json_payload = report.export_json();
                        if ui
                            .button("Copy Report")
                            .on_hover_text("Copy the full JSON performance report to the clipboard")
                            .clicked()
                        {
                            copy_text_payload = Some(json_payload.clone());
                        }
                        if ui
                            .button("Save Report...")
                            .on_hover_text("Save the full JSON performance report to a file")
                            .clicked()
                        {
                            save_text_payload = Some(json_payload);
                        }
                        if ui.button("Run Again").clicked() {
                            rerun_clicked = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Close").clicked() {
                                close_clicked = true;
                            }
                        });
                    });
                }
            });

        if close_clicked {
            open = false;
        }
        self.show_perf_check = open;
        if !open || cancel_clicked {
            self.cancel_perf_check();
        }
        if let Some(text) = copy_text_payload {
            ctx.copy_text(text);
            self.notify("Full JSON performance report copied to clipboard");
        }
        if let Some(text) = save_text_payload
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("rustboy-selftest-report.json")
                .add_filter("JSON Report", &["json", "txt"])
                .save_file()
        {
            match std::fs::write(&path, text) {
                Ok(()) => self.notify(&format!("Saved report: {}", path.display())),
                Err(err) => self.notify_error(&format!("Failed to save report: {err}")),
            }
        }
        if rerun_clicked {
            self.start_performance_check();
        }
    }

    /// Confirms before Restart game or Stop when unsaved in-game progress exists.
    pub(crate) fn draw_confirm_action_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_confirm_action else {
            return;
        };
        if !self.has_active_game() {
            self.pending_confirm_action = None;
            return;
        }

        let (title, confirm_label, body) = match action {
            PendingConfirmAction::HardReset => (
                "Restart game?",
                "Restart game",
                "You may lose progress since your last save. Restart anyway?".to_string(),
            ),
            PendingConfirmAction::SoftReset => (
                "Soft reset game?",
                "Soft Reset",
                "You may lose progress since your last save. Restart anyway?".to_string(),
            ),
            PendingConfirmAction::CloseRom => (
                "Return to Library?",
                "Return to Library",
                "You may lose progress since your last save. Return to Library anyway?".to_string(),
            ),
        };

        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;

        egui::Window::new("Unsaved Progress")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .fixed_size([410.0, 155.0])
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .size(14.0)
                        .color(COLOR_TEXT),
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new(body).color(COLOR_TEXT_SECONDARY));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(confirm_label)
                                    .color(egui::Color32::from_rgb(255, 130, 130)),
                            )
                            .stroke(STROKE_BORDER),
                        )
                        .clicked()
                    {
                        confirmed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
            });

        if confirmed {
            self.pending_confirm_action = None;
            self.show_pause_overlay = false;
            self.paused = false;
            match action {
                PendingConfirmAction::HardReset => self.hard_reset(),
                PendingConfirmAction::SoftReset => self.soft_reset(),
                PendingConfirmAction::CloseRom => self.close_rom(ctx),
            }
        } else if !open || cancelled {
            self.pending_confirm_action = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_selftest_report;
    use crate::app::{PendingConfirmAction, RustboyApp};
    use crate::headless::CliConfig;
    use std::mem::ManuallyDrop;

    #[test]
    fn parses_selftest_pass_report_fields() {
        let sample = "\
=== RUSTBOY ADVANCE PORTABLE --SELFTEST (PASS) ===\n\
  Build:             commit=072e4078c7b3 | built=2026-11-14T09:22:04Z | exe_sha256=9a11a338ca98028ee0dfd44d542f71e326ea49890979ad658938b201fbb98fb4\n\
  Host:              AMD Ryzen 9 8945HX with Radeon Graphics (16c/32t) | RAM=32560 MB | OS=Windows 11 (10.0.26300)\n\
  Embedded ROM:      embedded:STRESS_SELF (Mode0 4xBG + 128 Sprites + Alpha)\n\
  Enumerated GPUs:   2 adapter(s) visible to wgpu\n\
    [1] NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan / DiscreteGpu) — driver: NVIDIA 616.92\n\
    [2] Microsoft Basic Render Driver (Dx12 / Cpu) — driver: 10.0.26100.9818\n\
  Chosen Adapter:    NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan / DiscreteGpu) [gpu-shader]\n\
  Calibration:       75.527 ms (ref 75.769 ms, ratio vs ref = 1.00x)\n\
  Headless (600f):   998.1 FPS | headroom = 16.71x | emu_avg=0.836 ms | total avg=1.002 p95=1.317 worst=2.627 ms\n\
  Paced (600f +120w): work avg=1.042 p95=1.368 p99.5=1.494 worst=2.031 ms | over_budget=0 (0.000%)\n\
  Slowdown Sweep:    [1x(p99.5=1.91ms,PASS), 8x(p99.5=12.05ms,PASS), 12x(p99.5=18.28ms,FAIL)]\n\
  Measured Ceiling:  smooth (<0.5% over, 0 underruns) = 8x | zero-over-budget = 8x\n\
  Recommended Filter: smooth-bilinear — Hardware GPU shader path provides clean interpolation\n\
  JSON:              perf/selftest-20261114-094156.json\n";

        let full_json = serde_json::json!({
            "schema_version": 3,
            "exe_sha256": "9a11a338ca98028ee0dfd44d542f71e326ea49890979ad658938b201fbb98fb4",
            "git_commit": "072e4078c7b3",
            "build_time_utc": "2026-11-14T09:22:04Z",
            "calibration_ratio_vs_ref": 1.0032,
            "available_adapters": [
                { "name": "NVIDIA GeForce RTX 5070 Laptop GPU", "backend": "Vulkan" }
            ],
            "slowdown_sweep": [
                { "slowdown_x": 8, "smooth_passed": true }
            ],
            "measured_smooth_ceiling_x": 8,
            "measured_zero_over_budget_x": 8,
            "recommended_filter": "smooth-bilinear",
            "passed": true
        })
        .to_string();

        let report =
            super::parse_selftest_report_with_json(sample, "", Some(full_json), true, 12450);
        assert!(report.passed);
        assert_eq!(report.status_label, "PASS");
        assert_eq!(report.verdict_headline(), "Runs smoothly on this PC");
        assert_eq!(
            report.chosen_adapter.as_deref(),
            Some("NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan / DiscreteGpu) [gpu-shader]")
        );
        assert_eq!(report.enumerated_adapters.len(), 2);
        assert_eq!(
            report.measured_ceiling.as_deref(),
            Some("smooth (<0.5% over, 0 underruns) = 8x | zero-over-budget = 8x")
        );
        assert!(
            report
                .recommended_filter
                .as_deref()
                .unwrap()
                .starts_with("smooth-bilinear")
        );
        assert!(report.fail_reasons.is_none());

        let exported = report.export_json();
        for key in [
            "exe_sha256",
            "git_commit",
            "build_time_utc",
            "calibration_ratio_vs_ref",
            "available_adapters",
            "slowdown_sweep",
            "measured_smooth_ceiling_x",
            "measured_zero_over_budget_x",
            "recommended_filter",
        ] {
            assert!(exported.contains(key), "exported JSON missing `{key}`");
        }
    }

    #[test]
    fn parses_selftest_fail_report_with_reasons() {
        let sample = "\
=== RUSTBOY ADVANCE PORTABLE --SELFTEST (FAIL) ===\n\
  Chosen Adapter:    Basic Render Driver (Software)\n\
  Recommended Filter: sharp1x — Step down for tighter frame budgets\n\
  Fail Reasons:      no hardware adapter; missed_frames=14\n";

        let report = parse_selftest_report(sample, "warn: slow path", false, 8200);
        assert!(!report.passed);
        assert_eq!(report.status_label, "FAILED");
        assert_eq!(report.verdict_headline(), "May stutter: try Sharp Pixels");
        assert_eq!(
            report.fail_reasons.as_deref(),
            Some("no hardware adapter; missed_frames=14")
        );
        assert!(report.raw_output.contains("--- stderr ---"));
        assert!(report.raw_output.contains("warn: slow path"));
    }

    #[test]
    fn esc_closes_top_active_dialog_by_layer_order() {
        let mut app = ManuallyDrop::new(RustboyApp::new(CliConfig::default()));
        let ctx = egui::Context::default();

        app.show_settings = true;
        app.debugger.show_about = true;

        // Render 2 frames so egui records Middle layer IDs for both windows,
        // with "Settings" explicitly brought to front.
        for frame in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::Window::new("About Rustboy Advance").show(ctx, |ui| {
                    ui.label("about");
                });
                egui::Window::new("Settings").show(ctx, |ui| {
                    ui.label("settings");
                });
                if frame == 1 {
                    ctx.move_to_top(egui::LayerId::new(
                        egui::Order::Middle,
                        egui::Id::new("Settings"),
                    ));
                }
            });
        }

        // First Esc closes topmost ("Settings"), leaving About open.
        assert!(app.close_top_dialog_on_escape(&ctx));
        assert!(!app.show_settings);
        assert!(app.debugger.show_about);

        // Second Esc closes About.
        assert!(app.close_top_dialog_on_escape(&ctx));
        assert!(!app.debugger.show_about);

        // No dialogs open -> returns false so Esc can fall through to fullscreen/pause.
        assert!(!app.close_top_dialog_on_escape(&ctx));
    }

    fn make_unsaved_prompt_test_app(temp_dir: &std::path::Path) -> ManuallyDrop<RustboyApp> {
        let mut app = ManuallyDrop::new(RustboyApp::new(CliConfig::default()));
        app.core = Some(gba_core::GbaCore::new(None, vec![0u8; 256]));
        app.rom_title = "Summon Night".to_string();
        app.rom_code = "AB4E".to_string();
        app.rom_path = Some(temp_dir.join("summon_night.gba"));
        app.sav_path = None;
        app
    }

    #[test]
    fn unsaved_progress_fresh_boot_and_idle_title_screen_never_prompt() {
        let temp_dir = std::env::temp_dir().join("rba_test_unsaved_fresh_boot");
        let mut app = make_unsaved_prompt_test_app(&temp_dir);
        let ctx = egui::Context::default();

        // Fresh boot at frame 0 -> no prompt.
        app.request_hard_reset();
        assert!(app.pending_confirm_action.is_none());
        app.request_close_rom(&ctx);
        assert!(app.pending_confirm_action.is_none());

        // Simulating 2 minutes (7200 frames) idling on title screen with NO player input (0x03FF)
        // must still NOT trigger the unsaved-progress confirmation dialog.
        app.sink.keypad = 0x03FF;
        for _ in 0..7200 {
            app.track_frame_unsaved_progress(0x03FF, false);
        }
        assert!(!app.has_input_since_save);
        assert!(!app.has_unsaved_progress());
        app.request_hard_reset();
        assert!(app.pending_confirm_action.is_none());
        app.request_close_rom(&ctx);
        assert!(app.pending_confirm_action.is_none());
    }

    #[test]
    fn unsaved_progress_two_minutes_of_play_with_input_prompts_until_save_write() {
        let temp_dir = std::env::temp_dir().join("rba_test_unsaved_two_minutes");
        let mut app = make_unsaved_prompt_test_app(&temp_dir);
        let ctx = egui::Context::default();

        // Player presses Start once and plays for 2 minutes (7200 frames).
        app.track_frame_unsaved_progress(0x03F7, false);
        for _ in 1..7200 {
            app.track_frame_unsaved_progress(0x03FF, false);
        }
        assert!(app.has_unsaved_progress());

        app.request_hard_reset();
        assert_eq!(
            app.pending_confirm_action,
            Some(PendingConfirmAction::HardReset)
        );
        assert!(app.close_top_dialog_on_escape(&ctx));
        assert!(app.pending_confirm_action.is_none());

        app.request_close_rom(&ctx);
        assert_eq!(
            app.pending_confirm_action,
            Some(PendingConfirmAction::CloseRom)
        );
        assert!(app.close_top_dialog_on_escape(&ctx));

        // An in-game save-memory write clears the unsaved-play timer.
        app.core.as_mut().unwrap().bus.cart.save_dirty = true;
        app.track_frame_unsaved_progress(0x03FF, false);
        assert!(!app.has_unsaved_progress());
        app.request_hard_reset();
        assert!(app.pending_confirm_action.is_none());
    }

    #[test]
    fn unsaved_progress_clears_after_save_state() {
        let temp_dir = std::env::temp_dir().join("rba_test_unsaved_after_state");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let rom_file = temp_dir.join("summon_night.gba");
        std::fs::write(&rom_file, vec![0u8; 256]).unwrap();

        let mut app = make_unsaved_prompt_test_app(&temp_dir);
        let ctx = egui::Context::default();

        app.track_frame_unsaved_progress(0x03FE, false);
        for _ in 1..7200 {
            app.track_frame_unsaved_progress(0x03FF, false);
        }
        assert!(app.has_unsaved_progress());

        // Saving a state checkpoints progress -> no prompt afterwards.
        app.quick_save(1);
        assert!(!app.has_unsaved_progress());
        app.request_hard_reset();
        assert!(app.pending_confirm_action.is_none());
        app.request_close_rom(&ctx);
        assert!(app.pending_confirm_action.is_none());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
