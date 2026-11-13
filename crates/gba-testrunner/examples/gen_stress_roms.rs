//! Generates minimal, deterministic ARM7TDMI `.gba` stress ROMs for Phase 3C PPU benchmarking.
//!
//! Run with:
//!   cargo run -p gba-testrunner --example gen_stress_roms

use std::collections::HashMap;
use std::fs;
use std::path::Path;

const NINTENDO_LOGO: [u8; 156] = [
    0x24, 0xFF, 0xAE, 0x51, 0x69, 0x9A, 0xA2, 0x21, 0x3D, 0x84, 0x82, 0x0A, 0x84, 0xE4, 0x09, 0xAD,
    0x11, 0x24, 0x8B, 0x98, 0xC0, 0x81, 0x7F, 0x21, 0xA3, 0x52, 0xBE, 0x19, 0x93, 0x09, 0xCE, 0x20,
    0x10, 0x46, 0x4A, 0x4A, 0xF8, 0x27, 0x31, 0xEC, 0x58, 0xC7, 0xE8, 0x33, 0x82, 0xE3, 0xCE, 0xBF,
    0x85, 0xF4, 0xDF, 0x94, 0xCE, 0x4B, 0x09, 0xC1, 0x94, 0x56, 0x8A, 0xC0, 0x13, 0x72, 0xA7, 0xFC,
    0x9F, 0x84, 0x4D, 0x73, 0xA3, 0xCA, 0x9A, 0x61, 0x58, 0x97, 0xA3, 0x27, 0xFC, 0x03, 0x98, 0x76,
    0x23, 0x1D, 0xC7, 0x61, 0x03, 0x04, 0xAE, 0x56, 0xBF, 0x38, 0x84, 0x00, 0x40, 0xA7, 0x0E, 0xFD,
    0xFF, 0x52, 0xFE, 0x03, 0x6F, 0x95, 0x30, 0xF1, 0x97, 0xFB, 0xC0, 0x85, 0x60, 0xD6, 0x80, 0x25,
    0xA9, 0x63, 0xBE, 0x03, 0x01, 0x4E, 0x38, 0xE2, 0xF9, 0xA2, 0x34, 0xFF, 0xBB, 0x3E, 0x03, 0x44,
    0x78, 0x00, 0x90, 0xCB, 0x88, 0x11, 0x3A, 0x94, 0x65, 0xC0, 0x7C, 0x63, 0x87, 0xF0, 0x3C, 0xAF,
    0xD6, 0x25, 0xE4, 0x8B, 0x38, 0x0A, 0xAC, 0x72, 0x21, 0xD4, 0xF8, 0x07,
];

fn encode_arm_imm(val: u32) -> Option<u32> {
    for rot in 0..16u32 {
        let rotated = val.rotate_left(rot * 2);
        if rotated <= 0xFF {
            return Some((rot << 8) | rotated);
        }
    }
    None
}

enum Patch {
    Branch { word_idx: usize, cond: u32, link: bool, label: String },
    LdrLitLabel { word_idx: usize, rd: u32, label: String },
    LdrLitConst { word_idx: usize, rd: u32, value: u32 },
}

struct ArmBuilder {
    words: Vec<u32>,
    labels: HashMap<String, usize>,
    patches: Vec<Patch>,
    tail_blobs: Vec<(String, Vec<u8>)>,
}

#[allow(dead_code)]
impl ArmBuilder {
    fn new(title12: &str, game_code4: &str) -> Self {
        // Reserve 0xC0 bytes (48 words) for standard GBA cartridge header.
        let mut words = vec![0u32; 48];
        // Build temporary header bytes except branch & complement, patched at finalize.
        let mut hdr = [0u8; 0xC0];
        hdr[0x04..0xA0].copy_from_slice(&NINTENDO_LOGO);
        let tb = title12.as_bytes();
        for i in 0..12.min(tb.len()) {
            hdr[0xA0 + i] = tb[i];
        }
        let cb = game_code4.as_bytes();
        for i in 0..4.min(cb.len()) {
            hdr[0xAC + i] = cb[i];
        }
        hdr[0xB0] = b'0';
        hdr[0xB1] = b'1';
        hdr[0xB2] = 0x96;
        // Compute complement at 0xBD
        let mut chk: i32 = 0;
        for &b in &hdr[0xA0..=0xBC] {
            chk = chk.wrapping_sub(b as i32);
        }
        hdr[0xBD] = chk.wrapping_sub(0x19) as u8;
        for i in 0..48 {
            words[i] = u32::from_le_bytes([
                hdr[i * 4],
                hdr[i * 4 + 1],
                hdr[i * 4 + 2],
                hdr[i * 4 + 3],
            ]);
        }
        // Entry branch at word 0 -> ROM 0x0800_00C0 (word 48), skip offset = 48 - 2 = 46 words
        words[0] = 0xEA00_0000 | 46;

        Self {
            words,
            labels: HashMap::new(),
            patches: Vec::new(),
            tail_blobs: Vec::new(),
        }
    }

