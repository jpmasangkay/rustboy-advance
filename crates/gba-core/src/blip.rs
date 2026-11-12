//! Pure `no_std` band-limited audio synthesis buffer (inspired by blargg's `blip_buf`).
//!
//! Records amplitude transitions at exact 16.777 MHz CPU-cycle timestamps using a
//! compile-time precomputed windowed-sinc step response table, then integrates and
//! high-pass filters them out at a fixed output sample rate.

use alloc::vec;
use alloc::vec::Vec;

const KERNEL_WIDTH: usize = 32;
const HALF_WIDTH: usize = KERNEL_WIDTH / 2;
const NUM_PHASES: usize = 64;
const STEP_SCALE: i32 = 1 << 16; // 65_536
const Q_SHIFT: i32 = 12; // 4_096 fractional units below 1 GBA mixer unit
const KERNEL_CUTOFF: f64 = 0.425;
const KAISER_BETA: f64 = 7.0;
const DC_BLOCKER_CUTOFF_HZ: f64 = 16.0;

const PI: f64 = core::f64::consts::PI;
const TWO_PI: f64 = 2.0 * PI;
const HALF_PI: f64 = 0.5 * PI;

/// Compile-time cosine on arbitrary finite `f64` using range reduction + 12-term Taylor series.
const fn cos_f64(mut x: f64) -> f64 {
    if x < 0.0 {
        x = -x;
    }
    let k = (x / TWO_PI) as u64;
    x -= (k as f64) * TWO_PI;
    if x > PI {
        x -= TWO_PI;
    }
    if x < 0.0 {
        x = -x;
    }

    let mut sign = 1.0;
    if x > HALF_PI {
        x = PI - x;
        sign = -1.0;
    }

    let x2 = x * x;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 1usize;
    while n <= 12 {
        let denom = ((2 * n - 1) * (2 * n)) as f64;
        term *= -x2 / denom;
        sum += term;
        n += 1;
    }
    sign * sum
}

const fn sin_f64(x: f64) -> f64 {
    cos_f64(x - HALF_PI)
}

const fn exp_f64(x: f64) -> f64 {
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 1usize;
    while n <= 24 {
        term *= x / (n as f64);
        sum += term;
        n += 1;
    }
    sum
}

const fn bessel_i0(x: f64) -> f64 {
    let half_x = 0.5 * x;
    let half_x2 = half_x * half_x;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1usize;
    while k <= 20 {
        let kf = k as f64;
        term *= half_x2 / (kf * kf);
        sum += term;
        k += 1;
    }
    sum
}

const fn sqrt_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = if x > 1.0 { x * 0.5 } else { 1.0 };
    let mut i = 0;
    while i < 32 {
        g = 0.5 * (g + x / g);
        i += 1;
    }
    g
}

/// Generates a 65-phase x 32-tap normalized windowed-sinc step response LUT at compile time.
/// Uses a Kaiser window (beta = 7.0) for sharp transition and >70 dB stopband rejection above Nyquist.
const fn build_step_table() -> [[i32; KERNEL_WIDTH]; NUM_PHASES + 1] {
    let mut table = [[0i32; KERNEL_WIDTH]; NUM_PHASES + 1];
    let inv_i0_beta = 1.0 / bessel_i0(KAISER_BETA);
    let mut p = 0usize;
    while p <= NUM_PHASES {
        let frac = (p as f64) / (NUM_PHASES as f64);
        let center = (HALF_WIDTH as f64 - 1.0) + frac;

        let mut imp = [0.0f64; KERNEL_WIDTH];
        let mut sum = 0.0f64;
        let mut k = 0usize;
        while k < KERNEL_WIDTH {
            let x = (k as f64) - center;
            let r = x / (HALF_WIDTH as f64);
            let w = if r <= -1.0 || r >= 1.0 {
                0.0
            } else {
                bessel_i0(KAISER_BETA * sqrt_f64(1.0 - r * r)) * inv_i0_beta
            };

            let sinc = if x > -1e-12 && x < 1e-12 {
                2.0 * KERNEL_CUTOFF
            } else {
                sin_f64(TWO_PI * KERNEL_CUTOFF * x) / (PI * x)
            };

            let v = w * sinc;
            imp[k] = v;
            sum += v;
            k += 1;
        }

        let inv_sum = 1.0 / sum;
        let mut acc = 0.0f64;
        let mut k = 0usize;
        while k < KERNEL_WIDTH - 1 {
            acc += imp[k] * inv_sum;
            let scaled = acc * (STEP_SCALE as f64);
            table[p][k] = if scaled >= 0.0 {
                (scaled + 0.5) as i32
            } else {
                (scaled - 0.5) as i32
            };
            k += 1;
        }
        table[p][KERNEL_WIDTH - 1] = STEP_SCALE;
        p += 1;
    }
    table
}

