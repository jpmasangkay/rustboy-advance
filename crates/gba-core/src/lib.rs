#![no_std]
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::new_without_default
)]

extern crate alloc;

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod hw;
pub mod ppu;
pub mod serde_helpers;

use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::bus::SysBus;
use crate::cpu::Arm7tdmi;

pub const CPU_CLOCK_HZ: u32 = 16_777_216;
pub const CYCLES_PER_SCANLINE: u64 = 1_232;
pub const CYCLES_PER_FRAME: u64 = 280_896;
pub const LCD_WIDTH: usize = 240;
pub const LCD_HEIGHT: usize = 160;
pub const SAMPLE_RATE_HZ: u32 = 32_768;
pub const APU_SAMPLE_INTERVAL: u64 = 512; // 16_777_216 / 32_768

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
}

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
    pub events: [ScheduledEvent; 24],
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            current_cycle: 0,
            next_event_cycle: u64::MAX,
            events: [ScheduledEvent::empty(); 24],
        }
    }

    pub fn schedule(&mut self, target_cycle: u64, kind: EventType) {
        if kind == EventType::None {
            return;
        }
        // Replace existing slot with same EventType if already queued.
        let mut free_idx = None;
        for (i, ev) in self.events.iter().enumerate() {
            if ev.active && ev.kind == kind {
                self.events[i].cycle = target_cycle;
                self.recompute_next();
                return;
            }
            if !ev.active && free_idx.is_none() {
                free_idx = Some(i);
            }
        }
        if let Some(idx) = free_idx {
            self.events[idx] = ScheduledEvent {
                cycle: target_cycle,
                kind,
                active: true,
            };
        } else {
            // Fallback: replace the farthest inactive/latest slot only if out of 24 slots
            log::warn!("Scheduler event table full; dropping {:?}", kind);
        }
        self.recompute_next();
    }

    pub fn cancel(&mut self, kind: EventType) {
        for ev in self.events.iter_mut() {
            if ev.active && ev.kind == kind {
                ev.active = false;
                ev.cycle = u64::MAX;
                ev.kind = EventType::None;
            }
        }
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

    fn recompute_next(&mut self) {
        let mut min_cycle = u64::MAX;
        for ev in self.events.iter() {
            if ev.active && ev.cycle < min_cycle {
                min_cycle = ev.cycle;
            }
        }
        self.next_event_cycle = min_cycle;
    }
}

#[derive(Serialize, Deserialize)]
struct SaveStatePayload {
    cpu: Arm7tdmi,
    bus: SysBus,
}

pub struct GbaCore {
    pub cpu: Arm7tdmi,
    pub bus: SysBus,
}

impl GbaCore {
    pub fn new(bios: Option<&[u8]>, rom: Vec<u8>) -> Self {
        let mut bus = SysBus::new(bios, rom);
        let cpu = Arm7tdmi::new(&mut bus);
        Self { cpu, bus }
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

        *self = Self::new(bios, rom);
        self.bus.cart.save_type = save_type;
        self.bus.cart.set_save_type_override(save_type_override);
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
    }

    /// Mimics GBA SWI 0x00 SoftReset: resets stacks, clears top 0x200 bytes of IWRAM, and restarts entry.
    pub fn soft_reset(&mut self) {
        let boot_ewram = self.bus.iwram[0x7FFA] != 0;
        self.bus.iwram[0x7E00..0x8000].fill(0);
        self.cpu.halted = false;
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
        while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
            self.bus.dispatch_event(ev_cycle, kind);
        }

        if self.bus.hw.halt_requested {
            self.cpu.halted = true;
            self.bus.hw.halt_requested = false;
        }

        if self.bus.hw.active_dma.is_some() {
            let cycles = self.bus.step_active_dma();
            self.bus.scheduler.current_cycle += cycles.max(1);
            return;
        }

        if self.cpu.halted {
            if (self.bus.hw.ie & self.bus.hw.if_flag) != 0 {
                self.cpu.halted = false;
            } else {
                let target = self.bus.scheduler.next_event_cycle;
                if target != u64::MAX && target > self.bus.scheduler.current_cycle {
                    self.bus.scheduler.current_cycle = target;
                } else {
                    self.bus.scheduler.current_cycle += 4;
                }
                return;
            }
        }

        if self.bus.hw.ime != 0
            && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
            && !self.cpu.is_irq_disabled()
            && self.bus.irq_sync_delay == 0
        {
            self.cpu.trigger_irq(&mut self.bus);
            self.bus.scheduler.current_cycle += 3;
            return;
        }

