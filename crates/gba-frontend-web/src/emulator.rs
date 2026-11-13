use alloc::string::String;
use alloc::vec::Vec;

use gba_core::cartridge::SaveType;
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH, SAMPLE_RATE_HZ, encode_keyinput};

use crate::color::ColorProfile;

/// Number of RGBA8888 bytes in a single GBA LCD frame (`240 * 160 * 4`).
pub const RGBA_FRAME_BYTES: usize = LCD_WIDTH * LCD_HEIGHT * 4;

/// Maximum ROM size accepted by the web frontend (64 MiB, covering GBA Video carts).
pub const MAX_ROM_BYTES: usize = 64 * 1024 * 1024;

/// Exact size in bytes of an authentic GBA BIOS image (16 KiB).
pub const BIOS_BYTES: usize = 0x4000;

/// Maximum buffered stereo i16 audio samples (~1 second) to bound memory if undrained.
const MAX_AUDIO_BUFFER_SAMPLES: usize = (SAMPLE_RATE_HZ as usize) * 2;

/// Host frontend adapter bridging [`GbaCore`] output to RGBA frame and PCM audio buffers.
pub struct WebFrontend {
    pub rgba_frame: Vec<u8>,
    pub prev_rgba: Vec<u8>,
    pub audio_samples: Vec<i16>,
    pub keyinput: u16,
    color_profile: ColorProfile,
    lcd_ghosting: bool,
}

impl WebFrontend {
    /// Creates a new frontend buffer initialized to black RGBA pixels and no keys pressed.
    pub fn new() -> Self {
        Self {
            rgba_frame: alloc::vec![0u8; RGBA_FRAME_BYTES],
            prev_rgba: alloc::vec![0u8; RGBA_FRAME_BYTES],
            audio_samples: Vec::with_capacity(4096),
            keyinput: 0x03FF,
            color_profile: ColorProfile::Linear,
            lcd_ghosting: false,
        }
    }

    /// Runs a single GBA frame using this frontend.
    pub fn step_frame(&mut self, core: &mut GbaCore) {
        core.run_frame(self);
    }

    /// Updates active keypad state from an active-high 10-bit GBA key mask.
    pub fn set_pressed_keys(&mut self, pressed_mask: u16) {
        self.keyinput = encode_keyinput(pressed_mask);
    }

    /// Returns the active display color profile.
    pub const fn color_profile(&self) -> ColorProfile {
        self.color_profile
    }

    /// Sets the display color correction profile.
    pub fn set_color_profile(&mut self, profile: ColorProfile) {
        self.color_profile = profile;
    }

    /// Returns whether LCD frame persistence blending (~65/35) is enabled.
    pub const fn lcd_ghosting(&self) -> bool {
        self.lcd_ghosting
    }

    /// Enables or disables LCD frame persistence blending.
    pub fn set_lcd_ghosting(&mut self, enabled: bool) {
        self.lcd_ghosting = enabled;
        if !enabled {
            self.prev_rgba.copy_from_slice(&self.rgba_frame);
        }
    }

    /// Clears pending audio samples and resets frame buffers to transparent black.
    pub fn clear_buffers(&mut self) {
        self.rgba_frame.fill(0);
        self.prev_rgba.fill(0);
        self.audio_samples.clear();
    }
}

impl Default for WebFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFrontend for WebFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        let lut = self.color_profile.lut();
        if self.lcd_ghosting {
            for (idx, &px) in fb.iter().enumerate() {
                let off = idx * 4;
                let mapped = lut[(px & 0x7FFF) as usize];
                let r = ((mapped[0] as u16 * 166 + self.prev_rgba[off] as u16 * 90) >> 8) as u8;
                let g = ((mapped[1] as u16 * 166 + self.prev_rgba[off + 1] as u16 * 90) >> 8) as u8;
                let b = ((mapped[2] as u16 * 166 + self.prev_rgba[off + 2] as u16 * 90) >> 8) as u8;
                self.rgba_frame[off] = r;
                self.rgba_frame[off + 1] = g;
                self.rgba_frame[off + 2] = b;
                self.rgba_frame[off + 3] = 255;
            }
            self.prev_rgba.copy_from_slice(&self.rgba_frame);
        } else {
            for (idx, &px) in fb.iter().enumerate() {
                let off = idx * 4;
                let mapped = lut[(px & 0x7FFF) as usize];
                self.rgba_frame[off..off + 4].copy_from_slice(&mapped);
            }
        }
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        // Keep stereo pairs aligned and prevent unbounded growth if caller stalls.
        if self.audio_samples.len() + samples.len() > MAX_AUDIO_BUFFER_SAMPLES {
            let keep = MAX_AUDIO_BUFFER_SAMPLES / 2;
            let drop_count = self.audio_samples.len().saturating_sub(keep) & !1;
            if drop_count > 0 {
                self.audio_samples.drain(..drop_count);
            }
        }
        self.audio_samples.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keyinput
    }
}

