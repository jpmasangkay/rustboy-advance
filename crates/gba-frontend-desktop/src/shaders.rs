use std::sync::Arc;
use std::time::Instant;

use eframe::egui_wgpu::{self, wgpu};
use egui::PaintCallback;
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::config::{ColorProfileChoice, DisplayFilter, ShaderPreset};
use crate::filters::{
    FilterCost, VideoPostProcessor, fill_linear_rgba, sort_filter_costs, test_filter_pattern_fb,
};

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
fn fs_plain(in: VertexOutput) -> @location(0) vec4<f32> {
    let max_coord = vec2<i32>(uniforms.src_size) - vec2<i32>(1);
    let texel = clamp(vec2<i32>(in.uv * uniforms.src_size), vec2<i32>(0), max_coord);
    return textureLoad(curr_tex, texel, 0);
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
    plain_pipeline: Arc<wgpu::RenderPipeline>,
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

        let mut pipelines: [Option<Arc<wgpu::RenderPipeline>>; ShaderPreset::COUNT] =
            Default::default();
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

        let plain_pipeline = Arc::new(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fs_plain"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_plain"),
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
        }));

        let blank = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4];

        Ok(Self {
            _device: device,
            queue,
            curr_tex,
            prev_tex,
            uniform_buf,
            bind_group,
            pipelines,
            plain_pipeline,
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

    pub fn render_to_texture_view(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        preset: ShaderPreset,
        color_profile_id: i32,
        dst_width: u32,
        dst_height: u32,
    ) {
        let pipeline = &self.pipelines[preset as usize];

        let uniforms = Uniforms {
            src_size: [LCD_WIDTH as f32, LCD_HEIGHT as f32],
            dst_size: [dst_width as f32, dst_height as f32],
            ndc_rect: [-1.0, 1.0, 1.0, -1.0],
            color_profile: color_profile_id,
            frame_blend: 0.0,
            lcd_ghosting: 0.0,
            _pad: 0.0,
        };
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bench_filter_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        rpass.set_pipeline(pipeline.as_ref());
        rpass.set_bind_group(0, self.bind_group.as_ref(), &[]);
        rpass.draw(0..6, 0..1);
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

static VK_ICD_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Locate an x86-64 CPU software Vulkan ICD manifest (SwiftShader on Windows,
/// Mesa Lavapipe on Linux) when the caller requests a true software GPU path.
pub fn find_software_vulkan_icd() -> Option<std::path::PathBuf> {
    if let Some(custom) = std::env::var_os("VK_SWIFTSHADER_ICD") {
        let p = std::path::PathBuf::from(custom);
        if p.is_file() {
            return Some(p);
        }
    }

    let candidates = [
        r"C:\Windows\System32\Microsoft-Edge-WebView\vk_swiftshader_icd.json",
        r"C:\Program Files\LGHUB\vk_swiftshader_icd.json",
        r"C:\Program Files\Lenovo\LegionSpace\2.0.11.6\vk_swiftshader_icd.json",
        "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json",
        "/usr/share/vulkan/icd.d/vk_swiftshader_icd.json",
    ];
    for candidate in candidates {
        let p = std::path::Path::new(candidate);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Documented graphics adapter fallback chain from fastest hardware path to no-GPU software:
/// `Vulkan -> DX12 -> GL -> No-GPU presenter (winit + softbuffer)`.
pub const ADAPTER_FALLBACK_ORDER: [&str; 4] = [
    "Vulkan",
    "DX12",
    "GL",
    "No-GPU presenter (winit + softbuffer)",
];

#[derive(Clone, Debug, serde::Serialize)]
pub struct EnumeratedAdapterInfo {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub vendor: u32,
    pub device: u32,
}

/// Returns `true` when zero graphics adapters are being simulated via CLI flag or env var.
pub fn is_simulated_no_adapter(cli_flag: bool) -> bool {
    cli_flag
        || std::env::var_os("RBA_SIMULATE_NO_ADAPTER")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Enumerates all `wgpu` adapters visible on this system across enabled backends (`Vulkan`, `DX12`, `GL`).
#[allow(dead_code)]
pub fn enumerate_all_wgpu_adapters() -> Vec<EnumeratedAdapterInfo> {
    enumerate_all_wgpu_adapters_with_sim(false)
}

/// Enumerates all visible `wgpu` adapters unless zero-adapter fallback is being simulated.
pub fn enumerate_all_wgpu_adapters_with_sim(
    simulate_no_adapter: bool,
) -> Vec<EnumeratedAdapterInfo> {
    if is_simulated_no_adapter(simulate_no_adapter) {
        return Vec::new();
    }
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    instance
        .enumerate_adapters(wgpu::Backends::all())
        .into_iter()
        .map(|adapter| {
            let info = adapter.get_info();
            EnumeratedAdapterInfo {
                name: info.name,
                backend: format!("{:?}", info.backend),
                device_type: format!("{:?}", info.device_type),
                driver: info.driver,
                driver_info: info.driver_info,
                vendor: info.vendor,
                device: info.device,
            }
        })
        .collect()
}

#[inline]
pub const fn backend_fallback_rank(backend: wgpu::Backend) -> u8 {
    match backend {
        wgpu::Backend::Vulkan => 0,
        wgpu::Backend::Dx12 => 1,
        wgpu::Backend::Gl => 2,
        _ => 3,
    }
}

#[inline]
pub const fn device_type_rank(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Cpu => 3,
        wgpu::DeviceType::Other => 4,
    }
}

/// Selects the highest-priority `wgpu::Adapter` along `Vulkan -> DX12 -> GL`, preferring
/// hardware adapters (`DiscreteGpu -> IntegratedGpu -> VirtualGpu`) before software `Cpu` adapters.
pub fn select_preferred_adapter<'a>(
    adapters: impl IntoIterator<Item = &'a wgpu::Adapter>,
) -> Option<wgpu::Adapter> {
    adapters
        .into_iter()
        .min_by_key(|adapter| {
            let info = adapter.get_info();
            let is_cpu_or_other = matches!(
                info.device_type,
                wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
            ) as u8;
            (
                is_cpu_or_other,
                backend_fallback_rank(info.backend),
                device_type_rank(info.device_type),
            )
        })
        .cloned()
}

/// Discover a real CPU-based `wgpu::Adapter` (`DeviceType::Cpu`) constrained to `backends`.
///
/// Registers any system software Vulkan ICD (`SwiftShader` / `Lavapipe`) via `VK_ADD_DRIVER_FILES`
/// when Vulkan is enabled so `Vulkan -> DX12 -> GL` priority holds, and seamlessly falls back to
/// Windows' built-in DX12 WARP (`Microsoft Basic Render Driver`) on any Windows 10+ machine.
pub fn discover_software_cpu_adapter_with_backends(
    backends: wgpu::Backends,
) -> Result<wgpu::Adapter, String> {
    if backends.is_empty() {
        return Err(format!(
            "No graphics backend enabled for CPU adapter lookup (fallback order: {})",
            ADAPTER_FALLBACK_ORDER.join(" -> ")
        ));
    }

    let _guard = VK_ICD_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if backends.contains(wgpu::Backends::VULKAN)
        && std::env::var_os("VK_ADD_DRIVER_FILES").is_none()
        && std::env::var_os("VK_DRIVER_FILES").is_none()
        && let Some(icd_path) = find_software_vulkan_icd()
    {
        // SAFETY: Serialized under `VK_ICD_ENV_LOCK` for headless adapter probing.
        unsafe {
            std::env::set_var("VK_ADD_DRIVER_FILES", &icd_path);
        }
    }

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });
    let cpu_adapters: Vec<wgpu::Adapter> = instance
        .enumerate_adapters(backends)
        .into_iter()
        .filter(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
        .collect();
    if let Some(adapter) = select_preferred_adapter(cpu_adapters.iter()) {
        return Ok(adapter);
    }

    Err(format!(
        "No DeviceType::Cpu adapter found for backends {backends:?} (fallback order: {}). Use No-GPU presenter fallback.",
        ADAPTER_FALLBACK_ORDER.join(" -> ")
    ))
}

pub fn discover_software_cpu_adapter() -> Result<wgpu::Adapter, String> {
    discover_software_cpu_adapter_with_backends(wgpu::Backends::all())
}

/// Offscreen `wgpu` rendering context shared by `--bench-filters` and
/// `--bench-realtime --software-adapter`.
pub struct SwiftShaderNoneProfile {
    pub a_total_sync_ms: f64,
    pub a1_stage_ms: f64,
    pub a2_flush_curr_ms: f64,
    pub a3_flush_prev_ms: f64,
    pub a4_encode_ms: f64,
    pub a5_submit_poll_ms: f64,
    pub b_bench_filters_none_ms: f64,
    pub c_loadop_load_ms: f64,
    pub d_empty_submit_poll_ms: f64,
    pub e_plain_shader_720p_ms: f64,
    pub f_write_tex_720p_sync_ms: f64,
    pub g_write_tex_240p_sync_ms: f64,
    pub h_write_240_plus_plain_720_ms: f64,
    pub i_copy_tex_240_sync_ms: f64,
    pub j_copy_tex_720_sync_ms: f64,
    pub k_write_buf_copy_720p_ms: f64,
    pub l_plain_clear_720p_ms: f64,
}

const PLAIN_BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;

struct PlainVertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_plain(@builtin(vertex_index) vid: u32) -> PlainVertexOut {
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let uv = uvs[vid];
    var out: PlainVertexOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_plain_sample(in: PlainVertexOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src_tex, src_samp, in.uv, 0.0);
}
"#;

struct PlainTexSlot {
    width: u32,
    height: u32,
    tex: wgpu::Texture,
    bg_nearest: wgpu::BindGroup,
    bg_linear: wgpu::BindGroup,
}

impl PlainTexSlot {
    fn new(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        nearest_samp: &wgpu::Sampler,
        linear_samp: &wgpu::Sampler,
        width: u32,
        height: u32,
    ) -> Self {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gba_plain_src_tex"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mk_bg = |samp: &wgpu::Sampler, label: &'static str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(samp),
                    },
                ],
            })
        };
        Self {
            width,
            height,
            tex,
            bg_nearest: mk_bg(nearest_samp, "gba_plain_bg_near"),
            bg_linear: mk_bg(linear_samp, "gba_plain_bg_lin"),
        }
    }
}

