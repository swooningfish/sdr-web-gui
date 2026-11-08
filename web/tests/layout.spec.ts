import { expect, test, type Page } from '@playwright/test';

const USER = process.env.SDR_USER ?? 'admin';
const PASS = process.env.SDR_PASS ?? 'devpassword123';

async function login(page: Page) {
  const r = await page.request.post('/api/auth/login', { data: { username: USER, password: PASS } });
  expect(r.ok(), `login as ${USER}`).toBeTruthy();
  await page.goto('/');
  await expect(page.locator('nav')).toBeVisible();
}

async function openTab(page: Page, name: string) {
  await page.locator('nav').getByRole('button', { name, exact: true }).click();
  await page.waitForTimeout(400);
}

/** Pixels the page is wider than the window (0 = no sideways scrolling). */
async function overflow(page: Page) {
  return page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
}

test.beforeEach(async ({ page }) => login(page));

test('no tab scrolls sideways', async ({ page }) => {
  const tabs = await page.locator('nav button').allInnerTexts();
  expect(tabs.length).toBeGreaterThan(5);
  for (const t of tabs) {
    await openTab(page, t.trim());
    expect(await overflow(page), `"${t}" is wider than the window`).toBeLessThanOrEqual(0);
  }
});

test('spectrum toolbar does not move when hovering the spectrum', async ({ page }) => {
  // Hover needs live spectrum data, so the receiver must be delivering IQ.
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected, `receiver not connected (${radio.error ?? radio.source}); pick a working source to run this`);
  await openTab(page, 'Spectrum');
  const toolbar = page.locator('.wf .toolbar');
  const controls = page.locator('.wf .controls');
  const readout = page.locator('.wf .readout');
  await expect(controls).toBeVisible();
  const before = { bar: await toolbar.boundingBox(), ctl: await controls.boundingBox() };

  const plot = page.locator('.wf canvas.spectrum');
  const box = (await plot.boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.4, box.y + box.height / 2);
  await expect(page.locator('.wf .cursor:not(.idle)')).toBeVisible();
  const after = { bar: await toolbar.boundingBox(), ctl: await controls.boundingBox() };

  expect(after.bar!.height, 'toolbar height changed on hover').toBeCloseTo(before.bar!.height, 0);
  expect(after.ctl!.y, 'controls jumped on hover').toBeCloseTo(before.ctl!.y, 0);
  // Controls always sit on their own row below the readout.
  const r = (await readout.boundingBox())!;
  expect(after.ctl!.y).toBeGreaterThanOrEqual(r.y + r.height - 1);
});

test('settings are split into tabs, admin only', async ({ page }) => {
  await openTab(page, 'Settings');
  const tabs = page.getByRole('tab');
  await expect(tabs).toHaveCount(6);
  for (const name of ['Receiver', 'Decoders', 'Passes', 'Integrations', 'Users', 'System']) {
    await page.getByRole('tab', { name }).click();
    await expect(page.getByRole('tab', { name })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('#settings-panel .panel').first()).toBeVisible();
    expect(await overflow(page), `Settings → ${name} is wider than the window`).toBeLessThanOrEqual(0);
  }
  // Password change moved to the profile page.
  await expect(page.getByRole('heading', { name: 'Change password' })).toHaveCount(0);
});

test('profile page opens from the username and has the password form', async ({ page }) => {
  await page.locator('header button.user').click();
  await expect(page.getByRole('heading', { name: USER })).toBeVisible();
  await expect(page.getByLabel(/current password/i)).toBeVisible();
  expect(await overflow(page)).toBeLessThanOrEqual(0);
});

test('waterfall colour choice is remembered', async ({ page }) => {
  await openTab(page, 'Spectrum');
  const pick = page.getByLabel('Waterfall colours');
  await pick.selectOption('viridis');
  await page.reload();
  await expect(page.getByLabel('Waterfall colours')).toHaveValue('viridis');
  await page.getByLabel('Waterfall colours').selectOption('turbo');
});