    fn label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.words.len());
    }

    fn emit(&mut self, op: u32) {
        self.words.push(op);
    }

    fn mov_u32(&mut self, rd: u32, val: u32) {
        if let Some(imm) = encode_arm_imm(val) {
            self.emit(0xE3A0_0000 | ((rd & 0xF) << 12) | imm);
        } else if let Some(imm) = encode_arm_imm(!val) {
            // MVN Rd, #imm
            self.emit(0xE3E0_0000 | ((rd & 0xF) << 12) | imm);
        } else {
            let word_idx = self.words.len();
            self.emit(0);
            self.patches.push(Patch::LdrLitConst {
                word_idx,
                rd,
                value: val,
            });
        }
    }

    fn ldr_addr(&mut self, rd: u32, label: &str) {
        let word_idx = self.words.len();
        self.emit(0);
        self.patches.push(Patch::LdrLitLabel {
            word_idx,
            rd,
            label: label.to_string(),
        });
    }

    fn mov_reg(&mut self, rd: u32, rm: u32) {
        self.emit(0xE1A0_0000 | ((rd & 0xF) << 12) | (rm & 0xF));
    }

    fn mov_lsl(&mut self, rd: u32, rm: u32, shift: u32) {
        assert!(shift < 32);
        self.emit(0xE1A0_0000 | ((rd & 0xF) << 12) | (shift << 7) | (rm & 0xF));
    }

    fn add_imm(&mut self, rd: u32, rn: u32, val: u32) {
        let imm = encode_arm_imm(val).expect("add_imm expects encodable ARM imm12");
        self.emit(0xE280_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | imm);
    }

    fn subs_imm(&mut self, rd: u32, rn: u32, val: u32) {
        let imm = encode_arm_imm(val).expect("subs_imm expects encodable ARM imm12");
        self.emit(0xE250_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | imm);
    }

    fn add_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xE080_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | (rm & 0xF));
    }

    fn orr_reg(&mut self, rd: u32, rn: u32, rm: u32) {
        self.emit(0xE180_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | (rm & 0xF));
    }

    fn and_imm(&mut self, rd: u32, rn: u32, val: u32) {
        let imm = encode_arm_imm(val).expect("and_imm expects encodable ARM imm12");
        self.emit(0xE200_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | imm);
    }

    fn cmp_reg(&mut self, rn: u32, rm: u32) {
        self.emit(0xE150_0000 | ((rn & 0xF) << 16) | (rm & 0xF));
    }

    fn cmp_imm(&mut self, rn: u32, val: u32) {
        let imm = encode_arm_imm(val).expect("cmp_imm expects encodable ARM imm12");
        self.emit(0xE350_0000 | ((rn & 0xF) << 16) | imm);
    }

    fn ldr32(&mut self, rd: u32, rn: u32, imm: u32) {
        assert!(imm < 4096);
        self.emit(0xE590_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | imm);
    }

    fn str32(&mut self, rd: u32, rn: u32, imm: u32) {
        assert!(imm < 4096);
        self.emit(0xE580_0000 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | imm);
    }

    fn ldr32_post4(&mut self, rd: u32, rn: u32) {
        self.emit(0xE490_0004 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12));
    }

    fn str32_post4(&mut self, rd: u32, rn: u32) {
        self.emit(0xE480_0004 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12));
    }

    fn ldrh(&mut self, rd: u32, rn: u32, imm8: u32) {
        assert!(imm8 < 256);
        let hi = (imm8 & 0xF0) << 4;
        let lo = imm8 & 0x0F;
        self.emit(0xE1D0_00B0 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | hi | lo);
    }

    fn strh(&mut self, rd: u32, rn: u32, imm8: u32) {
        assert!(imm8 < 256);
        let hi = (imm8 & 0xF0) << 4;
        let lo = imm8 & 0x0F;
        self.emit(0xE1C0_00B0 | ((rn & 0xF) << 16) | ((rd & 0xF) << 12) | hi | lo);
    }

    fn b(&mut self, label: &str) {
        let word_idx = self.words.len();
        self.emit(0);
        self.patches.push(Patch::Branch {
            word_idx,
            cond: 0xE,
            link: false,
            label: label.to_string(),
        });
    }

    fn bl(&mut self, label: &str) {
        let word_idx = self.words.len();
        self.emit(0);
        self.patches.push(Patch::Branch {
            word_idx,
            cond: 0xE,
            link: true,
            label: label.to_string(),
        });
    }

    fn beq(&mut self, label: &str) {
        let word_idx = self.words.len();
        self.emit(0);
        self.patches.push(Patch::Branch {
            word_idx,
            cond: 0x0,
            link: false,
            label: label.to_string(),
        });
    }

    fn bne(&mut self, label: &str) {
        let word_idx = self.words.len();
        self.emit(0);
        self.patches.push(Patch::Branch {
            word_idx,
            cond: 0x1,
            link: false,
            label: label.to_string(),
        });
    }

    fn bx(&mut self, rm: u32) {
        self.emit(0xE12F_FF10 | (rm & 0xF));
    }

    fn swi(&mut self, comment: u8) {
        self.emit(0xEF00_0000 | ((comment as u32) << 16));
    }

    fn write_io16(&mut self, addr: u32, val: u16) {
        // Uses r0 = addr, r1 = val
        self.mov_u32(0, addr);
        self.mov_u32(1, val as u32);
        self.strh(1, 0, 0);
    }

    fn write_io32(&mut self, addr: u32, val: u32) {
        self.mov_u32(0, addr);
        self.mov_u32(1, val);
        self.str32(1, 0, 0);
    }

    fn add_blob(&mut self, name: &str, mut data: Vec<u8>) {
        while data.len() % 4 != 0 {
            data.push(0);
        }
        self.tail_blobs.push((name.to_string(), data));
    }

    fn emit_standard_subroutines(&mut self) {
        // irq_handler: ack IF, update BIOS_IF (0x03007FF8), bx lr
        self.label("irq_handler");
        self.mov_u32(0, 0x0400_0200);
        self.ldrh(1, 0, 2); // REG_IF
        self.strh(1, 0, 2); // ack REG_IF
        self.mov_u32(2, 0x0300_7F00);
        self.ldrh(3, 2, 0xF8);
        self.orr_reg(3, 3, 1);
        self.strh(3, 2, 0xF8);
        self.bx(14);

        // copy_words(r0 = src, r1 = dst, r2 = word_count)
        self.label("copy_words");
        self.cmp_imm(2, 0);
        self.beq("copy_words_done");
        self.label("copy_words_loop");
        self.ldr32_post4(3, 0);
        self.str32_post4(3, 1);
        self.subs_imm(2, 2, 1);
        self.bne("copy_words_loop");
        self.label("copy_words_done");
        self.bx(14);
    }

    fn call_copy_blob(&mut self, blob_label: &str, dst_addr: u32, byte_len: usize) {
        assert!(byte_len % 4 == 0);
        self.ldr_addr(0, blob_label);
        self.mov_u32(1, dst_addr);
        self.mov_u32(2, (byte_len / 4) as u32);
        self.bl("copy_words");
    }

    fn setup_irq_vblank(&mut self) {
        self.ldr_addr(0, "irq_handler");
        self.mov_u32(1, 0x0300_7FFC);
        self.str32(0, 1, 0);
        self.write_io16(0x0400_0004, 0x0008); // DISPSTAT: VBlank IRQ enable
        self.write_io16(0x0400_0200, 0x0001); // IE: VBlank
        self.write_io16(0x0400_0208, 0x0001); // IME: 1
    }

    fn finalize(mut self) -> Vec<u8> {
        // Reserve literal pool right after code (always within +/- 4 KiB of code).
        let mut const_pool_idx: HashMap<u32, usize> = HashMap::new();
        let mut label_pool_idx: HashMap<String, usize> = HashMap::new();

        // Collect all pool slots first so offsets are deterministic.
        let mut extra_const_words = Vec::new();
        for p in &self.patches {
            match p {
                Patch::LdrLitConst { value, .. } => {
                    if !const_pool_idx.contains_key(value) {
                        let idx = self.words.len() + extra_const_words.len();
                        const_pool_idx.insert(*value, idx);
                        extra_const_words.push(*value);
                    }
                }
                Patch::LdrLitLabel { label, .. } => {
                    if !label_pool_idx.contains_key(label) {
                        let idx = self.words.len() + extra_const_words.len();
                        label_pool_idx.insert(label.clone(), idx);
                        extra_const_words.push(0); // patched once blobs are placed
                    }
                }
                Patch::Branch { .. } => {}
            }
        }
        self.words.extend(extra_const_words);

        // Append blobs after literal pool and record their ROM word indices.
        for (name, bytes) in self.tail_blobs {
            let start_word = self.words.len();
            self.labels.insert(name, start_word);
            for chunk in bytes.chunks_exact(4) {
                self.words.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
        }

        // Fill LdrLitLabel pool values with 0x0800_0000 + word * 4.
        for (label, pool_word) in &label_pool_idx {
            let target_word = *self
                .labels
                .get(label)
                .unwrap_or_else(|| panic!("undefined label `{label}`"));
            self.words[*pool_word] = 0x0800_0000 + (target_word as u32) * 4;
        }

        // Patch branch & LDR literal instructions.
        for p in self.patches {
            match p {
                Patch::Branch {
                    word_idx,
                    cond,
                    link,
                    label,
                } => {
                    let target_word = *self
                        .labels
                        .get(&label)
                        .unwrap_or_else(|| panic!("undefined branch label `{label}`"));
                    let rel = (target_word as isize) - (word_idx as isize + 2);
                    assert!(rel >= -(1 << 23) && rel < (1 << 23));
                    let link_bit = if link { 1 << 24 } else { 0 };
                    self.words[word_idx] = (cond << 28) | (0x5 << 25) | link_bit | ((rel as u32) & 0x00FF_FFFF);
                }
                Patch::LdrLitConst { word_idx, rd, value } => {
                    let pool_word = const_pool_idx[&value];
                    let byte_off = ((pool_word as isize) - (word_idx as isize + 2)) * 4;
                    assert!((0..4096).contains(&byte_off), "const literal offset out of range: {byte_off}");
                    self.words[word_idx] = 0xE59F_0000 | ((rd & 0xF) << 12) | (byte_off as u32);
                }
                Patch::LdrLitLabel { word_idx, rd, label } => {
                    let pool_word = label_pool_idx[&label];
                    let byte_off = ((pool_word as isize) - (word_idx as isize + 2)) * 4;
                    assert!((0..4096).contains(&byte_off), "label literal offset out of range: {byte_off}");
                    self.words[word_idx] = 0xE59F_0000 | ((rd & 0xF) << 12) | (byte_off as u32);
                }
            }
        }

        // Serialize to LE bytes and pad to at least 128 KiB (power-of-two friendly ROM).
        let mut out = Vec::with_capacity(self.words.len() * 4);
        for w in self.words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        let target_len = out.len().next_power_of_two().max(128 * 1024);
        out.resize(target_len, 0xFF);
        out
    }
}

