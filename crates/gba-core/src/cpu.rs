use serde::{Deserialize, Serialize};

use crate::bus::{AccessType, SysBus};

pub const CPSR_N: u32 = 1 << 31;
pub const CPSR_Z: u32 = 1 << 30;
pub const CPSR_C: u32 = 1 << 29;
pub const CPSR_V: u32 = 1 << 28;
pub const CPSR_I: u32 = 1 << 7;
pub const CPSR_F: u32 = 1 << 6;
pub const CPSR_T: u32 = 1 << 5;
pub const CPSR_MODE_MASK: u32 = 0x1F;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CpuMode {
    User = 0x10,
    Fiq = 0x11,
    Irq = 0x12,
    Supervisor = 0x13,
    Abort = 0x17,
    Undefined = 0x1B,
    System = 0x1F,
}

impl CpuMode {
    #[inline]
    pub fn from_cpsr(cpsr: u32) -> Self {
        match cpsr & CPSR_MODE_MASK {
            0x10 => Self::User,
            0x11 => Self::Fiq,
            0x12 => Self::Irq,
            0x13 => Self::Supervisor,
            0x17 => Self::Abort,
            0x1B => Self::Undefined,
            0x1F => Self::System,
            _ => Self::System,
        }
    }

    #[inline]
    pub const fn bank_idx(self) -> usize {
        match self {
            Self::User | Self::System => 0,
            Self::Fiq => 1,
            Self::Irq => 2,
            Self::Supervisor => 3,
            Self::Abort => 4,
            Self::Undefined => 5,
        }
    }
}

type ArmFn = fn(&mut Arm7tdmi, &mut SysBus, u32) -> u64;
type ThumbFn = fn(&mut Arm7tdmi, &mut SysBus, u16) -> u64;

static ARM_LUT: [ArmFn; 4096] = build_arm_lut();
static THUMB_LUT: [ThumbFn; 1024] = build_thumb_lut();

#[derive(Clone, Serialize, Deserialize)]
pub struct Arm7tdmi {
    pub gpr: [u32; 16],
    pub cpsr: u32,
    pub banked_r8_12_usr: [u32; 5],
    pub banked_r8_12_fiq: [u32; 5],
    pub banked_r13: [u32; 6],
    pub banked_r14: [u32; 6],
    pub spsr_banked: [u32; 6],
    pub pipeline: [u32; 2],
    pub halted: bool,
    #[serde(default)]
    pub stopped: bool,
    pub intr_wait: Option<u16>,
    #[serde(default)]
    pub pipeline_flushed: bool,
    #[serde(default)]
    pub flush_cycles: u64,
}

impl Arm7tdmi {
    pub fn new(bus: &mut SysBus) -> Self {
        let mut cpu = Self {
            gpr: [0; 16],
            cpsr: if bus.bios_present { 0x0000_00D3 } else { 0x0000_001F },
            banked_r8_12_usr: [0; 5],
            banked_r8_12_fiq: [0; 5],
            banked_r13: [0x0300_7F00, 0, 0x0300_7FA0, 0x0300_7FE0, 0, 0],
            banked_r14: [0; 6],
            spsr_banked: [0; 6],
            pipeline: [0; 2],
            halted: false,
            stopped: false,
            intr_wait: None,
            pipeline_flushed: false,
            flush_cycles: 0,
        };

        if bus.bios_present {
            cpu.gpr[13] = 0x0300_7FE0; // SVC initial SP (bios initializes it too)
            cpu.flush_pipeline(bus, 0x0000_0000);
        } else {
            cpu.gpr[13] = 0x0300_7F00; // SYS/USR SP
            bus.hw.postflg = 1;
            bus.apu.soundbias = 0x0200;
            cpu.flush_pipeline(bus, 0x0800_0000);
        }
        cpu.flush_cycles = 0;
        cpu.pipeline_flushed = false;
        cpu
    }

    #[inline]
    pub fn is_thumb(&self) -> bool {
        (self.cpsr & CPSR_T) != 0
    }

    /// Address of the instruction currently at the execute stage (PC - 4 in Thumb, PC - 8 in ARM).
    #[inline]
    pub fn current_exec_pc(&self) -> u32 {
        if self.is_thumb() {
            self.gpr[15].wrapping_sub(4) & !1
        } else {
            self.gpr[15].wrapping_sub(8) & !3
        }
    }

    #[inline]
    pub fn is_irq_disabled(&self) -> bool {
        (self.cpsr & CPSR_I) != 0
    }

    #[inline]
    pub fn mode(&self) -> CpuMode {
        CpuMode::from_cpsr(self.cpsr)
    }

    #[inline]
    pub fn carry_flag(&self) -> bool {
        (self.cpsr & CPSR_C) != 0
    }

    pub fn set_cpsr(&mut self, new_cpsr: u32) {
        let old_mode = self.mode();
        let new_mode = CpuMode::from_cpsr(new_cpsr);
        if old_mode != new_mode {
            let old_bank = old_mode.bank_idx();
            let new_bank = new_mode.bank_idx();

            self.banked_r13[old_bank] = self.gpr[13];
            self.banked_r14[old_bank] = self.gpr[14];

            if old_mode == CpuMode::Fiq {
                self.banked_r8_12_fiq.copy_from_slice(&self.gpr[8..13]);
                self.gpr[8..13].copy_from_slice(&self.banked_r8_12_usr);
            } else if new_mode == CpuMode::Fiq {
                self.banked_r8_12_usr.copy_from_slice(&self.gpr[8..13]);
                self.gpr[8..13].copy_from_slice(&self.banked_r8_12_fiq);
            }

            self.gpr[13] = self.banked_r13[new_bank];
            self.gpr[14] = self.banked_r14[new_bank];
        }
        self.cpsr = new_cpsr;
    }

    #[inline]
    pub fn get_spsr(&self) -> u32 {
        let m = self.mode();
        if m == CpuMode::User || m == CpuMode::System {
            self.cpsr
        } else {
            self.spsr_banked[m.bank_idx()]
        }
    }

    #[inline]
    pub fn set_spsr(&mut self, val: u32) {
        let m = self.mode();
        if m != CpuMode::User && m != CpuMode::System {
            self.spsr_banked[m.bank_idx()] = val;
        }
    }

    #[inline]
    pub fn set_nz(&mut self, res: u32) {
        self.cpsr = (self.cpsr & !(CPSR_N | CPSR_Z))
            | (res & CPSR_N)
            | if res == 0 { CPSR_Z } else { 0 };
    }

    #[inline]
    pub fn set_nzc(&mut self, res: u32, carry: bool) {
        self.cpsr = (self.cpsr & !(CPSR_N | CPSR_Z | CPSR_C))
            | (res & CPSR_N)
            | if res == 0 { CPSR_Z } else { 0 }
            | if carry { CPSR_C } else { 0 };
    }

    #[inline]
    pub fn set_nzcv(&mut self, res: u32, carry: bool, overflow: bool) {
        self.cpsr = (self.cpsr & !(CPSR_N | CPSR_Z | CPSR_C | CPSR_V))
            | (res & CPSR_N)
            | if res == 0 { CPSR_Z } else { 0 }
            | if carry { CPSR_C } else { 0 }
            | if overflow { CPSR_V } else { 0 };
    }

    #[inline]
    pub fn check_cond(&self, cond: u32) -> bool {
        let n = (self.cpsr & CPSR_N) != 0;
        let z = (self.cpsr & CPSR_Z) != 0;
        let c = (self.cpsr & CPSR_C) != 0;
        let v = (self.cpsr & CPSR_V) != 0;
        match cond {
            0x0 => z,
            0x1 => !z,
            0x2 => c,
            0x3 => !c,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => c && !z,
            0x9 => !c || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && (n == v),
            0xD => z || (n != v),
            0xE => true,
            _ => false,
        }
    }

    pub fn flush_pipeline(&mut self, bus: &mut SysBus, target_addr: u32) {
        self.pipeline_flushed = true;
        if self.is_thumb() {
            let pc = target_addr & !1;
            let (p0, c0) = bus.fetch_16(pc, AccessType::NonSequential);
            let (p1, c1) = bus.fetch_16(pc.wrapping_add(2), AccessType::Sequential);
            self.pipeline[0] = p0 as u32;
            self.pipeline[1] = p1 as u32;
            self.gpr[15] = pc.wrapping_add(4);
            self.flush_cycles += c0 + c1;
        } else {
            let pc = target_addr & !3;
            let (p0, c0) = bus.fetch_32(pc, AccessType::NonSequential);
            let (p1, c1) = bus.fetch_32(pc.wrapping_add(4), AccessType::Sequential);
            self.pipeline[0] = p0;
            self.pipeline[1] = p1;
            self.gpr[15] = pc.wrapping_add(8);
            self.flush_cycles += c0 + c1;
        }
    }

