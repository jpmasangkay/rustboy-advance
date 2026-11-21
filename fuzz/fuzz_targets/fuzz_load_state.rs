#![no_main]

use std::cell::RefCell;

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use libfuzzer_sys::fuzz_target;

const MAX_STATE_INPUT_BYTES: usize = 256 * 1024;

struct NullFrontend;

impl HostFrontend for NullFrontend {
    fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
    fn push_audio_samples(&mut self, _samples: &[i16]) {}
    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

thread_local! {
    static CORE: RefCell<GbaCore> = RefCell::new(GbaCore::new(None, vec![0u8; 0x200]));
    static HEADER_PREFIX: [u8; 10] = {
        let core = GbaCore::new(None, vec![0u8; 0x200]);
        let sample = core.save_state().expect("initial save_state");
        let mut hdr = [0u8; 10];
        hdr.copy_from_slice(&sample[..10]);
        hdr
    };
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > MAX_STATE_INPUT_BYTES {
        return;
    }
    let mut fe = NullFrontend;
    CORE.with(|cell| {
        let mut core = cell.borrow_mut();
        let mode = data[0] % 3;
        let payload = &data[1..];
        let loaded = match mode {
            0 => core.load_state(payload).is_ok(),
            1 => {
                // Prepend valid RBAS v2 + ROM CRC32 header so fuzzer immediately reaches LZ4 & postcard.
                let mut framed = Vec::with_capacity(10 + payload.len());
                HEADER_PREFIX.with(|hdr| framed.extend_from_slice(hdr));
                framed.extend_from_slice(payload);
                core.load_state(&framed).is_ok()
            }
            _ => core.load_state_raw(payload).is_ok(),
        };
        if loaded {
            for _ in 0..2 {
                core.run_frame(&mut fe);
            }
            core.reset();
        }
    });
});
