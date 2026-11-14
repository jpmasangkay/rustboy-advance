use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke};

use crate::app::RustboyApp;
use crate::config::{HotkeyAction, KeyBindings};
use crate::rom_loader::{
    RomPlatform, RomSourceKind, detect_rom_platform, gb_cart_type_label, gb_short_game_code,
    is_supported_rom_or_archive_extension, read_rom_header_summary,
};
use crate::theme::*;

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

impl TableSortColumn {
    pub(crate) const ALL: [TableSortColumn; 5] = [
        TableSortColumn::Title,
        TableSortColumn::Region,
        TableSortColumn::Size,
        TableSortColumn::LastPlayed,
        TableSortColumn::PlayTime,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Region => "Region",
            Self::Size => "Size",
            Self::LastPlayed => "Last Played",
            Self::PlayTime => "Play Time",
        }
    }
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

pub(crate) fn first_frame_cover_path(rom_path: &Path) -> Option<PathBuf> {
    let stem = rom_path.file_stem()?.to_string_lossy();
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(crate::config::AppConfig::covers_dir().join(format!("{trimmed}.firstframe.png")))
}

pub(crate) fn has_user_cover_for_path(rom_path: &Path, rom_code: &str) -> bool {
    let stem = rom_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dummy = RomLibraryEntry {
        path: rom_path.to_path_buf(),
        display_name: stem,
        internal_title: String::new(),
        game_code: rom_code.to_string(),
        region: "",
        size_mb: 0.0,
        has_battery_sav: false,
        source_kind: RomSourceKind::Raw,
        platform: RomPlatform::Gba,
        cart_type_label: String::new(),
    };
    find_user_cover_image(&dummy).is_some()
}

pub(crate) fn capture_first_frame_cover_fallback(
    rom_path: &Path,
    rom_code: &str,
    fb_bgr555: &[u16; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT],
    is_gb: bool,
    force_fallback: bool,
) -> Result<Option<PathBuf>, String> {
    if has_user_cover_for_path(rom_path, rom_code) {
        return Ok(None);
    }
    let Some(dest) = first_frame_cover_path(rom_path) else {
        return Ok(None);
    };
    if dest.exists() {
        return Ok(None);
    }

    let mut rgba_full = vec![0u8; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT * 4];
    crate::capture::bgr555_to_rgba(
        fb_bgr555,
        true,
        crate::config::ColorProfileChoice::Agb001,
        &mut rgba_full,
    );
    let (rgba, width, height) = if is_gb {
        (
            crate::gb_bridge::crop_gb_rgba_from_240x160(&rgba_full),
            gb_core::LCD_WIDTH as u32,
            gb_core::LCD_HEIGHT as u32,
        )
    } else {
        (
            rgba_full,
            gba_core::LCD_WIDTH as u32,
            gba_core::LCD_HEIGHT as u32,
        )
    };

    if !force_fallback && is_uniform_rgba_frame(&rgba) {
        return Ok(None);
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed creating covers dir: {e}"))?;
    }
    // Atomically create the fallback file so existing artwork is never clobbered.
    let file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(e) => return Err(format!("Failed creating first-frame cover file: {e}")),
    };
    let mut writer = std::io::BufWriter::new(file);
    let encoder = image::codecs::png::PngEncoder::new(&mut writer);
    use image::ImageEncoder;
    if let Err(e) = encoder.write_image(&rgba, width, height, image::ExtendedColorType::Rgba8) {
        let _ = fs::remove_file(&dest);
        return Err(format!("Failed encoding first-frame cover PNG: {e}"));
    }
    Ok(Some(dest))
}

fn is_uniform_rgba_frame(rgba: &[u8]) -> bool {
    let chunks = rgba.as_chunks::<4>().0;
    let Some(first) = chunks.first() else {
        return true;
    };
    let (r0, g0, b0) = (first[0], first[1], first[2]);
    let mut distinct_pixels = 0usize;
    for px in chunks {
        if px[0].abs_diff(r0) > 6 || px[1].abs_diff(g0) > 6 || px[2].abs_diff(b0) > 6 {
            distinct_pixels += 1;
            if distinct_pixels >= 48 {
                return false;
            }
        }
    }
    true
}

pub(crate) fn find_cover_image(entry: &RomLibraryEntry) -> Option<PathBuf> {
    if let Some(user_cover) = find_user_cover_image(entry) {
        return Some(user_cover);
    }
    first_frame_cover_path(&entry.path).filter(|p| p.is_file())
}

fn find_user_cover_image(entry: &RomLibraryEntry) -> Option<PathBuf> {
    let stem = entry.path.file_stem()?.to_string_lossy();
    let display = &entry.display_name;

    // 1. In AppData/portable covers/, near executable, or alongside ROM
    let mut search_dirs = Vec::new();
    search_dirs.push(crate::config::AppConfig::covers_dir());
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        search_dirs.push(exe_dir.join("covers"));
    }

    for dir in &search_dirs {
        let candidates = [
            dir.join(format!("{display}.png")),
            dir.join(format!("{display}.jpg")),
            dir.join(format!("{display}.jpeg")),
            dir.join(format!("{stem}.png")),
            dir.join(format!("{stem}.jpg")),
            dir.join(format!("{stem}.jpeg")),
            dir.join(format!("{}.png", entry.game_code)),
            dir.join(format!("{}.jpg", entry.game_code)),
        ];
        for c in &candidates {
            if c.exists() {
                return Some(c.clone());
            }
        }
    }

    // 2. Alongside ROM or in parent/covers
    if let Some(parent) = entry.path.parent() {
        let parent_covers = parent.join("covers");
        let local_candidates = [
            entry.path.with_extension("png"),
            entry.path.with_extension("jpg"),
            entry.path.with_extension("jpeg"),
            parent.join(format!("{stem}.cover.png")),
            parent.join(format!("{stem}_cover.png")),
            parent_covers.join(format!("{display}.png")),
            parent_covers.join(format!("{display}.jpg")),
            parent_covers.join(format!("{stem}.png")),
            parent_covers.join(format!("{stem}.jpg")),
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

pub(crate) fn trigger_cover_download(
    entry: &RomLibraryEntry,
    done_tx: Option<mpsc::Sender<(PathBuf, bool)>>,
) {
    let entry_path = entry.path.clone();
    let display_name = entry.display_name.clone();
    let stem = entry
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| display_name.clone());
    let region = entry.region.to_string();
    let platform = entry.platform;

    std::thread::spawn(move || {
        let repo_folder = match platform {
            RomPlatform::Gba => "Nintendo_-_Game_Boy_Advance",
            RomPlatform::Gbc => "Nintendo_-_Game_Boy_Color",
            RomPlatform::Gb => "Nintendo_-_Game_Boy",
        };

        // Try candidate names in order
        let mut candidates = vec![display_name.clone()];
        if stem != display_name {
            candidates.push(stem.clone());
        }
        if !display_name.contains('(') && !region.is_empty() {
            candidates.push(format!("{display_name} ({region})"));
        }

        let mut downloaded_bytes = None;
        for cand in &candidates {
            let encoded = url_encode_name(cand);
            let url = format!(
                "https://raw.githubusercontent.com/libretro-thumbnails/{repo_folder}/master/Named_Boxarts/{encoded}.png"
            );
            if let Ok(resp) = ureq::get(&url)
                .timeout(std::time::Duration::from_secs(6))
                .call()
                && resp.status() == 200
            {
                let mut bytes = Vec::new();
                use std::io::Read;
                if resp.into_reader().read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
                    downloaded_bytes = Some(bytes);
                    break;
                }
            }
        }

        let succeeded = downloaded_bytes.is_some();
        if let Some(bytes) = downloaded_bytes {
            let appdata_covers = crate::config::AppConfig::covers_dir();
            let _ = std::fs::create_dir_all(&appdata_covers);
            let _ = std::fs::write(appdata_covers.join(format!("{display_name}.png")), &bytes);

            if let Ok(exe) = std::env::current_exe()
                && let Some(exe_dir) = exe.parent()
            {
                let _ = std::fs::create_dir_all(exe_dir.join("covers"));
                let _ = std::fs::write(
                    exe_dir.join("covers").join(format!("{display_name}.png")),
                    &bytes,
                );
                if let Some(target_dir) = exe_dir.parent()
                    && let Some(proj) = target_dir.parent()
                {
                    let _ = std::fs::create_dir_all(proj.join("covers"));
                    let _ = std::fs::write(
                        proj.join("covers").join(format!("{display_name}.png")),
                        &bytes,
                    );
                }
            }
        }
        if let Some(tx) = done_tx {
            let _ = tx.send((entry_path, succeeded));
        }
    });
}

