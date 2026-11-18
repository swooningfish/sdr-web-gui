import type { Bookmark, BookmarkInput, ImportResult } from './bookmarks.svelte';
import type { AudioMode } from './audio';
import type { FKey, FKeyInput } from './fkeys.svelte';

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
  /** The other receiver using it ("ADS-B", "the main receiver"). */
  used_by?: string;
  /** Per entry of `SdrDevices.features`: whether this device can receive it. */
  supported: boolean[];
}

export interface AdsbTrailPoint {
  ts: number;
  lat: number;
  lon: number;
  alt: number | null;
  ground: boolean;
}

export interface Aircraft {
  /** 24-bit address, 6 hex digits. */
  icao: string;
  callsign: string | null;
  /** Emitter category, e.g. "A3". */
  category: string | null;
  squawk: string | null;
  emergency: string | null;
  alt_baro_ft: number | null;
  alt_gnss_ft: number | null;
  on_ground: boolean;
  gs_kt: number | null;
  track_deg: number | null;
  heading_deg: number | null;
  airspeed_kt: number | null;
  tas: boolean;
  vrate_fpm: number | null;
  lat: number | null;
  lon: number | null;
  pos_ms: number | null;
  distance_km: number | null;
  bearing_deg: number | null;
  first_ms: number;
  seen_ms: number;
  msgs: number;
  rssi: number;
  reg: string | null;
  type_code: string | null;
  desc: string | null;
  military: boolean;
  trail: AdsbTrailPoint[];
}

export interface AdsbDbStatus {
  rows: number;
  updated_ms: number | null;
  downloading: boolean;
  error: string | null;
  source: string;
}

export interface AdsbStatus {
  enabled: boolean;
  /** Where 1090 MHz IQ comes from right now. */
  source: 'receiver' | 'main' | 'none';
  receiver: RadioState;
  msg_rate: number;
  messages: number;
  aircraft: number;
  with_position: number;
  home: [number, number] | null;
  max_range_km: number | null;
  db: AdsbDbStatus;
}

export interface AdsbReceiverSettings {
  gain_db: number | null;
  ppm: number;
  bias_tee: boolean;
}

export type MeshProto = 'meshtastic' | 'meshcore';

export interface MeshPreset {
  id: string;
  proto: MeshProto;
  name: string;
  freq_hz: number;
  sf: number;
  bw: number;
  cr: number;
  sync: number;
}

/** A private channel key; `key` is only sent to admins. */
export interface MeshChannelKey {
  name: string;
  key?: string;
}

export interface MeshConfig {
  enabled: boolean;
  presets: string[];
  meshtastic_channels: MeshChannelKey[];
  meshcore_channels: MeshChannelKey[];
}

export interface MeshInfo {
  config: MeshConfig;
  status: {
    enabled: boolean;
    /** IQ is being decoded (from the main receiver, or Mesh's own). */
    listening: boolean;
    presets: { preset: MeshPreset; enabled: boolean; in_band: boolean; packets: number; last_ms: number | null }[];
    tune_hz: number;
    nodes_24h: { meshtastic: number; meshcore: number };
    crc_errors: number;
  };
  presets: MeshPreset[];
  /** The receiver in use (main, or Mesh's own) hears at least one preset. */
  main_tuned: boolean;
  /** Decoding from a receiver of Mesh's own rather than the main one. */
  own_receiver: boolean;
  tile_url: string;
}

export interface MeshNode {
  key: string;
  proto: MeshProto;
  id: string;
  short_name: string | null;
  long_name: string | null;
  hw: string | null;
  role: string | null;
  lat: number | null;
  lon: number | null;
  alt: number | null;
  pos_ms: number | null;
  battery: number | null;
  voltage: number | null;
  channel_util: number | null;
  air_util_tx: number | null;
  uptime_s: number | null;
  temperature: number | null;
  humidity: number | null;
  pressure: number | null;
  snr: number | null;
  hops: number | null;
  preset: string;
  first_ms: number;
  last_ms: number;
  packets: number;
}

export interface MeshMessage {
  id: number;
  ts_ms: number;
  proto: MeshProto;
  channel: string | null;
  private: boolean;
  from_key: string | null;
  from_name: string | null;
  text: string;
  snr: number | null;
  hops: number | null;
}

