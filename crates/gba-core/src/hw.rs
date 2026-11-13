use serde::{Deserialize, Serialize};

use crate::EventType;
use crate::bus::{AccessType, SysBus};
use crate::cpu::Arm7tdmi;

pub const IRQ_VBLANK: u16 = 1 << 0;
pub const IRQ_HBLANK: u16 = 1 << 1;
pub const IRQ_VCOUNT: u16 = 1 << 2;
pub const IRQ_TIMER0: u16 = 1 << 3;
pub const IRQ_TIMER1: u16 = 1 << 4;
pub const IRQ_TIMER2: u16 = 1 << 5;
pub const IRQ_TIMER3: u16 = 1 << 6;
pub const IRQ_SERIAL: u16 = 1 << 7;
pub const IRQ_DMA0: u16 = 1 << 8;
pub const IRQ_DMA1: u16 = 1 << 9;
pub const IRQ_DMA2: u16 = 1 << 10;
pub const IRQ_DMA3: u16 = 1 << 11;
pub const IRQ_KEYPAD: u16 = 1 << 12;
pub const IRQ_GAMEPAK: u16 = 1 << 13;
pub const IRQ_STOP_WAKE_MASK: u16 = IRQ_KEYPAD | IRQ_GAMEPAK | IRQ_SERIAL;

const TIMER_PRESCALERS: [u64; 4] = [1, 64, 256, 1024];

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct DmaChannel {
    pub sad: u32,
    pub dad: u32,
    pub count: u16,
    pub cnt: u16,

    pub int_sad: u32,
    pub int_dad: u32,
    pub int_count: u32,
    pub active: bool,
    pub first_unit: bool,
    pub latch: u32,
}

