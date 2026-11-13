#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod callstack;
#[cfg(feature = "std")]
pub mod gdb;
pub mod io_regs;
pub mod rsp;

pub use callstack::{
    CallFrame, CallStackTracker, MAX_CALL_STACK_DEPTH, PreStepSnapshot, arm_cond_passed,
    is_call_instruction, is_return_instruction, step_out_return_addr,
};
#[cfg(feature = "std")]
pub use gdb::{GdbPollOutcome, GdbStub};
pub use io_regs::{IoField, IoRegDesc, decode_io_reg, known_io_regs};
pub use rsp::{
    ARM7TDMI_TARGET_XML, GdbAction, GdbSession, PacketOutcome, RspEvent, RspParser,
    encode_rsp_packet, escape_rsp_bytes, rsp_checksum, unescape_rsp_bytes,
};

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use gba_core::GbaCore;

const REG_NAMES: [&str; 16] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "sp", "lr",
    "pc",
];

const COND_NAMES: [&str; 16] = [
    "eq", "ne", "cs", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "", "nv",
];

const ARM_DP_OPS: [&str; 16] = [
    "and", "eor", "sub", "rsb", "add", "adc", "sbc", "rsc", "tst", "teq", "cmp", "cmn", "orr",
    "mov", "bic", "mvn",
];

const THUMB_ALU_OPS: [&str; 16] = [
    "and", "eor", "lsl", "lsr", "asr", "adc", "sbc", "ror", "tst", "neg", "cmp", "cmn", "orr",
    "mul", "bic", "mvn",
];

const SHIFT_NAMES: [&str; 4] = ["lsl", "lsr", "asr", "ror"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisasmLine {
    pub addr: u32,
    pub opcode: u32,
    pub is_thumb: bool,
    pub text: String,
}

#[inline]
fn reg(idx: u32) -> &'static str {
    REG_NAMES[(idx & 0xF) as usize]
}

#[inline]
fn cond(c: u32) -> &'static str {
    COND_NAMES[(c & 0xF) as usize]
}

fn format_reg_list(mask: u16, extra: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut r = 0usize;
    while r < 16 {
        if (mask & (1 << r)) != 0 {
            let start = r;
            while r + 1 < 16 && (mask & (1 << (r + 1))) != 0 {
                r += 1;
            }
            if r > start + 1 {
                parts.push(format!("{}-{}", reg(start as u32), reg(r as u32)));
            } else if r == start + 1 {
                parts.push(reg(start as u32).to_string());
                parts.push(reg(r as u32).to_string());
            } else {
                parts.push(reg(start as u32).to_string());
            }
        }
        r += 1;
    }
    if let Some(ex) = extra {
        parts.push(ex.to_string());
    }
    format!("{{{}}}", parts.join(", "))
}

fn format_arm_shift(instr: u32) -> String {
    let rm = reg(instr & 0xF);
    let stype = (instr >> 5) & 3;
    if (instr & 0x10) == 0 {
        let amt = (instr >> 7) & 0x1F;
        if amt == 0 {
            match stype {
                0 => rm.to_string(),
                1 | 2 => format!("{rm}, {} #32", SHIFT_NAMES[stype as usize]),
                _ => format!("{rm}, rrx"),
            }
        } else {
            format!("{rm}, {} #{amt}", SHIFT_NAMES[stype as usize])
        }
    } else {
        let rs = reg((instr >> 8) & 0xF);
        format!("{rm}, {} {rs}", SHIFT_NAMES[stype as usize])
    }
}

pub fn format_registers(core: &GbaCore) -> String {
    let cpu = &core.cpu;
    let mut out = String::new();
    for r in 0..16 {
        let sep = if r % 4 == 3 { '\n' } else { ' ' };
        out.push_str(&format!("R{:02}={:08X}{}", r, cpu.gpr[r], sep));
    }
    out.push_str(&format!(
        "CPSR={:08X} ({}) VCOUNT={} IE={:04X} IF={:04X} IME={}\n",
        cpu.cpsr,
        if cpu.is_thumb() { "THUMB" } else { "ARM" },
        core.bus.ppu.vcount,
        core.bus.hw.ie,
        core.bus.hw.if_flag,
        core.bus.hw.ime
    ));
    out
}