        if self.bus.irq_sync_delay > 0 {
            self.bus.irq_sync_delay = self.bus.irq_sync_delay.saturating_sub(1);
        }

        let cycles = self.cpu.step(&mut self.bus);
        self.bus.scheduler.current_cycle += cycles.max(1);
    }

    pub fn run_frame<F: HostFrontend + ?Sized>(&mut self, frontend: &mut F) {
        let _ = self.run_frame_with_breakpoints(frontend, &[]);
    }

    /// Runs a single GBA frame, stopping early if an instruction PC matches `breakpoints`.
    /// Returns `Some(hit_pc)` when halted at a breakpoint, or `None` after completing the frame.
    pub fn run_frame_with_breakpoints<F: HostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        breakpoints: &[u32],
    ) -> Option<u32> {
        let keypad = frontend.poll_keypad() & 0x03FF;
        self.bus.hw.update_keypad(keypad);

        let end_cycle = self.bus.scheduler.current_cycle + CYCLES_PER_FRAME;
        while self.bus.scheduler.current_cycle < end_cycle {
            while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
                self.bus.dispatch_event(ev_cycle, kind);
            }

            if self.bus.ppu.frame_ready {
                frontend.present_frame(&self.bus.ppu.framebuffer);
                self.bus.ppu.frame_ready = false;
            }

            if self.bus.hw.halt_requested {
                self.cpu.halted = true;
                self.bus.hw.halt_requested = false;
            }

            if self.bus.hw.active_dma.is_some() {
                let cycles = self.bus.step_active_dma();
                self.bus.scheduler.current_cycle += cycles.max(1);
                continue;
            }

            if self.cpu.halted {
                if (self.bus.hw.ie & self.bus.hw.if_flag) != 0 {
                    self.cpu.halted = false;
                } else {
                    let next = self.bus.scheduler.next_event_cycle.min(end_cycle);
                    if next > self.bus.scheduler.current_cycle {
                        self.bus.scheduler.current_cycle = next;
                    } else {
                        self.bus.scheduler.current_cycle += 1;
                    }
                    continue;
                }
            }

            if self.bus.hw.ime != 0
                && (self.bus.hw.ie & self.bus.hw.if_flag) != 0
                && !self.cpu.is_irq_disabled()
                && self.bus.irq_sync_delay == 0
            {
                self.cpu.trigger_irq(&mut self.bus);
                self.bus.scheduler.current_cycle += 3;
                continue;
            }

            if self.bus.irq_sync_delay > 0 {
                self.bus.irq_sync_delay = self.bus.irq_sync_delay.saturating_sub(1);
            }

            if !breakpoints.is_empty() {
                let exec_pc = self.cpu.current_exec_pc();
                if breakpoints.contains(&exec_pc) {
                    if !self.bus.apu.output_buffer.is_empty() {
                        frontend.push_audio_samples(&self.bus.apu.output_buffer);
                        self.bus.apu.output_buffer.clear();
                    }
                    return Some(exec_pc);
                }
            }

            let cycles = self.cpu.step(&mut self.bus);
            self.bus.scheduler.current_cycle += cycles.max(1);
        }

        while let Some((ev_cycle, kind)) = self.bus.scheduler.pop_due() {
            self.bus.dispatch_event(ev_cycle, kind);
        }

        if self.bus.ppu.frame_ready {
            frontend.present_frame(&self.bus.ppu.framebuffer);
            self.bus.ppu.frame_ready = false;
        }

        if !self.bus.apu.output_buffer.is_empty() {
            frontend.push_audio_samples(&self.bus.apu.output_buffer);
            self.bus.apu.output_buffer.clear();
        }
        None
    }

    pub fn save_state(&self) -> Result<Vec<u8>, postcard::Error> {
        let mut bus_clone = self.bus.clone();
        // Avoid duplicating multi-megabyte immutable ROM inside save state files.
        bus_clone.cart.rom.clear();
        let payload = SaveStatePayload {
            cpu: self.cpu.clone(),
            bus: bus_clone,
        };
        postcard::to_allocvec(&payload)
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), postcard::Error> {
        let mut payload: SaveStatePayload = postcard::from_bytes(bytes)?;
        let rom = core::mem::take(&mut self.bus.cart.rom);
        payload.bus.cart.rom = rom;
        payload.bus.rebuild_fastmem();
        self.cpu = payload.cpu;
        self.bus = payload.bus;
        Ok(())
    }
}

