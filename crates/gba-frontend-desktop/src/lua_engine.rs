//! mGBA-inspired Lua 5.4 scripting engine powered by pure-Rust `piccolo`.

#![allow(dead_code)]

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    ptr::NonNull,
    rc::Rc,
};

use egui::{self, Color32, RichText, Stroke};
use gba_core::{
    GbaCore, KEY_A, KEY_B, KEY_DOWN, KEY_L, KEY_LEFT, KEY_R, KEY_RIGHT, KEY_SELECT, KEY_START,
    KEY_UP,
};
use piccolo::{
    Callback, CallbackReturn, Closure, Context, Executor, ExecutorMode, Fuel, Function,
    IntoValue, Lua, StashedFunction, Table, TypeError, Value,
};

/// Maximum console lines kept in the Lua console output ring buffer.
pub const MAX_CONSOLE_LINES: usize = 500;

/// Per-execution VM instruction fuel budget to stop runaway loops cleanly.
pub const DEFAULT_FUEL_LIMIT: i32 = 100_000;

/// Maximum overlay draw commands accepted in a single frame.
pub const MAX_OVERLAY_CMDS: usize = 256;

const KEY_MAP: [(&str, u16); 10] = [
    ("A", KEY_A),
    ("B", KEY_B),
    ("Select", KEY_SELECT),
    ("Start", KEY_START),
    ("Right", KEY_RIGHT),
    ("Left", KEY_LEFT),
    ("Up", KEY_UP),
    ("Down", KEY_DOWN),
    ("R", KEY_R),
    ("L", KEY_L),
];

/// Draw primitive queued by Lua `gui.*` calls in native GBA LCD coordinates (240x160).
#[derive(Clone, Debug, PartialEq)]
pub enum GuiDrawCmd {
    Text {
        x: f32,
        y: f32,
        text: String,
        color: [u8; 4],
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [u8; 4],
        fill: Option<[u8; 4]>,
    },
}

/// Built-in sample scripts selectable from the Lua Console toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleScript {
    MemoryMonitor,
    InputDisplayHud,
    AutoMashA,
}

impl SampleScript {
    pub const ALL: [Self; 3] = [Self::MemoryMonitor, Self::InputDisplayHud, Self::AutoMashA];

    pub fn label(self) -> &'static str {
        match self {
            Self::MemoryMonitor => "Memory Monitor",
            Self::InputDisplayHud => "Input Display HUD",
            Self::AutoMashA => "Auto-Mash A",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::MemoryMonitor => {
                r##"-- Memory Monitor: watches IWRAM & EWRAM addresses on screen
console.log("Loaded Memory Monitor for:", emu.romTitle(), emu.gameCode())

local prev_val = emu.read32(0x03000000)

callbacks.add("frame", function()
    local frame = emu.frameCount()
    local ewram0 = emu.read32(0x02000000)
    local iwram0 = emu.read32(0x03000000)

    if iwram0 ~= prev_val then
        console.log(string.format("Frame %d: IWRAM[0x03000000] = 0x%08X", frame, iwram0))
        prev_val = iwram0
    end

    gui.rect(2, 2, 134, 34, 0xCC12161E, 0xCC12161E)
    gui.rect(2, 2, 134, 34, "#00D2AF")
    gui.text(6, 5, string.format("EWRAM: 0x%08X", ewram0), "#E6EDF3")
    gui.text(6, 15, string.format("IWRAM: 0x%08X", iwram0), "#00D2AF")
    gui.text(6, 25, string.format("Frame: %d", frame), "#9BA3B4")
end)
"##
            }
            Self::InputDisplayHud => {
                r##"-- Input Display HUD: renders live GBA button states onto the viewport
console.log("Input Display HUD active")

local function draw_btn(x, y, w, h, label, active)
    local border = active and "#00D2AF" or "#475166"
    local fill = active and 0xB000D2AF or 0xA0141820
    local text_col = active and "#0E1117" or "#C4CBD8"
    gui.rect(x, y, w, h, border, fill)
    gui.text(x + 2, y + 1, label, text_col)
end

callbacks.add("frame", function()
    local k = joypad.get()
    draw_btn(4, 144, 12, 12, "L", k.L)
    draw_btn(18, 144, 12, 12, "<", k.Left)
    draw_btn(30, 136, 12, 12, "^", k.Up)
    draw_btn(30, 148, 12, 12, "v", k.Down)
    draw_btn(42, 144, 12, 12, ">", k.Right)
    draw_btn(92, 146, 24, 10, "SEL", k.Select)
    draw_btn(119, 146, 24, 10, "STA", k.Start)
    draw_btn(192, 144, 12, 12, "B", k.B)
    draw_btn(206, 140, 12, 12, "A", k.A)
    draw_btn(222, 144, 12, 12, "R", k.R)
end)
"##
            }
            Self::AutoMashA => {
                r##"-- Auto-Mash A: toggles A every other emulated frame
console.log("Auto-Mash A started")

callbacks.add("frame", function()
    local f = emu.frameCount()
    local press = (f % 2) == 0
    joypad.set({ A = press })

    gui.rect(4, 4, 86, 14, "#EAB308", 0xCC181B22)
    gui.text(8, 7, press and "AUTO A: ON" or "AUTO A: ..", press and "#FACC15" or "#9CA3AF")
end)
"##
            }
        }
    }
}

/// UI actions emitted by the Lua Console window back to `RustboyApp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaConsoleAction {
    Run,
    Stop,
}

#[derive(Default)]
struct LuaSharedState {
    console_log: VecDeque<String>,
    overlay_cmds: Vec<GuiDrawCmd>,
    joypad_set_mask: u16,
    joypad_clear_mask: u16,
    pressed_mask: u16,
    frame_count: u64,
    rom_title: String,
    game_code: String,
    hardcore_active: bool,
    frame_callbacks: Vec<StashedFunction>,
}

