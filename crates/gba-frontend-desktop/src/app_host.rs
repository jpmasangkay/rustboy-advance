use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_winit::winit;
use glow::HasContext as _;
use glutin::context::NotCurrentGlContext;
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::GlSurface;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::{configure_obsidian_style, RustboyApp};

struct GlutinWindowContext {
    window: Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlutinWindowContext {
    #[allow(unsafe_code)]
    unsafe fn new(
        event_loop: &ActiveEventLoop,
        title: &str,
        init_width: f32,
        init_height: f32,
    ) -> Result<Self, String> {
        let window_attrs = WindowAttributes::default()
            .with_resizable(true)
            .with_inner_size(LogicalSize::new(init_width as f64, init_height as f64))
            .with_min_inner_size(LogicalSize::new(520.0, 380.0))
            .with_title(title)
            .with_visible(false);

        let config_template_builder = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (mut window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(window_attrs.clone()))
            .build(event_loop, config_template_builder, |mut configs| {
                configs
                    .next()
                    .expect("no matching OpenGL pixel format configuration found")
            })
            .map_err(|e| format!("failed to initialize display / GL config: {e}"))?;

        let gl_display = gl_config.display();

        let raw_window_handle = window
            .as_ref()
            .and_then(|w| w.window_handle().ok())
            .map(|h| h.as_raw());

        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        let fallback_context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);

        let not_current_gl_context = unsafe {
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| {
                    gl_config
                        .display()
                        .create_context(&gl_config, &fallback_context_attributes)
                })
                .map_err(|e| format!("failed to create OpenGL context: {e}"))?
        };

        let window = match window.take() {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, window_attrs, &gl_config)
                .map_err(|e| format!("failed to finalize window: {e}"))?,
        };

        let (width, height): (u32, u32) = window.inner_size().into();
        let width = NonZeroU32::new(width).unwrap_or(NonZeroU32::MIN);
        let height = NonZeroU32::new(height).unwrap_or(NonZeroU32::MIN);

        let raw_handle = window
            .window_handle()
            .map_err(|e| format!("window handle unavailable: {e}"))?
            .as_raw();
        let surface_attributes =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(raw_handle, width, height);

        let gl_surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attributes)
                .map_err(|e| format!("failed to create OpenGL window surface: {e}"))?
        };

        let gl_context = not_current_gl_context
            .make_current(&gl_surface)
            .map_err(|e| format!("failed to make OpenGL context current: {e}"))?;

        let _ = gl_surface.set_swap_interval(
            &gl_context,
            glutin::surface::SwapInterval::Wait(NonZeroU32::MIN),
        );

        Ok(Self {
            window,
            gl_context,
            gl_display,
            gl_surface,
        })
    }

    fn window(&self) -> &Window {
        &self.window
    }

    fn resize(&self, physical_size: winit::dpi::PhysicalSize<u32>) {
        if let (Some(w), Some(h)) = (
            NonZeroU32::new(physical_size.width),
            NonZeroU32::new(physical_size.height),
        ) {
            self.gl_surface.resize(&self.gl_context, w, h);
        }
    }

    fn swap_buffers(&self) -> glutin::error::Result<()> {
        self.gl_surface.swap_buffers(&self.gl_context)
    }

    fn get_proc_address(&self, addr: &std::ffi::CStr) -> *const std::ffi::c_void {
        self.gl_display.get_proc_address(addr)
    }
}

#[derive(Debug)]
enum UserEvent {
    RequestRepaint { when: Instant },
}

struct HostApp {
    proxy: EventLoopProxy<UserEvent>,
    title: String,
    init_width: f32,
    init_height: f32,
    app: RustboyApp,
    gl_window: Option<GlutinWindowContext>,
    gl: Option<Arc<glow::Context>>,
    egui_glow: Option<egui_glow::EguiGlow>,
    next_repaint_time: Option<Instant>,
    shown: bool,
    exited: bool,
}

impl HostApp {
    fn new(
        proxy: EventLoopProxy<UserEvent>,
        title: String,
        init_width: f32,
        init_height: f32,
        app: RustboyApp,
    ) -> Self {
        Self {
            proxy,
            title,
            init_width,
            init_height,
            app,
            gl_window: None,
            gl: None,
            egui_glow: None,
            next_repaint_time: None,
            shown: false,
            exited: false,
        }
    }