/// Computes standard IEEE 802.3 CRC32 over a byte slice.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Validates ROM bytes before constructing [`GbaCore`].
pub fn validate_rom(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err(String::from("ROM file is empty."));
    }
    if bytes.len() < 4 {
        return Err(alloc::format!(
            "ROM file is too small ({} bytes; expected at least 4 bytes).",
            bytes.len()
        ));
    }
    if bytes.len() > MAX_ROM_BYTES {
        return Err(alloc::format!(
            "ROM exceeds the 64 MiB GBA limit ({} bytes).",
            bytes.len()
        ));
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Err(String::from(
            "This looks like a ZIP archive. Please extract the .gba ROM file first.",
        ));
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(String::from(
            "This is a PNG image, not a Game Boy Advance ROM.",
        ));
    }
    if bytes.starts_with(b"\x7FELF") {
        return Err(String::from(
            "This is an ELF executable. Convert it to a raw .gba binary with objcopy first.",
        ));
    }
    if bytes.starts_with(b"%PDF-") {
        return Err(String::from("This is a PDF document, not a GBA ROM."));
    }
    Ok(())
}

/// Validates a GBA BIOS dump (must be exactly 16 KiB).
pub fn validate_bios(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != BIOS_BYTES {
        return Err(alloc::format!(
            "Invalid GBA BIOS: expected exactly {BIOS_BYTES} bytes (16 KiB), got {} bytes.",
            bytes.len()
        ));
    }
    Ok(())
}

/// Stateful web emulator controller independent of `wasm-bindgen` for host testing.
pub struct WebEmulator {
    core: Option<GbaCore>,
    frontend: WebFrontend,
    bios: Option<Vec<u8>>,
    rom_crc32: u32,
    rom_size: usize,
    speed: f32,
    volume_pct: u8,
}

impl WebEmulator {
    /// Creates an idle emulator instance waiting for a ROM (and optional BIOS).
    pub fn new() -> Self {
        Self {
            core: None,
            frontend: WebFrontend::new(),
            bios: None,
            rom_crc32: 0,
            rom_size: 0,
            speed: 1.0,
            volume_pct: 100,
        }
    }

    /// Loads a `.gba` ROM image and boots a fresh [`GbaCore`].
    pub fn load_rom(&mut self, bytes: &[u8]) -> Result<(), String> {
        validate_rom(bytes)?;
        let crc = crc32(bytes);
        let rom_len = bytes.len();
        let mut core = GbaCore::new(self.bios.as_deref(), bytes.to_vec());
        core.bus.apu.master_volume_pct = self.volume_pct;
        self.frontend.clear_buffers();
        self.frontend.present_frame(&core.bus.ppu.framebuffer);
        self.core = Some(core);
        self.rom_crc32 = crc;
        self.rom_size = rom_len;
        Ok(())
    }

    /// Loads a 16 KiB GBA BIOS image. If a ROM is active, resets the core with LLE BIOS.
    pub fn load_bios(&mut self, bytes: &[u8]) -> Result<(), String> {
        validate_bios(bytes)?;
        let owned = bytes.to_vec();
        if let Some(core) = &mut self.core {
            core.load_bios(Some(&owned));
            core.bus.apu.master_volume_pct = self.volume_pct;
            self.frontend.clear_buffers();
            self.frontend.present_frame(&core.bus.ppu.framebuffer);
        }
        self.bios = Some(owned);
        Ok(())
    }

    /// Clears the loaded BIOS image and falls back to the built-in HLE BIOS.
    pub fn clear_bios(&mut self) {
        self.bios = None;
        if let Some(core) = &mut self.core {
            core.load_bios(None);
            core.bus.apu.master_volume_pct = self.volume_pct;
            self.frontend.clear_buffers();
            self.frontend.present_frame(&core.bus.ppu.framebuffer);
        }
    }

