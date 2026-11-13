#![cfg_attr(not(feature = "std"), no_std)]
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::new_without_default
)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod apu;
pub mod blip;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod gamedb;
pub mod hw;
pub mod jit;
pub mod ppu;
pub mod serde_helpers;

use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub use crate::bus::{SysBus, Watchpoint, WatchpointHit, WatchpointKind};
use crate::cartridge::SaveType;
use crate::cpu::Arm7tdmi;
pub use crate::hw::{SioMode, SioResponse, SioTransfer};

pub const CPU_CLOCK_HZ: u32 = 16_777_216;
pub const CYCLES_PER_SCANLINE: u64 = 1_232;
pub const CYCLES_PER_FRAME: u64 = 280_896;
pub const LCD_WIDTH: usize = 240;
pub const LCD_HEIGHT: usize = 160;
pub const SAMPLE_RATE_HZ: u32 = 48_000;
pub const APU_SAMPLE_INTERVAL: u64 = 512; // 16_777_216 / 32_768
pub const APU_FRAME_SEQ_INTERVAL: u64 = 32_768; // 512 Hz PSG frame sequencer

// Active-high masks to construct active-low KEYINPUT register via `encode_keyinput`.
pub const KEY_A: u16 = 1 << 0;
pub const KEY_B: u16 = 1 << 1;
pub const KEY_SELECT: u16 = 1 << 2;
pub const KEY_START: u16 = 1 << 3;
pub const KEY_RIGHT: u16 = 1 << 4;
pub const KEY_LEFT: u16 = 1 << 5;
pub const KEY_UP: u16 = 1 << 6;
pub const KEY_DOWN: u16 = 1 << 7;
pub const KEY_R: u16 = 1 << 8;
pub const KEY_L: u16 = 1 << 9;

#[inline]
pub const fn encode_keyinput(pressed_mask: u16) -> u16 {
    (!pressed_mask) & 0x03FF
}

pub trait HostFrontend {
    /// Called when the PPU finishes VDraw of a frame (scanline 160).
    /// Framebuffer contains 240x160 pixels in GBA BGR555 (0..0x7FFF) format.
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT]);

    /// Called when the APU has stereo interleaved signed 16-bit PCM samples ready.
    fn push_audio_samples(&mut self, samples_interleaved_i16: &[i16]);

    /// Returns active-low 10-bit KEYINPUT register state (`0x03FF` = no buttons pressed).
    fn poll_keypad(&mut self) -> u16;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    None,
    PpuHDrawEnd,
    PpuHBlank,
    PpuDotStep,
    TimerOverflow(u8),
    DmaActivate(u8),
    ApuSampleTick,
    IrqSynchronizer,
    SioTransferComplete,
}

impl EventType {
    /// Returns the fixed scheduler slot index for this event variant, or `None` for `EventType::None`.
    #[inline]
    pub const fn slot_index(self) -> Option<usize> {
        match self {
            EventType::None => None,
            EventType::PpuHDrawEnd => Some(0),
            EventType::PpuHBlank => Some(1),
            EventType::PpuDotStep => Some(2),
            EventType::TimerOverflow(ch) if ch < 4 => Some(3 + ch as usize),
            EventType::DmaActivate(ch) if ch < 4 => Some(7 + ch as usize),
            EventType::ApuSampleTick => Some(11),
            EventType::IrqSynchronizer => Some(12),
            EventType::SioTransferComplete => Some(13),
            EventType::TimerOverflow(_) | EventType::DmaActivate(_) => None,
        }
    }
}

pub const SCHEDULER_SLOTS: usize = 14;

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub cycle: u64,
    pub kind: EventType,
    pub active: bool,
}

impl ScheduledEvent {
    pub const fn empty() -> Self {
        Self {
            cycle: u64::MAX,
            kind: EventType::None,
            active: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Scheduler {
    pub current_cycle: u64,
    pub next_event_cycle: u64,
    pub events: [ScheduledEvent; SCHEDULER_SLOTS],
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            current_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::empty(); SCHEDULER_SLOTS],
        }
    }

    pub fn schedule(&mut self, target_cycle: u64, kind: EventType) {
        let Some(idx) = kind.slot_index() else {
            return;
        };
        self.events[idx] = ScheduledEvent {
            cycle: target_cycle,
            kind,
            active: true,
        };
        self.recompute_next();
    }

    pub fn cancel(&mut self, kind: EventType) {
        let Some(idx) = kind.slot_index() else {
            return;
        };
        self.events[idx] = ScheduledEvent::empty();
        self.recompute_next();
    }

    #[inline]
    pub fn pop_due(&mut self) -> Option<(u64, EventType)> {
        if self.current_cycle < self.next_event_cycle {
            return None;
        }
        let mut best_idx = None;
        let mut best_cycle = u64::MAX;
        for (i, ev) in self.events.iter().enumerate() {
            if ev.active && ev.cycle <= self.current_cycle && ev.cycle < best_cycle {
                best_cycle = ev.cycle;
                best_idx = Some(i);
            }
        }
        let idx = best_idx?;
        let kind = self.events[idx].kind;
        self.events[idx] = ScheduledEvent::empty();
        self.recompute_next();
        Some((best_cycle, kind))
    }

    #[inline]
    pub fn pop_due_timer_until(&mut self, limit_cycle: u64) -> Option<(u64, u8)> {
        if limit_cycle < self.next_event_cycle {
            return None;
        }
        let mut best_idx = None;
        let mut best_cycle = u64::MAX;
        for idx in 3..7 {
            let ev = &self.events[idx];
            if ev.active && ev.cycle <= limit_cycle && ev.cycle < best_cycle {
                best_cycle = ev.cycle;
                best_idx = Some(idx);
            }
        }
        let idx = best_idx?;
        let ch = (idx - 3) as u8;
        self.events[idx] = ScheduledEvent::empty();
        self.recompute_next();
        Some((best_cycle, ch))
    }

    fn recompute_next(&mut self) {
        let mut min_cycle = u64::MAX;
        for ev in self.events.iter() {
            if ev.active && ev.cycle < min_cycle {
                min_cycle = ev.cycle;
            }
        }
        self.next_event_cycle = min_cycle;
    }

