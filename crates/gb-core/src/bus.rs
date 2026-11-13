use alloc::{boxed::Box, vec::Vec};
use serde::{Deserialize, Serialize};

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::ppu::{DmgPalette, Ppu};
use crate::serde_helpers::{alloc_boxed_array, box_bytes, fixed_bytes};
use crate::serial::Serial;
use crate::timer::Timer;
use crate::{KEY_A, KEY_B, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_SELECT, KEY_START, KEY_UP};

pub const IRQ_VBLANK: u8 = 1 << 0;
pub const IRQ_STAT: u8 = 1 << 1;
pub const IRQ_TIMER: u8 = 1 << 2;
pub const IRQ_SERIAL: u8 = 1 << 3;
pub const IRQ_JOYPAD: u8 = 1 << 4;

/// Game Boy / Game Boy Color System Bus connecting CPU, PPU, APU, Timer, Serial,
/// WRAM/HRAM, DMA/HDMA, and Cartridge MBC.
#[derive(Clone, Serialize, Deserialize)]
pub struct GbBus {
    pub is_cgb: bool,
    pub gba_mode: bool,

    pub boot_rom: Vec<u8>,
    pub boot_rom_enabled: bool,

    pub cart: Cartridge,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub serial: Serial,

    #[serde(with = "box_bytes")]
    pub wram: Box<[u8; 0x8000]>,
    #[serde(with = "fixed_bytes")]
    pub hram: [u8; 0x7F],
    pub svbk: u8,

    // CGB speed switch state
    pub key1_arm: bool,
    pub double_speed: bool,

    // Interrupts
    pub if_flag: u8,
    pub ie_flag: u8,

    // Joypad (`P1` 0xFF00) using GBA active-low 10-bit key layout
    pub p1_select: u8,
    pub keypad_active_low: u16,
    prev_p1_low: u8,
    pub joypad_wake: bool,

    // OAM DMA (`0xFF46`)
    pub oam_dma_active: bool,
    pub oam_dma_src: u16,
    pub oam_dma_index: u16,
    pub oam_dma_delay: u8,

    // CGB HDMA / GDMA (`0xFF51..=0xFF55`)
    pub hdma_src: u16,
    pub hdma_dst: u16,
    pub hdma_active: bool,
    pub hdma_cancelled: bool,
    pub hdma_remaining: u8,
    pub hdma_stall_m_cycles: u16,

    // Track whether CPU is halted (pauses HBlank HDMA per CGB hardware)
    pub cpu_halted: bool,

    /// Normal-speed T-cycles (dots) elapsed since creation/reset.
    pub total_normal_dots: u64,
}

impl GbBus {
    pub fn new(
        rom: Vec<u8>,
        boot_rom: Option<&[u8]>,
        is_cgb: bool,
        gba_mode: bool,
        sample_rate_hz: u32,
        dmg_palette: DmgPalette,
    ) -> Self {
        let boot_vec = boot_rom.map(|b| b.to_vec()).unwrap_or_default();
        let has_boot = !boot_vec.is_empty();

        let initial_div = if has_boot {
            0x0000
        } else if is_cgb {
            if gba_mode { 0x2600 } else { 0x1E00 }
        } else {
            0xABCC
        };

        let mut bus = Self {
            is_cgb,
            gba_mode,
            boot_rom: boot_vec,
            boot_rom_enabled: has_boot,
            cart: Cartridge::new(rom),
            ppu: Ppu::new(dmg_palette),
            apu: Apu::new(sample_rate_hz),
            timer: Timer::new(initial_div),
            serial: Serial::new(),
            wram: alloc_boxed_array(0u8),
            hram: [0u8; 0x7F],
            svbk: 1,
            key1_arm: false,
            double_speed: false,
            if_flag: if has_boot { 0x00 } else { 0x01 },
            ie_flag: 0x00,
            p1_select: 0x30,
            keypad_active_low: 0x03FF,
            prev_p1_low: 0x0F,
            joypad_wake: false,
            oam_dma_active: false,
            oam_dma_src: 0,
            oam_dma_index: 0,
            oam_dma_delay: 0,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_active: false,
            hdma_cancelled: false,
            hdma_remaining: 0,
            hdma_stall_m_cycles: 0,
            cpu_halted: false,
            total_normal_dots: 0,
        };

        if !has_boot {
            bus.ppu.init_post_boot(is_cgb);
            bus.apu.init_post_boot();
            bus.p1_select = 0x30;
        }
        bus
    }

