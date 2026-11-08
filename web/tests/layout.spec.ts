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
