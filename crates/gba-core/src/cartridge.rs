use alloc::{boxed::Box, string::String, vec::Vec};
use serde::{Deserialize, Serialize};

use crate::serde_helpers::alloc_boxed_array;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveType {
    None,
    Sram32K,
    Flash64K,
    Flash128K,
    Eeprom512B,
    Eeprom8K,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MapperType {
    Standard,
    GbaVideo64M,
    VastFame,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlashStage {
    Ready,
    Cmd1,
    Cmd2,
    EraseCmd,
    EraseCmd1,
    EraseCmd2,
    WriteByte,
    BankSwitch,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashChipId {
    pub manufacturer: u8,
    pub device: u8,
}

impl FlashChipId {
    pub const PANASONIC_64K: Self = Self {
        manufacturer: 0x32,
        device: 0x1B,
    };
    pub const SANYO_128K: Self = Self {
        manufacturer: 0x62,
        device: 0x13,
    };
    pub const SST_64K: Self = Self {
        manufacturer: 0xBF,
        device: 0xD4,
    };
    pub const MACRONIX_64K: Self = Self {
        manufacturer: 0xC2,
        device: 0x1C,
    };
    pub const MACRONIX_128K: Self = Self {
        manufacturer: 0xC2,
        device: 0x09,
    };
    pub const ATMEL_64K: Self = Self {
        manufacturer: 0x1F,
        device: 0x3D,
    };

    pub const fn new(manufacturer: u8, device: u8) -> Self {
        Self {
            manufacturer,
            device,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FlashState {
    pub stage: FlashStage,
    pub id_mode: bool,
    pub bank: u8,
    pub is_128k: bool,
    #[serde(default)]
    pub id_override: Option<FlashChipId>,
}

impl FlashState {
    pub fn new(is_128k: bool) -> Self {
        Self {
            stage: FlashStage::Ready,
            id_mode: false,
            bank: 0,
            is_128k,
            id_override: None,
        }
    }

    #[inline]
    pub fn chip_id(&self) -> FlashChipId {
        if let Some(id) = self.id_override {
            id
        } else if self.is_128k {
            FlashChipId::SANYO_128K
        } else {
            FlashChipId::PANASONIC_64K
        }
    }

    #[inline]
    pub fn set_id_override(&mut self, id: Option<FlashChipId>) {
        self.id_override = id;
    }

    pub fn read(&self, sram: &[u8; 0x20000], addr_16: u16) -> u8 {
        if self.id_mode {
            let id = self.chip_id();
            return match addr_16 {
                0x0000 => id.manufacturer,
                0x0001 => id.device,
                _ => 0xFF,
            };
        }
        let bank_base = if self.is_128k {
            (self.bank as usize & 1) * 0x10000
        } else {
            0
        };
        sram[bank_base + (addr_16 as usize)]
    }

    pub fn write(&mut self, sram: &mut [u8; 0x20000], dirty: &mut bool, addr_16: u16, val: u8) {
        match self.stage {
            FlashStage::WriteByte => {
                let bank_base = if self.is_128k {
                    (self.bank as usize & 1) * 0x10000
                } else {
                    0
                };
                // NOR Flash can only clear bits (1 -> 0); erase sets back to 0xFF.
                sram[bank_base + (addr_16 as usize)] &= val;
                *dirty = true;
                self.stage = FlashStage::Ready;
            }
            FlashStage::BankSwitch => {
                if self.is_128k && addr_16 == 0x0000 {
                    self.bank = val & 1;
                }
                self.stage = FlashStage::Ready;
            }
            FlashStage::Ready => {
                if addr_16 == 0x5555 && val == 0xAA {
                    self.stage = FlashStage::Cmd1;
                } else if val == 0xF0 {
                    self.id_mode = false;
                }
            }
            FlashStage::Cmd1 => {
                if addr_16 == 0x2AAA && val == 0x55 {
                    self.stage = FlashStage::Cmd2;
                } else {
                    self.stage = FlashStage::Ready;
                }
            }
            FlashStage::Cmd2 => {
                if addr_16 == 0x5555 {
                    match val {
                        0x90 => {
                            self.id_mode = true;
                            self.stage = FlashStage::Ready;
                        }
                        0xF0 => {
                            self.id_mode = false;
                            self.stage = FlashStage::Ready;
                        }
                        0x80 => {
                            self.stage = FlashStage::EraseCmd;
                        }
                        0xA0 => {
                            self.stage = FlashStage::WriteByte;
                        }
                        0xB0 if self.is_128k => {
                            self.stage = FlashStage::BankSwitch;
                        }
                        _ => self.stage = FlashStage::Ready,
                    }
                } else {
                    self.stage = FlashStage::Ready;
                }
            }
            FlashStage::EraseCmd => {
                if addr_16 == 0x5555 && val == 0xAA {
                    self.stage = FlashStage::EraseCmd1;
                } else {
                    self.stage = FlashStage::Ready;
                }
            }
            FlashStage::EraseCmd1 => {
                if addr_16 == 0x2AAA && val == 0x55 {
                    self.stage = FlashStage::EraseCmd2;
                } else {
                    self.stage = FlashStage::Ready;
                }
            }
            FlashStage::EraseCmd2 => {
                if addr_16 == 0x5555 && val == 0x10 {
                    // Chip erase
                    let len = if self.is_128k { 0x20000 } else { 0x10000 };
                    sram[..len].fill(0xFF);
                    *dirty = true;
                } else if val == 0x30 {
                    // 4KB sector erase
                    let bank_base = if self.is_128k {
                        (self.bank as usize & 1) * 0x10000
                    } else {
                        0
                    };
                    let sector = (addr_16 as usize) & 0xF000;
                    sram[bank_base + sector..bank_base + sector + 0x1000].fill(0xFF);
                    *dirty = true;
                }
                self.stage = FlashStage::Ready;
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EepromStage {
    Idle,
    Command,
    ReadDummy,
    ReadData,
    WriteData,
    WriteDummyZero,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct EepromState {
    pub stage: EepromStage,
    pub detected_8k: bool,
    pub addr: usize,
    pub bit_idx: usize,
    pub cmd_bits: u64,
    pub cmd_len: usize,
    pub read_word: u64,
    pub write_word: u64,
}

impl EepromState {
    pub fn new(detected_8k: bool) -> Self {
        Self {
            stage: EepromStage::Idle,
            detected_8k,
            addr: 0,
            bit_idx: 0,
            cmd_bits: 0,
            cmd_len: 0,
            read_word: 0,
            write_word: 0,
        }
    }

    pub fn reset(&mut self) {
        self.stage = EepromStage::Idle;
        self.cmd_bits = 0;
        self.cmd_len = 0;
        self.bit_idx = 0;
    }

    pub fn read_bit(&mut self) -> u16 {
        match self.stage {
            EepromStage::Idle => 1, // Ready
            EepromStage::ReadDummy => {
                self.bit_idx += 1;
                if self.bit_idx >= 4 {
                    self.stage = EepromStage::ReadData;
                    self.bit_idx = 0;
                }
                0
            }
            EepromStage::ReadData => {
                let bit = ((self.read_word >> (63 - self.bit_idx)) & 1) as u16;
                self.bit_idx += 1;
                if self.bit_idx >= 64 {
                    self.stage = EepromStage::Idle;
                    self.bit_idx = 0;
                }
                bit
            }
            _ => 1,
        }
    }

    pub fn write_bit(
        &mut self,
        sram: &mut [u8; 0x20000],
        dirty: &mut bool,
        val: u16,
        dma_count: u32,
    ) {
        let bit = (val & 1) as u64;
        match self.stage {
            EepromStage::Idle => {
                self.cmd_bits = bit;
                self.cmd_len = 1;
                self.stage = EepromStage::Command;
                if dma_count == 17 || dma_count == 81 {
                    self.detected_8k = true;
                } else if dma_count == 9 || dma_count == 73 {
                    self.detected_8k = false;
                }
            }
            EepromStage::Command => {
                self.cmd_bits = (self.cmd_bits << 1) | bit;
                self.cmd_len += 1;
                let addr_bits = if self.detected_8k { 14 } else { 6 };
                if self.cmd_len == 2 {
                    // Check opcode top 2 bits
                }
                let is_read = ((self.cmd_bits >> (self.cmd_len - 2)) & 0b11) == 0b11;
                let is_write = ((self.cmd_bits >> (self.cmd_len - 2)) & 0b11) == 0b10;

                if is_read && self.cmd_len == 2 + addr_bits + 1 {
                    let raw_addr = ((self.cmd_bits >> 1) & ((1 << addr_bits) - 1)) as usize;
                    let max_blocks = if self.detected_8k { 1024 } else { 64 };
                    self.addr = (raw_addr % max_blocks) * 8;
                    let mut w = 0u64;
                    for i in 0..8 {
                        w = (w << 8) | (sram[self.addr + i] as u64);
                    }
                    self.read_word = w;
                    self.bit_idx = 0;
                    self.stage = EepromStage::ReadDummy;
                } else if is_write && self.cmd_len == 2 + addr_bits {
                    let raw_addr = (self.cmd_bits & ((1 << addr_bits) - 1)) as usize;
                    let max_blocks = if self.detected_8k { 1024 } else { 64 };
                    self.addr = (raw_addr % max_blocks) * 8;
                    self.write_word = 0;
                    self.bit_idx = 0;
                    self.stage = EepromStage::WriteData;
                } else if self.cmd_len > 2 + addr_bits + 1 {
                    self.stage = EepromStage::Idle;
                }
            }
            EepromStage::WriteData => {
                self.write_word = (self.write_word << 1) | bit;
                self.bit_idx += 1;
                if self.bit_idx == 64 {
                    self.stage = EepromStage::WriteDummyZero;
                }
            }
            EepromStage::WriteDummyZero => {
                for i in 0..8 {
                    sram[self.addr + i] = ((self.write_word >> (56 - i * 8)) & 0xFF) as u8;
                }
                *dirty = true;
                self.stage = EepromStage::Idle;
            }
            _ => {
                self.stage = EepromStage::Idle;
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RtcStage {
    Idle,
    Command,
    TransferOut,
    TransferIn,
}

/// Minimal Seiko S-3511A RTC implementation via GPIO pins on 0x0800_00C4..C8.
/// Bit 0 = SCK, Bit 1 = SIO, Bit 2 = CS.
#[derive(Clone, Serialize, Deserialize)]
pub struct GpioRtc {
    pub data: u16,
    pub dir: u16,
    pub control: u16,
    pub stage: RtcStage,
    pub prev_sck: bool,
    pub prev_cs: bool,
    pub shift_reg: u8,
    pub bit_idx: u8,
    pub cmd: u8,
    pub status: u8,
    pub regs: [u8; 7],
    pub reg_len: usize,
    pub reg_pos: usize,
}

impl GpioRtc {
    pub fn new() -> Self {
        Self {
            data: 0,
            dir: 0,
            control: 0,
            stage: RtcStage::Idle,
            prev_sck: false,
            prev_cs: false,
            shift_reg: 0,
            bit_idx: 0,
            cmd: 0,
            status: 0x40, // 24-hour mode enabled
            // Year(26), Month(11), Day(11), DoW(3), Hour(12), Min(0), Sec(0) in BCD
            regs: [0x26, 0x11, 0x11, 0x03, 0x12, 0x00, 0x00],
            reg_len: 0,
            reg_pos: 0,
        }
    }

    #[inline]
    pub fn is_readable(&self) -> bool {
        (self.control & 1) != 0
    }

    pub fn read(&self, addr: u32) -> Option<u16> {
        if !self.is_readable() {
            return None;
        }
        match addr & 0xFFFF_FFFE {
            0x0800_00C4 => Some(self.data),
            0x0800_00C6 => Some(self.dir),
            0x0800_00C8 => Some(self.control),
            _ => None,
        }
    }

    pub fn write(&mut self, addr: u32, val: u16) -> bool {
        match addr & 0xFFFF_FFFE {
            0x0800_00C4 => {
                // Only bits configured as output in `dir` are driven by GBA,
                // preserve input bits (like SIO when dir bit 1 == 0).
                let out_mask = self.dir & 0x0F;
                self.data = (self.data & !out_mask) | (val & out_mask);
                self.step_rtc();
                true
            }
            0x0800_00C6 => {
                self.dir = val & 0x0F;
                true
            }
            0x0800_00C8 => {
                self.control = val & 1;
                true
            }
            _ => false,
        }
    }

    fn step_rtc(&mut self) {
        let sck = (self.data & 1) != 0;
        let sio_in = ((self.data >> 1) & 1) as u8;
        let cs = (self.data & 4) != 0;

        if !cs {
            self.stage = RtcStage::Idle;
            self.bit_idx = 0;
            self.shift_reg = 0;
            self.prev_sck = sck;
            self.prev_cs = false;
            return;
        }

        if !self.prev_cs && cs {
            self.stage = RtcStage::Command;
            self.bit_idx = 0;
            self.shift_reg = 0;
        }

        // Rising edge of SCK samples SIO (input)
        let rising = !self.prev_sck && sck;
        // Falling edge shifts out SIO (output)
        let falling = self.prev_sck && !sck;

        match self.stage {
            RtcStage::Command if rising => {
                self.shift_reg = (self.shift_reg << 1) | sio_in;
                self.bit_idx += 1;
                if self.bit_idx == 8 {
                    // Check fixed prefix 0b0110 (0x6)
                    let byte = self.shift_reg;
                    self.cmd = (byte >> 1) & 0x07;
                    let is_read = (byte & 1) != 0;
                    self.bit_idx = 0;
                    self.shift_reg = 0;
                    self.reg_pos = 0;

                    if (byte >> 4) != 0x06 {
                        self.stage = RtcStage::Idle;
                    } else {
                        self.reg_len = match self.cmd {
                            0 | 1 | 3 => 1, // Reset / Control
                            2 => 7,         // DateTime
                            6 => 3,         // Time
                            _ => 0,
                        };
                        if self.cmd == 0 {
                            self.status = 0x40;
                            self.stage = RtcStage::Idle;
                        } else if self.reg_len == 0 {
                            self.stage = RtcStage::Idle;
                        } else if is_read {
                            self.stage = RtcStage::TransferOut;
                            self.load_out_byte();
                        } else {
                            self.stage = RtcStage::TransferIn;
                        }
                    }
                }
            }
            RtcStage::TransferIn if rising => {
                self.shift_reg |= sio_in << self.bit_idx;
                self.bit_idx += 1;
                if self.bit_idx == 8 {
                    let b = self.shift_reg;
                    if self.cmd == 1 || self.cmd == 3 {
                        self.status = b & 0x6A;
                    } else if self.cmd == 2 && self.reg_pos < 7 {
                        self.regs[self.reg_pos] = b;
                    } else if self.cmd == 6 && self.reg_pos < 3 {
                        self.regs[4 + self.reg_pos] = b;
                    }
                    self.reg_pos += 1;
                    self.bit_idx = 0;
                    self.shift_reg = 0;
                    if self.reg_pos >= self.reg_len {
                        self.stage = RtcStage::Idle;
                    }
                }
            }
            RtcStage::TransferOut if falling => {
                let out_bit = ((self.shift_reg >> self.bit_idx) & 1) as u16;
                if (self.dir & 2) == 0 {
                    self.data = (self.data & !2) | (out_bit << 1);
                }
                self.bit_idx += 1;
                if self.bit_idx == 8 {
                    self.bit_idx = 0;
                    self.reg_pos += 1;
                    if self.reg_pos >= self.reg_len {
                        self.stage = RtcStage::Idle;
                    } else {
                        self.load_out_byte();
                    }
                }
            }
            _ => {}
        }

        self.prev_sck = sck;
        self.prev_cs = cs;
    }

    fn load_out_byte(&mut self) {
        self.shift_reg = match self.cmd {
            1 | 3 => self.status,
            2 => self.regs[self.reg_pos.min(6)],
            6 => self.regs[(4 + self.reg_pos).min(6)],
            _ => 0,
        };
    }

    pub fn advance_second(&mut self) {
        fn bcd_inc(v: u8, limit: u8) -> (u8, bool) {
            let mut val = (v >> 4) * 10 + (v & 0xF) + 1;
            let carry = val >= limit;
            if carry {
                val = 0;
            }
            (((val / 10) << 4) | (val % 10), carry)
        }
        let (sec, c1) = bcd_inc(self.regs[6] & 0x7F, 60);
        self.regs[6] = sec;
        if c1 {
            let (min, c2) = bcd_inc(self.regs[5] & 0x7F, 60);
            self.regs[5] = min;
            if c2 {
                let (hr, c3) = bcd_inc(self.regs[4] & 0x3F, 24);
                self.regs[4] = hr;
                if c3 {
                    self.regs[3] = (self.regs[3] + 1) % 7;
                    let (day, c4) = bcd_inc(self.regs[2] & 0x3F, 32);
                    self.regs[2] = if day == 0 { 1 } else { day };
                    if c4 {
                        let (month, c5) = bcd_inc(self.regs[1] & 0x1F, 13);
                        self.regs[1] = if month == 0 { 1 } else { month };
                        if c5 {
                            let (year, _) = bcd_inc(self.regs[0], 100);
                            self.regs[0] = year;
                        }
                    }
                }
            }
        }
    }

    pub fn set_datetime(
        &mut self,
        year: u8,
        month: u8,
        day: u8,
        dow: u8,
        hour: u8,
        min: u8,
        sec: u8,
    ) {
        fn to_bcd(v: u8) -> u8 {
            ((v / 10) << 4) | (v % 10)
        }
        self.regs[0] = to_bcd(year % 100);
        self.regs[1] = to_bcd(month.clamp(1, 12));
        self.regs[2] = to_bcd(day.clamp(1, 31));
        self.regs[3] = dow % 7;
        self.regs[4] = to_bcd(hour % 24);
        self.regs[5] = to_bcd(min % 60);
        self.regs[6] = to_bcd(sec % 60);
    }
}

#[inline]
fn default_solar_lux() -> u8 {
    128
}

#[inline]
fn default_tilt_center() -> u16 {
    0x03A0
}

#[inline]
fn default_gyro_center() -> u16 {
    0x06C0
}

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

#[inline]
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc as u8) ^ b) as usize];
    }
    !crc
}

fn alloc_sram() -> Box<[u8; 0x20000]> {
    alloc_boxed_array(0xFFu8)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cartridge {
    #[serde(skip)]
    pub rom: Vec<u8>,
    pub rom_size_mask: usize,
    #[serde(default)]
    pub rom_crc32: u32,
    #[serde(default)]
    pub idle_loop_addr: Option<u32>,
    #[serde(skip, default = "alloc_sram")]
    pub sram: Box<[u8; 0x20000]>,
    pub save_type: SaveType,
    pub mapper: MapperType,
    pub flash: FlashState,
    pub eeprom: EepromState,
    pub rtc: GpioRtc,
    pub has_rtc: bool,
    pub save_dirty: bool,

    // Module 6 Overrides & Special Cartridge Hardware Sensors
    #[serde(default)]
    pub save_type_override: Option<SaveType>,
    #[serde(default)]
    pub rtc_override: Option<bool>,
    #[serde(default = "default_solar_lux")]
    pub solar_lux: u8,
    #[serde(default)]
    pub solar_counter: u8,
    #[serde(default)]
    pub solar_prev_clk: bool,
    #[serde(default)]
    pub solar_enabled: bool,
    #[serde(default)]
    pub tilt_enabled: bool,
    #[serde(default = "default_tilt_center")]
    pub tilt_x: u16,
    #[serde(default = "default_tilt_center")]
    pub tilt_y: u16,
    #[serde(default)]
    pub tilt_latched: bool,
    #[serde(default)]
    pub gyro_enabled: bool,
    #[serde(default = "default_gyro_center")]
    pub gyro_val: u16,
    #[serde(default)]
    pub gyro_shift: u16,
    #[serde(default)]
    pub gyro_prev_clk: bool,
    #[serde(default)]
    pub rumble_enabled: bool,
    #[serde(default)]
    pub rumble_active: bool,
}

impl Cartridge {
    pub fn new(mut rom: Vec<u8>) -> Self {
        let orig_len = rom.len();
        let mapper = Self::detect_mapper(&rom);
        let db_entry = if orig_len >= 0xB0 {
            crate::gamedb::lookup_bytes(&rom[0xAC..0xB0])
        } else {
            None
        };
        let sig_save_type = Self::detect_save_type(&rom);
        let (
            save_type,
            has_rtc,
            solar_enabled,
            tilt_enabled,
            gyro_enabled,
            rumble_enabled,
            idle_loop_addr,
        ) = if let Some(entry) = db_entry {
            (
                if sig_save_type != SaveType::None {
                    sig_save_type
                } else {
                    entry.save_type
                },
                entry.has_rtc,
                entry.solar,
                entry.tilt,
                entry.gyro,
                entry.rumble,
                entry.idle_loop_addr,
            )
        } else {
            let code3 = if orig_len >= 0xB0 {
                &rom[0xAC..0xAF]
            } else {
                b"---"
            };
            (
                sig_save_type,
                Self::detect_rtc(&rom),
                matches!(code3, b"U3I" | b"U32" | b"U33"),
                code3 == b"KYG",
                code3 == b"RZW",
                code3 == b"RZW" || code3 == b"V49",
                None,
            )
        };

        // Pad non-power-of-two small test ROMs cleanly up to >= 16KB or next power/multiple if tiny,
        // preserving exact ROM bounds behavior for open-bus / mirrored reads.
        if rom.is_empty() {
            rom.resize(4, 0);
        }
        let rom_crc32 = crc32_ieee(&rom);

        let is_128k = save_type == SaveType::Flash128K;
        let is_eeprom_8k = save_type == SaveType::Eeprom8K;

        Self {
            rom_size_mask: orig_len,
            rom_crc32,
            idle_loop_addr,
            rom,
            sram: alloc_boxed_array(0xFFu8),
            save_type,
            mapper,
            flash: FlashState::new(is_128k),
            eeprom: EepromState::new(is_eeprom_8k),
            rtc: GpioRtc::new(),
            has_rtc,
            save_dirty: false,
            save_type_override: None,
            rtc_override: None,
            solar_lux: 128,
            solar_counter: 0,
            solar_prev_clk: false,
            solar_enabled,
            tilt_enabled,
            tilt_x: 0x03A0,
            tilt_y: 0x03A0,
            tilt_latched: false,
            gyro_enabled,
            gyro_val: 0x06C0,
            gyro_shift: 0,
            gyro_prev_clk: false,
            rumble_enabled,
            rumble_active: false,
        }
    }

    pub fn game_title(&self) -> String {
        if self.rom.len() < 0xAC {
            return String::from("Homebrew / Unheadered");
        }
        let raw = &self.rom[0xA0..0xAC];
        let mut s = String::new();
        for &b in raw {
            if b == 0 {
                break;
            }
            if (0x20..0x7F).contains(&b) {
                s.push(b as char);
            }
        }
        let trimmed = s.trim();
        if trimmed.is_empty() {
            String::from("Untitled ROM")
        } else {
            String::from(trimmed)
        }
    }

    pub fn game_code(&self) -> String {
        if self.rom.len() < 0xB0 {
            return String::from("----");
        }
        let raw = &self.rom[0xAC..0xB0];
        let mut s = String::new();
        for &b in raw {
            if (0x20..0x7F).contains(&b) {
                s.push(b as char);
            } else {
                s.push('-');
            }
        }
        s
    }

    #[inline]
    pub fn effective_save_type(&self) -> SaveType {
        if let Some(ov) = self.save_type_override {
            return ov;
        }
        if matches!(self.save_type, SaveType::Eeprom512B | SaveType::Eeprom8K) {
            if self.eeprom.detected_8k {
                SaveType::Eeprom8K
            } else {
                SaveType::Eeprom512B
            }
        } else {
            self.save_type
        }
    }

    #[inline]
    pub fn effective_rtc(&self) -> bool {
        self.rtc_override.unwrap_or(self.has_rtc)
    }

    pub fn set_save_type_override(&mut self, ov: Option<SaveType>) {
        self.save_type_override = ov;
        let eff = self.effective_save_type();
        self.flash.is_128k = eff == SaveType::Flash128K;
        self.eeprom.detected_8k = eff == SaveType::Eeprom8K;
    }

    pub fn set_flash_id_override(&mut self, id: Option<FlashChipId>) {
        self.flash.set_id_override(id);
    }

    pub fn load_save_data(&mut self, data: &[u8]) {
        let len = data.len().min(self.sram.len());
        self.sram[..len].copy_from_slice(&data[..len]);
        self.save_dirty = false;
    }

    pub fn export_save_data(&self) -> &[u8] {
        match self.effective_save_type() {
            SaveType::None => &[],
            SaveType::Eeprom512B => &self.sram[..512],
            SaveType::Eeprom8K => &self.sram[..0x2000],
            SaveType::Sram32K => &self.sram[..0x8000],
            SaveType::Flash64K => &self.sram[..0x10000],
            SaveType::Flash128K => &self.sram[..0x20000],
        }
    }

    fn detect_mapper(rom: &[u8]) -> MapperType {
        if rom.len() > 0x0200_0000 {
            MapperType::GbaVideo64M
        } else {
            MapperType::Standard
        }
    }

    fn detect_rtc(rom: &[u8]) -> bool {
        if rom.len() < 0xB0 {
            return false;
        }
        let code = &rom[0xAC..0xB0];
        // Pokémon Ruby/Sapphire/Emerald, Boktai, WarioWare Twisted, etc.
        matches!(
            code,
            b"AXVE"
                | b"AXPE"
                | b"AXVJ"
                | b"AXPJ"
                | b"BPEE"
                | b"BPEJ"
                | b"BPED"
                | b"BPEF"
                | b"BPEI"
                | b"BPES"
                | b"U3IE"
                | b"U3IJ"
                | b"U32E"
                | b"U32J"
                | b"U33J"
        )
    }

    pub fn detect_save_type(rom: &[u8]) -> SaveType {
        let contains = |pat: &[u8]| rom.windows(pat.len()).any(|w| w == pat);
        if contains(b"FLASH1M_V") {
            SaveType::Flash128K
        } else if contains(b"FLASH512_V") || contains(b"FLASH_V") {
            SaveType::Flash64K
        } else if contains(b"EEPROM_V") {
            SaveType::Eeprom8K
        } else if contains(b"SRAM_V") || contains(b"SRAM_F_V") {
            SaveType::Sram32K
        } else {
            SaveType::None
        }
    }

    #[inline]
    pub fn is_eeprom_addr(&self, addr: u32) -> bool {
        let region = (addr >> 24) & 0x0F;
        if region != 0x0D {
            return false;
        }
        matches!(
            self.effective_save_type(),
            SaveType::Eeprom512B | SaveType::Eeprom8K
        ) || (self.save_type_override.is_none()
            && self.rom.len() <= 0x0100_0000
            && addr >= 0x0D00_0000)
            || (self.save_type_override.is_none()
                && self.rom.len() > 0x0100_0000
                && addr >= 0x0DFF_FF00)
    }

    #[inline]
    pub fn read_rom_16(&self, addr: u32) -> u16 {
        if (0x0800_00C4..=0x0800_00C8).contains(&(addr & 0xFFFF_FFFE)) {
            let gpio_allowed = !matches!(self.rtc_override, Some(false))
                || self.solar_enabled
                || self.gyro_enabled
                || self.rumble_enabled;
            if gpio_allowed && let Some(gpio_val) = self.rtc.read(addr) {
                return gpio_val;
            }
        }
        let offset = (addr as usize) & 0x01FF_FFFE;
        if offset + 1 < self.rom.len() {
            u16::from_le_bytes([self.rom[offset], self.rom[offset + 1]])
        } else if offset < self.rom.len() {
            u16::from(self.rom[offset])
        } else {
            // Out-of-bounds ROM returns lower 16 bits of word address >> 1 (GBA hardware open-bus pattern).
            ((addr >> 1) & 0xFFFF) as u16
        }
    }

    #[inline]
    pub fn read_rom_32(&self, addr: u32) -> u32 {
        let lo = self.read_rom_16(addr & !3) as u32;
        let hi = self.read_rom_16((addr & !3) + 2) as u32;
        lo | (hi << 16)
    }

    pub fn write_rom_16(&mut self, addr: u32, val: u16) {
        let aligned = addr & 0xFFFF_FFFE;
        if (0x0800_00C4..=0x0800_00C8).contains(&aligned) {
            let allow_gpio = !matches!(self.rtc_override, Some(false))
                || self.solar_enabled
                || self.gyro_enabled
                || self.rumble_enabled;
            if allow_gpio && self.rtc.write(addr, val) && aligned == 0x0800_00C4 {
                if self.solar_enabled && (self.rtc.data & 0x04) == 0 {
                    let rst = (self.rtc.data & 0x02) != 0;
                    let clk = (self.rtc.data & 0x01) != 0;
                    if rst {
                        self.solar_counter = 0;
                        self.rtc.data &= !0x08;
                    } else if !self.solar_prev_clk && clk {
                        self.solar_counter = self.solar_counter.saturating_add(1);
                        let threshold = 255u8.saturating_sub(self.solar_lux).max(1);
                        if self.solar_counter >= threshold {
                            self.rtc.data |= 0x08;
                        } else {
                            self.rtc.data &= !0x08;
                        }
                    }
                    self.solar_prev_clk = clk;
                }
                if self.rumble_enabled && (self.rtc.dir & 0x08) != 0 {
                    self.rumble_active = (self.rtc.data & 0x08) != 0;
                }
                if self.gyro_enabled {
                    let sample_start = (self.rtc.data & 0x02) != 0;
                    let clk = (self.rtc.data & 0x01) != 0;
                    if sample_start {
                        self.gyro_shift = (self.gyro_val & 0x0FFF) << 4;
                    } else if self.gyro_prev_clk && !clk {
                        let bit = (self.gyro_shift >> 15) & 1;
                        if (self.rtc.dir & 0x04) == 0 {
                            self.rtc.data = (self.rtc.data & !0x04) | (bit << 2);
                        }
                        self.gyro_shift <<= 1;
                    }
                    self.gyro_prev_clk = clk;
                }
            }
        }
    }

    pub fn read_sram(&self, addr: u32) -> u8 {
        let offset = (addr as usize) & 0xFFFF;
        if self.tilt_enabled {
            match offset {
                0x8200 => return (self.tilt_x & 0xFF) as u8,
                0x8300 => {
                    return (((self.tilt_x >> 8) & 0x0F) as u8)
                        | if self.tilt_latched { 0x80 } else { 0 };
                }
                0x8400 => return (self.tilt_y & 0xFF) as u8,
                0x8500 => return ((self.tilt_y >> 8) & 0x0F) as u8,
                _ => {}
            }
        }
        match self.effective_save_type() {
            SaveType::Flash64K | SaveType::Flash128K => self.flash.read(&self.sram, offset as u16),
            SaveType::Sram32K | SaveType::None => self.sram[offset & 0x7FFF],
            SaveType::Eeprom512B | SaveType::Eeprom8K => 0xFF,
        }
    }

    pub fn write_sram(&mut self, addr: u32, val: u8) {
        let offset = (addr as usize) & 0xFFFF;
        if self.tilt_enabled {
            match offset {
                0x8000 if val == 0x55 => {
                    self.tilt_latched = false;
                    return;
                }
                0x8100 if val == 0xAA => {
                    self.tilt_latched = true;
                    return;
                }
                _ => {}
            }
        }
        // Fallback detection if test ROM omits Nintendo SDK signature string (unless overridden):
        if self.save_type_override.is_none() && self.save_type == SaveType::None {
            if offset == 0x5555 && val == 0xAA {
                self.save_type = SaveType::Flash64K;
            } else {
                self.save_type = SaveType::Sram32K;
            }
        }
        if self.save_type_override == Some(SaveType::None) {
            return;
        }
        match self.effective_save_type() {
            SaveType::Flash64K | SaveType::Flash128K => {
                self.flash
                    .write(&mut self.sram, &mut self.save_dirty, offset as u16, val);
            }
            SaveType::Sram32K => {
                self.sram[offset & 0x7FFF] = val;
                self.save_dirty = true;
            }
            _ => {}
        }
    }

    /// Side-effect-free SRAM/Flash backing-store write for debug `poke*` APIs.
    pub fn debug_write_sram(&mut self, addr: u32, val: u8) {
        let offset = (addr as usize) & 0xFFFF;
        match self.effective_save_type() {
            SaveType::Flash64K => {
                self.sram[offset] = val;
            }
            SaveType::Flash128K => {
                let bank_base = (self.flash.bank as usize & 1) * 0x10000;
                self.sram[bank_base + offset] = val;
            }
            SaveType::Sram32K | SaveType::None => {
                self.sram[offset & 0x7FFF] = val;
            }
            SaveType::Eeprom512B | SaveType::Eeprom8K => {}
        }
    }
}
