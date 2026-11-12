#![no_std]
#![allow(clippy::too_many_arguments, clippy::new_without_default)]

extern crate alloc;

pub mod apu;
pub mod bus;
pub mod cartridge;
pub mod cpu;
pub mod ppu;
pub mod serde_helpers;
pub mod serial;
pub mod timer;

use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

use crate::bus::GbBus;
use crate::cpu::Cpu;

pub use crate::cartridge::CartridgeHeader;
pub use crate::ppu::DmgPalette;

/// DMG / normal-speed base clock frequency in Hz (~4.194 MHz).
pub const CPU_CLOCK_HZ: u32 = 4_194_304;
/// Number of normal-speed T-cycles (dots) per frame (154 scanlines * 456 dots = 59.7275 Hz).
pub const CYCLES_PER_FRAME: u64 = 70_224;
/// LCD horizontal resolution in pixels.
pub const LCD_WIDTH: usize = 160;
/// LCD vertical resolution in pixels.
pub const LCD_HEIGHT: usize = 144;
/// Default stereo audio output sample rate in Hz.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 48_000;

// Active-high masks matching gba-core's `KEYINPUT` layout for frontend reuse.
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

/// Encodes an active-high button mask into GBA's active-low 10-bit `KEYINPUT` format.
#[inline]
pub const fn encode_keyinput(pressed_mask: u16) -> u16 {
    (!pressed_mask) & 0x03FF
}

/// Host frontend callback interface mirroring `gba_core::HostFrontend`.
pub trait GbHostFrontend {
    /// Called when a 160x144 frame in BGR555 (`0..=0x7FFF`) format is ready to display.
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT]);

    /// Called when the APU has stereo interleaved signed 16-bit PCM samples ready.
    fn push_audio_samples(&mut self, samples_interleaved_i16: &[i16]);

    /// Returns active-low 10-bit GBA `KEYINPUT` register state (`0x03FF` = no buttons pressed).
    fn poll_keypad(&mut self) -> u16;
}

/// Hardware model selection.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GbModel {
    /// Detect from cartridge header (`0x0143` CGB flag `0x80` / `0xC0` -> CGB, otherwise DMG).
    #[default]
    Auto,
    /// Force original Game Boy (DMG) hardware mode.
    Dmg,
    /// Force Game Boy Color (CGB) hardware mode.
    Cgb,
}

/// Configuration options for [`GbCore`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GbConfig {
    pub model: GbModel,
    /// When running in CGB mode without a boot ROM, set `B` bit 0 = 1 so games detect GBA hardware.
    pub gba_mode: bool,
    /// Target stereo PCM sample rate in Hz (default `48_000`).
    pub sample_rate_hz: u32,
    /// 4-shade BGR555 palette used for DMG rendering (default `DmgPalette::Greyscale`).
    pub dmg_palette: DmgPalette,
}

