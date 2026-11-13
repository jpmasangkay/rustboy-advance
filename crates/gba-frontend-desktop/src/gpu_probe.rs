use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::config::{AppConfig, BlockedGpuAdapterEntry, VerifiedGpuAdapterEntry};
use crate::headless::CliConfig;
use crate::shaders::{discover_software_cpu_adapter_with_backends, select_preferred_adapter};

pub const DEFAULT_GPU_PROBE_TIMEOUT_MS: u64 = 7000;
pub const RETRY_GPU_PROBE_TIMEOUT_MS: u64 = 20_000;
pub const GPU_BLOCKLIST_FAILURE_THRESHOLD: u32 = 2;
pub const GPU_BLOCKLIST_TTL_SECS: u64 = 30 * 24 * 60 * 60;
const ADAPTER_ENUM_SENTINEL_KEY: &str = "adapter-enum";
const PREFIX_ADAPTER: &str = "GPU_PROBE_ADAPTER:";
const PREFIX_STAGE: &str = "GPU_PROBE_STAGE:";
const PREFIX_OK: &str = "GPU_PROBE_OK:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeFailStage {
    Device,
    Surface,
    Hang,
}

impl ProbeFailStage {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "device" => Ok(Self::Device),
            "surface" => Ok(Self::Surface),
            "hang" => Ok(Self::Hang),
            other => Err(format!(
                "invalid --probe-fail-stage `{other}`; expected device, surface, or hang"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Surface => "surface",
            Self::Hang => "hang",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeAdapterMeta {
    pub block_key: String,
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
}

impl ProbeAdapterMeta {
    pub fn from_adapter_info(info: &wgpu::AdapterInfo) -> Self {
        let backend = format!("{:?}", info.backend);
        let device_type = format!("{:?}", info.device_type);
        let block_key =
            AppConfig::gpu_adapter_block_key(&info.name, &backend, info.vendor, info.device);
        Self {
            block_key,
            name: info.name.clone(),
            backend,
            device_type,
            vendor: info.vendor,
            device: info.device,
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuGuardOutcome {
    AllowGpu,
    UseNoGpu { reason: String },
}

#[derive(Clone, Debug)]
struct ProbeExecResult {
    success: bool,
    timed_out: bool,
    adapter: Option<ProbeAdapterMeta>,
    last_stage: Option<String>,
    detail: String,
}

fn utc_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn utc_epoch_label(secs: u64) -> String {
    format!("unix:{secs}")
}

fn is_blocked_entry_expired(
    entry: &BlockedGpuAdapterEntry,
    meta: Option<&ProbeAdapterMeta>,
    now_secs: u64,
) -> Option<&'static str> {
    if entry.recorded_epoch_secs > 0
        && now_secs.saturating_sub(entry.recorded_epoch_secs) >= GPU_BLOCKLIST_TTL_SECS
    {
        return Some("30-day expiry elapsed");
    }
    if let Some(m) = meta
        && (entry.driver != m.driver || entry.driver_info != m.driver_info)
    {
        return Some("driver version changed");
    }
    None
}

fn emit_probe_line(prefix: &str, payload: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{prefix}{payload}");
    let _ = out.flush();
}

/// Entrypoint executed inside the `--gpu-probe` child process (`adapter` -> `device` ->
/// `surface` -> `first frame`) so a driver panic under `panic = "abort"` or an infinite hang
/// cannot terminate the main emulator process.
pub fn run_gpu_probe_child(cli: &CliConfig) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetErrorMode(u_mode: u32) -> u32;
        }
        // Suppress modal GP-fault / critical-error dialogs in the disposable child GPU probe.
        unsafe {
            SetErrorMode(0x0001 | 0x0002 | 0x8000);
        }
    }
    emit_probe_line(PREFIX_STAGE, "adapter");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = if cli.software_adapter {
        discover_software_cpu_adapter_with_backends(wgpu::Backends::all())?
    } else {
        let adapters = instance.enumerate_adapters(wgpu::Backends::all());
        let Some(picked) = select_preferred_adapter(adapters.iter()) else {
            emit_probe_line(PREFIX_OK, "no-adapter");
            return Ok(());
        };
        picked
    };

    let meta = ProbeAdapterMeta::from_adapter_info(&adapter.get_info());
    let meta_json = serde_json::to_string(&meta)
        .map_err(|e| format!("failed to encode probe adapter metadata: {e}"))?;
    emit_probe_line(PREFIX_ADAPTER, &meta_json);

    emit_probe_line(PREFIX_STAGE, "device");
    if cli.probe_fail_stage == Some(ProbeFailStage::Device) {
        panic!(
            "Injected GPU startup failure at stage=device for adapter `{}`",
            meta.block_key
        );
    }

    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .map_err(|e| format!("request_device failed for `{}`: {e}", meta.name))?;

    emit_probe_line(PREFIX_STAGE, "surface");
    struct ProbeSurfaceApp {
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        meta: ProbeAdapterMeta,
        fail_stage: Option<ProbeFailStage>,
        window: Option<Arc<Window>>,
        err: Option<String>,
        done: bool,
    }

    impl ApplicationHandler for ProbeSurfaceApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.done {
                return;
            }
            let attrs = Window::default_attributes()
                .with_title("Rustboy Advance GPU Probe")
                .with_inner_size(LogicalSize::new(128u32, 128u32))
                .with_visible(false);
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    self.err = Some(format!("probe hidden window creation failed: {e}"));
                    self.done = true;
                    event_loop.exit();
                    return;
                }
            };
            self.window = Some(Arc::clone(&window));

