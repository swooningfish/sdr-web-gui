/**
 * The page shown, kept in the URL's hash (`#sensors`, `#satellites/sstv`,
 * `#settings/decoders`) so it can be linked to, survives a reload and
 * follows the browser's back and forward buttons.
 */

export const VIEWS = [
  'spectrum',
  'dashboard',
  'sensors',
  'aprs',
  'aircraft',
  'mesh',
  'satellites',
  'settings',
  'profile',
] as const;
export type View = (typeof VIEWS)[number];

export const SAT_TABS = ['passes', 'sstv'] as const;
export type SatTab = (typeof SAT_TABS)[number];

export const SETTINGS_TABS = ['receiver', 'decoders', 'rotation', 'passes', 'integrations', 'users', 'system', 'about'] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

/** The parts of Settings → Decoders (`#settings/decoders/mesh`). */
export const DECODER_TABS = ['ism', 'aprs', 'sstv', 'mesh'] as const;
export type DecoderTab = (typeof DECODER_TABS)[number];

export interface Route {
  view: View;
  /** Only set when the hash names one. */
  satTab?: SatTab;
  settingsTab?: SettingsTab;
  decoderTab?: DecoderTab;
}

const TITLES: Record<View, string> = {
  spectrum: 'Spectrum',
  dashboard: 'Dashboard',
  sensors: 'Sensors',
  aprs: 'APRS',
  aircraft: 'Aircraft',
  mesh: 'Mesh',
  satellites: 'Satellites',
  settings: 'Settings',
  profile: 'Profile',
};

const has = <T extends string>(list: readonly T[], v: string | undefined): v is T =>
  v !== undefined && (list as readonly string[]).includes(v);

/** The route in `hash` (`#view/sub`), or null when it doesn't name a page. */
export function parseRoute(hash: string): Route | null {
  const [v, sub, sub2] = decodeURIComponent(hash.replace(/^#\/?/, '')).toLowerCase().split('/');
  if (!has(VIEWS, v)) return null;
  const r: Route = { view: v };
  if (v === 'satellites' && has(SAT_TABS, sub)) r.satTab = sub;
  if (v === 'settings' && has(SETTINGS_TABS, sub)) r.settingsTab = sub;
  if (r.settingsTab === 'decoders' && has(DECODER_TABS, sub2)) r.decoderTab = sub2;
  return r;
}

export function routeHash(view: View, satTab: SatTab, settingsTab: SettingsTab, decoderTab: DecoderTab = 'ism'): string {
  if (view === 'satellites') return `#satellites/${satTab}`;
  if (view === 'settings' && settingsTab === 'decoders') return `#settings/decoders/${decoderTab}`;
  if (view === 'settings') return `#settings/${settingsTab}`;
  return `#${view}`;
}

export function routeTitle(view: View): string {
  return `${TITLES[view]} · SDR Monitor`;
}