pub struct PlainTexturePresenter {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    nearest_samp: wgpu::Sampler,
    linear_samp: wgpu::Sampler,
    slots: Vec<PlainTexSlot>,
}

impl PlainTexturePresenter {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gba_plain_blit_wgsl"),
            source: wgpu::ShaderSource::Wgsl(PLAIN_BLIT_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gba_plain_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gba_plain_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fs_plain_sample"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_plain"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_plain_sample"),
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
        let mk_samp = |mode: wgpu::FilterMode, label: &'static str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: mode,
                min_filter: mode,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            })
        };
        let nearest_samp = mk_samp(wgpu::FilterMode::Nearest, "gba_plain_nearest");
        let linear_samp = mk_samp(wgpu::FilterMode::Linear, "gba_plain_linear");
        let mut slots = Vec::with_capacity(3);
        for mult in [1u32, 2u32, 3u32] {
            slots.push(PlainTexSlot::new(
                device,
                &bgl,
                &nearest_samp,
                &linear_samp,
                LCD_WIDTH as u32 * mult,
                LCD_HEIGHT as u32 * mult,
            ));
        }
        Self {
            pipeline,
            bgl,
            nearest_samp,
            linear_samp,
            slots,
        }
    }

    fn slot_index(&mut self, device: &wgpu::Device, width: u32, height: u32) -> usize {
        if let Some(pos) = self
            .slots
            .iter()
            .position(|s| s.width == width && s.height == height)
        {
            return pos;
        }
        self.slots.push(PlainTexSlot::new(
            device,
            &self.bgl,
            &self.nearest_samp,
            &self.linear_samp,
            width,
            height,
        ));
        self.slots.len() - 1
    }

    #[allow(clippy::too_many_arguments)]
    fn present_rgba_sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        out_tex: &wgpu::Texture,
        output_view: &wgpu::TextureView,
        dst_width: u32,
        dst_height: u32,
        rgba: &[u8],
        src_width: u32,
        src_height: u32,
        linear: bool,
    ) {
        let extent = wgpu::Extent3d {
            width: src_width,
            height: src_height,
            depth_or_array_layers: 1,
        };
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(src_width * 4),
            rows_per_image: Some(src_height),
        };
        if src_width == dst_width && src_height == dst_height {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: out_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                layout,
                extent,
            );
            let enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gba_plain_direct_sync"),
            });
            queue.submit(Some(enc.finish()));
            device.poll(wgpu::Maintain::Wait);
            return;
        }

        let slot_idx = self.slot_index(device, src_width, src_height);
        let slot = &self.slots[slot_idx];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &slot.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            layout,
            extent,
        );
        let bg = if linear {
            &slot.bg_linear
        } else {
            &slot.bg_nearest
        };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gba_plain_blit_encoder"),
        });
        {
            let mut rpass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gba_plain_blit_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, bg, &[]);
            rpass.draw(0..6, 0..1);
        }
        queue.submit(Some(enc.finish()));
        device.poll(wgpu::Maintain::Wait);
    }

    #[allow(clippy::too_many_arguments)]
    fn measure_cpu_filter_costs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        out_tex: &wgpu::Texture,
        output_view: &wgpu::TextureView,
        dst_width: u32,
        dst_height: u32,
        postproc: &mut VideoPostProcessor,
        color_correction: bool,
        lcd_ghosting: bool,
        grid_intensity: f32,
    ) -> Vec<FilterCost> {
        let pattern = test_filter_pattern_fb();
        postproc.prewarm(
            color_correction,
            lcd_ghosting,
            DisplayFilter::Sharp1x,
            false,
            grid_intensity,
        );
        let mut costs = Vec::with_capacity(DisplayFilter::ALL.len());
        for &filter in &DisplayFilter::ALL {
            let linear = filter.texture_options() == egui::TextureOptions::LINEAR;
            for _ in 0..2 {
                let img = postproc.process_display_filter(
                    &pattern,
                    color_correction,
                    lcd_ghosting,
                    filter,
                    false,
                    grid_intensity,
                );
                self.present_rgba_sync(
                    device,
                    queue,
                    out_tex,
                    output_view,
                    dst_width,
                    dst_height,
                    img.as_raw(),
                    img.size[0] as u32,
                    img.size[1] as u32,
                    linear,
                );
            }
            let mut samples = [0.0f64; 3];
            for slot in &mut samples {
                let t0 = Instant::now();
                for _ in 0..4 {
                    let img = postproc.process_display_filter(
                        &pattern,
                        color_correction,
                        lcd_ghosting,
                        filter,
                        false,
                        grid_intensity,
                    );
                    self.present_rgba_sync(
                        device,
                        queue,
                        out_tex,
                        output_view,
                        dst_width,
                        dst_height,
                        img.as_raw(),
                        img.size[0] as u32,
                        img.size[1] as u32,
                        linear,
                    );
                }
                *slot = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;
            }
            samples.sort_by(f64::total_cmp);
            costs.push(FilterCost {
                filter,
                cost_ms: samples[1],
            });
        }
        postproc.reset();
        sort_filter_costs(&mut costs);
        costs
    }
}