    pub fn trigger_irq(&mut self, bus: &mut SysBus) {
        self.halted = false;
        self.stopped = false;
        let lr_irq = if self.is_thumb() {
            self.gpr[15] // A + 4
        } else {
            self.gpr[15].wrapping_sub(4) // A + 4
        };
        let old_cpsr = self.cpsr;
        self.set_cpsr((old_cpsr & !(CPSR_MODE_MASK | CPSR_T)) | (CpuMode::Irq as u32) | CPSR_I);
        self.spsr_banked[CpuMode::Irq.bank_idx()] = old_cpsr;
        self.gpr[14] = lr_irq;
        self.flush_cycles = 0;
        self.flush_pipeline(bus, 0x0000_0018);
    }

    pub fn enter_undefined(&mut self, bus: &mut SysBus, was_thumb: bool) {
        let lr_und = if was_thumb {
            self.gpr[15].wrapping_sub(2)
        } else {
            self.gpr[15].wrapping_sub(4)
        };
        let old_cpsr = self.cpsr;
        self.set_cpsr(
            (old_cpsr & !(CPSR_MODE_MASK | CPSR_T)) | (CpuMode::Undefined as u32) | CPSR_I,
        );
        self.spsr_banked[CpuMode::Undefined.bank_idx()] = old_cpsr;
        self.gpr[14] = lr_und;
        self.flush_pipeline(bus, 0x0000_0004);
    }

    pub fn return_from_swi(&mut self, bus: &mut SysBus) {
        if !bus.bios_present {
            bus.bios_latch = 0xE3A0_2004;
        }
        let ret = self.gpr[14];
        let spsr = self.get_spsr();
        self.set_cpsr(spsr);
        self.flush_pipeline(bus, ret);
    }

    #[inline]
    pub fn step(&mut self, bus: &mut SysBus) -> u64 {
        let was_thumb = self.is_thumb();
        self.pipeline_flushed = false;
        self.flush_cycles = 0;

        let instr = self.pipeline[0];
        self.pipeline[0] = self.pipeline[1];

        let fetch_cyc = if was_thumb {
            let (next_h, c) = bus.fetch_16(self.gpr[15], AccessType::Sequential);
            self.pipeline[1] = next_h as u32;
            c
        } else {
            let (next_w, c) = bus.fetch_32(self.gpr[15], AccessType::Sequential);
            self.pipeline[1] = next_w;
            c
        };

        let exec_cyc = if was_thumb {
            let lut_idx = ((instr >> 6) & 0x3FF) as usize;
            THUMB_LUT[lut_idx](self, bus, instr as u16)
        } else {
            let cond = (instr >> 28) & 0xF;
            if !self.check_cond(cond) {
                self.gpr[15] = self.gpr[15].wrapping_add(4);
                return fetch_cyc;
            }
            let lut_idx = (((instr >> 16) & 0xFF0) | ((instr >> 4) & 0x00F)) as usize;
            ARM_LUT[lut_idx](self, bus, instr)
        };

        if !self.pipeline_flushed {
            self.gpr[15] = self.gpr[15].wrapping_add(if was_thumb { 2 } else { 4 });
        }

        fetch_cyc + exec_cyc + self.flush_cycles
    }

    fn read_usr_reg(&self, r: usize) -> u32 {
        match r {
            0..=7 | 15 => self.gpr[r],
            8..=12 => {
                if self.mode() == CpuMode::Fiq {
                    self.banked_r8_12_usr[r - 8]
                } else {
                    self.gpr[r]
                }
            }
            13 => {
                if matches!(self.mode(), CpuMode::User | CpuMode::System) {
                    self.gpr[13]
                } else {
                    self.banked_r13[0]
                }
            }
            14 => {
                if matches!(self.mode(), CpuMode::User | CpuMode::System) {
                    self.gpr[14]
                } else {
                    self.banked_r14[0]
                }
            }
            _ => 0,
        }
    }

    fn write_usr_reg(&mut self, r: usize, val: u32) {
        match r {
            0..=7 => self.gpr[r] = val,
            8..=12 => {
                if self.mode() == CpuMode::Fiq {
                    self.banked_r8_12_usr[r - 8] = val;
                } else {
                    self.gpr[r] = val;
                }
            }
            13 => {
                if matches!(self.mode(), CpuMode::User | CpuMode::System) {
                    self.gpr[13] = val;
                } else {
                    self.banked_r13[0] = val;
                }
            }
            14 => {
                if matches!(self.mode(), CpuMode::User | CpuMode::System) {
                    self.gpr[14] = val;
                } else {
                    self.banked_r14[0] = val;
                }
            }
            _ => {}
        }
    }

    #[inline]
    fn barrel_shift(&self, val: u32, shift_type: u32, amount: u32, is_reg_shift: bool) -> (u32, bool) {
        let c_in = self.carry_flag();
        if is_reg_shift && amount == 0 {
            return (val, c_in);
        }
        match shift_type & 3 {
            0 => {
                // LSL
                if amount == 0 {
                    (val, c_in)
                } else if amount < 32 {
                    (val << amount, ((val >> (32 - amount)) & 1) != 0)
                } else if amount == 32 {
                    (0, (val & 1) != 0)
                } else {
                    (0, false)
                }
            }
            1 => {
                // LSR
                let amt = if !is_reg_shift && amount == 0 { 32 } else { amount };
                if amt < 32 {
                    (val >> amt, ((val >> (amt - 1)) & 1) != 0)
                } else if amt == 32 {
                    (0, (val >> 31) != 0)
                } else {
                    (0, false)
                }
            }
            2 => {
                // ASR
                let amt = if !is_reg_shift && amount == 0 { 32 } else { amount };
                if amt < 32 {
                    (((val as i32) >> amt) as u32, ((val >> (amt - 1)) & 1) != 0)
                } else {
                    let sign = (val >> 31) != 0;
                    (if sign { u32::MAX } else { 0 }, sign)
                }
            }
            _ => {
                // ROR / RRX
                if !is_reg_shift && amount == 0 {
                    // RRX #1
                    let res = ((c_in as u32) << 31) | (val >> 1);
                    (res, (val & 1) != 0)
                } else {
                    let rot = amount & 31;
                    let res = val.rotate_right(rot);
                    (res, (res >> 31) != 0)
                }
            }
        }
    }

    #[inline]
    fn alu_add(lhs: u32, rhs: u32, carry_in: u32) -> (u32, bool, bool) {
        let sum64 = (lhs as u64) + (rhs as u64) + (carry_in as u64);
        let res = sum64 as u32;
        let carry = (sum64 >> 32) != 0;
        let overflow = ((lhs ^ res) & (rhs ^ res)) >> 31 != 0;
        (res, carry, overflow)
    }

    #[inline]
    fn booth_cycles(rs: u32, signed: bool) -> u64 {
        let mut mask = 0xFFFF_FF00u32;
        for cycles in 1..=3 {
            let masked = rs & mask;
            if masked == 0 || (signed && masked == mask) {
                return cycles;
            }
            mask <<= 8;
        }
        4
    }

    #[inline]
    fn bus_read_32_rot(&self, bus: &mut SysBus, addr: u32, access: AccessType, is_thumb: bool) -> (u32, u64) {
        let raw = bus.read_32(addr, access, self.gpr[15]);
        let cyc = bus.last_cycles;
        let rot = (addr & 3) * 8;
        let _ = is_thumb;
        (raw.rotate_right(rot), cyc)
    }
}

macro_rules! lut_4 {
    ($fn:ident) => {
        [$fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>]
    };
}

macro_rules! lut_8 {
    ($fn:ident) => {
        [
            $fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>, $fn::<4>, $fn::<5>, $fn::<6>, $fn::<7>,
        ]
    };
}

macro_rules! lut_16 {
    ($fn:ident) => {
        [
            $fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>, $fn::<4>, $fn::<5>, $fn::<6>, $fn::<7>,
            $fn::<8>, $fn::<9>, $fn::<10>, $fn::<11>, $fn::<12>, $fn::<13>, $fn::<14>, $fn::<15>,
        ]
    };
}

