use std::sync::Arc;
use eframe::egui_wgpu::{self, wgpu};
use crate::{configure_obsidian_style, RustboyApp};

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
    app: RustboyApp,
) -> Result<(), String> {
    // In eframe 0.31 with wgpu, adapter and surface capabilities are inspected
    // during adapter selection via `native_adapter_selector`.
    let mut wgpu_setup = egui_wgpu::WgpuSetupCreateNew::default();
    wgpu_setup.native_adapter_selector = Some(Arc::new(|adapters, surface| {
        let adapter = adapters
            .iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
            .or_else(|| adapters.first())
            .cloned()
            .ok_or_else(|| "No suitable wgpu adapter found".to_string())?;

        if let Some(surf) = surface {
            let mode = query_supported_present_mode(surf, &adapter);
            log::info!("Adapter selector confirmed present mode: {:?}", mode);
        }
        Ok(adapter)
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([init_width, init_height])
            .with_min_inner_size([520.0, 380.0])
            .with_resizable(true),
        wgpu_options: egui_wgpu::WgpuConfiguration {
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: Some(1),
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        title,
        options,
        Box::new(|cc| {
            configure_obsidian_style(&cc.egui_ctx);
            let mut app = app;
            if let Some(rs) = &cc.wgpu_render_state {
                app.init_wgpu(rs);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("eframe execution failed: {e}"))
}