            if self.fail_stage == Some(ProbeFailStage::Surface) {
                panic!(
                    "Injected GPU startup failure at stage=surface for adapter `{}`",
                    self.meta.block_key
                );
            }

            let surface = match self.instance.create_surface(Arc::clone(&window)) {
                Ok(s) => s,
                Err(e) => {
                    self.err = Some(format!("create_surface failed: {e}"));
                    self.done = true;
                    event_loop.exit();
                    return;
                }
            };
            let caps = surface.get_capabilities(&self.adapter);
            let Some(&format) = caps.formats.first() else {
                self.err = Some("surface returned zero supported texture formats".to_string());
                self.done = true;
                event_loop.exit();
                return;
            };
            let Some(&present_mode) = caps.present_modes.first() else {
                self.err = Some("surface returned zero supported present modes".to_string());
                self.done = true;
                event_loop.exit();
                return;
            };
            let Some(&alpha_mode) = caps.alpha_modes.first() else {
                self.err = Some("surface returned zero supported alpha modes".to_string());
                self.done = true;
                event_loop.exit();
                return;
            };

            let phys = window.inner_size();
            let surf_cfg = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: phys.width.max(1),
                height: phys.height.max(1),
                present_mode,
                alpha_mode,
                view_formats: vec![],
                desired_maximum_frame_latency: 1,
            };
            surface.configure(&self.device, &surf_cfg);

            emit_probe_line(PREFIX_STAGE, "first_frame");
            if self.fail_stage == Some(ProbeFailStage::Hang) {
                emit_probe_line(PREFIX_STAGE, "first_frame_hanging");
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }

            let frame = match surface.get_current_texture() {
                Ok(f) => f,
                Err(e) => {
                    self.err = Some(format!("surface get_current_texture failed: {e}"));
                    self.done = true;
                    event_loop.exit();
                    return;
                }
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("rba_gpu_probe_first_frame"),
                    });
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("rba_gpu_probe_clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                });
            }
            self.queue.submit([encoder.finish()]);
            frame.present();
            let _ = self.device.poll(wgpu::Maintain::Wait);

            emit_probe_line(PREFIX_OK, &self.meta.block_key);
            self.done = true;
            event_loop.exit();
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            _event: WindowEvent,
        ) {
        }
    }

    let mut builder = EventLoop::builder();
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(true);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("failed to build event loop for GPU probe: {e}"))?;

    let mut app = ProbeSurfaceApp {
        instance,
        adapter,
        device,
        queue,
        meta,
        fail_stage: cli.probe_fail_stage,
        window: None,
        err: None,
        done: false,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("GPU probe event loop failed: {e}"))?;
    if let Some(err) = app.err {
        return Err(err);
    }
    Ok(())
}

fn peek_primary_adapter_meta(software_adapter: bool) -> Option<ProbeAdapterMeta> {
    if software_adapter {
        discover_software_cpu_adapter_with_backends(wgpu::Backends::all())
            .ok()
            .map(|a| ProbeAdapterMeta::from_adapter_info(&a.get_info()))
    } else {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapters = instance.enumerate_adapters(wgpu::Backends::all());
        select_preferred_adapter(adapters.iter())
            .map(|a| ProbeAdapterMeta::from_adapter_info(&a.get_info()))
    }
}

