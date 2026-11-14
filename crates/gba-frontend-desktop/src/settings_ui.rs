use crate::app::RustboyApp;
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
    pub(crate) chosen_adapter: Option<String>,
    pub(crate) calibration: Option<String>,
    pub(crate) headless: Option<String>,
    pub(crate) paced: Option<String>,
    pub(crate) measured_ceiling: Option<String>,
    pub(crate) recommended_filter: Option<String>,
    pub(crate) fail_reasons: Option<String>,
    pub(crate) raw_output: String,
    pub(crate) error_message: Option<String>,
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
pub(crate) fn parse_selftest_report(
    stdout: &str,
    stderr: &str,
    exit_success: bool,
    elapsed_ms: u64,
) -> PerfCheckReport {
    let mut header_pass = None;
    let mut chosen_adapter = None;
    let mut calibration = None;
    let mut headless = None;
    let mut paced = None;
    let mut measured_ceiling = None;
    let mut recommended_filter = None;
    let mut fail_reasons = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.contains("--SELFTEST (PASS)") {
            header_pass = Some(true);
        } else if line.contains("--SELFTEST (FAIL)") {
            header_pass = Some(false);
        } else if let Some(val) = line.strip_prefix("Chosen Adapter:") {
            chosen_adapter = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Calibration:") {
            calibration = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Headless (600f):") {
            headless = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Paced (600f +120w):") {
            paced = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Measured Ceiling:") {
            measured_filter_assign(&mut measured_ceiling, val);
        } else if let Some(val) = line.strip_prefix("Recommended Filter:") {
            recommended_filter = Some(val.trim().to_string());
        } else if let Some(val) = line.strip_prefix("Fail Reasons:") {
            fail_reasons = Some(val.trim().to_string());
        }
    }

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
        chosen_adapter,
        calibration,
        headless,
        paced,
        measured_ceiling,
        recommended_filter,
        fail_reasons,
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
        let mut cmd = Command::new(&exe_path);
        cmd.arg("--selftest")
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
                let _ = tx.send(PerfCheckReport {
                    passed: false,
                    exit_success: false,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    status_label: "ERROR".to_string(),
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
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

        let mut report = parse_selftest_report(&stdout, &stderr, exit_success, elapsed_ms);
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
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
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
                    chosen_adapter: None,
                    calibration: None,
                    headless: None,
                    paced: None,
                    measured_ceiling: None,
                    recommended_filter: None,
                    fail_reasons: None,
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
        let default_pos = self
            .volume_popup_anchor
            .map(|p| {
                egui::pos2(
                    (p.x - 90.0).clamp(8.0, (screen.max.x - 230.0).max(8.0)),
                    (p.y + 4.0).clamp(8.0, (screen.max.y - 120.0).max(8.0)),
                )
            })
            .unwrap_or_else(|| egui::pos2((screen.max.x - 240.0).max(8.0), 60.0));

        let mute_key = self.config.keybindings.hotkeys.display(HotkeyAction::Mute);
        let resp = egui::Area::new(egui::Id::new("toolbar_volume_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(default_pos)
            .show(ctx, |ui| {
                egui::Frame::window(ui.style())
                    .fill(COLOR_TILE)
                    .stroke(STROKE_BORDER)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width(210.0);
                        ui.horizontal(|ui| {
                            ui.strong("Sound");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("Mute: {mute_key}"))
                                            .small()
                                            .color(COLOR_TEXT_SECONDARY),
                                    );
                                },
                            );
                        });
                        ui.separator();
                        if ui
                            .checkbox(&mut self.config.muted, "Mute audio")
                            .changed()
                        {
                            self.sync_audio_volume();
                            self.config.save();
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("Volume:");
                            let slider = ui.add_enabled(
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
            && !self
                .volume_button_rect
                .is_some_and(|r| r.contains(ctx.input(|i| i.pointer.interact_pos()).unwrap_or(egui::Pos2::ZERO)))
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
                            if ui
                                .add_enabled(
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
                    let badge_color = if report.passed {
                        ACCENT_TEAL
                    } else {
                        egui::Color32::from_rgb(220, 95, 95)
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
                                    egui::RichText::new(format!("Status: {}", report.status_label))
                                        .strong()
                                        .color(badge_color),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Completed in {:.2}s",
                                                report.elapsed_ms as f32 / 1000.0
                                            ))
                                            .small()
                                            .color(COLOR_TEXT_SECONDARY),
                                        );
                                    },
                                );
                            });

                            if let Some(ref err) = report.error_message {
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(err).color(badge_color));
                            }

                            ui.add_space(6.0);
                            egui::Grid::new("perf_check_summary_grid")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    let rows: [(&str, Option<&str>); 7] = [
                                        ("Adapter", report.chosen_adapter.as_deref()),
                                        ("Calibration", report.calibration.as_deref()),
                                        ("Headless", report.headless.as_deref()),
                                        ("Paced", report.paced.as_deref()),
                                        ("Ceiling", report.measured_ceiling.as_deref()),
                                        ("Recommended", report.recommended_filter.as_deref()),
                                        ("Issues", report.fail_reasons.as_deref()),
                                    ];
                                    for (label, value) in rows {
                                        if let Some(val) = value {
                                            ui.label(
                                                egui::RichText::new(label)
                                                    .color(COLOR_TEXT_SECONDARY),
                                            );
                                            ui.label(egui::RichText::new(val).color(COLOR_TEXT));
                                            ui.end_row();
                                        }
                                    }
                                });
                        });

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Full Report")
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
                                .max_height(170.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut report.raw_output.as_str())
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(f32::INFINITY)
                                            .frame(false),
                                    );
                                });
                        });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Copy Report").clicked() {
                            copy_text_payload = Some(report.raw_output.clone());
                        }
                        if ui.button("Save Report...").clicked() {
                            save_text_payload = Some(report.raw_output.clone());
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
            self.notify("Performance report copied to clipboard");
        }
        if let Some(text) = save_text_payload
            && let Some(path) = rfd::FileDialog::new()
                .set_file_name("rustboy-selftest-report.txt")
                .add_filter("Text Report", &["txt", "log"])
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
}

