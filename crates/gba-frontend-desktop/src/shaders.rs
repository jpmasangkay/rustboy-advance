use std::sync::{Arc, Mutex};

use egui::{PaintCallback, PaintCallbackInfo};
use glow::HasContext as _;
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::config::{ColorProfileChoice, ShaderPreset};
use crate::filters::fill_linear_rgba;

const QUAD_VERTICES: [f32; 16] = [
    // NDC (x, y), UV (u, v) — top-left maps to GBA scanline 0 (v = 0.0)
    -1.0, 1.0, 0.0, 0.0, // top-left
    -1.0, -1.0, 0.0, 1.0, // bottom-left
    1.0, 1.0, 1.0, 0.0, // top-right
    1.0, -1.0, 1.0, 1.0, // bottom-right
];

const ATTRIB_POS: u32 = 0;
const ATTRIB_UV: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlslTarget {
    Desktop330Core,
    Desktop140,
    Gles300,
}

impl GlslTarget {
    pub fn from_gl(gl: &glow::Context) -> Self {
        let ver = gl.version();
        if ver.is_embedded {
            Self::Gles300
        } else if ver.major > 3 || (ver.major == 3 && ver.minor >= 3) {
            Self::Desktop330Core
        } else {
            Self::Desktop140
        }
    }

    pub fn supports_shaders(gl: &glow::Context) -> bool {
        let ver = gl.version();
        ver.major >= 3
    }

    pub const fn version_header(self) -> &'static str {
        match self {
            Self::Desktop330Core => "#version 330 core\n",
            Self::Desktop140 => "#version 140\n",
            Self::Gles300 => "#version 300 es\nprecision highp float;\nprecision highp sampler2D;\nprecision highp int;\n",
        }
    }
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

pub fn build_vertex_shader_source(target: GlslTarget) -> String {
    format!(
        "{header}\
in vec2 a_pos;
in vec2 a_uv;
out vec2 v_uv;

void main() {{
    v_uv = a_uv;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}}
",
        header = target.version_header()
    )
}

const FRAGMENT_COMMON_HEADER: &str = r#"
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_curr_tex;
uniform sampler2D u_prev_tex;
uniform vec2 u_src_size;
uniform vec2 u_dst_size;
uniform int u_color_profile;
uniform float u_frame_blend;
uniform float u_lcd_ghosting;

vec3 safe_pow(vec3 v, float e) {
    vec3 mask = step(vec3(0.000001), v);
    return mask * pow(max(v, vec3(0.000001)), vec3(e));
}

vec3 apply_color_profile(vec3 c, int profile) {
    if (profile == 1) {
        // AGB-001 unbacklit reflective TFT -> modern sRGB matrix
        vec3 lin = safe_pow(c, 2.4);
        vec3 mapped = vec3(
            dot(lin, vec3(0.845, 0.125, 0.030)),
            dot(lin, vec3(0.055, 0.675, 0.270)),
            dot(lin, vec3(0.000, 0.195, 0.805))
        );
        return clamp(safe_pow(mapped, 1.0 / 2.2), 0.0, 1.0);
    } else if (profile == 2) {
        // AGS-101 backlit vibrant TFT curve
        vec3 mapped = vec3(
            dot(c, vec3(0.94, 0.04, 0.02)),
            dot(c, vec3(0.03, 0.92, 0.05)),
            dot(c, vec3(0.01, 0.07, 0.92))
        );
        return clamp(safe_pow(mapped, 1.15), 0.0, 1.0);
    } else if (profile == 3) {
        // AGS-001 frontlit soft pastel curve
        vec3 mapped = vec3(
            0.04 + 0.82 * dot(c, vec3(0.84, 0.12, 0.04)),
            0.05 + 0.82 * dot(c, vec3(0.06, 0.82, 0.12)),
            0.08 + 0.84 * dot(c, vec3(0.04, 0.10, 0.86))
        );
        return clamp(mapped, 0.0, 1.0);
    }
    return clamp(c, 0.0, 1.0);
}

