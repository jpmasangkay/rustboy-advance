use std::collections::VecDeque;
use std::time::Instant;

use egui::{self, Color32, CornerRadius, RichText, Stroke};
use lz4_flex::block::{compress, decompress};

use crate::app::{
    ACCENT_TEAL, ACCENT_TEAL_BG, BG_CARD, BG_PANEL, BORDER_HAIRLINE, GBA_FRAME_SECS, RustboyApp,
    TEXT_MUTED,
};

/// Store a full LZ4 keyframe every N recorded snapshots.
pub(crate) const KEYFRAME_INTERVAL: usize = 30;

const BUDGET_PRESETS_MB: [u32; 5] = [16, 32, 64, 128, 256];

/// Computes `out[i] = a[i] ^ b[i]` over `max(a.len(), b.len())` with zero-extension.
fn xor_buffers(a: &[u8], b: &[u8], out: &mut [u8]) {
    let min_len = a.len().min(b.len());
    let (a_chunks, _) = a[..min_len].as_chunks::<8>();
    let (b_chunks, _) = b[..min_len].as_chunks::<8>();
    let (out_chunks, _) = out[..min_len].as_chunks_mut::<8>();
    let chunk_bytes = a_chunks.len() * 8;

    for ((ca, cb), co) in a_chunks.iter().zip(b_chunks.iter()).zip(out_chunks.iter_mut()) {
        let va = u64::from_ne_bytes(*ca);
        let vb = u64::from_ne_bytes(*cb);
        *co = (va ^ vb).to_ne_bytes();
    }

    for i in chunk_bytes..min_len {
        out[i] = a[i] ^ b[i];
    }

    if a.len() > min_len {
        out[min_len..a.len()].copy_from_slice(&a[min_len..]);
    } else if b.len() > min_len {
        out[min_len..b.len()].copy_from_slice(&b[min_len..]);
    }
}

/// LZ4-compressed XOR delta between two consecutive serialized core states.
#[derive(Clone, Debug)]
pub struct XorDelta {
    compressed_xor: Vec<u8>,
    prev_len: usize,
    curr_len: usize,
}

impl XorDelta {
    /// Encodes an XOR delta between `prev` and `curr`, reusing `scratch` to avoid heap churn.
    pub fn encode_with_scratch(prev: &[u8], curr: &[u8], scratch: &mut Vec<u8>) -> Self {
        let max_len = prev.len().max(curr.len());
        if scratch.len() < max_len {
            scratch.resize(max_len, 0);
        }
        xor_buffers(prev, curr, &mut scratch[..max_len]);
        let compressed_xor = compress(&scratch[..max_len]);
        Self {
            compressed_xor,
            prev_len: prev.len(),
            curr_len: curr.len(),
        }
    }

    /// Reconstructs `prev` given `curr` by decompressing and XORing the stored delta.
    pub fn apply_backward(&self, curr: &[u8]) -> Result<Vec<u8>, String> {
        let max_len = self.prev_len.max(self.curr_len);
        let xor = decompress(&self.compressed_xor, max_len)
            .map_err(|e| format!("Rewind delta decompress failed: {e}"))?;
        if xor.len() != max_len {
            return Err(format!(
                "Rewind delta size mismatch: got {} bytes, expected {max_len}",
                xor.len()
            ));
        }

        let mut prev = vec![0u8; self.prev_len];
        let common = self.prev_len.min(curr.len()).min(xor.len());
        let (curr_chunks, _) = curr[..common].as_chunks::<8>();
        let (xor_chunks, _) = xor[..common].as_chunks::<8>();
        let (prev_chunks, _) = prev[..common].as_chunks_mut::<8>();
        let chunk_bytes = curr_chunks.len() * 8;
        for ((cc, cx), cp) in curr_chunks
            .iter()
            .zip(xor_chunks.iter())
            .zip(prev_chunks.iter_mut())
        {
            let vc = u64::from_ne_bytes(*cc);
            let vx = u64::from_ne_bytes(*cx);
            *cp = (vc ^ vx).to_ne_bytes();
        }
        for i in chunk_bytes..common {
            prev[i] = curr[i] ^ xor[i];
        }
        if self.prev_len > common {
            prev[common..self.prev_len].copy_from_slice(&xor[common..self.prev_len]);
        }
        Ok(prev)
    }

    pub fn byte_len(&self) -> usize {
        self.compressed_xor.len() + std::mem::size_of::<Self>()
    }
}