macro_rules! lut_32 {
    ($fn:ident) => {
        [
            $fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>, $fn::<4>, $fn::<5>, $fn::<6>, $fn::<7>,
            $fn::<8>, $fn::<9>, $fn::<10>, $fn::<11>, $fn::<12>, $fn::<13>, $fn::<14>, $fn::<15>,
            $fn::<16>, $fn::<17>, $fn::<18>, $fn::<19>, $fn::<20>, $fn::<21>, $fn::<22>, $fn::<23>,
            $fn::<24>, $fn::<25>, $fn::<26>, $fn::<27>, $fn::<28>, $fn::<29>, $fn::<30>, $fn::<31>,
        ]
    };
}

macro_rules! lut_64 {
    ($fn:ident) => {
        [
            $fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>, $fn::<4>, $fn::<5>, $fn::<6>, $fn::<7>,
            $fn::<8>, $fn::<9>, $fn::<10>, $fn::<11>, $fn::<12>, $fn::<13>, $fn::<14>, $fn::<15>,
            $fn::<16>, $fn::<17>, $fn::<18>, $fn::<19>, $fn::<20>, $fn::<21>, $fn::<22>, $fn::<23>,
            $fn::<24>, $fn::<25>, $fn::<26>, $fn::<27>, $fn::<28>, $fn::<29>, $fn::<30>, $fn::<31>,
            $fn::<32>, $fn::<33>, $fn::<34>, $fn::<35>, $fn::<36>, $fn::<37>, $fn::<38>, $fn::<39>,
            $fn::<40>, $fn::<41>, $fn::<42>, $fn::<43>, $fn::<44>, $fn::<45>, $fn::<46>, $fn::<47>,
            $fn::<48>, $fn::<49>, $fn::<50>, $fn::<51>, $fn::<52>, $fn::<53>, $fn::<54>, $fn::<55>,
            $fn::<56>, $fn::<57>, $fn::<58>, $fn::<59>, $fn::<60>, $fn::<61>, $fn::<62>, $fn::<63>,
        ]
    };
}

macro_rules! lut_128 {
    ($fn:ident) => {
        [
            $fn::<0>, $fn::<1>, $fn::<2>, $fn::<3>, $fn::<4>, $fn::<5>, $fn::<6>, $fn::<7>,
            $fn::<8>, $fn::<9>, $fn::<10>, $fn::<11>, $fn::<12>, $fn::<13>, $fn::<14>, $fn::<15>,
            $fn::<16>, $fn::<17>, $fn::<18>, $fn::<19>, $fn::<20>, $fn::<21>, $fn::<22>, $fn::<23>,
            $fn::<24>, $fn::<25>, $fn::<26>, $fn::<27>, $fn::<28>, $fn::<29>, $fn::<30>, $fn::<31>,
            $fn::<32>, $fn::<33>, $fn::<34>, $fn::<35>, $fn::<36>, $fn::<37>, $fn::<38>, $fn::<39>,
            $fn::<40>, $fn::<41>, $fn::<42>, $fn::<43>, $fn::<44>, $fn::<45>, $fn::<46>, $fn::<47>,
            $fn::<48>, $fn::<49>, $fn::<50>, $fn::<51>, $fn::<52>, $fn::<53>, $fn::<54>, $fn::<55>,
            $fn::<56>, $fn::<57>, $fn::<58>, $fn::<59>, $fn::<60>, $fn::<61>, $fn::<62>, $fn::<63>,
            $fn::<64>, $fn::<65>, $fn::<66>, $fn::<67>, $fn::<68>, $fn::<69>, $fn::<70>, $fn::<71>,
            $fn::<72>, $fn::<73>, $fn::<74>, $fn::<75>, $fn::<76>, $fn::<77>, $fn::<78>, $fn::<79>,
            $fn::<80>, $fn::<81>, $fn::<82>, $fn::<83>, $fn::<84>, $fn::<85>, $fn::<86>, $fn::<87>,
            $fn::<88>, $fn::<89>, $fn::<90>, $fn::<91>, $fn::<92>, $fn::<93>, $fn::<94>, $fn::<95>,
            $fn::<96>, $fn::<97>, $fn::<98>, $fn::<99>, $fn::<100>, $fn::<101>, $fn::<102>,
            $fn::<103>, $fn::<104>, $fn::<105>, $fn::<106>, $fn::<107>, $fn::<108>, $fn::<109>,
            $fn::<110>, $fn::<111>, $fn::<112>, $fn::<113>, $fn::<114>, $fn::<115>, $fn::<116>,
            $fn::<117>, $fn::<118>, $fn::<119>, $fn::<120>, $fn::<121>, $fn::<122>, $fn::<123>,
            $fn::<124>, $fn::<125>, $fn::<126>, $fn::<127>,
        ]
    };
}

const ARM_BLOCK_FNS: [ArmFn; 32] = lut_32!(arm_block_transfer);
const ARM_SINGLE_FNS: [ArmFn; 64] = lut_64!(arm_single_transfer);
const ARM_HALFWORD_FNS: [ArmFn; 128] = lut_128!(arm_halfword_transfer);
const ARM_DATA_PROC_FNS: [ArmFn; 128] = lut_128!(arm_data_proc);
const ARM_MUL_FNS: [ArmFn; 4] = lut_4!(arm_mul);
const ARM_MULL_FNS: [ArmFn; 8] = lut_8!(arm_mull);
const ARM_PSR_REG_FNS: [ArmFn; 4] = lut_4!(arm_psr_reg);

const THUMB_MOVE_SHIFTED_FNS: [ThumbFn; 4] = lut_4!(thumb_move_shifted);
const THUMB_ADD_SUB_FNS: [ThumbFn; 4] = lut_4!(thumb_add_sub);
const THUMB_ALU_IMM_FNS: [ThumbFn; 4] = lut_4!(thumb_alu_imm);
const THUMB_ALU_REG_FNS: [ThumbFn; 16] = lut_16!(thumb_alu_reg);
const THUMB_HI_REG_FNS: [ThumbFn; 16] = lut_16!(thumb_hi_reg_bx);
const THUMB_LDR_STR_REG_FNS: [ThumbFn; 4] = lut_4!(thumb_ldr_str_reg);
const THUMB_LDR_STR_HALF_SIGN_FNS: [ThumbFn; 4] = lut_4!(thumb_ldr_str_half_sign);
const THUMB_LDR_STR_IMM_FNS: [ThumbFn; 4] = lut_4!(thumb_ldr_str_imm);
const THUMB_PUSH_POP_FNS: [ThumbFn; 4] = lut_4!(thumb_push_pop);
const THUMB_COND_BRANCH_FNS: [ThumbFn; 16] = lut_16!(thumb_cond_branch);

const fn build_arm_lut() -> [ArmFn; 4096] {
    let mut lut: [ArmFn; 4096] = [arm_undefined; 4096];
    let mut i = 0usize;
    while i < 4096 {
        let hi = (i >> 4) & 0xFF;
        let lo = i & 0x0F;
        lut[i] = if (hi & 0xF0) == 0xF0 {
            arm_swi
        } else if (hi & 0xC0) == 0xC0 {
            arm_undefined
        } else if (hi & 0xE0) == 0xA0 {
            if (hi & 0x10) != 0 {
                arm_branch::<true>
            } else {
                arm_branch::<false>
            }
        } else if (hi & 0xE0) == 0x80 {
            ARM_BLOCK_FNS[hi & 0x1F]
        } else if (hi & 0xE0) == 0x60 && (lo & 0x01) != 0 {
            arm_undefined
        } else if (hi & 0xC0) == 0x40 {
            ARM_SINGLE_FNS[hi & 0x3F]
        } else if hi == 0x12 && lo == 0x1 {
            arm_bx
        } else if (hi & 0xFC) == 0x00 && lo == 0x9 {
            ARM_MUL_FNS[hi & 0x03]
        } else if (hi & 0xF8) == 0x08 && lo == 0x9 {
            ARM_MULL_FNS[hi & 0x07]
        } else if (hi & 0xFB) == 0x10 && lo == 0x9 {
            if (hi & 0x04) != 0 {
                arm_swp::<true>
            } else {
                arm_swp::<false>
            }
        } else if (hi & 0x20) == 0 && (lo & 0x9) == 0x9 && lo != 0x9 {
            ARM_HALFWORD_FNS[((hi & 0x1F) << 2) | ((lo >> 1) & 0x03)]
        } else if (hi & 0xFB) == 0x32 {
            if (hi & 0x04) != 0 {
                arm_msr_imm::<true>
            } else {
                arm_msr_imm::<false>
            }
        } else if (hi & 0xD9) == 0x10 && lo == 0x0 {
            ARM_PSR_REG_FNS[(hi >> 1) & 0x03]
        } else if (hi & 0x20) == 0 && (lo & 0x9) == 0x9 {
            arm_undefined
        } else {
            let imm_bit = (hi >> 5) & 1;
            let reg_shift = if imm_bit == 0 { lo & 1 } else { 0 };
            ARM_DATA_PROC_FNS[((hi & 0x3F) << 1) | reg_shift]
        };
        i += 1;
    }
    lut
}

