/** The receiver's listening side: audio playback, demodulator settings, the
 * two VFOs and what the server decodes for this listener. One per signed-in
 * session; the front panel drives it and the decoder strip shows its output. */

import { AFC_MODES, AudioPlayer, type AudioFrame, type AudioMode, type Listen, type PlayerStats } from './audio';
import type { DecodedMsg, DigitalCall, DigitalEvent, RdsMsg } from './stream';

export type VfoName = 'A' | 'B';
export interface Vfo {
  hz: number;
  mode: AudioMode;
}

/** RDS programme types (European list). */
export const PTY = [
  'No programme type', 'News', 'Current affairs', 'Information', 'Sport', 'Education', 'Drama', 'Culture',
  'Science', 'Varied', 'Pop music', 'Rock music', 'Easy listening', 'Light classical', 'Serious classical',
  'Other music', 'Weather', 'Finance', "Children's", 'Social affairs', 'Religion', 'Phone-in', 'Travel',
  'Leisure', 'Jazz', 'Country', 'National music', 'Oldies', 'Folk', 'Documentary', 'Alarm test', 'Alarm',
];

const PREFS = 'sdr.listen';
/** dsd-neo gives the odd false sync on noise: a mode counts only after
 * SYNC_RUN syncs in a row within SYNC_GAP_MS of each other. */
const SYNC_RUN = 3;
const SYNC_GAP_MS = 500;

export class Listener {
  // ---- settings (remembered per browser)
  squelchOn = $state(false);
  squelch = $state(-60);
  volume = $state(0.8);
  muted = $state(false);
  nr = $state(false);
  cwOn = $state(false);
  afcOn = $state(false);
  digitalOn = $state(false);
  consoleOn = $state(false);
  /** Clicking the spectrum picks the mode the band plan suggests. */
  autoMode = $state(true);

  // ---- VFOs: `listen` is the one heard; the other is kept here.
  active = $state<VfoName>('A');
  other = $state<Vfo>({ hz: 145_500_000, mode: 'nfm' });

  // ---- live state
  level = $state<number | null>(null);
  inBand = $state(true);
  open = $state(true);
  stats = $state<PlayerStats | null>(null);
  kbps = $state(0);
  err = $state<string | null>(null);
  starting = $state(false);
  afcLocked = $state(false);

  // ---- decoded
  cwText = $state('');
  cwWpm = $state<number | null>(null);
  rds = $state<RdsMsg | null>(null);
  onAir = $state<DigitalCall[]>([]);
  heard = $state<(DigitalEvent & { hz: number })[]>([]);
  dsdLog = $state<string[]>([]);
  dsdErr = $state<string | null>(null);
  sync = $state<{ mode: string; at: number } | null>(null);
  now = $state(Date.now());

  readonly listen: Listen;
  private send: (msg: unknown) => void;
  private player = new AudioPlayer();
  private bytes = 0;
  private t0 = performance.now();
  private syncRun = { mode: '', n: 0, at: 0 };

  constructor(listen: Listen, send: (msg: unknown) => void) {
    this.listen = listen;
    this.send = send;
    this.player.onstats = (s) => (this.stats = s);
    let saved: Record<string, unknown> = {};
    try {
      saved = JSON.parse(localStorage.getItem(PREFS) ?? '{}');
    } catch {
      /* private mode */
    }
    const bool = (k: string, d: boolean) => (typeof saved[k] === 'boolean' ? (saved[k] as boolean) : d);
    const num = (k: string, d: number) => (typeof saved[k] === 'number' ? (saved[k] as number) : d);
    this.squelchOn = bool('squelchOn', false);
    this.squelch = num('squelch', -60);
    this.volume = num('volume', 0.8);
    this.nr = bool('nr', false);
    this.cwOn = bool('cw', false);
    this.afcOn = bool('afc', false);
    this.digitalOn = bool('digital', false);
    this.consoleOn = bool('dsdConsole', false);
    this.autoMode = bool('autoMode', true);
    if (typeof saved.hz === 'number') listen.hz = saved.hz;
    if (typeof saved.mode === 'string') listen.mode = saved.mode as AudioMode;
    if (saved.active === 'B') this.active = 'B';
    const o = saved.other as Vfo | undefined;
    if (o && typeof o.hz === 'number' && typeof o.mode === 'string') this.other = { hz: o.hz, mode: o.mode };
  }

  get afcWorks() {
    return AFC_MODES.includes(this.listen.mode);
  }
  get afcActive() {
    return this.afcOn && this.afcWorks;
  }
  get cwActive() {
    return this.cwOn && this.listen.mode !== 'wfm';
  }
  get digitalActive() {
    return this.digitalOn && this.listen.mode === 'nfm';
  }
  get syncFresh() {
    return !!this.sync && this.now - this.sync.at < 1500;
  }

  /** VFO A and B as they stand (one of them is `listen`). */
  vfo(name: VfoName): Vfo {
    return name === this.active ? { hz: this.listen.hz, mode: this.listen.mode } : this.other;
  }

  /** A/B: hear the other VFO. */
  swap() {
    const cur = { hz: this.listen.hz, mode: this.listen.mode };
    this.listen.mode = this.other.mode;
    this.listen.hz = this.other.hz;
    this.other = cur;
    this.active = this.active === 'A' ? 'B' : 'A';
  }

  /** A=B: copy the VFO being heard to the other one. */
  copy() {
    this.other = { hz: this.listen.hz, mode: this.listen.mode };
  }

