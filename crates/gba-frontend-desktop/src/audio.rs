use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gba_core::SAMPLE_RATE_HZ;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

const RING_CAPACITY_SAMPLES: usize = 32_768; // 16,384 stereo frames (~500ms @ 32.768kHz max headroom)
const INITIAL_TARGET_FILL_SAMPLES: usize = 4_096; // 2,048 stereo frames (~62.5ms @ 32.768kHz steady target)
const MAX_TARGET_FILL_SAMPLES: usize = 16_384; // 8,192 stereo frames (~250ms adaptive upper bound)
const ADAPTIVE_TARGET_STEP_SAMPLES: usize = 2_048; // +31.25ms headroom per underrun event
const PLAY_RING_CAPACITY_F32: usize = 4_096; // 2,048 stereo host frames lock-free stage for callback
const PLAY_TARGET_F32: usize = 1_024; // 512 stereo host frames (~10.7ms @ 48kHz)
const PLAY_IDLE_FLOOR_F32: usize = 192; // 96 stereo host frames (~2.0ms @ 48kHz) safety floor

#[cfg(windows)]
pub(crate) mod win_mmcss {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::OnceLock;

    const AVRT_PRIORITY_CRITICAL: i32 = 2;
    const AVRT_PRIORITY_HIGH: i32 = 1;
    const THREAD_PRIORITY_ABOVE_NORMAL: i32 = 1;
    const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
    const PRO_AUDIO_TASK: [u16; 10] = [
        b'P' as u16,
        b'r' as u16,
        b'o' as u16,
        b' ' as u16,
        b'A' as u16,
        b'u' as u16,
        b'd' as u16,
        b'i' as u16,
        b'o' as u16,
        0,
    ];
    const GAMES_TASK: [u16; 6] = [
        b'G' as u16,
        b'a' as u16,
        b'm' as u16,
        b'e' as u16,
        b's' as u16,
        0,
    ];
    const AVRT_DLL: [u16; 9] = [
        b'a' as u16,
        b'v' as u16,
        b'r' as u16,
        b't' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *const c_void;
        fn GetCurrentThread() -> *mut c_void;
        fn GetThreadPriority(h_thread: *mut c_void) -> i32;
        fn SetThreadPriority(h_thread: *mut c_void, n_priority: i32) -> i32;
    }

    type FnAvSetMmThreadCharacteristicsW =
        unsafe extern "system" fn(task_name: *const u16, task_index: *mut u32) -> *mut c_void;
    type FnAvSetMmThreadPriority =
        unsafe extern "system" fn(avrt_handle: *mut c_void, priority: i32) -> i32;
    type FnAvRevertMmThreadCharacteristics =
        unsafe extern "system" fn(avrt_handle: *mut c_void) -> i32;

    struct AvrtApi {
        set_char: FnAvSetMmThreadCharacteristicsW,
        set_prio: FnAvSetMmThreadPriority,
        revert: FnAvRevertMmThreadCharacteristics,
    }

    // Function pointers resolved from system `avrt.dll` stay valid process-wide.
    unsafe impl Sync for AvrtApi {}
    unsafe impl Send for AvrtApi {}

    static AVRT_API: OnceLock<Option<AvrtApi>> = OnceLock::new();

