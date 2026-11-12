#![no_main]

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use libfuzzer_sys::fuzz_target;

struct NullFrontend;

impl HostFrontend for NullFrontend {
    fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
    fn push_audio_samples(&mut self, _samples: &[i16]) {}
    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

fuzz_target!(|data: &[u8]| {
    // Cap at maximum GBA cartridge ROM size (32 MiB) to avoid OOM in libFuzzer.
    if data.len() > 32 * 1024 * 1024 {
        return;
    }
    let mut core = GbaCore::new(None, data.to_vec());
    let mut fe = NullFrontend;
    for _ in 0..4 {
        core.run_frame(&mut fe);
    }
});
