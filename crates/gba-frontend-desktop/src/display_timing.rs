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
}