    fn load_avrt() -> Option<&'static AvrtApi> {
        AVRT_API
            .get_or_init(|| unsafe {
                let module = LoadLibraryW(AVRT_DLL.as_ptr());
                if module.is_null() {
                    return None;
                }
                let p_set_char = GetProcAddress(module, b"AvSetMmThreadCharacteristicsW\0".as_ptr());
                let p_set_prio = GetProcAddress(module, b"AvSetMmThreadPriority\0".as_ptr());
                let p_revert = GetProcAddress(module, b"AvRevertMmThreadCharacteristics\0".as_ptr());
                if p_set_char.is_null() || p_set_prio.is_null() || p_revert.is_null() {
                    return None;
                }
                Some(AvrtApi {
                    set_char: std::mem::transmute::<*const c_void, FnAvSetMmThreadCharacteristicsW>(
                        p_set_char,
                    ),
                    set_prio: std::mem::transmute::<*const c_void, FnAvSetMmThreadPriority>(
                        p_set_prio,
                    ),
                    revert: std::mem::transmute::<*const c_void, FnAvRevertMmThreadCharacteristics>(
                        p_revert,
                    ),
                })
            })
            .as_ref()
    }

    pub struct MmcssGuard {
        handle: *mut c_void,
        prev_thread_priority: Option<i32>,
    }

    // Only owned and dropped on the thread where created (`!Sync` via handle).
    unsafe impl Send for MmcssGuard {}

    impl MmcssGuard {
        fn enter_task(task: &[u16], avrt_prio: i32, fallback_prio: i32) -> Self {
            // SAFETY: Valid null-terminated UTF-16 task name and dynamically loaded `avrt.dll`.
            unsafe {
                let mut handle = null_mut();
                if let Some(api) = load_avrt() {
                    let mut task_index = 0u32;
                    handle = (api.set_char)(task.as_ptr(), &mut task_index);
                    if !handle.is_null() {
                        let _ = (api.set_prio)(handle, avrt_prio);
                    }
                }
                let prev_prio = GetThreadPriority(GetCurrentThread());
                let prev_thread_priority = if SetThreadPriority(GetCurrentThread(), fallback_prio) != 0 {
                    Some(prev_prio)
                } else {
                    None
                };
                Self {
                    handle,
                    prev_thread_priority,
                }
            }
        }

        pub fn enter_pro_audio() -> Self {
            Self::enter_task(
                &PRO_AUDIO_TASK,
                AVRT_PRIORITY_CRITICAL,
                THREAD_PRIORITY_TIME_CRITICAL,
            )
        }

        /// Elevates the emulation thread via MMCSS `"Games"` + `THREAD_PRIORITY_ABOVE_NORMAL`.
        pub fn enter_games() -> Self {
            Self::enter_task(
                &GAMES_TASK,
                AVRT_PRIORITY_HIGH,
                THREAD_PRIORITY_ABOVE_NORMAL,
            )
        }

        pub fn is_mmcss_active(&self) -> bool {
            !self.handle.is_null()
        }
    }

    pub fn current_thread_priority() -> i32 {
        // SAFETY: `GetCurrentThread()` pseudo-handle is always valid on the calling thread.
        unsafe { GetThreadPriority(GetCurrentThread()) }
    }

    impl Drop for MmcssGuard {
        fn drop(&mut self) {
            if let Some(prev) = self.prev_thread_priority.take() {
                // SAFETY: Restores previous Win32 thread priority on owning thread.
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), prev);
                }
            }
            if !self.handle.is_null()
                && let Some(api) = load_avrt()
            {
                // SAFETY: Reverts MMCSS task handle obtained from `AvSetMmThreadCharacteristicsW`.
                unsafe {
                    let _ = (api.revert)(self.handle);
                    self.handle = null_mut();
                }
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) mod win_mmcss {
    pub struct MmcssGuard;
    impl MmcssGuard {
        pub fn enter_pro_audio() -> Self {
            Self
        }
        pub fn enter_games() -> Self {
            Self
        }
        pub fn is_mmcss_active(&self) -> bool {
            false
        }
    }
    pub fn current_thread_priority() -> i32 {
        0
    }
}

const SINC_TAPS: usize = 8;
const SINC_PHASES: usize = 64;

struct SincKernel {
    rows: [[f32; SINC_TAPS]; SINC_PHASES + 1],
}

impl SincKernel {
    fn new(base_ratio: f64) -> Self {
        let mut rows = [[0.0f32; SINC_TAPS]; SINC_PHASES + 1];
        // Scale low-pass cutoff down when downsampling (e.g. 48kHz -> 44.1kHz) to prevent aliasing.
        let cutoff = if base_ratio > 1.0 {
            (0.46 / base_ratio).max(0.20)
        } else {
            0.46
        };
        let pi = std::f64::consts::PI;
        let two_pi = 2.0 * pi;
        let half_taps = (SINC_TAPS / 2) as f64;

        for (p, row) in rows.iter_mut().enumerate() {
            let frac = (p as f64) / (SINC_PHASES as f64);
            let center = (half_taps - 1.0) + frac;
            let mut weights = [0.0f64; SINC_TAPS];
            let mut sum = 0.0f64;

            for (k, w_out) in weights.iter_mut().enumerate() {
                let x = (k as f64) - center;
                let r = (x + half_taps) / (SINC_TAPS as f64);
                let win = if !(0.0..=1.0).contains(&r) {
                    0.0
                } else {
                    0.42 - 0.5 * (two_pi * r).cos() + 0.08 * (2.0 * two_pi * r).cos()
                };
                let sinc = if x.abs() < 1e-12 {
                    2.0 * cutoff
                } else {
                    (two_pi * cutoff * x).sin() / (pi * x)
                };
                let val = win * sinc;
                *w_out = val;
                sum += val;
            }

            let inv_sum = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
            for (dst, &w) in row.iter_mut().zip(weights.iter()) {
                *dst = (w * inv_sum) as f32;
            }
        }

        Self { rows }
    }

