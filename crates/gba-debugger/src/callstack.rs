use alloc::vec::Vec;
use gba_core::{DebugControl, GbaCore, StopReason};

/// Maximum number of nested call frames retained by [`CallStackTracker`].
pub const MAX_CALL_STACK_DEPTH: usize = 64;

/// A single recorded subroutine call or exception frame on the call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFrame {
    /// Address of the call instruction (or interrupted PC for exceptions).
    pub call_site_pc: u32,
    /// Target address entered by the call or exception vector.
    pub entry_addr: u32,
    /// Address execution is expected to return to when this frame unwinds.
    pub return_addr: u32,
    /// Stack pointer (`SP` / `R13`) captured when entering the call.
    pub stack_pointer: u32,
    /// Whether the caller was executing in Thumb state.
    pub thumb: bool,
    /// Whether this frame was created by an exception (IRQ/SWI) rather than a call.
    pub is_irq: bool,
}

/// Evaluates an ARMv4T condition code (top 4 bits `0..=15`) against `cpsr`.
pub fn arm_cond_passed(cpsr: u32, cond: u32) -> bool {
    let n = (cpsr & (1 << 31)) != 0;
    let z = (cpsr & (1 << 30)) != 0;
    let c = (cpsr & (1 << 29)) != 0;
    let v = (cpsr & (1 << 28)) != 0;
    match cond & 0xF {
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

#[inline]
fn is_arm_bx_non_lr(op: u32) -> bool {
    (op >> 28) != 0xF && (op & 0x0FFF_FFF0) == 0x012F_FF10 && (op & 0xF) != 14
}

#[inline]
fn is_arm_b(op: u32) -> bool {
    (op >> 28) != 0xF && (op & 0x0F00_0000) == 0x0A00_0000
}

#[inline]
fn is_thumb_bx_non_lr(op: u16) -> bool {
    (op & 0xFF87) == 0x4700 && ((op >> 3) & 0xF) != 14
}

#[inline]
fn is_thumb_b_uncond(op: u16) -> bool {
    (op & 0xF800) == 0xE000
}

/// Returns the return address after the instruction at `pc` if it initiates a subroutine call.
///
/// Recognizes:
/// - ARM `BL` (`cond` != `0xF`) -> `pc + 4`
/// - ARM `MOV LR, PC` followed by `B` or `BX Rm` (`Rm != LR`) -> `pc + 8`
/// - ARM `BX Rm` (`Rm != LR`) / `B` when `LR & !1 == pc + 4` -> `pc + 4`
/// - Thumb long `BL` (`0xF000..=0xF7FF` followed by `0xF800..=0xFFFF`) -> `pc + 4`
/// - Thumb `MOV LR, PC` (`0x46FE`) followed by `B` or `BX Rm` (`Rm != LR`) -> `pc + 4`
/// - Thumb `BX Rm` (`Rm != LR`) / `B` when `LR & !1 == pc + 2` -> `pc + 2`
pub fn is_call_instruction(core: &GbaCore, pc: u32, thumb: bool) -> Option<u32> {
    if thumb {
        let op = core.peek16(pc);
        if (op & 0xF800) == 0xF000 {
            let op2 = core.peek16(pc.wrapping_add(2));
            if (op2 & 0xF800) == 0xF800 {
                return Some(pc.wrapping_add(4));
            }
        }
        if op == 0x46FE {
            let op2 = core.peek16(pc.wrapping_add(2));
            if is_thumb_bx_non_lr(op2) || is_thumb_b_uncond(op2) {
                return Some(pc.wrapping_add(4));
            }
        }
        if (is_thumb_bx_non_lr(op) || is_thumb_b_uncond(op))
            && (core.cpu.gpr[14] & !1) == pc.wrapping_add(2)
        {
            return Some(pc.wrapping_add(2));
        }
        None
    } else {
        let op = core.peek32(pc);
        if (op >> 28) == 0xF {
            return None;
        }
        if (op & 0x0F00_0000) == 0x0B00_0000 {
            return Some(pc.wrapping_add(4));
        }
        if (op & 0x0FFF_FFFF) == 0x01A0_E00F {
            let op2 = core.peek32(pc.wrapping_add(4));
            if is_arm_bx_non_lr(op2) || is_arm_b(op2) {
                return Some(pc.wrapping_add(8));
            }
        }
        if (is_arm_bx_non_lr(op) || is_arm_b(op)) && (core.cpu.gpr[14] & !1) == pc.wrapping_add(4) {
            return Some(pc.wrapping_add(4));
        }
        None
    }
}

/// Returns `true` if the instruction at `pc` would execute as a subroutine or exception return
/// given `cpsr`.
pub fn is_return_instruction(core: &GbaCore, pc: u32, thumb: bool, cpsr: u32) -> bool {
    if thumb {
        let op = core.peek16(pc);
        // BX LR
        if (op & 0xFF87) == 0x4700 && ((op >> 3) & 0xF) == 14 {
            return true;
        }
        // POP {..., pc}
        if (op & 0xFF00) == 0xBD00 {
            return true;
        }
        // MOV pc, lr
        if op == 0x46F7 {
            return true;
        }
        false
    } else {
        let op = core.peek32(pc);
        if !arm_cond_passed(cpsr, op >> 28) {
            return false;
        }
        // BX LR
        if (op & 0x0FFF_FFFF) == 0x012F_FF1E {
            return true;
        }
        // LDM {..., pc} (all addressing modes, with or without S bit)
        if (op & 0x0E10_8000) == 0x0810_8000 {
            return true;
        }
        // SUB / SUBS pc, lr, #imm
        if (op & 0x0FEF_F000) == 0x024E_F000 {
            return true;
        }
        // MOV / MOVS pc, lr
        if (op & 0x0FEF_FFFF) == 0x01A0_F00E {
            return true;
        }
        // LDR pc, [sp, ...] (imm or unshifted/shifted reg, excluding [pc, ...])
        if (op & 0x0E1F_F000) == 0x041D_F000 || (op & 0x0E1F_F010) == 0x061D_F000 {
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy)]
enum PendingStepKind {
    None,
    ArmBl {
        call_site_pc: u32,
        return_addr: u32,
        sp: u32,
    },
    ThumbBlSecond {
        call_site_pc: u32,
        return_addr: u32,
        sp: u32,
    },
    LinkedBranch {
        call_site_pc: u32,
        return_addr: u32,
        sp: u32,
        thumb: bool,
    },
    Return,
}

/// Snapshot captured before executing a single instruction for [`CallStackTracker::finish_step`].
#[derive(Debug, Clone, Copy)]
pub struct PreStepSnapshot {
    old_pc: u32,
    old_cpsr: u32,
    kind: PendingStepKind,
}

/// Tracks active call frames (up to [`MAX_CALL_STACK_DEPTH`]) across single steps.
#[derive(Debug, Default, Clone)]
pub struct CallStackTracker {
    frames: Vec<CallFrame>,
}

impl CallStackTracker {
    /// Creates an empty call stack tracker.
    pub fn new() -> Self {
        Self { frames: Vec::new() }
    }

    /// Clears all tracked frames (e.g., on ROM load or CPU reset).
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Truncates the call stack to at most `len` outermost frames.
    pub fn truncate(&mut self, len: usize) {
        self.frames.truncate(len);
    }

    /// Returns active frames ordered from outermost caller (`[0]`) to innermost (`last()`).
    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// Returns the expected return address for the innermost active frame, if any.
    pub fn current_return_addr(&self) -> Option<u32> {
        self.frames.last().map(|f| f.return_addr)
    }

    /// Pushes a frame while enforcing [`MAX_CALL_STACK_DEPTH`].
    pub fn push_frame(&mut self, frame: CallFrame) {
        if self.frames.len() >= MAX_CALL_STACK_DEPTH {
            self.frames.remove(0);
        }
        self.frames.push(frame);
    }

    /// Captures pre-step instruction context before executing one CPU step.
    pub fn prepare_step(&self, core: &GbaCore) -> PreStepSnapshot {
        let old_pc = core.cpu.current_exec_pc();
        let old_cpsr = core.cpu.cpsr;
        let thumb = core.cpu.is_thumb();
        let sp = core.cpu.gpr[13];
        let lr = core.cpu.gpr[14];

        let kind = if thumb {
            let op = core.peek16(old_pc);
            if (op & 0xF800) == 0xF800 {
                // Second halfword of Thumb BL actually performs the jump & sets LR.
                let call_site = if (core.peek16(old_pc.wrapping_sub(2)) & 0xF800) == 0xF000 {
                    old_pc.wrapping_sub(2)
                } else {
                    old_pc
                };
                PendingStepKind::ThumbBlSecond {
                    call_site_pc: call_site,
                    return_addr: old_pc.wrapping_add(2),
                    sp,
                }
            } else if (is_thumb_bx_non_lr(op) || is_thumb_b_uncond(op))
                && (lr & !1) == old_pc.wrapping_add(2)
            {
                PendingStepKind::LinkedBranch {
                    call_site_pc: old_pc.wrapping_sub(2),
                    return_addr: old_pc.wrapping_add(2),
                    sp,
                    thumb: true,
                }
            } else if is_return_instruction(core, old_pc, true, old_cpsr) {
                PendingStepKind::Return
            } else {
                PendingStepKind::None
            }
        } else {
            let op = core.peek32(old_pc);
            if arm_cond_passed(old_cpsr, op >> 28) {
                if (op & 0x0F00_0000) == 0x0B00_0000 {
                    PendingStepKind::ArmBl {
                        call_site_pc: old_pc,
                        return_addr: old_pc.wrapping_add(4),
                        sp,
                    }
                } else if (is_arm_bx_non_lr(op) || is_arm_b(op))
                    && (lr & !1) == old_pc.wrapping_add(4)
                {
                    PendingStepKind::LinkedBranch {
                        call_site_pc: old_pc.wrapping_sub(4),
                        return_addr: old_pc.wrapping_add(4),
                        sp,
                        thumb: false,
                    }
                } else if is_return_instruction(core, old_pc, false, old_cpsr) {
                    PendingStepKind::Return
                } else {
                    PendingStepKind::None
                }
            } else {
                PendingStepKind::None
            }
        };

        PreStepSnapshot {
            old_pc,
            old_cpsr,
            kind,
        }
    }

    /// Updates the call stack after one CPU step completes.
    pub fn finish_step(&mut self, core: &GbaCore, pre: PreStepSnapshot) {
        let new_pc = core.cpu.current_exec_pc();
        let new_cpsr = core.cpu.cpsr;
        let old_mode = pre.old_cpsr & 0x1F;
        let new_mode = new_cpsr & 0x1F;

        // Hardware IRQ entry preempts normal flow before normal step effects take place.
        if new_mode == 0x12 && old_mode != 0x12 && new_pc == 0x0000_0018 {
            let ret = core.cpu.gpr[14].wrapping_sub(4);
            self.push_frame(CallFrame {
                call_site_pc: pre.old_pc,
                entry_addr: 0x0000_0018,
                return_addr: ret,
                stack_pointer: core.cpu.gpr[13],
                thumb: (pre.old_cpsr & (1 << 5)) != 0,
                is_irq: true,
            });
            return;
        }

        // Software interrupt entry to BIOS SWI vector (real BIOS path).
        if new_mode == 0x13 && old_mode != 0x13 && new_pc == 0x0000_0008 {
            self.push_frame(CallFrame {
                call_site_pc: pre.old_pc,
                entry_addr: 0x0000_0008,
                return_addr: core.cpu.gpr[14],
                stack_pointer: core.cpu.gpr[13],
                thumb: (pre.old_cpsr & (1 << 5)) != 0,
                is_irq: true,
            });
            return;
        }

        match pre.kind {
            PendingStepKind::ArmBl {
                call_site_pc,
                return_addr,
                sp,
            } => {
                self.push_frame(CallFrame {
                    call_site_pc,
                    entry_addr: new_pc,
                    return_addr,
                    stack_pointer: sp,
                    thumb: false,
                    is_irq: false,
                });
            }
            PendingStepKind::ThumbBlSecond {
                call_site_pc,
                return_addr,
                sp,
            } => {
                self.push_frame(CallFrame {
                    call_site_pc,
                    entry_addr: new_pc,
                    return_addr,
                    stack_pointer: sp,
                    thumb: true,
                    is_irq: false,
                });
            }
            PendingStepKind::LinkedBranch {
                call_site_pc,
                return_addr,
                sp,
                thumb,
            } => {
                self.push_frame(CallFrame {
                    call_site_pc,
                    entry_addr: new_pc,
                    return_addr,
                    stack_pointer: sp,
                    thumb,
                    is_irq: false,
                });
            }
            PendingStepKind::Return => {
                if self.frames.is_empty() {
                    return;
                }
                let target = new_pc & !1;
                if let Some(idx) = self
                    .frames
                    .iter()
                    .rposition(|f| (f.return_addr & !1) == target)
                {
                    self.frames.truncate(idx);
                } else {
                    self.frames.pop();
                }
            }
            PendingStepKind::None => {}
        }
    }

    /// Executes one debug step while updating the tracked call stack.
    pub fn step_one_with_tracker(
        &mut self,
        core: &mut GbaCore,
        ctrl: &DebugControl,
    ) -> Option<StopReason> {
        let pre = self.prepare_step(core);
        let reason = core.step_one_debug(ctrl);
        self.finish_step(core, pre);
        reason
    }
}

/// Computes the best Step-Out target return address for the current execution point.
///
/// Prefers the innermost frame on `tracker`; falls back to `LR` (`gpr[14]`) when it
/// holds a plausible GBA executable address distinct from the current PC.
pub fn step_out_return_addr(core: &GbaCore, tracker: &CallStackTracker) -> Option<u32> {
    if let Some(ret) = tracker.current_return_addr() {
        return Some(ret & !1);
    }
    let lr = core.cpu.gpr[14] & !1;
    let pc = core.cpu.current_exec_pc() & !1;
    let in_code_region = lr < 0x0000_4000
        || (0x0200_0000..0x0204_0000).contains(&lr)
        || (0x0300_0000..0x0300_8000).contains(&lr)
        || (0x0800_0000..0x0E00_0000).contains(&lr);
    if lr != 0 && lr != pc && in_code_region {
        Some(lr)
    } else {
        None
    }
}
