use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gba_core::SAMPLE_RATE_HZ;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

const RING_CAPACITY_SAMPLES: usize = 65_536; // 32,768 stereo frames (~682ms @ 48kHz)
const TARGET_FILL_SAMPLES: usize = 4_096; // 2,048 stereo frames (~42.7ms steady-state target)
const MAX_QUEUED_SAMPLES: usize = 12_288; // Cap queued latency during fast-forward (~128ms @ 48kHz)

const SINC_TAPS: usize = 8;
const SINC_PHASES: usize = 64;

struct SincKernel {
    rows: [[f32; SINC_TAPS]; SINC_PHASES + 1],
}

impl SincKernel {
    fn new(base_ratio: f64) -> Self {
        let mut rows = [[0.0f32; SINC_TAPS]; SINC_PHASES + 1];
        // Scale low-pass cutoff down when downsampling (e.g. 48kHz -> 44.1kHz) to prevent aliasing.
        let cutoff = if base_ratio > 1.0 {
            (0.46 / base_ratio).max(0.20)
        } else {
            0.46
        };
        let pi = std::f64::consts::PI;
        let two_pi = 2.0 * pi;
        let half_taps = (SINC_TAPS / 2) as f64;

        for (p, row) in rows.iter_mut().enumerate() {
            let frac = (p as f64) / (SINC_PHASES as f64);
            let center = (half_taps - 1.0) + frac;
            let mut weights = [0.0f64; SINC_TAPS];
            let mut sum = 0.0f64;

            for (k, w_out) in weights.iter_mut().enumerate() {
                let x = (k as f64) - center;
                let r = (x + half_taps) / (SINC_TAPS as f64);
                let win = if !(0.0..=1.0).contains(&r) {
                    0.0
                } else {
                    0.42 - 0.5 * (two_pi * r).cos() + 0.08 * (2.0 * two_pi * r).cos()
                };
                let sinc = if x.abs() < 1e-12 {
                    2.0 * cutoff
                } else {
                    (two_pi * cutoff * x).sin() / (pi * x)
                };
                let val = win * sinc;
                *w_out = val;
                sum += val;
            }

            let inv_sum = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
            for (dst, &w) in row.iter_mut().zip(weights.iter()) {
                *dst = (w * inv_sum) as f32;
            }
        }

        Self { rows }
    }

    #[inline]
    fn interpolate(&self, phase: f64, hist_l: &[f32; SINC_TAPS], hist_r: &[f32; SINC_TAPS]) -> (f32, f32) {
        let scaled = phase.clamp(0.0, 0.999_999) * (SINC_PHASES as f64);
        let p = (scaled as usize).min(SINC_PHASES - 1);
        let frac = (scaled - (p as f64)) as f32;
        let r0 = &self.rows[p];
        let r1 = &self.rows[p + 1];

        let mut acc_l = 0.0f32;
        let mut acc_r = 0.0f32;
        for k in 0..SINC_TAPS {
            let w = r0[k] + (r1[k] - r0[k]) * frac;
            acc_l += hist_l[k] * w;
            acc_r += hist_r[k] * w;
        }
        (acc_l, acc_r)
    }
}

struct AudioConsumer {
    cons: HeapCons<i16>,
    muted: Arc<AtomicBool>,
    base_ratio: f64,
    avg_fill: f64,
    phase: f64,
    hist_l: [f32; SINC_TAPS],
    hist_r: [f32; SINC_TAPS],
    kernel: SincKernel,
    mute_gain: f32,
}

impl AudioConsumer {
    fn new(cons: HeapCons<i16>, muted: Arc<AtomicBool>, out_rate: f64) -> Self {
        let base_ratio = (SAMPLE_RATE_HZ as f64) / out_rate.max(8000.0);
        let start_muted = muted.load(Ordering::Relaxed);
        Self {
            cons,
            muted,
            base_ratio,
            avg_fill: TARGET_FILL_SAMPLES as f64,
            phase: 0.0,
            hist_l: [0.0; SINC_TAPS],
            hist_r: [0.0; SINC_TAPS],
            kernel: SincKernel::new(base_ratio),
            mute_gain: if start_muted { 0.0 } else { 1.0 },
        }
    }

    #[inline]
    fn advance_input_sample(&mut self) {
        self.hist_l.copy_within(1..SINC_TAPS, 0);
        self.hist_r.copy_within(1..SINC_TAPS, 0);
        if self.cons.occupied_len() >= 2 {
            if let (Some(l), Some(r)) = (self.cons.try_pop(), self.cons.try_pop()) {
                self.hist_l[SINC_TAPS - 1] = (l as f32) / 32768.0;
                self.hist_r[SINC_TAPS - 1] = (r as f32) / 32768.0;
                return;
            }
        }
        // Smooth exponential decay on underrun/pause prevents speaker pops.
        let prev_l = self.hist_l[SINC_TAPS - 2] * 0.96;
        let prev_r = self.hist_r[SINC_TAPS - 2] * 0.96;
        self.hist_l[SINC_TAPS - 1] = if prev_l.abs() < 1e-6 { 0.0 } else { prev_l };
        self.hist_r[SINC_TAPS - 1] = if prev_r.abs() < 1e-6 { 0.0 } else { prev_r };
    }

    #[inline]
    fn step_frame(&mut self) -> (f32, f32) {
        let fill = self.cons.occupied_len() as f64;
        // Smooth fill-level EMA suppresses callback-burst pitch wobble while locking latency.
        self.avg_fill += 0.0005 * (fill - self.avg_fill);
        let fill_norm = ((self.avg_fill - TARGET_FILL_SAMPLES as f64) / TARGET_FILL_SAMPLES as f64)
            .clamp(-1.0, 1.0);
        // Dynamic Rate Control (DRC): skew input step by up to +/-1.5% (covers 59.73Hz vs 60Hz cleanly).
        let r_dynamic = self.base_ratio * (1.0 + 0.015 * fill_norm);

        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.advance_input_sample();
        }