enum RewindEntry {
    Keyframe {
        compressed: Vec<u8>,
        #[allow(dead_code)]
        uncompressed_len: usize,
        back_delta: Option<XorDelta>,
    },
    Delta(XorDelta),
}

impl RewindEntry {
    fn compressed_bytes(&self) -> usize {
        match self {
            Self::Keyframe {
                compressed,
                back_delta,
                ..
            } => {
                compressed.len()
                    + back_delta.as_ref().map_or(0, XorDelta::byte_len)
                    + std::mem::size_of::<Self>()
            }
            Self::Delta(delta) => delta.byte_len() + std::mem::size_of::<usize>(),
        }
    }
}

/// Bounded ring buffer of LZ4-compressed XOR deltas with periodic full keyframes.
pub struct RewindBuffer {
    entries: VecDeque<RewindEntry>,
    head_raw: Option<Vec<u8>>,
    budget_bytes: usize,
    total_bytes: usize,
    frames_since_keyframe: usize,
    xor_scratch: Vec<u8>,
    pub last_encode_us: f32,
    pub ema_encode_us: f32,
}

impl RewindBuffer {
    pub fn new(budget_mb: u32) -> Self {
        Self {
            entries: VecDeque::new(),
            head_raw: None,
            budget_bytes: (budget_mb as usize) * 1024 * 1024,
            total_bytes: 0,
            frames_since_keyframe: 0,
            xor_scratch: Vec::new(),
            last_encode_us: 0.0,
            ema_encode_us: 0.0,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_budget_bytes(budget_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            head_raw: None,
            budget_bytes,
            total_bytes: 0,
            frames_since_keyframe: 0,
            xor_scratch: Vec::new(),
            last_encode_us: 0.0,
            ema_encode_us: 0.0,
        }
    }

    pub fn set_budget_mb(&mut self, budget_mb: u32) {
        self.budget_bytes = (budget_mb as usize) * 1024 * 1024;
        self.evict_to_budget();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.head_raw = None;
        self.total_bytes = 0;
        self.frames_since_keyframe = 0;
        self.last_encode_us = 0.0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn seconds_available(&self, interval_frames: u8) -> f32 {
        let step_frames = interval_frames.max(1) as f32;
        ((self.entries.len() as f32) * step_frames) / (1.0_f64 / GBA_FRAME_SECS) as f32
    }

    #[cfg(test)]
    pub(crate) fn keyframe_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, RewindEntry::Keyframe { .. }))
            .count()
    }

    /// Compresses and appends a serialized core state snapshot, evicting oldest entries to fit the budget.
    pub fn push(&mut self, state: Vec<u8>) {
        if self.budget_bytes == 0 {
            return;
        }
        let start = Instant::now();

        match self.head_raw.take() {
            None => {
                let compressed = compress(&state);
                let uncompressed_len = state.len();
                let entry = RewindEntry::Keyframe {
                    compressed,
                    uncompressed_len,
                    back_delta: None,
                };
                let needed = entry.compressed_bytes() + state.len();
                if needed > self.budget_bytes {
                    return;
                }
                self.total_bytes = needed;
                self.head_raw = Some(state);
                self.entries.push_back(entry);
                self.frames_since_keyframe = 0;
            }
            Some(prev) => {
                let delta = XorDelta::encode_with_scratch(&prev, &state, &mut self.xor_scratch);
                self.frames_since_keyframe += 1;

                let entry = if self.frames_since_keyframe >= KEYFRAME_INTERVAL {
                    let compressed = compress(&state);
                    self.frames_since_keyframe = 0;
                    RewindEntry::Keyframe {
                        compressed,
                        uncompressed_len: state.len(),
                        back_delta: Some(delta),
                    }
                } else {
                    RewindEntry::Delta(delta)
                };

                self.total_bytes = self
                    .total_bytes
                    .saturating_sub(prev.len())
                    .saturating_add(state.len())
                    .saturating_add(entry.compressed_bytes());
                self.head_raw = Some(state);
                self.entries.push_back(entry);
                self.evict_to_budget();
            }
        }

        let us = start.elapsed().as_secs_f32() * 1_000_000.0;
        self.last_encode_us = us;
        self.ema_encode_us = if self.ema_encode_us <= 0.0 {
            us
        } else {
            self.ema_encode_us * 0.9 + us * 0.1
        };
    }

    /// Pops and returns the most recent snapshot, reconstructing the preceding state in memory.
    pub fn pop_back(&mut self) -> Option<Vec<u8>> {
        let head = self.head_raw.take()?;
        let entry = self.entries.pop_back()?;
        self.total_bytes = self
            .total_bytes
            .saturating_sub(entry.compressed_bytes() + head.len());

        if self.entries.is_empty() {
            self.frames_since_keyframe = 0;
            self.total_bytes = 0;
            return Some(head);
        }

        if let RewindEntry::Keyframe { .. } = &entry {
            // Restored prior to a keyframe; reset counter conservatively for future appends.
            self.frames_since_keyframe = KEYFRAME_INTERVAL.saturating_sub(1);
        } else {
            self.frames_since_keyframe = self.frames_since_keyframe.saturating_sub(1);
        }

        let back_delta = match &entry {
            RewindEntry::Delta(d) => Some(d),
            RewindEntry::Keyframe { back_delta, .. } => back_delta.as_ref(),
        };

        if let Some(delta) = back_delta {
            match delta.apply_backward(&head) {
                Ok(prev) => {
                    self.total_bytes += prev.len();
                    self.head_raw = Some(prev);
                }
                Err(_) => {
                    self.clear();
                }
            }
        } else {
            self.clear();
        }
        Some(head)
    }

    /// Pops up to `steps` snapshots in one call, returning the oldest popped snapshot.
    pub fn pop_steps(&mut self, steps: usize) -> Option<Vec<u8>> {
        let count = steps.max(1);
        let mut target = None;
        for _ in 0..count {
            if let Some(snap) = self.pop_back() {
                target = Some(snap);
            } else {
                break;
            }
        }
        target
    }

    fn evict_to_budget(&mut self) {
        while self.total_bytes > self.budget_bytes && self.entries.len() > 1 {
            if let Some(rel_kf) = self
                .entries
                .iter()
                .skip(1)
                .position(|e| matches!(e, RewindEntry::Keyframe { .. }))
            {
                let drop_count = rel_kf + 1;
                for _ in 0..drop_count {
                    if let Some(evicted) = self.entries.pop_front() {
                        self.total_bytes = self.total_bytes.saturating_sub(evicted.compressed_bytes());
                    }
                }
                if let Some(RewindEntry::Keyframe { back_delta, .. }) = self.entries.front_mut()
                    && let Some(delta) = back_delta.take()
                {
                    self.total_bytes = self.total_bytes.saturating_sub(delta.byte_len());
                }
            } else if let Some(evicted) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.compressed_bytes());
            } else {
                break;
            }
        }

        if self.total_bytes > self.budget_bytes && self.entries.len() == 1 {
            self.clear();
        }
    }

    /// Decompresses the oldest keyframe if present (used for integrity verification in tests).
    #[cfg(test)]
    pub(crate) fn decode_oldest_keyframe(&self) -> Option<Result<Vec<u8>, String>> {
        self.entries.front().and_then(|entry| match entry {
            RewindEntry::Keyframe {
                compressed,
                uncompressed_len,
                ..
            } => Some(
                decompress(compressed, *uncompressed_len)
                    .map_err(|e| format!("Keyframe decompress failed: {e}")),
            ),
            RewindEntry::Delta(_) => None,
        })
    }
}