fn build_palette() -> Vec<u8> {
    // 256 BG colors + 256 OBJ colors (1024 bytes total), vibrant non-zero entries
    let mut pal = Vec::with_capacity(1024);
    for bank in 0..2 {
        for idx in 0..256u16 {
            let r = ((idx.wrapping_mul(7) + bank * 11) & 0x1F).max(if idx == 0 { 4 } else { 1 });
            let g = (((idx >> 2).wrapping_mul(11) + bank * 5) & 0x1F).max(1);
            let b = (((idx >> 4).wrapping_mul(13) + idx + bank * 3) & 0x1F).max(2);
            let c = r | (g << 5) | (b << 10);
            pal.extend_from_slice(&c.to_le_bytes());
        }
    }
    pal
}

/// Generates 128 OAM entries (1024 bytes) all overlapping virtually every scanline.
/// Uses 64x64 Double-Size Affine sprites (128x128 rendered bounding box!) across all 128 slots:
/// - Slots 0..31:  OBJWIN double-size affine
/// - Slots 32..79: Semi-transparent double-size affine
/// - Slots 80..127: Normal double-size affine
/// Plus 32 distinct rotation/scale affine matrices stored in OAM parameter slots.
fn build_oam_128_double_aff(tile_base: u16, mosaic: bool) -> Vec<u8> {
    let mut oam = vec![0u8; 1024];
    let mos_bit = if mosaic { 1u16 << 12 } else { 0 };
    for i in 0..128usize {
        // Spread across Y in [-32, 80] (wrapping 8-bit) so the 128x128 double-size box
        // spans most or all scanlines y=0..159 simultaneously!
        let y_signed = ((i * 5) as i32 % 112) - 24;
        let y = (y_signed as u8) as u16;
        let x = ((i * 19) as i32 % 180) as u16;
        let mode_bits = if i < 32 {
            2u16 << 10 // OBJWIN
        } else if i < 80 {
            1u16 << 10 // Semi-transparent
        } else {
            0u16 << 10 // Normal
        };
        // Rot/Scale enabled (bit 8) + Double-Size enabled (bit 9) + 4bpp (bit 13 = 0) + mosaic
        // Square 64x64 => shape=0 (attr0 bits 14..15 = 0), size=3 (attr1 bits 14..15 = 3)
        let attr0 = (y & 0xFF) | (0b11 << 8) | mode_bits | mos_bit;
        let aff_param = ((i % 32) as u16) << 9;
        let attr1 = (x & 0x1FF) | aff_param | (3u16 << 14);
        let tile = (tile_base + ((i as u16 % 4) * 4)) & 0x3FF;
        let prio = ((i % 3) as u16 + 1) << 10;
        let pal_bank = ((i % 15) as u16 + 1) << 12;
        let attr2 = tile | prio | pal_bank;

        let base = i * 8;
        oam[base..base + 2].copy_from_slice(&attr0.to_le_bytes());
        oam[base + 2..base + 4].copy_from_slice(&attr1.to_le_bytes());
        oam[base + 4..base + 6].copy_from_slice(&attr2.to_le_bytes());
    }

    // Write 32 affine matrices at bytes i*32 + {6, 14, 22, 30}
    for m in 0..32usize {
        let angle = (m as f64) * std::f64::consts::TAU / 32.0;
        let scale = 0.85 + 0.02 * (m as f64);
        let pa = (angle.cos() * scale * 256.0) as i16;
        let pb = (-angle.sin() * scale * 256.0) as i16;
        let pc = (angle.sin() * scale * 256.0) as i16;
        let pd = (angle.cos() * scale * 256.0) as i16;
        let base = m * 32;
        oam[base + 6..base + 8].copy_from_slice(&pa.to_le_bytes());
        oam[base + 14..base + 16].copy_from_slice(&pb.to_le_bytes());
        oam[base + 22..base + 24].copy_from_slice(&pc.to_le_bytes());
        oam[base + 30..base + 32].copy_from_slice(&pd.to_le_bytes());
    }
    oam
}

