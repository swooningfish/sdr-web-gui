import type { Bookmark, BookmarkInput, ImportResult } from './bookmarks.svelte';

export type Role = 'admin' | 'viewer';

export interface User {
  id: number;
  username: string;
  role: Role;
}

export interface UserRow extends User {
  created_at: number;
}

export interface RadioSettings {
  center_hz: number;
  sample_rate: number;
  gain_db: number | null;
  ppm: number;
  bias_tee: boolean;
  direct_sampling: number;
}

/** Inclusive span of frequencies a receiver can tune. */
export interface FreqRange {
  lo_hz: number;
  hi_hz: number;
}

export interface RadioState {
  source: string;
  can_tune: boolean;
  can_select: boolean;
  connected: boolean;
  device_id: string | null;
  device: string | null;
  tuner: string | null;
  /** What the selected receiver can tune; empty = unknown. */
  ranges: FreqRange[];
  settings: RadioSettings;
  gains_db: number[];
  sample_rates: number[];
  error: string | null;
}

/** Kind of receiver: a USB dongle, an rtl_tcp server, a recording or the simulator. */
export type SdrSource = 'rtlsdr' | 'rtl_tcp' | 'replay' | 'synthetic';

export interface SdrDevice {
  id: string;
  source: SdrSource;
  name: string;
  serial: string | null;
  tuner: string | null;
  ranges: FreqRange[];
  active: boolean;
  note: string | null;
  /** Per entry of `SdrDevices.features`: whether this device can receive it. */
  supported: boolean[];
}

/** Something the station listens to on a fixed frequency. */
export interface SdrFeature {
  group: string;
  label: string;
  freq_hz: number;
  enabled: boolean;
}

export interface SdrDevices {
  can_select: boolean;
  /** sdrd was built with USB RTL-SDR support. */
  usb_supported: boolean;
  /** A recording is configured (SDRD_REPLAY_FILE). */
  replay_available: boolean;
  devices: SdrDevice[];
  features: SdrFeature[];
}

export type Json = Record<string, unknown>;

export interface Device {
  id: number;
  decoder: string;
  key: string;
  model: string;
  dev_id: string | null;
  channel: string | null;
  kind: string | null;
  name: string | null;
  tracked: boolean;
  first_seen: number;
  last_seen: number;
  count: number;
  last_data: Json;
  lat: number | null;
  lon: number | null;
  pos_ts: number | null;
  symbol: string | null;
  /** Position on the sensor dashboard; null = not shown there. */
  dash_pos: number | null;
}

export interface DevicePatch {
  name?: string;
  tracked?: boolean;
  dashboard?: boolean;
}

export interface AprsChannel {
  label: string;
  frequency_hz: number;
  enabled: boolean;
  doppler: boolean;
}

export interface AprsConfig {
  enabled: boolean;
  channels: AprsChannel[];
}

export interface AprsChannelStatus extends AprsChannel {
  in_band: boolean;
  running: boolean;
  packets: number;
  error: string | null;
}

export interface AprsInfo {
  config: AprsConfig;
  status: { enabled: boolean; audio_rate: number; channels: AprsChannelStatus[]; log: string[] };
  map_tiles: string;
}

export interface Track {
  device_id: number;
  points: [number, number, number][]; // [ts, lat, lon]
}

export interface EventRow {
  id: number;
  device_id: number;
  ts: number;
  freq_hz: number | null;
  rssi: number | null;
  snr: number | null;
  data: Json;
}

export interface FlexDecoder {
  spec: string;
  enabled: boolean;
}

export interface Rtl433Config {
  enabled: boolean;
  frequency_hz: number;
  protocols: number[];
  flex: FlexDecoder[];
}

export interface Rtl433Status {
  enabled: boolean;
  running: boolean;
  in_band: boolean;
  frequency_hz: number;
  sample_rate: number;
  restarts: number;
  error: string | null;
  log: string[];
}

export interface MqttStatus {
  server: string;
  connected: boolean;
  error: string | null;
}

export interface Rtl433Info {
  config: Rtl433Config;
  status: Rtl433Status;
  mqtt: MqttStatus | null;
}

export type Demod = 'fm' | 'usb' | 'lsb';

export interface SstvChannel {
  label: string;
  frequency_hz: number;
  demod: Demod;
  enabled: boolean;
  doppler: boolean;
}

