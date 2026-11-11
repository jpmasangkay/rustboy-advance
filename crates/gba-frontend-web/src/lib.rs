extern crate alloc;

use alloc::vec::Vec;
use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

pub struct WebFrontend {
    pub rgba_frame: Vec<u8>,
    pub audio_samples: Vec<i16>,
    pub keyinput: u16,
}

impl WebFrontend {
    pub fn new() -> Self {
        Self {
            rgba_frame: alloc::vec![0u8; LCD_WIDTH * LCD_HEIGHT * 4],
            audio_samples: Vec::with_capacity(2048),
            keyinput: 0x03FF,
        }
    }

    pub fn step_frame(&mut self, core: &mut GbaCore) {
        core.run_frame(self);
    }
}

impl Default for WebFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFrontend for WebFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        for (i, &px) in fb.iter().enumerate() {
            let r = (((px & 0x1F) * 255) / 31) as u8;
            let g = ((((px >> 5) & 0x1F) * 255) / 31) as u8;
            let b = ((((px >> 10) & 0x1F) * 255) / 31) as u8;
            let off = i * 4;
            self.rgba_frame[off] = r;
            self.rgba_frame[off + 1] = g;
            self.rgba_frame[off + 2] = b;
            self.rgba_frame[off + 3] = 255;
        }
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_samples.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keyinput
    }
}

