//! Self-contained MIT/Apache synthetic GBA PPU stress ROM generator for `--selftest`.
//!
//! Exercises Mode 0 with all 4 tiled 64x64 backgrounds scrolling every frame (mosaic enabled),
//! 128 overlapping 64x64 sprites across every scanline (half semi-transparent), WIN0 + WIN1,
//! and 50/50 alpha blending, requiring no external ROM or BIOS files.

use gba_core::LCD_WIDTH;

const HEADER_BYTES: usize = 0xC0;

struct RomEmitter {
    words: Vec<u32>,
    lit_patches: Vec<(usize, u8, u32)>,
    blobs: Vec<(usize, Vec<u8>)>,
    blob_patches: Vec<(usize, u8, usize)>,
}

impl RomEmitter {
    fn new(title: &str, code: &str) -> Self {
        let mut hdr = [0u8; HEADER_BYTES];
        hdr[0xB2] = 0x96;
        let t = title.as_bytes();
        let t_len = t.len().min(12);
        hdr[0xA0..0xA0 + t_len].copy_from_slice(&t[..t_len]);
        let c = code.as_bytes();
        let c_len = c.len().min(4);
        hdr[0xAC..0xAC + c_len].copy_from_slice(&c[..c_len]);
        hdr[0xB0] = b'0';
        hdr[0xB1] = b'1';
        let sum: u8 = hdr[0xA0..=0xBC].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        hdr[0xBD] = 0u8.wrapping_sub(sum.wrapping_add(0x19));

        let mut words = Vec::with_capacity(4096);
        for chunk in hdr.chunks_exact(4) {
            words.push(u32::from_le_bytes(chunk.try_into().unwrap()));
        }
        Self {
            words,
            lit_patches: Vec::new(),
            blobs: Vec::new(),
            blob_patches: Vec::new(),
        }
    }

    #[inline]
    fn here(&self) -> usize {
        self.words.len()
    }

    #[inline]
    fn emit(&mut self, insn: u32) -> usize {
        let idx = self.words.len();
        self.words.push(insn);
        idx
    }

    fn patch_b(&mut self, insn_idx: usize, target_word: usize, cond: u32) {
        let rel = (target_word as isize) - (insn_idx as isize + 2);
        self.words[insn_idx] = (cond << 28) | 0x0A00_0000 | ((rel as u32) & 0x00FF_FFFF);
    }

    #[inline]
    fn b_always(&mut self, target_word: usize) {
        let idx = self.emit(0);
        self.patch_b(idx, target_word, 0xE);
    }

    #[inline]
    fn bne(&mut self, target_word: usize) {
        let idx = self.emit(0);
        self.patch_b(idx, target_word, 0x1);
    }

    #[inline]
    fn mov_imm(&mut self, rd: u8, imm8: u8) {
        self.emit(0xE3A0_0000 | ((rd as u32) << 12) | (imm8 as u32));
    }

    #[inline]
    fn add_imm(&mut self, rd: u8, rn: u8, imm8: u8) {
        self.emit(0xE280_0000 | ((rn as u32) << 16) | ((rd as u32) << 12) | (imm8 as u32));
    }

    #[inline]
    fn subs_imm(&mut self, rd: u8, rn: u8, imm8: u8) {
        self.emit(0xE250_0000 | ((rn as u32) << 16) | ((rd as u32) << 12) | (imm8 as u32));
    }

    #[inline]
    fn orr_imm(&mut self, rd: u8, rn: u8, imm8: u8) {
        self.emit(0xE380_0000 | ((rn as u32) << 16) | ((rd as u32) << 12) | (imm8 as u32));
    }

    #[inline]
    fn and_imm(&mut self, rd: u8, rn: u8, imm8: u8) {
        self.emit(0xE200_0000 | ((rn as u32) << 16) | ((rd as u32) << 12) | (imm8 as u32));
    }

    #[inline]
    fn ldr_lit(&mut self, rd: u8, val: u32) {
        let idx = self.emit(0);
        self.lit_patches.push((idx, rd, val));
    }

    #[inline]
    fn ldr_post4(&mut self, rd: u8, rn: u8) {
        self.emit(0xE490_0004 | ((rn as u32) << 16) | ((rd as u32) << 12));
    }

    #[inline]
    fn str_post4(&mut self, rd: u8, rn: u8) {
        self.emit(0xE480_0004 | ((rn as u32) << 16) | ((rd as u32) << 12));
    }

