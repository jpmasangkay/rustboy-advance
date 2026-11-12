use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke};

use crate::app::RustboyApp;
use crate::theme::*;
use crate::rom_loader::{
    detect_rom_platform, gb_cart_type_label, gb_short_game_code,
    is_supported_rom_or_archive_extension, read_rom_header_summary, RomPlatform, RomSourceKind,
};

#[derive(Clone, Debug)]
pub struct RomLibraryEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub internal_title: String,
    pub game_code: String,
    pub region: &'static str,
    pub size_mb: f32,
    #[allow(dead_code)]
    pub has_battery_sav: bool,
    #[allow(dead_code)]
    pub source_kind: RomSourceKind,
    pub platform: RomPlatform,
    pub cart_type_label: String,
}

pub(crate) struct LibraryScanResult {
    pub(crate) entries: Vec<RomLibraryEntry>,
    pub(crate) error: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum LibraryViewMode {
    Table,
    Grid,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum TableSortColumn {
    #[default]
    Title,
    Region,
    Size,
    LastPlayed,
    PlayTime,
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn format_last_played_ts(ts: u64) -> String {
    if ts == 0 {
        return "-".to_string();
    }
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let days = (ts / 86400) as i64;
    let now_days = (now_ts / 86400) as i64;
    let time_of_day = ts % 86400;
    let hour = time_of_day / 3600;
    let min = (time_of_day % 3600) / 60;

    if now_days == days {
        format!("Today, {:02}:{:02}", hour, min)
    } else if now_days == days + 1 {
        format!("Yesterday, {:02}:{:02}", hour, min)
    } else {
        let (y, m, d) = civil_from_days(days);
        let months = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let m_idx = (m.saturating_sub(1) as usize).min(11);
        format!("{} {:02}, {}", months[m_idx], d, y)
    }
}

fn format_playtime(seconds: u64) -> String {
    if seconds == 0 {
        return "-".to_string();
    }
    let mins = seconds / 60;
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if hours > 0 {
        format!("{:02}h {:02}m", hours, rem_mins)
    } else if mins > 0 {
        format!("{:02}m", mins)
    } else {
        format!("{}s", seconds)
    }
}

pub(crate) fn find_cover_image(entry: &RomLibraryEntry) -> Option<PathBuf> {
    let stem = entry.path.file_stem()?.to_string_lossy();
    let display = &entry.display_name;

    // 1. In ./covers/
    let cwd_covers = Path::new("covers");
    let candidates = [
        cwd_covers.join(format!("{display}.png")),
        cwd_covers.join(format!("{display}.jpg")),
        cwd_covers.join(format!("{stem}.png")),
        cwd_covers.join(format!("{stem}.jpg")),
        cwd_covers.join(format!("{}.png", entry.game_code)),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    // 2. Alongside ROM or in parent/covers
    if let Some(parent) = entry.path.parent() {
        let parent_covers = parent.join("covers");
        let local_candidates = [
            entry.path.with_extension("png"),
            entry.path.with_extension("jpg"),
            parent.join(format!("{stem}.cover.png")),
            parent.join(format!("{stem}_cover.png")),
            parent_covers.join(format!("{display}.png")),
            parent_covers.join(format!("{stem}.png")),
            parent_covers.join(format!("{}.png", entry.game_code)),
        ];
        for c in &local_candidates {
            if c.exists() {
                return Some(c.clone());
            }
        }
    }

    None
}

pub(crate) fn trigger_cover_download(entry: &RomLibraryEntry) {
    let display_name = entry.display_name.clone();
    let platform = entry.platform;
    std::thread::spawn(move || {
        let repo_folder = match platform {
            RomPlatform::Gba => "Nintendo_-_Game_Boy_Advance",
            RomPlatform::Gbc => "Nintendo_-_Game_Boy_Color",
            RomPlatform::Gb => "Nintendo_-_Game_Boy",
        };

        let encoded = url_encode_name(&display_name);
        let url = format!(
            "https://raw.githubusercontent.com/libretro-thumbnails/{repo_folder}/master/Named_Boxarts/{encoded}.png"
        );
        if let Ok(resp) = ureq::get(&url).timeout(std::time::Duration::from_secs(6)).call() {
            if resp.status() == 200 {
                let mut bytes = Vec::new();
                use std::io::Read;
                if resp.into_reader().read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
                    let _ = std::fs::create_dir_all("covers");
                    let out = format!("covers/{display_name}.png");
                    let _ = std::fs::write(out, bytes);
                }
            }
        }
    });
}

fn url_encode_name(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'(' | b')' | b'!' | b'\'' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            b'&' => out.push_str("%26"),
            b'+' => out.push_str("%2B"),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

impl RustboyApp {
    pub(crate) fn scan_folder_recursive(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 3 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if is_supported_rom_or_archive_extension(&p) {
                    out.push(p);
                }
            } else if p.is_dir() {
                Self::scan_folder_recursive(&p, depth + 1, out);
            }
        }
    }

    pub(crate) fn parse_rom_header(path: &Path) -> Option<RomLibraryEntry> {
        let summary = read_rom_header_summary(path)?;
        let size_mb = (summary.rom_size_bytes as f32) / (1024.0 * 1024.0);
        let header = summary.header;
        let bytes_read = summary.bytes_read;

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let platform = detect_rom_platform(
            path,
            summary.inner_name.as_deref(),
            &header[..bytes_read],
        );

        let (internal_title, game_code, region, cart_type_label) = match platform {
            RomPlatform::Gb | RomPlatform::Gbc => {
                let slice = &header[..bytes_read];
                let gb_hdr = gb_core::CartridgeHeader::parse(slice);
                let title = if gb_hdr.title.is_empty() {
                    display_name.clone()
                } else {
                    gb_hdr.title.clone()
                };
                let is_cgb = platform == RomPlatform::Gbc;
                let code = gb_short_game_code(slice, is_cgb);
                let reg = match slice.get(0x014A).copied().unwrap_or(0xFF) {
                    0x00 => "JPN",
                    0x01 => "World",
                    _ => "Unk",
                };
                let cart = gb_cart_type_label(gb_hdr.cart_type).to_string();
                (title, code, reg, cart)
            }
            RomPlatform::Gba => {
                if bytes_read >= 0xB0 {
                    let title_raw = &header[0xA0..0xAC];
                    let title_clean: String = title_raw
                        .iter()
                        .take_while(|&&c| c != 0)
                        .map(|&c| {
                            if c.is_ascii_graphic() || c == b' ' {
                                c as char
                            } else {
                                ' '
                            }
                        })
                        .collect();
                    let code_raw = &header[0xAC..0xB0];
                    let code_clean: String = code_raw
                        .iter()
                        .map(|&c| {
                            if c.is_ascii_alphanumeric() {
                                c as char
                            } else {
                                '-'
                            }
                        })
                        .collect();
                    let reg = match code_raw.get(3).copied().unwrap_or(b' ') {
                        b'E' | b'U' => "USA",
                        b'P' | b'D' | b'F' | b'I' | b'S' => "EUR",
                        b'J' => "JPN",
                        b'A' => "World",
                        _ => "Unk",
                    };
                    (
                        title_clean.trim().to_string(),
                        code_clean,
                        reg,
                        String::from("GBA Cart"),
                    )
                } else {
                    (
                        String::from("Homebrew"),
                        String::from("HOME"),
                        "Unk",
                        String::from("GBA ROM"),
                    )
                }
            }
        };

        let has_battery_sav = path.with_extension("sav").exists();

        Some(RomLibraryEntry {
            path: path.to_path_buf(),
            display_name,
            internal_title,
            game_code,
            region,
            size_mb,
            has_battery_sav,
            source_kind: summary.source_kind,
            platform,
            cart_type_label,
        })
    }

    #[cfg(test)]
    pub(crate) fn parse_gba_header(path: &Path) -> Option<RomLibraryEntry> {
        Self::parse_rom_header(path)
    }

    pub(crate) fn scan_folders_collect(folders: &[PathBuf]) -> LibraryScanResult {
        let mut missing = Vec::new();
        let mut found_files = Vec::new();
        for folder in folders {
            if !folder.exists() {
                missing.push(folder.display().to_string());
                continue;
            }
            Self::scan_folder_recursive(folder, 0, &mut found_files);
        }
        found_files.sort();
        found_files.dedup();

        let mut entries = Vec::with_capacity(found_files.len());
        for path in found_files {
            if let Some(entry) = Self::parse_rom_header(&path) {
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        });

        let error = if missing.is_empty() {
            None
        } else {
            Some(format!(
                "Unreachable ROM folder{}: {}",
                if missing.len() == 1 { "" } else { "s" },
                missing.join(", ")
            ))
        };
        LibraryScanResult { entries, error }
    }

    pub(crate) fn rescan_rom_folders(&mut self) {
        if self.rom_folders.is_empty() {
            self.library_entries.clear();
            self.library_scanning = false;
            self.library_scan_rx = None;
            self.library_scan_error = None;
            return;
        }
        let folders = self.rom_folders.clone();
        let (tx, rx) = mpsc::channel();
        self.library_scanning = true;
        self.library_scan_error = None;
        self.library_scan_rx = Some(rx);

        std::thread::spawn(move || {
            let res = Self::scan_folders_collect(&folders);
            let _ = tx.send(res);
        });
    }

    pub(crate) fn poll_library_scan(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.library_scan_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.library_entries = result.entries;
                self.library_scan_error = result.error;
                self.library_scanning = false;
                self.library_scan_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.library_scanning = false;
                self.library_scan_rx = None;
            }
        }
    }