        let (l, r) = self.kernel.interpolate(self.phase, &self.hist_l, &self.hist_r);
        self.phase += r_dynamic;

        let target_gain = if self.muted.load(Ordering::Relaxed) {
            0.0f32
        } else {
            1.0f32
        };
        if (self.mute_gain - target_gain).abs() <= 0.005 {
            self.mute_gain = target_gain;
        } else if self.mute_gain < target_gain {
            self.mute_gain += 0.005;
        } else {
            self.mute_gain -= 0.005;
        }

        let out_l = (l * self.mute_gain).clamp(-1.0, 1.0);
        let out_r = (r * self.mute_gain).clamp(-1.0, 1.0);
        (out_l, out_r)
    }

    fn fill_f32(&mut self, data: &mut [f32], channels: usize) {
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.step_frame();
            if channels >= 2 {
                frame[0] = l;
                frame[1] = r;
                for ch in &mut frame[2..] {
                    *ch = 0.0;
                }
            } else if !frame.is_empty() {
                frame[0] = (l + r) * 0.5;
            }
        }
    }

    fn fill_i16(&mut self, data: &mut [i16], channels: usize) {
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.step_frame();
            let il = (l * 32767.0) as i16;
            let ir = (r * 32767.0) as i16;
            if channels >= 2 {
                frame[0] = il;
                frame[1] = ir;
                for ch in &mut frame[2..] {
                    *ch = 0;
                }
            } else if !frame.is_empty() {
                frame[0] = ((il as i32 + ir as i32) / 2) as i16;
            }
        }
    }
}

pub struct AudioOutput {
    prod: HeapProd<i16>,
    muted: Arc<AtomicBool>,
    _stream: Option<cpal::Stream>,
}

impl AudioOutput {
    pub fn start() -> Self {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (prod, cons) = rb.split();
        let muted = Arc::new(AtomicBool::new(false));
        let stream = Self::init_cpal(cons, muted.clone());
        Self {
            prod,
            muted,
            _stream: stream,
        }
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Pushes interleaved stereo i16 samples while keeping fast-forward latency bounded lock-free.
    pub fn push_interleaved(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        let occ = self.prod.occupied_len();
        if occ >= MAX_QUEUED_SAMPLES {
            return;
        }
        let vacant = self.prod.vacant_len() & !1;
        let to_push = (samples.len() & !1).min(vacant);
        if to_push > 0 {
            let _ = self.prod.push_slice(&samples[..to_push]);
        }
    }

    fn init_cpal(cons: HeapCons<i16>, muted: Arc<AtomicBool>) -> Option<cpal::Stream> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let out_rate = config.sample_rate().0 as f64;
        let channels = config.channels() as usize;
        let err_fn = |err| log::warn!("cpal output stream error: {err}");

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut consumer = AudioConsumer::new(cons, muted, out_rate);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [f32], _| consumer.fill_f32(data, channels),
                        err_fn,
                        None,
                    )
                    .ok()?
            }
            cpal::SampleFormat::I16 => {
                let mut consumer = AudioConsumer::new(cons, muted, out_rate);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [i16], _| consumer.fill_i16(data, channels),
                        err_fn,
                        None,
                    )
                    .ok()?
            }
            _ => return None,
        };

        let _ = stream.play();
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinc_kernel_preserves_unity_dc_gain() {
        for &ratio in &[0.5, 1.0, 48_000.0 / 44_100.0] {
            let kernel = SincKernel::new(ratio);
            let hist_l = [0.75f32; SINC_TAPS];
            let hist_r = [-0.25f32; SINC_TAPS];
            for step in 0..100 {
                let phase = (step as f64) / 100.0;
                let (l, r) = kernel.interpolate(phase, &hist_l, &hist_r);
                assert!((l - 0.75).abs() < 1e-5, "DC gain error L: {l}");
                assert!((r + 0.25).abs() < 1e-5, "DC gain error R: {r}");
            }
        }
    }

    #[test]
    fn resampler_tracks_sine_wave_and_fades_underrun() {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (mut prod, cons) = rb.split();
        let muted = Arc::new(AtomicBool::new(false));
        let mut consumer = AudioConsumer::new(cons, muted.clone(), 44_100.0);

        // Push 100ms of 1 kHz stereo sine wave at SAMPLE_RATE_HZ
        let total_frames = (SAMPLE_RATE_HZ as usize) / 10;
        let mut input = Vec::with_capacity(total_frames * 2);
        for i in 0..total_frames {
            let theta = 2.0 * std::f64::consts::PI * 1000.0 * (i as f64) / (SAMPLE_RATE_HZ as f64);
            let sl = (theta.sin() * 16_000.0) as i16;
            let sr = (theta.cos() * 16_000.0) as i16;
            input.push(sl);
            input.push(sr);
        }
        let _ = prod.push_slice(&input);

        let mut out = vec![0.0f32; 4096 * 2];
        consumer.fill_f32(&mut out, 2);
        let peak = out.iter().fold(0.0f32, |acc, &v| acc.max(v.abs()));
        assert!(peak > 0.45 && peak < 0.55, "unexpected resampled sine peak: {peak}");

        // Drain rest and verify underrun decays cleanly to zero without popping
        let mut tail = vec![0.0f32; 4096 * 2];
        consumer.fill_f32(&mut tail, 2);
        consumer.fill_f32(&mut tail, 2);
        assert!(
            tail.iter().rev().take(256).all(|&v| v.abs() < 1e-4),
            "underrun fade should settle cleanly near zero"
        );
    }
}