impl LuaSharedState {
    fn push_log(&mut self, line: String) {
        for segment in line.lines() {
            if self.console_log.len() >= MAX_CONSOLE_LINES {
                self.console_log.pop_front();
            }
            self.console_log.push_back(segment.to_string());
        }
        if line.is_empty() {
            if self.console_log.len() >= MAX_CONSOLE_LINES {
                self.console_log.pop_front();
            }
            self.console_log.push_back(String::new());
        }
    }
}

#[derive(Default)]
struct ScopedCoreSlot {
    core: Option<NonNull<GbaCore>>,
    in_use: bool,
}

thread_local! {
    static CORE_SCOPE: RefCell<ScopedCoreSlot> = const { RefCell::new(ScopedCoreSlot { core: None, in_use: false }) };
}

struct CoreScopeGuard;

impl CoreScopeGuard {
    fn enter(core: Option<&mut GbaCore>) -> Self {
        CORE_SCOPE.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.core.is_none() && !slot.in_use,
                "nested Lua host scopes are not permitted"
            );
            slot.core = core.map(NonNull::from);
        });
        Self
    }
}

impl Drop for CoreScopeGuard {
    fn drop(&mut self) {
        CORE_SCOPE.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.core = None;
            slot.in_use = false;
        });
    }
}

fn with_core_mut<R>(f: impl FnOnce(&mut GbaCore) -> R) -> Result<R, &'static str> {
    CORE_SCOPE.with(|slot| {
        let mut ptr = {
            let mut slot = slot.borrow_mut();
            if slot.in_use {
                return Err("re-entrant GBA core access from Lua is disallowed");
            }
            let ptr = slot.core.ok_or("no ROM is loaded")?;
            slot.in_use = true;
            ptr
        };
        // SAFETY: `CoreScopeGuard::enter` installs a valid `&mut GbaCore` pointer for the
        // synchronous duration of `Lua::enter`, `in_use` guarantees exclusive access without
        // nested borrows, and `CoreScopeGuard::drop` always clears the slot before returning.
        let out = f(unsafe { ptr.as_mut() });
        slot.borrow_mut().in_use = false;
        Ok(out)
    })
}

/// Stateful mGBA-inspired Lua scripting engine for Rustboy Advance.
pub struct LuaEngine {
    pub show_console: bool,
    pub script_source: String,
    pub loaded_script_path: Option<PathBuf>,
    pub selected_sample: SampleScript,
    pub running: bool,
    pub last_error: Option<String>,
    pub fuel_limit: i32,
    pub frames_executed: u64,
    lua: Lua,
    shared: Rc<RefCell<LuaSharedState>>,
}

impl Default for LuaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaEngine {
    pub fn new() -> Self {
        let selected_sample = SampleScript::MemoryMonitor;
        Self {
            show_console: false,
            script_source: selected_sample.source().to_string(),
            loaded_script_path: None,
            selected_sample,
            running: false,
            last_error: None,
            fuel_limit: DEFAULT_FUEL_LIMIT,
            frames_executed: 0,
            lua: Lua::core(),
            shared: Rc::new(RefCell::new(LuaSharedState::default())),
        }
    }

