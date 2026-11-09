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
    // The list shows recently heard stations only; there may be none.
    const all = page.locator('.list-head').getByText('Select all');
    if (!(await all.count())) return;
    await all.click();
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

test('rotation can be switched on and off from the Receiver panel', async ({ page }, info) => {
  // This changes server state, so run it at one screen size only (the sizes
  // run in parallel against the same server).
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const cfg = (await (await page.request.get('/api/schedule')).json()).config;
  test.skip(!cfg.profiles.length, 'no rotation profiles');
  const before: boolean = cfg.rotation;
  await openTab(page, 'Spectrum');
  const sw = page.locator('.rotation-box').getByRole('switch', { name: 'Rotation between profiles' });
  await expect(sw).toHaveAttribute('aria-checked', String(before));
  await sw.click();
  await expect(sw).toHaveAttribute('aria-checked', String(!before));
  expect((await (await page.request.get('/api/schedule')).json()).config.rotation).toBe(!before);
  // The Passes tab shows the same state.
  await openTab(page, 'Passes');
  await expect(page.locator('.rotation-row').getByRole('switch')).toHaveAttribute('aria-checked', String(!before));
  await page.locator('.rotation-row').getByRole('switch').click(); // put it back
  await expect(page.locator('.rotation-row').getByRole('switch')).toHaveAttribute('aria-checked', String(before));
  expect(await overflow(page)).toBeLessThanOrEqual(0);
});

test('band plan strip shows and its groups are remembered', async ({ page }) => {
  await openTab(page, 'Spectrum');
  await expect(page.locator('.wf canvas.bands')).toBeVisible();
  const menu = page.locator('.bands-menu');
  await menu.locator('summary').click();
  const amateur = menu.getByRole('checkbox', { name: 'Amateur' });
  await expect(amateur).toBeChecked();
  await amateur.uncheck();
  await page.reload();
  await page.locator('.bands-menu summary').click();
  await expect(page.locator('.bands-menu').getByRole('checkbox', { name: 'Amateur' })).not.toBeChecked();
  await page.locator('.bands-menu').getByRole('checkbox', { name: 'Amateur' }).check();
  expect(await overflow(page)).toBeLessThanOrEqual(0);
});

test('bookmarks panel opens the bookmarks page, which fits the screen', async ({ page }) => {
  await openTab(page, 'Spectrum');
  await expect(page.getByRole('heading', { name: 'Bookmarks' })).toBeVisible();
  await page.getByRole('button', { name: '★ Save' }).click();
  await expect(page.getByRole('form', { name: /Bookmark/ })).toBeVisible();
  expect(await overflow(page), 'add form is wider than the window').toBeLessThanOrEqual(0);
  await page.getByRole('button', { name: 'Cancel' }).click();
  await page.getByRole('button', { name: 'Manage bookmarks' }).click();
  await expect(page.getByRole('heading', { name: 'Import and export' })).toBeVisible();
  expect(await overflow(page), 'bookmarks page is wider than the window').toBeLessThanOrEqual(0);
  await page.getByRole('button', { name: '← Spectrum' }).click();
  await expect(page.locator('.wf')).toBeVisible();
});

test('a bookmark can be added, listened to and deleted', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const name = `Test ${Date.now()}`;
  await openTab(page, 'Spectrum');
  await page.getByRole('button', { name: 'Manage bookmarks' }).click();
  await page.getByRole('button', { name: 'Add bookmark' }).click();
  const form = page.getByRole('form', { name: 'New bookmark' });
  await form.getByLabel('Name').fill(name);
  await form.getByLabel('Frequency (MHz)').fill('145.5');
  await form.getByLabel('Mode').selectOption('nfm');
  await form.getByLabel('My bookmarks').check();
  await form.getByRole('button', { name: 'Save' }).click();
  const row = page.locator('tbody tr').filter({ hasText: name });
  await expect(row).toContainText('mine');
  await row.getByRole('button', { name: 'Listen' }).click();
  // Back on the spectrum, tuned to it, and it's in the side list.
  await expect(page.locator('#listen-freq')).toHaveValue('145.5000');
  await expect(page.getByRole('list', { name: 'Bookmarks' })).toContainText(name);
  // Delete it again from the page.
  await page.getByRole('button', { name: 'Manage bookmarks' }).click();
  page.once('dialog', (d) => d.accept());
  await page.getByLabel(`Select ${name}`).check();
  await page.getByRole('toolbar', { name: 'Selected bookmarks' }).getByRole('button', { name: 'Delete' }).click();
  await expect(page.locator('tbody tr').filter({ hasText: name })).toHaveCount(0);
});

