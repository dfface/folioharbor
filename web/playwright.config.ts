import { randomBytes } from "node:crypto";

import { defineConfig, devices } from "@playwright/test";

const testSecrets = [
  "POSTGRES_PASSWORD",
  "OWNER_PASSWORD",
  "API_PASSWORD",
  "WORKER_PASSWORD",
  "APPLICATION_SECRET",
  "ADMIN_PASSWORD",
] as const;

for (const name of testSecrets) {
  process.env[`FOLIOHARBOR_E2E_${name}`] ??= randomBytes(32).toString("hex");
}

export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/global-setup.ts",
  timeout: 30_000,
  fullyParallel: false,
  workers: 1,
  reporter: "line",
  outputDir: "test-results",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "off",
    screenshot: "off",
    video: "off",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
