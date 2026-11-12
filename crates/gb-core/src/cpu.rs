use serde::{Deserialize, Serialize};

use crate::bus::GbBus;

const FLAG_Z: u8 = 0x80;
const FLAG_N: u8 = 0x40;
const FLAG_H: u8 = 0x20;
const FLAG_C: u8 = 0x10;

/// Sharp SM83 CPU (Game Boy / Game Boy Color) with M-cycle-accurate bus access ordering.
#[derive(Clone, Serialize, Deserialize)]
pub struct Cpu {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,

    pub ime: bool,
    pub ime_delay: bool,
    pub halted: bool,
    pub halt_bug: bool,
    pub stopped: bool,
    pub locked: bool,
}

impl Cpu {
    pub fn new(boot_rom_present: bool, is_cgb: bool, gba_mode: bool, header_checksum: u8) -> Self {
        if boot_rom_present {
            return Self {
                a: 0,
                f: 0,
                b: 0,
                c: 0,
                d: 0,
                e: 0,
                h: 0,
                l: 0,
                sp: 0,
                pc: 0,
                ime: false,
                ime_delay: false,
                halted: false,
                halt_bug: false,
                stopped: false,
                locked: false,
            };
        }

        if is_cgb {
            Self {
                a: 0x11,
                f: 0x80,
                b: if gba_mode { 0x01 } else { 0x00 },
                c: 0x00,
                d: 0xFF,
                e: 0x56,
                h: 0x00,
                l: 0x0D,
                sp: 0xFFFE,
                pc: 0x0100,
                ime: false,
                ime_delay: false,
                halted: false,
                halt_bug: false,
                stopped: false,
                locked: false,
            }
        } else {
            Self {
                a: 0x01,
                f: if header_checksum == 0 { 0x80 } else { 0xB0 },
                b: 0x00,
                c: 0x13,
                d: 0x00,
                e: 0xD8,
                h: 0x01,
                l: 0x4D,
                sp: 0xFFFE,
                pc: 0x0100,
                ime: false,
                ime_delay: false,
                halted: false,
                halt_bug: false,
                stopped: false,
                locked: false,
            }
        }
    }

    #[inline]
    pub fn af(&self) -> u16 {
        ((self.a as u16) << 8) | ((self.f & 0xF0) as u16)
    }

