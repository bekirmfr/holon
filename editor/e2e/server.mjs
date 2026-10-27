// An isolated Holon server (`holon serve`) for the end-to-end tests: its store, deployments, flows, secrets and
// keys in a fresh temporary folder, its control plane on 127.0.0.1:7520 (the slot's, ports.mjs), so it never touches a
// developer's own server or state, and everything it wrote goes when it stops. Built first and
// run as its own binary (not through `cargo run`), so stopping it stops the server itself.
//
// Its setup code is fixed (HOLON_SETUP_CODE), and once it answers it is set up with the tests'
// admin (fixtures.ts). `node e2e/server.mjs --fresh` starts another on 7521 (likewise) left as installed,
// not set up: the accounts tests (accounts.e2e.ts) set it up through the editor, and sign in
// through the stand-in identity provider (mock-oidc.mjs) once its admin panel says to.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PORTS, RUN_DIR } from "./ports.mjs";

const fresh = process.argv.includes("--fresh");
const port = fresh ? PORTS.fresh : PORTS.server;
export const SETUP_CODE = "E2E0-SETUP";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
execFileSync("cargo", ["build", "-p", "holon"], { cwd: root, stdio: "inherit" });

// A run stopped abruptly (as a test runner stops its servers on Windows) can't clean up after
// itself: the next one does. One run at a time (workers: 1), so any left is stale.
for (const old of readdirSync(tmpdir()).filter((d) => d.startsWith(`holon-e2e-${port}-`))) {
  rmSync(join(tmpdir(), old), { recursive: true, force: true });
}
const dir = mkdtempSync(join(tmpdir(), `holon-e2e-${port}-`));
// Its run file (local.rs) goes beside the other test server's, not into ~/.holon/run (ports.mjs).
const runDir = RUN_DIR;
// Its own ~/.holon, so nothing it doesn't name (its policy, say) is read from the real one.
const holonHome = join(dir, "home");
// CARGO_TARGET_DIR, when set, is where cargo put it (e.g. to build beside a dev server holding
// target/debug's binary open).
// The atlas-notes sample's MongoDB, when the environment names a replica set (CI does:
// HOLON_TEST_MONGODB), in a database of the tests' own: the server fetches its URL from its
// environment, as an admin's connection is.
const mongo = process.env.HOLON_TEST_MONGODB?.replace(/^(mongodb(\+srv)?:\/\/[^/?]+)\/?[^?]*/, "$1/holon_e2e_notes");
const binary = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
// Its policy lets a flow call plain HTTP and private addresses when its deploy asks: the
// chat-proxy test points the sample at a stand-in upstream on loopback.
const policy = join(dir, "holon.toml");
writeFileSync(policy, "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n");
// Its dotenv file (secret_sources.rs): the variables ⚙ offers an env secret; the tests add lines.
const dotenv = join(dir, "secrets.env");
writeFileSync(dotenv, "HOLON_SECRET_GEMINI_API_KEY=e2e-not-a-key\nVAULT_TOKEN=e2e-not-offered\n");
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
    "--policy", policy,
    "--dotenv", dotenv,
  ],
  { cwd: dir, stdio: "inherit", env: { ...process.env, HOLON_SETUP_CODE: SETUP_CODE, HOLON_RUN_DIR: runDir, HOLON_HOME: holonHome, HOLON_DATA: "", HOLON_NO_FOLDER_DIALOG: "1", ...(mongo ? { HOLON_SECRET_NOTES_URL: mongo } : {}) } },
);

// The folders the main one binds for flows to read and write (folders.rs): outside its data, as an
// admin binds them; fixtures.ts names them.
const folders = join(tmpdir(), `holon-e2e-folders-${port}`);
const cleanUp = () => {
  rmSync(dir, { recursive: true, force: true });
  rmSync(folders, { recursive: true, force: true });
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
  // The inbox-report, log-archive, journal, shared-ledger, drop-box, file-audit and drop-board samples' folders, bound as an admin binds
  // them: the ones they only read, read-only; the journal's texts kept a week, the folder capped at
  // 64 KiB.
  const { token } = res.ok ? await res.json() : {};
  rmSync(folders, { recursive: true, force: true });
  const bindings = [
    { name: "inbox", access: "read" },
    { name: "reports", access: "read-write" },
    { name: "logs", access: "read" },
    { name: "archive", access: "read-write" },
    { name: "journal", access: "read-write", keep: "bytes for 7d", cap: "64K" },
    { name: "ledger", access: "read-write" },
    { name: "drop", access: "read" },
    { name: "receipts", access: "read-write" },
    { name: "outbox", access: "read" },
    { name: "audit", access: "read-write" },
    { name: "board", access: "read" },
  ];
  for (const { name, ...binding } of bindings) {
    mkdirSync(join(folders, name), { recursive: true });
    const bound = await fetch(`${base}/admin/folders`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({ name, path: join(folders, name), ...binding }),
    });
    if (!bound.ok) console.error(`e2e server: binding the folder ${name} failed: ${await bound.text()}`);
  }
  // The orders-api and refund-once samples' database (databases.rs), made by its schema as an admin binds it.
  const shop = await fetch(`${base}/admin/databases`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "shop",
      path: join(folders, "shop.db"),
      schema: "create table if not exists orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0); create table if not exists refunds (order_id integer not null, amount numeric not null, status integer)",
    }),
  });
  if (!shop.ok) console.error(`e2e server: binding the database shop failed: ${await shop.text()}`);
  // The ticket-numbers sample's, held for whole runs (an admin's setting: locks.rs).
  const tickets = await fetch(`${base}/admin/databases`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "tickets",
      path: join(folders, "tickets.db"),
      lock: "run",
      schema: "create table if not exists counter (id integer primary key, n integer not null); insert or ignore into counter values (1, 0); create table if not exists issued (n integer primary key, holder text not null)",
    }),
  });
  if (!tickets.ok) console.error(`e2e server: binding the database tickets failed: ${await tickets.text()}`);
  // The approved-payout sample's: one account, a hundred in it.
  const bank = await fetch(`${base}/admin/databases`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "bank",
      path: join(folders, "bank.db"),
      schema: "create table if not exists accounts (id integer primary key, balance integer not null); insert or ignore into accounts values (1, 100); create table if not exists payouts (account integer not null, amount integer not null, status integer)",
    }),
  });
  if (!bank.ok) console.error(`e2e server: binding the database bank failed: ${await bank.text()}`);
  // The atlas-notes sample's, a MongoDB bound by its connection.
  if (mongo) {
    const notes = await fetch(`${base}/admin/databases`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({ name: "notes", connection: "env:HOLON_SECRET_NOTES_URL", access: "read-write" }),
    });
    if (!notes.ok) console.error(`e2e server: binding the database notes failed: ${await notes.text()}`);
  }
}