test('arrow keys on the spectrum retune the radio', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected || !radio.can_tune, 'receiver not connected or not tunable');
  const before: number = radio.settings.center_hz;
  await openTab(page, 'Spectrum');
  const plots = page.locator('.wf .plots');
  await plots.focus();
  await page.keyboard.press('ArrowRight');
  await expect
    .poll(async () => (await (await page.request.get('/api/radio')).json()).settings.center_hz, { timeout: 5000 })
    .toBeGreaterThan(before);
  await page.keyboard.press('ArrowLeft');
  await expect
    .poll(async () => (await (await page.request.get('/api/radio')).json()).settings.center_hz, { timeout: 5000 })
    .toBe(before);
});

/** View and sampled window of the spectrum, in Hz. */
async function spectrumView(page: Page) {
  return page.locator('.wf .plots').evaluate((el) => {
    const d = (el as HTMLElement).dataset;
    return { viewLo: Number(d.viewLo), viewHi: Number(d.viewHi), winLo: Number(d.winLo), winHi: Number(d.winHi) };
  });
}

test('spectrum zooms in and out, outlining the sampled band', async ({ page }) => {
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected, `receiver not connected (${radio.error ?? radio.source})`);
  await openTab(page, 'Spectrum');
  const zoom = page.getByRole('group', { name: 'Zoom' });
  await expect(zoom.getByRole('button', { name: 'Zoom in' })).toBeEnabled();
  const v0 = await spectrumView(page);
  expect(v0.viewHi - v0.viewLo).toBeCloseTo(v0.winHi - v0.winLo, -3);

  await zoom.getByRole('button', { name: 'Zoom in' }).click();
  const v1 = await spectrumView(page);
  expect((v1.viewHi - v1.viewLo) * 2).toBeCloseTo(v0.winHi - v0.winLo, -3);
  expect(v1.viewLo).toBeGreaterThanOrEqual(v1.winLo - 1000);
  expect(v1.viewHi).toBeLessThanOrEqual(v1.winHi + 1000);

  await zoom.getByRole('button', { name: 'Zoom out' }).click();
  await zoom.getByRole('button', { name: 'Zoom out' }).click();
  await zoom.getByRole('button', { name: 'Zoom out' }).click();
  const v2 = await spectrumView(page);
  expect(v2.viewHi - v2.viewLo).toBeCloseTo((v0.winHi - v0.winLo) * 4, -3);
  await expect(page.locator('.wf .notice')).toContainText('Zoomed out');
  await expect(page.getByRole('slider', { name: /Whole receiver range/ })).toBeVisible();
  expect(await overflow(page), 'zoomed out view is wider than the window').toBeLessThanOrEqual(0);

  await page.locator('.wf .notice').getByRole('button', { name: 'Back to ×1' }).click();
  await expect(page.locator('.wf .notice')).toHaveCount(0);
  const v3 = await spectrumView(page);
  expect(v3.viewLo).toBeCloseTo(v3.winLo, -3);
});

test('the sampled window and the mini map move the radio', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected || !radio.can_tune, 'receiver not connected or not tunable');
  const centre = async () => (await (await page.request.get('/api/radio')).json()).settings.center_hz as number;
  const before: number = radio.settings.center_hz;
  await openTab(page, 'Spectrum');
  const zoom = page.getByRole('group', { name: 'Zoom' });
  await zoom.getByRole('button', { name: 'Zoom out' }).click();
  await zoom.getByRole('button', { name: 'Zoom out' }).click();

  // Drag the outlined window a tenth of the view to the right.
  const plots = page.locator('.wf .plots');
  const box = (await plots.boundingBox())!;
  const v = await spectrumView(page);
  const x = box.x + (((v.winLo + v.winHi) / 2 - v.viewLo) / (v.viewHi - v.viewLo)) * box.width;
  const y = box.y + box.height * 0.3;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + box.width * 0.1, y, { steps: 6 });
  await page.mouse.up();
  await expect.poll(centre, { timeout: 5000 }).toBeGreaterThan(before + (v.viewHi - v.viewLo) * 0.05);

  // Click the mini map back near where it was (log scale).
  const map = page.getByRole('slider', { name: /Whole receiver range/ });
  const lo = Number(await map.getAttribute('data-lo'));
  const hi = Number(await map.getAttribute('data-hi'));
  const m = (await map.boundingBox())!;
  const mx = m.x + ((Math.log(before) - Math.log(lo)) / (Math.log(hi) - Math.log(lo))) * m.width;
  await map.click({ position: { x: mx - m.x, y: 8 } });
  await expect.poll(async () => Math.abs((await centre()) - before), { timeout: 5000 }).toBeLessThan(before * 0.01);

  // Put it back exactly.
  await page.locator('#freq').fill(String(before / 1e6));
  await page.getByRole('button', { name: 'Tune', exact: true }).click();
  await expect.poll(centre, { timeout: 5000 }).toBe(before);
  await zoom.getByRole('button', { name: /^Zoom ÷/ }).click();
});

