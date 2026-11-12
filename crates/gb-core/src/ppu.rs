use alloc::boxed::Box;
use serde::{Deserialize, Serialize};

use crate::serde_helpers::{alloc_boxed_array, box_bytes, box_u16s, fixed_bytes};
use crate::{LCD_HEIGHT, LCD_WIDTH};

/// Configurable 4-shade BGR555 palette used when rendering DMG games.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmgPalette {
    /// Standard 4-shade greyscale (`0x7FFF, 0x56B5, 0x294A, 0x0000`).
    #[default]
    Greyscale,
    /// GBA/CGB-style colorized DMG palette (warm highlights, green midtones, dark ink).
    GbaColorized,
    /// Classic DMG-01 pea-green LCD palette.
    ClassicGreen,
    /// Custom 4-shade BGR555 palette (`[shade0, shade1, shade2, shade3]`).
    Custom([u16; 4]),
}

impl DmgPalette {
    #[inline]
    pub const fn colors(self) -> [u16; 4] {
        match self {
            DmgPalette::Greyscale => [0x7FFF, 0x56B5, 0x294A, 0x0000],
            DmgPalette::GbaColorized => [0x7FFF, 0x1BEF, 0x0240, 0x0000],
            DmgPalette::ClassicGreen => [0x4753, 0x2EA6, 0x15A4, 0x04A1],
            DmgPalette::Custom(cols) => cols,
        }
    }
}

#[derive(Copy, Clone, Default)]
pub struct PpuStepResult {
    pub vblank_irq: bool,
    pub stat_irq: bool,
    pub entered_hblank: bool,
}

#[derive(Copy, Clone)]
struct OamEntry {
    y: i16,
    x: i16,
    tile: u8,
    flags: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Ppu {
    #[serde(with = "box_bytes")]
    pub vram: Box<[u8; 0x4000]>,
    #[serde(with = "box_bytes")]
    pub oam: Box<[u8; 0xA0]>,
    #[serde(with = "fixed_bytes")]
    pub bg_cram: [u8; 64],
    #[serde(with = "fixed_bytes")]
    pub obj_cram: [u8; 64],

    #[serde(with = "box_u16s")]
    pub framebuffer: Box<[u16; LCD_WIDTH * LCD_HEIGHT]>,
    pub frame_ready: bool,

    pub vbk: u8,
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
    pub bcps: u8,
    pub ocps: u8,

    pub dot: u16,
    pub mode: u8,
    pub win_line: u8,
    pub wy_triggered: bool,
    pub stat_irq_line: bool,
    pub ly_eq_lyc: bool,

    pub dmg_palette: DmgPalette,
}

impl Ppu {
    pub fn new(dmg_palette: DmgPalette) -> Self {
        Self {
            vram: alloc_boxed_array(0u8),
            oam: alloc_boxed_array(0u8),
            bg_cram: [0xFF; 64],
            obj_cram: [0xFF; 64],
            framebuffer: alloc_boxed_array(0x7FFF),
            frame_ready: false,
            vbk: 0,
            lcdc: 0,
            stat: 0,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            bcps: 0,
            ocps: 0,
            dot: 0,
            mode: 0,
            win_line: 0,
            wy_triggered: false,
            stat_irq_line: false,
            ly_eq_lyc: false,
            dmg_palette,
        }
    }

    pub fn init_post_boot(&mut self, is_cgb: bool) {
        self.lcdc = 0x91;
        self.stat = 0x00;
        self.scy = 0x00;
        self.scx = 0x00;
        self.ly = 0x00;
        self.lyc = 0x00;
        self.bgp = 0xFC;
        self.obp0 = 0x00;
        self.obp1 = 0x00;
        self.wy = 0x00;
        self.wx = 0x00;
        self.vbk = 0;
        self.bcps = 0;
        self.ocps = 0;
        self.dot = 0;
        self.mode = 2;
        self.win_line = 0;
        self.wy_triggered = false;
        self.ly_eq_lyc = true;
        self.stat_irq_line = false;
        if is_cgb {
            self.bg_cram = [0xFF; 64];
            self.obj_cram = [0x00; 64];
        }
    }

    #[inline]
    pub fn lcd_enabled(&self) -> bool {
        (self.lcdc & 0x80) != 0
    }

    fn update_stat_irq(&mut self) -> bool {
        if !self.lcd_enabled() {
            self.stat_irq_line = false;
            return false;
        }
        self.ly_eq_lyc = self.ly == self.lyc;
        let new_line = ((self.stat & 0x08) != 0 && self.mode == 0)
            || ((self.stat & 0x10) != 0 && self.mode == 1)
            || ((self.stat & 0x20) != 0 && self.mode == 2)
            || ((self.stat & 0x40) != 0 && self.ly_eq_lyc);
        let rising = !self.stat_irq_line && new_line;
        self.stat_irq_line = new_line;
        rising
    }