#[cfg(test)]
mod tests {
    use super::parse_selftest_report;
    use crate::app::RustboyApp;
    use crate::headless::CliConfig;
    use std::mem::ManuallyDrop;

    #[test]
    fn parses_selftest_pass_report_fields() {
        let sample = "\
=== RUSTBOY ADVANCE PORTABLE --SELFTEST (PASS) ===\n\
  Chosen Adapter:    NVIDIA GeForce RTX (Vulkan)\n\
  Calibration:       Present=0.45ms Blit=0.20ms Filter=1.10ms (xbr_fast=1.80ms)\n\
  Headless (600f):   1.20s -> 498.5 fps | Jitter p99=0.08ms | Underruns=0\n\
  Paced (600f +120w): 10.02s -> 59.88 fps | Missed=0\n\
  Measured Ceiling:  filter=LCD Matrix\n\
  Recommended Filter: LCD Matrix\n\
  JSON:              {\"passed\":true}\n";

        let report = parse_selftest_report(sample, "", true, 12450);
        assert!(report.passed);
        assert_eq!(report.status_label, "PASS");
        assert_eq!(
            report.chosen_adapter.as_deref(),
            Some("NVIDIA GeForce RTX (Vulkan)")
        );
        assert_eq!(report.recommended_filter.as_deref(), Some("LCD Matrix"));
        assert!(report.fail_reasons.is_none());
        assert!(report.raw_output.contains("\"passed\":true"));
    }

    #[test]
    fn parses_selftest_fail_report_with_reasons() {
        let sample = "\
=== RUSTBOY ADVANCE PORTABLE --SELFTEST (FAIL) ===\n\
  Chosen Adapter:    Basic Render Driver (Software)\n\
  Fail Reasons:      no hardware adapter; missed_frames=14\n";

        let report = parse_selftest_report(sample, "warn: slow path", false, 8200);
        assert!(!report.passed);
        assert_eq!(report.status_label, "FAILED");
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
}