    fn schedule_repaint(&mut self, when: Instant) {
        self.next_repaint_time = Some(match self.next_repaint_time {
            Some(prev) => prev.min(when),
            None => when,
        });
    }

    fn check_redraw_requests(&mut self, event_loop: &ActiveEventLoop) {
        if self.exited {
            return;
        }
        let now = Instant::now();
        if let Some(when) = self.next_repaint_time {
            if now >= when {
                self.next_repaint_time = None;
                event_loop.set_control_flow(ControlFlow::Poll);
                if let Some(gl_window) = &self.gl_window {
                    gl_window.window().request_redraw();
                }
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(when));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn cleanup_on_exit(&mut self) {
        if !self.exited {
            self.exited = true;
            self.app.on_exit();
            if let Some(mut egui_glow) = self.egui_glow.take() {
                egui_glow.destroy();
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for HostApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gl_window.is_some() {
            return;
        }

        let gl_window = match unsafe {
            GlutinWindowContext::new(event_loop, &self.title, self.init_width, self.init_height)
        } {
            Ok(w) => w,
            Err(err) => {
                eprintln!("OpenGL initialization error: {err}");
                event_loop.exit();
                return;
            }
        };

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function(|s| {
                let Ok(cs) = std::ffi::CString::new(s) else {
                    return std::ptr::null();
                };
                gl_window.get_proc_address(&cs)
            })
        });

        let egui_glow = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        configure_obsidian_style(&egui_glow.egui_ctx);

        let proxy_mutex = egui::mutex::Mutex::new(self.proxy.clone());
        egui_glow
            .egui_ctx
            .set_request_repaint_callback(move |info| {
                let when = Instant::now()
                    .checked_add(info.delay.min(Duration::from_secs(60)))
                    .unwrap_or_else(Instant::now);
                let _ = proxy_mutex
                    .lock()
                    .send_event(UserEvent::RequestRepaint { when });
            });

        gl_window.window().set_visible(true);
        self.shown = true;

        self.gl_window = Some(gl_window);
        self.gl = Some(gl);
        self.egui_glow = Some(egui_glow);
        self.schedule_repaint(Instant::now());
        self.check_redraw_requests(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            self.cleanup_on_exit();
            event_loop.exit();
            return;
        }

        if self.exited {
            return;
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            let (Some(gl_window), Some(gl), Some(egui_glow)) = (
                self.gl_window.as_mut(),
                self.gl.as_ref(),
                self.egui_glow.as_mut(),
            ) else {
                return;
            };

            egui_glow.run(gl_window.window(), |ctx| {
                self.app.update(ctx);
            });

            if self.app.quit_requested {
                self.cleanup_on_exit();
                event_loop.exit();
                return;
            }

            unsafe {
                gl.clear_color(13.0 / 255.0, 15.0 / 255.0, 19.0 / 255.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }

            egui_glow.paint(gl_window.window());
            let _ = gl_window.swap_buffers();
            if !self.shown {
                gl_window.window().set_visible(true);
                self.shown = true;
            }

            self.check_redraw_requests(event_loop);
            return;
        }

        let repaint_now = {
            let (Some(gl_window), Some(egui_glow)) =
                (self.gl_window.as_ref(), self.egui_glow.as_mut())
            else {
                return;
            };

            let mut need_repaint = false;
            if let WindowEvent::Resized(physical_size) = &event {
                gl_window.resize(*physical_size);
                if physical_size.width > 0 && physical_size.height > 0 {
                    need_repaint = true;
                }
            }

            let response = egui_glow.on_window_event(gl_window.window(), &event);
            need_repaint || response.repaint
        };

        if repaint_now {
            self.schedule_repaint(Instant::now());
        }

        self.check_redraw_requests(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::RequestRepaint { when } => {
                self.schedule_repaint(when);
                self.check_redraw_requests(event_loop);
            }
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        self.check_redraw_requests(event_loop);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.cleanup_on_exit();
    }
}

pub fn run_desktop(
    title: &str,
    init_width: f32,
    init_height: f32,
    app: RustboyApp,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| format!("failed to build winit event loop: {e}"))?;
    let proxy = event_loop.create_proxy();
    let mut host = HostApp::new(proxy, title.to_string(), init_width, init_height, app);
    event_loop
        .run_app(&mut host)
        .map_err(|e| format!("event loop terminated with error: {e}"))
}
