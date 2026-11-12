use crate::cheats::{CheatEntry, CheatFormat};
use egui::{Color32, RichText, Stroke};
use gba_core::GbaCore;
use std::cmp::Ordering;

const PANEL_BORDER: Color32 = Color32::from_rgb(42, 48, 61);
const SURFACE_DARK: Color32 = Color32::from_rgb(13, 15, 19);
const SURFACE_CARD: Color32 = Color32::from_rgb(20, 23, 30);
const SURFACE_ERR: Color32 = Color32::from_rgb(44, 22, 26);
const BORDER_ERR: Color32 = Color32::from_rgb(180, 68, 76);
const ACCENT_TEAL: Color32 = Color32::from_rgb(46, 196, 182);
const ACCENT_AMBER: Color32 = Color32::from_rgb(224, 159, 62);
const TEXT_MUTED: Color32 = Color32::from_rgb(142, 152, 170);

/// Maximum number of result rows rendered in the UI table at once.
pub const MAX_DISPLAY_ROWS: usize = 500;

/// Selectable GBA RAM scan regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanRegion {
    /// On-board External Work RAM (`0x0200_0000..0x0204_0000`, 256 KB).
    #[default]
    Ewram,
    /// On-chip Internal Work RAM (`0x0300_0000..0x0300_8000`, 32 KB).
    Iwram,
    /// Scan both EWRAM and IWRAM together.
    Both,
}

impl ScanRegion {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ewram => "EWRAM (256 KB)",
            Self::Iwram => "IWRAM (32 KB)",
            Self::Both => "Both (EWRAM + IWRAM)",
        }
    }

    fn ranges(self) -> &'static [(u32, u32)] {
        match self {
            Self::Ewram => &[(0x0200_0000, 0x0204_0000)],
            Self::Iwram => &[(0x0300_0000, 0x0300_8000)],
            Self::Both => &[(0x0200_0000, 0x0204_0000), (0x0300_0000, 0x0300_8000)],
        }
    }
}

/// Scalar integer interpretation for memory scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanValueType {
    U8,
    I8,
    #[default]
    U16,
    I16,
    U32,
    I32,
}

impl ScanValueType {
    pub const ALL: [Self; 6] = [
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::U8 => "u8 (8-bit)",
            Self::I8 => "i8 (8-bit signed)",
            Self::U16 => "u16 (16-bit)",
            Self::I16 => "i16 (16-bit signed)",
            Self::U32 => "u32 (32-bit)",
            Self::I32 => "i32 (32-bit signed)",
        }
    }

    pub fn byte_width(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 => 4,
        }
    }

    pub fn format_value(self, raw: u32, hex_display: bool) -> String {
        if hex_display {
            match self {
                Self::U8 | Self::I8 => format!("0x{:02X}", raw as u8),
                Self::U16 | Self::I16 => format!("0x{:04X}", raw as u16),
                Self::U32 | Self::I32 => format!("0x{raw:08X}"),
            }
        } else {
            match self {
                Self::U8 => format!("{}", raw as u8),
                Self::I8 => format!("{}", raw as i8),
                Self::U16 => format!("{}", raw as u16),
                Self::I16 => format!("{}", raw as i16),
                Self::U32 => format!("{raw}"),
                Self::I32 => format!("{}", raw as i32),
            }
        }
    }
}

/// Comparison operator used to seed or narrow candidate addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanCompare {
    #[default]
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    Changed,
    Unchanged,
    Increased,
    Decreased,
}

impl ScanCompare {
    pub const ALL: [Self; 8] = [
        Self::Equal,
        Self::NotEqual,
        Self::LessThan,
        Self::GreaterThan,
        Self::Changed,
        Self::Unchanged,
        Self::Increased,
        Self::Decreased,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "Equal (==)",
            Self::NotEqual => "Not Equal (!=)",
            Self::LessThan => "Less Than (<)",
            Self::GreaterThan => "Greater Than (>)",
            Self::Changed => "Changed",
            Self::Unchanged => "Unchanged",
            Self::Increased => "Increased",
            Self::Decreased => "Decreased",
        }
    }

    pub fn needs_input(self) -> bool {
        matches!(
            self,
            Self::Equal | Self::NotEqual | Self::LessThan | Self::GreaterThan
        )
    }
}