const fn build_thumb_lut() -> [ThumbFn; 1024] {
    let mut lut: [ThumbFn; 1024] = [thumb_undefined; 1024];
    let mut i = 0usize;
    while i < 1024 {
        let top = (i << 6) as u16;
        lut[i] = if (top & 0xF800) == 0x1800 {
            THUMB_ADD_SUB_FNS[((top >> 9) & 0x03) as usize]
        } else if (top & 0xE000) == 0x0000 {
            THUMB_MOVE_SHIFTED_FNS[((top >> 11) & 0x03) as usize]
        } else if (top & 0xE000) == 0x2000 {
            THUMB_ALU_IMM_FNS[((top >> 11) & 0x03) as usize]
        } else if (top & 0xFC00) == 0x4000 {
            THUMB_ALU_REG_FNS[((top >> 6) & 0x0F) as usize]
        } else if (top & 0xFC00) == 0x4400 {
            THUMB_HI_REG_FNS[((top >> 6) & 0x0F) as usize]
        } else if (top & 0xF800) == 0x4800 {
            thumb_pc_ldr
        } else if (top & 0xF200) == 0x5000 {
            THUMB_LDR_STR_REG_FNS[((top >> 10) & 0x03) as usize]
        } else if (top & 0xF200) == 0x5200 {
            THUMB_LDR_STR_HALF_SIGN_FNS[((top >> 10) & 0x03) as usize]
        } else if (top & 0xE000) == 0x6000 {
            THUMB_LDR_STR_IMM_FNS[((top >> 11) & 0x03) as usize]
        } else if (top & 0xF000) == 0x8000 {
            if (top & (1 << 11)) != 0 {
                thumb_ldr_str_half_imm::<true>
            } else {
                thumb_ldr_str_half_imm::<false>
            }
        } else if (top & 0xF000) == 0x9000 {
            if (top & (1 << 11)) != 0 {
                thumb_ldr_str_sp::<true>
            } else {
                thumb_ldr_str_sp::<false>
            }
        } else if (top & 0xF000) == 0xA000 {
            if (top & (1 << 11)) != 0 {
                thumb_load_addr::<true>
            } else {
                thumb_load_addr::<false>
            }
        } else if (top & 0xFF00) == 0xB000 {
            if (top & (1 << 7)) != 0 {
                thumb_add_sp::<true>
            } else {
                thumb_add_sp::<false>
            }
        } else if (top & 0xF600) == 0xB400 {
            let idx = (((top >> 10) & 0x02) | ((top >> 8) & 0x01)) as usize;
            THUMB_PUSH_POP_FNS[idx]
        } else if (top & 0xF000) == 0xC000 {
            if (top & (1 << 11)) != 0 {
                thumb_ldm_stm::<true>
            } else {
                thumb_ldm_stm::<false>
            }
        } else if (top & 0xFF00) == 0xDF00 {
            thumb_swi
        } else if (top & 0xFF00) == 0xDE00 {
            thumb_undefined
        } else if (top & 0xF000) == 0xD000 {
            THUMB_COND_BRANCH_FNS[((top >> 8) & 0x0F) as usize]
        } else if (top & 0xF800) == 0xE000 {
            thumb_uncond_branch
        } else if (top & 0xF000) == 0xF000 {
            if (top & (1 << 11)) != 0 {
                thumb_long_bl::<true>
            } else {
                thumb_long_bl::<false>
            }
        } else {
            thumb_undefined
        };
        i += 1;
    }
    lut
}

// ======================= ARM INSTRUCTIONS =======================

fn arm_undefined(cpu: &mut Arm7tdmi, bus: &mut SysBus, _instr: u32) -> u64 {
    cpu.enter_undefined(bus, false);
    1
}

fn arm_swi(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let mut comment = ((instr >> 16) & 0xFF) as u8;
    if comment == 0 {
        comment = (instr & 0xFF) as u8;
    }
    if !bus.bios_present && comment == 0xFE {
        bus.hle_swi(cpu, comment);
        return 1;
    }
    let lr_svc = cpu.gpr[15].wrapping_sub(4);
    let old_cpsr = cpu.cpsr;
    cpu.set_cpsr((old_cpsr & !(CPSR_MODE_MASK | CPSR_T)) | (CpuMode::Supervisor as u32) | CPSR_I);
    cpu.spsr_banked[CpuMode::Supervisor.bank_idx()] = old_cpsr;
    cpu.gpr[14] = lr_svc;
    if bus.bios_present {
        cpu.flush_pipeline(bus, 0x0000_0008);
    } else {
        bus.hle_swi(cpu, comment);
    }
    1
}

fn arm_bx(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    if (instr & 0x0FFF_FFF0) != 0x012F_FF10 {
        return arm_undefined(cpu, bus, instr);
    }
    let rm = (instr & 0xF) as usize;
    let target = cpu.gpr[rm];
    if (target & 1) != 0 {
        cpu.cpsr |= CPSR_T;
    } else {
        cpu.cpsr &= !CPSR_T;
    }
    cpu.flush_pipeline(bus, target);
    1
}

fn arm_branch<const LINK: bool>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let offset = (((instr & 0x00FF_FFFF) as i32) << 8) >> 6;
    if LINK {
        cpu.gpr[14] = cpu.gpr[15].wrapping_sub(4);
    }
    let target = cpu.gpr[15].wrapping_add(offset as u32);
    cpu.flush_pipeline(bus, target);
    1
}

fn arm_psr_reg<const CFG: u8>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u32) -> u64 {
    let spsr_flag = (CFG & 2) != 0;
    let is_msr = (CFG & 1) != 0;
    if !is_msr {
        // MRS
        let rd = ((instr >> 12) & 0xF) as usize;
        let val = if spsr_flag { cpu.get_spsr() } else { cpu.cpsr };
        if rd < 15 {
            cpu.gpr[rd] = val;
        }
    } else {
        // MSR register
        let rm = (instr & 0xF) as usize;
        let val = cpu.gpr[rm];
        apply_msr(cpu, spsr_flag, instr, val);
    }
    0
}

fn arm_msr_imm<const SPSR_FLAG: bool>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u32) -> u64 {
    let imm8 = instr & 0xFF;
    let rot = ((instr >> 8) & 0xF) * 2;
    let val = imm8.rotate_right(rot);
    apply_msr(cpu, SPSR_FLAG, instr, val);
    0
}

fn apply_msr(cpu: &mut Arm7tdmi, spsr_flag: bool, instr: u32, val: u32) {
    let mut mask = 0u32;
    if (instr & (1 << 16)) != 0 {
        mask |= 0x0000_00FF;
    }
    if (instr & (1 << 17)) != 0 {
        mask |= 0x0000_FF00;
    }
    if (instr & (1 << 18)) != 0 {
        mask |= 0x00FF_0000;
    }
    if (instr & (1 << 19)) != 0 {
        mask |= 0xFF00_0000;
    }

    if cpu.mode() == CpuMode::User {
        mask &= 0xFF00_0000;
    }

    if spsr_flag {
        let old = cpu.get_spsr();
        cpu.set_spsr((old & !mask) | (val & mask));
    } else {
        // MSR CPSR never modifies the T bit directly
        let cpsr_mask = mask & !CPSR_T;
        let new_cpsr = (cpu.cpsr & !cpsr_mask) | (val & cpsr_mask);
        cpu.set_cpsr(new_cpsr);
    }
}