impl RustboyApp {
    pub(crate) fn step_rewind_frame(&mut self) {
        if !self.config.rewind_enabled {
            self.rewinding = false;
            return;
        }
        let speed = self.config.rewind_speed.clamp(1, 4) as usize;
        if let Some(target) = self.rewind_buffer.pop_steps(speed) {
            self.apply_rewind_snapshot(&target, None);
        } else {
            self.notify("Rewind buffer empty");
            self.rewinding = false;
        }
    }

    pub(crate) fn step_rewind_manual(&mut self) {
        if !self.config.rewind_enabled {
            self.notify("Rewind is disabled in Emulation -> Rewind Settings");
            return;
        }
        let interval = self.config.rewind_interval.clamp(1, 4) as usize;
        let steps = (30 / interval).max(1);
        if let Some(target) = self.rewind_buffer.pop_steps(steps) {
            self.apply_rewind_snapshot(&target, Some("Rewound ~0.5s"));
        } else {
            self.notify("Rewind buffer empty");
        }
    }

    fn apply_rewind_snapshot(&mut self, bytes: &[u8], toast_msg: Option<&str>) {
        if self.load_active_core_state_raw(bytes).is_ok()
            || self.load_active_core_state(bytes).is_ok()
        {
            self.show_pause_overlay = false;
            if let Some(msg) = toast_msg {
                self.notify(msg);
            }
        }
    }