impl DmaChannel {
    pub const fn new() -> Self {
        Self {
            sad: 0,
            dad: 0,
            count: 0,
            cnt: 0,
            int_sad: 0,
            int_dad: 0,
            int_count: 0,
            active: false,
            first_unit: true,
            latch: 0,
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        (self.cnt & 0x8000) != 0
    }

    #[inline]
    pub fn timing(&self) -> u16 {
        (self.cnt >> 12) & 0x03
    }

    #[inline]
    pub fn is_32bit(&self) -> bool {
        (self.cnt & 0x0400) != 0
    }

    #[inline]
    pub fn repeat(&self) -> bool {
        (self.cnt & 0x0200) != 0
    }

    #[inline]
    pub fn irq_enabled(&self) -> bool {
        (self.cnt & 0x4000) != 0
    }
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct TimerChannel {
    pub reload: u16,
    pub counter: u16,
    pub cnt: u16,
    pub start_cycle: u64,
    pub running_non_cascade: bool,
}

impl TimerChannel {
    pub const fn new() -> Self {
        Self {
            reload: 0,
            counter: 0,
            cnt: 0,
            start_cycle: 0,
            running_non_cascade: false,
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        (self.cnt & 0x0080) != 0
    }

    #[inline]
    pub fn cascade(&self) -> bool {
        (self.cnt & 0x0004) != 0
    }

    #[inline]
    pub fn irq_enabled(&self) -> bool {
        (self.cnt & 0x0040) != 0
    }

    #[inline]
    pub fn prescaler(&self) -> u64 {
        TIMER_PRESCALERS[(self.cnt & 0x03) as usize]
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SioMode {
    Normal8,
    Normal32,
    Multi,
    Uart,
}

/// Outgoing GBA SIO transfer captured when [`HardwareControl::link_connected`] is `true`.
///
/// Link-cable backends poll pending transfers via [`crate::GbaCore::take_pending_sio_transfer`],
/// perform peer exchange, and complete the transfer via [`crate::GbaCore::complete_sio_transfer`].
/// When `link_connected` is `false`, internal-clock Normal and Multiplayer transfers
/// auto-complete after their hardware cycle duration via [`crate::EventType::SioTransferComplete`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SioTransfer {
    pub mode: SioMode,
    pub siocnt: u16,
    pub data8_or_send: u16,
    pub data32: u32,
    pub duration_cycles: u64,
}

/// Incoming peer data supplied to [`crate::GbaCore::complete_sio_transfer`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SioResponse {
    pub data8: u8,
    pub data32: u32,
    pub multi: [u16; 4],
    pub multi_id: u8,
    pub error: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HardwareControl {
    pub dma: [DmaChannel; 4],
    pub active_dma: Option<u8>,
    pub timers: [TimerChannel; 4],

    pub ie: u16,
    pub if_flag: u16,
    pub ime: u16,
    pub waitcnt: u16,
    pub postflg: u8,
    pub halt_requested: bool,
    #[serde(default)]
    pub stop_requested: bool,

    pub keyinput: u16,
    pub keycnt: u16,
    pub key_irq_latch: bool,

    #[serde(default = "default_rcnt")]
    pub rcnt: u16,
    pub siocnt: u16,
    #[serde(default)]
    pub siodata8: u16,
    pub siomulti: [u16; 4],
    #[serde(default)]
    pub link_connected: bool,
    #[serde(default)]
    pub pending_sio_transfer: Option<SioTransfer>,
    pub memcnt: u32,
}

fn default_rcnt() -> u16 {
    0x8000
}

impl HardwareControl {
    pub fn new() -> Self {
        Self {
            dma: [DmaChannel::new(); 4],
            active_dma: None,
            timers: [TimerChannel::new(); 4],
            ie: 0,
            if_flag: 0,
            ime: 0,
            waitcnt: 0x0000,
            postflg: 0,
            halt_requested: false,
            stop_requested: false,
            keyinput: 0x03FF,
            keycnt: 0,
            key_irq_latch: false,
            rcnt: default_rcnt(),
            siocnt: 0,
            siodata8: 0,
            siomulti: [0xFFFF; 4],
            link_connected: false,
            pending_sio_transfer: None,
            memcnt: 0x0D00_0020,
        }
    }

    #[inline]
    pub fn request_irq(&mut self, mask: u16) {
        self.if_flag |= mask;
    }

    pub fn update_keypad(&mut self, keyinput_active_low: u16) {
        self.keyinput = keyinput_active_low & 0x03FF;
        self.check_keypad_irq();
    }

    pub fn check_keypad_irq(&mut self) {
        if (self.keycnt & 0x4000) == 0 {
            self.key_irq_latch = false;
            return;
        }
        let pressed = (!self.keyinput) & 0x03FF;
        let selected = self.keycnt & 0x03FF;
        let is_and = (self.keycnt & 0x8000) != 0;
        let triggered = if selected == 0 {
            false
        } else if is_and {
            (pressed & selected) == selected
        } else {
            (pressed & selected) != 0
        };
        if triggered {
            if !self.key_irq_latch {
                self.request_irq(IRQ_KEYPAD);
                self.key_irq_latch = true;
            }
        } else {
            self.key_irq_latch = false;
        }
    }

    pub fn recompute_active_dma(&mut self) {
        self.active_dma = (0u8..4).find(|&ch| self.dma[ch as usize].active);
    }

    pub fn shift_timers_by(&mut self, delta: u64) {
        for t in self.timers.iter_mut() {
            if t.running_non_cascade {
                t.start_cycle = t.start_cycle.saturating_add(delta);
            }
        }
    }

    pub fn sio_transfer_duration(siocnt: u16) -> Option<(SioMode, u64)> {
        let mode_bits = (siocnt >> 12) & 0x03;
        match mode_bits {
            0 => {
                let internal_clk = (siocnt & 0x0001) != 0;
                if !internal_clk {
                    return Some((SioMode::Normal8, 0));
                }
                let cycles_per_bit = if (siocnt & 0x0002) != 0 { 8 } else { 64 };
                Some((SioMode::Normal8, 8 * cycles_per_bit))
            }
            1 => {
                let internal_clk = (siocnt & 0x0001) != 0;
                if !internal_clk {
                    return Some((SioMode::Normal32, 0));
                }
                let cycles_per_bit = if (siocnt & 0x0002) != 0 { 8 } else { 64 };
                Some((SioMode::Normal32, 32 * cycles_per_bit))
            }
            2 => {
                const MULTI_BAUD: [u64; 4] = [9_600, 38_400, 57_600, 115_200];
                let baud = MULTI_BAUD[(siocnt & 0x0003) as usize];
                let duration = (16_777_216u64 * 72) / baud;
                Some((SioMode::Multi, duration))
            }
            _ => Some((SioMode::Uart, 64)),
        }
    }

    pub fn current_timer_counter(&self, ch: usize, current_cycle: u64) -> u16 {
        let t = &self.timers[ch];
        if !t.running_non_cascade {
            return t.counter;
        }
        let elapsed = current_cycle.saturating_sub(t.start_cycle);
        let ticks = elapsed / t.prescaler();
        let period = (0x1_0000u64 - (t.reload as u64)).max(1);
        let first_rem = 0x1_0000u64 - (t.counter as u64);
        if ticks < first_rem {
            (t.counter as u64 + ticks) as u16
        } else {
            let after = ticks - first_rem;
            ((t.reload as u64) + (after % period)) as u16
        }
    }
}

impl SysBus {
    pub fn trigger_dma_by_timing(&mut self, timing: u16) {
        let mut changed = false;
        for ch in 0..4 {
            let dma = &mut self.hw.dma[ch];
            if dma.enabled() && dma.timing() == timing && !dma.active {
                dma.int_count = if dma.count == 0 {
                    if ch == 3 { 0x10000 } else { 0x4000 }
                } else if ch < 3 {
                    (dma.count as u32) & 0x3FFF
                } else {
                    dma.count as u32
                };
                if ch < 3 && dma.int_count == 0 {
                    dma.int_count = 0x4000;
                }
                let dest_ctrl = (dma.cnt >> 5) & 0x03;
                if dest_ctrl == 3 {
                    dma.int_dad = dma.dad;
                }
                dma.active = true;
                dma.first_unit = true;
                changed = true;
            }
        }
        if changed {
            self.hw.recompute_active_dma();
        }
    }

    pub fn trigger_sound_dma(&mut self, ch: usize) {
        let dma = &mut self.hw.dma[ch];
        if dma.enabled() && dma.timing() == 3 && !dma.active {
            dma.int_count = 4;
            dma.active = true;
            dma.first_unit = true;
            self.hw.recompute_active_dma();
        }
    }

    pub fn step_active_dma(&mut self) -> u64 {
        #[cfg(feature = "std")]
        let t0 = if self.bench_active {
            Some(std::time::Instant::now())
        } else {
            None
        };

        let Some(ch_u8) = self.hw.active_dma else {
            return 1;
        };
        let ch = ch_u8 as usize;

        // Sound FIFO DMA (ch 1 & 2 timing == 3) always uses 32-bit, fixed dest!
        let is_sound_fifo = (ch == 1 || ch == 2) && self.hw.dma[ch].timing() == 3;
        let is_32bit = is_sound_fifo || self.hw.dma[ch].is_32bit();
        let src_ctrl = (self.hw.dma[ch].cnt >> 7) & 0x03;
        let dst_ctrl = (self.hw.dma[ch].cnt >> 5) & 0x03;

        let sad = self.hw.dma[ch].int_sad;
        let dad = self.hw.dma[ch].int_dad;
        let access_kind = if self.hw.dma[ch].first_unit {
            self.hw.dma[ch].first_unit = false;
            AccessType::NonSequential
        } else {
            AccessType::Sequential
        };

        let unit_bytes: i32 = if is_32bit { 4 } else { 2 };
        let src_step: i32 = match src_ctrl {
            0 | 3 => unit_bytes,
            1 => -unit_bytes,
            _ => 0,
        };
        let dst_step: i32 = if is_sound_fifo {
            0
        } else {
            match dst_ctrl {
                0 | 3 => unit_bytes,
                1 => -unit_bytes,
                _ => 0,
            }
        };

        // Any active DMA invalidates GamePak prefetch
        self.prefetch.active = false;
        self.prefetch.count = 0;

        if access_kind == AccessType::NonSequential {
            self.scheduler.current_cycle += 2;
        }
        let mut cycles = 0u64;

        if self.hw.dma[ch].int_count > 0 {
            if is_32bit {
                let s_addr = sad & !3;
                let d_addr = dad & !3;
                // DMA reading below 0x0200_0000 returns open bus (DMA latch) rather than touching BIOS/unmapped
                let (val, rc) = if (s_addr >> 24) >= 0x02 {
                    let (v, c) = self.read_32_dma(s_addr, access_kind);
                    self.hw.dma[ch].latch = v;
                    self.dma_latch = v;
                    (v, c)
                } else {
                    (self.hw.dma[ch].latch, 1)
                };
                self.last_access_was_dma = true;
                let wc = self.write_32(d_addr, val, access_kind);
                cycles += rc + wc;
            } else {
                let s_addr = sad & !1;
                let d_addr = dad & !1;
                let (val, rc) = if (s_addr >> 24) >= 0x02 {
                    let (v, c) = if self.cart.is_eeprom_addr(s_addr) {
                        (
                            self.cart.eeprom.read_bit(),
                            self.rom_fetch_cycles(s_addr, access_kind),
                        )
                    } else {
                        self.read_16_dma(s_addr, access_kind)
                    };
                    let full = (v as u32) | ((v as u32) << 16);
                    self.hw.dma[ch].latch = full;
                    self.dma_latch = full;
                    (v, c)
                } else {
                    let v = (self.hw.dma[ch].latch >> ((d_addr & 2) * 8)) as u16;
                    (v, 1)
                };
                self.last_access_was_dma = true;
                let wc = if self.cart.is_eeprom_addr(d_addr) {
                    let total_len = if self.hw.dma[ch].count == 0 {
                        0x10000
                    } else {
                        self.hw.dma[ch].count as u32
                    };
                    self.cart.eeprom.write_bit(
                        &mut self.cart.sram,
                        &mut self.cart.save_dirty,
                        val,
                        total_len,
                    );
                    if self.cart.save_type == crate::cartridge::SaveType::None {
                        self.cart.save_type = if self.cart.eeprom.detected_8k {
                            crate::cartridge::SaveType::Eeprom8K
                        } else {
                            crate::cartridge::SaveType::Eeprom512B
                        };
                        self.rebuild_fastmem();
                    }
                    self.rom_fetch_cycles(d_addr, access_kind)
                } else {
                    self.write_16(d_addr, val, access_kind)
                };
                cycles += rc + wc;
            }

            self.hw.dma[ch].int_sad = sad.wrapping_add(src_step as u32);
            self.hw.dma[ch].int_dad = dad.wrapping_add(dst_step as u32);
            self.hw.dma[ch].int_count -= 1;
        }

        if self.hw.dma[ch].int_count == 0 {
            self.hw.dma[ch].active = false;
            self.force_next_fetch_nseq = true;
            self.invalidate_code_page();
            if self.hw.dma[ch].irq_enabled() {
                self.hw.request_irq(IRQ_DMA0 << ch);
            }
            if !self.hw.dma[ch].repeat() || self.hw.dma[ch].timing() == 0 {
                self.hw.dma[ch].cnt &= !0x8000;
            } else if dst_ctrl == 3 && !is_sound_fifo {
                self.hw.dma[ch].int_dad = self.hw.dma[ch].dad;
            }
            self.hw.recompute_active_dma();
        }

        #[cfg(feature = "std")]
        if let Some(t) = t0 {
            self.dma_bench_time += t.elapsed();
            self.dma_bench_cycles += cycles;
        }

        cycles
    }

    pub fn reschedule_timer(&mut self, ch: usize, current_cycle: u64) {
        self.scheduler.cancel(EventType::TimerOverflow(ch as u8));
        let t = &mut self.hw.timers[ch];
        t.running_non_cascade = false;

        let can_cascade = ch > 0 && t.cascade();
        if t.enabled() && !can_cascade {
            t.running_non_cascade = true;
            t.start_cycle = current_cycle;
            let rem_ticks = (0x1_0000u64 - (t.counter as u64)).max(1);
            let target = current_cycle + rem_ticks * t.prescaler();
            self.scheduler
                .schedule(target, EventType::TimerOverflow(ch as u8));
        }
    }

    #[inline]
    fn timer_read_cycle(&self) -> u64 {
        self.scheduler.current_cycle + self.instr_fetch_cycles.saturating_sub(1)
    }

    #[inline]
    fn timer_write_cycle(&self) -> u64 {
        self.scheduler.current_cycle + self.instr_fetch_cycles
    }

    #[inline]
    pub fn flush_timer_overflows_until(&mut self, limit_cycle: u64) {
        while let Some((ev_cycle, ch)) = self.scheduler.pop_due_timer_until(limit_cycle) {
            self.handle_timer_overflow(ch as usize, ev_cycle);
        }
    }

    fn trigger_timer_overflow_effects(&mut self, ch: usize, ev_cycle: u64) {
        if self.hw.timers[ch].irq_enabled() {
            self.hw.request_irq(IRQ_TIMER0 << ch);
        }

        // Audio FIFO sampling on Timer 0 or 1
        if ch <= 1 {
            let (req_a, req_b) = self.apu.on_timer_overflow(ch as u8, ev_cycle);
            if req_a {
                for dma_ch in 1..=2 {
                    if (self.hw.dma[dma_ch].dad & !3) == 0x0400_00A0 {
                        self.trigger_sound_dma(dma_ch);
                    }
                }
            }
            if req_b {
                for dma_ch in 1..=2 {
                    if (self.hw.dma[dma_ch].dad & !3) == 0x0400_00A4 {
                        self.trigger_sound_dma(dma_ch);
                    }
                }
            }
        }

        // Cascade increment into ch + 1
        let mut next_ch = ch + 1;
        while next_ch < 4 {
            let nt = &mut self.hw.timers[next_ch];
            if nt.enabled() && nt.cascade() {
                let (val, overflow) = nt.counter.overflowing_add(1);
                if overflow {
                    nt.counter = nt.reload;
                    if nt.irq_enabled() {
                        self.hw.request_irq(IRQ_TIMER0 << next_ch);
                    }
                    next_ch += 1;
                    continue;
                } else {
                    nt.counter = val;
                }
            }
            break;
        }
    }

    pub fn handle_timer_overflow(&mut self, ch: usize, ev_cycle: u64) {
        // Reload and schedule next overflow if still enabled non-cascade
        if self.hw.timers[ch].enabled() && !(ch > 0 && self.hw.timers[ch].cascade()) {
            self.hw.timers[ch].counter = self.hw.timers[ch].reload;
            self.hw.timers[ch].start_cycle = ev_cycle;
            let period_ticks = (0x1_0000u64 - (self.hw.timers[ch].reload as u64)).max(1);
            let next_target = ev_cycle + period_ticks * self.hw.timers[ch].prescaler();
            self.scheduler
                .schedule(next_target, EventType::TimerOverflow(ch as u8));
        }

        self.trigger_timer_overflow_effects(ch, ev_cycle);
    }

    pub fn read_io_16(&mut self, addr: u32, pc: u32, is_thumb: bool) -> u16 {
        let off = addr & 0x0FFF_FFFE;
        if off >= 0x0400_0400 {
            if (off & 0x0FFF) == 0x0800 {
                self.last_access_was_dma = false;
                return (self.hw.memcnt & 0xFFFF) as u16;
            }
            if (off & 0x0FFF) == 0x0802 {
                self.last_access_was_dma = false;
                return (self.hw.memcnt >> 16) as u16;
            }
            return self.read_open_bus_16(addr, pc, is_thumb);
        }

        let val = match off {
            // PPU Display Control & Status
            0x0400_0000 => self.ppu.dispcnt,
            0x0400_0002 => self.ppu.greenswap,
            0x0400_0004 => self.ppu.dispstat,
            0x0400_0006 => self.ppu.vcount,
            0x0400_0008 => self.ppu.bgcnt[0],
            0x0400_000A => self.ppu.bgcnt[1],
            0x0400_000C => self.ppu.bgcnt[2],
            0x0400_000E => self.ppu.bgcnt[3],
            0x0400_0048 => self.ppu.winin,
            0x0400_004A => self.ppu.winout,
            0x0400_0050 => self.ppu.bldcnt,
            0x0400_0052 => self.ppu.bldalpha,

            // APU PSG & Control
            0x0400_0060 => self.apu.sq1.reg_l & 0x007F,
            0x0400_0062 => self.apu.sq1.reg_h & 0xFFC0,
            0x0400_0064 => self.apu.sq1.reg_x & 0x4000,
            0x0400_0068 => self.apu.sq2.reg_h & 0xFFC0,
            0x0400_006C => self.apu.sq2.reg_x & 0x4000,
            0x0400_0070 => self.apu.wave.reg_l & 0x00E0,
            0x0400_0072 => self.apu.wave.reg_h & 0xE000,
            0x0400_0074 => self.apu.wave.reg_x & 0x4000,
            0x0400_0078 => self.apu.noise.reg_l & 0xFF00,
            0x0400_007C => self.apu.noise.reg_h & 0x40FF,
            0x0400_0080 => self.apu.soundcnt_l,
            0x0400_0082 => self.apu.soundcnt_h,
            0x0400_0084 => {
                self.apu.sync(self.scheduler.current_cycle);
                self.apu.read_soundcnt_x()
            }
            0x0400_0088 => self.apu.soundbias,
            0x0400_0090..=0x0400_009E => {
                let idx = (off - 0x0400_0090) as usize;
                let lo = self.apu.wave.read_wave_ram(idx);
                let hi = self.apu.wave.read_wave_ram(idx + 1);
                u16::from_le_bytes([lo, hi])
            }

            // DMA Control (SAD, DAD, CNT_L are write-only on GBA!)
            0x0400_00BA => self.hw.dma[0].cnt & 0xF7E0,
            0x0400_00C6 => self.hw.dma[1].cnt & 0xF7E0,
            0x0400_00D2 => self.hw.dma[2].cnt & 0xF7E0,
            0x0400_00DE => self.hw.dma[3].cnt & 0xFFE0,

            // Timers
            0x0400_0100 => {
                let read_cycle = self.timer_read_cycle();
                self.flush_timer_overflows_until(read_cycle);
                self.hw.current_timer_counter(0, read_cycle)
            }
            0x0400_0102 => self.hw.timers[0].cnt,
            0x0400_0104 => {
                let read_cycle = self.timer_read_cycle();
                self.flush_timer_overflows_until(read_cycle);
                self.hw.current_timer_counter(1, read_cycle)
            }
            0x0400_0106 => self.hw.timers[1].cnt,
            0x0400_0108 => {
                let read_cycle = self.timer_read_cycle();
                self.flush_timer_overflows_until(read_cycle);
                self.hw.current_timer_counter(2, read_cycle)
            }
            0x0400_010A => self.hw.timers[2].cnt,
            0x0400_010C => {
                let read_cycle = self.timer_read_cycle();
                self.flush_timer_overflows_until(read_cycle);
                self.hw.current_timer_counter(3, read_cycle)
            }
            0x0400_010E => self.hw.timers[3].cnt,

            // Serial I/O
            0x0400_0120 => self.hw.siomulti[0],
            0x0400_0122 => self.hw.siomulti[1],
            0x0400_0124 => self.hw.siomulti[2],
            0x0400_0126 => self.hw.siomulti[3],
            0x0400_0128 => self.hw.siocnt,
            0x0400_012A => self.hw.siodata8,
            0x0400_0134 => self.hw.rcnt & 0xC1FF,

            // Keypad
            0x0400_0130 => self.hw.keyinput,
            0x0400_0132 => self.hw.keycnt,

            // Interrupts & System
            0x0400_0200 => self.hw.ie,
            0x0400_0202 => {
                let read_cycle = self.timer_read_cycle();
                self.flush_timer_overflows_until(read_cycle);
                self.hw.if_flag
            }
            0x0400_0204 => self.hw.waitcnt & 0x7FFF,
            0x0400_0208 => self.hw.ime & 1,
            0x0400_0300 => self.hw.postflg as u16,

            // Read unused / write-only registers -> open bus (some return 0 if paired with readable half on GBA,
            // e.g. DMA_CNT_L paired with CNT_H reads 0).
            0x0400_00B8 | 0x0400_00C4 | 0x0400_00D0 | 0x0400_00DC => 0,
            0x0400_0066 | 0x0400_006A | 0x0400_006E | 0x0400_0076 | 0x0400_007A | 0x0400_007E
            | 0x0400_0086 | 0x0400_008A | 0x0400_0136 | 0x0400_0142 | 0x0400_015A | 0x0400_0206
            | 0x0400_020A | 0x0400_0302 => 0,
            _ => return self.read_open_bus_16(addr, pc, is_thumb),
        };
        self.last_access_was_dma = false;
        val
    }

    pub fn write_io_8(&mut self, addr: u32, val: u8) {
        let off = addr & 0x0FFF_FFFF;
        if off == 0x0400_0300 {
            if self.debug_pc < 0x4000 || self.last_access_was_dma {
                self.hw.postflg = val;
            }
            return;
        }
        if off == 0x0400_0301 {
            if self.debug_pc < 0x4000 || self.last_access_was_dma {
                if (val & 0x80) == 0 {
                    self.hw.halt_requested = true;
                } else {
                    self.hw.stop_requested = true;
                }
            }
            return;
        }
        if (0x0400_0090..=0x0400_009F).contains(&off) {
            let cycle = self.scheduler.current_cycle;
            self.apu.sync(cycle);
            self.apu
                .wave
                .write_wave_ram((off - 0x0400_0090) as usize, val);
            self.apu.commit_mix(cycle);
            return;
        }
        if (0x0400_00A0..=0x0400_00A3).contains(&off) {
            self.apu.fifo_a.push_byte(off, val);
            return;
        }
        if (0x0400_00A4..=0x0400_00A7).contains(&off) {
            self.apu.fifo_b.push_byte(off, val);
            return;
        }

        // Read-modify-write for 16-bit register byte writes
        let aligned = off & !1;
        let is_apu_byte = (0x0400_0060..=0x0400_0088).contains(&aligned);
        if is_apu_byte {
            self.apu.sync(self.scheduler.current_cycle);
        }
        let cur = self.read_io_16_for_rmw(aligned);
        let new_val = if (off & 1) == 0 {
            (cur & 0xFF00) | (val as u16)
        } else {
            (cur & 0x00FF) | ((val as u16) << 8)
        };
        // Avoid accidental trigger bit re-firing if only lower byte of SOUNDxCNT_X / TMxCNT_H written, etc.
        match aligned {
            0x0400_0064 => {
                if (off & 1) == 0 {
                    self.apu.sq1.freq = (self.apu.sq1.freq & 0x0700) | (val as u16);
                    self.apu.sq1.reg_x = (self.apu.sq1.reg_x & 0xFF00) | (val as u16);
                } else {
                    self.apu.sq1.write_freq_ctrl(new_val);
                }
            }
            0x0400_006C => {
                if (off & 1) == 0 {
                    self.apu.sq2.freq = (self.apu.sq2.freq & 0x0700) | (val as u16);
                    self.apu.sq2.reg_x = (self.apu.sq2.reg_x & 0xFF00) | (val as u16);
                } else {
                    self.apu.sq2.write_freq_ctrl(new_val);
                }
            }
            0x0400_0074 => {
                if (off & 1) == 0 {
                    self.apu.wave.freq = (self.apu.wave.freq & 0x0700) | (val as u16);
                    self.apu.wave.reg_x = (self.apu.wave.reg_x & 0xFF00) | (val as u16);
                } else {
                    self.apu.wave.write_x(new_val);
                }
            }
            0x0400_007C => {
                if (off & 1) == 0 {
                    self.apu.noise.divisor_code = val & 0x07;
                    self.apu.noise.width_7 = (val & 0x08) != 0;
                    self.apu.noise.shift_clock = (val >> 4) & 0x0F;
                    self.apu.noise.reg_h = (self.apu.noise.reg_h & 0xFF00) | (val as u16);
                } else {
                    self.apu.noise.write_h(new_val);
                }
            }
            0x0400_0202 => {
                // IF 1-to-clear byte write must NOT clear bits of the untouched byte!
                let clear_mask = if (off & 1) == 0 {
                    val as u16
                } else {
                    (val as u16) << 8
                };
                self.hw.if_flag &= !clear_mask;
            }
            _ => self.write_io_16(aligned, new_val),
        }
        if is_apu_byte {
            self.apu.commit_mix(self.scheduler.current_cycle);
        }
    }

    fn read_io_16_for_rmw(&self, aligned: u32) -> u16 {
        match aligned {
            0x0400_0000 => self.ppu.dispcnt,
            0x0400_0002 => self.ppu.greenswap,
            0x0400_0004 => self.ppu.dispstat,
            0x0400_0008..=0x0400_000E => self.ppu.bgcnt[((aligned - 0x0400_0008) / 2) as usize],
            0x0400_0040 => self.ppu.winh[0],
            0x0400_0042 => self.ppu.winh[1],
            0x0400_0044 => self.ppu.winv[0],
            0x0400_0046 => self.ppu.winv[1],
            0x0400_0048 => self.ppu.winin,
            0x0400_004A => self.ppu.winout,
            0x0400_004C => self.ppu.mosaic,
            0x0400_0050 => self.ppu.bldcnt,
            0x0400_0052 => self.ppu.bldalpha,
            0x0400_0054 => self.ppu.bldy,
            0x0400_0060 => self.apu.sq1.reg_l,
            0x0400_0062 => self.apu.sq1.reg_h,
            0x0400_0064 => self.apu.sq1.reg_x,
            0x0400_0068 => self.apu.sq2.reg_h,
            0x0400_006C => self.apu.sq2.reg_x,
            0x0400_0070 => self.apu.wave.reg_l,
            0x0400_0072 => self.apu.wave.reg_h,
            0x0400_0074 => self.apu.wave.reg_x,
            0x0400_0078 => self.apu.noise.reg_l,
            0x0400_007C => self.apu.noise.reg_h,
            0x0400_0080 => self.apu.soundcnt_l,
            0x0400_0082 => self.apu.soundcnt_h,
            0x0400_0084 => self.apu.read_soundcnt_x(),
            0x0400_0088 => self.apu.soundbias,
            0x0400_00BA => self.hw.dma[0].cnt,
            0x0400_00C6 => self.hw.dma[1].cnt,
            0x0400_00D2 => self.hw.dma[2].cnt,
            0x0400_00DE => self.hw.dma[3].cnt,
            0x0400_0100 => self.hw.timers[0].reload,
            0x0400_0102 => self.hw.timers[0].cnt,
            0x0400_0104 => self.hw.timers[1].reload,
            0x0400_0106 => self.hw.timers[1].cnt,
            0x0400_0108 => self.hw.timers[2].reload,
            0x0400_010A => self.hw.timers[2].cnt,
            0x0400_010C => self.hw.timers[3].reload,
            0x0400_010E => self.hw.timers[3].cnt,
            0x0400_0120 => self.hw.siomulti[0],
            0x0400_0122 => self.hw.siomulti[1],
            0x0400_0124 => self.hw.siomulti[2],
            0x0400_0126 => self.hw.siomulti[3],
            0x0400_0128 => self.hw.siocnt,
            0x0400_012A => self.hw.siodata8,
            0x0400_0132 => self.hw.keycnt,
            0x0400_0134 => self.hw.rcnt,
            0x0400_0200 => self.hw.ie,
            0x0400_0204 => self.hw.waitcnt,
            0x0400_0208 => self.hw.ime,
            _ => 0,
        }
    }

    pub fn write_io_16(&mut self, addr: u32, val: u16) {
        let off = addr & 0x0FFF_FFFE;
        if off >= 0x0400_0400 {
            if (off & 0x0FFF) == 0x0800 {
                self.hw.memcnt = (self.hw.memcnt & 0xFFFF_0000) | (val as u32);
            } else if (off & 0x0FFF) == 0x0802 {
                self.hw.memcnt = (self.hw.memcnt & 0x0000_FFFF) | ((val as u32) << 16);
            }
            return;
        }

        if off <= 0x0400_0054 {
            self.sync_ppu_hdraw();
        }

        let is_apu = (0x0400_0060..=0x0400_009E).contains(&off);
        if is_apu {
            self.apu.sync(self.scheduler.current_cycle);
        }

        match off {
            // PPU
            0x0400_0000 => self.ppu.dispcnt = val & 0xFFF7,
            0x0400_0002 => self.ppu.greenswap = val & 1,
            0x0400_0004 => {
                let prev_match = (self.ppu.dispstat & 0x0004) != 0;
                self.ppu.dispstat = (self.ppu.dispstat & 0x0007) | (val & 0xFF38);
                let match_now = self.ppu.vcount == self.ppu.vcount_match_target();
                if match_now {
                    self.ppu.dispstat |= 0x0004;
                    if !prev_match && (self.ppu.dispstat & 0x0020) != 0 {
                        self.hw.request_irq(IRQ_VCOUNT);
                    }
                } else {
                    self.ppu.dispstat &= !0x0004;
                }
            }
            0x0400_0008..=0x0400_000E => {
                let idx = ((off - 0x0400_0008) / 2) as usize;
                self.ppu.bgcnt[idx] = val;
            }
            0x0400_0010 | 0x0400_0014 | 0x0400_0018 | 0x0400_001C => {
                let idx = ((off - 0x0400_0010) / 4) as usize;
                self.ppu.bghofs[idx] = val & 0x01FF;
            }
            0x0400_0012 | 0x0400_0016 | 0x0400_001A | 0x0400_001E => {
                let idx = ((off - 0x0400_0012) / 4) as usize;
                self.ppu.bgvofs[idx] = val & 0x01FF;
            }
            0x0400_0020 => self.ppu.bgp_a[0] = val as i16,
            0x0400_0022 => self.ppu.bgp_b[0] = val as i16,
            0x0400_0024 => self.ppu.bgp_c[0] = val as i16,
            0x0400_0026 => self.ppu.bgp_d[0] = val as i16,
            0x0400_0028 => {
                let new = ((self.ppu.bgx[0] as u32) & 0xFFFF_0000) | (val as u32);
                self.ppu.set_bgx(0, new);
            }
            0x0400_002A => {
                let new = ((self.ppu.bgx[0] as u32) & 0x0000_FFFF) | ((val as u32) << 16);
                self.ppu.set_bgx(0, new);
            }
            0x0400_002C => {
                let new = ((self.ppu.bgy[0] as u32) & 0xFFFF_0000) | (val as u32);
                self.ppu.set_bgy(0, new);
            }
            0x0400_002E => {
                let new = ((self.ppu.bgy[0] as u32) & 0x0000_FFFF) | ((val as u32) << 16);
                self.ppu.set_bgy(0, new);
            }
            0x0400_0030 => self.ppu.bgp_a[1] = val as i16,
            0x0400_0032 => self.ppu.bgp_b[1] = val as i16,
            0x0400_0034 => self.ppu.bgp_c[1] = val as i16,
            0x0400_0036 => self.ppu.bgp_d[1] = val as i16,
            0x0400_0038 => {
                let new = ((self.ppu.bgx[1] as u32) & 0xFFFF_0000) | (val as u32);
                self.ppu.set_bgx(1, new);
            }
            0x0400_003A => {
                let new = ((self.ppu.bgx[1] as u32) & 0x0000_FFFF) | ((val as u32) << 16);
                self.ppu.set_bgx(1, new);
            }
            0x0400_003C => {
                let new = ((self.ppu.bgy[1] as u32) & 0xFFFF_0000) | (val as u32);
                self.ppu.set_bgy(1, new);
            }
            0x0400_003E => {
                let new = ((self.ppu.bgy[1] as u32) & 0x0000_FFFF) | ((val as u32) << 16);
                self.ppu.set_bgy(1, new);
            }
            0x0400_0040 => self.ppu.winh[0] = val,
            0x0400_0042 => self.ppu.winh[1] = val,
            0x0400_0044 => self.ppu.winv[0] = val,
            0x0400_0046 => self.ppu.winv[1] = val,
            0x0400_0048 => self.ppu.winin = val & 0x3F3F,
            0x0400_004A => self.ppu.winout = val & 0x3F3F,
            0x0400_004C => self.ppu.mosaic = val,
            0x0400_0050 => self.ppu.bldcnt = val & 0x3FFF,
            0x0400_0052 => self.ppu.bldalpha = val & 0x1F1F,
            0x0400_0054 => self.ppu.bldy = val & 0x001F,

            // APU PSG (only active when SOUNDCNT_X bit 7 set, except SOUNDCNT_X/H/BIAS/WaveRAM)
            0x0400_0060 if self.apu.master_enabled() => self.apu.sq1.write_sweep(val),
            0x0400_0062 if self.apu.master_enabled() => self.apu.sq1.write_duty_env(val),
            0x0400_0064 if self.apu.master_enabled() => self.apu.sq1.write_freq_ctrl(val),
            0x0400_0068 if self.apu.master_enabled() => self.apu.sq2.write_duty_env(val),
            0x0400_006C if self.apu.master_enabled() => self.apu.sq2.write_freq_ctrl(val),
            0x0400_0070 if self.apu.master_enabled() => self.apu.wave.write_l(val),
            0x0400_0072 if self.apu.master_enabled() => self.apu.wave.write_h(val),
            0x0400_0074 if self.apu.master_enabled() => self.apu.wave.write_x(val),
            0x0400_0078 if self.apu.master_enabled() => self.apu.noise.write_l(val),
            0x0400_007C if self.apu.master_enabled() => self.apu.noise.write_h(val),
            0x0400_0080 if self.apu.master_enabled() => self.apu.soundcnt_l = val,
            0x0400_0082 => self.apu.write_soundcnt_h(val),
            0x0400_0084 => self.apu.write_soundcnt_x(val),
            0x0400_0088 => self.apu.soundbias = val,
            0x0400_0090..=0x0400_009E => {
                let idx = (off - 0x0400_0090) as usize;
                self.apu.wave.write_wave_ram(idx, val as u8);
                self.apu.wave.write_wave_ram(idx + 1, (val >> 8) as u8);
            }
            0x0400_00A0 | 0x0400_00A2 => self.apu.fifo_a.push_half(off, val),
            0x0400_00A4 | 0x0400_00A6 => self.apu.fifo_b.push_half(off, val),

            // DMA 0..3 (stride 12 bytes = 0x0C)
            0x0400_00B0..=0x0400_00DF => {
                let rel = off - 0x0400_00B0;
                let ch = (rel / 12) as usize;
                let reg = rel % 12;
                let sad_mask = if ch == 0 { 0x07FF_FFFF } else { 0x0FFF_FFFF };
                let dad_mask = if ch == 3 { 0x0FFF_FFFF } else { 0x07FF_FFFF };
                match reg {
                    0 => {
                        self.hw.dma[ch].sad =
                            ((self.hw.dma[ch].sad & 0xFFFF_0000) | (val as u32)) & sad_mask;
                    }
                    2 => {
                        self.hw.dma[ch].sad =
                            ((self.hw.dma[ch].sad & 0x0000_FFFF) | ((val as u32) << 16)) & sad_mask;
                    }
                    4 => {
                        self.hw.dma[ch].dad =
                            ((self.hw.dma[ch].dad & 0xFFFF_0000) | (val as u32)) & dad_mask;
                    }
                    6 => {
                        self.hw.dma[ch].dad =
                            ((self.hw.dma[ch].dad & 0x0000_FFFF) | ((val as u32) << 16)) & dad_mask;
                    }
                    8 => {
                        self.hw.dma[ch].count = val;
                    }
                    10 => {
                        let mask = if ch == 3 { 0xFFE0 } else { 0xF7E0 };
                        let was_en = self.hw.dma[ch].enabled();
                        self.hw.dma[ch].cnt = val & mask;
                        let now_en = self.hw.dma[ch].enabled();

                        if !was_en && now_en {
                            self.hw.dma[ch].int_sad = self.hw.dma[ch].sad;
                            self.hw.dma[ch].int_dad = self.hw.dma[ch].dad;
                            self.hw.dma[ch].int_count = if self.hw.dma[ch].count == 0 {
                                if ch == 3 { 0x10000 } else { 0x4000 }
                            } else if ch < 3 {
                                let c = (self.hw.dma[ch].count as u32) & 0x3FFF;
                                if c == 0 { 0x4000 } else { c }
                            } else {
                                self.hw.dma[ch].count as u32
                            };

                            if self.hw.dma[ch].timing() == 0 {
                                // Immediate DMA starts after 2 cycles startup/bus-arbitration latency
                                self.scheduler.schedule(
                                    self.scheduler.current_cycle + 2,
                                    EventType::DmaActivate(ch as u8),
                                );
                            } else if (ch == 1 || ch == 2) && self.hw.dma[ch].timing() == 3 {
                                // Sound FIFO DMA refill if target FIFO needs initial data
                                let dad = self.hw.dma[ch].dad & !3;
                                let needs_refill = if dad == 0x0400_00A0 {
                                    self.apu.fifo_a.count <= 4
                                } else if dad == 0x0400_00A4 {
                                    self.apu.fifo_b.count <= 4
                                } else {
                                    false
                                };
                                if needs_refill {
                                    self.trigger_sound_dma(ch);
                                }
                            }
                        } else if !now_en {
                            self.hw.dma[ch].active = false;
                            self.scheduler.cancel(EventType::DmaActivate(ch as u8));
                            self.hw.recompute_active_dma();
                        }
                    }
                    _ => {}
                }
            }

            // Timers 0..3
            0x0400_0100..=0x0400_010E => {
                let rel = off - 0x0400_0100;
                let ch = (rel / 4) as usize;
                let is_cnt = (rel & 2) != 0;
                let write_cycle = self.timer_write_cycle();
                self.flush_timer_overflows_until(write_cycle);
                if !is_cnt {
                    self.hw.timers[ch].reload = val;
                } else {
                    let was_en = self.hw.timers[ch].enabled();
                    let cur = self.hw.current_timer_counter(ch, write_cycle);
                    let mask = if ch == 0 { 0x00C3 } else { 0x00C7 };
                    self.hw.timers[ch].cnt = val & mask;
                    let now_en = self.hw.timers[ch].enabled();

                    if !was_en && now_en {
                        // Tick-before-reload: on 0->1 enable, a stopped timer with counter=0xFFFF
                        // ticks once on startup before the reload value overwrites the counter.
                        if cur == 0xFFFF
                            && !(ch > 0 && self.hw.timers[ch].cascade())
                            && self.hw.timers[ch].prescaler() == 1
                        {
                            self.trigger_timer_overflow_effects(ch, write_cycle);
                        }
                        self.hw.timers[ch].counter = self.hw.timers[ch].reload;
                        // 2-cycle startup latency from write phase (write_cycle + 1)
                        self.reschedule_timer(ch, write_cycle + 1);
                    } else {
                        self.hw.timers[ch].counter = cur;
                        self.reschedule_timer(ch, write_cycle);
                    }
                }
            }

            0x0400_0120..=0x0400_0126 => {
                let idx = ((off - 0x0400_0120) / 2) as usize;
                self.hw.siomulti[idx] = val;
            }
            0x0400_0128 => {
                let was_busy = (self.hw.siocnt & 0x0080) != 0;
                let is_multi = (val & 0x3000) == 0x2000;
                let cleaned = if is_multi {
                    (val & !0x007C) | (self.hw.siocnt & 0x0070) | 0x0008
                } else {
                    val
                };
                self.hw.siocnt = cleaned;

                let start_requested =
                    !was_busy && (cleaned & 0x0080) != 0 && (self.hw.rcnt & 0x8000) == 0;
                if start_requested
                    && let Some((mode, duration)) = HardwareControl::sio_transfer_duration(cleaned)
                {
                    if self.hw.link_connected {
                        self.hw.pending_sio_transfer = Some(SioTransfer {
                            mode,
                            siocnt: cleaned,
                            data8_or_send: self.hw.siodata8,
                            data32: u32::from(self.hw.siomulti[0])
                                | (u32::from(self.hw.siomulti[1]) << 16),
                            duration_cycles: duration,
                        });
                    } else if duration > 0 {
                        self.scheduler.schedule(
                            self.scheduler.current_cycle + duration,
                            EventType::SioTransferComplete,
                        );
                    }
                }
            }
            0x0400_012A => {
                self.hw.siodata8 = val;
            }
            0x0400_0134 => self.hw.rcnt = val & 0xC1FF,

            0x0400_0132 => {
                self.hw.keycnt = val & 0xC3FF;
                self.hw.check_keypad_irq();
            }

            // Interrupts & System Control
            0x0400_0200 => self.hw.ie = val & 0x3FFF,
            0x0400_0202 => self.hw.if_flag &= !val,
            0x0400_0204 => {
                self.hw.waitcnt = val & 0x5FFF;
                self.update_waitcnt(self.hw.waitcnt);
            }
            0x0400_0208 => {
                let prev_ime = self.hw.ime;
                self.hw.ime = val & 1;
                if prev_ime == 0 && self.hw.ime == 1 {
                    self.irq_sync_delay = 1;
                }
            }
            0x0400_0300 if self.debug_pc < 0x4000 || self.last_access_was_dma => {
                self.hw.postflg = val as u8;
                let halt_byte = (val >> 8) as u8;
                if (halt_byte & 0x80) == 0 {
                    self.hw.halt_requested = true;
                } else {
                    self.hw.stop_requested = true;
                }
            }
            _ => {}
        }
        if is_apu {
            self.apu.commit_mix(self.scheduler.current_cycle);
        }
    }

    pub fn hle_swi(&mut self, cpu: &mut Arm7tdmi, comment: u8) {
        self.debug_pc = 0x0100;
        // Magic internal HLE wait sentinel used by IntrWait / VBlankIntrWait loop at 0x0000_0140
        if comment == 0xFE {
            if let Some((wait_flags, saved_ret, saved_spsr)) = cpu.intr_wait {
                let check = self.read_32(0x0300_7FF8, AccessType::NonSequential, 0);
                let matched = (check as u16) & wait_flags;
                if matched != 0 {
                    self.write_16(
                        0x0300_7FF8,
                        (check as u16) & !matched,
                        AccessType::NonSequential,
                    );
                    cpu.intr_wait = None;
                    if !self.bios_present {
                        self.bios_latch = 0xE3A0_2004;
                    }
                    cpu.set_cpsr(saved_spsr);
                    cpu.flush_pipeline(self, saved_ret);
                    return;
                }
            }
            // Still waiting: keep IRQ enabled in CPSR and halt
            cpu.cpsr &= !(1 << 7);
            cpu.flush_pipeline(self, 0x0000_0140);
            cpu.halted = true;
            return;
        }

        match comment {
            0x00 => {
                // SoftReset
                let flag = self.iwram[0x7FFA];
                self.iwram[0x7E00..0x8000].fill(0);
                cpu.gpr.fill(0);
                cpu.banked_r13 = [0x0300_7F00, 0, 0x0300_7FA0, 0x0300_7FE0, 0, 0];
                cpu.banked_r14 = [0; 6];
                cpu.spsr_banked = [0; 6];
                cpu.set_cpsr(0x0000_001F); // System mode, ARM
                cpu.gpr[13] = 0x0300_7F00;
                cpu.intr_wait = None;
                let entry = if flag != 0 { 0x0200_0000 } else { 0x0800_0000 };
                cpu.flush_pipeline(self, entry);
            }
            0x01 => {
                // RegisterRamReset
                let flags = cpu.gpr[0];
                if (flags & 0x01) != 0 {
                    self.ewram.fill(0);
                }
                if (flags & 0x02) != 0 {
                    // Clears all IWRAM except last 0x200 bytes (SP area)
                    self.iwram[..0x7E00].fill(0);
                }
                if (flags & 0x04) != 0 {
                    self.ppu.pram.fill(0);
                }
                if (flags & 0x08) != 0 {
                    self.ppu.vram.fill(0);
                }
                if (flags & 0x10) != 0 {
                    self.ppu.oam.fill(0);
                }
                if (flags & 0x20) != 0 {
                    self.hw.siocnt = 0;
                    self.hw.siodata8 = 0;
                    self.hw.siomulti = [0xFFFF; 4];
                    self.hw.pending_sio_transfer = None;
                    self.scheduler.cancel(EventType::SioTransferComplete);
                    self.hw.rcnt = 0x8000;
                }
                if (flags & 0x40) != 0 {
                    let cycle = self.scheduler.current_cycle;
                    self.apu.sync(cycle);
                    self.apu.write_soundcnt_x(0);
                    self.apu.commit_mix(cycle);
                }
                if (flags & 0x80) != 0 {
                    self.ppu.dispcnt = 0;
                    self.ppu.dispstat = 0;
                    self.ppu.bldcnt = 0;
                }
                cpu.return_from_swi(self);
            }
            0x02 => {
                // Halt
                cpu.halted = true;
                cpu.return_from_swi(self);
            }
            0x03 => {
                // Stop
                cpu.stopped = true;
                cpu.halted = false;
                cpu.return_from_swi(self);
            }
            0x04 | 0x05 => {
                // IntrWait (r0 = discard_old, r1 = wait_flags) / VBlankIntrWait (wait_flags = 1)
                let (discard, flags) = if comment == 0x05 {
                    (true, 0x0001u16)
                } else {
                    (cpu.gpr[0] != 0, cpu.gpr[1] as u16)
                };

                self.hw.ime = 1;
                let cur_check = self.read_32(0x0300_7FF8, AccessType::NonSequential, 0) as u16;
                if discard {
                    let cleared = cur_check & !flags;
                    self.write_16(0x0300_7FF8, cleared, AccessType::NonSequential);
                } else if (cur_check & flags) != 0 {
                    let cleared = cur_check & !(cur_check & flags);
                    self.write_16(0x0300_7FF8, cleared, AccessType::NonSequential);
                    cpu.return_from_swi(self);
                    return;
                }

                // Enter SVC sleep loop at 0x0000_0140 with IRQ unmasked so user IRQ handler can run!
                let saved_ret = cpu.gpr[14];
                let saved_spsr = cpu.get_spsr();
                cpu.intr_wait = Some((flags, saved_ret, saved_spsr));
                cpu.cpsr &= !((1 << 7) | (1 << 5)); // Enable IRQ, force ARM
                cpu.flush_pipeline(self, 0x0000_0140);
                cpu.halted = true;
            }
            0x06 | 0x07 => {
                // Div (r0 / r1) / DivArm (r1 / r0) -> r0 = quot, r1 = rem, r3 = |quot|
                let (num, den) = if comment == 0x06 {
                    (cpu.gpr[0] as i32, cpu.gpr[1] as i32)
                } else {
                    (cpu.gpr[1] as i32, cpu.gpr[0] as i32)
                };
                if den == 0 {
                    cpu.gpr[0] = if num < 0 { u32::MAX } else { 1 };
                    cpu.gpr[1] = num as u32;
                    cpu.gpr[3] = 1;
                } else if num == i32::MIN && den == -1 {
                    cpu.gpr[0] = i32::MIN as u32;
                    cpu.gpr[1] = 0;
                    cpu.gpr[3] = i32::MIN as u32;
                } else {
                    let q = num / den;
                    let r = num % den;
                    cpu.gpr[0] = q as u32;
                    cpu.gpr[1] = r as u32;
                    cpu.gpr[3] = q.wrapping_abs() as u32;
                }
                cpu.return_from_swi(self);
            }
            0x08 => {
                // Sqrt(u32)
                let x = cpu.gpr[0];
                let mut res = 0u32;
                let mut bit = 1u32 << 30;
                let mut n = x;
                while bit > n {
                    bit >>= 2;
                }
                while bit != 0 {
                    if n >= res + bit {
                        n -= res + bit;
                        res = (res >> 1) + bit;
                    } else {
                        res >>= 1;
                    }
                    bit >>= 2;
                }
                cpu.gpr[0] = res;
                cpu.return_from_swi(self);
            }
            0x09 => {
                // ArcTan(i16 tan_q14) -> i16 in (-0x4000..0x4000)
                let i = (cpu.gpr[0] as i16) as i32;
                cpu.gpr[0] = (hle_arctan(i) as i16 as i32) as u32;
                cpu.return_from_swi(self);
            }
            0x0A => {
                // ArcTan2(i16 x, i16 y) -> u16 in 0..0xFFFF
                let x = (cpu.gpr[0] as i16) as i32;
                let y = (cpu.gpr[1] as i16) as i32;
                cpu.gpr[0] = hle_arctan2(x, y) as u32;
                cpu.return_from_swi(self);
            }
            0x0B => {
                // CpuSet(src, dst, cnt)
                let mut src = cpu.gpr[0];
                let mut dst = cpu.gpr[1];
                let cnt = cpu.gpr[2];
                let count = cnt & 0x001F_FFFF;
                let fixed_src = (cnt & (1 << 24)) != 0;
                let is_32 = (cnt & (1 << 26)) != 0;

                // Model BIOS execution cycles: SWI dispatch, register stacking, and loop overhead
                let caller_pc = cpu.gpr[14];
                let bios_base_cycles = if caller_pc >= 0x0800_0000 {
                    116u64
                } else {
                    104u64
                };
                self.scheduler.current_cycle += bios_base_cycles;
                if is_32 {
                    src &= !3;
                    dst &= !3;
                    for _ in 0..count {
                        let v = self.read_32(src, AccessType::Sequential, 0);
                        self.write_32(dst, v, AccessType::Sequential);
                        if !fixed_src {
                            src = src.wrapping_add(4);
                        }
                        dst = dst.wrapping_add(4);
                    }
                } else {
                    src &= !1;
                    dst &= !1;
                    for _ in 0..count {
                        let v = self.read_16(src, AccessType::Sequential, 0, false);
                        self.write_16(dst, v, AccessType::Sequential);
                        if !fixed_src {
                            src = src.wrapping_add(2);
                        }
                        dst = dst.wrapping_add(2);
                    }
                }
                cpu.return_from_swi(self);
            }
            0x0C => {
                // CpuFastSet(src, dst, cnt)
                let mut src = cpu.gpr[0] & !3;
                let mut dst = cpu.gpr[1] & !3;
                let cnt = cpu.gpr[2];
                let words = (cnt & 0x001F_FFFC).max(if (cnt & 0x001F_FFFF) == 0 { 0 } else { 8 });
                let count = ((cnt & 0x001F_FFFF) + 7) & !7;
                let _ = words;
                let fixed_src = (cnt & (1 << 24)) != 0;
                for _ in 0..count {
                    let v = self.read_32(src, AccessType::Sequential, 0);
                    self.write_32(dst, v, AccessType::Sequential);
                    if !fixed_src {
                        src = src.wrapping_add(4);
                    }
                    dst = dst.wrapping_add(4);
                }
                cpu.return_from_swi(self);
            }
            0x0D => {
                // GetBiosChecksum
                cpu.gpr[0] = 0xBAAE_187F;
                cpu.return_from_swi(self);
            }
            0x0E => {
                // BgAffineSet(src, dst, count)
                let mut src = cpu.gpr[0];
                let mut dst = cpu.gpr[1];
                let count = cpu.gpr[2];
                for _ in 0..count {
                    let orig_x = self.read_32(src, AccessType::Sequential, 0) as i32;
                    let orig_y = self.read_32(src + 4, AccessType::Sequential, 0) as i32;
                    let disp_x =
                        self.read_16(src + 8, AccessType::Sequential, 0, false) as i16 as i32;
                    let disp_y =
                        self.read_16(src + 10, AccessType::Sequential, 0, false) as i16 as i32;
                    let sx = self.read_16(src + 12, AccessType::Sequential, 0, false) as i16 as i32;
                    let sy = self.read_16(src + 14, AccessType::Sequential, 0, false) as i16 as i32;
                    let alpha = self.read_16(src + 16, AccessType::Sequential, 0, false);
                    src += 20;

                    let (sin_v, cos_v) = hle_sin_cos(alpha);
                    let pa = ((cos_v * sx) >> 14) as i16;
                    let pb = ((-sin_v * sx) >> 14) as i16;
                    let pc = ((sin_v * sy) >> 14) as i16;
                    let pd = ((cos_v * sy) >> 14) as i16;
                    let rx = orig_x - ((pa as i32) * disp_x + (pb as i32) * disp_y);
                    let ry = orig_y - ((pc as i32) * disp_x + (pd as i32) * disp_y);

                    self.write_16(dst, pa as u16, AccessType::Sequential);
                    self.write_16(dst + 2, pb as u16, AccessType::Sequential);
                    self.write_16(dst + 4, pc as u16, AccessType::Sequential);
                    self.write_16(dst + 6, pd as u16, AccessType::Sequential);
                    self.write_32(dst + 8, rx as u32, AccessType::Sequential);
                    self.write_32(dst + 12, ry as u32, AccessType::Sequential);
                    dst += 16;
                }
                cpu.return_from_swi(self);
            }
            0x0F => {
                // ObjAffineSet(src, dst, count, stride)
                let mut src = cpu.gpr[0];
                let mut dst = cpu.gpr[1];
                let count = cpu.gpr[2];
                let stride = cpu.gpr[3];
                for _ in 0..count {
                    let sx = self.read_16(src, AccessType::Sequential, 0, false) as i16 as i32;
                    let sy = self.read_16(src + 2, AccessType::Sequential, 0, false) as i16 as i32;
                    let alpha = self.read_16(src + 4, AccessType::Sequential, 0, false);
                    src += 8;

                    let (sin_v, cos_v) = hle_sin_cos(alpha);
                    let pa = ((cos_v * sx) >> 14) as i16;
                    let pb = ((-sin_v * sx) >> 14) as i16;
                    let pc = ((sin_v * sy) >> 14) as i16;
                    let pd = ((cos_v * sy) >> 14) as i16;

                    self.write_16(dst, pa as u16, AccessType::Sequential);
                    dst += stride;
                    self.write_16(dst, pb as u16, AccessType::Sequential);
                    dst += stride;
                    self.write_16(dst, pc as u16, AccessType::Sequential);
                    dst += stride;
                    self.write_16(dst, pd as u16, AccessType::Sequential);
                    dst += stride;
                }
                cpu.return_from_swi(self);
            }
            0x10 => {
                // BitUnPack(src, dst, info_ptr)
                let mut src = cpu.gpr[0];
                let mut dst = cpu.gpr[1];
                let info = cpu.gpr[2];
                let src_len = self.read_16(info, AccessType::Sequential, 0, false) as usize;
                let src_width = self.read_8(info + 2, AccessType::Sequential, 0, false) as u32;
                let dst_width = self.read_8(info + 3, AccessType::Sequential, 0, false) as u32;
                let offset_cfg = self.read_32(info + 4, AccessType::Sequential, 0);
                let data_offset = offset_cfg & 0x7FFF_FFFF;
                let zero_flag = (offset_cfg & 0x8000_0000) != 0;

                if src_width > 0 && dst_width > 0 {
                    let src_mask = (1u32 << src_width) - 1;
                    let mut out_word = 0u32;
                    let mut out_bits = 0u32;
                    for _ in 0..src_len {
                        let byte = self.read_8(src, AccessType::Sequential, 0, false) as u32;
                        src += 1;
                        let mut bit = 0u32;
                        while bit < 8 {
                            let val = (byte >> bit) & src_mask;
                            let unpacked = if val == 0 && !zero_flag {
                                0
                            } else {
                                val.wrapping_add(data_offset)
                            };
                            out_word |= unpacked << out_bits;
                            out_bits += dst_width;
                            if out_bits >= 32 {
                                self.write_32(dst, out_word, AccessType::Sequential);
                                dst += 4;
                                out_word = 0;
                                out_bits = 0;
                            }
                            bit += src_width;
                        }
                    }
                }
                cpu.return_from_swi(self);
            }
            0x11 | 0x12 => {
                // LZ77UnCompWram (8-bit) / LZ77UnCompVram (16-bit)
                let is_vram = comment == 0x12;
                self.hle_lz77_uncomp(cpu.gpr[0], cpu.gpr[1], is_vram);
                cpu.return_from_swi(self);
            }
            0x13 => {
                // HuffUnComp
                self.hle_huff_uncomp(cpu.gpr[0], cpu.gpr[1]);
                cpu.return_from_swi(self);
            }
            0x14 | 0x15 => {
                // RLUnCompWram / RLUnCompVram
                let is_vram = comment == 0x15;
                self.hle_rl_uncomp(cpu.gpr[0], cpu.gpr[1], is_vram);
                cpu.return_from_swi(self);
            }
            0x16..=0x18 => {
                // Diff8bitUnFilterWram / Diff8bitUnFilterVram / Diff16bitUnFilter
                self.hle_diff_unfilter(cpu.gpr[0], cpu.gpr[1], comment);
                cpu.return_from_swi(self);
            }
            0x19 => {
                // SoundBias: level in r0 (bits 1-9)
                let bias_level = (cpu.gpr[0] & 0x03FE) as u16;
                let cur = self.apu.soundbias & !0x03FE;
                self.write_io_16(0x0400_0088, cur | bias_level);
                cpu.return_from_swi(self);
            }
            0x1F => {
                // MidiKey2Freq(waveData, key, finePitch)
                let wave_ptr = cpu.gpr[0];
                let wave_freq =
                    self.read_32(wave_ptr.wrapping_add(4), AccessType::NonSequential, 0);
                let key = (cpu.gpr[1] & 0xFF) as u8;
                let pitch = (cpu.gpr[2] & 0xFF) as u8;
                cpu.gpr[0] = hle_midi_key_to_freq(wave_freq, key, pitch);
                cpu.return_from_swi(self);
            }
            0x1A..=0x1E | 0x20..=0x29 => {
                // SoundDriverInit, SoundDriverMode, SoundDriverMain, SoundDriverVSync,
                // SoundChannelClear, SoundDriverVSyncOff, SoundDriverVSyncOn, etc.
                cpu.return_from_swi(self);
            }
            0x2A => {
                // CustomHalt
                cpu.halted = true;
                cpu.return_from_swi(self);
            }
            _ => {
                log::debug!("Unhandled HLE SWI {:#04x}", comment);
                cpu.return_from_swi(self);
            }
        }
    }

    fn hle_lz77_uncomp(&mut self, mut src: u32, mut dst: u32, is_16bit: bool) {
        let header = self.read_32(src, AccessType::Sequential, 0);
        src += 4;
        let decomp_len = (header >> 8) as usize;
        let mut written = 0usize;
        let mut half_buf = 0u16;

        while written < decomp_len {
            let flags = self.read_8(src, AccessType::Sequential, 0, false);
            src += 1;
            for bit in (0..8).rev() {
                if written >= decomp_len {
                    break;
                }
                if (flags & (1 << bit)) == 0 {
                    let b = self.read_8(src, AccessType::Sequential, 0, false);
                    src += 1;
                    if is_16bit {
                        if (written & 1) == 0 {
                            half_buf = b as u16;
                        } else {
                            self.write_16(
                                dst,
                                half_buf | ((b as u16) << 8),
                                AccessType::Sequential,
                            );
                            dst += 2;
                        }
                    } else {
                        self.write_8(dst, b, AccessType::Sequential);
                        dst += 1;
                    }
                    written += 1;
                } else {
                    let b1 = self.read_8(src, AccessType::Sequential, 0, false) as usize;
                    let b2 = self.read_8(src + 1, AccessType::Sequential, 0, false) as usize;
                    src += 2;
                    let len = (b1 >> 4) + 3;
                    let disp = (((b1 & 0x0F) << 8) | b2) + 1;
                    for _ in 0..len {
                        if written >= decomp_len {
                            break;
                        }
                        let copy_addr = if is_16bit {
                            let cur_byte_addr = dst + ((written as u32) & 1);
                            cur_byte_addr.wrapping_sub(disp as u32)
                        } else {
                            dst.wrapping_sub(disp as u32)
                        };
                        let b = if is_16bit && (disp == 1) && ((written & 1) == 1) {
                            (half_buf & 0xFF) as u8
                        } else {
                            self.read_8(copy_addr, AccessType::Sequential, 0, false)
                        };
                        if is_16bit {
                            if (written & 1) == 0 {
                                half_buf = b as u16;
                            } else {
                                self.write_16(
                                    dst,
                                    half_buf | ((b as u16) << 8),
                                    AccessType::Sequential,
                                );
                                dst += 2;
                            }
                        } else {
                            self.write_8(dst, b, AccessType::Sequential);
                            dst += 1;
                        }
                        written += 1;
                    }
                }
            }
        }
    }

    fn emit_decomp_byte(
        &mut self,
        dst: &mut u32,
        written: &mut usize,
        half_buf: &mut u16,
        is_16bit: bool,
        b: u8,
    ) {
        if is_16bit {
            if (*written & 1) == 0 {
                *half_buf = b as u16;
            } else {
                self.write_16(*dst, *half_buf | ((b as u16) << 8), AccessType::Sequential);
                *dst += 2;
            }
        } else {
            self.write_8(*dst, b, AccessType::Sequential);
            *dst += 1;
        }
        *written += 1;
    }

    fn hle_rl_uncomp(&mut self, mut src: u32, mut dst: u32, is_16bit: bool) {
        let header = self.read_32(src, AccessType::Sequential, 0);
        src += 4;
        let decomp_len = (header >> 8) as usize;
        let mut written = 0usize;
        let mut half_buf = 0u16;

        while written < decomp_len {
            let flag = self.read_8(src, AccessType::Sequential, 0, false);
            src += 1;
            if (flag & 0x80) != 0 {
                let len = ((flag & 0x7F) as usize) + 3;
                let data = self.read_8(src, AccessType::Sequential, 0, false);
                src += 1;
                for _ in 0..len {
                    if written >= decomp_len {
                        break;
                    }
                    self.emit_decomp_byte(&mut dst, &mut written, &mut half_buf, is_16bit, data);
                }
            } else {
                let len = ((flag & 0x7F) as usize) + 1;
                for _ in 0..len {
                    if written >= decomp_len {
                        break;
                    }
                    let data = self.read_8(src, AccessType::Sequential, 0, false);
                    src += 1;
                    self.emit_decomp_byte(&mut dst, &mut written, &mut half_buf, is_16bit, data);
                }
            }
        }
    }

    fn hle_huff_uncomp(&mut self, src: u32, mut dst: u32) {
        let header = self.read_32(src, AccessType::Sequential, 0);
        let bit_depth = header & 0x0F;
        if bit_depth != 4 && bit_depth != 8 {
            return;
        }
        let decomp_len = (header >> 8) as usize;
        let tree_size = ((self.read_8(src + 4, AccessType::Sequential, 0, false) as u32) + 1) * 2;
        let tree_base = src + 4;
        let mut data_ptr = src + 4 + tree_size;

        let mut written = 0usize;
        let mut out_word = 0u32;
        let mut out_bits = 0u32;
        let mut node_off = 1u32;

        while written < decomp_len {
            let word = self.read_32(data_ptr, AccessType::Sequential, 0);
            data_ptr += 4;
            for shift in (0..32).rev() {
                if written >= decomp_len {
                    break;
                }
                let bit = (word >> shift) & 1;
                let node_val = self.read_8(tree_base + node_off, AccessType::Sequential, 0, false);
                let is_leaf = if bit == 0 {
                    (node_val & 0x80) != 0
                } else {
                    (node_val & 0x40) != 0
                };
                let next_off = ((node_off & !1) + ((node_val as u32 & 0x3F) + 1) * 2) + bit;
                if is_leaf {
                    let sym = self.read_8(tree_base + next_off, AccessType::Sequential, 0, false);
                    out_word |= (sym as u32) << out_bits;
                    out_bits += bit_depth;
                    if out_bits == 32 {
                        self.write_32(dst, out_word, AccessType::Sequential);
                        dst += 4;
                        out_word = 0;
                        out_bits = 0;
                        written += 4;
                    }
                    node_off = 1;
                } else {
                    node_off = next_off;
                }
            }
        }
    }

    fn hle_diff_unfilter(&mut self, mut src: u32, mut dst: u32, mode: u8) {
        let header = self.read_32(src, AccessType::Sequential, 0);
        src += 4;
        let len = (header >> 8) as usize;
        let is_16bit_data = mode == 0x18;
        let is_vram = mode == 0x17;

        if is_16bit_data {
            let mut acc = 0u16;
            for _ in (0..len).step_by(2) {
                let d = self.read_16(src, AccessType::Sequential, 0, false);
                src += 2;
                acc = acc.wrapping_add(d);
                self.write_16(dst, acc, AccessType::Sequential);
                dst += 2;
            }
        } else {
            let mut acc = 0u8;
            let mut half = 0u16;
            for i in 0..len {
                let d = self.read_8(src, AccessType::Sequential, 0, false);
                src += 1;
                acc = acc.wrapping_add(d);
                if is_vram {
                    if (i & 1) == 0 {
                        half = acc as u16;
                    } else {
                        self.write_16(dst, half | ((acc as u16) << 8), AccessType::Sequential);
                        dst += 2;
                    }
                } else {
                    self.write_8(dst, acc, AccessType::Sequential);
                    dst += 1;
                }
            }
        }
    }
}

fn hle_arctan(i: i32) -> i32 {
    // Polynomial minimax approximation accurate across Q1.14 range [-16384..16384] -> [-0x2000..0x2000]
    let a = -((i * i) >> 14);
    let mut b = ((109 * a) >> 14) - 574;
    b = ((b * a) >> 14) + 1707;
    b = ((b * a) >> 14) - 3476;
    b = ((b * a) >> 14) + 10430;
    (b * i) >> 14
}

fn hle_arctan2(x: i32, y: i32) -> u16 {
    if y == 0 {
        return if x >= 0 { 0 } else { 0x8000 };
    }
    if x == 0 {
        return if y >= 0 { 0x4000 } else { 0xC000 };
    }
    let ax = x.abs();
    let ay = y.abs();
    let angle = if ax >= ay {
        let tan = (y << 14) / x;
        let t = hle_arctan(tan);
        if x > 0 && y >= 0 {
            t
        } else if x < 0 {
            0x8000 + t
        } else {
            0x10000 + t
        }
    } else {
        let cot = (x << 14) / y;
        let t = hle_arctan(cot);
        if y > 0 { 0x4000 - t } else { 0xC000 - t }
    };
    (angle & 0xFFFF) as u16
}

/// Returns `(sin, cos)` in Q2.14 (`-16384..=16384`) for GBA 16-bit angle (`0..0x10000` = `0..2pi`).
fn hle_sin_cos(alpha: u16) -> (i32, i32) {
    // Parabolic / Taylor 5th-order integer approximation on quadrant 0..0x4000
    fn quad_sin(phase: u16) -> i32 {
        let q = (phase >> 14) & 3;
        let mut p = (phase & 0x3FFF) as i64;
        if (q & 1) != 0 {
            p = 0x4000 - p;
        }
        // theta = p * pi / 32768; Taylor series around 0 scaled to 16384:
        // x in Q14 = (p * 25736) >> 14  (pi/2 * 16384 = 25736)
        let x = (p * 25736) >> 14;
        let x2 = (x * x) >> 14;
        // sin(x) ~= x * (1 - x^2/6 + x^4/120)
        // = x - (x * x2 / 6) + ...
        let term1 = x;
        let term2 = (x * x2) / (6 << 14);
        let x5 = (((x * x2) >> 14) * x2) >> 14;
        let term3 = x5 / 120;
        let val = (term1 - term2 + term3).clamp(0, 16384) as i32;
        if q >= 2 { -val } else { val }
    }
    (quad_sin(alpha), quad_sin(alpha.wrapping_add(0x4000)))
}

const SEMITONE_LUT: [u32; 13] = [
    65536,  // 2^(0/12)
    69433,  // 2^(1/12)
    73562,  // 2^(2/12)
    77936,  // 2^(3/12)
    82570,  // 2^(4/12)
    87480,  // 2^(5/12)
    92682,  // 2^(6/12)
    98194,  // 2^(7/12)
    104034, // 2^(8/12)
    110221, // 2^(9/12)
    116776, // 2^(10/12)
    123721, // 2^(11/12)
    131072, // 2^(12/12)
];

fn hle_midi_key_to_freq(wave_freq: u32, key: u8, pitch: u8) -> u32 {
    let total = (key as i32 - 180) * 256 + (pitch as i32);
    let octaves = total.div_euclid(3072);
    let step = total.rem_euclid(3072) as usize;
    let semitone = step / 256;
    let fine = (step % 256) as u32;
    let diff = SEMITONE_LUT[semitone + 1] - SEMITONE_LUT[semitone];
    let mult = SEMITONE_LUT[semitone] + ((diff * fine) >> 8);
    let mut val = ((wave_freq as u64) * (mult as u64)) >> 16;
    if octaves >= 0 {
        if octaves >= 32 {
            u32::MAX
        } else {
            val <<= octaves;
            val.min(u32::MAX as u64) as u32
        }
    } else {
        let neg_octaves = (-octaves) as u32;
        if neg_octaves >= 32 {
            0
        } else {
            (val >> neg_octaves) as u32
        }
    }
}