test('sensor list sorts by column', async ({ page }) => {
  const devices = await (await page.request.get('/api/devices?decoder=rtl_433')).json();
  test.skip(!Array.isArray(devices) || devices.length < 2, 'needs at least two sensors');
  await openTab(page, 'Sensors');
  const header = page.locator('th[aria-sort]').filter({ hasText: 'Device' });
  await header.getByRole('button').click();
  await expect(header).toHaveAttribute('aria-sort', /ascending|descending/);
  const names = async () => page.locator('tbody tr td:nth-child(2) button.link').allInnerTexts();
  const a = await names();
  await header.getByRole('button').click(); // reverse
  const b = await names();
  expect(b).toEqual([...a].reverse());
  expect(await overflow(page)).toBeLessThanOrEqual(0);
});

test('sensor and APRS lists can select several items', async ({ page }) => {
  const sensors = await (await page.request.get('/api/devices?decoder=rtl_433')).json();
  const aprs = await (await page.request.get('/api/devices?decoder=aprs')).json();
  test.skip(!sensors.length && !aprs.length, 'no sensors or APRS stations yet');
  if (sensors.length) {
    await openTab(page, 'Sensors');
    await page.getByLabel('Select all shown devices').check();
    await expect(page.getByRole('toolbar', { name: 'Selected devices' })).toContainText(`${sensors.length} selected`);
    await page.getByRole('toolbar', { name: 'Selected devices' }).getByRole('button', { name: 'Clear' }).click();
    await expect(page.getByRole('toolbar', { name: 'Selected devices' })).toHaveCount(0);
  }
  if (aprs.length) {
    await openTab(page, 'APRS');
    await page.locator('.list-head').getByText('Select all').click();
    await expect(page.getByRole('toolbar', { name: 'Selected stations' })).toContainText('selected');
    expect(await overflow(page)).toBeLessThanOrEqual(0);
  }
});

test('SSTV gallery select mode offers ZIP download', async ({ page }) => {
  const images = await (await page.request.get('/api/events?decoder=sstv')).json();
  test.skip(!images.length, 'no SSTV images yet');
  await openTab(page, 'SSTV');
  await page.getByRole('button', { name: 'Select', exact: true }).click();
  const bar = page.getByRole('toolbar', { name: 'Selected images' });
  await expect(bar).toContainText('0 selected');
  await page.locator('.grid .card').first().click();
  await expect(bar).toContainText('1 selected');
  await expect(bar.getByRole('link', { name: 'Download ZIP' })).toHaveAttribute('href', /\/api\/sstv\/images\.zip\?ids=\d+/);
  expect(await overflow(page)).toBeLessThanOrEqual(0);
  await page.keyboard.press('Escape');
  await expect(bar).toHaveCount(0);
});

test('APRS map shows home and can centre on it', async ({ page }) => {
  // The dev stack has a station location set (Settings → Passes).
  const loc = (await (await page.request.get('/api/schedule')).json()).status.location;
  test.skip(!loc, 'no station location set');
  await openTab(page, 'APRS');
  const home = page.locator('.aprs-home');
  await expect(home).toBeVisible();
  const btn = page.getByRole('button', { name: 'Centre on home' });
  await expect(btn).toBeEnabled();
  // Pan away, then centre: the house should end up in the middle of the map.
  const map = page.getByRole('region', { name: 'APRS station map' });
  const m = (await map.boundingBox())!;
  await page.mouse.move(m.x + m.width * 0.5, m.y + m.height * 0.5);
  await page.mouse.down();
  await page.mouse.move(m.x + m.width * 0.1, m.y + m.height * 0.2, { steps: 5 });
  await page.mouse.up();
  await btn.click();
  await page.waitForTimeout(600);
  const h = (await home.locator('.house').boundingBox())!;
  expect(Math.abs(h.x + h.width / 2 - (m.x + m.width / 2))).toBeLessThan(25);
  expect(Math.abs(h.y + h.height / 2 - (m.y + m.height / 2))).toBeLessThan(25);
});