pub fn disassemble_range(
    core: &GbaCore,
    start_addr: u32,
    count: usize,
    thumb: bool,
) -> Vec<DisasmLine> {
    let mut out = Vec::with_capacity(count);
    let step = if thumb { 2u32 } else { 4u32 };
    let mut addr = start_addr & !(step - 1);
    let mut i = 0;
    while i < count {
        if thumb {
            let op = core.bus.debug_read_16(addr);
            // Combine Thumb 32-bit BL pair (prefix 0xF000 + suffix 0xF800) if present.
            if (op & 0xF800) == 0xF000 {
                let next = core.bus.debug_read_16(addr.wrapping_add(2));
                if (next & 0xF800) == 0xF800 {
                    let hi = (((op & 0x7FF) as i32) << 21) >> 9;
                    let lo = ((next & 0x7FF) as i32) << 1;
                    let target = addr.wrapping_add(4).wrapping_add((hi + lo) as u32);
                    out.push(DisasmLine {
                        addr,
                        opcode: (op as u32) | ((next as u32) << 16),
                        is_thumb: true,
                        text: format!("bl 0x{target:08X}"),
                    });
                    addr = addr.wrapping_add(4);
                    i += 1;
                    continue;
                }
            }
            out.push(DisasmLine {
                addr,
                opcode: op as u32,
                is_thumb: true,
                text: disassemble_thumb(op, addr),
            });
            addr = addr.wrapping_add(2);
        } else {
            let op = core.bus.debug_read_32(addr);
            out.push(DisasmLine {
                addr,
                opcode: op,
                is_thumb: false,
                text: disassemble_arm(op, addr),
            });
            addr = addr.wrapping_add(4);
        }
        i += 1;
    }
    out
}

