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
pub struct ObjPixel(u32);

impl ObjPixel {
    const COLOR_MASK: u32 = 0x7FFF;
    const PRESENT_BIT: u32 = 1 << 15;
    const PRIO_SHIFT: u32 = 16;
    const PRIO_MASK: u32 = 0x7 << Self::PRIO_SHIFT;
    const SEMI_BIT: u32 = 1 << 19;
    const MOSAIC_BIT: u32 = 1 << 20;

    #[inline(always)]
    pub const fn empty() -> Self {
        Self(4 << Self::PRIO_SHIFT)
    }

    #[inline(always)]
    pub const fn new(color: u16, prio: u8, semi_transparent: bool, mosaic: bool) -> Self {
        Self(
            (color as u32 & Self::COLOR_MASK)
                | Self::PRESENT_BIT
                | ((prio as u32 & 0x7) << Self::PRIO_SHIFT)
                | ((semi_transparent as u32) << 19)
                | ((mosaic as u32) << 20),
        )
    }

    #[inline(always)]
    pub const fn is_present(self) -> bool {
        (self.0 & Self::PRESENT_BIT) != 0
    }

    #[inline(always)]
    pub const fn prio(self) -> u8 {
        ((self.0 >> Self::PRIO_SHIFT) & 0x7) as u8
    }

    #[inline(always)]
    pub const fn color(self) -> u16 {
        (self.0 & Self::COLOR_MASK) as u16
    }

    #[inline(always)]
    pub const fn semi_transparent(self) -> bool {
        (self.0 & Self::SEMI_BIT) != 0
    }

    #[inline(always)]
    pub const fn mosaic(self) -> bool {
        (self.0 & Self::MOSAIC_BIT) != 0
    }

