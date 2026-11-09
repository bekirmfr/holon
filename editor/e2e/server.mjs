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
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PORTS, RUN_DIR } from "./ports.mjs";
import { imapServer } from "./imap-server.mjs";
import { smtpSink } from "./smtp-sink.mjs";
import { tokenEndpoint } from "./token-endpoint.mjs";
import { sseServer } from "./sse-server.mjs";
import { wsServer } from "./ws-server.mjs";

const fresh = process.argv.includes("--fresh");
const port = fresh ? PORTS.fresh : PORTS.server;
export const SETUP_CODE = "E2E0-SETUP";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

// The editor's wasm bridge (editor/wasm, not in git) is built by hand; one older than its crates
// lacks what they added since (a new sample has no card on the flows page). The main server
// rebuilds it, as CI does; the tests wait for that server, so they never see the old one.
const newest = (path) => {
  const s = statSync(path);
  if (!s.isDirectory()) return s.mtimeMs;
  return Math.max(0, ...readdirSync(path).map((name) => newest(join(path, name))));
};
const wasm = join(root, "editor", "wasm", "holon_wasm_bg.wasm");
const sources = ["holon-core", "holon-author", "holon-module", "holon-wasm"].map((c) => join(root, "crates", c));
if (!fresh && (!existsSync(wasm) || Math.max(newest(join(root, "Cargo.lock")), ...sources.map(newest)) > statSync(wasm).mtimeMs)) {
  console.log("e2e: the editor's wasm bridge is older than its crates; rebuilding it");
  execFileSync("wasm-pack", ["build", "crates/holon-wasm", "--target", "web", "--out-dir", "../../editor/wasm", "--no-opt"], { cwd: root, stdio: "inherit" });
}

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
// The folders the main one binds for flows to read and write (folders.rs): outside its data, as an
// admin binds them; fixtures.ts names them.
const folders = join(tmpdir(), `holon-e2e-folders-${port}`);
// The approval-mail sample's SMTP server (smtp-sink.mjs), its URL in the dotenv as an admin's
// would be: each mail it takes lands in the folders' mail/, where the tests read it.
const smtp = fresh ? null : await smtpSink(join(folders, "mail"));
// The remittance-mail sample's mailbox (imap-server.mjs): its folders in the folders' mailbox/, a
// message an .eml file in one, where the tests drop them and read where they were moved.
const imap = fresh ? null : await imapServer(join(folders, "mailbox"));
// The support-inbox sample's mailbox and mail, logged in to with a token only, and the endpoint
// the host asks for it (token-endpoint.mjs), as Microsoft 365 is: the mailbox in the folders'
// support-mailbox/, what it sends in their support-mail/.
const oauth = fresh ? null : await tokenEndpoint("e2e-client", "e2e-not-a-secret", "e2e-access-token");
const supportImap = fresh ? null : await imapServer(join(folders, "support-mailbox"), "e2e-access-token");
const supportSmtp = fresh ? null : await smtpSink(join(folders, "support-mail"), "e2e-access-token");
// The price-feed sample's feed of server-sent events (sse-server.mjs): an event a .json file
// dropped in the folders' feed/, its URL in the dotenv with a key in its query, as a provider's.
const feed = fresh ? null : await sseServer(join(folders, "feed"));
// The trading-bot sample's ticker, a WebSocket (ws-server.mjs): a message a .json file dropped in
// the folders' ticker/, what it is sent in its heard.log; its URL in the dotenv, with a key.
const ticker = fresh ? null : await wsServer(join(folders, "ticker"));
// Its dotenv file (secret_sources.rs): the variables ⚙ offers an env secret; the tests add lines.
const dotenv = join(dir, "secrets.env");
writeFileSync(
  dotenv,
  `HOLON_SECRET_GEMINI_API_KEY=e2e-not-a-key\nVAULT_TOKEN=e2e-not-offered\n${smtp ? `HOLON_SECRET_OPS_SMTP=smtp://127.0.0.1:${smtp}\n` : ""}${imap ? `HOLON_SECRET_REMIT_IMAP=imap://remit:e2e-not-a-password@127.0.0.1:${imap}\n` : ""}${oauth ? `HOLON_SECRET_SUPPORT_OAUTH=http://e2e-client:e2e-not-a-secret@127.0.0.1:${oauth}/token?scope=mail\nHOLON_SECRET_SUPPORT_IMAP=imap://support%40acme.com@127.0.0.1:${supportImap}\nHOLON_SECRET_SUPPORT_SMTP=smtp://support%40acme.com@127.0.0.1:${supportSmtp}\n` : ""}${feed ? `HOLON_SECRET_PRICES_URL=http://127.0.0.1:${feed}/events?key=e2e-not-a-key\n` : ""}${ticker ? `HOLON_SECRET_TICKER_URL=ws://127.0.0.1:${ticker}/ws?key=e2e-not-a-key\n` : ""}`,
);
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
  // The inbox-report, log-archive, journal, shared-ledger, drop-box, file-audit, file-checksums, payout-totals, drop-board, stock-sheet, statement-archive and pgp-outbox samples' folders, bound as an admin binds
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
    { name: "sheets", access: "read-write" },
    { name: "payouts", access: "read" },
    { name: "payments", access: "read" },
    { name: "upload", access: "read-write" },
    { name: "download", access: "read" },
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
  // The license-check sample's: the vendor's table of licences, one in it.
  const vendor = await fetch(`${base}/admin/databases`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "vendor",
      path: join(folders, "vendor.db"),
      schema: "create table if not exists licenses (license_key text primary key, plan text not null, seats integer not null); insert or ignore into licenses values ('lic-demo-1234', 'team', 5)",
    }),
  });
  if (!vendor.ok) console.error(`e2e server: binding the database vendor failed: ${await vendor.text()}`);
  // The shared-key sample's: the server's own secret, which may go only to httpbin.dev.
  const shared = await fetch(`${base}/admin/secrets/ops_api_key`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ value: "ops-e2e-key-0123", to: ["httpbin.dev"] }),
  });
  if (!shared.ok) console.error(`e2e server: binding the secret ops_api_key failed: ${await shared.text()}`);
  // The atlas-notes sample's, a MongoDB bound by its connection.
  if (mongo) {
    const notes = await fetch(`${base}/admin/databases`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({ name: "notes", connection: "env:HOLON_SECRET_NOTES_URL", access: "read-write" }),
    });
    if (!notes.ok) console.error(`e2e server: binding the database notes failed: ${await notes.text()}`);
  }
  // The approval-mail sample's mail: through the sink, from the treasury, to anyone at acme.com.
  const ops = await fetch(`${base}/admin/mails`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ name: "ops", smtp: "env:HOLON_SECRET_OPS_SMTP", from: "Treasury <treasury@acme.com>", to: "*@acme.com" }),
  });
  if (!ops.ok) console.error(`e2e server: binding the mail ops failed: ${await ops.text()}`);
  // The remittance-mail sample's mailbox, looked at every 10s, believing mx.acme.com's verdict. Its
  // INBOX made again: the folders were emptied above, after imapServer made it.
  if (imap) mkdirSync(join(folders, "mailbox", "INBOX"), { recursive: true });
  const remit = await fetch(`${base}/admin/mails`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ name: "remit", imap: "env:HOLON_SECRET_REMIT_IMAP", done: "Processed", verdict_from: "mx.acme.com", every: "10s" }),
  });
  if (!remit.ok) console.error(`e2e server: binding the mail remit failed: ${await remit.text()}`);
  // The support-inbox sample's mail, receiving and sending, logged in to both servers with the
  // token the endpoint gives (--oauth). Its INBOX made again, as remit's is.
  if (supportImap) mkdirSync(join(folders, "support-mailbox", "INBOX"), { recursive: true });
  const support = await fetch(`${base}/admin/mails`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "support",
      imap: "env:HOLON_SECRET_SUPPORT_IMAP",
      done: "Answered",
      verdict_from: "mx.acme.com",
      every: "10s",
      smtp: "env:HOLON_SECRET_SUPPORT_SMTP",
      from: "Acme support <support@acme.com>",
      to: "*",
      oauth: "env:HOLON_SECRET_SUPPORT_OAUTH",
    }),
  });
  if (!support.ok) console.error(`e2e server: binding the mail support failed: ${await support.text()}`);
  // The price-feed sample's feed. Its folder made again, as remit's INBOX is.
  if (feed) mkdirSync(join(folders, "feed"), { recursive: true });
  const prices = await fetch(`${base}/admin/feeds`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({ name: "prices", url: "env:HOLON_SECRET_PRICES_URL" }),
  });
  if (!prices.ok) console.error(`e2e server: binding the feed prices failed: ${await prices.text()}`);
  // The trading-bot sample's ticker, bound as its comment says: subscribed to, its ticker messages
  // run, their id the product and sequence, each product's newest kept while runs are slow.
  if (ticker) mkdirSync(join(folders, "ticker"), { recursive: true });
  const tickerBound = await fetch(`${base}/admin/feeds`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify({
      name: "ticker",
      url: "env:HOLON_SECRET_TICKER_URL",
      subscribe: ['{"type":"subscribe","channels":["ticker"],"product_ids":["BTC-USD"]}'],
      only: ["type=ticker"],
      id: "product_id,sequence",
      on_full: "latest",
      key: "product_id",
    }),
  });
  if (!tickerBound.ok) console.error(`e2e server: binding the feed ticker failed: ${await tickerBound.text()}`);
}