    #[inline]
    fn interpolate(
        &self,
        phase: f64,
        hist_l: &[f32; SINC_TAPS],
        hist_r: &[f32; SINC_TAPS],
        hist_idx: usize,
    ) -> (f32, f32) {
        let scaled = phase.clamp(0.0, 0.999_999) * (SINC_PHASES as f64);
        let p = (scaled as usize).min(SINC_PHASES - 1);
        let frac = (scaled - (p as f64)) as f32;
        let r0 = &self.rows[p];
        let r1 = &self.rows[p + 1];

        let mut acc_l = 0.0f32;
        let mut acc_r = 0.0f32;
        for k in 0..SINC_TAPS {
            let w = r0[k] + (r1[k] - r0[k]) * frac;
            let idx = (hist_idx + k) & (SINC_TAPS - 1);
            acc_l += hist_l[idx] * w;
            acc_r += hist_r[idx] * w;
        }
        (acc_l, acc_r)
    }
}

struct AudioConsumer {
    cons: HeapCons<i16>,
    muted: Arc<AtomicBool>,
    base_ratio: f64,
    avg_fill: f64,
    phase: f64,
    hist_idx: usize,
    hist_l: [f32; SINC_TAPS],
    hist_r: [f32; SINC_TAPS],
    kernel: SincKernel,
    mute_gain: f32,
    underruns: Arc<AtomicU64>,
    #[cfg_attr(not(test), allow(dead_code))]
    occupancy: Arc<AtomicUsize>,
    target_fill: Arc<AtomicUsize>,
    was_underrunning: bool,
}

impl AudioConsumer {
    fn new(
        cons: HeapCons<i16>,
        muted: Arc<AtomicBool>,
        out_rate: f64,
        underruns: Arc<AtomicU64>,
        occupancy: Arc<AtomicUsize>,
        target_fill: Arc<AtomicUsize>,
    ) -> Self {
        let base_ratio = (SAMPLE_RATE_HZ as f64) / out_rate.max(8000.0);
        let start_muted = muted.load(Ordering::Relaxed);
        let initial_target = target_fill.load(Ordering::Relaxed);
        Self {
            cons,
            muted,
            base_ratio,
            avg_fill: initial_target as f64,
            phase: 0.0,
            hist_idx: 0,
            hist_l: [0.0; SINC_TAPS],
            hist_r: [0.0; SINC_TAPS],
            kernel: SincKernel::new(base_ratio),
            mute_gain: if start_muted { 0.0 } else { 1.0 },
            underruns,
            occupancy,
            target_fill,
            was_underrunning: true,
        }
    }

