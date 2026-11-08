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
  onConnection: (connected: boolean) => void;
  onAuthLost: () => void;
}

const FRAME_WATERFALL = 1;
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
        } catch {
          /* ignore malformed */
        }
      } else {
        const f = parseFrame(ev.data as ArrayBuffer);
        if (f) this.h.onFrame(f);
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