fn run_probe_subprocess(
    exe: &std::path::Path,
    software_adapter: bool,
    fail_stage: Option<ProbeFailStage>,
    timeout: Duration,
) -> ProbeExecResult {
    let mut cmd = Command::new(exe);
    cmd.arg("--gpu-probe");
    if software_adapter {
        cmd.arg("--software-adapter");
    }
    if let Some(stage) = fail_stage {
        cmd.arg("--probe-fail-stage").arg(stage.as_str());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ProbeExecResult {
                success: false,
                timed_out: false,
                adapter: None,
                last_stage: Some("spawn".to_string()),
                detail: format!("failed to spawn child GPU probe: {e}"),
            };
        }
    };

    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
    }
    let stderr_handle = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = BufReader::new(stderr);
            let _ = std::io::Read::read_to_string(&mut reader, &mut buf);
            buf
        })
    });

    let start = Instant::now();
    let mut adapter: Option<ProbeAdapterMeta> = None;
    let mut last_stage: Option<String> = None;
    let mut saw_ok = false;

    let mut drain_lines = |rx: &mpsc::Receiver<String>| {
        while let Ok(line) = rx.try_recv() {
            if let Some(json) = line.strip_prefix(PREFIX_ADAPTER) {
                if let Ok(meta) = serde_json::from_str::<ProbeAdapterMeta>(json) {
                    adapter = Some(meta);
                }
            } else if let Some(stage) = line.strip_prefix(PREFIX_STAGE) {
                last_stage = Some(stage.trim().to_string());
            } else if line.starts_with(PREFIX_OK) {
                saw_ok = true;
            }
        }
    };

    let (timed_out, status) = loop {
        drain_lines(&rx);
        match child.try_wait() {
            Ok(Some(st)) => break (false, Some(st)),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break (true, None);
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                let _ = child.kill();
                drain_lines(&rx);
                let stderr_text = stderr_handle
                    .and_then(|h| h.join().ok())
                    .unwrap_or_default();
                return ProbeExecResult {
                    success: false,
                    timed_out: false,
                    adapter,
                    last_stage,
                    detail: format!("try_wait failed: {e}; stderr={stderr_text}"),
                };
            }
        }
    };

    drain_lines(&rx);
    let stderr_text = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr_snippet: String = stderr_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" | ");

    if !timed_out && status.is_some_and(|s| s.success()) && saw_ok {
        return ProbeExecResult {
            success: true,
            timed_out: false,
            adapter,
            last_stage,
            detail: "GPU probe succeeded".to_string(),
        };
    }

    let detail = if timed_out {
        format!(
            "child GPU probe timed out after {} ms at stage `{}`",
            timeout.as_millis(),
            last_stage.as_deref().unwrap_or("unknown")
        )
    } else if let Some(st) = status {
        let code_str = st
            .code()
            .map(|c| format!("exit code {c}"))
            .unwrap_or_else(|| "terminated by signal/abort".to_string());
        if stderr_snippet.is_empty() {
            format!(
                "child GPU probe failed ({code_str}) at stage `{}`",
                last_stage.as_deref().unwrap_or("unknown")
            )
        } else {
            format!(
                "child GPU probe failed ({code_str}) at stage `{}`: {stderr_snippet}",
                last_stage.as_deref().unwrap_or("unknown")
            )
        }
    } else {
        "child GPU probe failed with unknown status".to_string()
    };

    ProbeExecResult {
        success: false,
        timed_out,
        adapter,
        last_stage,
        detail,
    }
}

#[cfg(target_os = "windows")]
mod checking_splash {
    use std::thread::JoinHandle;
    use super::*;

    const WM_DESTROY: u32 = 0x0002;
    const WM_PAINT: u32 = 0x000F;
    const WM_CLOSE: u32 = 0x0010;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    const SM_CXSCREEN: i32 = 0;
    const SM_CYSCREEN: i32 = 1;
    const TRANSPARENT: i32 = 1;
    const DEFAULT_GUI_FONT: i32 = 17;
    const DT_CENTER: u32 = 0x0001;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_VCENTER: u32 = 0x0004;
    const DT_NOPREFIX: u32 = 0x0800;

    static CLASS_SEQ: AtomicUsize = AtomicUsize::new(1);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct PaintStruct {
        hdc: isize,
        f_erase: i32,
        rc_paint: Rect,
        f_restore: i32,
        f_inc_update: i32,
        rgb_reserved: [u8; 32],
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: Point,
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: isize,
        h_icon: isize,
        h_cursor: isize,
        hbr_background: isize,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: isize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> isize;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassExW(lp_wnd_class: *const WndClassExW) -> u16;
        fn UnregisterClassW(lp_class_name: *const u16, h_instance: isize) -> i32;
        fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            h_wnd_parent: isize,
            h_menu: isize,
            h_instance: isize,
            lp_param: isize,
        ) -> isize;
        fn DestroyWindow(hwnd: isize) -> i32;
        fn DefWindowProcW(hwnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
        fn GetMessageW(
            lp_msg: *mut Msg,
            hwnd: isize,
            w_msg_filter_min: u32,
            w_msg_filter_max: u32,
        ) -> i32;
        fn TranslateMessage(lp_msg: *const Msg) -> i32;
        fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        fn PostMessageW(hwnd: isize, msg: u32, w_param: usize, l_param: isize) -> i32;
        fn PostQuitMessage(n_exit_code: i32);
        fn BeginPaint(hwnd: isize, lp_paint: *mut PaintStruct) -> isize;
        fn EndPaint(hwnd: isize, lp_paint: *const PaintStruct) -> i32;
        fn FillRect(hdc: isize, lprc: *const Rect, hbr: isize) -> i32;
        fn DrawTextW(
            hdc: isize,
            lpch_text: *const u16,
            cch_text: i32,
            lprc: *mut Rect,
            format: u32,
        ) -> i32;
        fn GetSystemMetrics(n_index: i32) -> i32;
        fn GetClientRect(hwnd: isize, lp_rect: *mut Rect) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateSolidBrush(color: u32) -> isize;
        fn DeleteObject(ho: isize) -> i32;
        fn SetBkMode(hdc: isize, mode: i32) -> i32;
        fn SetTextColor(hdc: isize, color: u32) -> u32;
        fn SelectObject(hdc: isize, h: isize) -> isize;
        fn GetStockObject(i: i32) -> isize;
    }

    fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn splash_wndproc(
        hwnd: isize,
        msg: u32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        match msg {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps = std::mem::MaybeUninit::<PaintStruct>::zeroed();
                let hdc = unsafe { BeginPaint(hwnd, ps.as_mut_ptr()) };
                if hdc != 0 {
                    let mut rc = Rect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    unsafe { GetClientRect(hwnd, &mut rc) };

                    // Subtle 1px hairline border (#2B3340 -> 0x0040332B) and dark card (#141820 -> 0x00201814).
                    let border_brush = unsafe { CreateSolidBrush(0x0040_332B) };
                    let bg_brush = unsafe { CreateSolidBrush(0x0020_1814) };
                    if border_brush != 0 {
                        unsafe { FillRect(hdc, &rc, border_brush) };
                    }
                    let inner = Rect {
                        left: rc.left + 1,
                        top: rc.top + 1,
                        right: (rc.right - 1).max(rc.left + 1),
                        bottom: (rc.bottom - 1).max(rc.top + 1),
                    };
                    if bg_brush != 0 {
                        unsafe { FillRect(hdc, &inner, bg_brush) };
                    }

                    unsafe { SetBkMode(hdc, TRANSPARENT) };
                    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
                    let prev_font = if font != 0 {
                        unsafe { SelectObject(hdc, font) }
                    } else {
                        0
                    };

                    let mut title_rc = Rect {
                        left: inner.left + 12,
                        top: inner.top + 16,
                        right: inner.right - 12,
                        bottom: inner.top + 42,
                    };
                    let mut sub_rc = Rect {
                        left: inner.left + 12,
                        top: inner.top + 42,
                        right: inner.right - 12,
                        bottom: inner.bottom - 12,
                    };

                    let title = to_wide_null("Checking graphics...");
                    let sub = to_wide_null("Verifying GPU adapter before startup");
                    unsafe {
                        SetTextColor(hdc, 0x00F9_F5F1);
                        DrawTextW(
                            hdc,
                            title.as_ptr(),
                            (title.len() - 1) as i32,
                            &mut title_rc,
                            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                        );
                        SetTextColor(hdc, 0x00B8_A394);
                        DrawTextW(
                            hdc,
                            sub.as_ptr(),
                            (sub.len() - 1) as i32,
                            &mut sub_rc,
                            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                        );
                    }

                    if prev_font != 0 {
                        unsafe { SelectObject(hdc, prev_font) };
                    }
                    if bg_brush != 0 {
                        unsafe { DeleteObject(bg_brush) };
                    }
                    if border_brush != 0 {
                        unsafe { DeleteObject(border_brush) };
                    }
                    unsafe { EndPaint(hwnd, ps.as_ptr()) };
                }
                0
            }
            WM_CLOSE => {
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
        }
    }

    pub struct SplashGuard {
        hwnd: Arc<AtomicIsize>,
        closed: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl SplashGuard {
        pub fn spawn() -> Self {
            let hwnd = Arc::new(AtomicIsize::new(0));
            let closed = Arc::new(AtomicBool::new(false));
            let finished = Arc::new(AtomicBool::new(false));

            let hwnd_c = Arc::clone(&hwnd);
            let closed_c = Arc::clone(&closed);
            let finished_c = Arc::clone(&finished);

            let thread = std::thread::Builder::new()
                .name("rba-gpu-probe-splash".to_string())
                .spawn(move || {
                    let seq = CLASS_SEQ.fetch_add(1, Ordering::Relaxed);
                    let class_name = to_wide_null(&format!("RbaGpuProbeSplash_{seq}"));
                    let title = to_wide_null("Checking graphics...");
                    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };

                    let wc = WndClassExW {
                        cb_size: std::mem::size_of::<WndClassExW>() as u32,
                        style: 0,
                        lpfn_wnd_proc: splash_wndproc,
                        cb_cls_extra: 0,
                        cb_wnd_extra: 0,
                        h_instance: hinstance,
                        h_icon: 0,
                        h_cursor: 0,
                        hbr_background: 0,
                        lpsz_menu_name: std::ptr::null(),
                        lpsz_class_name: class_name.as_ptr(),
                        h_icon_sm: 0,
                    };

                    if unsafe { RegisterClassExW(&wc) } != 0 {
                        let w = 320;
                        let h = 84;
                        let sx = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(w);
                        let sy = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(h);
                        let x = (sx - w) / 2;
                        let y = (sy - h) / 2;

                        let handle = unsafe {
                            CreateWindowExW(
                                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                                class_name.as_ptr(),
                                title.as_ptr(),
                                WS_POPUP | WS_VISIBLE,
                                x,
                                y,
                                w,
                                h,
                                0,
                                0,
                                hinstance,
                                0,
                            )
                        };
                        if handle != 0 {
                            hwnd_c.store(handle, Ordering::Release);
                            if closed_c.load(Ordering::Acquire) {
                                unsafe { PostMessageW(handle, WM_CLOSE, 0, 0) };
                            }
                            let mut msg = std::mem::MaybeUninit::<Msg>::zeroed();
                            while unsafe { GetMessageW(msg.as_mut_ptr(), 0, 0, 0) } > 0 {
                                let m = unsafe { msg.assume_init_ref() };
                                unsafe {
                                    TranslateMessage(m);
                                    DispatchMessageW(m);
                                }
                            }
                        }
                        unsafe { UnregisterClassW(class_name.as_ptr(), hinstance) };
                    }
                    finished_c.store(true, Ordering::Release);
                })
                .ok();

            Self {
                hwnd,
                closed,
                finished,
                thread,
            }
        }
    }