    /// Loads a `.lua` source file from disk into the inline editor and stops any running script.
    pub fn load_file(&mut self, path: &Path) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|err| format!("Failed to read Lua script {}: {err}", path.display()))?;
        self.stop_internal(false);
        self.script_source = content;
        self.loaded_script_path = Some(path.to_path_buf());
        self.last_error = None;
        self.shared
            .borrow_mut()
            .push_log(format!("[Lua] Loaded {}", path.display()));
        Ok(())
    }

    /// Populates the inline editor with one of the built-in sample scripts.
    pub fn load_sample(&mut self, sample: SampleScript) {
        self.stop_internal(false);
        self.selected_sample = sample;
        self.script_source = sample.source().to_string();
        self.loaded_script_path = None;
        self.last_error = None;
    }

    /// Compiles and executes the current `script_source` from a clean VM state.
    pub fn start_script(
        &mut self,
        core: Option<&mut GbaCore>,
        rom_title: &str,
        game_code: &str,
        frame_count: u64,
        pressed_mask: u16,
        hardcore: bool,
    ) -> Result<(), String> {
        self.stop_internal(false);
        self.last_error = None;
        self.frames_executed = 0;

        {
            let mut shared = self.shared.borrow_mut();
            shared.rom_title = rom_title.to_string();
            shared.game_code = game_code.to_string();
            shared.frame_count = frame_count;
            shared.pressed_mask = pressed_mask;
            shared.hardcore_active = hardcore;
        }

        // Always start from a clean VM so old globals/callbacks cannot leak across runs.
        self.lua = Lua::core();
        register_api(&mut self.lua, Rc::clone(&self.shared));

        let script_name = self
            .loaded_script_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("inline.lua")
            .to_string();
        let source = self.script_source.clone();
        let fuel_limit = self.fuel_limit.max(1);

        let result = {
            let _guard = CoreScopeGuard::enter(core);
            self.execute_chunk(&script_name, &source, fuel_limit)
        };

        match result {
            Ok(()) => {
                self.running = true;
                Ok(())
            }
            Err(err) => {
                self.halt_with_error(err.clone());
                Err(err)
            }
        }
    }

    /// Stops script execution and clears registered callbacks, overlays, and key overrides.
    pub fn stop(&mut self) {
        self.stop_internal(true);
    }

    fn stop_internal(&mut self, log_stop: bool) {
        let was_running = self.running;
        self.running = false;
        let mut shared = self.shared.borrow_mut();
        shared.frame_callbacks.clear();
        shared.overlay_cmds.clear();
        shared.joypad_set_mask = 0;
        shared.joypad_clear_mask = 0;
        if log_stop && was_running {
            shared.push_log("[Lua] Script stopped.".to_string());
        }
    }

    /// Clears console output log lines.
    pub fn clear_log(&mut self) {
        self.shared.borrow_mut().console_log.clear();
    }

    /// Updates ROM metadata when a ROM is opened or closed.
    pub fn on_rom_changed(&mut self, rom_title: &str, game_code: &str) {
        let mut shared = self.shared.borrow_mut();
        shared.rom_title = rom_title.to_string();
        shared.game_code = game_code.to_string();
        shared.overlay_cmds.clear();
        shared.joypad_set_mask = 0;
        shared.joypad_clear_mask = 0;
    }

    /// Executes registered `"frame"` callbacks once per emulated frame.
    pub fn run_frame(
        &mut self,
        core: &mut GbaCore,
        frame_count: u64,
        rom_title: &str,
        game_code: &str,
        pressed_mask: u16,
        hardcore: bool,
    ) {
        if !self.running {
            return;
        }

        let callbacks: Vec<StashedFunction> = {
            let mut shared = self.shared.borrow_mut();
            shared.frame_count = frame_count;
            shared.rom_title = rom_title.to_string();
            shared.game_code = game_code.to_string();
            shared.pressed_mask = pressed_mask;
            shared.hardcore_active = hardcore;
            // Clear pending key overrides from the previous frame; callbacks below may queue new ones.
            shared.joypad_set_mask = 0;
            shared.joypad_clear_mask = 0;
            if shared.frame_callbacks.is_empty() {
                return;
            }
            // Clear previous frame HUD draw commands before redrawing frame callbacks.
            shared.overlay_cmds.clear();
            shared.frame_callbacks.clone()
        };

        let fuel_limit = self.fuel_limit.max(1);
        let res = {
            let _guard = CoreScopeGuard::enter(Some(core));
            self.run_frame_callbacks(&callbacks, fuel_limit)
        };

        match res {
            Ok(()) => {
                self.frames_executed += 1;
            }
            Err(err) => {
                self.halt_with_error(err);
            }
        }
    }

    /// Applies any active `joypad.set` override masks onto a physical GBA keypad mask (active-high).
    pub fn apply_joypad_override(&self, physical_mask: u16) -> u16 {
        if !self.running {
            return physical_mask;
        }
        let shared = self.shared.borrow();
        ((physical_mask & !shared.joypad_clear_mask) | shared.joypad_set_mask) & 0x03FF
    }

    /// Returns a snapshot of queued HUD draw commands for viewport rendering.
    pub fn with_overlay_commands(&self, f: impl FnOnce(&[GuiDrawCmd])) {
        let shared = self.shared.borrow();
        f(&shared.overlay_cmds);
    }

    /// Returns a cloned copy of queued HUD draw commands (convenient for testing/inspection).
    pub fn overlay_commands(&self) -> Vec<GuiDrawCmd> {
        self.shared.borrow().overlay_cmds.clone()
    }

    /// Returns a snapshot of all console log lines (up to `MAX_CONSOLE_LINES`).
    pub fn console_lines(&self) -> Vec<String> {
        self.shared.borrow().console_log.iter().cloned().collect()
    }

    fn halt_with_error(&mut self, err: String) {
        self.running = false;
        self.last_error = Some(err.clone());
        let mut shared = self.shared.borrow_mut();
        shared.frame_callbacks.clear();
        shared.joypad_set_mask = 0;
        shared.joypad_clear_mask = 0;
        shared.push_log(format!("[Error] {err}"));
    }

    fn execute_chunk(&mut self, name: &str, source: &str, fuel_limit: i32) -> Result<(), String> {
        let stashed_exec = self
            .lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, Some(name), source.as_bytes())?;
                let exec = Executor::start(ctx, closure.into(), ());
                Ok(ctx.stash(exec))
            })
            .map_err(|err| format!("Lua compile error: {err}"))?;

        self.step_to_completion(&stashed_exec, fuel_limit)
    }

    fn run_frame_callbacks(
        &mut self,
        callbacks: &[StashedFunction],
        fuel_limit: i32,
    ) -> Result<(), String> {
        let mut remaining_fuel = Fuel::with(fuel_limit);
        for cb in callbacks {
            let stashed_exec = self.lua.enter(|ctx| {
                let func: Function = ctx.fetch(cb);
                let exec = Executor::start(ctx, func, ());
                ctx.stash(exec)
            });

            let budget = remaining_fuel.remaining().max(1);
            let used = self.step_executor_with_budget(&stashed_exec, budget, fuel_limit)?;
            remaining_fuel.consume(used);
            if !remaining_fuel.should_continue() {
                return Err(format!(
                    "Lua execution halted: exceeded {fuel_limit}-step frame fuel budget (infinite loop guard)"
                ));
            }
        }
        Ok(())
    }

    fn step_to_completion(
        &mut self,
        stashed_exec: &piccolo::StashedExecutor,
        fuel_limit: i32,
    ) -> Result<(), String> {
        let _used = self.step_executor_with_budget(stashed_exec, fuel_limit, fuel_limit)?;
        Ok(())
    }

    fn step_executor_with_budget(
        &mut self,
        stashed_exec: &piccolo::StashedExecutor,
        budget: i32,
        total_limit: i32,
    ) -> Result<i32, String> {
        let mut fuel = Fuel::with(budget);
        let (done, mode) = self.lua.enter(|ctx| {
            let exec = ctx.fetch(stashed_exec);
            let done = exec.step(ctx, &mut fuel);
            (done, exec.mode())
        });

        if !done && mode != ExecutorMode::Result {
            return Err(format!(
                "Lua execution halted: exceeded {total_limit}-step frame fuel budget (infinite loop guard)"
            ));
        }

        self.lua
            .try_enter(|ctx| {
                let exec = ctx.fetch(stashed_exec);
                match exec.take_result::<()>(ctx) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err(err),
                    Err(bad_mode) => Err(lua_err(
                        ctx,
                        format!("unexpected executor state: {bad_mode}"),
                    )),
                }
            })
            .map_err(|err| format!("Lua runtime error: {err}"))?;

        Ok(budget - fuel.remaining())
    }

    /// Renders the `Tools -> Lua Scripting Console...` window.
    pub fn show_lua_console(
        &mut self,
        ctx: &egui::Context,
        rom_loaded: bool,
        hardcore: bool,
    ) -> Option<LuaConsoleAction> {
        if !self.show_console {
            return None;
        }

        let mut open = self.show_console;
        let mut action = None;

        egui::Window::new("Lua Scripting Console")
            .open(&mut open)
            .default_width(640.0)
            .default_height(520.0)
            .min_width(480.0)
            .min_height(360.0)
            .resizable(true)
            .show(ctx, |ui| {
                action = self.draw_console_contents(ui, rom_loaded, hardcore);
            });

        self.show_console = open;
        action
    }

    fn draw_console_contents(
        &mut self,
        ui: &mut egui::Ui,
        rom_loaded: bool,
        hardcore: bool,
    ) -> Option<LuaConsoleAction> {
        let mut action = None;

        let card_bg = Color32::from_rgb(17, 19, 25);
        let elevated_bg = Color32::from_rgb(24, 27, 36);
        let hairline = Color32::from_rgb(42, 46, 58);
        let accent_teal = Color32::from_rgb(0, 210, 175);
        let text_muted = Color32::from_rgb(142, 150, 168);

        // Top toolbar
        egui::Frame::new()
            .fill(elevated_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Load .lua...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Lua scripts", &["lua", "txt"])
                            .pick_file()
                        {
                            if let Err(err) = self.load_file(&path) {
                                self.last_error = Some(err);
                            }
                        }
                    }

                    ui.separator();

                    egui::ComboBox::from_id_salt("lua_sample_picker")
                        .selected_text(self.selected_sample.label())
                        .width(148.0)
                        .show_ui(ui, |ui| {
                            for sample in SampleScript::ALL {
                                ui.selectable_value(
                                    &mut self.selected_sample,
                                    sample,
                                    sample.label(),
                                );
                            }
                        });

                    if ui.button("Load Sample").clicked() {
                        let sample = self.selected_sample;
                        self.load_sample(sample);
                    }

                    ui.separator();

                    let run_label = if self.running { "Restart" } else { "Run" };
                    if ui
                        .add(
                            egui::Button::new(RichText::new(run_label).strong().color(
                                if self.running {
                                    accent_teal
                                } else {
                                    Color32::from_rgb(12, 16, 20)
                                },
                            ))
                            .fill(if self.running {
                                Color32::from_rgba_premultiplied(0, 210, 175, 28)
                            } else {
                                accent_teal
                            }),
                        )
                        .clicked()
                    {
                        action = Some(LuaConsoleAction::Run);
                    }

                    if ui
                        .add_enabled(self.running, egui::Button::new("Stop"))
                        .clicked()
                    {
                        action = Some(LuaConsoleAction::Stop);
                    }

                    if ui.button("Clear Log").clicked() {
                        self.clear_log();
                    }
                });
            });

        ui.add_space(6.0);

        // Status strip
        ui.horizontal(|ui| {
            let (status_text, status_color) = if self.last_error.is_some() {
                ("ERROR", Color32::from_rgb(248, 113, 113))
            } else if self.running {
                ("RUNNING", accent_teal)
            } else {
                ("STOPPED", text_muted)
            };

            egui::Frame::new()
                .fill(Color32::from_rgb(20, 23, 30))
                .stroke(Stroke::new(1.0_f32, status_color))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(status_text)
                            .size(10.5)
                            .strong()
                            .color(status_color),
                    );
                });

            let source_label = self
                .loaded_script_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Inline Buffer");
            ui.label(RichText::new(source_label).small().color(text_muted));

            if !rom_loaded {
                ui.label(
                    RichText::new("| No ROM loaded (emu.read/write unavailable)")
                        .small()
                        .color(Color32::from_rgb(234, 179, 8)),
                );
            } else if hardcore {
                ui.label(
                    RichText::new("| RA Hardcore: emu.write* blocked")
                        .small()
                        .color(Color32::from_rgb(234, 179, 8)),
                );
            }

            if self.running {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} frames", self.frames_executed))
                            .small()
                            .monospace()
                            .color(text_muted),
                    );
                });
            }
        });

        ui.add_space(6.0);

        // Error banner
        if let Some(err) = &self.last_error {
            egui::Frame::new()
                .fill(Color32::from_rgb(42, 18, 22))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(185, 56, 68)))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(err)
                            .monospace()
                            .size(11.5)
                            .color(Color32::from_rgb(252, 165, 165)),
                    );
                });
            ui.add_space(6.0);
        }

        // Script Editor
        ui.label(RichText::new("Script Editor").small().strong().color(text_muted));
        egui::Frame::new()
            .fill(card_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("lua_editor_scroll")
                    .max_height(210.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.script_source)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(12),
                        );
                    });
            });

        ui.add_space(6.0);

        // Console output
        ui.label(
            RichText::new(format!(
                "Console Output (capped at {} lines)",
                MAX_CONSOLE_LINES
            ))
            .small()
            .strong()
            .color(text_muted),
        );
        egui::Frame::new()
            .fill(card_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                let lines = self.console_lines();
                egui::ScrollArea::vertical()
                    .id_salt("lua_console_output_scroll")
                    .stick_to_bottom(true)
                    .min_scrolled_height(110.0)
                    .max_height(150.0)
                    .show(ui, |ui| {
                        if lines.is_empty() {
                            ui.label(
                                RichText::new(
                                    "No console output yet. Call console.log(...) in your script and click Run.",
                                )
                                .small()
                                .italics()
                                .color(text_muted),
                            );
                        } else {
                            for line in lines {
                                let color = if line.starts_with("[Error]") {
                                    Color32::from_rgb(252, 165, 165)
                                } else if line.starts_with("[Lua]") {
                                    accent_teal
                                } else {
                                    Color32::from_rgb(220, 226, 236)
                                };
                                ui.label(RichText::new(line).monospace().size(11.5).color(color));
                            }
                        }
                    });
            });

        action
    }
}