fn arm_data_proc<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let imm = (CFG & 64) != 0;
    let opcode = (CFG >> 2) & 0xF;
    let set_flags = (CFG & 2) != 0;
    let reg_shift = !imm && (CFG & 1) != 0;
    let rn = ((instr >> 16) & 0xF) as usize;
    let rd = ((instr >> 12) & 0xF) as usize;

    let mut extra_cycles = 0u64;

    let (op2, shifter_carry) = if imm {
        let imm8 = instr & 0xFF;
        let rot = ((instr >> 8) & 0xF) * 2;
        if rot == 0 {
            (imm8, cpu.carry_flag())
        } else {
            let v = imm8.rotate_right(rot);
            (v, (v >> 31) != 0)
        }
    } else {
        let rm = (instr & 0xF) as usize;
        let shift_type = (instr >> 5) & 0x3;
        let mut rm_val = cpu.gpr[rm];
        let amount = if reg_shift {
            let rs = ((instr >> 8) & 0xF) as usize;
            if rm == 15 {
                rm_val = rm_val.wrapping_add(4);
            }
            extra_cycles += 1;
            bus.step_internal(1);
            cpu.gpr[rs] & 0xFF
        } else {
            (instr >> 7) & 0x1F
        };
        cpu.barrel_shift(rm_val, shift_type, amount, reg_shift)
    };

    let op1 = if rn == 15 && reg_shift {
        cpu.gpr[15].wrapping_add(4)
    } else {
        cpu.gpr[rn]
    };

    let c_in = cpu.carry_flag() as u32;
    let mut write_rd = true;
    let mut res = 0u32;

    match opcode {
        0x0 => {
            res = op1 & op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        0x1 => {
            res = op1 ^ op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        0x2 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, !op2, 1);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x3 => {
            let (r, c, v) = Arm7tdmi::alu_add(op2, !op1, 1);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x4 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, op2, 0);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x5 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, op2, c_in);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x6 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, !op2, c_in);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x7 => {
            let (r, c, v) = Arm7tdmi::alu_add(op2, !op1, c_in);
            res = r;
            if set_flags {
                cpu.set_nzcv(res, c, v);
            }
        }
        0x8 => {
            res = op1 & op2;
            write_rd = false;
            cpu.set_nzc(res, shifter_carry);
        }
        0x9 => {
            res = op1 ^ op2;
            write_rd = false;
            cpu.set_nzc(res, shifter_carry);
        }
        0xA => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, !op2, 1);
            res = r;
            write_rd = false;
            cpu.set_nzcv(res, c, v);
        }
        0xB => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, op2, 0);
            res = r;
            write_rd = false;
            cpu.set_nzcv(res, c, v);
        }
        0xC => {
            res = op1 | op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        0xD => {
            res = op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        0xE => {
            res = op1 & !op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        0xF => {
            res = !op2;
            if set_flags {
                cpu.set_nzc(res, shifter_carry);
            }
        }
        _ => {}
    }

    if write_rd {
        if rd == 15 {
            if set_flags {
                let spsr = cpu.get_spsr();
                cpu.set_cpsr(spsr);
            }
            cpu.flush_pipeline(bus, res);
        } else {
            cpu.gpr[rd] = res;
        }
    } else if rd == 15 && set_flags {
        // TST/TEQ/CMP/CMN with Rd == 15 (TSTP/TEQP/CMPP/CMNP) restore CPSR = SPSR without writing PC or flushing.
        let spsr = cpu.get_spsr();
        cpu.set_cpsr(spsr);
    }

    extra_cycles
}

fn arm_mul<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let acc = (CFG & 2) != 0;
    let set_flags = (CFG & 1) != 0;
    let rd = ((instr >> 16) & 0xF) as usize;
    let rn = ((instr >> 12) & 0xF) as usize;
    let rs = ((instr >> 8) & 0xF) as usize;
    let rm = (instr & 0xF) as usize;

    let rs_val = cpu.gpr[rs];
    let mut res = cpu.gpr[rm].wrapping_mul(rs_val);
    let mut extra = Arm7tdmi::booth_cycles(rs_val, true);
    if acc {
        res = res.wrapping_add(cpu.gpr[rn]);
        extra += 1;
    }
    bus.step_internal(extra);

    if rd < 15 {
        cpu.gpr[rd] = res;
    }
    if set_flags {
        cpu.set_nz(res);
    }
    extra
}

fn arm_mull<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let signed = (CFG & 4) != 0;
    let acc = (CFG & 2) != 0;
    let set_flags = (CFG & 1) != 0;
    let rd_hi = ((instr >> 16) & 0xF) as usize;
    let rd_lo = ((instr >> 12) & 0xF) as usize;
    let rs = ((instr >> 8) & 0xF) as usize;
    let rm = (instr & 0xF) as usize;

    let rm_val = cpu.gpr[rm];
    let rs_val = cpu.gpr[rs];

    let mut prod: u64 = if signed {
        ((rm_val as i32 as i64) * (rs_val as i32 as i64)) as u64
    } else {
        (rm_val as u64) * (rs_val as u64)
    };

    let mut extra = 1 + Arm7tdmi::booth_cycles(rs_val, signed);
    if acc {
        let existing = ((cpu.gpr[rd_hi] as u64) << 32) | (cpu.gpr[rd_lo] as u64);
        prod = prod.wrapping_add(existing);
        extra += 1;
    }
    bus.step_internal(extra);

    if rd_lo < 15 {
        cpu.gpr[rd_lo] = prod as u32;
    }
    if rd_hi < 15 {
        cpu.gpr[rd_hi] = (prod >> 32) as u32;
    }

    if set_flags {
        cpu.cpsr = (cpu.cpsr & !(CPSR_N | CPSR_Z))
            | (((prod >> 32) as u32) & CPSR_N)
            | if prod == 0 { CPSR_Z } else { 0 };
    }
    extra
}

fn arm_swp<const BYTE: bool>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let rn = ((instr >> 16) & 0xF) as usize;
    let rd = ((instr >> 12) & 0xF) as usize;
    let rm = (instr & 0xF) as usize;

    let addr = cpu.gpr[rn];
    let val_to_write = cpu.gpr[rm];
    let mut cycles = 1u64; // internal swap cycle

    if BYTE {
        let old = bus.read_8(addr, AccessType::NonSequential, cpu.gpr[15], false) as u32;
        cycles += bus.last_cycles;
        cycles += bus.write_8(addr, val_to_write as u8, AccessType::NonSequential);
        if rd < 15 {
            cpu.gpr[rd] = old;
        }
    } else {
        let (old, rc) = cpu.bus_read_32_rot(bus, addr, AccessType::NonSequential, false);
        let wc = bus.write_32(addr, val_to_write, AccessType::NonSequential);
        cycles += rc + wc;
        if rd < 15 {
            cpu.gpr[rd] = old;
        }
    }
    bus.step_internal(1);
    cycles
}

fn arm_single_transfer<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let reg_off = (CFG & 32) != 0;
    let pre = (CFG & 16) != 0;
    let add = (CFG & 8) != 0;
    let byte = (CFG & 4) != 0;
    let writeback = (CFG & 2) != 0;
    let load = (CFG & 1) != 0;
    let rn = ((instr >> 16) & 0xF) as usize;
    let rd = ((instr >> 12) & 0xF) as usize;

    let offset = if !reg_off {
        instr & 0xFFF
    } else {
        let rm = (instr & 0xF) as usize;
        let shift_type = (instr >> 5) & 0x3;
        let amt = (instr >> 7) & 0x1F;
        cpu.barrel_shift(cpu.gpr[rm], shift_type, amt, false).0
    };

    let base = cpu.gpr[rn];
    let eff_addr = if add {
        base.wrapping_add(offset)
    } else {
        base.wrapping_sub(offset)
    };
    let access_addr = if pre { eff_addr } else { base };

    let mut cyc = 0u64;
    if load {
        let loaded = if byte {
            let v = bus.read_8(access_addr, AccessType::NonSequential, cpu.gpr[15], false) as u32;
            cyc += bus.last_cycles;
            v
        } else {
            let (v, rc) = cpu.bus_read_32_rot(bus, access_addr, AccessType::NonSequential, false);
            cyc += rc;
            v
        };
        cyc += 1;
        bus.step_internal(1);

        if (!pre || writeback) && rn != 15 {
            cpu.gpr[rn] = eff_addr;
        }
        if rd == 15 {
            cpu.flush_pipeline(bus, loaded);
        } else {
            cpu.gpr[rd] = loaded;
        }
    } else {
        let store_val = if rd == 15 {
            cpu.gpr[15].wrapping_add(4)
        } else {
            cpu.gpr[rd]
        };
        if byte {
            cyc += bus.write_8(access_addr, store_val as u8, AccessType::NonSequential);
        } else {
            cyc += bus.write_32(access_addr, store_val, AccessType::NonSequential);
        }
        if (!pre || writeback) && rn != 15 {
            cpu.gpr[rn] = eff_addr;
        }
    }
    cyc
}

