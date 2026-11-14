use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use eframe::egui_wgpu::{self, wgpu};

use crate::app::{RustboyApp, configure_obsidian_style};
use crate::config::{AppConfig, DisplayFilter, HotkeyAction};
use crate::headless::CliConfig;
use crate::library::{LibraryViewMode, RomLibraryEntry};
use crate::rom_loader::{RomPlatform, RomSourceKind};
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

struct OffscreenUiRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: egui_wgpu::Renderer,
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl OffscreenUiRenderer {
    fn new(width: u32, height: u32) -> Result<Self, String> {
        let backends = wgpu::Backends::VULKAN | wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapters = instance.enumerate_adapters(backends);
        let adapter = crate::shaders::select_preferred_adapter(adapters.iter())
            .or_else(|| crate::shaders::discover_software_cpu_adapter().ok())
            .ok_or_else(|| "No offscreen wgpu adapter available".to_string())?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .map_err(|e| format!("request_device failed: {e}"))?;

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui_snapshot_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            device,
            queue,
            egui_renderer,
            tex,
            view,
            width,
            height,
        })
    }

    fn render_app_to_png(
        &mut self,
        ctx: &egui::Context,
        app: &mut RustboyApp,
        hover_pos: Option<egui::Pos2>,
        out_path: &Path,
    ) -> Result<(), String> {
        self.render_app_with_click_to_png(ctx, app, None, hover_pos, out_path)
    }

    fn render_app_with_click_to_png(
        &mut self,
        ctx: &egui::Context,
        app: &mut RustboyApp,
        click_pos: Option<egui::Pos2>,
        hover_pos: Option<egui::Pos2>,
        out_path: &Path,
    ) -> Result<(), String> {
        let screen_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.width as f32, self.height as f32),
        );

        let mut last_output = None;
        let mut to_free = Vec::new();
        for step in 0..4 {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen_rect),
                time: Some(10.0 + step as f64 * 0.25),
                ..Default::default()
            };
            if let Some(cpos) = click_pos {
                if step == 1 {
                    raw.events.push(egui::Event::PointerMoved(cpos));
                    raw.events.push(egui::Event::PointerButton {
                        pos: cpos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    });
                } else if step == 2 {
                    raw.events.push(egui::Event::PointerMoved(cpos));
                    raw.events.push(egui::Event::PointerButton {
                        pos: cpos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            if step == 3
                && let Some(pos) = hover_pos
            {
                raw.events.push(egui::Event::PointerMoved(pos));
            }
            let out = ctx.run(raw, |c| {
                app.update(c);
            });
            for (id, delta) in &out.textures_delta.set {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
            to_free.extend(out.textures_delta.free.iter().copied());
            last_output = Some(out);
        }
        let full_output = last_output.expect("at least one frame rendered");

        let clipped_primitives = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.width, self.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ui_snapshot_encoder"),
            });
        let user_cmds = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        {
            let rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui_snapshot_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.06,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut rpass = rpass.forget_lifetime();
            self.egui_renderer
                .render(&mut rpass, &clipped_primitives, &screen_descriptor);
        }

        let unpadded_bpr = self.width * 4;
        let aligned_bpr = (unpadded_bpr + 255) & !255;
        let buf_size = u64::from(aligned_bpr) * u64::from(self.height);
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui_snapshot_readback"),
            size: buf_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_bpr),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        let mut cmds = user_cmds;
        cmds.push(encoder.finish());
        self.queue.submit(cmds);
        self.device.poll(wgpu::Maintain::Wait);

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| format!("map recv failed: {e}"))?
            .map_err(|e| format!("map failed: {e:?}"))?;

        let mapped = slice.get_mapped_range();
        let mut rgba = vec![0u8; (unpadded_bpr * self.height) as usize];
        for row in 0..self.height as usize {
            let src_off = row * aligned_bpr as usize;
            let dst_off = row * unpadded_bpr as usize;
            rgba[dst_off..dst_off + unpadded_bpr as usize]
                .copy_from_slice(&mapped[src_off..src_off + unpadded_bpr as usize]);
        }
        drop(mapped);
        staging.unmap();

        for id in &to_free {
            self.egui_renderer.free_texture(id);
        }

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        image::save_buffer(
            out_path,
            &rgba,
            self.width,
            self.height,
            image::ColorType::Rgba8,
        )
        .map_err(|e| format!("save PNG failed: {e}"))
    }
}