/// Measures CPU filter + plain-texture upload costs for a `DeviceType::Cpu` wgpu adapter.
pub fn measure_cpu_adapter_filter_costs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    postproc: &mut VideoPostProcessor,
    color_correction: bool,
    lcd_ghosting: bool,
    grid_intensity: f32,
) -> Vec<FilterCost> {
    let width = LCD_WIDTH as u32 * 3;
    let height = LCD_HEIGHT as u32 * 3;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let out_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gba_cpu_probe_output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let output_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut presenter = PlainTexturePresenter::new(device, format);
    presenter.measure_cpu_filter_costs(
        device,
        queue,
        &out_tex,
        &output_view,
        width,
        height,
        postproc,
        color_correction,
        lcd_ghosting,
        grid_intensity,
    )
}

pub struct BenchWgpuContext {
    pub adapter_name: String,
    pub adapter_backend: String,
    pub adapter_device_type: String,
    pub device_type: wgpu::DeviceType,
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub pipeline: WgpuShaderPipeline,
    plain_presenter: PlainTexturePresenter,
    pub out_tex: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub last_poll_wait_ms: f64,
    pub last_submit_cpu_ms: f64,
}

#[derive(Clone, Debug)]
pub struct StartupBackendDecision {
    pub use_wgpu: bool,
    pub adapter_name: String,
    pub adapter_backend: String,
    pub adapter_device_type: String,
    pub is_cpu_adapter: bool,
    pub reason: String,
    pub fallback_reason: Option<String>,
    pub cpu_adapter_ms: Option<f64>,
    pub no_gpu_ms: Option<f64>,
    pub available_adapters: Vec<EnumeratedAdapterInfo>,
}

