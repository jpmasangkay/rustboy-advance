#![no_main]

use gba_debugger::{disassemble_arm, disassemble_thumb};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }

    let addr = if data.len() >= 8 {
        u32::from_le_bytes([data[4], data[5], data[6], data[7]])
    } else {
        0x0800_0000
    };

    for (idx, chunk) in data.chunks_exact(4).enumerate() {
        let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let cur_addr = addr.wrapping_add((idx as u32).wrapping_mul(4));
        let _ = disassemble_arm(word, cur_addr);
        let _ = disassemble_thumb((word & 0xFFFF) as u16, cur_addr);
        let _ = disassemble_thumb((word >> 16) as u16, cur_addr.wrapping_add(2));
    }
});
