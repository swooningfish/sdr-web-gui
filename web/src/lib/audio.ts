/** Live audio: frame parsing (layout in crates/sdrd/src/audio/mod.rs),
 * IMA ADPCM decoding, resampling and playback through an AudioWorklet. */

export type AudioMode = 'nfm' | 'wfm' | 'am' | 'usb' | 'lsb' | 'cw';

/** What the user is listening to (shared by the Listen panel and the waterfall). */
export interface Listen {
  on: boolean;
  hz: number;
  mode: AudioMode;
  /** AFC correction from the dial frequency (Hz) while AFC is following a signal. */
  afc?: number | null;
}

/** Modes AFC works in (mirrors Mode::afc_range_hz on the server). */
export const AFC_MODES: AudioMode[] = ['nfm', 'am'];

/** Where the receiver is actually listening: the dial plus any AFC correction. */
export function tunedHz(l: Listen): number {
  return l.hz + (l.afc ?? 0);
}

export const MODES: { value: AudioMode; label: string; bw: number }[] = [
  { value: 'nfm', label: 'NFM', bw: 12_500 },
  { value: 'am', label: 'AM', bw: 10_000 },
  { value: 'wfm', label: 'WFM', bw: 200_000 },
  { value: 'usb', label: 'USB', bw: 3_000 },
  { value: 'lsb', label: 'LSB', bw: 3_000 },
  { value: 'cw', label: 'CW', bw: 500 },
];

/** Beat note for CW: a carrier on the dial frequency is heard at this pitch. */
export const CW_PITCH_HZ = 700;

/** Usual channel raster per mode (Hz). */
export const CHANNEL_STEP: Record<AudioMode, number> = { nfm: 6_250, am: 1_000, wfm: 100_000, usb: 100, lsb: 100, cw: 50 };

/** Snap a clicked frequency to the usual channel raster for the mode. */
export function snap(hz: number, mode: AudioMode): number {
  const step = CHANNEL_STEP[mode];
  return Math.round(hz / step) * step;
}

/** Channel as drawn on the waterfall: [low, high] Hz around the dial frequency. */
export function channelEdges(hz: number, mode: AudioMode): [number, number] {
  const bw = MODES.find((m) => m.value === mode)!.bw;
  if (mode === 'usb') return [hz, hz + bw];
  if (mode === 'lsb') return [hz - bw, hz];
  return [hz - bw / 2, hz + bw / 2];
}

export interface AudioFrame {
  inBand: boolean;
  open: boolean;
  rate: number;
  level: number;
  seq: number;
  predictor: number;
  index: number;
  samples: number;
  data: Uint8Array;
}

export const FRAME_AUDIO = 3;

export function parseAudioFrame(buf: ArrayBuffer): AudioFrame | null {
  if (buf.byteLength < 24) return null;
  const dv = new DataView(buf);
  if (dv.getUint8(0) !== FRAME_AUDIO) return null;
  const flags = dv.getUint8(2);
  const samples = dv.getUint32(20, true);
  if (buf.byteLength < 24 + Math.ceil(samples / 2)) return null;
  return {
    inBand: (flags & 1) !== 0,
    open: (flags & 2) !== 0,
    rate: dv.getUint32(4, true),
    level: dv.getFloat32(8, true),
    seq: dv.getUint32(12, true),
    predictor: dv.getInt16(16, true),
    index: dv.getUint8(18),
    samples,
    data: new Uint8Array(buf, 24, Math.ceil(samples / 2)),
  };
}