    #[inline]
    fn advance_input_sample(&mut self) {
        let (val_l, val_r) = if self.cons.occupied_len() >= 2
            && let (Some(l), Some(r)) = (self.cons.try_pop(), self.cons.try_pop())
        {
            self.was_underrunning = false;
            ((l as f32) / 32768.0, (r as f32) / 32768.0)
        } else {
            if !self.was_underrunning {
                self.underruns.fetch_add(1, Ordering::Relaxed);
                self.was_underrunning = true;
                let _ = self.target_fill.try_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |cur| {
                        let next =
                            (cur + ADAPTIVE_TARGET_STEP_SAMPLES).min(MAX_TARGET_FILL_SAMPLES);
                        (next > cur).then_some(next)
                    },
                );
            }
            // Smooth exponential decay on underrun (0.985 factor) prevents speaker pops.
            let newest_idx = (self.hist_idx + SINC_TAPS - 1) & (SINC_TAPS - 1);
            let prev_l = self.hist_l[newest_idx] * 0.985;
            let prev_r = self.hist_r[newest_idx] * 0.985;
            (
                if prev_l.abs() < 1e-6 { 0.0 } else { prev_l },
                if prev_r.abs() < 1e-6 { 0.0 } else { prev_r },
            )
        };
        self.hist_l[self.hist_idx] = val_l;
        self.hist_r[self.hist_idx] = val_r;
        self.hist_idx = (self.hist_idx + 1) & (SINC_TAPS - 1);
    }

    #[inline]
    #[cfg(test)]
    fn step_frame(&mut self) -> (f32, f32) {
        let fill = self.cons.occupied_len();
        self.occupancy.store(fill, Ordering::Relaxed);
        self.step_frame_with_fill(fill)
    }

    #[inline]
    fn step_frame_with_fill(&mut self, fill: usize) -> (f32, f32) {
        let fill_f = fill as f64;
        let target = self.target_fill.load(Ordering::Relaxed).max(1) as f64;
        // Smooth fill-level EMA suppresses callback-burst pitch wobble while locking latency.
        self.avg_fill += 0.0005 * (fill_f - self.avg_fill);
        let fill_delta = ((self.avg_fill - target) / target).clamp(-1.0, 1.0);
        // Dynamic Rate Control (DRC): skew input step by up to +/-1.0% keeping ring buffer centered at target_fill.
        let drc_adjust = (fill_delta * 0.010).clamp(-0.010, 0.010);
        let r_dynamic = self.base_ratio * (1.0 + drc_adjust);

        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.advance_input_sample();
        }

        let (l, r) = self
            .kernel
            .interpolate(self.phase, &self.hist_l, &self.hist_r, self.hist_idx);
        self.phase += r_dynamic;

        let target_gain = if self.muted.load(Ordering::Relaxed) {
            0.0f32
        } else {
            1.0f32
        };
        if (self.mute_gain - target_gain).abs() <= 0.005 {
            self.mute_gain = target_gain;
        } else if self.mute_gain < target_gain {
            self.mute_gain += 0.005;
        } else {
            self.mute_gain -= 0.005;
        }

        let out_l = (l * self.mute_gain).clamp(-1.0, 1.0);
        let out_r = (r * self.mute_gain).clamp(-1.0, 1.0);
        (out_l, out_r)
    }

    #[cfg(test)]
    fn fill_f32(&mut self, data: &mut [f32], channels: usize) {
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.step_frame();
            if channels >= 2 {
                frame[0] = l;
                frame[1] = r;
                for ch in &mut frame[2..] {
                    *ch = 0.0;
                }
            } else if !frame.is_empty() {
                frame[0] = (l + r) * 0.5;
            }
        }
    }
}

struct PlayConsumer {
    cons: HeapCons<f32>,
    last_l: f32,
    last_r: f32,
    wake_worker: Option<Thread>,
    mmcss: Option<win_mmcss::MmcssGuard>,
}

impl PlayConsumer {
    fn new(cons: HeapCons<f32>, wake_worker: Option<Thread>) -> Self {
        Self {
            cons,
            last_l: 0.0,
            last_r: 0.0,
            wake_worker,
            mmcss: None,
        }
    }

    #[inline]
    fn ensure_mmcss(&mut self) {
        if self.mmcss.is_none() {
            self.mmcss = Some(win_mmcss::MmcssGuard::enter_pro_audio());
        }
    }

    #[inline]
    fn next_stereo(&mut self) -> (f32, f32) {
        if self.cons.occupied_len() >= 2
            && let (Some(l), Some(r)) = (self.cons.try_pop(), self.cons.try_pop())
        {
            self.last_l = l;
            self.last_r = r;
            if self.cons.occupied_len() <= PLAY_IDLE_FLOOR_F32
                && let Some(t) = &self.wake_worker
            {
                t.unpark();
            }
            (l, r)
        } else {
            self.last_l *= 0.99;
            self.last_r *= 0.99;
            if let Some(t) = &self.wake_worker {
                t.unpark();
            }
            (self.last_l, self.last_r)
        }
    }

