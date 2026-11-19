// Signal labels (M9) against the simulator: the SRD on 434.2 MHz is labelled
// as data, and hovering it shows the "what's this?" card.
import { expect, test } from '@playwright/test';

const USER = process.env.SDR_USER ?? 'admin';
const PASS = process.env.SDR_PASS ?? 'devpassword123';

test('signals are labelled and explained on hover', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop-1920', 'state-changing test runs once');
  const login = await page.request.post('/api/auth/login', { data: { username: USER, password: PASS } });
  expect(login.ok()).toBeTruthy();
  const radio = await (await page.request.get('/api/radio')).json();
  test.skip(!radio.connected || radio.source !== 'synthetic', `needs the simulated receiver (have ${radio.source})`);
  const me = await (await page.request.get('/api/auth/me')).json();
  const headers = { 'X-CSRF-Token': me.csrf };
  const before = { center_hz: radio.settings.center_hz, sample_rate: radio.settings.sample_rate };

  try {
    // The simulator's ISM block: the SRD at 434.2, its spurs and a steady carrier.
    const r = await page.request.post('/api/radio', { data: { center_hz: 434_000_000, sample_rate: 1_024_000 }, headers });
    expect(r.ok()).toBeTruthy();
    await page.goto('/');
    await page.locator('nav').getByRole('button', { name: 'Spectrum', exact: true }).click();
    await expect(page.getByRole('checkbox', { name: 'Signals' })).toBeChecked();

    // Labels arrive about once a second after a few seconds of looking.
    const summary = page.locator('.wfbox .sr-only');
    await expect(summary).toContainText(/434\.200\d MHz Data/, { timeout: 20_000 });
    await expect(summary).toContainText('Carrier');

    // Hover the SRD on the waterfall.
    const plots = page.locator('.wf .plots');
    const lo = Number(await plots.getAttribute('data-view-lo'));
    const hi = Number(await plots.getAttribute('data-view-hi'));
    const wf = page.locator('.wf .wfbox');
    await wf.scrollIntoViewIfNeeded();
    const box = (await wf.boundingBox())!;
    await page.mouse.move(box.x + ((434_200_000 - lo) / (hi - lo)) * box.width, box.y + box.height / 2);
    const card = page.getByTestId('signal-card');
    await expect(card).toBeVisible();
    await expect(card).toContainText('Data');
    await expect(card).toContainText('wide');
    await expect(card).toContainText('Band plan');

    // A spur 50 kHz up keys with it.
    await page.mouse.move(box.x + ((434_250_000 - lo) / (hi - lo)) * box.width, box.y + box.height / 2);
    await expect(card).toContainText('keys on and off with 434.2');

    // Off: no labels, no card.
    await page.getByRole('checkbox', { name: 'Signals' }).uncheck();
    await expect(card).toHaveCount(0);
    await expect(summary).toHaveCount(0);
    await page.getByRole('checkbox', { name: 'Signals' }).check();
  } finally {
    await page.request.post('/api/radio', { data: before, headers });
  }
});