fn build_4bpp_obj_tiles(bytes: usize) -> Vec<u8> {
    // Rich non-zero 4bpp nibble patterns (no transparent nibble 0!) so sprites draw solid pixels.
    let mut v = vec![0u8; bytes];
    for (idx, b) in v.iter_mut().enumerate() {
        let lo = ((idx % 15) + 1) as u8;
        let hi = (((idx.wrapping_mul(7)) % 15) + 1) as u8;
        *b = lo | (hi << 4);
    }
    v
}

fn build_8bpp_bg_tiles(num_tiles: usize) -> Vec<u8> {
    // 64 bytes per 8bpp tile; all non-zero indices (1..=255) so no transparent skips.
    let mut v = vec![0u8; num_tiles * 64];
    for (i, b) in v.iter_mut().enumerate() {
        *b = ((i.wrapping_mul(13) ^ (i >> 3)) % 255 + 1) as u8;
    }
    v
}

fn build_affine_bg_map_1024() -> Vec<u8> {
    // 128 x 128 affine tilemap = 16,384 bytes
    let mut map = vec![0u8; 128 * 128];
    for (i, b) in map.iter_mut().enumerate() {
        *b = ((i ^ (i >> 7)) & 0x3F) as u8;
    }
    map
}

fn build_reg_bg_map_64x64(tile_count_mask: u16) -> Vec<u8> {
    // 4 screenblocks * 2048 bytes = 8192 bytes (64x64 regular tilemap)
    let mut map = Vec::with_capacity(8192);
    for i in 0..4096u16 {
        let tid = ((i.wrapping_mul(3) ^ (i >> 5)) & tile_count_mask).max(1);
        let flip = (i & 3) << 10;
        let entry = tid | flip;
        map.extend_from_slice(&entry.to_le_bytes());
    }
    map
}