    pub fn shift_by(&mut self, delta: u64) {
        self.current_cycle = self.current_cycle.saturating_add(delta);
        if self.next_event_cycle != u64::MAX {
            self.next_event_cycle = self.next_event_cycle.saturating_add(delta);
        }
        for ev in self.events.iter_mut() {
            if ev.active && ev.cycle != u64::MAX {
                ev.cycle = ev.cycle.saturating_add(delta);
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemoryRegionKind {
    Ewram,
    Iwram,
    PaletteRam,
    Vram,
    Oam,
    Sram,
}

#[derive(Copy, Clone, Debug)]
pub struct MemoryRegionSlice<'a> {
    pub kind: MemoryRegionKind,
    pub name: &'static str,
    pub base: u32,
    pub len: usize,
    pub data: &'a [u8],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    FrameComplete,
    Breakpoint(u32),
    Watchpoint {
        addr: u32,
        pc: u32,
        write: bool,
        value: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DebugControl {
    pub breakpoints: Vec<u32>,
    pub watchpoints: Vec<Watchpoint>,
}

impl DebugControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_breakpoints(breakpoints: &[u32]) -> Self {
        Self {
            breakpoints: breakpoints.to_vec(),
            watchpoints: Vec::new(),
        }
    }
}

enum InternalStep {
    Continue,
    StoppedFrame(u64),
    DebugStop(StopReason),
}

#[derive(Serialize, Deserialize)]
struct SaveStatePayload {
    cpu: Arm7tdmi,
    bus: SysBus,
}

/// Borrowed twin of [`SaveStatePayload`]; serde encodes `&T` exactly like `T`, so this produces
/// the same bytes without cloning the bus (which would otherwise copy the multi-MiB ROM).
#[derive(Serialize)]
struct SaveStatePayloadRef<'a> {
    cpu: &'a Arm7tdmi,
    bus: &'a SysBus,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SubsystemTiming {
    pub total_time: core::time::Duration,
    pub cpu_time: core::time::Duration,
    pub ppu_time: core::time::Duration,
    pub apu_time: core::time::Duration,
    pub dma_time: core::time::Duration,
    pub bus_time: core::time::Duration,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FunctionProfile {
    pub fetch_cycles: u64,
    pub decode_exec_cycles: u64,
    pub mem_read_cycles: u64,
    pub mem_write_cycles: u64,
    pub branch_flush_cycles: u64,
    pub ppu_cycles: u64,
    pub apu_cycles: u64,
    pub dma_cycles: u64,
}

pub struct GbaCore {
    pub cpu: Arm7tdmi,
    pub bus: SysBus,
    pub idle_skip_enabled: bool,
    pending_idle_check_pc: Option<u32>,
    pub jit: jit::JitEngine,
}

impl GbaCore {
    pub fn new(bios: Option<&[u8]>, rom: Vec<u8>) -> Self {
        let mut bus = SysBus::new(bios, rom);
        let cpu = Arm7tdmi::new(&mut bus);
        Self {
            cpu,
            bus,
            idle_skip_enabled: true,
            pending_idle_check_pc: None,
            jit: jit::JitEngine::new(),
        }
    }

    pub fn reset(&mut self) {
        let bios = if self.bus.bios_present {
            Some(self.bus.bios.to_vec())
        } else {
            None
        };
        self.reset_with_bios(bios.as_deref());
    }

    pub fn load_bios(&mut self, bios: Option<&[u8]>) {
        self.reset_with_bios(bios);
    }

    fn reset_with_bios(&mut self, bios: Option<&[u8]>) {
        let rom = core::mem::take(&mut self.bus.cart.rom);
        let save_type = self.bus.cart.save_type;
        let save_type_override = self.bus.cart.save_type_override;
        let flash_id_override = self.bus.cart.flash.id_override;
        let idle_loop_addr = self.bus.cart.idle_loop_addr;
        let rtc_override = self.bus.cart.rtc_override;
        let solar_lux = self.bus.cart.solar_lux;
        let solar_enabled = self.bus.cart.solar_enabled;
        let tilt_enabled = self.bus.cart.tilt_enabled;
        let tilt_x = self.bus.cart.tilt_x;
        let tilt_y = self.bus.cart.tilt_y;
        let gyro_enabled = self.bus.cart.gyro_enabled;
        let gyro_val = self.bus.cart.gyro_val;
        let rumble_enabled = self.bus.cart.rumble_enabled;
        let saved_sram = self.bus.cart.sram.clone();

        let dot_mode = self.bus.ppu.dot_accurate_mode;
        let layer_mask = self.bus.ppu.layer_enable_mask;
        let chan_mask = self.bus.apu.channel_enable_mask;
        let master_vol = self.bus.apu.master_volume_pct;
        let idle_skip_enabled = self.idle_skip_enabled;
        let fifo_interp = self.bus.apu.fifo_interpolation;

        *self = Self::new(bios, rom);
        self.idle_skip_enabled = idle_skip_enabled;
        self.bus.cart.save_type = save_type;
        self.bus.set_save_type_override(save_type_override);
        self.bus.cart.set_flash_id_override(flash_id_override);
        self.bus.cart.idle_loop_addr = idle_loop_addr;
        self.bus.cart.rtc_override = rtc_override;
        self.bus.cart.solar_lux = solar_lux;
        self.bus.cart.solar_enabled = solar_enabled;
        self.bus.cart.tilt_enabled = tilt_enabled;
        self.bus.cart.tilt_x = tilt_x;
        self.bus.cart.tilt_y = tilt_y;
        self.bus.cart.gyro_enabled = gyro_enabled;
        self.bus.cart.gyro_val = gyro_val;
        self.bus.cart.rumble_enabled = rumble_enabled;
        self.bus.cart.sram = saved_sram;

        self.bus.ppu.dot_accurate_mode = dot_mode;
        self.bus.ppu.layer_enable_mask = layer_mask;
        self.bus.apu.channel_enable_mask = chan_mask;
        self.bus.apu.master_volume_pct = master_vol;
        self.bus.apu.fifo_interpolation = fifo_interp;
    }

    /// Mimics GBA SWI 0x00 SoftReset: resets stacks, clears top 0x200 bytes of IWRAM, and restarts entry.
    pub fn soft_reset(&mut self) {
        let boot_ewram = self.bus.iwram[0x7FFA] != 0;
        self.bus.iwram[0x7E00..0x8000].fill(0);
        self.cpu.halted = false;
        self.cpu.stopped = false;
        self.pending_idle_check_pc = None;
        self.bus.hw.halt_requested = false;
        self.bus.hw.stop_requested = false;
        self.bus.hw.ime = 0;
        self.bus.hw.ie = 0;
        self.bus.hw.if_flag = 0;
        self.cpu.cpsr = 0x1F; // System mode, ARM, IRQ/FIQ enabled
        for r in &mut self.cpu.gpr[..13] {
            *r = 0;
        }
        self.cpu.gpr[13] = 0x0300_7F00;
        self.cpu.gpr[14] = 0;
        self.cpu.banked_r13[cpu::CpuMode::System.bank_idx()] = 0x0300_7F00;
        self.cpu.banked_r13[cpu::CpuMode::Irq.bank_idx()] = 0x0300_7FA0;
        self.cpu.banked_r13[cpu::CpuMode::Supervisor.bank_idx()] = 0x0300_7FE0;
        let entry = if boot_ewram { 0x0200_0000 } else { 0x0800_0000 };
        self.cpu.flush_pipeline(&mut self.bus, entry);
    }

    pub fn step_scanline(&mut self) {
        let start_vcount = self.bus.ppu.vcount;
        let limit = self.bus.scheduler.current_cycle + CYCLES_PER_SCANLINE * 2;
        while self.bus.ppu.vcount == start_vcount && self.bus.scheduler.current_cycle < limit {
            self.step_one();
        }
    }

    #[inline]
    pub fn step_one(&mut self) {
        let _ = self.step_internal(&[], None, false);
    }

    pub fn step_one_debug(&mut self, ctrl: &DebugControl) -> Option<StopReason> {
        let has_watchpoints = !ctrl.watchpoints.is_empty();
        if has_watchpoints {
            self.bus.set_watchpoints(&ctrl.watchpoints);
        }
        let res = match self.step_internal(&ctrl.breakpoints, None, false) {
            InternalStep::DebugStop(reason) => Some(reason),
            InternalStep::Continue | InternalStep::StoppedFrame(_) => None,
        };
        if has_watchpoints {
            self.bus.clear_watchpoints();
        }
        res
    }

    #[inline]
    pub fn step_jit(&mut self) -> u64 {
        let pc = self.cpu.current_exec_pc();
        self.bus.debug_pc = pc;
        let is_thumb = self.cpu.is_thumb();
        let key = jit::BlockKey { pc, is_thumb };

        let block = self
            .jit
            .get_block(&key)
            .map(|b| (b.func_ptr, b.cycle_count))
            .or_else(|| {
                self.jit
                    .compile_block(&mut self.cpu, &mut self.bus, key)
                    .map(|b| (b.func_ptr, b.cycle_count))
            });

        if let Some((func_ptr, block_cyc)) = block {
            if self.bus.scheduler.current_cycle + block_cyc >= self.bus.scheduler.next_event_cycle
                || self.bus.hw.active_dma.is_some()
                || (self.bus.hw.ime != 0
                    && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                    && !self.cpu.is_irq_disabled())
            {
                return self.cpu.step(&mut self.bus);
            }
            unsafe { (func_ptr)(&mut self.cpu as *mut _, &mut self.bus as *mut _) }
        } else {
            self.cpu.step(&mut self.bus)
        }
    }

    /// Runs this core in lockstep against a reference interpreter core for `frames` frames,
    /// asserting 100% equivalence of GPRs, CPSR, scheduler cycles, and memory state after every frame.
    pub fn run_differential_test<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        reference_rom: &[u8],
        frames: usize,
    ) -> Result<(), String> {
        let mut ref_core = GbaCore::new(None, reference_rom.to_vec());
        ref_core.jit.enabled = false;
        self.jit.enabled = self.jit.available;

        for frame in 0..frames {
            self.run_frame(frontend);
            ref_core.run_frame(frontend);

            if self.cpu.gpr != ref_core.cpu.gpr {
                return Err(format!(
                    "Divergence at frame {frame}: GPR mismatch!\nJIT: {:08X?}\nREF: {:08X?}",
                    self.cpu.gpr, ref_core.cpu.gpr
                ));
            }
            if self.cpu.cpsr != ref_core.cpu.cpsr {
                return Err(format!(
                    "Divergence at frame {frame}: CPSR mismatch!\nJIT: 0x{:08X}\nREF: 0x{:08X}",
                    self.cpu.cpsr, ref_core.cpu.cpsr
                ));
            }
            if self.bus.scheduler.current_cycle != ref_core.bus.scheduler.current_cycle {
                return Err(format!(
                    "Divergence at frame {frame}: Cycle count mismatch!\nJIT: {}\nREF: {}",
                    self.bus.scheduler.current_cycle, ref_core.bus.scheduler.current_cycle
                ));
            }
        }
        Ok(())
    }

    fn step_internal(
        &mut self,
        breakpoints: &[u32],
        halt_end_cycle: Option<u64>,
        allow_idle_skip: bool,
    ) -> InternalStep {
        if let Some(page) = self.bus.code_page_invalidated.take() {
            self.jit.invalidate_page(page);
        }

        while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
            self.bus.dispatch_event(ev_cycle, kind);
        }

        if self.bus.hw.stop_requested {
            self.cpu.stopped = true;
            self.cpu.halted = false;
            self.bus.hw.stop_requested = false;
        }
        if self.bus.hw.halt_requested {
            if !self.cpu.stopped {
                self.cpu.halted = true;
            }
            self.bus.hw.halt_requested = false;
        }

        if self.cpu.stopped {
            if (self.bus.hw.ie & self.bus.hw.if_flag & hw::IRQ_STOP_WAKE_MASK) != 0 {
                self.cpu.stopped = false;
            } else {
                let delta = halt_end_cycle
                    .map(|end| end.saturating_sub(self.bus.scheduler.current_cycle).max(1))
                    .unwrap_or(4);
                self.bus.apu.flush(self.bus.scheduler.current_cycle);
                self.bus.scheduler.shift_by(delta);
                self.bus.hw.shift_timers_by(delta);
                self.bus.apu.on_stop_shift(self.bus.scheduler.current_cycle);
                self.bus.ppu.framebuffer.fill(0x7FFF);
                if halt_end_cycle.is_some() {
                    return InternalStep::StoppedFrame(delta);
                }
                return InternalStep::Continue;
            }
        }

        if self.bus.hw.active_dma.is_some() {
            self.bus.debug_pc = self.cpu.current_exec_pc();
            if self.bus.watchpoint_active {
                self.bus.watchpoint_hit = None;
            }
            let cycles = self.bus.step_active_dma();
            self.bus.scheduler.current_cycle += cycles.max(1);
            if let Some(hit) = self.bus.watchpoint_hit.take() {
                return InternalStep::DebugStop(StopReason::Watchpoint {
                    addr: hit.addr,
                    pc: hit.pc,
                    write: hit.write,
                    value: hit.value,
                });
            }
            return InternalStep::Continue;
        }

        if self.cpu.halted {
            if (self.bus.hw.ie & self.bus.hw.if_flag) != 0 {
                self.cpu.halted = false;
            } else {
                let next = match halt_end_cycle {
                    Some(end) => self.bus.scheduler.next_event_cycle.min(end),
                    None => self.bus.scheduler.next_event_cycle,
                };
                if next != u64::MAX && next > self.bus.scheduler.current_cycle {
                    self.bus.scheduler.current_cycle = next;
                } else {
                    self.bus.scheduler.current_cycle += 4;
                }
                return InternalStep::Continue;
            }
        }

        if self.bus.hw.ime != 0
            && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
            && !self.cpu.is_irq_disabled()
            && self.bus.irq_sync_delay == 0
        {
            self.pending_idle_check_pc = None;
            self.cpu.trigger_irq(&mut self.bus);
            self.bus.scheduler.current_cycle += 3;
            return InternalStep::Continue;
        }

        if self.bus.irq_sync_delay > 0 {
            self.bus.irq_sync_delay = self.bus.irq_sync_delay.saturating_sub(1);
        }

        let exec_pc = self.cpu.current_exec_pc();
        if !breakpoints.is_empty() && breakpoints.contains(&exec_pc) {
            return InternalStep::DebugStop(StopReason::Breakpoint(exec_pc));
        }

        if allow_idle_skip && self.pending_idle_check_pc == Some(exec_pc) {
            self.pending_idle_check_pc = None;
            if self.is_safe_idle_loop(exec_pc) {
                let target = match halt_end_cycle {
                    Some(end) => self.bus.scheduler.next_event_cycle.min(end),
                    None => self.bus.scheduler.next_event_cycle,
                };
                if target != u64::MAX && target > self.bus.scheduler.current_cycle {
                    self.bus.scheduler.current_cycle = target;
                    return InternalStep::Continue;
                }
            }
        }

        if halt_end_cycle.is_some()
            && breakpoints.is_empty()
            && !self.bus.watchpoint_active
            && !allow_idle_skip
        {
            let target_cycle = match halt_end_cycle {
                Some(end) => self.bus.scheduler.next_event_cycle.min(end),
                None => self.bus.scheduler.next_event_cycle,
            };

            let mut cur_cycle = self.bus.scheduler.current_cycle;
            while cur_cycle < target_cycle {
                let cycles = if self.jit.enabled && self.jit.available {
                    self.step_jit()
                } else {
                    self.cpu.step(&mut self.bus)
                };
                cur_cycle += cycles.max(1);

                if self.bus.hw.active_dma.is_some()
                    || self.bus.hw.halt_requested
                    || self.bus.hw.stop_requested
                    || (self.bus.hw.ime != 0
                        && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                        && !self.cpu.is_irq_disabled())
                {
                    break;
                }
            }
            self.bus.scheduler.current_cycle = cur_cycle;
            return InternalStep::Continue;
        }

        self.bus.debug_pc = exec_pc;
        if self.bus.watchpoint_active {
            self.bus.watchpoint_hit = None;
        }
        let cycles = if self.jit.enabled
            && self.jit.available
            && breakpoints.is_empty()
            && !self.bus.watchpoint_active
        {
            self.step_jit()
        } else {
            self.cpu.step(&mut self.bus)
        };
        self.bus.scheduler.current_cycle += cycles.max(1);

        if allow_idle_skip {
            let next_pc = self.cpu.current_exec_pc();
            if next_pc <= exec_pc && !self.cpu.halted && !self.cpu.stopped {
                self.pending_idle_check_pc = Some(next_pc);
            } else {
                self.pending_idle_check_pc = None;
            }
        }

        if let Some(hit) = self.bus.watchpoint_hit.take() {
            return InternalStep::DebugStop(StopReason::Watchpoint {
                addr: hit.addr,
                pc: hit.pc,
                write: hit.write,
                value: hit.value,
            });
        }
        InternalStep::Continue
    }

    fn is_safe_idle_loop(&self, loop_start: u32) -> bool {
        let region = (loop_start >> 24) & 0xFF;
        if !matches!(region, 0x02 | 0x03 | 0x08..=0x0D) {
            return false;
        }
        if self.bus.cart.idle_loop_addr == Some(loop_start) {
            return true;
        }
        if self.cpu.is_thumb() {
            self.is_safe_thumb_idle_loop(loop_start)
        } else {
            self.is_safe_arm_idle_loop(loop_start)
        }
    }

    fn is_safe_thumb_idle_loop(&self, loop_start: u32) -> bool {
        let mut saw_poll_load = false;
        let mut base_regs_mask: u16 = 0;
        let mut written_regs_mask: u16 = 0;

        for i in 0..3u32 {
            let addr = loop_start.wrapping_add(i * 2);
            let op = self.bus.debug_read_16(addr);

            if i == 0 && op == 0xE7FE {
                return true;
            }
            if (op & 0xF000) == 0xD000 && (op & 0x0F00) < 0x0E00 {
                let imm8 = (op & 0xFF) as i8 as i32;
                let target = addr.wrapping_add(4).wrapping_add_signed(imm8 << 1);
                return target == loop_start
                    && saw_poll_load
                    && (written_regs_mask & base_regs_mask) == 0;
            }
            if (op & 0xF800) == 0xE000 {
                let imm11 = ((op & 0x07FF) as i32) << 21 >> 21;
                let target = addr.wrapping_add(4).wrapping_add_signed(imm11 << 1);
                return target == loop_start
                    && saw_poll_load
                    && (written_regs_mask & base_regs_mask) == 0;
            }
            if i == 2 {
                return false;
            }

            match op & 0xF800 {
                0x6800 | 0x7800 | 0x8800 => {
                    let scale = match op & 0xF800 {
                        0x6800 => 4,
                        0x8800 => 2,
                        _ => 1,
                    };
                    let rn = (op >> 3) & 7;
                    let rd = op & 7;
                    let imm = u32::from((op >> 6) & 0x1F) * scale;
                    let eff = self.cpu.gpr[rn as usize].wrapping_add(imm);
                    if !is_safe_idle_poll_addr(eff) {
                        return false;
                    }
                    base_regs_mask |= 1 << rn;
                    written_regs_mask |= 1 << rd;
                    saw_poll_load = true;
                }
                0x5800 => {
                    let kind_bits = op & 0xFE00;
                    if !matches!(kind_bits, 0x5800 | 0x5A00 | 0x5C00 | 0x5E00) {
                        return false;
                    }
                    let rn = (op >> 3) & 7;
                    let rm = (op >> 6) & 7;
                    let rd = op & 7;
                    let eff = self.cpu.gpr[rn as usize].wrapping_add(self.cpu.gpr[rm as usize]);
                    if !is_safe_idle_poll_addr(eff) {
                        return false;
                    }
                    base_regs_mask |= (1 << rn) | (1 << rm);
                    written_regs_mask |= 1 << rd;
                    saw_poll_load = true;
                }
                0x2800 => {} // CMP Rn, #imm8 (flags only)
                _ => {
                    if matches!(op & 0xFFC0, 0x4200 | 0x4280 | 0x42C0) {
                        // TST / CMP / CMN (flags only)
                    } else if ((op & 0xE000) == 0x0000 && (op & 0xF800) != 0x1800)
                        || (op & 0xFC00) == 0x4000
                    {
                        written_regs_mask |= 1 << (op & 7);
                    } else {
                        return false;
                    }
                }
            }
        }
        false
    }

    fn is_safe_arm_idle_loop(&self, loop_start: u32) -> bool {
        let mut saw_poll_load = false;
        let mut base_regs_mask: u16 = 0;
        let mut written_regs_mask: u16 = 0;

        for i in 0..3u32 {
            let addr = loop_start.wrapping_add(i * 4);
            let instr = self.bus.debug_read_32(addr);
            if (instr >> 28) == 15 {
                return false;
            }
            if i == 0 && instr == 0xEAFF_FFFE {
                return true;
            }
            if ((instr >> 24) & 0x0F) == 0x0A {
                let imm24 = ((instr as i32) << 8) >> 8;
                let target = addr.wrapping_add(8).wrapping_add_signed(imm24 << 2);
                return target == loop_start
                    && saw_poll_load
                    && (written_regs_mask & base_regs_mask) == 0;
            }
            if i == 2 {
                return false;
            }

            if (instr & 0x0F30_0000) == 0x0510_0000 {
                // LDR / LDRB [Rn, #+/-imm] with P=1, W=0, L=1
                let rn = ((instr >> 16) & 0xF) as u16;
                let rd = ((instr >> 12) & 0xF) as u16;
                if rn == 15 || rd == 15 {
                    return false;
                }
                let off = instr & 0xFFF;
                let base = self.cpu.gpr[rn as usize];
                let eff = if (instr & 0x0080_0000) != 0 {
                    base.wrapping_add(off)
                } else {
                    base.wrapping_sub(off)
                };
                if !is_safe_idle_poll_addr(eff) {
                    return false;
                }
                base_regs_mask |= 1 << rn;
                written_regs_mask |= 1 << rd;
                saw_poll_load = true;
            } else if (instr & 0x0F70_0090) == 0x0150_0090 && (instr & 0x0000_0060) != 0 {
                // LDRH / LDRSB / LDRSH [Rn, #+/-imm8] with P=1, I=1, W=0, L=1
                let rn = ((instr >> 16) & 0xF) as u16;
                let rd = ((instr >> 12) & 0xF) as u16;
                if rn == 15 || rd == 15 {
                    return false;
                }
                let off = ((instr >> 4) & 0xF0) | (instr & 0x0F);
                let base = self.cpu.gpr[rn as usize];
                let eff = if (instr & 0x0080_0000) != 0 {
                    base.wrapping_add(off)
                } else {
                    base.wrapping_sub(off)
                };
                if !is_safe_idle_poll_addr(eff) {
                    return false;
                }
                base_regs_mask |= 1 << rn;
                written_regs_mask |= 1 << rd;
                saw_poll_load = true;
            } else if (instr & 0x0C00_0000) == 0
                && ((instr & 0x0200_0000) != 0 || (instr & 0x0000_0010) == 0)
            {
                let opcode = (instr >> 21) & 0xF;
                let rd = ((instr >> 12) & 0xF) as u16;
                if (0x8..=0xB).contains(&opcode) {
                    if (instr & 0x0010_0000) == 0 {
                        // MRS / MSR
                        return false;
                    }
                } else if rd != 15 {
                    written_regs_mask |= 1 << rd;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }
        false
    }

    pub fn run_frame<F: HostFrontend + ?Sized>(&mut self, frontend: &mut F) {
        let _ = self.run_frame_debug(frontend, &DebugControl::default());
    }

    #[cfg(feature = "std")]
    pub fn run_frame_bench<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
    ) -> (SubsystemTiming, FunctionProfile) {
        let keypad = frontend.poll_keypad() & 0x03FF;
        self.bus.hw.update_keypad(keypad);

        // Reset bench telemetry
        self.bus.bench_active = true;
        self.bus.ppu_bench_time = core::time::Duration::ZERO;
        self.bus.apu_bench_time = core::time::Duration::ZERO;
        self.bus.dma_bench_time = core::time::Duration::ZERO;
        self.bus.fetch_bench_cycles = 0;
        self.bus.read_bench_cycles = 0;
        self.bus.write_bench_cycles = 0;
        self.bus.dma_bench_cycles = 0;

        let mut total_instructions = 0u64;
        let mut branch_flushes = 0u64;
        let mut present_bench_time = core::time::Duration::ZERO;

        let frame_start = std::time::Instant::now();
        let end_cycle = self.bus.scheduler.current_cycle + CYCLES_PER_FRAME;

        while self.bus.scheduler.current_cycle < end_cycle {
            while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
                self.bus.dispatch_event(ev_cycle, kind);
            }

            if self.bus.hw.stop_requested {
                self.cpu.stopped = true;
                self.cpu.halted = false;
                self.bus.hw.stop_requested = false;
            }
            if self.bus.hw.halt_requested {
                if !self.cpu.stopped {
                    self.cpu.halted = true;
                }
                self.bus.hw.halt_requested = false;
            }

            if self.cpu.stopped {
                if (self.bus.hw.ie & self.bus.hw.if_flag & hw::IRQ_STOP_WAKE_MASK) != 0 {
                    self.cpu.stopped = false;
                } else {
                    let delta = end_cycle
                        .saturating_sub(self.bus.scheduler.current_cycle)
                        .max(1);
                    self.bus.apu.flush(self.bus.scheduler.current_cycle);
                    self.bus.scheduler.shift_by(delta);
                    self.bus.hw.shift_timers_by(delta);
                    self.bus.apu.on_stop_shift(self.bus.scheduler.current_cycle);
                    self.bus.ppu.framebuffer.fill(0x7FFF);
                    frontend.present_frame(&self.bus.ppu.framebuffer);
                    break;
                }
            }

            if self.bus.hw.active_dma.is_some() {
                self.pending_idle_check_pc = None;
                let cycles = self.bus.step_active_dma();
                self.bus.scheduler.current_cycle += cycles.max(1);
                continue;
            }

            if self.cpu.halted {
                if (self.bus.hw.ie & self.bus.hw.if_flag) != 0 {
                    self.cpu.halted = false;
                } else {
                    let next = self.bus.scheduler.next_event_cycle.min(end_cycle);
                    if next != u64::MAX && next > self.bus.scheduler.current_cycle {
                        self.bus.scheduler.current_cycle = next;
                    } else {
                        self.bus.scheduler.current_cycle += 4;
                    }
                    continue;
                }
            }

            if self.bus.hw.ime != 0
                && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                && !self.cpu.is_irq_disabled()
                && self.bus.irq_sync_delay == 0
            {
                self.pending_idle_check_pc = None;
                self.cpu.trigger_irq(&mut self.bus);
                self.bus.scheduler.current_cycle += 3;
                continue;
            }

            if self.bus.irq_sync_delay > 0 {
                self.bus.irq_sync_delay = self.bus.irq_sync_delay.saturating_sub(1);
            }

            let target_cycle = self.bus.scheduler.next_event_cycle.min(end_cycle);
            let mut cur_cycle = self.bus.scheduler.current_cycle;

            while cur_cycle < target_cycle {
                let prev_flush = self.cpu.flush_cycles;
                let cycles = self.cpu.step(&mut self.bus);
                total_instructions += 1;
                branch_flushes += self.cpu.flush_cycles.saturating_sub(prev_flush);
                cur_cycle += cycles.max(1);

                if self.bus.hw.active_dma.is_some()
                    || self.bus.hw.halt_requested
                    || self.bus.hw.stop_requested
                    || (self.bus.hw.ime != 0
                        && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                        && !self.cpu.is_irq_disabled())
                {
                    break;
                }
            }
            self.bus.scheduler.current_cycle = cur_cycle;

            if self.bus.ppu.frame_ready {
                let p0 = std::time::Instant::now();
                frontend.present_frame(&self.bus.ppu.framebuffer);
                present_bench_time += p0.elapsed();
                self.bus.ppu.frame_ready = false;
            }
        }

        let raw_elapsed = frame_start.elapsed();
        let total_time = raw_elapsed.saturating_sub(present_bench_time);
        self.bus.bench_active = false;

        let ppu_time = self.bus.ppu_bench_time;
        let apu_time = self.bus.apu_bench_time;
        let dma_time = self.bus.dma_bench_time;

        let other_time = total_time.saturating_sub(ppu_time + apu_time + dma_time);
        let cpu_weight = (total_instructions as f64 * 1.0) + (branch_flushes as f64 * 0.5);
        let bus_weight = (self.bus.fetch_bench_cycles as f64 * 0.20)
            + (self.bus.read_bench_cycles as f64 * 0.35)
            + (self.bus.write_bench_cycles as f64 * 0.35);
        let total_weight = (cpu_weight + bus_weight).max(1.0);
        let cpu_fraction = (cpu_weight / total_weight).clamp(0.05, 0.95);
        let cpu_time = other_time.mul_f64(cpu_fraction);
        let bus_time = other_time.saturating_sub(cpu_time);

        let timing = SubsystemTiming {
            total_time,
            cpu_time,
            ppu_time,
            apu_time,
            dma_time,
            bus_time,
        };

        let profile = FunctionProfile {
            fetch_cycles: self.bus.fetch_bench_cycles,
            decode_exec_cycles: total_instructions,
            mem_read_cycles: self.bus.read_bench_cycles,
            mem_write_cycles: self.bus.write_bench_cycles,
            branch_flush_cycles: branch_flushes,
            ppu_cycles: 160 * 960,
            apu_cycles: 228 * 256,
            dma_cycles: self.bus.dma_bench_cycles,
        };

        (timing, profile)
    }

    /// Runs a single GBA frame with breakpoint and memory watchpoint support.
    pub fn run_frame_debug<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        ctrl: &DebugControl,
    ) -> StopReason {
        let keypad = frontend.poll_keypad() & 0x03FF;
        self.bus.hw.update_keypad(keypad);

        let has_watchpoints = !ctrl.watchpoints.is_empty();
        if has_watchpoints {
            self.bus.set_watchpoints(&ctrl.watchpoints);
        }
        let allow_idle_skip =
            self.idle_skip_enabled && ctrl.breakpoints.is_empty() && !has_watchpoints;

        if ctrl.breakpoints.is_empty() && !has_watchpoints {
            self.run_frame_fast(frontend, allow_idle_skip);
            return StopReason::FrameComplete;
        }

        let end_cycle = self.bus.scheduler.current_cycle + CYCLES_PER_FRAME;
        while self.bus.scheduler.current_cycle < end_cycle {
            let step = self.step_internal(&ctrl.breakpoints, Some(end_cycle), allow_idle_skip);
            if self.bus.ppu.frame_ready {
                frontend.present_frame(&self.bus.ppu.framebuffer);
                self.bus.ppu.frame_ready = false;
            }
            match step {
                InternalStep::Continue => {}
                InternalStep::StoppedFrame(delta) => {
                    frontend.present_frame(&self.bus.ppu.framebuffer);
                    if !self.bus.apu.output_buffer.is_empty() {
                        frontend.push_audio_samples(&self.bus.apu.output_buffer);
                        self.bus.apu.output_buffer.clear();
                    }
                    let silence_samples = ((delta * u64::from(SAMPLE_RATE_HZ))
                        / u64::from(CPU_CLOCK_HZ))
                    .max(1) as usize
                        * 2;
                    let silence = alloc::vec![0i16; silence_samples];
                    frontend.push_audio_samples(&silence);
                    if has_watchpoints {
                        self.bus.clear_watchpoints();
                    }
                    return StopReason::FrameComplete;
                }
                InternalStep::DebugStop(reason) => {
                    self.bus.apu.flush(self.bus.scheduler.current_cycle);
                    if !self.bus.apu.output_buffer.is_empty() {
                        frontend.push_audio_samples(&self.bus.apu.output_buffer);
                        self.bus.apu.output_buffer.clear();
                    }
                    if has_watchpoints {
                        self.bus.clear_watchpoints();
                    }
                    return reason;
                }
            }
        }

        while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
            self.bus.dispatch_event(ev_cycle, kind);
        }

        if self.bus.ppu.frame_ready {
            frontend.present_frame(&self.bus.ppu.framebuffer);
            self.bus.ppu.frame_ready = false;
        }

        self.bus.apu.flush(self.bus.scheduler.current_cycle);
        if !self.bus.apu.output_buffer.is_empty() {
            frontend.push_audio_samples(&self.bus.apu.output_buffer);
            self.bus.apu.output_buffer.clear();
        }
        if has_watchpoints {
            self.bus.clear_watchpoints();
        }
        StopReason::FrameComplete
    }