fn arm_halfword_transfer<const CFG: u8>(
    cpu: &mut Arm7tdmi,
    bus: &mut SysBus,
    instr: u32,
) -> u64 {
    let pre = (CFG & 64) != 0;
    let add = (CFG & 32) != 0;
    let imm = (CFG & 16) != 0;
    let writeback = (CFG & 8) != 0;
    let load = (CFG & 4) != 0;
    let sh = CFG & 3;
    let rn = ((instr >> 16) & 0xF) as usize;
    let rd = ((instr >> 12) & 0xF) as usize;

    let offset = if imm {
        ((instr >> 4) & 0xF0) | (instr & 0xF)
    } else {
        cpu.gpr[(instr & 0xF) as usize]
    };

    let base = cpu.gpr[rn];
    let eff_addr = if add {
        base.wrapping_add(offset)
    } else {
        base.wrapping_sub(offset)
    };
    let access_addr = if pre { eff_addr } else { base };

    let mut cyc = 0u64;
    if load {
        let val: u32 = match sh {
            1 => {
                // LDRH: unaligned rotates right by 8
                let raw = bus.read_16(access_addr, AccessType::NonSequential, cpu.gpr[15], false)
                    as u32;
                cyc += bus.last_cycles;
                if (access_addr & 1) != 0 {
                    raw.rotate_right(8)
                } else {
                    raw
                }
            }
            2 => {
                // LDRSB
                let v = bus.read_8(access_addr, AccessType::NonSequential, cpu.gpr[15], false)
                    as i8 as i32 as u32;
                cyc += bus.last_cycles;
                v
            }
            3 => {
                // LDRSH: unaligned behaves like LDRSB on ARMv4T!
                if (access_addr & 1) != 0 {
                    let v = bus.read_8(access_addr, AccessType::NonSequential, cpu.gpr[15], false)
                        as i8 as i32 as u32;
                    cyc += bus.last_cycles;
                    v
                } else {
                    let v =
                        bus.read_16(access_addr, AccessType::NonSequential, cpu.gpr[15], false)
                            as i16 as i32 as u32;
                    cyc += bus.last_cycles;
                    v
                }
            }
            _ => 0,
        };
        cyc += 1;
        bus.step_internal(1);

        if (!pre || writeback) && rn != 15 {
            cpu.gpr[rn] = eff_addr;
        }
        if rd == 15 {
            cpu.flush_pipeline(bus, val);
        } else {
            cpu.gpr[rd] = val;
        }
    } else {
        let store_val = if rd == 15 {
            cpu.gpr[15].wrapping_add(4)
        } else {
            cpu.gpr[rd]
        };
        if sh == 1 {
            cyc += bus.write_16(access_addr, store_val as u16, AccessType::NonSequential);
        }
        if (!pre || writeback) && rn != 15 {
            cpu.gpr[rn] = eff_addr;
        }
    }
    cyc
}

fn arm_block_transfer<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u32) -> u64 {
    let pre = (CFG & 16) != 0;
    let inc = (CFG & 8) != 0;
    let s_bit = (CFG & 4) != 0;
    let writeback = (CFG & 2) != 0;
    let load = (CFG & 1) != 0;
    let rn = ((instr >> 16) & 0xF) as usize;
    let rlist = instr & 0xFFFF;

    let base = cpu.gpr[rn];

    // ARMv4T hardware quirk: empty register list transfers R15 and offsets base by +/- 0x40!
    if rlist == 0 {
        let new_base = if inc {
            base.wrapping_add(0x40)
        } else {
            base.wrapping_sub(0x40)
        };
        let addr = match (inc, pre) {
            (true, false) => base,
            (true, true) => base.wrapping_add(4),
            (false, false) => new_base.wrapping_add(4),
            (false, true) => new_base,
        };
        let cyc = if load {
            let v = bus.read_32(addr, AccessType::NonSequential, cpu.gpr[15]);
            let rc = bus.last_cycles;
            if writeback && rn != 15 {
                cpu.gpr[rn] = new_base;
            }
            cpu.flush_pipeline(bus, v);
            rc + 1
        } else {
            let pc_val = cpu.gpr[15].wrapping_add(4);
            let wc = bus.write_32(addr, pc_val, AccessType::NonSequential);
            if writeback && rn != 15 {
                cpu.gpr[rn] = new_base;
            }
            wc
        };
        return cyc;
    }

    let count = rlist.count_ones();
    let total_bytes = count * 4;
    let new_base = if inc {
        base.wrapping_add(total_bytes)
    } else {
        base.wrapping_sub(total_bytes)
    };
    let mut addr = match (inc, pre) {
        (true, false) => base,
        (true, true) => base.wrapping_add(4),
        (false, false) => new_base.wrapping_add(4),
        (false, true) => new_base,
    };

    let pc_in_list = (rlist & (1 << 15)) != 0;
    let user_bank = s_bit && (!load || !pc_in_list);
    let rn_lowest = rlist.trailing_zeros() as usize == rn;
    let mut cyc = 0u64;
    let mut first = true;

    if load {
        for r in 0..16 {
            if (rlist & (1 << r)) == 0 {
                continue;
            }
            let acc = if first {
                first = false;
                AccessType::NonSequential
            } else {
                AccessType::Sequential
            };
            let v = bus.read_32(addr, acc, cpu.gpr[15]);
            cyc += bus.last_cycles;
            addr = addr.wrapping_add(4);

            if r == 15 {
                if s_bit {
                    let spsr = cpu.get_spsr();
                    cpu.set_cpsr(spsr);
                }
                cpu.flush_pipeline(bus, v);
            } else if user_bank {
                cpu.write_usr_reg(r, v);
            } else {
                cpu.gpr[r] = v;
            }
        }
        // LDM writeback does NOT happen if Rn was loaded (unless user_bank, wait: ARM7TDMI writes back only if Rn not in rlist!)
        if writeback && !user_bank && (rlist & (1 << rn)) == 0 && rn != 15 {
            cpu.gpr[rn] = new_base;
        }
        cyc += 1;
        bus.step_internal(1);
    } else {
        for r in 0..16 {
            if (rlist & (1 << r)) == 0 {
                continue;
            }
            let acc = if first {
                first = false;
                AccessType::NonSequential
            } else {
                AccessType::Sequential
            };
            let v = if r == 15 {
                cpu.gpr[15].wrapping_add(4)
            } else if r == rn && writeback && !rn_lowest {
                new_base
            } else if user_bank {
                cpu.read_usr_reg(r)
            } else {
                cpu.gpr[r]
            };
            cyc += bus.write_32(addr, v, acc);
            addr = addr.wrapping_add(4);
        }
        if writeback && !user_bank && rn != 15 {
            cpu.gpr[rn] = new_base;
        }
    }

    cyc
}

// ======================= THUMB INSTRUCTIONS =======================

fn thumb_undefined(cpu: &mut Arm7tdmi, bus: &mut SysBus, _instr: u16) -> u64 {
    cpu.enter_undefined(bus, true);
    1
}

fn thumb_swi(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let lr_svc = cpu.gpr[15].wrapping_sub(2);
    let old_cpsr = cpu.cpsr;
    cpu.set_cpsr((old_cpsr & !(CPSR_MODE_MASK | CPSR_T)) | (CpuMode::Supervisor as u32) | CPSR_I);
    cpu.spsr_banked[CpuMode::Supervisor.bank_idx()] = old_cpsr;
    cpu.gpr[14] = lr_svc;
    if bus.bios_present {
        cpu.flush_pipeline(bus, 0x0000_0008);
    } else {
        bus.hle_swi(cpu, (instr & 0xFF) as u8);
    }
    1
}

fn thumb_move_shifted<const OP: u8>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u16) -> u64 {
    let imm5 = ((instr >> 6) & 0x1F) as u32;
    let rs = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;

    let (res, c) = cpu.barrel_shift(cpu.gpr[rs], u32::from(OP), imm5, false);
    cpu.gpr[rd] = res;
    cpu.set_nzc(res, c);
    0
}