/// Auto-selects the desktop backend along `Vulkan -> DX12 -> GL -> No-GPU presenter`, preferring
/// the pure-CPU `No-GPU` presenter (`winit + softbuffer`) over any `DeviceType::Cpu` wgpu adapter
/// (e.g. DX12 WARP or SwiftShader) unless startup micro-timing shows the `DeviceType::Cpu` adapter
/// is faster.
pub fn choose_desktop_backend(
    simulate_no_adapter: bool,
    force_no_gpu: bool,
    force_software_adapter: bool,
    filter: crate::config::DisplayFilter,
    color_correction: bool,
    lcd_ghosting: bool,
    grid_intensity: f32,
) -> StartupBackendDecision {
    let available_adapters = enumerate_all_wgpu_adapters_with_sim(simulate_no_adapter);

    if force_no_gpu {
        return StartupBackendDecision {
            use_wgpu: false,
            adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
            adapter_backend: "NoGpu".to_string(),
            adapter_device_type: "CpuSoftware".to_string(),
            is_cpu_adapter: true,
            reason: format!(
                "Bypassed wgpu via --no-gpu; running CPU VideoPostProcessor + XRGB8/softbuffer along fallback order [{}]",
                ADAPTER_FALLBACK_ORDER.join(" -> ")
            ),
            fallback_reason: Some("Forced via --no-gpu flag".to_string()),
            cpu_adapter_ms: None,
            no_gpu_ms: None,
            available_adapters,
        };
    }

    if is_simulated_no_adapter(simulate_no_adapter) {
        let msg = "Simulated zero wgpu adapters via --simulate-no-adapter".to_string();
        return StartupBackendDecision {
            use_wgpu: false,
            adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
            adapter_backend: "NoGpu".to_string(),
            adapter_device_type: "CpuFallback".to_string(),
            is_cpu_adapter: true,
            reason: format!(
                "{msg}; fell back automatically to No-GPU presenter (winit + softbuffer) along [{}]",
                ADAPTER_FALLBACK_ORDER.join(" -> ")
            ),
            fallback_reason: Some(msg),
            cpu_adapter_ms: None,
            no_gpu_ms: None,
            available_adapters,
        };
    }

    if force_software_adapter {
        return match BenchWgpuContext::init(true, LCD_WIDTH as u32 * 3, LCD_HEIGHT as u32 * 3) {
            Ok(ctx) => StartupBackendDecision {
                use_wgpu: true,
                adapter_name: ctx.adapter_name.clone(),
                adapter_backend: ctx.adapter_backend.clone(),
                adapter_device_type: ctx.adapter_device_type.clone(),
                is_cpu_adapter: ctx.is_cpu_adapter(),
                reason: format!(
                    "--software-adapter selected `{}` ({} / {}) along fallback order [{}]",
                    ctx.adapter_name,
                    ctx.adapter_backend,
                    ctx.adapter_device_type,
                    ADAPTER_FALLBACK_ORDER.join(" -> ")
                ),
                fallback_reason: None,
                cpu_adapter_ms: None,
                no_gpu_ms: None,
                available_adapters,
            },
            Err(err) => StartupBackendDecision {
                use_wgpu: false,
                adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
                adapter_backend: "NoGpu".to_string(),
                adapter_device_type: "CpuFallback".to_string(),
                is_cpu_adapter: true,
                reason: format!(
                    "--software-adapter unavailable ({err}); fell back to No-GPU presenter along [{}]",
                    ADAPTER_FALLBACK_ORDER.join(" -> ")
                ),
                fallback_reason: Some(err),
                cpu_adapter_ms: None,
                no_gpu_ms: None,
                available_adapters,
            },
        };
    }

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    let Some(preferred) = select_preferred_adapter(adapters.iter()) else {
        let msg = format!(
            "No compatible wgpu adapter found along fallback order [{}]",
            ADAPTER_FALLBACK_ORDER.join(" -> ")
        );
        return StartupBackendDecision {
            use_wgpu: false,
            adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
            adapter_backend: "NoGpu".to_string(),
            adapter_device_type: "CpuFallback".to_string(),
            is_cpu_adapter: true,
            reason: format!("{msg}; fell back automatically to No-GPU presenter (winit + softbuffer)"),
            fallback_reason: Some(msg),
            cpu_adapter_ms: None,
            no_gpu_ms: None,
            available_adapters,
        };
    };

    let info = preferred.get_info();
    if !matches!(
        info.device_type,
        wgpu::DeviceType::Cpu | wgpu::DeviceType::Other
    ) {
        let backend_str = format!("{:?}", info.backend);
        let dev_str = format!("{:?}", info.device_type);
        return StartupBackendDecision {
            use_wgpu: true,
            adapter_name: info.name.clone(),
            adapter_backend: backend_str.clone(),
            adapter_device_type: dev_str.clone(),
            is_cpu_adapter: false,
            reason: format!(
                "Auto-selected hardware adapter `{}` ({backend_str} / {dev_str}) along fallback order [{}]",
                info.name,
                ADAPTER_FALLBACK_ORDER.join(" -> ")
            ),
            fallback_reason: None,
            cpu_adapter_ms: None,
            no_gpu_ms: None,
            available_adapters,
        };
    }

    // Only a DeviceType::Cpu adapter is present (e.g. DX12 WARP or SwiftShader): time its
    // CPU-filter + plain-texture path vs the pure-CPU No-GPU XRGB8 presenter and prefer
    // No-GPU unless the wgpu CPU adapter proves faster at startup.
    let w = LCD_WIDTH as u32 * 3;
    let h = LCD_HEIGHT as u32 * 3;
    let pattern = test_filter_pattern_fb();
    let linear = filter.texture_options() == egui::TextureOptions::LINEAR;

    let mut no_gpu_post = VideoPostProcessor::new();
    let mut no_gpu_blit = crate::no_gpu::NoGpuCpuPresenter::new(w as usize, h as usize);
    no_gpu_post.prewarm(color_correction, lcd_ghosting, filter, false, grid_intensity);
    for _ in 0..8 {
        let img = no_gpu_post.process_display_filter(
            &pattern,
            color_correction,
            lcd_ghosting,
            filter,
            false,
            grid_intensity,
        );
        let xrgb = no_gpu_blit.present_rgba_to_xrgb(
            &img.pixels,
            img.size[0],
            img.size[1],
            w as usize,
            h as usize,
            linear,
        );
        std::hint::black_box(xrgb.as_ptr());
    }
    let t_ng = Instant::now();
    for _ in 0..20 {
        let img = no_gpu_post.process_display_filter(
            &pattern,
            color_correction,
            lcd_ghosting,
            filter,
            false,
            grid_intensity,
        );
        let xrgb = no_gpu_blit.present_rgba_to_xrgb(
            &img.pixels,
            img.size[0],
            img.size[1],
            w as usize,
            h as usize,
            linear,
        );
        std::hint::black_box(xrgb.as_ptr());
    }
    let no_gpu_ms = t_ng.elapsed().as_secs_f64() * 1000.0 / 20.0;

    let Ok(mut cpu_ctx) = BenchWgpuContext::init(true, w, h) else {
        let msg = format!(
            "Failed to initialize DeviceType::Cpu adapter `{}`",
            info.name
        );
        return StartupBackendDecision {
            use_wgpu: false,
            adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
            adapter_backend: "NoGpu".to_string(),
            adapter_device_type: "CpuFallback".to_string(),
            is_cpu_adapter: true,
            reason: format!("{msg}; fell back to No-GPU presenter (CPU blitter {no_gpu_ms:.3} ms)"),
            fallback_reason: Some(msg),
            cpu_adapter_ms: None,
            no_gpu_ms: Some(no_gpu_ms),
            available_adapters,
        };
    };

    let mut cpu_post = VideoPostProcessor::new();
    cpu_post.prewarm(color_correction, lcd_ghosting, filter, false, grid_intensity);
    for _ in 0..8 {
        let img = cpu_post.process_display_filter(
            &pattern,
            color_correction,
            lcd_ghosting,
            filter,
            false,
            grid_intensity,
        );
        cpu_ctx.present_plain_rgba_sync(
            img.as_raw(),
            img.size[0] as u32,
            img.size[1] as u32,
            linear,
        );
    }
    let t_cpu = Instant::now();
    for _ in 0..20 {
        let img = cpu_post.process_display_filter(
            &pattern,
            color_correction,
            lcd_ghosting,
            filter,
            false,
            grid_intensity,
        );
        cpu_ctx.present_plain_rgba_sync(
            img.as_raw(),
            img.size[0] as u32,
            img.size[1] as u32,
            linear,
        );
    }
    let cpu_adapter_ms = t_cpu.elapsed().as_secs_f64() * 1000.0 / 20.0;

    if cpu_adapter_ms < no_gpu_ms {
        StartupBackendDecision {
            use_wgpu: true,
            adapter_name: cpu_ctx.adapter_name.clone(),
            adapter_backend: cpu_ctx.adapter_backend.clone(),
            adapter_device_type: cpu_ctx.adapter_device_type.clone(),
            is_cpu_adapter: true,
            reason: format!(
                "Startup timing chose DeviceType::Cpu adapter `{}` ({} / {}): {:.3} ms < No-GPU {:.3} ms on `{}`",
                cpu_ctx.adapter_name,
                cpu_ctx.adapter_backend,
                cpu_ctx.adapter_device_type,
                cpu_adapter_ms,
                no_gpu_ms,
                filter.label()
            ),
            fallback_reason: None,
            cpu_adapter_ms: Some(cpu_adapter_ms),
            no_gpu_ms: Some(no_gpu_ms),
            available_adapters,
        }
    } else {
        StartupBackendDecision {
            use_wgpu: false,
            adapter_name: "winit + softbuffer (No-GPU presenter)".to_string(),
            adapter_backend: "NoGpu".to_string(),
            adapter_device_type: "CpuFallback".to_string(),
            is_cpu_adapter: true,
            reason: format!(
                "Preferred No-GPU presenter ({no_gpu_ms:.3} ms) over DeviceType::Cpu adapter `{}` ({} / {}, {cpu_adapter_ms:.3} ms) on `{}`",
                cpu_ctx.adapter_name,
                cpu_ctx.adapter_backend,
                cpu_ctx.adapter_device_type,
                filter.label()
            ),
            fallback_reason: Some(format!(
                "DeviceType::Cpu adapter `{}` slower than No-GPU presenter ({cpu_adapter_ms:.3} ms vs {no_gpu_ms:.3} ms)",
                cpu_ctx.adapter_name
            )),
            cpu_adapter_ms: Some(cpu_adapter_ms),
            no_gpu_ms: Some(no_gpu_ms),
            available_adapters,
        }
    }
}

