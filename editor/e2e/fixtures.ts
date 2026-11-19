// The tests against the main test server start signed in as its admin: e2e/server.mjs sets that
// server up with a known setup code and this admin, and every page here begins with that admin's
// session in the browser, as signing in leaves it (server.ts's holon.session). Ports, and the
// folder of run files, are the run's slot's (ports.mjs).

import { mkdtempSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ANSWER_WITHIN, READ_AGAIN_MS } from "../src/limits";
import { test as base, expect, type Page } from "@playwright/test";
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
export const OWN_PORTS = { tagged: 8141, greeting: 8142, total: 8143, priceReplay: 8144, proposal: 8145, traced: 8146, ledgerWriter: 8148 } as const;
/** The server left as installed, for the accounts tests (accounts.e2e.ts). */
export const FRESH = `http://127.0.0.1:${PORTS.fresh}`;
/** The editor pointed at it. */
export const FRESH_EDITOR = `http://localhost:${PORTS.accountsEditor}`;
/** The stand-in identity provider (mock-oidc.mjs). */
export const ISSUER = `http://127.0.0.1:${PORTS.oidc}`;
/** The folders the main test server binds (server.mjs) for the inbox-report and log-archive samples: outside its
 *  data, as an admin binds them. */
export const FOLDERS = join(tmpdir(), `holon-e2e-folders-${PORTS.server}`);
/** A message for a stand-in that reads each file in its folder once, when it next looks (the
 *  queue's sqs-server.mjs, the feeds' sse-server.mjs and ws-server.mjs, every 200ms): written
 *  under a name they pass over, then renamed to `name`. Written in place, a look between the file
 *  made and its text written takes an empty message, for good. */
export function dropWhole(folder: string, name: string, text: string) {
  const aside = join(folder, `.${name}.part`);
  writeFileSync(aside, text);
  renameSync(aside, join(folder, name));
}
export const ADMIN = { name: "e2e", password: "e2e-password-0123" };

/** How long the editor may take to load in a page: its core (the wasm, built unoptimized for the
 *  tests) downloads and compiles before any of its pages shows. In a fresh browser context on CI's
 *  slow runner that took up to 25 s, past the 15 s an expectation waits. */
export const LOADING = 120_000;

/** How long a test waits on the server's time, by what it waits for: set here alone, for the
 *  slowest runner (a self-hosted one, beside a build), and never under what the editor itself
 *  waits for the same (src/limits.ts), so no test gives up on what the page still waits for. An
 *  expectation's own 15 s (playwright.config.ts) is for what a page shows of what it holds; on
 *  a busy runner the server's work ran past it, failing tests with nothing answered wrong (R97).
 *  A wait that runs out still fails: nothing is tried again. */
export const WAITS = {
  /** A read of what the server holds, given up past its time and asked again by the editor
   *  (three tries and the pauses between: 28 s), then the answer shown (15 s). Above all what
   *  someone may do, and the buttons and links that follow it: Deploy, a viewer's Policy link,
   *  what a new binding gives. */
  read: ANSWER_WITHIN.read * (READ_AGAIN_MS.length + 1) + READ_AGAIN_MS.reduce((a, b) => a + b, 0) + 15_000,
  /** A version replayed on a deployment's recent runs: as long as the editor gives it. */
  replay: ANSWER_WITHIN.work,
  /** What the server does in its own time, unasked, and the run that starts: what arrives from
   *  outside, seen at its next look (a file once it stops changing, a mail, a queue's or a feed's
   *  message, a connection made or closed), a clock's tick, a binding that ends; then what that
   *  run writes, after the calls it makes. */
  arrival: 60_000,
};
/** The editors the tests open: the main test server's, and the accounts tests'. */
const EDITORS = [`http://localhost:${PORTS.editor}`, FRESH_EDITOR];

/** Once `page`, loaded, has settled: the editor's page there, and nothing on it saying it's busy
 *  (`aria-busy`: signing in deciding, the editor's core loading). What a test then expects within
 *  15 s is what the page shows once loaded. A page away from the editors isn't waited for. Every
 *  page the fixtures give settles so after each goto and reload. */
export async function settled(page: Page): Promise<void> {
  if (!EDITORS.some((at) => page.url().startsWith(at))) return;
  await page.waitForFunction(() => !!document.getElementById("root")?.firstElementChild && !document.querySelector('[aria-busy="true"]'), undefined, {
    timeout: LOADING,
    polling: 100,
  });
}

/** Once the editor shows in `page`, signed in, its core loaded: after signing in, joining or
 *  setting up, which load it without a navigation. */
export async function editorLoaded(page: Page): Promise<void> {
  await expect(page.locator('.hl-body[aria-busy="false"]')).toBeAttached({ timeout: LOADING });
}

const settling = new WeakSet<Page>();
/** `page`, settling after each goto and reload (`settled`). */
function settles(page: Page): Page {
  if (settling.has(page)) return page;
  settling.add(page);
  const goto = page.goto.bind(page);
  const reload = page.reload.bind(page);
  page.goto = async (url, options) => {
    const res = await goto(url, options);
    await settled(page);
    return res;
  };
  page.reload = async (options) => {
    const res = await reload(options);
    await settled(page);
    return res;
  };
  return page;
}

export const test = base.extend<object, { adminToken: string; agentToken: string }>({
  /** The browser, whose every context's pages settle after each load (`settled`): another
   *  person's editor, opened in a context of its own, as the test's own page. */
  browser: [
    async ({ browser }, use) => {
      const newContext = browser.newContext.bind(browser);
      browser.newContext = async (options) => {
        const context = await newContext(options);
        const newPage = context.newPage.bind(context);
        context.newPage = async () => settles(await newPage());
        return context;
      };
      await use(browser);
    },
    { scope: "worker" },
  ],
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
    await use(settles(page));
  },
});

export { expect };
