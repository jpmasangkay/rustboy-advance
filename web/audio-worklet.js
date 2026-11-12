/**
 * AudioWorklet processor with lock-free stereo i16 ring buffer, linear resampling
 * from GBA core's dynamic sample_rate() to AudioContext.sampleRate, and
 * Dynamic Rate Control (DRC) to keep audio latency bounded without pitch drift.
 */

const RING_CAPACITY_SAMPLES = 32768; // 16,384 stereo pairs

class GbaAudioProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.ring = new Int16Array(RING_CAPACITY_SAMPLES);
    this.readIdx = 0;
    this.writeIdx = 0;
    this.count = 0;

    this.inputRate = 32768;
    this.baseRatio = this.inputRate / sampleRate;
    this.phase = 0;
    this.curL = 0;
    this.curR = 0;
    this.nextL = 0;
    this.nextR = 0;
    this.muted = false;
    this.volume = 1.0;

    this.updateTargets();

    this.port.onmessage = ({ data }) => {
      if (!data) return;
      switch (data.type) {
        case "config": {
          const rate = Number(data.inputSampleRate);
          if (Number.isFinite(rate) && rate >= 8000) {
            this.inputRate = rate;
            this.baseRatio = this.inputRate / sampleRate;
            this.updateTargets();
          }
          break;
        }
        case "samples": {
          const samples = new Int16Array(data.buffer);
          this.pushSamples(samples);
          break;
        }
        case "clear": {
          this.readIdx = 0;
          this.writeIdx = 0;
          this.count = 0;
          this.phase = 0;
          this.curL = 0;
          this.curR = 0;
          this.nextL = 0;
          this.nextR = 0;
          break;
        }
        case "mute": {
          this.muted = Boolean(data.muted);
          break;
        }
        case "volume": {
          const vol = Number(data.volume);
          if (Number.isFinite(vol)) {
            this.volume = Math.max(0, Math.min(1, vol));
          }
          break;
        }
      }
    };
  }

  updateTargets() {
    // Target ~45ms steady-state stereo buffer (even sample count).
    this.targetFill = Math.max(512, Math.round(this.inputRate * 0.045) * 2);
    // Cap queued audio at ~200ms so fast-forward or tab resume never builds seconds of lag.
    this.maxQueued = Math.max(this.targetFill * 4, Math.round(this.inputRate * 0.2) * 2);
  }

  pushSamples(samples) {
    const len = samples.length & ~1;
    if (len === 0) return;

    // If fast-forward or stall overfills the queue, skip oldest samples down to targetFill.
    if (this.count + len > this.maxQueued) {
      const keep = Math.min(this.targetFill, RING_CAPACITY_SAMPLES - len) & ~1;
      const toDrop = Math.max(0, this.count - Math.max(0, keep)) & ~1;
      if (toDrop > 0) {
        this.readIdx = (this.readIdx + toDrop) % RING_CAPACITY_SAMPLES;
        this.count -= toDrop;
      }
    }

    const free = (RING_CAPACITY_SAMPLES - this.count) & ~1;
    const toCopy = Math.min(len, free);
    const startOffset = len - toCopy;

    for (let i = startOffset; i < len; i++) {
      this.ring[this.writeIdx] = samples[i];
      this.writeIdx = (this.writeIdx + 1) % RING_CAPACITY_SAMPLES;
    }
    this.count += toCopy;
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0) return true;

    const leftOut = output[0];
    const rightOut = output.length > 1 ? output[1] : null;
    const frames = leftOut.length;

    const fillNorm = Math.max(
      -1,
      Math.min(1, (this.count - this.targetFill) / this.targetFill),
    );
    // Smooth +/-2.5% DRC skew keeps ring buffer locked near target latency.
    const dynamicRatio = this.baseRatio * (1 + 0.025 * fillNorm);
    const gain = this.muted ? 0 : this.volume;

    for (let i = 0; i < frames; i++) {
      while (this.phase >= 1) {
        this.phase -= 1;
        this.curL = this.nextL;
        this.curR = this.nextR;
        if (this.count >= 2) {
          const l = this.ring[this.readIdx];
          const r = this.ring[(this.readIdx + 1) % RING_CAPACITY_SAMPLES];
          this.readIdx = (this.readIdx + 2) % RING_CAPACITY_SAMPLES;
          this.count -= 2;
          this.nextL = l / 32768;
          this.nextR = r / 32768;
        } else {
          // Gentle exponential fade prevents speaker clicks on brief underruns.
          this.nextL *= 0.96;
          this.nextR *= 0.96;
        }
      }

      const t = this.phase;
      this.phase += dynamicRatio;

      if (gain === 0) {
        leftOut[i] = 0;
        if (rightOut) rightOut[i] = 0;
      } else {
        const l = (this.curL + (this.nextL - this.curL) * t) * gain;
        const r = (this.curR + (this.nextR - this.curR) * t) * gain;
        if (rightOut) {
          leftOut[i] = Math.max(-1, Math.min(1, l));
          rightOut[i] = Math.max(-1, Math.min(1, r));
        } else {
          leftOut[i] = Math.max(-1, Math.min(1, (l + r) * 0.5));
        }
      }
    }

    return true;
  }
}

registerProcessor("gba-audio-processor", GbaAudioProcessor);