fn thumb_add_sub<const CFG: u8>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u16) -> u64 {
    let imm = (CFG & 2) != 0;
    let sub = (CFG & 1) != 0;
    let rn_imm = ((instr >> 6) & 0x7) as u32;
    let rs = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;

    let op1 = cpu.gpr[rs];
    let op2 = if imm { rn_imm } else { cpu.gpr[rn_imm as usize] };

    let (res, c, v) = if sub {
        Arm7tdmi::alu_add(op1, !op2, 1)
    } else {
        Arm7tdmi::alu_add(op1, op2, 0)
    };
    cpu.gpr[rd] = res;
    cpu.set_nzcv(res, c, v);
    0
}

fn thumb_alu_imm<const OP: u8>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u16) -> u64 {
    let rd = ((instr >> 8) & 0x7) as usize;
    let imm = (instr & 0xFF) as u32;

    match OP {
        0 => {
            cpu.gpr[rd] = imm;
            cpu.set_nz(imm);
        }
        1 => {
            let (res, c, v) = Arm7tdmi::alu_add(cpu.gpr[rd], !imm, 1);
            cpu.set_nzcv(res, c, v);
        }
        2 => {
            let (res, c, v) = Arm7tdmi::alu_add(cpu.gpr[rd], imm, 0);
            cpu.gpr[rd] = res;
            cpu.set_nzcv(res, c, v);
        }
        _ => {
            let (res, c, v) = Arm7tdmi::alu_add(cpu.gpr[rd], !imm, 1);
            cpu.gpr[rd] = res;
            cpu.set_nzcv(res, c, v);
        }
    }
    0
}

fn thumb_alu_reg<const OP: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let rs = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;

    let op1 = cpu.gpr[rd];
    let op2 = cpu.gpr[rs];
    let mut cyc = 0u64;

    match OP {
        0x0 => {
            let r = op1 & op2;
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
        }
        0x1 => {
            let r = op1 ^ op2;
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
        }
        0x2 => {
            let (r, c) = cpu.barrel_shift(op1, 0, op2 & 0xFF, true);
            cpu.gpr[rd] = r;
            cpu.set_nzc(r, c);
            cyc += 1;
            bus.step_internal(1);
        }
        0x3 => {
            let (r, c) = cpu.barrel_shift(op1, 1, op2 & 0xFF, true);
            cpu.gpr[rd] = r;
            cpu.set_nzc(r, c);
            cyc += 1;
            bus.step_internal(1);
        }
        0x4 => {
            let (r, c) = cpu.barrel_shift(op1, 2, op2 & 0xFF, true);
            cpu.gpr[rd] = r;
            cpu.set_nzc(r, c);
            cyc += 1;
            bus.step_internal(1);
        }
        0x5 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, op2, cpu.carry_flag() as u32);
            cpu.gpr[rd] = r;
            cpu.set_nzcv(r, c, v);
        }
        0x6 => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, !op2, cpu.carry_flag() as u32);
            cpu.gpr[rd] = r;
            cpu.set_nzcv(r, c, v);
        }
        0x7 => {
            let (r, c) = cpu.barrel_shift(op1, 3, op2 & 0xFF, true);
            cpu.gpr[rd] = r;
            cpu.set_nzc(r, c);
            cyc += 1;
            bus.step_internal(1);
        }
        0x8 => cpu.set_nz(op1 & op2),
        0x9 => {
            let (r, c, v) = Arm7tdmi::alu_add(0, !op2, 1);
            cpu.gpr[rd] = r;
            cpu.set_nzcv(r, c, v);
        }
        0xA => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, !op2, 1);
            cpu.set_nzcv(r, c, v);
        }
        0xB => {
            let (r, c, v) = Arm7tdmi::alu_add(op1, op2, 0);
            cpu.set_nzcv(r, c, v);
        }
        0xC => {
            let r = op1 | op2;
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
        }
        0xD => {
            let r = op1.wrapping_mul(op2);
            let extra = Arm7tdmi::booth_cycles(op1, true);
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
            cyc += extra;
            bus.step_internal(extra);
        }
        0xE => {
            let r = op1 & !op2;
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
        }
        _ => {
            let r = !op2;
            cpu.gpr[rd] = r;
            cpu.set_nz(r);
        }
    }

    cyc
}

fn thumb_hi_reg_bx<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let op = (CFG >> 2) & 0x3;
    let h1 = usize::from((CFG & 2) != 0);
    let h2 = usize::from((CFG & 1) != 0);
    let rs = (h2 << 3) | (((instr >> 3) & 0x7) as usize);
    let rd = (h1 << 3) | ((instr & 0x7) as usize);

    let op1 = cpu.gpr[rd];
    let op2 = cpu.gpr[rs];

    match op {
        0 => {
            let res = op1.wrapping_add(op2);
            if rd == 15 {
                cpu.flush_pipeline(bus, res & !1);
            } else {
                cpu.gpr[rd] = res;
            }
        }
        1 => {
            let (res, c, v) = Arm7tdmi::alu_add(op1, !op2, 1);
            cpu.set_nzcv(res, c, v);
        }
        2 => {
            if rd == 15 {
                cpu.flush_pipeline(bus, op2 & !1);
            } else {
                cpu.gpr[rd] = op2;
            }
        }
        _ => {
            // BX Rs (switching to ARM if bit 0 == 0!)
            if (op2 & 1) != 0 {
                cpu.cpsr |= CPSR_T;
            } else {
                cpu.cpsr &= !CPSR_T;
            }
            cpu.flush_pipeline(bus, op2);
        }
    }
    0
}

fn thumb_pc_ldr(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let rd = ((instr >> 8) & 0x7) as usize;
    let imm = ((instr & 0xFF) as u32) << 2;
    let addr = (cpu.gpr[15] & !3).wrapping_add(imm);
    let val = bus.read_32(addr, AccessType::NonSequential, cpu.gpr[15]);
    let rc = bus.last_cycles;
    cpu.gpr[rd] = val;
    bus.step_internal(1);
    rc + 1
}

fn thumb_ldr_str_reg<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let load = (CFG & 2) != 0;
    let byte = (CFG & 1) != 0;
    let ro = ((instr >> 6) & 0x7) as usize;
    let rb = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;

    let addr = cpu.gpr[rb].wrapping_add(cpu.gpr[ro]);
    if load {
        let val = if byte {
            bus.read_8(addr, AccessType::NonSequential, cpu.gpr[15], true) as u32
        } else {
            cpu.bus_read_32_rot(bus, addr, AccessType::NonSequential, true).0
        };
        let rc = bus.last_cycles;
        cpu.gpr[rd] = val;
        bus.step_internal(1);
        rc + 1
    } else if byte {
        bus.write_8(addr, cpu.gpr[rd] as u8, AccessType::NonSequential)
    } else {
        bus.write_32(addr, cpu.gpr[rd], AccessType::NonSequential)
    }
}

fn thumb_ldr_str_half_sign<const OP: u8>(
    cpu: &mut Arm7tdmi,
    bus: &mut SysBus,
    instr: u16,
) -> u64 {
    let ro = ((instr >> 6) & 0x7) as usize;
    let rb = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;
    let addr = cpu.gpr[rb].wrapping_add(cpu.gpr[ro]);

    if OP == 0 {
        // STRH
        return bus.write_16(addr, cpu.gpr[rd] as u16, AccessType::NonSequential);
    }

    let val: u32 = match OP {
        1 => bus.read_8(addr, AccessType::NonSequential, cpu.gpr[15], true) as i8 as i32 as u32,
        2 => {
            let raw = bus.read_16(addr, AccessType::NonSequential, cpu.gpr[15], true) as u32;
            if (addr & 1) != 0 {
                raw.rotate_right(8)
            } else {
                raw
            }
        }
        _ => {
            if (addr & 1) != 0 {
                bus.read_8(addr, AccessType::NonSequential, cpu.gpr[15], true) as i8 as i32 as u32
            } else {
                bus.read_16(addr, AccessType::NonSequential, cpu.gpr[15], true) as i16 as i32 as u32
            }
        }
    };
    let rc = bus.last_cycles;
    cpu.gpr[rd] = val;
    bus.step_internal(1);
    rc + 1
}

fn thumb_ldr_str_imm<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let byte = (CFG & 2) != 0;
    let load = (CFG & 1) != 0;
    let imm5 = ((instr >> 6) & 0x1F) as u32;
    let rb = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;

    let offset = if byte { imm5 } else { imm5 << 2 };
    let addr = cpu.gpr[rb].wrapping_add(offset);

    if load {
        let val = if byte {
            bus.read_8(addr, AccessType::NonSequential, cpu.gpr[15], true) as u32
        } else {
            cpu.bus_read_32_rot(bus, addr, AccessType::NonSequential, true).0
        };
        let rc = bus.last_cycles;
        cpu.gpr[rd] = val;
        bus.step_internal(1);
        rc + 1
    } else if byte {
        bus.write_8(addr, cpu.gpr[rd] as u8, AccessType::NonSequential)
    } else {
        bus.write_32(addr, cpu.gpr[rd], AccessType::NonSequential)
    }
}