export interface MeshPacket {
  ts_ms: number;
  preset: string;
  proto: MeshProto;
  crc_ok: boolean;
  snr: number;
  offset_hz: number;
  len: number;
  /** Hex; empty for viewers on private channels. */
  raw: string;
  from: string | null;
  from_name: string | null;
  to: string | null;
  kind: string;
  summary: string;
  channel: string | null;
  private: boolean;
  hops: number | null;
  repeat: boolean;
}

export interface AdsbInfo {
  config: { enabled: boolean };
  status: AdsbStatus;
  receiver_settings: AdsbReceiverSettings;
  seen_24h: number;
  /** The main receiver is tuned to 1090 MHz. */
  main_tuned: boolean;
  tile_url: string;
  database_url: string;
}

export interface AdsbRange {
  sectors: number;
  since_ms: number;
  all_time: number[];
  last_24h: number[];
  home: [number, number] | null;
}

export interface AdsbDevice extends Omit<SdrDevice, 'supported'> {
  covers_1090: boolean;
}

/** Decoders that can have a receiver of their own. */
export type ReceiverUse = 'mesh' | 'sensors' | 'aprs';

export interface ReceiverSettings {
  gain_db: number | null;
  ppm: number;
  bias_tee: boolean;
}

/** An extra receiver for Mesh, Sensors or APRS. */
export interface ExtraReceiver {
  use: ReceiverUse;
  label: string;
  /** Device id, or "off" to use the main receiver. */
  selected: string | null;
  /** Its decoder is fed from it rather than from the main receiver. */
  dedicated: boolean;
  state: RadioState;
  settings: ReceiverSettings;
  /** Where it's kept tuned, following its decoder's settings. */
  tuning: { center_hz: number; sample_rate: number };
}

export interface ExtraReceiverDevice extends Omit<SdrDevice, 'supported'> {
  /** Reaches the frequency this receiver is kept on. */
  covers: boolean;
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
  /** Receiver debug details (raw rtl_433 line, demod, IF, rates); null for older events. */
  rx?: Json | null;
  /** A radio burst was saved with this event ("log next burst"). */
  burst?: boolean;
}

/** The radio signal behind a device's latest packet (see /devices/{id}/raw). */
export interface RawSignal {
  /** Matches the EventRow.ts of the packet it belongs to. */
  ts: number;
  rate: number;
  freq_hz: number;
  demod: string | null;
  start_us: number;
  duration_us: number;
  /** 0–1000 relative to the burst peak. */
  envelope: number[];
  freq_khz: (number | null)[];
  /** Envelope of the first `detail_us` only, fine enough to see single pulses. */
  detail: number[];
  detail_us: number;
  snr_db: number;
  /** µs; positive = pulse / mark, negative = gap / space. */
  pulses: number[];
  pulses_truncated: boolean;
  slicer: 'level' | 'frequency';
  iq_samples: number;
  /** rtl_433 decoded this same sensor from just this burst (checked when
   *  logging; absent on bursts logged before the check existed). */
  verified?: boolean;
  file_name: string;
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
  /** Every verified burst is saved with its packet. */
  record_all_bursts: boolean;
  distance: DistanceModel;
}

/** Log-distance path loss calibration for sensor range estimates. */
export interface DistanceModel {
  /** Signal level (dB) expected 1 m from a sensor. */
  ref_db: number;
  /** Path loss exponent: 2 in free space, 2.5–4 around buildings. */
  exponent: number;
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
  /** Retune for passes automatically and switch browsers to the Satellites page. */
  auto_switch: boolean;
  /** Warn this many seconds before retuning for a pass. */
  notice_s: number;
  /** Receive ISS passes on the event's frequency during ARISS SSTV events. */
  ariss_auto: boolean;
}

/** An ARISS SSTV event (pictures sent from the ISS for several days). */
export interface ArissEvent {
  series: number;
  name: string;
  title: string;
  start_ms: number;
  end_ms: number;
  freq_hz: number | null;
  mode: string | null;
  cadence: string | null;
}

export interface ArissState {
  checked_ms: number | null;
  error: string | null;
  events: ArissEvent[];
}

/** The next pass the scheduler would tune for. */
export interface Upcoming {
  satellite: string;
  aos_ms: number;
  los_ms: number;
  max_el_deg: number;
  tune_ms: number;
  center_hz: number;
  sample_rate: number;
  /** An admin chose to stay on the current frequency for this pass. */
  skipped: boolean;
}