const STEPS = [
  7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
  130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060,
  1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484,
  7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX = [-1, -1, -1, -1, 2, 4, 6, 8];

/** IMA ADPCM → float samples (±1). Mirrors audio/adpcm.rs. */
export function decodeAdpcm(f: AudioFrame): Float32Array {
  const out = new Float32Array(f.samples);
  let pred = f.predictor;
  let idx = f.index;
  for (let i = 0; i < f.samples; i++) {
    const code = (f.data[i >> 1] >> ((i & 1) * 4)) & 0x0f;
    const step = STEPS[idx];
    let delta = step >> 3;
    if (code & 4) delta += step;
    if (code & 2) delta += step >> 1;
    if (code & 1) delta += step >> 2;
    pred = code & 8 ? pred - delta : pred + delta;
    if (pred > 32767) pred = 32767;
    else if (pred < -32768) pred = -32768;
    idx += INDEX[code & 7];
    if (idx < 0) idx = 0;
    else if (idx > 88) idx = 88;
    out[i] = pred / 32768;
  }
  return out;
}

/** Streaming linear-interpolation resampler. */
class Resampler {
  private t = 0;
  private last = 0;

  reset() {
    this.t = 0;
    this.last = 0;
  }

  process(x: Float32Array, step: number): Float32Array {
    const out: number[] = [];
    while (this.t + 1 < x.length) {
      const i = Math.floor(this.t);
      const frac = this.t - i;
      const a = i < 0 ? this.last : x[i];
      const b = x[i + 1];
      out.push(a + (b - a) * frac);
      this.t += step;
    }
    this.t -= x.length;
    if (x.length) this.last = x[x.length - 1];
    return Float32Array.from(out);
  }
}

export interface PlayerStats {
  buffered: number;
  playing: boolean;
  underruns: number;
}

const TARGET = 0.25;
const MAX = 0.8;

/** FIFO with a small jitter buffer, used on the main thread when AudioWorklet
 * is unavailable. Same behaviour as public/audio-worklet.js: starts once
 * TARGET seconds are buffered, re-buffers after an underrun, and trims back to
 * TARGET if the buffer grows past MAX. */
export class JitterBuffer {
  private buf: Float32Array;
  private r = 0;
  private w = 0;
  private n = 0;
  playing = false;
  underruns = 0;

  constructor(private sampleRate: number) {
    this.buf = new Float32Array(Math.ceil(sampleRate * 4));
  }

  get buffered() {
    return this.n / this.sampleRate;
  }

  reset() {
    this.r = this.w = this.n = 0;
    this.playing = false;
  }

  push(s: Float32Array) {
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
    if (this.n > this.sampleRate * MAX) {
      const skip = this.n - Math.floor(this.sampleRate * TARGET);
      this.r = (this.r + skip) % L;
      this.n -= skip;
    }
  }

  pull(out: Float32Array) {
    const L = this.buf.length;
    if (!this.playing && this.n >= this.sampleRate * TARGET) this.playing = true;
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
  }
}

/** Where resampled audio goes: an AudioWorklet, or a ScriptProcessor fallback. */
interface Sink {
  node: AudioNode;
  push(s: Float32Array): void;
  reset(): void;
  close(): void;
}

async function workletSink(ctx: AudioContext, onstats: (s: PlayerStats) => void): Promise<Sink> {
  await ctx.audioWorklet.addModule('/audio-worklet.js');
  const node = new AudioWorkletNode(ctx, 'sdr-player', { outputChannelCount: [1] });
  node.port.onmessage = (e) => onstats(e.data as PlayerStats);
  return {
    node,
    push: (s) => node.port.postMessage({ samples: s }, [s.buffer]),
    reset: () => node.port.postMessage({ reset: true }),
    close: () => node.disconnect(),
  };
}

/** AudioWorklet is only exposed in secure contexts (HTTPS or localhost), so a
 * page opened over plain HTTP on a LAN address falls back to this. */
function scriptSink(ctx: AudioContext, onstats: (s: PlayerStats) => void): Sink {
  const fifo = new JitterBuffer(ctx.sampleRate);
  const node = ctx.createScriptProcessor(4096, 0, 1);
  let sinceReport = 0;
  node.onaudioprocess = (e) => {
    const out = e.outputBuffer.getChannelData(0);
    fifo.pull(out);
    sinceReport += out.length;
    if (sinceReport >= ctx.sampleRate / 4) {
      sinceReport = 0;
      onstats({ buffered: fifo.buffered, playing: fifo.playing, underruns: fifo.underruns });
    }
  };
  return {
    node,
    push: (s) => fifo.push(s),
    reset: () => fifo.reset(),
    close: () => {
      node.onaudioprocess = null;
      node.disconnect();
    },
  };
}

export class AudioPlayer {
  private ctx: AudioContext | null = null;
  private sink: Sink | null = null;
  private gain: GainNode | null = null;
  private analyser: AnalyserNode | null = null;
  private scratch: Float32Array<ArrayBuffer> | null = null;
  private res = new Resampler();
  private rate = 0;
  onstats: (s: PlayerStats) => void = () => {};

  /** Must be called from a user gesture (browser autoplay rules). */
  async start(volume: number) {
    if (this.ctx) return;
    const ctx = new AudioContext({ latencyHint: 'playback' });
    try {
      const onstats = (s: PlayerStats) => this.onstats(s);
      const sink = ctx.audioWorklet ? await workletSink(ctx, onstats) : scriptSink(ctx, onstats);
      const gain = ctx.createGain();
      gain.gain.value = volume;
      // The level meter reads what you hear: after the volume control.
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 2048;
      sink.node.connect(gain).connect(ctx.destination);
      gain.connect(analyser);
      await ctx.resume();
      this.ctx = ctx;
      this.sink = sink;
      this.gain = gain;
      this.analyser = analyser;
      this.scratch = new Float32Array(analyser.fftSize);
      this.rate = 0;
    } catch (e) {
      await ctx.close();
      throw e;
    }
  }

  feed(f: AudioFrame) {
    if (!this.ctx || !this.sink || f.samples === 0) return;
    if (f.rate !== this.rate) {
      // New mode or capture rate: start the buffer afresh.
      this.rate = f.rate;
      this.res.reset();
      this.sink.reset();
    }
    const pcm = decodeAdpcm(f);
    this.sink.push(this.res.process(pcm, f.rate / this.ctx.sampleRate));
  }

  /** RMS and peak of the last ~40 ms of output, in dBFS (null when stopped). */
  levels(): { rms: number; peak: number } | null {
    if (!this.analyser || !this.scratch) return null;
    this.analyser.getFloatTimeDomainData(this.scratch);
    let sum = 0;
    let peak = 0;
    for (const v of this.scratch) {
      sum += v * v;
      const a = Math.abs(v);
      if (a > peak) peak = a;
    }
    const db = (x: number) => (x > 1e-6 ? 20 * Math.log10(x) : -120);
    return { rms: db(Math.sqrt(sum / this.scratch.length)), peak: db(peak) };
  }

  setVolume(v: number) {
    if (this.gain) this.gain.gain.value = v;
  }

  async stop() {
    const ctx = this.ctx;
    this.sink?.close();
    this.ctx = null;
    this.sink = null;
    this.gain = null;
    this.analyser = null;
    this.scratch = null;
    await ctx?.close();
  }
}