test('the mini map zooms, pans and fits the whole range', async ({ page }, info) => {
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected, `receiver not connected (${radio.error ?? radio.source})`);
  await openTab(page, 'Spectrum');
  const map = page.getByRole('slider', { name: /Whole receiver range/ });
  const range = page.getByRole('group', { name: 'Mini map range' });
  const shown = async () => ({
    lo: Number(await map.getAttribute('data-lo')),
    hi: Number(await map.getAttribute('data-hi')),
  });
  const full = { lo: Number(await map.getAttribute('aria-valuemin')), hi: Number(await map.getAttribute('aria-valuemax')) };
  const c: number = radio.settings.center_hz;
  expect(await shown()).toEqual(full);
  await expect(range.getByRole('button', { name: 'Zoom the mini map out' })).toBeDisabled();

  // Zooming in keeps the radio on the map.
  for (let i = 0; i < 3; i++) await range.getByRole('button', { name: 'Zoom the mini map in' }).click();
  const z = await shown();
  expect(Math.log(z.hi / z.lo)).toBeCloseTo(Math.log(full.hi / full.lo) / 8, 1);
  expect(z.lo).toBeLessThan(c);
  expect(z.hi).toBeGreaterThan(c);
  await expect(range.getByRole('button', { name: /^Mini map ×8/ })).toBeVisible();

  // Dragging away from the radio's marker pans the map, not the radio.
  const m = (await map.boundingBox())!;
  const cx = m.x + ((Math.log(c) - Math.log(z.lo)) / Math.log(z.hi / z.lo)) * m.width;
  const from = cx > m.x + m.width / 2 ? m.x + m.width * 0.1 : m.x + m.width * 0.9;
  const dir = cx > m.x + m.width / 2 ? 1 : -1;
  await page.mouse.move(from, m.y + 8);
  await page.mouse.down();
  await page.mouse.move(from + dir * m.width * 0.2, m.y + 8, { steps: 5 });
  await page.mouse.up();
  const p = await shown();
  expect(dir > 0 ? p.lo < z.lo : p.lo > z.lo, 'map panned').toBe(true);
  expect((await (await page.request.get('/api/radio')).json()).settings.center_hz).toBe(c);

  // Ctrl + wheel zooms out about the cursor (phones have no wheel: the
  // button); the level button fits it all.
  if (info.project.use.isMobile) {
    await range.getByRole('button', { name: 'Zoom the mini map out' }).click();
  } else {
    await page.mouse.move(m.x + m.width / 2, m.y + 8);
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, 200);
    await page.keyboard.up('Control');
  }
  const w = await shown();
  expect(w.hi / w.lo).toBeGreaterThan(p.hi / p.lo);
  await range.getByRole('button', { name: /fit the whole range/ }).click();
  expect(await shown()).toEqual(full);
  expect(await overflow(page), 'mini map controls are wider than the window').toBeLessThanOrEqual(0);
});

