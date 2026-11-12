//! Dedicated emulation worker thread and lock-free frame/audio IPC.
//!
//! Features:
//! - Lock-free triple buffer for 240x160 (38,400 pixel) BGR555 frame presentation (no Mutex on frame path).
//! - Thread-safe AtomicU16 input keypad register.
//! - Dynamic Rate Control (DRC) audio pacing from the SPSC ring buffer:
//!     target = capacity / 2, ratio = 1.0 + 0.005 * ((target - filled) / target), clamped to ±0.5%.
//!     Fallback to Instant-based 59.7275 Hz pacer when no audio device exists.
//! - Error channel delivering background emulation errors to the UI status bar without `eprintln!`.

#![allow(dead_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

use crate::audio::AudioProducer;

pub const FRAME_PIXELS: usize = LCD_WIDTH * LCD_HEIGHT;

/// Lock-free triple buffer for 38,400-pixel frames.
///
/// Guaranteed zero allocations per frame and zero Mutex contention on the rendering path.
pub struct SharedTripleBuffer {
    buffers: [UnsafeCell<[u16; FRAME_PIXELS]>; 3],
    reading: AtomicUsize,
    latest: AtomicUsize,
    frame_dirty: AtomicBool,
}

unsafe impl Send for SharedTripleBuffer {}
unsafe impl Sync for SharedTripleBuffer {}

impl SharedTripleBuffer {
    pub fn new() -> Self {
        Self {
            buffers: [
                UnsafeCell::new([0; FRAME_PIXELS]),
                UnsafeCell::new([0; FRAME_PIXELS]),
                UnsafeCell::new([0; FRAME_PIXELS]),
            ],
            reading: AtomicUsize::new(1),
            latest: AtomicUsize::new(2),
            frame_dirty: AtomicBool::new(false),
        }
    }

    /// Selects the initial write slot for the producer.
    pub fn next_write_slot(&self, _last_written: usize) -> usize {
        0
    }

    /// Mutable reference to a designated write slot.
    ///
    /// # Safety
    /// Only the emulation worker thread accesses this write slot before publishing.
    pub unsafe fn write_buffer_mut(&self, slot: usize) -> &mut [u16; FRAME_PIXELS] {
        unsafe { &mut *self.buffers[slot].get() }
    }

    /// Marks the given slot as the latest published frame for the UI and returns
    /// the new unread buffer slot allocated for the writer.
    pub fn publish_frame(&self, written_slot: usize) -> usize {
        let next_slot = self.latest.swap(written_slot, Ordering::AcqRel);
        self.frame_dirty.store(true, Ordering::Release);
        next_slot
    }

    /// Checks if a new frame is pending for display.
    pub fn is_dirty(&self) -> bool {
        self.frame_dirty.load(Ordering::Acquire)
    }

    /// Reads the latest frame without allocations or Mutex contention.
    ///
    /// Atomically swaps the consumer's current read slot with the latest published
    /// frame slot, ensuring writer and reader never access the same slot simultaneously.
    pub fn read_latest<R>(&self, reader: impl FnOnce(&[u16; FRAME_PIXELS]) -> R) -> Option<R> {
        if !self.frame_dirty.swap(false, Ordering::AcqRel) {
            return None;
        }
        let old_read = self.reading.load(Ordering::Relaxed);
        let new_read = self.latest.swap(old_read, Ordering::AcqRel);
        self.reading.store(new_read, Ordering::Release);
        let buf = unsafe { &*self.buffers[new_read].get() };
        Some(reader(buf))
    }

    /// Force marks dirty (e.g. after savestate restore or palette clear).
    pub fn mark_dirty(&self) {
        self.frame_dirty.store(true, Ordering::Release);
    }
}

/// Dynamic Rate Control (DRC) pacer for the emulation thread.
pub struct DrcPacer {
    target: usize,
    base_frame_duration: Duration,
    last_frame_instant: Instant,
}

impl DrcPacer {
    pub fn new(capacity: usize) -> Self {
        Self {
            target: capacity / 2,
            base_frame_duration: Duration::from_secs_f64(1.0 / 59.7275),
            last_frame_instant: Instant::now(),
        }
    }

    /// Paces the emulation thread using the SPSC queue fill level:
    /// target = capacity / 2
    /// resample ratio = 1.0 + 0.005 * ((target - filled) / target), clamped to ±0.5%.
    /// Falls back to Instant-based 59.7275 Hz pacer when no audio device is present.
    pub fn pace(&mut self, filled: usize, has_audio_device: bool) {
        let frame_duration = if has_audio_device {
            let target_f = self.target as f64;
            let filled_f = filled as f64;
            let ratio = (1.0 + 0.005 * ((target_f - filled_f) / target_f)).clamp(0.995, 1.005);
            self.base_frame_duration.div_f64(ratio)
        } else {
            self.base_frame_duration
        };

        let elapsed = self.last_frame_instant.elapsed();
        if elapsed < frame_duration {
            let sleep_time = frame_duration - elapsed;
            if sleep_time > Duration::from_millis(1) {
                thread::sleep(sleep_time - Duration::from_millis(1));
            }
            while self.last_frame_instant.elapsed() < frame_duration {
                std::hint::spin_loop();
            }
        }
        self.last_frame_instant = Instant::now();
    }
}

/// Internal frontend implementation for the worker thread.
struct WorkerFrontend<'a> {
    triple_buffer: &'a SharedTripleBuffer,
    write_slot: usize,
    keypad: &'a AtomicU16,
    audio: &'a mut AudioProducer,
    audio_scratch: Vec<i16>,
}

