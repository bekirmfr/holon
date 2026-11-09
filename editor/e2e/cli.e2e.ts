// The `holon` command against the real server, no browser: what the editor's deployed page, flows
// page and admin page do, done from the command line (docs/manual/reference/cli.md). Each command
// is the built binary, signed in with the e2e admin's token (HOLON_TOKEN), as a script would be.

import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, SERVER, test } from "./fixtures";

const root = join(fileURLToPath(import.meta.url), "..", "..", "..");
const HOLON = join(process.env.CARGO_TARGET_DIR ?? join(root, "target"), "debug", process.platform === "win32" ? "holon.exe" : "holon");
const SAMPLES = join(root, "crates", "holon-author", "samples");

type Ran = { status: number; out: string; err: string };

/** Run `holon args… --server SERVER`, with `input` piped in: its exit code, stdout and stderr. */
function holon(token: string, args: string[], input = ""): Ran {
  try {
    const out = execFileSync(HOLON, [...args, "--server", SERVER], { env: { ...process.env, HOLON_TOKEN: token }, input, encoding: "utf8", stdio: "pipe" });
    return { status: 0, out, err: "" };
  } catch (e) {
    const failed = e as { status: number; stdout: string; stderr: string };
    return { status: failed.status, out: failed.stdout, err: failed.stderr };
  }
}

/** `holon args…`, which must succeed: its stdout. */
function ok(token: string, args: string[], input = ""): string {
  const ran = holon(token, args, input);
  expect(ran.status, `holon ${args.join(" ")}: ${ran.err}`).toBe(0);
  return ran.out;
}

test("a deployment is run, called, read and changed from the command line, as its card does", async ({ adminToken }) => {
  // Ticker's own slug: its port is its text's, so a browser test's ticker is this one, redeployed.
  ok(adminToken, ["deploy", join(SAMPLES, "ticker.holon.ts"), "--approve"]);
  holon(adminToken, ["run", "ticker"]);
  expect(ok(adminToken, ["deployments"])).toMatch(/^ticker\s+running\s+:\d+/m);
  expect(ok(adminToken, ["deployments", "ticker"])).toContain("waiting    no run");

  // Its action, by hand: its answer, and the run it was.
  const reset = holon(adminToken, ["action", "ticker", "reset", "--input", '{"to": 5}']);
  expect(reset.status).toBe(0);
  expect(JSON.parse(reset.out)).toMatchObject({ set: 5 });
  expect(holon(adminToken, ["action", "ticker", "nope"]).status).toBe(1);

  // Its clock, now: what it did.
  expect(ok(adminToken, ["tick", "ticker", "every 2s"])).toContain('did        StateWrite("ticks")');
  // A request, through the server, as the request panel sends it.
  expect(Number(ok(adminToken, ["call", "ticker", "GET", "/count"]).trim())).toBeGreaterThanOrEqual(6);
  expect(holon(adminToken, ["call", "ticker", "GET", "/nowhere"]).status).toBe(1);
  // Its state, a value at a time, as the language writes them.
  expect(ok(adminToken, ["state", "ticker"])).toMatch(/^ticks\s+\d+$/m);

  // Held and started again, from the state it had.
  expect(ok(adminToken, ["pause", "ticker"]).trim()).toBe("ticker: paused");
  expect(ok(adminToken, ["run", "ticker"]).trim()).toBe("ticker: running");
  expect(ok(adminToken, ["state", "ticker", "--sessions"])).toMatch(/^session \d+ {2}from /m);
});