    #[inline]
    fn str32(&mut self, rd: u8, rn: u8) {
        self.emit(0xE580_0000 | ((rn as u32) << 16) | ((rd as u32) << 12));
    }

    #[inline]
    fn ldrh(&mut self, rd: u8, rn: u8) {
        self.emit(0xE1D0_00B0 | ((rn as u32) << 16) | ((rd as u32) << 12));
    }

    #[inline]
    fn strh_imm(&mut self, rd: u8, rn: u8, imm8: u8) {
        let hi = ((imm8 as u32) >> 4) & 0xF;
        let lo = (imm8 as u32) & 0xF;
        self.emit(0xE1C0_00B0 | ((rn as u32) << 16) | ((rd as u32) << 12) | (hi << 8) | lo);
    }

    fn write_io16(&mut self, addr: u32, val: u16) {
        self.ldr_lit(0, addr);
        self.ldr_lit(1, val as u32);
        self.strh_imm(1, 0, 0);
    }

    fn add_blob(&mut self, mut bytes: Vec<u8>) -> usize {
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let id = self.blobs.len();
        self.blobs.push((0, bytes));
        id
    }

    fn copy_blob_words(&mut self, blob_id: usize, dst_addr: u32) {
        let word_count = (self.blobs[blob_id].1.len() / 4) as u32;
        let idx = self.emit(0);
        self.blob_patches.push((idx, 0, blob_id));
        self.ldr_lit(1, dst_addr);
        self.ldr_lit(2, word_count);
        let loop_word = self.here();
        self.ldr_post4(3, 0);
        self.str_post4(3, 1);
        self.subs_imm(2, 2, 1);
        self.bne(loop_word);
    }

    fn finalize(mut self) -> Vec<u8> {
        let pool_start = self.words.len();
        for &(insn_idx, rd, val) in &self.lit_patches {
            let val_word = self.words.len();
            self.words.push(val);
            let off = ((val_word as isize - (insn_idx as isize + 2)) * 4) as u32;
            debug_assert!(off < 4096, "literal pool offset out of range: {off}");
            self.words[insn_idx] = 0xE59F_0000 | ((rd as u32) << 12) | (off & 0xFFF);
        }
        let mut blob_ptr_words = vec![0usize; self.blobs.len()];
        for b_idx in 0..self.blobs.len() {
            blob_ptr_words[b_idx] = self.words.len();
            self.words.push(0);
        }
        let _ = pool_start;
        for &(insn_idx, rd, blob_id) in &self.blob_patches {
            let ptr_word = blob_ptr_words[blob_id];
            let off = ((ptr_word as isize - (insn_idx as isize + 2)) * 4) as u32;
            debug_assert!(off < 4096, "blob ptr offset out of range: {off}");
            self.words[insn_idx] = 0xE59F_0000 | ((rd as u32) << 12) | (off & 0xFFF);
        }
        for (b_idx, (_, bytes)) in self.blobs.iter().enumerate() {
            let rom_addr = 0x0800_0000u32 + (self.words.len() as u32) * 4;
            self.words[blob_ptr_words[b_idx]] = rom_addr;
            for chunk in bytes.chunks_exact(4) {
                self.words.push(u32::from_le_bytes(chunk.try_into().unwrap()));
            }
        }
        let mut out = Vec::with_capacity(self.words.len() * 4);
        for w in self.words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }
}

fn build_palette() -> Vec<u8> {
    let mut out = Vec::with_capacity(1024);
    for i in 0..512usize {
        let bank = (i >> 4) & 31;
        let idx = i & 15;
        let c = if i >= 256 && idx == 0 {
            0
        } else {
            let r = ((idx * 2 + bank) & 31) as u16;
            let g = (((idx * 3) ^ (bank * 5)) & 31) as u16;
            let b = (((31 - idx * 2) + bank * 2) & 31) as u16;
            (b << 10) | (g << 5) | r
        };
        out.extend_from_slice(&c.to_le_bytes());
    }
    out
}

fn build_4bpp_tiles(tile_count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(tile_count * 32);
    for t in 0..tile_count {
        for py in 0..8usize {
            for px_pair in 0..4usize {
                let x0 = px_pair * 2;
                let x1 = x0 + 1;
                let c0 = (((x0 ^ py).wrapping_add(t)) % 15 + 1) as u8;
                let c1 = (((x1 ^ py).wrapping_add(t)) % 15 + 1) as u8;
                out.push((c1 << 4) | c0);
            }
        }
    }
    out
}