pub fn disassemble_thumb(instr: u16, addr: u32) -> String {
    let top = (instr >> 13) & 7;
    match top {
        0 => {
            // Format 1 & Format 2
            let op = (instr >> 11) & 3;
            let rd = reg((instr & 7) as u32);
            let rs = reg(((instr >> 3) & 7) as u32);
            if op == 3 {
                let is_imm = (instr & (1 << 10)) != 0;
                let is_sub = (instr & (1 << 9)) != 0;
                let rn = (instr >> 6) & 7;
                let mnemonic = if is_sub { "sub" } else { "add" };
                if is_imm {
                    format!("{mnemonic} {rd}, {rs}, #{rn}")
                } else {
                    format!("{mnemonic} {rd}, {rs}, {}", reg(rn as u32))
                }
            } else {
                let imm5 = (instr >> 6) & 0x1F;
                let shift_amt = if imm5 == 0 && op != 0 { 32 } else { imm5 };
                let mnemonic = match op {
                    0 => "lsl",
                    1 => "lsr",
                    _ => "asr",
                };
                format!("{mnemonic} {rd}, {rs}, #{shift_amt}")
            }
        }
        1 => {
            // Format 3: move/compare/add/subtract immediate
            let op = (instr >> 11) & 3;
            let rd = reg(((instr >> 8) & 7) as u32);
            let imm = instr & 0xFF;
            let mnemonic = match op {
                0 => "mov",
                1 => "cmp",
                2 => "add",
                _ => "sub",
            };
            format!("{mnemonic} {rd}, #0x{imm:02X}")
        }
        2 => {
            let sub = (instr >> 10) & 7;
            if sub == 0 {
                // Format 4: ALU operations
                let op = ((instr >> 6) & 0xF) as usize;
                let rd = reg((instr & 7) as u32);
                let rs = reg(((instr >> 3) & 7) as u32);
                format!("{} {rd}, {rs}", THUMB_ALU_OPS[op])
            } else if sub == 1 {
                // Format 5: Hi-register ops / BX
                let op = (instr >> 8) & 3;
                let rd = (instr & 7) | ((instr >> 4) & 8);
                let rs = ((instr >> 3) & 7) | ((instr >> 3) & 8);
                if op == 3 {
                    format!("bx {}", reg(rs as u32))
                } else if op == 2 && rd == 8 && rs == 8 {
                    "nop".to_string()
                } else {
                    let mnemonic = match op {
                        0 => "add",
                        1 => "cmp",
                        _ => "mov",
                    };
                    format!("{mnemonic} {}, {}", reg(rd as u32), reg(rs as u32))
                }
            } else if (sub & 6) == 2 {
                // Format 6: PC-relative load
                let rd = reg(((instr >> 8) & 7) as u32);
                let off = ((instr & 0xFF) as u32) << 2;
                let ea = (addr.wrapping_add(4) & !3).wrapping_add(off);
                format!("ldr {rd}, [pc, #0x{off:03X}] ; =0x{ea:08X}")
            } else if (instr & 0x0200) == 0 {
                // Format 7: load/store register offset
                let op = (instr >> 10) & 3;
                let rd = reg((instr & 7) as u32);
                let rb = reg(((instr >> 3) & 7) as u32);
                let ro = reg(((instr >> 6) & 7) as u32);
                let mnemonic = match op {
                    0 => "str",
                    1 => "strb",
                    2 => "ldr",
                    _ => "ldrb",
                };
                format!("{mnemonic} {rd}, [{rb}, {ro}]")
            } else {
                // Format 8: sign-extended byte/halfword
                let op = (instr >> 10) & 3;
                let rd = reg((instr & 7) as u32);
                let rb = reg(((instr >> 3) & 7) as u32);
                let ro = reg(((instr >> 6) & 7) as u32);
                let mnemonic = match op {
                    0 => "strh",
                    1 => "ldrsb",
                    2 => "ldrh",
                    _ => "ldrsh",
                };
                format!("{mnemonic} {rd}, [{rb}, {ro}]")
            }
        }
        3 => {
            // Format 9: load/store with immediate offset
            let is_byte = (instr & (1 << 12)) != 0;
            let is_load = (instr & (1 << 11)) != 0;
            let imm5 = (instr >> 6) & 0x1F;
            let off = if is_byte { imm5 } else { imm5 << 2 };
            let rb = reg(((instr >> 3) & 7) as u32);
            let rd = reg((instr & 7) as u32);
            let mnemonic = match (is_load, is_byte) {
                (false, false) => "str",
                (false, true) => "strb",
                (true, false) => "ldr",
                (true, true) => "ldrb",
            };
            format!("{mnemonic} {rd}, [{rb}, #0x{off:02X}]")
        }
        4 => {
            if (instr & (1 << 12)) == 0 {
                // Format 10: halfword immediate
                let is_load = (instr & (1 << 11)) != 0;
                let off = ((instr >> 6) & 0x1F) << 1;
                let rb = reg(((instr >> 3) & 7) as u32);
                let rd = reg((instr & 7) as u32);
                let mnemonic = if is_load { "ldrh" } else { "strh" };
                format!("{mnemonic} {rd}, [{rb}, #0x{off:02X}]")
            } else {
                // Format 11: SP-relative load/store
                let is_load = (instr & (1 << 11)) != 0;
                let rd = reg(((instr >> 8) & 7) as u32);
                let off = ((instr & 0xFF) as u32) << 2;
                let mnemonic = if is_load { "ldr" } else { "str" };
                format!("{mnemonic} {rd}, [sp, #0x{off:03X}]")
            }
        }
        5 => {
            if (instr & (1 << 12)) == 0 {
                // Format 12: load address
                let src = if (instr & (1 << 11)) != 0 { "sp" } else { "pc" };
                let rd = reg(((instr >> 8) & 7) as u32);
                let off = ((instr & 0xFF) as u32) << 2;
                format!("add {rd}, {src}, #0x{off:03X}")
            } else if (instr & 0x0F00) == 0x0000 {
                // Format 13: add offset to SP
                let is_sub = (instr & 0x80) != 0;
                let off = ((instr & 0x7F) as u32) << 2;
                let mnemonic = if is_sub { "sub" } else { "add" };
                format!("{mnemonic} sp, #0x{off:03X}")
            } else if (instr & 0x0600) == 0x0400 {
                // Format 14: push/pop
                let is_pop = (instr & (1 << 11)) != 0;
                let extra = if (instr & (1 << 8)) != 0 {
                    Some(if is_pop { "pc" } else { "lr" })
                } else {
                    None
                };
                let mnemonic = if is_pop { "pop" } else { "push" };
                format!("{mnemonic} {}", format_reg_list(instr & 0xFF, extra))
            } else {
                format!("udf #0x{instr:04X}")
            }
        }
        6 => {
            if (instr & (1 << 12)) == 0 {
                // Format 15: multiple load/store
                let is_load = (instr & (1 << 11)) != 0;
                let rb = reg(((instr >> 8) & 7) as u32);
                let mnemonic = if is_load { "ldmia" } else { "stmia" };
                format!("{mnemonic} {rb}!, {}", format_reg_list(instr & 0xFF, None))
            } else {
                // Format 16 & 17: conditional branch / SWI
                let c = (instr >> 8) & 0xF;
                if c == 0xF {
                    format!("swi #0x{:02X}", instr & 0xFF)
                } else if c == 0xE {
                    format!("udf #0x{:02X}", instr & 0xFF)
                } else {
                    let off = ((instr as i8) as i32) << 1;
                    let tgt = addr.wrapping_add(4).wrapping_add(off as u32);
                    format!("b{} 0x{tgt:08X}", cond(c as u32))
                }
            }
        }
        _ => {
            // Format 18 & 19: unconditional branch / long branch with link
            let sub = (instr >> 11) & 3;
            match sub {
                0 => {
                    let off = (((instr & 0x7FF) as i32) << 21) >> 20;
                    let tgt = addr.wrapping_add(4).wrapping_add(off as u32);
                    format!("b 0x{tgt:08X}")
                }
                2 => {
                    let off = (((instr & 0x7FF) as i32) << 21) >> 9;
                    format!("bl_hi #0x{:06X}", off & 0xFF_FFFF)
                }
                3 => format!("bl_lo #0x{:03X}", (instr & 0x7FF) << 1),
                _ => format!("udf #0x{instr:04X}"),
            }
        }
    }
}