/// A single matching RAM address retained across scan passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchCandidate {
    pub addr: u32,
    pub cur_raw: u32,
    pub prev_raw: u32,
    pub hit_count: u32,
}

/// UI action requested by the Memory Search window on the host shell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MemSearchAction {
    #[default]
    None,
    /// Append an active freeze cheat directly to the desktop cheat table.
    AddCheat(CheatEntry),
    /// Open and scroll the Hex Viewer to the selected RAM address.
    JumpHex(u32),
}

/// Parses a user search literal according to `value_type` and optional hex toggle.
pub fn parse_scan_value(
    input: &str,
    value_type: ScanValueType,
    hex_mode: bool,
) -> Result<u32, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(String::from("Enter a numeric value for this comparison."));
    }

    let explicit_hex = trimmed.starts_with("0x") || trimmed.starts_with("0X");
    if hex_mode || explicit_hex {
        let digits = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        let parsed = u64::from_str_radix(digits, 16)
            .map_err(|_| format!("Invalid hexadecimal value '{trimmed}'."))?;
        let max_val = match value_type.byte_width() {
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        if parsed > max_val {
            return Err(format!(
                "Hex value 0x{parsed:X} exceeds {} width.",
                value_type.label()
            ));
        }
        return Ok(parsed as u32);
    }

    match value_type {
        ScanValueType::U8 => trimmed
            .parse::<u8>()
            .map(|v| v as u32)
            .map_err(|_| format!("Expected u8 (0..=255), got '{trimmed}'.")),
        ScanValueType::I8 => trimmed
            .parse::<i8>()
            .map(|v| (v as u8) as u32)
            .map_err(|_| format!("Expected i8 (-128..=127), got '{trimmed}'.")),
        ScanValueType::U16 => trimmed
            .parse::<u16>()
            .map(|v| v as u32)
            .map_err(|_| format!("Expected u16 (0..=65535), got '{trimmed}'.")),
        ScanValueType::I16 => trimmed
            .parse::<i16>()
            .map(|v| (v as u16) as u32)
            .map_err(|_| format!("Expected i16 (-32768..=32767), got '{trimmed}'.")),
        ScanValueType::U32 => trimmed
            .parse::<u32>()
            .map_err(|_| format!("Expected u32 (0..=4294967295), got '{trimmed}'.")),
        ScanValueType::I32 => trimmed
            .parse::<i32>()
            .map(|v| v as u32)
            .map_err(|_| format!("Expected i32 signed integer, got '{trimmed}'.")),
    }
}

/// Formats a RAM write cheat compatible with [`CheatFormat::AutoRaw`].
pub fn format_autoraw_cheat(addr: u32, raw_val: u32, value_type: ScanValueType) -> CheatEntry {
    let masked = addr & 0x0FFF_FFFF;
    let code = match value_type.byte_width() {
        1 => format!("{:08X} {:04X}", 0x3000_0000 | masked, raw_val as u8),
        2 => format!("{:08X} {:04X}", 0x8000_0000 | masked, raw_val as u16),
        _ => format!("{:08X} {:08X}", 0x2000_0000 | masked, raw_val),
    };
    CheatEntry {
        name: format!("RAM 0x{addr:08X}"),
        code,
        format: CheatFormat::AutoRaw,
        enabled: true,
    }
}

fn read_typed(core: &GbaCore, addr: u32, ty: ScanValueType) -> u32 {
    match ty.byte_width() {
        1 => core.peek8(addr) as u32,
        2 => core.peek16(addr) as u32,
        _ => core.peek32(addr),
    }
}

fn cmp_typed(a: u32, b: u32, ty: ScanValueType) -> Ordering {
    match ty {
        ScanValueType::U8 => (a as u8).cmp(&(b as u8)),
        ScanValueType::I8 => (a as i8).cmp(&(b as i8)),
        ScanValueType::U16 => (a as u16).cmp(&(b as u16)),
        ScanValueType::I16 => (a as i16).cmp(&(b as i16)),
        ScanValueType::U32 => a.cmp(&b),
        ScanValueType::I32 => (a as i32).cmp(&(b as i32)),
    }
}

