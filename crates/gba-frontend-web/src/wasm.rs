use std::sync::Once;
use wasm_bindgen::prelude::*;

use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::emulator::WebEmulator;

static PANIC_HOOK_INIT: Once = Once::new();

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(msg: &str);
}

fn install_panic_hook() {
    PANIC_HOOK_INIT.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            console_error(&format!("Rustboy Advance WASM panic: {info}"));
        }));
    });
}

/// WebAssembly bindings for the Rustboy Advance GBA emulator.
#[wasm_bindgen]
pub struct Emulator {
    inner: WebEmulator,
}

#[wasm_bindgen]
impl Emulator {
    /// Creates a new uninitialized emulator awaiting a ROM.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        install_panic_hook();
        Self {
            inner: WebEmulator::new(),
        }
    }

    /// Loads a GBA ROM image (`Uint8Array`) and initializes the core.
    pub fn load_rom(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        self.inner
            .load_rom(bytes)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// Loads a 16 KiB GBA BIOS image (`Uint8Array`).
    pub fn load_bios(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        self.inner
            .load_bios(bytes)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// Removes any loaded BIOS and reverts to built-in HLE BIOS.
    pub fn clear_bios(&mut self) {
        self.inner.clear_bios();
    }

    /// Executes one full GBA frame (280,896 cycles). Returns `false` if no ROM is loaded.
    pub fn run_frame(&mut self) -> bool {
        self.inner.run_frame()
    }

    /// Hard-resets the console while preserving ROM, BIOS, and cartridge SRAM.
    pub fn reset(&mut self) -> bool {
        self.inner.reset()
    }

    /// Triggers GBA SWI 0x00 SoftReset.
    pub fn soft_reset(&mut self) -> bool {
        self.inner.soft_reset()
    }

    /// Native GBA horizontal resolution in pixels (`240`).
    pub fn width(&self) -> usize {
        LCD_WIDTH
    }

    /// Native GBA vertical resolution in pixels (`160`).
    pub fn height(&self) -> usize {
        LCD_HEIGHT
    }

    /// Byte pointer into WASM linear memory for the `240x160` RGBA8888 framebuffer.
    pub fn framebuffer_ptr(&self) -> *const u8 {
        self.inner.framebuffer().as_ptr()
    }

    /// Byte length of the RGBA8888 framebuffer (`153,600`).
    pub fn framebuffer_len(&self) -> usize {
        self.inner.framebuffer().len()
    }

    /// Copies the latest RGBA8888 framebuffer directly into a JS `Uint8Array`.
    pub fn copy_rgba_frame(&self, dst: &mut [u8]) -> usize {
        self.inner.copy_rgba_frame(dst)
    }

    /// Element pointer into WASM linear memory for buffered stereo `i16` audio samples.
    pub fn audio_ptr(&self) -> *const i16 {
        self.inner.audio_samples().as_ptr()
    }

    /// Number of buffered `i16` samples currently awaiting consumption.
    pub fn audio_len(&self) -> usize {
        self.inner.audio_samples().len()
    }

    /// Clears buffered audio samples after copying directly from linear memory.
    pub fn clear_audio(&mut self) {
        self.inner.clear_audio();
    }

    /// Drains and returns all buffered stereo interleaved `i16` audio samples.
    pub fn drain_audio(&mut self) -> Vec<i16> {
        self.inner.drain_audio()
    }

    /// Returns the core's audio sample rate in Hz (`gba_core::SAMPLE_RATE_HZ`).
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// Updates keypad state from an active-high 10-bit mask (`KEY_A`..`KEY_L`).
    pub fn set_keys(&mut self, mask: u16) {
        self.inner.set_keys(mask);
    }

    /// Sets speed multiplier (`0.25..=8.0`).
    pub fn set_speed(&mut self, speed: f32) {
        self.inner.set_speed(speed);
    }

    /// Returns the active speed multiplier.
    pub fn speed(&self) -> f32 {
        self.inner.speed()
    }

    /// Sets display color profile (`0` Linear, `1` AGB-001, `2` AGS-101, `3` AGS-001).
    pub fn set_color_profile(&mut self, profile: u8) {
        self.inner.set_color_profile(profile);
    }

    /// Returns the active color profile ID.
    pub fn color_profile(&self) -> u8 {
        self.inner.color_profile()
    }

    /// Toggles LCD frame persistence blending.
    pub fn set_lcd_ghosting(&mut self, enabled: bool) {
        self.inner.set_lcd_ghosting(enabled);
    }

    /// Returns whether LCD frame persistence blending is enabled.
    pub fn lcd_ghosting(&self) -> bool {
        self.inner.lcd_ghosting()
    }

    /// Sets master audio volume percentage (`0..=100`).
    pub fn set_volume(&mut self, pct: u8) {
        self.inner.set_volume(pct);
    }

    /// Serializes a save state blob (`Uint8Array`).
    pub fn save_state(&self) -> Result<Vec<u8>, JsValue> {
        self.inner.save_state().map_err(|e| JsValue::from_str(&e))
    }

    /// Restores emulator state from a save state blob (`Uint8Array`).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        self.inner
            .load_state(bytes)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// Exports cartridge battery save data (`Uint8Array`).
    pub fn export_sram(&self) -> Vec<u8> {
        self.inner.export_sram()
    }

    /// Imports cartridge battery save data (`Uint8Array`).
    pub fn import_sram(&mut self, bytes: &[u8]) -> Result<(), JsValue> {
        self.inner
            .import_sram(bytes)
            .map_err(|e| JsValue::from_str(&e))
    }

    /// Returns whether save memory has unwritten changes since the last flush.
    pub fn sram_dirty(&self) -> bool {
        self.inner.sram_dirty()
    }

    /// Clears the dirty flag after IndexedDB persistence succeeds.
    pub fn clear_sram_dirty(&mut self) {
        self.inner.clear_sram_dirty();
    }

    /// Whether a ROM is currently loaded.
    #[wasm_bindgen(getter)]
    pub fn rom_loaded(&self) -> bool {
        self.inner.rom_loaded()
    }

    /// Whether a 16 KiB LLE BIOS is currently loaded.
    #[wasm_bindgen(getter)]
    pub fn bios_loaded(&self) -> bool {
        self.inner.bios_loaded()
    }

    /// Game title parsed from the cartridge header (`0xA0..0xAC`).
    #[wasm_bindgen(getter)]
    pub fn title(&self) -> String {
        self.inner.game_title()
    }

    /// 4-character game code parsed from the cartridge header (`0xAC..0xB0`).
    #[wasm_bindgen(getter)]
    pub fn game_code(&self) -> String {
        self.inner.game_code()
    }

    /// IEEE 802.3 CRC32 checksum of the loaded ROM.
    #[wasm_bindgen(getter)]
    pub fn rom_crc32(&self) -> u32 {
        self.inner.rom_crc32()
    }

    /// Uppercase 8-digit hex CRC32 string for the loaded ROM.
    #[wasm_bindgen(getter)]
    pub fn rom_crc32_hex(&self) -> String {
        self.inner.rom_crc32_hex()
    }

    /// Canonical persistence key (`<GAME_CODE>_<CRC32>`).
    #[wasm_bindgen(getter)]
    pub fn storage_key(&self) -> String {
        self.inner.storage_key()
    }

    /// Human-readable cartridge save type label.
    #[wasm_bindgen(getter)]
    pub fn save_type(&self) -> String {
        self.inner.save_type_name().to_string()
    }

    /// Whether cartridge Real-Time Clock support is active.
    #[wasm_bindgen(getter)]
    pub fn has_rtc(&self) -> bool {
        self.inner.has_rtc()
    }

    /// Byte length of the loaded ROM.
    #[wasm_bindgen(getter)]
    pub fn rom_size(&self) -> usize {
        self.inner.rom_size()
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}
