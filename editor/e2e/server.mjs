// An isolated Holon server (`holon serve`) for the end-to-end tests: its store, deployments, flows, secrets and
// keys in a fresh temporary folder, its control plane on 127.0.0.1:7520, so it never touches a
// developer's own server or state, and everything it wrote goes when it stops. Built first and
// run as its own binary (not through `cargo run`), so stopping it stops the server itself.
//
// Its setup code is fixed (HOLON_SETUP_CODE), and once it answers it is set up with the tests'
// admin (fixtures.ts). `node e2e/server.mjs --fresh` starts another on 7521 left as installed,
// not set up: the accounts tests (accounts.e2e.ts) set it up through the editor, and sign in
// through the stand-in identity provider (mock-oidc.mjs) once its admin panel says to.

import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const fresh = process.argv.includes("--fresh");
const port = fresh ? 7521 : 7520;
export const SETUP_CODE = "E2E0-SETUP";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
execFileSync("cargo", ["build", "-p", "holon"], { cwd: root, stdio: "inherit" });

// A run stopped abruptly (as a test runner stops its servers on Windows) can't clean up after
// itself: the next one does. One run at a time (workers: 1), so any left is stale.
for (const old of readdirSync(tmpdir()).filter((d) => d.startsWith(`holon-e2e-${port}-`))) {
  rmSync(join(tmpdir(), old), { recursive: true, force: true });
}
const dir = mkdtempSync(join(tmpdir(), `holon-e2e-${port}-`));
// Its run file (local.rs) goes beside the other test server's, not into ~/.holon/run (fixtures.ts).
const runDir = join(tmpdir(), "holon-e2e-run");
// Its own ~/.holon, so nothing it doesn't name (its policy, say) is read from the real one.
const holonHome = join(dir, "home");
// CARGO_TARGET_DIR, when set, is where cargo put it (e.g. to build beside a dev server holding
// target/debug's binary open).
const binary = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
const server = spawn(
  binary,
  [
    "serve",
    "--at", `127.0.0.1:${port}`,
    "--store", join(dir, "store"),
    "--deployments", join(dir, "deployments.json"),
    "--flows", join(dir, "flows.json"),
    "--secrets", join(dir, "secrets.json"),
    "--secrets-key", `file:${join(dir, "secrets.key")}`,
    "--host-key", join(dir, "host.key"),
    "--accounts", join(dir, "accounts.json"),
  ],
  { cwd: dir, stdio: "inherit", env: { ...process.env, HOLON_SETUP_CODE: SETUP_CODE, HOLON_RUN_DIR: runDir, HOLON_HOME: holonHome, HOLON_DATA: "" } },
);

const cleanUp = () => {
  rmSync(dir, { recursive: true, force: true });
  rmSync(join(runDir, `${port}.json`), { force: true });
};
const stop = () => {
  server.kill();
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
process.on("exit", stop);
server.on("exit", (code) => {
  cleanUp();
  process.exit(code ?? 0);
});

// The main one: set up with the tests' admin as soon as it answers.
if (!fresh) {
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 600; i++) {
    if (await fetch(`${base}/auth/state`).then((r) => r.ok).catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 500));
  }
  const res = await fetch(`${base}/auth/setup`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code: SETUP_CODE, name: "e2e", password: "e2e-password-0123" }),
  });
  if (!res.ok) console.error(`e2e server: setting up its admin failed: ${await res.text()}`);
}