/// 1) `stress_aff_objwin.gba`
/// Mode 2: BG2 + BG3 (both 1024x1024 affine w/ wraparound & mosaic) + 128 OAM double-size affine
/// sprites (OBJWIN + semi-transparent + normal, mosaic) + WIN0 + WIN1 + OBJWIN + 50/50 Alpha Blend.
fn gen_stress_aff_objwin() -> Vec<u8> {
    let mut b = ArmBuilder::new("STRESS_AFFOW", "ASAW");
    b.emit_standard_subroutines();

    b.label("start");
    b.setup_irq_vblank();

    b.add_blob("pal_data", build_palette());
    b.add_blob("bg_tiles", build_8bpp_bg_tiles(64)); // 4096 bytes -> 0x0600_0000
    b.add_blob("aff_map", build_affine_bg_map_1024()); // 16384 bytes -> 0x0600_8000 & 0x0600_C000
    b.add_blob("obj_tiles", build_4bpp_obj_tiles(16384)); // 16 KiB -> 0x0601_0000
    b.add_blob("oam_data", build_oam_128_double_aff(0, true));

    b.call_copy_blob("pal_data", 0x0500_0000, 1024);
    b.call_copy_blob("bg_tiles", 0x0600_0000, 4096);
    b.call_copy_blob("aff_map", 0x0600_8000, 16384);
    b.call_copy_blob("aff_map", 0x0600_C000, 16384);
    b.call_copy_blob("obj_tiles", 0x0601_0000, 16384);
    b.call_copy_blob("oam_data", 0x0700_0000, 1024);

    // BG2CNT: prio 1, char=0, mosaic=1 (0x40), wrap=1 (0x2000), screen_base=16 (0x1000), size=3 (0xC000) => 0xF041
    b.write_io16(0x0400_000C, 0xF041);
    // BG3CNT: prio 2, char=0, mosaic=1 (0x40), wrap=1 (0x2000), screen_base=24 (0x1800), size=3 (0xC000) => 0xF842
    b.write_io16(0x0400_000E, 0xF842);

    // Non-identity affine matrices on BG2 and BG3
    b.write_io16(0x0400_0020, 0x00D0); // BG2PA
    b.write_io16(0x0400_0022, 0x0030); // BG2PB
    b.write_io16(0x0400_0024, (-0x0030i16) as u16); // BG2PC
    b.write_io16(0x0400_0026, 0x00D0); // BG2PD
    b.write_io32(0x0400_0028, 0x0000_2000); // BG2X
    b.write_io32(0x0400_002C, 0x0000_1800); // BG2Y

    b.write_io16(0x0400_0030, 0x00B8); // BG3PA
    b.write_io16(0x0400_0032, (-0x0048i16) as u16); // BG3PB
    b.write_io16(0x0400_0034, 0x0048); // BG3PC
    b.write_io16(0x0400_0036, 0x00B8); // BG3PD
    b.write_io32(0x0400_0038, 0x0000_3400); // BG3X
    b.write_io32(0x0400_003C, 0x0000_2200); // BG3Y

    // WIN0 + WIN1 + OBJWIN active across all scanlines 0..160
    b.write_io16(0x0400_0040, (24 << 8) | 216); // WIN0H
    b.write_io16(0x0400_0042, (48 << 8) | 192); // WIN1H
    b.write_io16(0x0400_0044, (0 << 8) | 160);  // WIN0V
    b.write_io16(0x0400_0046, (0 << 8) | 160);  // WIN1V
    b.write_io16(0x0400_0048, 0x3F3F);          // WININ
    b.write_io16(0x0400_004A, 0x3F3F);          // WINOUT

    // Mosaic 3x3 on BG & OBJ
    b.write_io16(0x0400_004C, 0x2222);

    // Alpha Blend (mode 1) across BG2, BG3, OBJ, BD -> BLDCNT = 0x3F4C, BLDALPHA = 0x0808
    b.write_io16(0x0400_0050, 0x3F4C);
    b.write_io16(0x0400_0052, 0x0808);

    // DISPCNT = Mode 2 (0x2) | 1D OBJ (0x40) | BG2 (0x400) | BG3 (0x800) | OBJ (0x1000) | WIN0 (0x2000) | WIN1 (0x4000) | OBJWIN (0x8000) = 0xFC42
    b.write_io16(0x0400_0000, 0xFC42);

    b.mov_u32(8, 0); // r8 = frame counter
    b.label("main_loop");
    b.swi(0x05);
    b.add_imm(8, 8, 1);
    // Shift BG2X / BG3X smoothly (1 pixel/frame = 0x100 in 24.8 fixed point)
    b.mov_u32(0, 0x0400_0028);
    b.mov_lsl(2, 8, 8);
    b.str32(2, 0, 0);    // BG2X
    b.str32(2, 0, 0x10); // BG3X
    b.b("main_loop");

    // Re-patch entry branch to "start" instead of fixed word 48
    let start_word = b.labels["start"];
    let rel = (start_word as isize) - 2;
    b.words[0] = 0xEA00_0000 | ((rel as u32) & 0x00FF_FFFF);
    b.finalize()
}

