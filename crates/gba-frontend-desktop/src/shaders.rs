use std::sync::Arc;

use eframe::egui_wgpu::{self, wgpu};
use egui::PaintCallback;
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::config::{ColorProfileChoice, ShaderPreset};
use crate::filters::fill_linear_rgba;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub src_size: [f32; 2],
    pub dst_size: [f32; 2],
    pub ndc_rect: [f32; 4],
    pub color_profile: i32,
    pub frame_blend: f32,
    pub lcd_ghosting: f32,
    pub _pad: f32,
}

pub const fn color_profile_uniform(color_correction: bool, profile: ColorProfileChoice) -> i32 {
    if !color_correction {
        return 0;
    }
    match profile {
        ColorProfileChoice::Linear => 0,
        ColorProfileChoice::Agb001 => 1,
        ColorProfileChoice::Ags101 => 2,
        ColorProfileChoice::Ags001 => 3,
    }
}

pub const WGSL_SOURCE: &str = r#"
struct Uniforms {
    src_size: vec2<f32>,
    dst_size: vec2<f32>,
    ndc_rect: vec4<f32>,
    color_profile: i32,
    frame_blend: f32,
    lcd_ghosting: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var curr_tex: texture_2d<f32>;
@group(0) @binding(2) var prev_tex: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let uv = uvs[in_vertex_index];
    let x = mix(uniforms.ndc_rect.x, uniforms.ndc_rect.z, uv.x);
    let y = mix(uniforms.ndc_rect.y, uniforms.ndc_rect.w, uv.y);