static STEP_TABLE: [[i32; KERNEL_WIDTH]; NUM_PHASES + 1] = build_step_table();

#[inline]
fn round_to_i16(v: f64) -> i16 {
    let r = if v >= 0.0 { v + 0.5 } else { v - 0.5 };
    (r as i32).clamp(-32768, 32767) as i16
}

/// Stereo band-limited step buffer clocked at `clock_rate` Hz and drained at `sample_rate` Hz.
#[derive(Clone)]
pub struct BlipBuf {
    clock_rate: u32,
    sample_rate: u32,
    factor: u64, // Q32 output samples per input clock cycle
    offset: u64, // Q32 sub-sample phase offset within current window
    base_cycle: u64,
    buf_l: Vec<i32>,
    buf_r: Vec<i32>,
    integrator_l: i64,
    integrator_r: i64,
    dc_cap_l: f64,
    dc_cap_r: f64,
    dc_alpha: f64,
}

impl BlipBuf {
    /// Creates a new stereo `BlipBuf` with capacity for at least `max_samples` buffered output frames.
    pub fn new(clock_rate: u32, sample_rate: u32, max_samples: usize) -> Self {
        let cap = max_samples.max(256) + KERNEL_WIDTH + 16;
        let factor = ((((sample_rate as u128) << 32) + (clock_rate as u128 / 2))
            / (clock_rate as u128)) as u64;
        let dc_alpha = 1.0 - exp_f64(-TWO_PI * DC_BLOCKER_CUTOFF_HZ / (sample_rate.max(1) as f64));
        Self {
            clock_rate,
            sample_rate,
            factor,
            offset: 0,
            base_cycle: 0,
            buf_l: vec![0; cap],
            buf_r: vec![0; cap],
            integrator_l: 0,
            integrator_r: 0,
            dc_cap_l: 0.0,
            dc_cap_r: 0.0,
            dc_alpha,
        }
    }

    #[inline]
    pub fn clock_rate(&self) -> u32 {
        self.clock_rate
    }

    #[inline]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the maximum number of CPU cycles that can safely accumulate before draining.
    #[inline]
    pub fn max_chunk_cycles(&self) -> u64 {
        let safe_samples = self.buf_l.len().saturating_sub(KERNEL_WIDTH + 4) as u64;
        ((safe_samples << 32) / self.factor.max(1)).max(1)
    }

    /// Clears all buffered impulses, integrator state, and DC-blocker state, anchoring at `base_cycle`.
    pub fn reset(&mut self, base_cycle: u64) {
        self.offset = 0;
        self.base_cycle = base_cycle;
        self.buf_l.fill(0);
        self.buf_r.fill(0);
        self.integrator_l = 0;
        self.integrator_r = 0;
        self.dc_cap_l = 0.0;
        self.dc_cap_r = 0.0;
    }