  /** Set up the effects; call while a component is initialising. */
  attach() {
    // Remember the settings.
    $effect(() => {
      const prefs = {
        squelchOn: this.squelchOn,
        squelch: this.squelch,
        volume: this.volume,
        nr: this.nr,
        cw: this.cwOn,
        afc: this.afcOn,
        digital: this.digitalOn,
        dsdConsole: this.consoleOn,
        autoMode: this.autoMode,
        hz: this.listen.hz,
        mode: this.listen.mode,
        active: this.active,
        other: { hz: this.other.hz, mode: this.other.mode },
      };
      try {
        localStorage.setItem(PREFS, JSON.stringify(prefs));
      } catch {
        /* private mode etc. */
      }
    });

    // Keep the server's listener in step with the controls.
    $effect(() => {
      if (!this.listen.on) return;
      this.send(this.params());
    });

    $effect(() => this.player.setVolume(this.muted ? 0 : this.volume));

    // The server starts AFC afresh on a new station, or when it is turned on
    // again; drop the old correction until it reports. A new station also
    // forgets the old one's RDS and what was on air.
    $effect(() => {
      void this.listen.hz;
      void this.listen.mode;
      void this.afcActive;
      this.listen.afc = null;
      this.afcLocked = false;
    });
    $effect(() => {
      void this.listen.hz;
      void this.listen.mode;
      this.rds = null;
      this.onAir = [];
      this.sync = null;
    });
    // Digital decoding turned off or on: start afresh.
    $effect(() => {
      void this.digitalActive;
      this.onAir = [];
      this.sync = null;
      this.dsdErr = null;
    });
    // A clock for "sync seen recently".
    $effect(() => {
      if (!this.digitalActive || !this.listen.on) return;
      const t = setInterval(() => (this.now = Date.now()), 500);
      return () => clearInterval(t);
    });

    return () => {
      if (this.listen.on) this.send({ type: 'audio_stop' });
      this.player.stop();
    };
  }

  params() {
    return {
      type: 'audio',
      freq_hz: Math.round(this.listen.hz),
      mode: this.listen.mode,
      squelch_db: this.squelchOn ? this.squelch : null,
      nr: this.nr,
      cw: this.cwActive,
      afc: this.afcActive,
      digital: this.digitalActive,
    };
  }

  async start() {
    this.err = null;
    this.starting = true;
    try {
      await this.player.start(this.muted ? 0 : this.volume);
      this.listen.on = true;
    } catch (e) {
      this.err = `Couldn't start audio: ${e instanceof Error ? e.message : e}`;
    } finally {
      this.starting = false;
    }
  }

  stop() {
    if (this.listen.on) this.send({ type: 'audio_stop' });
    this.listen.on = false;
    this.listen.afc = null;
    this.level = null;
    this.stats = null;
    this.player.stop();
  }

  /** Output level now (dBFS RMS and peak), for the audio meter. */
  audioLevels() {
    return this.listen.on ? this.player.levels() : null;
  }

  toggle() {
    if (this.listen.on) this.stop();
    else this.start();
  }

  /** Audio frames from the WebSocket. */
  feed(f: AudioFrame) {
    if (!this.listen.on) return;
    this.level = Number.isFinite(f.level) ? f.level : null;
    this.inBand = f.inBand;
    this.open = f.open;
    this.player.feed(f);
    this.bytes += 24 + f.data.length;
    const now = performance.now();
    if (now - this.t0 > 2000) {
      this.kbps = Math.round((this.bytes * 8) / (now - this.t0));
      this.bytes = 0;
      this.t0 = now;
    }
  }

  /** Replies from the server. */
  status(kind: string, message: string | null) {
    if (kind === 'audio_error') {
      this.err = message;
      this.stop();
    }
  }

  /** The WebSocket reconnected: the server forgot our listener. */
  reconnected() {
    if (this.listen.on) this.send(this.params());
  }

  /** Morse, RDS, AFC and digital voice decoded by the server for this listener. */
  decoded(m: DecodedMsg) {
    if (m.type === 'afc') {
      if (!this.afcActive || !this.listen.on) return;
      this.listen.afc = m.offset_hz;
      this.afcLocked = m.locked;
    } else if (m.type === 'digital') {
      if (!this.digitalActive) return;
      if (m.error !== undefined) this.dsdErr = m.error;
      if (m.calls) this.onAir = m.calls;
      if (m.event) {
        const e = { ...m.event, hz: this.listen.hz };
        const i = this.heard.findIndex((h) => h.id === e.id);
        if (i >= 0) this.heard[i] = { ...e, hz: this.heard[i].hz };
        else this.heard = [e, ...this.heard].slice(0, 50);
      }
      if (m.log) {
        this.dsdErr = null;
        this.dsdLog = [...this.dsdLog, ...m.log].slice(-300);
        for (const l of m.log) {
          const s = /Sync:\s*[+-]?([A-Za-z0-9-]+)/.exec(l);
          if (s) this.sawSync(s[1]);
        }
      }
    } else if (m.type === 'cw') {
      this.cwText = (this.cwText + m.text).slice(-800);
      this.cwWpm = m.wpm;
    } else {
      this.rds = m;
    }
  }

  private sawSync(mode: string) {
    const t = Date.now();
    const r = this.syncRun;
    this.syncRun = mode === r.mode && t - r.at < SYNC_GAP_MS ? { mode, n: r.n + 1, at: t } : { mode, n: 1, at: t };
    if (this.syncRun.n >= SYNC_RUN) this.sync = { mode, at: t };
  }
}