    fn run_frame_fast<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        allow_idle_skip: bool,
    ) {
        let end_cycle = self.bus.scheduler.current_cycle + CYCLES_PER_FRAME;
        while self.bus.scheduler.current_cycle < end_cycle {
            if self.jit.available
                && let Some(page) = self.bus.code_page_invalidated.take()
            {
                self.jit.invalidate_page(page);
            }

            while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
                self.bus.dispatch_event(ev_cycle, kind);
            }
            if self.bus.ppu.frame_ready {
                frontend.present_frame(&self.bus.ppu.framebuffer);
                self.bus.ppu.frame_ready = false;
            }

            if self.bus.hw.stop_requested {
                self.cpu.stopped = true;
                self.cpu.halted = false;
                self.bus.hw.stop_requested = false;
            }
            if self.bus.hw.halt_requested {
                if !self.cpu.stopped {
                    self.cpu.halted = true;
                }
                self.bus.hw.halt_requested = false;
            }

            if self.cpu.stopped {
                if (self.bus.hw.ie & self.bus.hw.if_flag & hw::IRQ_STOP_WAKE_MASK) != 0 {
                    self.cpu.stopped = false;
                } else {
                    let delta = end_cycle
                        .saturating_sub(self.bus.scheduler.current_cycle)
                        .max(1);
                    self.bus.apu.flush(self.bus.scheduler.current_cycle);
                    self.bus.scheduler.shift_by(delta);
                    self.bus.hw.shift_timers_by(delta);
                    self.bus.apu.on_stop_shift(self.bus.scheduler.current_cycle);
                    self.bus.ppu.framebuffer.fill(0x7FFF);
                    frontend.present_frame(&self.bus.ppu.framebuffer);
                    if !self.bus.apu.output_buffer.is_empty() {
                        frontend.push_audio_samples(&self.bus.apu.output_buffer);
                        self.bus.apu.output_buffer.clear();
                    }
                    let silence_samples = ((delta * u64::from(SAMPLE_RATE_HZ))
                        / u64::from(CPU_CLOCK_HZ))
                    .max(1) as usize
                        * 2;
                    let silence = alloc::vec![0i16; silence_samples];
                    frontend.push_audio_samples(&silence);
                    return;
                }
            }

            if self.bus.hw.active_dma.is_some() {
                self.pending_idle_check_pc = None;
                let cycles = self.bus.step_active_dma();
                self.bus.scheduler.current_cycle += cycles.max(1);
                continue;
            }

            if self.cpu.halted {
                if (self.bus.hw.ie & self.bus.hw.if_flag) != 0 {
                    self.cpu.halted = false;
                } else {
                    let next = self.bus.scheduler.next_event_cycle.min(end_cycle);
                    if next != u64::MAX && next > self.bus.scheduler.current_cycle {
                        self.bus.scheduler.current_cycle = next;
                    } else {
                        self.bus.scheduler.current_cycle += 4;
                    }
                    continue;
                }
            }

            if self.bus.hw.ime != 0
                && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                && !self.cpu.is_irq_disabled()
                && self.bus.irq_sync_delay == 0
            {
                self.pending_idle_check_pc = None;
                self.cpu.trigger_irq(&mut self.bus);
                self.bus.scheduler.current_cycle += 3;
                continue;
            }

            let use_jit = self.jit.enabled && self.jit.available;

            while self.bus.scheduler.current_cycle
                < self.bus.scheduler.next_event_cycle.min(end_cycle)
            {
                if self.bus.irq_sync_delay > 0 {
                    self.bus.irq_sync_delay = self.bus.irq_sync_delay.saturating_sub(1);
                }

                let exec_pc = self.cpu.current_exec_pc();
                if allow_idle_skip
                    && let Some(check_pc) = self.pending_idle_check_pc.take()
                    && check_pc == exec_pc
                    && self.is_safe_idle_loop(exec_pc)
                {
                    self.bus.scheduler.current_cycle =
                        self.bus.scheduler.next_event_cycle.min(end_cycle);
                    break;
                }
                self.bus.debug_pc = exec_pc;

                let cycles = if use_jit {
                    if let Some(page) = self.bus.code_page_invalidated.take() {
                        self.jit.invalidate_page(page);
                    }
                    self.step_jit()
                } else {
                    self.cpu.step(&mut self.bus)
                };
                self.bus.scheduler.current_cycle += cycles.max(1);

                if allow_idle_skip
                    && self.cpu.pipeline_flushed
                    && !self.cpu.halted
                    && !self.cpu.stopped
                {
                    let next_pc = self.cpu.current_exec_pc();
                    if next_pc <= exec_pc {
                        self.pending_idle_check_pc = Some(next_pc);
                    }
                }

                if self.bus.hw.active_dma.is_some()
                    || self.bus.hw.halt_requested
                    || self.bus.hw.stop_requested
                    || self.cpu.halted
                    || self.cpu.stopped
                    || (self.bus.hw.ime != 0
                        && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                        && !self.cpu.is_irq_disabled()
                        && self.bus.irq_sync_delay == 0)
                {
                    break;
                }
            }
        }

        while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
            self.bus.dispatch_event(ev_cycle, kind);
        }

        if self.bus.ppu.frame_ready {
            frontend.present_frame(&self.bus.ppu.framebuffer);
            self.bus.ppu.frame_ready = false;
        }

        self.bus.apu.flush(self.bus.scheduler.current_cycle);
        if !self.bus.apu.output_buffer.is_empty() {
            frontend.push_audio_samples(&self.bus.apu.output_buffer);
            self.bus.apu.output_buffer.clear();
        }
    }

    /// Runs a single GBA frame, stopping early if an instruction PC matches `breakpoints`.
    /// Returns `Some(hit_pc)` when halted at a breakpoint, or `None` after completing the frame.
    pub fn run_frame_with_breakpoints<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        breakpoints: &[u32],
    ) -> Option<u32> {
        let ctrl = DebugControl::with_breakpoints(breakpoints);
        match self.run_frame_debug(frontend, &ctrl) {
            StopReason::Breakpoint(pc) => Some(pc),
            StopReason::FrameComplete | StopReason::Watchpoint { .. } => None,
        }
    }

    /// Serializes the uncompressed state payload (no header) by borrowing the live core.
    ///
    /// `Cartridge::rom` is `#[serde(skip)]`, so the ROM is never copied or encoded; the output is
    /// byte-identical to the payload previously produced from a cloned bus.
    fn serialize_payload(&self) -> Result<Vec<u8>, SaveStateError> {
        let payload = SaveStatePayloadRef {
            cpu: &self.cpu,
            bus: &self.bus,
        };
        postcard::to_allocvec(&payload).map_err(|_| SaveStateError::Corrupt)
    }

    /// Uncompressed, header-less state snapshot intended for in-memory rewind buffers.
    ///
    /// Consecutive raw snapshots differ only in the bytes that actually changed, so XOR deltas
    /// between them compress extremely well (unlike LZ4-compressed [`Self::save_state`] blobs).
    /// Restore with [`Self::load_state_raw`]; not intended for persistence across builds.
    pub fn save_state_raw(&self) -> Result<Vec<u8>, SaveStateError> {
        self.serialize_payload()
    }

    /// Restores a snapshot produced by [`Self::save_state_raw`].
    pub fn load_state_raw(&mut self, raw: &[u8]) -> Result<(), SaveStateError> {
        if raw.is_empty() || raw.len() > MAX_STATE_PAYLOAD_LEN {
            return Err(SaveStateError::Corrupt);
        }
        self.apply_payload(raw)
    }

    /// Executes 1 speculative Run-Ahead frame and rolls back persistent state while keeping
    /// the live APU DSP state intact so audio remains continuous and click-free.
    pub fn speculate_run_ahead_frame<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
    ) -> Result<(), SaveStateError> {
        let snapshot = self.serialize_payload()?;
        let saved_dsp = self.bus.apu.take_dsp_state();
        let saved_scopes = self.bus.apu.chan_scopes;
        let saved_cursor = self.bus.apu.scope_cursor;

        let _ = self.run_frame_debug(frontend, &DebugControl::default());

        self.apply_payload(&snapshot)?;
        self.bus.apu.restore_dsp_state(saved_dsp);
        self.bus.apu.chan_scopes = saved_scopes;
        self.bus.apu.scope_cursor = saved_cursor;
        Ok(())
    }

    pub fn save_state(&self) -> Result<Vec<u8>, SaveStateError> {
        let raw = self.serialize_payload()?;
        let payload_len = u32::try_from(raw.len()).map_err(|_| SaveStateError::Corrupt)?;
        let compressed = lz4_flex::block::compress(&raw);

        let mut out = Vec::with_capacity(STATE_HEADER_LEN + compressed.len());
        out.extend_from_slice(&STATE_MAGIC);
        out.extend_from_slice(&STATE_VERSION.to_le_bytes());
        out.extend_from_slice(&self.rom_crc32().to_le_bytes());
        out.extend_from_slice(&payload_len.to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), SaveStateError> {
        let header = state_info(bytes)?;
        if header.version != STATE_VERSION {
            return Err(SaveStateError::VersionMismatch);
        }
        let expected_crc = self.rom_crc32();
        if header.rom_crc32 != expected_crc {
            return Err(SaveStateError::RomMismatch {
                expected: expected_crc,
                found: header.rom_crc32,
            });
        }

        let payload_len = header.payload_len as usize;
        if payload_len == 0 || payload_len > MAX_STATE_PAYLOAD_LEN {
            return Err(SaveStateError::Corrupt);
        }
        let compressed = &bytes[STATE_HEADER_LEN..];
        if compressed.is_empty() {
            return Err(SaveStateError::Corrupt);
        }

        let uncompressed = lz4_flex::block::decompress(compressed, payload_len)
            .map_err(|_| SaveStateError::Corrupt)?;
        if uncompressed.len() != payload_len {
            return Err(SaveStateError::Corrupt);
        }
        self.apply_payload(&uncompressed)
    }

    fn apply_payload(&mut self, uncompressed: &[u8]) -> Result<(), SaveStateError> {
        let (mut payload, remaining): (SaveStatePayload, &[u8]) =
            postcard::take_from_bytes(uncompressed).map_err(|_| SaveStateError::Corrupt)?;
        if !remaining.is_empty() {
            return Err(SaveStateError::Corrupt);
        }

        let rom = core::mem::take(&mut self.bus.cart.rom);
        payload.bus.cart.rom = rom;
        core::mem::swap(&mut payload.bus.cart.sram, &mut self.bus.cart.sram);
        payload.bus.rebuild_fastmem();
        payload.bus.rebuild_waitstate_table();
        payload.bus.invalidate_code_page();
        payload.bus.code_page_invalidated = None;
        payload.bus.clear_watchpoints();
        self.cpu = payload.cpu;
        self.bus = payload.bus;
        self.pending_idle_check_pc = None;
        Ok(())
    }

    #[inline]
    pub fn state_info(bytes: &[u8]) -> Result<StateHeader, SaveStateError> {
        state_info(bytes)
    }

    #[inline]
    pub fn peek8(&self, addr: u32) -> u8 {
        self.bus.debug_read_8(addr)
    }

    #[inline]
    pub fn peek16(&self, addr: u32) -> u16 {
        self.bus.debug_read_16(addr)
    }

    #[inline]
    pub fn peek32(&self, addr: u32) -> u32 {
        self.bus.debug_read_32(addr)
    }

    #[inline]
    pub fn poke8(&mut self, addr: u32, val: u8) {
        self.bus.debug_write_8(addr, val);
    }

    #[inline]
    pub fn poke16(&mut self, addr: u32, val: u16) {
        self.bus.debug_write_16(addr, val);
    }

    #[inline]
    pub fn poke32(&mut self, addr: u32, val: u32) {
        self.bus.debug_write_32(addr, val);
    }

    pub fn memory_regions(&self) -> [MemoryRegionSlice<'_>; 6] {
        let sram_slice = match self.bus.cart.effective_save_type() {
            SaveType::Flash64K => &self.bus.cart.sram[..0x10000],
            SaveType::Flash128K => &self.bus.cart.sram[..0x20000],
            SaveType::Eeprom512B => &self.bus.cart.sram[..512],
            SaveType::Eeprom8K => &self.bus.cart.sram[..0x2000],
            SaveType::Sram32K | SaveType::None => &self.bus.cart.sram[..0x8000],
        };
        [
            MemoryRegionSlice {
                kind: MemoryRegionKind::Ewram,
                name: "EWRAM",
                base: 0x0200_0000,
                len: self.bus.ewram.len(),
                data: &self.bus.ewram[..],
            },
            MemoryRegionSlice {
                kind: MemoryRegionKind::Iwram,
                name: "IWRAM",
                base: 0x0300_0000,
                len: self.bus.iwram.len(),
                data: &self.bus.iwram[..],
            },
            MemoryRegionSlice {
                kind: MemoryRegionKind::PaletteRam,
                name: "Palette RAM",
                base: 0x0500_0000,
                len: self.bus.ppu.pram.len(),
                data: &self.bus.ppu.pram[..],
            },
            MemoryRegionSlice {
                kind: MemoryRegionKind::Vram,
                name: "VRAM",
                base: 0x0600_0000,
                len: self.bus.ppu.vram.len(),
                data: &self.bus.ppu.vram[..],
            },
            MemoryRegionSlice {
                kind: MemoryRegionKind::Oam,
                name: "OAM",
                base: 0x0700_0000,
                len: self.bus.ppu.oam.len(),
                data: &self.bus.ppu.oam[..],
            },
            MemoryRegionSlice {
                kind: MemoryRegionKind::Sram,
                name: "SRAM",
                base: 0x0E00_0000,
                len: sram_slice.len(),
                data: sram_slice,
            },
        ]
    }

    #[inline]
    pub fn rom_crc32(&self) -> u32 {
        self.bus.cart.rom_crc32
    }

    #[inline]
    pub fn game_code(&self) -> String {
        self.bus.cart.game_code()
    }

    /// Returns whether an external link cable is attached for SIO transfers.
    ///
    /// When `false` (the default), internal-clock Normal and Multiplayer transfers complete
    /// automatically after their cycle duration; when `true`, initiated transfers are staged for
    /// [`Self::take_pending_sio_transfer`] and completed via [`Self::complete_sio_transfer`].
    #[inline]
    pub fn link_connected(&self) -> bool {
        self.bus.hw.link_connected
    }

    /// Enables or disables external link-cable mode for SIO transfers.
    #[inline]
    pub fn set_link_connected(&mut self, connected: bool) {
        self.bus.hw.link_connected = connected;
        if !connected {
            self.bus.hw.pending_sio_transfer = None;
        }
    }

    /// Pops a queued outgoing SIO transfer when [`Self::link_connected`] is `true`.
    #[inline]
    pub fn take_pending_sio_transfer(&mut self) -> Option<SioTransfer> {
        self.bus.hw.pending_sio_transfer.take()
    }

    /// Applies peer response data from a link cable, clearing busy status and firing `IRQ_SERIAL`
    /// if enabled in `SIOCNT`.
    #[inline]
    pub fn complete_sio_transfer(&mut self, response: SioResponse) {
        self.bus.complete_sio_transfer(response);
    }
}

