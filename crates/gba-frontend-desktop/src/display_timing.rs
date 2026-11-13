#[cfg(target_os = "windows")]
use std::collections::BTreeMap;

/// Exact native GBA refresh rate (`16_777_216 / 280_896` Hz ≈ `59.72750057 Hz`).
pub const NATIVE_GBA_HZ: f64 = 16_777_216.0 / 280_896.0;

#[derive(Clone, Debug)]
pub struct SyncRateDecision {
    pub effective_refresh_hz: Option<f64>,
    pub sync_60hz: bool,
    pub reason: String,
}

/// Returns `true` when the display refresh rate is cleanly within `±0.35 Hz` of a
/// `60 Hz` integer multiple (`60, 120, 180, 240, ...`) so snapping GBA pacing from
/// `59.7275 Hz` -> `60.000 Hz` (`+0.456%`, absorbed by +/-1% audio DRC) eliminates
/// periodic vblank beat repeats/judder. Non-multiples (`100 Hz`, `144 Hz`, `165 Hz`,
/// `75 Hz`) and sub-60 Hz modes (`30 Hz`, `48 Hz`) keep authentic `59.7275 Hz`.
pub fn should_snap_to_60hz(refresh_hz: f64) -> bool {
    if !refresh_hz.is_finite() || refresh_hz < 59.5 {
        return false;
    }
    let nearest_mult = (refresh_hz / 60.0).round().max(1.0);
    (refresh_hz - nearest_mult * 60.0).abs() <= 0.35
}