fn matches_compare(
    cur: u32,
    prev: u32,
    target: Option<u32>,
    op: ScanCompare,
    ty: ScanValueType,
) -> bool {
    match op {
        ScanCompare::Equal => target.is_some_and(|t| cmp_typed(cur, t, ty) == Ordering::Equal),
        ScanCompare::NotEqual => target.is_some_and(|t| cmp_typed(cur, t, ty) != Ordering::Equal),
        ScanCompare::LessThan => target.is_some_and(|t| cmp_typed(cur, t, ty) == Ordering::Less),
        ScanCompare::GreaterThan => {
            target.is_some_and(|t| cmp_typed(cur, t, ty) == Ordering::Greater)
        }
        ScanCompare::Changed => cmp_typed(cur, prev, ty) != Ordering::Equal,
        ScanCompare::Unchanged => cmp_typed(cur, prev, ty) == Ordering::Equal,
        ScanCompare::Increased => cmp_typed(cur, prev, ty) == Ordering::Greater,
        ScanCompare::Decreased => cmp_typed(cur, prev, ty) == Ordering::Less,
    }
}

/// Interactive Memory Search / Cheat Finder window state.
pub struct MemSearchWindow {
    pub open: bool,
    pub region: ScanRegion,
    pub value_type: ScanValueType,
    pub compare: ScanCompare,
    pub value_input: String,
    pub hex_input: bool,
    pub hex_display: bool,
    pub candidates: Vec<SearchCandidate>,
    pub has_scanned: bool,
    pub scan_passes: u32,
    pub error_msg: Option<String>,
    pub toast_msg: Option<String>,
}

impl Default for MemSearchWindow {
    fn default() -> Self {
        Self {
            open: false,
            region: ScanRegion::Ewram,
            value_type: ScanValueType::U16,
            compare: ScanCompare::Equal,
            value_input: String::from("0"),
            hex_input: false,
            hex_display: false,
            candidates: Vec::new(),
            has_scanned: false,
            scan_passes: 0,
            error_msg: None,
            toast_msg: None,
        }
    }
}

impl MemSearchWindow {
    /// Resets scan results and error messages while preserving selected configuration.
    pub fn reset_scan(&mut self) {
        self.candidates.clear();
        self.has_scanned = false;
        self.scan_passes = 0;
        self.error_msg = None;
        self.toast_msg = None;
    }

    /// Captures an initial baseline across the selected RAM region(s).
    ///
    /// For relational/relative operators (`Changed`, `Unchanged`, `Increased`, `Decreased`),
    /// captures all aligned addresses in the region as the baseline snapshot.
    pub fn run_new_scan(&mut self, core: &GbaCore) -> Result<usize, String> {
        self.error_msg = None;
        self.toast_msg = None;

        let target = if self.compare.needs_input() {
            Some(parse_scan_value(
                &self.value_input,
                self.value_type,
                self.hex_input,
            )?)
        } else {
            None
        };

        let stride = self.value_type.byte_width();
        self.candidates.clear();

        for &(start, end) in self.region.ranges() {
            let mut addr = start;
            while addr + stride <= end {
                let cur = read_typed(core, addr, self.value_type);
                let keep = match target {
                    Some(_) => matches_compare(cur, cur, target, self.compare, self.value_type),
                    None => true, // Unknown initial snapshot for relative next scans
                };
                if keep {
                    self.candidates.push(SearchCandidate {
                        addr,
                        cur_raw: cur,
                        prev_raw: cur,
                        hit_count: 1,
                    });
                }
                addr += stride;
            }
        }

        self.has_scanned = true;
        self.scan_passes = 1;
        Ok(self.candidates.len())
    }

