// End-to-end tests (e2e/*.e2e.ts): the real editor, in a browser, against a real holon-server.
// Both are started here, apart from a developer's own: the server with throwaway state
// (e2e/server.mjs), the editor pointed at it. Samples deploy on ports the tests set in Settings, so
// they don't clash with deployments already running; it is set up with an admin the tests sign in
// as (e2e/fixtures.ts). Accounts have their own: a server left as installed, an editor pointed at
// it, and a stand-in identity provider (e2e/mock-oidc.mjs). Every port is the run's slot's
// (e2e/ports.mjs: 7520, 7521, 7530, 5273 and 5274 on slot 0), so runs can go side by side.
// `npx playwright test`; HOLON_E2E_SLOT=2 to choose the slot.

import { defineConfig } from "@playwright/test";
import { PORTS, SLOT } from "./e2e/ports.mjs";

const SERVER = `http://127.0.0.1:${PORTS.server}`;
const FRESH = `http://127.0.0.1:${PORTS.fresh}`;
/** How long a test server may take to start, its build included: as long as CI's build step
 *  (e2e.yml, "The server's build"). */
const BUILDS = 45 * 60_000;

export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.e2e.ts",
  // A folder per slot: a run clears its own when it starts, never one beside it.
  outputDir: `test-results/slot-${SLOT}`,
  // One server, deployments shared between tests: one at a time.
  workers: 1,
  timeout: 180_000,
  // For what a page shows of what it holds. What waits on the server's time (what someone may do,
  // a replay, what arrives) waits as long as fixtures.ts's WAITS says, by what it waits for. No
  // test is tried again: one that runs out fails.
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: { baseURL: `http://localhost:${PORTS.editor}`, trace: "retain-on-failure" },
  webServer: [
    {
      command: "node e2e/server.mjs",
      url: `${SERVER}/health`,
      // The first run compiles the server: past 15 minutes once on a cold runner (#116). CI builds
      // it in a step of its own first (e2e.yml), so there this waits on a build already done.
      timeout: BUILDS,
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
      timeout: BUILDS,
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
