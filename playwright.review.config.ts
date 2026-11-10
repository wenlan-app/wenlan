// SPDX-License-Identifier: AGPL-3.0-only
import { defineConfig, devices } from "@playwright/test";

const port = Number(process.env.E2E_REVIEW_PORT ?? 14322);
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.review.spec.ts",
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  // In CI a failed test runs once more. A pass on the retry is reported as
  // "flaky" in the run summary instead of failing the merge queue; a real
  // regression fails both tries. See docs/ci-flake-policy.md.
  retries: process.env.CI ? 1 : 0,
  expect: {
    timeout: 10_000,
  },
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: {
    baseURL,
    headless: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: {
    command: `VITE_DISABLE_REACT_DEVTOOLS=1 pnpm exec vite --mode review --config vite.review.config.ts --host 127.0.0.1 --port ${port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 120_000,
  },
  projects: [
    {
      name: "review-chromium",
      use: {
        ...devices["Desktop Chrome"],
        timezoneId: "America/Los_Angeles",
      },
    },
  ],
});
