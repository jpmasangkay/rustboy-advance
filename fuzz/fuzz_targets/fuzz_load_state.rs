#![no_main]

use gba_core::GbaCore;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut core = GbaCore::new(None, vec![0u8; 0x200]);
    let _ = core.load_state(data);
});
