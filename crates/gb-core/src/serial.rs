use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// Game Boy Link Port / Serial (`SB` `0xFF01`, `SC` `0xFF02`) stub.
///
/// With no link partner connected, transfers using the internal clock shift in `0xFF`,
/// clear `SC` bit 7, raise the Serial interrupt, and capture outgoing `SB` bytes into
/// `output_buffer` for test ROM output (e.g. blargg tests).
#[derive(Clone, Serialize, Deserialize)]
pub struct Serial {
    pub sb: u8,
    pub sc: u8,
    active: bool,
    bits_remaining: u8,
    cycle_acc: u16,
    bit_period_cycles: u16,
    pub output_buffer: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Self {
            sb: 0x00,
            sc: 0x00,
            active: false,
            bits_remaining: 0,
            cycle_acc: 0,
            bit_period_cycles: 512,
            output_buffer: Vec::new(),
        }
    }

    pub fn read_sb(&self) -> u8 {
        self.sb
    }

    pub fn write_sb(&mut self, val: u8) {
        self.sb = val;
    }

    pub fn read_sc(&self, is_cgb: bool) -> u8 {
        if is_cgb {
            self.sc | 0x7C
        } else {
            self.sc | 0x7E
        }
    }

    pub fn write_sc(&mut self, val: u8, is_cgb: bool) {
        let mask = if is_cgb { 0x83 } else { 0x81 };
        self.sc = val & mask;

        if (self.sc & 0x81) == 0x81 {
            // Capture outgoing byte for blargg / debug serial console.
            self.output_buffer.push(self.sb);
            self.active = true;
            self.bits_remaining = 8;
            self.cycle_acc = 0;
            self.bit_period_cycles = if is_cgb && (self.sc & 0x02) != 0 {
                16
            } else {
                512
            };
        } else if (self.sc & 0x80) == 0 {
            self.active = false;
        }
    }

    /// Advances serial transfer by 1 CPU M-cycle (4 CPU T-cycles).
    /// Returns `true` if a transfer completed and the Serial IRQ should be requested.
    pub fn step_m_cycle(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.cycle_acc += 4;
        let mut irq = false;
        while self.active && self.cycle_acc >= self.bit_period_cycles {
            self.cycle_acc -= self.bit_period_cycles;
            // Disconnected link line pulls MOSI/MISO high -> shifts in 1 bits (0xFF).
            self.sb = (self.sb << 1) | 1;
            self.bits_remaining = self.bits_remaining.saturating_sub(1);
            if self.bits_remaining == 0 {
                self.active = false;
                self.sc &= !0x80;
                irq = true;
            }
        }
        irq
    }
}