    impl Drop for SplashGuard {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_millis(500);
            while self.hwnd.load(Ordering::Acquire) == 0
                && !self.finished.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            }
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod checking_splash {
    pub struct SplashGuard;
    impl SplashGuard {
        pub fn spawn() -> Self {
            Self
        }
    }
}

fn evaluate_guard_with_runner_at<F>(
    cli: &CliConfig,
    config: &mut AppConfig,
    persist: bool,
    now_secs: u64,
    peek_meta: Option<ProbeAdapterMeta>,
    run_probe: F,
) -> GpuGuardOutcome
where
    F: FnOnce(Duration) -> ProbeExecResult,
{
    let mut config_dirty = false;

    if cli.retry_gpu {
        let had_verified = !config.verified_gpu_adapters.is_empty();
        let cleared = config.clear_blocked_gpu_adapters();
        if cleared > 0 || had_verified {
            config_dirty = true;
        }
        println!(
            "[gpu-guard] --retry-gpu cleared {cleared} blocked adapter record(s); re-probing GPU"
        );
    }

    if let Some(entry) = config
        .blocked_gpu_adapters
        .get(ADAPTER_ENUM_SENTINEL_KEY)
        .cloned()
    {
        if let Some(why) = is_blocked_entry_expired(&entry, None, now_secs) {
            config
                .blocked_gpu_adapters
                .remove(ADAPTER_ENUM_SENTINEL_KEY);
            config_dirty = true;
            println!(
                "[gpu-guard] Expired blocklist entry for `{ADAPTER_ENUM_SENTINEL_KEY}` ({why}); re-probing"
            );
        } else if entry.consecutive_failures >= GPU_BLOCKLIST_FAILURE_THRESHOLD {
            if persist && config_dirty {
                config.save();
            }
            let reason = format!(
                "Skipped GPU initialization: previous `{}` failure ({} consecutive) recorded for `{}` ({}); pass --retry-gpu to retry",
                entry.failed_stage, entry.consecutive_failures, entry.adapter_name, entry.reason
            );
            println!("[gpu-guard] {reason}");
            return GpuGuardOutcome::UseNoGpu { reason };
        }
    }

    if let Some(ref meta) = peek_meta
        && let Some(entry) = config.blocked_gpu_adapters.get(&meta.block_key).cloned()
    {
        if let Some(why) = is_blocked_entry_expired(&entry, Some(meta), now_secs) {
            config.blocked_gpu_adapters.remove(&meta.block_key);
            config_dirty = true;
            println!(
                "[gpu-guard] Expired blocklist entry for `{}` ({why}); re-probing",
                meta.block_key
            );
        } else if entry.consecutive_failures >= GPU_BLOCKLIST_FAILURE_THRESHOLD {
            if persist && config_dirty {
                config.save();
            }
            let reason = format!(
                "Skipped blocked GPU adapter `{}` ({} consecutive failures; latest `{}`: {}); pass --retry-gpu to retry",
                meta.block_key, entry.consecutive_failures, entry.failed_stage, entry.reason
            );
            println!("[gpu-guard] {reason}");
            return GpuGuardOutcome::UseNoGpu { reason };
        }
    }

    // Check prior active failures (if 1st failure occurred without hitting the 2-failure blocklist).
    let prior_failures = peek_meta
        .as_ref()
        .and_then(|meta| config.blocked_gpu_adapters.get(&meta.block_key))
        .or_else(|| config.blocked_gpu_adapters.get(ADAPTER_ENUM_SENTINEL_KEY))
        .map(|e| e.consecutive_failures)
        .unwrap_or(0);

    // Verified cache lookup: only valid when adapter + driver match cleanly, no retry/fault flags,
    // and no active un-recovered failures remain.
    if !cli.retry_gpu
        && cli.probe_fail_stage.is_none()
        && prior_failures == 0
        && let Some(ref meta) = peek_meta
    {
        if config
            .verified_gpu_adapter(&meta.block_key, &meta.driver, &meta.driver_info)
            .is_some()
        {
            if persist && config_dirty {
                config.save();
            }
            println!(
                "[gpu-guard] Using cached GPU probe result for `{}` (driver `{}` / `{}`); skipping child probe",
                meta.block_key, meta.driver, meta.driver_info
            );
            return GpuGuardOutcome::AllowGpu;
        }
        if config.verified_gpu_adapters.remove(&meta.block_key).is_some() {
            config_dirty = true;
            println!(
                "[gpu-guard] Cached GPU probe invalidated for `{}` (driver changed to `{}` / `{}`); re-probing",
                meta.block_key, meta.driver, meta.driver_info
            );
        }
    }

    let default_timeout_ms = if prior_failures >= 1 {
        RETRY_GPU_PROBE_TIMEOUT_MS
    } else {
        DEFAULT_GPU_PROBE_TIMEOUT_MS
    };
    let timeout = Duration::from_millis(
        cli.gpu_probe_timeout_ms
            .unwrap_or(default_timeout_ms)
            .max(200),
    );
    if prior_failures >= 1 {
        let target_label = peek_meta
            .as_ref()
            .map(|m| m.block_key.as_str())
            .unwrap_or(ADAPTER_ENUM_SENTINEL_KEY);
        println!(
            "[gpu-guard] Retrying GPU probe for `{target_label}` after {prior_failures} failure(s) with {} ms timeout",
            timeout.as_millis()
        );
    }

    let probe_start = Instant::now();
    let result = run_probe(timeout);
    let probe_elapsed_ms = probe_start.elapsed().as_secs_f64() * 1000.0;
    if result.success {
        let resolved_meta = result.adapter.or(peek_meta);
        if config
            .blocked_gpu_adapters
            .remove(ADAPTER_ENUM_SENTINEL_KEY)
            .is_some()
        {
            config_dirty = true;
        }
        if let Some(ref meta) = resolved_meta {
            if config
                .blocked_gpu_adapters
                .remove(&meta.block_key)
                .is_some()
            {
                config_dirty = true;
            }
            if cli.probe_fail_stage.is_none() {
                config.record_verified_gpu_adapter(
                    meta.block_key.clone(),
                    VerifiedGpuAdapterEntry {
                        adapter_name: meta.name.clone(),
                        backend: meta.backend.clone(),
                        device_type: meta.device_type.clone(),
                        vendor: meta.vendor,
                        device: meta.device,
                        driver: meta.driver.clone(),
                        driver_info: meta.driver_info.clone(),
                        verified_epoch_secs: now_secs,
                    },
                );
                config_dirty = true;
            }
            println!(
                "[gpu-guard] GPU probe passed in {probe_elapsed_ms:.1} ms for `{}` (driver `{}` / `{}`)",
                meta.block_key, meta.driver, meta.driver_info
            );
        } else {
            println!("[gpu-guard] GPU probe passed in {probe_elapsed_ms:.1} ms (no adapter)");
        }
        if persist && config_dirty {
            config.save();
        }
        return GpuGuardOutcome::AllowGpu;
    }

    let stage = if result.timed_out {
        "hang".to_string()
    } else {
        result
            .last_stage
            .clone()
            .unwrap_or_else(|| "adapter".to_string())
    };
    let resolved_meta = result.adapter.or(peek_meta);
    let (key, mut entry) = if let Some(meta) = resolved_meta {
        let key = meta.block_key.clone();
        let e = BlockedGpuAdapterEntry {
            adapter_name: meta.name,
            backend: meta.backend,
            device_type: meta.device_type,
            vendor: meta.vendor,
            device: meta.device,
            driver: meta.driver,
            driver_info: meta.driver_info,
            failed_stage: stage.clone(),
            reason: result.detail.clone(),
            recorded_utc: utc_epoch_label(now_secs),
            consecutive_failures: 0,
            recorded_epoch_secs: now_secs,
        };
        (key, e)
    } else {
        (
            ADAPTER_ENUM_SENTINEL_KEY.to_string(),
            BlockedGpuAdapterEntry {
                adapter_name: "Adapter enumeration / ICD".to_string(),
                backend: "All".to_string(),
                device_type: "Unknown".to_string(),
                vendor: 0,
                device: 0,
                driver: String::new(),
                driver_info: String::new(),
                failed_stage: stage.clone(),
                reason: result.detail.clone(),
                recorded_utc: utc_epoch_label(now_secs),
                consecutive_failures: 0,
                recorded_epoch_secs: now_secs,
            },
        )
    };

    let prior_for_key = config
        .blocked_gpu_adapters
        .get(&key)
        .filter(|prev| {
            key == ADAPTER_ENUM_SENTINEL_KEY
                || (prev.driver == entry.driver && prev.driver_info == entry.driver_info)
        })
        .map(|prev| prev.consecutive_failures)
        .unwrap_or(0);
    let new_failures = prior_for_key.saturating_add(1);
    entry.consecutive_failures = new_failures;

    config.verified_gpu_adapters.remove(&key);
    config.record_blocked_gpu_adapter(key.clone(), entry);
    if persist {
        config.save();
    }

    let reason = if new_failures < GPU_BLOCKLIST_FAILURE_THRESHOLD {
        format!(
            "GPU startup guard caught failure #{new_failures} at `{stage}` for `{key}` in {probe_elapsed_ms:.1} ms ({}); starting No-GPU window and will retry GPU on next launch with {} ms timeout",
            result.detail, RETRY_GPU_PROBE_TIMEOUT_MS
        )
    } else {
        format!(
            "GPU startup guard blocklisted `{key}` after {new_failures} consecutive failures (latest `{stage}`: {}); starting No-GPU window (expires after 30 days, driver change, or --retry-gpu)",
            result.detail
        )
    };
    println!("[gpu-guard] {reason}");
    GpuGuardOutcome::UseNoGpu { reason }
}

/// Checks per-adapter failure config and runs the isolated child-process GPU startup probe
/// (`adapter`, `device`, `surface`, `first frame`). Never crashes the parent on child abort/hang.
pub fn ensure_gpu_startup_safe(cli: &CliConfig, config: &mut AppConfig) -> GpuGuardOutcome {
    if cli.no_gpu || cli.simulate_no_adapter {
        return GpuGuardOutcome::AllowGpu;
    }
    let guard_start = Instant::now();
    let peek_meta = if cli.retry_gpu
        || !config
            .blocked_gpu_adapters
            .contains_key(ADAPTER_ENUM_SENTINEL_KEY)
    {
        peek_primary_adapter_meta(cli.software_adapter)
    } else {
        None
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return GpuGuardOutcome::UseNoGpu {
                reason: format!("unable to locate current_exe for GPU probe: {e}"),
            };
        }
    };
    let outcome = evaluate_guard_with_runner_at(
        cli,
        config,
        true,
        utc_epoch_secs(),
        peek_meta,
        |timeout| {
            let _splash = checking_splash::SplashGuard::spawn();
            run_probe_subprocess(&exe, cli.software_adapter, cli.probe_fail_stage, timeout)
        },
    );
    let total_ms = guard_start.elapsed().as_secs_f64() * 1000.0;
    println!("[gpu-guard] Startup GPU guard completed in {total_ms:.1} ms");
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> ProbeAdapterMeta {
        ProbeAdapterMeta {
            block_key: "Vulkan|Test GPU|0x1002|0x1234".to_string(),
            name: "Test GPU".to_string(),
            backend: "Vulkan".to_string(),
            device_type: "IntegratedGpu".to_string(),
            vendor: 0x1002,
            device: 0x1234,
            driver: "radv".to_string(),
            driver_info: "1.0".to_string(),
        }
    }