    #[inline(always)]
    pub const fn with_transparent_prio(self, prio: u8, mosaic: bool) -> Self {
        Self(
            (self.0 & !(Self::PRIO_MASK | Self::MOSAIC_BIT))
                | ((prio as u32 & 0x7) << Self::PRIO_SHIFT)
                | ((mosaic as u32) << 20),
        )
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
    #[serde(skip, default)]
    pub skip_rendering: bool,
    #[serde(skip, default)]
    pub skipped_frames: u64,

    // Scratch scanline buffers (heap-less inline fixed 240 elements; bg_lines bit 15 = opaque)
    #[serde(skip, default = "default_bg_line")]
    bg_lines: [[u16; LCD_WIDTH]; 4],
    #[serde(skip, default = "default_obj_line")]
    obj_line: [ObjPixel; LCD_WIDTH],
    #[serde(skip, default = "default_obj_win")]
    obj_win: [bool; LCD_WIDTH],
    #[serde(skip, default)]
    obj_line_dirty: bool,
    #[serde(skip, default)]
    obj_win_dirty: bool,
    #[serde(skip, default)]
    obj_rendered_this_line: bool,
    #[serde(skip, default)]
    obj_win_rendered: bool,
    #[serde(skip, default)]
    obj_has_semi: bool,
}

fn default_layer_enable_mask() -> u8 {
    0x1F
}

fn default_bg_line() -> [[u16; LCD_WIDTH]; 4] {
    [[0; LCD_WIDTH]; 4]
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
            skip_rendering: false,
            skipped_frames: 0,
            bg_lines: default_bg_line(),
            obj_line: default_obj_line(),
            obj_win: default_obj_win(),
            obj_line_dirty: false,
            obj_win_dirty: false,
            obj_rendered_this_line: false,
            obj_win_rendered: false,
            obj_has_semi: false,
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
    pub fn mirror_vram(byte_off: usize, dispcnt: u16) -> Option<usize> {
        let masked = byte_off & 0x1FFFF;
        if masked >= 0x18000 {
            if (dispcnt & 0x07) >= 3 {
                if masked < 0x1C000 {
                    None
                } else {
                    Some(masked - 0x8000)
                }
            } else {
                Some(masked - 0x8000)
            }
        } else {
            Some(masked)
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

    #[inline]
    fn mode_bg_mask(mode: u16) -> u8 {
        match mode {
            0 => 0x0F,
            1 => 0x07,
            2 => 0x0C,
            3 | 4 | 5 => 0x04,
            _ => 0,
        }
    }

    pub fn begin_scanline(&mut self, y: usize) {
        if y >= LCD_HEIGHT {
            return;
        }
        self.dot_x = 0;
        self.obj_rendered_this_line = false;
        self.obj_win_rendered = false;
        self.obj_has_semi = false;
        if self.skip_rendering {
            return;
        }

        let mode = self.dispcnt & 0x07;
        if (self.dispcnt & 0x0080) == 0 && (self.dispcnt & 0x1000) != 0 {
            if self.obj_line_dirty {
                self.obj_line.fill(ObjPixel::empty());
                self.obj_line_dirty = false;
            }
            self.obj_rendered_this_line = true;
            self.render_sprites(y, mode >= 3);
        }
    }

    pub fn render_dot_range(&mut self, y: usize, x_start: usize, x_end: usize) {
        if self.skip_rendering {
            return;
        }
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

        let mode = self.dispcnt & 0x07;
        let active_bg_mask =
            ((self.dispcnt >> 8) as u8) & Self::mode_bg_mask(mode) & self.layer_enable_mask;

        for bg in 0..4 {
            if (active_bg_mask & (1 << bg)) != 0 {
                self.bg_lines[bg][x0..x1].fill(0);
            }
        }

        match mode {
            0 => {
                for bg in 0..4 {
                    if (active_bg_mask & (1 << bg)) != 0 {
                        self.render_regular_bg(bg, y, x0, x1);
                    }
                }
            }
            1 => {
                if (active_bg_mask & 0x01) != 0 {
                    self.render_regular_bg(0, y, x0, x1);
                }
                if (active_bg_mask & 0x02) != 0 {
                    self.render_regular_bg(1, y, x0, x1);
                }
                if (active_bg_mask & 0x04) != 0 {
                    self.render_affine_bg(2, x0, x1);
                }
            }
            2 => {
                if (active_bg_mask & 0x04) != 0 {
                    self.render_affine_bg(2, x0, x1);
                }
                if (active_bg_mask & 0x08) != 0 {
                    self.render_affine_bg(3, x0, x1);
                }
            }
            3 => {
                if (active_bg_mask & 0x04) != 0 {
                    self.render_mode3_bg(x0, x1);
                }
            }
            4 => {
                if (active_bg_mask & 0x04) != 0 {
                    self.render_mode4_bg(x0, x1);
                }
            }
            5 if (active_bg_mask & 0x04) != 0 => {
                self.render_mode5_bg(x0, x1);
            }
            _ => {}
        }

        self.compose_scanline(y, row_start, x0, x1, active_bg_mask);
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
        if self.skip_rendering {
            if y == 159 {
                self.skipped_frames = self.skipped_frames.saturating_add(1);
            }
            self.end_scanline();
            return;
        }

        // Mode 4 Fast Path:
        // Active only when ALL of the following conditions hold:
        // 1. Mode 4 (bitmap 240x160 8bpp)
        // 2. BG2 enabled in DISPCNT and layer_enable_mask
        // 3. Forced blank is OFF
        // 4. Sprites and windows disabled: (dispcnt & 0xF000) == 0
        // 5. Special color effects disabled: (bldcnt & 0x00C0) == 0
        // 6. Mosaic disabled for BG2: (bgcnt[2] & 0x0040) == 0
        // 7. BG2 affine transform is identity: PA=0x0100, PB=0, PC=0, PD=0x0100, RefX=0, RefY/256 == y
        let mode = self.dispcnt & 0x07;
        let mode4_fast_path = mode == 4
            && (self.dispcnt & 0x0400) != 0
            && (self.layer_enable_mask & 0x04) != 0
            && (self.dispcnt & 0x0080) == 0
            && (self.dispcnt & 0xF000) == 0
            && (self.bldcnt & 0x00C0) == 0
            && (self.bgcnt[2] & 0x0040) == 0
            && self.bgp_a[0] == 0x0100
            && self.bgp_b[0] == 0
            && self.bgp_c[0] == 0
            && self.bgp_d[0] == 0x0100
            && self.ref_x[0] == 0
            && (self.ref_y[0] >> 8) == (y as i32);

        if mode4_fast_path {
            let row_start = y * LCD_WIDTH;
            let page_base = if (self.dispcnt & 0x0010) != 0 {
                0xA000
            } else {
                0
            };
            let backdrop = self.read_pram_u16(0);
            let vram_row = &self.vram[page_base + row_start..page_base + row_start + LCD_WIDTH];
            for x in 0..LCD_WIDTH {
                let col = vram_row[x] as usize;
                self.framebuffer[row_start + x] = if col != 0 {
                    self.read_pram_u16(col)
                } else {
                    backdrop
                };
            }
            if (self.greenswap & 1) != 0 {
                self.apply_greenswap(row_start, 0, LCD_WIDTH);
            }
            self.end_scanline();
            return;
        }

        self.begin_scanline(y);
        self.render_dot_range(y, 0, LCD_WIDTH);
        self.end_scanline();
    }

    #[inline(always)]
    fn bg_vram_off(byte_off: usize) -> usize {
        let masked = byte_off & 0x1FFFF;
        if masked >= 0x18000 {
            masked - 0x8000
        } else {
            masked
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_regular_bg_span<const BPP8: bool>(
        &mut self,
        bg: usize,
        x_start: usize,
        x_end: usize,
        char_base: usize,
        screen_base: usize,
        width_mask: usize,
        scroll_x: usize,
        fine_y: usize,
        y_entry_offset: usize,
    ) {
        let mut x = x_start;
        while x < x_end {
            let bg_x = (x + scroll_x) & width_mask;
            let tile_x = bg_x >> 3;
            let fine_x = bg_x & 7;
            let run = (8 - fine_x).min(x_end - x);

            let block_x_offset = if tile_x >= 32 { 1024 } else { 0 };
            let entry_idx = y_entry_offset + block_x_offset + (tile_x & 31);
            let entry_off = Self::bg_vram_off(screen_base + entry_idx * 2) & !1;
            let entry = u16::from_le_bytes([self.vram[entry_off], self.vram[entry_off + 1]]);
            let tile_num = (entry & 0x03FF) as usize;
            let h_flip = (entry & 0x0400) != 0;
            let v_flip = (entry & 0x0800) != 0;
            let ty = if v_flip { 7 - fine_y } else { fine_y };

            let (mut bits, pal_base, shift, mask): (u64, usize, u32, u64) = if BPP8 {
                let row_off = Self::bg_vram_off(char_base + tile_num * 64 + ty * 8);
                let mut raw = u64::from_le_bytes([
                    self.vram[row_off],
                    self.vram[row_off + 1],
                    self.vram[row_off + 2],
                    self.vram[row_off + 3],
                    self.vram[row_off + 4],
                    self.vram[row_off + 5],
                    self.vram[row_off + 6],
                    self.vram[row_off + 7],
                ]);
                if h_flip {
                    raw = raw.swap_bytes();
                }
                (raw >> (fine_x * 8), 0, 8, 0xFF)
            } else {
                let pal_bank = ((entry >> 12) & 0x0F) as usize;
                let row_off = Self::bg_vram_off(char_base + tile_num * 32 + ty * 4);
                let mut raw = u32::from_le_bytes([
                    self.vram[row_off],
                    self.vram[row_off + 1],
                    self.vram[row_off + 2],
                    self.vram[row_off + 3],
                ]);
                if h_flip {
                    let swapped = raw.swap_bytes();
                    raw = ((swapped & 0x0F0F_0F0F) << 4) | ((swapped >> 4) & 0x0F0F_0F0F);
                }
                ((raw as u64) >> (fine_x * 4), pal_bank * 16, 4, 0x0F)
            };

            if bits != 0 {
                let out_col = &mut self.bg_lines[bg][x..x + run];
                let pram = &self.pram;
                for i in 0..run {
                    let col_idx = (bits & mask) as usize;
                    bits >>= shift;
                    if col_idx != 0 {
                        let off = (pal_base + col_idx) * 2;
                        out_col[i] = u16::from_le_bytes([pram[off], pram[off + 1]]) | 0x8000;
                    }
                }
            }

            x += run;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_regular_bg_mosaic<const BPP8: bool>(
        &mut self,
        bg: usize,
        x_start: usize,
        x_end: usize,
        char_base: usize,
        screen_base: usize,
        width_mask: usize,
        mos_h: usize,
        scroll_x: usize,
        fine_y: usize,
        y_entry_offset: usize,
    ) {
        let mut cached_entry_idx = usize::MAX;
        let mut tile_num = 0;
        let mut h_flip = false;
        let mut v_flip = false;
        let mut pal_base = 0;

        for x in x_start..x_end {
            let eff_x = x - (x % mos_h);
            let bg_x = (eff_x + scroll_x) & width_mask;
            let tile_x = bg_x >> 3;
            let fine_x = bg_x & 7;

            let block_x_offset = if tile_x >= 32 { 1024 } else { 0 };
            let entry_idx = y_entry_offset + block_x_offset + (tile_x & 31);

            if entry_idx != cached_entry_idx {
                cached_entry_idx = entry_idx;
                let entry_off = Self::bg_vram_off(screen_base + entry_idx * 2) & !1;
                let entry = u16::from_le_bytes([self.vram[entry_off], self.vram[entry_off + 1]]);
                tile_num = (entry & 0x03FF) as usize;
                h_flip = (entry & 0x0400) != 0;
                v_flip = (entry & 0x0800) != 0;
                pal_base = (((entry >> 12) & 0x0F) as usize) * 16;
            }

            let tx = if h_flip { 7 - fine_x } else { fine_x };
            let ty = if v_flip { 7 - fine_y } else { fine_y };

            let (col_idx, pram_idx) = if BPP8 {
                let off = Self::bg_vram_off(char_base + tile_num * 64 + ty * 8 + tx);
                let idx = self.vram[off] as usize;
                (idx, idx)
            } else {
                let off = Self::bg_vram_off(char_base + tile_num * 32 + ty * 4 + (tx >> 1));
                let byte = self.vram[off];
                let idx = ((byte >> ((tx & 1) * 4)) & 0x0F) as usize;
                (idx, pal_base + idx)
            };

            if col_idx != 0 {
                self.bg_lines[bg][x] = self.read_pram_u16(pram_idx) | 0x8000;
            }
        }
    }

    fn render_regular_bg(&mut self, bg: usize, y: usize, x_start: usize, x_end: usize) {
        let cnt = self.bgcnt[bg];
        let char_base = (((cnt >> 2) & 0x03) as usize) * 0x4000;
        let mosaic_en = (cnt & 0x0040) != 0;
        let is_8bpp = (cnt & 0x0080) != 0;
        let screen_base = (((cnt >> 8) & 0x1F) as usize) * 0x0800;
        let size_mode = (cnt >> 14) & 0x03;

        let width_mask: usize = if (size_mode & 1) != 0 { 511 } else { 255 };
        let height_mask: usize = if (size_mode & 2) != 0 { 511 } else { 255 };

        let mos_h = if mosaic_en {
            ((self.mosaic & 0x0F) as usize) + 1
        } else {
            1
        };
        let mos_v = if mosaic_en {
            (((self.mosaic >> 4) & 0x0F) as usize) + 1
        } else {
            1
        };
        let eff_y = if mos_v > 1 { y - (y % mos_v) } else { y };

        let scroll_x = (self.bghofs[bg] & 0x1FF) as usize;
        let scroll_y = (self.bgvofs[bg] & 0x1FF) as usize;

        let bg_y = (eff_y + scroll_y) & height_mask;
        let tile_y = bg_y >> 3;
        let fine_y = bg_y & 7;

        let block_y_offset = if tile_y >= 32 {
            if (size_mode & 1) != 0 { 2048 } else { 1024 }
        } else {
            0
        };
        let y_entry_offset = block_y_offset + (tile_y & 31) * 32;

        match (mos_h > 1, is_8bpp) {
            (false, false) => self.render_regular_bg_span::<false>(
                bg, x_start, x_end, char_base, screen_base, width_mask, scroll_x, fine_y,
                y_entry_offset,
            ),
            (false, true) => self.render_regular_bg_span::<true>(
                bg, x_start, x_end, char_base, screen_base, width_mask, scroll_x, fine_y,
                y_entry_offset,
            ),
            (true, false) => self.render_regular_bg_mosaic::<false>(
                bg, x_start, x_end, char_base, screen_base, width_mask, mos_h, scroll_x, fine_y,
                y_entry_offset,
            ),
            (true, true) => self.render_regular_bg_mosaic::<true>(
                bg, x_start, x_end, char_base, screen_base, width_mask, mos_h, scroll_x, fine_y,
                y_entry_offset,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_affine_bg_inner<const WRAP: bool>(
        &mut self,
        bg: usize,
        x_start: usize,
        x_end: usize,
        char_base: usize,
        screen_base: usize,
        map_px: i32,
        tiles_per_row: usize,
        pa: i32,
        pc: i32,
        mut cx: i32,
        mut cy: i32,
    ) {
        let map_mask = map_px - 1;
        let map_u32 = map_px as u32;
        for x in x_start..x_end {
            let mut px = cx >> 8;
            let mut py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if WRAP {
                px &= map_mask;
                py &= map_mask;
            } else if (px as u32) >= map_u32 || (py as u32) >= map_u32 {
                continue;
            }

            let tile_x = (px >> 3) as usize;
            let tile_y = (py >> 3) as usize;
            let fine_x = (px & 7) as usize;
            let fine_y = (py & 7) as usize;

            let map_off = Self::bg_vram_off(screen_base + tile_y * tiles_per_row + tile_x);
            let tile_num = self.vram[map_off] as usize;
            let pixel_off = Self::bg_vram_off(char_base + tile_num * 64 + fine_y * 8 + fine_x);
            let col_idx = self.vram[pixel_off] as usize;

            if col_idx != 0 {
                self.bg_lines[bg][x] = self.read_pram_u16(col_idx) | 0x8000;
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
        let tiles_per_row = (map_px >> 3) as usize;

        let pa = self.bgp_a[aff_idx] as i32;
        let pc = self.bgp_c[aff_idx] as i32;
        let cx = self.ref_x[aff_idx].wrapping_add(pa.wrapping_mul(x_start as i32));
        let cy = self.ref_y[aff_idx].wrapping_add(pc.wrapping_mul(x_start as i32));

        if wraparound {
            self.render_affine_bg_inner::<true>(
                bg,
                x_start,
                x_end,
                char_base,
                screen_base,
                map_px,
                tiles_per_row,
                pa,
                pc,
                cx,
                cy,
            );
        } else {
            self.render_affine_bg_inner::<false>(
                bg,
                x_start,
                x_end,
                char_base,
                screen_base,
                map_px,
                tiles_per_row,
                pa,
                pc,
                cx,
                cy,
            );
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
            } else if (px as u32) >= LCD_WIDTH as u32 || (py as u32) >= LCD_HEIGHT as u32 {
                continue;
            }

            let offset = ((py as usize) * LCD_WIDTH + (px as usize)) * 2;
            self.bg_lines[2][x] = u16::from_le_bytes([self.vram[offset], self.vram[offset + 1]]) | 0x8000;
        }
    }

    fn render_mode4_bg(&mut self, x_start: usize, x_end: usize) {
        let page_base = if (self.dispcnt & 0x0010) != 0 {
            0xA000
        } else {
            0
        };
        let pa = self.bgp_a[0] as i32;
        let pc = self.bgp_c[0] as i32;
        let mut cx = self.ref_x[0].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[0].wrapping_add(pc.wrapping_mul(x_start as i32));

        for x in x_start..x_end {
            let px = cx >> 8;
            let py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if (px as u32) >= LCD_WIDTH as u32 || (py as u32) >= LCD_HEIGHT as u32 {
                continue;
            }

            let offset = page_base + (py as usize) * LCD_WIDTH + (px as usize);
            let col_idx = self.vram[offset] as usize;
            if col_idx != 0 {
                self.bg_lines[2][x] = self.read_pram_u16(col_idx) | 0x8000;
            }
        }
    }

    fn render_mode5_bg(&mut self, x_start: usize, x_end: usize) {
        let page_base = if (self.dispcnt & 0x0010) != 0 {
            0xA000
        } else {
            0
        };
        let pa = self.bgp_a[0] as i32;
        let pc = self.bgp_c[0] as i32;
        let mut cx = self.ref_x[0].wrapping_add(pa.wrapping_mul(x_start as i32));
        let mut cy = self.ref_y[0].wrapping_add(pc.wrapping_mul(x_start as i32));

        for x in x_start..x_end {
            let px = cx >> 8;
            let py = cy >> 8;
            cx = cx.wrapping_add(pa);
            cy = cy.wrapping_add(pc);

            if (px as u32) >= 160 || (py as u32) >= 128 {
                continue;
            }

            let offset = page_base + ((py as usize) * 160 + (px as usize)) * 2;
            self.bg_lines[2][x] = u16::from_le_bytes([self.vram[offset], self.vram[offset + 1]]) | 0x8000;
        }
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn plot_sprite_pixel<const OBJ_WIN: bool>(
        &mut self,
        sx: usize,
        col_idx: usize,
        pal_offset: usize,
        prio: u8,
        semi_transparent: bool,
        mosaic_en: bool,
        obj_win_cleared: &mut bool,
    ) {
        if col_idx == 0 {
            if !OBJ_WIN {
                let cur = self.obj_line[sx];
                if !cur.is_present() && prio < cur.prio() {
                    self.obj_line[sx] = cur.with_transparent_prio(prio, mosaic_en);
                    self.obj_line_dirty = true;
                }
            }
            return;
        }

        if OBJ_WIN {
            if !*obj_win_cleared {
                self.obj_win.fill(false);
                *obj_win_cleared = true;
            }
            self.obj_win_dirty = true;
            self.obj_win_rendered = true;
            self.obj_win[sx] = true;
            return;
        }

        let cur = self.obj_line[sx];
        if !cur.is_present() || prio < cur.prio() {
            self.obj_line[sx] = ObjPixel::new(
                self.read_pram_u16(pal_offset + col_idx),
                prio,
                semi_transparent,
                mosaic_en,
            );
            self.obj_line_dirty = true;
            if semi_transparent {
                self.obj_has_semi = true;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_non_affine_sprite_span<const BPP8: bool, const OBJ_WIN: bool>(
        &mut self,
        spr_x: i32,
        spr_w: i32,
        x_min: i32,
        x_max: i32,
        ty: usize,
        h_flip: bool,
        mapping_1d: bool,
        bitmap_mode: bool,
        tile_base: usize,
        pal_offset: usize,
        prio: u8,
        semi_transparent: bool,
        mosaic_en: bool,
        obj_win_cleared: &mut bool,
    ) {
        let step = if BPP8 { 2 } else { 1 };
        let tile_row = ty >> 3;
        let fine_y = ty & 7;
        let row_bytes = if BPP8 { 8 } else { 4 };
        let row_part = if mapping_1d {
            tile_base + tile_row * ((spr_w as usize) >> 3) * step
        } else {
            ((tile_base & 0x3E0) + tile_row * 32) & 0x3FF
        };
        let col_base = tile_base & 0x01F;

        let mut tx = if h_flip { spr_w - 1 - x_min } else { x_min };
        let tx_step = if h_flip { -1i32 } else { 1i32 };
        let mut cached_col = usize::MAX;
        let mut row_off = 0usize;
        let mut tile_valid = false;

        for local_x in x_min..x_max {
            let sx = (spr_x + local_x) as usize;
            let utx = tx as usize;
            let tile_col = utx >> 3;
            if tile_col != cached_col {
                cached_col = tile_col;
                let tile_num = if mapping_1d {
                    (row_part + tile_col * step) & 0x03FF
                } else {
                    row_part | ((col_base + tile_col * step) & 0x01F)
                };
                tile_valid = !bitmap_mode || tile_num >= 512;
                row_off = 0x10000 + tile_num * 32 + fine_y * row_bytes;
            }
            if tile_valid {
                let fine_x = utx & 7;
                let col_idx = if BPP8 {
                    self.vram[row_off + fine_x] as usize
                } else {
                    let byte = self.vram[row_off + (fine_x >> 1)];
                    ((byte >> ((fine_x & 1) * 4)) & 0x0F) as usize
                };
                self.plot_sprite_pixel::<OBJ_WIN>(
                    sx,
                    col_idx,
                    pal_offset,
                    prio,
                    semi_transparent,
                    mosaic_en,
                    obj_win_cleared,
                );
            }
            tx += tx_step;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_affine_sprite_span<const BPP8: bool, const OBJ_WIN: bool>(
        &mut self,
        spr_x: i32,
        spr_w: i32,
        spr_h: i32,
        box_w: i32,
        box_h: i32,
        x_min: i32,
        x_max: i32,
        eff_y: i32,
        pa: i32,
        pb: i32,
        pc: i32,
        pd: i32,
        mapping_1d: bool,
        bitmap_mode: bool,
        tile_base: usize,
        pal_offset: usize,
        prio: u8,
        semi_transparent: bool,
        mosaic_en: bool,
        obj_win_cleared: &mut bool,
    ) {
        let step = if BPP8 { 2 } else { 1 };
        let row_stride = (spr_w as usize) >> 3;
        let row_bytes = if BPP8 { 8 } else { 4 };
        let col_base = tile_base & 0x01F;
        let row_base_2d = tile_base & 0x3E0;

        let dy = eff_y - (box_h >> 1);
        let dx0 = x_min - (box_w >> 1);
        let mut fx = pa * dx0 + pb * dy + ((spr_w >> 1) << 8);
        let mut fy = pc * dx0 + pd * dy + ((spr_h >> 1) << 8);

        for local_x in x_min..x_max {
            let tx = fx >> 8;
            let ty = fy >> 8;
            fx += pa;
            fy += pc;
            if tx < 0 || ty < 0 || tx >= spr_w || ty >= spr_h {
                continue;
            }
            let utx = tx as usize;
            let uty = ty as usize;
            let tile_col = utx >> 3;
            let tile_row = uty >> 3;
            let fine_x = utx & 7;
            let fine_y = uty & 7;

            let tile_num = if mapping_1d {
                (tile_base + (tile_row * row_stride + tile_col) * step) & 0x03FF
            } else {
                let row_part = (row_base_2d + tile_row * 32) & 0x3FF;
                let col_part = (col_base + tile_col * step) & 0x01F;
                row_part | col_part
            };

            if bitmap_mode && tile_num < 512 {
                continue;
            }

            let row_off = 0x10000 + tile_num * 32 + fine_y * row_bytes;
            let col_idx = if BPP8 {
                self.vram[row_off + fine_x] as usize
            } else {
                let byte = self.vram[row_off + (fine_x >> 1)];
                ((byte >> ((fine_x & 1) * 4)) & 0x0F) as usize
            };

            self.plot_sprite_pixel::<OBJ_WIN>(
                (spr_x + local_x) as usize,
                col_idx,
                pal_offset,
                prio,
                semi_transparent,
                mosaic_en,
                obj_win_cleared,
            );
        }
    }

    fn render_sprites(&mut self, y: usize, bitmap_mode: bool) {
        const SPR_DIMS: [(i32, i32); 16] = [
            (8, 8),
            (16, 16),
            (32, 32),
            (64, 64),
            (16, 8),
            (32, 8),
            (32, 16),
            (64, 32),
            (8, 16),
            (8, 32),
            (16, 32),
            (32, 64),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
        ];

        let mapping_1d = (self.dispcnt & 0x0040) != 0;
        let mut cycle_budget: i32 = if (self.dispcnt & 0x0020) != 0 {
            954
        } else {
            1210
        };
        let mos_v = (((self.mosaic >> 12) & 0x0F) as usize) + 1;
        let mos_h = (((self.mosaic >> 8) & 0x0F) as usize) + 1;
        let mut obj_win_cleared = !self.obj_win_dirty;

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
            let shape_size = ((((attr0 >> 14) & 0x03) << 2) | ((attr1 >> 14) & 0x03)) as usize;
            let (spr_w, spr_h) = SPR_DIMS[shape_size];
            if spr_w == 0 {
                continue;
            }

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

            let x_min = (-spr_x).max(0);
            let x_max = (LCD_WIDTH as i32 - spr_x).min(box_w);
            if x_min >= x_max {
                continue;
            }

            let attr2 = self.read_oam_u16(idx * 8 + 4);
            let mosaic_en = (attr0 & 0x1000) != 0;
            let is_8bpp = (attr0 & 0x2000) != 0;
            let mut tile_base = (attr2 & 0x03FF) as usize;
            if is_8bpp {
                tile_base &= !1;
            }
            let prio = ((attr2 >> 10) & 0x03) as u8;
            let pal_bank = ((attr2 >> 12) & 0x0F) as usize;
            let pal_offset = if is_8bpp { 256 } else { 256 + pal_bank * 16 };
            let semi_transparent = obj_mode == 1;
            let is_obj_win = obj_mode == 2;

            let eff_y = if mosaic_en {
                let my = y - (y % mos_v);
                (my as i32).max(spr_y) - spr_y
            } else {
                (y as i32) - spr_y
            };

            if is_affine {
                let aff_idx = ((attr1 >> 9) & 0x1F) as usize;
                let pa = self.read_oam_u16(aff_idx * 32 + 0x06) as i16 as i32;
                let pb = self.read_oam_u16(aff_idx * 32 + 0x0E) as i16 as i32;
                let pc = self.read_oam_u16(aff_idx * 32 + 0x16) as i16 as i32;
                let pd = self.read_oam_u16(aff_idx * 32 + 0x1E) as i16 as i32;
                match (is_8bpp, is_obj_win) {
                    (false, false) => self.draw_affine_sprite_span::<false, false>(
                        spr_x, spr_w, spr_h, box_w, box_h, x_min, x_max, eff_y, pa, pb, pc, pd,
                        mapping_1d, bitmap_mode, tile_base, pal_offset, prio, semi_transparent,
                        mosaic_en, &mut obj_win_cleared,
                    ),
                    (false, true) => self.draw_affine_sprite_span::<false, true>(
                        spr_x, spr_w, spr_h, box_w, box_h, x_min, x_max, eff_y, pa, pb, pc, pd,
                        mapping_1d, bitmap_mode, tile_base, pal_offset, prio, semi_transparent,
                        mosaic_en, &mut obj_win_cleared,
                    ),
                    (true, false) => self.draw_affine_sprite_span::<true, false>(
                        spr_x, spr_w, spr_h, box_w, box_h, x_min, x_max, eff_y, pa, pb, pc, pd,
                        mapping_1d, bitmap_mode, tile_base, pal_offset, prio, semi_transparent,
                        mosaic_en, &mut obj_win_cleared,
                    ),
                    (true, true) => self.draw_affine_sprite_span::<true, true>(
                        spr_x, spr_w, spr_h, box_w, box_h, x_min, x_max, eff_y, pa, pb, pc, pd,
                        mapping_1d, bitmap_mode, tile_base, pal_offset, prio, semi_transparent,
                        mosaic_en, &mut obj_win_cleared,
                    ),
                }
            } else {
                let h_flip = (attr1 & 0x1000) != 0;
                let v_flip = (attr1 & 0x2000) != 0;
                let ty = if v_flip { spr_h - 1 - eff_y } else { eff_y } as usize;
                match (is_8bpp, is_obj_win) {
                    (false, false) => self.draw_non_affine_sprite_span::<false, false>(
                        spr_x, spr_w, x_min, x_max, ty, h_flip, mapping_1d, bitmap_mode, tile_base,
                        pal_offset, prio, semi_transparent, mosaic_en, &mut obj_win_cleared,
                    ),
                    (false, true) => self.draw_non_affine_sprite_span::<false, true>(
                        spr_x, spr_w, x_min, x_max, ty, h_flip, mapping_1d, bitmap_mode, tile_base,
                        pal_offset, prio, semi_transparent, mosaic_en, &mut obj_win_cleared,
                    ),
                    (true, false) => self.draw_non_affine_sprite_span::<true, false>(
                        spr_x, spr_w, x_min, x_max, ty, h_flip, mapping_1d, bitmap_mode, tile_base,
                        pal_offset, prio, semi_transparent, mosaic_en, &mut obj_win_cleared,
                    ),
                    (true, true) => self.draw_non_affine_sprite_span::<true, true>(
                        spr_x, spr_w, x_min, x_max, ty, h_flip, mapping_1d, bitmap_mode, tile_base,
                        pal_offset, prio, semi_transparent, mosaic_en, &mut obj_win_cleared,
                    ),
                }
            }
        }

        if mos_h > 1 {
            let mut mosaic_px = ObjPixel::empty();
            for x in 0..LCD_WIDTH {
                let cur = self.obj_line[x];
                let higher_prio = cur.prio() < mosaic_px.prio();
                if (x % mos_h) == 0 || !cur.mosaic() || !mosaic_px.mosaic() || higher_prio {
                    mosaic_px = cur;
                } else {
                    self.obj_line[x] = mosaic_px;
                }
            }
        }
    }

    #[inline]
    fn win_contains_bounds(b1: usize, b2: usize, val: usize) -> bool {
        if b1 < b2 {
            val >= b1 && val < b2
        } else {
            val >= b1 || val < b2
        }
    }

    #[inline]
    fn win_contains(bounds: u16, val: usize) -> bool {
        let b1 = ((bounds >> 8) & 0xFF) as usize;
        let b2 = (bounds & 0xFF) as usize;
        b1 != b2 && Self::win_contains_bounds(b1, b2, val)
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_span<const WIN: bool, const OBJ: bool, const BLD: bool>(
        &mut self,
        row_start: usize,
        x_start: usize,
        x_end: usize,
        backdrop: u16,
        active_bgs: [usize; 4],
        bg_prios: [u8; 4],
        win0_active_line: bool,
        win0_x1: usize,
        win0_x2: usize,
        win1_active_line: bool,
        win1_x1: usize,
        win1_x2: usize,
        objwin_en: bool,
        win0_mask: u8,
        win1_mask: u8,
        objwin_mask: u8,
        winout_mask: u8,
        bld_mode: u16,
        eva: u16,
        evb: u16,
        evy: u16,
    ) {
        let x0 = x_start.min(LCD_WIDTH);
        let x1 = x_end.min(LCD_WIDTH);
        for x in x0..x1 {
            let win_mask: u8 = if !WIN {
                0x3F
            } else if win0_active_line && Self::win_contains_bounds(win0_x1, win0_x2, x) {
                win0_mask
            } else if win1_active_line && Self::win_contains_bounds(win1_x1, win1_x2, x) {
                win1_mask
            } else if objwin_en && self.obj_win[x] {
                objwin_mask
            } else {
                winout_mask
            };

            let mut top_layer = LAYER_BD;
            let mut top_col = backdrop;
            let mut top_semi = false;

            let mut bot_layer = LAYER_BD;
            let mut bot_col = backdrop;

            let obj = if OBJ {
                self.obj_line[x]
            } else {
                ObjPixel::empty()
            };
            let obj_visible = OBJ && obj.is_present() && (!WIN || (win_mask & (1 << LAYER_OBJ)) != 0);

            let mut obj_placed = false;

            for &raw_bg in &active_bgs {
                if raw_bg >= 4 {
                    break;
                }
                let bg = raw_bg & 3;

                if OBJ && obj_visible && !obj_placed && obj.prio() <= bg_prios[bg] {
                    obj_placed = true;
                    if top_layer == LAYER_BD {
                        top_layer = LAYER_OBJ;
                        top_col = obj.color();
                        top_semi = obj.semi_transparent();
                        if !BLD || (bld_mode != 1 && !top_semi) {
                            break;
                        }
                    } else {
                        bot_layer = LAYER_OBJ;
                        bot_col = obj.color();
                        break;
                    }
                }

                let bg_px = self.bg_lines[bg][x];
                if bg_px != 0 && (!WIN || (win_mask & (1 << bg)) != 0) {
                    if top_layer == LAYER_BD {
                        top_layer = bg;
                        top_col = bg_px;
                        if !BLD || (bld_mode != 1 && !top_semi) {
                            break;
                        }
                    } else {
                        bot_layer = bg;
                        bot_col = bg_px;
                        break;
                    }
                }
            }

            if OBJ
                && obj_visible
                && !obj_placed
                && (top_layer == LAYER_BD || (BLD && (bld_mode == 1 || top_semi) && bot_layer == LAYER_BD))
            {
                if top_layer == LAYER_BD {
                    top_layer = LAYER_OBJ;
                    top_col = obj.color();
                    top_semi = obj.semi_transparent();
                } else {
                    bot_layer = LAYER_OBJ;
                    bot_col = obj.color();
                }
            }

            let final_col = if BLD && (!WIN || (win_mask & 0x20) != 0) {
                let is_target1 = (self.bldcnt & (1 << top_layer)) != 0;
                let is_target2 = (self.bldcnt & (0x0100 << bot_layer)) != 0;
                if OBJ && top_semi && is_target2 {
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

            self.framebuffer[row_start + x] = final_col & 0x7FFF;
        }
    }

    fn compose_scanline(
        &mut self,
        y: usize,
        row_start: usize,
        x_start: usize,
        x_end: usize,
        active_bg_mask: u8,
    ) {
        let backdrop = self.read_pram_u16(0);
        let any_win_active = (self.dispcnt & 0xE000) != 0;
        let win0_x1 = ((self.winh[0] >> 8) & 0xFF) as usize;
        let win0_x2 = (self.winh[0] & 0xFF) as usize;
        let win1_x1 = ((self.winh[1] >> 8) & 0xFF) as usize;
        let win1_x2 = (self.winh[1] & 0xFF) as usize;
        let win0_active_line =
            (self.dispcnt & 0x2000) != 0 && win0_x1 != win0_x2 && Self::win_contains(self.winv[0], y);
        let win1_active_line =
            (self.dispcnt & 0x4000) != 0 && win1_x1 != win1_x2 && Self::win_contains(self.winv[1], y);
        let objwin_en = (self.dispcnt & 0x8000) != 0 && self.obj_win_rendered;
        let win_per_pixel = win0_active_line || win1_active_line || objwin_en;

        let win0_mask = (self.winin & 0x3F) as u8;
        let win1_mask = ((self.winin >> 8) & 0x3F) as u8;
        let objwin_mask = ((self.winout >> 8) & 0x3F) as u8;
        let winout_mask = (self.winout & 0x3F) as u8;
        let constant_win_mask = if !any_win_active { 0x3F } else { winout_mask };

        let bld_mode = (self.bldcnt >> 6) & 0x03;
        let eva = (self.bldalpha & 0x1F).min(16);
        let evb = ((self.bldalpha >> 8) & 0x1F).min(16);
        let evy = (self.bldy & 0x1F).min(16);

        let static_layer_mask = if !win_per_pixel {
            active_bg_mask & constant_win_mask
        } else {
            active_bg_mask
        };

        let bg_prios = [
            (self.bgcnt[0] & 3) as u8,
            (self.bgcnt[1] & 3) as u8,
            (self.bgcnt[2] & 3) as u8,
            (self.bgcnt[3] & 3) as u8,
        ];

        let mut active_bgs = [4usize; 4];
        let mut num_active_bgs = 0usize;
        for bg in 0..4 {
            if (static_layer_mask & (1 << bg)) != 0 {
                active_bgs[num_active_bgs] = bg;
                let mut j = num_active_bgs;
                while j > 0 && bg_prios[active_bgs[j]] < bg_prios[active_bgs[j - 1]] {
                    active_bgs.swap(j, j - 1);
                    j -= 1;
                }
                num_active_bgs += 1;
            }
        }

        let obj_layer_en = (self.dispcnt & 0x1000) != 0
            && (self.layer_enable_mask & 0x10) != 0
            && self.obj_rendered_this_line
            && self.obj_line_dirty
            && (win_per_pixel || (constant_win_mask & (1 << LAYER_OBJ)) != 0);

        let need_bld = (win_per_pixel || (constant_win_mask & 0x20) != 0)
            && (bld_mode != 0 || (obj_layer_en && self.obj_has_semi));

        if num_active_bgs == 0 && !obj_layer_en {
            let sfx_ok = win_per_pixel || (constant_win_mask & 0x20) != 0;
            let bd_col = if sfx_ok && (self.bldcnt & (1 << LAYER_BD)) != 0 {
                match bld_mode {
                    2 => Self::brighten(backdrop, evy),
                    3 => Self::darken(backdrop, evy),
                    _ => backdrop,
                }
            } else {
                backdrop
            };
            if !win_per_pixel || (bld_mode != 2 && bld_mode != 3) {
                self.framebuffer[row_start + x_start..row_start + x_end].fill(bd_col);
                if (self.greenswap & 1) != 0 {
                    self.apply_greenswap(row_start, x_start, x_end);
                }
                return;
            }
        }

        match (win_per_pixel, obj_layer_en, need_bld) {
            (false, false, false) => self.compose_span::<false, false, false>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (false, false, true) => self.compose_span::<false, false, true>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (false, true, false) => self.compose_span::<false, true, false>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (false, true, true) => self.compose_span::<false, true, true>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (true, false, false) => self.compose_span::<true, false, false>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (true, false, true) => self.compose_span::<true, false, true>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (true, true, false) => self.compose_span::<true, true, false>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
            (true, true, true) => self.compose_span::<true, true, true>(
                row_start, x_start, x_end, backdrop, active_bgs, bg_prios, win0_active_line,
                win0_x1, win0_x2, win1_active_line, win1_x1, win1_x2, objwin_en, win0_mask,
                win1_mask, objwin_mask, winout_mask, bld_mode, eva, evb, evy,
            ),
        }

        if (self.greenswap & 1) != 0 {
            self.apply_greenswap(row_start, x_start, x_end);
        }
    }

    #[inline]
    fn apply_greenswap(&mut self, row_start: usize, x_start: usize, x_end: usize) {
        let mut x = x_start | 1;
        let end = x_end.min(LCD_WIDTH);
        while x < end {
            let idx0 = row_start + x - 1;
            let idx1 = row_start + x;
            let c0 = self.framebuffer[idx0];
            let c1 = self.framebuffer[idx1];
            let g0 = (c0 >> 5) & 0x1F;
            let g1 = (c1 >> 5) & 0x1F;
            self.framebuffer[idx0] = (c0 & !0x03E0) | (g1 << 5);
            self.framebuffer[idx1] = (c1 & !0x03E0) | (g0 << 5);
            x += 2;
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