fn make_isolated_app() -> ManuallyDrop<RustboyApp> {
    let mut app = RustboyApp::new(CliConfig::default());
    app.library_scanning = false;
    app.library_scan_rx = None;
    app.library_scan_error = None;
    app.rom_folders = vec![
        PathBuf::from(r"C:\Games\GBA"),
        PathBuf::from(r"C:\Games\GBC"),
    ];
    app.config = AppConfig::default();
    app.config.display_filter = DisplayFilter::SoftHandheld;
    app.favorites.clear();
    let p1 = PathBuf::from(r"C:\Games\GBA\Summon Night - Swordcraft Story (USA).gba");
    let p2 = PathBuf::from(r"C:\Games\GBA\Homebrew Micro Demo.gba");
    let p3 = PathBuf::from(r"C:\Games\GBC\Links Awakening DX.gbc");
    app.favorites.insert(p1.clone());
    app.library_entries = vec![
        RomLibraryEntry {
            path: p1,
            display_name: "Summon Night - Swordcraft Story (USA)".to_string(),
            internal_title: "SUMMON NIGHT".to_string(),
            game_code: "BSNE".to_string(),
            region: "USA",
            size_mb: 16.0,
            has_battery_sav: true,
            source_kind: RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: String::new(),
        },
        RomLibraryEntry {
            path: p2,
            display_name: "Homebrew Micro Demo (Long Verifiable Title Showcase Edition)"
                .to_string(),
            internal_title: "MICRODEMO".to_string(),
            game_code: "HOMZ".to_string(),
            region: "",
            size_mb: 0.125,
            has_battery_sav: false,
            source_kind: RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: String::new(),
        },
        RomLibraryEntry {
            path: p3,
            display_name: "Links Awakening DX".to_string(),
            internal_title: "ZELDA".to_string(),
            game_code: "C123".to_string(),
            region: "World",
            size_mb: 1.0,
            has_battery_sav: true,
            source_kind: RomSourceKind::Zip,
            platform: RomPlatform::Gbc,
            cart_type_label: "MBC5+RAM+BATT".to_string(),
        },
    ];
    ManuallyDrop::new(app)
}

fn load_synthetic_active_game(app: &mut RustboyApp) {
    let rom = crate::stress_rom::build_stress_mode0_4bg_128spr_rom();
    let core = gba_core::GbaCore::new(None, rom);
    app.core = Some(core);
    app.rom_title = "Summon Night".to_string();
    app.rom_code = "BSNE".to_string();
    app.paused = false;
    app.show_pause_overlay = false;
    app.measured_fps = 59.73;

    // Populate recognizable test pattern in core framebuffer
    let mut fb = [0u16; LCD_WIDTH * LCD_HEIGHT];
    for y in 0..LCD_HEIGHT {
        for x in 0..LCD_WIDTH {
            let r = ((x * 31) / LCD_WIDTH) as u16;
            let g = ((y * 31) / LCD_HEIGHT) as u16;
            let b = if (x / 16 + y / 16) % 2 == 0 { 22 } else { 8 };
            fb[y * LCD_WIDTH + x] = (b << 10) | (g << 5) | r;
        }
    }
    app.sink.push_frame(&fb);
}

