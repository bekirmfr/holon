// End-to-end tests (e2e/*.e2e.ts): the real editor, in a browser, against a real holon-server.
// Both are started here, apart from a developer's own: the server with throwaway state
// (e2e/server.mjs), the editor pointed at it. Samples deploy on ports the tests set under ⚙, so
// they don't clash with deployments already running; it is set up with an admin the tests sign in
// as (e2e/fixtures.ts). Accounts have their own: a server left as installed, an editor pointed at
// it, and a stand-in identity provider (e2e/mock-oidc.mjs). Every port is the run's slot's
// (e2e/ports.mjs: 7520, 7521, 7530, 5273 and 5274 on slot 0), so runs can go side by side.
// `npx playwright test`; HOLON_E2E_SLOT=2 to choose the slot.

import { defineConfig } from "@playwright/test";
import { PORTS, SLOT } from "./e2e/ports.mjs";

const SERVER = `http://127.0.0.1:${PORTS.server}`;
const FRESH = `http://127.0.0.1:${PORTS.fresh}`;

export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.e2e.ts",
  // A folder per slot: a run clears its own when it starts, never one beside it.
  outputDir: `test-results/slot-${SLOT}`,
  // One server, deployments shared between tests: one at a time.
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: { baseURL: `http://localhost:${PORTS.editor}`, trace: "retain-on-failure" },
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
      url: `http://127.0.0.1:${PORTS.oidc}/.well-known/openid-configuration`,
      timeout: 30_000,
      reuseExistingServer: false,
    },
    {
      command: "node e2e/server.mjs --fresh",
      url: `${FRESH}/health`,
      timeout: 900_000,
      reuseExistingServer: false,
      stdout: "pipe",
    },
    {
      command: `npx vite --port ${PORTS.accountsEditor} --strictPort`,
      url: `http://localhost:${PORTS.accountsEditor}`,
      env: { VITE_HOLON_SERVER: FRESH },
      timeout: 120_000,
      reuseExistingServer: false,
    },
    {
      command: `npx vite --port ${PORTS.editor} --strictPort`,
      url: `http://localhost:${PORTS.editor}`,
      env: { VITE_HOLON_SERVER: SERVER },
      timeout: 120_000,
      reuseExistingServer: false,
    },
  ],
});
