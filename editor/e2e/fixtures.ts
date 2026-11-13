// The tests against the main test server start signed in as its admin: e2e/server.mjs sets that
// server up with a known setup code and this admin, and every page here begins with that admin's
// session in the browser, as signing in leaves it (server.ts's holon.session). Ports, and the
// folder of run files, are the run's slot's (ports.mjs).

import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test as base, expect } from "@playwright/test";
import { PORTS, RUN_DIR, SAMPLE_SHIFT } from "./ports.mjs";

export { PORTS, RUN_DIR, SAMPLE_SHIFT };
export const SERVER = `http://127.0.0.1:${PORTS.server}`;
const ROOT = join(fileURLToPath(import.meta.url), "..", "..", "..");
/** The `holon` command server.mjs built and runs the test servers from. */
export const HOLON = join(process.env.CARGO_TARGET_DIR ?? join(ROOT, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
/** The samples' texts (crates/holon-author/samples) and their entries (samples.rs). */
export const SAMPLES = join(ROOT, "crates", "holon-author", "samples");
export const SAMPLES_RS = join(ROOT, "crates", "holon-author", "src", "samples.rs");

/** The sample `file` as a command-line deploy takes it: a copy with its port (in its `flow(…)`
 *  line) moved by SAMPLE_SHIFT, as the editor's tests move it in Settings, and that port; null for
 *  a sample without one. Deployed as it is, a sample listens on its own port, where a developer's
 *  server or another session's may hold it: the deploy still succeeds (the deployment just can't
 *  listen), and a test sent there talks to the other server's flow, with its other data. */
export function movedSample(file: string): { path: string; port: number | null } {
  const text = readFileSync(join(SAMPLES, file), "utf8");
  const line = /^(flow\(.*\bport: )(\d+)\b/m;
  const own = line.exec(text)?.[2];
  const port = own === undefined ? null : Number(own) + SAMPLE_SHIFT;
  const path = join(mkdtempSync(join(tmpdir(), "holon-e2e-sample-")), file);
  writeFileSync(path, port === null ? text : text.replace(line, `$1${port}`));
  return { path, port };
}
/** The ports the tests' own flows take (a flow under a slug of its own), before SAMPLE_SHIFT:
 *  in the samples' range, so a slot's ports stay its own, and in its gaps, so the sweep, which
 *  deploys every sample on its own port moved, never finds one taken. The sweep says so first. */
export const OWN_PORTS = { greeting: 8142, total: 8143, priceReplay: 8144, proposal: 8145, traced: 8146, ledgerWriter: 8148 } as const;
/** The server left as installed, for the accounts tests (accounts.e2e.ts). */
export const FRESH = `http://127.0.0.1:${PORTS.fresh}`;
/** The editor pointed at it. */
export const FRESH_EDITOR = `http://localhost:${PORTS.accountsEditor}`;
/** The stand-in identity provider (mock-oidc.mjs). */
export const ISSUER = `http://127.0.0.1:${PORTS.oidc}`;
/** The folders the main test server binds (server.mjs) for the inbox-report and log-archive samples: outside its
 *  data, as an admin binds them. */
export const FOLDERS = join(tmpdir(), `holon-e2e-folders-${PORTS.server}`);
export const ADMIN = { name: "e2e", password: "e2e-password-0123" };

export const test = base.extend<object, { adminToken: string; agentToken: string }>({
  /** A session of the test server's admin, for calls to its control plane: one per worker, since
   *  signing in hashes the password (argon2, slow in the debug build the tests run) and no test
   *  here ends it (the accounts tests sign out on a server of their own). */
  adminToken: [
    async ({ playwright }, use) => {
      const request = await playwright.request.newContext();
      // The server answers /health a moment before server.mjs has set it up: wait for that (not
      // by trying to sign in, which a few failures would lock).
      for (let i = 0; i < 60; i++) {
        const state = (await (await request.get(`${SERVER}/auth/state`)).json()) as { setup: boolean };
        if (!state.setup) break;
        await new Promise((r) => setTimeout(r, 500));
      }
      const res = await request.post(`${SERVER}/auth/sign-in`, { data: ADMIN });
      expect(res.ok(), await res.text()).toBe(true);
      const { token } = (await res.json()) as { token: string };
      await request.dispose();
      await use(token);
    },
    { scope: "worker" },
  ],
  /** A token of an agent the admin owns, as `holon mcp` signs in: the agent made once per
   *  worker (kept from an earlier run on a reused server), a new token each time. */
  agentToken: [
    async ({ playwright, adminToken }, use, worker) => {
      const request = await playwright.request.newContext({ extraHTTPHeaders: { Authorization: `Bearer ${adminToken}` } });
      const name = `e2e-agent-${worker.workerIndex}`;
      const people = (await (await request.get(`${SERVER}/people`)).json()) as { name: string }[];
      if (!people.some((p) => p.name === name)) {
        const added = await request.post(`${SERVER}/people`, { data: { name, kind: "agent", role: "agent" } });
        expect(added.ok(), await added.text()).toBe(true);
      }
      const made = await request.post(`${SERVER}/people/${name}/tokens`, { data: { label: "e2e" } });
      expect(made.ok(), await made.text()).toBe(true);
      const { token } = (await made.json()) as { token: string };
      await request.dispose();
      await use(token);
    },
    { scope: "worker" },
  ],
  page: async ({ page, adminToken }, use) => {
    await page.addInitScript((session) => localStorage.setItem("holon.session", session), JSON.stringify({ token: adminToken, name: ADMIN.name, kind: "person", role: "admin" }));
    await use(page);
  },
});

export { expect };