vec3 sample_texel(ivec2 coord) {
    ivec2 max_coord = ivec2(u_src_size) - ivec2(1);
    ivec2 c = clamp(coord, ivec2(0), max_coord);
    vec3 curr = apply_color_profile(texelFetch(u_curr_tex, c, 0).rgb, u_color_profile);
    float temporal = clamp(max(u_frame_blend, u_lcd_ghosting), 0.0, 0.5);
    if (temporal > 0.001) {
        vec3 prev = apply_color_profile(texelFetch(u_prev_tex, c, 0).rgb, u_color_profile);
        return mix(curr, prev, temporal);
    }
    return curr;
}

vec3 sample_sharp_bilinear(vec2 uv) {
    vec2 scale = max(u_dst_size / max(u_src_size, vec2(1.0)), vec2(1.0));
    vec2 pos = uv * u_src_size - vec2(0.5);
    ivec2 p0 = ivec2(floor(pos));
    vec2 f = pos - vec2(p0);
    vec2 w = clamp((f - vec2(0.5)) * scale + vec2(0.5), 0.0, 1.0);

    vec3 c00 = sample_texel(p0);
    vec3 c10 = sample_texel(p0 + ivec2(1, 0));
    vec3 c01 = sample_texel(p0 + ivec2(0, 1));
    vec3 c11 = sample_texel(p0 + ivec2(1, 1));
    return mix(mix(c00, c10, w.x), mix(c01, c11, w.x), w.y);
}
"#;

const FRAGMENT_MAIN_NEAREST: &str = r#"
void main() {
    ivec2 texel = ivec2(floor(v_uv * u_src_size));
    frag_color = vec4(sample_texel(texel), 1.0);
}
"#;

const FRAGMENT_MAIN_SHARP_BILINEAR: &str = r#"
void main() {
    vec3 col = sample_sharp_bilinear(v_uv);
    frag_color = vec4(col, 1.0);
}
"#;

const FRAGMENT_MAIN_LCD_GRID: &str = r#"
void main() {
    vec3 base = sample_sharp_bilinear(v_uv);
    vec2 scale_xy = u_dst_size / max(u_src_size, vec2(1.0));
    float scale = min(scale_xy.x, scale_xy.y);

    vec2 sub = fract(v_uv * u_src_size);

    // Inter-pixel dark matrix gaps (fade smoothly below 2x so small windows never alias)
    float grid_strength = smoothstep(1.4, 2.8, scale);
    float row_gap = smoothstep(0.34, 0.50, abs(sub.y - 0.5));
    float col_gap = smoothstep(0.38, 0.50, abs(sub.x - 0.5));
    float grid_mask = (1.0 - grid_strength * 0.28 * row_gap) * (1.0 - grid_strength * 0.14 * col_gap);

    // Authentic GBA vertical RGB subpixel triads (active when >= 3 screen px per GBA pixel)
    float triad_strength = smoothstep(2.2, 3.0, scale) * 0.72;
    float rw = exp(-18.0 * (sub.x - 1.0 / 6.0) * (sub.x - 1.0 / 6.0));
    float gw = exp(-18.0 * (sub.x - 0.5) * (sub.x - 0.5));
    float bw = exp(-18.0 * (sub.x - 5.0 / 6.0) * (sub.x - 5.0 / 6.0));
    vec3 triad = mix(vec3(1.0), vec3(0.76) + 0.44 * vec3(rw, gw, bw), triad_strength);

    // Subtle luminance compensation so grid lines remain bright and readable
    float gain = 1.0 + 0.10 * grid_strength;
    vec3 col = clamp(base * triad * grid_mask * gain, 0.0, 1.0);
    frag_color = vec4(col, 1.0);
}
"#;

const FRAGMENT_MAIN_CRT_SCANLINES: &str = r#"
vec2 crt_curve(vec2 uv) {
    vec2 c = uv * 2.0 - vec2(1.0);
    vec2 offset = c.yx * c.yx * vec2(0.050, 0.070);
    c = c + c * offset;
    return c * 0.5 + vec2(0.5);
}