impl BenchWgpuContext {
    pub fn init(software_cpu: bool, width: u32, height: u32) -> Result<Self, String> {
        Self::init_with_options(wgpu::Backends::all(), software_cpu, false, width, height)
    }

    pub fn init_with_backends(
        backends: wgpu::Backends,
        software_cpu: bool,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        Self::init_with_options(backends, software_cpu, false, width, height)
    }

    /// Initializes a wgpu context across `backends` using the fallback order
    /// `Vulkan -> DX12 (when compiled) -> GL`, returning a clear diagnostic `Err`
    /// if no adapter can be initialized (never panicking).
    pub fn init_with_options(
        backends: wgpu::Backends,
        software_cpu: bool,
        simulate_no_adapter: bool,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if is_simulated_no_adapter(simulate_no_adapter) {
            return Err(format!(
                "Simulated zero wgpu adapters via --simulate-no-adapter (fallback order: {}); using No-GPU presenter.",
                ADAPTER_FALLBACK_ORDER.join(" -> ")
            ));
        }
        let adapter = if software_cpu {
            if backends == wgpu::Backends::all() {
                discover_software_cpu_adapter()?
            } else {
                discover_software_cpu_adapter_with_backends(backends)?
            }
        } else if backends.is_empty() {
            return Err(format!(
                "No graphics backends enabled to initialize a wgpu adapter (fallback order: {})",
                ADAPTER_FALLBACK_ORDER.join(" -> ")
            ));
        } else {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                ..Default::default()
            });
            let adapters = instance.enumerate_adapters(backends);
            select_preferred_adapter(adapters.iter()).ok_or_else(|| {
                format!(
                    "No wgpu adapter available for backends {backends:?} along fallback order ({}); use No-GPU presenter fallback.",
                    ADAPTER_FALLBACK_ORDER.join(" -> ")
                )
            })?
        };

        let info = adapter.get_info();
        let adapter_name = info.name;
        let adapter_backend = format!("{:?}", info.backend);
        let device_type = info.device_type;
        let adapter_device_type = format!("{device_type:?}");

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .map_err(|e| format!("request_device failed for `{adapter_name}`: {e}"))?;
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let tex_format = wgpu::TextureFormat::Rgba8Unorm;
        let out_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gba_bench_output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: tex_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let output_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let pipeline = WgpuShaderPipeline::new(Arc::clone(&device), Arc::clone(&queue), tex_format)?;
        let plain_presenter = PlainTexturePresenter::new(&device, tex_format);

        Ok(Self {
            adapter_name,
            adapter_backend,
            adapter_device_type,
            device_type,
            device,
            queue,
            pipeline,
            plain_presenter,
            out_tex,
            output_view,
            width,
            height,
            last_poll_wait_ms: 0.0,
            last_submit_cpu_ms: 0.0,
        })
    }

    #[inline]
    pub fn is_cpu_adapter(&self) -> bool {
        self.device_type == wgpu::DeviceType::Cpu
    }

    /// Uploads a CPU-filtered RGBA8 image directly and presents it via a plain pass-through texture
    /// (zero fragment shader ALU for 1:1 dimensions, or single plain textured quad when scaling).
    pub fn present_plain_rgba_sync(
        &mut self,
        rgba: &[u8],
        src_width: u32,
        src_height: u32,
        linear: bool,
    ) {
        self.plain_presenter.present_rgba_sync(
            &self.device,
            &self.queue,
            &self.out_tex,
            &self.output_view,
            self.width,
            self.height,
            rgba,
            src_width,
            src_height,
            linear,
        );
    }

    /// Briefly measures every display filter's real present cost on this adapter during prewarm
    /// and returns the sorted list ascending.
    pub fn measure_filter_costs(
        &mut self,
        postproc: &mut VideoPostProcessor,
        color_correction: bool,
        lcd_ghosting: bool,
        grid_intensity: f32,
    ) -> Vec<FilterCost> {
        if self.is_cpu_adapter() {
            return self.plain_presenter.measure_cpu_filter_costs(
                &self.device,
                &self.queue,
                &self.out_tex,
                &self.output_view,
                self.width,
                self.height,
                postproc,
                color_correction,
                lcd_ghosting,
                grid_intensity,
            );
        }
        let pattern = test_filter_pattern_fb();
        let color_profile_id = if color_correction { 1 } else { 0 };
        let mut costs = Vec::with_capacity(DisplayFilter::ALL.len());
        for &filter in &DisplayFilter::ALL {
            let preset = filter.default_shader_preset();
            for _ in 0..2 {
                self.present_frame_sync(&pattern, &pattern, true, preset, color_profile_id);
            }
            let mut samples = [0.0f64; 3];
            for slot in &mut samples {
                let t0 = Instant::now();
                for _ in 0..4 {
                    self.present_frame_sync(&pattern, &pattern, true, preset, color_profile_id);
                }
                *slot = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;
            }
            samples.sort_by(f64::total_cmp);
            costs.push(FilterCost {
                filter,
                cost_ms: samples[1],
            });
        }
        sort_filter_costs(&mut costs);
        costs
    }

    pub fn profile_swiftshader_none(&mut self, frames: usize) -> SwiftShaderNoneProfile {
        use std::time::Instant;
        let frames = frames.max(20);
        let curr_fb: Vec<u16> = (0..(LCD_WIDTH * LCD_HEIGHT))
            .map(|i| ((i as u16).wrapping_mul(73)) & 0x7FFF)
            .collect();
        let curr_arr: &[u16; LCD_WIDTH * LCD_HEIGHT] = curr_fb.as_slice().try_into().unwrap();
        let rgba_240 = vec![0xA5u8; LCD_WIDTH * LCD_HEIGHT * 4];
        let rgba_720 = vec![0xA5u8; (self.width * self.height * 4) as usize];

        let src_240 = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prof_src_240"),
            size: wgpu::Extent3d {
                width: LCD_WIDTH as u32,
                height: LCD_HEIGHT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let dst_240 = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prof_dst_240"),
            size: wgpu::Extent3d {
                width: LCD_WIDTH as u32,
                height: LCD_HEIGHT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let src_720 = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prof_src_720"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let ext_240 = wgpu::Extent3d {
            width: LCD_WIDTH as u32,
            height: LCD_HEIGHT as u32,
            depth_or_array_layers: 1,
        };
        let ext_720 = wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        };

        // Warmup
        for _ in 0..10 {
            self.present_frame_sync(curr_arr, curr_arr, true, ShaderPreset::Nearest, 0);
        }

        // A: breakdown of full baseline present_frame_sync
        let (mut s_stage, mut s_curr, mut s_prev, mut s_enc, mut s_sub) =
            (0.0, 0.0, 0.0, 0.0, 0.0);
        for _ in 0..frames {
            let t0 = Instant::now();
            self.pipeline.stage_frames(curr_arr, curr_arr, true);
            s_stage += t0.elapsed().as_secs_f64() * 1000.0;

            let t1 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pipeline.curr_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &self.pipeline.curr_staging,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                    rows_per_image: Some(LCD_HEIGHT as u32),
                },
                ext_240,
            );
            s_curr += t1.elapsed().as_secs_f64() * 1000.0;

            let t2 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pipeline.prev_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &self.pipeline.prev_staging,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                    rows_per_image: Some(LCD_HEIGHT as u32),
                },
                ext_240,
            );
            s_prev += t2.elapsed().as_secs_f64() * 1000.0;

            let t3 = Instant::now();
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.pipeline.render_to_texture_view(
                &mut enc,
                &self.output_view,
                ShaderPreset::Nearest,
                0,
                self.width,
                self.height,
            );
            s_enc += t3.elapsed().as_secs_f64() * 1000.0;

            let t4 = Instant::now();
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            s_sub += t4.elapsed().as_secs_f64() * 1000.0;
        }
        let n = frames as f64;

        let bench_render = |load: wgpu::LoadOp<wgpu::Color>, pipe: &wgpu::RenderPipeline| -> f64 {
            let mut sum = 0.0;
            for _ in 0..frames {
                let t0 = Instant::now();
                let uniforms = Uniforms {
                    src_size: [LCD_WIDTH as f32, LCD_HEIGHT as f32],
                    dst_size: [self.width as f32, self.height as f32],
                    ndc_rect: [-1.0, 1.0, 1.0, -1.0],
                    color_profile: 0,
                    frame_blend: 0.0,
                    lcd_ghosting: 0.0,
                    _pad: 0.0,
                };
                self.queue
                    .write_buffer(&self.pipeline.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
                let mut enc = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                {
                    let mut rpass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.output_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    rpass.set_pipeline(pipe);
                    rpass.set_bind_group(0, self.pipeline.bind_group.as_ref(), &[]);
                    rpass.draw(0..6, 0..1);
                }
                self.queue.submit(Some(enc.finish()));
                self.device.poll(wgpu::Maintain::Wait);
                sum += t0.elapsed().as_secs_f64() * 1000.0;
            }
            sum / n
        };

        let b_bench_filters_none_ms =
            bench_render(wgpu::LoadOp::Clear(wgpu::Color::BLACK), &self.pipeline.pipelines[0]);
        let c_loadop_load_ms =
            bench_render(wgpu::LoadOp::Load, &self.pipeline.pipelines[0]);
        let e_plain_shader_720p_ms =
            bench_render(wgpu::LoadOp::Load, &self.pipeline.plain_pipeline);
        let l_plain_clear_720p_ms =
            bench_render(wgpu::LoadOp::Clear(wgpu::Color::BLACK), &self.pipeline.plain_pipeline);

        let mut d_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            let enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            d_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let mut f_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.out_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba_720,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.width * 4),
                    rows_per_image: Some(self.height),
                },
                ext_720,
            );
            let enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            f_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let mut g_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &dst_240,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba_240,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                    rows_per_image: Some(LCD_HEIGHT as u32),
                },
                ext_240,
            );
            let enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            g_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let mut h_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pipeline.curr_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba_240,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                    rows_per_image: Some(LCD_HEIGHT as u32),
                },
                ext_240,
            );
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut rpass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.output_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rpass.set_pipeline(&self.pipeline.plain_pipeline);
                rpass.set_bind_group(0, self.pipeline.bind_group.as_ref(), &[]);
                rpass.draw(0..6, 0..1);
            }
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            h_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let mut i_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &src_240,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba_240,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LCD_WIDTH as u32 * 4),
                    rows_per_image: Some(LCD_HEIGHT as u32),
                },
                ext_240,
            );
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_texture_to_texture(
                src_240.as_image_copy(),
                dst_240.as_image_copy(),
                ext_240,
            );
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            i_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let mut j_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &src_720,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba_720,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.width * 4),
                    rows_per_image: Some(self.height),
                },
                ext_720,
            );
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_texture_to_texture(
                src_720.as_image_copy(),
                self.out_tex.as_image_copy(),
                ext_720,
            );
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            j_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let aligned_bpr = ((self.width * 4) + 255) & !255;
        let staging_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prof_staging_720"),
            size: (aligned_bpr * self.height) as u64,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let staging_bytes = vec![0xA5u8; (aligned_bpr * self.height) as usize];
        let mut k_sum = 0.0;
        for _ in 0..frames {
            let t0 = Instant::now();
            self.queue.write_buffer(&staging_buf, 0, &staging_bytes);
            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            enc.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging_buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(aligned_bpr),
                        rows_per_image: Some(self.height),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.out_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                ext_720,
            );
            self.queue.submit(Some(enc.finish()));
            self.device.poll(wgpu::Maintain::Wait);
            k_sum += t0.elapsed().as_secs_f64() * 1000.0;
        }

        let a1 = s_stage / n;
        let a2 = s_curr / n;
        let a3 = s_prev / n;
        let a4 = s_enc / n;
        let a5 = s_sub / n;
        SwiftShaderNoneProfile {
            a_total_sync_ms: a1 + a2 + a3 + a4 + a5,
            a1_stage_ms: a1,
            a2_flush_curr_ms: a2,
            a3_flush_prev_ms: a3,
            a4_encode_ms: a4,
            a5_submit_poll_ms: a5,
            b_bench_filters_none_ms,
            c_loadop_load_ms,
            d_empty_submit_poll_ms: d_sum / n,
            e_plain_shader_720p_ms,
            f_write_tex_720p_sync_ms: f_sum / n,
            g_write_tex_240p_sync_ms: g_sum / n,
            h_write_240_plus_plain_720_ms: h_sum / n,
            i_copy_tex_240_sync_ms: i_sum / n,
            j_copy_tex_720_sync_ms: j_sum / n,
            k_write_buf_copy_720p_ms: k_sum / n,
            l_plain_clear_720p_ms,
        }
    }

    /// Stage, upload, render, and submit asynchronously (non-blocking `Maintain::Poll`),
    /// returning pure CPU-side encode+submit milliseconds so `--slowdown N` never multiplies
    /// GPU fence/poll wait latency.
    pub fn present_frame_async(
        &mut self,
        curr_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        has_prev: bool,
        preset: ShaderPreset,
        color_profile_id: i32,
    ) -> f64 {
        let t0 = Instant::now();
        self.pipeline.stage_frames(curr_fb, prev_fb, has_prev);
        self.pipeline.flush_uploads();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gba_bench_present_async_encoder"),
            });
        self.pipeline.render_to_texture_view(
            &mut encoder,
            &self.output_view,
            preset,
            color_profile_id,
            self.width,
            self.height,
        );
        self.queue.submit(Some(encoder.finish()));
        self.device.poll(wgpu::Maintain::Poll);
        let cpu_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.last_submit_cpu_ms = cpu_ms;
        self.last_poll_wait_ms = 0.0;
        cpu_ms
    }

    /// Drains any pending submitted GPU work once between sweep windows (never called per-frame).
    pub fn drain_gpu(&self) {
        self.device.poll(wgpu::Maintain::Wait);
    }

    /// Measures isolated per-frame GPU wait latency (`device.poll(Maintain::Wait)`) as a fixed
    /// hardware/driver round-trip cost, completely separate from CPU emulation and `--slowdown`.
    pub fn measure_gpu_wait_latency_ms(
        &mut self,
        preset: ShaderPreset,
        color_profile_id: i32,
        frames: usize,
    ) -> (f64, f64, f64) {
        let pattern = test_filter_pattern_fb();
        for _ in 0..16 {
            self.present_frame_sync(&pattern, &pattern, true, preset, color_profile_id);
        }
        let n = frames.max(20);
        let mut waits = Vec::with_capacity(n);
        for _ in 0..n {
            self.present_frame_sync(&pattern, &pattern, true, preset, color_profile_id);
            waits.push(self.last_poll_wait_ms);
        }
        waits.sort_by(f64::total_cmp);
        let avg = waits.iter().sum::<f64>() / waits.len().max(1) as f64;
        let p95 = waits[((waits.len().saturating_sub(1)) * 95) / 100];
        let p995 = waits[((waits.len().saturating_sub(1)) * 995) / 1000];
        (avg, p95, p995)
    }

    /// Stage, upload, render, and wait for completion synchronously so present cost is measurable.
    pub fn present_frame_sync(
        &mut self,
        curr_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb: &[u16; LCD_WIDTH * LCD_HEIGHT],
        has_prev: bool,
        preset: ShaderPreset,
        color_profile_id: i32,
    ) {
        self.pipeline.stage_frames(curr_fb, prev_fb, has_prev);
        self.pipeline.flush_uploads();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gba_bench_present_encoder"),
            });
        self.pipeline.render_to_texture_view(
            &mut encoder,
            &self.output_view,
            preset,
            color_profile_id,
            self.width,
            self.height,
        );
        self.queue.submit(Some(encoder.finish()));
        let poll_start = Instant::now();
        self.device.poll(wgpu::Maintain::Wait);
        self.last_poll_wait_ms = poll_start.elapsed().as_secs_f64() * 1000.0;
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
        let entry_points: Vec<&str> = module
            .entry_points
            .iter()
            .map(|ep| ep.name.as_str())
            .collect();
        assert!(entry_points.contains(&"vs_main"));
        assert!(entry_points.contains(&"fs_nearest"));
        assert!(entry_points.contains(&"fs_sharp_bilinear"));
        assert!(entry_points.contains(&"fs_lcd_grid"));
        assert!(entry_points.contains(&"fs_crt_scanlines"));
        assert!(entry_points.contains(&"fs_scale_edge"));
    }

    #[test]
    fn test_adapter_discovery() {
        let normal = BenchWgpuContext::init(false, 720, 480);
        let sw_cpu = discover_software_cpu_adapter();
        println!("\n=== WGPU ADAPTERS ===");
        println!(
            "Normal adapter:   {:?}",
            normal
                .as_ref()
                .map(|c| (&c.adapter_name, &c.adapter_backend, &c.adapter_device_type))
        );
        println!(
            "Software CPU:     {:?}",
            sw_cpu.as_ref().map(|a| {
                let info = a.get_info();
                (info.name, format!("{:?}", info.backend), format!("{:?}", info.device_type))
            })
        );
        if find_software_vulkan_icd().is_some() {
            let sw_adapter = sw_cpu.expect("CPU Vulkan software ICD exists so Cpu adapter should load");
            assert_eq!(sw_adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        }
    }

    #[test]
    fn test_adapter_fallback_order_and_missing_adapter_returns_clear_error() {
        assert_eq!(ADAPTER_FALLBACK_ORDER.len(), 4);
        assert!(backend_fallback_rank(wgpu::Backend::Vulkan) < backend_fallback_rank(wgpu::Backend::Dx12));
        assert!(backend_fallback_rank(wgpu::Backend::Dx12) < backend_fallback_rank(wgpu::Backend::Gl));
        assert!(device_type_rank(wgpu::DeviceType::DiscreteGpu) < device_type_rank(wgpu::DeviceType::IntegratedGpu));
        assert!(device_type_rank(wgpu::DeviceType::IntegratedGpu) < device_type_rank(wgpu::DeviceType::Cpu));

        let empty_hw = BenchWgpuContext::init_with_backends(wgpu::Backends::empty(), false, 720, 480);
        let empty_hw_err = match empty_hw {
            Ok(_) => panic!("empty backends must not initialize an adapter"),
            Err(e) => e,
        };
        assert!(
            empty_hw_err.contains("Vulkan") && empty_hw_err.contains("No-GPU presenter"),
            "unexpected error message: {empty_hw_err}"
        );

        let empty_sw = BenchWgpuContext::init_with_backends(wgpu::Backends::empty(), true, 720, 480);
        let empty_sw_err = match empty_sw {
            Ok(_) => panic!("empty software backends must return clear Err"),
            Err(e) => e,
        };
        assert!(
            empty_sw_err.contains("Vulkan") && empty_sw_err.contains("No-GPU presenter"),
            "unexpected software empty error: {empty_sw_err}"
        );

        #[cfg(target_os = "windows")]
        {
            let mut warp_ctx =
                BenchWgpuContext::init_with_backends(wgpu::Backends::DX12, true, 720, 480)
                    .expect("Windows 10+ must expose built-in DX12 WARP Cpu adapter");
            assert_eq!(warp_ctx.adapter_backend, "Dx12");
            assert!(warp_ctx.is_cpu_adapter());
            let mut postproc = VideoPostProcessor::new();
            let fb = test_filter_pattern_fb();
            let img = postproc.process_display_filter(
                &fb,
                false,
                false,
                DisplayFilter::MyBoySmooth,
                false,
                0.08,
            );
            warp_ctx.present_plain_rgba_sync(
                img.as_raw(),
                img.size[0] as u32,
                img.size[1] as u32,
                true,
            );
        }
    }
}