#[inline]
fn is_safe_idle_poll_addr(addr: u32) -> bool {
    match addr >> 24 {
        0x02 | 0x03 => true,
        0x04 => matches!(
            addr & 0xFFFF_FFFE,
            0x0400_0004 | 0x0400_0006 | 0x0400_0128 | 0x0400_0130 | 0x0400_0200 | 0x0400_0202
        ),
        _ => false,
    }
}

pub const STATE_MAGIC: [u8; 4] = *b"RBAS";
pub const STATE_VERSION: u16 = 1;
const STATE_HEADER_LEN: usize = 14;
const MAX_STATE_PAYLOAD_LEN: usize = 8 * 1024 * 1024;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StateHeader {
    pub magic: [u8; 4],
    pub version: u16,
    pub rom_crc32: u32,
    pub payload_len: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SaveStateError {
    BadMagic,
    VersionMismatch,
    RomMismatch { expected: u32, found: u32 },
    Corrupt,
}

impl core::fmt::Display for SaveStateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::BadMagic => write!(f, "invalid save state magic (expected RBAS)"),
            Self::VersionMismatch => write!(f, "unsupported save state format version"),
            Self::RomMismatch { expected, found } => write!(
                f,
                "save state ROM mismatch (expected CRC32 {expected:#010X}, found {found:#010X})"
            ),
            Self::Corrupt => write!(f, "corrupt save state payload"),
        }
    }
}

impl core::error::Error for SaveStateError {}

/// Parses and validates the `RBAS` save state header without decompressing the payload.
pub fn state_info(bytes: &[u8]) -> Result<StateHeader, SaveStateError> {
    if bytes.len() >= 4 && bytes[..4] != STATE_MAGIC {
        return Err(SaveStateError::BadMagic);
    }
    if bytes.len() < STATE_HEADER_LEN {
        return Err(SaveStateError::Corrupt);
    }

    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let rom_crc32 = u32::from_le_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    let payload_len = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);

    Ok(StateHeader {
        magic,
        version,
        rom_crc32,
        payload_len,
    })
}
