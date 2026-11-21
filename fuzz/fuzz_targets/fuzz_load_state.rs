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
    let mut fe = NullFrontend;
    let mut core = GbaCore::new(None, vec![0u8; 0x200]);
    if core.load_state(data).is_ok() {
        for _ in 0..4 {
            core.run_frame(&mut fe);
        }
    }

    let mut raw_core = GbaCore::new(None, vec![0u8; 0x200]);
    if raw_core.load_state_raw(data).is_ok() {
        for _ in 0..2 {
            raw_core.run_frame(&mut fe);
        }
    }
});