void main() {
    vec2 curved_uv = crt_curve(v_uv);
    vec2 edge_dist = abs(curved_uv - vec2(0.5)) * 2.0;

    // Rounded CRT glass tube mask
    vec2 corner_d = max(edge_dist - vec2(0.94), vec2(0.0));
    float rounded_sdf = length(corner_d);
    float edge_mask = (1.0 - smoothstep(0.985, 1.005, max(edge_dist.x, edge_dist.y)))
                    * (1.0 - smoothstep(0.055, 0.080, rounded_sdf));

    if (edge_mask <= 0.001) {
        frag_color = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }

    vec3 base = sample_sharp_bilinear(clamp(curved_uv, vec2(0.0), vec2(1.0)));

    // Subtle phosphor glow / halation from immediate neighbors
    vec2 texel_step = vec2(1.0) / max(u_src_size, vec2(1.0));
    vec3 glow = (
        sample_sharp_bilinear(clamp(curved_uv + vec2(texel_step.x, 0.0), vec2(0.0), vec2(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv - vec2(texel_step.x, 0.0), vec2(0.0), vec2(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv + vec2(0.0, texel_step.y), vec2(0.0), vec2(1.0))) +
        sample_sharp_bilinear(clamp(curved_uv - vec2(0.0, texel_step.y), vec2(0.0), vec2(1.0)))
    ) * 0.25;

    float luma = dot(base, vec3(0.299, 0.587, 0.114));
    vec2 scale_xy = u_dst_size / max(u_src_size, vec2(1.0));
    float scale = min(scale_xy.x, scale_xy.y);
    float scan_strength = smoothstep(1.2, 2.5, scale);

    // Scanline electron beam profile: brighter pixels bloom wider
    float dy = abs(fract(curved_uv.y * u_src_size.y) - 0.5);
    float beam_width = mix(0.28, 0.48, luma);
    float norm_dy = dy / beam_width;
    float beam = exp(-norm_dy * norm_dy * 1.35);
    float scanline = mix(1.0, mix(0.48, 1.12, beam), scan_strength);

    // Subtle aperture grille modulation
    float grille_phase = sin(v_uv.x * u_dst_size.x * 3.14159265);
    float grille = mix(1.0, 0.95 + 0.05 * grille_phase, smoothstep(2.0, 4.0, scale));

    // Soft corner vignette
    vec2 centered = curved_uv * 2.0 - vec2(1.0);
    float vignette = clamp(1.0 - 0.16 * dot(centered, centered), 0.72, 1.0);

    vec3 col = (base * scanline + glow * (0.12 + 0.08 * luma)) * grille * vignette * edge_mask;
    frag_color = vec4(clamp(col, 0.0, 1.0), 1.0);
}
"#;

const FRAGMENT_MAIN_SCALE_EDGE: &str = r#"
float yuv_dist(vec3 c1, vec3 c2) {
    vec3 d = c1 - c2;
    float y = abs(dot(d, vec3(0.299, 0.587, 0.114)));
    float u = abs(dot(d, vec3(-0.14713, -0.28886, 0.436)));
    float v = abs(dot(d, vec3(0.615, -0.51499, -0.10001)));
    return 48.0 * y + 7.0 * u + 6.0 * v;
}

void main() {
    vec2 texel_pos = v_uv * u_src_size;
    ivec2 p = ivec2(floor(texel_pos));
    vec2 f = fract(texel_pos) - vec2(0.5);

    int sx = (f.x >= 0.0) ? 1 : -1;
    int sy = (f.y >= 0.0) ? 1 : -1;
    vec2 q = abs(f);

    vec3 E  = sample_texel(p);
    vec3 F  = sample_texel(p + ivec2(sx, 0));
    vec3 H  = sample_texel(p + ivec2(0, sy));
    vec3 I  = sample_texel(p + ivec2(sx, sy));
    vec3 D  = sample_texel(p + ivec2(-sx, 0));
    vec3 B  = sample_texel(p + ivec2(0, -sy));
    vec3 C  = sample_texel(p + ivec2(sx, -sy));
    vec3 G  = sample_texel(p + ivec2(-sx, sy));
    vec3 F4 = sample_texel(p + ivec2(2 * sx, 0));
    vec3 I4 = sample_texel(p + ivec2(2 * sx, sy));
    vec3 H5 = sample_texel(p + ivec2(0, 2 * sy));
    vec3 I5 = sample_texel(p + ivec2(sx, 2 * sy));

    float df_EF = yuv_dist(E, F);
    float df_EH = yuv_dist(E, H);
    float df_FH = yuv_dist(F, H);
    float df_EI = yuv_dist(E, I);

    float aa = clamp(
        max(u_src_size.x / max(u_dst_size.x, 1.0), u_src_size.y / max(u_dst_size.y, 1.0)) * 0.75,
        0.02,
        0.35
    );

    vec3 out_col = E;

    if (df_EF > 1.2 && df_EH > 1.2) {
        float df_EC = yuv_dist(E, C);
        float df_EG = yuv_dist(E, G);
        float df_IH5 = yuv_dist(I, H5);
        float df_IF4 = yuv_dist(I, F4);
        float df_HD = yuv_dist(H, D);
        float df_HI5 = yuv_dist(H, I5);
        float df_FB = yuv_dist(F, B);
        float df_FI4 = yuv_dist(F, I4);

        float w_diag = df_EC + df_EG + df_IH5 + df_IF4 + 4.0 * df_FH;
        float w_orth = df_HD + df_HI5 + df_FB + df_FI4 + 4.0 * df_EI;

        float df_EB = yuv_dist(E, B);
        float df_ED = yuv_dist(E, D);
        bool connected = (min(min(df_EC, df_EG), min(df_EB, df_ED)) < 4.0) || (df_FH < 2.5);

        if (w_diag < w_orth && connected) {
            float df_FG = yuv_dist(F, G);
            float df_HC = yuv_dist(H, C);
            bool shallow_h = (2.0 * df_FG <= df_EC) && (df_EI > 1.0) && (df_EF > df_FG);
            bool steep_v = (2.0 * df_HC <= df_EG) && (df_EI > 1.0) && (df_EH > df_HC);

            vec3 corner_col = (df_EF <= df_EH) ? F : H;
            if (df_FH < 3.0) {
                corner_col = mix(F, H, 0.5);
            }

            float edge_val = 0.0;
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

    frag_color = vec4(out_col, 1.0);
}
"#;

pub fn build_fragment_shader_source(target: GlslTarget, preset: ShaderPreset) -> String {
    let body = match preset {
        ShaderPreset::Nearest => FRAGMENT_MAIN_NEAREST,
        ShaderPreset::SharpBilinear => FRAGMENT_MAIN_SHARP_BILINEAR,
        ShaderPreset::LcdGrid => FRAGMENT_MAIN_LCD_GRID,
        ShaderPreset::CrtScanlines => FRAGMENT_MAIN_CRT_SCANLINES,
        ShaderPreset::ScaleEdge => FRAGMENT_MAIN_SCALE_EDGE,
    };
    format!(
        "{header}{common}{body}",
        header = target.version_header(),
        common = FRAGMENT_COMMON_HEADER,
        body = body,
    )
}

#[derive(Clone, Copy, Debug)]
pub struct ShaderRenderParams {
    pub preset: ShaderPreset,
    pub color_profile_id: i32,
    pub frame_blend: f32,
    pub lcd_ghosting: f32,
}

struct CompiledProgram {
    program: glow::Program,
    u_curr_tex: Option<glow::UniformLocation>,
    u_prev_tex: Option<glow::UniformLocation>,
    u_src_size: Option<glow::UniformLocation>,
    u_dst_size: Option<glow::UniformLocation>,
    u_color_profile: Option<glow::UniformLocation>,
    u_frame_blend: Option<glow::UniformLocation>,
    u_lcd_ghosting: Option<glow::UniformLocation>,
}

pub struct GlShaderPipeline {
    gl: Arc<glow::Context>,
    programs: [Option<CompiledProgram>; ShaderPreset::COUNT],
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    curr_tex: glow::Texture,
    prev_tex: glow::Texture,
    curr_staging: Vec<u8>,
    prev_staging: Vec<u8>,
    upload_dirty: bool,
    has_uploaded_once: bool,
    warned_failure: bool,
    destroyed: bool,
}

impl GlShaderPipeline {
    #[allow(unsafe_code)]
    pub fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        if !GlslTarget::supports_shaders(&gl) {
            let ver = gl.version();
            return Err(format!(
                "OpenGL {}.{} does not support GLSL 3.0+ shader pipeline",
                ver.major, ver.minor
            ));
        }

        let target = GlslTarget::from_gl(&gl);
        let vert_src = build_vertex_shader_source(target);

        let mut programs: [Option<CompiledProgram>; ShaderPreset::COUNT] = Default::default();
        let mut any_compiled = false;
        let mut errors = Vec::new();

        for preset in ShaderPreset::ALL {
            let frag_src = build_fragment_shader_source(target, preset);
            match unsafe { compile_program(&gl, &vert_src, &frag_src) } {
                Ok(prog) => {
                    programs[preset.index()] = Some(prog);
                    any_compiled = true;
                }
                Err(err) => {
                    errors.push(format!("{preset:?}: {err}"));
                }
            }
        }

        if !any_compiled {
            return Err(format!(
                "All GLSL shader presets failed to compile: {}",
                errors.join(" | ")
            ));
        }
        if !errors.is_empty() {
            log::warn!(
                "Some GLSL shader presets failed to compile and will fall back to software rendering: {}",
                errors.join(" | ")
            );
        }

        let (vao, vbo, curr_tex, prev_tex) = unsafe { create_gl_objects(&gl)? };

        let byte_len = LCD_WIDTH * LCD_HEIGHT * 4;
        Ok(Self {
            gl,
            programs,
            vao,
            vbo,
            curr_tex,
            prev_tex,
            curr_staging: vec![0; byte_len],
            prev_staging: vec![0; byte_len],
            upload_dirty: false,
            has_uploaded_once: false,
            warned_failure: false,
            destroyed: false,
        })
    }

    pub fn is_preset_available(&self, preset: ShaderPreset) -> bool {
        !self.destroyed && self.programs[preset.index()].is_some()
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

    #[allow(unsafe_code)]
    pub fn render(
        &mut self,
        gl: &glow::Context,
        info: &PaintCallbackInfo,
        params: ShaderRenderParams,
    ) {
        if self.destroyed {
            return;
        }

        let vp = info.viewport_in_pixels();
        let clip = info.clip_rect_in_pixels();
        let sc_x0 = vp.left_px.max(clip.left_px);
        let sc_y0 = vp.from_bottom_px.max(clip.from_bottom_px);
        let sc_x1 = (vp.left_px + vp.width_px).min(clip.left_px + clip.width_px);
        let sc_y1 = (vp.from_bottom_px + vp.height_px).min(clip.from_bottom_px + clip.height_px);
        let sc_w = (sc_x1 - sc_x0).max(0);
        let sc_h = (sc_y1 - sc_y0).max(0);
        if vp.width_px <= 0 || vp.height_px <= 0 || sc_w <= 0 || sc_h <= 0 {
            return;
        }

        let Some(prog) = self.programs[params.preset.index()].as_ref() else {
            return;
        };

        unsafe {
            if self.upload_dirty {
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);

                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.curr_tex));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    LCD_WIDTH as i32,
                    LCD_HEIGHT as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&self.curr_staging)),
                );

                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.prev_tex));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    LCD_WIDTH as i32,
                    LCD_HEIGHT as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&self.prev_staging)),
                );

                self.upload_dirty = false;
            }

            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(sc_x0, sc_y0, sc_w, sc_h);
            gl.disable(glow::BLEND);

            gl.use_program(Some(prog.program));

            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.curr_tex));
            if let Some(loc) = prog.u_curr_tex.as_ref() {
                gl.uniform_1_i32(Some(loc), 0);
            }

            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.prev_tex));
            if let Some(loc) = prog.u_prev_tex.as_ref() {
                gl.uniform_1_i32(Some(loc), 1);
            }

            if let Some(loc) = prog.u_src_size.as_ref() {
                gl.uniform_2_f32(Some(loc), LCD_WIDTH as f32, LCD_HEIGHT as f32);
            }
            if let Some(loc) = prog.u_dst_size.as_ref() {
                gl.uniform_2_f32(Some(loc), vp.width_px as f32, vp.height_px as f32);
            }
            if let Some(loc) = prog.u_color_profile.as_ref() {
                gl.uniform_1_i32(Some(loc), params.color_profile_id);
            }
            if let Some(loc) = prog.u_frame_blend.as_ref() {
                gl.uniform_1_f32(Some(loc), params.frame_blend);
            }
            if let Some(loc) = prog.u_lcd_ghosting.as_ref() {
                gl.uniform_1_f32(Some(loc), params.lcd_ghosting);
            }

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    #[allow(unsafe_code)]
    pub fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        unsafe {
            for slot in &mut self.programs {
                if let Some(prog) = slot.take() {
                    self.gl.delete_program(prog.program);
                }
            }
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_texture(self.curr_tex);
            self.gl.delete_texture(self.prev_tex);
        }
        self.destroyed = true;
    }
}