#[test]
fn capture_a1_ui_screenshots_if_requested() {
    let Some(dir) = std::env::var_os("RBA_SCREENSHOT_DIR").map(PathBuf::from) else {
        // Lightweight non-GPU UI logic smoke check
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        let _ = ctx.run(Default::default(), |c| app.update(c));
        return;
    };

    let mut renderer =
        OffscreenUiRenderer::new(1120, 720).expect("offscreen wgpu renderer for screenshots");

    // 1. Library Table view (2 Folders, no "Unk", MB/KB formatting, Favorites stars, clean status bar)
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.library_view_mode = LibraryViewMode::Table;
        let _ = HotkeyAction::Pause;
        renderer
            .render_app_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(120.0, 165.0)),
                &dir.join("a1_library_table.png"),
            )
            .expect("render table");
    }

    // 2. Library Grid view (1 Folder singular grammar, consistent sizes, top-right ★)
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.rom_folders.truncate(1);
        app.library_view_mode = LibraryViewMode::Grid;
        renderer
            .render_app_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(95.0, 210.0)),
                &dir.join("a1_library_grid.png"),
            )
            .expect("render grid");
    }

    // 3. In-game HUD/Toolbar (developer_tools = false -> hides kHz/DRC/BIOS, shows FPS %)
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.config.developer_tools = false;
        // Hover over Pause toolbar button area
        renderer
            .render_app_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(230.0, 38.0)),
                &dir.join("a1_gameplay_hud.png"),
            )
            .expect("render gameplay hud");
    }

    // 4. In-game HUD with developer_tools = true (restores BIOS/DRC/kHz)
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.config.developer_tools = true;
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a1_gameplay_devtools.png"))
            .expect("render devtools status bar");
    }
}