test('harmonics check rates the simulated SRD against Ofcom and FCC', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected || !radio.can_tune || radio.source !== 'synthetic', 'needs the simulated receiver');
  const sched = await (await page.request.get('/api/schedule')).json();
  test.skip(sched.config.rotation, 'rotation is on');
  const before: number = radio.settings.center_hz;
  await openTab(page, 'Spectrum');
  // Listen on the simulated SRD.
  await page.evaluate(() => {
    const p = JSON.parse(localStorage.getItem('sdr.listen') ?? '{}');
    localStorage.setItem('sdr.listen', JSON.stringify({ ...p, hz: 434_200_000, mode: 'nfm' }));
    localStorage.setItem('sdr.harmonics', JSON.stringify({ ofcom: true, fcc: true, backoff: 0, dwell: 2, maxN: 4 }));
  });
  await page.reload();
  const panel = page.locator('section', { has: page.getByRole('heading', { name: 'Harmonics' }) });
  await panel.getByRole('button', { name: 'Check harmonics' }).click();
  await expect(panel.getByLabel('Overall result')).toBeVisible({ timeout: 60_000 });
  // 2nd at -43 dBc (Ofcom limit -46: fails), 3rd at -35 dBc (fails Ofcom -40, passes FCC -26.8 by 8 dB).
  await expect(panel.locator('.overall').filter({ hasText: 'Ofcom' }).locator('.badge')).toHaveText('Bad');
  await expect(panel.locator('.overall').filter({ hasText: 'FCC' }).locator('.badge')).toHaveText('Good');
  // Spurs near the carrier, from the same capture: a comb every 50 kHz keyed with the SRD
  // (+50 kHz at -40 dBc fails Ofcom's -46), and a steady neighbour 250 kHz up that isn't the SRD's.
  await expect(panel.getByTestId('near-summary')).toContainText(/every (49|50)\.\d kHz/);
  const near = panel.locator('ul.near > li');
  const plus50 = near.filter({ hasText: /\+(49|50)\.\d kHz/ });
  await expect(plus50.locator('.badge').filter({ hasText: 'Ofcom' })).toHaveText('Ofcom: Bad');
  await expect(plus50.locator('.badge').filter({ hasText: 'FCC' })).toHaveText('FCC: Good');
  await expect(near.filter({ hasText: /\+2(49|50)\.\d kHz/ })).toContainText('Not steady with the device');
  // The radio goes back where it was.
  await expect
    .poll(async () => (await (await page.request.get('/api/radio')).json()).settings.center_hz, { timeout: 5000 })
    .toBe(before);
});

/** ADS-B status, as the Aircraft tab sees it. */
async function adsbInfo(page: Page) {
  return (await page.request.get('/api/adsb')).json();
}

test('aircraft tab shows traffic on the map and in the list', async ({ page }) => {
  const info = await adsbInfo(page);
  test.skip(info.status.source === 'none', 'nothing is receiving 1090 MHz');
  await openTab(page, 'Aircraft');
  const rows = page.locator('.adsb tbody tr');
  await expect(rows.first()).toBeVisible({ timeout: 15000 });
  await expect(page.locator('.adsb .adsb-marker').first()).toBeVisible();
  expect(await overflow(page), 'aircraft tab is wider than the window').toBeLessThanOrEqual(0);

  // Details of one aircraft, then back.
  await rows.first().click();
  await expect(page.locator('.adsb dt', { hasText: 'ICAO address' })).toBeVisible();
  await page.getByRole('button', { name: 'Back to list' }).click();
  await expect(rows.first()).toBeVisible();

  // Filtering by something no aircraft has empties the list.
  await page.getByRole('searchbox', { name: 'Filter aircraft' }).fill('zzzz-none');
  await expect(rows).toHaveCount(0);
  await expect(page.getByText('No aircraft match the filter.')).toBeVisible();
});

test('a simulated ADS-B receiver can be chosen in settings', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(radio.source !== 'synthetic', 'needs the simulated receiver');
  const before: string = (await adsbInfo(page)).status.receiver.device_id ?? 'off';
  await openTab(page, 'Settings');
  const select = page.locator('#adsb-dev');
  await select.selectOption('sim-v4');
  await expect.poll(async () => (await adsbInfo(page)).status.source, { timeout: 10000 }).toBe('receiver');
  await expect.poll(async () => (await adsbInfo(page)).status.aircraft, { timeout: 15000 }).toBeGreaterThan(3);
  // The main receiver's list doesn't offer simulated dongles as "used for ADS-B" (they're shareable),
  // and the ADS-B list never offers a dongle that can't reach 1090 MHz.
  await expect(select.locator('option', { hasText: 'FC2580' })).toBeDisabled();
  await select.selectOption(before);
  await expect.poll(async () => (await adsbInfo(page)).status.receiver.device_id ?? 'off', { timeout: 10000 }).toBe(before);
});
