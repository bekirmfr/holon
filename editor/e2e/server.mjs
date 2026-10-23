// An isolated holon-server for the end-to-end tests: its store, deployments, flows, secrets and
// keys in a fresh temporary folder, its control plane on 127.0.0.1:7520, so it never touches a
// developer's own server or state, and everything it wrote goes when it stops. Built first and
// run as its own binary (not through `cargo run`), so stopping it stops the server itself.
//
// `node e2e/server.mjs --signin` starts the one that signs in instead: on 7521, through the
// stand-in identity provider (mock-oidc.mjs, waited for), with ada@example.com an admin and
// cy@example.com a viewer.

import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const signin = process.argv.includes("--signin");
const port = signin ? 7521 : 7520;
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
execFileSync("cargo", ["build", "-p", "holon-server"], { cwd: root, stdio: "inherit" });

// A run stopped abruptly (as a test runner stops its servers on Windows) can't clean up after
// itself: the next one does. One run at a time (workers: 1), so any left is stale.
for (const old of readdirSync(tmpdir()).filter((d) => d.startsWith(`holon-e2e-${port}-`))) {
  rmSync(join(tmpdir(), old), { recursive: true, force: true });
}
const dir = mkdtempSync(join(tmpdir(), `holon-e2e-${port}-`));
const signinArgs = [];
if (signin) {
  const issuer = "http://127.0.0.1:7530";
  for (let i = 0; i < 120; i++) {
    if (await fetch(`${issuer}/.well-known/openid-configuration`).then((r) => r.ok).catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 500));
  }
  writeFileSync(
    join(dir, "users.json"),
    JSON.stringify([
      { name: "ada", role: "admin", email: "ada@example.com" },
      { name: "cy", role: "viewer", email: "cy@example.com" },
    ]),
  );
  signinArgs.push("--oidc-issuer", issuer, "--oidc-client-id", "holon-e2e", "--users-file", join(dir, "users.json"));
}
// CARGO_TARGET_DIR, when set, is where cargo put it (e.g. to build beside a dev server holding
// target/debug's binary open).
const binary = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon-server.exe" : "holon-server");
const server = spawn(
  binary,
  [
    "--store", join(dir, "store"),
    "--deployments", join(dir, "deployments.json"),
    "--flows", join(dir, "flows.json"),
    "--secrets", join(dir, "secrets.json"),
    "--secrets-key", `file:${join(dir, "secrets.key")}`,
    "--host-key", join(dir, "host.key"),
    ...signinArgs,
  ],
  { cwd: dir, stdio: "inherit", env: { ...process.env, HOLON_SERVER_ADDR: `127.0.0.1:${port}` } },
);

const cleanUp = () => rmSync(dir, { recursive: true, force: true });
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