    pub(crate) fn draw_rewind_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_rewind_settings {
            return;
        }
        let mut open = self.show_rewind_settings;
        let mut changed = false;

        egui::Window::new("Rewind Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(BG_CARD)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                ui.set_min_width(420.0);

                ui.horizontal(|ui| {
                    if ui
                        .checkbox(&mut self.config.rewind_enabled, "Enable Real-Time Rewind")
                        .changed()
                    {
                        changed = true;
                        if !self.config.rewind_enabled {
                            self.rewinding = false;
                            self.rewind_buffer.clear();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let hotkey = self
                            .config
                            .keybindings
                            .hotkeys
                            .display(crate::config::HotkeyAction::Rewind);
                        ui.label(
                            RichText::new(format!("Hold {hotkey} in-game"))
                                .small()
                                .color(TEXT_MUTED),
                        );
                    });
                });

                ui.add_space(10.0);

                egui::Frame::new()
                    .fill(BG_PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Memory Budget").strong());
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            for &mb in &BUDGET_PRESETS_MB {
                                let selected = self.config.rewind_budget_mb == mb;
                                let mut btn = egui::Button::new(format!("{mb} MB"));
                                if selected {
                                    btn = btn
                                        .fill(ACCENT_TEAL_BG)
                                        .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
                                }
                                if ui.add(btn).clicked() && !selected {
                                    self.config.rewind_budget_mb = mb;
                                    self.rewind_buffer.set_budget_mb(mb);
                                    changed = true;
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.label(RichText::new("Capture Interval").strong());
                        ui.label(
                            RichText::new(
                                "Lower intervals rewind smoother; higher intervals stretch history duration.",
                            )
                            .small()
                            .color(TEXT_MUTED),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            for interval in 1u8..=4u8 {
                                let selected = self.config.rewind_interval.clamp(1, 4) == interval;
                                let label = if interval == 1 {
                                    "Every Frame".to_string()
                                } else {
                                    format!("Every {interval} Frames")
                                };
                                let mut btn = egui::Button::new(label);
                                if selected {
                                    btn = btn
                                        .fill(ACCENT_TEAL_BG)
                                        .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
                                }
                                if ui.add(btn).clicked() && !selected {
                                    self.config.rewind_interval = interval;
                                    changed = true;
                                }
                            }
                        });

                        ui.add_space(10.0);
                        ui.label(RichText::new("Rewind Playback Speed").strong());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            for spd in 1u8..=4u8 {
                                let selected = self.config.rewind_speed.clamp(1, 4) == spd;
                                let mut btn = egui::Button::new(format!("{spd}x"));
                                if selected {
                                    btn = btn
                                        .fill(ACCENT_TEAL_BG)
                                        .stroke(Stroke::new(1.0_f32, ACCENT_TEAL));
                                }
                                if ui.add(btn).clicked() && !selected {
                                    self.config.rewind_speed = spd;
                                    changed = true;
                                }
                            }
                        });
                    });

                ui.add_space(10.0);

                // Buffer status card
                egui::Frame::new()
                    .fill(BG_PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        let used_mb = self.rewind_buffer.total_bytes() as f32 / (1024.0 * 1024.0);
                        let budget_mb =
                            (self.rewind_buffer.budget_bytes() as f32 / (1024.0 * 1024.0)).max(1.0);
                        let ratio = (used_mb / budget_mb).clamp(0.0, 1.0);
                        let secs = self
                            .rewind_buffer
                            .seconds_available(self.config.rewind_interval);

                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Live Buffer").strong());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{used_mb:.1} / {budget_mb:.0} MB"))
                                            .small()
                                            .color(TEXT_MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(4.0);
                        ui.add(
                            egui::ProgressBar::new(ratio)
                                .desired_height(6.0)
                                .fill(ACCENT_TEAL),
                        );
                        ui.add_space(6.0);

                        if !self.config.rewind_enabled {
                            ui.label(
                                RichText::new("Rewind recording is currently disabled.")
                                    .small()
                                    .color(Color32::from_rgb(240, 180, 90)),
                            );
                        } else if self.rewind_buffer.is_empty() {
                            ui.label(
                                RichText::new(
                                    "0.0 seconds available — Run a game to populate rewind history.",
                                )
                                .small()
                                .color(TEXT_MUTED),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{secs:.1} seconds available ({} snapshots)",
                                        self.rewind_buffer.len()
                                    ))
                                    .color(ACCENT_TEAL),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if self.rewind_buffer.ema_encode_us > 0.0 {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{:.2} ms/snap",
                                                    self.rewind_buffer.ema_encode_us / 1000.0
                                                ))
                                                .small()
                                                .color(TEXT_MUTED),
                                            );
                                        }
                                    },
                                );
                            });
                        }
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let has_entries = !self.rewind_buffer.is_empty();
                    if ui
                        .add_enabled(has_entries, egui::Button::new("Clear Buffer"))
                        .clicked()
                    {
                        self.rewind_buffer.clear();
                        self.notify("Rewind buffer cleared");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.show_rewind_settings = false;
                        }
                    });
                });
            });

        if changed {
            self.config.save();
        }
        if !open {
            self.show_rewind_settings = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_synthetic_state(frame: u32, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        // Simulate mostly-stable WRAM/VRAM with small evolving regions and changing registers
        for (i, b) in buf.iter_mut().enumerate().take(len.min(4096)) {
            *b = ((i as u32).wrapping_mul(31) ^ (i as u32 >> 2)) as u8;
        }
        let fbytes = frame.to_le_bytes();
        let off = (frame as usize * 17) % (len.saturating_sub(64).max(1));
        for k in 0..16.min(len.saturating_sub(off)) {
            buf[off + k] = fbytes[k % 4].wrapping_add(k as u8);
        }
        buf[0..4].copy_from_slice(&fbytes);
        buf
    }

    #[test]
    fn xor_delta_roundtrip_with_variable_lengths() {
        let mut scratch = Vec::new();
        let a = make_synthetic_state(1, 512 + 3);
        let b = make_synthetic_state(2, 512 + 9); // grows (varint growth)
        let c = make_synthetic_state(3, 512 + 1); // shrinks

        let d_ab = XorDelta::encode_with_scratch(&a, &b, &mut scratch);
        let d_bc = XorDelta::encode_with_scratch(&b, &c, &mut scratch);

        let rec_b = d_bc.apply_backward(&c).unwrap();
        assert_eq!(rec_b, b);
        let rec_a = d_ab.apply_backward(&rec_b).unwrap();
        assert_eq!(rec_a, a);
    }

    #[test]
    fn ring_buffer_roundtrips_across_keyframes_and_evicts_within_budget() {
        let state_len = 65_536;
        let budget = 512 * 1024;
        let mut rb = RewindBuffer::with_budget_bytes(budget);
        let mut history = Vec::new();

        for f in 0..95u32 {
            // Vary snapshot size slightly every 7 frames to exercise variable-length deltas
            let len = state_len + ((f as usize % 5) * 3);
            let snap = make_synthetic_state(f, len);
            history.push(snap.clone());
            rb.push(snap);
            assert!(
                rb.total_bytes() <= rb.budget_bytes(),
                "total_bytes {} exceeded budget {}",
                rb.total_bytes(),
                rb.budget_bytes()
            );
        }

        assert!(rb.len() > 40);
        assert!(rb.keyframe_count() >= 2);
        if let Some(res) = rb.decode_oldest_keyframe() {
            assert!(res.is_ok());
        }

        // Unwind everything currently in buffer and verify exact byte-for-byte match against tail of history
        let mut popped = Vec::new();
        while let Some(snap) = rb.pop_back() {
            popped.push(snap);
        }
        assert!(rb.is_empty());
        assert_eq!(rb.total_bytes(), 0);
        assert!(!popped.is_empty());

        let expected_start = history.len() - popped.len();
        for (i, restored) in popped.iter().rev().enumerate() {
            assert_eq!(restored, &history[expected_start + i]);
        }
    }

    #[test]
    fn test_rewind_encode_performance_and_seconds_estimate() {
        let mut rb = RewindBuffer::new(64);
        let s0 = make_synthetic_state(0, 420 * 1024);
        let s1 = make_synthetic_state(1, 420 * 1024);
        rb.push(s0);

        let t0 = Instant::now();
        rb.push(s1);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Smoke sanity check: even in unoptimized debug builds, XOR+LZ4 of 420 KB should complete quickly
        assert!(elapsed_ms < 50.0, "unexpectedly slow delta encode: {elapsed_ms:.2} ms");
        assert!(rb.seconds_available(1) > 0.0);
    }
}