fn build_mode0_64x64_map(seed: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(8192);
    for screen in 0..4u16 {
        for ty in 0..32u16 {
            for tx in 0..32u16 {
                let tile = (tx.wrapping_mul(3)
                    ^ ty.wrapping_mul(5)
                    ^ screen.wrapping_mul(7)
                    ^ seed)
                    & 63;
                let pal = ((tx + ty + seed) & 15) << 12;
                let entry = pal | tile;
                out.extend_from_slice(&entry.to_le_bytes());
            }
        }
    }
    out
}

fn build_oam_128_heavy() -> Vec<u8> {
    let mut out = Vec::with_capacity(1024);
    for i in 0..128usize {
        // Arrange 128 64x64 sprites across Y=0..96 and X=0..176 so dozens of large
        // overlapping sprites touch every scanline from 0 to 159.
        let y = ((i % 8) * 16) as u16;
        let x = ((i / 8) * 12) as u16;
        let semi_transparent = if (i & 1) == 0 { 1u16 << 10 } else { 0 };
        let mosaic = 1u16 << 12;
        let attr0 = (y & 0xFF) | semi_transparent | mosaic;
        // Normal sprite size=3 (64x64) with square shape=0.
        let attr1 = (x & 0x1FF) | (3u16 << 14);
        let tile = ((i * 8) & 1023) as u16;
        let prio = ((i & 3) as u16) << 10;
        let pal = ((i & 15) as u16) << 12;
        let attr2 = pal | prio | tile;
        out.extend_from_slice(&attr0.to_le_bytes());
        out.extend_from_slice(&attr1.to_le_bytes());
        out.extend_from_slice(&attr2.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// Builds the deterministic MIT/Apache `STRESS_SELF` GBA Mode 0 stress ROM (all 4 BGs + 128
/// 64x64 sprites + WIN0/WIN1 + mosaic + 50/50 alpha blend scrolling continuously).
pub fn build_stress_mode0_4bg_128spr_rom() -> Vec<u8> {
    let mut b = RomEmitter::new("STRESS_SELF", "SSLF");

    let entry_word = HEADER_BYTES / 4;
    let b_placeholder = entry_word - 1;
    // Patch word 0 (`0x0800_0000`) to jump cleanly past header + inline IRQ handler to `start`.
    let _ = b_placeholder;

    let pal = b.add_blob(build_palette());
    let bg_tiles = b.add_blob(build_4bpp_tiles(64)); // 2 KiB -> 0x0600_0000
    let obj_tiles = b.add_blob(build_4bpp_tiles(512)); // 16 KiB -> 0x0601_0000
    let map0 = b.add_blob(build_mode0_64x64_map(0));
    let map1 = b.add_blob(build_mode0_64x64_map(3));
    let map2 = b.add_blob(build_mode0_64x64_map(7));
    let map3 = b.add_blob(build_mode0_64x64_map(11));
    let oam = b.add_blob(build_oam_128_heavy());

    // Emit VBlank IRQ handler immediately after the 192-byte header.
    let irq_word = b.here();
    b.ldr_lit(0, 0x0400_0202); // REG_IF
    b.mov_imm(1, 1);
    b.strh_imm(1, 0, 0);
    b.ldr_lit(0, 0x0300_7FF8); // BIOS IntrWait flag
    b.ldrh(2, 0);
    b.orr_imm(2, 2, 1);
    b.strh_imm(2, 0, 0);
    b.emit(0xE12F_FF1E); // BX LR

    let start_word = b.here();
    b.patch_b(0, start_word, 0xE);

    // Setup VBlank IRQ
    b.write_io16(0x0400_0208, 0); // IME = 0
    let irq_rom_addr = 0x0800_0000u32 + (irq_word as u32) * 4;
    b.ldr_lit(0, 0x0300_7FFC);
    b.ldr_lit(1, irq_rom_addr);
    b.str32(1, 0);
    b.write_io16(0x0400_0004, 0x0008); // DISPSTAT VBlank IRQ enable
    b.write_io16(0x0400_0200, 0x0001); // IE = VBlank
    b.write_io16(0x0400_0208, 1); // IME = 1

    b.copy_blob_words(pal, 0x0500_0000);
    b.copy_blob_words(bg_tiles, 0x0600_0000);
    b.copy_blob_words(map0, 0x0600_8000);
    b.copy_blob_words(map1, 0x0600_A000);
    b.copy_blob_words(map2, 0x0600_C000);
    b.copy_blob_words(map3, 0x0600_E000);
    b.copy_blob_words(obj_tiles, 0x0601_0000);
    b.copy_blob_words(oam, 0x0700_0000);

    // BG0..BG3: 64x64 mosaic-enabled regular backgrounds at SB 16, 20, 24, 28
    b.write_io16(0x0400_0008, 0xD040); // BG0CNT: prio 0, SB=16, mosaic=1, 64x64
    b.write_io16(0x0400_000A, 0xD441); // BG1CNT: prio 1, SB=20, mosaic=1, 64x64
    b.write_io16(0x0400_000C, 0xD842); // BG2CNT: prio 2, SB=24, mosaic=1, 64x64
    b.write_io16(0x0400_000E, 0xDC43); // BG3CNT: prio 3, SB=28, mosaic=1, 64x64

    // WIN0 + WIN1 across full height + BLDCNT alpha blend + Mosaic 2x2
    b.write_io16(0x0400_0040, (16 << 8) | (LCD_WIDTH as u16 - 16)); // WIN0H
    b.write_io16(0x0400_0042, (40 << 8) | (LCD_WIDTH as u16 - 40)); // WIN1H
    b.write_io16(0x0400_0044, 160); // WIN0V = 0..160
    b.write_io16(0x0400_0046, 160); // WIN1V = 0..160
    b.write_io16(0x0400_0048, 0x3F3F); // WININ
    b.write_io16(0x0400_004A, 0x3F3F); // WINOUT
    b.write_io16(0x0400_004C, 0x1111); // MOSAIC
    b.write_io16(0x0400_0050, 0x3F5F); // BLDCNT = alpha blend all layers
    b.write_io16(0x0400_0052, 0x0808); // BLDALPHA = 50/50

    // DISPCNT = Mode 0 | 1D OBJ (0x40) | BG0..BG3 (0x0F00) | OBJ (0x1000) | WIN0..1 (0x6000)
    b.write_io16(0x0400_0000, 0x7F40);

    b.mov_imm(8, 0); // r8 = frame counter
    let main_loop = b.here();
    b.emit(0xEF05_0000); // SWI 0x05 (VBlankIntrWait)
    b.add_imm(8, 8, 1);
    b.ldr_lit(0, 0x0400_0010); // BG0HOFS
    b.strh_imm(8, 0, 0x00); // BG0HOFS = frame
    b.add_imm(1, 8, 16);
    b.strh_imm(1, 0, 0x04); // BG1HOFS = frame + 16
    b.and_imm(2, 8, 0x7F);
    b.strh_imm(2, 0, 0x08); // BG2HOFS
    b.add_imm(3, 8, 48);
    b.strh_imm(3, 0, 0x0C); // BG3HOFS
    b.b_always(main_loop);

    b.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT};
    use std::collections::HashSet;

    struct CaptureFrontend {
        last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
        presented: usize,
    }

    impl HostFrontend for CaptureFrontend {
        fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
            self.last_fb.copy_from_slice(fb);
            self.presented += 1;
        }
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            0x03FF
        }
    }

    #[test]
    fn embedded_stress_rom_has_valid_header_and_renders_animated_scene() {
        let rom = build_stress_mode0_4bg_128spr_rom();
        assert!(rom.len() > HEADER_BYTES);
        assert_eq!(&rom[0xA0..0xAB], b"STRESS_SELF");
        assert_eq!(rom[0xB2], 0x96);
        let sum: u8 = rom[0xA0..=0xBC].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        assert_eq!(sum.wrapping_add(0x19).wrapping_add(rom[0xBD]), 0);

        let mut core = Box::new(GbaCore::new(None, rom));
        let mut fe = CaptureFrontend {
            last_fb: [0; LCD_WIDTH * LCD_HEIGHT],
            presented: 0,
        };
        for _ in 0..10 {
            core.run_frame(&mut fe);
        }
        let f10 = fe.last_fb;
        for _ in 0..20 {
            core.run_frame(&mut fe);
        }
        let f30 = fe.last_fb;
        assert_eq!(fe.presented, 30);
        let distinct_colors: HashSet<u16> = f30.iter().copied().collect();
        assert!(
            distinct_colors.len() >= 16,
            "expected rich multi-color Mode 0 scene, found {} colors",
            distinct_colors.len()
        );
        assert_ne!(f10, f30, "expected scrolling animation between frame 10 and 30");
    }
}