    /// Narrows existing candidates by re-reading live RAM and applying `self.compare`.
    pub fn run_next_scan(&mut self, core: &GbaCore) -> Result<usize, String> {
        self.error_msg = None;
        self.toast_msg = None;

        if !self.has_scanned {
            return self.run_new_scan(core);
        }

        let target = if self.compare.needs_input() {
            Some(parse_scan_value(
                &self.value_input,
                self.value_type,
                self.hex_input,
            )?)
        } else {
            None
        };

        let ty = self.value_type;
        let cmp = self.compare;
        let mut next = Vec::with_capacity(self.candidates.len().min(4096));

        for cand in &self.candidates {
            let live = read_typed(core, cand.addr, ty);
            if matches_compare(live, cand.cur_raw, target, cmp, ty) {
                next.push(SearchCandidate {
                    addr: cand.addr,
                    cur_raw: live,
                    prev_raw: cand.cur_raw,
                    hit_count: cand.hit_count.saturating_add(1),
                });
            }
        }

        self.candidates = next;
        self.scan_passes = self.scan_passes.saturating_add(1);
        Ok(self.candidates.len())
    }

    /// Renders the floating Memory Search / Cheat Finder window.
    pub fn show(&mut self, ctx: &egui::Context, core: Option<&GbaCore>) -> MemSearchAction {
        if !self.open {
            return MemSearchAction::None;
        }

        let mut open = self.open;
        let mut action = MemSearchAction::None;

        egui::Window::new("Memory Search / Cheat Finder")
            .open(&mut open)
            .default_size([680.0, 520.0])
            .resizable(true)
            .show(ctx, |ui| {
                let Some(core) = core else {
                    egui::Frame::NONE
                        .fill(SURFACE_CARD)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(16.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("No ROM Loaded")
                                    .color(ACCENT_AMBER)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new("Open a GBA ROM first to search EWRAM and IWRAM.")
                                    .color(TEXT_MUTED),
                            );
                        });
                    return;
                };

                // Controls card
                egui::Frame::NONE
                    .fill(SURFACE_CARD)
                    .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                    .inner_margin(10.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Region:").color(TEXT_MUTED));
                            egui::ComboBox::from_id_salt("ms_region")
                                .selected_text(self.region.label())
                                .show_ui(ui, |ui| {
                                    for r in [ScanRegion::Ewram, ScanRegion::Iwram, ScanRegion::Both]
                                    {
                                        ui.selectable_value(&mut self.region, r, r.label());
                                    }
                                });

                            ui.separator();
                            ui.label(RichText::new("Type:").color(TEXT_MUTED));
                            let prev_type = self.value_type;
                            egui::ComboBox::from_id_salt("ms_type")
                                .selected_text(self.value_type.label())
                                .show_ui(ui, |ui| {
                                    for ty in ScanValueType::ALL {
                                        ui.selectable_value(&mut self.value_type, ty, ty.label());
                                    }
                                });
                            if self.value_type != prev_type && self.has_scanned {
                                // Width/signedness changed -> require fresh baseline.
                                self.reset_scan();
                            }

                            ui.separator();
                            ui.checkbox(&mut self.hex_input, "Hex Input");
                            ui.checkbox(&mut self.hex_display, "Hex View");
                        });

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Compare:").color(TEXT_MUTED));
                            egui::ComboBox::from_id_salt("ms_cmp")
                                .selected_text(self.compare.label())
                                .show_ui(ui, |ui| {
                                    for c in ScanCompare::ALL {
                                        ui.selectable_value(&mut self.compare, c, c.label());
                                    }
                                });