#[test]
fn capture_a2_ui_screenshots_if_requested() {
    use crate::settings_ui::{PerfCheckState, parse_selftest_report_with_json};

    let Some(dir) = std::env::var_os("RBA_SCREENSHOT_DIR").map(PathBuf::from) else {
        // Lightweight non-GPU smoke check for Settings & Performance Check dialogs
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.show_settings = true;
        app.show_perf_check = true;
        let _ = ctx.run(Default::default(), |c| app.update(c));
        return;
    };
    let _ = std::fs::create_dir_all(&dir);

    let mut renderer =
        OffscreenUiRenderer::new(1120, 720).expect("offscreen wgpu renderer for A2 screenshots");

    // 1. 5-menu bar + 9-button toolbar + Play menu dropdown (normal player mode)
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.config.developer_tools = false;
        renderer
            .render_app_with_click_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(46.0, 12.0)),
                Some(egui::pos2(60.0, 95.0)),
                &dir.join("a2_menus_and_toolbar.png"),
            )
            .expect("render A2 menus and toolbar");
    }

    // 2. Volume popup anchored under toolbar
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.show_volume_popup = true;
        app.volume_popup_anchor = Some(egui::pos2(540.0, 56.0));
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a2_volume_popup.png"))
            .expect("render A2 volume popup");
    }

    // 2b. Help menu expanded
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        renderer
            .render_app_with_click_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(212.0, 12.0)),
                Some(egui::pos2(230.0, 70.0)),
                &dir.join("a2_help_menu.png"),
            )
            .expect("render A2 Help menu");
    }

    // 3. Settings dialog with Developer tools switch
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.show_settings = true;
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a2_settings.png"))
            .expect("render A2 Settings dialog");
    }

    // 4. Performance Check (--selftest) modal with summary + Copy/Save
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        let sample = "\
=== RUSTBOY ADVANCE PORTABLE --SELFTEST (PASS) ===\n\
  Build:             commit=072e4078c7b3 | built=2026-11-14T09:22:04Z | exe_sha256=9a11a338ca98028e\n\
  Host:              AMD Ryzen 9 8945HX with Radeon Graphics (16c/32t) | RAM=32560 MB | OS=Windows 11\n\
  Enumerated GPUs:   2 adapter(s) visible to wgpu\n\
    [1] NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan / DiscreteGpu)\n\
    [2] Microsoft Basic Render Driver (Dx12 / Cpu)\n\
  Chosen Adapter:    NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan / DiscreteGpu) [gpu-shader]\n\
  Calibration:       75.527 ms (ref 75.769 ms, ratio vs ref = 1.00x)\n\
  Headless (600f):   998.1 FPS | headroom = 16.71x | emu_avg=0.836 ms | total avg=1.002 ms\n\
  Paced (600f +120w): work avg=1.042 p95=1.368 p99.5=1.494 ms | over_budget=0 (0.000%)\n\
  Slowdown Sweep:    [1x(p99.5=1.91ms,PASS), 8x(p99.5=12.05ms,PASS), 12x(p99.5=18.28ms,FAIL)]\n\
  Measured Ceiling:  smooth (<0.5% over, 0 underruns) = 8x | zero-over-budget = 8x\n\
  Recommended Filter: smooth-bilinear — Hardware GPU shader path provides clean interpolation\n";
        let full_json = serde_json::json!({
            "schema_version": 3,
            "exe_sha256": "9a11a338ca98028ee0dfd44d542f71e326ea49890979ad658938b201fbb98fb4",
            "git_commit": "072e4078c7b3",
            "build_time_utc": "2026-11-14T09:22:04Z",
            "calibration_ratio_vs_ref": 1.0032,
            "available_adapters": [
                { "name": "NVIDIA GeForce RTX 5070 Laptop GPU", "backend": "Vulkan" }
            ],
            "slowdown_sweep": [
                { "slowdown_x": 8, "smooth_passed": true }
            ],
            "measured_smooth_ceiling_x": 8,
            "measured_zero_over_budget_x": 8,
            "recommended_filter": "smooth-bilinear",
            "passed": true
        })
        .to_string();
        app.show_perf_check = true;
        app.perf_check = PerfCheckState::Finished(Box::new(parse_selftest_report_with_json(
            sample,
            "",
            Some(full_json),
            true,
            11340,
        )));
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a2_perf_check.png"))
            .expect("render A2 Performance Check modal");
    }

    // 5. Tools menu with developer_tools = true
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        load_synthetic_active_game(&mut app);
        app.config.developer_tools = true;
        renderer
            .render_app_with_click_to_png(
                &ctx,
                &mut app,
                Some(egui::pos2(190.0, 12.0)),
                Some(egui::pos2(210.0, 290.0)),
                &dir.join("a2_devtools_menu.png"),
            )
            .expect("render A2 Tools developer menu");
    }
}