fn url_encode_name(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z'
            | b'A'..=b'Z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'('
            | b')'
            | b'!'
            | b'\'' => {
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

/// Parses internal title, game code, region, and cartridge label from a GBA header slice,
/// falling back to `"Homebrew"` when no cartridge header is present.
fn parse_gba_cart_metadata(header: &[u8]) -> (String, String, &'static str, String) {
    if header.len() < 0xB0 {
        return (
            String::from("Homebrew"),
            String::from("HOME"),
            "",
            String::from("GBA ROM"),
        );
    }

    let title_raw = &header[0xA0..0xAC];
    let has_printable_title = title_raw
        .iter()
        .take_while(|&&c| c != 0)
        .all(|&c| c.is_ascii_graphic() || c == b' ');
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
    let has_valid_code = code_raw.iter().all(|&c| c.is_ascii_alphanumeric());
    let title_trimmed = title_clean.trim();
    let has_cart_header = crate::rom_loader::is_valid_gba_rom_header(header)
        || (has_printable_title
            && has_valid_code
            && title_trimmed.chars().any(|c| c.is_ascii_alphanumeric()));

    if !has_cart_header {
        return (
            String::from("Homebrew"),
            String::from("HOME"),
            "",
            String::from("GBA ROM"),
        );
    }

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
        _ => "",
    };
    (
        title_trimmed.to_string(),
        code_clean,
        reg,
        String::from("GBA Cart"),
    )
}

/// Formats the region label shown in the library UI, labeling unheadered or homebrew GBA ROMs as `"Homebrew"`.
pub(crate) fn library_region_label(entry: &RomLibraryEntry) -> &str {
    if !entry.region.is_empty() {
        entry.region
    } else if entry.game_code == "HOME" || entry.platform == RomPlatform::Gba {
        "Homebrew"
    } else {
        ""
    }
}

/// Formats the empty-state Controls summary line from live keybindings in config.
pub(crate) fn format_empty_state_controls_hint(kb: &KeyBindings) -> String {
    let dpad = if kb.up == "ArrowUp"
        && kb.down == "ArrowDown"
        && kb.left == "ArrowLeft"
        && kb.right == "ArrowRight"
    {
        "Arrow Keys".to_string()
    } else {
        format!("{}/{}/{}/{}", kb.up, kb.down, kb.left, kb.right)
    };
    let pause_key = kb.hotkeys.display(HotkeyAction::Pause);
    format!(
        "3. Controls: {dpad} = D-Pad, {} / {} = B / A, {} / {} = L / R, {} = Start, {pause_key} / Esc = Pause.",
        kb.b, kb.a, kb.l, kb.r, kb.start
    )
}

impl RustboyApp {
    fn draw_welcome_card(&mut self, ui: &mut egui::Ui, centered: bool) {
        let max_w = if centered {
            540.0
        } else {
            ui.available_width().min(920.0)
        };
        let draw_inner = |ui: &mut egui::Ui, app: &mut Self| {
            egui::Frame::new()
                .fill(COLOR_SURFACE)
                .stroke(STROKE_BORDER)
                .corner_radius(RADIUS_DEFAULT)
                .inner_margin(if centered { 22.0 } else { 14.0 })
                .show(ui, |ui| {
                    ui.set_width((max_w - if centered { 44.0 } else { 28.0 }).max(240.0));
                    ui.label(
                        RichText::new("Welcome to Rustboy Advance")
                            .size(if centered { 20.0 } else { 16.0 })
                            .strong()
                            .color(COLOR_TEXT),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Set up your game folder and review controls before jumping in.",
                        )
                        .size(12.5)
                        .color(COLOR_TEXT_SECONDARY),
                    );
                    ui.add_space(12.0);

                    // Step 1: Game folder
                    egui::Frame::new()
                        .fill(COLOR_TILE)
                        .stroke(STROKE_BORDER)
                        .corner_radius(RADIUS_DEFAULT)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new("1. Choose Game Folder")
                                            .size(13.0)
                                            .strong()
                                            .color(COLOR_TEXT),
                                    );
                                    let folder_status = if app.rom_folders.is_empty() {
                                        "Add a folder containing .gba, .gb, .gbc, or .zip games."
                                            .to_string()
                                    } else {
                                        let count = app.rom_folders.len();
                                        let noun = if count == 1 { "folder" } else { "folders" };
                                        format!(
                                            "{count} {noun} configured ({} games found)",
                                            app.library_entries.len()
                                        )
                                    };
                                    ui.label(
                                        RichText::new(folder_status).size(11.5).color(
                                            if app.rom_folders.is_empty() {
                                                COLOR_TEXT_SECONDARY
                                            } else {
                                                ACCENT_TEAL
                                            },
                                        ),
                                    );
                                });
                                ui.with_layout(
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        let btn_label = if app.rom_folders.is_empty() {
                                            "📁 Choose Game Folder..."
                                        } else {
                                            "📁 Add Another Folder..."
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(btn_label).color(COLOR_TEXT),
                                                )
                                                .fill(COLOR_BG)
                                                .stroke(STROKE_BORDER),
                                            )
                                            .clicked()
                                        {
                                            app.add_rom_folder_dialog();
                                        }
                                    },
                                );
                            });
                        });

                    ui.add_space(8.0);

                    // Step 2: Controls & Gamepad
                    egui::Frame::new()
                        .fill(COLOR_TILE)
                        .stroke(STROKE_BORDER)
                        .corner_radius(RADIUS_DEFAULT)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_max_width((ui.available_width() - 165.0).max(160.0));
                                    ui.label(
                                        RichText::new("2. Check Controls")
                                            .size(13.0)
                                            .strong()
                                            .color(COLOR_TEXT),
                                    );
                                    let controls_summary = format_empty_state_controls_hint(
                                        &app.config.keybindings,
                                    );
                                    let controls_summary = controls_summary
                                        .strip_prefix("3. Controls: ")
                                        .unwrap_or(&controls_summary);
                                    ui.label(
                                        RichText::new(controls_summary)
                                            .size(11.5)
                                            .color(COLOR_TEXT_SECONDARY),
                                    );
                                    let pads = app.connected_gamepad_names();
                                    let pad_text = if pads.is_empty() {
                                        "Controller: None connected (auto-detected when plugged in)"
                                            .to_string()
                                    } else {
                                        format!("Controller connected: {}", pads.join(", "))
                                    };
                                    ui.label(
                                        RichText::new(pad_text).size(11.0).color(
                                            if pads.is_empty() {
                                                COLOR_TEXT_SECONDARY
                                            } else {
                                                ACCENT_TEAL
                                            },
                                        ),
                                    );
                                });
                                ui.with_layout(
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("🎮 Check Controls...")
                                                        .color(COLOR_TEXT),
                                                )
                                                .fill(COLOR_BG)
                                                .stroke(STROKE_BORDER),
                                            )
                                            .clicked()
                                        {
                                            app.show_controls_dialog = true;
                                        }
                                    },
                                );
                            });
                        });

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let primary_fill = if app.rom_folders.is_empty() {
                            COLOR_TILE
                        } else {
                            ACCENT_TEAL
                        };
                        let primary_text = if app.rom_folders.is_empty() {
                            COLOR_TEXT
                        } else {
                            Color32::from_rgb(14, 18, 20)
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Continue to Library")
                                        .strong()
                                        .color(primary_text),
                                )
                                .fill(primary_fill)
                                .stroke(STROKE_BORDER),
                            )
                            .clicked()
                        {
                            app.config.welcome_completed = true;
                            app.config.save();
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Skip").color(COLOR_TEXT_SECONDARY),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(STROKE_BORDER),
                            )
                            .clicked()
                        {
                            app.config.welcome_completed = true;
                            app.config.save();
                        }
                    });
                });
        };

        if centered {
            ui.vertical_centered(|ui| {
                ui.add_space(28.0);
                draw_inner(ui, self);
            });
        } else {
            ui.vertical_centered(|ui| {
                draw_inner(ui, self);
            });
            ui.add_space(10.0);
        }
    }

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

        let platform =
            detect_rom_platform(path, summary.inner_name.as_deref(), &header[..bytes_read]);

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
                    _ => "",
                };
                let cart = gb_cart_type_label(gb_hdr.cart_type).to_string();
                (title, code, reg, cart)
            }
            RomPlatform::Gba => parse_gba_cart_metadata(&header[..bytes_read]),
        };

        let has_battery_sav =
            crate::config::AppConfig::find_existing_sav_path_for_rom(path).is_some();

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
        self.attempted_cover_downloads.clear();
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
                if let Some(sel) = &self.selected_rom_path
                    && !self.library_entries.iter().any(|e| &e.path == sel)
                {
                    self.selected_rom_path = None;
                }
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

    /// Drains completed cover art download notifications so idle repaints stop once finished.
    pub(crate) fn poll_cover_downloads(&mut self, ctx: &egui::Context) {
        let mut any_finished = false;
        while let Ok((path, succeeded)) = self.cover_download_rx.try_recv() {
            self.pending_cover_downloads.remove(&path);
            if succeeded {
                self.cover_textures.remove(&path);
            }
            any_finished = true;
        }
        if any_finished {
            ctx.request_repaint();
        }
    }

    /// Sorts library entries according to the active `table_sort_column` and direction.
    pub(crate) fn sort_library_entries(&self, entries: &mut [RomLibraryEntry]) {
        entries.sort_by(|a, b| {
            let ord = match self.table_sort_column {
                TableSortColumn::Title => a
                    .display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase()),
                TableSortColumn::Region => library_region_label(a).cmp(library_region_label(b)),
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
            let ordered = if self.table_sort_descending {
                ord.reverse()
            } else {
                ord
            };
            ordered.then_with(|| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            })
        });
    }

    /// Launches a ROM from the library or recent menu, pruning ghost entries cleanly if deleted.
    pub(crate) fn launch_library_rom(&mut self, path: &Path, ctx: &egui::Context) {
        if !path.is_file() {
            let file_label = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Selected ROM");
            let prev_len = self.library_entries.len();
            self.library_entries.retain(|e| e.path != path);
            if self.selected_rom_path.as_deref() == Some(path) {
                self.selected_rom_path = None;
            }
            self.cover_textures.remove(path);
            self.pending_cover_downloads.remove(path);
            self.attempted_cover_downloads.remove(path);

            let path_str = path.to_string_lossy();
            let before_recent = self.config.recent_roms.len();
            self.config.recent_roms.retain(|r| r.path != path_str);
            if self.config.recent_roms.len() != before_recent {
                self.config.save();
            }

            let ghost_note = if self.library_entries.len() != prev_len {
                " The missing entry was removed from the library list."
            } else {
                ""
            };
            let msg = format!(
                "Could not launch '{file_label}': file was not found at '{}'.{ghost_note}",
                path.display()
            );
            if self.has_active_game() {
                self.notify_error(&msg);
            }
            self.library_scan_error = Some(msg);
            return;
        }

        if let Err(e) = self.open_rom(path, None, Some(ctx)) {
            let file_label = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Selected ROM");
            let msg = format!("Could not launch '{file_label}': {e}");
            if self.has_active_game() {
                self.notify_error(&e);
            }
            self.library_scan_error = Some(msg);
        }
    }

    pub(crate) fn remove_rom_folder_from_vec(
        rom_folders: &mut Vec<PathBuf>,
        entry_path: &Path,
    ) -> bool {
        let before = rom_folders.len();
        rom_folders.retain(|f| !entry_path.starts_with(f));
        if rom_folders.len() == before
            && let Some(parent) = entry_path.parent()
        {
            rom_folders.retain(|f| f != parent);
        }
        rom_folders.len() != before
    }

    fn remove_rom_folder_for_entry(&mut self, entry_path: &Path) {
        if Self::remove_rom_folder_from_vec(&mut self.rom_folders, entry_path) {
            self.save_persistent_config();
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
                    let add_btn =
                        egui::Button::new(RichText::new("📁 Add ROM Folder...").color(COLOR_TEXT))
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
                    let refresh_btn =
                        egui::Button::new(RichText::new("🔄 Refresh").color(refresh_color))
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(STROKE_BORDER)
                            .min_size(egui::vec2(0.0, 26.0));
                    if ui.add_enabled(refresh_enabled, refresh_btn).clicked() {
                        self.rescan_rom_folders();
                    }

                    ui.add_space(6.0);
                    if ui
                        .checkbox(&mut self.config.auto_download_covers, "Download Covers")
                        .on_hover_text(
                            "Opt-in: download missing box art thumbnails from thumbnails.libretro.com via GitHub.",
                        )
                        .changed()
                    {
                        self.config.save();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Table / Grid toggle buttons (26x26)
                        let is_grid = self.library_view_mode == LibraryViewMode::Grid;
                        let is_table = self.library_view_mode == LibraryViewMode::Table;

                        let grid_stroke = if is_grid { STROKE_TEAL } else { STROKE_BORDER };
                        let grid_color = if is_grid {
                            ACCENT_TEAL
                        } else {
                            COLOR_TEXT_SECONDARY
                        };
                        let grid_btn = egui::Button::new(RichText::new("⊞").color(grid_color))
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(grid_stroke)
                            .min_size(egui::vec2(26.0, 26.0));
                        if ui
                            .add(grid_btn)
                            .on_hover_text("Switch to Grid View (box art tiles)")
                            .clicked()
                        {
                            self.library_view_mode = LibraryViewMode::Grid;
                            self.save_persistent_config();
                        }

                        let table_stroke = if is_table { STROKE_TEAL } else { STROKE_BORDER };
                        let table_color = if is_table {
                            ACCENT_TEAL
                        } else {
                            COLOR_TEXT_SECONDARY
                        };
                        let table_btn = egui::Button::new(RichText::new("☰").color(table_color))
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(table_stroke)
                            .min_size(egui::vec2(26.0, 26.0));
                        if ui
                            .add(table_btn)
                            .on_hover_text("Switch to List / Table View")
                            .clicked()
                        {
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
                        let fav_tooltip = if fav_active {
                            "Showing Favorites only (click to show all games)"
                        } else {
                            "Show only Favorite games"
                        };
                        if ui.add(fav_btn).on_hover_text(fav_tooltip).clicked() {
                            self.filter_favorites_only = !self.filter_favorites_only;
                        }

                        // Search box
                        let search_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search ROM library...")
                            .desired_width(180.0)
                            .min_size(egui::vec2(180.0, 26.0));
                        ui.add(search_edit);

                        if is_grid {
                            let dir_arrow = if self.table_sort_descending {
                                "↓"
                            } else {
                                "↑"
                            };
                            let dir_tip = if self.table_sort_descending {
                                "Sort Descending (click for Ascending)"
                            } else {
                                "Sort Ascending (click for Descending)"
                            };
                            let dir_btn = egui::Button::new(
                                RichText::new(dir_arrow).color(COLOR_TEXT),
                            )
                            .corner_radius(RADIUS_DEFAULT)
                            .stroke(STROKE_BORDER)
                            .min_size(egui::vec2(26.0, 26.0));
                            if ui.add(dir_btn).on_hover_text(dir_tip).clicked() {
                                self.table_sort_descending = !self.table_sort_descending;
                            }

                            egui::ComboBox::from_id_salt("library_grid_sort")
                                .width(104.0)
                                .selected_text(
                                    RichText::new(format!(
                                        "Sort: {}",
                                        self.table_sort_column.label()
                                    ))
                                    .size(12.0)
                                    .color(COLOR_TEXT),
                                )
                                .show_ui(ui, |ui| {
                                    for col in TableSortColumn::ALL {
                                        if ui
                                            .selectable_label(
                                                self.table_sort_column == col,
                                                col.label(),
                                            )
                                            .clicked()
                                        {
                                            if self.table_sort_column == col {
                                                self.table_sort_descending =
                                                    !self.table_sort_descending;
                                            } else {
                                                self.table_sort_column = col;
                                                self.table_sort_descending = false;
                                            }
                                        }
                                    }
                                })
                                .response
                                .on_hover_text("Sort games in Grid View");
                        }
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
                            if ui.button(RichText::new("Rescan folders").small()).clicked() {
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
        if !self.config.welcome_completed {
            let welcome_centered = self.rom_folders.is_empty() && filtered.is_empty();
            self.draw_welcome_card(ui, welcome_centered);
            if welcome_centered && !self.config.welcome_completed {
                return;
            }
        }

        if self.rom_folders.is_empty() && filtered.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.16).max(32.0));
                let card_w = (ui.available_width() - 48.0).clamp(320.0, 540.0);
                egui::Frame::new()
                    .fill(COLOR_SURFACE)
                    .stroke(STROKE_BORDER)
                    .corner_radius(RADIUS_DEFAULT)
                    .inner_margin(egui::Margin::symmetric(24, 22))
                    .show(ui, |ui| {
                        ui.set_width(card_w);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new("No Game Directories Configured")
                                    .size(16.0)
                                    .strong()
                                    .color(COLOR_TEXT),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Add a folder with your .gba, .agb, .bin, .gb, .gbc, .zip, or .7z ROMs to populate the library, or open a single file.",
                                )
                                .color(COLOR_TEXT_SECONDARY)
                                .size(12.0),
                            );
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                let total_btn_w = 150.0 * 2.0 + 10.0;
                                let pad = (ui.available_width() - total_btn_w).max(0.0) * 0.5;
                                ui.add_space(pad);

                                let add_btn = egui::Button::new(
                                    RichText::new("📁 Add Game Folder...").color(COLOR_TEXT),
                                )
                                .fill(ACCENT_TEAL_BG)
                                .corner_radius(RADIUS_DEFAULT)
                                .stroke(STROKE_TEAL)
                                .min_size(egui::vec2(150.0, 30.0));
                                if ui.add(add_btn).clicked() {
                                    self.add_rom_folder_dialog();
                                }

                                ui.add_space(10.0);

                                let open_btn = egui::Button::new(
                                    RichText::new("📄 Open ROM File...").color(COLOR_TEXT),
                                )
                                .corner_radius(RADIUS_DEFAULT)
                                .stroke(STROKE_BORDER)
                                .min_size(egui::vec2(150.0, 30.0));
                                if ui.add(open_btn).clicked() {
                                    self.trigger_open_rom_dialog(ctx);
                                }
                            });
                        });

                        ui.add_space(16.0);
                        egui::Frame::new()
                            .fill(COLOR_TILE)
                            .stroke(STROKE_BORDER)
                            .corner_radius(RADIUS_DEFAULT)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width((card_w - 28.0).max(200.0));
                                ui.label(
                                    RichText::new("How to play")
                                        .size(12.0)
                                        .strong()
                                        .color(ACCENT_TEAL),
                                );
                                ui.add_space(6.0);
                                let controls_tip =
                                    format_empty_state_controls_hint(&self.config.keybindings);
                                for tip in [
                                    "1. Click Add Game Folder above (or + Open in the toolbar) to pick your games.",
                                    "2. Double-click any title to start playing; progress saves automatically to .sav.",
                                    controls_tip.as_str(),
                                ] {
                                    ui.label(
                                        RichText::new(tip)
                                            .size(11.5)
                                            .color(COLOR_TEXT_SECONDARY),
                                    );
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
                        RichText::new(
                            "Indexing .gba, .agb, .bin, .gb, .gbc, .zip, and .7z ROM headers",
                        )
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
                    ui.heading(RichText::new("No Matching ROMs Found").size(20.0).strong());
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(
                            "No games match the current search query or favorites filter.",
                        )
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
        let mut remove_folder_path: Option<PathBuf> = None;
        let mut rescan_needed = false;
        let mut toggle_favorite: Option<PathBuf> = None;

        match self.library_view_mode {
            LibraryViewMode::Table => {
                let avail_w = ui.available_width();
                let fav_col_w = 24.0_f32;
                let region_w = 80.0_f32;
                let size_w = 90.0_f32;
                let last_played_w = 130.0_f32;
                let play_time_w = 100.0_f32;
                let title_w =
                    (avail_w - fav_col_w - region_w - size_w - last_played_w - play_time_w - 40.0)
                        .max(180.0);

                let mut sorted_entries = filtered;
                self.sort_library_entries(&mut sorted_entries);

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
                        ui.add_space(16.0 + fav_col_w);
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
                                        #[cfg(windows)]
                                        {
                                            use std::os::windows::process::CommandExt;
                                            let _ = std::process::Command::new("explorer")
                                                .arg(parent)
                                                .creation_flags(0x08000000)
                                                .spawn();
                                        }
                                        #[cfg(not(windows))]
                                        {
                                            let _ = std::process::Command::new("xdg-open")
                                                .arg(parent)
                                                .spawn();
                                        }
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
                                    remove_folder_path = Some(entry.path.clone());
                                    ui.close_menu();
                                }
                            });

                            let bg_color = if is_selected || row_resp.hovered() {
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

                            let is_fav = self.favorites.contains(&entry.path);
                            let lp_str = format_last_played_ts(last_played_ts);
                            let pt_str = format_playtime(total_seconds);
                            let size_str = format_rom_size_mb(entry.size_mb);

                            ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(12.0);
                                    let star_btn = egui::Button::new(
                                        RichText::new(if is_fav { "★" } else { "☆" })
                                            .size(12.0)
                                            .color(if is_fav {
                                                ACCENT_TEAL
                                            } else {
                                                COLOR_TEXT_SECONDARY
                                            }),
                                    )
                                    .frame(false)
                                    .min_size(egui::vec2(20.0, 26.0));
                                    let star_tip = if is_fav {
                                        "Remove from Favorites"
                                    } else {
                                        "Add to Favorites"
                                    };
                                    if ui
                                        .add_sized([20.0, 28.0], star_btn)
                                        .on_hover_text(star_tip)
                                        .clicked()
                                    {
                                        toggle_favorite = Some(entry.path.clone());
                                    }
                                    ui.add_space(8.0);

                                    // Title: left-aligned inside fixed title_w column
                                    let title_resp = ui
                                        .allocate_ui_with_layout(
                                            egui::vec2(title_w, 28.0),
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                ui.set_min_size(egui::vec2(title_w, 28.0));
                                                ui.set_max_width(title_w);
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&entry.display_name)
                                                            .color(COLOR_TEXT)
                                                            .size(12.0),
                                                    )
                                                    .selectable(false)
                                                    .truncate(),
                                                )
                                            },
                                        )
                                        .inner;
                                    if title_resp.clicked() {
                                        self.selected_rom_path = Some(entry.path.clone());
                                    }
                                    if title_resp.double_clicked() {
                                        launch_target = Some(entry.path.clone());
                                    }
                                    title_resp.on_hover_text(&entry.display_name);

                                    // Region: left-aligned inside fixed region_w column ("Homebrew" for homebrew/unheadered GBA ROMs)
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(region_w, 28.0),
                                        Layout::left_to_right(Align::Center),
                                        |ui| {
                                            ui.set_min_size(egui::vec2(region_w, 28.0));
                                            ui.set_max_width(region_w);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(library_region_label(entry))
                                                        .color(COLOR_TEXT_SECONDARY)
                                                        .size(12.0),
                                                )
                                                .selectable(false),
                                            );
                                        },
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
                let mut sorted_entries = filtered;
                self.sort_library_entries(&mut sorted_entries);

                // Ensure covers are loaded or scheduled once for download
                for entry in &sorted_entries {
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
                        } else if self.config.auto_download_covers
                            && !self.pending_cover_downloads.contains(&entry.path)
                            && !self.attempted_cover_downloads.contains(&entry.path)
                        {
                            self.pending_cover_downloads.insert(entry.path.clone());
                            self.attempted_cover_downloads.insert(entry.path.clone());
                            trigger_cover_download(entry, Some(self.cover_download_tx.clone()));
                        }
                    }
                }

                if !self.pending_cover_downloads.is_empty() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 16,
                                right: 16,
                                top: 12,
                                bottom: 16,
                            })
                            .show(ui, |ui| {
                                let tile_size = 160.0_f32; // Authentic GBA 1:1 square boxart
                                let tile_w = tile_size;
                                let tile_h = tile_size;
                                let card_meta_h = 40.0_f32;
                                let spacing_x = 16.0_f32;
                                let spacing_y = 20.0_f32;
                                let avail_w = ui.available_width().max(tile_w);
                                let cols = (((avail_w + spacing_x) / (tile_w + spacing_x)).floor()
                                    as usize)
                                    .max(1);

                                ui.spacing_mut().item_spacing = egui::vec2(spacing_x, spacing_y);

                                for row_entries in sorted_entries.chunks(cols) {
                                    ui.horizontal_top(|ui| {
                                        ui.spacing_mut().item_spacing.x = spacing_x;
                                        for entry in row_entries {
                                            let is_selected =
                                                self.selected_rom_path.as_ref() == Some(&entry.path);

                                            let (card_rect, _) = ui.allocate_exact_size(
                                                egui::vec2(tile_w, tile_h + card_meta_h),
                                                Sense::hover(),
                                            );
                                            ui.scope_builder(
                                                egui::UiBuilder::new()
                                                    .max_rect(card_rect)
                                                    .layout(Layout::top_down(Align::Min)),
                                                |ui| {
                                                    ui.set_width(tile_w);
                                                    ui.spacing_mut().item_spacing.y = 0.0;

                                                    // 1. Cover Tile (1:1 square aspect ratio)
                                                    let (tile_rect, tile_resp) = ui
                                                        .allocate_exact_size(
                                                            egui::vec2(tile_w, tile_h),
                                                            Sense::click(),
                                                        );

                                                    if tile_resp.clicked() {
                                                        self.selected_rom_path =
                                                            Some(entry.path.clone());
                                                    }
                                                    if tile_resp.double_clicked() {
                                                        launch_target = Some(entry.path.clone());
                                                    }

                                                    tile_resp.context_menu(|ui| {
                                                        if ui.button("▶ Play Game").clicked() {
                                                            launch_target =
                                                                Some(entry.path.clone());
                                                            ui.close_menu();
                                                        }
                                                        if ui.button("📁 Open Save Folder").clicked()
                                                        {
                                                            if let Some(parent) = entry.path.parent()
                                                            {
                                                                #[cfg(windows)]
                                                                {
                                                                    use std::os::windows::process::CommandExt;
                                                                    let _ = std::process::Command::new(
                                                                        "explorer",
                                                                    )
                                                                    .arg(parent)
                                                                    .creation_flags(0x08000000)
                                                                    .spawn();
                                                                }
                                                                #[cfg(not(windows))]
                                                                {
                                                                    let _ = std::process::Command::new(
                                                                        "xdg-open",
                                                                    )
                                                                    .arg(parent)
                                                                    .spawn();
                                                                }
                                                            }
                                                            ui.close_menu();
                                                        }
                                                        if ui
                                                            .button("🖼 Refresh / Download Cover")
                                                            .clicked()
                                                        {
                                                            self.cover_textures.remove(&entry.path);
                                                            self.attempted_cover_downloads
                                                                .insert(entry.path.clone());
                                                            self.pending_cover_downloads
                                                                .insert(entry.path.clone());
                                                            trigger_cover_download(
                                                                entry,
                                                                Some(self.cover_download_tx.clone()),
                                                            );
                                                            ui.close_menu();
                                                        }
                                                        if ui.button("⚙ Game Settings...").clicked()
                                                        {
                                                            game_settings_entry =
                                                                Some(entry.clone());
                                                            ui.close_menu();
                                                        }
                                                        let is_fav =
                                                            self.favorites.contains(&entry.path);
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
                                                                self.favorites
                                                                    .insert(entry.path.clone());
                                                            }
                                                            self.save_persistent_config();
                                                            ui.close_menu();
                                                        }
                                                        ui.separator();
                                                        if ui
                                                            .button("🗑 Remove Folder from Library")
                                                            .clicked()
                                                        {
                                                            remove_folder_path =
                                                                Some(entry.path.clone());
                                                            ui.close_menu();
                                                        }
                                                    });

                                                    // Base tile background
                                                    ui.painter().rect_filled(
                                                        tile_rect,
                                                        RADIUS_DEFAULT,
                                                        COLOR_TILE,
                                                    );

                                                    // Render cover image if loaded, aspect-fitted inside square tile
                                                    if let Some(tex) =
                                                        self.cover_textures.get(&entry.path)
                                                    {
                                                        let tex_w = tex.size()[0] as f32;
                                                        let tex_h = tex.size()[1] as f32;
                                                        if tex_w > 0.0 && tex_h > 0.0 {
                                                            let scale = (tile_w / tex_w)
                                                                .min(tile_h / tex_h);
                                                            let draw_size = egui::vec2(
                                                                tex_w * scale,
                                                                tex_h * scale,
                                                            );
                                                            let draw_rect =
                                                                egui::Rect::from_center_size(
                                                                    tile_rect.center(),
                                                                    draw_size,
                                                                );
                                                            ui.painter().image(
                                                                tex.id(),
                                                                draw_rect,
                                                                egui::Rect::from_min_max(
                                                                    egui::pos2(0.0, 0.0),
                                                                    egui::pos2(1.0, 1.0),
                                                                ),
                                                                Color32::WHITE,
                                                            );
                                                        }
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

                                                    let tile_hovered = tile_resp.hovered();
                                                    tile_resp.on_hover_text(&entry.display_name);

                                                    // 2. Metadata Directly Below Tile
                                                    ui.add_space(6.0);

                                                    // Title 12px white, single-line ellipsis + full title tooltip
                                                    let title_lbl = egui::Label::new(
                                                        RichText::new(&entry.display_name)
                                                            .color(COLOR_TEXT)
                                                            .size(12.0),
                                                    )
                                                    .selectable(false)
                                                    .truncate();
                                                    let title_resp = ui
                                                        .allocate_ui_with_layout(
                                                            egui::vec2(tile_w, 16.0),
                                                            Layout::left_to_right(Align::Center),
                                                            |ui| ui.add(title_lbl),
                                                        )
                                                        .inner;
                                                    if title_resp.clicked() {
                                                        self.selected_rom_path =
                                                            Some(entry.path.clone());
                                                    }
                                                    if title_resp.double_clicked() {
                                                        launch_target = Some(entry.path.clone());
                                                    }
                                                    title_resp.on_hover_text(&entry.display_name);

                                                    ui.add_space(2.0);

                                                    // "REGION · SIZE" ("Homebrew · SIZE" for unheadered, or "SIZE" when region unknown)
                                                    let size_str =
                                                        format_rom_size_mb(entry.size_mb);
                                                    let region_lbl = library_region_label(entry);
                                                    let meta_str = if region_lbl.is_empty() {
                                                        size_str
                                                    } else {
                                                        format!("{region_lbl} · {size_str}")
                                                    };
                                                    ui.label(
                                                        RichText::new(meta_str)
                                                            .color(COLOR_TEXT_SECONDARY)
                                                            .size(11.0),
                                                    );

                                                    // Subtle top-right Favorite star badge rendered in its own scoped rect so it never shifts title/metadata below the tile
                                                    let is_fav =
                                                        self.favorites.contains(&entry.path);
                                                    let show_star = is_fav || tile_hovered;
                                                    if show_star {
                                                        let star_rect = egui::Rect::from_min_size(
                                                            egui::pos2(
                                                                tile_rect.right() - 26.0,
                                                                tile_rect.top() + 4.0,
                                                            ),
                                                            egui::vec2(22.0, 22.0),
                                                        );
                                                        let star_btn = egui::Button::new(
                                                            RichText::new(if is_fav {
                                                                "★"
                                                            } else {
                                                                "☆"
                                                            })
                                                            .size(13.0)
                                                            .color(if is_fav {
                                                                ACCENT_TEAL
                                                            } else {
                                                                COLOR_TEXT_SECONDARY
                                                            }),
                                                        )
                                                        .fill(Color32::from_rgba_unmultiplied(
                                                            0x12, 0x12, 0x12, 200,
                                                        ))
                                                        .stroke(STROKE_BORDER)
                                                        .corner_radius(RADIUS_DEFAULT);
                                                        let star_tip = if is_fav {
                                                            "Remove from Favorites"
                                                        } else {
                                                            "Add to Favorites"
                                                        };
                                                        ui.scope_builder(
                                                            egui::UiBuilder::new()
                                                                .max_rect(star_rect),
                                                            |ui| {
                                                                if ui
                                                                    .add_sized(
                                                                        star_rect.size(),
                                                                        star_btn,
                                                                    )
                                                                    .on_hover_text(star_tip)
                                                                    .clicked()
                                                                {
                                                                    toggle_favorite =
                                                                        Some(entry.path.clone());
                                                                }
                                                            },
                                                        );
                                                    }
                                                },
                                            );
                                        }
                                    });
                                }
                            });
                    });
            }
        }

        if let Some(fav_path) = toggle_favorite {
            if !self.favorites.remove(&fav_path) {
                self.favorites.insert(fav_path);
            }
            self.save_persistent_config();
        }

        if let Some(rm_path) = remove_folder_path {
            self.remove_rom_folder_for_entry(&rm_path);
            rescan_needed = true;
        }

        if rescan_needed {
            self.rescan_rom_folders();
        }

        if let Some(entry) = game_settings_entry {
            self.open_game_settings_for_library_entry(&entry);
        }

        if let Some(path) = launch_target {
            self.launch_library_rom(&path, ctx);
        }
    }
}

