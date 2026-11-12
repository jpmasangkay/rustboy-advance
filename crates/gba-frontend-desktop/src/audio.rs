use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gba_core::SAMPLE_RATE_HZ;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

const RING_CAPACITY_SAMPLES: usize = 8_192; // 4,096 stereo frames (~85.3ms @ 48kHz)
const TARGET_FILL_SAMPLES: usize = 4_096; // 2,048 stereo frames (~42.7ms steady-state target)

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
    fn interpolate(
        &self,
        phase: f64,
        hist_l: &[f32; SINC_TAPS],
        hist_r: &[f32; SINC_TAPS],
        hist_idx: usize,
    ) -> (f32, f32) {
        let scaled = phase.clamp(0.0, 0.999_999) * (SINC_PHASES as f64);
        let p = (scaled as usize).min(SINC_PHASES - 1);
        let frac = (scaled - (p as f64)) as f32;
        let r0 = &self.rows[p];
        let r1 = &self.rows[p + 1];

        let mut acc_l = 0.0f32;
        let mut acc_r = 0.0f32;
        for k in 0..SINC_TAPS {
            let w = r0[k] + (r1[k] - r0[k]) * frac;
            let idx = (hist_idx + k) & (SINC_TAPS - 1);
            acc_l += hist_l[idx] * w;
            acc_r += hist_r[idx] * w;
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
    hist_idx: usize,
    hist_l: [f32; SINC_TAPS],
    hist_r: [f32; SINC_TAPS],
    kernel: SincKernel,
    mute_gain: f32,
    underruns: Arc<AtomicU64>,
    occupancy: Arc<AtomicUsize>,
}

impl AudioConsumer {
    fn new(
        cons: HeapCons<i16>,
        muted: Arc<AtomicBool>,
        out_rate: f64,
        underruns: Arc<AtomicU64>,
        occupancy: Arc<AtomicUsize>,
    ) -> Self {
        let base_ratio = (SAMPLE_RATE_HZ as f64) / out_rate.max(8000.0);
        let start_muted = muted.load(Ordering::Relaxed);
        Self {
            cons,
            muted,
            base_ratio,
            avg_fill: TARGET_FILL_SAMPLES as f64,
            phase: 0.0,
            hist_idx: 0,
            hist_l: [0.0; SINC_TAPS],
            hist_r: [0.0; SINC_TAPS],
            kernel: SincKernel::new(base_ratio),
            mute_gain: if start_muted { 0.0 } else { 1.0 },
            underruns,
            occupancy,
        }
    }

    #[inline]
    fn advance_input_sample(&mut self) {
        let (val_l, val_r) = if self.cons.occupied_len() >= 2
            && let (Some(l), Some(r)) = (self.cons.try_pop(), self.cons.try_pop())
        {
            ((l as f32) / 32768.0, (r as f32) / 32768.0)
        } else {
            self.underruns.fetch_add(1, Ordering::Relaxed);
            // Smooth exponential decay on underrun (0.985 factor) prevents speaker pops.
            let newest_idx = (self.hist_idx + SINC_TAPS - 1) & (SINC_TAPS - 1);
            let prev_l = self.hist_l[newest_idx] * 0.985;
            let prev_r = self.hist_r[newest_idx] * 0.985;
            (
                if prev_l.abs() < 1e-6 { 0.0 } else { prev_l },
                if prev_r.abs() < 1e-6 { 0.0 } else { prev_r },
            )
        };
        self.hist_l[self.hist_idx] = val_l;
        self.hist_r[self.hist_idx] = val_r;
        self.hist_idx = (self.hist_idx + 1) & (SINC_TAPS - 1);
    }

    #[inline]
    fn step_frame(&mut self) -> (f32, f32) {
        let fill = self.cons.occupied_len();
        self.occupancy.store(fill, Ordering::Relaxed);
        let fill_f = fill as f64;
        // Smooth fill-level EMA suppresses callback-burst pitch wobble while locking latency.
        self.avg_fill += 0.0005 * (fill_f - self.avg_fill);
        let fill_delta = ((self.avg_fill - TARGET_FILL_SAMPLES as f64) / TARGET_FILL_SAMPLES as f64)
            .clamp(-1.0, 1.0);
        // Dynamic Rate Control (DRC): skew input step by up to +/-1.0% keeping ring buffer centered at 50%
        let drc_adjust = (fill_delta * 0.010).clamp(-0.010, 0.010);
        let r_dynamic = self.base_ratio * (1.0 + drc_adjust);

        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.advance_input_sample();
        }

        let (l, r) = self
            .kernel
            .interpolate(self.phase, &self.hist_l, &self.hist_r, self.hist_idx);
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

    fn fill_u16(&mut self, data: &mut [u16], channels: usize) {
        for frame in data.chunks_mut(channels) {
            let (l, r) = self.step_frame();
            let ul = ((l * 32767.0) as i32 + 32768).clamp(0, 65535) as u16;
            let ur = ((r * 32767.0) as i32 + 32768).clamp(0, 65535) as u16;
            if channels >= 2 {
                frame[0] = ul;
                frame[1] = ur;
                for ch in &mut frame[2..] {
                    *ch = 32768;
                }
            } else if !frame.is_empty() {
                frame[0] = ((ul as u32 + ur as u32) / 2) as u16;
            }
        }
    }
}

pub struct AudioProducer {
    prod: HeapProd<i16>,
    #[allow(dead_code)]
    has_device: bool,
}

#[allow(dead_code)]
impl AudioProducer {
    #[inline]
    pub fn occupied_len(&self) -> usize {
        self.prod.occupied_len()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        RING_CAPACITY_SAMPLES
    }

    #[inline]
    pub fn has_device(&self) -> bool {
        self.has_device
    }

    /// Pushes interleaved stereo i16 samples lock-free, filling available ring buffer capacity.
    pub fn push_interleaved(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        let vacant = self.prod.vacant_len() & !1;
        let to_push = (samples.len() & !1).min(vacant);
        if to_push > 0 {
            let _ = self.prod.push_slice(&samples[..to_push]);
        }
    }
}

pub struct AudioOutput {
    producer: Option<AudioProducer>,
    muted: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
    occupancy: Arc<AtomicUsize>,
    _stream: Option<cpal::Stream>,
    host_sample_rate: u32,
    #[allow(dead_code)]
    has_device: bool,
}

impl AudioOutput {
    pub fn start() -> Self {
        let rb = HeapRb::<i16>::new(RING_CAPACITY_SAMPLES);
        let (prod, cons) = rb.split();
        let muted = Arc::new(AtomicBool::new(false));
        let underruns = Arc::new(AtomicU64::new(0));
        let occupancy = Arc::new(AtomicUsize::new(0));
        let (stream, host_sample_rate) =
            Self::init_cpal(cons, muted.clone(), underruns.clone(), occupancy.clone());
        let has_device = stream.is_some();
        Self {
            producer: Some(AudioProducer { prod, has_device }),
            muted,
            underruns,
            occupancy,
            _stream: stream,
            host_sample_rate,
            has_device,
        }
    }

    #[allow(dead_code)]
    pub fn take_producer(&mut self) -> Option<AudioProducer> {
        self.producer.take()
    }

    pub fn host_sample_rate(&self) -> u32 {
        self.host_sample_rate
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    #[inline]
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn occupancy_percent(&self) -> f32 {
        let occ = self.occupancy.load(Ordering::Relaxed);
        (occ as f32 / RING_CAPACITY_SAMPLES as f32) * 100.0
    }

    #[inline]
    #[allow(dead_code)]
    pub fn occupied_len(&self) -> usize {
        if let Some(prod) = &self.producer {
            prod.occupied_len()
        } else {
            self.occupancy.load(Ordering::Relaxed)
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        RING_CAPACITY_SAMPLES
    }

    #[inline]
    #[allow(dead_code)]
    pub fn has_device(&self) -> bool {
        self.has_device
    }

    /// Pushes interleaved stereo i16 samples while keeping fast-forward latency bounded lock-free.
    pub fn push_interleaved(&mut self, samples: &[i16]) {
        if let Some(prod) = &mut self.producer {
            prod.push_interleaved(samples);
        }
    }

    fn init_cpal(
        cons: HeapCons<i16>,
        muted: Arc<AtomicBool>,
        underruns: Arc<AtomicU64>,
        occupancy: Arc<AtomicUsize>,
    ) -> (Option<cpal::Stream>, u32) {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return (None, 48_000);
        };
        let Ok(config) = device.default_output_config() else {
            return (None, 48_000);
        };
        let sample_rate = config.sample_rate().0;
        let out_rate = sample_rate as f64;
        let channels = config.channels() as usize;
        let err_fn = |err| log::warn!("cpal output stream error: {err}");

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut consumer =
                    AudioConsumer::new(cons, muted, out_rate, underruns, occupancy);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [f32], _| consumer.fill_f32(data, channels),
                        err_fn,
                        None,
                    )
                    .ok()
            }
            cpal::SampleFormat::I16 => {
                let mut consumer =
                    AudioConsumer::new(cons, muted, out_rate, underruns, occupancy);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [i16], _| consumer.fill_i16(data, channels),
                        err_fn,
                        None,
                    )
                    .ok()
            }
            cpal::SampleFormat::U16 => {
                let mut consumer =
                    AudioConsumer::new(cons, muted, out_rate, underruns, occupancy);
                device
                    .build_output_stream(
                        &config.into(),
                        move |data: &mut [u16], _| consumer.fill_u16(data, channels),
                        err_fn,
                        None,
                    )
                    .ok()
            }
            _ => None,
        };

        if let Some(s) = &stream {
            let _ = s.play();
        }
        (stream, sample_rate)
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
                let (l, r) = kernel.interpolate(phase, &hist_l, &hist_r, 0);
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
        let underruns = Arc::new(AtomicU64::new(0));
        let occupancy = Arc::new(AtomicUsize::new(0));
        let mut consumer = AudioConsumer::new(cons, muted.clone(), 44_100.0, underruns, occupancy);

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
