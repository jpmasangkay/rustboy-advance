use egui::{
    self, Align2, Color32, ColorImage, CornerRadius, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use gba_core::cpu::{CpuMode, CPSR_C, CPSR_F, CPSR_I, CPSR_N, CPSR_T, CPSR_V, CPSR_Z};
use gba_core::GbaCore;
use gba_debugger::disassemble_range;

use crate::cheats::{decode_cheat_lines, CheatEntry, CheatFormat};
use crate::config::{format_key_name, GbaButton, KeyBindings, SaveOverrideChoice};
use crate::filters::{agb_srgb_lut, linear_lut};

const PANEL_BORDER: Color32 = Color32::from_rgb(42, 48, 61);
const SURFACE_DARK: Color32 = Color32::from_rgb(13, 15, 19);
const SURFACE_CARD: Color32 = Color32::from_rgb(20, 23, 30);
const ACCENT_TEAL: Color32 = Color32::from_rgb(46, 196, 182);
const ACCENT_AMBER: Color32 = Color32::from_rgb(224, 159, 62);
const TEXT_MUTED: Color32 = Color32::from_rgb(142, 152, 170);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryRegion {
    Bios,
    Ewram,
    Iwram,
    Io,
    Pram,
    Vram,
    Oam,
    Rom,
}

impl MemoryRegion {
    pub const ALL: [MemoryRegion; 8] = [
        Self::Bios,
        Self::Ewram,
        Self::Iwram,
        Self::Io,
        Self::Pram,
        Self::Vram,
        Self::Oam,
        Self::Rom,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bios => "0x00 BIOS (16 KB)",
            Self::Ewram => "0x02 EWRAM (256 KB)",
            Self::Iwram => "0x03 IWRAM (32 KB)",
            Self::Io => "0x04 I/O Registers",
            Self::Pram => "0x05 PRAM (1 KB)",
            Self::Vram => "0x06 VRAM (96 KB)",
            Self::Oam => "0x07 OAM (1 KB)",
            Self::Rom => "0x08 ROM",
        }
    }

    pub const fn base_addr(self) -> u32 {
        match self {
            Self::Bios => 0x0000_0000,
            Self::Ewram => 0x0200_0000,
            Self::Iwram => 0x0300_0000,
            Self::Io => 0x0400_0000,
            Self::Pram => 0x0500_0000,
            Self::Vram => 0x0600_0000,
            Self::Oam => 0x0700_0000,
            Self::Rom => 0x0800_0000,
        }
    }

    pub fn byte_len(self, rom_len: usize) -> usize {
        match self {
            Self::Bios => 0x4000,
            Self::Ewram => 0x4_0000,
            Self::Iwram => 0x8000,
            Self::Io => 0x400,
            Self::Pram => 0x400,
            Self::Vram => 0x1_8000,
            Self::Oam => 0x400,
            Self::Rom => rom_len.max(0x100).min(0x0200_0000),
        }
    }
}

#[derive(Default)]
pub enum DebugAction {
    #[default]
    None,
    TogglePause,
    StepInstruction,
    StepScanline,
    StepFrame,
    HardReset,
    ConfigChanged,
}

pub struct DebuggerWindows {
    // Window open toggles
    pub show_cpu: bool,
    pub show_hex: bool,
    pub show_palette: bool,
    pub show_vram: bool,
    pub show_bg: bool,
    pub show_oam: bool,
    pub show_audio: bool,
    pub show_overrides: bool,
    pub show_cheats: bool,
    pub show_controls: bool,
    pub show_about: bool,

    // CPU & Disassembler state
    pub follow_pc: bool,
    pub disasm_addr_input: String,
    pub disasm_view_addr: u32,
    pub bp_addr_input: String,
    pub breakpoints: Vec<u32>,

    // Hex Viewer state
    pub hex_region: MemoryRegion,
    pub hex_goto_input: String,
    pub hex_scroll_row: Option<usize>,
    pub hex_search_input: String,
    pub hex_search_ascii: bool,
    pub hex_match_addr: Option<u32>,
    pub hex_search_status: String,

    // Palette Viewer state
    pub selected_pal_idx: usize,

    // VRAM Tile Viewer state
    pub tile_cbb: usize, // 0..=3 = BG CBB 0..3, 4 = OBJ Low, 5 = OBJ High
    pub tile_8bpp: bool,
    pub tile_pal_bank: usize,
    pub tile_zoom: f32,
    pub selected_tile: Option<usize>,
    tile_tex: Option<TextureHandle>,

    // BG Map Viewer state
    pub bg_layer: usize,
    bg_tex: Option<TextureHandle>,

    // OAM Sprite Viewer state
    pub selected_sprite: usize,
    pub hide_disabled_sprites: bool,
    sprite_tex: Option<TextureHandle>,

    // Cheat manager draft form
    pub draft_cheat_name: String,
    pub draft_cheat_code: String,
    pub draft_cheat_format: CheatFormat,
    pub cheat_error: Option<String>,

    // Controls rebinding state
    pub awaiting_rebind: Option<GbaButton>,
}

impl Default for DebuggerWindows {
    fn default() -> Self {
        Self {
            show_cpu: false,
            show_hex: false,
            show_palette: false,
            show_vram: false,
            show_bg: false,
            show_oam: false,
            show_audio: false,
            show_overrides: false,
            show_cheats: false,
            show_controls: false,
            show_about: false,
            follow_pc: true,
            disasm_addr_input: "08000000".to_string(),
            disasm_view_addr: 0x0800_0000,
            bp_addr_input: String::new(),
            breakpoints: Vec::new(),
            hex_region: MemoryRegion::Ewram,
            hex_goto_input: "02000000".to_string(),
            hex_scroll_row: None,
            hex_search_input: String::new(),
            hex_search_ascii: false,
            hex_match_addr: None,
            hex_search_status: String::new(),
            selected_pal_idx: 0,
            tile_cbb: 0,
            tile_8bpp: false,
            tile_pal_bank: 0,
            tile_zoom: 2.0,
            selected_tile: None,
            tile_tex: None,
            bg_layer: 0,
            bg_tex: None,
            selected_sprite: 0,
            hide_disabled_sprites: true,
            sprite_tex: None,
            draft_cheat_name: String::new(),
            draft_cheat_code: String::new(),
            draft_cheat_format: CheatFormat::AutoRaw,
            cheat_error: None,
            awaiting_rebind: None,
        }
    }
}

#[inline]
fn bgr555_to_rgba(raw: u16, color_correction: bool) -> [u8; 4] {
    let idx = (raw & 0x7FFF) as usize;
    if color_correction {
        agb_srgb_lut()[idx]
    } else {
        linear_lut()[idx]
    }
}

#[inline]
fn read_pram_u16(pram: &[u8; 0x400], idx: usize) -> u16 {
    let off = (idx & 0x1FF) * 2;
    u16::from_le_bytes([pram[off], pram[off + 1]])
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let trimmed = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(trimmed, 16).ok()
}

fn checkerboard_color(x: usize, y: usize) -> Color32 {
    if ((x / 4) + (y / 4)) % 2 == 0 {
        Color32::from_rgb(28, 32, 40)
    } else {
        Color32::from_rgb(38, 44, 54)
    }
}