    fn fill_f32(&mut self, data: &mut [f32], channels: usize) {
        self.ensure_mmcss();
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.next_stereo();
            if channels >= 2 {
                frame[0] = l;
                frame[1] = r;
                for ch in &mut frame[2..] {
                    *ch = 0.0;
                }
            } else if !frame.is_empty() {
                frame[0] = (l + r) * 0.5;
            }
        }
    }

    fn fill_i16(&mut self, data: &mut [i16], channels: usize) {
        self.ensure_mmcss();
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.next_stereo();
            let il = (l * 32767.0) as i16;
            let ir = (r * 32767.0) as i16;
            if channels >= 2 {
                frame[0] = il;
                frame[1] = ir;
                for ch in &mut frame[2..] {
                    *ch = 0;
                }
            } else if !frame.is_empty() {
                frame[0] = ((il as i32 + ir as i32) / 2) as i16;
            }
        }
    }

    fn fill_u16(&mut self, data: &mut [u16], channels: usize) {
        self.ensure_mmcss();
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.next_stereo();
            let ul = ((l * 32767.0) as i32 + 32768).clamp(0, 65535) as u16;
            let ur = ((r * 32767.0) as i32 + 32768).clamp(0, 65535) as u16;
            if channels >= 2 {
                frame[0] = ul;
                frame[1] = ur;
                for ch in &mut frame[2..] {
                    *ch = 32768;
                }
            } else if !frame.is_empty() {
                frame[0] = ((ul as u32 + ur as u32) / 2) as u16;
            }
        }
    }
}

struct AudioWorkerHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for AudioWorkerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

pub struct AudioProducer {
    prod: HeapProd<i16>,
    play_gba_equiv: Arc<AtomicUsize>,
    target_fill: Arc<AtomicUsize>,
    wake_worker: Option<Thread>,
    primed: bool,
    #[allow(dead_code)]
    has_device: bool,
}

#[allow(dead_code)]
impl AudioProducer {
    #[inline]
    pub fn occupied_len(&self) -> usize {
        self.prod.occupied_len() + self.play_gba_equiv.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        RING_CAPACITY_SAMPLES
    }

    #[inline]
    pub fn target_fill(&self) -> usize {
        self.target_fill.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn has_device(&self) -> bool {
        self.has_device
    }

    /// Pushes interleaved stereo i16 samples lock-free, filling available ring buffer capacity.
    pub fn push_interleaved(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        if self.has_device && !self.primed {
            self.primed = true;
            // Seed target silence right when emulation emits frame 0 so prewarm/menu idle
            // cannot drain startup headroom beforehand.
            let target = self
                .target_fill
                .load(Ordering::Relaxed)
                .min(INITIAL_TARGET_FILL_SAMPLES)
                & !1;
            let prefill_len = target.min(self.prod.vacant_len() & !1);
            if prefill_len > 0 {
                let prefill = [0i16; INITIAL_TARGET_FILL_SAMPLES];
                let _ = self.prod.push_slice(&prefill[..prefill_len]);
            }
        }
        let vacant = self.prod.vacant_len() & !1;
        let to_push = (samples.len() & !1).min(vacant);
        if to_push > 0 {
            let _ = self.prod.push_slice(&samples[..to_push]);
            if let Some(t) = &self.wake_worker {
                t.unpark();
            }
        }
    }
}

pub struct AudioOutput {
    producer: Option<AudioProducer>,
    muted: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
    occupancy: Arc<AtomicUsize>,
    target_fill: Arc<AtomicUsize>,
    // Drop `_stream` before `_worker` so the OS audio callback stops prior to worker shutdown.
    _stream: Option<cpal::Stream>,
    _worker: Option<AudioWorkerHandle>,
    host_sample_rate: u32,
    #[allow(dead_code)]
    has_device: bool,
}

impl AudioOutput {
    pub fn start() -> Self {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (prod, cons) = rb.split();
        let muted = Arc::new(AtomicBool::new(false));
        let underruns = Arc::new(AtomicU64::new(0));
        let occupancy = Arc::new(AtomicUsize::new(0));
        let play_gba_equiv = Arc::new(AtomicUsize::new(0));
        let target_fill = Arc::new(AtomicUsize::new(INITIAL_TARGET_FILL_SAMPLES));

        let (stream, worker, wake_worker, host_sample_rate) = Self::init_cpal(
            cons,
            muted.clone(),
            underruns.clone(),
            occupancy.clone(),
            play_gba_equiv.clone(),
            target_fill.clone(),
        );
        let has_device = stream.is_some();
        Self {
            producer: Some(AudioProducer {
                prod,
                play_gba_equiv,
                target_fill: target_fill.clone(),
                wake_worker,
                primed: false,
                has_device,
            }),
            muted,
            underruns,
            occupancy,
            target_fill,
            _stream: stream,
            _worker: worker,
            host_sample_rate,
            has_device,
        }
    }

