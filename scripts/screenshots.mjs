// Takes the README screenshots from a running sdrd (normally the dev stack with
// the simulated receiver). Run it through ./scripts/screenshots.sh.
import { chromium } from 'playwright';

const URL = process.env.SDR_URL ?? 'http://localhost:8080';
const USER = process.env.SDR_USER ?? 'admin';
const PASS = process.env.SDR_PASS ?? 'devpassword123';
const OUT = process.env.OUT_DIR ?? '/out';

const browser = await chromium.launch();
const ctx = await browser.newContext({
  baseURL: URL,
  viewport: { width: 1440, height: 900 },
  colorScheme: 'dark',
});
const page = await ctx.newPage();
const shot = (name, opts = {}) => page.screenshot({ path: `${OUT}/${name}.png`, ...opts });
const tab = async (name, wait = 2500) => {
  await page.locator('nav').getByRole('button', { name, exact: true }).click();
  await page.waitForTimeout(wait);
};

const r = await page.request.post('/api/auth/login', { data: { username: USER, password: PASS } });
if (!r.ok()) throw new Error(`login failed: ${r.status()}`);
await page.goto('/');
await page.locator('nav').waitFor();

const receiver = page.locator('section[aria-label="Receiver"]');
const adsb = async () => (await (await page.request.get('/api/adsb')).json()).status;

// Receiver + spectrum, listening, so the meters and channel marker show. Taller
// window so the waterfall fits under the full-size front panel.
await page.setViewportSize({ width: 1440, height: 1250 });
await tab('Spectrum');
await receiver.getByRole('button', { name: 'Play' }).first().click();
await page.waitForTimeout(6000);
await shot('spectrum');

await page.setViewportSize({ width: 1440, height: 900 });
await receiver.getByRole('button', { name: 'Bookmarks' }).first().click();
await page.waitForTimeout(800);
await page.getByRole('button', { name: 'Calling', exact: true }).click();
await page.waitForTimeout(800);
await shot('bookmarks');
await page.keyboard.press('Escape');
await receiver.getByRole('button', { name: 'Stop' }).first().click();

// The other pages with the front panel folded to one line.
await page.getByRole('group', { name: 'Receiver size' }).getByRole('button', { name: 'Compact' }).click();

// Aircraft need an ADS-B receiver: borrow a simulated one, then put back
// whatever was chosen before.
const adsbBefore = (await adsb()).receiver?.device_id ?? 'off';
await tab('Settings');
await page.locator('#adsb-dev').selectOption('sim-v4');
for (let i = 0; i < 30 && (await adsb()).aircraft < 6; i++) await page.waitForTimeout(1000);
await tab('Aircraft', 6000);
await shot('aircraft');
await tab('Settings');
await page.locator('#adsb-dev').selectOption(adsbBefore);

for (const [name, file] of [
  ['Sensors', 'sensors'],
  ['APRS', 'aprs'],
  ['SSTV', 'sstv'],
  ['Passes', 'passes'],
  ['Settings', 'settings'],
]) {
  await tab(name, name === 'APRS' ? 5000 : 2500);
  await shot(file);
}

// The phone layout.
await page.setViewportSize({ width: 390, height: 844 });
await tab('Spectrum', 3000);
await shot('phone');

await browser.close();