    fn ok_result(meta: &ProbeAdapterMeta) -> ProbeExecResult {
        ProbeExecResult {
            success: true,
            timed_out: false,
            adapter: Some(meta.clone()),
            last_stage: None,
            detail: String::new(),
        }
    }

    fn fail_result(meta: &ProbeAdapterMeta, stage: &str, timed_out: bool) -> ProbeExecResult {
        ProbeExecResult {
            success: false,
            timed_out,
            adapter: Some(meta.clone()),
            last_stage: Some(if timed_out {
                "first_frame".into()
            } else {
                stage.into()
            }),
            detail: format!("simulated {stage} failure"),
        }
    }

    #[test]
    fn guard_requires_two_consecutive_failures_retries_at_20s_and_recovers_with_retry_gpu() {
        let base_time = 1_700_000_000u64;
        for (stage, timed_out) in [("device", false), ("surface", false), ("hang", true)] {
            let mut config = AppConfig::default();
            let cli = CliConfig::default();
            let meta = sample_meta();

            // Launch 1 fails (count = 1 < 2): falls back to No-GPU for this launch, NOT blocklisted yet.
            let mut t1 = None;
            let outcome1 = evaluate_guard_with_runner_at(
                &cli,
                &mut config,
                false,
                base_time,
                Some(meta.clone()),
                |timeout| {
                    t1 = Some(timeout);
                    fail_result(&meta, stage, timed_out)
                },
            );
            assert_eq!(t1, Some(Duration::from_millis(DEFAULT_GPU_PROBE_TIMEOUT_MS)));
            assert!(matches!(outcome1, GpuGuardOutcome::UseNoGpu { .. }));
            let recorded1 = config
                .blocked_gpu_adapters
                .get(&meta.block_key)
                .expect("failure #1 recorded");
            assert_eq!(recorded1.consecutive_failures, 1);
            assert_eq!(recorded1.failed_stage, stage);

            // Launch 2 MUST retry the GPU probe with the longer 20s timeout; second consecutive failure blocklists.
            let mut t2 = None;
            let outcome2 = evaluate_guard_with_runner_at(
                &cli,
                &mut config,
                false,
                base_time + 60,
                Some(meta.clone()),
                |timeout| {
                    t2 = Some(timeout);
                    fail_result(&meta, stage, timed_out)
                },
            );
            assert_eq!(
                t2,
                Some(Duration::from_millis(RETRY_GPU_PROBE_TIMEOUT_MS)),
                "retry after 1st failure must use 20s timeout"
            );
            assert!(matches!(outcome2, GpuGuardOutcome::UseNoGpu { .. }));
            let recorded2 = config
                .blocked_gpu_adapters
                .get(&meta.block_key)
                .expect("failure #2 recorded");
            assert_eq!(recorded2.consecutive_failures, 2);

            // Launch 3 now skips spawning the GPU probe entirely.
            let mut probed_third_time = false;
            let outcome3 = evaluate_guard_with_runner_at(
                &cli,
                &mut config,
                false,
                base_time + 120,
                Some(meta.clone()),
                |_| {
                    probed_third_time = true;
                    ok_result(&meta)
                },
            );
            assert!(
                !probed_third_time,
                "adapter blocklisted after 2 failures must skip probe next launch"
            );
            assert!(matches!(outcome3, GpuGuardOutcome::UseNoGpu { .. }));

            // Launch 4 with --retry-gpu clears blocklist, probes again, and caches success.
            let mut retry_cli = CliConfig::default();
            retry_cli.retry_gpu = true;
            let mut probed_after_retry = false;
            let outcome4 = evaluate_guard_with_runner_at(
                &retry_cli,
                &mut config,
                false,
                base_time + 180,
                Some(meta.clone()),
                |_| {
                    probed_after_retry = true;
                    ok_result(&meta)
                },
            );
            assert!(probed_after_retry);
            assert_eq!(outcome4, GpuGuardOutcome::AllowGpu);
            assert!(config.blocked_gpu_adapters.is_empty());
            assert!(
                config
                    .verified_gpu_adapter(&meta.block_key, &meta.driver, &meta.driver_info)
                    .is_some()
            );
        }
    }