impl Default for GbConfig {
    fn default() -> Self {
        Self {
            model: GbModel::Auto,
            gba_mode: true,
            sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
            dmg_palette: DmgPalette::Greyscale,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SaveStatePayload {
    cpu: Cpu,
    bus: GbBus,
    config: GbConfig,
}

#[derive(Serialize)]
struct SaveStatePayloadRef<'a> {
    cpu: &'a Cpu,
    bus: &'a GbBus,
    config: GbConfig,
}

/// Game Boy / Game Boy Color emulation core (`#![no_std]` + `alloc`).
pub struct GbCore {
    pub cpu: Cpu,
    pub bus: GbBus,
    pub config: GbConfig,
}

impl GbCore {
    /// Creates a new emulator core with default configuration (`gba_mode: true`, 48 kHz audio,
    /// greyscale DMG palette).
    pub fn new(rom: Vec<u8>, boot_rom: Option<&[u8]>, model: GbModel) -> Self {
        Self::new_with_config(
            rom,
            boot_rom,
            GbConfig {
                model,
                ..GbConfig::default()
            },
        )
    }

    /// Creates a new emulator core with explicit [`GbConfig`].
    pub fn new_with_config(rom: Vec<u8>, boot_rom: Option<&[u8]>, config: GbConfig) -> Self {
        let header = CartridgeHeader::parse(&rom);
        let is_cgb = match config.model {
            GbModel::Auto => header.supports_cgb,
            GbModel::Dmg => false,
            GbModel::Cgb => true,
        };
        let has_boot = boot_rom.map(|b| !b.is_empty()).unwrap_or(false);
        let cpu = Cpu::new(has_boot, is_cgb, config.gba_mode, header.header_checksum);
        let bus = GbBus::new(
            rom,
            boot_rom,
            is_cgb,
            config.gba_mode,
            config.sample_rate_hz,
            config.dmg_palette,
        );
        Self { cpu, bus, config }
    }

    /// Resets the emulated system while preserving ROM, optional boot ROM, battery RAM/RTC,
    /// and configuration.
    pub fn reset(&mut self) {
        let rom = core::mem::take(&mut self.bus.cart.rom);
        let saved_ram = self.bus.cart.ram.clone();
        let saved_rtc = self.bus.cart.rtc.clone();
        let saved_dirty = self.bus.cart.battery_dirty;
        let boot_rom = if self.bus.boot_rom.is_empty() {
            None
        } else {
            Some(self.bus.boot_rom.clone())
        };

        *self = Self::new_with_config(rom, boot_rom.as_deref(), self.config);
        self.bus.cart.ram = saved_ram;
        self.bus.cart.rtc = saved_rtc;
        self.bus.cart.battery_dirty = saved_dirty;
    }

    /// Steps one CPU instruction (or one M-cycle if halted/stopped/stalled) and returns
    /// the number of normal-speed T-cycles (dots) elapsed.
    #[inline]
    pub fn step_one(&mut self) -> u64 {
        let start = self.bus.total_normal_dots;
        self.cpu.step(&mut self.bus);
        self.bus.total_normal_dots - start
    }

    /// Steps until the PPU advances to the next scanline.
    pub fn step_scanline(&mut self) {
        let start_ly = self.bus.ppu.ly;
        let limit = self.bus.total_normal_dots + 912;
        while self.bus.ppu.ly == start_ly && self.bus.total_normal_dots < limit {
            self.step_one();
        }
    }

    /// Runs one frame (`70_224` normal-speed T-cycles, ~59.7275 Hz) and delivers video/audio
    /// callbacks to `frontend`.
    pub fn run_frame<F: GbHostFrontend + ?Sized>(&mut self, frontend: &mut F) {
        let _ = self.run_frame_with_breakpoints(frontend, &[]);
    }

    /// Runs one frame, stopping early if `PC` matches any address in `breakpoints`.
    pub fn run_frame_with_breakpoints<F: GbHostFrontend + ?Sized>(
        &mut self,
        frontend: &mut F,
        breakpoints: &[u16],
    ) -> Option<u16> {
        let keypad = frontend.poll_keypad() & 0x03FF;
        self.bus.update_keypad(keypad);

        let target_dots = self.bus.total_normal_dots + CYCLES_PER_FRAME;
        let mut presented = false;

        while self.bus.total_normal_dots < target_dots {
            if !breakpoints.is_empty()
                && !self.cpu.halted
                && !self.cpu.stopped
                && breakpoints.contains(&self.cpu.pc)
            {
                if !self.bus.apu.output_buffer.is_empty() {
                    frontend.push_audio_samples(&self.bus.apu.output_buffer);
                    self.bus.apu.output_buffer.clear();
                }
                return Some(self.cpu.pc);
            }

            self.cpu.step(&mut self.bus);

            if self.bus.ppu.frame_ready {
                frontend.present_frame(&self.bus.ppu.framebuffer);
                self.bus.ppu.frame_ready = false;
                presented = true;
            }
        }

        if !presented {
            frontend.present_frame(&self.bus.ppu.framebuffer);
            self.bus.ppu.frame_ready = false;
        }

        if !self.bus.apu.output_buffer.is_empty() {
            frontend.push_audio_samples(&self.bus.apu.output_buffer);
            self.bus.apu.output_buffer.clear();
        }
        None
    }

    /// Serializes full emulator state (excluding the ROM bytes) using `postcard`.
    pub fn save_state(&self) -> Result<Vec<u8>, postcard::Error> {
        let payload = SaveStatePayloadRef {
            cpu: &self.cpu,
            bus: &self.bus,
            config: self.config,
        };
        postcard::to_allocvec(&payload)
    }

    /// Restores emulator state from a `postcard` snapshot while retaining the loaded ROM.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), postcard::Error> {
        let mut payload: SaveStatePayload = postcard::from_bytes(bytes)?;
        let rom = core::mem::take(&mut self.bus.cart.rom);
        payload.bus.cart.rom = rom;
        self.cpu = payload.cpu;
        self.bus = payload.bus;
        self.config = payload.config;
        Ok(())
    }

    /// Exports battery-backed save RAM (with 48-byte MBC3 RTC trailer appended when applicable).
    pub fn battery_ram(&self) -> Vec<u8> {
        self.bus.cart.export_battery_ram()
    }

    /// Imports battery-backed save RAM (and optional 48-byte MBC3 RTC trailer).
    pub fn load_battery_ram(&mut self, data: &[u8]) {
        self.bus.cart.import_battery_ram(data);
    }

    #[inline]
    pub fn has_battery(&self) -> bool {
        self.bus.cart.has_battery
    }

    #[inline]
    pub fn battery_dirty(&self) -> bool {
        self.bus.cart.battery_dirty
    }

    #[inline]
    pub fn clear_battery_dirty(&mut self) {
        self.bus.cart.battery_dirty = false;
    }

    /// Returns the cartridge title parsed from the ROM header.
    #[inline]
    pub fn title(&self) -> &str {
        &self.bus.cart.header.title
    }

    /// Returns `true` when executing in Game Boy Color (CGB) mode.
    #[inline]
    pub fn is_cgb(&self) -> bool {
        self.bus.is_cgb
    }

    /// Returns parsed cartridge header metadata.
    #[inline]
    pub fn header(&self) -> &CartridgeHeader {
        &self.bus.cart.header
    }

    /// Side-effect-free debug byte read.
    #[inline]
    pub fn peek8(&self, addr: u16) -> u8 {
        self.bus.peek8(addr)
    }

    /// Side-effect-free debug byte write.
    #[inline]
    pub fn poke8(&mut self, addr: u16, val: u8) {
        self.bus.poke8(addr, val);
    }

    /// Returns a reference to the current 160x144 BGR555 framebuffer.
    #[inline]
    pub fn framebuffer(&self) -> &[u16; LCD_WIDTH * LCD_HEIGHT] {
        &self.bus.ppu.framebuffer
    }

    /// Returns all bytes written through the serial port (`SB` / `SC`).
    #[inline]
    pub fn serial_output(&self) -> &[u8] {
        &self.bus.serial.output_buffer
    }

    /// Returns captured serial output bytes decoded lossily as UTF-8.
    pub fn serial_output_str(&self) -> String {
        String::from_utf8_lossy(&self.bus.serial.output_buffer).into_owned()
    }

    pub fn clear_serial_output(&mut self) {
        self.bus.serial.output_buffer.clear();
    }

    #[inline]
    pub fn is_rumble_active(&self) -> bool {
        self.bus.cart.rumble_active
    }

    #[inline]
    pub fn gba_mode(&self) -> bool {
        self.config.gba_mode
    }

    pub fn set_gba_mode(&mut self, gba_mode: bool) {
        self.config.gba_mode = gba_mode;
        self.bus.gba_mode = gba_mode;
    }

    #[inline]
    pub fn dmg_palette(&self) -> DmgPalette {
        self.bus.ppu.dmg_palette
    }

    pub fn set_dmg_palette(&mut self, palette: DmgPalette) {
        self.config.dmg_palette = palette;
        self.bus.ppu.dmg_palette = palette;
    }

    #[inline]
    pub fn sample_rate(&self) -> u32 {
        self.bus.apu.sample_rate_hz
    }

    pub fn set_sample_rate(&mut self, sample_rate_hz: u32) {
        self.config.sample_rate_hz = sample_rate_hz;
        self.bus.apu.set_sample_rate(sample_rate_hz);
    }

    pub fn advance_rtc_seconds(&mut self, seconds: u64) {
        if self.bus.cart.has_rtc() {
            self.bus.cart.rtc.advance_seconds(seconds);
        }
    }
}