/// 2) `stress_mode0_4bg_128spr.gba`
/// Mode 0: 4 full 8bpp scrolling BGs (64x64 tiles) + Mosaic + 128 overlapping sprites (Normal,
/// Semi-Transparent, OBJWIN) + WIN0/WIN1/OBJWIN + alternating Brightness Fade & Alpha Blend.
fn gen_stress_mode0_4bg_128spr() -> Vec<u8> {
    let mut b = ArmBuilder::new("STRESS_M0_4B", "AS04");
    b.emit_standard_subroutines();

    b.label("start");
    b.setup_irq_vblank();

    b.add_blob("pal_data", build_palette());
    b.add_blob("bg_tiles", build_8bpp_bg_tiles(64)); // 4096 bytes -> 0x0600_0000
    b.add_blob("reg_map", build_reg_bg_map_64x64(63)); // 8192 bytes -> 0x0600_8000
    b.add_blob("obj_tiles", build_4bpp_obj_tiles(16384));
    b.add_blob("oam_data", build_oam_128_double_aff(0, true));

    b.call_copy_blob("pal_data", 0x0500_0000, 1024);
    b.call_copy_blob("bg_tiles", 0x0600_0000, 4096);
    b.call_copy_blob("reg_map", 0x0600_8000, 8192);
    b.call_copy_blob("reg_map", 0x0600_A000, 8192);
    b.call_copy_blob("obj_tiles", 0x0601_0000, 16384);
    b.call_copy_blob("oam_data", 0x0700_0000, 1024);

    // BG0..BG3: 8bpp (0x80), mosaic (0x40), size=3 (64x64 -> 0xC000), screen_base 16..22, prios 0..3
    b.write_io16(0x0400_0008, 0xD0C0); // BG0: sb 16, prio 0
    b.write_io16(0x0400_000A, 0xD2C1); // BG1: sb 18, prio 1
    b.write_io16(0x0400_000C, 0xD4C2); // BG2: sb 20, prio 2
    b.write_io16(0x0400_000E, 0xD6C3); // BG3: sb 22, prio 3

    b.write_io16(0x0400_0040, (20 << 8) | 220);
    b.write_io16(0x0400_0042, (50 << 8) | 190);
    b.write_io16(0x0400_0044, (0 << 8) | 160);
    b.write_io16(0x0400_0046, (0 << 8) | 160);
    b.write_io16(0x0400_0048, 0x3F3F);
    b.write_io16(0x0400_004A, 0x3F3F);
    b.write_io16(0x0400_004C, 0x1111); // Mosaic 2x2
    // Brightness increase fade across all 4 BGs + OBJ + BD, BLDY = 6
    b.write_io16(0x0400_0050, 0x3FBF);
    b.write_io16(0x0400_0052, 0x0808);
    b.write_io16(0x0400_0054, 0x0006);

    // DISPCNT = Mode 0 | 1D OBJ (0x40) | BG0..3 (0x0F00) | OBJ (0x1000) | WIN0/1/OBJWIN (0xE000) = 0xFF40
    b.write_io16(0x0400_0000, 0xFF40);

    b.mov_u32(8, 0);
    b.label("main_loop");
    b.swi(0x05);
    b.add_imm(8, 8, 1);
    b.mov_u32(0, 0x0400_0010);
    b.strh(8, 0, 0x00); // BG0HOFS
    b.strh(8, 0, 0x04); // BG1HOFS
    b.strh(8, 0, 0x08); // BG2HOFS
    b.strh(8, 0, 0x0C); // BG3HOFS
    b.and_imm(1, 8, 0x0F);
    b.mov_u32(0, 0x0400_0054);
    b.strh(1, 0, 0); // BLDY sweep 0..15
    b.b("main_loop");

    let start_word = b.labels["start"];
    b.words[0] = 0xEA00_0000 | (((start_word as isize - 2) as u32) & 0x00FF_FFFF);
    b.finalize()
}