impl DebuggerWindows {
    #[allow(clippy::too_many_arguments)]
    pub fn show_all(
        &mut self,
        ctx: &egui::Context,
        core: Option<&mut GbaCore>,
        paused: bool,
        color_correction: bool,
        cheats: &mut Vec<CheatEntry>,
        keybindings: &mut KeyBindings,
        gamepad_names: &[String],
    ) -> DebugAction {
        let mut action = DebugAction::None;

        if self.show_about {
            self.draw_about_window(ctx);
        }
        if self.show_controls {
            if self.draw_controls_window(ctx, keybindings, gamepad_names) {
                action = DebugAction::ConfigChanged;
            }
        }
        if self.show_cheats {
            if self.draw_cheats_window(ctx, cheats) {
                action = DebugAction::ConfigChanged;
            }
        }

        let Some(core) = core else {
            // Even without a ROM, keep debugger toggles clean without panicking.
            return action;
        };

        if self.show_overrides {
            if self.draw_overrides_window(ctx, core) {
                action = DebugAction::ConfigChanged;
            }
        }
        if self.show_cpu {
            let cpu_act = self.draw_cpu_window(ctx, core, paused);
            if !matches!(cpu_act, DebugAction::None) {
                action = cpu_act;
            }
        }
        if self.show_hex {
            self.draw_hex_window(ctx, core);
        }
        if self.show_palette {
            self.draw_palette_window(ctx, core, color_correction);
        }
        if self.show_vram {
            self.draw_vram_window(ctx, core, color_correction);
        }
        if self.show_bg {
            self.draw_bg_window(ctx, core, color_correction);
        }
        if self.show_oam {
            self.draw_oam_window(ctx, core, color_correction);
        }
        if self.show_audio {
            self.draw_audio_window(ctx, core);
        }

        action
    }

    fn draw_cpu_window(
        &mut self,
        ctx: &egui::Context,
        core: &GbaCore,
        paused: bool,
    ) -> DebugAction {
        let mut action = DebugAction::None;
        let mut open = self.show_cpu;

        egui::Window::new("ARM7TDMI CPU & Disassembler")
            .open(&mut open)
            .default_size([740.0, 500.0])
            .resizable(true)
            .show(ctx, |ui| {
                // Control bar
                ui.horizontal(|ui| {
                    let run_lbl = if paused { "Resume" } else { "Pause" };
                    if ui.button(run_lbl).clicked() {
                        action = DebugAction::TogglePause;
                    }
                    if ui.button("Step 1 Instr").clicked() {
                        action = DebugAction::StepInstruction;
                    }
                    if ui.button("Step Scanline").clicked() {
                        action = DebugAction::StepScanline;
                    }
                    if ui.button("Step Frame").clicked() {
                        action = DebugAction::StepFrame;
                    }
                    if ui.button("Hard Reset").clicked() {
                        action = DebugAction::HardReset;
                    }
                    ui.separator();
                    ui.checkbox(&mut self.follow_pc, "Follow PC");
                });
                ui.separator();

                let exec_pc = core.cpu.current_exec_pc();
                let is_thumb = core.cpu.is_thumb();

                ui.columns(2, |cols| {
                    // Left column: Disassembler & Breakpoints
                    let ui = &mut cols[0];
                    ui.horizontal(|ui| {
                        ui.label("Go to:");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.disasm_addr_input)
                                .desired_width(84.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if ui.button("Jump").clicked()
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            if let Some(a) = parse_hex_u32(&self.disasm_addr_input) {
                                self.disasm_view_addr = a;
                                self.follow_pc = false;
                            }
                        }
                        if ui.button("Sync PC").clicked() {
                            self.follow_pc = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Break @");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.bp_addr_input)
                                .desired_width(84.0)
                                .font(egui::TextStyle::Monospace),
                        );
                        if ui.button("Add BP").clicked() {
                            if let Some(a) = parse_hex_u32(&self.bp_addr_input) {
                                let aligned = a & if is_thumb { !1 } else { !3 };
                                if !self.breakpoints.contains(&aligned) {
                                    self.breakpoints.push(aligned);
                                }
                            }
                        }
                        if ui.button("Clear All").clicked() {
                            self.breakpoints.clear();
                        }
                        ui.label(
                            RichText::new(format!("{} active", self.breakpoints.len()))
                                .color(TEXT_MUTED),
                        );
                    });

                    ui.add_space(4.0);
                    let start_addr = if self.follow_pc {
                        let back = if is_thumb { 10 } else { 20 };
                        exec_pc.saturating_sub(back)
                    } else {
                        self.disasm_view_addr
                    };
                    let lines = disassemble_range(core, start_addr, 24, is_thumb);

                    egui::Frame::new()
                        .fill(SURFACE_DARK)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for line in lines {
                                    let is_pc = line.addr == exec_pc;
                                    let has_bp = self.breakpoints.contains(&line.addr);
                                    ui.horizontal(|ui| {
                                        let bp_char = if has_bp { "●" } else { "○" };
                                        let bp_color = if has_bp {
                                            Color32::from_rgb(239, 68, 68)
                                        } else {
                                            Color32::from_rgb(75, 85, 99)
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(bp_char)
                                                        .color(bp_color)
                                                        .monospace(),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            if has_bp {
                                                self.breakpoints.retain(|&x| x != line.addr);
                                            } else {
                                                self.breakpoints.push(line.addr);
                                            }
                                        }

                                        let marker = if is_pc { "▶" } else { " " };
                                        let op_str = if line.is_thumb && line.opcode <= 0xFFFF {
                                            format!("{:04X}    ", line.opcode)
                                        } else {
                                            format!("{:08X}", line.opcode)
                                        };
                                        let row_text = format!(
                                            "{marker} {:08X}  {op_str}  {}",
                                            line.addr, line.text
                                        );
                                        let color = if is_pc {
                                            ACCENT_TEAL
                                        } else {
                                            Color32::from_rgb(218, 224, 234)
                                        };
                                        ui.label(RichText::new(row_text).monospace().color(color));
                                    });
                                }
                            });
                        });

                    // Right column: Registers, Flags & Pipeline
                    let r_ui = &mut cols[1];
                    r_ui.label(RichText::new("General Purpose Registers").strong());
                    egui::Frame::new()
                        .fill(SURFACE_DARK)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(8.0)
                        .show(r_ui, |ui| {
                            egui::Grid::new("cpu_gpr_grid").num_columns(2).spacing([18.0, 4.0]).show(ui, |ui| {
                                for r in 0..8 {
                                    let r_hi = r + 8;
                                    let name_lo = format!("R{r:<2}");
                                    let name_hi = match r_hi {
                                        13 => "SP ".to_string(),
                                        14 => "LR ".to_string(),
                                        15 => "PC ".to_string(),
                                        n => format!("R{n:<2}"),
                                    };
                                    ui.monospace(format!("{name_lo}  {:08X}", core.cpu.gpr[r]));
                                    ui.monospace(format!("{name_hi}  {:08X}", core.cpu.gpr[r_hi]));
                                    ui.end_row();
                                }
                            });
                        });