fn register_api(lua: &mut Lua, shared: Rc<RefCell<LuaSharedState>>) {
    lua.enter(|ctx| {
        register_emu_table(ctx, Rc::clone(&shared));
        register_joypad_table(ctx, Rc::clone(&shared));
        register_console_table(ctx, Rc::clone(&shared));
        register_gui_table(ctx, Rc::clone(&shared));
        register_callbacks_table(ctx, shared);
        patch_string_format(ctx);
    });
}

fn register_emu_table<'gc>(ctx: Context<'gc>, shared: Rc<RefCell<LuaSharedState>>) {
    let emu = Table::new(&ctx);

    emu.set(
        ctx,
        "read8",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = with_core_mut(|core| core.peek8(addr)).map_err(|e| lua_err(ctx, e))?;
            stack.replace(ctx, val as i64);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    emu.set(
        ctx,
        "read16",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = with_core_mut(|core| core.peek16(addr)).map_err(|e| lua_err(ctx, e))?;
            stack.replace(ctx, val as i64);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    emu.set(
        ctx,
        "read32",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = with_core_mut(|core| core.peek32(addr)).map_err(|e| lua_err(ctx, e))?;
            stack.replace(ctx, val as i64);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "write8",
        Callback::from_fn(&ctx, move |ctx, _, stack| {
            if s.borrow().hardcore_active {
                return Err(lua_err(
                    ctx,
                    "emu.write8 is disabled while RetroAchievements Hardcore mode is active",
                ));
            }
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = read_i64_arg(stack.get(1), "value")? as u8;
            with_core_mut(|core| core.poke8(addr, val)).map_err(|e| lua_err(ctx, e))?;
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "write16",
        Callback::from_fn(&ctx, move |ctx, _, stack| {
            if s.borrow().hardcore_active {
                return Err(lua_err(
                    ctx,
                    "emu.write16 is disabled while RetroAchievements Hardcore mode is active",
                ));
            }
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = read_i64_arg(stack.get(1), "value")? as u16;
            with_core_mut(|core| core.poke16(addr, val)).map_err(|e| lua_err(ctx, e))?;
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "write32",
        Callback::from_fn(&ctx, move |ctx, _, stack| {
            if s.borrow().hardcore_active {
                return Err(lua_err(
                    ctx,
                    "emu.write32 is disabled while RetroAchievements Hardcore mode is active",
                ));
            }
            let addr = read_u32_arg(ctx, stack.get(0))?;
            let val = read_i64_arg(stack.get(1), "value")? as u32;
            with_core_mut(|core| core.poke32(addr, val)).map_err(|e| lua_err(ctx, e))?;
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "romTitle",
        Callback::from_fn(&ctx, move |ctx, _, mut stack| {
            let title = s.borrow().rom_title.clone();
            stack.replace(ctx, title);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "gameCode",
        Callback::from_fn(&ctx, move |ctx, _, mut stack| {
            let code = s.borrow().game_code.clone();
            stack.replace(ctx, code);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    let s = Rc::clone(&shared);
    emu.set(
        ctx,
        "frameCount",
        Callback::from_fn(&ctx, move |ctx, _, mut stack| {
            let count = s.borrow().frame_count as i64;
            stack.replace(ctx, count);
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    ctx.set_global("emu", emu).unwrap();
}

fn register_joypad_table<'gc>(ctx: Context<'gc>, shared: Rc<RefCell<LuaSharedState>>) {
    let joypad = Table::new(&ctx);

    let s = Rc::clone(&shared);
    joypad
        .set(
            ctx,
            "get",
            Callback::from_fn(&ctx, move |ctx, _, mut stack| {
                let mask = s.borrow().pressed_mask;
                let out = Table::new(&ctx);
                for (name, bit) in KEY_MAP {
                    out.set(ctx, name, (mask & bit) != 0).unwrap();
                }
                stack.replace(ctx, out);
                Ok(CallbackReturn::Return)
            }),
        )
        .unwrap();

    let s = Rc::clone(&shared);
    joypad
        .set(
            ctx,
            "set",
            Callback::from_fn(&ctx, move |ctx, _, stack| {
                let Value::Table(tbl) = stack.get(0) else {
                    return Err(TypeError {
                        expected: "table",
                        found: stack.get(0).type_name(),
                    }
                    .into());
                };

                let mut state = s.borrow_mut();
                for (name, bit) in KEY_MAP {
                    let mut val = tbl.get(ctx, name);
                    if val.is_nil() {
                        val = tbl.get(ctx, name.to_ascii_lowercase());
                    }
                    if val.is_nil() {
                        val = tbl.get(ctx, name.to_ascii_uppercase());
                    }
                    if !val.is_nil() {
                        let active = match val {
                            Value::Integer(i) => i != 0,
                            Value::Number(f) => f != 0.0,
                            other => other.to_bool(),
                        };
                        if active {
                            state.joypad_set_mask |= bit;
                            state.joypad_clear_mask &= !bit;
                        } else {
                            state.joypad_clear_mask |= bit;
                            state.joypad_set_mask &= !bit;
                        }
                    }
                }

                Ok(CallbackReturn::Return)
            }),
        )
        .unwrap();

    ctx.set_global("joypad", joypad).unwrap();
}

fn register_console_table<'gc>(ctx: Context<'gc>, shared: Rc<RefCell<LuaSharedState>>) {
    let console = Table::new(&ctx);

    console
        .set(
            ctx,
            "log",
            Callback::from_fn(&ctx, move |_, _, stack| {
                let mut parts = Vec::with_capacity(stack.len());
                for v in &stack {
                    parts.push(format_lua_value(v));
                }
                shared.borrow_mut().push_log(parts.join(" "));
                Ok(CallbackReturn::Return)
            }),
        )
        .unwrap();

    ctx.set_global("console", console).unwrap();
}

fn register_gui_table<'gc>(ctx: Context<'gc>, shared: Rc<RefCell<LuaSharedState>>) {
    let gui = Table::new(&ctx);

    let s = Rc::clone(&shared);
    gui.set(
        ctx,
        "text",
        Callback::from_fn(&ctx, move |_, _, stack| {
            let x = stack.get(0).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(0).type_name(),
            })? as f32;
            let y = stack.get(1).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(1).type_name(),
            })? as f32;
            let text = format_lua_value(stack.get(2));
            let color = parse_lua_color(stack.get(3), [255, 255, 255, 255]);

            let mut state = s.borrow_mut();
            if state.overlay_cmds.len() < MAX_OVERLAY_CMDS {
                state
                    .overlay_cmds
                    .push(GuiDrawCmd::Text { x, y, text, color });
            }
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    gui.set(
        ctx,
        "rect",
        Callback::from_fn(&ctx, move |_, _, stack| {
            let x = stack.get(0).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(0).type_name(),
            })? as f32;
            let y = stack.get(1).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(1).type_name(),
            })? as f32;
            let w = stack.get(2).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(2).type_name(),
            })? as f32;
            let h = stack.get(3).to_number().ok_or(TypeError {
                expected: "number",
                found: stack.get(3).type_name(),
            })? as f32;
            let color = parse_lua_color(stack.get(4), [0, 210, 175, 240]);
            let fill = match stack.get(5) {
                Value::Nil | Value::Boolean(false) => None,
                Value::Boolean(true) => Some([color[0], color[1], color[2], 70]),
                other => Some(parse_lua_color(other, [color[0], color[1], color[2], 70])),
            };

            let mut state = shared.borrow_mut();
            if state.overlay_cmds.len() < MAX_OVERLAY_CMDS {
                state.overlay_cmds.push(GuiDrawCmd::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    fill,
                });
            }
            Ok(CallbackReturn::Return)
        }),
    )
    .unwrap();

    ctx.set_global("gui", gui).unwrap();
}

fn register_callbacks_table<'gc>(ctx: Context<'gc>, shared: Rc<RefCell<LuaSharedState>>) {
    let callbacks = Table::new(&ctx);

    callbacks
        .set(
            ctx,
            "add",
            Callback::from_fn(&ctx, move |ctx, _, stack| {
                let kind = match stack.get(0) {
                    Value::String(s) => std::str::from_utf8(s.as_bytes())
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                    other => {
                        return Err(TypeError {
                            expected: "string",
                            found: other.type_name(),
                        }
                        .into())
                    }
                };
                if kind != "frame" {
                    return Err(lua_err(
                        ctx,
                        format!("unsupported callback event '{kind}'; expected 'frame'"),
                    ));
                }
                let Value::Function(func) = stack.get(1) else {
                    return Err(TypeError {
                        expected: "function",
                        found: stack.get(1).type_name(),
                    }
                    .into());
                };
                shared.borrow_mut().frame_callbacks.push(ctx.stash(func));
                Ok(CallbackReturn::Return)
            }),
        )
        .unwrap();

    ctx.set_global("callbacks", callbacks).unwrap();
}

/// Adds lightweight `string.format` support to `piccolo`'s `string` stdlib table so common
/// emulator scripts (`0x%08X`, `%d`, `%s`, `%.2f`) work out of the box.
fn patch_string_format<'gc>(ctx: Context<'gc>) {
    let Value::Table(string_tbl) = ctx.get_global("string") else {
        return;
    };
    let _ = string_tbl.set(
        ctx,
        "format",
        Callback::from_fn(&ctx, |ctx, _, mut stack| {
            let fmt_val = stack.get(0);
            let Value::String(fmt_str) = fmt_val else {
                return Err(TypeError {
                    expected: "string",
                    found: fmt_val.type_name(),
                }
                .into());
            };
            let fmt_str = std::str::from_utf8(fmt_str.as_bytes())
                .map_err(|_| lua_err(ctx, "invalid UTF-8 in string.format pattern"))?;
            let args: Vec<Value> = stack.into_iter().skip(1).collect();
            let rendered = format_lua_string(fmt_str, &args).map_err(|e| lua_err(ctx, e))?;
            stack.replace(ctx, rendered);
            Ok(CallbackReturn::Return)
        }),
    );
}

fn format_lua_string(pattern: &str, args: &[Value<'_>]) -> Result<String, String> {
    let mut out = String::with_capacity(pattern.len() + 16);
    let bytes = pattern.as_bytes();
    let mut i = 0;
    let mut arg_idx = 0;

    while i < bytes.len() {
        if bytes[i] != b'%' {
            let ch = pattern[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
        if i >= bytes.len() {
            return Err("incomplete format specifier at end of string.format".into());
        }
        if bytes[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }

        let mut zero_pad = false;
        while i < bytes.len() && bytes[i] == b'0' {
            zero_pad = true;
            i += 1;
        }

        let mut width: Option<usize> = None;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            let w = width.unwrap_or(0) * 10 + usize::from(bytes[i] - b'0');
            width = Some(w);
            i += 1;
        }

        let mut precision: Option<usize> = None;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                p = p * 10 + usize::from(bytes[i] - b'0');
                i += 1;
            }
            precision = Some(p);
        }

        if i >= bytes.len() {
            return Err("incomplete format specifier in string.format".into());
        }
        let spec = bytes[i] as char;
        i += 1;

        let arg = args
            .get(arg_idx)
            .copied()
            .ok_or_else(|| format!("missing argument #{} for string.format", arg_idx + 1))?;
        arg_idx += 1;

        let formatted = match spec {
            'd' | 'i' => {
                let n = arg
                    .to_integer()
                    .ok_or_else(|| format!("bad argument #{} to '%{spec}': expected integer", arg_idx))?;
                pad_numeric(format!("{n}"), width, zero_pad)
            }
            'u' => {
                let n = arg
                    .to_integer()
                    .ok_or_else(|| format!("bad argument #{} to '%u': expected integer", arg_idx))?
                    as u64;
                pad_numeric(format!("{n}"), width, zero_pad)
            }
            'x' => {
                let n = arg
                    .to_integer()
                    .ok_or_else(|| format!("bad argument #{} to '%x': expected integer", arg_idx))?
                    as u64;
                pad_numeric(format!("{n:x}"), width, zero_pad)
            }
            'X' => {
                let n = arg
                    .to_integer()
                    .ok_or_else(|| format!("bad argument #{} to '%X': expected integer", arg_idx))?
                    as u64;
                pad_numeric(format!("{n:X}"), width, zero_pad)
            }
            'f' => {
                let f = arg
                    .to_number()
                    .ok_or_else(|| format!("bad argument #{} to '%f': expected number", arg_idx))?;
                let prec = precision.unwrap_or(6);
                format!("{f:.prec$}")
            }
            's' => format_lua_value(arg),
            'c' => {
                let code = arg
                    .to_integer()
                    .ok_or_else(|| format!("bad argument #{} to '%c': expected integer", arg_idx))?;
                char::from_u32(code as u32)
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            }
            other => return Err(format!("unsupported format specifier '%{other}' in string.format")),
        };
        out.push_str(&formatted);
    }

    Ok(out)
}

fn pad_numeric(s: String, width: Option<usize>, zero_pad: bool) -> String {
    let Some(w) = width else {
        return s;
    };
    if s.len() >= w {
        return s;
    }
    let pad_len = w - s.len();
    if zero_pad {
        if let Some(rest) = s.strip_prefix('-') {
            return format!("-{}{}", "0".repeat(pad_len), rest);
        }
        format!("{}{}", "0".repeat(pad_len), s)
    } else {
        format!("{}{}", " ".repeat(pad_len), s)
    }
}

fn lua_err<'gc>(ctx: Context<'gc>, msg: impl Into<String>) -> piccolo::Error<'gc> {
    msg.into().into_value(ctx).into()
}

fn read_u32_arg<'gc>(ctx: Context<'gc>, val: Value<'gc>) -> Result<u32, piccolo::Error<'gc>> {
    let n = val.to_integer().ok_or(TypeError {
        expected: "integer address",
        found: val.type_name(),
    })?;
    if !(0..=u32::MAX as i64).contains(&n) {
        return Err(lua_err(
            ctx,
            format!("address 0x{n:X} out of 32-bit GBA range"),
        ));
    }
    Ok(n as u32)
}

fn read_i64_arg<'gc>(val: Value<'gc>, expected: &'static str) -> Result<i64, piccolo::Error<'gc>> {
    val.to_integer().ok_or_else(|| {
        TypeError {
            expected,
            found: val.type_name(),
        }
        .into()
    })
}

fn format_lua_value(val: Value<'_>) -> String {
    let mut buf = Vec::new();
    let _ = val.display(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn parse_lua_color(val: Value<'_>, default: [u8; 4]) -> [u8; 4] {
    match val {
        Value::Nil => default,
        Value::Integer(n) => unpack_int_color(n as u64),
        Value::Number(f) => unpack_int_color(f as u64),
        Value::String(s) => {
            let Ok(text) = std::str::from_utf8(s.as_bytes()) else {
                return default;
            };
            parse_color_str(text).unwrap_or(default)
        }
        _ => default,
    }
}

fn unpack_int_color(v: u64) -> [u8; 4] {
    let c = v as u32;
    let alpha = if (c & 0xFF00_0000) == 0 {
        255
    } else {
        ((c >> 24) & 0xFF) as u8
    };
    let r = ((c >> 16) & 0xFF) as u8;
    let g = ((c >> 8) & 0xFF) as u8;
    let b = (c & 0xFF) as u8;
    [r, g, b, alpha]
}

fn parse_color_str(s: &str) -> Option<[u8; 4]> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some([r, g, b, 255])
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                let a = u8::from_str_radix(&hex[3..4], 16).ok()? * 17;
                Some([r, g, b, a])
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r, g, b, 255])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([r, g, b, a])
            }
            _ => None,
        };
    }

    match t.to_ascii_lowercase().as_str() {
        "white" => Some([255, 255, 255, 255]),
        "black" => Some([0, 0, 0, 255]),
        "red" => Some([239, 68, 68, 255]),
        "green" => Some([34, 197, 94, 255]),
        "blue" => Some([59, 130, 246, 255]),
        "yellow" => Some([250, 204, 21, 255]),
        "amber" => Some([245, 158, 11, 255]),
        "orange" => Some([249, 115, 22, 255]),
        "cyan" => Some([34, 211, 238, 255]),
        "teal" => Some([0, 210, 175, 255]),
        "emerald" => Some([16, 185, 129, 255]),
        "gray" | "grey" => Some([156, 163, 175, 255]),
        "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x200];
        rom[0xA0..0xAC].copy_from_slice(b"LUATESTROM\0\0");
        rom[0xAC..0xB0].copy_from_slice(b"ALTE");
        rom
    }

    fn new_test_core() -> GbaCore {
        GbaCore::new(None, dummy_rom())
    }

    #[test]
    fn emu_read_write_and_metadata() {
        let mut core = new_test_core();
        core.poke8(0x0200_0010, 0x5A);
        core.poke16(0x0200_0020, 0xBEEF);
        core.poke32(0x0300_0100, 0x1234_5678);

        let mut engine = LuaEngine::new();
        engine.script_source = r#"
            local b = emu.read8(0x02000010)
            local h = emu.read16(0x02000020)
            local w = emu.read32(0x03000100)
            console.log(string.format("%02X %04X %08X %s %s %d", b, h, w, emu.romTitle(), emu.gameCode(), emu.frameCount()))
            emu.write8(0x02000011, 0xA5)
            emu.write16(0x02000022, 0xCAFE)
            emu.write32(0x03000104, 0x89ABCDEF)
        "#
        .to_string();

        engine
            .start_script(Some(&mut core), "LUATESTROM", "ALTE", 42, 0, false)
            .expect("script should succeed");

        assert_eq!(core.peek8(0x0200_0011), 0xA5);
        assert_eq!(core.peek16(0x0200_0022), 0xCAFE);
        assert_eq!(core.peek32(0x0300_0104), 0x89AB_CDEF);
        let logs = engine.console_lines();
        assert_eq!(logs, vec!["5A BEEF 12345678 LUATESTROM ALTE 42"]);
    }

    #[test]
    fn emu_write8_blocked_in_hardcore_mode() {
        let mut core = new_test_core();
        core.poke8(0x0200_0011, 0x11);

        let mut engine = LuaEngine::new();
        engine.script_source = "emu.write8(0x02000011, 0x99)".to_string();

        let res = engine.start_script(Some(&mut core), "LUATESTROM", "ALTE", 0, 0, true);
        assert!(res.is_err());
        assert_eq!(core.peek8(0x0200_0011), 0x11);
        assert!(!engine.running);
        assert!(engine.last_error.is_some());
    }

    #[test]
    fn callbacks_frame_gui_text_and_joypad_set() {
        let mut core = new_test_core();
        let mut engine = LuaEngine::new();
        engine.script_source = r##"
            callbacks.add("frame", function()
                local k = joypad.get()
                local f = emu.frameCount()
                gui.rect(2, 4, 40, 12, "#00D2AF")
                gui.text(8, 6, "F" .. tostring(f) .. (k.B and "B" or ""), "#FF0000")
                joypad.set({ A = true, Up = true, B = false })
            end)
        "##
        .to_string();

        engine
            .start_script(Some(&mut core), "LUATESTROM", "ALTE", 1, KEY_B, false)
            .expect("top-level registration should succeed");

        engine.run_frame(&mut core, 7, "LUATESTROM", "ALTE", KEY_B, false);
        assert!(engine.running);
        assert_eq!(engine.frames_executed, 1);

        let cmds = engine.overlay_commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[1],
            GuiDrawCmd::Text {
                x: 8.0,
                y: 6.0,
                text: "F7B".to_string(),
                color: [255, 0, 0, 255],
            }
        );

        let effective = engine.apply_joypad_override(KEY_B | KEY_LEFT);
        assert_eq!(effective, KEY_LEFT | KEY_A | KEY_UP);
    }

    #[test]
    fn fuel_limit_stops_infinite_loop_without_hanging() {
        let mut core = new_test_core();
        let mut engine = LuaEngine::new();
        engine.fuel_limit = 5_000;
        engine.script_source = "while true do end".to_string();

        let res = engine.start_script(Some(&mut core), "LUATESTROM", "ALTE", 0, 0, false);
        assert!(res.is_err());
        assert!(!engine.running);
        let err = engine.last_error.as_deref().unwrap_or_default();
        assert!(err.contains("fuel"), "unexpected error: {err}");

        // Also verify frame-callback infinite loop guard.
        let mut engine2 = LuaEngine::new();
        engine2.fuel_limit = 5_000;
        engine2.script_source = r#"
            callbacks.add("frame", function()
                while true do end
            end)
        "#
        .to_string();
        engine2
            .start_script(Some(&mut core), "LUATESTROM", "ALTE", 0, 0, false)
            .expect("callback registration should succeed");
        engine2.run_frame(&mut core, 1, "LUATESTROM", "ALTE", 0, false);
        assert!(!engine2.running);
        assert!(
            engine2
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("fuel")
        );
    }

    #[test]
    fn sample_scripts_compile_and_run_cleanly() {
        let mut core = new_test_core();
        for sample in SampleScript::ALL {
            let mut engine = LuaEngine::new();
            engine.load_sample(sample);
            engine
                .start_script(Some(&mut core), "LUATESTROM", "ALTE", 0, KEY_A, false)
                .unwrap_or_else(|e| panic!("{} failed start: {e}", sample.label()));
            engine.run_frame(&mut core, 1, "LUATESTROM", "ALTE", KEY_A, false);
            engine.run_frame(&mut core, 2, "LUATESTROM", "ALTE", 0, false);
            assert!(engine.running, "{} stopped unexpectedly: {:?}", sample.label(), engine.last_error);
            assert!(engine.last_error.is_none());
            assert!(!engine.overlay_commands().is_empty());
        }
    }
}
