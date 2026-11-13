use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use egui::{self, Color32, CornerRadius, RichText, Stroke};
use gba_core::{CYCLES_PER_FRAME, GbaCore};

use crate::app::{
    ACCENT_TEAL, ACCENT_TEAL_BG, BG_CARD, BG_PANEL, BORDER_HAIRLINE, RustboyApp, TEXT_MUTED,
};

pub(crate) const RBMV_MAGIC: [u8; 4] = *b"RBMV";
pub(crate) const RBMV_VERSION: u16 = 1;
const HEADER_LEN: usize = 23;
const MAX_INITIAL_STATE_LEN: usize = 16 * 1024 * 1024;
const MAX_MOVIE_FRAMES: usize = 100_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MovieError {
    Io(String),
    Corrupt(String),
    VersionMismatch {
        found: u16,
        expected: u16,
    },
    RomMismatch {
        expected_crc32: u32,
        found_crc32: u32,
    },
    CoreError(String),
    InvalidState(String),
}

impl fmt::Display for MovieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "Movie I/O error: {msg}"),
            Self::Corrupt(msg) => write!(f, "Corrupt .rbm movie: {msg}"),
            Self::VersionMismatch { found, expected } => write!(
                f,
                "Movie format version mismatch (file v{found}, expected v{expected})"
            ),
            Self::RomMismatch {
                expected_crc32,
                found_crc32,
            } => write!(
                f,
                "ROM CRC32 mismatch: movie expects {found_crc32:#010X}, active ROM is {expected_crc32:#010X}"
            ),
            Self::CoreError(msg) => write!(f, "Failed applying movie initial state: {msg}"),
            Self::InvalidState(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for MovieError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RbmMovie {
    pub(crate) rom_crc32: u32,
    pub(crate) bios_present: bool,
    pub(crate) rerecord_count: u32,
    pub(crate) initial_state: Vec<u8>,
    pub(crate) frames: Vec<u16>,
}

impl RbmMovie {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, MovieError> {
        if self.initial_state.len() > MAX_INITIAL_STATE_LEN {
            return Err(MovieError::Corrupt(
                "Embedded initial save state exceeds size limit".into(),
            ));
        }
        if !self.initial_state.is_empty() && !self.initial_state.starts_with(b"RBAS") {
            return Err(MovieError::Corrupt(
                "Embedded initial save state must start with RBAS header".into(),
            ));
        }
        if self.frames.len() > MAX_MOVIE_FRAMES {
            return Err(MovieError::Corrupt(
                "Movie frame count exceeds limit".into(),
            ));
        }

        let frame_count = u32::try_from(self.frames.len())
            .map_err(|_| MovieError::Corrupt("Movie frame count overflows u32".into()))?;
        let init_len = u32::try_from(self.initial_state.len())
            .map_err(|_| MovieError::Corrupt("Initial state length overflows u32".into()))?;

        let mut raw_inputs = Vec::with_capacity(self.frames.len() * 2);
        for &mask in &self.frames {
            raw_inputs.extend_from_slice(&(mask & 0x03FF).to_le_bytes());
        }
        let compressed_inputs = if raw_inputs.is_empty() {
            Vec::new()
        } else {
            lz4_flex::block::compress(&raw_inputs)
        };
        let comp_len = u32::try_from(compressed_inputs.len())
            .map_err(|_| MovieError::Corrupt("Compressed movie payload overflows u32".into()))?;

        let total_len = HEADER_LEN + self.initial_state.len() + 4 + compressed_inputs.len();
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(&RBMV_MAGIC);
        out.extend_from_slice(&RBMV_VERSION.to_le_bytes());
        out.extend_from_slice(&self.rom_crc32.to_le_bytes());
        out.push(u8::from(self.bios_present));
        out.extend_from_slice(&self.rerecord_count.to_le_bytes());
        out.extend_from_slice(&frame_count.to_le_bytes());
        out.extend_from_slice(&init_len.to_le_bytes());
        out.extend_from_slice(&self.initial_state);
        out.extend_from_slice(&comp_len.to_le_bytes());
        out.extend_from_slice(&compressed_inputs);
        Ok(out)
    }

    pub(crate) fn decode(data: &[u8]) -> Result<Self, MovieError> {
        if data.len() < HEADER_LEN + 4 {
            return Err(MovieError::Corrupt(
                "File is too short to be a valid .rbm movie".into(),
            ));
        }
        if data[0..4] != RBMV_MAGIC {
            return Err(MovieError::Corrupt("Missing RBMV signature".into()));
        }

        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != RBMV_VERSION {
            return Err(MovieError::VersionMismatch {
                found: version,
                expected: RBMV_VERSION,
            });
        }

        let rom_crc32 = u32::from_le_bytes([data[6], data[7], data[8], data[9]]);
        let bios_present = match data[10] {
            0 => false,
            1 => true,
            other => {
                return Err(MovieError::Corrupt(format!(
                    "Invalid bios_present byte: {other}"
                )));
            }
        };
        let rerecord_count = u32::from_le_bytes([data[11], data[12], data[13], data[14]]);
        let frame_count = u32::from_le_bytes([data[15], data[16], data[17], data[18]]) as usize;
        let init_len = u32::from_le_bytes([data[19], data[20], data[21], data[22]]) as usize;

        if frame_count > MAX_MOVIE_FRAMES {
            return Err(MovieError::Corrupt(
                "Movie frame count exceeds limit".into(),
            ));
        }
        if init_len > MAX_INITIAL_STATE_LEN {
            return Err(MovieError::Corrupt(
                "Embedded initial save state exceeds size limit".into(),
            ));
        }

        let comp_len_offset = HEADER_LEN
            .checked_add(init_len)
            .ok_or_else(|| MovieError::Corrupt("Header offset overflow".into()))?;
        let inputs_offset = comp_len_offset
            .checked_add(4)
            .ok_or_else(|| MovieError::Corrupt("Header offset overflow".into()))?;
        if inputs_offset > data.len() {
            return Err(MovieError::Corrupt(
                "Truncated embedded initial save state".into(),
            ));
        }

        let initial_state = data[HEADER_LEN..comp_len_offset].to_vec();
        if !initial_state.is_empty() && !initial_state.starts_with(b"RBAS") {
            return Err(MovieError::Corrupt(
                "Embedded initial save state missing RBAS header".into(),
            ));
        }

        let comp_len = u32::from_le_bytes([
            data[comp_len_offset],
            data[comp_len_offset + 1],
            data[comp_len_offset + 2],
            data[comp_len_offset + 3],
        ]) as usize;

        let expected_total = inputs_offset
            .checked_add(comp_len)
            .ok_or_else(|| MovieError::Corrupt("Compressed payload offset overflow".into()))?;
        if expected_total != data.len() {
            return Err(MovieError::Corrupt(
                "Movie payload length does not match file size".into(),
            ));
        }

        let compressed = &data[inputs_offset..expected_total];
        let frames = if frame_count == 0 {
            if !compressed.is_empty() {
                return Err(MovieError::Corrupt(
                    "Non-empty input stream for zero-frame movie".into(),
                ));
            }
            Vec::new()
        } else {
            let uncompressed_len = frame_count
                .checked_mul(2)
                .ok_or_else(|| MovieError::Corrupt("Uncompressed input size overflow".into()))?;
            let raw = lz4_flex::block::decompress(compressed, uncompressed_len)
                .map_err(|e| MovieError::Corrupt(format!("LZ4 input decompress failed: {e}")))?;
            if raw.len() != uncompressed_len {
                return Err(MovieError::Corrupt(
                    "Decompressed input length mismatch".into(),
                ));
            }
            raw.as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c) & 0x03FF)
                .collect()
        };

        Ok(Self {
            rom_crc32,
            bios_present,
            rerecord_count,
            initial_state,
            frames,
        })
    }

    pub(crate) fn save_to_file(&self, path: &Path) -> Result<(), MovieError> {
        let encoded = self.encode()?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| MovieError::Io(e.to_string()))?;
        }
        fs::write(path, encoded).map_err(|e| MovieError::Io(e.to_string()))
    }

    pub(crate) fn load_from_file(path: &Path) -> Result<Self, MovieError> {
        let bytes = fs::read(path).map_err(|e| MovieError::Io(e.to_string()))?;
        Self::decode(&bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MovieState {
    Idle,
    Recording {
        movie: RbmMovie,
        anchor_cycle: u64,
    },
    Playing {
        movie: RbmMovie,
        cursor: usize,
        read_only: bool,
        anchor_cycle: u64,
    },
}

pub(crate) struct MovieController {
    state: MovieState,
    read_only_pref: bool,
    pub(crate) active_path: Option<PathBuf>,
    last_movie: Option<RbmMovie>,
    state_frame_bookmarks: HashMap<PathBuf, usize>,
    undo_frame_bookmark: Option<usize>,
    pending_notice: Option<String>,
    pub(crate) last_error: Option<String>,
}

impl Default for MovieController {
    fn default() -> Self {
        Self::new()
    }
}

impl MovieController {
    pub(crate) fn new() -> Self {
        Self {
            state: MovieState::Idle,
            read_only_pref: true,
            active_path: None,
            last_movie: None,
            state_frame_bookmarks: HashMap::new(),
            undo_frame_bookmark: None,
            pending_notice: None,
            last_error: None,
        }
    }

    pub(crate) fn state(&self) -> &MovieState {
        &self.state
    }

    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.state, MovieState::Idle)
    }

    pub(crate) fn is_recording(&self) -> bool {
        matches!(self.state, MovieState::Recording { .. })
    }

    pub(crate) fn is_playing(&self) -> bool {
        matches!(self.state, MovieState::Playing { .. })
    }

    pub(crate) fn read_only(&self) -> bool {
        match &self.state {
            MovieState::Playing { read_only, .. } => *read_only,
            MovieState::Recording { .. } => false,
            MovieState::Idle => self.read_only_pref,
        }
    }

    pub(crate) fn current_frame(&self) -> usize {
        match &self.state {
            MovieState::Idle => 0,
            MovieState::Recording { movie, .. } => movie.frames.len(),
            MovieState::Playing { cursor, .. } => *cursor,
        }
    }

    pub(crate) fn total_frames(&self) -> usize {
        match &self.state {
            MovieState::Idle => self.last_movie.as_ref().map_or(0, |m| m.frames.len()),
            MovieState::Recording { movie, .. } | MovieState::Playing { movie, .. } => {
                movie.frames.len()
            }
        }
    }

    pub(crate) fn rerecord_count(&self) -> u32 {
        match &self.state {
            MovieState::Idle => self.last_movie.as_ref().map_or(0, |m| m.rerecord_count),
            MovieState::Recording { movie, .. } | MovieState::Playing { movie, .. } => {
                movie.rerecord_count
            }
        }
    }

    pub(crate) fn has_initial_state(&self) -> bool {
        match &self.state {
            MovieState::Idle => self
                .last_movie
                .as_ref()
                .is_some_and(|m| !m.initial_state.is_empty()),
            MovieState::Recording { movie, .. } | MovieState::Playing { movie, .. } => {
                !movie.initial_state.is_empty()
            }
        }
    }

    pub(crate) fn has_saveable_movie(&self) -> bool {
        !matches!(self.state, MovieState::Idle) || self.last_movie.is_some()
    }

    pub(crate) fn can_replay_last(&self) -> bool {
        matches!(self.state, MovieState::Idle) && self.last_movie.is_some()
    }

    pub(crate) fn peek_playback_keyinput(&self) -> Option<u16> {
        match &self.state {
            MovieState::Playing { movie, cursor, .. } => {
                movie.frames.get(*cursor).map(|&k| k & 0x03FF)
            }
            _ => None,
        }
    }

    pub(crate) fn start_recording(
        &mut self,
        core: &mut GbaCore,
        from_current_state: bool,
    ) -> Result<(), MovieError> {
        let initial_state = if from_current_state {
            core.save_state()
                .map_err(|e| MovieError::CoreError(format!("{e:?}")))?
        } else {
            core.reset();
            Vec::new()
        };

        let anchor_cycle = core.bus.scheduler.current_cycle;
        let movie = RbmMovie {
            rom_crc32: core.rom_crc32(),
            bios_present: core.bus.bios_present,
            rerecord_count: 0,
            initial_state,
            frames: Vec::new(),
        };

        self.state = MovieState::Recording {
            movie,
            anchor_cycle,
        };
        self.last_movie = None;
        self.active_path = None;
        self.state_frame_bookmarks.clear();
        self.undo_frame_bookmark = None;
        self.last_error = None;
        self.pending_notice = Some(if from_current_state {
            "TAS movie recording started from current state".into()
        } else {
            "TAS movie recording started from hard reset".into()
        });
        Ok(())
    }

    pub(crate) fn start_playback(
        &mut self,
        path: &Path,
        core: &mut GbaCore,
    ) -> Result<(), MovieError> {
        let movie = RbmMovie::load_from_file(path)?;
        self.start_playback_movie(movie, core)?;
        self.active_path = Some(path.to_path_buf());
        Ok(())
    }

    pub(crate) fn start_playback_movie(
        &mut self,
        movie: RbmMovie,
        core: &mut GbaCore,
    ) -> Result<(), MovieError> {
        let active_crc = core.rom_crc32();
        if movie.rom_crc32 != active_crc {
            let err = MovieError::RomMismatch {
                expected_crc32: active_crc,
                found_crc32: movie.rom_crc32,
            };
            self.last_error = Some(err.to_string());
            return Err(err);
        }

        if movie.initial_state.is_empty() {
            core.reset();
        } else {
            core.load_state(&movie.initial_state)
                .map_err(|e| MovieError::CoreError(format!("{e:?}")))?;
        }

        let anchor_cycle = core.bus.scheduler.current_cycle;
        let bios_mismatch = movie.bios_present != core.bus.bios_present;
        let total = movie.frames.len();
        let read_only = self.read_only_pref;

        self.state = MovieState::Playing {
            movie,
            cursor: 0,
            read_only,
            anchor_cycle,
        };
        self.last_movie = None;
        self.state_frame_bookmarks.clear();
        self.undo_frame_bookmark = None;
        self.last_error = None;
        self.pending_notice = Some(if bios_mismatch {
            format!(
                "Playing movie ({total} frames) — Warning: BIOS presence differs from recording"
            )
        } else {
            format!("Playing movie ({total} frames)")
        });
        Ok(())
    }

    pub(crate) fn replay_last(&mut self, core: &mut GbaCore) -> Result<(), MovieError> {
        let Some(movie) = self.last_movie.clone() else {
            return Err(MovieError::InvalidState(
                "No previous movie to replay".into(),
            ));
        };
        let path = self.active_path.clone();
        self.start_playback_movie(movie, core)?;
        self.active_path = path;
        Ok(())
    }

    pub(crate) fn step_frame(&mut self, live_keyinput: u16) -> u16 {
        let live = live_keyinput & 0x03FF;
        match &mut self.state {
            MovieState::Idle => live,
            MovieState::Recording { movie, .. } => {
                movie.frames.push(live);
                live
            }
            MovieState::Playing {
                movie,
                cursor,
                read_only,
                anchor_cycle,
            } => {
                if *cursor < movie.frames.len() {
                    let recorded = movie.frames[*cursor] & 0x03FF;
                    *cursor += 1;
                    if *cursor >= movie.frames.len() {
                        let total = movie.frames.len();
                        let ro = *read_only;
                        let anchor = *anchor_cycle;
                        let owned_movie = match std::mem::replace(&mut self.state, MovieState::Idle)
                        {
                            MovieState::Playing { movie, .. } => movie,
                            _ => unreachable!(),
                        };
                        if ro {
                            self.last_movie = Some(owned_movie);
                            self.pending_notice =
                                Some(format!("Movie playback finished ({total} frames)"));
                        } else {
                            self.state = MovieState::Recording {
                                movie: owned_movie,
                                anchor_cycle: anchor,
                            };
                            self.pending_notice =
                                Some("Reached end of movie — switched to Recording (R+W)".into());
                        }
                    }
                    recorded
                } else {
                    let total = movie.frames.len();
                    let ro = *read_only;
                    let anchor = *anchor_cycle;
                    let owned_movie = match std::mem::replace(&mut self.state, MovieState::Idle) {
                        MovieState::Playing { movie, .. } => movie,
                        _ => unreachable!(),
                    };
                    if ro {
                        self.last_movie = Some(owned_movie);
                        self.pending_notice =
                            Some(format!("Movie playback finished ({total} frames)"));
                        live
                    } else {
                        let mut rec_movie = owned_movie;
                        rec_movie.frames.push(live);
                        self.state = MovieState::Recording {
                            movie: rec_movie,
                            anchor_cycle: anchor,
                        };
                        self.pending_notice =
                            Some("Reached end of movie — switched to Recording (R+W)".into());
                        live
                    }
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn on_state_loaded(&mut self, core: &GbaCore) {
        self.on_state_loaded_at(core, None);
    }

    pub(crate) fn on_state_loaded_at(&mut self, core: &GbaCore, frame_hint: Option<usize>) {
        let estimate_frame = |anchor_cycle: u64, max_len: usize| -> usize {
            let raw = frame_hint.unwrap_or_else(|| {
                let delta = core
                    .bus
                    .scheduler
                    .current_cycle
                    .saturating_sub(anchor_cycle);
                (delta / CYCLES_PER_FRAME) as usize
            });
            raw.min(max_len)
        };

        match &mut self.state {
            MovieState::Idle => {}
            MovieState::Recording {
                movie,
                anchor_cycle,
            } => {
                let target = estimate_frame(*anchor_cycle, movie.frames.len());
                movie.frames.truncate(target);
                movie.rerecord_count = movie.rerecord_count.saturating_add(1);
            }
            MovieState::Playing {
                movie,
                cursor,
                read_only,
                anchor_cycle,
            } => {
                let target = estimate_frame(*anchor_cycle, movie.frames.len());
                if *read_only {
                    *cursor = target;
                } else {
                    let anchor = *anchor_cycle;
                    let mut owned_movie = match std::mem::replace(&mut self.state, MovieState::Idle)
                    {
                        MovieState::Playing { movie, .. } => movie,
                        _ => unreachable!(),
                    };
                    owned_movie.frames.truncate(target);
                    owned_movie.rerecord_count = owned_movie.rerecord_count.saturating_add(1);
                    self.state = MovieState::Recording {
                        movie: owned_movie,
                        anchor_cycle: anchor,
                    };
                    self.pending_notice = Some(format!(
                        "Rerecorded from frame {target} — Recording resumed"
                    ));
                }
            }
        }
    }

    pub(crate) fn record_state_bookmark(&mut self, path: &Path) {
        if self.is_active() {
            self.state_frame_bookmarks
                .insert(path.to_path_buf(), self.current_frame());
        }
    }

    pub(crate) fn record_undo_bookmark(&mut self) {
        self.undo_frame_bookmark = if self.is_active() {
            Some(self.current_frame())
        } else {
            None
        };
    }

    pub(crate) fn on_state_loaded_from_path(&mut self, core: &GbaCore, path: &Path) {
        let hint = self.state_frame_bookmarks.get(path).copied();
        self.on_state_loaded_at(core, hint);
    }

    pub(crate) fn on_undo_state_loaded(&mut self, core: &GbaCore) {
        let hint = self.undo_frame_bookmark.take();
        self.on_state_loaded_at(core, hint);
    }

    pub(crate) fn save_to_path(&mut self, path: &Path) -> Result<(), MovieError> {
        let movie = match &self.state {
            MovieState::Recording { movie, .. } | MovieState::Playing { movie, .. } => movie,
            MovieState::Idle => self.last_movie.as_ref().ok_or_else(|| {
                MovieError::InvalidState("No active or completed movie to save".into())
            })?,
        };
        movie.save_to_file(path)?;
        self.active_path = Some(path.to_path_buf());
        self.last_error = None;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn stop_and_save(&mut self, path: &Path) -> Result<(), MovieError> {
        self.save_to_path(path)?;
        self.stop();
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        let prev = std::mem::replace(&mut self.state, MovieState::Idle);
        match prev {
            MovieState::Idle => {}
            MovieState::Recording { movie, .. } => {
                let n = movie.frames.len();
                self.last_movie = Some(movie);
                self.pending_notice = Some(format!("Stopped movie recording ({n} frames)"));
            }
            MovieState::Playing { movie, .. } => {
                self.last_movie = Some(movie);
                self.pending_notice = Some("Stopped movie playback".into());
            }
        }
        self.state_frame_bookmarks.clear();
        self.undo_frame_bookmark = None;
    }

    pub(crate) fn toggle_read_only(&mut self) {
        let next = !self.read_only_pref;
        self.set_read_only(next);
    }

    pub(crate) fn set_read_only(&mut self, read_only: bool) {
        self.read_only_pref = read_only;
        if let MovieState::Playing {
            read_only: active_ro,
            ..
        } = &mut self.state
        {
            *active_ro = read_only;
        }
    }

    pub(crate) fn reset_for_rom_change(&mut self) {
        self.state = MovieState::Idle;
        self.last_movie = None;
        self.active_path = None;
        self.state_frame_bookmarks.clear();
        self.undo_frame_bookmark = None;
        self.pending_notice = None;
        self.last_error = None;
    }

    pub(crate) fn take_notice(&mut self) -> Option<String> {
        self.pending_notice.take()
    }
}

impl RustboyApp {
    pub(crate) fn draw_movies_panel(&mut self, ctx: &egui::Context) {
        if !self.show_movies {
            return;
        }

        let mut open = self.show_movies;
        let has_rom = self.core.is_some();

        let mut action_start_reset = false;
        let mut action_start_current = false;
        let mut action_open = false;
        let mut action_save = false;
        let mut action_stop = false;
        let mut action_replay = false;
        let mut dismiss_error = false;

        egui::Window::new("Input Movies (TAS)")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(430.0)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(BG_CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(14.0),
            )
            .show(ctx, |ui| {
                ui.set_min_width(400.0);

                if !has_rom {
                    egui::Frame::new()
                        .fill(BG_PANEL)
                        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new("No ROM Loaded").strong().color(Color32::WHITE));
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(
                                    "Open a GBA ROM first to record or play back .rbm TAS input movies.",
                                )
                                .small()
                                .color(TEXT_MUTED),
                            );
                        });
                    return;
                }

                if let Some(err) = &self.movie.last_error {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(56, 24, 28))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(170, 64, 68)))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(err)
                                        .small()
                                        .color(Color32::from_rgb(255, 165, 165)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("×").clicked() {
                                            dismiss_error = true;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(8.0);
                }

                // Status telemetry card
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        let (state_lbl, dot_color) = match self.movie.state() {
                            MovieState::Idle => ("IDLE", TEXT_MUTED),
                            MovieState::Recording { .. } => {
                                ("RECORDING", Color32::from_rgb(235, 85, 85))
                            }
                            MovieState::Playing { read_only, .. } => (
                                if *read_only {
                                    "PLAYING (READ-ONLY)"
                                } else {
                                    "PLAYING (READ+WRITE)"
                                },
                                ACCENT_TEAL,
                            ),
                        };

                        ui.horizontal(|ui| {
                            ui.label(RichText::new("●").color(dot_color).small());
                            ui.label(RichText::new(state_lbl).strong().color(Color32::WHITE));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.monospace(
                                        RichText::new(format!(
                                            "Rerecords: {}",
                                            self.movie.rerecord_count()
                                        ))
                                        .small()
                                        .color(TEXT_MUTED),
                                    );
                                },
                            );
                        });

                        ui.add_space(6.0);
                        let cur = self.movie.current_frame();
                        let total = self.movie.total_frames();
                        let frame_line = if self.movie.is_recording() {
                            format!("Frame: {cur}")
                        } else {
                            format!("Frame: {cur} / {total}")
                        };
                        ui.horizontal(|ui| {
                            ui.monospace(RichText::new(frame_line).color(Color32::WHITE));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let anchor_lbl = if self.movie.has_initial_state() {
                                        "Anchor: RBAS Save State"
                                    } else {
                                        "Anchor: Hard Reset"
                                    };
                                    ui.label(RichText::new(anchor_lbl).small().color(TEXT_MUTED));
                                },
                            );
                        });

                        if self.movie.is_playing() && total > 0 {
                            ui.add_space(6.0);
                            let progress = (cur as f32 / total as f32).clamp(0.0, 1.0);
                            ui.add(
                                egui::ProgressBar::new(progress)
                                    .desired_height(5.0)
                                    .fill(ACCENT_TEAL),
                            );
                        }

                        if let Some(path) = &self.movie.active_path {
                            ui.add_space(4.0);
                            let file_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            ui.label(
                                RichText::new(format!("File: {file_name}"))
                                    .small()
                                    .color(TEXT_MUTED),
                            )
                            .on_hover_text(path.display().to_string());
                        }
                    });

                ui.add_space(8.0);

                // Playback mode option
                ui.horizontal(|ui| {
                    let mut ro = self.movie.read_only();
                    let ro_enabled = !self.movie.is_recording();
                    if ui
                        .add_enabled(ro_enabled, egui::Checkbox::new(&mut ro, "Read-Only"))
                        .on_hover_text(
                            "When unchecked during playback, loading a save state switches to Recording and increments rerecords.",
                        )
                        .changed()
                    {
                        self.movie.toggle_read_only();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Frame Advance: Ctrl+N")
                                .small()
                                .color(TEXT_MUTED),
                        );
                    });
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // Primary recording / playback actions
                ui.horizontal(|ui| {
                    let idle = !self.movie.is_active();
                    if ui
                        .add_enabled(idle, egui::Button::new("Start From Reset"))
                        .on_hover_text("Hard reset core and record inputs from frame 0")
                        .clicked()
                    {
                        action_start_reset = true;
                    }
                    if ui
                        .add_enabled(idle, egui::Button::new("Start From Current State"))
                        .on_hover_text("Embed current RBAS save state and record from here")
                        .clicked()
                    {
                        action_start_current = true;
                    }
                });

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    let open_btn = egui::Button::new("Open .rbm...");
                    if ui
                        .add_enabled(!self.movie.is_active(), open_btn)
                        .clicked()
                    {
                        action_open = true;
                    }

                    let save_btn = egui::Button::new("Save .rbm...");
                    if ui
                        .add_enabled(self.movie.has_saveable_movie(), save_btn)
                        .clicked()
                    {
                        action_save = true;
                    }

                    let stop_btn = if self.movie.is_active() {
                        egui::Button::new(
                            RichText::new("Stop").color(Color32::from_rgb(255, 130, 130)),
                        )
                    } else {
                        egui::Button::new("Stop")
                    };
                    if ui
                        .add_enabled(self.movie.is_active(), stop_btn)
                        .clicked()
                    {
                        action_stop = true;
                    }

                    if self.movie.can_replay_last()
                        && ui
                            .add(
                                egui::Button::new(RichText::new("Replay").color(ACCENT_TEAL))
                                    .fill(ACCENT_TEAL_BG)
                                    .stroke(Stroke::new(1.0_f32, ACCENT_TEAL)),
                            )
                            .clicked()
                    {
                        action_replay = true;
                    }
                });
            });

        self.show_movies = open;

        if dismiss_error {
            self.movie.last_error = None;
        }
        if action_start_reset {
            self.start_movie_recording(false);
        }
        if action_start_current {
            self.start_movie_recording(true);
        }
        if action_open {
            self.open_movie_dialog();
        }
        if action_save {
            self.save_movie_dialog();
        }
        if action_stop {
            self.stop_movie();
        }
        if action_replay {
            self.replay_last_movie();
        }
    }

    pub(crate) fn start_movie_recording(&mut self, from_current_state: bool) {
        let Some(core) = &mut self.core else {
            return;
        };
        match self.movie.start_recording(core, from_current_state) {
            Ok(()) => {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
                self.rewinding = false;
                if !from_current_state {
                    self.frame_counter = 0;
                    self.sink.framebuffer.fill(0);
                    self.sink.frame_dirty = true;
                    self.apply_active_game_settings_to_core();
                }
                if let Some(msg) = self.movie.take_notice() {
                    self.notify(&msg);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                self.movie.last_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    pub(crate) fn open_movie_dialog(&mut self) {
        if self.core.is_none() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Open Input Movie (.rbm)")
            .add_filter("Rustboy Input Movie", &["rbm"])
            .pick_file()
        {
            self.start_movie_playback_from_path(&path);
        }
    }

    pub(crate) fn start_movie_playback_from_path(&mut self, path: &Path) {
        let Some(core) = &mut self.core else {
            return;
        };
        match self.movie.start_playback(path, core) {
            Ok(()) => {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
                self.rewinding = false;
                self.frame_counter = 0;
                self.apply_active_game_settings_to_core();
                if let Some(core) = &self.core {
                    self.sink
                        .framebuffer
                        .copy_from_slice(&*core.bus.ppu.framebuffer);
                    self.sink.frame_dirty = true;
                }
                if let Some(msg) = self.movie.take_notice() {
                    self.notify(&msg);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                self.movie.last_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    pub(crate) fn replay_last_movie(&mut self) {
        let Some(core) = &mut self.core else {
            return;
        };
        match self.movie.replay_last(core) {
            Ok(()) => {
                self.rewind_buffer.clear();
                self.rewind_movie_frames.clear();
                self.rewinding = false;
                self.frame_counter = 0;
                self.apply_active_game_settings_to_core();
                if let Some(core) = &self.core {
                    self.sink
                        .framebuffer
                        .copy_from_slice(&*core.bus.ppu.framebuffer);
                    self.sink.frame_dirty = true;
                }
                if let Some(msg) = self.movie.take_notice() {
                    self.notify(&msg);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                self.movie.last_error = Some(msg.clone());
                self.notify_error(&msg);
            }
        }
    }

    pub(crate) fn save_movie_dialog(&mut self) {
        let default_name = if self.rom_title.trim().is_empty() {
            "movie.rbm".to_string()
        } else {
            format!("{}.rbm", self.rom_title.trim())
        };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save Input Movie (.rbm)")
            .add_filter("Rustboy Input Movie", &["rbm"])
            .set_file_name(default_name)
            .save_file()
        {
            match self.movie.save_to_path(&path) {
                Ok(()) => self.notify(&format!("Saved movie: {}", path.display())),
                Err(e) => {
                    let msg = e.to_string();
                    self.movie.last_error = Some(msg.clone());
                    self.notify_error(&msg);
                }
            }
        }
    }

    pub(crate) fn stop_movie(&mut self) {
        self.movie.stop();
        if let Some(msg) = self.movie.take_notice() {
            self.notify(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gba_core::{
        HostFrontend, KEY_A, KEY_B, KEY_RIGHT, KEY_START, LCD_HEIGHT, LCD_WIDTH, encode_keyinput,
    };

    struct TestFrontend(u16);
    impl HostFrontend for TestFrontend {
        fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            self.0
        }
    }

    fn make_test_rom(tag: u32) -> Vec<u8> {
        // Simple ARM loop that increments R0 by 1 each iteration and writes KEYINPUT to EWRAM:
        //   0x0800_0000: MOV R0, #0          (0xE3A00000)
        //   0x0800_0004: MOV R1, #0x04000000 (0xE3A01301)
        //   0x0800_0008: ADD R1, R1, #0x130  (0xE2811C01? no: #0x130 = rot=14, imm8=0x4C -> 0xE2811E13? Let's just use MOV R2, #0x02000000)
        // Actually even simpler:
        //   0x0800_0000: ADD R0, R0, #1      (0xE2800001)
        //   0x0800_0004: B 0x0800_0000       (0xEAFFFFFD)
        // Wait: 0x0800_0004 + 8 + (-3*4) = 0x0800_0000 -> offset -3 (0xFFFFFD).
        let mut rom = vec![0u8; 0x200];
        let words: [u32; 4] = [0xE280_0001, 0xEAFF_FFFD, tag, 0xE1A0_0000];
        for (i, &w) in words.iter().enumerate() {
            rom[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        rom
    }

    fn step_core_frame(core: &mut GbaCore, keypad: u16) {
        let mut fe = TestFrontend(keypad);
        core.run_frame(&mut fe);
    }

    #[test]
    fn rbm_encode_decode_roundtrip_reset_and_anchored() {
        let frames = vec![
            encode_keyinput(0),
            encode_keyinput(KEY_A),
            encode_keyinput(KEY_A | KEY_RIGHT),
            encode_keyinput(KEY_START | KEY_B),
            encode_keyinput(0),
        ];

        // 1. Start from hard reset (empty initial_state)
        let movie_reset = RbmMovie {
            rom_crc32: 0xCAFE_BABE,
            bios_present: false,
            rerecord_count: 17,
            initial_state: Vec::new(),
            frames: frames.clone(),
        };
        let bytes = movie_reset.encode().unwrap();
        assert_eq!(&bytes[0..4], b"RBMV");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), RBMV_VERSION);
        let decoded = RbmMovie::decode(&bytes).unwrap();
        assert_eq!(decoded, movie_reset);

        // 2. Embedded initial RBAS save state + bios_present = true
        let core = GbaCore::new(None, make_test_rom(0x1111_2222));
        let state_bytes = core.save_state().unwrap();
        assert!(state_bytes.starts_with(b"RBAS"));

        let movie_anchored = RbmMovie {
            rom_crc32: core.rom_crc32(),
            bios_present: true,
            rerecord_count: 42,
            initial_state: state_bytes,
            frames,
        };
        let bytes2 = movie_anchored.encode().unwrap();
        let decoded2 = RbmMovie::decode(&bytes2).unwrap();
        assert_eq!(decoded2, movie_anchored);

        // 3. Zero-frame movie roundtrip
        let movie_empty = RbmMovie {
            rom_crc32: 0x1234_5678,
            bios_present: false,
            rerecord_count: 0,
            initial_state: Vec::new(),
            frames: Vec::new(),
        };
        let bytes3 = movie_empty.encode().unwrap();
        assert_eq!(RbmMovie::decode(&bytes3).unwrap(), movie_empty);
    }

    #[test]
    fn rbm_decode_rejects_corrupt_header_and_version_mismatch() {
        let movie = RbmMovie {
            rom_crc32: 0xABCD_1234,
            bios_present: false,
            rerecord_count: 1,
            initial_state: Vec::new(),
            frames: vec![0x03FF, 0x03FE],
        };
        let valid = movie.encode().unwrap();

        let mut bad_magic = valid.clone();
        bad_magic[0..4].copy_from_slice(b"NOPE");
        assert!(matches!(
            RbmMovie::decode(&bad_magic),
            Err(MovieError::Corrupt(_))
        ));

        let mut bad_ver = valid.clone();
        bad_ver[4..6].copy_from_slice(&99u16.to_le_bytes());
        assert!(matches!(
            RbmMovie::decode(&bad_ver),
            Err(MovieError::VersionMismatch {
                found: 99,
                expected: RBMV_VERSION
            })
        ));

        let truncated = &valid[..valid.len() - 1];
        assert!(matches!(
            RbmMovie::decode(truncated),
            Err(MovieError::Corrupt(_))
        ));
    }

    #[test]
    fn playback_rejects_rom_crc32_mismatch() {
        let mut core_a = GbaCore::new(None, make_test_rom(0xAAAA_0001));
        let mut core_b = GbaCore::new(None, make_test_rom(0xBBBB_0002));

        let mut mc = MovieController::new();
        mc.start_recording(&mut core_a, false).unwrap();
        let k = mc.step_frame(encode_keyinput(KEY_A));
        step_core_frame(&mut core_a, k);

        let dir = std::env::temp_dir().join("rustboy_movie_test_crc");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("mismatch.rbm");
        mc.stop_and_save(&path).unwrap();

        let mut player = MovieController::new();
        let res = player.start_playback(&path, &mut core_b);
        assert!(matches!(res, Err(MovieError::RomMismatch { .. })));
        assert!(!player.is_active());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn initial_savestate_anchor_restores_exact_state() {
        let rom = make_test_rom(0x5555_6666);
        let mut core = GbaCore::new(None, rom);

        // Advance 4 frames and mutate EWRAM before starting anchored recording
        for _ in 0..4 {
            step_core_frame(&mut core, encode_keyinput(0));
        }
        core.poke32(0x0200_0100, 0xDEAD_C0DE);
        let expected_r0_at_anchor = core.cpu.gpr[0];
        let expected_cycle_at_anchor = core.bus.scheduler.current_cycle;

        let mut mc = MovieController::new();
        mc.start_recording(&mut core, true).unwrap();
        assert!(mc.has_initial_state());

        let recorded_inputs = [
            encode_keyinput(KEY_A),
            encode_keyinput(KEY_B),
            encode_keyinput(KEY_START),
        ];
        for &inp in &recorded_inputs {
            let k = mc.step_frame(inp);
            step_core_frame(&mut core, k);
        }
        let expected_r0_after = core.cpu.gpr[0];

        let dir = std::env::temp_dir().join("rustboy_movie_test_anchor");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("anchored.rbm");
        mc.stop_and_save(&path).unwrap();

        // Corrupt core memory & advance wildly before starting playback
        core.poke32(0x0200_0100, 0x0000_0000);
        for _ in 0..10 {
            step_core_frame(&mut core, encode_keyinput(KEY_RIGHT));
        }
        assert_ne!(core.peek32(0x0200_0100), 0xDEAD_C0DE);

        let mut player = MovieController::new();
        player.start_playback(&path, &mut core).unwrap();
        assert_eq!(core.peek32(0x0200_0100), 0xDEAD_C0DE);
        assert_eq!(core.cpu.gpr[0], expected_r0_at_anchor);
        assert_eq!(core.bus.scheduler.current_cycle, expected_cycle_at_anchor);

        for _ in 0..recorded_inputs.len() {
            let k = player.step_frame(encode_keyinput(0));
            step_core_frame(&mut core, k);
        }
        assert_eq!(core.cpu.gpr[0], expected_r0_after);
        assert!(!player.is_active());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_playback_vs_rerecord_truncation() {
        let rom = make_test_rom(0x7777_8888);
        let mut core = GbaCore::new(None, rom);

        // Record 6 frames, saving a state snapshot at frame 3
        let mut mc = MovieController::new();
        mc.start_recording(&mut core, false).unwrap();

        let frame_inputs = [
            encode_keyinput(KEY_A),
            encode_keyinput(KEY_B),
            encode_keyinput(KEY_START),
            encode_keyinput(KEY_RIGHT),
            encode_keyinput(KEY_A | KEY_RIGHT),
            encode_keyinput(KEY_B | KEY_RIGHT),
        ];
        let mut state_at_3 = Vec::new();
        for (idx, &inp) in frame_inputs.iter().enumerate() {
            if idx == 3 {
                state_at_3 = core.save_state().unwrap();
            }
            let k = mc.step_frame(inp);
            step_core_frame(&mut core, k);
        }
        assert_eq!(mc.total_frames(), 6);
        assert_eq!(mc.rerecord_count(), 0);

        let dir = std::env::temp_dir().join("rustboy_movie_test_rerecord");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rerecord.rbm");
        mc.stop_and_save(&path).unwrap();

        // 1. Read-Only playback: loading state_at_3 jumps cursor to 3 without rerecords/truncation
        let mut player = MovieController::new();
        player.start_playback(&path, &mut core).unwrap();
        assert!(player.read_only());

        for _ in 0..5 {
            let k = player.step_frame(encode_keyinput(0));
            step_core_frame(&mut core, k);
        }
        assert_eq!(player.current_frame(), 5);

        core.load_state(&state_at_3).unwrap();
        player.on_state_loaded(&core);
        assert_eq!(player.rerecord_count(), 0);
        assert_eq!(player.total_frames(), 6);
        assert_eq!(player.current_frame(), 3);
        assert!(player.is_playing());
        assert_eq!(player.step_frame(encode_keyinput(0)), frame_inputs[3]);

        // 2. Switch to Read+Write: loading state_at_3 increments rerecords, truncates to 3, resumes Recording
        player.toggle_read_only();
        assert!(!player.read_only());

        core.load_state(&state_at_3).unwrap();
        player.on_state_loaded(&core);
        assert!(player.is_recording());
        assert_eq!(player.rerecord_count(), 1);
        assert_eq!(player.total_frames(), 3);
        assert_eq!(player.current_frame(), 3);

        let new_input = encode_keyinput(KEY_A | KEY_B);
        assert_eq!(player.step_frame(new_input), new_input);
        assert_eq!(player.total_frames(), 4);

        // Save updated movie and verify truncated+appended frames
        player.stop_and_save(&path).unwrap();
        let reloaded = RbmMovie::load_from_file(&path).unwrap();
        assert_eq!(reloaded.rerecord_count, 1);
        assert_eq!(
            reloaded.frames,
            vec![frame_inputs[0], frame_inputs[1], frame_inputs[2], new_input]
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