    pub fn write_lcdc(&mut self, val: u8) -> bool {
        let was_on = self.lcd_enabled();
        let now_on = (val & 0x80) != 0;
        self.lcdc = val;

        if was_on && !now_on {
            // LCD turned off: reset scanline/dot/mode and clear screen to shade 0 / white.
            self.ly = 0;
            self.dot = 0;
            self.mode = 0;
            self.win_line = 0;
            self.wy_triggered = false;
            self.stat_irq_line = false;
            self.framebuffer.fill(0x7FFF);
            return false;
        }

        if !was_on && now_on {
            // LCD turned on: begin at LY=0, dot=0, Mode 2, check WY & LY=LYC immediately.
            self.ly = 0;
            self.dot = 0;
            self.mode = 2;
            self.win_line = 0;
            self.wy_triggered = (self.lcdc & 0x20) != 0 && self.ly == self.wy;
            return self.update_stat_irq();
        }

        if now_on && (self.lcdc & 0x20) != 0 && self.ly == self.wy {
            self.wy_triggered = true;
        }
        false
    }

    #[inline]
    pub fn read_stat(&self) -> u8 {
        let mode_bits = if self.lcd_enabled() { self.mode & 0x03 } else { 0 };
        let lyc_bit = if self.ly_eq_lyc { 0x04 } else { 0 };
        0x80 | (self.stat & 0x78) | lyc_bit | mode_bits
    }

    pub fn write_stat(&mut self, val: u8) -> bool {
        self.stat = val & 0x78;
        self.update_stat_irq()
    }

    pub fn write_lyc(&mut self, val: u8) -> bool {
        self.lyc = val;
        self.update_stat_irq()
    }

    pub fn read_vram(&self, addr: u16, is_cgb: bool) -> u8 {
        if self.lcd_enabled() && self.mode == 3 {
            return 0xFF;
        }
        let bank = if is_cgb { (self.vbk & 1) as usize } else { 0 };
        self.vram[bank * 0x2000 + ((addr as usize) & 0x1FFF)]
    }

    pub fn write_vram(&mut self, addr: u16, val: u8, is_cgb: bool) {
        if self.lcd_enabled() && self.mode == 3 {
            return;
        }
        let bank = if is_cgb { (self.vbk & 1) as usize } else { 0 };
        self.vram[bank * 0x2000 + ((addr as usize) & 0x1FFF)] = val;
    }

    pub fn read_oam(&self, addr: u16) -> u8 {
        if self.lcd_enabled() && (self.mode == 2 || self.mode == 3) {
            return 0xFF;
        }
        self.oam[(addr as usize) - 0xFE00]
    }

    pub fn write_oam(&mut self, addr: u16, val: u8) {
        if self.lcd_enabled() && (self.mode == 2 || self.mode == 3) {
            return;
        }
        self.oam[(addr as usize) - 0xFE00] = val;
    }

    #[inline]
    pub fn read_vbk(&self) -> u8 {
        0xFE | (self.vbk & 1)
    }

    #[inline]
    pub fn write_vbk(&mut self, val: u8) {
        self.vbk = val & 1;
    }

    #[inline]
    pub fn read_bcps(&self) -> u8 {
        self.bcps | 0x40
    }

    #[inline]
    pub fn write_bcps(&mut self, val: u8) {
        self.bcps = val & 0xBF;
    }

    pub fn read_bcpd(&self) -> u8 {
        if self.lcd_enabled() && self.mode == 3 {
            return 0xFF;
        }
        self.bg_cram[(self.bcps & 0x3F) as usize]
    }

    pub fn write_bcpd(&mut self, val: u8) {
        if !self.lcd_enabled() || self.mode != 3 {
            self.bg_cram[(self.bcps & 0x3F) as usize] = val;
        }
        if (self.bcps & 0x80) != 0 {
            let next = (self.bcps.wrapping_add(1)) & 0x3F;
            self.bcps = (self.bcps & 0x80) | next;
        }
    }

    #[inline]
    pub fn read_ocps(&self) -> u8 {
        self.ocps | 0x40
    }

    #[inline]
    pub fn write_ocps(&mut self, val: u8) {
        self.ocps = val & 0xBF;
    }

    pub fn read_ocpd(&self) -> u8 {
        if self.lcd_enabled() && self.mode == 3 {
            return 0xFF;
        }
        self.obj_cram[(self.ocps & 0x3F) as usize]
    }

