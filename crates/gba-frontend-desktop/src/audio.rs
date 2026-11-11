use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gba_core::SAMPLE_RATE_HZ;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

const RING_CAPACITY_SAMPLES: usize = 32_768; // 16,384 stereo frames (~500ms @ 32,768Hz)
const TARGET_FILL_SAMPLES: usize = 2_048; // ~31ms steady-state target
const MAX_QUEUED_SAMPLES: usize = 8_192; // Cap queued latency during fast-forward (~125ms)

struct AudioConsumer {
    cons: HeapCons<i16>,
    muted: Arc<AtomicBool>,
    base_ratio: f64,
    phase: f64,
    cur_l: f32,
    cur_r: f32,
    next_l: f32,
    next_r: f32,
}

impl AudioConsumer {
    fn new(cons: HeapCons<i16>, muted: Arc<AtomicBool>, out_rate: f64) -> Self {
        Self {
            cons,
            muted,
            base_ratio: (SAMPLE_RATE_HZ as f64) / out_rate.max(8000.0),
            phase: 0.0,
            cur_l: 0.0,
            cur_r: 0.0,
            next_l: 0.0,
            next_r: 0.0,
        }
    }

    #[inline]
    fn step_frame(&mut self) -> (f32, f32) {
        let fill = self.cons.occupied_len() as f64;
        let fill_norm = ((fill - TARGET_FILL_SAMPLES as f64) / TARGET_FILL_SAMPLES as f64)
            .clamp(-1.0, 1.0);
        // Dynamic Rate Control (DRC): skew input step by up to +/-2.5% to keep latency locked.
        let r_dynamic = self.base_ratio * (1.0 + 0.025 * fill_norm);

        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.cur_l = self.next_l;
            self.cur_r = self.next_r;
            if self.cons.occupied_len() >= 2 {
                if let (Some(l), Some(r)) = (self.cons.try_pop(), self.cons.try_pop()) {
                    self.next_l = (l as f32) / 32768.0;
                    self.next_r = (r as f32) / 32768.0;
                }
            } else {
                // Smooth exponential decay on underrun/pause prevents speaker pops.
                self.next_l *= 0.96;
                self.next_r *= 0.96;
            }
        }

        let t = self.phase as f32;
        self.phase += r_dynamic;

        if self.muted.load(Ordering::Relaxed) {
            return (0.0, 0.0);
        }

        let l = self.cur_l + (self.next_l - self.cur_l) * t;
        let r = self.cur_r + (self.next_r - self.cur_r) * t;
        (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
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