    /// Records an amplitude delta `(delta_l, delta_r)` at the exact CPU cycle `cycle`.
    #[inline]
    pub fn add_delta(&mut self, cycle: u64, delta_l: i32, delta_r: i32) {
        if delta_l == 0 && delta_r == 0 {
            return;
        }
        let rel = cycle.saturating_sub(self.base_cycle);
        let fixed = self.offset + rel * self.factor;
        let idx = (fixed >> 32) as usize;
        if idx + KERNEL_WIDTH > self.buf_l.len() {
            return;
        }

        // Top 6 bits select [0..64) phase row; next 8 bits interpolate toward row + 1.
        let sub = ((fixed >> 18) & 0x3FFF) as usize;
        let phase = sub >> 8;
        let frac = (sub & 0xFF) as i32;
        let t0 = &STEP_TABLE[phase];
        let t1 = &STEP_TABLE[phase + 1];

        if delta_l != 0 && delta_r != 0 {
            let mut prev_l = 0i32;
            let mut prev_r = 0i32;
            for k in 0..(KERNEL_WIDTH - 1) {
                let s = t0[k] + (((t1[k] - t0[k]) * frac) >> 8);
                let cur_l = (delta_l * s) >> 4;
                let cur_r = (delta_r * s) >> 4;
                self.buf_l[idx + k] += cur_l - prev_l;
                self.buf_r[idx + k] += cur_r - prev_r;
                prev_l = cur_l;
                prev_r = cur_r;
            }
            self.buf_l[idx + KERNEL_WIDTH - 1] += (delta_l << Q_SHIFT) - prev_l;
            self.buf_r[idx + KERNEL_WIDTH - 1] += (delta_r << Q_SHIFT) - prev_r;
        } else if delta_l != 0 {
            let mut prev_l = 0i32;
            for k in 0..(KERNEL_WIDTH - 1) {
                let s = t0[k] + (((t1[k] - t0[k]) * frac) >> 8);
                let cur_l = (delta_l * s) >> 4;
                self.buf_l[idx + k] += cur_l - prev_l;
                prev_l = cur_l;
            }
            self.buf_l[idx + KERNEL_WIDTH - 1] += (delta_l << Q_SHIFT) - prev_l;
        } else {
            let mut prev_r = 0i32;
            for k in 0..(KERNEL_WIDTH - 1) {
                let s = t0[k] + (((t1[k] - t0[k]) * frac) >> 8);
                let cur_r = (delta_r * s) >> 4;
                self.buf_r[idx + k] += cur_r - prev_r;
                prev_r = cur_r;
            }
            self.buf_r[idx + KERNEL_WIDTH - 1] += (delta_r << Q_SHIFT) - prev_r;
        }
    }