export interface Observer {
  lat_deg: number;
  lon_deg: number;
  alt_m: number;
}

export interface Profile {
  name: string;
  center_hz: number;
  sample_rate: number;
  dwell_s: number;
}

export interface SatConfig {
  name: string;
  norad_id: number;
  enabled: boolean;
  min_elevation_deg: number;
  center_hz: number;
  sample_rate: number;
  record: boolean;
  record_hz: number;
}

export interface ScheduleConfig {
  rotation: boolean;
  profiles: Profile[];
  satellites: SatConfig[];
  lead_s: number;
}

/** [time ms, azimuth°, elevation°] */
export type TrackPoint = [number, number, number];

export interface Pass {
  satellite: string;
  aos_ms: number;
  los_ms: number;
  max_ms: number;
  max_el_deg: number;
  aos_az_deg: number;
  los_az_deg: number;
  track: TrackPoint[];
}

export interface Look {
  az_deg: number;
  el_deg: number;
  range_km: number;
  range_rate_km_s: number;
}

export interface ActivePass {
  satellite: string;
  aos_ms: number;
  los_ms: number;
  max_el_deg: number;
  test: boolean;
  recording: boolean;
  look: Look | null;
  doppler_hz: number | null;
  record_hz: number;
  track: TrackPoint[];
}

export interface TleInfo {
  norad_id: number;
  name: string;
  epoch_ms: number | null;
  fetched_ms: number | null;
  source: string | null;
  error: string | null;
}

export interface ScheduleStatus {
  mode: 'manual' | 'rotation' | 'pass';
  profile: string | null;
  until_ms: number | null;
  holding: boolean;
  active: ActivePass | null;
  location: Observer | null;
  passes: Pass[];
  tles: TleInfo[];
}

export interface ScheduleInfo {
  config: ScheduleConfig;
  status: ScheduleStatus;
}

export interface SstvConfig {
  enabled: boolean;
  channels: SstvChannel[];
}

export interface SstvChannelStatus extends SstvChannel {
  in_band: boolean;
  receiving: { mode: string; lines: number; height: number } | null;
  images: number;
}

export interface SstvInfo {
  config: SstvConfig;
  status: { enabled: boolean; channels: SstvChannelStatus[] };
}

export interface SystemInfo {
  version: string;
  source: string;
  event_retention_days: number;
  ism_viewers: boolean;
  audio_viewers: boolean;
  audio_max_listeners: number;
  audio_listeners: number;
  session_hours: number;
  trust_proxy: boolean;
  map_tiles: string;
  mqtt: { prefix: string; discovery_prefix: string; events: boolean } | null;
  mqtt_status: MqttStatus | null;
}

export type RadioPatch = Partial<RadioSettings> & { auto_gain?: boolean };

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

// CSRF token returned at login / from /auth/me; sent on every mutating request.
let csrf = '';

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  if (method !== 'GET' && csrf) headers['X-CSRF-Token'] = csrf;
  const r = await fetch(`/api${path}`, {
    method,
    headers,
    credentials: 'same-origin',
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await r.text();
  let data: any = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    /* non-JSON error page from a proxy */
  }
  if (!r.ok) throw new ApiError(r.status, data?.error ?? `${r.status} ${r.statusText}`);
  return data as T;
}