                            let needs_val = self.compare.needs_input();
                            ui.add_enabled_ui(needs_val, |ui| {
                                ui.label(RichText::new("Value:").color(TEXT_MUTED));
                                let edit = ui.add(
                                    egui::TextEdit::singleline(&mut self.value_input)
                                        .desired_width(120.0)
                                        .hint_text(if self.hex_input { "0x00FF" } else { "100" }),
                                );
                                if edit.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    let res = if self.has_scanned {
                                        self.run_next_scan(core)
                                    } else {
                                        self.run_new_scan(core)
                                    };
                                    if let Err(err) = res {
                                        self.error_msg = Some(err);
                                    }
                                }
                            });

                            ui.separator();
                            if ui
                                .button(RichText::new("New Scan").color(ACCENT_TEAL).strong())
                                .clicked()
                                && let Err(err) = self.run_new_scan(core)
                            {
                                self.error_msg = Some(err);
                            }

                            if ui
                                .add_enabled(self.has_scanned, egui::Button::new("Next Scan"))
                                .clicked()
                                && let Err(err) = self.run_next_scan(core)
                            {
                                self.error_msg = Some(err);
                            }

                            if ui.button("Reset").clicked() {
                                self.reset_scan();
                            }
                        });
                    });

                // Validation / status toast bar
                if let Some(err) = self.error_msg.clone() {
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(SURFACE_ERR)
                        .stroke(Stroke::new(1.0_f32, BORDER_ERR))
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&err).color(Color32::from_rgb(245, 130, 135)),
                                );
                                if ui.small_button("Dismiss").clicked() {
                                    self.error_msg = None;
                                }
                            });
                        });
                } else if let Some(toast) = &self.toast_msg {
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(SURFACE_CARD)
                        .stroke(Stroke::new(1.0_f32, ACCENT_TEAL))
                        .inner_margin(8.0)
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(toast).color(ACCENT_TEAL));
                        });
                }

                ui.add_space(6.0);

                // Results badge row
                let total = self.candidates.len();
                let shown = total.min(MAX_DISPLAY_ROWS);
                ui.horizontal(|ui| {
                    if !self.has_scanned {
                        ui.label(RichText::new("Ready").color(TEXT_MUTED));
                    } else {
                        egui::Frame::NONE
                            .fill(SURFACE_CARD)
                            .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .corner_radius(4.0)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("Matches: {total}"))
                                        .color(if total == 0 { ACCENT_AMBER } else { ACCENT_TEAL })
                                        .strong(),
                                );
                            });
                        ui.label(
                            RichText::new(format!(
                                "Pass #{}  |  Showing {shown} of {total}",
                                self.scan_passes
                            ))
                            .color(TEXT_MUTED),
                        );
                    }
                });

                ui.add_space(6.0);

                // Empty state or results table
                if !self.has_scanned {
                    egui::Frame::NONE
                        .fill(SURFACE_DARK)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(16.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("No scan active yet.")
                                    .color(ACCENT_TEAL)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(
                                    "Pick a RAM region, scalar width, comparison operator, and click New Scan. For Changed/Unchanged/Increased/Decreased, New Scan captures the entire region as a baseline.",
                                )
                                .color(TEXT_MUTED),
                            );
                        });
                    return;
                }

                if self.candidates.is_empty() {
                    egui::Frame::NONE
                        .fill(SURFACE_DARK)
                        .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                        .inner_margin(16.0)
                        .corner_radius(6.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("0 matching addresses remain.")
                                    .color(ACCENT_AMBER)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new("Click Reset or New Scan to start a fresh search.")
                                    .color(TEXT_MUTED),
                            );
                        });
                    return;
                }

                egui::Frame::NONE
                    .fill(SURFACE_DARK)
                    .stroke(Stroke::new(1.0_f32, PANEL_BORDER))
                    .inner_margin(6.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        let col_addr = 100.0;
                        let col_val = 115.0;
                        let col_hits = 56.0;
                        ui.horizontal(|ui| {
                            ui.add_sized([col_addr, 18.0], egui::Label::new(RichText::new("Address").color(TEXT_MUTED).strong()));
                            ui.add_sized([col_val, 18.0], egui::Label::new(RichText::new("Current").color(TEXT_MUTED).strong()));
                            ui.add_sized([col_val, 18.0], egui::Label::new(RichText::new("Previous").color(TEXT_MUTED).strong()));
                            ui.add_sized([col_hits, 18.0], egui::Label::new(RichText::new("Hits").color(TEXT_MUTED).strong()));
                            ui.label(RichText::new("Actions").color(TEXT_MUTED).strong());
                        });
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("mem_search_results")
                            .auto_shrink([false, false])
                            .show_rows(ui, 22.0, shown, |ui, range| {
                                for (idx, row) in self.candidates[range].iter().enumerate() {
                                    let live_val = read_typed(core, row.addr, self.value_type);
                                    ui.push_id(idx, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(
                                                [col_addr, 18.0],
                                                egui::Label::new(
                                                    RichText::new(format!("0x{:08X}", row.addr))
                                                        .monospace(),
                                                ),
                                            );
                                            ui.add_sized(
                                                [col_val, 18.0],
                                                egui::Label::new(
                                                    RichText::new(
                                                        self.value_type
                                                            .format_value(live_val, self.hex_display),
                                                    )
                                                    .monospace()
                                                    .color(ACCENT_TEAL),
                                                ),
                                            );
                                            ui.add_sized(
                                                [col_val, 18.0],
                                                egui::Label::new(
                                                    RichText::new(
                                                        self.value_type
                                                            .format_value(row.prev_raw, self.hex_display),
                                                    )
                                                    .monospace(),
                                                ),
                                            );
                                            ui.add_sized(
                                                [col_hits, 18.0],
                                                egui::Label::new(
                                                    RichText::new(format!("{}", row.hit_count))
                                                        .monospace(),
                                                ),
                                            );
                                            if ui.small_button("+ Cheat").clicked() {
                                                let entry = format_autoraw_cheat(
                                                    row.addr,
                                                    live_val,
                                                    self.value_type,
                                                );
                                                self.toast_msg = Some(format!(
                                                    "Added cheat {} ({})",
                                                    entry.name, entry.code
                                                ));
                                                action = MemSearchAction::AddCheat(entry);
                                            }
                                            if ui.small_button("Hex").clicked() {
                                                action = MemSearchAction::JumpHex(row.addr);
                                            }
                                        });
                                    });
                                }
                            });
                    });
            });

        self.open = open;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cheats::{CheatAction, decode_cheat_lines};

    #[test]
    fn parses_signed_unsigned_and_hex_values() {
        assert_eq!(parse_scan_value("42", ScanValueType::U8, false), Ok(42));
        assert!(parse_scan_value("256", ScanValueType::U8, false).is_err());
        assert_eq!(
            parse_scan_value("-5", ScanValueType::I8, false),
            Ok(0xFB)
        );
        assert_eq!(
            parse_scan_value("0x1A2B", ScanValueType::U16, false),
            Ok(0x1A2B)
        );
        assert_eq!(
            parse_scan_value("DEAD", ScanValueType::U16, true),
            Ok(0xDEAD)
        );
        assert!(parse_scan_value("", ScanValueType::U32, false).is_err());
    }

    #[test]
    fn narrows_ewram_and_iwram_across_new_and_next_scans() {
        let mut core = GbaCore::new(None, vec![0u8; 0x200]);
        core.poke16(0x0200_0100, 50);
        core.poke16(0x0200_0200, 50);
        core.poke16(0x0300_0040, 50);

        let mut win = MemSearchWindow {
            region: ScanRegion::Both,
            value_type: ScanValueType::U16,
            compare: ScanCompare::Equal,
            value_input: String::from("50"),
            ..Default::default()
        };

        let first = win.run_new_scan(&core).unwrap();
        assert_eq!(first, 3);

        core.poke16(0x0200_0100, 75);
        core.poke16(0x0200_0200, 40);
        core.poke16(0x0300_0040, 75);
        win.compare = ScanCompare::Increased;

        let second = win.run_next_scan(&core).unwrap();
        assert_eq!(second, 2);
        assert!(win.candidates.iter().any(|c| c.addr == 0x0200_0100 && c.hit_count == 2));
        assert!(win.candidates.iter().any(|c| c.addr == 0x0300_0040 && c.hit_count == 2));
    }

    #[test]
    fn generates_valid_autoraw_cheats_for_all_widths() {
        let c8 = format_autoraw_cheat(0x0200_0120, 0x7B, ScanValueType::U8);
        assert_eq!(
            decode_cheat_lines(&c8.code, c8.format),
            Ok(vec![CheatAction::Write8 {
                addr: 0x0200_0120,
                val: 0x7B
            }])
        );

        let c16 = format_autoraw_cheat(0x0300_0040, 0x03E7, ScanValueType::U16);
        assert_eq!(
            decode_cheat_lines(&c16.code, c16.format),
            Ok(vec![CheatAction::Write16 {
                addr: 0x0300_0040,
                val: 0x03E7
            }])
        );

        let c32 = format_autoraw_cheat(0x0200_0400, 0x1234_5678, ScanValueType::U32);
        assert_eq!(
            decode_cheat_lines(&c32.code, c32.format),
            Ok(vec![CheatAction::Write32 {
                addr: 0x0200_0400,
                val: 0x1234_5678
            }])
        );
    }
}
