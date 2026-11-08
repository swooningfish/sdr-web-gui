// Browser layout tests, run against a live sdrd (normally the dev stack):
//   ./scripts/ui-test.sh            (runs in the official Playwright image)
// SDR_URL / SDR_USER / SDR_PASS override the target and login.
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: process.env.SDR_URL ?? 'http://localhost:8080',
    colorScheme: 'dark',
  },
  // Layout bugs tend to show at one size only, so run everything at several.
  projects: [
    { name: 'desktop-1920', use: { viewport: { width: 1920, height: 1080 } } },
    { name: 'laptop-1366', use: { viewport: { width: 1366, height: 768 } } },
    { name: 'tablet-820', use: { viewport: { width: 820, height: 1180 } } },
    { name: 'phone-375', use: { viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true } },
  ],
});