#[test]
fn capture_a3_ui_screenshots_if_requested() {
    use crate::library::TableSortColumn;

    let Some(dir) = std::env::var_os("RBA_SCREENSHOT_DIR").map(PathBuf::from) else {
        // Lightweight non-GPU smoke check for Grid sort & Empty state
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.config.auto_download_covers = false;
        app.library_view_mode = LibraryViewMode::Grid;
        app.table_sort_column = TableSortColumn::Size;
        app.table_sort_descending = true;
        let _ = ctx.run(Default::default(), |c| app.update(c));
        app.rom_folders.clear();
        app.library_entries.clear();
        let _ = ctx.run(Default::default(), |c| app.update(c));
        return;
    };
    let _ = std::fs::create_dir_all(&dir);

    let mut renderer =
        OffscreenUiRenderer::new(1120, 720).expect("offscreen wgpu renderer for A3 screenshots");

    // 1. Multi-row Grid view sorted by Size descending with even tile alignment & Homebrew badge
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.config.auto_download_covers = false;
        app.library_view_mode = LibraryViewMode::Grid;
        app.table_sort_column = TableSortColumn::Size;
        app.table_sort_descending = true;
        app.library_entries = vec![
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Summon Night - Swordcraft Story (USA).gba"),
                display_name: "Summon Night - Swordcraft Story (USA)".to_string(),
                internal_title: "SUMMON NIGHT".to_string(),
                game_code: "BSNE".to_string(),
                region: "USA",
                size_mb: 16.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA Cart".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Golden Sun - The Lost Age (USA).gba"),
                display_name: "Golden Sun - The Lost Age (USA)".to_string(),
                internal_title: "GOLDEN SUN 2".to_string(),
                game_code: "AGFE".to_string(),
                region: "USA",
                size_mb: 16.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA Cart".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Metroid Fusion (USA).gba"),
                display_name: "Metroid Fusion (USA)".to_string(),
                internal_title: "METROID4".to_string(),
                game_code: "AMTE".to_string(),
                region: "USA",
                size_mb: 8.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA Cart".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Advance Wars (EUR).gba"),
                display_name: "Advance Wars (EUR)".to_string(),
                internal_title: "ADVANCEWARS".to_string(),
                game_code: "AWRP".to_string(),
                region: "EUR",
                size_mb: 4.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA Cart".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBC\Links Awakening DX.gbc"),
                display_name: "Links Awakening DX".to_string(),
                internal_title: "ZELDA".to_string(),
                game_code: "C123".to_string(),
                region: "World",
                size_mb: 1.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Zip,
                platform: RomPlatform::Gbc,
                cart_type_label: "MBC5+RAM+BATT".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Castlevania - Circle of the Moon (JPN).gba"),
                display_name: "Castlevania - Circle of the Moon (JPN)".to_string(),
                internal_title: "DRACULA".to_string(),
                game_code: "AAMJ".to_string(),
                region: "JPN",
                size_mb: 8.0,
                has_battery_sav: true,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA Cart".to_string(),
            },
            RomLibraryEntry {
                path: PathBuf::from(r"C:\Games\GBA\Homebrew Micro Demo.gba"),
                display_name: "Homebrew Micro Demo (Unheadered ROM)".to_string(),
                internal_title: "Homebrew".to_string(),
                game_code: "HOME".to_string(),
                region: "",
                size_mb: 0.125,
                has_battery_sav: false,
                source_kind: RomSourceKind::Raw,
                platform: RomPlatform::Gba,
                cart_type_label: "GBA ROM".to_string(),
            },
        ];
        app.favorites
            .insert(PathBuf::from(r"C:\Games\GBA\Metroid Fusion (USA).gba"));
        app.selected_rom_path = Some(PathBuf::from(r"C:\Games\GBA\Metroid Fusion (USA).gba"));
        renderer
            .render_app_to_png(
                &ctx,
                &mut app,
                None,
                &dir.join("a3_library_grid_sorted.png"),
            )
            .expect("render A3 sorted grid");
    }

    // 2. Empty-state card with "How to play" hint
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.config.auto_download_covers = false;
        app.rom_folders.clear();
        app.library_entries.clear();
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a3_empty_state.png"))
            .expect("render A3 empty state");
    }

    // 3. Persistent error message after attempting to launch a deleted/missing ROM file
    {
        let ctx = egui::Context::default();
        configure_obsidian_style(&ctx);
        let mut app = make_isolated_app();
        app.config.auto_download_covers = false;
        app.library_view_mode = LibraryViewMode::Table;
        let ghost = PathBuf::from(r"C:\Games\GBA\Deleted Ghost Game.gba");
        app.library_entries.push(RomLibraryEntry {
            path: ghost.clone(),
            display_name: "Deleted Ghost Game".to_string(),
            internal_title: "GHOST".to_string(),
            game_code: "GHST".to_string(),
            region: "USA",
            size_mb: 8.0,
            has_battery_sav: false,
            source_kind: RomSourceKind::Raw,
            platform: RomPlatform::Gba,
            cart_type_label: "GBA Cart".to_string(),
        });
        app.launch_library_rom(&ghost, &ctx);
        renderer
            .render_app_to_png(&ctx, &mut app, None, &dir.join("a3_missing_file_error.png"))
            .expect("render A3 missing file error");
    }
}