export const api = {
  async me(): Promise<User> {
    const r = await req<{ user: User; csrf: string }>('GET', '/auth/me');
    csrf = r.csrf;
    return r.user;
  },
  async login(username: string, password: string): Promise<User> {
    const r = await req<{ user: User; csrf: string }>('POST', '/auth/login', { username, password });
    csrf = r.csrf;
    return r.user;
  },
  async logout(): Promise<void> {
    try {
      await req('POST', '/auth/logout');
    } finally {
      csrf = '';
    }
  },
  changePassword: (current_password: string, new_password: string) =>
    req('POST', '/auth/password', { current_password, new_password }),
  radio: () => req<RadioState>('GET', '/radio'),
  configure: (patch: RadioPatch) => req<RadioSettings>('POST', '/radio', patch),
  sdrDevices: () => req<SdrDevices>('GET', '/radio/devices'),
  selectSdr: (id: string) => req('POST', '/radio/device', { id }),
  forgetRtlTcp: (addr: string) => req('DELETE', '/radio/rtl_tcp', { addr }),
  users: () => req<UserRow[]>('GET', '/users'),
  createUser: (username: string, password: string, role: Role) =>
    req<{ id: number }>('POST', '/users', { username, password, role }),
  deleteUser: (id: number) => req('DELETE', `/users/${id}`),
  devices: (decoder?: string) => req<Device[]>('GET', `/devices${decoder ? `?decoder=${decoder}` : ''}`),
  deviceEvents: (id: number, opts: { limit?: number; since?: number } = {}) => {
    const q = new URLSearchParams({ limit: String(opts.limit ?? 200) });
    if (opts.since !== undefined) q.set('since', String(Math.floor(opts.since)));
    return req<EventRow[]>('GET', `/devices/${id}/events?${q}`);
  },
  updateDevice: (id: number, patch: DevicePatch) => req<Device>('PATCH', `/devices/${id}`, patch),
  dashboard: () => req<Device[]>('GET', '/dashboard'),
  setDashboardOrder: (ids: number[]) => req<Device[]>('PUT', '/dashboard/order', { ids }),
  deleteDevice: (id: number) => req('DELETE', `/devices/${id}`),
  events: (limit = 100, decoder?: string) =>
    req<EventRow[]>('GET', `/events?limit=${limit}${decoder ? `&decoder=${decoder}` : ''}`),
  aprs: () => req<AprsInfo>('GET', '/aprs'),
  setAprs: (config: AprsConfig) => req<{ config: AprsConfig }>('PUT', '/aprs', config),
  sstv: () => req<SstvInfo>('GET', '/sstv'),
  setSstv: (config: SstvConfig) => req<{ config: SstvConfig }>('PUT', '/sstv', config),
  deleteSstvImage: (id: number) => req('DELETE', `/sstv/image/${id}`),
  deleteSstvImages: (ids: number[]) => req<{ deleted: number }>('POST', '/sstv/images/delete', { ids }),
  deleteDevices: (ids: number[]) => req<{ deleted: number }>('POST', '/devices/delete', { ids }),
  schedule: () => req<ScheduleInfo>('GET', '/schedule'),
  setSchedule: (config: ScheduleConfig) => req<{ config: ScheduleConfig }>('PUT', '/schedule', config),
  setLocation: (obs: Observer) => req<{ location: Observer }>('PUT', '/schedule/location', obs),
  setTle: (name: string, line1: string, line2: string) =>
    req<{ norad_id: number }>('PUT', '/schedule/tle', { name, line1, line2 }),
  refreshTle: () => req('POST', '/schedule/tle/refresh'),
  testPass: (seconds: number) => req('POST', '/schedule/test', { seconds }),
  stopPass: () => req('POST', '/schedule/stop'),
  setRotation: (enabled: boolean) => req<{ rotation: boolean }>('PUT', '/schedule/rotation', { enabled }),
  bookmarks: () => req<Bookmark[]>('GET', '/bookmarks'),
  addBookmark: (b: BookmarkInput) => req<{ id: number }>('POST', '/bookmarks', b),
  updateBookmark: (id: number, b: BookmarkInput) => req<{ ok: boolean }>('PUT', `/bookmarks/${id}`, b),
  deleteBookmarks: (ids: number[]) => req<{ deleted: number; skipped: number }>('POST', '/bookmarks/delete', { ids }),
  importBookmarks: (csv: string, shared: boolean) => req<ImportResult>('POST', '/bookmarks/import', { csv, shared }),
  geocode: (q: string) =>
    req<{ places: { name: string; lat_deg: number; lon_deg: number }[] }>(
      'GET',
      `/schedule/geocode?q=${encodeURIComponent(q)}`,
    ),
  system: () => req<SystemInfo>('GET', '/system'),
  deletePass: (id: number) => req('DELETE', `/passes/${id}`),
  aprsTracks: (since: number) => req<Track[]>('GET', `/aprs/tracks?since=${Math.floor(since)}`),
  rtl433: () => req<Rtl433Info>('GET', '/decoders/rtl_433'),
  setRtl433: (config: Rtl433Config) => req<{ config: Rtl433Config }>('PUT', '/decoders/rtl_433', config),
  checkSpec: (spec: string) => req<{ ok: boolean; message?: string }>('POST', '/decoders/rtl_433/check', { spec }),
};