export interface Repeater {
  callsign: string;
  freq_hz: number;
  input_hz: number | null;
  tone: string | null;
  city: string;
  mode: string;
  status: string | null;
  distance_mi: number;
}

export type ScanBand = 'fm' | 'dab';

/** A station the band scan found. */
export interface ScanFound {
  id: string;
  band: ScanBand;
  name: string;
  freq_hz: number;
  mode: AudioMode;
  sample_rate: number | null;
  group: string;
  notes: string;
  /** Above the noise floor (FM) or signal-to-noise (DAB), dB. */
  level_db: number;
  /** RDS PI (FM) or service id (DAB), hex. */
  ident: string | null;
  ensemble: string | null;
  channel: string | null;
  /** Already among your bookmarks (same frequency and name). */
  saved: boolean;
}

export interface ScanStatus {
  running: boolean;
  band: ScanBand | null;
  step: string;
  progress: number;
  started_ms: number | null;
  finished_ms: number | null;
  by: string | null;
  stopped: boolean;
  error: string | null;
  notes: string[];
  found: ScanFound[];
}

export interface RepeaterList {
  enabled: boolean;
  radius_miles?: number;
  fetched_ms?: number | null;
  fetching?: boolean;
  error?: string | null;
  repeaters: Repeater[];
}

export interface RepeaterConfig {
  enabled: boolean;
  has_token: boolean;
  email: string;
  country: string;
  radius_miles: number;
}

export interface ObserverConfig {
  enabled: boolean;
  broker: string;
  iata: string;
  name: string;
  owner: string;
  email: string;
}

export interface ObserverInfo {
  config: ObserverConfig;
  /** This observer's MeshCore public key (hex). */
  public_key: string;
  brokers: { id: string; name: string; host: string }[];
  status: {
    connected: boolean;
    server: string | null;
    error: string | null;
    connected_ms: number | null;
    sent: number;
    last_sent_ms: number | null;
  };
  /** Why nothing is sent although it's on. */
  paused: string | null;
}

/** [time ms, azimuth°, elevation°] */
export type TrackPoint = [number, number, number];

/** Where a satellite is over the Earth: time (ms), latitude, longitude, height (km). */
export type GroundPoint = [number, number, number, number];