    pub(crate) fn add_rom_folder_dialog(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Select ROM Directory")
            .pick_folder()
            && !self.rom_folders.contains(&folder)
        {
            self.rom_folders.push(folder);
            self.save_persistent_config();
            self.rescan_rom_folders();
            self.notify("Added ROM directory — Scanning library...");
        }
    }

    pub(crate) fn draw_rom_library_browser(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let query = self.search_query.trim().to_lowercase();
        let filtered: Vec<RomLibraryEntry> = self
            .library_entries
            .iter()
            .filter(|e| {
                if self.filter_favorites_only && !self.favorites.contains(&e.path) {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                e.display_name.to_lowercase().contains(&query)
                    || e.internal_title.to_lowercase().contains(&query)
                    || e.game_code.to_lowercase().contains(&query)
                    || e.platform.badge().to_lowercase().contains(&query)
                    || e.cart_type_label.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        // 1. Library Sub-Bar
        egui::Frame::new()
            .fill(COLOR_SURFACE)
            .inner_margin(egui::Margin::symmetric(14, 6))
            .stroke(STROKE_BORDER)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let total = self.library_entries.len();
                    let count_str = format!("{total} Title{}", if total == 1 { "" } else { "s" });
                    ui.label(RichText::new(count_str).color(COLOR_TEXT));

                    ui.add_space(8.0);

                    // Add ROM Folder... button (26px height, neutral gray folder icon)
                    let add_btn = egui::Button::new(
                        RichText::new("📁 Add ROM Folder...").color(COLOR_TEXT),
                    )
                    .corner_radius(RADIUS_DEFAULT)
                    .stroke(STROKE_BORDER)
                    .min_size(egui::vec2(0.0, 26.0));
                    if ui.add(add_btn).clicked() {
                        self.add_rom_folder_dialog();
                    }

                    // Refresh button (26px height)
                    let refresh_enabled = !self.rom_folders.is_empty();
                    let refresh_color = if refresh_enabled {
                        COLOR_TEXT
                    } else {
                        COLOR_TEXT_DISABLED
                    };
                    let refresh_btn = egui::Button::new(RichText::new("🔄 Refresh").color(refresh_color))
                        .corner_radius(RADIUS_DEFAULT)
                        .stroke(STROKE_BORDER)
                        .min_size(egui::vec2(0.0, 26.0));
                    if ui.add_enabled(refresh_enabled, refresh_btn).clicked() {
                        self.rescan_rom_folders();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Table / Grid toggle buttons (26x26)
                        let is_grid = self.library_view_mode == LibraryViewMode::Grid;
                        let is_table = self.library_view_mode == LibraryViewMode::Table;

                        let grid_stroke = if is_grid {
                            STROKE_TEAL
                        } else {
                            STROKE_BORDER
                        };
                        let grid_color = if is_grid {
                            ACCENT_TEAL
                        } else {
                            COLOR_TEXT_SECONDARY
                        };
                        let grid_btn = egui::Button::new(RichText::new("⊞").color(grid_color))
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(grid_stroke)
                            .min_size(egui::vec2(26.0, 26.0));
                        if ui.add(grid_btn).on_hover_text("Grid View").clicked() {
                            self.library_view_mode = LibraryViewMode::Grid;
                            self.save_persistent_config();
                        }

                        let table_stroke = if is_table {
                            STROKE_TEAL
                        } else {
                            STROKE_BORDER
                        };
                        let table_color = if is_table {
                            ACCENT_TEAL
                        } else {
                            COLOR_TEXT_SECONDARY
                        };
                        let table_btn = egui::Button::new(RichText::new("☰").color(table_color))
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(table_stroke)
                            .min_size(egui::vec2(26.0, 26.0));
                        if ui.add(table_btn).on_hover_text("Table View").clicked() {
                            self.library_view_mode = LibraryViewMode::Table;
                            self.save_persistent_config();
                        }

                        // Favorites star: neutral gray, teal only when active (26x26)
                        let fav_active = self.filter_favorites_only;
                        let star_color = if fav_active {
                            ACCENT_TEAL
                        } else {
                            COLOR_TEXT_SECONDARY
                        };
                        let star_stroke = if fav_active {
                            STROKE_TEAL
                        } else {
                            STROKE_BORDER
                        };
                        let fav_btn = egui::Button::new(
                            RichText::new(if fav_active { "★" } else { "☆" }).color(star_color),
                        )
                        .corner_radius(RADIUS_DEFAULT)
                        .stroke(star_stroke)
                        .min_size(egui::vec2(26.0, 26.0));
                        if ui.add(fav_btn).on_hover_text("Filter Favorites").clicked() {
                            self.filter_favorites_only = !self.filter_favorites_only;
                        }

                        // Search box
                        let search_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search ROM library...")
                            .desired_width(180.0)
                            .min_size(egui::vec2(180.0, 26.0));
                        ui.add(search_edit);
                    });
                });
            });

        ui.add_space(6.0);

        if let Some(scan_err) = self.library_scan_error.clone() {
            egui::Frame::new()
                .fill(Color32::from_rgb(56, 24, 28))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(170, 64, 68)))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(scan_err)
                                .small()
                                .color(Color32::from_rgb(255, 165, 165)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(RichText::new("Dismiss").small()).clicked() {
                                self.library_scan_error = None;
                            }
                            if ui.button(RichText::new("Retry").small()).clicked() {
                                self.rescan_rom_folders();
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }

        if self.library_scanning && !self.library_entries.is_empty() {
            egui::Frame::new()
                .fill(BG_PANEL)
                .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Scanning ROM folders and archives...")
                                .small()
                                .color(TEXT_MUTED),
                        );
                    });
                });
            ui.add_space(6.0);
        }

        // 2. Empty & Loading States
        if self.rom_folders.is_empty() && filtered.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.35).max(40.0));
                ui.label(
                    RichText::new("No Game Directories Configured")
                        .size(16.0)
                        .strong()
                        .color(COLOR_TEXT),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Add a folder containing your .gba, .gb, .gbc, .zip, or .7z ROMs to populate the game list, or open a single file.",
                    )
                    .color(COLOR_TEXT_SECONDARY)
                    .size(12.0),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let total_btn_w = 140.0 * 2.0 + 8.0;
                    let pad = (ui.available_width() - total_btn_w).max(0.0) * 0.5;
                    ui.add_space(pad);

                    let add_btn = egui::Button::new(
                        RichText::new("📁 Add Game Folder...").color(COLOR_TEXT),
                    )
                    .corner_radius(RADIUS_DEFAULT)
                    .stroke(STROKE_BORDER)
                    .min_size(egui::vec2(140.0, 28.0));
                    if ui.add(add_btn).clicked() {
                        self.add_rom_folder_dialog();
                    }

                    ui.add_space(8.0);

                    let open_btn = egui::Button::new(
                        RichText::new("📄 Open ROM File...").color(COLOR_TEXT),
                    )
                    .corner_radius(RADIUS_DEFAULT)
                    .stroke(STROKE_BORDER)
                    .min_size(egui::vec2(140.0, 28.0));
                    if ui.add(open_btn).clicked() {
                        self.trigger_open_rom_dialog(ctx);
                    }
                });
            });
            return;
        }

        if self.library_scanning && self.library_entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.25);
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.heading(RichText::new("Scanning ROM Library...").size(18.0).strong());
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Indexing .gba, .agb, .bin, .gb, .gbc, .zip, and .7z ROM headers")
                            .color(TEXT_MUTED),
                    );
                });
            });
            return;
        }

        if !self.rom_folders.is_empty() && self.library_entries.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Supported ROMs Found")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "The configured directories do not contain any .gba, .agb, .bin, .gb, .gbc, .zip, or .7z ROM files.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let pad = (ui.available_width() - 320.0).max(0.0) * 0.5;
                        ui.add_space(pad);
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📁 Add Another Folder..."))
                            .clicked()
                        {
                            self.add_rom_folder_dialog();
                        }
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("🔄 Rescan Folders"))
                            .clicked()
                        {
                            self.rescan_rom_folders();
                        }
                    });
                });
            });
            return;
        }

        if !self.library_entries.is_empty() && filtered.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Matching ROMs Found")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("No games match the current search query or favorites filter.")
                            .color(TEXT_MUTED),
                    );
                    ui.add_space(16.0);
                    if ui.button("Clear Search & Filters").clicked() {
                        self.search_query.clear();
                        self.filter_favorites_only = false;
                    }
                });
            });
            return;
        }

        // 3. Render Table or Grid View
        let mut launch_target: Option<PathBuf> = None;
        let mut game_settings_entry: Option<RomLibraryEntry> = None;
        let mut rescan_needed = false;

        match self.library_view_mode {
            LibraryViewMode::Table => {
                let avail_w = ui.available_width();
                let region_w = 80.0_f32;
                let size_w = 90.0_f32;
                let last_played_w = 130.0_f32;
                let play_time_w = 100.0_f32;
                let title_w = (avail_w - region_w - size_w - last_played_w - play_time_w - 40.0).max(180.0);

                let mut sorted_entries = filtered;
                sorted_entries.sort_by(|a, b| {
                    let ord = match self.table_sort_column {
                        TableSortColumn::Title => {
                            a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase())
                        }
                        TableSortColumn::Region => a.region.cmp(b.region),
                        TableSortColumn::Size => a
                            .size_mb
                            .partial_cmp(&b.size_mb)
                            .unwrap_or(std::cmp::Ordering::Equal),
                        TableSortColumn::LastPlayed => {
                            let ts_a = self
                                .config
                                .playtimes
                                .get(&a.game_code)
                                .map(|p| p.last_played_timestamp)
                                .unwrap_or(0);
                            let ts_b = self
                                .config
                                .playtimes
                                .get(&b.game_code)
                                .map(|p| p.last_played_timestamp)
                                .unwrap_or(0);
                            ts_a.cmp(&ts_b)
                        }
                        TableSortColumn::PlayTime => {
                            let sec_a = self
                                .config
                                .playtimes
                                .get(&a.game_code)
                                .map(|p| p.total_seconds)
                                .unwrap_or(0);
                            let sec_b = self
                                .config
                                .playtimes
                                .get(&b.game_code)
                                .map(|p| p.total_seconds)
                                .unwrap_or(0);
                            sec_a.cmp(&sec_b)
                        }
                    };
                    if self.table_sort_descending {
                        ord.reverse()
                    } else {
                        ord
                    }
                });

                // Table Header (28px height, 1px bottom border)
                let (header_rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail_w, 28.0), Sense::hover());
                ui.painter()
                    .rect_filled(header_rect, CornerRadius::ZERO, COLOR_SURFACE);
                ui.painter().line_segment(
                    [
                        egui::pos2(header_rect.left(), header_rect.bottom()),
                        egui::pos2(header_rect.right(), header_rect.bottom()),
                    ],
                    STROKE_BORDER,
                );

                let draw_header_btn = |ui: &mut egui::Ui,
                                       title: &str,
                                       col: TableSortColumn,
                                       curr_col: TableSortColumn,
                                       desc: bool,
                                       width: f32,
                                       align_right: bool|
                 -> bool {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(width, 28.0), Sense::click());
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::ZERO, COLOR_HOVER_SELECTED);
                    }
                    let arrow = if col == curr_col {
                        if desc { " ↓" } else { " ↑" }
                    } else {
                        ""
                    };
                    let text = format!("{title}{arrow}");
                    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
                    let align = if align_right {
                        egui::Align2::RIGHT_CENTER
                    } else {
                        egui::Align2::LEFT_CENTER
                    };
                    let text_pos = if align_right {
                        egui::pos2(rect.right() - 4.0, rect.center().y)
                    } else {
                        egui::pos2(rect.left() + 4.0, rect.center().y)
                    };
                    ui.painter()
                        .text(text_pos, align, text, font, COLOR_TEXT_SECONDARY);
                    resp.clicked()
                };

                let curr_col = self.table_sort_column;
                let desc = self.table_sort_descending;
                let mut new_sort_col = None;

                ui.scope_builder(egui::UiBuilder::new().max_rect(header_rect), |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        if draw_header_btn(
                            ui,
                            "TITLE",
                            TableSortColumn::Title,
                            curr_col,
                            desc,
                            title_w,
                            false,
                        ) {
                            new_sort_col = Some(TableSortColumn::Title);
                        }
                        if draw_header_btn(
                            ui,
                            "REGION",
                            TableSortColumn::Region,
                            curr_col,
                            desc,
                            region_w,
                            false,
                        ) {
                            new_sort_col = Some(TableSortColumn::Region);
                        }
                        if draw_header_btn(
                            ui,
                            "SIZE",
                            TableSortColumn::Size,
                            curr_col,
                            desc,
                            size_w,
                            true,
                        ) {
                            new_sort_col = Some(TableSortColumn::Size);
                        }
                        if draw_header_btn(
                            ui,
                            "LAST PLAYED",
                            TableSortColumn::LastPlayed,
                            curr_col,
                            desc,
                            last_played_w,
                            true,
                        ) {
                            new_sort_col = Some(TableSortColumn::LastPlayed);
                        }
                        if draw_header_btn(
                            ui,
                            "PLAY TIME",
                            TableSortColumn::PlayTime,
                            curr_col,
                            desc,
                            play_time_w,
                            true,
                        ) {
                            new_sort_col = Some(TableSortColumn::PlayTime);
                        }
                    });
                });

                if let Some(col) = new_sort_col {
                    if self.table_sort_column == col {
                        self.table_sort_descending = !self.table_sort_descending;
                    } else {
                        self.table_sort_column = col;
                        self.table_sort_descending = false;
                    }
                }

                // Table Rows
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (row_idx, entry) in sorted_entries.iter().enumerate() {
                            let is_selected = self.selected_rom_path.as_ref() == Some(&entry.path);
                            let (row_rect, row_resp) =
                                ui.allocate_exact_size(egui::vec2(avail_w, 28.0), Sense::click());

                            if row_resp.clicked() {
                                self.selected_rom_path = Some(entry.path.clone());
                            }
                            if row_resp.double_clicked() {
                                launch_target = Some(entry.path.clone());
                            }

                            row_resp.context_menu(|ui| {
                                if ui.button("▶ Play Game").clicked() {
                                    launch_target = Some(entry.path.clone());
                                    ui.close_menu();
                                }
                                if ui.button("📁 Open Save Folder").clicked() {
                                    if let Some(parent) = entry.path.parent() {
                                        #[cfg(target_os = "windows")]
                                        let _ = std::process::Command::new("explorer")
                                            .arg(parent)
                                            .spawn();
                                    }
                                    ui.close_menu();
                                }
                                if ui.button("⚙ Game Settings...").clicked() {
                                    game_settings_entry = Some(entry.clone());
                                    ui.close_menu();
                                }
                                let is_fav = self.favorites.contains(&entry.path);
                                if ui
                                    .button(if is_fav {
                                        "☆ Remove from Favorites"
                                    } else {
                                        "★ Add to Favorites"
                                    })
                                    .clicked()
                                {
                                    if is_fav {
                                        self.favorites.remove(&entry.path);
                                    } else {
                                        self.favorites.insert(entry.path.clone());
                                    }
                                    self.save_persistent_config();
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("🗑 Remove Folder from Library").clicked() {
                                    if let Some(parent) = entry.path.parent() {
                                        self.rom_folders.retain(|f| f != parent);
                                        self.save_persistent_config();
                                        rescan_needed = true;
                                    }
                                    ui.close_menu();
                                }
                            });

                            let bg_color = if is_selected {
                                COLOR_HOVER_SELECTED
                            } else if row_resp.hovered() {
                                COLOR_HOVER_SELECTED
                            } else if row_idx % 2 == 1 {
                                Color32::from_rgb(0x14, 0x14, 0x14)
                            } else {
                                COLOR_SURFACE
                            };
                            ui.painter()
                                .rect_filled(row_rect, CornerRadius::ZERO, bg_color);

                            if is_selected {
                                let bar_rect = egui::Rect::from_min_size(
                                    row_rect.left_top(),
                                    egui::vec2(2.0, 28.0),
                                );
                                ui.painter()
                                    .rect_filled(bar_rect, CornerRadius::ZERO, ACCENT_TEAL);
                            }

                            let pt = self.config.playtimes.get(&entry.game_code);
                            let last_played_ts = pt.map(|p| p.last_played_timestamp).unwrap_or(0);
                            let total_seconds = pt.map(|p| p.total_seconds).unwrap_or(0);

                            let lp_str = format_last_played_ts(last_played_ts);
                            let pt_str = format_playtime(total_seconds);
                            let size_str = format!("{:.1} MB", entry.size_mb);

                            ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);

                                    // Title
                                    ui.add_sized(
                                        [title_w, 28.0],
                                        egui::Label::new(
                                            RichText::new(&entry.display_name)
                                                .color(COLOR_TEXT)
                                                .size(12.0),
                                        )
                                        .truncate(),
                                    );

                                    // Region: plain text
                                    ui.add_sized(
                                        [region_w, 28.0],
                                        egui::Label::new(
                                            RichText::new(entry.region)
                                                .color(COLOR_TEXT_SECONDARY)
                                                .size(12.0),
                                        ),
                                    );

                                    // Size: right-aligned
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(size_w, 28.0),
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(size_str)
                                                    .color(COLOR_TEXT_SECONDARY)
                                                    .size(12.0),
                                            );
                                        },
                                    );

                                    // Last Played: right-aligned
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(last_played_w, 28.0),
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(lp_str)
                                                    .color(COLOR_TEXT_SECONDARY)
                                                    .size(12.0),
                                            );
                                        },
                                    );

                                    // Play Time: right-aligned
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(play_time_w, 28.0),
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(pt_str)
                                                    .color(COLOR_TEXT_SECONDARY)
                                                    .size(12.0),
                                            );
                                        },
                                    );
                                });
                            });
                        }
                    });
            }
            LibraryViewMode::Grid => {
                // Ensure covers are loaded or scheduled for download
                for entry in &filtered {
                    if !self.cover_textures.contains_key(&entry.path) {
                        if let Some(cover_path) = find_cover_image(entry) {
                            if let Ok(img) = image::open(&cover_path) {
                                let rgba = img.to_rgba8();
                                let size = [rgba.width() as usize, rgba.height() as usize];
                                let pixels = rgba.into_raw();
                                let color_img =
                                    egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                                let tex = ctx.load_texture(
                                    format!("cover_{}", entry.display_name),
                                    color_img,
                                    egui::TextureOptions::LINEAR,
                                );
                                self.cover_textures.insert(entry.path.clone(), tex);
                            }
                        } else if !self.pending_cover_downloads.contains(&entry.path) {
                            self.pending_cover_downloads.insert(entry.path.clone());
                            trigger_cover_download(entry);
                        }
                    }
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            let tile_w = 144.0_f32;
                            let tile_h = 192.0_f32; // Exact 3:4 aspect ratio

                            for entry in &filtered {
                                let is_selected =
                                    self.selected_rom_path.as_ref() == Some(&entry.path);

                                ui.allocate_ui_with_layout(
                                    egui::vec2(tile_w, tile_h + 38.0),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        // 1. Cover Tile (3:4 aspect ratio)
                                        let (tile_rect, tile_resp) = ui.allocate_exact_size(
                                            egui::vec2(tile_w, tile_h),
                                            Sense::click(),
                                        );

                                        if tile_resp.clicked() {
                                            self.selected_rom_path = Some(entry.path.clone());
                                        }
                                        if tile_resp.double_clicked() {
                                            launch_target = Some(entry.path.clone());
                                        }

                                        tile_resp.context_menu(|ui| {
                                            if ui.button("▶ Play Game").clicked() {
                                                launch_target = Some(entry.path.clone());
                                                ui.close_menu();
                                            }
                                            if ui.button("📁 Open Save Folder").clicked() {
                                                if let Some(parent) = entry.path.parent() {
                                                    #[cfg(target_os = "windows")]
                                                    let _ = std::process::Command::new("explorer")
                                                        .arg(parent)
                                                        .spawn();
                                                }
                                                ui.close_menu();
                                            }
                                            if ui.button("🖼 Refresh / Download Cover").clicked() {
                                                self.cover_textures.remove(&entry.path);
                                                self.pending_cover_downloads.remove(&entry.path);
                                                trigger_cover_download(entry);
                                                ui.close_menu();
                                            }
                                            if ui.button("⚙ Game Settings...").clicked() {
                                                game_settings_entry = Some(entry.clone());
                                                ui.close_menu();
                                            }
                                            let is_fav = self.favorites.contains(&entry.path);
                                            if ui
                                                .button(if is_fav {
                                                    "☆ Remove from Favorites"
                                                } else {
                                                    "★ Add to Favorites"
                                                })
                                                .clicked()
                                            {
                                                if is_fav {
                                                    self.favorites.remove(&entry.path);
                                                } else {
                                                    self.favorites.insert(entry.path.clone());
                                                }
                                                self.save_persistent_config();
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("🗑 Remove Folder from Library").clicked() {
                                                if let Some(parent) = entry.path.parent() {
                                                    self.rom_folders.retain(|f| f != parent);
                                                    self.save_persistent_config();
                                                    rescan_needed = true;
                                                }
                                                ui.close_menu();
                                            }
                                        });

                                        // Render cover image if loaded, or flat placeholder
                                        if let Some(tex) = self.cover_textures.get(&entry.path) {
                                            ui.painter().image(
                                                tex.id(),
                                                tile_rect,
                                                egui::Rect::from_min_max(
                                                    egui::pos2(0.0, 0.0),
                                                    egui::pos2(1.0, 1.0),
                                                ),
                                                Color32::WHITE,
                                            );
                                        } else {
                                            ui.painter()
                                                .rect_filled(tile_rect, RADIUS_DEFAULT, COLOR_TILE);
                                        }

                                        let stroke = if is_selected {
                                            STROKE_TEAL_2PX
                                        } else {
                                            STROKE_BORDER
                                        };
                                        ui.painter().rect_stroke(
                                            tile_rect,
                                            RADIUS_DEFAULT,
                                            stroke,
                                            egui::StrokeKind::Inside,
                                        );

                                        // 2. Metadata Directly Below Tile
                                        ui.add_space(4.0);

                                        // Title 12px white, single-line ellipsis
                                        let title_lbl = egui::Label::new(
                                            RichText::new(&entry.display_name)
                                                .color(COLOR_TEXT)
                                                .size(12.0),
                                        )
                                        .truncate();
                                        let title_resp = ui.add_sized([tile_w, 16.0], title_lbl);
                                        if title_resp.clicked() {
                                            self.selected_rom_path = Some(entry.path.clone());
                                        }
                                        if title_resp.double_clicked() {
                                            launch_target = Some(entry.path.clone());
                                        }

                                        // "REGION · SIZE" in secondary text
                                        let size_mb_int = entry.size_mb.round() as u32;
                                        let meta_str = format!(
                                            "{} · {} MB",
                                            entry.region,
                                            size_mb_int.max(1)
                                        );
                                        ui.label(
                                            RichText::new(meta_str)
                                                .color(COLOR_TEXT_SECONDARY)
                                                .size(11.0),
                                        );
                                    },
                                );
                            }
                        });
                    });
            }
        }

        if rescan_needed {
            self.rescan_rom_folders();
        }

        if let Some(entry) = game_settings_entry {
            self.open_game_settings_for_library_entry(&entry);
        }

        if let Some(path) = launch_target
            && let Err(e) = self.open_rom(&path, None, Some(ctx))
        {
            self.notify_error(&e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gba_header_parsing() {
        let mut rom = vec![0u8; 512];
        // Title at 0xA0..0xAC
        let title_bytes = b"POKEMON EMER";
        rom[0xA0..0xAC].copy_from_slice(title_bytes);
        // Game code at 0xAC..0xB0
        let code_bytes = b"BPEE";
        rom[0xAC..0xB0].copy_from_slice(code_bytes);

        let temp_dir = std::env::temp_dir().join("rba_test_header");
        let _ = fs::create_dir_all(&temp_dir);
        let rom_path = temp_dir.join("Pokemon Emerald.gba");
        fs::write(&rom_path, &rom).unwrap();

        let parsed = RustboyApp::parse_gba_header(&rom_path).expect("header parse failed");
        assert_eq!(parsed.display_name, "Pokemon Emerald");
        assert_eq!(parsed.internal_title, "POKEMON EMER");
        assert_eq!(parsed.game_code, "BPEE");
        assert_eq!(parsed.region, "USA");
        assert!(!parsed.has_battery_sav);

        // Test with .sav present
        let sav_path = temp_dir.join("Pokemon Emerald.sav");
        fs::write(&sav_path, b"SAVEDATA").unwrap();
        let parsed_with_sav = RustboyApp::parse_gba_header(&rom_path).unwrap();
        assert!(parsed_with_sav.has_battery_sav);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_gb_and_gbc_header_parsing_in_library() {
        let temp_dir = std::env::temp_dir().join("rba_test_gb_header");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let mut dmg_rom = vec![0u8; 0x8000];
        dmg_rom[0x0134..0x0142].copy_from_slice(b"LINKS AWAKENIN");
        dmg_rom[0x0143] = 0x00;
        dmg_rom[0x0147] = 0x03; // MBC1+RAM+BATT
        dmg_rom[0x014A] = 0x01; // Non-Japanese -> World
        let dmg_path = temp_dir.join("Links Awakening.gb");
        fs::write(&dmg_path, &dmg_rom).unwrap();

        let dmg_entry = RustboyApp::parse_rom_header(&dmg_path).expect("dmg parse");
        assert_eq!(dmg_entry.platform, RomPlatform::Gb);
        assert_eq!(dmg_entry.internal_title, "LINKS AWAKENIN");
        assert_eq!(dmg_entry.cart_type_label, "MBC1+RAM+BATT");
        assert_eq!(dmg_entry.region, "World");
        assert_eq!(dmg_entry.game_code.len(), 4);
        assert!(dmg_entry.game_code.starts_with('G'));

        let mut cgb_rom = vec![0u8; 0x8000];
        cgb_rom[0x0134..0x013F].copy_from_slice(b"POKEMON_SLV");
        cgb_rom[0x0143] = 0x80; // Dual-mode GBC
        cgb_rom[0x0147] = 0x10; // MBC3+RTC+RAM+BATT
        cgb_rom[0x014A] = 0x00; // JPN
        let cgb_path = temp_dir.join("Pokemon Silver.gbc");
        fs::write(&cgb_path, &cgb_rom).unwrap();

        let cgb_entry = RustboyApp::parse_rom_header(&cgb_path).expect("cgb parse");
        assert_eq!(cgb_entry.platform, RomPlatform::Gbc);
        assert_eq!(cgb_entry.internal_title, "POKEMON_SLV");
        assert_eq!(cgb_entry.cart_type_label, "MBC3+RTC+RAM+BATT");
        assert_eq!(cgb_entry.region, "JPN");
        assert!(cgb_entry.game_code.starts_with('C'));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