    /// Steps one GBA frame (`280,896` CPU cycles). Returns `false` if no ROM is loaded.
    pub fn run_frame(&mut self) -> bool {
        let Some(core) = &mut self.core else {
            return false;
        };
        self.frontend.step_frame(core);
        true
    }

    /// Performs a hard reset while keeping the loaded ROM, BIOS, and battery save.
    pub fn reset(&mut self) -> bool {
        let Some(core) = &mut self.core else {
            return false;
        };
        core.reset();
        core.bus.apu.master_volume_pct = self.volume_pct;
        self.frontend.clear_buffers();
        self.frontend.present_frame(&core.bus.ppu.framebuffer);
        true
    }

    /// Performs a GBA SWI 0x00 SoftReset if a ROM is loaded.
    pub fn soft_reset(&mut self) -> bool {
        let Some(core) = &mut self.core else {
            return false;
        };
        core.soft_reset();
        self.frontend.audio_samples.clear();
        true
    }

    /// Sets the active-high 10-bit GBA button mask (`KEY_A`..`KEY_L`).
    pub fn set_keys(&mut self, pressed_mask: u16) {
        self.frontend.set_pressed_keys(pressed_mask);
    }

    /// Sets target emulation speed multiplier (clamped to `0.25..=8.0`).
    pub fn set_speed(&mut self, speed: f32) {
        if speed.is_finite() {
            self.speed = speed.clamp(0.25, 8.0);
        }
    }

    /// Returns the active target speed multiplier.
    pub const fn speed(&self) -> f32 {
        self.speed
    }

    /// Sets display color profile (`0` Linear, `1` AGB-001, `2` AGS-101, `3` AGS-001).
    pub fn set_color_profile(&mut self, profile_id: u8) {
        let profile = ColorProfile::from_u8(profile_id);
        self.frontend.set_color_profile(profile);
        if let Some(core) = &self.core {
            self.frontend.present_frame(&core.bus.ppu.framebuffer);
        }
    }

    /// Returns numeric color profile ID (`0..=3`).
    pub const fn color_profile(&self) -> u8 {
        self.frontend.color_profile() as u8
    }

    /// Toggles LCD frame persistence blending.
    pub fn set_lcd_ghosting(&mut self, enabled: bool) {
        self.frontend.set_lcd_ghosting(enabled);
    }

    /// Returns whether LCD frame persistence blending is active.
    pub const fn lcd_ghosting(&self) -> bool {
        self.frontend.lcd_ghosting()
    }

    /// Sets APU master volume percentage (`0..=100`).
    pub fn set_volume(&mut self, pct: u8) {
        self.volume_pct = pct.min(100);
        if let Some(core) = &mut self.core {
            core.bus.apu.master_volume_pct = self.volume_pct;
        }
    }

    /// Returns the current RGBA8888 framebuffer (`240 * 160 * 4` bytes).
    pub fn framebuffer(&self) -> &[u8] {
        &self.frontend.rgba_frame
    }

    /// Copies the RGBA8888 framebuffer into `dst`, returning the number of bytes written.
    pub fn copy_rgba_frame(&self, dst: &mut [u8]) -> usize {
        let len = dst.len().min(self.frontend.rgba_frame.len());
        dst[..len].copy_from_slice(&self.frontend.rgba_frame[..len]);
        len
    }

    /// Returns buffered interleaved stereo PCM `i16` audio samples.
    pub fn audio_samples(&self) -> &[i16] {
        &self.frontend.audio_samples
    }

    /// Clears buffered audio samples without releasing vector capacity.
    pub fn clear_audio(&mut self) {
        self.frontend.audio_samples.clear();
    }