    #[cfg(test)]
    pub fn dummy_for_tests() -> Self {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (prod, _cons) = rb.split();
        let target_fill = Arc::new(AtomicUsize::new(INITIAL_TARGET_FILL_SAMPLES));
        Self {
            producer: Some(AudioProducer {
                prod,
                play_gba_equiv: Arc::new(AtomicUsize::new(0)),
                target_fill: target_fill.clone(),
                wake_worker: None,
                primed: false,
                has_device: false,
            }),
            muted: Arc::new(AtomicBool::new(false)),
            underruns: Arc::new(AtomicU64::new(0)),
            occupancy: Arc::new(AtomicUsize::new(0)),
            target_fill,
            _stream: None,
            _worker: None,
            host_sample_rate: 48_000,
            has_device: false,
        }
    }

    #[allow(dead_code)]
    pub fn take_producer(&mut self) -> Option<AudioProducer> {
        self.producer.take()
    }

    pub fn host_sample_rate(&self) -> u32 {
        self.host_sample_rate
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    #[inline]
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn reset_underruns(&self) {
        self.underruns.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn occupancy_percent(&self) -> f32 {
        let occ = self.occupancy.load(Ordering::Relaxed);
        let scale = (self.target_fill.load(Ordering::Relaxed).max(1) * 2).min(RING_CAPACITY_SAMPLES);
        (occ as f32 / scale as f32) * 100.0
    }

    #[inline]
    #[allow(dead_code)]
    pub fn occupied_len(&self) -> usize {
        if let Some(prod) = &self.producer {
            prod.occupied_len()
        } else {
            self.occupancy.load(Ordering::Relaxed)
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        RING_CAPACITY_SAMPLES
    }

    #[inline]
    #[allow(dead_code)]
    pub fn target_fill(&self) -> usize {
        self.target_fill.load(Ordering::Relaxed)
    }

    #[inline]
    #[allow(dead_code)]
    pub fn has_device(&self) -> bool {
        self.has_device
    }

    /// Pushes interleaved stereo i16 samples while keeping fast-forward latency bounded lock-free.
    pub fn push_interleaved(&mut self, samples: &[i16]) {
        if let Some(prod) = &mut self.producer {
            prod.push_interleaved(samples);
        }
    }

    fn init_cpal(
        cons: HeapCons<i16>,
        muted: Arc<AtomicBool>,
        underruns: Arc<AtomicU64>,
        occupancy: Arc<AtomicUsize>,
        play_gba_equiv: Arc<AtomicUsize>,
        target_fill: Arc<AtomicUsize>,
    ) -> (
        Option<cpal::Stream>,
        Option<AudioWorkerHandle>,
        Option<Thread>,
        u32,
    ) {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return (None, None, None, 48_000);
        };
        let Ok(config) = device.default_output_config() else {
            return (None, None, None, 48_000);
        };
        let sample_rate = config.sample_rate().0;
        let out_rate = sample_rate as f64;
        let channels = config.channels() as usize;
        let err_fn = |err| log::warn!("cpal output stream error: {err}");

        let play_rb = HeapRb::<f32>::new(PLAY_RING_CAPACITY_F32);
        let (mut play_prod, play_cons) = play_rb.split();

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let worker_occupancy = occupancy.clone();
        let worker_join = thread::Builder::new()
            .name("gba-audio".to_string())
            .spawn(move || {
                let _mmcss = win_mmcss::MmcssGuard::enter_pro_audio();
                let mut consumer = AudioConsumer::new(
                    cons,
                    muted,
                    out_rate,
                    underruns,
                    occupancy,
                    target_fill,
                );
                while !stop_thread.load(Ordering::Acquire) {
                    let mut out_occ = play_prod.occupied_len();
                    let gba_factor = consumer.base_ratio * 2.0;
                    loop {
                        let in_avail = consumer.cons.occupied_len();
                        let gba_in_out = (((out_occ / 2) as f64) * gba_factor).round() as usize;
                        let total_occ = in_avail + gba_in_out;
                        let want_more = if in_avail >= 2 {
                            out_occ < PLAY_TARGET_F32
                        } else {
                            out_occ < PLAY_IDLE_FLOOR_F32
                        };
                        if !want_more || play_prod.vacant_len() < 2 {
                            play_gba_equiv.store(gba_in_out, Ordering::Relaxed);
                            worker_occupancy.store(total_occ, Ordering::Relaxed);
                            break;
                        }
                        let (l, r) = consumer.step_frame_with_fill(total_occ);
                        let _ = play_prod.push_slice(&[l, r]);
                        out_occ += 2;
                    }
                    thread::park_timeout(Duration::from_millis(2));
                }
            })
            .ok();

        let Some(join) = worker_join else {
            return (None, None, None, 48_000);
        };
        let wake_worker = join.thread().clone();
        let worker = AudioWorkerHandle {
            stop,
            join: Some(join),
        };

        let mut play_cons = PlayConsumer::new(play_cons, Some(wake_worker.clone()));
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &config.into(),
                    move |data: &mut [f32], _| play_cons.fill_f32(data, channels),
                    err_fn,
                    None,
                )
                .ok(),
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    &config.into(),
                    move |data: &mut [i16], _| play_cons.fill_i16(data, channels),
                    err_fn,
                    None,
                )
                .ok(),
            cpal::SampleFormat::U16 => device
                .build_output_stream(
                    &config.into(),
                    move |data: &mut [u16], _| play_cons.fill_u16(data, channels),
                    err_fn,
                    None,
                )
                .ok(),
            _ => None,
        };