    /// Advances the buffer clock to `end_cycle`, drains all completed stereo samples through
    /// the DC-blocking high-pass filter, and appends interleaved `i16` pairs to `out`.
    /// Returns the number of stereo sample frames appended.
    pub fn end_frame_and_read(
        &mut self,
        end_cycle: u64,
        gain_per_unit: f64,
        out: &mut Vec<i16>,
    ) -> usize {
        if end_cycle > self.base_cycle {
            let elapsed = end_cycle - self.base_cycle;
            self.offset += elapsed * self.factor;
            self.base_cycle = end_cycle;
        }

        let avail = ((self.offset >> 32) as usize).min(self.buf_l.len().saturating_sub(KERNEL_WIDTH));
        if avail == 0 {
            return 0;
        }
        self.offset -= (avail as u64) << 32;

        let scale = gain_per_unit / ((1i32 << Q_SHIFT) as f64);
        out.reserve(avail * 2);

        for i in 0..avail {
            self.integrator_l += self.buf_l[i] as i64;
            self.integrator_r += self.buf_r[i] as i64;

            let x_l = self.integrator_l as f64;
            let x_r = self.integrator_r as f64;

            let hp_l = x_l - self.dc_cap_l;
            let hp_r = x_r - self.dc_cap_r;
            self.dc_cap_l += hp_l * self.dc_alpha;
            self.dc_cap_r += hp_r * self.dc_alpha;

            if self.dc_cap_l.abs() < 1e-9 {
                self.dc_cap_l = 0.0;
            }
            if self.dc_cap_r.abs() < 1e-9 {
                self.dc_cap_r = 0.0;
            }

            out.push(round_to_i16(hp_l * scale));
            out.push(round_to_i16(hp_r * scale));
        }

        // Preserve the overlap tail of any recent impulse kernels and clear drained slots.
        let clear_end = (avail + KERNEL_WIDTH).min(self.buf_l.len());
        self.buf_l.copy_within(avail..clear_end, 0);
        self.buf_r.copy_within(avail..clear_end, 0);
        let tail_len = clear_end - avail;
        self.buf_l[tail_len..clear_end].fill(0);
        self.buf_r[tail_len..clear_end].fill(0);

        avail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trig_and_step_table_sanity() {
        assert!((cos_f64(0.0) - 1.0).abs() < 1e-14);
        assert!(cos_f64(HALF_PI).abs() < 1e-14);
        assert!((cos_f64(PI) + 1.0).abs() < 1e-14);
        assert!((sin_f64(HALF_PI) - 1.0).abs() < 1e-14);
        assert!((sin_f64(-HALF_PI) + 1.0).abs() < 1e-14);

        for row in &STEP_TABLE {
            assert_eq!(row[KERNEL_WIDTH - 1], STEP_SCALE);
        }
    }

    #[test]
    fn step_telescopes_and_dc_blocks_cleanly() {
        let mut blip = BlipBuf::new(16_777_216, 48_000, 4096);
        // Step up by +100 at cycle 1000
        blip.add_delta(1000, 100, -50);
        let mut out = Vec::new();
        // Drain 100,000 cycles (~286 samples), integrator must hold exact +100 / -50 in Q12
        blip.end_frame_and_read(100_000, 48.0, &mut out);
        assert_eq!(blip.integrator_l, 100 << Q_SHIFT);
        assert_eq!(blip.integrator_r, -50 << Q_SHIFT);

        // Continue silence for ~300 ms so DC blocker decays to exact zero
        let mut cyc = 100_000u64;
        for _ in 0..20 {
            out.clear();
            cyc += 280_896;
            blip.end_frame_and_read(cyc, 48.0, &mut out);
        }
        assert!(
            out.iter().all(|&s| s == 0),
            "DC blocker should settle to exact zero after ~330 ms of constant level"
        );
    }

    #[test]
    fn high_freq_square_wave_suppresses_nyquist_aliases() {
        let mut blip = BlipBuf::new(16_777_216, 48_000, 4096);
        // Square wave at 8,738.13 Hz (half-period = 960 cycles, period = 1920 cycles).
        // Fundamental is 8,738 Hz (< 24 kHz Nyquist). Odd harmonics 3f=26.2k, 5f=43.7k, 7f=61.2k
        // would alias strongly to 21.8k, 4.3k, 13.2k without band-limiting.
        let mut out = Vec::new();
        let mut cur_val = 60i32;
        blip.add_delta(0, cur_val, cur_val);
        let mut next_edge = 960u64;
        let mut cyc = 0u64;

        // Run ~120 ms (8 frames) so DC blocker settles, then capture 2048 stereo samples
        for frame in 0..12 {
            let end = cyc + 280_896;
            while next_edge <= end {
                let new_val = -cur_val;
                blip.add_delta(next_edge, new_val - cur_val, new_val - cur_val);
                cur_val = new_val;
                next_edge += 960;
            }
            if frame < 8 {
                out.clear();
            }
            blip.end_frame_and_read(end, 48.0, &mut out);
            cyc = end;
        }

        let n = 2048usize;
        let mono: Vec<f64> = out.chunks_exact(2).take(n).map(|c| c[0] as f64).collect();
        assert_eq!(mono.len(), n);

        // Compute 4-term Blackman-Harris windowed DFT magnitude squared for bins 4..n/2
        let mut mags = Vec::with_capacity(n / 2);
        let mut peak_bin = 4usize;
        let mut peak_mag = 0.0f64;
        for k in 4..(n / 2) {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (i, &x) in mono.iter().enumerate() {
                let u = (i as f64) / ((n - 1) as f64);
                let win = 0.355768
                    - 0.487396 * cos_f64(TWO_PI * u)
                    + 0.144232 * cos_f64(2.0 * TWO_PI * u)
                    - 0.012604 * cos_f64(3.0 * TWO_PI * u);
                let angle = TWO_PI * (k as f64) * (i as f64) / (n as f64);
                re += x * win * cos_f64(angle);
                im -= x * win * sin_f64(angle);
            }
            let mag = re * re + im * im;
            if mag > peak_mag {
                peak_mag = mag;
                peak_bin = k;
            }
            mags.push((k, mag));
        }

        // Fundamental 8738.13 Hz at 48000 Hz, N=2048 -> bin ~ 8738.13 * 2048 / 48000 = 372.8
        assert!(
            (371..=375).contains(&peak_bin),
            "expected fundamental peak near bin 373, got {peak_bin}"
        );

        // Exclude fundamental main lobe (+/- 5 bins for 4-term Nuttall) and verify all other bins < -60 dB
        let mut worst_bin = 0usize;
        let mut max_alias_mag = 0.0f64;
        for &(k, m) in &mags {
            if k.abs_diff(peak_bin) > 5 && m > max_alias_mag {
                max_alias_mag = m;
                worst_bin = k;
            }
        }

        let ratio = max_alias_mag / peak_mag;
        assert!(
            ratio < 1e-6, // -60 dB in power
            "aliased spur too strong: power ratio={ratio:.3e} at bin={worst_bin}, peak_bin={peak_bin}"
        );
    }
}
