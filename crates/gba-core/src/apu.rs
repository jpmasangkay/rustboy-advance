use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::APU_SAMPLE_INTERVAL;

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

#[derive(Clone, Serialize, Deserialize)]
pub struct DmaAudioFifo {
    pub buffer: [u32; 7],
    pub read_idx: u8,
    pub write_idx: u8,
    pub count: u8,
    pub active_latch: u32,
    pub latch_bytes_left: u8,
    pub current_sample: i8,
}

impl DmaAudioFifo {
    pub fn new() -> Self {
        Self {
            buffer: [0; 7],
            read_idx: 0,
            write_idx: 0,
            count: 0,
            active_latch: 0,
            latch_bytes_left: 0,
            current_sample: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buffer = [0; 7];
        self.read_idx = 0;
        self.write_idx = 0;
        self.count = 0;
        self.active_latch = 0;
        self.latch_bytes_left = 0;
        self.current_sample = 0;
    }

    pub fn push_word(&mut self, val: u32) {
        if self.count >= 7 {
            // Overwrite oldest on hardware overflow, keep FIFO intact
            self.read_idx = (self.read_idx + 1) % 7;
            self.count -= 1;
        }
        self.buffer[self.write_idx as usize] = val;
        self.write_idx = (self.write_idx + 1) % 7;
        self.count += 1;
    }

    pub fn push_half(&mut self, offset: u32, val: u16) {
        let shift = (offset & 2) * 8;
        let word = (val as u32) << shift;
        self.push_word(word);
    }

    pub fn push_byte(&mut self, offset: u32, val: u8) {
        let shift = (offset & 3) * 8;
        let word = (val as u32) << shift;
        self.push_word(word);
    }

    /// Consumes 1 signed byte from active latch (or pops next 32-bit word).
    /// Returns `true` if remaining buffered words <= 4 (meaning <= 16 bytes -> DMA refill request).
    pub fn pop_sample(&mut self) -> bool {
        if self.latch_bytes_left == 0 {
            if self.count > 0 {
                self.active_latch = self.buffer[self.read_idx as usize];
                self.read_idx = (self.read_idx + 1) % 7;
                self.count -= 1;
                self.latch_bytes_left = 4;
            } else {
                // FIFO empty: keep outputting last sample or 0.
                return true;
            }
        }
        self.current_sample = (self.active_latch & 0xFF) as i8;
        self.active_latch >>= 8;
        self.latch_bytes_left -= 1;
        self.count <= 4
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PsgSquare {
    pub has_sweep: bool,
    pub sweep_period: u8,
    pub sweep_negate: bool,
    pub sweep_shift: u8,
    pub sweep_timer: u8,
    pub sweep_enabled: bool,
    pub sweep_shadow: u16,

    pub duty: u8,
    pub duty_pos: u8,
    pub length_counter: u8,
    pub length_enabled: bool,

    pub env_init_vol: u8,
    pub env_add: bool,
    pub env_period: u8,
    pub env_timer: u8,
    pub current_vol: u8,

    pub freq: u16,
    pub timer_cycles: i32,
    pub dac_enabled: bool,
    pub enabled: bool,

    pub reg_l: u16,
    pub reg_h: u16,
    pub reg_x: u16,
}

impl PsgSquare {
    pub fn new(has_sweep: bool) -> Self {
        Self {
            has_sweep,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            sweep_shadow: 0,
            duty: 0,
            duty_pos: 0,
            length_counter: 0,
            length_enabled: false,
            env_init_vol: 0,
            env_add: false,
            env_period: 0,
            env_timer: 0,
            current_vol: 0,
            freq: 0,
            timer_cycles: 0,
            dac_enabled: false,
            enabled: false,
            reg_l: 0,
            reg_h: 0,
            reg_x: 0,
        }
    }

    pub fn write_sweep(&mut self, val: u16) {
        self.reg_l = val;
        self.sweep_period = ((val >> 4) & 0x07) as u8;
        self.sweep_negate = (val & 0x08) != 0;
        self.sweep_shift = (val & 0x07) as u8;
    }

    pub fn write_duty_env(&mut self, val: u16) {
        self.reg_h = val;
        self.duty = ((val >> 6) & 0x03) as u8;
        let len_load = (val & 0x3F) as u8;
        self.length_counter = 64 - len_load;
        self.env_init_vol = ((val >> 12) & 0x0F) as u8;
        self.env_add = (val & 0x0800) != 0;
        self.env_period = ((val >> 8) & 0x07) as u8;
        self.dac_enabled = (val & 0xF800) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_freq_ctrl(&mut self, val: u16) {
        self.reg_x = val;
        self.freq = val & 0x07FF;
        self.length_enabled = (val & 0x4000) != 0;
        if (val & 0x8000) != 0 {
            self.trigger();
        }
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer_cycles = ((2048 - self.freq as i32) * 16).max(16);
        self.current_vol = self.env_init_vol;
        self.env_timer = if self.env_period == 0 { 8 } else { self.env_period };

        if self.has_sweep {
            self.sweep_shadow = self.freq;
            self.sweep_timer = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            self.sweep_enabled = self.sweep_period > 0 || self.sweep_shift > 0;
            if self.sweep_shift > 0 && self.calc_sweep_freq() > 2047 {
                self.enabled = false;
            }
        }
    }

    fn calc_sweep_freq(&self) -> u16 {
        let delta = self.sweep_shadow >> self.sweep_shift;
        if self.sweep_negate {
            self.sweep_shadow.saturating_sub(delta)
        } else {
            self.sweep_shadow + delta
        }
    }

    pub fn tick_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
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
            self.sweep_timer = if self.sweep_period == 0 {
                8
            } else {
                self.sweep_period
            };
            if self.sweep_enabled && self.sweep_period > 0 {
                let new_freq = self.calc_sweep_freq();
                if new_freq <= 2047 && self.sweep_shift > 0 {
                    self.freq = new_freq;
                    self.sweep_shadow = new_freq;
                    if self.calc_sweep_freq() > 2047 {
                        self.enabled = false;
                    }
                } else if new_freq > 2047 {
                    self.enabled = false;
                }
            }
        }
    }

    pub fn tick_envelope(&mut self) {
        if self.env_period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = self.env_period;
            if self.env_add && self.current_vol < 15 {
                self.current_vol += 1;
            } else if !self.env_add && self.current_vol > 0 {
                self.current_vol -= 1;
            }
        }
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if !self.enabled {
            return;
        }
        self.timer_cycles -= cycles;
        let period = ((2048 - self.freq as i32) * 16).max(16);
        while self.timer_cycles <= 0 {
            self.timer_cycles += period;
            self.duty_pos = (self.duty_pos + 1) & 7;
        }
    }

    #[inline]
    pub fn sample(&self) -> i16 {
        if !self.enabled || !self.dac_enabled {
            return 0;
        }
        let bit = DUTY_TABLE[self.duty as usize][self.duty_pos as usize];
        if bit != 0 {
            self.current_vol as i16
        } else {
            -(self.current_vol as i16)
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PsgWave {
    pub dac_enabled: bool,
    pub dimension_64: bool,
    pub bank_select: u8,
    pub length_counter: u16,
    pub length_enabled: bool,
    pub volume_code: u8,
    pub force_75: bool,
    pub freq: u16,
    pub timer_cycles: i32,
    pub enabled: bool,
    pub sample_pos: u8,
    pub wave_ram: [u8; 32], // 2 banks of 16 bytes (32 x 4-bit samples per bank)
    pub reg_l: u16,
    pub reg_h: u16,
    pub reg_x: u16,
}

impl PsgWave {
    pub fn new() -> Self {
        Self {
            dac_enabled: false,
            dimension_64: false,
            bank_select: 0,
            length_counter: 0,
            length_enabled: false,
            volume_code: 0,
            force_75: false,
            freq: 0,
            timer_cycles: 0,
            enabled: false,
            sample_pos: 0,
            wave_ram: [0; 32],
            reg_l: 0,
            reg_h: 0,
            reg_x: 0,
        }
    }

    pub fn write_l(&mut self, val: u16) {
        self.reg_l = val;
        self.dimension_64 = (val & 0x0020) != 0;
        self.bank_select = ((val >> 6) & 1) as u8;
        self.dac_enabled = (val & 0x0080) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_h(&mut self, val: u16) {
        self.reg_h = val;
        self.length_counter = 256 - (val & 0xFF);
        self.volume_code = ((val >> 13) & 0x03) as u8;
        self.force_75 = (val & 0x8000) != 0;
    }

    pub fn write_x(&mut self, val: u16) {
        self.reg_x = val;
        self.freq = val & 0x07FF;
        self.length_enabled = (val & 0x4000) != 0;
        if (val & 0x8000) != 0 {
            self.enabled = self.dac_enabled;
            if self.length_counter == 0 {
                self.length_counter = 256;
            }
            self.sample_pos = 0;
            self.timer_cycles = ((2048 - self.freq as i32) * 8).max(8);
        }
    }

    pub fn read_wave_ram(&self, offset: usize) -> u8 {
        // CPU accesses the bank opposite to the currently playing bank.
        let cpu_bank = (self.bank_select ^ 1) as usize;
        self.wave_ram[cpu_bank * 16 + (offset & 0x0F)]
    }

    pub fn write_wave_ram(&mut self, offset: usize, val: u8) {
        let cpu_bank = (self.bank_select ^ 1) as usize;
        self.wave_ram[cpu_bank * 16 + (offset & 0x0F)] = val;
    }

    pub fn tick_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if !self.enabled {
            return;
        }
        self.timer_cycles -= cycles;
        let period = ((2048 - self.freq as i32) * 8).max(8);
        while self.timer_cycles <= 0 {
            self.timer_cycles += period;
            if self.dimension_64 {
                self.sample_pos = (self.sample_pos + 1) & 63;
            } else {
                self.sample_pos = (self.sample_pos + 1) & 31;
            }
        }
    }

    pub fn sample(&self) -> i16 {
        if !self.enabled || !self.dac_enabled {
            return 0;
        }
        let bank = if self.dimension_64 {
            (self.sample_pos >> 5) as usize
        } else {
            self.bank_select as usize
        };
        let byte_idx = ((self.sample_pos & 31) >> 1) as usize;
        let byte = self.wave_ram[bank * 16 + byte_idx];
        let nibble = if (self.sample_pos & 1) == 0 {
            (byte >> 4) & 0x0F
        } else {
            byte & 0x0F
        } as i16;

        let scaled = if self.force_75 {
            (nibble * 3) >> 2
        } else {
            match self.volume_code {
                0 => 0,
                1 => nibble,
                2 => nibble >> 1,
                _ => nibble >> 2,
            }
        };
        (scaled * 2) - 15
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PsgNoise {
    pub length_counter: u8,
    pub length_enabled: bool,
    pub env_init_vol: u8,
    pub env_add: bool,
    pub env_period: u8,
    pub env_timer: u8,
    pub current_vol: u8,
    pub shift_clock: u8,
    pub width_7: bool,
    pub divisor_code: u8,
    pub lfsr: u16,
    pub timer_cycles: i32,
    pub dac_enabled: bool,
    pub enabled: bool,
    pub reg_l: u16,
    pub reg_h: u16,
}

impl PsgNoise {
    pub fn new() -> Self {
        Self {
            length_counter: 0,
            length_enabled: false,
            env_init_vol: 0,
            env_add: false,
            env_period: 0,
            env_timer: 0,
            current_vol: 0,
            shift_clock: 0,
            width_7: false,
            divisor_code: 0,
            lfsr: 0x7FFF,
            timer_cycles: 0,
            dac_enabled: false,
            enabled: false,
            reg_l: 0,
            reg_h: 0,
        }
    }

    pub fn write_l(&mut self, val: u16) {
        self.reg_l = val;
        self.length_counter = 64 - (val & 0x3F) as u8;
        self.env_init_vol = ((val >> 12) & 0x0F) as u8;
        self.env_add = (val & 0x0800) != 0;
        self.env_period = ((val >> 8) & 0x07) as u8;
        self.dac_enabled = (val & 0xF800) != 0;
        if !self.dac_enabled {
            self.enabled = false;
        }
    }

    pub fn write_h(&mut self, val: u16) {
        self.reg_h = val;
        self.divisor_code = (val & 0x07) as u8;
        self.width_7 = (val & 0x08) != 0;
        self.shift_clock = ((val >> 4) & 0x0F) as u8;
        self.length_enabled = (val & 0x4000) != 0;
        if (val & 0x8000) != 0 {
            self.enabled = self.dac_enabled;
            if self.length_counter == 0 {
                self.length_counter = 64;
            }
            self.lfsr = if self.width_7 { 0x007F } else { 0x7FFF };
            self.current_vol = self.env_init_vol;
            self.env_timer = if self.env_period == 0 { 8 } else { self.env_period };
            self.timer_cycles = self.calc_period();
        }
    }

    fn calc_period(&self) -> i32 {
        let base = if self.divisor_code == 0 {
            8
        } else {
            (self.divisor_code as i32) * 16
        };
        (base << self.shift_clock.min(13)) * 4
    }

    pub fn tick_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub fn tick_envelope(&mut self) {
        if self.env_period == 0 {
            return;
        }
        if self.env_timer > 0 {
            self.env_timer -= 1;
        }
        if self.env_timer == 0 {
            self.env_timer = self.env_period;
            if self.env_add && self.current_vol < 15 {
                self.current_vol += 1;
            } else if !self.env_add && self.current_vol > 0 {
                self.current_vol -= 1;
            }
        }
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if !self.enabled || self.shift_clock >= 14 {
            return;
        }
        self.timer_cycles -= cycles;
        let period = self.calc_period().max(32);
        while self.timer_cycles <= 0 {
            self.timer_cycles += period;
            let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr = (self.lfsr >> 1) | (bit << 14);
            if self.width_7 {
                self.lfsr = (self.lfsr & !(1 << 6)) | (bit << 6);
            }
        }
    }

    #[inline]
    pub fn sample(&self) -> i16 {
        if !self.enabled || !self.dac_enabled {
            return 0;
        }
        let bit = (!self.lfsr) & 1;
        if bit != 0 {
            self.current_vol as i16
        } else {
            -(self.current_vol as i16)
        }
    }
}

#[inline]
fn default_channel_mask() -> u8 {
    0x3F
}

#[inline]
fn default_master_vol() -> u8 {
    100
}

#[inline]
fn default_scopes() -> [[i16; 128]; 6] {
    [[0; 128]; 6]
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Apu {
    pub fifo_a: DmaAudioFifo,
    pub fifo_b: DmaAudioFifo,
    pub sq1: PsgSquare,
    pub sq2: PsgSquare,
    pub wave: PsgWave,
    pub noise: PsgNoise,

    pub soundcnt_l: u16,
    pub soundcnt_h: u16,
    pub soundcnt_x: u16,
    pub soundbias: u16,

    pub frame_seq_step: u8,
    pub sample_tick_count: u8,
    pub output_buffer: Vec<i16>,

    /// Bitmask of enabled audio channels:
    /// 0: Sq1, 1: Sq2, 2: Wave, 3: Noise, 4: FIFO A, 5: FIFO B.
    #[serde(default = "default_channel_mask")]
    pub channel_enable_mask: u8,
    /// Frontend master volume multiplier percentage (`0..=100`).
    #[serde(default = "default_master_vol")]
    pub master_volume_pct: u8,
    /// Ring buffers storing the latest 128 mono samples per channel for visualizer UI.
    #[serde(skip, default = "default_scopes")]
    pub chan_scopes: [[i16; 128]; 6],
    #[serde(skip)]
    pub scope_cursor: usize,
}

impl Apu {
    pub fn new() -> Self {
        Self {
            fifo_a: DmaAudioFifo::new(),
            fifo_b: DmaAudioFifo::new(),
            sq1: PsgSquare::new(true),
            sq2: PsgSquare::new(false),
            wave: PsgWave::new(),
            noise: PsgNoise::new(),
            soundcnt_l: 0,
            soundcnt_h: 0,
            soundcnt_x: 0,
            soundbias: 0x0200,
            frame_seq_step: 0,
            sample_tick_count: 0,
            output_buffer: Vec::with_capacity(2048),
            channel_enable_mask: 0x3F,
            master_volume_pct: 100,
            chan_scopes: [[0; 128]; 6],
            scope_cursor: 0,
        }
    }

    #[inline]
    pub fn master_enabled(&self) -> bool {
        (self.soundcnt_x & 0x0080) != 0
    }

    pub fn read_soundcnt_x(&self) -> u16 {
        let mut status = self.soundcnt_x & 0x0080;
        if self.sq1.enabled {
            status |= 0x01;
        }
        if self.sq2.enabled {
            status |= 0x02;
        }
        if self.wave.enabled {
            status |= 0x04;
        }
        if self.noise.enabled {
            status |= 0x08;
        }
        status
    }

    pub fn write_soundcnt_x(&mut self, val: u16) {
        let was_enabled = self.master_enabled();
        let now_enabled = (val & 0x0080) != 0;
        self.soundcnt_x = val & 0x0080;
        if was_enabled && !now_enabled {
            self.sq1 = PsgSquare::new(true);
            self.sq2 = PsgSquare::new(false);
            let wave_ram = self.wave.wave_ram;
            self.wave = PsgWave::new();
            self.wave.wave_ram = wave_ram;
            self.noise = PsgNoise::new();
            self.soundcnt_l = 0;
        } else if !was_enabled && now_enabled {
            self.frame_seq_step = 0;
        }
    }

    pub fn write_soundcnt_h(&mut self, val: u16) {
        if (val & 0x0800) != 0 {
            self.fifo_a.reset();
        }
        if (val & 0x8000) != 0 {
            self.fifo_b.reset();
        }
        self.soundcnt_h = val & 0x770F;
    }

    /// Returns `(dma1_refill_needed, dma2_refill_needed)` for timer overflow `timer_idx` (0 or 1).
    pub fn on_timer_overflow(&mut self, timer_idx: u8) -> (bool, bool) {
        if !self.master_enabled() {
            return (false, false);
        }
        let timer_a = ((self.soundcnt_h >> 10) & 1) as u8;
        let timer_b = ((self.soundcnt_h >> 14) & 1) as u8;
        let mut req_a = false;
        let mut req_b = false;

        if timer_a == timer_idx {
            req_a = self.fifo_a.pop_sample();
        }
        if timer_b == timer_idx {
            req_b = self.fifo_b.pop_sample();
        }
        (req_a, req_b)
    }

    pub fn step_sample_tick(&mut self) {
        if !self.master_enabled() {
            for ch in &mut self.chan_scopes {
                ch[self.scope_cursor] = 0;
            }
            self.scope_cursor = (self.scope_cursor + 1) & 127;
            self.output_buffer.push(0);
            self.output_buffer.push(0);
            return;
        }

        // Frame sequencer clocks at 512 Hz -> 32_768 / 512 = every 64 ticks of 512 CPU cycles.
        self.sample_tick_count = self.sample_tick_count.wrapping_add(1);
        if self.sample_tick_count >= 64 {
            self.sample_tick_count = 0;
            self.tick_frame_sequencer();
        }

        let cycles = APU_SAMPLE_INTERVAL as i32;
        self.sq1.step_cycles(cycles);
        self.sq2.step_cycles(cycles);
        self.wave.step_cycles(cycles);
        self.noise.step_cycles(cycles);

        let (left, right) = self.mix_and_record();
        self.output_buffer.push(left);
        self.output_buffer.push(right);
    }

    fn tick_frame_sequencer(&mut self) {
        match self.frame_seq_step {
            0 => {
                self.sq1.tick_length();
                self.sq2.tick_length();
                self.wave.tick_length();
                self.noise.tick_length();
            }
            2 => {
                self.sq1.tick_length();
                self.sq2.tick_length();
                self.wave.tick_length();
                self.noise.tick_length();
                self.sq1.tick_sweep();
            }
            4 => {
                self.sq1.tick_length();
                self.sq2.tick_length();
                self.wave.tick_length();
                self.noise.tick_length();
            }
            6 => {
                self.sq1.tick_length();
                self.sq2.tick_length();
                self.wave.tick_length();
                self.noise.tick_length();
                self.sq1.tick_sweep();
            }
            7 => {
                self.sq1.tick_envelope();
                self.sq2.tick_envelope();
                self.noise.tick_envelope();
            }
            _ => {}
        }
        self.frame_seq_step = (self.frame_seq_step + 1) & 7;
    }

    fn mix_and_record(&mut self) -> (i16, i16) {
        let mask = self.channel_enable_mask;
        let s1 = if (mask & 0x01) != 0 { self.sq1.sample() } else { 0 };
        let s2 = if (mask & 0x02) != 0 { self.sq2.sample() } else { 0 };
        let s3 = if (mask & 0x04) != 0 { self.wave.sample() } else { 0 };
        let s4 = if (mask & 0x08) != 0 { self.noise.sample() } else { 0 };

        let cur = self.scope_cursor;
        self.chan_scopes[0][cur] = (s1 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[1][cur] = (s2 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[2][cur] = (s3 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[3][cur] = (s4 as i32 * 2048).clamp(-32768, 32767) as i16;

        let mut psg_r = 0i32;
        let mut psg_l = 0i32;

        if (self.soundcnt_l & 0x0100) != 0 {
            psg_r += s1 as i32;
        }
        if (self.soundcnt_l & 0x0200) != 0 {
            psg_r += s2 as i32;
        }
        if (self.soundcnt_l & 0x0400) != 0 {
            psg_r += s3 as i32;
        }
        if (self.soundcnt_l & 0x0800) != 0 {
            psg_r += s4 as i32;
        }

        if (self.soundcnt_l & 0x1000) != 0 {
            psg_l += s1 as i32;
        }
        if (self.soundcnt_l & 0x2000) != 0 {
            psg_l += s2 as i32;
        }
        if (self.soundcnt_l & 0x4000) != 0 {
            psg_l += s3 as i32;
        }
        if (self.soundcnt_l & 0x8000) != 0 {
            psg_l += s4 as i32;
        }

        let vol_r = ((self.soundcnt_l & 0x07) as i32) + 1;
        let vol_l = (((self.soundcnt_l >> 4) & 0x07) as i32) + 1;
        psg_r = (psg_r * vol_r) / 8;
        psg_l = (psg_l * vol_l) / 8;

        match self.soundcnt_h & 0x03 {
            0 => {
                psg_r >>= 2;
                psg_l >>= 2;
            }
            1 => {
                psg_r >>= 1;
                psg_l >>= 1;
            }
            _ => {}
        }

        let raw_a = if (mask & 0x10) != 0 { self.fifo_a.current_sample as i32 } else { 0 };
        let raw_b = if (mask & 0x20) != 0 { self.fifo_b.current_sample as i32 } else { 0 };
        self.chan_scopes[4][cur] = (raw_a * 256).clamp(-32768, 32767) as i16;
        self.chan_scopes[5][cur] = (raw_b * 256).clamp(-32768, 32767) as i16;
        self.scope_cursor = (cur + 1) & 127;

        let mut fifo_a = raw_a * 4;
        if (self.soundcnt_h & 0x04) == 0 {
            fifo_a >>= 1;
        }

        let mut fifo_b = raw_b * 4;
        if (self.soundcnt_h & 0x08) == 0 {
            fifo_b >>= 1;
        }

        let mut total_r = psg_r;
        let mut total_l = psg_l;

        if (self.soundcnt_h & 0x0100) != 0 {
            total_r += fifo_a;
        }
        if (self.soundcnt_h & 0x0200) != 0 {
            total_l += fifo_a;
        }
        if (self.soundcnt_h & 0x1000) != 0 {
            total_r += fifo_b;
        }
        if (self.soundcnt_h & 0x2000) != 0 {
            total_l += fifo_b;
        }

        let bias = (self.soundbias & 0x03FE) as i32;
        let res_bits = (self.soundbias >> 14) & 0x03;
        let quant_mask = !((1i32 << (res_bits + 1)) - 1);
        let raw_l = (total_l + bias).clamp(0, 1023);
        let raw_r = (total_r + bias).clamp(0, 1023);
        let clamped_l = (raw_l & quant_mask) - (bias & quant_mask);
        let clamped_r = (raw_r & quant_mask) - (bias & quant_mask);

        let vol_scale = self.master_volume_pct.min(100) as i32;
        let out_l = ((clamped_l * 48 * vol_scale) / 100).clamp(-32768, 32767) as i16;
        let out_r = ((clamped_r * 48 * vol_scale) / 100).clamp(-32768, 32767) as i16;
        (out_l, out_r)
    }
}

