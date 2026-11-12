use crate::app::RustboyApp;

impl RustboyApp {
    pub(crate) fn step_rewind_frame(&mut self) {
        if let Some(prev) = self.rewind_buffer.pop_back() {
            if let Some(core) = &mut self.core {
                if core.load_state(&prev).is_ok() {
                    self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                    self.sink.frame_dirty = true;
                    self.notify("Rewinding ⏪");
                }
            }
        } else {
            self.notify("Rewind buffer empty");
            self.rewinding = false;
        }
    }

    pub(crate) fn step_rewind_manual(&mut self) {
        if let Some(prev) = self.rewind_buffer.pop_back() {
            if let Some(core) = &mut self.core {
                if core.load_state(&prev).is_ok() {
                    self.sink.framebuffer.copy_from_slice(&*core.bus.ppu.framebuffer);
                    self.sink.frame_dirty = true;
                    self.notify("Rewound 1 step (0.5s)");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    #[test]
    fn test_rewind_buffer_limit() {
        let mut buffer: VecDeque<Vec<u8>> = VecDeque::with_capacity(60);
        for i in 0..80 {
            if buffer.len() >= 60 {
                buffer.pop_front();
            }
            buffer.push_back(vec![i as u8]);
        }
        assert_eq!(buffer.len(), 60);
        assert_eq!(buffer.front(), Some(&vec![20u8]));
        assert_eq!(buffer.back(), Some(&vec![79u8]));

        let popped = buffer.pop_back();
        assert_eq!(popped, Some(vec![79u8]));
        assert_eq!(buffer.len(), 59);
    }
}