    var out: VertexOutput;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn safe_pow(v: vec3<f32>, e: f32) -> vec3<f32> {
    let m = step(vec3<f32>(0.000001), v);
    return m * pow(max(v, vec3<f32>(0.000001)), vec3<f32>(e));
}

fn apply_color_profile(c: vec3<f32>, profile: i32) -> vec3<f32> {
    if (profile == 1) {
        let lin = safe_pow(c, 2.4);
        let mapped = vec3<f32>(
            dot(lin, vec3<f32>(0.845, 0.125, 0.030)),
            dot(lin, vec3<f32>(0.055, 0.675, 0.270)),
            dot(lin, vec3<f32>(0.000, 0.195, 0.805))
        );
        return clamp(safe_pow(mapped, 1.0 / 2.2), vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (profile == 2) {
        let mapped = vec3<f32>(
            dot(c, vec3<f32>(0.94, 0.04, 0.02)),
            dot(c, vec3<f32>(0.03, 0.92, 0.05)),
            dot(c, vec3<f32>(0.01, 0.07, 0.92))
        );
        return clamp(safe_pow(mapped, 1.15), vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (profile == 3) {
        let mapped = vec3<f32>(
            0.04 + 0.82 * dot(c, vec3<f32>(0.84, 0.12, 0.04)),
            0.05 + 0.82 * dot(c, vec3<f32>(0.06, 0.82, 0.12)),
            0.08 + 0.84 * dot(c, vec3<f32>(0.04, 0.10, 0.86))
        );
        return clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn sample_texel(coord: vec2<i32>) -> vec3<f32> {
    let max_coord = vec2<i32>(uniforms.src_size) - vec2<i32>(1);
    let c = clamp(coord, vec2<i32>(0), max_coord);
    let curr_raw = textureLoad(curr_tex, c, 0).rgb;
    let curr = apply_color_profile(curr_raw, uniforms.color_profile);
    let temporal = clamp(max(uniforms.frame_blend, uniforms.lcd_ghosting), 0.0, 0.5);
    if (temporal > 0.001) {
        let prev_raw = textureLoad(prev_tex, c, 0).rgb;
        let prev = apply_color_profile(prev_raw, uniforms.color_profile);
        return mix(curr, prev, temporal);
    }
    return curr;
}

fn sample_sharp_bilinear(uv: vec2<f32>) -> vec3<f32> {
    let scale = max(uniforms.dst_size / max(uniforms.src_size, vec2<f32>(1.0)), vec2<f32>(1.0));
    let pos = uv * uniforms.src_size - vec2<f32>(0.5);
    let p0 = vec2<i32>(floor(pos));
    let f = pos - vec2<f32>(p0);
    let w = clamp((f - vec2<f32>(0.5)) * scale + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));

    let c00 = sample_texel(p0);
    let c10 = sample_texel(p0 + vec2<i32>(1, 0));
    let c01 = sample_texel(p0 + vec2<i32>(0, 1));
    let c11 = sample_texel(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, w.x), mix(c01, c11, w.x), w.y);
}

@fragment
fn fs_nearest(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(in.uv * uniforms.src_size));
    return vec4<f32>(sample_texel(texel), 1.0);
}

@fragment
fn fs_sharp_bilinear(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(sample_sharp_bilinear(in.uv), 1.0);
}

@fragment
fn fs_lcd_grid(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = sample_sharp_bilinear(in.uv);
    let scale_xy = uniforms.dst_size / max(uniforms.src_size, vec2<f32>(1.0));
    let scale = min(scale_xy.x, scale_xy.y);

    let sub = fract(in.uv * uniforms.src_size);

    let grid_strength = smoothstep(1.4, 2.8, scale);
    let row_gap = smoothstep(0.34, 0.50, abs(sub.y - 0.5));
    let col_gap = smoothstep(0.38, 0.50, abs(sub.x - 0.5));
    let grid_mask = (1.0 - grid_strength * 0.28 * row_gap) * (1.0 - grid_strength * 0.14 * col_gap);

    let triad_strength = smoothstep(2.2, 3.0, scale) * 0.72;
    let rw = exp(-18.0 * (sub.x - 1.0 / 6.0) * (sub.x - 1.0 / 6.0));
    let gw = exp(-18.0 * (sub.x - 0.5) * (sub.x - 0.5));
    let bw = exp(-18.0 * (sub.x - 5.0 / 6.0) * (sub.x - 5.0 / 6.0));
    let triad = mix(vec3<f32>(1.0), vec3<f32>(0.76) + 0.44 * vec3<f32>(rw, gw, bw), triad_strength);

    let gain = 1.0 + 0.10 * grid_strength;
    let col = clamp(base * triad * grid_mask * gain, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(col, 1.0);
}

fn crt_curve(uv: vec2<f32>) -> vec2<f32> {
    let c0 = uv * 2.0 - vec2<f32>(1.0);
    let offset = c0.yx * c0.yx * vec2<f32>(0.050, 0.070);
    let c = c0 + c0 * offset;
    return c * 0.5 + vec2<f32>(0.5);
}

@fragment
fn fs_crt_scanlines(in: VertexOutput) -> @location(0) vec4<f32> {
    let curved_uv = crt_curve(in.uv);
    let edge_dist = abs(curved_uv - vec2<f32>(0.5)) * 2.0;

    let corner_d = max(edge_dist - vec2<f32>(0.94), vec2<f32>(0.0));
    let rounded_sdf = length(corner_d);
    let edge_mask = (1.0 - smoothstep(0.985, 1.005, max(edge_dist.x, edge_dist.y)))
                  * (1.0 - smoothstep(0.055, 0.080, rounded_sdf));

    if (edge_mask <= 0.001) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let base = sample_sharp_bilinear(clamp(curved_uv, vec2<f32>(0.0), vec2<f32>(1.0)));

    let texel_step = vec2<f32>(1.0) / max(uniforms.src_size, vec2<f32>(1.0));
    let glow = (
        sample_sharp_bilinear(clamp(curved_uv + vec2<f32>(texel_step.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv - vec2<f32>(texel_step.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv + vec2<f32>(0.0, texel_step.y), vec2<f32>(0.0), vec2<f32>(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv - vec2<f32>(0.0, texel_step.y), vec2<f32>(0.0), vec2<f32>(1.0)))
    ) * 0.25;

    let luma = dot(base, vec3<f32>(0.299, 0.587, 0.114));
    let scale_xy = uniforms.dst_size / max(uniforms.src_size, vec2<f32>(1.0));
    let scale = min(scale_xy.x, scale_xy.y);
    let scan_strength = smoothstep(1.2, 2.5, scale);

    let dy = abs(fract(curved_uv.y * uniforms.src_size.y) - 0.5);
    let beam_width = mix(0.28, 0.48, luma);
    let norm_dy = dy / beam_width;
    let beam = exp(-norm_dy * norm_dy * 1.35);
    let scanline = mix(1.0, mix(0.48, 1.12, beam), scan_strength);

    let grille_phase = sin(in.uv.x * uniforms.dst_size.x * 3.14159265);
    let grille = mix(1.0, 0.95 + 0.05 * grille_phase, smoothstep(2.0, 4.0, scale));

    let centered = curved_uv * 2.0 - vec2<f32>(1.0);
    let vignette = clamp(1.0 - 0.16 * dot(centered, centered), 0.72, 1.0);

    let col = (base * scanline + glow * (0.12 + 0.08 * luma)) * grille * vignette * edge_mask;
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

fn yuv_dist(c1: vec3<f32>, c2: vec3<f32>) -> f32 {
    let d = c1 - c2;
    let y = abs(dot(d, vec3<f32>(0.299, 0.587, 0.114)));
    let u = abs(dot(d, vec3<f32>(-0.14713, -0.28886, 0.436)));
    let v = abs(dot(d, vec3<f32>(0.615, -0.51499, -0.10001)));
    return 48.0 * y + 7.0 * u + 6.0 * v;
}

@fragment
fn fs_scale_edge(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel_pos = in.uv * uniforms.src_size;
    let p = vec2<i32>(floor(texel_pos));
    let f = fract(texel_pos) - vec2<f32>(0.5);

    let sx = select(-1, 1, f.x >= 0.0);
    let sy = select(-1, 1, f.y >= 0.0);
    let q = abs(f);

    let E  = sample_texel(p);
    let F  = sample_texel(p + vec2<i32>(sx, 0));
    let H  = sample_texel(p + vec2<i32>(0, sy));
    let I  = sample_texel(p + vec2<i32>(sx, sy));
    let D  = sample_texel(p + vec2<i32>(-sx, 0));
    let B  = sample_texel(p + vec2<i32>(0, -sy));
    let C  = sample_texel(p + vec2<i32>(sx, -sy));
    let G  = sample_texel(p + vec2<i32>(-sx, sy));
    let F4 = sample_texel(p + vec2<i32>(2 * sx, 0));
    let I4 = sample_texel(p + vec2<i32>(2 * sx, sy));
    let H5 = sample_texel(p + vec2<i32>(0, 2 * sy));
    let I5 = sample_texel(p + vec2<i32>(sx, 2 * sy));

    let df_EF = yuv_dist(E, F);
    let df_EH = yuv_dist(E, H);
    let df_FH = yuv_dist(F, H);
    let df_EI = yuv_dist(E, I);

    let aa = clamp(
        max(uniforms.src_size.x / max(uniforms.dst_size.x, 1.0), uniforms.src_size.y / max(uniforms.dst_size.y, 1.0)) * 0.75,
        0.02,
        0.35
    );

    var out_col = E;

    if (df_EF > 1.2 && df_EH > 1.2) {
        let df_EC = yuv_dist(E, C);
        let df_EG = yuv_dist(E, G);
        let df_IH5 = yuv_dist(I, H5);
        let df_IF4 = yuv_dist(I, F4);
        let df_HD = yuv_dist(H, D);
        let df_HI5 = yuv_dist(H, I5);
        let df_FB = yuv_dist(F, B);
        let df_FI4 = yuv_dist(F, I4);

        let w_diag = df_EC + df_EG + df_IH5 + df_IF4 + 4.0 * df_FH;
        let w_orth = df_HD + df_HI5 + df_FB + df_FI4 + 4.0 * df_EI;

        let df_EB = yuv_dist(E, B);
        let df_ED = yuv_dist(E, D);
        let connected = (min(min(df_EC, df_EG), min(df_EB, df_ED)) < 4.0) || (df_FH < 2.5);

        if (w_diag < w_orth && connected) {
            let df_FG = yuv_dist(F, G);
            let df_HC = yuv_dist(H, C);
            let shallow_h = (2.0 * df_FG <= df_EC) && (df_EI > 1.0) && (df_EF > df_FG);
            let steep_v = (2.0 * df_HC <= df_EG) && (df_EI > 1.0) && (df_EH > df_HC);

            var corner_col = select(H, F, df_EF <= df_EH);
            if (df_FH < 3.0) {
                corner_col = mix(F, H, 0.5);
            }

            var edge_val = 0.0;
            if (shallow_h && !steep_v) {
                edge_val = smoothstep(0.5 - aa, 0.5 + aa, 0.5 * q.x + q.y);
            } else if (steep_v && !shallow_h) {
                edge_val = smoothstep(0.5 - aa, 0.5 + aa, q.x + 0.5 * q.y);
            } else {
                edge_val = smoothstep(0.5 - aa, 0.5 + aa, q.x + q.y);
            }

            out_col = mix(E, corner_col, edge_val);
        }
    }

    return vec4<f32>(out_col, 1.0);
}
"#;

#[derive(Clone, Copy, Debug)]
pub struct ShaderRenderParams {
    pub preset: ShaderPreset,
    pub color_profile_id: i32,
    pub frame_blend: f32,
    pub lcd_ghosting: f32,
}

pub struct WgpuShaderPipeline {
    _device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    curr_tex: wgpu::Texture,
    prev_tex: wgpu::Texture,
    uniform_buf: Arc<wgpu::Buffer>,
    bind_group: Arc<wgpu::BindGroup>,
    pipelines: [Arc<wgpu::RenderPipeline>; ShaderPreset::COUNT],
    curr_staging: Vec<u8>,
    prev_staging: Vec<u8>,
    upload_dirty: bool,
    has_uploaded_once: bool,
    warned_failure: bool,
}

impl WgpuShaderPipeline {
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        target_format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gba_postproc_wgsl"),
            source: wgpu::ShaderSource::Wgsl(WGSL_SOURCE.into()),
        });

        let tex_desc = |label: &'static str| wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: LCD_WIDTH as u32,
                height: LCD_HEIGHT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };

        let curr_tex = device.create_texture(&tex_desc("gba_curr_frame"));
        let prev_tex = device.create_texture(&tex_desc("gba_prev_frame"));

        let curr_view = curr_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let prev_view = prev_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let initial_uniforms = Uniforms {
            src_size: [LCD_WIDTH as f32, LCD_HEIGHT as f32],
            dst_size: [LCD_WIDTH as f32, LCD_HEIGHT as f32],
            ndc_rect: [-1.0, 1.0, 1.0, -1.0],
            color_profile: 0,
            frame_blend: 0.0,
            lcd_ghosting: 0.0,
            _pad: 0.0,
        };

        let uniform_buf = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gba_shader_uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&initial_uniforms));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gba_shader_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let bind_group = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gba_shader_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&curr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&prev_view),
                },
            ],
        }));

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gba_shader_pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let entry_points = [
            "fs_nearest",
            "fs_sharp_bilinear",
            "fs_lcd_grid",
            "fs_crt_scanlines",
            "fs_scale_edge",
        ];

        let mut pipelines: [Option<Arc<wgpu::RenderPipeline>>; ShaderPreset::COUNT] = Default::default();
        for (i, entry) in entry_points.iter().enumerate() {
            let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            pipelines[i] = Some(Arc::new(pipe));
        }

        let pipelines = [
            pipelines[0].clone().unwrap(),
            pipelines[1].clone().unwrap(),
            pipelines[2].clone().unwrap(),
            pipelines[3].clone().unwrap(),
            pipelines[4].clone().unwrap(),
        ];

        let blank = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4];

        Ok(Self {
            _device: device,
            queue,
            curr_tex,
            prev_tex,
            uniform_buf,
            bind_group,
            pipelines,
            curr_staging: blank.clone(),
            prev_staging: blank,
            upload_dirty: false,
            has_uploaded_once: false,
            warned_failure: false,
        })
    }

    pub fn is_preset_available(&self, _preset: ShaderPreset) -> bool {
        true
    }

    pub fn needs_initial_upload(&self) -> bool {
        !self.has_uploaded_once
    }

    pub fn stage_frames(
        &mut self,
        curr_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        has_prev: bool,
    ) {
        fill_linear_rgba(curr_fb, &mut self.curr_staging);
        if has_prev {
            fill_linear_rgba(prev_fb, &mut self.prev_staging);
        } else {
            self.prev_staging.copy_from_slice(&self.curr_staging);
        }
        self.upload_dirty = true;
        self.has_uploaded_once = true;
    }

    pub fn log_fallback_warning_once(&mut self, msg: &str) {
        if !self.warned_failure {
            log::warn!("{msg}");
            self.warned_failure = true;
        }
    }

    pub fn flush_uploads(&mut self) {
        if !self.upload_dirty {
            return;
        }

        let extent = wgpu::Extent3d {
            width: LCD_WIDTH as u32,
            height: LCD_HEIGHT as u32,
            depth_or_array_layers: 1,
        };

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.curr_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.curr_staging,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                rows_per_image: Some(LCD_HEIGHT as u32),
            },
            extent,
        );

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.prev_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.prev_staging,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                rows_per_image: Some(LCD_HEIGHT as u32),
            },
            extent,
        );

        self.upload_dirty = false;
    }

    pub fn create_viewport_paint_callback(
        &mut self,
        rect: egui::Rect,
        params: ShaderRenderParams,
    ) -> PaintCallback {
        self.flush_uploads();

        let uniforms = Uniforms {
            src_size: [LCD_WIDTH as f32, LCD_HEIGHT as f32],
            dst_size: [rect.width(), rect.height()],
            ndc_rect: [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
            color_profile: params.color_profile_id,
            frame_blend: params.frame_blend,
            lcd_ghosting: params.lcd_ghosting,
            _pad: 0.0,
        };

        let cb = WgpuViewportCallback {
            pipeline: self.pipelines[params.preset.index()].clone(),
            bind_group: self.bind_group.clone(),
            uniform_buf: self.uniform_buf.clone(),
            uniforms,
        };

        egui_wgpu::Callback::new_paint_callback(rect, cb)
    }
}