pub fn create_viewport_paint_callback(
    pipeline: Arc<Mutex<GlShaderPipeline>>,
    rect: egui::Rect,
    params: ShaderRenderParams,
) -> PaintCallback {
    PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            if let Ok(mut guard) = pipeline.lock() {
                guard.render(painter.gl(), &info, params);
            }
        })),
    }
}

#[allow(unsafe_code)]
unsafe fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    source: &str,
) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl.create_shader(shader_type)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if gl.get_shader_compile_status(shader) {
            Ok(shader)
        } else {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            Err(log)
        }
    }
}

#[allow(unsafe_code)]
unsafe fn compile_program(
    gl: &glow::Context,
    vert_src: &str,
    frag_src: &str,
) -> Result<CompiledProgram, String> {
    unsafe {
        let vert = compile_shader(gl, glow::VERTEX_SHADER, vert_src)?;
        let frag = match compile_shader(gl, glow::FRAGMENT_SHADER, frag_src) {
            Ok(s) => s,
            Err(err) => {
                gl.delete_shader(vert);
                return Err(err);
            }
        };

        let program = match gl.create_program() {
            Ok(p) => p,
            Err(err) => {
                gl.delete_shader(vert);
                gl.delete_shader(frag);
                return Err(err);
            }
        };

        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.bind_attrib_location(program, ATTRIB_POS, "a_pos");
        gl.bind_attrib_location(program, ATTRIB_UV, "a_uv");
        gl.link_program(program);

        gl.detach_shader(program, vert);
        gl.detach_shader(program, frag);
        gl.delete_shader(vert);
        gl.delete_shader(frag);

        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(log);
        }

        let u_curr_tex = gl.get_uniform_location(program, "u_curr_tex");
        let u_prev_tex = gl.get_uniform_location(program, "u_prev_tex");
        let u_src_size = gl.get_uniform_location(program, "u_src_size");
        let u_dst_size = gl.get_uniform_location(program, "u_dst_size");
        let u_color_profile = gl.get_uniform_location(program, "u_color_profile");
        let u_frame_blend = gl.get_uniform_location(program, "u_frame_blend");
        let u_lcd_ghosting = gl.get_uniform_location(program, "u_lcd_ghosting");

        Ok(CompiledProgram {
            program,
            u_curr_tex,
            u_prev_tex,
            u_src_size,
            u_dst_size,
            u_color_profile,
            u_frame_blend,
            u_lcd_ghosting,
        })
    }
}