    pub fn write_ocpd(&mut self, val: u8) {
        if !self.lcd_enabled() || self.mode != 3 {
            self.obj_cram[(self.ocps & 0x3F) as usize] = val;
        }
        if (self.ocps & 0x80) != 0 {
            let next = (self.ocps.wrapping_add(1)) & 0x3F;
            self.ocps = (self.ocps & 0x80) | next;
        }
    }

    /// Advances the PPU by `dots` normal-speed T-cycles (4.194304 MHz).
    pub fn step(&mut self, dots: u32, is_cgb: bool) -> PpuStepResult {
        let mut res = PpuStepResult::default();
        if !self.lcd_enabled() {
            return res;
        }

        let mut rem = dots as u16;
        while rem > 0 {
            let next_boundary = if self.ly < 144 {
                if self.dot < 80 {
                    80
                } else if self.dot < 252 {
                    252
                } else {
                    456
                }
            } else {
                456
            };

            let step = rem.min(next_boundary - self.dot);
            self.dot += step;
            rem -= step;

            if self.ly < 144 {
                if self.dot == 80 {
                    self.mode = 3;
                    if self.update_stat_irq() {
                        res.stat_irq = true;
                    }
                } else if self.dot == 252 {
                    self.render_scanline(self.ly, is_cgb);
                    self.mode = 0;
                    res.entered_hblank = true;
                    if self.update_stat_irq() {
                        res.stat_irq = true;
                    }
                } else if self.dot == 456 {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly == 144 {
                        self.mode = 1;
                        self.frame_ready = true;
                        self.win_line = 0;
                        self.wy_triggered = false;
                        res.vblank_irq = true;
                    } else {
                        self.mode = 2;
                        if (self.lcdc & 0x20) != 0 && self.ly == self.wy {
                            self.wy_triggered = true;
                        }
                    }
                    if self.update_stat_irq() {
                        res.stat_irq = true;
                    }
                }
            } else if self.dot == 456 {
                self.dot = 0;
                self.ly += 1;
                if self.ly >= 154 {
                    self.ly = 0;
                    self.mode = 2;
                    self.win_line = 0;
                    self.wy_triggered = (self.lcdc & 0x20) != 0 && self.ly == self.wy;
                }
                if self.update_stat_irq() {
                    res.stat_irq = true;
                }
            }
        }

        res
    }

    #[inline]
    fn cgb_color(cram: &[u8; 64], palette: u8, color_idx: u8) -> u16 {
        let idx = ((palette as usize & 0x07) * 4 + (color_idx as usize & 0x03)) * 2;
        u16::from_le_bytes([cram[idx], cram[idx + 1]]) & 0x7FFF
    }

    fn render_scanline(&mut self, ly: u8, is_cgb: bool) {
        let row_offset = (ly as usize) * LCD_WIDTH;
        let dmg_cols = self.dmg_palette.colors();

        let mut bg_color_idx = [0u8; LCD_WIDTH];
        let mut bg_prio = [false; LCD_WIDTH];
        let mut bg_rgb = [0u16; LCD_WIDTH];

        let bg_enabled_dmg = (self.lcdc & 0x01) != 0;
        let window_enabled = (self.lcdc & 0x20) != 0
            && self.wy_triggered
            && self.wx <= 166
            && (is_cgb || bg_enabled_dmg);

        let bg_map_base: usize = if (self.lcdc & 0x08) != 0 { 0x1C00 } else { 0x1800 };
        let win_map_base: usize = if (self.lcdc & 0x40) != 0 { 0x1C00 } else { 0x1800 };
        let unsigned_tiles = (self.lcdc & 0x10) != 0;
        let wx_start = (self.wx as i16) - 7;

        let mut window_rendered = false;

        if !is_cgb && !bg_enabled_dmg {
            let blank = dmg_cols[(self.bgp & 0x03) as usize];
            bg_rgb.fill(blank);
        } else {
            for x in 0..LCD_WIDTH {
                let in_win = window_enabled && (x as i16) >= wx_start;
                let (map_base, map_x, map_y) = if in_win {
                    window_rendered = true;
                    (win_map_base, ((x as i16) - wx_start) as u8, self.win_line)
                } else {
                    (bg_map_base, (x as u8).wrapping_add(self.scx), ly.wrapping_add(self.scy))
                };

                let tile_col = (map_x >> 3) as usize;
                let tile_row = (map_y >> 3) as usize;
                let map_off = map_base + tile_row * 32 + tile_col;
                let tile_num = self.vram[map_off];

                let (attr, vram_bank, x_flip, y_flip, pal_num) = if is_cgb {
                    let a = self.vram[0x2000 + map_off];
                    (
                        a,
                        ((a >> 3) & 1) as usize,
                        (a & 0x20) != 0,
                        (a & 0x40) != 0,
                        a & 0x07,
                    )
                } else {
                    (0, 0, false, false, 0)
                };

                let tile_data_off = if unsigned_tiles {
                    (tile_num as usize) * 16
                } else {
                    (0x1000isize + ((tile_num as i8 as isize) * 16)) as usize
                };

                let ty = if y_flip { 7 - (map_y & 7) } else { map_y & 7 } as usize;
                let tx = if x_flip { 7 - (map_x & 7) } else { map_x & 7 };

                let bank_base = vram_bank * 0x2000;
                let lo = self.vram[bank_base + tile_data_off + ty * 2];
                let hi = self.vram[bank_base + tile_data_off + ty * 2 + 1];
                let bit = 7 - tx;
                let cidx = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);

                bg_color_idx[x] = cidx;
                bg_prio[x] = is_cgb && (attr & 0x80) != 0;
                bg_rgb[x] = if is_cgb {
                    Self::cgb_color(&self.bg_cram, pal_num, cidx)
                } else {
                    dmg_cols[((self.bgp >> (cidx * 2)) & 0x03) as usize]
                };
            }
        }

