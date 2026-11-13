use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::config::{AppConfig, BlockedGpuAdapterEntry};
use crate::headless::CliConfig;
use crate::shaders::{discover_software_cpu_adapter_with_backends, select_preferred_adapter};

pub const DEFAULT_GPU_PROBE_TIMEOUT_MS: u64 = 7000;
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

fn utc_epoch_label() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
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

fn evaluate_guard_with_runner<F>(
    cli: &CliConfig,
    config: &mut AppConfig,
    persist: bool,
    peek_meta: Option<ProbeAdapterMeta>,
    run_probe: F,
) -> GpuGuardOutcome
where
    F: FnOnce(Duration) -> ProbeExecResult,
{
    if cli.retry_gpu {
        let cleared = config.clear_blocked_gpu_adapters();
        if persist {
            config.save();
        }
        println!(
            "[gpu-guard] --retry-gpu cleared {cleared} blocked adapter record(s); re-probing GPU"
        );
    }

    if let Some(entry) = config.blocked_gpu_adapter(ADAPTER_ENUM_SENTINEL_KEY).cloned() {
        let reason = format!(
            "Skipped GPU initialization: previous `{}` failure recorded for `{}` ({}); pass --retry-gpu to retry",
            entry.failed_stage, entry.adapter_name, entry.reason
        );
        println!("[gpu-guard] {reason}");
        return GpuGuardOutcome::UseNoGpu { reason };
    }

    if let Some(ref meta) = peek_meta
        && let Some(entry) = config.blocked_gpu_adapter(&meta.block_key).cloned()
    {
        let reason = format!(
            "Skipped blocked GPU adapter `{}` (failed at `{}`: {}); pass --retry-gpu to retry",
            meta.block_key, entry.failed_stage, entry.reason
        );
        println!("[gpu-guard] {reason}");
        return GpuGuardOutcome::UseNoGpu { reason };
    }

    let timeout = Duration::from_millis(
        cli.gpu_probe_timeout_ms
            .unwrap_or(DEFAULT_GPU_PROBE_TIMEOUT_MS)
            .max(200),
    );
    let result = run_probe(timeout);
    if result.success {
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
    let (key, entry) = if let Some(meta) = result.adapter.or(peek_meta) {
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
            recorded_utc: utc_epoch_label(),
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
                recorded_utc: utc_epoch_label(),
            },
        )
    };

    config.record_blocked_gpu_adapter(key.clone(), entry);
    if persist {
        config.save();
    }

    let reason = format!(
        "GPU startup guard caught failure at `{stage}` for `{key}` ({}); starting No-GPU window (pass --retry-gpu to re-enable GPU)",
        result.detail
    );
    println!("[gpu-guard] {reason}");
    GpuGuardOutcome::UseNoGpu { reason }
}

/// Checks per-adapter failure config and runs the isolated child-process GPU startup probe
/// (`adapter`, `device`, `surface`, `first frame`). Never crashes the parent on child abort/hang.
pub fn ensure_gpu_startup_safe(cli: &CliConfig, config: &mut AppConfig) -> GpuGuardOutcome {
    if cli.no_gpu || cli.simulate_no_adapter {
        return GpuGuardOutcome::AllowGpu;
    }
    let peek_meta = if cli.retry_gpu || !config.blocked_gpu_adapters.contains_key(ADAPTER_ENUM_SENTINEL_KEY) {
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
    evaluate_guard_with_runner(cli, config, true, peek_meta, |timeout| {
        run_probe_subprocess(&exe, cli.software_adapter, cli.probe_fail_stage, timeout)
    })
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

    #[test]
    fn guard_records_each_failure_stage_skips_next_launch_and_recovers_with_retry() {
        for (stage, timed_out) in [("device", false), ("surface", false), ("hang", true)] {
            let mut config = AppConfig::default();
            let cli = CliConfig::default();
            let meta = sample_meta();

            // First launch fails at `stage` -> fallback to No-GPU + recorded in config.
            let outcome1 =
                evaluate_guard_with_runner(&cli, &mut config, false, Some(meta.clone()), |_| {
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
                });
            assert!(matches!(outcome1, GpuGuardOutcome::UseNoGpu { .. }));
            let recorded = config
                .blocked_gpu_adapter(&meta.block_key)
                .expect("adapter should be recorded in blocked_gpu_adapters");
            assert_eq!(recorded.failed_stage, stage);

            // Second launch skips spawning the GPU probe entirely.
            let mut probed_second_time = false;
            let outcome2 =
                evaluate_guard_with_runner(&cli, &mut config, false, Some(meta.clone()), |_| {
                    probed_second_time = true;
                    ProbeExecResult {
                        success: true,
                        timed_out: false,
                        adapter: Some(meta.clone()),
                        last_stage: None,
                        detail: String::new(),
                    }
                });
            assert!(
                !probed_second_time,
                "blocked adapter must skip probe next launch"
            );
            assert!(matches!(outcome2, GpuGuardOutcome::UseNoGpu { .. }));

            // Launch with --retry-gpu clears blocked entry and re-enables GPU when probe succeeds.
            let mut retry_cli = CliConfig::default();
            retry_cli.retry_gpu = true;
            let mut probed_after_retry = false;
            let outcome3 = evaluate_guard_with_runner(
                &retry_cli,
                &mut config,
                false,
                Some(meta.clone()),
                |_| {
                    probed_after_retry = true;
                    ProbeExecResult {
                        success: true,
                        timed_out: false,
                        adapter: Some(meta.clone()),
                        last_stage: None,
                        detail: String::new(),
                    }
                },
            );
            assert!(probed_after_retry);
            assert_eq!(outcome3, GpuGuardOutcome::AllowGpu);
            assert!(config.blocked_gpu_adapters.is_empty());
        }
    }
}
