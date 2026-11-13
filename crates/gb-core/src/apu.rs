use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{CPU_CLOCK_HZ, DEFAULT_SAMPLE_RATE_HZ};

const CPU_CLOCK_U64: u64 = CPU_CLOCK_HZ as u64;

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

const NOISE_DIVISORS: [u32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

#[derive(Clone, Serialize, Deserialize)]
pub struct SquareChannel {
    pub has_sweep: bool,
    pub enabled: bool,
    pub dac_enabled: bool,

    // Registers
    pub nr_sweep: u8,
    pub nr_len_duty: u8,
    pub nr_env: u8,
    pub freq: u16,
    pub length_enable: bool,

    // Runtime state
    pub length_counter: u16,
    pub timer: u32,
    pub duty_pos: u8,
    pub volume: u8,
    pub env_timer: u8,

    // Sweep state
    pub sweep_timer: u8,
    pub sweep_enabled: bool,
    pub sweep_shadow: u16,
    pub sweep_neg_used: bool,
}

impl SquareChannel {
    pub fn new(has_sweep: bool) -> Self {
        Self {
            has_sweep,
            enabled: false,
            dac_enabled: false,
            nr_sweep: 0,
            nr_len_duty: 0,
            nr_env: 0,
            freq: 0,
            length_enable: false,
            length_counter: 0,
            timer: 4,
            duty_pos: 0,
            volume: 0,
            env_timer: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            sweep_shadow: 0,
            sweep_neg_used: false,
        }
    }

    pub fn power_off(&mut self, preserve_length: bool) {
        let len = self.length_counter;
        *self = Self::new(self.has_sweep);
        if preserve_length {
            self.length_counter = len;
        }
    }

    #[inline]
    fn period(&self) -> u32 {
        ((2048 - (self.freq as u32 & 0x7FF)) * 4).max(4)
    }

    pub fn write_sweep(&mut self, val: u8) {
        let old_neg = (self.nr_sweep & 0x08) != 0;
        self.nr_sweep = val & 0x7F;
        let new_neg = (self.nr_sweep & 0x08) != 0;
        if old_neg && !new_neg && self.sweep_neg_used {
            self.enabled = false;
        }
    }

    pub fn write_len_duty(&mut self, val: u8) {
        self.nr_len_duty = val;
        self.length_counter = 64 - ((val & 0x3F) as u16);
    }

    pub fn write_env(&mut self, val: u8) {
        self.nr_env = val;
        self.dac_enabled = (val & 0xF8) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_freq_lo(&mut self, val: u8) {
        self.freq = (self.freq & 0x0700) | (val as u16);
    }

    pub fn write_freq_hi(&mut self, val: u8, frame_seq_step: u8) {
        self.freq = (self.freq & 0x00FF) | (((val as u16) & 0x07) << 8);
        let prev_len_en = self.length_enable;
        self.length_enable = (val & 0x40) != 0;

        let odd_step = (frame_seq_step & 1) != 0;
        if !prev_len_en && self.length_enable && odd_step && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 && (val & 0x80) == 0 {
                self.enabled = false;
            }
        }

        if (val & 0x80) != 0 {
            self.trigger(odd_step);
        }
    }

    fn trigger(&mut self, odd_step: bool) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
            if self.length_enable && odd_step {
                self.length_counter -= 1;
            }
        }
        self.timer = self.period();
        self.volume = (self.nr_env >> 4) & 0x0F;
        self.env_timer = self.nr_env & 0x07;
        if self.env_timer == 0 {
            self.env_timer = 8;
        }

        if self.has_sweep {
            self.sweep_shadow = self.freq;
            let p = (self.nr_sweep >> 4) & 0x07;
            let s = self.nr_sweep & 0x07;
            self.sweep_timer = if p == 0 { 8 } else { p };
            self.sweep_enabled = p != 0 || s != 0;
            self.sweep_neg_used = false;
            if s != 0 {
                let _ = self.calc_sweep_freq();
            }
        }
    }

    fn calc_sweep_freq(&mut self) -> Option<u16> {
        let shift = self.nr_sweep & 0x07;
        let negate = (self.nr_sweep & 0x08) != 0;
        let delta = self.sweep_shadow >> shift;
        let next = if negate {
            self.sweep_neg_used = true;
            self.sweep_shadow.wrapping_sub(delta)
        } else {
            self.sweep_shadow.wrapping_add(delta)
        };
        if next > 2047 {
            self.enabled = false;
            None
        } else {
            Some(next)
        }
    }

    pub fn tick_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub fn tick_sweep(&mut self) {
        if !self.has_sweep {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer == 0 {
            let p = (self.nr_sweep >> 4) & 0x07;
            self.sweep_timer = if p == 0 { 8 } else { p };
            if self.sweep_enabled
                && p != 0
                && let Some(new_freq) = self.calc_sweep_freq()
                && (self.nr_sweep & 0x07) != 0
            {
                self.sweep_shadow = new_freq;
                self.freq = new_freq;
                let _ = self.calc_sweep_freq();
            }
        }
    }

    pub fn tick_envelope(&mut self) {
        let period = self.nr_env & 0x07;
        if period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = period;
            if (self.nr_env & 0x08) != 0 {
                if self.volume < 15 {
                    self.volume += 1;
                }
            } else if self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    #[inline]
    pub fn step_cycles(&mut self, dt: u32) {
        if !self.enabled {
            return;
        }
        let mut rem = dt;
        while rem > 0 {
            if self.timer > rem {
                self.timer -= rem;
                break;
            }
            rem -= self.timer;
            self.timer = self.period();
            self.duty_pos = (self.duty_pos + 1) & 7;
        }
    }

    #[inline]
    pub fn dac_output(&self) -> i32 {
        if !self.dac_enabled {
            return 0;
        }
        if !self.enabled {
            return 15;
        }
        let duty = ((self.nr_len_duty >> 6) & 0x03) as usize;
        let sample = if DUTY_TABLE[duty][self.duty_pos as usize] != 0 {
            self.volume as i32
        } else {
            0
        };
        15 - 2 * sample
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WaveChannel {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub length_counter: u16,
    pub length_enable: bool,
    pub volume_code: u8,
    pub freq: u16,
    pub timer: u32,
    pub pos: u8,
    pub sample_buf: u8,
    pub wave_ram: [u8; 16],
}

impl WaveChannel {
    pub fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            length_counter: 0,
            length_enable: false,
            volume_code: 0,
            freq: 0,
            timer: 2,
            pos: 0,
            sample_buf: 0,
            wave_ram: [0; 16],
        }
    }

    pub fn power_off(&mut self, preserve_length: bool) {
        let ram = self.wave_ram;
        let len = self.length_counter;
        *self = Self::new();
        self.wave_ram = ram;
        if preserve_length {
            self.length_counter = len;
        }
    }

    #[inline]
    fn period(&self) -> u32 {
        ((2048 - (self.freq as u32 & 0x7FF)) * 2).max(2)
    }

    pub fn write_dac(&mut self, val: u8) {
        self.dac_enabled = (val & 0x80) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_len(&mut self, val: u8) {
        self.length_counter = 256 - (val as u16);
    }

    pub fn write_volume(&mut self, val: u8) {
        self.volume_code = (val >> 5) & 0x03;
    }

    pub fn write_freq_lo(&mut self, val: u8) {
        self.freq = (self.freq & 0x0700) | (val as u16);
    }

    pub fn write_freq_hi(&mut self, val: u8, frame_seq_step: u8) {
        self.freq = (self.freq & 0x00FF) | (((val as u16) & 0x07) << 8);
        let prev_len_en = self.length_enable;
        self.length_enable = (val & 0x40) != 0;

        let odd_step = (frame_seq_step & 1) != 0;
        if !prev_len_en && self.length_enable && odd_step && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 && (val & 0x80) == 0 {
                self.enabled = false;
            }
        }

        if (val & 0x80) != 0 {
            self.enabled = self.dac_enabled;
            if self.length_counter == 0 {
                self.length_counter = 256;
                if self.length_enable && odd_step {
                    self.length_counter -= 1;
                }
            }
            self.timer = self.period() + 6;
            self.pos = 0;
        }
    }

    pub fn tick_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    #[inline]
    pub fn step_cycles(&mut self, dt: u32) {
        if !self.enabled {
            return;
        }
        let mut rem = dt;
        while rem > 0 {
            if self.timer > rem {
                self.timer -= rem;
                break;
            }
            rem -= self.timer;
            self.timer = self.period();
            self.pos = (self.pos + 1) & 31;
            let byte = self.wave_ram[(self.pos >> 1) as usize];
            self.sample_buf = if (self.pos & 1) == 0 {
                byte >> 4
            } else {
                byte & 0x0F
            };
        }
    }

    #[inline]
    pub fn dac_output(&self) -> i32 {
        if !self.dac_enabled {
            return 0;
        }
        if !self.enabled {
            return 15;
        }
        let sample = match self.volume_code {
            0 => 0,
            1 => self.sample_buf,
            2 => self.sample_buf >> 1,
            _ => self.sample_buf >> 2,
        } as i32;
        15 - 2 * sample
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NoiseChannel {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub length_counter: u16,
    pub length_enable: bool,
    pub nr_env: u8,
    pub nr_poly: u8,
    pub volume: u8,
    pub env_timer: u8,
    pub lfsr: u16,
    pub timer: u32,
}

impl NoiseChannel {
    pub fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            length_counter: 0,
            length_enable: false,
            nr_env: 0,
            nr_poly: 0,
            volume: 0,
            env_timer: 0,
            lfsr: 0x7FFF,
            timer: 8,
        }
    }

    pub fn power_off(&mut self, preserve_length: bool) {
        let len = self.length_counter;
        *self = Self::new();
        if preserve_length {
            self.length_counter = len;
        }
    }

    #[inline]
    fn period(&self) -> u32 {
        let div = NOISE_DIVISORS[(self.nr_poly & 0x07) as usize];
        let shift = (self.nr_poly >> 4) & 0x0F;
        if shift >= 14 { u32::MAX } else { div << shift }
    }

    pub fn write_len(&mut self, val: u8) {
        self.length_counter = 64 - ((val & 0x3F) as u16);
    }

    pub fn write_env(&mut self, val: u8) {
        self.nr_env = val;
        self.dac_enabled = (val & 0xF8) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_poly(&mut self, val: u8) {
        self.nr_poly = val;
    }

    pub fn write_ctrl(&mut self, val: u8, frame_seq_step: u8) {
        let prev_len_en = self.length_enable;
        self.length_enable = (val & 0x40) != 0;

        let odd_step = (frame_seq_step & 1) != 0;
        if !prev_len_en && self.length_enable && odd_step && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 && (val & 0x80) == 0 {
                self.enabled = false;
            }
        }

        if (val & 0x80) != 0 {
            self.enabled = self.dac_enabled;
            if self.length_counter == 0 {
                self.length_counter = 64;
                if self.length_enable && odd_step {
                    self.length_counter -= 1;
                }
            }
            self.lfsr = 0x7FFF;
            self.timer = self.period();
            self.volume = (self.nr_env >> 4) & 0x0F;
            self.env_timer = self.nr_env & 0x07;
            if self.env_timer == 0 {
                self.env_timer = 8;
            }
        }
    }

    pub fn tick_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub fn tick_envelope(&mut self) {
        let period = self.nr_env & 0x07;
        if period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = period;
            if (self.nr_env & 0x08) != 0 {
                if self.volume < 15 {
                    self.volume += 1;
                }
            } else if self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    #[inline]
    pub fn step_cycles(&mut self, dt: u32) {
        if !self.enabled || ((self.nr_poly >> 4) & 0x0F) >= 14 {
            return;
        }
        let mut rem = dt;
        while rem > 0 {
            if self.timer > rem {
                self.timer -= rem;
                break;
            }
            rem -= self.timer;
            self.timer = self.period();
            let xor_bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
            self.lfsr = (self.lfsr >> 1) | (xor_bit << 14);
            if (self.nr_poly & 0x08) != 0 {
                self.lfsr = (self.lfsr & !(1 << 6)) | (xor_bit << 6);
            }
        }
    }

    #[inline]
    pub fn dac_output(&self) -> i32 {
        if !self.dac_enabled {
            return 0;
        }
        if !self.enabled {
            return 15;
        }
        let sample = if (self.lfsr & 1) == 0 {
            self.volume as i32
        } else {
            0
        };
        15 - 2 * sample
    }
}

/// Game Boy / Game Boy Color APU with 4 channels, DIV-driven frame sequencer,
/// cycle-integrated box-averaged downsampling, and fixed-point DC-blocking HPF.
#[derive(Clone, Serialize, Deserialize)]
pub struct Apu {
    pub ch1: SquareChannel,
    pub ch2: SquareChannel,
    pub ch3: WaveChannel,
    pub ch4: NoiseChannel,

    pub nr50: u8,
    pub nr51: u8,
    pub power_on: bool,
    pub frame_seq_step: u8,

    pub sample_rate_hz: u32,
    sample_acc: u64,
    integ_l: i64,
    integ_r: i64,
    hpf_cap_l: i32,
    hpf_cap_r: i32,

    #[serde(skip)]
    pub output_buffer: Vec<i16>,
}

impl Apu {
    pub fn new(sample_rate_hz: u32) -> Self {
        Self {
            ch1: SquareChannel::new(true),
            ch2: SquareChannel::new(false),
            ch3: WaveChannel::new(),
            ch4: NoiseChannel::new(),
            nr50: 0,
            nr51: 0,
            power_on: false,
            frame_seq_step: 0,
            sample_rate_hz: sample_rate_hz.clamp(8_000, 192_000),
            sample_acc: 0,
            integ_l: 0,
            integ_r: 0,
            hpf_cap_l: 0,
            hpf_cap_r: 0,
            output_buffer: Vec::with_capacity(2048),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate_hz: u32) {
        self.sample_rate_hz = sample_rate_hz.clamp(8_000, 192_000);
        self.sample_acc = 0;
        self.integ_l = 0;
        self.integ_r = 0;
    }

    pub fn init_post_boot(&mut self) {
        self.power_on = true;
        self.nr50 = 0x77;
        self.nr51 = 0xF3;
        self.ch1.nr_sweep = 0x80;
        self.ch1.nr_len_duty = 0xBF;
        self.ch1.nr_env = 0xF3;
        self.ch1.dac_enabled = true;
        self.ch1.enabled = true;
        self.ch1.length_counter = 1;
        self.ch2.nr_len_duty = 0x3F;
        self.ch3.nr_len_duty_dummy();
        self.ch3.volume_code = 0;
        self.ch4.length_counter = 1;
    }

    pub fn tick_frame_sequencer(&mut self) {
        if !self.power_on {
            return;
        }
        match self.frame_seq_step {
            0 | 4 => {
                self.ch1.tick_length();
                self.ch2.tick_length();
                self.ch3.tick_length();
                self.ch4.tick_length();
            }
            2 | 6 => {
                self.ch1.tick_length();
                self.ch2.tick_length();
                self.ch3.tick_length();
                self.ch4.tick_length();
                self.ch1.tick_sweep();
            }
            7 => {
                self.ch1.tick_envelope();
                self.ch2.tick_envelope();
                self.ch4.tick_envelope();
            }
            _ => {}
        }
        self.frame_seq_step = (self.frame_seq_step + 1) & 7;
    }

    /// Advances the APU by `normal_cycles` (4.194304 MHz ticks).
    pub fn step(&mut self, normal_cycles: u32) {
        let mut rem = normal_cycles;
        while rem > 0 {
            let mut step = rem;
            if self.power_on {
                if self.ch1.enabled {
                    step = step.min(self.ch1.timer.max(1));
                }
                if self.ch2.enabled {
                    step = step.min(self.ch2.timer.max(1));
                }
                if self.ch3.enabled {
                    step = step.min(self.ch3.timer.max(1));
                }
                if self.ch4.enabled && ((self.ch4.nr_poly >> 4) & 0x0F) < 14 {
                    step = step.min(self.ch4.timer.max(1));
                }
            }

            if self.power_on {
                self.ch1.step_cycles(step);
                self.ch2.step_cycles(step);
                self.ch3.step_cycles(step);
                self.ch4.step_cycles(step);
            }

            let (left, right) = self.mix_instantaneous();
            self.integrate_cycles(step, left, right);
            rem -= step;
        }
    }

    #[inline]
    fn mix_instantaneous(&self) -> (i64, i64) {
        if !self.power_on {
            return (0, 0);
        }
        let s1 = self.ch1.dac_output();
        let s2 = self.ch2.dac_output();
        let s3 = self.ch3.dac_output();
        let s4 = self.ch4.dac_output();

        let mut mix_r = 0i32;
        let mut mix_l = 0i32;
        if (self.nr51 & 0x01) != 0 {
            mix_r += s1;
        }
        if (self.nr51 & 0x02) != 0 {
            mix_r += s2;
        }
        if (self.nr51 & 0x04) != 0 {
            mix_r += s3;
        }
        if (self.nr51 & 0x08) != 0 {
            mix_r += s4;
        }
        if (self.nr51 & 0x10) != 0 {
            mix_l += s1;
        }
        if (self.nr51 & 0x20) != 0 {
            mix_l += s2;
        }
        if (self.nr51 & 0x40) != 0 {
            mix_l += s3;
        }
        if (self.nr51 & 0x80) != 0 {
            mix_l += s4;
        }

        let vol_r = ((self.nr50 & 0x07) as i32) + 1;
        let vol_l = (((self.nr50 >> 4) & 0x07) as i32) + 1;
        ((mix_l * vol_l * 48) as i64, (mix_r * vol_r * 48) as i64)
    }

    #[inline]
    fn integrate_cycles(&mut self, dt: u32, left: i64, right: i64) {
        let rate = self.sample_rate_hz as u64;
        let mut work = (dt as u64) * rate;
        while work > 0 {
            let need = CPU_CLOCK_U64 - self.sample_acc;
            let take = work.min(need);
            self.integ_l += left * (take as i64);
            self.integ_r += right * (take as i64);
            self.sample_acc += take;
            work -= take;

            if self.sample_acc == CPU_CLOCK_U64 {
                let raw_l = (self.integ_l / (CPU_CLOCK_U64 as i64)) as i32;
                let raw_r = (self.integ_r / (CPU_CLOCK_U64 as i64)) as i32;
                self.push_filtered_sample(raw_l, raw_r);
                self.integ_l = 0;
                self.integ_r = 0;
                self.sample_acc = 0;
            }
        }
    }

    #[inline]
    fn push_filtered_sample(&mut self, raw_l: i32, raw_r: i32) {
        let scaled_l = raw_l << 8;
        let scaled_r = raw_r << 8;
        let hp_l = scaled_l - self.hpf_cap_l;
        let hp_r = scaled_r - self.hpf_cap_r;
        self.hpf_cap_l += hp_l >> 8;
        self.hpf_cap_r += hp_r >> 8;

        let out_l = (hp_l >> 8).clamp(-32768, 32767) as i16;
        let out_r = (hp_r >> 8).clamp(-32768, 32767) as i16;
        self.output_buffer.push(out_l);
        self.output_buffer.push(out_r);
    }

    pub fn read_reg(&self, addr: u16) -> u8 {
        match addr {
            0xFF10 => self.ch1.nr_sweep | 0x80,
            0xFF11 => (self.ch1.nr_len_duty & 0xC0) | 0x3F,
            0xFF12 => self.ch1.nr_env,
            0xFF13 => 0xFF,
            0xFF14 => (if self.ch1.length_enable { 0x40 } else { 0 }) | 0xBF,
            0xFF15 => 0xFF,
            0xFF16 => (self.ch2.nr_len_duty & 0xC0) | 0x3F,
            0xFF17 => self.ch2.nr_env,
            0xFF18 => 0xFF,
            0xFF19 => (if self.ch2.length_enable { 0x40 } else { 0 }) | 0xBF,
            0xFF1A => (if self.ch3.dac_enabled { 0x80 } else { 0 }) | 0x7F,
            0xFF1B => 0xFF,
            0xFF1C => ((self.ch3.volume_code & 0x03) << 5) | 0x9F,
            0xFF1D => 0xFF,
            0xFF1E => (if self.ch3.length_enable { 0x40 } else { 0 }) | 0xBF,
            0xFF1F | 0xFF20 => 0xFF,
            0xFF21 => self.ch4.nr_env,
            0xFF22 => self.ch4.nr_poly,
            0xFF23 => (if self.ch4.length_enable { 0x40 } else { 0 }) | 0xBF,
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => {
                let mut v = 0x70;
                if self.power_on {
                    v |= 0x80;
                    if self.ch1.enabled {
                        v |= 0x01;
                    }
                    if self.ch2.enabled {
                        v |= 0x02;
                    }
                    if self.ch3.enabled {
                        v |= 0x04;
                    }
                    if self.ch4.enabled {
                        v |= 0x08;
                    }
                }
                v
            }
            0xFF27..=0xFF2F => 0xFF,
            0xFF30..=0xFF3F => self.ch3.wave_ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    pub fn write_reg(&mut self, addr: u16, val: u8, is_cgb: bool) {
        if addr == 0xFF26 {
            let was_on = self.power_on;
            let now_on = (val & 0x80) != 0;
            self.power_on = now_on;
            if was_on && !now_on {
                self.nr50 = 0;
                self.nr51 = 0;
                self.ch1.power_off(!is_cgb);
                self.ch2.power_off(!is_cgb);
                self.ch3.power_off(!is_cgb);
                self.ch4.power_off(!is_cgb);
            } else if !was_on && now_on {
                self.frame_seq_step = 0;
            }
            return;
        }

        if (0xFF30..=0xFF3F).contains(&addr) {
            self.ch3.wave_ram[(addr - 0xFF30) as usize] = val;
            return;
        }

        if !self.power_on {
            // On DMG, length counters remain writable while APU is powered off.
            if !is_cgb {
                match addr {
                    0xFF11 => self.ch1.write_len_duty(val & 0x3F),
                    0xFF16 => self.ch2.write_len_duty(val & 0x3F),
                    0xFF1B => self.ch3.write_len(val),
                    0xFF20 => self.ch4.write_len(val),
                    _ => {}
                }
            }
            return;
        }

        let step = self.frame_seq_step;
        match addr {
            0xFF10 => self.ch1.write_sweep(val),
            0xFF11 => self.ch1.write_len_duty(val),
            0xFF12 => self.ch1.write_env(val),
            0xFF13 => self.ch1.write_freq_lo(val),
            0xFF14 => self.ch1.write_freq_hi(val, step),
            0xFF16 => self.ch2.write_len_duty(val),
            0xFF17 => self.ch2.write_env(val),
            0xFF18 => self.ch2.write_freq_lo(val),
            0xFF19 => self.ch2.write_freq_hi(val, step),
            0xFF1A => self.ch3.write_dac(val),
            0xFF1B => self.ch3.write_len(val),
            0xFF1C => self.ch3.write_volume(val),
            0xFF1D => self.ch3.write_freq_lo(val),
            0xFF1E => self.ch3.write_freq_hi(val, step),
            0xFF20 => self.ch4.write_len(val),
            0xFF21 => self.ch4.write_env(val),
            0xFF22 => self.ch4.write_poly(val),
            0xFF23 => self.ch4.write_ctrl(val, step),
            0xFF24 => self.nr50 = val,
            0xFF25 => self.nr51 = val,
            _ => {}
        }
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new(DEFAULT_SAMPLE_RATE_HZ)
    }
}

impl WaveChannel {
    fn nr_len_duty_dummy(&mut self) {
        self.length_counter = 1;
    }
}