    #[test]
    fn single_failure_that_recovers_on_20s_retry_clears_count_and_caches_adapter() {
        let mut config = AppConfig::default();
        let cli = CliConfig::default();
        let meta = sample_meta();
        let now = 1_700_000_000u64;

        // First launch times out -> 1 consecutive failure, not blocklisted.
        let o1 = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            now,
            Some(meta.clone()),
            |_| fail_result(&meta, "hang", true),
        );
        assert!(matches!(o1, GpuGuardOutcome::UseNoGpu { .. }));
        assert_eq!(config.blocked_gpu_adapters[&meta.block_key].consecutive_failures, 1);

        // Second launch retries with 20s timeout and succeeds -> resets failure state and caches verified adapter.
        let mut retry_timeout = None;
        let o2 = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            now + 10,
            Some(meta.clone()),
            |t| {
                retry_timeout = Some(t);
                ok_result(&meta)
            },
        );
        assert_eq!(retry_timeout, Some(Duration::from_millis(RETRY_GPU_PROBE_TIMEOUT_MS)));
        assert_eq!(o2, GpuGuardOutcome::AllowGpu);
        assert!(config.blocked_gpu_adapters.is_empty());
        assert!(
            config
                .verified_gpu_adapter(&meta.block_key, &meta.driver, &meta.driver_info)
                .is_some()
        );
    }

    #[test]
    fn blocklist_expires_after_30_days_or_driver_change_and_verified_cache_tracks_driver() {
        let mut config = AppConfig::default();
        let cli = CliConfig::default();
        let meta = sample_meta();
        let t0 = 1_700_000_000u64;

        // Two failures blocklist the GPU.
        for i in 0..2 {
            let _ = evaluate_guard_with_runner_at(
                &cli,
                &mut config,
                false,
                t0 + i,
                Some(meta.clone()),
                |_| fail_result(&meta, "hang", true),
            );
        }
        assert_eq!(config.blocked_gpu_adapters[&meta.block_key].consecutive_failures, 2);

        // At +29 days with same driver: still blocked (probe skipped).
        let mut probed_day29 = false;
        let o_day29 = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            t0 + 1 + (29 * 86_400),
            Some(meta.clone()),
            |_| {
                probed_day29 = true;
                ok_result(&meta)
            },
        );
        assert!(!probed_day29);
        assert!(matches!(o_day29, GpuGuardOutcome::UseNoGpu { .. }));

        // At +30 days: expired -> re-probes and allows GPU + caches verified result.
        let mut probed_day30 = false;
        let mut t_day30 = None;
        let o_day30 = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            t0 + 1 + GPU_BLOCKLIST_TTL_SECS,
            Some(meta.clone()),
            |t| {
                probed_day30 = true;
                t_day30 = Some(t);
                ok_result(&meta)
            },
        );
        assert!(probed_day30);
        assert_eq!(t_day30, Some(Duration::from_millis(DEFAULT_GPU_PROBE_TIMEOUT_MS)));
        assert_eq!(o_day30, GpuGuardOutcome::AllowGpu);

        // Next warm launch with same driver skips child probe via verified cache.
        let mut probed_warm = false;
        let o_warm = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            t0 + 2 + GPU_BLOCKLIST_TTL_SECS,
            Some(meta.clone()),
            |_| {
                probed_warm = true;
                ok_result(&meta)
            },
        );
        assert!(!probed_warm, "verified cache should skip child probe");
        assert_eq!(o_warm, GpuGuardOutcome::AllowGpu);

        // Driver update invalidates verified cache AND expires any blocklist immediately.
        let mut updated_meta = meta.clone();
        updated_meta.driver_info = "2.0".to_string();
        let mut probed_new_driver = false;
        let o_drv = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            t0 + 3 + GPU_BLOCKLIST_TTL_SECS,
            Some(updated_meta.clone()),
            |_| {
                probed_new_driver = true;
                ok_result(&updated_meta)
            },
        );
        assert!(probed_new_driver, "driver change must invalidate verified cache");
        assert_eq!(o_drv, GpuGuardOutcome::AllowGpu);

        // Block with old driver -> driver change also immediately expires blocklist.
        for i in 0..2 {
            let _ = evaluate_guard_with_runner_at(
                &cli,
                &mut config,
                false,
                t0 + i,
                Some(meta.clone()),
                |_| fail_result(&meta, "device", false),
            );
        }
        let mut probed_after_drv_update = false;
        let o_unblocked = evaluate_guard_with_runner_at(
            &cli,
            &mut config,
            false,
            t0 + 5,
            Some(updated_meta.clone()),
            |_| {
                probed_after_drv_update = true;
                ok_result(&updated_meta)
            },
        );
        assert!(probed_after_drv_update, "driver change must expire blocklist");
        assert_eq!(o_unblocked, GpuGuardOutcome::AllowGpu);
    }
}
