use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke};

use crate::app::{RustboyApp, ACCENT_TEAL, BG_CARD, BG_PANEL, BORDER_HAIRLINE, TEXT_MUTED};
use crate::rom_loader::{
    is_supported_rom_or_archive_extension, read_rom_header_summary, RomSourceKind,
};

#[derive(Clone, Debug)]
pub struct RomLibraryEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub internal_title: String,
    pub game_code: String,
    pub region: &'static str,
    pub size_mb: f32,
    pub has_battery_sav: bool,
    pub source_kind: RomSourceKind,
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

    pub(crate) fn parse_gba_header(path: &Path) -> Option<RomLibraryEntry> {
        let summary = read_rom_header_summary(path)?;
        let size_mb = (summary.rom_size_bytes as f32) / (1024.0 * 1024.0);
        let header = summary.header;
        let bytes_read = summary.bytes_read;

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let (internal_title, game_code, region) = if bytes_read >= 0xB0 {
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
            (title_clean.trim().to_string(), code_clean, reg)
        } else {
            (String::from("Homebrew"), String::from("HOME"), "Unk")
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
        })
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
            if let Some(entry) = Self::parse_gba_header(&path) {
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
            })
            .cloned()
            .collect();

        // 1. Library Sub-Bar
        egui::Frame::new()
            .fill(BG_PANEL)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let total = self.library_entries.len();
                    let count_str = format!("{total} Title{}", if total == 1 { "" } else { "s" });
                    ui.label(RichText::new(count_str).strong());
                    if !query.is_empty() || self.filter_favorites_only {
                        ui.label(
                            RichText::new(format!("({} shown)", filtered.len()))
                                .small()
                                .color(TEXT_MUTED),
                        );
                    }

                    ui.separator();

                    if ui.button("📁 Add ROM Folder...").clicked() {
                        self.add_rom_folder_dialog();
                    }
                    if ui.button("🔄 Refresh").clicked() {
                        self.rescan_rom_folders();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let is_grid = self.library_view_mode == LibraryViewMode::Grid;
                        let is_table = self.library_view_mode == LibraryViewMode::Table;
                        if ui.selectable_label(is_grid, "⊞ Grid").clicked() {
                            self.library_view_mode = LibraryViewMode::Grid;
                            self.save_persistent_config();
                        }
                        if ui.selectable_label(is_table, "☰ Table").clicked() {
                            self.library_view_mode = LibraryViewMode::Table;
                            self.save_persistent_config();
                        }

                        ui.separator();

                        let fav_active = self.filter_favorites_only;
                        if ui
                            .selectable_label(fav_active, "★ Favorites")
                            .clicked()
                        {
                            self.filter_favorites_only = !self.filter_favorites_only;
                        }

                        ui.separator();

                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Search ROM library...")
                                .desired_width(190.0),
                        );
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
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.22);
                    ui.heading(
                        RichText::new("No Game Directories Configured")
                            .size(20.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "Add a folder containing your .gba, .zip, or .7z ROMs to populate the game list, or open a single file.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        let pad = (ui.available_width() - 320.0).max(0.0) * 0.5;
                        ui.add_space(pad);
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📁 Add Game Folder..."))
                            .clicked()
                        {
                            self.add_rom_folder_dialog();
                        }
                        if ui
                            .add_sized([150.0, 32.0], egui::Button::new("📂 Open ROM File..."))
                            .clicked()
                        {
                            self.trigger_open_rom_dialog(ctx);
                        }
                    });
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
                        RichText::new("Indexing .gba, .agb, .bin, .zip, and .7z ROM headers")
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
                            "The configured directories do not contain any .gba, .agb, .bin, .zip, or .7z ROM files.",
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
        let mut open_overrides = false;
        let mut rescan_needed = false;

        match self.library_view_mode {
            LibraryViewMode::Table => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("rba_rom_table")
                            .striped(true)
                            .min_col_width(30.0)
                            .spacing([16.0, 7.0])
                            .show(ui, |ui| {
                                ui.strong("★");
                                ui.strong("Game Title (Filename)");
                                ui.strong("Internal Header");
                                ui.strong("Game ID");
                                ui.strong("Region");
                                ui.strong("Size");
                                ui.strong("Playtime");
                                ui.strong("Save File");
                                ui.strong("Action");
                                ui.end_row();

                                for entry in &filtered {
                                    let is_fav = self.favorites.contains(&entry.path);
                                    let star = if is_fav { "★" } else { "☆" };
                                    let star_color = if is_fav {
                                        Color32::from_rgb(255, 204, 0)
                                    } else {
                                        TEXT_MUTED
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(RichText::new(star).color(star_color))
                                                .frame(false),
                                        )
                                        .clicked()
                                    {
                                        if is_fav {
                                            self.favorites.remove(&entry.path);
                                        } else {
                                            self.favorites.insert(entry.path.clone());
                                        }
                                        self.save_persistent_config();
                                    }

                                    let title_text = if let Some(badge) = entry.source_kind.badge() {
                                        format!("{} [{}]", entry.display_name, badge)
                                    } else {
                                        entry.display_name.clone()
                                    };
                                    let resp = ui.selectable_label(
                                        false,
                                        RichText::new(title_text).strong(),
                                    );
                                    if resp.double_clicked() {
                                        launch_target = Some(entry.path.clone());
                                    }
                                    resp.context_menu(|ui| {
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
                                        if ui.button("⚙ Configure Game Overrides").clicked() {
                                            launch_target = Some(entry.path.clone());
                                            open_overrides = true;
                                            ui.close_menu();
                                        }
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

                                    ui.monospace(
                                        RichText::new(&entry.internal_title).color(TEXT_MUTED),
                                    );
                                    ui.monospace(RichText::new(&entry.game_code).strong());
                                    ui.label(RichText::new(entry.region).color(match entry.region {
                                        "USA" => Color32::from_rgb(120, 180, 255),
                                        "EUR" => Color32::from_rgb(120, 220, 160),
                                        "JPN" => Color32::from_rgb(255, 140, 140),
                                        _ => TEXT_MUTED,
                                    }));
                                    ui.monospace(format!("{:.1} MB", entry.size_mb));

                                    let pt_str = self
                                        .config
                                        .playtimes
                                        .get(&entry.game_code)
                                        .map(|p| p.formatted_playtime())
                                        .unwrap_or_else(|| "-".to_string());
                                    ui.monospace(RichText::new(pt_str).color(TEXT_MUTED));

                                    if entry.has_battery_sav {
                                        ui.label(
                                            RichText::new("✔ .sav")
                                                .color(Color32::from_rgb(100, 220, 120)),
                                        );
                                    } else {
                                        ui.label(RichText::new("—").color(TEXT_MUTED));
                                    }

                                    if ui.button("▶ Play").clicked() {
                                        launch_target = Some(entry.path.clone());
                                    }

                                    ui.end_row();
                                }
                            });
                    });
            }
            LibraryViewMode::Grid => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for entry in &filtered {
                                let is_fav = self.favorites.contains(&entry.path);
                                let star = if is_fav { "★" } else { "☆" };
                                let star_col = if is_fav {
                                    Color32::from_rgb(255, 204, 0)
                                } else {
                                    TEXT_MUTED
                                };

                                let frame = egui::Frame::new()
                                    .fill(BG_CARD)
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        if is_fav {
                                            Color32::from_rgb(160, 140, 50)
                                        } else {
                                            BORDER_HAIRLINE
                                        },
                                    ))
                                    .corner_radius(CornerRadius::same(6))
                                    .inner_margin(12.0);

                                let card_resp = frame.show(ui, |ui| {
                                    ui.set_width(210.0);
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new(star).color(star_col),
                                                    )
                                                    .frame(false),
                                                )
                                                .clicked()
                                            {
                                                if is_fav {
                                                    self.favorites.remove(&entry.path);
                                                } else {
                                                    self.favorites.insert(entry.path.clone());
                                                }
                                                self.save_persistent_config();
                                            }
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&entry.display_name).strong(),
                                                )
                                                .truncate(),
                                            );
                                        });

                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.monospace(
                                                RichText::new(format!("[{}]", entry.game_code))
                                                    .small(),
                                            );
                                            ui.label(RichText::new("•").color(TEXT_MUTED).small());
                                            ui.label(
                                                RichText::new(entry.region)
                                                    .color(match entry.region {
                                                        "USA" => Color32::from_rgb(120, 180, 255),
                                                        "EUR" => Color32::from_rgb(120, 220, 160),
                                                        "JPN" => Color32::from_rgb(255, 140, 140),
                                                        _ => TEXT_MUTED,
                                                    })
                                                    .small(),
                                            );
                                            ui.label(RichText::new("•").color(TEXT_MUTED).small());
                                            ui.monospace(
                                                RichText::new(format!("{:.1} MB", entry.size_mb))
                                                    .small(),
                                            );
                                        });

                                        ui.label(
                                            RichText::new(format!("Header: {}", entry.internal_title))
                                                .small()
                                                .color(TEXT_MUTED),
                                        );

                                        let pt_str = self
                                            .config
                                            .playtimes
                                            .get(&entry.game_code)
                                            .map(|p| p.formatted_playtime())
                                            .unwrap_or_else(|| "-".to_string());
                                        if pt_str != "-" {
                                            ui.label(RichText::new(format!("⏱ {pt_str}")).small().color(ACCENT_TEAL));
                                        }

                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            if entry.has_battery_sav {
                                                ui.label(
                                                    RichText::new("✔ .sav")
                                                        .small()
                                                        .color(Color32::from_rgb(100, 220, 120)),
                                                );
                                            } else {
                                                ui.label(RichText::new("—").small().color(TEXT_MUTED));
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if ui.button("▶ Play").clicked() {
                                                        launch_target = Some(entry.path.clone());
                                                    }
                                                },
                                            );
                                        });
                                    });
                                });

                                if card_resp.response.interact(Sense::click()).double_clicked() {
                                    launch_target = Some(entry.path.clone());
                                }
                                card_resp.response.context_menu(|ui| {
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
                                    if ui.button("⚙ Configure Game Overrides").clicked() {
                                        launch_target = Some(entry.path.clone());
                                        open_overrides = true;
                                        ui.close_menu();
                                    }
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
                            }
                        });
                    });
            }
        }

        if rescan_needed {
            self.rescan_rom_folders();
        }

        if let Some(path) = launch_target {
            if let Err(e) = self.open_rom(&path, None, Some(ctx)) {
                self.notify_error(&e);
            }
            if open_overrides {
                self.debugger.show_overrides = true;
            }
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
}
