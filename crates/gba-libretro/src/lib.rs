use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

pub struct LibretroAdapter {
    pub xrgb1555: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub audio: Vec<i16>,
    pub keyinput: u16,
}

impl LibretroAdapter {
    pub fn new() -> Self {
        Self {
            xrgb1555: [0x7FFF; LCD_WIDTH * LCD_HEIGHT],
            audio: Vec::with_capacity(2048),
            keyinput: 0x03FF,
        }
    }

    pub fn run_frame(&mut self, core: &mut GbaCore) {
        self.audio.clear();
        core.run_frame(self);
    }
}

impl Default for LibretroAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFrontend for LibretroAdapter {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        for (dst, &px) in self.xrgb1555.iter_mut().zip(fb.iter()) {
            // Convert BGR555 -> RGB555 (0RRR RRGG GGGB BBBB)
            let r = px & 0x1F;
            let g = (px >> 5) & 0x1F;
            let b = (px >> 10) & 0x1F;
            *dst = (r << 10) | (g << 5) | b;
        }
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keyinput
    }
}