                    r_ui.add_space(6.0);
                    r_ui.label(RichText::new("CPSR & Hardware State").strong());
                    egui::Frame::new()
                        .fill(SURFACE_DARK)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(8.0)
                        .show(r_ui, |ui| {
                            let cpsr = core.cpu.cpsr;
                            let mode_name = match core.cpu.mode() {
                                CpuMode::User => "USR",
                                CpuMode::Fiq => "FIQ",
                                CpuMode::Irq => "IRQ",
                                CpuMode::Supervisor => "SVC",
                                CpuMode::Abort => "ABT",
                                CpuMode::Undefined => "UND",
                                CpuMode::System => "SYS",
                            };
                            let flag_badge = |ui: &mut Ui, lbl: &str, active: bool| {
                                let col = if active { ACCENT_TEAL } else { Color32::from_rgb(72, 80, 96) };
                                ui.label(RichText::new(lbl).monospace().strong().color(col));
                            };
                            ui.monospace(format!("CPSR: {:08X}  Mode: {mode_name}", cpsr));
                            ui.horizontal(|ui| {
                                flag_badge(ui, "N", (cpsr & CPSR_N) != 0);
                                flag_badge(ui, "Z", (cpsr & CPSR_Z) != 0);
                                flag_badge(ui, "C", (cpsr & CPSR_C) != 0);
                                flag_badge(ui, "V", (cpsr & CPSR_V) != 0);
                                ui.separator();
                                flag_badge(ui, "I", (cpsr & CPSR_I) != 0);
                                flag_badge(ui, "F", (cpsr & CPSR_F) != 0);
                                flag_badge(ui, "T", (cpsr & CPSR_T) != 0);
                            });
                            ui.separator();
                            ui.monospace(format!(
                                "Pipe: [{:08X}, {:08X}]  Exec: {:08X}",
                                core.cpu.pipeline[0], core.cpu.pipeline[1], exec_pc
                            ));
                            ui.monospace(format!(
                                "VCOUNT: {:>3}  DISPSTAT: {:04X}  DISPCNT: {:04X}",
                                core.bus.ppu.vcount, core.bus.ppu.dispstat, core.bus.ppu.dispcnt
                            ));
                            ui.monospace(format!(
                                "IME: {}  IE: {:04X}  IF: {:04X}  Halt: {}",
                                core.bus.hw.ime, core.bus.hw.ie, core.bus.hw.if_flag, core.cpu.halted
                            ));
                        });
                });
            });

        self.show_cpu = open;
        action
    }

    fn draw_hex_window(&mut self, ctx: &egui::Context, core: &GbaCore) {
        let mut open = self.show_hex;
        egui::Window::new("Memory Hex Viewer")
            .open(&mut open)
            .default_size([700.0, 460.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Region:");
                    let prev_region = self.hex_region;
                    egui::ComboBox::from_id_salt("hex_region_cb")
                        .selected_text(self.hex_region.label())
                        .show_ui(ui, |ui| {
                            for r in MemoryRegion::ALL {
                                ui.selectable_value(&mut self.hex_region, r, r.label());
                            }
                        });
                    if self.hex_region != prev_region {
                        self.hex_goto_input = format!("{:08X}", self.hex_region.base_addr());
                        self.hex_scroll_row = Some(0);
                    }

                    ui.separator();
                    ui.label("Go to:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.hex_goto_input)
                            .desired_width(78.0)
                            .font(egui::TextStyle::Monospace),
                    );
                    if ui.button("Jump").clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        if let Some(addr) = parse_hex_u32(&self.hex_goto_input) {
                            let target_region = match (addr >> 24) & 0xFF {
                                0x00 => Some(MemoryRegion::Bios),
                                0x02 => Some(MemoryRegion::Ewram),
                                0x03 => Some(MemoryRegion::Iwram),
                                0x04 => Some(MemoryRegion::Io),
                                0x05 => Some(MemoryRegion::Pram),
                                0x06 => Some(MemoryRegion::Vram),
                                0x07 => Some(MemoryRegion::Oam),
                                0x08..=0x0D => Some(MemoryRegion::Rom),
                                _ => None,
                            };
                            if let Some(tr) = target_region {
                                self.hex_region = tr;
                                let off = addr.saturating_sub(tr.base_addr()) as usize;
                                self.hex_scroll_row = Some(off / 16);
                            }
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Find:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.hex_search_input)
                            .desired_width(170.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("Hex (DE AD) or ASCII"),
                    );
                    ui.checkbox(&mut self.hex_search_ascii, "ASCII");
                    if ui.button("Find Next").clicked() {
                        self.search_memory_next(core);
                    }
                    if !self.hex_search_status.is_empty() {
                        ui.label(RichText::new(&self.hex_search_status).color(ACCENT_AMBER));
                    }
                });
                ui.separator();

                let base = self.hex_region.base_addr();
                let total_bytes = self.hex_region.byte_len(core.bus.cart.rom.len());
                let total_rows = total_bytes.div_ceil(16);
                let row_height = 18.0;

                let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if let Some(row) = self.hex_scroll_row.take() {
                    scroll = scroll.vertical_scroll_offset(row as f32 * row_height);
                }

                egui::Frame::new()
                    .fill(SURFACE_DARK)
                    .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        scroll.show_rows(ui, row_height, total_rows, |ui, row_range| {
                            for row in row_range {
                                let row_addr = base.wrapping_add((row * 16) as u32);
                                let mut hex_part = String::with_capacity(48);
                                let mut ascii_part = String::with_capacity(16);
                                let mut row_has_match = false;
                                for c in 0..16usize {
                                    let a = row_addr.wrapping_add(c as u32);
                                    if self.hex_match_addr == Some(a) {
                                        row_has_match = true;
                                    }
                                    let b = core.bus.debug_read_8(a);
                                    if c == 8 {
                                        hex_part.push(' ');
                                    }
                                    use std::fmt::Write;
                                    let _ = write!(hex_part, "{b:02X} ");
                                    let ch = if (0x20..0x7F).contains(&b) {
                                        b as char
                                    } else {
                                        '.'
                                    };
                                    ascii_part.push(ch);
                                }
                                let col = if row_has_match {
                                    ACCENT_TEAL
                                } else {
                                    Color32::from_rgb(214, 220, 232)
                                };
                                ui.monospace(
                                    RichText::new(format!("{row_addr:08X} | {hex_part} | {ascii_part}"))
                                        .color(col),
                                );
                            }
                        });
                    });
            });
        self.show_hex = open;
    }

    fn search_memory_next(&mut self, core: &GbaCore) {
        let needle: Vec<u8> = if self.hex_search_ascii {
            self.hex_search_input.as_bytes().to_vec()
        } else {
            let cleaned: String = self
                .hex_search_input
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect();
            if cleaned.len() < 2 || cleaned.len() % 2 != 0 {
                self.hex_search_status = "Enter even count of hex digits".to_string();
                return;
            }
            (0..cleaned.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).ok())
                .collect()
        };
        if needle.is_empty() {
            return;
        }
        let base = self.hex_region.base_addr();
        let len = self.hex_region.byte_len(core.bus.cart.rom.len());
        let start_off = match self.hex_match_addr {
            Some(m) if m >= base => ((m - base) as usize + 1) % len,
            _ => 0,
        };
        if needle.len() > len {
            return;
        }
        for i in start_off..=(len - needle.len()) {
            let a = base + i as u32;
            if (0..needle.len()).all(|j| core.bus.debug_read_8(a + j as u32) == needle[j]) {
                self.hex_match_addr = Some(a);
                self.hex_scroll_row = Some(i / 16);
                self.hex_search_status = format!("Match @ 0x{a:08X}");
                return;
            }
        }
        self.hex_match_addr = None;
        self.hex_search_status = "No further match in region".to_string();
    }

    fn draw_palette_window(&mut self, ctx: &egui::Context, core: &GbaCore, color_correction: bool) {
        let mut open = self.show_palette;
        egui::Window::new("Palette Viewer (PRAM)")
            .open(&mut open)
            .default_size([560.0, 350.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (bank_title, base_idx) in [("BG Palette (0..255)", 0usize), ("OBJ Palette (256..511)", 256usize)] {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(bank_title).strong());
                            let cell = 14.0;
                            let size = Vec2::splat(cell * 16.0);
                            let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                            let painter = ui.painter_at(rect);

                            for row in 0..16 {
                                for col in 0..16 {
                                    let idx = base_idx + row * 16 + col;
                                    let raw = read_pram_u16(&core.bus.ppu.pram, idx);
                                    let [r, g, b, _] = bgr555_to_rgba(raw, color_correction);
                                    let c_rect = Rect::from_min_size(
                                        Pos2::new(rect.left() + col as f32 * cell, rect.top() + row as f32 * cell),
                                        Vec2::splat(cell),
                                    );
                                    painter.rect_filled(c_rect, CornerRadius::ZERO, Color32::from_rgb(r, g, b));
                                    if idx == self.selected_pal_idx {
                                        painter.rect_stroke(
                                            c_rect,
                                            CornerRadius::ZERO,
                                            Stroke::new(2.0_f32, Color32::WHITE),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                }
                            }
                            painter.rect_stroke(
                                rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0_f32, PANEL_BORDER),
                                egui::StrokeKind::Outside,
                            );

                            if let Some(pos) = resp.hover_pos() {
                                if rect.contains(pos) {
                                    let c = ((pos.x - rect.left()) / cell).floor() as usize;
                                    let r = ((pos.y - rect.top()) / cell).floor() as usize;
                                    if c < 16 && r < 16 && (resp.clicked() || ctx.input(|i| i.pointer.any_down())) {
                                        self.selected_pal_idx = base_idx + r * 16 + c;
                                    } else if c < 16 && r < 16 {
                                        // Also show hover preview smoothly by updating selection on hover if desired, or keep click + tooltip
                                        let hover_idx = base_idx + r * 16 + c;
                                        let hraw = read_pram_u16(&core.bus.ppu.pram, hover_idx);
                                        let [hr, hg, hb, _] = bgr555_to_rgba(hraw, color_correction);
                                        resp.clone().on_hover_text(format!(
                                            "Index #{hover_idx} (Bank {} Color {}) | Addr 0x{:08X} | BGR555 0x{hraw:04X} | #{hr:02X}{hg:02X}{hb:02X}",
                                            (hover_idx % 256) / 16,
                                            hover_idx % 16,
                                            0x0500_0000 + hover_idx * 2
                                        ));
                                    }
                                }
                            }
                        });
                        ui.add_space(12.0);
                    }
                });

                ui.add_space(6.0);
                ui.separator();
                let idx = self.selected_pal_idx.min(511);
                let raw = read_pram_u16(&core.bus.ppu.pram, idx);
                let [r, g, b, _] = bgr555_to_rgba(raw, color_correction);
                let r5 = raw & 0x1F;
                let g5 = (raw >> 5) & 0x1F;
                let b5 = (raw >> 10) & 0x1F;
                ui.horizontal(|ui| {
                    let (sw, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    ui.painter().rect_filled(sw, CornerRadius::same(3), Color32::from_rgb(r, g, b));
                    ui.painter().rect_stroke(sw, CornerRadius::same(3), Stroke::new(1.0_f32, PANEL_BORDER), egui::StrokeKind::Outside);
                    ui.monospace(format!(
                        "Selected: {} #{idx} (Bank {}[{}]) | PRAM 0x{:08X} | BGR555: 0x{raw:04X} (R:{r5} G:{g5} B:{b5}) | sRGB #{r:02X}{g:02X}{b:02X}",
                        if idx < 256 { "BG" } else { "OBJ" },
                        (idx % 256) / 16,
                        idx % 16,
                        0x0500_0000 + idx * 2
                    ));
                });
            });
        self.show_palette = open;
    }

    fn draw_vram_window(&mut self, ctx: &egui::Context, core: &GbaCore, color_correction: bool) {
        let mut open = self.show_vram;
        egui::Window::new("VRAM Tile Viewer")
            .open(&mut open)
            .default_size([560.0, 500.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Charblock:");
                    let cbb_names = [
                        "CBB 0 (06000000)",
                        "CBB 1 (06004000)",
                        "CBB 2 (06008000)",
                        "CBB 3 (0600C000)",
                        "OBJ Low (06010000)",
                        "OBJ High (06014000)",
                    ];
                    egui::ComboBox::from_id_salt("vram_cbb_cb")
                        .selected_text(cbb_names[self.tile_cbb])
                        .show_ui(ui, |ui| {
                            for (i, &name) in cbb_names.iter().enumerate() {
                                ui.selectable_value(&mut self.tile_cbb, i, name);
                            }
                        });
                    ui.checkbox(&mut self.tile_8bpp, "8bpp");
                    if !self.tile_8bpp {
                        ui.add(egui::Slider::new(&mut self.tile_pal_bank, 0..=15).text("Pal"));
                    }
                    egui::ComboBox::from_id_salt("vram_zoom_cb")
                        .selected_text(format!("{}x", self.tile_zoom as u8))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.tile_zoom, 1.0, "1x");
                            ui.selectable_value(&mut self.tile_zoom, 2.0, "2x");
                            ui.selectable_value(&mut self.tile_zoom, 3.0, "3x");
                        });
                });
                ui.separator();

                let cbb_base = self.tile_cbb * 0x4000;
                let tile_bytes = if self.tile_8bpp { 64 } else { 32 };
                let num_tiles = 0x4000 / tile_bytes; // 512 (4bpp) or 256 (8bpp)
                let cols = 16usize;
                let rows = num_tiles / cols;
                let img_w = cols * 8;
                let img_h = rows * 8;
                let is_obj = self.tile_cbb >= 4;
                let pal_base = if is_obj { 256 } else { 0 };

                let mut rgba = vec![0u8; img_w * img_h * 4];
                for t in 0..num_tiles {
                    let tx = (t % cols) * 8;
                    let ty = (t / cols) * 8;
                    let tile_off = cbb_base + t * tile_bytes;
                    for py in 0..8usize {
                        for px in 0..8usize {
                            let color_idx = if self.tile_8bpp {
                                let b = core.bus.ppu.vram[tile_off + py * 8 + px] as usize;
                                if b == 0 { None } else { Some(pal_base + b) }
                            } else {
                                let byte = core.bus.ppu.vram[tile_off + py * 4 + (px / 2)];
                                let nib = if (px & 1) == 0 { byte & 0xF } else { byte >> 4 } as usize;
                                if nib == 0 {
                                    None
                                } else {
                                    Some(pal_base + self.tile_pal_bank * 16 + nib)
                                }
                            };
                            let dst = ((ty + py) * img_w + (tx + px)) * 4;
                            if let Some(pidx) = color_idx {
                                let raw = read_pram_u16(&core.bus.ppu.pram, pidx);
                                let c = bgr555_to_rgba(raw, color_correction);
                                rgba[dst..dst + 4].copy_from_slice(&c);
                            } else {
                                let cb = checkerboard_color(tx + px, ty + py);
                                rgba[dst] = cb.r();
                                rgba[dst + 1] = cb.g();
                                rgba[dst + 2] = cb.b();
                                rgba[dst + 3] = 255;
                            }
                        }
                    }
                }

                let img = ColorImage::from_rgba_unmultiplied([img_w, img_h], &rgba);
                let tex = self.tile_tex.get_or_insert_with(|| {
                    ctx.load_texture("vram_tiles", img.clone(), TextureOptions::NEAREST)
                });
                tex.set(img, TextureOptions::NEAREST);

                let disp_size = Vec2::new(img_w as f32 * self.tile_zoom, img_h as f32 * self.tile_zoom);
                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    let resp = ui.add(egui::Image::new((tex.id(), disp_size)).sense(Sense::click()));
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if resp.clicked() {
                            let rel = pos - resp.rect.min;
                            let col = ((rel.x / self.tile_zoom) as usize) / 8;
                            let row = ((rel.y / self.tile_zoom) as usize) / 8;
                            let tid = row * cols + col;
                            if tid < num_tiles {
                                self.selected_tile = Some(tid);
                            }
                        }
                    }
                });

                if let Some(tid) = self.selected_tile {
                    let vaddr = 0x0600_0000 + cbb_base + tid * tile_bytes;
                    ui.monospace(format!("Selected Tile #{tid} | VRAM Address: 0x{vaddr:08X}"));
                }
            });
        self.show_vram = open;
    }

    fn draw_bg_window(&mut self, ctx: &egui::Context, core: &GbaCore, color_correction: bool) {
        let mut open = self.show_bg;
        egui::Window::new("BG Map Viewer")
            .open(&mut open)
            .default_size([560.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for bg in 0..4usize {
                        ui.selectable_value(&mut self.bg_layer, bg, format!("BG{bg}"));
                    }
                });

                let dispcnt = core.bus.ppu.dispcnt;
                let mode = dispcnt & 7;
                let bgcnt = core.bus.ppu.bgcnt[self.bg_layer];
                let prio = bgcnt & 3;
                let char_base = ((bgcnt >> 2) & 3) as usize * 0x4000;
                let screen_base = ((bgcnt >> 8) & 0x1F) as usize * 0x800;
                let is_8bpp = (bgcnt & (1 << 7)) != 0;
                let size_flag = ((bgcnt >> 14) & 3) as usize;
                let enabled = (dispcnt & (1 << (8 + self.bg_layer))) != 0;
                let hofs = core.bus.ppu.bghofs[self.bg_layer];
                let vofs = core.bus.ppu.bgvofs[self.bg_layer];

                ui.monospace(format!(
                    "Mode {mode} | Enabled: {enabled} | Prio: {prio} | CBB: 0x{:05X} | SBB: 0x{:05X} | {} | Scroll: ({hofs}, {vofs})",
                    char_base,
                    screen_base,
                    if is_8bpp { "8bpp" } else { "4bpp" }
                ));
                ui.separator();

                // Determine dimensions in pixels cleanly for text mode vs bitmap/affine BG2/BG3
                let is_bitmap_bg2 = self.bg_layer == 2 && (3..=5).contains(&mode);
                let (map_w, map_h) = if is_bitmap_bg2 {
                    match mode {
                        3 | 4 => (240usize, 160usize),
                        _ => (160usize, 128usize),
                    }
                } else {
                    match size_flag {
                        0 => (256, 256),
                        1 => (512, 256),
                        2 => (256, 512),
                        _ => (512, 512),
                    }
                };

                let mut rgba = vec![0u8; map_w * map_h * 4];
                if is_bitmap_bg2 {
                    for y in 0..map_h {
                        for x in 0..map_w {
                            let dst = (y * map_w + x) * 4;
                            let raw = match mode {
                                3 | 5 => {
                                    let off = (y * map_w + x) * 2;
                                    u16::from_le_bytes([
                                        core.bus.ppu.vram[off],
                                        core.bus.ppu.vram[off + 1],
                                    ])
                                }
                                _ => {
                                    let frame_off = if (dispcnt & 0x10) != 0 { 0xA000 } else { 0 };
                                    let pidx = core.bus.ppu.vram[frame_off + y * map_w + x] as usize;
                                    read_pram_u16(&core.bus.ppu.pram, pidx)
                                }
                            };
                            rgba[dst..dst + 4].copy_from_slice(&bgr555_to_rgba(raw, color_correction));
                        }
                    }
                } else {
                    let bd_raw = read_pram_u16(&core.bus.ppu.pram, 0);
                    let bd_col = bgr555_to_rgba(bd_raw, color_correction);
                    let tiles_x = map_w / 8;
                    let tiles_y = map_h / 8;
                    for ty in 0..tiles_y {
                        for tx in 0..tiles_x {
                            let sc_block = match size_flag {
                                1 => usize::from(tx >= 32),
                                2 => usize::from(ty >= 32),
                                3 => usize::from(tx >= 32) + usize::from(ty >= 32) * 2,
                                _ => 0,
                            };
                            let ltx = tx & 31;
                            let lty = ty & 31;
                            let entry_off = (screen_base + sc_block * 0x800 + (lty * 32 + ltx) * 2)
                                % core.bus.ppu.vram.len();
                            let entry = u16::from_le_bytes([
                                core.bus.ppu.vram[entry_off],
                                core.bus.ppu.vram[entry_off + 1],
                            ]);
                            let tile_id = (entry & 0x03FF) as usize;
                            let hflip = (entry & 0x0400) != 0;
                            let vflip = (entry & 0x0800) != 0;
                            let pal_bank = ((entry >> 12) & 0xF) as usize;

                            for py in 0..8usize {
                                let sy = if vflip { 7 - py } else { py };
                                for px in 0..8usize {
                                    let sx = if hflip { 7 - px } else { px };
                                    let pidx = if is_8bpp {
                                        let off = char_base + tile_id * 64 + sy * 8 + sx;
                                        if off < 0x10000 {
                                            core.bus.ppu.vram[off] as usize
                                        } else {
                                            0
                                        }
                                    } else {
                                        let off = char_base + tile_id * 32 + sy * 4 + (sx / 2);
                                        if off < 0x10000 {
                                            let b = core.bus.ppu.vram[off];
                                            let nib = if (sx & 1) == 0 { b & 0xF } else { b >> 4 } as usize;
                                            if nib == 0 { 0 } else { pal_bank * 16 + nib }
                                        } else {
                                            0
                                        }
                                    };
                                    let dst = ((ty * 8 + py) * map_w + (tx * 8 + px)) * 4;
                                    if pidx == 0 {
                                        rgba[dst..dst + 4].copy_from_slice(&bd_col);
                                    } else {
                                        let raw = read_pram_u16(&core.bus.ppu.pram, pidx);
                                        rgba[dst..dst + 4]
                                            .copy_from_slice(&bgr555_to_rgba(raw, color_correction));
                                    }
                                }
                            }
                        }
                    }
                }

                let img = ColorImage::from_rgba_unmultiplied([map_w, map_h], &rgba);
                let tex = self.bg_tex.get_or_insert_with(|| {
                    ctx.load_texture("bg_map_view", img.clone(), TextureOptions::NEAREST)
                });
                tex.set(img, TextureOptions::NEAREST);

                egui::ScrollArea::both().show(ui, |ui| {
                    let scale = if map_w > 256 { 1.0 } else { 1.5 };
                    let size = Vec2::new(map_w as f32 * scale, map_h as f32 * scale);
                    let resp = ui.image((tex.id(), size));
                    // Draw 240x160 hardware viewport scroll box
                    if !is_bitmap_bg2 {
                        let rx = (hofs as usize % map_w) as f32 * scale;
                        let ry = (vofs as usize % map_h) as f32 * scale;
                        let vp_rect = Rect::from_min_size(
                            Pos2::new(resp.rect.left() + rx, resp.rect.top() + ry),
                            Vec2::new(240.0 * scale, 160.0 * scale),
                        );
                        ui.painter().rect_stroke(
                            vp_rect,
                            CornerRadius::ZERO,
                            Stroke::new(1.5_f32, ACCENT_TEAL),
                            egui::StrokeKind::Outside,
                        );
                    }
                });
            });
        self.show_bg = open;
    }

    fn draw_oam_window(&mut self, ctx: &egui::Context, core: &GbaCore, color_correction: bool) {
        let mut open = self.show_oam;
        egui::Window::new("OAM Sprite Viewer")
            .open(&mut open)
            .default_size([620.0, 460.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.hide_disabled_sprites, "Hide Disabled/Blank Sprites");
                    ui.label(
                        RichText::new(if (core.bus.ppu.dispcnt & 0x40) != 0 {
                            "OBJ Mapping: 1D"
                        } else {
                            "OBJ Mapping: 2D"
                        })
                        .color(TEXT_MUTED),
                    );
                });
                ui.separator();

                ui.columns(2, |cols| {
                    let list_ui = &mut cols[0];
                    egui::ScrollArea::vertical().max_height(380.0).show(list_ui, |ui| {
                        for idx in 0..128usize {
                            let base = idx * 8;
                            let a0 = u16::from_le_bytes([core.bus.ppu.oam[base], core.bus.ppu.oam[base + 1]]);
                            let obj_mode_rot = (a0 >> 8) & 3;
                            if self.hide_disabled_sprites && obj_mode_rot == 2 {
                                continue;
                            }
                            let a1 = u16::from_le_bytes([core.bus.ppu.oam[base + 2], core.bus.ppu.oam[base + 3]]);
                            let a2 = u16::from_le_bytes([core.bus.ppu.oam[base + 4], core.bus.ppu.oam[base + 5]]);
                            let (w, h) = obj_dimensions((a0 >> 14) & 3, (a1 >> 14) & 3);
                            let x = a1 & 0x1FF;
                            let y = a0 & 0xFF;
                            let tile = a2 & 0x03FF;
                            let lbl = format!(
                                "#{idx:03}  ({x:>3},{y:>3})  {w:>2}x{h:<2}  Tile {tile:03X}"
                            );
                            if ui.selectable_label(self.selected_sprite == idx, RichText::new(lbl).monospace()).clicked() {
                                self.selected_sprite = idx;
                            }
                        }
                    });

                    // Right column: Sprite Preview & Detailed Flags
                    let det_ui = &mut cols[1];
                    let idx = self.selected_sprite.min(127);
                    let base = idx * 8;
                    let a0 = u16::from_le_bytes([core.bus.ppu.oam[base], core.bus.ppu.oam[base + 1]]);
                    let a1 = u16::from_le_bytes([core.bus.ppu.oam[base + 2], core.bus.ppu.oam[base + 3]]);
                    let a2 = u16::from_le_bytes([core.bus.ppu.oam[base + 4], core.bus.ppu.oam[base + 5]]);
                    let (w, h) = obj_dimensions((a0 >> 14) & 3, (a1 >> 14) & 3);
                    let obj_mode_rot = (a0 >> 8) & 3;
                    let is_8bpp = (a0 & (1 << 13)) != 0;
                    let tile_id = (a2 & 0x03FF) as usize;
                    let prio = (a2 >> 10) & 3;
                    let pal_bank = ((a2 >> 12) & 0xF) as usize;
                    let mapping_1d = (core.bus.ppu.dispcnt & 0x40) != 0;
                    let hflip = obj_mode_rot == 0 && (a1 & (1 << 12)) != 0;
                    let vflip = obj_mode_rot == 0 && (a1 & (1 << 13)) != 0;

                    let mut rgba = vec![0u8; w * h * 4];
                    for py in 0..h {
                        let sy = if vflip { h - 1 - py } else { py };
                        let ty = sy / 8;
                        let in_ty = sy % 8;
                        for px in 0..w {
                            let sx = if hflip { w - 1 - px } else { px };
                            let tx = sx / 8;
                            let in_tx = sx % 8;
                            let tile_off = if mapping_1d {
                                let step = if is_8bpp { 2 } else { 1 };
                                tile_id + ty * (w / 8) * step + tx * step
                            } else {
                                let row_stride = 32;
                                let step = if is_8bpp { 2 } else { 1 };
                                tile_id + ty * row_stride + tx * step
                            };
                            let byte_base = 0x10000 + tile_off * 32;
                            let pidx = if is_8bpp {
                                let off = byte_base + in_ty * 8 + in_tx;
                                if off < core.bus.ppu.vram.len() {
                                    core.bus.ppu.vram[off] as usize
                                } else {
                                    0
                                }
                            } else {
                                let off = byte_base + in_ty * 4 + (in_tx / 2);
                                if off < core.bus.ppu.vram.len() {
                                    let b = core.bus.ppu.vram[off];
                                    if (in_tx & 1) == 0 {
                                        (b & 0xF) as usize
                                    } else {
                                        (b >> 4) as usize
                                    }
                                } else {
                                    0
                                }
                            };
                            let dst = (py * w + px) * 4;
                            if pidx == 0 {
                                let cb = checkerboard_color(px, py);
                                rgba[dst] = cb.r();
                                rgba[dst + 1] = cb.g();
                                rgba[dst + 2] = cb.b();
                                rgba[dst + 3] = 255;
                            } else {
                                let real_pidx = 256 + if is_8bpp { pidx } else { pal_bank * 16 + pidx };
                                let raw = read_pram_u16(&core.bus.ppu.pram, real_pidx);
                                rgba[dst..dst + 4].copy_from_slice(&bgr555_to_rgba(raw, color_correction));
                            }
                        }
                    }

                    let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    let tex = self.sprite_tex.get_or_insert_with(|| {
                        ctx.load_texture("oam_sprite_preview", img.clone(), TextureOptions::NEAREST)
                    });
                    tex.set(img, TextureOptions::NEAREST);

                    det_ui.label(RichText::new(format!("Sprite #{idx} Preview ({w}x{h})")).strong());
                    let scale = (96.0 / (w.max(h) as f32)).clamp(1.5, 4.0);
                    det_ui.image((tex.id(), Vec2::new(w as f32 * scale, h as f32 * scale)));
                    det_ui.separator();
                    let raw_x = a1 & 0x1FF;
                    let signed_x = if raw_x >= 256 { raw_x as i32 - 512 } else { raw_x as i32 };
                    let y = a0 & 0xFF;
                    det_ui.monospace(format!("Coords: ({signed_x}, {y})  Size: {w}x{h}"));
                    det_ui.monospace(format!(
                        "Tile: 0x{tile_id:03X}  Prio: {prio}  Color: {}",
                        if is_8bpp { "8bpp".to_string() } else { format!("4bpp Pal {pal_bank}") }
                    ));
                    let rot_str = match obj_mode_rot {
                        0 => "Normal",
                        1 => "Affine",
                        2 => "Disabled",
                        _ => "Affine Double",
                    };
                    let gfx_str = match (a0 >> 10) & 3 {
                        0 => "Normal",
                        1 => "Semi-Transparent",
                        2 => "OBJ Window",
                        _ => "Invalid",
                    };
                    det_ui.monospace(format!("Transform: {rot_str}  Blend: {gfx_str}"));
                    det_ui.monospace(format!("H-Flip: {hflip}  V-Flip: {vflip}"));
                });
            });
        self.show_oam = open;
    }

    fn draw_audio_window(&mut self, ctx: &egui::Context, core: &mut GbaCore) {
        let mut open = self.show_audio;
        egui::Window::new("Audio Channel Visualizer")
            .open(&mut open)
            .default_size([560.0, 440.0])
            .resizable(true)
            .show(ctx, |ui| {
                let names = [
                    ("PSG Square 1 (Sweep)", core.bus.apu.sq1.enabled),
                    ("PSG Square 2", core.bus.apu.sq2.enabled),
                    ("PSG Wave RAM", core.bus.apu.wave.enabled),
                    ("PSG Noise LFSR", core.bus.apu.noise.enabled),
                    ("Direct Sound FIFO A", (core.bus.apu.soundcnt_h & 0x0300) != 0),
                    ("Direct Sound FIFO B", (core.bus.apu.soundcnt_h & 0x3000) != 0),
                ];

                let cursor = core.bus.apu.scope_cursor;
                for (ch, (label, hw_active)) in names.iter().enumerate() {
                    let mut ch_enabled = (core.bus.apu.channel_enable_mask & (1 << ch)) != 0;
                    egui::Frame::new()
                        .fill(SURFACE_CARD)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(6.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut ch_enabled, "").changed() {
                                    if ch_enabled {
                                        core.bus.apu.channel_enable_mask |= 1 << ch;
                                    } else {
                                        core.bus.apu.channel_enable_mask &= !(1 << ch);
                                    }
                                }
                                let status_col = if *hw_active && ch_enabled {
                                    ACCENT_TEAL
                                } else {
                                    TEXT_MUTED
                                };
                                ui.label(RichText::new(*label).strong().color(status_col));
                            });

                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 34.0),
                                Sense::hover(),
                            );
                            let painter = ui.painter_at(rect);
                            painter.rect_filled(rect, CornerRadius::same(2), SURFACE_DARK);
                            let mid_y = rect.center().y;
                            painter.line_segment(
                                [Pos2::new(rect.left(), mid_y), Pos2::new(rect.right(), mid_y)],
                                Stroke::new(1.0_f32, Color32::from_rgb(35, 41, 52)),
                            );

                            let samples = &core.bus.apu.chan_scopes[ch];
                            let mut points = Vec::with_capacity(128);
                            let w = rect.width().max(1.0);
                            let half_h = (rect.height() * 0.44).max(1.0);
                            for i in 0..128usize {
                                let s = samples[(cursor + i) & 127] as f32 / 32768.0;
                                let x = rect.left() + (i as f32 / 127.0) * w;
                                let y = mid_y - s.clamp(-1.0, 1.0) * half_h;
                                points.push(Pos2::new(x, y));
                            }
                            let line_col = if ch_enabled { ACCENT_TEAL } else { Color32::from_rgb(70, 76, 90) };
                            painter.add(egui::Shape::line(points, Stroke::new(1.5_f32, line_col)));
                        });
                    ui.add_space(4.0);
                }
            });
        self.show_audio = open;
    }

    fn draw_overrides_window(&mut self, ctx: &egui::Context, core: &mut GbaCore) -> bool {
        let mut open = self.show_overrides;
        let mut changed = false;

        egui::Window::new("Hardware & Game Overrides")
            .open(&mut open)
            .default_size([420.0, 380.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Backup Save Memory").strong());
                let detected = format!("{:?}", core.bus.cart.save_type);
                ui.label(RichText::new(format!("Auto-detected: {detected}")).color(TEXT_MUTED));

                let mut choice = SaveOverrideChoice::from_core(core.bus.cart.save_type_override);
                egui::ComboBox::from_label("Save Type Override")
                    .selected_text(choice.label())
                    .show_ui(ui, |ui| {
                        for c in SaveOverrideChoice::ALL {
                            if ui.selectable_value(&mut choice, c, c.label()).changed() {
                                core.bus.cart.set_save_type_override(c.to_core());
                                changed = true;
                            }
                        }
                    });

                ui.separator();
                ui.label(RichText::new("Real-Time Clock (GPIO RTC)").strong());
                let rtc_label = match core.bus.cart.rtc_override {
                    None => format!("Auto ({})", if core.bus.cart.has_rtc { "Enabled" } else { "Disabled" }),
                    Some(true) => "Force Enabled".to_string(),
                    Some(false) => "Force Disabled".to_string(),
                };
                egui::ComboBox::from_label("RTC Override")
                    .selected_text(rtc_label)
                    .show_ui(ui, |ui| {
                        for (opt, lbl) in [
                            (None, "Auto-Detect"),
                            (Some(true), "Force Enabled"),
                            (Some(false), "Force Disabled"),
                        ] {
                            if ui
                                .selectable_value(&mut core.bus.cart.rtc_override, opt, lbl)
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });

                ui.separator();
                ui.label(RichText::new("Cartridge Sensors (Boktai / WarioWare / Yoshi)").strong());
                if ui
                    .checkbox(&mut core.bus.cart.solar_enabled, "Enable Boktai Solar Sensor")
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .add(
                        egui::Slider::new(&mut core.bus.cart.solar_lux, 0..=255)
                            .text("Solar Lux"),
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.horizontal(|ui| {
                    for (lbl, v) in [("Dark (0)", 0), ("Shade (96)", 96), ("Sun (224)", 224), ("Max (255)", 255)] {
                        if ui.button(lbl).clicked() {
                            core.bus.cart.solar_lux = v;
                            changed = true;
                        }
                    }
                });

                ui.add_space(4.0);
                if ui
                    .checkbox(&mut core.bus.cart.tilt_enabled, "Enable Yoshi Tilt Sensor")
                    .changed()
                {
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut core.bus.cart.tilt_x, 0x200..=0x540).text("Tilt X"));
                    ui.add(egui::Slider::new(&mut core.bus.cart.tilt_y, 0x200..=0x540).text("Tilt Y"));
                    if ui.button("Center").clicked() {
                        core.bus.cart.tilt_x = 0x03A0;
                        core.bus.cart.tilt_y = 0x03A0;
                    }
                });

                ui.add_space(4.0);
                if ui
                    .checkbox(&mut core.bus.cart.gyro_enabled, "Enable Gyro / Rumble")
                    .changed()
                {
                    changed = true;
                }
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut core.bus.cart.gyro_val, 0x100..=0xC80).text("Gyro"));
                    if ui.button("Zero").clicked() {
                        core.bus.cart.gyro_val = 0x06C0;
                    }
                    let rumble_col = if core.bus.cart.rumble_active {
                        ACCENT_AMBER
                    } else {
                        TEXT_MUTED
                    };
                    ui.label(RichText::new("● Rumble").color(rumble_col));
                });
            });

        self.show_overrides = open;
        changed
    }

    fn draw_cheats_window(&mut self, ctx: &egui::Context, cheats: &mut Vec<CheatEntry>) -> bool {
        let mut open = self.show_cheats;
        let mut changed = false;

        egui::Window::new("Cheat Code Manager")
            .open(&mut open)
            .default_size([520.0, 440.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(RichText::new("Active Cheats").strong());
                let mut remove_idx = None;
                egui::Frame::new()
                    .fill(SURFACE_DARK)
                    .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                            if cheats.is_empty() {
                                ui.label(RichText::new("No cheat codes added for this game.").color(TEXT_MUTED));
                            }
                            for (i, ch) in cheats.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut ch.enabled, "").changed() {
                                        changed = true;
                                    }
                                    ui.label(RichText::new(&ch.name).strong());
                                    ui.label(RichText::new(format!("[{}]", ch.format.label())).color(TEXT_MUTED));
                                    let preview = ch.code.lines().next().unwrap_or("");
                                    ui.monospace(preview);
                                    if ui.button("Delete").clicked() {
                                        remove_idx = Some(i);
                                    }
                                });
                            }
                        });
                    });
                if let Some(idx) = remove_idx {
                    cheats.remove(idx);
                    changed = true;
                }

                ui.separator();
                ui.label(RichText::new("Add New Cheat Code").strong());
                ui.horizontal(|ui| {
                    ui.label("Description:");
                    ui.text_edit_singleline(&mut self.draft_cheat_name);
                    egui::ComboBox::from_id_salt("cheat_fmt_cb")
                        .selected_text(self.draft_cheat_format.label())
                        .show_ui(ui, |ui| {
                            for fmt in CheatFormat::ALL {
                                ui.selectable_value(&mut self.draft_cheat_format, fmt, fmt.label());
                            }
                        });
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft_cheat_code)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("Enter CodeBreaker (XXXXXXXX YYYY) or GameShark / AR (XXXXXXXX YYYYYYYY) lines"),
                );

                if let Some(err) = &self.cheat_error {
                    ui.label(RichText::new(err).color(Color32::from_rgb(239, 68, 68)));
                }

                ui.horizontal(|ui| {
                    if ui.button("Add Cheat").clicked() {
                        match decode_cheat_lines(&self.draft_cheat_code, self.draft_cheat_format) {
                            Ok(acts) if !acts.is_empty() => {
                                let name = if self.draft_cheat_name.trim().is_empty() {
                                    format!("Cheat #{}", cheats.len() + 1)
                                } else {
                                    self.draft_cheat_name.trim().to_string()
                                };
                                cheats.push(CheatEntry {
                                    name,
                                    code: self.draft_cheat_code.trim().to_string(),
                                    format: self.draft_cheat_format,
                                    enabled: true,
                                });
                                self.draft_cheat_name.clear();
                                self.draft_cheat_code.clear();
                                self.cheat_error = None;
                                changed = true;
                            }
                            Ok(_) => {
                                self.cheat_error = Some("Enter at least one valid cheat line".to_string());
                            }
                            Err(e) => {
                                self.cheat_error = Some(e);
                            }
                        }
                    }
                    if ui.button("Enable All").clicked() {
                        for c in cheats.iter_mut() {
                            c.enabled = true;
                        }
                        changed = true;
                    }
                    if ui.button("Disable All").clicked() {
                        for c in cheats.iter_mut() {
                            c.enabled = false;
                        }
                        changed = true;
                    }
                });
            });

        self.show_cheats = open;
        changed
    }

    fn draw_controls_window(
        &mut self,
        ctx: &egui::Context,
        bindings: &mut KeyBindings,
        gamepad_names: &[String],
    ) -> bool {
        let mut open = self.show_controls;
        let mut changed = false;

        // Capture next key press cleanly when rebinding
        if let Some(btn) = self.awaiting_rebind {
            let captured = ctx.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        ..
                    } = ev
                    {
                        return Some(*key);
                    }
                }
                None
            });
            if let Some(key) = captured {
                if key != egui::Key::Escape {
                    bindings.set_key_name(btn, format_key_name(key));
                    changed = true;
                }
                self.awaiting_rebind = None;
            }
        }

        egui::Window::new("Controls & Keybindings")
            .open(&mut open)
            .default_size([400.0, 420.0])
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Keyboard Mapping").strong());
                egui::Frame::new()
                    .fill(SURFACE_DARK)
                    .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        egui::Grid::new("keybind_grid")
                            .num_columns(3)
                            .spacing([18.0, 6.0])
                            .show(ui, |ui| {
                                for btn in GbaButton::ALL {
                                    ui.label(btn.label());
                                    let waiting = self.awaiting_rebind == Some(btn);
                                    let btn_text = if waiting {
                                        "Press key... (Esc cancels)".to_string()
                                    } else {
                                        bindings.key_name(btn).to_string()
                                    };
                                    if ui.button(RichText::new(btn_text).monospace()).clicked() {
                                        self.awaiting_rebind = Some(btn);
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(6.0);
                if ui.button("Reset to Defaults").clicked() {
                    *bindings = KeyBindings::default();
                    self.awaiting_rebind = None;
                    changed = true;
                }

                ui.separator();
                ui.label(RichText::new("Connected Gamepads (XInput / Gilrs)").strong());
                if gamepad_names.is_empty() {
                    ui.label(
                        RichText::new("No active controllers detected — plug in an Xbox/XInput pad anytime.")
                            .color(TEXT_MUTED),
                    );
                } else {
                    for name in gamepad_names {
                        ui.label(RichText::new(format!("• {name} (Mapped Standard)")).color(ACCENT_TEAL));
                    }
                }

                ui.separator();
                ui.label(
                    RichText::new("Hotkeys: Space = Pause  |  Tab (Hold) = Fast-Forward  |  F1-F5 / Shift+F1-F5 = Save/Load  |  F11 = Fullscreen  |  F12 = Screenshot")
                        .small()
                        .color(TEXT_MUTED),
                );
            });

        self.show_controls = open;
        changed
    }

    fn draw_about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About Rustboy Advance")
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.heading("Rustboy Advance");
                ui.label(RichText::new("Version 0.1.0 — Cycle-Aware GBA Emulator & Hardware Debugger").color(TEXT_MUTED));
                ui.separator();
                ui.label("• CPU: ARM7TDMI (ARMv4T + Thumb, 3-stage pipeline, Prefetch)");
                ui.label("• PPU: Modes 0–5, Windows, Blending, Scanline & Per-Dot Renderer");
                ui.label("• APU: 4 PSG + Dual 8-bit DMA FIFOs w/ Dynamic Rate Control");
                ui.label("• Carts: SRAM, Flash 64/128K, EEPROM, GPIO RTC, Solar, Tilt, Gyro");
                ui.add_space(6.0);
                ui.label(RichText::new("Built cleanly in pure safe/minimal Rust 2024.").small().color(TEXT_MUTED));
            });
        self.show_about = open;
    }
}

const fn obj_dimensions(shape: u16, size: u16) -> (usize, usize) {
    match (shape, size) {
        (0, 0) => (8, 8),
        (0, 1) => (16, 16),
        (0, 2) => (32, 32),
        (0, 3) => (64, 64),
        (1, 0) => (16, 8),
        (1, 1) => (32, 8),
        (1, 2) => (32, 16),
        (1, 3) => (64, 32),
        (2, 0) => (8, 16),
        (2, 1) => (8, 32),
        (2, 2) => (16, 32),
        (2, 3) => (32, 64),
        _ => (8, 8),
    }
}