pub struct WgpuViewportCallback {
    pipeline: Arc<wgpu::RenderPipeline>,
    bind_group: Arc<wgpu::BindGroup>,
    uniform_buf: Arc<wgpu::Buffer>,
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for WgpuViewportCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut uniforms = self.uniforms;
        let ppp = screen_descriptor.pixels_per_point;
        let screen_w = screen_descriptor.size_in_pixels[0] as f32;
        let screen_h = screen_descriptor.size_in_pixels[1] as f32;

        if screen_w > 0.0 && screen_h > 0.0 {
            let left_px = uniforms.ndc_rect[0] * ppp;
            let top_px = uniforms.ndc_rect[1] * ppp;
            let right_px = uniforms.ndc_rect[2] * ppp;
            let bottom_px = uniforms.ndc_rect[3] * ppp;

            uniforms.ndc_rect = [
                (left_px / screen_w) * 2.0 - 1.0,
                1.0 - (top_px / screen_h) * 2.0,
                (right_px / screen_w) * 2.0 - 1.0,
                1.0 - (bottom_px / screen_h) * 2.0,
            ];
            uniforms.dst_size = [
                (right_px - left_px).abs().max(1.0),
                (bottom_px - top_px).abs().max(1.0),
            ];
        }

        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let clip = info.clip_rect_in_pixels();
        if clip.width_px > 0 && clip.height_px > 0 {
            render_pass.set_scissor_rect(
                clip.left_px.max(0) as u32,
                clip.top_px.max(0) as u32,
                clip.width_px as u32,
                clip.height_px as u32,
            );
            render_pass.set_pipeline(self.pipeline.as_ref());
            render_pass.set_bind_group(0, self.bind_group.as_ref(), &[]);
            render_pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniforms_layout_and_size() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 48);
        assert_eq!(std::mem::size_of::<Uniforms>() % 16, 0);
    }

    #[test]
    fn test_color_profile_uniform_mapping() {
        assert_eq!(color_profile_uniform(false, ColorProfileChoice::Agb001), 0);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Linear), 0);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Agb001), 1);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Ags101), 2);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Ags001), 3);
    }

    #[test]
    fn test_wgsl_shader_module_validates_with_wgpu_naga() {
        let module = wgpu::naga::front::wgsl::parse_str(WGSL_SOURCE)
            .expect("WGSL post-processing shader failed to parse");
        let entry_points: Vec<&str> = module.entry_points.iter().map(|ep| ep.name.as_str()).collect();
        assert!(entry_points.contains(&"vs_main"));
        assert!(entry_points.contains(&"fs_nearest"));
        assert!(entry_points.contains(&"fs_sharp_bilinear"));
        assert!(entry_points.contains(&"fs_lcd_grid"));
        assert!(entry_points.contains(&"fs_crt_scanlines"));
        assert!(entry_points.contains(&"fs_scale_edge"));
    }
}