fn build_mode3_bitmap() -> Vec<u8> {
    // 240 x 160 RGB15 = 76,800 bytes
    let mut buf = Vec::with_capacity(240 * 160 * 2);
    for y in 0..160u16 {
        for x in 0..240u16 {
            let r = (x.wrapping_mul(3) + y) & 0x1F;
            let g = (y.wrapping_mul(5) ^ x) & 0x1F;
            let b = ((x ^ y).wrapping_mul(7)) & 0x1F;
            let px = (r | (g << 5) | (b << 10)).max(0x0421);
            buf.extend_from_slice(&px.to_le_bytes());
        }
    }
    buf
}

fn build_mode4_bitmap() -> Vec<u8> {
    // 240 x 160 8-bit palette indices = 38,400 bytes
    let mut buf = Vec::with_capacity(240 * 160);
    for y in 0..160usize {
        for x in 0..240usize {
            buf.push((((x * 3 + y * 5) % 255) + 1) as u8);
        }
    }
    buf
}

fn build_mode5_bitmap() -> Vec<u8> {
    // 160 x 128 RGB15 = 40,960 bytes
    let mut buf = Vec::with_capacity(160 * 128 * 2);
    for y in 0..128u16 {
        for x in 0..160u16 {
            let r = ((x ^ y) + 4) & 0x1F;
            let g = (y * 2 + 3) & 0x1F;
            let b = (x + 7) & 0x1F;
            let px = r | (g << 5) | (b << 10);
            buf.extend_from_slice(&px.to_le_bytes());
        }
    }
    buf
}

