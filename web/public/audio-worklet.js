// AudioWorklet that plays live SDR audio from a FIFO with a small jitter
// buffer. Samples arrive (already at the AudioContext rate) via the port.
// Starts once TARGET seconds are buffered, re-buffers after an underrun, and
// trims back to TARGET if the buffer grows past MAX (clock drift / bursts).

const TARGET = 0.25;
const MAX = 0.8;

class SdrPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buf = new Float32Array(Math.ceil(sampleRate * 4));
    this.r = 0;
    this.w = 0;
    this.n = 0;
    this.playing = false;
    this.underruns = 0;
    this.sinceReport = 0;
    this.port.onmessage = (e) => {
      const d = e.data;
      if (d.reset) {
        this.r = this.w = this.n = 0;
        this.playing = false;
      } else if (d.samples) {
        this.push(d.samples);
      }
    };
  }

  push(s) {
    const L = this.buf.length;
    for (let i = 0; i < s.length; i++) {
      if (this.n === L) {
        this.r = (this.r + 1) % L;
        this.n--;
      }
      this.buf[this.w] = s[i];
      this.w = (this.w + 1) % L;
      this.n++;
    }
    const max = sampleRate * MAX;
    if (this.n > max) {
      const skip = this.n - Math.floor(sampleRate * TARGET);
      this.r = (this.r + skip) % L;
      this.n -= skip;
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0][0];
    const L = this.buf.length;
    if (!this.playing && this.n >= sampleRate * TARGET) this.playing = true;
    for (let i = 0; i < out.length; i++) {
      if (this.playing && this.n > 0) {
        out[i] = this.buf[this.r];
        this.r = (this.r + 1) % L;
        this.n--;
      } else {
        out[i] = 0;
        if (this.playing) {
          this.playing = false;
          this.underruns++;
        }
      }
    }
    this.sinceReport += out.length;
    if (this.sinceReport >= sampleRate / 4) {
      this.sinceReport = 0;
      this.port.postMessage({ buffered: this.n / sampleRate, playing: this.playing, underruns: this.underruns });
    }
    return true;
  }
}

registerProcessor('sdr-player', SdrPlayer);
