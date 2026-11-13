use alloc::{string::String, vec, vec::Vec};
use serde::{Deserialize, Serialize};

const NINTENDO_LOGO_PREFIX: [u8; 8] = [0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B];
const CYCLES_PER_SECOND: u64 = 4_194_304;

/// Parsed Game Boy / Game Boy Color cartridge header (`0x0100..=0x014F`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartridgeHeader {
    pub title: String,
    pub cgb_flag: u8,
    pub cart_type: u8,
    pub rom_size_code: u8,
    pub ram_size_code: u8,
    pub rom_size_bytes: usize,
    pub ram_size_bytes: usize,
    pub header_checksum: u8,
    pub header_checksum_valid: bool,
    pub supports_cgb: bool,
    pub cgb_only: bool,
}

impl CartridgeHeader {
    pub fn parse(rom: &[u8]) -> Self {
        let byte = |addr: usize| -> u8 { rom.get(addr).copied().unwrap_or(0) };

        let cgb_flag = byte(0x0143);
        let supports_cgb = cgb_flag == 0x80 || cgb_flag == 0xC0;
        let cgb_only = cgb_flag == 0xC0;

        let title_end = if supports_cgb { 0x0143 } else { 0x0144 };
        let mut title = String::new();
        for addr in 0x0134..title_end {
            let b = byte(addr);
            if b == 0 {
                break;
            }
            if (0x20..=0x7E).contains(&b) {
                title.push(b as char);
            }
        }
        let trimmed = title.trim().into();

        let cart_type = byte(0x0147);
        let rom_size_code = byte(0x0148);
        let ram_size_code = byte(0x0149);

        let rom_size_bytes = match rom_size_code {
            0x00..=0x08 => 0x8000usize << rom_size_code,
            0x52 => 72 * 0x4000,
            0x53 => 80 * 0x4000,
            0x54 => 96 * 0x4000,
            _ => rom.len().max(0x8000),
        };

        let mut ram_size_bytes = match ram_size_code {
            0x01 => 0x800,
            0x02 => 0x2000,
            0x03 => 0x8000,
            0x04 => 0x20000,
            0x05 => 0x10000,
            _ => 0,
        };
        // MBC2 always includes 512x4-bit internal RAM even when header RAM code is 0x00.
        if matches!(cart_type, 0x05 | 0x06) {
            ram_size_bytes = 512;
        }

        let header_checksum = byte(0x014D);
        let mut calc = 0u8;
        for addr in 0x0134..=0x014C {
            calc = calc.wrapping_sub(byte(addr)).wrapping_sub(1);
        }
        let header_checksum_valid = rom.len() >= 0x0150 && calc == header_checksum;

        Self {
            title: trimmed,
            cgb_flag,
            cart_type,
            rom_size_code,
            ram_size_code,
            rom_size_bytes,
            ram_size_bytes,
            header_checksum,
            header_checksum_valid,
            supports_cgb,
            cgb_only,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MbcType {
    RomOnly,
    Mbc1 { multicart: bool },
    Mbc2,
    Mbc3 { has_rtc: bool },
    Mbc5 { has_rumble: bool },
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtcRegs {
    pub sec: u8,
    pub min: u8,
    pub hour: u8,
    pub day_lo: u8,
    pub day_hi: u8,
}

/// MBC3 Real-Time Clock state with latching and 48-byte save trailer support.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RtcState {
    pub current: RtcRegs,
    pub latched: RtcRegs,
    pub prev_latch_write: u8,
    pub cycle_acc: u64,
    pub timestamp: u64,
}

impl RtcState {
    pub fn tick_normal_cycles(&mut self, normal_cycles: u64) {
        self.cycle_acc += normal_cycles;
        while self.cycle_acc >= CYCLES_PER_SECOND {
            self.cycle_acc -= CYCLES_PER_SECOND;
            self.timestamp = self.timestamp.wrapping_add(1);
            if (self.current.day_hi & 0x40) == 0 {
                self.tick_one_second();
            }
        }
    }

    pub fn advance_seconds(&mut self, seconds: u64) {
        self.timestamp = self.timestamp.wrapping_add(seconds);
        if (self.current.day_hi & 0x40) != 0 {
            return;
        }
        for _ in 0..seconds {
            self.tick_one_second();
        }
    }

    fn tick_one_second(&mut self) {
        self.current.sec = (self.current.sec + 1) & 0x3F;
        if self.current.sec < 60 {
            return;
        }
        self.current.sec = 0;
        self.current.min = (self.current.min + 1) & 0x3F;
        if self.current.min < 60 {
            return;
        }
        self.current.min = 0;
        self.current.hour = (self.current.hour + 1) & 0x1F;
        if self.current.hour < 24 {
            return;
        }
        self.current.hour = 0;
        let days = (self.current.day_lo as u16) | (((self.current.day_hi & 0x01) as u16) << 8);
        let next_days = days + 1;
        self.current.day_lo = (next_days & 0xFF) as u8;
        self.current.day_hi = (self.current.day_hi & 0xFE) | (((next_days >> 8) & 1) as u8);
        if next_days >= 512 {
            self.current.day_hi |= 0x80;
        }
    }

    pub fn latch(&mut self) {
        self.latched = self.current;
    }

    pub fn read_reg(&self, reg: u8) -> u8 {
        match reg {
            0x08 => self.latched.sec,
            0x09 => self.latched.min,
            0x0A => self.latched.hour,
            0x0B => self.latched.day_lo,
            0x0C => self.latched.day_hi,
            _ => 0xFF,
        }
    }

    pub fn write_reg(&mut self, reg: u8, val: u8) {
        match reg {
            0x08 => {
                self.current.sec = val & 0x3F;
                self.cycle_acc = 0;
            }
            0x09 => self.current.min = val & 0x3F,
            0x0A => self.current.hour = val & 0x1F,
            0x0B => self.current.day_lo = val,
            0x0C => self.current.day_hi = val & 0xC1,
            _ => {}
        }
    }

    /// Appends the standard 48-byte MBC3 RTC save trailer (`10 * u32 + u64` LE).
    pub fn append_48_byte_trailer(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.current.sec as u32).to_le_bytes());
        out.extend_from_slice(&(self.current.min as u32).to_le_bytes());
        out.extend_from_slice(&(self.current.hour as u32).to_le_bytes());
        out.extend_from_slice(&(self.current.day_lo as u32).to_le_bytes());
        out.extend_from_slice(&((self.current.day_hi & 0xC1) as u32).to_le_bytes());
        out.extend_from_slice(&(self.latched.sec as u32).to_le_bytes());
        out.extend_from_slice(&(self.latched.min as u32).to_le_bytes());
        out.extend_from_slice(&(self.latched.hour as u32).to_le_bytes());
        out.extend_from_slice(&(self.latched.day_lo as u32).to_le_bytes());
        out.extend_from_slice(&((self.latched.day_hi & 0xC1) as u32).to_le_bytes());
        out.extend_from_slice(&self.timestamp.to_le_bytes());
    }

    /// Parses a 48-byte (or 44-byte) MBC3 RTC save trailer.
    pub fn load_trailer(&mut self, trailer: &[u8]) {
        if trailer.len() < 44 {
            return;
        }
        let r32 = |off: usize| -> u32 {
            u32::from_le_bytes([
                trailer[off],
                trailer[off + 1],
                trailer[off + 2],
                trailer[off + 3],
            ])
        };
        self.current.sec = (r32(0) & 0x3F) as u8;
        self.current.min = (r32(4) & 0x3F) as u8;
        self.current.hour = (r32(8) & 0x1F) as u8;
        self.current.day_lo = (r32(12) & 0xFF) as u8;
        self.current.day_hi = (r32(16) & 0xC1) as u8;

        self.latched.sec = (r32(20) & 0x3F) as u8;
        self.latched.min = (r32(24) & 0x3F) as u8;
        self.latched.hour = (r32(28) & 0x1F) as u8;
        self.latched.day_lo = (r32(32) & 0xFF) as u8;
        self.latched.day_hi = (r32(36) & 0xC1) as u8;

        if trailer.len() >= 48 {
            self.timestamp = u64::from_le_bytes([
                trailer[40],
                trailer[41],
                trailer[42],
                trailer[43],
                trailer[44],
                trailer[45],
                trailer[46],
                trailer[47],
            ]);
        } else {
            self.timestamp = r32(40) as u64;
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cartridge {
    #[serde(skip)]
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
    pub header: CartridgeHeader,
    pub mbc: MbcType,
    pub has_battery: bool,
    pub battery_dirty: bool,

    // Mapper register state
    pub ram_enabled: bool,
    pub rom_bank: u16,
    pub bank_hi: u8,
    pub banking_mode: u8,
    pub ram_or_rtc_reg: u8,
    pub rumble_active: bool,
    pub rtc: RtcState,
}

impl Cartridge {
    pub fn new(mut rom: Vec<u8>) -> Self {
        if rom.len() < 0x8000 {
            rom.resize(0x8000, 0xFF);
        }
        let header = CartridgeHeader::parse(&rom);

        let (mbc, has_battery) = match header.cart_type {
            0x00 => (MbcType::RomOnly, false),
            0x08 => (MbcType::RomOnly, false),
            0x09 => (MbcType::RomOnly, true),
            0x01 | 0x02 => (
                MbcType::Mbc1 {
                    multicart: Self::detect_mbc1m(&rom),
                },
                false,
            ),
            0x03 => (
                MbcType::Mbc1 {
                    multicart: Self::detect_mbc1m(&rom),
                },
                true,
            ),
            0x05 => (MbcType::Mbc2, false),
            0x06 => (MbcType::Mbc2, true),
            0x0F | 0x10 => (MbcType::Mbc3 { has_rtc: true }, true),
            0x11 | 0x12 => (MbcType::Mbc3 { has_rtc: false }, false),
            0x13 => (MbcType::Mbc3 { has_rtc: false }, true),
            0x19 | 0x1A => (MbcType::Mbc5 { has_rumble: false }, false),
            0x1B => (MbcType::Mbc5 { has_rumble: false }, true),
            0x1C | 0x1D => (MbcType::Mbc5 { has_rumble: true }, false),
            0x1E => (MbcType::Mbc5 { has_rumble: true }, true),
            _ => {
                log::warn!(
                    "Unsupported GB cartridge type 0x{:02X}; falling back to MBC5",
                    header.cart_type
                );
                (MbcType::Mbc5 { has_rumble: false }, false)
            }
        };

        let ram_len = match mbc {
            MbcType::Mbc2 => 512,
            MbcType::RomOnly => {
                if matches!(header.cart_type, 0x08 | 0x09) {
                    header.ram_size_bytes.max(0x2000)
                } else {
                    header.ram_size_bytes
                }
            }
            _ => header.ram_size_bytes,
        };

        Self {
            rom,
            ram: vec![0xFF; ram_len],
            header,
            mbc,
            has_battery,
            battery_dirty: false,
            ram_enabled: false,
            rom_bank: 1,
            bank_hi: 0,
            banking_mode: 0,
            ram_or_rtc_reg: 0,
            rumble_active: false,
            rtc: RtcState::default(),
        }
    }

    /// Detects 1 MiB MBC1M multicarts (which shift the 2-bit upper bank by 4 instead of 5).
    fn detect_mbc1m(rom: &[u8]) -> bool {
        if rom.len() != 0x10_0000 {
            return false;
        }
        let has_logo_at = |base: usize| -> bool {
            rom.get(base + 0x0104..base + 0x010C)
                .map(|s| s == NINTENDO_LOGO_PREFIX)
                .unwrap_or(false)
        };
        has_logo_at(0x40000) && has_logo_at(0x80000)
    }

    pub fn reset_mapper(&mut self) {
        self.ram_enabled = false;
        self.rom_bank = 1;
        self.bank_hi = 0;
        self.banking_mode = 0;
        self.ram_or_rtc_reg = 0;
        self.rumble_active = false;
        self.rtc.prev_latch_write = 0;
    }

    #[inline]
    fn rom_banks(&self) -> usize {
        (self.rom.len() / 0x4000).max(1)
    }

    #[inline]
    fn ram_banks(&self) -> usize {
        (self.ram.len() / 0x2000).max(1)
    }

    fn effective_rom_banks(&self) -> (usize, usize) {
        let total = self.rom_banks();
        match self.mbc {
            MbcType::RomOnly => (0, 1 % total),
            MbcType::Mbc1 { multicart } => {
                let lo = (self.rom_bank as usize) & 0x1F;
                let lo_nz = if lo == 0 { 1 } else { lo };
                let hi = (self.bank_hi as usize) & 0x03;
                if multicart {
                    let b0 = if self.banking_mode != 0 {
                        (hi << 4) % total
                    } else {
                        0
                    };
                    let b1 = ((hi << 4) | (lo_nz & 0x0F)) % total;
                    (b0, b1)
                } else {
                    let b0 = if self.banking_mode != 0 {
                        (hi << 5) % total
                    } else {
                        0
                    };
                    let b1 = ((hi << 5) | lo_nz) % total;
                    (b0, b1)
                }
            }
            MbcType::Mbc2 => (0, ((self.rom_bank as usize & 0x0F).max(1)) % total),
            MbcType::Mbc3 { .. } => {
                let mask = if self.rom.len() > 0x20_0000 {
                    0xFF
                } else {
                    0x7F
                };
                let b1 = ((self.rom_bank as usize & mask).max(1)) % total;
                (0, b1)
            }
            MbcType::Mbc5 { .. } => (0, (self.rom_bank as usize & 0x1FF) % total),
        }
    }

    pub fn read_rom(&self, addr: u16) -> u8 {
        let (b0, b1) = self.effective_rom_banks();
        let offset = if addr < 0x4000 {
            b0 * 0x4000 + (addr as usize)
        } else {
            b1 * 0x4000 + ((addr as usize) & 0x3FFF)
        };
        self.rom.get(offset).copied().unwrap_or(0xFF)
    }

    pub fn write_rom(&mut self, addr: u16, val: u8) {
        match self.mbc {
            MbcType::RomOnly => {}
            MbcType::Mbc1 { .. } => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => self.rom_bank = (val & 0x1F) as u16,
                0x4000..=0x5FFF => self.bank_hi = val & 0x03,
                0x6000..=0x7FFF => self.banking_mode = val & 0x01,
                _ => {}
            },
            MbcType::Mbc2 => {
                if addr <= 0x3FFF {
                    if (addr & 0x0100) == 0 {
                        self.ram_enabled = (val & 0x0F) == 0x0A;
                    } else {
                        self.rom_bank = (val & 0x0F) as u16;
                    }
                }
            }
            MbcType::Mbc3 { has_rtc } => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => self.rom_bank = val as u16,
                0x4000..=0x5FFF => self.ram_or_rtc_reg = val,
                0x6000..=0x7FFF => {
                    if has_rtc && self.rtc.prev_latch_write == 0x00 && val == 0x01 {
                        self.rtc.latch();
                    }
                    self.rtc.prev_latch_write = val;
                }
                _ => {}
            },
            MbcType::Mbc5 { has_rumble } => match addr {
                0x0000..=0x1FFF => self.ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x2FFF => {
                    self.rom_bank = (self.rom_bank & 0x0100) | (val as u16);
                }
                0x3000..=0x3FFF => {
                    self.rom_bank = (self.rom_bank & 0x00FF) | (((val as u16) & 0x01) << 8);
                }
                0x4000..=0x5FFF => {
                    if has_rumble {
                        self.rumble_active = (val & 0x08) != 0;
                        self.ram_or_rtc_reg = val & 0x07;
                    } else {
                        self.ram_or_rtc_reg = val & 0x0F;
                    }
                }
                _ => {}
            },
        }
    }

    pub fn read_ram(&self, addr: u16) -> u8 {
        match self.mbc {
            MbcType::RomOnly => {
                if self.ram.is_empty() {
                    return 0xFF;
                }
                let off = (addr as usize - 0xA000) % self.ram.len();
                self.ram[off]
            }
            MbcType::Mbc1 { .. } => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return 0xFF;
                }
                let bank = if self.banking_mode != 0 {
                    (self.bank_hi as usize) % self.ram_banks()
                } else {
                    0
                };
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off]
            }
            MbcType::Mbc2 => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return 0xFF;
                }
                let off = (addr as usize - 0xA000) & 0x01FF;
                0xF0 | (self.ram[off] & 0x0F)
            }
            MbcType::Mbc3 { has_rtc } => {
                if !self.ram_enabled {
                    return 0xFF;
                }
                if has_rtc && (0x08..=0x0C).contains(&self.ram_or_rtc_reg) {
                    return self.rtc.read_reg(self.ram_or_rtc_reg);
                }
                if self.ram.is_empty() {
                    return 0xFF;
                }
                let mask = if self.ram.len() > 0x8000 { 0x07 } else { 0x03 };
                let bank = (self.ram_or_rtc_reg as usize & mask) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off]
            }
            MbcType::Mbc5 { .. } => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return 0xFF;
                }
                let bank = (self.ram_or_rtc_reg as usize) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off]
            }
        }
    }

    pub fn write_ram(&mut self, addr: u16, val: u8) {
        match self.mbc {
            MbcType::RomOnly => {
                if !self.ram.is_empty() {
                    let off = (addr as usize - 0xA000) % self.ram.len();
                    self.ram[off] = val;
                    self.battery_dirty = true;
                }
            }
            MbcType::Mbc1 { .. } => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return;
                }
                let bank = if self.banking_mode != 0 {
                    (self.bank_hi as usize) % self.ram_banks()
                } else {
                    0
                };
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
                self.battery_dirty = true;
            }
            MbcType::Mbc2 => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return;
                }
                let off = (addr as usize - 0xA000) & 0x01FF;
                self.ram[off] = val & 0x0F;
                self.battery_dirty = true;
            }
            MbcType::Mbc3 { has_rtc } => {
                if !self.ram_enabled {
                    return;
                }
                if has_rtc && (0x08..=0x0C).contains(&self.ram_or_rtc_reg) {
                    self.rtc.write_reg(self.ram_or_rtc_reg, val);
                    self.battery_dirty = true;
                    return;
                }
                if self.ram.is_empty() {
                    return;
                }
                let mask = if self.ram.len() > 0x8000 { 0x07 } else { 0x03 };
                let bank = (self.ram_or_rtc_reg as usize & mask) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
                self.battery_dirty = true;
            }
            MbcType::Mbc5 { .. } => {
                if !self.ram_enabled || self.ram.is_empty() {
                    return;
                }
                let bank = (self.ram_or_rtc_reg as usize) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
                self.battery_dirty = true;
            }
        }
    }

    pub fn poke_ram_direct(&mut self, addr: u16, val: u8) {
        if self.ram.is_empty() {
            return;
        }
        match self.mbc {
            MbcType::Mbc2 => {
                let off = (addr as usize - 0xA000) & 0x01FF;
                self.ram[off] = val & 0x0F;
            }
            MbcType::RomOnly => {
                let off = (addr as usize - 0xA000) % self.ram.len();
                self.ram[off] = val;
            }
            MbcType::Mbc1 { .. } => {
                let bank = if self.banking_mode != 0 {
                    (self.bank_hi as usize) % self.ram_banks()
                } else {
                    0
                };
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
            }
            MbcType::Mbc3 { .. } => {
                let mask = if self.ram.len() > 0x8000 { 0x07 } else { 0x03 };
                let bank = (self.ram_or_rtc_reg as usize & mask) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
            }
            MbcType::Mbc5 { .. } => {
                let bank = (self.ram_or_rtc_reg as usize) % self.ram_banks();
                let off = (bank * 0x2000 + (addr as usize - 0xA000)) % self.ram.len();
                self.ram[off] = val;
            }
        }
    }

    pub fn tick_rtc(&mut self, normal_cycles: u64) {
        if matches!(self.mbc, MbcType::Mbc3 { has_rtc: true }) {
            self.rtc.tick_normal_cycles(normal_cycles);
        }
    }

    pub fn has_rtc(&self) -> bool {
        matches!(self.mbc, MbcType::Mbc3 { has_rtc: true })
    }

    pub fn export_battery_ram(&self) -> Vec<u8> {
        if !self.has_battery {
            return Vec::new();
        }
        let mut out = self.ram.clone();
        if self.has_rtc() {
            self.rtc.append_48_byte_trailer(&mut out);
        }
        out
    }

    pub fn import_battery_ram(&mut self, data: &[u8]) {
        let ram_len = self.ram.len();
        let copy_len = data.len().min(ram_len);
        self.ram[..copy_len].copy_from_slice(&data[..copy_len]);
        if matches!(self.mbc, MbcType::Mbc2) {
            for b in &mut self.ram[..copy_len] {
                *b &= 0x0F;
            }
        }
        if self.has_rtc() && data.len() >= ram_len + 44 {
            self.rtc.load_trailer(&data[ram_len..]);
        }
        self.battery_dirty = false;
    }
}