#[allow(unsafe_code)]
unsafe fn create_gl_objects(
    gl: &glow::Context,
) -> Result<(glow::VertexArray, glow::Buffer, glow::Texture, glow::Texture), String> {
    unsafe {
        let vao = gl.create_vertex_array()?;
        let vbo = gl.create_buffer()?;

        let mut quad_bytes = [0u8; 16 * 4];
        for (i, val) in QUAD_VERTICES.iter().enumerate() {
            quad_bytes[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
        }

        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &quad_bytes, glow::STATIC_DRAW);

        let stride = (4 * std::mem::size_of::<f32>()) as i32;
        gl.vertex_attrib_pointer_f32(ATTRIB_POS, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(ATTRIB_POS);
        gl.vertex_attrib_pointer_f32(
            ATTRIB_UV,
            2,
            glow::FLOAT,
            false,
            stride,
            (2 * std::mem::size_of::<f32>()) as i32,
        );
        gl.enable_vertex_attrib_array(ATTRIB_UV);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);

        let curr_tex = create_frame_texture(gl)?;
        let prev_tex = match create_frame_texture(gl) {
            Ok(t) => t,
            Err(err) => {
                gl.delete_texture(curr_tex);
                gl.delete_buffer(vbo);
                gl.delete_vertex_array(vao);
                return Err(err);
            }
        };

        Ok((vao, vbo, curr_tex, prev_tex))
    }
}