pub fn resolve_sync_decision(
    cli_override: Option<bool>,
    simulated_refresh_hz: Option<f64>,
) -> SyncRateDecision {
    let detected = detect_display_refresh_hz();
    let effective = simulated_refresh_hz.or(detected);

    if let Some(forced) = cli_override {
        let rate_str = effective
            .map(|hz| format!("{hz:.2} Hz"))
            .unwrap_or_else(|| "unknown Hz".to_string());
        let mode_str = if forced { "60.0 Hz DRC lock" } else { "native 59.7275 Hz" };
        return SyncRateDecision {
            effective_refresh_hz: effective,
            sync_60hz: forced,
            reason: format!("CLI override -> {mode_str} (display {rate_str})"),
        };
    }

    match effective {
        Some(hz) => {
            let snap = should_snap_to_60hz(hz);
            let source = if simulated_refresh_hz.is_some() {
                "simulated"
            } else {
                "detected"
            };
            let reason = if snap {
                let k = (hz / 60.0).round().max(1.0) as u32;
                format!(
                    "{source} {hz:.2} Hz (~{k}x 60 Hz) -> snapping to 60.000 Hz (+0.456% DRC) to avoid periodic vblank repeats"
                )
            } else {
                format!(
                    "{source} {hz:.2} Hz (non-60Hz multiple) -> running authentic {NATIVE_GBA_HZ:.4} Hz"
                )
            };
            SyncRateDecision {
                effective_refresh_hz: Some(hz),
                sync_60hz: snap,
                reason,
            }
        }
        None => SyncRateDecision {
            effective_refresh_hz: None,
            sync_60hz: false,
            reason: format!(
                "display refresh rate unavailable -> running authentic {NATIVE_GBA_HZ:.4} Hz"
            ),
        },
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DwmCompositionSnapshot {
    pub c_refresh: u64,
    pub qpc_vblank: u64,
    pub qpc_refresh_period: u64,
    pub c_frames_dropped: u64,
    pub c_frames_missed: u64,
    pub refresh_hz: f64,
}

#[cfg(target_os = "windows")]
mod win32 {
    use super::DwmCompositionSnapshot;
    use std::sync::OnceLock;

    #[repr(C, packed(1))]
    #[derive(Clone, Copy)]
    pub(super) struct UnsignedRatio {
        pub ui_numerator: u32,
        pub ui_denominator: u32,
    }

    // Windows SDK dwmapi.h wraps DWM_TIMING_INFO in #include <pshpack1.h>; total size = 292 bytes.
    #[repr(C, packed(1))]
    #[derive(Clone, Copy)]
    pub(super) struct DwmTimingInfo {
        pub cb_size: u32,
        pub rate_refresh: UnsignedRatio,
        pub qpc_refresh_period: u64,
        pub rate_compose: UnsignedRatio,
        pub qpc_vblank: u64,
        pub c_refresh: u64,
        pub c_dx_refresh: u32,
        pub qpc_compose: u64,
        pub c_frame: u64,
        pub c_dx_present: u32,
        pub c_refresh_frame: u64,
        pub c_frame_submitted: u64,
        pub c_dx_present_submitted: u32,
        pub c_frame_confirmed: u64,
        pub c_dx_present_confirmed: u32,
        pub c_refresh_confirmed: u64,
        pub c_dx_refresh_confirmed: u32,
        pub c_frames_late: u64,
        pub c_frames_outstanding: u32,
        pub c_frame_displayed: u64,
        pub qpc_frame_displayed: u64,
        pub c_refresh_frame_displayed: u64,
        pub c_frame_complete: u64,
        pub qpc_frame_complete: u64,
        pub c_frame_pending: u64,
        pub qpc_frame_pending: u64,
        pub c_frames_displayed: u64,
        pub c_frames_complete: u64,
        pub c_frames_pending: u64,
        pub c_frames_available: u64,
        pub c_frames_dropped: u64,
        pub c_frames_missed: u64,
        pub c_refresh_next_displayed: u64,
        pub c_refresh_next_presented: u64,
        pub c_refreshes_displayed: u64,
        pub c_refreshes_presented: u64,
        pub c_refresh_started: u64,
        pub c_pixels_received: u64,
        pub c_pixels_drawn: u64,
        pub c_buffers_empty: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PointL {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct DevModeW {
        dm_device_name: [u16; 32],
        dm_spec_version: u16,
        dm_driver_version: u16,
        dm_size: u16,
        dm_driver_extra: u16,
        dm_fields: u32,
        dm_position: PointL,
        dm_display_orientation: u32,
        dm_display_fixed_output: u32,
        dm_color: i16,
        dm_duplex: i16,
        dm_y_resolution: i16,
        dm_tt_option: i16,
        dm_collate: i16,
        dm_form_name: [u16; 32],
        dm_log_pixels: u16,
        dm_bits_per_pel: u32,
        dm_pels_width: u32,
        dm_pels_height: u32,
        dm_display_flags: u32,
        dm_display_frequency: u32,
        dm_icm_method: u32,
        dm_icm_intent: u32,
        dm_media_type: u32,
        dm_dither_type: u32,
        dm_reserved1: u32,
        dm_reserved2: u32,
        dm_panning_width: u32,
        dm_panning_height: u32,
    }

    const ENUM_CURRENT_SETTINGS: u32 = u32::MAX;

    type DwmGetTimingFn = unsafe extern "system" fn(isize, *mut DwmTimingInfo) -> i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(lp_lib_file_name: *const u8) -> *mut core::ffi::c_void;
        fn GetProcAddress(
            h_module: *mut core::ffi::c_void,
            lp_proc_name: *const u8,
        ) -> *const core::ffi::c_void;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumDisplaySettingsW(
            lpsz_device_name: *const u16,
            i_mode_num: u32,
            lp_dev_mode: *mut DevModeW,
        ) -> i32;
    }

    fn resolve_dwm_fn() -> Option<DwmGetTimingFn> {
        static FN_CACHE: OnceLock<Option<DwmGetTimingFn>> = OnceLock::new();
        *FN_CACHE.get_or_init(|| unsafe {
            let hmod = LoadLibraryA(b"dwmapi.dll\0".as_ptr());
            if hmod.is_null() {
                return None;
            }
            let proc = GetProcAddress(hmod, b"DwmGetCompositionTimingInfo\0".as_ptr());
            if proc.is_null() {
                None
            } else {
                Some(std::mem::transmute::<*const core::ffi::c_void, DwmGetTimingFn>(proc))
            }
        })
    }

    pub(super) fn query_dwm_timing() -> Option<DwmCompositionSnapshot> {
        let dwm_fn = resolve_dwm_fn()?;
        let mut info: DwmTimingInfo = unsafe { std::mem::zeroed() };
        info.cb_size = std::mem::size_of::<DwmTimingInfo>() as u32;
        debug_assert_eq!(std::mem::size_of::<DwmTimingInfo>(), 292);
        let hr = unsafe { dwm_fn(0, &mut info) };
        if hr != 0 {
            return None;
        }
        let num = info.rate_refresh.ui_numerator as f64;
        let den = info.rate_refresh.ui_denominator as f64;
        let refresh_hz = if den > 0.0 { num / den } else { 0.0 };
        Some(DwmCompositionSnapshot {
            c_refresh: info.c_refresh,
            qpc_vblank: info.qpc_vblank,
            qpc_refresh_period: info.qpc_refresh_period,
            c_frames_dropped: info.c_frames_dropped,
            c_frames_missed: info.c_frames_missed,
            refresh_hz,
        })
    }

    pub(super) fn query_enum_display_hz() -> Option<f64> {
        let mut dm: DevModeW = unsafe { std::mem::zeroed() };
        dm.dm_size = std::mem::size_of::<DevModeW>() as u16;
        let ok = unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut dm) };
        if ok != 0 && dm.dm_display_frequency > 1 {
            Some(dm.dm_display_frequency as f64)
        } else {
            None
        }
    }
}

pub fn query_dwm_timing() -> Option<DwmCompositionSnapshot> {
    #[cfg(target_os = "windows")]
    {
        win32::query_dwm_timing()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

pub fn detect_display_refresh_hz() -> Option<f64> {
    #[cfg(target_os = "windows")]
    {
        let enum_hz = win32::query_enum_display_hz();
        if let Some(dwm) = win32::query_dwm_timing()
            && dwm.refresh_hz > 1.0
        {
            if let Some(ehz) = enum_hz
                && (dwm.refresh_hz - ehz).abs() > 1.5
            {
                return Some(ehz);
            }
            return Some(dwm.refresh_hz);
        }
        enum_hz
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// Computes expected nominal compositor vblank bounds `(min_expected, max_expected)` per GBA
/// frame given `(display_refresh_hz, target_gba_hz)`.
/// For integer multiples (e.g. `240 / 60 = 4.0`, `60 / 60 = 1.0`, `120 / 60 = 2.0`),
/// nominal is a single exact vblank count `(k, k)`.
/// For fractional ratios (e.g. `100 / 59.7275 = 1.674`), normal pulldown alternates cleanly
/// between `floor(r) = 1` and `ceil(r) = 2`.
pub fn expected_vblank_bounds(display_refresh_hz: f64, target_gba_hz: f64) -> (u64, u64) {
    if !display_refresh_hz.is_finite()
        || !target_gba_hz.is_finite()
        || display_refresh_hz <= 0.0
        || target_gba_hz <= 0.0
    {
        return (1, 1);
    }
    let ratio = display_refresh_hz / target_gba_hz;
    let nearest = ratio.round();
    if (ratio - nearest).abs() < 0.025 && nearest >= 1.0 {
        let k = nearest as u64;
        (k, k)
    } else {
        let lo = ratio.floor().max(1.0) as u64;
        let hi = ratio.ceil().max(lo as f64) as u64;
        (lo, hi)
    }
}

/// Evaluates DWM compositor vblanks elapsed per presented unique GBA frame so real-window
/// telemetry detects on-screen vblank holds (repeats) and zero-vblank / compositor drops even
/// without administrative ETW PresentMon privileges.
#[derive(Clone, Debug)]
pub struct DwmScanoutTracker {
    pub display_refresh_hz: f64,
    pub expected_min_vblanks: u64,
    pub expected_max_vblanks: u64,
    last_present_c_refresh: Option<u64>,
    last_gba_c_refresh: Option<u64>,
    last_gba_qpc_vblank: Option<u64>,
    last_dropped: Option<u64>,
    last_missed: Option<u64>,
    pub unusual_long_vblank_frames: usize,
    pub zero_or_short_vblank_frames: usize,
    pub dwm_dropped_delta_total: u64,
    pub dwm_missed_delta_total: u64,
    #[cfg(target_os = "windows")]
    pub gba_vblank_span_hist: BTreeMap<u64, usize>,
    #[cfg(not(target_os = "windows"))]
    pub gba_vblank_span_hist: std::collections::BTreeMap<u64, usize>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DwmFrameSample {
    pub c_refresh: u64,
    pub vblanks_per_gba_frame: Option<u64>,
    pub dropped_delta: u64,
    pub unusual_vblank: bool,
}

impl DwmScanoutTracker {
    pub fn new(display_refresh_hz: Option<f64>, sync_60hz: bool) -> Self {
        let target_gba_hz = if sync_60hz { 60.0 } else { NATIVE_GBA_HZ };
        let initial = query_dwm_timing();
        let hz = display_refresh_hz
            .or_else(|| initial.map(|s| s.refresh_hz))
            .unwrap_or(60.0);
        let (min_v, max_v) = expected_vblank_bounds(hz, target_gba_hz);
        Self {
            display_refresh_hz: hz,
            expected_min_vblanks: min_v,
            expected_max_vblanks: max_v,
            last_present_c_refresh: initial.map(|s| s.c_refresh),
            last_gba_c_refresh: initial.map(|s| s.c_refresh),
            last_gba_qpc_vblank: initial.map(|s| s.qpc_vblank),
            last_dropped: initial.map(|s| s.c_frames_dropped),
            last_missed: initial.map(|s| s.c_frames_missed),
            unusual_long_vblank_frames: 0,
            zero_or_short_vblank_frames: 0,
            dwm_dropped_delta_total: 0,
            dwm_missed_delta_total: 0,
            gba_vblank_span_hist: Default::default(),
        }
    }

    pub fn sample_present(&mut self, frames_stepped: u32, steady: bool) -> Option<DwmFrameSample> {
        let snap = query_dwm_timing()?;
        let _ = self
            .last_present_c_refresh
            .map(|prev| snap.c_refresh.saturating_sub(prev));
        self.last_present_c_refresh = Some(snap.c_refresh);

        let dropped_delta = self
            .last_dropped
            .map(|prev| snap.c_frames_dropped.saturating_sub(prev))
            .unwrap_or(0);
        let missed_delta = self
            .last_missed
            .map(|prev| snap.c_frames_missed.saturating_sub(prev))
            .unwrap_or(0);
        self.last_dropped = Some(snap.c_frames_dropped);
        self.last_missed = Some(snap.c_frames_missed);

        if steady {
            self.dwm_dropped_delta_total = self.dwm_dropped_delta_total.saturating_add(dropped_delta);
            self.dwm_missed_delta_total = self.dwm_missed_delta_total.saturating_add(missed_delta);
        }

        let mut gba_span = None;
        let mut unusual = false;
        if frames_stepped > 0 {
            // Note: on Windows 11 DWM with idle-skipping/PSR on high-refresh panels, `cRefresh`
            // only increments per DWM composition wakeup rather than every physical monitor vblank,
            // whereas `(qpc_vblank - prev_qpc_vblank) / qpc_refresh_period` recovers exact physical
            // monitor vblank periods elapsed between DWM composition ticks.
            let span = if snap.qpc_refresh_period > 0 {
                self.last_gba_qpc_vblank.map(|prev_qpc| {
                    let delta = snap.qpc_vblank.saturating_sub(prev_qpc);
                    ((delta as f64) / (snap.qpc_refresh_period as f64)).round() as u64
                })
            } else {
                self.last_gba_c_refresh
                    .map(|prev_c| snap.c_refresh.saturating_sub(prev_c))
            };
            if let Some(span) = span {
                gba_span = Some(span);
                if steady {
                    *self.gba_vblank_span_hist.entry(span).or_insert(0) += 1;
                    if span > self.expected_max_vblanks {
                        self.unusual_long_vblank_frames += 1;
                        unusual = true;
                    } else if span < self.expected_min_vblanks {
                        self.zero_or_short_vblank_frames += 1;
                        unusual = true;
                    }
                }
            }
            self.last_gba_c_refresh = Some(snap.c_refresh);
            self.last_gba_qpc_vblank = Some(snap.qpc_vblank);
        }

        Some(DwmFrameSample {
            c_refresh: snap.c_refresh,
            vblanks_per_gba_frame: gba_span,
            dropped_delta,
            unusual_vblank: unusual,
        })
    }
}

#[cfg(target_os = "windows")]
pub fn qpc_frequency() -> Option<u64> {
    static FREQ: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let f = *FREQ.get_or_init(|| {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn QueryPerformanceFrequency(lp_frequency: *mut i64) -> i32;
        }
        let mut freq: i64 = 0;
        // SAFETY: Valid stack pointer for i64 frequency output.
        if unsafe { QueryPerformanceFrequency(&mut freq) } != 0 && freq > 0 {
            freq as u64
        } else {
            0
        }
    });
    (f > 0).then_some(f)
}

#[cfg(not(target_os = "windows"))]
pub fn qpc_frequency() -> Option<u64> {
    None
}

#[cfg(target_os = "windows")]
pub fn qpc_now() -> Option<u64> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(lp_performance_count: *mut i64) -> i32;
    }
    let mut count: i64 = 0;
    // SAFETY: Valid stack pointer for i64 counter output.
    if unsafe { QueryPerformanceCounter(&mut count) } != 0 && count > 0 {
        Some(count as u64)
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
pub fn qpc_now() -> Option<u64> {
    None
}

/// Sleeps cleanly (relying on an active high-resolution OS timer when available) then yields
/// briefly on QPC until `target_qpc` is reached, ignoring absurd future deadlines (>25 ms).
pub fn wait_until_qpc(target_qpc: u64) {
    let Some(freq) = qpc_frequency() else {
        return;
    };
    let Some(now) = qpc_now() else {
        return;
    };
    if now >= target_qpc {
        return;
    }
    let rem_secs = (target_qpc - now) as f64 / (freq as f64);
    if rem_secs > 0.025 {
        return;
    }
    if rem_secs > 0.0020 {
        std::thread::sleep(std::time::Duration::from_secs_f64(rem_secs - 0.0014));
    }
    while let Some(cur) = qpc_now() {
        if cur >= target_qpc {
            break;
        }
        std::hint::spin_loop();
    }
}

/// Phase-locks frame presentation to the middle (`target_phase = 0.50`) of a DWM refresh
/// interval using `DwmGetCompositionTimingInfo` (`qpc_vblank`, `qpc_refresh_period`) and QPC,
/// steering cadence gently within ±0.20% per frame so corrections fold seamlessly into audio DRC.
#[derive(Clone, Debug)]
pub struct VblankPhaseLock {
    enabled: bool,
    target_phase: f64,
    qpc_freq: u64,
    vblanks_per_frame: u64,
    target_finish_qpc: Option<u64>,
    ema_present_qpc: f64,
    ema_prep_qpc: f64,
    ema_phase_err_norm: f64,
    ema_step_qpc: f64,
    last_present_end_qpc: Option<u64>,
    drift_realign_count: u64,
    last_rate_offset_ppm: i32,
}

impl VblankPhaseLock {
    pub fn new(sync_60hz: bool, display_refresh_hz: Option<f64>) -> Self {
        Self::with_freq(sync_60hz, display_refresh_hz, qpc_frequency().unwrap_or(10_000_000))
    }

    pub fn with_freq(sync_60hz: bool, display_refresh_hz: Option<f64>, qpc_freq: u64) -> Self {
        let freq = qpc_freq.max(1);
        let refresh = display_refresh_hz.unwrap_or(60.0);
        let vblanks_per_frame = if sync_60hz && refresh >= 55.0 {
            ((refresh / 60.0).round() as u64).max(1)
        } else {
            1
        };
        let nominal_frame = (freq as f64) / 60.0;
        Self {
            enabled: sync_60hz && qpc_freq > 0,
            target_phase: 0.50,
            qpc_freq: freq,
            vblanks_per_frame,
            target_finish_qpc: None,
            ema_present_qpc: (freq as f64) * 0.0008, // 0.8 ms initial BitBlt/present estimate
            ema_prep_qpc: (freq as f64) * 0.0020,    // 2.0 ms initial filter+blit estimate
            ema_phase_err_norm: 0.0,
            ema_step_qpc: nominal_frame,
            last_present_end_qpc: None,
            drift_realign_count: 0,
            last_rate_offset_ppm: if sync_60hz {
                (((60.0 / NATIVE_GBA_HZ) - 1.0) * 1_000_000.0).round() as i32
            } else {
                0
            },
        }
    }

    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub fn qpc_freq(&self) -> u64 {
        self.qpc_freq
    }

    #[inline]
    #[allow(dead_code)]
    pub fn target_finish_qpc(&self) -> Option<u64> {
        self.target_finish_qpc
    }

    /// Expected start QPC timestamp so `present()` completes right around `target_finish_qpc`.
    #[inline]
    pub fn target_present_start_qpc(&self) -> Option<u64> {
        self.target_finish_qpc
            .map(|fin| fin.saturating_sub(self.ema_present_qpc.round() as u64))
    }

    /// Pre-present wake QPC timestamp allowing emulation + filter + blit to complete comfortably
    /// before `target_present_start_qpc`.
    #[inline]
    pub fn prework_wake_qpc(&self) -> Option<u64> {
        let start = self.target_present_start_qpc()?;
        let min_headroom = (self.qpc_freq as f64) * 0.0040; // >= 4.0 ms buffer for OS timer + prep
        let cap_headroom = (self.qpc_freq as f64) * 0.0115; // <= 11.5 ms so we still WaitUntil idle
        let lead = (self.ema_prep_qpc + (self.qpc_freq as f64) * 0.0025)
            .clamp(min_headroom, cap_headroom);
        Some(start.saturating_sub(lead.round() as u64))
    }

    #[inline]
    pub fn record_prep_ticks(&mut self, prep_qpc: u64) {
        let cap = (self.qpc_freq as f64) * 0.012;
        let clamped = (prep_qpc as f64).min(cap);
        self.ema_prep_qpc = self.ema_prep_qpc * 0.85 + clamped * 0.15;
    }

    #[inline]
    pub fn reset_schedule(&mut self) {
        self.target_finish_qpc = None;
        self.last_present_end_qpc = None;
        self.ema_phase_err_norm = 0.0;
    }

    #[inline]
    pub fn audio_rate_offset_ppm(&self) -> i32 {
        self.last_rate_offset_ppm
    }

    #[inline]
    #[allow(dead_code)]
    pub fn drift_realign_count(&self) -> u64 {
        self.drift_realign_count
    }

    /// Observes the completed present (`present_start_qpc..present_end_qpc`) and latest DWM timing
    /// snapshot, advancing `target_finish_qpc` toward `qpc_vblank + 0.50 * qpc_refresh_period`.
    pub fn observe_and_advance(
        &mut self,
        present_start_qpc: u64,
        present_end_qpc: u64,
        snap: Option<DwmCompositionSnapshot>,
    ) -> u64 {
        if !self.enabled {
            let step = ((self.qpc_freq as f64) / NATIVE_GBA_HZ).round() as u64;
            let next = present_end_qpc.saturating_add(step);
            self.target_finish_qpc = Some(next);
            self.last_rate_offset_ppm = 0;
            return next;
        }

        let Some(snap) = snap.filter(|s| s.qpc_refresh_period > 0) else {
            let step = ((self.qpc_freq as f64) / 60.0).round() as u64;
            let next = match self.target_finish_qpc {
                Some(prev) if prev + step > present_end_qpc + step / 4 => prev + step,
                _ => present_end_qpc.saturating_add(step),
            };
            self.target_finish_qpc = Some(next);
            self.last_rate_offset_ppm =
                (((60.0 / NATIVE_GBA_HZ) - 1.0) * 1_000_000.0).round() as i32;
            return next;
        };

        let p = snap.qpc_refresh_period.max(1);
        let refresh_hz = if snap.refresh_hz >= 55.0 {
            snap.refresh_hz
        } else {
            (self.qpc_freq as f64) / (p as f64)
        };
        if refresh_hz >= 55.0 {
            self.vblanks_per_frame = ((refresh_hz / 60.0).round() as u64).max(1);
        }
        let step_qpc = p.saturating_mul(self.vblanks_per_frame);
        let mid_offset = ((p as f64) * self.target_phase).round() as i128;

        // Track only the BitBlt/present submission latency so present completes at target_phase.
        let dur_ticks = present_end_qpc.saturating_sub(present_start_qpc) as f64;
        let max_present_lead = (p as f64) * if self.vblanks_per_frame == 1 { 0.45 } else { 0.55 };
        self.ema_present_qpc = (self.ema_present_qpc * 0.90 + dur_ticks * 0.10).min(max_present_lead);

        // Actual phase error of present completion within its DWM refresh interval [-0.5, +0.5).
        let end_mod = ((present_end_qpc as i128 - snap.qpc_vblank as i128)
            .rem_euclid(p as i128)) as f64
            / (p as f64);
        let end_err_norm = end_mod - self.target_phase;
        self.ema_phase_err_norm = self.ema_phase_err_norm * 0.92 + end_err_norm * 0.08;

        let min_headroom = (step_qpc / 2).max(p / 2);
        let snap_to_grid = |base_target: u64| -> u64 {
            let mut m = ((base_target as i128 - (snap.qpc_vblank as i128 + mid_offset)) as f64
                / (p as f64))
                .round() as i128;
            let mut cand = snap.qpc_vblank as i128 + m * (p as i128) + mid_offset;
            let floor = present_end_qpc as i128 + ((step_qpc as i128 * 3) / 4);
            while cand < floor {
                m += 1;
                cand = snap.qpc_vblank as i128 + m * (p as i128) + mid_offset;
            }
            cand.max(0) as u64
        };

        let next = match self.target_finish_qpc {
            None => snap_to_grid(present_end_qpc.saturating_add(step_qpc)),
            Some(prev_target) => {
                // Nominal target modulo p vs true DWM midpoint grid -> crystal drift term.
                let grid_mod = ((prev_target as i128 - snap.qpc_vblank as i128)
                    .rem_euclid(p as i128)) as f64
                    / (p as f64);
                let grid_err_norm = grid_mod - self.target_phase;
                // Blend DWM grid drift + smoothed actual present completion offset.
                let combined_err_norm = 0.70 * grid_err_norm + 0.30 * self.ema_phase_err_norm;
                let max_slew = (step_qpc as f64) * 0.0020; // ±0.20% per frame max (~3.4 cents)
                let delta_ticks =
                    (-(combined_err_norm * (p as f64)) * 0.10).clamp(-max_slew, max_slew);
                let cand = (prev_target as i128 + step_qpc as i128 + delta_ticks.round() as i128)
                    .max(0) as u64;
                if cand < present_end_qpc.saturating_add(min_headroom)
                    || cand > present_end_qpc.saturating_add(step_qpc * 2)
                {
                    self.drift_realign_count += 1;
                    snap_to_grid(present_end_qpc.saturating_add(step_qpc))
                } else {
                    cand
                }
            }
        };

        if let Some(prev) = self.target_finish_qpc {
            let applied_step = next.saturating_sub(prev) as f64;
            if applied_step > (step_qpc as f64) * 0.5 && applied_step < (step_qpc as f64) * 1.5 {
                self.ema_step_qpc = self.ema_step_qpc * 0.90 + applied_step * 0.10;
            }
        } else {
            self.ema_step_qpc = step_qpc as f64;
        }
        let eff_fps = (self.qpc_freq as f64) / self.ema_step_qpc.max(1.0);
        let ppm = (((eff_fps / NATIVE_GBA_HZ) - 1.0) * 1_000_000.0)
            .round()
            .clamp(-10_000.0, 10_000.0) as i32;
        self.last_rate_offset_ppm = ppm;
        self.last_present_end_qpc = Some(present_end_qpc);
        self.target_finish_qpc = Some(next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_rule_snaps_only_on_60hz_multiples() {
        assert!(should_snap_to_60hz(60.0));
        assert!(should_snap_to_60hz(60_000.0 / 1001.0)); // 59.94 Hz
        assert!(should_snap_to_60hz(120.0));
        assert!(should_snap_to_60hz(119.88));
        assert!(should_snap_to_60hz(180.0));
        assert!(should_snap_to_60hz(240.0));
        assert!(should_snap_to_60hz(239.998));

        // Non-multiples must stay on authentic 59.7275 Hz.
        assert!(!should_snap_to_60hz(100.0));
        assert!(!should_snap_to_60hz(144.0));
        assert!(!should_snap_to_60hz(165.0));
        assert!(!should_snap_to_60hz(75.0));
        assert!(!should_snap_to_60hz(48.0));
        assert!(!should_snap_to_60hz(30.0));
    }

    #[test]
    fn expected_vblanks_handles_integer_and_fractional_ratios() {
        assert_eq!(expected_vblank_bounds(60.0, 60.0), (1, 1));
        assert_eq!(expected_vblank_bounds(240.0, 60.0), (4, 4));
        assert_eq!(expected_vblank_bounds(100.0, NATIVE_GBA_HZ), (1, 2));
        assert_eq!(expected_vblank_bounds(144.0, NATIVE_GBA_HZ), (2, 3));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_display_timing_queries_succeed_unelevated() {
        let hz = detect_display_refresh_hz();
        assert!(hz.is_some(), "expected detected display refresh Hz on Windows");
        let dwm = query_dwm_timing();
        assert!(dwm.is_some(), "DwmGetCompositionTimingInfo must succeed with 292-byte pack(1) struct");
    }

    #[test]
    fn phase_lock_targets_midpoint_and_corrects_crystal_drift() {
        let freq = 10_000_000u64;
        let nominal_p = 166_666u64; // ~60.00 Hz
        let true_p_float = 166_666.0 * 1.0005; // +500 ppm clock drift on panel vs reported period
        let mut lock = VblankPhaseLock::with_freq(true, Some(60.0), freq);
        let base_vblank = 1_000_000_000u64;
        let bitblt_dur = 8_000u64; // 0.8 ms

        // Frame 0 starts near a vblank edge (0.12 phase)
        let mut end_qpc = base_vblank + 20_000;
        let snap0 = DwmCompositionSnapshot {
            c_refresh: 1,
            qpc_vblank: base_vblank,
            qpc_refresh_period: nominal_p,
            refresh_hz: 60.0,
            ..Default::default()
        };
        let mut next_target = lock.observe_and_advance(end_qpc - bitblt_dur, end_qpc, Some(snap0));

        let mut steady_phases = Vec::new();
        for frame in 1..400u64 {
            let vblank_k = base_vblank + ((frame as f64) * true_p_float).round() as u64;
            end_qpc = next_target;
            let snap = DwmCompositionSnapshot {
                c_refresh: frame + 1,
                qpc_vblank: vblank_k,
                qpc_refresh_period: nominal_p,
                refresh_hz: 60.0,
                ..Default::default()
            };
            next_target = lock.observe_and_advance(end_qpc - bitblt_dur, end_qpc, Some(snap));
            if frame >= 150 {
                let phase = ((end_qpc as i128 - vblank_k as i128).rem_euclid(nominal_p as i128))
                    as f64
                    / (nominal_p as f64);
                steady_phases.push(phase);
            }
        }
        let max_err = steady_phases
            .iter()
            .map(|&ph| (ph - 0.50).abs())
            .fold(0.0f64, f64::max);
        assert!(
            max_err < 0.05,
            "steady-state phase should hold within ±0.05 of 0.50 midpoint despite crystal drift, max_err={max_err:.4}"
        );
        let ppm = lock.audio_rate_offset_ppm();
        assert!(
            (4000..=5500).contains(&ppm),
            "expected audio rate offset around +4562 ppm at 60 Hz, got {ppm}"
        );
    }

    #[test]
    fn phase_lock_tracks_240hz_4v_cadence_and_recovers_from_stall() {
        let freq = 10_000_000u64;
        let p = 41_667u64; // 240 Hz
        let mut lock = VblankPhaseLock::with_freq(true, Some(240.0), freq);
        let base_v = 2_000_000_000u64;
        let mut target = lock.observe_and_advance(
            base_v + 1_000,
            base_v + 9_000,
            Some(DwmCompositionSnapshot {
                qpc_vblank: base_v,
                qpc_refresh_period: p,
                refresh_hz: 240.0,
                ..Default::default()
            }),
        );
        for f in 1..20u64 {
            let vb = base_v + f * 4 * p;
            let prev_end = target;
            target = lock.observe_and_advance(
                prev_end - 4_000,
                prev_end,
                Some(DwmCompositionSnapshot {
                    qpc_vblank: vb,
                    qpc_refresh_period: p,
                    refresh_hz: 240.0,
                    ..Default::default()
                }),
            );
            let step_vblanks = ((target as f64) - (prev_end as f64)) / (p as f64);
            assert!(
                (step_vblanks - 4.0).abs() < 0.05,
                "expected ~4v step at 240Hz, got {step_vblanks:.3}"
            );
        }
        // Inject a 35 ms stall (missed ~8 refreshes): should snap back onto 0.50 grid cleanly.
        let stall_end = target + 350_000;
        let stall_vb = stall_end - 8_000;
        let recovered = lock.observe_and_advance(
            stall_end - 4_000,
            stall_end,
            Some(DwmCompositionSnapshot {
                qpc_vblank: stall_vb,
                qpc_refresh_period: p,
                refresh_hz: 240.0,
                ..Default::default()
            }),
        );
        assert_eq!(lock.drift_realign_count(), 1);
        let rec_phase =
            ((recovered as i128 - stall_vb as i128).rem_euclid(p as i128)) as f64 / (p as f64);
        assert!(
            (rec_phase - 0.50).abs() < 0.02,
            "recovered target must snap right to 0.50 midpoint, got {rec_phase:.4}"
        );
    }
}

