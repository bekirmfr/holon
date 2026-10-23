// End-to-end tests (e2e/*.e2e.ts): the real editor, in a browser, against a real holon-server.
// Both are started here, apart from a developer's own: the server on 7520 with throwaway state
// (e2e/server.mjs), the editor on 5273 pointed at it. Samples deploy on ports the tests set
// under ⚙, so they don't clash with deployments already running. Signing in has its own: a
// stand-in identity provider on 7530 (e2e/mock-oidc.mjs), a server on 7521 that signs in
// through it, and an editor on 5274 pointed at that. `npx playwright test`.

import { defineConfig } from "@playwright/test";

const SERVER = "http://127.0.0.1:7520";

export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.e2e.ts",
  // One server, deployments shared between tests: one at a time.
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? "github" : "list",
  use: { baseURL: "http://localhost:5273", trace: "retain-on-failure" },
  webServer: [
    {
      command: "node e2e/server.mjs",
      url: `${SERVER}/health`,
      // The first run compiles the server.
      timeout: 900_000,
      reuseExistingServer: false,
      stdout: "pipe",
    },
    {
      command: "node e2e/mock-oidc.mjs",
      url: "http://127.0.0.1:7530/.well-known/openid-configuration",
      timeout: 30_000,
      reuseExistingServer: false,
    },
    {
      command: "node e2e/server.mjs --signin",
      url: "http://127.0.0.1:7521/health",
      timeout: 900_000,
      reuseExistingServer: false,
      stdout: "pipe",
    },
    {
      command: "npx vite --port 5274 --strictPort",
      url: "http://localhost:5274",
      env: { VITE_HOLON_SERVER: "http://127.0.0.1:7521" },
      timeout: 120_000,
      reuseExistingServer: false,
    },
    {
      command: "npx vite --port 5273 --strictPort",
      url: "http://localhost:5273",
      env: { VITE_HOLON_SERVER: SERVER },
      timeout: 120_000,
      reuseExistingServer: false,
    },
  ],
});
