use serde::{Deserialize, Serialize};

/// SM83 Timer (`DIV`, `TIMA`, `TMA`, `TAC`) with falling-edge multiplexer
/// and 1-M-cycle `TIMA` overflow reload delay.
#[derive(Clone, Serialize, Deserialize)]
pub struct Timer {
    /// Internal 16-bit system counter incremented every CPU T-cycle.
    /// `DIV` (`0xFF04`) exposes the upper 8 bits (`sys_clk >> 8`).
    pub sys_clk: u16,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    prev_mux: bool,
    /// Remaining CPU T-cycles until a pending `TIMA` overflow reloads from `TMA` and requests IRQ.
    /// `0` means no overflow pending.
    overflow_countdown: u8,
    /// Set during the M-cycle in which `TIMA` reloaded from `TMA` (Cycle B).
    reloaded_this_m_cycle: bool,
}

#[derive(Copy, Clone, Default)]
pub struct TimerStepResult {
    pub timer_irq: bool,
    pub apu_frame_seq: bool,
}

impl Timer {
    pub fn new(initial_sys_clk: u16) -> Self {
        Self {
            sys_clk: initial_sys_clk,
            tima: 0,
            tma: 0,
            tac: 0,
            prev_mux: false,
            overflow_countdown: 0,
            reloaded_this_m_cycle: false,
        }
    }

    #[inline]
    const fn tac_bit(tac: u8) -> u8 {
        match tac & 0x03 {
            0 => 9,
            1 => 3,
            2 => 5,
            _ => 7,
        }
    }

    #[inline]
    fn compute_mux(sys_clk: u16, tac: u8) -> bool {
        (tac & 0x04) != 0 && ((sys_clk >> Self::tac_bit(tac)) & 1) != 0
    }

    /// Advances the timer by one CPU M-cycle (4 CPU T-cycles).
    pub fn step_m_cycle(&mut self, double_speed: bool) -> TimerStepResult {
        self.reloaded_this_m_cycle = false;
        let mut res = TimerStepResult::default();
        let apu_bit = if double_speed { 13 } else { 12 };

        for _ in 0..4 {
            if self.overflow_countdown > 0 {
                self.overflow_countdown -= 1;
                if self.overflow_countdown == 0 {
                    self.tima = self.tma;
                    self.reloaded_this_m_cycle = true;
                    res.timer_irq = true;
                }
            }

            let old_clk = self.sys_clk;
            self.sys_clk = self.sys_clk.wrapping_add(1);

            if ((old_clk >> apu_bit) & 1) != 0 && ((self.sys_clk >> apu_bit) & 1) == 0 {
                res.apu_frame_seq = true;
            }

            let new_mux = Self::compute_mux(self.sys_clk, self.tac);
            if self.prev_mux && !new_mux {
                self.increment_tima();
            }
            self.prev_mux = new_mux;
        }

        res
    }

    #[inline]
    fn increment_tima(&mut self) {
        let (next, overflow) = self.tima.overflowing_add(1);
        self.tima = next;
        if overflow {
            self.overflow_countdown = 4;
        }
    }

    #[inline]
    pub fn read_div(&self) -> u8 {
        (self.sys_clk >> 8) as u8
    }

    /// Writing to `DIV` resets `sys_clk` to 0, which may trigger falling-edge ticks
    /// for both `TIMA` and the APU frame sequencer.
    pub fn write_div(&mut self, double_speed: bool) -> TimerStepResult {
        let apu_bit = if double_speed { 13 } else { 12 };
        let apu_frame_seq = ((self.sys_clk >> apu_bit) & 1) != 0;

        self.sys_clk = 0;
        let new_mux = Self::compute_mux(0, self.tac);
        if self.prev_mux && !new_mux {
            self.increment_tima();
        }
        self.prev_mux = new_mux;

        TimerStepResult {
            timer_irq: false,
            apu_frame_seq,
        }
    }

    #[inline]
    pub fn read_tima(&self) -> u8 {
        self.tima
    }

    pub fn write_tima(&mut self, val: u8) {
        if self.reloaded_this_m_cycle {
            // During Cycle B (when TMA->TIMA reload occurs), TMA wins over CPU writes to TIMA.
            return;
        }
        self.tima = val;
        // Writing TIMA during Cycle A cancels the pending reload & IRQ.
        self.overflow_countdown = 0;
    }

    #[inline]
    pub fn read_tma(&self) -> u8 {
        self.tma
    }

    pub fn write_tma(&mut self, val: u8) {
        self.tma = val;
        if self.reloaded_this_m_cycle {
            self.tima = val;
        }
    }

    #[inline]
    pub fn read_tac(&self) -> u8 {
        self.tac | 0xF8
    }

    pub fn write_tac(&mut self, val: u8) {
        self.tac = val & 0x07;
        let new_mux = Self::compute_mux(self.sys_clk, self.tac);
        if self.prev_mux && !new_mux {
            self.increment_tima();
        }
        self.prev_mux = new_mux;
    }

    /// Used by CGB speed switch to reset `DIV` without triggering spurious glitches.
    pub fn reset_div_on_speed_switch(&mut self) {
        self.sys_clk = 0;
        self.prev_mux = Self::compute_mux(0, self.tac);
    }
}