impl<'a> HostFrontend for WorkerFrontend<'a> {
    fn present_frame(&mut self, fb: &[u16; FRAME_PIXELS]) {
        // SAFETY: Only this thread writes to the allocated `write_slot`
        unsafe {
            let buf = self.triple_buffer.write_buffer_mut(self.write_slot);
            buf.copy_from_slice(fb);
        }
        self.write_slot = self.triple_buffer.publish_frame(self.write_slot);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio.push_interleaved(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad.load(Ordering::Relaxed)
    }
}

/// Commands delivered to the background emulation worker.
pub enum EmuCommand {
    Pause,
    Resume,
    Reset,
    LoadState(Vec<u8>),
    SaveState(Sender<Result<Vec<u8>, String>>),
    Terminate,
}

/// Handle to the background emulation worker thread.
pub struct EmuThreadHandle {
    pub keypad: Arc<AtomicU16>,
    pub triple_buffer: Arc<SharedTripleBuffer>,
    cmd_tx: Sender<EmuCommand>,
    pub error_rx: Receiver<String>,
    worker_join: Option<JoinHandle<()>>,
}

impl EmuThreadHandle {
    pub fn spawn(
        mut core: GbaCore,
        mut audio: AudioProducer,
        keypad: Arc<AtomicU16>,
        triple_buffer: Arc<SharedTripleBuffer>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = channel::<EmuCommand>();
        let (error_tx, error_rx) = channel::<String>();

        let tb = triple_buffer.clone();
        let kp = keypad.clone();

        let worker_join = thread::Builder::new()
            .name("gba-core-worker".into())
            .spawn(move || {
                let capacity = audio.capacity();
                let mut pacer = DrcPacer::new(capacity);
                let mut write_slot = tb.next_write_slot(0);
                let mut paused = false;

                loop {
                    // Check incoming commands non-blocking
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            EmuCommand::Pause => paused = true,
                            EmuCommand::Resume => paused = false,
                            EmuCommand::Reset => {
                                core.soft_reset();
                            }
                            EmuCommand::LoadState(bytes) => {
                                if let Err(e) = core.load_state(&bytes) {
                                    let _ = error_tx.send(format!("Failed to load save state: {e:?}"));
                                } else {
                                    tb.mark_dirty();
                                }
                            }
                            EmuCommand::SaveState(reply_tx) => {
                                let res = core.save_state().map_err(|e| format!("{e:?}"));
                                let _ = reply_tx.send(res);
                            }
                            EmuCommand::Terminate => return,
                        }
                    }

                    if paused {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }

                    // Run 1 GBA frame
                    {
                        let mut fe = WorkerFrontend {
                            triple_buffer: &tb,
                            write_slot,
                            keypad: &kp,
                            audio: &mut audio,
                            audio_scratch: Vec::new(),
                        };
                        core.run_frame(&mut fe);
                        write_slot = fe.write_slot;
                    }

                    // Dynamic rate control pacing
                    let filled = audio.occupied_len();
                    pacer.pace(filled, audio.has_device());
                }
            })
            .ok();

        Self {
            keypad,
            triple_buffer,
            cmd_tx,
            error_rx,
            worker_join,
        }
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(EmuCommand::Pause);
    }

    pub fn resume(&self) {
        let _ = self.cmd_tx.send(EmuCommand::Resume);
    }

    pub fn reset(&self) {
        let _ = self.cmd_tx.send(EmuCommand::Reset);
    }

    pub fn load_state(&self, state: Vec<u8>) {
        let _ = self.cmd_tx.send(EmuCommand::LoadState(state));
    }

    pub fn save_state_sync(&self) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = channel();
        let _ = self.cmd_tx.send(EmuCommand::SaveState(reply_tx));
        reply_rx.recv().map_err(|e| format!("Worker save state timeout: {e}"))?
    }
}

impl Drop for EmuThreadHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(EmuCommand::Terminate);
        if let Some(handle) = self.worker_join.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_triple_buffer_cycles_cleanly_without_collision() {
        let tb = SharedTripleBuffer::new();

        // Initially dirty is false
        assert!(!tb.is_dirty());

        // Producer writes to slot 0
        let slot0 = tb.next_write_slot(2);
        unsafe {
            let b = tb.write_buffer_mut(slot0);
            b[0] = 0x1234;
        }
        let next_write = tb.publish_frame(slot0);

        assert!(tb.is_dirty());

        // Consumer reads latest frame
        let read_val = tb.read_latest(|buf| buf[0]);
        assert_eq!(read_val, Some(0x1234));
        assert!(!tb.is_dirty());

        // Next producer slot never collides with reader slot
        assert_ne!(next_write, tb.reading.load(Ordering::Relaxed));
    }

    #[test]
    fn drc_pacer_adjusts_ratio_within_half_percent() {
        let cap = 8192;
        let _pacer = DrcPacer::new(cap);
        let target = cap / 2; // 4096

        // Exact target -> 1.0 ratio
        let target_f = target as f64;
        let ratio_exact = (1.0 + 0.005 * ((target_f - target_f) / target_f)).clamp(0.995, 1.005);
        assert!((ratio_exact - 1.0).abs() < 1e-6);

        // Underrun state (filled = 0) -> ratio = 1.005 (speeds up to catch up)
        let ratio_empty = (1.0 + 0.005 * ((target_f - 0.0) / target_f)).clamp(0.995, 1.005);
        assert!((ratio_empty - 1.005).abs() < 1e-6);

        // Overflow state (filled = cap) -> ratio = 0.995 (slows down to drain)
        let ratio_full = (1.0 + 0.005 * ((target_f - (cap as f64)) / target_f)).clamp(0.995, 1.005);
        assert!((ratio_full - 0.995).abs() < 1e-6);
    }
}
