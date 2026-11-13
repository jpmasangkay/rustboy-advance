use crate::{RustboyApp, configure_obsidian_style};
use eframe::egui_wgpu::{self, wgpu};
use std::sync::Arc;

impl eframe::App for RustboyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update(ctx);
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_exit(&mut self) {
        self.on_exit();
    }
}

/// Queries supported presentation modes with the eframe (0.31) / egui-wgpu version in use.
/// Prioritizes tear-free, stutter-free vsync modes (`AutoVsync` or `Fifo`) with a single-frame
/// latency buffer so gameplay is smooth and locked to display refresh cadence.
pub fn query_supported_present_mode(
    surface: &wgpu::Surface,
    adapter: &wgpu::Adapter,
) -> wgpu::PresentMode {
    let caps = surface.get_capabilities(adapter);
    log::info!(
        "eframe / wgpu surface presentation modes supported by adapter '{}': {:?}",
        adapter.get_info().name,
        caps.present_modes
    );

    if caps.present_modes.contains(&wgpu::PresentMode::AutoVsync) {
        log::info!("Surface supports AutoVsync -> choosing PresentMode::AutoVsync");
        wgpu::PresentMode::AutoVsync
    } else if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
        log::info!("Surface supports Fifo -> choosing PresentMode::Fifo");
        wgpu::PresentMode::Fifo
    } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        log::info!("Surface supports Mailbox -> choosing PresentMode::Mailbox");
        wgpu::PresentMode::Mailbox
    } else {
        log::info!("Defaulting to PresentMode::AutoNoVsync");
        wgpu::PresentMode::AutoNoVsync
    }
}

pub fn run_desktop(
    title: &str,
    init_width: f32,
    init_height: f32,
    cli: crate::headless::CliConfig,
) -> Result<(), String> {
    let pre_filter = cli
        .filter
        .unwrap_or_else(|| crate::config::AppConfig::load().display_filter);
    let decision = crate::shaders::choose_desktop_backend(
        cli.simulate_no_adapter,
        cli.no_gpu,
        cli.software_adapter,
        pre_filter,
        false,
        false,
        0.08,
    );
    println!(
        "[backend] Selected: {} ({} / {}) — reason: {}",
        decision.adapter_name, decision.adapter_backend, decision.adapter_device_type, decision.reason
    );
    log::info!(
        "[backend] Selected: {} ({} / {}) — reason: {}",
        decision.adapter_name,
        decision.adapter_backend,
        decision.adapter_device_type,
        decision.reason
    );
    if !decision.use_wgpu {
        return Err(decision.fallback_reason.unwrap_or(decision.reason));
    }

    let force_software = cli.software_adapter;
    let fullscreen = cli.fullscreen;

    // In eframe 0.31 with wgpu, adapter and surface capabilities are inspected
    // during adapter selection via `native_adapter_selector`.
    let wgpu_setup = egui_wgpu::WgpuSetupCreateNew {
        native_adapter_selector: Some(Arc::new(move |adapters, surface| {
            let adapter = if force_software {
                let sw_only: Vec<wgpu::Adapter> = adapters
                    .iter()
                    .filter(|a| matches!(a.get_info().device_type, wgpu::DeviceType::Cpu))
                    .cloned()
                    .collect();
                crate::shaders::select_preferred_adapter(sw_only.iter())
                    .or_else(|| crate::shaders::select_preferred_adapter(adapters))
            } else {
                crate::shaders::select_preferred_adapter(adapters)
            }
            .ok_or_else(|| {
                format!(
                    "No compatible graphics adapter found (fallback order: {}). Falling back to No-GPU presenter.",
                    crate::shaders::ADAPTER_FALLBACK_ORDER.join(" -> ")
                )
            })?;
            let info = adapter.get_info();
            log::info!(
                "Selected wgpu adapter '{}' ({:?} / {:?}) along fallback order [{}]",
                info.name,
                info.backend,
                info.device_type,
                crate::shaders::ADAPTER_FALLBACK_ORDER.join(" -> ")
            );

            if let Some(surf) = surface {
                let mode = query_supported_present_mode(surf, &adapter);
                log::info!("Adapter selector confirmed present mode: {:?}", mode);
            }
            Ok(adapter)
        })),
        ..Default::default()
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([init_width, init_height])
            .with_min_inner_size([520.0, 380.0])
            .with_fullscreen(fullscreen)
            .with_resizable(true),
        wgpu_options: egui_wgpu::WgpuConfiguration {
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: Some(1),
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        #[cfg(windows)]
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };

    eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            configure_obsidian_style(&cc.egui_ctx);
            let mut app = RustboyApp::new(cli);
            if let Some(rs) = &cc.wgpu_render_state {
                app.init_wgpu(rs);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("eframe execution failed: {e}"))
}