    /// Drains and returns all buffered interleaved stereo PCM `i16` samples.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        core::mem::take(&mut self.frontend.audio_samples)
    }

    /// Returns the core's native audio output sample rate in Hz.
    pub const fn sample_rate(&self) -> u32 {
        SAMPLE_RATE_HZ
    }

    /// Serializes full emulator state into bytes.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let core = self
            .core
            .as_ref()
            .ok_or_else(|| String::from("No ROM loaded."))?;
        core.save_state()
            .map_err(|e| alloc::format!("Failed to serialize save state: {e}"))
    }

    /// Restores emulator state from serialized bytes and refreshes the framebuffer.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        let core = self
            .core
            .as_mut()
            .ok_or_else(|| String::from("Load a ROM before loading a save state."))?;
        core.load_state(bytes)
            .map_err(|e| alloc::format!("Invalid or incompatible save state: {e}"))?;
        core.bus.apu.master_volume_pct = self.volume_pct;
        self.frontend.audio_samples.clear();
        self.frontend.present_frame(&core.bus.ppu.framebuffer);
        Ok(())
    }

    /// Exports active cartridge battery save bytes (empty if game has no save hardware).
    pub fn export_sram(&self) -> Vec<u8> {
        let Some(core) = &self.core else {
            return Vec::new();
        };
        core.bus.cart.export_save_data().to_vec()
    }

    /// Imports battery save bytes into cartridge RAM/Flash/EEPROM.
    pub fn import_sram(&mut self, bytes: &[u8]) -> Result<(), String> {
        let core = self
            .core
            .as_mut()
            .ok_or_else(|| String::from("Load a ROM before importing a save file."))?;
        if bytes.is_empty() {
            return Err(String::from("Save file is empty."));
        }
        core.bus.cart.load_save_data(bytes);
        Ok(())
    }

    /// Returns `true` when cartridge save memory was written since the last clear.
    pub fn sram_dirty(&self) -> bool {
        self.core
            .as_ref()
            .is_some_and(|core| core.bus.cart.save_dirty)
    }

    /// Marks cartridge save memory as persisted.
    pub fn clear_sram_dirty(&mut self) {
        if let Some(core) = &mut self.core {
            core.bus.cart.save_dirty = false;
        }
    }

    /// Returns whether a ROM is currently loaded.
    pub const fn rom_loaded(&self) -> bool {
        self.core.is_some()
    }

    /// Returns whether a user-supplied 16 KiB LLE BIOS is loaded.
    pub const fn bios_loaded(&self) -> bool {
        self.bios.is_some()
    }

    /// Returns the cartridge title from the GBA header (`0xA0..0xAC`).
    pub fn game_title(&self) -> String {
        self.core
            .as_ref()
            .map_or_else(String::new, |core| core.bus.cart.game_title())
    }

    /// Returns the 4-character game code from the GBA header (`0xAC..0xB0`).
    pub fn game_code(&self) -> String {
        self.core
            .as_ref()
            .map_or_else(String::new, |core| core.bus.cart.game_code())
    }

    /// Returns the IEEE 802.3 CRC32 checksum of the loaded ROM.
    pub const fn rom_crc32(&self) -> u32 {
        self.rom_crc32
    }

    /// Returns the uppercase 8-digit hex CRC32 of the loaded ROM.
    pub fn rom_crc32_hex(&self) -> String {
        if self.core.is_none() {
            return String::new();
        }
        alloc::format!("{:08X}", self.rom_crc32)
    }

    /// Returns a canonical IndexedDB storage key combining game code and ROM CRC32.
    pub fn storage_key(&self) -> String {
        if self.core.is_none() {
            return String::new();
        }
        alloc::format!("{}_{:08X}", self.game_code(), self.rom_crc32)
    }

    /// Returns a human-readable label for the detected cartridge save type.
    pub fn save_type_name(&self) -> &'static str {
        let Some(core) = &self.core else {
            return "None";
        };
        match core.bus.cart.effective_save_type() {
            SaveType::None => "None",
            SaveType::Sram32K => "SRAM 32 KiB",
            SaveType::Flash64K => "Flash 64 KiB",
            SaveType::Flash128K => "Flash 128 KiB",
            SaveType::Eeprom512B => "EEPROM 512 B",
            SaveType::Eeprom8K => "EEPROM 8 KiB",
        }
    }

    /// Returns whether the cartridge has a Real-Time Clock enabled.
    pub fn has_rtc(&self) -> bool {
        self.core
            .as_ref()
            .is_some_and(|core| core.bus.cart.effective_rtc())
    }

    /// Returns the byte length of the loaded ROM.
    pub const fn rom_size(&self) -> usize {
        self.rom_size
    }

    /// Provides read-only access to the active [`GbaCore`] for host tests.
    #[cfg(test)]
    pub(crate) fn core(&self) -> Option<&GbaCore> {
        self.core.as_ref()
    }
}

impl Default for WebEmulator {
    fn default() -> Self {
        Self::new()
    }
}
