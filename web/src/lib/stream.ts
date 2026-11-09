import { FRAME_AUDIO, parseAudioFrame, type AudioFrame } from './audio';
import { api, ApiError, type Device, type EventRow, type RadioState } from './api';

export interface WaterfallFrame {
  bins: Uint8Array;
  sampleRate: number;
  centerHz: number;
  minDb: number;
  maxDb: number;
  ts: number;
}

export interface StreamHandlers {
  onFrame: (f: WaterfallFrame) => void;
  onRadio: (s: RadioState) => void;
  onEvent: (device: Device, event: EventRow) => void;
  onSstvRows: (f: SstvRows) => void;
  onAudio: (f: AudioFrame) => void;
  /** audio_started / audio_stopped / audio_error replies. */
  onAudioStatus: (kind: string, message: string | null) => void;
  /** Decoded Morse (`cw`) or RDS (`rds`) for our listener. */
  onDecoded: (msg: DecodedMsg) => void;
  onConnection: (connected: boolean) => void;
  onAuthLost: () => void;
}

export interface CwMsg {
  type: 'cw';
  /** Characters decoded since the last message. */
  text: string;
  wpm: number;
}

export interface RdsMsg {
  type: 'rds';
  pi: string | null;
  ps: string | null;
  pty: number | null;
  rt: string | null;
  groups: number;
}

export type DecodedMsg = CwMsg | RdsMsg;

/** Live SSTV rows (layout documented in decoders/sstv.rs). */
export interface SstvRows {
  channel: number;
  vis: number;
  width: number;
  height: number;
  firstLine: number;
  lines: number;
  session: number;
  rgb: Uint8Array;
}

const FRAME_WATERFALL = 1;
const FRAME_SSTV_ROWS = 2;

export function parseSstvRows(buf: ArrayBuffer): SstvRows | null {
  if (buf.byteLength < 16) return null;
  const dv = new DataView(buf);
  if (dv.getUint8(0) !== FRAME_SSTV_ROWS) return null;
  const width = dv.getUint16(4, true);
  const lines = dv.getUint16(10, true);
  if (buf.byteLength < 16 + width * lines * 3) return null;
  return {
    channel: dv.getUint8(2),
    vis: dv.getUint8(3),
    width,
    height: dv.getUint16(6, true),
    firstLine: dv.getUint16(8, true),
    lines,
    session: dv.getUint32(12, true),
    rgb: new Uint8Array(buf, 16, width * lines * 3),
  };
}
const HEADER_LEN = 32;

/** Parse a binary frame from sdrd (layout documented in dsp/waterfall.rs). */
export function parseFrame(buf: ArrayBuffer): WaterfallFrame | null {
  if (buf.byteLength < HEADER_LEN) return null;
  const dv = new DataView(buf);
  if (dv.getUint8(0) !== FRAME_WATERFALL) return null;
  const n = dv.getUint16(2, true);
  if (buf.byteLength < HEADER_LEN + n) return null;
  return {
    bins: new Uint8Array(buf, HEADER_LEN, n),
    sampleRate: dv.getUint32(4, true),
    centerHz: Number(dv.getBigUint64(8, true)),
    minDb: dv.getFloat32(16, true),
    maxDb: dv.getFloat32(20, true),
    ts: Number(dv.getBigUint64(24, true)),
  };
}

/** WebSocket to /api/ws with reconnect + backoff. */
export class Stream {
  private ws: WebSocket | null = null;
  private stopped = false;
  private retryMs = 1000;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private h: StreamHandlers) {}

  start() {
    this.stopped = false;
    this.connect();
  }

  stop() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.ws?.close();
    this.ws = null;
  }

  /** Send a JSON control message (dropped while disconnected). */
  send(msg: unknown) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  private connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/api/ws`);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onopen = () => {
      this.retryMs = 1000;
      this.h.onConnection(true);
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        try {
          const m = JSON.parse(ev.data);
          if (m.type === 'radio') this.h.onRadio(m.state as RadioState);
          else if (m.type === 'event') this.h.onEvent(m.device as Device, m.event as EventRow);
          else if (typeof m.type === 'string' && m.type.startsWith('audio_')) this.h.onAudioStatus(m.type, m.message ?? null);
          else if (m.type === 'cw' || m.type === 'rds') this.h.onDecoded(m as DecodedMsg);
        } catch {
          /* ignore malformed */
        }
      } else {
        const buf = ev.data as ArrayBuffer;
        const kind = buf.byteLength ? new DataView(buf).getUint8(0) : 0;
        if (kind === FRAME_AUDIO) {
          const a = parseAudioFrame(buf);
          if (a) this.h.onAudio(a);
        } else if (kind === FRAME_SSTV_ROWS) {
          const r = parseSstvRows(buf);
          if (r) this.h.onSstvRows(r);
        } else {
          const f = parseFrame(buf);
          if (f) this.h.onFrame(f);
        }
      }
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.h.onConnection(false);
      if (this.stopped) return;
      this.timer = setTimeout(() => this.reconnect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 15000);
    };
  }

  private async reconnect() {
    if (this.stopped) return;
    try {
      await api.me(); // still logged in?
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        this.h.onAuthLost();
        return;
      }
    }
    if (!this.stopped) this.connect();
  }
}