    #[inline]
    pub fn request_irq(&mut self, mask: u8) {
        self.if_flag |= mask & 0x1F;
    }

    /// Updates keypad state from GBA active-low 10-bit KEYINPUT layout (`0x03FF` = none pressed).
    pub fn update_keypad(&mut self, keyinput_active_low: u16) {
        self.keypad_active_low = keyinput_active_low & 0x03FF;
        self.check_joypad_irq();
    }

    #[inline]
    fn compute_p1_low(&self) -> u8 {
        let mut low = 0x0Fu8;
        let k = self.keypad_active_low;
        // Bit 4 == 0 selects D-pad (Right, Left, Up, Down)
        if (self.p1_select & 0x10) == 0 {
            if (k & KEY_RIGHT) == 0 {
                low &= !0x01;
            }
            if (k & KEY_LEFT) == 0 {
                low &= !0x02;
            }
            if (k & KEY_UP) == 0 {
                low &= !0x04;
            }
            if (k & KEY_DOWN) == 0 {
                low &= !0x08;
            }
        }
        // Bit 5 == 0 selects Action buttons (A, B, Select, Start)
        if (self.p1_select & 0x20) == 0 {
            if (k & KEY_A) == 0 {
                low &= !0x01;
            }
            if (k & KEY_B) == 0 {
                low &= !0x02;
            }
            if (k & KEY_SELECT) == 0 {
                low &= !0x04;
            }
            if (k & KEY_START) == 0 {
                low &= !0x08;
            }
        }
        low
    }

    fn check_joypad_irq(&mut self) {
        let new_low = self.compute_p1_low();
        if (self.prev_p1_low & !new_low) != 0 {
            self.request_irq(IRQ_JOYPAD);
            self.joypad_wake = true;
        }
        self.prev_p1_low = new_low;
    }

    /// Advances all hardware subsystems by 1 CPU M-cycle (`4` CPU T-cycles).
    ///
    /// In normal speed, `4` CPU T-cycles equal `4` PPU/APU normal T-cycles (dots).
    /// In CGB double-speed mode, `4` CPU T-cycles equal `2` PPU/APU normal T-cycles.
    pub fn tick_m_cycle(&mut self) {
        let normal_dots: u32 = if self.double_speed { 2 } else { 4 };
        self.total_normal_dots += normal_dots as u64;

        // Timer & DIV-driven APU frame sequencer (CPU clock domain)
        let t_res = self.timer.step_m_cycle(self.double_speed);
        if t_res.timer_irq {
            self.request_irq(IRQ_TIMER);
        }
        if t_res.apu_frame_seq {
            self.apu.tick_frame_sequencer();
        }

        // Serial link stub (CPU clock domain)
        if self.serial.step_m_cycle() {
            self.request_irq(IRQ_SERIAL);
        }

        // OAM DMA (1 byte per CPU M-cycle after 1 M-cycle startup delay)
        if self.oam_dma_active {
            if self.oam_dma_delay > 0 {
                self.oam_dma_delay -= 1;
            } else {
                let addr = self.oam_dma_src.wrapping_add(self.oam_dma_index);
                let byte = self.dma_read8(addr);
                self.ppu.oam[self.oam_dma_index as usize] = byte;
                self.oam_dma_index += 1;
                if self.oam_dma_index >= 160 {
                    self.oam_dma_active = false;
                }
            }
        }

        // PPU & APU & Cartridge RTC (4.194304 MHz normal clock domain)
        let ppu_res = self.ppu.step(normal_dots, self.is_cgb);
        if ppu_res.vblank_irq {
            self.request_irq(IRQ_VBLANK);
        }
        if ppu_res.stat_irq {
            self.request_irq(IRQ_STAT);
        }
        if ppu_res.entered_hblank && self.is_cgb && self.hdma_active && !self.cpu_halted {
            self.transfer_hdma_block();
        }

        self.apu.step(normal_dots);
        self.cart.tick_rtc(normal_dots as u64);
    }