test("a clock flow is run by hand, its grant and secrets changed, and it is removed, from the command line", async ({ adminToken }) => {
  const slug = `cli-beats-${Date.now()}`;
  ok(adminToken, ["deploy", join(SAMPLES, "heartbeat.holon.ts"), "--slug", slug, "--approve"]);
  // Its clock beats on its own too, every 10s.
  expect(ok(adminToken, ["tick", slug, "every 10s"])).toMatch(/^did {8}StateWrite\("beats"\) \d+$/m);
  expect(ok(adminToken, ["state", slug])).toMatch(/^beats\s+\d+$/m);

  // Its grant: an item taken back pauses it; given back, it runs again.
  expect(ok(adminToken, ["grant", slug])).toContain('StateWrite("beats")');
  const revoked = ok(adminToken, ["grant", slug, "--revoke", 'StateWrite("beats")']);
  expect(revoked).toContain(`${slug} (paused)`);
  expect(revoked).toContain('revoked      StateWrite("beats")');
  expect(ok(adminToken, ["grants"])).toContain(`${slug} (paused)`);
  expect(ok(adminToken, ["grant", slug, "--grant", 'StateWrite("beats")'])).toContain(`${slug} (running)`);
  const policy = ok(adminToken, ["policy"]);
  expect(policy).toMatch(/^keep_runs\s+\d+\s+\d+$/m);
  expect(policy).toMatch(/^agents propose\s+allowed: what needs a person waits for one$/m);

  // A secret, piped in and never shown; rolled back after a dry run that names values by fingerprint.
  const value = `e2e-secret-${Date.now()}`;
  expect(ok(adminToken, ["secret", "set", slug, "api_key"], `${value}\n`).trim()).toBe(`api_key: version 1 set for ${slug}`);
  ok(adminToken, ["secret", "set", slug, "api_key"], `${value}-two\n`);
  const listed = ok(adminToken, ["secret", "list", slug]);
  expect(listed).toMatch(/^api_key\s+v2\s+/m);
  expect(listed).not.toContain(value);
  const dry = ok(adminToken, ["secret", "rollback", slug, "api_key", "--to", "1", "--dry-run"]);
  expect(dry).toMatch(/would make v1 \(#[0-9a-f]+, from local\) current again as v3/);
  expect(dry).not.toContain(value);
  expect(ok(adminToken, ["secret", "history", slug, "api_key"])).toContain("set v2 from local");
  expect(holon(adminToken, ["secret", "set", slug, "api_key"]).status).toBe(2);
  ok(adminToken, ["secret", "remove", slug, "api_key"]);

  // Removed: only once stopped, and only when asked twice.
  expect(holon(adminToken, ["remove", slug]).status).toBe(2);
  expect(holon(adminToken, ["remove", slug, "--yes"]).err).toContain("before deleting it");
  ok(adminToken, ["stop", slug]);
  expect(ok(adminToken, ["remove", slug, "--yes"]).trim()).toBe(`${slug}: removed, its secrets forgotten`);
  expect(ok(adminToken, ["deployments"])).not.toContain(slug);
});

test("flows are saved and read back, and people and tokens managed, from the command line, as the flows and admin pages do", async ({ adminToken }) => {
  const name = `cli-flow-${Date.now()}`;
  ok(adminToken, ["flows", "save", join(SAMPLES, "heartbeat.holon.ts"), "--name", name]);
  expect(ok(adminToken, ["flows"])).toMatch(new RegExp(`^${name}\\s+\\S+\\s+\\S+\\s+no$`, "m"));
  expect(ok(adminToken, ["flows", "show", name])).toContain('flow("heartbeat")\n\n// Each beat');
  ok(adminToken, ["flows", "remove", name]);
  expect(ok(adminToken, ["activity", "--last", "5"])).toMatch(new RegExp(`deleted flow\\s+${name}`));

  // A person invited: a link to the editor, once; then removed.
  const person = `cli${Date.now()}`;
  const invited = ok(adminToken, ["user", "invite", person, "--role", "viewer"]);
  expect(invited).toContain(`${SERVER}/?invite=hi_`);
  expect(ok(adminToken, ["user", "list"])).toMatch(new RegExp(`^${person}\\s+viewer\\s+.*invited, not joined yet$`, "m"));
  ok(adminToken, ["user", "remove", person, "--yes"]);

  // A token: printed once, on stdout alone; it signs a script in; revoked, it no longer does.
  const label = `cli-${Date.now()}`;
  const token = ok(adminToken, ["token", "add", label, "--role", "viewer"]).trim();
  expect(token).toMatch(/^ht_\S+$/);
  expect(holon(token, ["deployments"]).status).toBe(0);
  const id = (JSON.parse(ok(adminToken, ["token", "list", "--json"])) as { id: string; label: string }[]).find((t) => t.label === label)?.id ?? "";
  expect(ok(adminToken, ["token", "list"])).toContain(id);
  ok(adminToken, ["token", "revoke", id]);
  expect(holon(token, ["deployments"]).status).toBe(1);
  expect(ok(adminToken, ["oidc"])).toMatch(/^(off|signing in through)/);
});