    #[inline]
    pub fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = (v as u8) & 0xF0;
    }

    #[inline]
    pub fn bc(&self) -> u16 {
        ((self.b as u16) << 8) | (self.c as u16)
    }

    #[inline]
    pub fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }

    #[inline]
    pub fn de(&self) -> u16 {
        ((self.d as u16) << 8) | (self.e as u16)
    }

    #[inline]
    pub fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }

    #[inline]
    pub fn hl(&self) -> u16 {
        ((self.h as u16) << 8) | (self.l as u16)
    }

    #[inline]
    pub fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    #[inline]
    fn flag_z(&self) -> bool {
        (self.f & FLAG_Z) != 0
    }

    #[inline]
    fn flag_n(&self) -> bool {
        (self.f & FLAG_N) != 0
    }

    #[inline]
    fn flag_h(&self) -> bool {
        (self.f & FLAG_H) != 0
    }

    #[inline]
    fn flag_c(&self) -> bool {
        (self.f & FLAG_C) != 0
    }

    #[inline]
    fn set_flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = (if z { FLAG_Z } else { 0 })
            | (if n { FLAG_N } else { 0 })
            | (if h { FLAG_H } else { 0 })
            | (if c { FLAG_C } else { 0 });
    }

    #[inline]
    fn fetch8(&mut self, bus: &mut GbBus) -> u8 {
        let val = bus.read8(self.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.pc = self.pc.wrapping_add(1);
        }
        val
    }

    #[inline]
    fn fetch16(&mut self, bus: &mut GbBus) -> u16 {
        let lo = self.fetch8(bus) as u16;
        let hi = self.fetch8(bus) as u16;
        lo | (hi << 8)
    }

    fn read_r8(&mut self, bus: &mut GbBus, idx: u8) -> u8 {
        match idx & 7 {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => bus.read8(self.hl()),
            _ => self.a,
        }
    }

    fn write_r8(&mut self, bus: &mut GbBus, idx: u8, val: u8) {
        match idx & 7 {
            0 => self.b = val,
            1 => self.c = val,
            2 => self.d = val,
            3 => self.e = val,
            4 => self.h = val,
            5 => self.l = val,
            6 => bus.write8(self.hl(), val),
            _ => self.a = val,
        }
    }

    fn push16(&mut self, bus: &mut GbBus, val: u16) {
        self.sp = self.sp.wrapping_sub(1);
        bus.write8(self.sp, (val >> 8) as u8);
        self.sp = self.sp.wrapping_sub(1);
        bus.write8(self.sp, val as u8);
    }

    fn pop16(&mut self, bus: &mut GbBus) -> u16 {
        let lo = bus.read8(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        let hi = bus.read8(self.sp) as u16;
        self.sp = self.sp.wrapping_add(1);
        lo | (hi << 8)
    }

    #[inline]
    fn check_cc(&self, cc: u8) -> bool {
        match cc & 3 {
            0 => !self.flag_z(),
            1 => self.flag_z(),
            2 => !self.flag_c(),
            _ => self.flag_c(),
        }
    }

    /// Steps the CPU by one instruction (or one M-cycle if halted/stopped/locked/DMA-stalled).
    pub fn step(&mut self, bus: &mut GbBus) {
        if bus.hdma_stall_m_cycles > 0 {
            bus.hdma_stall_m_cycles -= 1;
            bus.tick_m_cycle();
            return;
        }

        if self.locked {
            bus.tick_m_cycle();
            return;
        }

        if self.stopped {
            if bus.joypad_wake || (bus.if_flag & bus.ie_flag & 0x1F) != 0 {
                self.stopped = false;
                bus.joypad_wake = false;
            } else {
                bus.tick_m_cycle();
                return;
            }
        }

        if self.halted {
            if (bus.ie_flag & bus.if_flag & 0x1F) != 0 {
                self.halted = false;
                bus.cpu_halted = false;
            } else {
                bus.tick_m_cycle();
                return;
            }
        }

        // Service pending maskable interrupt if IME is enabled.
        let pending = bus.ie_flag & bus.if_flag & 0x1F;
        if self.ime && pending != 0 {
            self.service_interrupt(bus);
            return;
        }

        // Promote delayed IME enable after the instruction following `EI`.
        if self.ime_delay {
            self.ime = true;
            self.ime_delay = false;
        }

        let op = self.fetch8(bus);
        self.exec(bus, op);
    }

    fn service_interrupt(&mut self, bus: &mut GbBus) {
        self.ime = false;
        self.ime_delay = false;

        bus.tick_m_cycle();
        bus.tick_m_cycle();

        self.sp = self.sp.wrapping_sub(1);
        bus.write8(self.sp, (self.pc >> 8) as u8);

        // Hardware quirk: IE is re-sampled after pushing PC high byte (in case SP == 0xFFFF).
        let pending = bus.ie_flag & bus.if_flag & 0x1F;
        let vector = if pending == 0 {
            0x0000
        } else {
            let bit = pending.trailing_zeros() as u8;
            bus.if_flag &= !(1 << bit);
            0x0040 + (bit as u16) * 8
        };

        self.sp = self.sp.wrapping_sub(1);
        bus.write8(self.sp, self.pc as u8);

        bus.tick_m_cycle();
        self.pc = vector;
    }

    fn alu_add(&mut self, val: u8, carry: bool) {
        let c = if carry && self.flag_c() { 1u16 } else { 0u16 };
        let a = self.a as u16;
        let v = val as u16;
        let sum = a + v + c;
        let h = (a & 0x0F) + (v & 0x0F) + c > 0x0F;
        let cf = sum > 0xFF;
        self.a = sum as u8;
        self.set_flags(self.a == 0, false, h, cf);
    }

    fn alu_sub(&mut self, val: u8, carry: bool, write_a: bool) {
        let c = if carry && self.flag_c() { 1u16 } else { 0u16 };
        let a = self.a as u16;
        let v = val as u16;
        let diff = a.wrapping_sub(v).wrapping_sub(c);
        let h = (a & 0x0F) < (v & 0x0F) + c;
        let cf = a < v + c;
        let res = diff as u8;
        if write_a {
            self.a = res;
        }
        self.set_flags(res == 0, true, h, cf);
    }

    fn alu_and(&mut self, val: u8) {
        self.a &= val;
        self.set_flags(self.a == 0, false, true, false);
    }

    fn alu_xor(&mut self, val: u8) {
        self.a ^= val;
        self.set_flags(self.a == 0, false, false, false);
    }

    fn alu_or(&mut self, val: u8) {
        self.a |= val;
        self.set_flags(self.a == 0, false, false, false);
    }

    fn add_hl(&mut self, bus: &mut GbBus, val: u16) {
        bus.tick_m_cycle();
        let hl = self.hl();
        let sum = (hl as u32) + (val as u32);
        let h = (hl & 0x0FFF) + (val & 0x0FFF) > 0x0FFF;
        let c = sum > 0xFFFF;
        self.set_hl(sum as u16);
        self.set_flags(self.flag_z(), false, h, c);
    }

    fn add_sp_signed(&mut self, offset: i8) -> u16 {
        let sp = self.sp;
        let e = offset as i16 as u16;
        let h = (sp & 0x0F) + (e & 0x0F) > 0x0F;
        let c = (sp & 0xFF) + (e & 0xFF) > 0xFF;
        self.set_flags(false, false, h, c);
        sp.wrapping_add(e)
    }

    fn exec(&mut self, bus: &mut GbBus, op: u8) {
        match op {
            0x00 => {} // NOP

            // 16-bit loads & ops
            0x01 => {
                let v = self.fetch16(bus);
                self.set_bc(v);
            }
            0x11 => {
                let v = self.fetch16(bus);
                self.set_de(v);
            }
            0x21 => {
                let v = self.fetch16(bus);
                self.set_hl(v);
            }
            0x31 => {
                self.sp = self.fetch16(bus);
            }
            0x08 => {
                let addr = self.fetch16(bus);
                bus.write8(addr, self.sp as u8);
                bus.write8(addr.wrapping_add(1), (self.sp >> 8) as u8);
            }
            0x02 => bus.write8(self.bc(), self.a),
            0x12 => bus.write8(self.de(), self.a),
            0x22 => {
                let hl = self.hl();
                bus.write8(hl, self.a);
                self.set_hl(hl.wrapping_add(1));
            }
            0x32 => {
                let hl = self.hl();
                bus.write8(hl, self.a);
                self.set_hl(hl.wrapping_sub(1));
            }
            0x0A => self.a = bus.read8(self.bc()),
            0x1A => self.a = bus.read8(self.de()),
            0x2A => {
                let hl = self.hl();
                self.a = bus.read8(hl);
                self.set_hl(hl.wrapping_add(1));
            }
            0x3A => {
                let hl = self.hl();
                self.a = bus.read8(hl);
                self.set_hl(hl.wrapping_sub(1));
            }

            // 16-bit INC/DEC/ADD HL
            0x03 => {
                bus.tick_m_cycle();
                self.set_bc(self.bc().wrapping_add(1));
            }
            0x13 => {
                bus.tick_m_cycle();
                self.set_de(self.de().wrapping_add(1));
            }
            0x23 => {
                bus.tick_m_cycle();
                self.set_hl(self.hl().wrapping_add(1));
            }
            0x33 => {
                bus.tick_m_cycle();
                self.sp = self.sp.wrapping_add(1);
            }
            0x0B => {
                bus.tick_m_cycle();
                self.set_bc(self.bc().wrapping_sub(1));
            }
            0x1B => {
                bus.tick_m_cycle();
                self.set_de(self.de().wrapping_sub(1));
            }
            0x2B => {
                bus.tick_m_cycle();
                self.set_hl(self.hl().wrapping_sub(1));
            }
            0x3B => {
                bus.tick_m_cycle();
                self.sp = self.sp.wrapping_sub(1);
            }
            0x09 => self.add_hl(bus, self.bc()),
            0x19 => self.add_hl(bus, self.de()),
            0x29 => self.add_hl(bus, self.hl()),
            0x39 => self.add_hl(bus, self.sp),

            // 8-bit INC/DEC
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
                let idx = (op >> 3) & 7;
                let v = self.read_r8(bus, idx);
                let res = v.wrapping_add(1);
                self.write_r8(bus, idx, res);
                self.set_flags(res == 0, false, (v & 0x0F) == 0x0F, self.flag_c());
            }
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
                let idx = (op >> 3) & 7;
                let v = self.read_r8(bus, idx);
                let res = v.wrapping_sub(1);
                self.write_r8(bus, idx, res);
                self.set_flags(res == 0, true, (v & 0x0F) == 0x00, self.flag_c());
            }

            // LD r, imm8
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
                let idx = (op >> 3) & 7;
                let imm = self.fetch8(bus);
                self.write_r8(bus, idx, imm);
            }

            // Accumulator rotates
            0x07 => {
                // RLCA
                let c = (self.a & 0x80) != 0;
                self.a = self.a.rotate_left(1);
                self.set_flags(false, false, false, c);
            }
            0x0F => {
                // RRCA
                let c = (self.a & 0x01) != 0;
                self.a = self.a.rotate_right(1);
                self.set_flags(false, false, false, c);
            }
            0x17 => {
                // RLA
                let old_c = if self.flag_c() { 1 } else { 0 };
                let c = (self.a & 0x80) != 0;
                self.a = (self.a << 1) | old_c;
                self.set_flags(false, false, false, c);
            }
            0x1F => {
                // RRA
                let old_c = if self.flag_c() { 0x80 } else { 0 };
                let c = (self.a & 0x01) != 0;
                self.a = (self.a >> 1) | old_c;
                self.set_flags(false, false, false, c);
            }

            // STOP
            0x10 => {
                let _ = self.fetch8(bus);
                if bus.is_cgb && bus.key1_arm {
                    bus.double_speed = !bus.double_speed;
                    bus.key1_arm = false;
                    bus.timer.reset_div_on_speed_switch();
                } else {
                    self.stopped = true;
                    bus.joypad_wake = false;
                    bus.timer.reset_div_on_speed_switch();
                }
            }

            // Jumps relative
            0x18 => {
                let e = self.fetch8(bus) as i8;
                bus.tick_m_cycle();
                self.pc = self.pc.wrapping_add(e as i16 as u16);
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let e = self.fetch8(bus) as i8;
                if self.check_cc((op >> 3) & 3) {
                    bus.tick_m_cycle();
                    self.pc = self.pc.wrapping_add(e as i16 as u16);
                }
            }

            // DAA, CPL, SCF, CCF
            0x27 => {
                let mut adj = 0u8;
                let mut carry = self.flag_c();
                if !self.flag_n() {
                    if self.flag_h() || (self.a & 0x0F) > 0x09 {
                        adj |= 0x06;
                    }
                    if carry || self.a > 0x99 {
                        adj |= 0x60;
                        carry = true;
                    }
                    self.a = self.a.wrapping_add(adj);
                } else {
                    if self.flag_h() {
                        adj |= 0x06;
                    }
                    if carry {
                        adj |= 0x60;
                    }
                    self.a = self.a.wrapping_sub(adj);
                }
                self.set_flags(self.a == 0, self.flag_n(), false, carry);
            }
            0x2F => {
                self.a = !self.a;
                self.set_flags(self.flag_z(), true, true, self.flag_c());
            }
            0x37 => {
                self.set_flags(self.flag_z(), false, false, true);
            }
            0x3F => {
                self.set_flags(self.flag_z(), false, false, !self.flag_c());
            }

            // HALT
            0x76 => {
                let pending = bus.ie_flag & bus.if_flag & 0x1F;
                if pending != 0 {
                    if !self.ime {
                        self.halt_bug = true;
                    }
                } else {
                    self.halted = true;
                    bus.cpu_halted = true;
                }
            }

            // LD r, r'
            0x40..=0x7F => {
                let dst = (op >> 3) & 7;
                let src = op & 7;
                let val = self.read_r8(bus, src);
                self.write_r8(bus, dst, val);
            }

            // ALU A, r
            0x80..=0x87 => {
                let v = self.read_r8(bus, op);
                self.alu_add(v, false);
            }
            0x88..=0x8F => {
                let v = self.read_r8(bus, op);
                self.alu_add(v, true);
            }
            0x90..=0x97 => {
                let v = self.read_r8(bus, op);
                self.alu_sub(v, false, true);
            }
            0x98..=0x9F => {
                let v = self.read_r8(bus, op);
                self.alu_sub(v, true, true);
            }
            0xA0..=0xA7 => {
                let v = self.read_r8(bus, op);
                self.alu_and(v);
            }
            0xA8..=0xAF => {
                let v = self.read_r8(bus, op);
                self.alu_xor(v);
            }
            0xB0..=0xB7 => {
                let v = self.read_r8(bus, op);
                self.alu_or(v);
            }
            0xB8..=0xBF => {
                let v = self.read_r8(bus, op);
                self.alu_sub(v, false, false);
            }

            // RET cc / RET / RETI
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                bus.tick_m_cycle();
                if self.check_cc((op >> 3) & 3) {
                    let addr = self.pop16(bus);
                    bus.tick_m_cycle();
                    self.pc = addr;
                }
            }
            0xC9 => {
                let addr = self.pop16(bus);
                bus.tick_m_cycle();
                self.pc = addr;
            }
            0xD9 => {
                let addr = self.pop16(bus);
                bus.tick_m_cycle();
                self.pc = addr;
                self.ime = true;
                self.ime_delay = false;
            }

            // POP / PUSH
            0xC1 => {
                let v = self.pop16(bus);
                self.set_bc(v);
            }
            0xD1 => {
                let v = self.pop16(bus);
                self.set_de(v);
            }
            0xE1 => {
                let v = self.pop16(bus);
                self.set_hl(v);
            }
            0xF1 => {
                let v = self.pop16(bus);
                self.set_af(v);
            }
            0xC5 => {
                bus.tick_m_cycle();
                self.push16(bus, self.bc());
            }
            0xD5 => {
                bus.tick_m_cycle();
                self.push16(bus, self.de());
            }
            0xE5 => {
                bus.tick_m_cycle();
                self.push16(bus, self.hl());
            }
            0xF5 => {
                bus.tick_m_cycle();
                self.push16(bus, self.af());
            }

            // JP
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let addr = self.fetch16(bus);
                if self.check_cc((op >> 3) & 3) {
                    bus.tick_m_cycle();
                    self.pc = addr;
                }
            }
            0xC3 => {
                let addr = self.fetch16(bus);
                bus.tick_m_cycle();
                self.pc = addr;
            }
            0xE9 => self.pc = self.hl(),

            // CALL
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let addr = self.fetch16(bus);
                if self.check_cc((op >> 3) & 3) {
                    bus.tick_m_cycle();
                    self.push16(bus, self.pc);
                    self.pc = addr;
                }
            }
            0xCD => {
                let addr = self.fetch16(bus);
                bus.tick_m_cycle();
                self.push16(bus, self.pc);
                self.pc = addr;
            }

            // RST
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                bus.tick_m_cycle();
                self.push16(bus, self.pc);
                self.pc = (op & 0x38) as u16;
            }

            // ALU A, imm8
            0xC6 => {
                let v = self.fetch8(bus);
                self.alu_add(v, false);
            }
            0xCE => {
                let v = self.fetch8(bus);
                self.alu_add(v, true);
            }
            0xD6 => {
                let v = self.fetch8(bus);
                self.alu_sub(v, false, true);
            }
            0xDE => {
                let v = self.fetch8(bus);
                self.alu_sub(v, true, true);
            }
            0xE6 => {
                let v = self.fetch8(bus);
                self.alu_and(v);
            }
            0xEE => {
                let v = self.fetch8(bus);
                self.alu_xor(v);
            }
            0xF6 => {
                let v = self.fetch8(bus);
                self.alu_or(v);
            }
            0xFE => {
                let v = self.fetch8(bus);
                self.alu_sub(v, false, false);
            }

            // High-page / memory / SP ops
            0xE0 => {
                let n = self.fetch8(bus) as u16;
                bus.write8(0xFF00 | n, self.a);
            }
            0xF0 => {
                let n = self.fetch8(bus) as u16;
                self.a = bus.read8(0xFF00 | n);
            }
            0xE2 => bus.write8(0xFF00 | (self.c as u16), self.a),
            0xF2 => self.a = bus.read8(0xFF00 | (self.c as u16)),
            0xEA => {
                let addr = self.fetch16(bus);
                bus.write8(addr, self.a);
            }
            0xFA => {
                let addr = self.fetch16(bus);
                self.a = bus.read8(addr);
            }
            0xE8 => {
                let e = self.fetch8(bus) as i8;
                bus.tick_m_cycle();
                bus.tick_m_cycle();
                self.sp = self.add_sp_signed(e);
            }
            0xF8 => {
                let e = self.fetch8(bus) as i8;
                bus.tick_m_cycle();
                let res = self.add_sp_signed(e);
                self.set_hl(res);
            }
            0xF9 => {
                bus.tick_m_cycle();
                self.sp = self.hl();
            }

            // DI / EI
            0xF3 => {
                self.ime = false;
                self.ime_delay = false;
            }
            0xFB => {
                self.ime_delay = true;
            }

            // CB prefix
            0xCB => {
                let cb = self.fetch8(bus);
                self.exec_cb(bus, cb);
            }

            // Illegal opcodes freeze CPU on real hardware
            0xD3 | 0xDB | 0xDD | 0xE3 | 0xE4 | 0xEB | 0xEC | 0xED | 0xF4 | 0xFC | 0xFD => {
                self.locked = true;
            }
        }
    }

    fn exec_cb(&mut self, bus: &mut GbBus, cb: u8) {
        let reg = cb & 7;
        let val = self.read_r8(bus, reg);

        match cb >> 6 {
            0 => {
                let res = match (cb >> 3) & 7 {
                    0 => {
                        // RLC
                        let c = (val & 0x80) != 0;
                        let r = val.rotate_left(1);
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    1 => {
                        // RRC
                        let c = (val & 0x01) != 0;
                        let r = val.rotate_right(1);
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    2 => {
                        // RL
                        let old_c = if self.flag_c() { 1 } else { 0 };
                        let c = (val & 0x80) != 0;
                        let r = (val << 1) | old_c;
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    3 => {
                        // RR
                        let old_c = if self.flag_c() { 0x80 } else { 0 };
                        let c = (val & 0x01) != 0;
                        let r = (val >> 1) | old_c;
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    4 => {
                        // SLA
                        let c = (val & 0x80) != 0;
                        let r = val << 1;
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    5 => {
                        // SRA
                        let c = (val & 0x01) != 0;
                        let r = ((val as i8) >> 1) as u8;
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                    6 => {
                        // SWAP
                        let r = val.rotate_left(4);
                        self.set_flags(r == 0, false, false, false);
                        r
                    }
                    _ => {
                        // SRL
                        let c = (val & 0x01) != 0;
                        let r = val >> 1;
                        self.set_flags(r == 0, false, false, c);
                        r
                    }
                };
                self.write_r8(bus, reg, res);
            }
            1 => {
                // BIT b, r (does not write back!)
                let bit = (cb >> 3) & 7;
                let z = (val & (1 << bit)) == 0;
                self.set_flags(z, false, true, self.flag_c());
            }
            2 => {
                // RES b, r
                let bit = (cb >> 3) & 7;
                self.write_r8(bus, reg, val & !(1 << bit));
            }
            _ => {
                // SET b, r
                let bit = (cb >> 3) & 7;
                self.write_r8(bus, reg, val | (1 << bit));
            }
        }
    }
}