fn thumb_ldr_str_half_imm<const LOAD: bool>(
    cpu: &mut Arm7tdmi,
    bus: &mut SysBus,
    instr: u16,
) -> u64 {
    let imm = (((instr >> 6) & 0x1F) as u32) << 1;
    let rb = ((instr >> 3) & 0x7) as usize;
    let rd = (instr & 0x7) as usize;
    let addr = cpu.gpr[rb].wrapping_add(imm);

    if LOAD {
        let raw = bus.read_16(addr, AccessType::NonSequential, cpu.gpr[15], true) as u32;
        let rc = bus.last_cycles;
        cpu.gpr[rd] = if (addr & 1) != 0 {
            raw.rotate_right(8)
        } else {
            raw
        };
        bus.step_internal(1);
        rc + 1
    } else {
        bus.write_16(addr, cpu.gpr[rd] as u16, AccessType::NonSequential)
    }
}

fn thumb_ldr_str_sp<const LOAD: bool>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let rd = ((instr >> 8) & 0x7) as usize;
    let offset = ((instr & 0xFF) as u32) << 2;
    let addr = cpu.gpr[13].wrapping_add(offset);

    if LOAD {
        let (val, rc) = cpu.bus_read_32_rot(bus, addr, AccessType::NonSequential, true);
        cpu.gpr[rd] = val;
        bus.step_internal(1);
        rc + 1
    } else {
        bus.write_32(addr, cpu.gpr[rd], AccessType::NonSequential)
    }
}

fn thumb_load_addr<const SP: bool>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u16) -> u64 {
    let rd = ((instr >> 8) & 0x7) as usize;
    let imm = ((instr & 0xFF) as u32) << 2;
    let base = if SP { cpu.gpr[13] } else { cpu.gpr[15] & !3 };
    cpu.gpr[rd] = base.wrapping_add(imm);
    0
}

fn thumb_add_sp<const SUB: bool>(cpu: &mut Arm7tdmi, _bus: &mut SysBus, instr: u16) -> u64 {
    let imm = ((instr & 0x7F) as u32) << 2;
    cpu.gpr[13] = if SUB {
        cpu.gpr[13].wrapping_sub(imm)
    } else {
        cpu.gpr[13].wrapping_add(imm)
    };
    0
}

fn thumb_push_pop<const CFG: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let load = (CFG & 2) != 0;
    let p_r = (CFG & 1) != 0;
    let rlist = instr & 0xFF;

    let mut sp = cpu.gpr[13];
    if rlist == 0 && !p_r {
        if load {
            let v = bus.read_32(sp, AccessType::NonSequential, cpu.gpr[15]);
            let rc = bus.last_cycles;
            cpu.gpr[13] = sp.wrapping_add(0x40);
            cpu.flush_pipeline(bus, v & !1);
            return rc + 1;
        } else {
            sp = sp.wrapping_sub(0x40);
            cpu.gpr[13] = sp;
            let pc_val = cpu.gpr[15].wrapping_add(2);
            return bus.write_32(sp, pc_val, AccessType::NonSequential);
        }
    }

    let mut cyc = 0u64;
    let mut first = true;
    if load {
        for r in 0..8usize {
            if (rlist & (1 << r)) != 0 {
                let acc = if first {
                    first = false;
                    AccessType::NonSequential
                } else {
                    AccessType::Sequential
                };
                let v = bus.read_32(sp, acc, cpu.gpr[15]);
                cyc += bus.last_cycles;
                cpu.gpr[r] = v;
                sp = sp.wrapping_add(4);
            }
        }
        if p_r {
            let acc = if first {
                AccessType::NonSequential
            } else {
                AccessType::Sequential
            };
            let v = bus.read_32(sp, acc, cpu.gpr[15]);
            cyc += bus.last_cycles;
            sp = sp.wrapping_add(4);
            cpu.gpr[13] = sp;
            cpu.flush_pipeline(bus, v & !1);
        } else {
            cpu.gpr[13] = sp;
        }
        bus.step_internal(1);
        cyc + 1
    } else {
        let count = rlist.count_ones() + (p_r as u32);
        sp = sp.wrapping_sub(count * 4);
        cpu.gpr[13] = sp;
        let mut addr = sp;
        for r in 0..8usize {
            if (rlist & (1 << r)) != 0 {
                let acc = if first {
                    first = false;
                    AccessType::NonSequential
                } else {
                    AccessType::Sequential
                };
                cyc += bus.write_32(addr, cpu.gpr[r], acc);
                addr = addr.wrapping_add(4);
            }
        }
        if p_r {
            let acc = if first {
                AccessType::NonSequential
            } else {
                AccessType::Sequential
            };
            cyc += bus.write_32(addr, cpu.gpr[14], acc);
        }
        cyc
    }
}

fn thumb_ldm_stm<const LOAD: bool>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let rb = ((instr >> 8) & 0x7) as usize;
    let rlist = (instr & 0xFF) as u32;

    let base = cpu.gpr[rb];
    if rlist == 0 {
        let new_base = base.wrapping_add(0x40);
        if LOAD {
            let v = bus.read_32(base, AccessType::NonSequential, cpu.gpr[15]);
            let rc = bus.last_cycles;
            cpu.gpr[rb] = new_base;
            cpu.flush_pipeline(bus, v & !1);
            return rc + 1;
        } else {
            let pc_val = cpu.gpr[15].wrapping_add(2);
            let wc = bus.write_32(base, pc_val, AccessType::NonSequential);
            cpu.gpr[rb] = new_base;
            return wc;
        }
    }

    let count = rlist.count_ones();
    let new_base = base.wrapping_add(count * 4);
    let mut addr = base;
    let rb_lowest = (rlist.trailing_zeros() as usize) == rb;
    let mut cyc = 0u64;
    let mut first = true;

    if LOAD {
        for r in 0..8usize {
            if (rlist & (1 << r)) != 0 {
                let acc = if first {
                    first = false;
                    AccessType::NonSequential
                } else {
                    AccessType::Sequential
                };
                let v = bus.read_32(addr, acc, cpu.gpr[15]);
                cyc += bus.last_cycles;
                cpu.gpr[r] = v;
                addr = addr.wrapping_add(4);
            }
        }
        if (rlist & (1 << rb)) == 0 {
            cpu.gpr[rb] = new_base;
        }
        bus.step_internal(1);
        cyc + 1
    } else {
        for r in 0..8usize {
            if (rlist & (1 << r)) != 0 {
                let acc = if first {
                    first = false;
                    AccessType::NonSequential
                } else {
                    AccessType::Sequential
                };
                let v = if r == rb && !rb_lowest {
                    new_base
                } else {
                    cpu.gpr[r]
                };
                cyc += bus.write_32(addr, v, acc);
                addr = addr.wrapping_add(4);
            }
        }
        cpu.gpr[rb] = new_base;
        cyc
    }
}

fn thumb_cond_branch<const COND: u8>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    if COND >= 0xE {
        return 0;
    }
    if cpu.check_cond(u32::from(COND)) {
        let offset = ((instr as i8) as i32) << 1;
        let target = cpu.gpr[15].wrapping_add(offset as u32);
        cpu.flush_pipeline(bus, target);
    }
    0
}

fn thumb_uncond_branch(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let offset = ((((instr & 0x07FF) as i32) << 21) >> 20) as u32;
    let target = cpu.gpr[15].wrapping_add(offset);
    cpu.flush_pipeline(bus, target);
    0
}

fn thumb_long_bl<const SECOND: bool>(cpu: &mut Arm7tdmi, bus: &mut SysBus, instr: u16) -> u64 {
    let offset11 = (instr & 0x07FF) as u32;
    if !SECOND {
        let signed = (((offset11 as i32) << 21) >> 9) as u32;
        cpu.gpr[14] = cpu.gpr[15].wrapping_add(signed);
    } else {
        let next_pc = cpu.gpr[15].wrapping_sub(2);
        let target = cpu.gpr[14].wrapping_add(offset11 << 1);
        cpu.gpr[14] = next_pc | 1;
        cpu.flush_pipeline(bus, target & !1);
    }
    0
}