/// 3) `stress_bitmap_345.gba`
/// Switches cleanly through Mode 3 (0..200), Mode 4 (200..400), Mode 5 (400..600) with
/// non-identity affine scaling, Mosaic, Alpha Blend, WIN0/WIN1/OBJWIN, and 128 sprites (`tile >= 512`).
fn gen_stress_bitmap_345() -> Vec<u8> {
    let mut b = ArmBuilder::new("STRESS_BM345", "ASBM");
    b.emit_standard_subroutines();

    b.label("start");
    b.setup_irq_vblank();

    b.add_blob("pal_data", build_palette());
    b.add_blob("obj_tiles", build_4bpp_obj_tiles(16384)); // 0x0601_4000 (tile_num >= 512)
    b.add_blob("oam_data", build_oam_128_double_aff(512, true));
    b.add_blob("bm3_data", build_mode3_bitmap());
    b.add_blob("bm4_data", build_mode4_bitmap());
    b.add_blob("bm5_data", build_mode5_bitmap());

    b.call_copy_blob("pal_data", 0x0500_0000, 1024);
    b.call_copy_blob("obj_tiles", 0x0601_4000, 16384);
    b.call_copy_blob("oam_data", 0x0700_0000, 1024);

    b.write_io16(0x0400_000C, 0x0041); // BG2CNT: mosaic + prio 1
    b.write_io16(0x0400_0040, (24 << 8) | 216);
    b.write_io16(0x0400_0042, (48 << 8) | 192);
    b.write_io16(0x0400_0044, (0 << 8) | 160);
    b.write_io16(0x0400_0046, (0 << 8) | 160);
    b.write_io16(0x0400_0048, 0x3F3F);
    b.write_io16(0x0400_004A, 0x3F3F);
    b.write_io16(0x0400_004C, 0x1111);
    b.write_io16(0x0400_0050, 0x3F4C);
    b.write_io16(0x0400_0052, 0x0808);
    b.write_io16(0x0400_0020, 0x00E8);
    b.write_io16(0x0400_0022, 0x0018);
    b.write_io16(0x0400_0024, (-0x0018i16) as u16);
    b.write_io16(0x0400_0026, 0x00E8);

    b.mov_u32(8, 0); // frame counter = 0
    b.b("enter_mode3");

    b.label("main_loop");
    b.swi(0x05);
    b.add_imm(8, 8, 1);

    // Vary BG2X subtly by (frame & 15) pixels so Mode 5 (160x128) stays on screen while still changing
    b.and_imm(2, 8, 0x0F);
    b.mov_lsl(2, 2, 8);
    b.mov_u32(0, 0x0400_0028);
    b.str32(2, 0, 0);

    // Check mode switch thresholds: 200 -> Mode 4, 400 -> Mode 5, 600 -> reset 0 & Mode 3
    b.mov_u32(4, 200);
    b.cmp_reg(8, 4);
    b.beq("enter_mode4");
    b.mov_u32(4, 400);
    b.cmp_reg(8, 4);
    b.beq("enter_mode5");
    b.mov_u32(4, 600);
    b.cmp_reg(8, 4);
    b.bne("main_loop");
    b.mov_u32(8, 0);
    b.b("enter_mode3");

    b.label("enter_mode3");
    b.call_copy_blob("bm3_data", 0x0600_0000, 76_800);
    b.write_io16(0x0400_0020, 0x00E8);
    b.write_io16(0x0400_0026, 0x00E8);
    b.write_io16(0x0400_0000, 0xF443);
    b.b("main_loop");

    b.label("enter_mode4");
    b.call_copy_blob("bm4_data", 0x0600_0000, 38_400);
    b.write_io16(0x0400_0020, 0x00E0);
    b.write_io16(0x0400_0026, 0x00E0);
    b.write_io16(0x0400_0000, 0xF444);
    b.b("main_loop");

    b.label("enter_mode5");
    b.call_copy_blob("bm5_data", 0x0600_0000, 40_960);
    b.write_io16(0x0400_0020, 0x00AA);
    b.write_io16(0x0400_0026, 0x00CC);
    b.write_io16(0x0400_0000, 0xF445);
    b.b("main_loop");

    let start_word = b.labels["start"];
    b.words[0] = 0xEA00_0000 | (((start_word as isize - 2) as u32) & 0x00FF_FFFF);
    b.finalize()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = Path::new("roms/homebrew");
    fs::create_dir_all(out_dir)?;

    let aff = gen_stress_aff_objwin();
    fs::write(out_dir.join("stress_aff_objwin.gba"), &aff)?;

    let m0 = gen_stress_mode0_4bg_128spr();
    fs::write(out_dir.join("stress_mode0_4bg_128spr.gba"), &m0)?;

    let bm = gen_stress_bitmap_345();
    fs::write(out_dir.join("stress_bitmap_345.gba"), &bm)?;

    println!(
        "Generated stress ROMs in {} ({} / {} / {} bytes)",
        out_dir.display(),
        aff.len(),
        m0.len(),
        bm.len()
    );
    Ok(())
}
