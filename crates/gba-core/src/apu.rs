use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::blip::BlipBuf;
use crate::{CPU_CLOCK_HZ, SAMPLE_RATE_HZ};

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

const SCOPE_INTERVAL_CYCLES: u64 = 512; // 32_768 Hz oscilloscope capture cadence
const FIFO_INTERP_STEP_CYCLES: u64 = 128; // 131_072 Hz sub-stepping for smooth interpolated ramps

#[derive(Clone, Serialize, Deserialize)]
pub struct DmaAudioFifo {
    pub buffer: [u32; 16],
    pub read_idx: u8,
    pub write_idx: u8,
    pub count: u8,
    pub active_latch: u32,
    pub latch_bytes_left: u8,
    pub current_sample: i8,
    #[serde(default)]
    pub prev_sample: i8,
    #[serde(default)]
    pub last_pop_cycle: u64,
    #[serde(default)]
    pub ramp_period: u64,
}

impl DmaAudioFifo {
    pub fn new() -> Self {
        Self {
            buffer: [0; 16],
            read_idx: 0,
            write_idx: 0,
            count: 0,
            active_latch: 0,
            latch_bytes_left: 0,
            current_sample: 0,
            prev_sample: 0,
            last_pop_cycle: 0,
            ramp_period: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buffer = [0; 16];
        self.read_idx = 0;
        self.write_idx = 0;
        self.count = 0;
        self.active_latch = 0;
        self.latch_bytes_left = 0;
        self.current_sample = 0;
        self.prev_sample = 0;
        self.last_pop_cycle = 0;
        self.ramp_period = 0;
    }

    pub fn push_word(&mut self, val: u32) {
        if self.count >= 16 {
            // Overwrite oldest on hardware overflow, keep FIFO intact
            self.read_idx = (self.read_idx + 1) % 16;
            self.count -= 1;
        }
        self.buffer[self.write_idx as usize] = val;
        self.write_idx = (self.write_idx + 1) % 16;
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
        self.pop_sample_at(self.last_pop_cycle)
    }

    /// Consumes 1 signed byte at exact CPU `cycle`, updating interpolation ramp state.
    pub fn pop_sample_at(&mut self, cycle: u64) -> bool {
        let prev_val = self.interpolated_sample(cycle).clamp(-128, 127) as i8;
        if self.latch_bytes_left == 0 {
            if self.count > 0 {
                self.active_latch = self.buffer[self.read_idx as usize];
                self.read_idx = (self.read_idx + 1) % 16;
                self.count -= 1;
                self.latch_bytes_left = 4;
            } else {
                // FIFO empty: keep holding last sample without ramping.
                self.prev_sample = self.current_sample;
                self.ramp_period = 0;
                self.last_pop_cycle = cycle;
                return true;
            }
        }

        let new_sample = (self.active_latch & 0xFF) as i8;
        self.active_latch >>= 8;
        self.latch_bytes_left -= 1;

        if self.last_pop_cycle > 0 && cycle > self.last_pop_cycle {
            let dt = cycle - self.last_pop_cycle;
            self.ramp_period = if (16..=16_384).contains(&dt) { dt } else { 0 };
        } else {
            self.ramp_period = 0;
        }
        self.prev_sample = prev_val;
        self.current_sample = new_sample;
        self.last_pop_cycle = cycle;

        self.count <= 4
    }

    #[inline]
    pub fn is_ramping(&self, cycle: u64) -> bool {
        self.ramp_period > 0
            && self.current_sample != self.prev_sample
            && cycle < self.last_pop_cycle.saturating_add(self.ramp_period)
    }

    #[inline]
    pub fn interpolated_sample(&self, cycle: u64) -> i32 {
        if self.ramp_period == 0 || cycle >= self.last_pop_cycle.saturating_add(self.ramp_period) {
            return self.current_sample as i32;
        }
        let elapsed = cycle.saturating_sub(self.last_pop_cycle) as i64;
        let p = self.ramp_period as i64;
        let diff = (self.current_sample as i32 - self.prev_sample as i32) as i64;
        self.prev_sample as i32 + ((diff * elapsed) / p) as i32
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

    #[inline]
    fn period_cycles(&self) -> i32 {
        ((2048 - self.freq as i32) * 16).max(16)
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.timer_cycles = self.period_cycles();
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

    #[inline]
    pub fn next_step_cycles(&self) -> Option<u64> {
        if !self.enabled {
            None
        } else {
            Some(self.timer_cycles.max(1) as u64)
        }
    }

    #[inline]
    pub fn advance(&mut self, cycles: u64) {
        if !self.enabled || cycles == 0 {
            return;
        }
        let cyc = cycles as i64;
        if cyc < self.timer_cycles as i64 {
            self.timer_cycles -= cyc as i32;
            return;
        }
        let period = self.period_cycles() as i64;
        let rem = cyc - (self.timer_cycles.max(0) as i64);
        let steps = 1 + rem / period;
        let rem_mod = rem % period;
        self.duty_pos = ((self.duty_pos as i64 + steps) & 7) as u8;
        self.timer_cycles = (period - rem_mod) as i32;
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if cycles > 0 {
            self.advance(cycles as u64);
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
            self.timer_cycles = self.period_cycles();
        }
    }

    #[inline]
    fn period_cycles(&self) -> i32 {
        ((2048 - self.freq as i32) * 8).max(8)
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

    #[inline]
    pub fn next_step_cycles(&self) -> Option<u64> {
        if !self.enabled {
            None
        } else {
            Some(self.timer_cycles.max(1) as u64)
        }
    }

    #[inline]
    pub fn advance(&mut self, cycles: u64) {
        if !self.enabled || cycles == 0 {
            return;
        }
        let cyc = cycles as i64;
        if cyc < self.timer_cycles as i64 {
            self.timer_cycles -= cyc as i32;
            return;
        }
        let period = self.period_cycles() as i64;
        let rem = cyc - (self.timer_cycles.max(0) as i64);
        let steps = 1 + rem / period;
        let rem_mod = rem % period;
        let mask = if self.dimension_64 { 63 } else { 31 };
        self.sample_pos = ((self.sample_pos as i64 + steps) & mask) as u8;
        self.timer_cycles = (period - rem_mod) as i32;
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if cycles > 0 {
            self.advance(cycles as u64);
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

    #[inline]
    pub fn next_step_cycles(&self) -> Option<u64> {
        if !self.enabled || self.shift_clock >= 14 {
            None
        } else {
            Some(self.timer_cycles.max(1) as u64)
        }
    }

    #[inline]
    pub fn advance(&mut self, cycles: u64) {
        if !self.enabled || self.shift_clock >= 14 || cycles == 0 {
            return;
        }
        let cyc = cycles as i64;
        if cyc < self.timer_cycles as i64 {
            self.timer_cycles -= cyc as i32;
            return;
        }
        let period = self.calc_period().max(32) as i64;
        let rem = cyc - (self.timer_cycles.max(0) as i64);
        let steps = 1 + rem / period;
        let rem_mod = rem % period;
        self.timer_cycles = (period - rem_mod) as i32;

        // If volume is permanently 0 until retrigger, skip LFSR stepping in O(1).
        if self.current_vol == 0 && (!self.env_add || self.env_period == 0) {
            return;
        }

        let mask14 = !(1u16 << 14);
        let mask6 = !(1u16 << 6);
        let max_period_bits = if self.width_7 { 127 } else { 32767 };
        let lfsr_steps = steps.min(max_period_bits);
        for _ in 0..lfsr_steps {
            let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr = ((self.lfsr >> 1) & mask14) | (bit << 14);
            if self.width_7 {
                self.lfsr = (self.lfsr & mask6) | (bit << 6);
            }
        }
    }

    pub fn step_cycles(&mut self, cycles: i32) {
        if cycles > 0 {
            self.advance(cycles as u64);
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

#[derive(Clone)]
struct ApuDspState {
    blip: BlipBuf,
    last_mix_l: i32,
    last_mix_r: i32,
    next_scope_cycle: u64,
    initialized: bool,
}

fn default_dsp_state() -> ApuDspState {
    ApuDspState {
        blip: BlipBuf::new(CPU_CLOCK_HZ, SAMPLE_RATE_HZ, 4096),
        last_mix_l: 0,
        last_mix_r: 0,
        next_scope_cycle: SCOPE_INTERVAL_CYCLES,
        initialized: false,
    }
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

    /// Optional per-sample linear interpolation for DirectSound FIFOs to smooth 8-bit PWM crunch.
    #[serde(default)]
    pub fifo_interpolation: bool,

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

    /// Current CPU cycle timestamp to which PSG and mixer state are synchronized.
    #[serde(default)]
    pub last_cycle: u64,

    #[serde(skip, default = "default_dsp_state")]
    dsp: ApuDspState,
}

impl Apu {
    pub fn new() -> Self {
        let mut dsp = default_dsp_state();
        dsp.initialized = true;
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
            fifo_interpolation: false,
            channel_enable_mask: 0x3F,
            master_volume_pct: 100,
            chan_scopes: [[0; 128]; 6],
            scope_cursor: 0,
            last_cycle: 0,
            dsp,
        }
    }

    #[inline]
    fn ensure_dsp_initialized(&mut self) {
        if !self.dsp.initialized {
            self.dsp.blip.reset(self.last_cycle);
            self.dsp.last_mix_l = 0;
            self.dsp.last_mix_r = 0;
            self.dsp.next_scope_cycle = (self.last_cycle & !(SCOPE_INTERVAL_CYCLES - 1)) + SCOPE_INTERVAL_CYCLES;
            self.dsp.initialized = true;
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

    /// Synchronizes all PSG and interpolated FIFO states up to exact CPU `target_cycle`.
    pub fn sync(&mut self, target_cycle: u64) {
        self.ensure_dsp_initialized();
        if target_cycle <= self.last_cycle {
            return;
        }
        let max_chunk = self.dsp.blip.max_chunk_cycles();
        while self.last_cycle < target_cycle {
            let chunk_end = target_cycle.min(self.last_cycle + max_chunk);
            self.step_to(chunk_end);
            if self.last_cycle < target_cycle {
                self.drain_blip(self.last_cycle);
            }
        }
    }

    /// Commits any amplitude delta at `cycle` (after register writes or FIFO pops) into `BlipBuf`.
    #[inline]
    pub fn commit_mix(&mut self, cycle: u64) {
        self.ensure_dsp_initialized();
        let c = cycle.max(self.last_cycle);
        let (mix_l, mix_r) = self.compute_mixed_levels(c);
        let dl = mix_l - self.dsp.last_mix_l;
        let dr = mix_r - self.dsp.last_mix_r;
        if dl != 0 || dr != 0 {
            self.dsp.blip.add_delta(c, dl, dr);
            self.dsp.last_mix_l = mix_l;
            self.dsp.last_mix_r = mix_r;
        }
    }

    /// Returns `(dma1_refill_needed, dma2_refill_needed)` for timer overflow `timer_idx` (0 or 1) at `cycle`.
    pub fn on_timer_overflow(&mut self, timer_idx: u8, cycle: u64) -> (bool, bool) {
        self.sync(cycle);
        if !self.master_enabled() {
            return (false, false);
        }
        let timer_a = ((self.soundcnt_h >> 10) & 1) as u8;
        let timer_b = ((self.soundcnt_h >> 14) & 1) as u8;
        let mut req_a = false;
        let mut req_b = false;

        if timer_a == timer_idx {
            req_a = self.fifo_a.pop_sample_at(cycle);
        }
        if timer_b == timer_idx {
            req_b = self.fifo_b.pop_sample_at(cycle);
        }
        self.commit_mix(cycle);
        (req_a, req_b)
    }

    /// Advances audio to `cycle`, ticks the 512 Hz PSG frame sequencer, and drains ready samples.
    pub fn on_frame_sequencer_tick(&mut self, cycle: u64) {
        self.sync(cycle);
        if self.master_enabled() {
            self.tick_frame_sequencer();
            self.commit_mix(cycle);
        }
        self.drain_blip(cycle);
    }

    /// Flushes all audio up to `cycle` into `output_buffer` (called at end of frame or breakpoint).
    pub fn flush(&mut self, cycle: u64) {
        self.sync(cycle);
        self.commit_mix(cycle);
        self.drain_blip(cycle);
    }

    #[inline]
    fn sq_wakes_edges(sq: &PsgSquare, route_mask: u16, soundcnt_l: u16) -> bool {
        sq.enabled && sq.dac_enabled && sq.current_vol > 0 && (soundcnt_l & route_mask) != 0
    }

    fn step_to(&mut self, target_cycle: u64) {
        if !self.master_enabled() {
            while self.dsp.next_scope_cycle <= target_cycle {
                let cur = self.scope_cursor;
                for ch in &mut self.chan_scopes {
                    ch[cur] = 0;
                }
                self.scope_cursor = (cur + 1) & 127;
                self.dsp.next_scope_cycle += SCOPE_INTERVAL_CYCLES;
            }
            self.last_cycle = target_cycle;
            self.commit_mix(target_cycle);
            return;
        }

        let mask = self.channel_enable_mask;
        let cnt_l = self.soundcnt_l;

        while self.last_cycle < target_cycle {
            let mut next_cycle = target_cycle;

            if (mask & 0x01) != 0 && Self::sq_wakes_edges(&self.sq1, 0x1100, cnt_l) {
                if let Some(dt) = self.sq1.next_step_cycles() {
                    next_cycle = next_cycle.min(self.last_cycle + dt);
                }
            }
            if (mask & 0x02) != 0 && Self::sq_wakes_edges(&self.sq2, 0x2200, cnt_l) {
                if let Some(dt) = self.sq2.next_step_cycles() {
                    next_cycle = next_cycle.min(self.last_cycle + dt);
                }
            }
            if (mask & 0x04) != 0
                && self.wave.enabled
                && self.wave.dac_enabled
                && (self.wave.volume_code != 0 || self.wave.force_75)
                && (cnt_l & 0x4400) != 0
            {
                if let Some(dt) = self.wave.next_step_cycles() {
                    next_cycle = next_cycle.min(self.last_cycle + dt);
                }
            }
            if (mask & 0x08) != 0
                && self.noise.enabled
                && self.noise.dac_enabled
                && self.noise.current_vol > 0
                && (cnt_l & 0x8800) != 0
            {
                if let Some(dt) = self.noise.next_step_cycles() {
                    next_cycle = next_cycle.min(self.last_cycle + dt);
                }
            }

            if self.fifo_interpolation
                && (((mask & 0x10) != 0 && self.fifo_a.is_ramping(self.last_cycle))
                    || ((mask & 0x20) != 0 && self.fifo_b.is_ramping(self.last_cycle)))
            {
                let next_interp =
                    (self.last_cycle & !(FIFO_INTERP_STEP_CYCLES - 1)) + FIFO_INTERP_STEP_CYCLES;
                next_cycle = next_cycle.min(next_interp);
            }

            if self.dsp.next_scope_cycle <= target_cycle {
                next_cycle = next_cycle.min(self.dsp.next_scope_cycle);
            }

            let dt = next_cycle - self.last_cycle;
            if dt > 0 {
                self.sq1.advance(dt);
                self.sq2.advance(dt);
                self.wave.advance(dt);
                self.noise.advance(dt);
                self.last_cycle = next_cycle;
            }

            if self.last_cycle == self.dsp.next_scope_cycle {
                self.record_scopes(self.last_cycle);
                self.dsp.next_scope_cycle += SCOPE_INTERVAL_CYCLES;
            }

            self.commit_mix(self.last_cycle);
        }
    }

    fn drain_blip(&mut self, end_cycle: u64) {
        let start = self.output_buffer.len();
        let vol = if self.channel_enable_mask == 0 {
            0
        } else {
            self.master_volume_pct.min(100)
        };
        let gain_per_unit = 48.0 * (vol as f64) / 100.0;
        self.dsp
            .blip
            .end_frame_and_read(end_cycle, gain_per_unit, &mut self.output_buffer);
        if self.channel_enable_mask == 0 {
            self.output_buffer[start..].fill(0);
        }
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

    #[inline]
    fn fifo_samples(&self, cycle: u64) -> (i32, i32) {
        let mask = self.channel_enable_mask;
        let raw_a = if (mask & 0x10) != 0 {
            if self.fifo_interpolation {
                self.fifo_a.interpolated_sample(cycle)
            } else {
                self.fifo_a.current_sample as i32
            }
        } else {
            0
        };
        let raw_b = if (mask & 0x20) != 0 {
            if self.fifo_interpolation {
                self.fifo_b.interpolated_sample(cycle)
            } else {
                self.fifo_b.current_sample as i32
            }
        } else {
            0
        };
        (raw_a, raw_b)
    }

    fn record_scopes(&mut self, cycle: u64) {
        let mask = self.channel_enable_mask;
        let s1 = if (mask & 0x01) != 0 { self.sq1.sample() } else { 0 };
        let s2 = if (mask & 0x02) != 0 { self.sq2.sample() } else { 0 };
        let s3 = if (mask & 0x04) != 0 { self.wave.sample() } else { 0 };
        let s4 = if (mask & 0x08) != 0 { self.noise.sample() } else { 0 };
        let (raw_a, raw_b) = self.fifo_samples(cycle);

        let cur = self.scope_cursor;
        self.chan_scopes[0][cur] = (s1 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[1][cur] = (s2 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[2][cur] = (s3 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[3][cur] = (s4 as i32 * 2048).clamp(-32768, 32767) as i16;
        self.chan_scopes[4][cur] = (raw_a * 256).clamp(-32768, 32767) as i16;
        self.chan_scopes[5][cur] = (raw_b * 256).clamp(-32768, 32767) as i16;
        self.scope_cursor = (cur + 1) & 127;
    }

    #[inline]
    fn compute_mixed_levels(&self, cycle: u64) -> (i32, i32) {
        if !self.master_enabled() || self.channel_enable_mask == 0 {
            return (0, 0);
        }

        let mask = self.channel_enable_mask;
        let s1 = if (mask & 0x01) != 0 { self.sq1.sample() } else { 0 };
        let s2 = if (mask & 0x02) != 0 { self.sq2.sample() } else { 0 };
        let s3 = if (mask & 0x04) != 0 { self.wave.sample() } else { 0 };
        let s4 = if (mask & 0x08) != 0 { self.noise.sample() } else { 0 };

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
        psg_r = (psg_r * vol_r * 4) / 8;
        psg_l = (psg_l * vol_l * 4) / 8;

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

        let (raw_a, raw_b) = self.fifo_samples(cycle);

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

        (clamped_l, clamped_r)
    }
}
