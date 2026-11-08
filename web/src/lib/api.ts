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

export interface RadioState {
  source: string;
  can_tune: boolean;
  connected: boolean;
  device: string | null;
  tuner: string | null;
  settings: RadioSettings;
  gains_db: number[];
  sample_rates: number[];
  error: string | null;
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
  users: () => req<UserRow[]>('GET', '/users'),
  createUser: (username: string, password: string, role: Role) =>
    req<{ id: number }>('POST', '/users', { username, password, role }),
  deleteUser: (id: number) => req('DELETE', `/users/${id}`),
  devices: () => req<Device[]>('GET', '/devices'),
  deviceEvents: (id: number, opts: { limit?: number; since?: number } = {}) => {
    const q = new URLSearchParams({ limit: String(opts.limit ?? 200) });
    if (opts.since !== undefined) q.set('since', String(Math.floor(opts.since)));
    return req<EventRow[]>('GET', `/devices/${id}/events?${q}`);
  },
  updateDevice: (id: number, patch: { name?: string; tracked?: boolean }) =>
    req<Device>('PATCH', `/devices/${id}`, patch),
  deleteDevice: (id: number) => req('DELETE', `/devices/${id}`),
  events: (limit = 100) => req<EventRow[]>('GET', `/events?limit=${limit}`),
  rtl433: () => req<Rtl433Info>('GET', '/decoders/rtl_433'),
  setRtl433: (config: Rtl433Config) => req<{ config: Rtl433Config }>('PUT', '/decoders/rtl_433', config),
  checkSpec: (spec: string) => req<{ ok: boolean; message?: string }>('POST', '/decoders/rtl_433/check', { spec }),
};