        if window_rendered {
            self.win_line = self.win_line.wrapping_add(1);
        }

        // Copy BG/Window pixels into framebuffer first; sprites composite on top.
        self.framebuffer[row_offset..row_offset + LCD_WIDTH].copy_from_slice(&bg_rgb);

        if (self.lcdc & 0x02) == 0 {
            return;
        }

        let sprite_h: i16 = if (self.lcdc & 0x04) != 0 { 16 } else { 8 };
        let ly_i16 = ly as i16;
        let mut sprites: [Option<OamEntry>; 10] = [None; 10];
        let mut count = 0usize;

        for i in 0..40 {
            let base = i * 4;
            let sy = (self.oam[base] as i16) - 16;
            if ly_i16 >= sy && ly_i16 < sy + sprite_h {
                sprites[count] = Some(OamEntry {
                    y: sy,
                    x: (self.oam[base + 1] as i16) - 8,
                    tile: self.oam[base + 2],
                    flags: self.oam[base + 3],
                });
                count += 1;
                if count == 10 {
                    break;
                }
            }
        }

        if count == 0 {
            return;
        }

        if !is_cgb {
            // DMG sorts sprites by X coordinate ascending, preserving OAM index order on ties.
            sprites[..count].sort_by_key(|entry| entry.unwrap().x);
        }

        for x in 0..LCD_WIDTH {
            let x_i16 = x as i16;
            for entry in &sprites[..count] {
                let s = entry.unwrap();
                let px = x_i16 - s.x;
                if !(0..8).contains(&px) {
                    continue;
                }
                let py = ly_i16 - s.y;
                let ty = if (s.flags & 0x40) != 0 {
                    sprite_h - 1 - py
                } else {
                    py
                };
                let tx = if (s.flags & 0x20) != 0 { 7 - px } else { px };

                let tile = if sprite_h == 16 {
                    (s.tile & 0xFE) + if ty >= 8 { 1 } else { 0 }
                } else {
                    s.tile
                };
                let row = (ty & 7) as usize;
                let vram_bank = if is_cgb && (s.flags & 0x08) != 0 { 1 } else { 0 };
                let addr = vram_bank * 0x2000 + (tile as usize) * 16 + row * 2;
                let lo = self.vram[addr];
                let hi = self.vram[addr + 1];
                let bit = 7 - (tx as u8);
                let cidx = (((hi >> bit) & 1) << 1) | ((lo >> bit) & 1);
                if cidx == 0 {
                    continue;
                }

                let obj_wins = if is_cgb {
                    (self.lcdc & 0x01) == 0
                        || bg_color_idx[x] == 0
                        || (!bg_prio[x] && (s.flags & 0x80) == 0)
                } else {
                    (s.flags & 0x80) == 0 || bg_color_idx[x] == 0
                };

                if obj_wins {
                    self.framebuffer[row_offset + x] = if is_cgb {
                        Self::cgb_color(&self.obj_cram, s.flags & 0x07, cidx)
                    } else {
                        let pal = if (s.flags & 0x10) != 0 {
                            self.obp1
                        } else {
                            self.obp0
                        };
                        dmg_cols[((pal >> (cidx * 2)) & 0x03) as usize]
                    };
                }
                // First non-transparent sprite pixel in priority order determines OBJ winner at `x`.
                break;
            }
        }
    }
}