#[allow(unsafe_code)]
unsafe fn create_frame_texture(gl: &glow::Context) -> Result<glow::Texture, String> {
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        let blank = vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4];
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            LCD_WIDTH as i32,
            LCD_HEIGHT as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&blank)),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(tex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED_UNIFORMS: [&str; 7] = [
        "u_curr_tex",
        "u_prev_tex",
        "u_src_size",
        "u_dst_size",
        "u_color_profile",
        "u_frame_blend",
        "u_lcd_ghosting",
    ];

    #[test]
    fn shader_sources_contain_version_uniforms_and_no_placeholders() {
        let targets = [
            (GlslTarget::Desktop330Core, "#version 330 core"),
            (GlslTarget::Gles300, "#version 300 es"),
            (GlslTarget::Desktop140, "#version 140"),
        ];

        for (target, expected_version) in targets {
            let vert = build_vertex_shader_source(target);
            assert!(
                vert.starts_with(expected_version),
                "vertex shader missing {expected_version}"
            );
            assert!(vert.contains("gl_Position"));
            assert!(vert.contains("void main()"));
            assert!(!vert.contains("TODO") && !vert.contains("FIXME") && !vert.contains("{{"));

            for preset in ShaderPreset::ALL {
                let frag = build_fragment_shader_source(target, preset);
                assert!(
                    frag.starts_with(expected_version),
                    "{preset:?} fragment shader missing {expected_version}"
                );
                for uniform in REQUIRED_UNIFORMS {
                    assert!(
                        frag.contains(uniform),
                        "{preset:?} ({target:?}) missing uniform {uniform}"
                    );
                }
                assert!(frag.contains("void main()"));
                assert!(frag.contains("frag_color"));
                assert!(frag.contains("apply_color_profile"));
                assert!(!frag.contains("TODO") && !frag.contains("FIXME") && !frag.contains("{{"));
            }
        }
    }

    #[test]
    fn color_profile_uniform_ids_match_glsl_branches() {
        assert_eq!(color_profile_uniform(false, ColorProfileChoice::Agb001), 0);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Linear), 0);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Agb001), 1);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Ags101), 2);
        assert_eq!(color_profile_uniform(true, ColorProfileChoice::Ags001), 3);
    }

    #[test]
    fn shader_preset_indices_are_unique_and_complete() {
        let mut seen = [false; ShaderPreset::COUNT];
        for preset in ShaderPreset::ALL {
            let idx = preset.index();
            assert!(!seen[idx]);
            seen[idx] = true;
        }
        assert!(seen.iter().all(|&x| x));
    }
}
