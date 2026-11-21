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

    this.inputRate = 48000;
    this.speed = 1.0;
    this.baseRatio = this.inputRate / sampleRate;
    this.phase = 0;
    this.curL = 0;
    this.curR = 0;
    this.nextL = 0;
    this.nextR = 0;
    this.muted = false;
    this.volume = 1.0;
    this.primed = false;
    this.avgFill = 0;

    this.updateTargets();
    this.avgFill = this.targetFill;

    this.port.onmessage = ({ data }) => {
      if (!data) return;
      switch (data.type) {
        case "config": {
          const rate = Number(data.inputSampleRate);
          if (Number.isFinite(rate) && rate >= 8000) {
            this.inputRate = rate;
            this.baseRatio = (this.inputRate * this.speed) / sampleRate;
            this.updateTargets();
          }
          break;
        }
        case "speed": {
          const spd = Number(data.speed);
          if (Number.isFinite(spd) && spd > 0) {
            this.speed = Math.max(0.25, Math.min(8.0, spd));
            this.baseRatio = (this.inputRate * this.speed) / sampleRate;
            this.updateTargets();
          }
          break;
        }
        case "samples": {
          const maxElems = data.buffer.byteLength >> 1;
          const rawLen = Number.isInteger(data.length) ? data.length : maxElems;
          const length = Math.max(0, Math.min(rawLen, maxElems));
          const samples = new Int16Array(data.buffer, 0, length);
          this.pushSamples(samples);
          // Return ArrayBuffer to main thread pool to eliminate per-frame GC churn.
          this.port.postMessage({ type: "recycle", buffer: data.buffer }, [data.buffer]);
          break;
        }
        case "clear": {
          this.readIdx = 0;
          this.writeIdx = 0;
          this.count = 0;
          this.phase = 0;
          this.primed = false;
          this.avgFill = this.targetFill;
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
    const effRate = this.inputRate * this.speed;
    // Target ~45ms steady-state stereo buffer (even sample count), clamped to ring capacity.
    this.targetFill = Math.min(
      RING_CAPACITY_SAMPLES >> 2,
      Math.max(512, Math.round(effRate * 0.045) * 2),
    ) & ~1;
    // Cap queued audio at ~200ms so fast-forward or tab resume never builds seconds of lag.
    this.maxQueued = Math.min(
      RING_CAPACITY_SAMPLES - 512,
      Math.max(this.targetFill * 4, Math.round(effRate * 0.2) * 2),
    ) & ~1;
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

    if (toCopy > 0) {
      const firstChunk = Math.min(toCopy, RING_CAPACITY_SAMPLES - this.writeIdx);
      this.ring.set(samples.subarray(startOffset, startOffset + firstChunk), this.writeIdx);
      if (firstChunk < toCopy) {
        this.ring.set(
          samples.subarray(startOffset + firstChunk, startOffset + toCopy),
          0,
        );
      }
      this.writeIdx = (this.writeIdx + toCopy) % RING_CAPACITY_SAMPLES;
      this.count += toCopy;
    }
    if (!this.primed && this.count >= this.targetFill) {
      this.primed = true;
      this.avgFill = this.count;
    }
  }

  process(_inputs, outputs) {
    const output = outputs[0];
    if (!output || output.length === 0) return true;

    const leftOut = output[0];
    const rightOut = output.length > 1 ? output[1] : null;
    const frames = leftOut.length;

    if (!this.primed) {
      // Smoothly decay any residual sample to zero to avoid pops after reset/clear.
      for (let i = 0; i < frames; i++) {
        this.curL *= 0.94;
        this.curR *= 0.94;
        if (Math.abs(this.curL) < 1e-6) this.curL = 0;
        if (Math.abs(this.curR) < 1e-6) this.curR = 0;
        leftOut[i] = rightOut ? this.curL : (this.curL + this.curR) * 0.5;
        if (rightOut) rightOut[i] = this.curR;
      }
      this.nextL = this.curL;
      this.nextR = this.curR;
      return true;
    }

    // Smooth fill-level EMA suppresses 60 Hz callback-burst pitch warble.
    this.avgFill += 0.04 * (this.count - this.avgFill);
    const fillNorm = Math.max(
      -1,
      Math.min(1, (this.avgFill - this.targetFill) / this.targetFill),
    );
    // Gentle +/-0.5% DRC skew locks ring buffer latency without audible pitch wobble.
    const dynamicRatio = this.baseRatio * (1 + 0.005 * fillNorm);
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
          if (Math.abs(this.nextL) < 1e-6) this.nextL = 0;
          if (Math.abs(this.nextR) < 1e-6) this.nextR = 0;
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
