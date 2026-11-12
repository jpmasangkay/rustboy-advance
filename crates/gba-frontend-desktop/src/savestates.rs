use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, Layout, RichText, Sense, Stroke,
    TextureOptions, Vec2,
};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};
use image::ImageEncoder;

use crate::app::{ACCENT_TEAL, BG_CARD, BG_PANEL, BORDER_HAIRLINE, RustboyApp, TEXT_MUTED};

const RBAS_MAGIC: [u8; 4] = *b"RBAS";
pub(crate) const RBAS_VERSION: u16 = 1;
const FLAG_CORE_LZ4: u16 = 1 << 0;
const HEADER_LEN: usize = 38;
const MAX_TITLE_LEN: usize = 512;
const MAX_THUMB_LEN: usize = 4 * 1024 * 1024;
const MAX_CORE_LEN: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveStateMeta {
    pub(crate) version: u16,
    pub(crate) flags: u16,
    pub(crate) timestamp_secs: u64,
    pub(crate) rom_crc32: u32,
    pub(crate) game_code: String,
    pub(crate) rom_title: String,
    pub(crate) thumbnail_png: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodedSaveState {
    pub(crate) meta: SaveStateMeta,
    pub(crate) core_bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveStateError {
    Io(String),
    Corrupt(String),
    VersionMismatch {
        found: u16,
        expected: u16,
    },
    WrongGame {
        expected_title: String,
        expected_code: String,
        found_title: String,
        found_code: String,
    },
    CoreError(String),
}

impl fmt::Display for SaveStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "Could not read save state: {msg}"),
            Self::Corrupt(msg) => write!(f, "Corrupt save state file: {msg}"),
            Self::VersionMismatch { found, expected } => write!(
                f,
                "Save state version mismatch (file v{found}, expected v{expected})"
            ),
            Self::WrongGame {
                expected_title,
                expected_code,
                found_title,
                found_code,
            } => write!(
                f,
                "Wrong game: state is for '{found_title}' [{found_code}], expected '{expected_title}' [{expected_code}]"
            ),
            Self::CoreError(msg) => write!(f, "Save state core mismatch: {msg}"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SlotMetaEntry {
    Empty,
    Ready {
        timestamp_secs: u64,
        rom_title: String,
        game_code: String,
        #[allow(dead_code)]
        rom_crc32: u32,
        error_note: Option<String>,
    },
    Corrupt(String),
}

pub(crate) fn encode_thumbnail_png(rgba: &[u8]) -> Result<Vec<u8>, String> {
    let expected = LCD_WIDTH * LCD_HEIGHT * 4;
    if rgba.len() != expected {
        return Err(format!(
            "Invalid thumbnail RGBA length: got {}, expected {expected}",
            rgba.len()
        ));
    }
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder
        .write_image(
            rgba,
            LCD_WIDTH as u32,
            LCD_HEIGHT as u32,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG thumbnail encode failed: {e}"))?;
    Ok(out)
}

pub(crate) fn encode_container(
    rom_crc32: u32,
    game_code: &str,
    rom_title: &str,
    timestamp_secs: u64,
    thumbnail_png: &[u8],
    core_bytes: &[u8],
) -> Result<Vec<u8>, SaveStateError> {
    let title_bytes = rom_title.as_bytes();
    if title_bytes.len() > MAX_TITLE_LEN {
        return Err(SaveStateError::Corrupt("ROM title exceeds limit".into()));
    }
    if thumbnail_png.len() > MAX_THUMB_LEN {
        return Err(SaveStateError::Corrupt("Thumbnail exceeds limit".into()));
    }
    if core_bytes.len() > MAX_CORE_LEN {
        return Err(SaveStateError::Corrupt("Core state exceeds limit".into()));
    }

    let compressed = lz4_flex::block::compress(core_bytes);
    let (flags, payload): (u16, &[u8]) = if compressed.len() < core_bytes.len() {
        (FLAG_CORE_LZ4, &compressed)
    } else {
        (0, core_bytes)
    };

    let mut code_raw = [0u8; 4];
    for (dst, &src) in code_raw.iter_mut().zip(game_code.as_bytes().iter()) {
        *dst = src;
    }

    let total_len = HEADER_LEN + title_bytes.len() + thumbnail_png.len() + payload.len() + 4;
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(&RBAS_MAGIC);
    buf.extend_from_slice(&RBAS_VERSION.to_le_bytes());
    buf.extend_from_slice(&flags.to_le_bytes());
    buf.extend_from_slice(&timestamp_secs.to_le_bytes());
    buf.extend_from_slice(&rom_crc32.to_le_bytes());
    buf.extend_from_slice(&code_raw);
    buf.extend_from_slice(&(title_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(&(thumbnail_png.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(core_bytes.len() as u32).to_le_bytes());

    buf.extend_from_slice(title_bytes);
    buf.extend_from_slice(thumbnail_png);
    buf.extend_from_slice(payload);

    let crc = crc32fast::hash(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    Ok(buf)
}

struct ParsedHeader<'a> {
    meta: SaveStateMeta,
    core_payload: &'a [u8],
    core_uncompressed_len: usize,
}

fn parse_and_verify_container(data: &[u8]) -> Result<ParsedHeader<'_>, SaveStateError> {
    if data.len() < HEADER_LEN + 4 {
        return Err(SaveStateError::Corrupt(
            "File is too short to be a valid RBAS save state".into(),
        ));
    }
    if data[0..4] != RBAS_MAGIC {
        return Err(SaveStateError::Corrupt(
            "Missing RBAS save state signature".into(),
        ));
    }

    let expected_crc = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap());
    let actual_crc = crc32fast::hash(&data[..data.len() - 4]);
    if expected_crc != actual_crc {
        return Err(SaveStateError::Corrupt(format!(
            "Checksum mismatch (stored {expected_crc:08X}, computed {actual_crc:08X})"
        )));
    }

    let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
    if version != RBAS_VERSION {
        return Err(SaveStateError::VersionMismatch {
            found: version,
            expected: RBAS_VERSION,
        });
    }

    let flags = u16::from_le_bytes(data[6..8].try_into().unwrap());
    let timestamp_secs = u64::from_le_bytes(data[8..16].try_into().unwrap());
    let rom_crc32 = u32::from_le_bytes(data[16..20].try_into().unwrap());
    let game_code = String::from_utf8_lossy(&data[20..24])
        .trim_end_matches(['\0', ' '])
        .to_string();

    let title_len = u16::from_le_bytes(data[24..26].try_into().unwrap()) as usize;
    let thumb_len = u32::from_le_bytes(data[26..30].try_into().unwrap()) as usize;
    let core_payload_len = u32::from_le_bytes(data[30..34].try_into().unwrap()) as usize;
    let core_uncompressed_len = u32::from_le_bytes(data[34..38].try_into().unwrap()) as usize;

    if title_len > MAX_TITLE_LEN
        || thumb_len > MAX_THUMB_LEN
        || core_payload_len > MAX_CORE_LEN
        || core_uncompressed_len > MAX_CORE_LEN
    {
        return Err(SaveStateError::Corrupt(
            "Section lengths exceed valid bounds".into(),
        ));
    }

    let expected_total = HEADER_LEN
        .checked_add(title_len)
        .and_then(|v| v.checked_add(thumb_len))
        .and_then(|v| v.checked_add(core_payload_len))
        .and_then(|v| v.checked_add(4));
    if expected_total != Some(data.len()) {
        return Err(SaveStateError::Corrupt(
            "Section sizes do not match container length".into(),
        ));
    }

    let mut pos = HEADER_LEN;
    let rom_title = std::str::from_utf8(&data[pos..pos + title_len])
        .map_err(|_| SaveStateError::Corrupt("Invalid UTF-8 ROM title".into()))?
        .to_string();
    pos += title_len;

    let thumbnail_png = data[pos..pos + thumb_len].to_vec();
    pos += thumb_len;

    let core_payload = &data[pos..pos + core_payload_len];

    Ok(ParsedHeader {
        meta: SaveStateMeta {
            version,
            flags,
            timestamp_secs,
            rom_crc32,
            game_code,
            rom_title,
            thumbnail_png,
        },
        core_payload,
        core_uncompressed_len,
    })
}

pub(crate) fn decode_container_meta(data: &[u8]) -> Result<SaveStateMeta, SaveStateError> {
    parse_and_verify_container(data).map(|p| p.meta)
}

pub(crate) fn decode_container(
    data: &[u8],
    expected_rom_crc32: Option<u32>,
    expected_game_code: Option<&str>,
    expected_title: &str,
) -> Result<DecodedSaveState, SaveStateError> {
    let parsed = parse_and_verify_container(data)?;

    let crc_mismatch = expected_rom_crc32
        .is_some_and(|exp| exp != 0 && parsed.meta.rom_crc32 != 0 && parsed.meta.rom_crc32 != exp);
    let code_mismatch = expected_game_code.is_some_and(|exp| {
        !exp.is_empty() && !parsed.meta.game_code.is_empty() && parsed.meta.game_code != exp
    });

    if crc_mismatch || code_mismatch {
        return Err(SaveStateError::WrongGame {
            expected_title: expected_title.to_string(),
            expected_code: expected_game_code.unwrap_or("").to_string(),
            found_title: parsed.meta.rom_title,
            found_code: parsed.meta.game_code,
        });
    }

    let core_bytes = if (parsed.meta.flags & FLAG_CORE_LZ4) != 0 {
        let decompressed =
            lz4_flex::block::decompress(parsed.core_payload, parsed.core_uncompressed_len)
                .map_err(|e| {
                    SaveStateError::Corrupt(format!("LZ4 payload decompress error: {e}"))
                })?;
        if decompressed.len() != parsed.core_uncompressed_len {
            return Err(SaveStateError::Corrupt(
                "Decompressed core length mismatch".into(),
            ));
        }
        decompressed
    } else {
        if parsed.core_payload.len() != parsed.core_uncompressed_len {
            return Err(SaveStateError::Corrupt(
                "Uncompressed core length mismatch".into(),
            ));
        }
        parsed.core_payload.to_vec()
    };

    Ok(DecodedSaveState {
        meta: parsed.meta,
        core_bytes,
    })
}

fn unix_to_utc_date_hm(ts_secs: u64) -> String {
    if ts_secs == 0 {
        return "Unknown time".to_string();
    }
    let days = (ts_secs / 86_400) as i64;
    let rem = ts_secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;

    // Howard Hinnant civil-from-days algorithm in integer arithmetic
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02} {hour:02}:{min:02} UTC")
}

fn format_saved_timestamp(ts_secs: u64) -> (String, String) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let rel = if ts_secs == 0 || ts_secs > now {
        "Saved".to_string()
    } else {
        let diff = now - ts_secs;
        if diff < 60 {
            "Just now".to_string()
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86_400 {
            format!("{}h ago", diff / 3600)
        } else {
            format!("{}d ago", diff / 86_400)
        }
    };
    (rel, unix_to_utc_date_hm(ts_secs))
}

impl RustboyApp {
    pub(crate) fn slot_path(&self, slot: usize) -> Option<PathBuf> {
        let rom_path = self.rom_path.as_ref()?;
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(rom_path.with_file_name(format!("{stem}.slot{slot}.state")))
    }

    pub(crate) fn slot_thumb_path(&self, slot: usize) -> Option<PathBuf> {
        let rom_path = self.rom_path.as_ref()?;
        let stem = rom_path.file_stem()?.to_string_lossy();
        Some(rom_path.with_file_name(format!("{stem}.slot{slot}.png")))
    }

    fn current_thumbnail_png(&mut self) -> Vec<u8> {
        if self.sink.frame_dirty && self.lcd_tex.is_none() {
            let color_profile = self.effective_color_profile();
            self.postproc.process_frame(
                &self.sink.framebuffer,
                self.config.color_correction,
                color_profile,
                false,
                false,
            );
        }
        encode_thumbnail_png(&self.postproc.curr_rgba).unwrap_or_default()
    }

    fn build_current_state_container(&mut self) -> Result<Vec<u8>, SaveStateError> {
        let thumb_png = self.current_thumbnail_png();
        let core_bytes = self
            .save_active_core_state()
            .map_err(SaveStateError::CoreError)?;
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        encode_container(
            self.rom_crc32,
            &self.rom_code,
            &self.rom_title,
            ts,
            &thumb_png,
            &core_bytes,
        )
    }

    pub(crate) fn quick_save(&mut self, slot: usize) {
        let Some(path) = self.slot_path(slot) else {
            return;
        };
        if !self.has_active_game() {
            return;
        }

        // Backup existing slot state (decoded core bytes) for Ctrl+Z undo
        if let Ok(old_bytes) = fs::read(&path)
            && let Ok(decoded) = decode_container(
                &old_bytes,
                Some(self.rom_crc32),
                Some(&self.rom_code),
                &self.rom_title,
            )
        {
            self.undo_state_buffer = Some(decoded.core_bytes);
        }

        let res = self
            .build_current_state_container()
            .and_then(|container| fs::write(&path, container).map_err(|e| SaveStateError::Io(e.to_string())));

        match res {
            Ok(()) => {
                if (1..=5).contains(&slot) {
                    self.slot_thumbnails[slot - 1] = None;
                    self.slot_meta[slot - 1] = None;
                }
                self.movie.record_state_bookmark(&path);
                self.state_manager_error = None;
                self.notify(&format!("Saved State Slot {slot}"));
            }
            Err(e) => {
                let msg = format!("Save State failed: {e}");
                self.state_manager_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    pub(crate) fn quick_load(&mut self, slot: usize) {
        let Some(path) = self.slot_path(slot) else {
            return;
        };
        // Backup current core state before loading for instant undo
        if let Ok(cur_bytes) = self.save_active_core_state() {
            self.undo_state_buffer = Some(cur_bytes);
            self.movie.record_undo_bookmark();
        }
        self.load_state_from_path(&path, &format!("Loaded State Slot {slot}"));
    }

    pub(crate) fn undo_last_state_action(&mut self) {
        if !self.has_active_game() {
            self.notify("No save/load action to undo");
            return;
        }
        let Some(prev_bytes) = self.undo_state_buffer.take() else {
            self.notify("No save/load action to undo");
            return;
        };
        let cur = self.save_active_core_state().ok();
        match self.load_active_core_state(&prev_bytes) {
            Ok(()) => {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
                self.rewinding = false;
                self.undo_state_buffer = cur;
                self.state_manager_error = None;
                if let Some(core) = &self.core {
                    self.movie.on_undo_state_loaded(core);
                }
                if let Some(msg) = self.movie.take_notice() {
                    self.notify(&msg);
                } else {
                    self.notify("Undo: Reverted state to previous checkpoint");
                }
            }
            Err(e) => {
                let msg = format!("Undo state load failed: {e}");
                self.state_manager_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    pub(crate) fn save_state_to_file(&mut self) {
        if !self.has_active_game() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save State File")
            .add_filter("Rustboy Save State", &["state", "gbas"])
            .set_file_name(format!("{}.state", self.rom_title))
            .save_file()
        {
            match self.build_current_state_container() {
                Ok(container) => match fs::write(&path, container) {
                    Ok(()) => {
                        self.movie.record_state_bookmark(&path);
                        self.notify("Saved State to file");
                    }
                    Err(e) => self.notify_error(&format!("Failed writing state: {e}")),
                },
                Err(e) => self.notify_error(&format!("State serialize failed: {e}")),
            }
        }
    }

    pub(crate) fn load_state_from_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Load Save State")
            .add_filter("Rustboy Save State", &["state", "gbas"])
            .pick_file()
        {
            self.load_state_from_path(&path, "Loaded State from file");
        }
    }

    pub(crate) fn load_state_from_path(&mut self, path: &Path, success_msg: &str) {
        if !self.has_active_game() {
            return;
        }
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                let msg = SaveStateError::Io(e.to_string()).to_string();
                self.state_manager_error = Some(msg.clone());
                self.notify_error(&msg);
                return;
            }
        };

        let decoded = match decode_container(
            &bytes,
            Some(self.rom_crc32),
            Some(&self.rom_code),
            &self.rom_title,
        ) {
            Ok(d) => d,
            Err(e) => {
                let msg = e.to_string();
                self.state_manager_error = Some(msg.clone());
                self.notify_error(&msg);
                return;
            }
        };

        match self.load_active_core_state(&decoded.core_bytes) {
            Ok(()) => {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
                self.rewinding = false;
                self.show_pause_overlay = false;
                self.state_manager_error = None;
                if let Some(core) = &self.core {
                    self.movie.on_state_loaded_from_path(core, path);
                }
                if let Some(msg) = self.movie.take_notice() {
                    self.notify(&format!("{success_msg} — {msg}"));
                } else {
                    self.notify(success_msg);
                }
            }
            Err(e) => {
                let msg = SaveStateError::CoreError(e).to_string();
                self.state_manager_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    fn ensure_slot_cached(&mut self, ctx: &egui::Context, slot: usize) {
        let idx = slot - 1;
        if self.slot_meta[idx].is_some() {
            return;
        }
        let Some(state_file) = self.slot_path(slot) else {
            self.slot_meta[idx] = Some(SlotMetaEntry::Empty);
            return;
        };
        if !state_file.exists() {
            self.slot_thumbnails[idx] = None;
            self.slot_meta[idx] = Some(SlotMetaEntry::Empty);
            return;
        }

        let bytes = match fs::read(&state_file) {
            Ok(b) => b,
            Err(e) => {
                self.slot_meta[idx] = Some(SlotMetaEntry::Corrupt(format!("Read error: {e}")));
                return;
            }
        };

        match decode_container_meta(&bytes) {
            Ok(meta) => {
                if !meta.thumbnail_png.is_empty()
                    && let Ok(img) =
                        image::load_from_memory_with_format(&meta.thumbnail_png, image::ImageFormat::Png)
                {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let cimg =
                        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba.into_raw());
                    self.slot_thumbnails[idx] = Some(ctx.load_texture(
                        format!("slot_thumb_{slot}"),
                        cimg,
                        TextureOptions::LINEAR,
                    ));
                } else {
                    self.slot_thumbnails[idx] = None;
                }

                let wrong_crc = self.rom_crc32 != 0
                    && meta.rom_crc32 != 0
                    && meta.rom_crc32 != self.rom_crc32;
                let wrong_code = !self.rom_code.is_empty()
                    && !meta.game_code.is_empty()
                    && meta.game_code != self.rom_code;
                let error_note = if wrong_crc || wrong_code {
                    Some(format!(
                        "Wrong game: {} [{}]",
                        meta.rom_title, meta.game_code
                    ))
                } else {
                    None
                };

                self.slot_meta[idx] = Some(SlotMetaEntry::Ready {
                    timestamp_secs: meta.timestamp_secs,
                    rom_title: meta.rom_title,
                    game_code: meta.game_code,
                    rom_crc32: meta.rom_crc32,
                    error_note,
                });
            }
            Err(e) => {
                self.slot_thumbnails[idx] = None;
                self.slot_meta[idx] = Some(SlotMetaEntry::Corrupt(e.to_string()));
            }
        }
    }

    pub(crate) fn draw_visual_state_manager(&mut self, ctx: &egui::Context) {
        if !self.show_state_manager {
            return;
        }
        let mut open = self.show_state_manager;
        let mut do_undo = false;
        let mut save_slot: Option<usize> = None;
        let mut load_slot: Option<usize> = None;
        let mut delete_slot: Option<usize> = None;

        for slot in 1..=5 {
            self.ensure_slot_cached(ctx, slot);
        }

        egui::Window::new("Visual Save State Manager")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(BG_CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(14.0),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Active ROM: {} [{}]", self.rom_title, self.rom_code))
                            .color(TEXT_MUTED)
                            .small(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.undo_state_buffer.is_some(),
                                egui::Button::new("Undo State Action (Ctrl+Z)"),
                            )
                            .clicked()
                        {
                            do_undo = true;
                        }
                    });
                });

                if let Some(err) = &self.state_manager_error {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(56, 24, 28))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(170, 64, 68)))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(err)
                                    .color(Color32::from_rgb(255, 165, 165))
                                    .small(),
                            );
                        });
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    for slot in 1..=5 {
                        let idx = slot - 1;
                        let meta_entry = self.slot_meta[idx].clone();
                        let (is_filled, has_error, status_rel, status_abs, tip_extra) =
                            match &meta_entry {
                                Some(SlotMetaEntry::Ready {
                                    timestamp_secs,
                                    rom_title,
                                    game_code,
                                    error_note,
                                    ..
                                }) => {
                                    let (rel, abs) = format_saved_timestamp(*timestamp_secs);
                                    let tip = if let Some(err) = error_note {
                                        err.clone()
                                    } else {
                                        format!("{rom_title} [{game_code}] · {abs}")
                                    };
                                    (true, error_note.is_some(), rel, abs, Some(tip))
                                }
                                Some(SlotMetaEntry::Corrupt(reason)) => (
                                    true,
                                    true,
                                    "Corrupt state".to_string(),
                                    String::new(),
                                    Some(reason.clone()),
                                ),
                                _ => (false, false, "Empty slot".to_string(), String::new(), None),
                            };

                        egui::Frame::new()
                            .fill(BG_PANEL)
                            .stroke(Stroke::new(
                                1.0_f32,
                                if has_error {
                                    Color32::from_rgb(165, 65, 68)
                                } else if is_filled {
                                    BORDER_HAIRLINE
                                } else {
                                    Color32::from_rgb(33, 38, 49)
                                },
                            ))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(136.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(format!("Slot {slot}"))
                                            .strong()
                                            .color(if has_error {
                                                Color32::from_rgb(255, 130, 130)
                                            } else if is_filled {
                                                ACCENT_TEAL
                                            } else {
                                                TEXT_MUTED
                                            }),
                                    );

                                    let thumb_size = Vec2::new(136.0, 90.0);
                                    let (rect, resp) =
                                        ui.allocate_exact_size(thumb_size, Sense::hover());
                                    if let Some(tex) = &self.slot_thumbnails[idx] {
                                        ui.painter().image(
                                            tex.id(),
                                            rect,
                                            egui::Rect::from_min_max(
                                                egui::Pos2::ZERO,
                                                egui::Pos2::new(1.0, 1.0),
                                            ),
                                            Color32::WHITE,
                                        );
                                        ui.painter().rect_stroke(
                                            rect,
                                            CornerRadius::same(4),
                                            Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                            egui::StrokeKind::Outside,
                                        );
                                    } else {
                                        ui.painter().rect_filled(
                                            rect,
                                            CornerRadius::same(4),
                                            BG_APP_CARD_BG,
                                        );
                                        ui.painter().rect_stroke(
                                            rect,
                                            CornerRadius::same(4),
                                            Stroke::new(1.0_f32, BORDER_HAIRLINE),
                                            egui::StrokeKind::Outside,
                                        );
                                        let placeholder = if has_error {
                                            "Incompatible"
                                        } else if is_filled {
                                            "No Preview"
                                        } else {
                                            "No Save"
                                        };
                                        ui.painter().text(
                                            rect.center(),
                                            Align2::CENTER_CENTER,
                                            placeholder,
                                            egui::FontId::proportional(11.5),
                                            TEXT_MUTED,
                                        );
                                    }
                                    if let Some(tip) = &tip_extra {
                                        resp.on_hover_text(tip);
                                    }

                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(status_rel)
                                            .small()
                                            .color(if has_error {
                                                Color32::from_rgb(255, 145, 145)
                                            } else if is_filled {
                                                Color32::from_rgb(210, 220, 232)
                                            } else {
                                                TEXT_MUTED
                                            }),
                                    );
                                    if !status_abs.is_empty() {
                                        ui.label(
                                            RichText::new(status_abs)
                                                .size(10.0)
                                                .color(TEXT_MUTED),
                                        );
                                    } else {
                                        ui.label(RichText::new(" ").size(10.0));
                                    }

                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_sized(
                                                [64.0, 22.0],
                                                egui::Button::new(RichText::new("Save").small()),
                                            )
                                            .on_hover_text(format!(
                                                "Quick Save to Slot {slot} (F{slot})"
                                            ))
                                            .clicked()
                                        {
                                            save_slot = Some(slot);
                                        }
                                        if ui
                                            .add_enabled(
                                                is_filled,
                                                egui::Button::new(RichText::new("Load").small())
                                                    .min_size(Vec2::new(64.0, 22.0)),
                                            )
                                            .on_hover_text(format!(
                                                "Quick Load from Slot {slot} (Shift+F{slot})"
                                            ))
                                            .clicked()
                                        {
                                            load_slot = Some(slot);
                                        }
                                    });

                                    ui.add_space(2.0);
                                    if ui
                                        .add_enabled(
                                            is_filled,
                                            egui::Button::new(
                                                RichText::new("Delete").small().color(if is_filled {
                                                    Color32::from_rgb(245, 120, 120)
                                                } else {
                                                    TEXT_MUTED
                                                }),
                                            )
                                            .min_size(Vec2::new(136.0, 20.0)),
                                        )
                                        .clicked()
                                    {
                                        delete_slot = Some(slot);
                                    }
                                });
                            });
                        if slot < 5 {
                            ui.add_space(4.0);
                        }
                    }
                });
            });

        self.show_state_manager = open;
        if do_undo {
            self.undo_last_state_action();
        }
        if let Some(s) = save_slot {
            self.quick_save(s);
        }
        if let Some(s) = load_slot {
            self.quick_load(s);
        }
        if let Some(s) = delete_slot {
            if let Some(sp) = self.slot_path(s) {
                let _ = fs::remove_file(sp);
            }
            if let Some(tp) = self.slot_thumb_path(s) {
                let _ = fs::remove_file(tp);
            }
            self.slot_thumbnails[s - 1] = None;
            self.slot_meta[s - 1] = Some(SlotMetaEntry::Empty);
            self.state_manager_error = None;
            self.notify(&format!("Deleted Slot {s}"));
        }
    }
}