pub fn disassemble_arm(instr: u32, addr: u32) -> String {
    let c = cond(instr >> 28);
    if (instr & 0x0F00_0000) == 0x0F00_0000 {
        let imm = instr & 0x00FF_FFFF;
        let comment = (imm >> 16) & 0xFF;
        if (imm & 0xFFFF) == 0 {
            return format!("swi{c} #0x{comment:02X}");
        }
        return format!("swi{c} #0x{imm:06X}");
    }

    if (instr & 0x0FFF_FFF0) == 0x012F_FF10 {
        return format!("bx{c} {}", reg(instr & 0xF));
    }

    let bits27_25 = (instr >> 25) & 7;
    match bits27_25 {
        5 => {
            let link = if (instr & (1 << 24)) != 0 { "l" } else { "" };
            let off = (((instr & 0x00FF_FFFF) as i32) << 8) >> 6;
            let tgt = addr.wrapping_add(8).wrapping_add(off as u32);
            format!("b{link}{c} 0x{tgt:08X}")
        }
        4 => {
            let load = (instr & (1 << 20)) != 0;
            let u = (instr & (1 << 23)) != 0;
            let p = (instr & (1 << 24)) != 0;
            let w = if (instr & (1 << 21)) != 0 { "!" } else { "" };
            let s = if (instr & (1 << 22)) != 0 { "^" } else { "" };
            let mode = match (p, u) {
                (false, false) => "da",
                (false, true) => "ia",
                (true, false) => "db",
                (true, true) => "ib",
            };
            let rn = reg((instr >> 16) & 0xF);
            let mnemonic = if load { "ldm" } else { "stm" };
            format!(
                "{mnemonic}{mode}{c} {rn}{w}, {}{s}",
                format_reg_list((instr & 0xFFFF) as u16, None)
            )
        }
        2 | 3 => {
            if bits27_25 == 3 && (instr & 0x10) != 0 {
                return format!("udf{c} #0x{instr:08X}");
            }
            let load = (instr & (1 << 20)) != 0;
            let b = if (instr & (1 << 22)) != 0 { "b" } else { "" };
            let p = (instr & (1 << 24)) != 0;
            let u = (instr & (1 << 23)) != 0;
            let w = (instr & (1 << 21)) != 0;
            let t = if !p && w { "t" } else { "" };
            let rd = reg((instr >> 12) & 0xF);
            let rn = reg((instr >> 16) & 0xF);
            let mnemonic = if load { "ldr" } else { "str" };
            let sign = if u { "" } else { "-" };

            let offset_str = if bits27_25 == 2 {
                let imm = instr & 0xFFF;
                if imm == 0 {
                    String::new()
                } else {
                    format!(", #{sign}0x{imm:X}")
                }
            } else {
                format!(", {sign}{}", format_arm_shift(instr))
            };

            let addr_mode = if p {
                let wb = if w { "!" } else { "" };
                if ((instr >> 16) & 0xF) == 15 && bits27_25 == 2 && !w {
                    let off = instr & 0xFFF;
                    let ea = if u {
                        addr.wrapping_add(8).wrapping_add(off)
                    } else {
                        addr.wrapping_add(8).wrapping_sub(off)
                    };
                    format!("[{rn}{offset_str}] ; =0x{ea:08X}")
                } else {
                    format!("[{rn}{offset_str}]{wb}")
                }
            } else if offset_str.is_empty() {
                format!("[{rn}]")
            } else {
                format!("[{rn}]{offset_str}")
            };

            format!("{mnemonic}{b}{t}{c} {rd}, {addr_mode}")
        }
        0 | 1 => {
            // Decode special encodings with bit 25 == 0 & bits 7..4 == 1xx1
            if bits27_25 == 0 && (instr & 0x90) == 0x90 {
                let bits7_4 = (instr >> 4) & 0xF;
                if bits7_4 == 9 {
                    if (instr & 0x0FB0_0FF0) == 0x0100_0090 {
                        let b = if (instr & (1 << 22)) != 0 { "b" } else { "" };
                        let rd = reg((instr >> 12) & 0xF);
                        let rm = reg(instr & 0xF);
                        let rn = reg((instr >> 16) & 0xF);
                        return format!("swp{b}{c} {rd}, {rm}, [{rn}]");
                    }
                    if (instr & 0x0FC0_00F0) == 0x0000_0090 {
                        let acc = (instr & (1 << 21)) != 0;
                        let s = if (instr & (1 << 20)) != 0 { "s" } else { "" };
                        let rd = reg((instr >> 16) & 0xF);
                        let rn = reg((instr >> 12) & 0xF);
                        let rs = reg((instr >> 8) & 0xF);
                        let rm = reg(instr & 0xF);
                        return if acc {
                            format!("mla{s}{c} {rd}, {rm}, {rs}, {rn}")
                        } else {
                            format!("mul{s}{c} {rd}, {rm}, {rs}")
                        };
                    }
                    if (instr & 0x0F80_00F0) == 0x0080_0090 {
                        let signed = (instr & (1 << 22)) != 0;
                        let acc = (instr & (1 << 21)) != 0;
                        let s = if (instr & (1 << 20)) != 0 { "s" } else { "" };
                        let rdhi = reg((instr >> 16) & 0xF);
                        let rdlo = reg((instr >> 12) & 0xF);
                        let rs = reg((instr >> 8) & 0xF);
                        let rm = reg(instr & 0xF);
                        let mnemonic = match (signed, acc) {
                            (false, false) => "umull",
                            (false, true) => "umlal",
                            (true, false) => "smull",
                            (true, true) => "smlal",
                        };
                        return format!("{mnemonic}{s}{c} {rdlo}, {rdhi}, {rm}, {rs}");
                    }
                } else {
                    // Halfword & signed transfer
                    let load = (instr & (1 << 20)) != 0;
                    let sh = (instr >> 5) & 3;
                    let p = (instr & (1 << 24)) != 0;
                    let u = (instr & (1 << 23)) != 0;
                    let imm_flag = (instr & (1 << 22)) != 0;
                    let w = (instr & (1 << 21)) != 0;
                    let rd = reg((instr >> 12) & 0xF);
                    let rn = reg((instr >> 16) & 0xF);
                    let mnemonic = match (load, sh) {
                        (false, 1) => "strh",
                        (true, 1) => "ldrh",
                        (true, 2) => "ldrsb",
                        (true, 3) => "ldrsh",
                        _ => return format!("udf{c} #0x{instr:08X}"),
                    };
                    let sign = if u { "" } else { "-" };
                    let off_str = if imm_flag {
                        let off = ((instr >> 4) & 0xF0) | (instr & 0xF);
                        if off == 0 {
                            String::new()
                        } else {
                            format!(", #{sign}0x{off:X}")
                        }
                    } else {
                        format!(", {sign}{}", reg(instr & 0xF))
                    };
                    let mode = if p {
                        let wb = if w { "!" } else { "" };
                        format!("[{rn}{off_str}]{wb}")
                    } else if off_str.is_empty() {
                        format!("[{rn}]")
                    } else {
                        format!("[{rn}]{off_str}")
                    };
                    return format!("{mnemonic}{c} {rd}, {mode}");
                }
            }

            // MRS / MSR (when S == 0 and opcode in 8..=11)
            let opcode = ((instr >> 21) & 0xF) as usize;
            let s_bit = (instr & (1 << 20)) != 0;
            if !s_bit && (8..=11).contains(&opcode) {
                let psr = if (instr & (1 << 22)) != 0 {
                    "spsr"
                } else {
                    "cpsr"
                };
                if (opcode & 1) == 0 && bits27_25 == 0 {
                    let rd = reg((instr >> 12) & 0xF);
                    return format!("mrs{c} {rd}, {psr}");
                }
                let mut fields = String::new();
                if (instr & (1 << 16)) != 0 {
                    fields.push('c');
                }
                if (instr & (1 << 17)) != 0 {
                    fields.push('x');
                }
                if (instr & (1 << 18)) != 0 {
                    fields.push('s');
                }
                if (instr & (1 << 19)) != 0 {
                    fields.push('f');
                }
                let src = if bits27_25 == 1 {
                    let rot = ((instr >> 8) & 0xF) * 2;
                    let val = (instr & 0xFF).rotate_right(rot);
                    format!("#0x{val:X}")
                } else {
                    reg(instr & 0xF).to_string()
                };
                return format!("msr{c} {psr}_{fields}, {src}");
            }

            // Standard Data Processing
            let s = if s_bit && !(8..=11).contains(&opcode) {
                "s"
            } else {
                ""
            };
            let op_name = ARM_DP_OPS[opcode];
            let rd = reg((instr >> 12) & 0xF);
            let rn = reg((instr >> 16) & 0xF);
            let op2 = if bits27_25 == 1 {
                let rot = ((instr >> 8) & 0xF) * 2;
                let val = (instr & 0xFF).rotate_right(rot);
                format!("#0x{val:X}")
            } else {
                format_arm_shift(instr)
            };

            if opcode == 13 || opcode == 15 {
                format!("{op_name}{s}{c} {rd}, {op2}")
            } else if (8..=11).contains(&opcode) {
                format!("{op_name}{c} {rn}, {op2}")
            } else {
                format!("{op_name}{s}{c} {rd}, {rn}, {op2}")
            }
        }
        _ => format!("cdp/stc{c} #0x{instr:08X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_disassembly_core_encodings() {
        assert_eq!(disassemble_arm(0xE3A0_00FF, 0x0800_0000), "mov r0, #0xFF");
        assert_eq!(disassemble_arm(0xE081_2003, 0x0800_0000), "add r2, r1, r3");
        assert_eq!(disassemble_arm(0x0A00_0002, 0x0800_0000), "beq 0x08000010");
        assert_eq!(
            disassemble_arm(0xE92D_401F, 0x0800_0000),
            "stmdb sp!, {r0-r4, lr}"
        );
        assert_eq!(disassemble_arm(0xE12F_FF1E, 0x0800_0000), "bx lr");
        assert_eq!(disassemble_arm(0xE10F_0000, 0x0800_0000), "mrs r0, cpsr");
        assert_eq!(
            disassemble_arm(0xE59F_1010, 0x0800_0000),
            "ldr r1, [pc, #0x10] ; =0x08000018"
        );
    }

    #[test]
    fn thumb_disassembly_core_encodings() {
        assert_eq!(disassemble_thumb(0x2010, 0x0800_0000), "mov r0, #0x10");
        assert_eq!(disassemble_thumb(0x46C0, 0x0800_0000), "nop");
        assert_eq!(disassemble_thumb(0xB51F, 0x0800_0000), "push {r0-r4, lr}");
        assert_eq!(disassemble_thumb(0xBD00, 0x0800_0000), "pop {pc}");
        assert_eq!(disassemble_thumb(0xDF11, 0x0800_0000), "swi #0x11");
        assert_eq!(disassemble_thumb(0x4770, 0x0800_0000), "bx lr");
        assert_eq!(disassemble_thumb(0xD0FE, 0x0800_0004), "beq 0x08000004");
    }
}
