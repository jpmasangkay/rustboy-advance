use alloc::boxed::Box;
use serde::{Deserialize, Serialize};

use crate::serde_helpers::{alloc_boxed_array, box_bytes, box_u16s};
use crate::{LCD_HEIGHT, LCD_WIDTH};

#[allow(dead_code)]
const LAYER_BG0: usize = 0;
#[allow(dead_code)]
const LAYER_BG1: usize = 1;
#[allow(dead_code)]
const LAYER_BG2: usize = 2;
#[allow(dead_code)]
const LAYER_BG3: usize = 3;
const LAYER_OBJ: usize = 4;
const LAYER_BD: usize = 5;

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct ObjPixel {
    pub color: u16,
    pub prio: u8,
    pub semi_transparent: bool,
    pub present: bool,
}

impl ObjPixel {
    pub const fn empty() -> Self {
        Self {
            color: 0,
            prio: 4,
            semi_transparent: false,
            present: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Ppu {
    #[serde(with = "box_bytes")]
    pub pram: Box<[u8; 0x400]>,
    #[serde(with = "box_bytes")]
    pub vram: Box<[u8; 0x18000]>,
    #[serde(with = "box_bytes")]
    pub oam: Box<[u8; 0x400]>,
    #[serde(with = "box_u16s")]
    pub framebuffer: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
    pub frame_ready: bool,

    pub dispcnt: u16,
    pub greenswap: u16,
    pub dispstat: u16,
    pub vcount: u16,

    pub bgcnt: [u16; 4],
    pub bghofs: [u16; 4],
    pub bgvofs: [u16; 4],

    // Affine BG2 & BG3 parameters (0 = BG2, 1 = BG3)
    pub bgp_a: [i16; 2],
    pub bgp_b: [i16; 2],
    pub bgp_c: [i16; 2],
    pub bgp_d: [i16; 2],
    pub bgx: [i32; 2],
    pub bgy: [i32; 2],
    pub ref_x: [i32; 2],
    pub ref_y: [i32; 2],

    pub winh: [u16; 2],
    pub winv: [u16; 2],
    pub winin: u16,
    pub winout: u16,
    pub mosaic: u16,
    pub bldcnt: u16,
    pub bldalpha: u16,
    pub bldy: u16,

    #[serde(default)]
    pub dot_accurate_mode: bool,
    #[serde(default)]
    pub dot_x: u16,
    #[serde(default = "default_layer_enable_mask")]
    pub layer_enable_mask: u8,

    // Scratch scanline buffers (heap-less inline fixed 240 elements)
    #[serde(skip, default = "default_bg_line")]
    bg_lines: [[u16; LCD_WIDTH]; 4],
    #[serde(skip, default = "default_bg_opaque")]
    bg_opaque: [[bool; LCD_WIDTH]; 4],
    #[serde(skip, default = "default_obj_line")]
    obj_line: [ObjPixel; LCD_WIDTH],
    #[serde(skip, default = "default_obj_win")]
    obj_win: [bool; LCD_WIDTH],
}

fn default_layer_enable_mask() -> u8 {
    0x1F
}

fn default_bg_line() -> [[u16; LCD_WIDTH]; 4] {
    [[0; LCD_WIDTH]; 4]
}
fn default_bg_opaque() -> [[bool; LCD_WIDTH]; 4] {
    [[false; LCD_WIDTH]; 4]
}
fn default_obj_line() -> [ObjPixel; LCD_WIDTH] {
    [ObjPixel::empty(); LCD_WIDTH]
}
fn default_obj_win() -> [bool; LCD_WIDTH] {
    [false; LCD_WIDTH]
}

impl Ppu {
    pub fn new() -> Self {
        Self {
            pram: alloc_boxed_array(0u8),
            vram: alloc_boxed_array(0u8),
            oam: alloc_boxed_array(0u8),
            framebuffer: alloc_boxed_array(0x7FFFu16),
            frame_ready: false,
            dispcnt: 0x0080, // Start forced blank until software clears it or direct boot
            greenswap: 0,
            dispstat: 0,
            vcount: 0,
            bgcnt: [0; 4],
            bghofs: [0; 4],
            bgvofs: [0; 4],
            bgp_a: [0x0100; 2],
            bgp_b: [0; 2],
            bgp_c: [0; 2],
            bgp_d: [0x0100; 2],
            bgx: [0; 2],
            bgy: [0; 2],
            ref_x: [0; 2],
            ref_y: [0; 2],
            winh: [0; 2],
            winv: [0; 2],
            winin: 0,
            winout: 0,
            mosaic: 0,
            bldcnt: 0,
            bldalpha: 0,
            bldy: 0,
            dot_accurate_mode: false,
            dot_x: 0,
            layer_enable_mask: 0x1F,
            bg_lines: default_bg_line(),
            bg_opaque: default_bg_opaque(),
            obj_line: default_obj_line(),
            obj_win: default_obj_win(),
        }
    }

    #[inline]
    pub fn vcount_match_target(&self) -> u16 {
        ((self.dispstat >> 8) & 0xFF) | ((self.dispstat & 0x80) << 1)
    }

    #[inline]
    pub fn set_bgx(&mut self, idx: usize, val: u32) {
        let signed = ((val << 4) as i32) >> 4;
        self.bgx[idx] = signed;
        self.ref_x[idx] = signed;
    }

    #[inline]
    pub fn set_bgy(&mut self, idx: usize, val: u32) {
        let signed = ((val << 4) as i32) >> 4;
        self.bgy[idx] = signed;
        self.ref_y[idx] = signed;
    }

    #[inline]
    fn read_pram_u16(&self, index: usize) -> u16 {
        let off = (index & 0x1FF) * 2;
        u16::from_le_bytes([self.pram[off], self.pram[off + 1]]) & 0x7FFF
    }

    #[inline]
    fn read_vram_u16(&self, byte_off: usize) -> u16 {
        let off = Self::mirror_vram(byte_off) & !1;
        u16::from_le_bytes([self.vram[off], self.vram[off + 1]])
    }

    #[inline]
    pub fn mirror_vram(byte_off: usize) -> usize {
        let masked = byte_off & 0x1FFFF;
        if masked >= 0x18000 {
            masked - 0x8000
        } else {
            masked
        }
    }

    #[inline]
    fn read_oam_u16(&self, byte_off: usize) -> u16 {
        let off = byte_off & 0x3FE;
        u16::from_le_bytes([self.oam[off], self.oam[off + 1]])
    }

    pub fn reload_affine_refs(&mut self) {
        self.ref_x = self.bgx;
        self.ref_y = self.bgy;
    }

    pub fn step_affine_refs_scanline(&mut self) {
        self.ref_x[0] = self.ref_x[0].wrapping_add(self.bgp_b[0] as i32);
        self.ref_y[0] = self.ref_y[0].wrapping_add(self.bgp_d[0] as i32);
        self.ref_x[1] = self.ref_x[1].wrapping_add(self.bgp_b[1] as i32);
        self.ref_y[1] = self.ref_y[1].wrapping_add(self.bgp_d[1] as i32);
    }

    pub fn begin_scanline(&mut self, y: usize) {
        if y >= LCD_HEIGHT {
            return;
        }
        self.dot_x = 0;
        for bg in 0..4 {
            self.bg_opaque[bg].fill(false);
        }
        self.obj_line.fill(ObjPixel::empty());
        self.obj_win.fill(false);

        let mode = self.dispcnt & 0x07;
        if (self.dispcnt & 0x0080) == 0 && (self.dispcnt & 0x1000) != 0 {
            self.render_sprites(y, mode >= 3);
        }
    }

    pub fn render_dot_range(&mut self, y: usize, x_start: usize, x_end: usize) {
        let x0 = x_start.min(LCD_WIDTH);
        let x1 = x_end.min(LCD_WIDTH);
        if y >= LCD_HEIGHT || x0 >= x1 {
            return;
        }
        let row_start = y * LCD_WIDTH;

        // Forced Blank -> pure white
        if (self.dispcnt & 0x0080) != 0 {
            self.framebuffer[row_start + x0..row_start + x1].fill(0x7FFF);
            return;
        }

        for bg in 0..4 {
            self.bg_opaque[bg][x0..x1].fill(false);
        }

        let mode = self.dispcnt & 0x07;
        match mode {
            0 => {
                for bg in 0..4 {
                    if (self.dispcnt & (0x0100 << bg)) != 0 {
                        self.render_regular_bg(bg, y, x0, x1);
                    }
                }
            }
            1 => {
                if (self.dispcnt & 0x0100) != 0 {
                    self.render_regular_bg(0, y, x0, x1);
                }
                if (self.dispcnt & 0x0200) != 0 {
                    self.render_regular_bg(1, y, x0, x1);
                }
                if (self.dispcnt & 0x0400) != 0 {
                    self.render_affine_bg(2, x0, x1);
                }
            }
            2 => {
                if (self.dispcnt & 0x0400) != 0 {
                    self.render_affine_bg(2, x0, x1);
                }
                if (self.dispcnt & 0x0800) != 0 {
                    self.render_affine_bg(3, x0, x1);
                }
            }
            3 => {
                if (self.dispcnt & 0x0400) != 0 {
                    self.render_mode3_bg(x0, x1);
                }
            }
            4 => {
                if (self.dispcnt & 0x0400) != 0 {
                    self.render_mode4_bg(x0, x1);
                }
            }
            5 if (self.dispcnt & 0x0400) != 0 => {
                self.render_mode5_bg(x0, x1);
            }
            _ => {}
        }

        self.compose_scanline(y, row_start, x0, x1);
    }

    pub fn end_scanline(&mut self) {
        let mode = self.dispcnt & 0x07;
        if (self.dispcnt & 0x0080) == 0 && (1..=5).contains(&mode) {
            self.step_affine_refs_scanline();
        }
    }

    pub fn render_scanline(&mut self, y: usize) {
        if y >= LCD_HEIGHT {
            return;
        }
        self.begin_scanline(y);
        self.render_dot_range(y, 0, LCD_WIDTH);
        self.end_scanline();
    }

    fn render_regular_bg(&mut self, bg: usize, y: usize, x_start: usize, x_end: usize) {
        let cnt = self.bgcnt[bg];
        let char_base = (((cnt >> 2) & 0x03) as usize) * 0x4000;
        let mosaic_en = (cnt & 0x0040) != 0;
        let is_8bpp = (cnt & 0x0080) != 0;
        let screen_base = (((cnt >> 8) & 0x1F) as usize) * 0x0800;
        let size_mode = (cnt >> 14) & 0x03;

        let width_px: usize = if (size_mode & 1) != 0 { 512 } else { 256 };
        let height_px: usize = if (size_mode & 2) != 0 { 512 } else { 256 };

        let mos_h = if mosaic_en { ((self.mosaic & 0x0F) as usize) + 1 } else { 1 };
        let mos_v = if mosaic_en { (((self.mosaic >> 4) & 0x0F) as usize) + 1 } else { 1 };
        let eff_y = y - (y % mos_v);

        let scroll_x = (self.bghofs[bg] & 0x1FF) as usize;
        let scroll_y = (self.bgvofs[bg] & 0x1FF) as usize;

        let bg_y = (eff_y + scroll_y) & (height_px - 1);
        let tile_y = bg_y >> 3;
        let fine_y = bg_y & 7;

        for x in x_start..x_end {
            let eff_x = if mos_h > 1 { x - (x % mos_h) } else { x };
            let bg_x = (eff_x + scroll_x) & (width_px - 1);
            let tile_x = bg_x >> 3;
            let fine_x = bg_x & 7;

            let mut block = 0usize;
            if tile_x >= 32 {
                block += 1;
            }
            if tile_y >= 32 {
                block += if (size_mode & 1) != 0 { 2 } else { 1 };
            }

            let entry_idx = block * 1024 + (tile_y & 31) * 32 + (tile_x & 31);
            let entry = self.read_vram_u16(screen_base + entry_idx * 2);

            let tile_num = (entry & 0x03FF) as usize;
            let h_flip = (entry & 0x0400) != 0;
            let v_flip = (entry & 0x0800) != 0;
            let pal_bank = ((entry >> 12) & 0x0F) as usize;

            let tx = if h_flip { 7 - fine_x } else { fine_x };
            let ty = if v_flip { 7 - fine_y } else { fine_y };

            if is_8bpp {
                let addr = char_base + tile_num * 64 + ty * 8 + tx;
                let col_idx = self.vram[Self::mirror_vram(addr)] as usize;
                if col_idx != 0 {
                    self.bg_lines[bg][x] = self.read_pram_u16(col_idx);
                    self.bg_opaque[bg][x] = true;
                }
            } else {
                let addr = char_base + tile_num * 32 + ty * 4 + (tx >> 1);
                let byte = self.vram[Self::mirror_vram(addr)];
                let col_idx = ((byte >> ((tx & 1) * 4)) & 0x0F) as usize;
                if col_idx != 0 {
                    self.bg_lines[bg][x] = self.read_pram_u16(pal_bank * 16 + col_idx);
                    self.bg_opaque[bg][x] = true;
                }
            }
        }
    }

    fn render_affine_bg(&mut self, bg: usize, x_start: usize, x_end: usize) {
        let aff_idx = bg - 2;
        let cnt = self.bgcnt[bg];
        let char_base = (((cnt >> 2) & 0x03) as usize) * 0x4000;
        let screen_base = (((cnt >> 8) & 0x1F) as usize) * 0x0800;
        let wraparound = (cnt & 0x2000) != 0;
        let size_mode = ((cnt >> 14) & 0x03) as usize;
        let map_px = 128i32 << size_mode;
        let tiles_per_row = map_px >> 3;

        let pa = self.bgp_a[aff_idx] as i32;
        let pc = self.bgp_c[aff_idx] as i32;
        let mut cx = self.ref_x[aff_idx].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[aff_idx].wrapping_add(pc.wrapping_mul(x_start as i32));

        for x in x_start..x_end {
            let mut px = cx >> 8;
            let mut py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if wraparound {
                px = px.rem_euclid(map_px);
                py = py.rem_euclid(map_px);
            } else if px < 0 || py < 0 || px >= map_px || py >= map_px {
                continue;
            }

            let tile_x = (px >> 3) as usize;
            let tile_y = (py >> 3) as usize;
            let fine_x = (px & 7) as usize;
            let fine_y = (py & 7) as usize;

            let map_addr = screen_base + tile_y * (tiles_per_row as usize) + tile_x;
            let tile_num = self.vram[Self::mirror_vram(map_addr)] as usize;
            let pixel_addr = char_base + tile_num * 64 + fine_y * 8 + fine_x;
            let col_idx = self.vram[Self::mirror_vram(pixel_addr)] as usize;

            if col_idx != 0 {
                self.bg_lines[bg][x] = self.read_pram_u16(col_idx);
                self.bg_opaque[bg][x] = true;
            }
        }
    }

    fn render_mode3_bg(&mut self, x_start: usize, x_end: usize) {
        let pa = self.bgp_a[0] as i32;
        let pc = self.bgp_c[0] as i32;
        let mut cx = self.ref_x[0].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[0].wrapping_add(pc.wrapping_mul(x_start as i32));
        let wrap = (self.bgcnt[2] & 0x2000) != 0;

        for x in x_start..x_end {
            let mut px = cx >> 8;
            let mut py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if wrap {
                px = px.rem_euclid(LCD_WIDTH as i32);
                py = py.rem_euclid(LCD_HEIGHT as i32);
            } else if px < 0 || py < 0 || px >= LCD_WIDTH as i32 || py >= LCD_HEIGHT as i32 {
                continue;
            }

            let offset = ((py as usize) * LCD_WIDTH + (px as usize)) * 2;
            self.bg_lines[2][x] = self.read_vram_u16(offset) & 0x7FFF;
            self.bg_opaque[2][x] = true;
        }
    }

    fn render_mode4_bg(&mut self, x_start: usize, x_end: usize) {
        let page_base = if (self.dispcnt & 0x0010) != 0 { 0xA000 } else { 0 };
        let pa = self.bgp_a[0] as i32;
        let pc = self.bgp_c[0] as i32;
        let mut cx = self.ref_x[0].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[0].wrapping_add(pc.wrapping_mul(x_start as i32));

        for x in x_start..x_end {
            let px = cx >> 8;
            let py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if px < 0 || py < 0 || px >= LCD_WIDTH as i32 || py >= LCD_HEIGHT as i32 {
                continue;
            }

            let offset = page_base + (py as usize) * LCD_WIDTH + (px as usize);
            let col_idx = self.vram[offset] as usize;
            if col_idx != 0 {
                self.bg_lines[2][x] = self.read_pram_u16(col_idx);
                self.bg_opaque[2][x] = true;
            }
        }
    }

    fn render_mode5_bg(&mut self, x_start: usize, x_end: usize) {
        let page_base = if (self.dispcnt & 0x0010) != 0 { 0xA000 } else { 0 };
        let pa = self.bgp_a[0] as i32;
        let pc = self.bgp_c[0] as i32;
        let mut cx = self.ref_x[0].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[0].wrapping_add(pc.wrapping_mul(x_start as i32));

        for x in x_start..x_end {
            let px = cx >> 8;
            let py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if px < 0 || py < 0 || px >= 160 || py >= 128 {
                continue;
            }

            let offset = page_base + ((py as usize) * 160 + (px as usize)) * 2;
            self.bg_lines[2][x] = self.read_vram_u16(offset) & 0x7FFF;
            self.bg_opaque[2][x] = true;
        }
    }

    fn render_sprites(&mut self, y: usize, bitmap_mode: bool) {
        let mapping_1d = (self.dispcnt & 0x0040) != 0;
        let mut cycle_budget: i32 = if (self.dispcnt & 0x0020) != 0 { 954 } else { 1210 };
        let mos_v = (((self.mosaic >> 12) & 0x0F) as usize) + 1;
        let mos_h = (((self.mosaic >> 8) & 0x0F) as usize) + 1;

        for idx in 0..128usize {
            let attr0 = self.read_oam_u16(idx * 8);
            let mode = (attr0 >> 8) & 0x03;
            if mode == 2 {
                continue; // Disabled OBJ
            }
            let obj_mode = (attr0 >> 10) & 0x03;
            if obj_mode == 3 {
                continue; // Prohibited
            }

            let attr1 = self.read_oam_u16(idx * 8 + 2);
            let attr2 = self.read_oam_u16(idx * 8 + 4);

            let shape = (attr0 >> 14) & 0x03;
            let size = (attr1 >> 14) & 0x03;
            let (spr_w, spr_h): (i32, i32) = match (shape, size) {
                (0, 0) => (8, 8),
                (0, 1) => (16, 16),
                (0, 2) => (32, 32),
                (0, 3) => (64, 64),
                (1, 0) => (16, 8),
                (1, 1) => (32, 8),
                (1, 2) => (32, 16),
                (1, 3) => (64, 32),
                (2, 0) => (8, 16),
                (2, 1) => (8, 32),
                (2, 2) => (16, 32),
                (2, 3) => (32, 64),
                _ => continue,
            };

            let is_affine = (mode & 1) != 0;
            let is_double = mode == 3;
            let box_w = if is_double { spr_w * 2 } else { spr_w };
            let box_h = if is_double { spr_h * 2 } else { spr_h };

            let mut spr_y = (attr0 & 0xFF) as i32;
            if spr_y >= 160 {
                spr_y -= 256;
            }
            if (y as i32) < spr_y || (y as i32) >= spr_y + box_h {
                continue;
            }

            let mut spr_x = (attr1 & 0x01FF) as i32;
            if spr_x >= 240 {
                spr_x -= 512;
            }

            // Budget deduction approximation matching GBA hardware (1 cyc/pixel or 2 cyc/pixel for affine + 10 setup)
            let cost = if is_affine { 10 + box_w * 2 } else { box_w };
            cycle_budget -= cost;
            if cycle_budget < 0 {
                break;
            }

            let mosaic_en = (attr0 & 0x1000) != 0;
            let is_8bpp = (attr0 & 0x2000) != 0;
            let mut tile_base = (attr2 & 0x03FF) as usize;
            if is_8bpp {
                tile_base &= !1;
            }
            let prio = ((attr2 >> 10) & 0x03) as u8;
            let pal_bank = ((attr2 >> 12) & 0x0F) as usize;

            let eff_y = if mosaic_en {
                let my = y - (y % mos_v);
                (my as i32).max(spr_y) - spr_y
            } else {
                (y as i32) - spr_y
            };

            let (pa, pb, pc, pd) = if is_affine {
                let aff_idx = ((attr1 >> 9) & 0x1F) as usize;
                (
                    self.read_oam_u16(aff_idx * 32 + 0x06) as i16 as i32,
                    self.read_oam_u16(aff_idx * 32 + 0x0E) as i16 as i32,
                    self.read_oam_u16(aff_idx * 32 + 0x16) as i16 as i32,
                    self.read_oam_u16(aff_idx * 32 + 0x1E) as i16 as i32,
                )
            } else {
                (0, 0, 0, 0)
            };

            let h_flip = !is_affine && (attr1 & 0x1000) != 0;
            let v_flip = !is_affine && (attr1 & 0x2000) != 0;
            let semi_transparent = obj_mode == 1;
            let is_obj_win = obj_mode == 2;

            for local_x in 0..box_w {
                let screen_x = spr_x + local_x;
                if !(0..LCD_WIDTH as i32).contains(&screen_x) {
                    continue;
                }
                let sx = screen_x as usize;

                let (tex_x, tex_y) = if is_affine {
                    let dx = local_x - (box_w >> 1);
                    let dy = eff_y - (box_h >> 1);
                    let tx = ((pa * dx + pb * dy) >> 8) + (spr_w >> 1);
                    let ty = ((pc * dx + pd * dy) >> 8) + (spr_h >> 1);
                    if tx < 0 || ty < 0 || tx >= spr_w || ty >= spr_h {
                        continue;
                    }
                    (tx as usize, ty as usize)
                } else {
                    let eff_local_x = if mosaic_en && mos_h > 1 {
                        (sx - (sx % mos_h)) as i32 - spr_x
                    } else {
                        local_x
                    }
                    .clamp(0, spr_w - 1);
                    let tx = if h_flip { spr_w - 1 - eff_local_x } else { eff_local_x } as usize;
                    let ty = if v_flip { spr_h - 1 - eff_y } else { eff_y } as usize;
                    (tx, ty)
                };

                let tile_col = tex_x >> 3;
                let tile_row = tex_y >> 3;
                let fine_x = tex_x & 7;
                let fine_y = tex_y & 7;

                let tile_num = if mapping_1d {
                    let row_stride = (spr_w as usize) >> 3;
                    let step = if is_8bpp { 2 } else { 1 };
                    (tile_base + (tile_row * row_stride + tile_col) * step) & 0x03FF
                } else {
                    let step = if is_8bpp { 2 } else { 1 };
                    let row_part = ((tile_base & 0x3E0) + tile_row * 32) & 0x3FF;
                    let col_part = ((tile_base & 0x01F) + tile_col * step) & 0x01F;
                    row_part | col_part
                };

                // Hardware rule: bitmap Modes 3-5 restrict OBJ tiles to >= 512 (second 16KB block).
                if bitmap_mode && tile_num < 512 {
                    continue;
                }

                let col_idx = if is_8bpp {
                    let vram_off = 0x10000 + tile_num * 32 + fine_y * 8 + fine_x;
                    self.vram[vram_off] as usize
                } else {
                    let vram_off = 0x10000 + tile_num * 32 + fine_y * 4 + (fine_x >> 1);
                    let byte = self.vram[vram_off];
                    ((byte >> ((fine_x & 1) * 4)) & 0x0F) as usize
                };

                if col_idx == 0 {
                    continue;
                }

                if is_obj_win {
                    self.obj_win[sx] = true;
                    continue;
                }

                // OAM priority vs index rule:
                // Smaller OAM index keeps pixel if priority is equal OR already has lower priority.
                // However, an OAM entry with higher prio can still overwrite lower? Wait:
                // On GBA hardware: priority takes precedence first; if priorities differ, strictly lower
                // priority number wins! On equal priority, lower OAM index wins!
                let cur = &self.obj_line[sx];
                if !cur.present || prio < cur.prio {
                    let pram_idx = if is_8bpp { 256 + col_idx } else { 256 + pal_bank * 16 + col_idx };
                    self.obj_line[sx] = ObjPixel {
                        color: self.read_pram_u16(pram_idx),
                        prio,
                        semi_transparent,
                        present: true,
                    };
                }
            }
        }
    }

    #[inline]
    fn inside_window(&self, win_idx: usize, x: usize, y: usize) -> bool {
        let h = self.winh[win_idx];
        let x1 = ((h >> 8) & 0xFF) as usize;
        let x2 = (h & 0xFF) as usize;
        let in_x = if x1 == x2 {
            false
        } else if x1 < x2 {
            x >= x1 && x < x2
        } else {
            x >= x1 || x < x2
        };
        if !in_x {
            return false;
        }

        let v = self.winv[win_idx];
        let y1 = ((v >> 8) & 0xFF) as usize;
        let y2 = (v & 0xFF) as usize;
        if y1 == y2 {
            false
        } else if y1 < y2 {
            y >= y1 && y < y2
        } else {
            y >= y1 || y < y2
        }
    }

    fn compose_scanline(&mut self, y: usize, row_start: usize, x_start: usize, x_end: usize) {
        let backdrop = self.read_pram_u16(0);
        let win0_en = (self.dispcnt & 0x2000) != 0;
        let win1_en = (self.dispcnt & 0x4000) != 0;
        let objwin_en = (self.dispcnt & 0x8000) != 0;
        let any_win = win0_en || win1_en || objwin_en;

        let bld_mode = (self.bldcnt >> 6) & 0x03;
        let eva = (self.bldalpha & 0x1F).min(16);
        let evb = ((self.bldalpha >> 8) & 0x1F).min(16);
        let evy = (self.bldy & 0x1F).min(16);

        let bg_prios = [
            (self.bgcnt[0] & 3) as u8,
            (self.bgcnt[1] & 3) as u8,
            (self.bgcnt[2] & 3) as u8,
            (self.bgcnt[3] & 3) as u8,
        ];

        let layer_mask = self.layer_enable_mask;

        for x in x_start..x_end {
            let win_mask: u8 = if !any_win {
                0x3F
            } else if win0_en && self.inside_window(0, x, y) {
                (self.winin & 0x3F) as u8
            } else if win1_en && self.inside_window(1, x, y) {
                ((self.winin >> 8) & 0x3F) as u8
            } else if objwin_en && self.obj_win[x] {
                ((self.winout >> 8) & 0x3F) as u8
            } else {
                (self.winout & 0x3F) as u8
            };

            let mut top_layer = LAYER_BD;
            let mut top_col = backdrop;
            let mut top_semi = false;

            let mut bot_layer = LAYER_BD;
            let mut bot_col = backdrop;

            let obj = self.obj_line[x];
            let obj_visible =
                obj.present && (win_mask & (1 << LAYER_OBJ)) != 0 && (layer_mask & 0x10) != 0;

            'outer: for prio in 0u8..4 {
                if obj_visible && obj.prio == prio {
                    if top_layer == LAYER_BD {
                        top_layer = LAYER_OBJ;
                        top_col = obj.color;
                        top_semi = obj.semi_transparent;
                    } else {
                        bot_layer = LAYER_OBJ;
                        bot_col = obj.color;
                        break 'outer;
                    }
                }
                for bg in 0..4 {
                    if self.bg_opaque[bg][x]
                        && (win_mask & (1 << bg)) != 0
                        && (layer_mask & (1 << bg)) != 0
                        && bg_prios[bg] == prio
                    {
                        if top_layer == LAYER_BD {
                            top_layer = bg;
                            top_col = self.bg_lines[bg][x];
                        } else {
                            bot_layer = bg;
                            bot_col = self.bg_lines[bg][x];
                            break 'outer;
                        }
                    }
                }
            }

            let sfx_allowed = (win_mask & 0x20) != 0;
            let is_target1 = (self.bldcnt & (1 << top_layer)) != 0;
            let is_target2 = (self.bldcnt & (0x0100 << bot_layer)) != 0;

            let final_col = if sfx_allowed {
                if top_semi && is_target2 {
                    Self::alpha_blend(top_col, bot_col, eva, evb)
                } else {
                    match bld_mode {
                        1 if is_target1 && is_target2 => {
                            Self::alpha_blend(top_col, bot_col, eva, evb)
                        }
                        2 if is_target1 => Self::brighten(top_col, evy),
                        3 if is_target1 => Self::darken(top_col, evy),
                        _ => top_col,
                    }
                }
            } else {
                top_col
            };

            self.framebuffer[row_start + x] = final_col;
        }
    }

    #[inline]
    fn alpha_blend(top: u16, bot: u16, eva: u16, evb: u16) -> u16 {
        let r = (((top & 0x1F) * eva + (bot & 0x1F) * evb) >> 4).min(31);
        let g = ((((top >> 5) & 0x1F) * eva + ((bot >> 5) & 0x1F) * evb) >> 4).min(31);
        let b = ((((top >> 10) & 0x1F) * eva + ((bot >> 10) & 0x1F) * evb) >> 4).min(31);
        r | (g << 5) | (b << 10)
    }

    #[inline]
    fn brighten(col: u16, evy: u16) -> u16 {
        let r = col & 0x1F;
        let g = (col >> 5) & 0x1F;
        let b = (col >> 10) & 0x1F;
        let nr = r + (((31 - r) * evy) >> 4);
        let ng = g + (((31 - g) * evy) >> 4);
        let nb = b + (((31 - b) * evy) >> 4);
        nr | (ng << 5) | (nb << 10)
    }

    #[inline]
    fn darken(col: u16, evy: u16) -> u16 {
        let r = col & 0x1F;
        let g = (col >> 5) & 0x1F;
        let b = (col >> 10) & 0x1F;
        let nr = r - ((r * evy) >> 4);
        let ng = g - ((g * evy) >> 4);
        let nb = b - ((b * evy) >> 4);
        nr | (ng << 5) | (nb << 10)
    }
}