pub(crate) fn format_rom_size_mb(size_mb: f32) -> String {
    if size_mb < 0.99 {
        let kb = (size_mb * 1024.0).round().max(1.0) as u32;
        format!("{kb} KB")
    } else {
        format!("{size_mb:.1} MB")
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

    #[test]
    fn test_rom_size_formatting_and_hidden_unknown_region() {
        assert_eq!(format_rom_size_mb(0.03125), "32 KB");
        assert_eq!(format_rom_size_mb(0.5), "512 KB");
        assert_eq!(format_rom_size_mb(16.0), "16.0 MB");

        let temp_dir = std::env::temp_dir().join("rba_test_unk_region");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let mut rom = vec![0u8; 512];
        rom[0xA0..0xA6].copy_from_slice(b"HOMEBR");
        rom[0xAC..0xB0].copy_from_slice(b"HOMZ");
        let rom_path = temp_dir.join("homebrew_demo.gba");
        fs::write(&rom_path, &rom).unwrap();

        let parsed = RustboyApp::parse_gba_header(&rom_path).expect("parse ok");
        assert_eq!(parsed.region, "");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_homebrew_unheadered_gba_shows_homebrew_and_is_not_rejected() {
        let temp_dir = std::env::temp_dir().join("rba_test_homebrew_gba");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let tiny_path = temp_dir.join("tiny_demo.gba");
        fs::write(&tiny_path, vec![0xEAu8; 64]).unwrap();
        let large_zero_path = temp_dir.join("zero_homebrew.agb");
        fs::write(&large_zero_path, vec![0x00u8; 512]).unwrap();
        let arm_stub_path = temp_dir.join("arm_blob.gba");
        let mut arm_bytes = vec![0xFFu8; 512];
        arm_bytes[0xA0..0xB0].copy_from_slice(&[0x01, 0x02, 0x7F, 0x80].repeat(4));
        fs::write(&arm_stub_path, &arm_bytes).unwrap();

        for path in [&tiny_path, &large_zero_path, &arm_stub_path] {
            let entry = RustboyApp::parse_rom_header(path).expect("homebrew .gba/.agb accepted");
            assert_eq!(entry.internal_title, "Homebrew");
            assert_eq!(entry.game_code, "HOME");
            assert_eq!(entry.region, "");
            assert_eq!(library_region_label(&entry), "Homebrew");
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_scan_filters_bin_and_exe_archives_and_refresh_clears_ghosts() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir().join("rba_test_scan_filter_refresh");
        let _ = fs::remove_dir_all(&temp_dir);
        let sub_dir = temp_dir.join("gba");
        fs::create_dir_all(&sub_dir).unwrap();

        // 1. Valid GBA ROM in subfolder
        let valid_gba = sub_dir.join("Summon Night.gba");
        let mut gba = vec![0u8; 256];
        gba[0xA0..0xAC].copy_from_slice(b"SUMMON NIGHT");
        gba[0xAC..0xB0].copy_from_slice(b"ASNE");
        gba[0xB2] = 0x96;
        gba[0xBD] = crate::rom_loader::gba_header_complement(&gba);
        fs::write(&valid_gba, &gba).unwrap();

        // 2. Unheadered .gba homebrew (must remain visible)
        let homebrew_gba = temp_dir.join("my_homebrew.gba");
        fs::write(&homebrew_gba, vec![0u8; 256]).unwrap();

        // 3. Junk .bin (must be rejected by header check)
        let junk_bin = temp_dir.join("bios_payload.bin");
        fs::write(&junk_bin, vec![0xAAu8; 512]).unwrap();

        // 4. Windows release zip containing .exe + a .gba (must be rejected)
        let app_zip = temp_dir.join("RustboyAdvance-v0.1.0-windows-x64.zip");
        {
            let f = std::fs::File::create(&app_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("test.gba", opts).unwrap();
            zw.write_all(&gba).unwrap();
            zw.start_file("rustboy-advance.exe", opts).unwrap();
            zw.write_all(b"MZ").unwrap();
            zw.finish().unwrap();
        }

        let scan1 = RustboyApp::scan_folders_collect(std::slice::from_ref(&temp_dir));
        assert!(scan1.error.is_none());
        let names1: Vec<&str> = scan1
            .entries
            .iter()
            .map(|e| e.display_name.as_str())
            .collect();
        assert_eq!(names1, vec!["my_homebrew", "Summon Night"]);

        // Delete one ROM from disk -> Refresh scan must remove the deleted ghost entry.
        fs::remove_file(&homebrew_gba).unwrap();
        let scan2 = RustboyApp::scan_folders_collect(std::slice::from_ref(&temp_dir));
        let names2: Vec<&str> = scan2
            .entries
            .iter()
            .map(|e| e.display_name.as_str())
            .collect();
        assert_eq!(names2, vec!["Summon Night"]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_sort_missing_launch_subfolder_remove_and_idle_repaint() {
        use std::mem::ManuallyDrop;

        let mut app = ManuallyDrop::new(RustboyApp::new(crate::headless::CliConfig::default()));
        app.library_scan_rx = None;
        app.library_scanning = false;
        app.rom_folders.clear();
        app.library_entries.clear();
        app.config.auto_download_covers = false;
        app.selected_rom_path = None;
        app.library_scan_error = None;

        let make_entry = |name: &str, reg: &'static str, mb: f32, code: &str| RomLibraryEntry {
            path: PathBuf::from(format!("C:\\roms\\sub\\{name}.gba")),
            display_name: name.to_string(),
            internal_title: name.to_string(),
            game_code: code.to_string(),
            region: reg,
            size_mb: mb,
            has_battery_sav: false,
            source_kind: crate::rom_loader::RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: "GBA Cart".to_string(),
        };

        let mut entries = vec![
            make_entry("Beta", "JPN", 4.0, "BBTJ"),
            make_entry("Alpha", "USA", 16.0, "AALE"),
            make_entry("Gamma", "EUR", 8.0, "CGMP"),
        ];
        app.table_sort_column = TableSortColumn::Size;
        app.table_sort_descending = false;
        app.sort_library_entries(&mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Beta", "Gamma", "Alpha"]
        );
        app.table_sort_descending = true;
        app.sort_library_entries(&mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Gamma", "Beta"]
        );

        // Missing file launch prunes ghost entry and records clear persistent error banner.
        let ghost_path = std::env::temp_dir().join("rba_missing_ghost_title.gba");
        let _ = fs::remove_file(&ghost_path);
        app.library_entries = vec![RomLibraryEntry {
            path: ghost_path.clone(),
            display_name: "Missing Title".into(),
            internal_title: "MISSING".into(),
            game_code: "MISS".into(),
            region: "USA",
            size_mb: 1.0,
            has_battery_sav: false,
            source_kind: crate::rom_loader::RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: "GBA Cart".into(),
        }];
        app.selected_rom_path = Some(ghost_path.clone());
        let ctx = egui::Context::default();
        app.launch_library_rom(&ghost_path, &ctx);
        assert!(app.library_entries.is_empty());
        assert!(app.selected_rom_path.is_none());
        let err = app.library_scan_error.as_deref().unwrap_or("");
        assert!(err.contains("Could not launch"));
        assert!(err.contains("removed from the library list"));

        // Subfolder ROM removes its parent root library folder cleanly.
        let mut folders = vec![PathBuf::from("C:\\roms"), PathBuf::from("D:\\other")];
        assert!(RustboyApp::remove_rom_folder_from_vec(
            &mut folders,
            Path::new("C:\\roms\\sub\\nested\\game.gba")
        ));
        assert_eq!(folders, vec![PathBuf::from("D:\\other")]);

        // Simulate an in-flight cover download failing, completing, and idling with no timer repaints.
        app.library_scan_error = None;
        app.toast = None;
        app.library_view_mode = LibraryViewMode::Grid;
        app.config.auto_download_covers = true;
        let sample_path = entries[0].path.clone();
        app.pending_cover_downloads.insert(sample_path.clone());
        for entry in &entries {
            app.attempted_cover_downloads.insert(entry.path.clone());
        }
        app.library_entries = entries;
        app.cover_download_tx.send((sample_path, false)).unwrap();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120.0, 720.0),
            )),
            ..Default::default()
        };
        let mut last_delay = std::time::Duration::ZERO;
        for _ in 0..4 {
            let out = ctx.run(raw_input.clone(), |ctx| {
                app.update(ctx);
            });
            last_delay = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.repaint_delay)
                .unwrap_or(std::time::Duration::MAX);
        }
        assert!(app.pending_cover_downloads.is_empty());
        assert!(
            last_delay > std::time::Duration::from_secs(60),
            "expected idle library frame to sleep for input only, got repaint_delay={last_delay:?}"
        );
    }

    #[test]
    fn test_homebrew_region_label_and_live_empty_state_controls_hint() {
        let gba_custom = RomLibraryEntry {
            path: PathBuf::from("demo.gba"),
            display_name: "Demo".into(),
            internal_title: "MICRODEMO".into(),
            game_code: "HOMZ".into(),
            region: "",
            size_mb: 0.125,
            has_battery_sav: false,
            source_kind: RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: String::new(),
        };
        assert_eq!(library_region_label(&gba_custom), "Homebrew");

        let mut kb = KeyBindings::default();
        let default_hint = format_empty_state_controls_hint(&kb);
        assert!(default_hint.contains("Arrow Keys = D-Pad"));
        assert!(default_hint.contains("X / Z = B / A"));
        assert!(default_hint.contains("Q / E = L / R"));

        kb.up = "W".into();
        kb.down = "S".into();
        kb.left = "A".into();
        kb.right = "D".into();
        kb.a = "J".into();
        kb.b = "K".into();
        kb.start = "Space".into();
        let custom_hint = format_empty_state_controls_hint(&kb);
        assert!(custom_hint.contains("W/S/A/D = D-Pad"));
        assert!(custom_hint.contains("K / J = B / A"));
        assert!(custom_hint.contains("Space = Start"));
    }

    #[test]
    fn test_first_frame_cover_fallback_writes_and_never_overwrites_user_cover() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let covers_dir = crate::config::AppConfig::covers_dir();
        let _ = fs::create_dir_all(&covers_dir);

        // 1. Uniform solid frame is skipped when force_fallback = false, then saved when forced or patterned.
        let rom_auto = PathBuf::from(format!("b1_auto_{nonce}.gba"));
        let solid_fb = [0x001Fu16; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT];
        let skipped =
            capture_first_frame_cover_fallback(&rom_auto, "B1AT", &solid_fb, false, false).unwrap();
        assert!(skipped.is_none(), "uniform boot frame should be skipped before fallback");

        let mut patterned_fb = solid_fb;
        for (idx, px) in patterned_fb.iter_mut().enumerate().take(400) {
            *px = ((idx as u16) * 17) & 0x7FFF;
        }
        let saved = capture_first_frame_cover_fallback(&rom_auto, "B1AT", &patterned_fb, false, false)
            .unwrap()
            .expect("non-uniform frame should write firstframe png");
        assert!(saved.is_file());
        assert!(saved.to_string_lossy().ends_with(".firstframe.png"));

        // Verify find_cover_image picks up firstframe fallback when no user cover exists.
        let entry_auto = RomLibraryEntry {
            path: rom_auto.clone(),
            display_name: format!("b1_auto_{nonce}"),
            internal_title: String::new(),
            game_code: "B1AT".into(),
            region: "Homebrew",
            size_mb: 0.1,
            has_battery_sav: false,
            source_kind: RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: String::new(),
        };
        assert_eq!(find_cover_image(&entry_auto), Some(saved.clone()));

        // Placing a real user cover in covers_dir immediately takes priority without overwriting anything.
        let user_cover = covers_dir.join(format!("b1_auto_{nonce}.png"));
        let sentinel = b"user-cover-sentinel";
        fs::write(&user_cover, sentinel).unwrap();
        assert_eq!(find_cover_image(&entry_auto), Some(user_cover.clone()));
        // Calling capture again never overwrites user_cover or firstframe.
        assert!(
            capture_first_frame_cover_fallback(&rom_auto, "B1AT", &patterned_fb, false, true)
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(&user_cover).unwrap(), sentinel);
        let _ = fs::remove_file(&user_cover);
        let _ = fs::remove_file(&saved);

        // 2. Game with existing adjacent user cover never creates a first-frame fallback file.
        let rom_with_cover_stem = format!("b1_existing_{nonce}");
        let existing_user_cover = covers_dir.join(format!("{rom_with_cover_stem}.png"));
        fs::write(&existing_user_cover, b"existing-user-art").unwrap();
        let rom_with_cover = PathBuf::from(format!("{rom_with_cover_stem}.gba"));
        let created = capture_first_frame_cover_fallback(
            &rom_with_cover,
            "B1EX",
            &patterned_fb,
            false,
            true,
        )
        .unwrap();
        assert!(created.is_none(), "must never create fallback when user cover exists");
        assert!(!first_frame_cover_path(&rom_with_cover).unwrap().exists());
        assert_eq!(fs::read(&existing_user_cover).unwrap(), b"existing-user-art");
        let _ = fs::remove_file(&existing_user_cover);
    }

    #[test]
    fn test_welcome_card_renders_and_stays_idle_without_timer_repaints() {
        let mut app =
            std::mem::ManuallyDrop::new(RustboyApp::new(crate::headless::CliConfig::default()));
        app.rom_folders.clear();
        app.library_entries.clear();
        app.config.welcome_completed = false;
        let ctx = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1120.0, 720.0),
            )),
            ..Default::default()
        };
        let mut last_delay = std::time::Duration::ZERO;
        for _ in 0..3 {
            let out = ctx.run(raw_input.clone(), |ctx| {
                app.update(ctx);
            });
            last_delay = out
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|v| v.repaint_delay)
                .unwrap_or(std::time::Duration::MAX);
        }
        assert!(
            last_delay > std::time::Duration::from_secs(60),
            "welcome card must stay idle (repaint on input only), got {last_delay:?}"
        );
    }
}