export interface GroundTrack {
  satellite: string;
  norad_id: number;
  points: GroundPoint[];
}

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
  upcoming: Upcoming | null;
  location: Observer | null;
  passes: Pass[];
  tles: TleInfo[];
  ariss: ArissState;
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
  deviceRaw: (id: number) => req<RawSignal>('GET', `/devices/${id}/raw`),
  burstLog: (id: number) => req<{ armed: boolean }>('GET', `/devices/${id}/burst-log`),
  setBurstLog: (id: number, armed: boolean) => req<{ armed: boolean }>('PUT', `/devices/${id}/burst-log`, { armed }),
  eventBurst: (eventId: number) => req<RawSignal>('GET', `/events/${eventId}/burst`),
  deleteEventBurst: (eventId: number) => req('DELETE', `/events/${eventId}/burst`),
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
  skipPass: (aos_ms: number, skip: boolean) => req('POST', '/schedule/skip', { aos_ms, skip }),
  repeaters: () => req<RepeaterList>('GET', '/repeaters'),
  repeaterConfig: () => req<RepeaterConfig>('GET', '/repeaters/config'),
  setRepeaterConfig: (c: Omit<RepeaterConfig, 'has_token'> & { token?: string }) =>
    req<RepeaterConfig>('PUT', '/repeaters/config', c),
  refreshRepeaters: () => req<{ count: number }>('POST', '/repeaters/refresh'),
  meshObserver: () => req<ObserverInfo>('GET', '/mesh/observer'),
  setMeshObserver: (c: ObserverConfig) => req<ObserverInfo>('PUT', '/mesh/observer', c),
  setRotation: (enabled: boolean) => req<{ rotation: boolean }>('PUT', '/schedule/rotation', { enabled }),
  bookmarks: () => req<Bookmark[]>('GET', '/bookmarks'),
  addBookmark: (b: BookmarkInput) => req<{ id: number }>('POST', '/bookmarks', b),
  updateBookmark: (id: number, b: BookmarkInput) => req<{ ok: boolean }>('PUT', `/bookmarks/${id}`, b),
  deleteBookmarks: (ids: number[]) => req<{ deleted: number; skipped: number }>('POST', '/bookmarks/delete', { ids }),
  importBookmarks: (csv: string, shared: boolean) => req<ImportResult>('POST', '/bookmarks/import', { csv, shared }),
  scan: () => req<ScanStatus>('GET', '/scan'),
  startScan: (band: ScanBand) => req<{ ok: boolean }>('POST', '/scan', { band }),
  stopScan: () => req<{ ok: boolean }>('POST', '/scan/stop'),
  saveScan: (ids: string[], shared: boolean) =>
    req<{ added: number; duplicates: number; missing: number }>('POST', '/scan/save', { ids, shared }),
  fkeys: () => req<FKey[]>('GET', '/fkeys'),
  setFkey: (slot: number, k: FKeyInput) => req<{ ok: boolean }>('PUT', `/fkeys/${slot}`, k),
  clearFkey: (slot: number) => req<{ ok: boolean }>('DELETE', `/fkeys/${slot}`),
  geocode: (q: string) =>
    req<{ places: { name: string; lat_deg: number; lon_deg: number }[] }>(
      'GET',
      `/schedule/geocode?q=${encodeURIComponent(q)}`,
    ),
  system: () => req<SystemInfo>('GET', '/system'),
  groundTracks: (from_ms?: number, minutes?: number) =>
    req<{ from_ms: number; to_ms: number; tracks: GroundTrack[] }>(
      'GET',
      `/schedule/tracks${from_ms === undefined ? '' : `?from_ms=${Math.floor(from_ms)}&minutes=${minutes ?? 110}`}`,
    ),
  deletePass: (id: number) => req('DELETE', `/passes/${id}`),
  aprsTracks: (since: number) => req<Track[]>('GET', `/aprs/tracks?since=${Math.floor(since)}`),
  rtl433: () => req<Rtl433Info>('GET', '/decoders/rtl_433'),
  setRecordAllBursts: (on: boolean) => req<{ on: boolean }>('PUT', '/decoders/rtl_433/record-bursts', { on }),
  setDistanceModel: (m: DistanceModel) => req<DistanceModel>('PUT', '/decoders/rtl_433/distance', m),
  setRtl433: (config: Rtl433Config) => req<{ config: Rtl433Config }>('PUT', '/decoders/rtl_433', config),
  adsb: () => req<AdsbInfo>('GET', '/adsb'),
  setAdsb: (config: { enabled: boolean }) => req('PUT', '/adsb', config),
  adsbAircraft: (since: number) =>
    req<{ now: number; aircraft: Aircraft[] }>('GET', `/adsb/aircraft?since=${Math.floor(since)}`),
  adsbRange: () => req<AdsbRange>('GET', '/adsb/range'),
  resetAdsbRange: () => req('POST', '/adsb/range/reset'),
  adsbDevices: () =>
    req<{ usb_supported: boolean; selected: string | null; devices: AdsbDevice[] }>('GET', '/adsb/devices'),
  selectAdsbDevice: (id: string) => req('POST', '/adsb/device', { id }),
  setAdsbReceiver: (s: AdsbReceiverSettings) => req('PUT', '/adsb/receiver', s),
  downloadAircraftDb: () => req('POST', '/adsb/database'),
  receivers: () => req<{ receivers: ExtraReceiver[] }>('GET', '/receivers'),
  receiverDevices: (use: ReceiverUse) =>
    req<{ selected: string | null; devices: ExtraReceiverDevice[] }>('GET', `/receivers/${use}/devices`),
  selectReceiver: (use: ReceiverUse, id: string) => req('POST', `/receivers/${use}/device`, { id }),
  setReceiver: (use: ReceiverUse, s: ReceiverSettings) => req('PUT', `/receivers/${use}/settings`, s),
  mesh: () => req<MeshInfo>('GET', '/mesh'),
  setMesh: (config: MeshConfig) => req('PUT', '/mesh', config),
  meshNodes: () => req<{ now: number; nodes: MeshNode[] }>('GET', '/mesh/nodes'),
  deleteMeshNodes: (keys: string[]) => req('POST', '/mesh/nodes/delete', { keys }),
  meshMessages: (limit = 300) => req<{ messages: MeshMessage[] }>('GET', `/mesh/messages?limit=${limit}`),
  meshPackets: () => req<{ packets: MeshPacket[] }>('GET', '/mesh/packets'),
  checkSpec: (spec: string) => req<{ ok: boolean; message?: string }>('POST', '/decoders/rtl_433/check', { spec }),
};