    fn transfer_hdma_block(&mut self) {
        let vbank = ((self.ppu.vbk & 1) as usize) * 0x2000;
        for _ in 0..16 {
            let val = self.dma_read8(self.hdma_src);
            let vram_off = (self.hdma_dst as usize) & 0x1FFF;
            self.ppu.vram[vbank + vram_off] = val;
            self.hdma_src = self.hdma_src.wrapping_add(1);
            self.hdma_dst = (self.hdma_dst + 1) & 0x1FFF;
        }
        self.hdma_stall_m_cycles += if self.double_speed { 16 } else { 8 };
        self.hdma_remaining = self.hdma_remaining.saturating_sub(1);
        if self.hdma_remaining == 0 {
            self.hdma_active = false;
            self.hdma_cancelled = false;
        }
    }

    #[inline]
    fn dma_read8(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cart.read_rom(addr),
            0x8000..=0x9FFF => {
                let bank = if self.is_cgb {
                    (self.ppu.vbk & 1) as usize
                } else {
                    0
                };
                self.ppu.vram[bank * 0x2000 + ((addr as usize) & 0x1FFF)]
            }
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr as usize) & 0x0FFF],
            0xD000..=0xDFFF => {
                let bank = if self.is_cgb {
                    let b = self.svbk & 0x07;
                    if b == 0 { 1 } else { b as usize }
                } else {
                    1
                };
                self.wram[bank * 0x1000 + ((addr as usize) & 0x0FFF)]
            }
            0xE000..=0xFFFF => {
                // Echo / upper addresses map to WRAM during OAM DMA on hardware.
                let mirrored = addr & 0xDFFF;
                if mirrored < 0xD000 {
                    self.wram[(mirrored as usize) & 0x0FFF]
                } else {
                    let bank = if self.is_cgb {
                        let b = self.svbk & 0x07;
                        if b == 0 { 1 } else { b as usize }
                    } else {
                        1
                    };
                    self.wram[bank * 0x1000 + ((mirrored as usize) & 0x0FFF)]
                }
            }
        }
    }

    fn read_boot_rom_byte(&self, addr: u16) -> Option<u8> {
        if !self.boot_rom_enabled {
            return None;
        }
        if self.is_cgb {
            if addr < 0x0100 || (0x0200..0x0900).contains(&addr) {
                return self.boot_rom.get(addr as usize).copied();
            }
        } else if addr < 0x0100 {
            return self.boot_rom.get(addr as usize).copied();
        }
        None
    }

    /// Performs one CPU read M-cycle: advances hardware by 1 M-cycle, then latches the read value.
    pub fn read8(&mut self, addr: u16) -> u8 {
        self.tick_m_cycle();
        self.read8_immediate(addr)
    }

    /// Reads an address without advancing cycles (used after `tick_m_cycle()`).
    pub fn read8_immediate(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => {
                if let Some(b) = self.read_boot_rom_byte(addr) {
                    return b;
                }
                self.cart.read_rom(addr)
            }
            0x8000..=0x9FFF => self.ppu.read_vram(addr, self.is_cgb),
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr as usize) & 0x0FFF],
            0xD000..=0xDFFF => self.wram[self.wram_bank() * 0x1000 + ((addr as usize) & 0x0FFF)],
            0xE000..=0xFDFF => {
                let m = addr - 0x2000;
                if m < 0xD000 {
                    self.wram[(m as usize) & 0x0FFF]
                } else {
                    self.wram[self.wram_bank() * 0x1000 + ((m as usize) & 0x0FFF)]
                }
            }
            0xFE00..=0xFE9F => {
                if self.oam_dma_active {
                    return 0xFF;
                }
                self.ppu.read_oam(addr)
            }
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie_flag,
        }
    }

    /// Performs one CPU write M-cycle: advances hardware by 1 M-cycle, then commits the write.
    pub fn write8(&mut self, addr: u16, val: u8) {
        self.tick_m_cycle();
        self.write8_immediate(addr, val);
    }

    pub fn write8_immediate(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => self.cart.write_rom(addr, val),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, val, self.is_cgb),
            0xA000..=0xBFFF => self.cart.write_ram(addr, val),
            0xC000..=0xCFFF => self.wram[(addr as usize) & 0x0FFF] = val,
            0xD000..=0xDFFF => {
                let bank = self.wram_bank();
                self.wram[bank * 0x1000 + ((addr as usize) & 0x0FFF)] = val;
            }
            0xE000..=0xFDFF => {
                let m = addr - 0x2000;
                if m < 0xD000 {
                    self.wram[(m as usize) & 0x0FFF] = val;
                } else {
                    let bank = self.wram_bank();
                    self.wram[bank * 0x1000 + ((m as usize) & 0x0FFF)] = val;
                }
            }
            0xFE00..=0xFE9F => {
                if !self.oam_dma_active {
                    self.ppu.write_oam(addr, val);
                }
            }
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => self.write_io(addr, val),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.ie_flag = val,
        }
    }

    #[inline]
    fn wram_bank(&self) -> usize {
        if self.is_cgb {
            let b = self.svbk & 0x07;
            if b == 0 { 1 } else { b as usize }
        } else {
            1
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => 0xC0 | (self.p1_select & 0x30) | self.compute_p1_low(),
            0xFF01 => self.serial.read_sb(),
            0xFF02 => self.serial.read_sc(self.is_cgb),
            0xFF04 => self.timer.read_div(),
            0xFF05 => self.timer.read_tima(),
            0xFF06 => self.timer.read_tma(),
            0xFF07 => self.timer.read_tac(),
            0xFF0F => 0xE0 | (self.if_flag & 0x1F),
            0xFF10..=0xFF3F => self.apu.read_reg(addr),
            0xFF40 => self.ppu.lcdc,
            0xFF41 => self.ppu.read_stat(),
            0xFF42 => self.ppu.scy,
            0xFF43 => self.ppu.scx,
            0xFF44 => self.ppu.ly,
            0xFF45 => self.ppu.lyc,
            0xFF46 => (self.oam_dma_src >> 8) as u8,
            0xFF47 => self.ppu.bgp,
            0xFF48 => self.ppu.obp0,
            0xFF49 => self.ppu.obp1,
            0xFF4A => self.ppu.wy,
            0xFF4B => self.ppu.wx,
            0xFF4D if self.is_cgb => {
                0x7E | (if self.double_speed { 0x80 } else { 0 })
                    | (if self.key1_arm { 1 } else { 0 })
            }
            0xFF4F if self.is_cgb => self.ppu.read_vbk(),
            0xFF55 if self.is_cgb => {
                if self.hdma_active {
                    self.hdma_remaining.saturating_sub(1) & 0x7F
                } else if self.hdma_cancelled {
                    0x80 | (self.hdma_remaining.saturating_sub(1) & 0x7F)
                } else {
                    0xFF
                }
            }
            0xFF68 if self.is_cgb => self.ppu.read_bcps(),
            0xFF69 if self.is_cgb => self.ppu.read_bcpd(),
            0xFF6A if self.is_cgb => self.ppu.read_ocps(),
            0xFF6B if self.is_cgb => self.ppu.read_ocpd(),
            0xFF70 if self.is_cgb => 0xF8 | (self.svbk & 0x07),
            _ => 0xFF,
        }
    }

    fn write_io(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF00 => {
                self.p1_select = val & 0x30;
                self.check_joypad_irq();
            }
            0xFF01 => self.serial.write_sb(val),
            0xFF02 => self.serial.write_sc(val, self.is_cgb),
            0xFF04 => {
                let res = self.timer.write_div(self.double_speed);
                if res.apu_frame_seq {
                    self.apu.tick_frame_sequencer();
                }
            }
            0xFF05 => self.timer.write_tima(val),
            0xFF06 => self.timer.write_tma(val),
            0xFF07 => self.timer.write_tac(val),
            0xFF0F => self.if_flag = val & 0x1F,
            0xFF10..=0xFF3F => self.apu.write_reg(addr, val, self.is_cgb),
            0xFF40 => {
                if self.ppu.write_lcdc(val) {
                    self.request_irq(IRQ_STAT);
                }
            }
            0xFF41 => {
                if self.ppu.write_stat(val) {
                    self.request_irq(IRQ_STAT);
                }
            }
            0xFF42 => self.ppu.scy = val,
            0xFF43 => self.ppu.scx = val,
            0xFF45 => {
                if self.ppu.write_lyc(val) {
                    self.request_irq(IRQ_STAT);
                }
            }
            0xFF46 => {
                self.oam_dma_src = (val as u16) << 8;
                self.oam_dma_index = 0;
                self.oam_dma_delay = 1;
                self.oam_dma_active = true;
            }
            0xFF47 => self.ppu.bgp = val,
            0xFF48 => self.ppu.obp0 = val,
            0xFF49 => self.ppu.obp1 = val,
            0xFF4A => self.ppu.wy = val,
            0xFF4B => self.ppu.wx = val,
            0xFF4D if self.is_cgb => self.key1_arm = (val & 0x01) != 0,
            0xFF4F if self.is_cgb => self.ppu.write_vbk(val),
            0xFF50 => {
                if val != 0 {
                    self.boot_rom_enabled = false;
                }
            }
            0xFF51 if self.is_cgb => {
                self.hdma_src = (self.hdma_src & 0x00FF) | ((val as u16) << 8);
            }
            0xFF52 if self.is_cgb => {
                self.hdma_src = (self.hdma_src & 0xFF00) | ((val as u16) & 0xF0);
            }
            0xFF53 if self.is_cgb => {
                self.hdma_dst = (self.hdma_dst & 0x00FF) | (((val as u16) & 0x1F) << 8);
            }
            0xFF54 if self.is_cgb => {
                self.hdma_dst = (self.hdma_dst & 0xFF00) | ((val as u16) & 0xF0);
            }
            0xFF55 if self.is_cgb => self.write_hdma5(val),
            0xFF68 if self.is_cgb => self.ppu.write_bcps(val),
            0xFF69 if self.is_cgb => self.ppu.write_bcpd(val),
            0xFF6A if self.is_cgb => self.ppu.write_ocps(val),
            0xFF6B if self.is_cgb => self.ppu.write_ocpd(val),
            0xFF70 if self.is_cgb => self.svbk = val & 0x07,
            _ => {}
        }
    }

    fn write_hdma5(&mut self, val: u8) {
        if self.hdma_active && (val & 0x80) == 0 {
            self.hdma_active = false;
            self.hdma_cancelled = true;
            return;
        }

        let blocks = (val & 0x7F) + 1;
        if (val & 0x80) == 0 {
            // General-purpose DMA (GDMA): immediately transfers all blocks while halting CPU.
            let vbank = ((self.ppu.vbk & 1) as usize) * 0x2000;
            for _ in 0..blocks {
                for _ in 0..16 {
                    let b = self.dma_read8(self.hdma_src);
                    let vram_off = (self.hdma_dst as usize) & 0x1FFF;
                    self.ppu.vram[vbank + vram_off] = b;
                    self.hdma_src = self.hdma_src.wrapping_add(1);
                    self.hdma_dst = (self.hdma_dst + 1) & 0x1FFF;
                }
            }
            self.hdma_active = false;
            self.hdma_cancelled = false;
            self.hdma_remaining = 0;
            self.hdma_stall_m_cycles += (blocks as u16) * if self.double_speed { 16 } else { 8 };
        } else {
            self.hdma_remaining = blocks;
            self.hdma_active = true;
            self.hdma_cancelled = false;
        }
    }

    /// Side-effect-free debug read (ignores PPU Mode 2/3 and OAM DMA locks, does not tick cycles).
    pub fn peek8(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => {
                if let Some(b) = self.read_boot_rom_byte(addr) {
                    return b;
                }
                self.cart.read_rom(addr)
            }
            0x8000..=0x9FFF => {
                let bank = if self.is_cgb {
                    (self.ppu.vbk & 1) as usize
                } else {
                    0
                };
                self.ppu.vram[bank * 0x2000 + ((addr as usize) & 0x1FFF)]
            }
            0xA000..=0xBFFF => self.cart.read_ram(addr),
            0xC000..=0xCFFF => self.wram[(addr as usize) & 0x0FFF],
            0xD000..=0xDFFF => self.wram[self.wram_bank() * 0x1000 + ((addr as usize) & 0x0FFF)],
            0xE000..=0xFDFF => {
                let m = addr - 0x2000;
                if m < 0xD000 {
                    self.wram[(m as usize) & 0x0FFF]
                } else {
                    self.wram[self.wram_bank() * 0x1000 + ((m as usize) & 0x0FFF)]
                }
            }
            0xFE00..=0xFE9F => self.ppu.oam[(addr as usize) - 0xFE00],
            0xFEA0..=0xFEFF => 0xFF,
            0xFF00..=0xFF7F => match addr {
                0xFF69 if self.is_cgb => self.ppu.bg_cram[(self.ppu.bcps & 0x3F) as usize],
                0xFF6B if self.is_cgb => self.ppu.obj_cram[(self.ppu.ocps & 0x3F) as usize],
                _ => self.read_io(addr),
            },
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.ie_flag,
        }
    }

    /// Side-effect-free debug write (writes directly to memory without triggering MBC switches,
    /// DMA transfers, or cycle ticks).
    pub fn poke8(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => {
                let off = addr as usize;
                if off < self.cart.rom.len() {
                    self.cart.rom[off] = val;
                }
            }
            0x8000..=0x9FFF => {
                let bank = if self.is_cgb {
                    (self.ppu.vbk & 1) as usize
                } else {
                    0
                };
                self.ppu.vram[bank * 0x2000 + ((addr as usize) & 0x1FFF)] = val;
            }
            0xA000..=0xBFFF => self.cart.poke_ram_direct(addr, val),
            0xC000..=0xCFFF => self.wram[(addr as usize) & 0x0FFF] = val,
            0xD000..=0xDFFF => {
                let bank = self.wram_bank();
                self.wram[bank * 0x1000 + ((addr as usize) & 0x0FFF)] = val;
            }
            0xE000..=0xFDFF => {
                let m = addr - 0x2000;
                if m < 0xD000 {
                    self.wram[(m as usize) & 0x0FFF] = val;
                } else {
                    let bank = self.wram_bank();
                    self.wram[bank * 0x1000 + ((m as usize) & 0x0FFF)] = val;
                }
            }
            0xFE00..=0xFE9F => self.ppu.oam[(addr as usize) - 0xFE00] = val,
            0xFEA0..=0xFEFF => {}
            0xFF00..=0xFF7F => match addr {
                0xFF05 => self.timer.tima = val,
                0xFF06 => self.timer.tma = val,
                0xFF0F => self.if_flag = val & 0x1F,
                0xFF42 => self.ppu.scy = val,
                0xFF43 => self.ppu.scx = val,
                0xFF47 => self.ppu.bgp = val,
                0xFF48 => self.ppu.obp0 = val,
                0xFF49 => self.ppu.obp1 = val,
                0xFF4A => self.ppu.wy = val,
                0xFF4B => self.ppu.wx = val,
                0xFF69 if self.is_cgb => {
                    self.ppu.bg_cram[(self.ppu.bcps & 0x3F) as usize] = val;
                }
                0xFF6B if self.is_cgb => {
                    self.ppu.obj_cram[(self.ppu.ocps & 0x3F) as usize] = val;
                }
                _ => {}
            },
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.ie_flag = val,
        }
    }
}