        if let Some(s) = &stream {
            let _ = s.play();
            (stream, Some(worker), Some(wake_worker), sample_rate)
        } else {
            drop(worker);
            (None, None, None, 48_000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinc_kernel_preserves_unity_dc_gain() {
        for &ratio in &[0.5, 1.0, 48_000.0 / 44_100.0] {
            let kernel = SincKernel::new(ratio);
            let hist_l = [0.75f32; SINC_TAPS];
            let hist_r = [-0.25f32; SINC_TAPS];
            for step in 0..100 {
                let phase = (step as f64) / 100.0;
                let (l, r) = kernel.interpolate(phase, &hist_l, &hist_r, 0);
                assert!((l - 0.75).abs() < 1e-5, "DC gain error L: {l}");
                assert!((r + 0.25).abs() < 1e-5, "DC gain error R: {r}");
            }
        }
    }

    #[test]
    fn resampler_tracks_sine_wave_and_fades_underrun() {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (mut prod, cons) = rb.split();
        let muted = Arc::new(AtomicBool::new(false));
        let underruns = Arc::new(AtomicU64::new(0));
        let occupancy = Arc::new(AtomicUsize::new(0));
        let target_fill = Arc::new(AtomicUsize::new(INITIAL_TARGET_FILL_SAMPLES));
        let mut consumer = AudioConsumer::new(
            cons,
            muted.clone(),
            44_100.0,
            underruns.clone(),
            occupancy,
            target_fill.clone(),
        );

        // Push 100ms of 1 kHz stereo sine wave at SAMPLE_RATE_HZ
        let total_frames = (SAMPLE_RATE_HZ as usize) / 10;
        let mut input = Vec::with_capacity(total_frames * 2);
        for i in 0..total_frames {
            let theta = 2.0 * std::f64::consts::PI * 1000.0 * (i as f64) / (SAMPLE_RATE_HZ as f64);
            let sl = (theta.sin() * 16_000.0) as i16;
            let sr = (theta.cos() * 16_000.0) as i16;
            input.push(sl);
            input.push(sr);
        }
        let _ = prod.push_slice(&input);

        let mut out = vec![0.0f32; 4096 * 2];
        consumer.fill_f32(&mut out, 2);
        let peak = out.iter().fold(0.0f32, |acc, &v| acc.max(v.abs()));
        assert!(
            peak > 0.45 && peak < 0.55,
            "unexpected resampled sine peak: {peak}"
        );

        // Drain rest and verify underrun decays cleanly to zero AND expands adaptive target fill
        let mut tail = vec![0.0f32; 4096 * 2];
        consumer.fill_f32(&mut tail, 2);
        consumer.fill_f32(&mut tail, 2);
        assert!(
            underruns.load(Ordering::Relaxed) >= 1,
            "draining ring should count an underrun"
        );
        assert!(
            target_fill.load(Ordering::Relaxed) > INITIAL_TARGET_FILL_SAMPLES,
            "underrun should grow adaptive target_fill headroom"
        );
        assert!(
            tail.iter().rev().take(256).all(|&v| v.abs() < 1e-4),
            "underrun fade should settle cleanly near zero"
        );
    }
}