const BG_APP_CARD_BG: Color32 = Color32::from_rgb(16, 19, 25);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rbas_container_roundtrips_metadata_thumbnail_and_core_bytes() {
        let mut rgba = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px[0] = (i % 256) as u8;
            px[1] = 128;
            px[2] = 64;
            px[3] = 255;
        }
        let thumb = encode_thumbnail_png(&rgba).unwrap();
        let core_bytes: Vec<u8> = (0..4096).map(|i| (i * 7 % 251) as u8).collect();

        let packed = encode_container(
            0xDEAD_BEEF,
            "BPEE",
            "POKEMON EMER",
            1_700_000_123,
            &thumb,
            &core_bytes,
        )
        .unwrap();

        let meta = decode_container_meta(&packed).unwrap();
        assert_eq!(meta.version, RBAS_VERSION);
        assert_eq!(meta.rom_crc32, 0xDEAD_BEEF);
        assert_eq!(meta.game_code, "BPEE");
        assert_eq!(meta.rom_title, "POKEMON EMER");
        assert_eq!(meta.timestamp_secs, 1_700_000_123);
        assert_eq!(meta.thumbnail_png, thumb);

        let decoded =
            decode_container(&packed, Some(0xDEAD_BEEF), Some("BPEE"), "POKEMON EMER").unwrap();
        assert_eq!(decoded.core_bytes, core_bytes);
    }

    #[test]
    fn rbas_container_rejects_wrong_game_version_mismatch_and_corruption() {
        let packed = encode_container(
            0x1122_3344,
            "AXVE",
            "POKEMON RUBY",
            1_700_000_000,
            b"",
            b"CORE_STATE_BYTES_0123456789",
        )
        .unwrap();

        // Wrong ROM CRC32 / code
        let wrong = decode_container(&packed, Some(0x9988_7766), Some("BPEE"), "POKEMON EMER");
        assert!(matches!(wrong, Err(SaveStateError::WrongGame { .. })));

        // Version mismatch (re-sign CRC so only version differs)
        let mut wrong_ver = packed.clone();
        wrong_ver[4..6].copy_from_slice(&99u16.to_le_bytes());
        let len_wo_crc = wrong_ver.len() - 4;
        let new_crc = crc32fast::hash(&wrong_ver[..len_wo_crc]);
        wrong_ver[len_wo_crc..].copy_from_slice(&new_crc.to_le_bytes());
        assert!(matches!(
            decode_container(&wrong_ver, None, None, ""),
            Err(SaveStateError::VersionMismatch {
                found: 99,
                expected: RBAS_VERSION
            })
        ));

        // Payload corruption
        let mut corrupt = packed.clone();
        let mid = corrupt.len() / 2;
        corrupt[mid] ^= 0x55;
        assert!(matches!(
            decode_container(&corrupt, None, None, ""),
            Err(SaveStateError::Corrupt(_))
        ));

        // Bad magic
        let mut bad_magic = packed;
        bad_magic[0..4].copy_from_slice(b"NOPE");
        assert!(matches!(
            decode_container(&bad_magic, None, None, ""),
            Err(SaveStateError::Corrupt(_))
        ));
    }
}
