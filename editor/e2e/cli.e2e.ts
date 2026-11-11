// The `holon` command against the real server, no browser: what the editor's deployed page, flows
// page and admin page do, done from the command line (docs/manual/reference/cli.md). Each command
// is the built binary, signed in with the e2e admin's token (HOLON_TOKEN), as a script would be.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, OWN_PORTS, RUN_DIR, SAMPLE_SHIFT, SERVER, test } from "./fixtures";
import { gotoPage } from "./helpers";

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
  // Each beat's time under the count before it, a number naming the item as its text.
  expect(ok(adminToken, ["state", slug, "--collection", "times"])).toMatch(/^0\s+\d+$/m);

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
  const invited = ok(adminToken, ["people", "add", person, "--person", "--role", "viewer"]);
  expect(invited).toContain(`${SERVER}/?invite=hi_`);
  expect(ok(adminToken, ["people", "list"])).toMatch(new RegExp(`^${person}\\s+person\\s+viewer\\s+.*invited, not joined yet$`, "m"));
  ok(adminToken, ["people", "remove", person, "--yes"]);

  // A service's token: printed once, on stdout alone; it signs a script in; revoked, it no longer
  // does; the service removed with its tokens.
  const service = `cli-${Date.now()}`;
  ok(adminToken, ["people", "add", service, "--service", "--role", "viewer"]);
  const token = ok(adminToken, ["people", "token", service, "--label", "a script"]).trim();
  expect(token).toMatch(/^ht_\S+$/);
  expect(holon(token, ["deployments"]).status).toBe(0);
  const listed = JSON.parse(ok(adminToken, ["people", "list", "--json"])) as { name: string; tokens?: { id: string }[] }[];
  const id = listed.find((p) => p.name === service)?.tokens?.[0]?.id ?? "";
  expect(ok(adminToken, ["people", "list"])).toMatch(new RegExp(`^${id}\\s+${service}\\s+a script`, "m"));
  ok(adminToken, ["people", "revoke", service, id]);
  expect(holon(token, ["deployments"]).status).toBe(1);
  ok(adminToken, ["people", "remove", service, "--yes"]);
  expect(ok(adminToken, ["oidc"])).toMatch(/^(off|signing in through)/);
});

test("roles, groups and bindings are made and read back from the command line, and People and access offers and lists them", async ({ adminToken, page }) => {
  const n = Date.now();
  const [role, person, group] = [`approver-${n}`, `clib${n}`, `cli-group-${n}`];
  expect(ok(adminToken, ["roles", "define", role, "--allow", "run.view,run.decide", "--about", "decides payouts", "--why", "e2e"])).toMatch(
    new RegExp(`^${role}: allows run\\.view, run\\.decide, held by person \\(version \\d+\\)$`, "m"),
  );
  expect(ok(adminToken, ["roles"])).toMatch(new RegExp(`^${role} \\(decides payouts\\)\\s+person\\s+0\\s+run\\.view, run\\.decide$`, "m"));
  ok(adminToken, ["people", "add", person, "--person", "--role", "viewer"]);

  // Bound at an approval point for a week: a viewer on the host sees the runs, so nothing more is
  // said. A group bound there sees nothing else, and is told what would let it.
  const direct = ok(adminToken, ["bind", role, "--to", person, "--approval", "payout:ok", "--until", "7d", "--why", "covers this week"]);
  expect(direct).toMatch(new RegExp(`^bound ${role} to ${person} for "payout"'s approval "ok", until \\S+ \\(binding \\d+, version \\d+\\)\\n$`));
  ok(adminToken, ["groups", "add", group, person, "--about", "e2e"]);
  expect(ok(adminToken, ["groups"])).toMatch(new RegExp(`^${group} \\(e2e\\)\\s+0\\s+${person}$`, "m"));
  const toGroup = ok(adminToken, ["bind", role, "--group", group, "--approval", "payout:ok"]);
  expect(toGroup).toContain(`the group ${group} decides payout's approval "ok" but can't see its runs: holon bind viewer --group ${group} --flow payout`);
  const listed = ok(adminToken, ["people"]).split("\n").find((l) => l.startsWith(`${person} `)) ?? "";
  expect(listed).toMatch(new RegExp(`^${person}\\s+person\\s+viewer, `));
  for (const holds of [`${role} at payout:ok until `, `${role} at payout:ok via ${group}`]) expect(listed).toContain(holds);

  // People and access lists what else holds beside the role, and offers the admin's own role.
  await page.goto("/");
  await gotoPage(page, "people");
  const bindings = page.getByLabel(`${person}'s bindings`);
  await expect(bindings).toContainText(`${role} at payout:ok until `);
  await expect(bindings).toContainText(`${role} at payout:ok via ${group}`);
  await expect(page.getByLabel(`${person}'s role`)).toHaveValue("viewer");
  await page.getByLabel(`${person}'s role`).selectOption(role);
  await expect(page.getByLabel(`${person}'s role`)).toHaveValue(role);
  // Its role on the host is the admin's own now: a bare entry, not one at a scope.
  expect(ok(adminToken, ["people"])).toMatch(new RegExp(`^${person}\\s+person\\s+(.*, )?${role}(, |\\s)`, "m"));

  // Every change is a version, with who made it and why; each binding has its id.
  const versions = ok(adminToken, ["access"]);
  expect(versions).toMatch(new RegExp(`bound ${role} to ${person} for "payout"'s approval "ok", until \\S+\\s+covers this week`));
  expect(versions).toContain(`defined the role ${role}`);
  expect(ok(adminToken, ["access", "show"])).toMatch(new RegExp(`^\\d+\\s+${role}\\s+the group ${group}\\s+for "payout"'s approval "ok"\\s+-\\s+\\S+$`, "m"));

  // Unbound by their ids; then nothing binds the role, and it goes.
  const idOf = (said: string) => /\(binding (\d+),/.exec(said)?.[1] ?? "";
  expect(ok(adminToken, ["unbind", idOf(direct), "--why", "back from leave"]).trim()).toBe(`unbound ${role} from ${person} for "payout"'s approval "ok"`);
  expect(ok(adminToken, ["unbind", idOf(toGroup)]).trim()).toBe(`unbound ${role} from the group ${group} for "payout"'s approval "ok"`);
  ok(adminToken, ["people", "remove", person, "--yes"]);
  expect(ok(adminToken, ["groups", "remove", group]).trim()).toBe(`${group}: removed`);
  expect(ok(adminToken, ["roles", "remove", role]).trim()).toBe(`${role}: removed`);
  expect(ok(adminToken, ["roles"])).not.toContain(role);
});

test("a flow's log lines are read from the command line, kept beside its runs and not in them, and an agent is refused them", async ({ adminToken, agentToken, request }) => {
  // The traced-payout sample, under a slug of its own, on a port no sample takes in this slot's range.
  const slug = `traced-${Date.now().toString(36)}`;
  const text = readFileSync(join(SAMPLES, "traced-payout.holon.ts"), "utf8").replace("port: 8147", `port: ${OWN_PORTS.traced + SAMPLE_SHIFT}`);
  mkdirSync(RUN_DIR, { recursive: true });
  const path = join(RUN_DIR, `${slug}.holon.ts`);
  writeFileSync(path, text);
  ok(adminToken, ["deploy", path, "--slug", slug, "--approve"]);
  const ask = (body: string) => holon(adminToken, ["call", slug, "POST", "/payout", "-H", "content-type: application/json", "--body", body]);
  expect(ask('{"id":"p-1","amount":250}').out).toContain('"paid":true');
  expect(ask('{"id":"p-2","amount":5000}').out).toContain("over the limit");

  // Each line, with its run and its level; the warning only where a payout was refused.
  expect(ok(adminToken, ["logs", slug])).toMatch(/LOG {3}run 0 {2}payout asked p-1 250$/m);
  const all = ok(adminToken, ["logs", slug]);
  expect(all).toMatch(/DEBUG run 0 {2}paying p-1$/m);
  expect(all).toMatch(/WARN {2}run 1 {2}over the usual limit p-2$/m);
  expect(all).not.toMatch(/DEBUG run 1/);
  expect(ok(adminToken, ["logs", slug, "--level", "warn"]).trim().split("\n")).toHaveLength(1);
  expect(ok(adminToken, ["logs", slug, "--run", "0"]).trim().split("\n")).toHaveLength(2);

  // Beside the runs: no run performed a line. (What a run computed, the template's text, is in its record
  // as any value is; the write of a line is not an effect it performed.)
  const headers = { Authorization: `Bearer ${adminToken}` };
  const entries = ((await (await request.get(`${SERVER}/deployments/${slug}/log`, { headers })).json()) as { entries: { performed?: unknown[] }[] }).entries;
  expect(entries.length).toBeGreaterThanOrEqual(2);
  expect(entries.flatMap((e) => e.performed ?? [])).toEqual([]);
  // And an agent is refused them: no grant lets it read a deployment's logs.
  expect((await request.get(`${SERVER}/deployments/${slug}/logs`, { headers: { Authorization: `Bearer ${agentToken}` } })).status()).toBe(403);

  // Removed with its deployment.
  ok(adminToken, ["stop", slug]);
  ok(adminToken, ["remove", slug, "--yes"]);
  expect(holon(adminToken, ["logs", slug]).status).not.toBe(0);
});

test("a new version is replayed on a deployment's recent runs from the command line before it is deployed", async ({ adminToken }) => {
  // The price-rules sample, under a slug of its own, on a port no sample takes in this slot's range.
  const slug = `price-replay-${Date.now().toString(36)}`;
  const text = readFileSync(join(SAMPLES, "price-rules.holon.ts"), "utf8").replace("port: 8185", `port: ${OWN_PORTS.priceReplay + SAMPLE_SHIFT}`);
  mkdirSync(RUN_DIR, { recursive: true });
  const file = (name: string, body: string) => {
    const path = join(RUN_DIR, `${slug}-${name}.holon.ts`);
    writeFileSync(path, body);
    return path;
  };
  const deployed = file("v1", text);
  ok(adminToken, ["deploy", deployed, "--slug", slug, "--approve"]);
  expect(ok(adminToken, ["replay", deployed, "--on", slug]).trim()).toBe(`"${slug}" has no runs yet to replay this version on.`);
  for (const body of ['{"total":80,"member":true}', '{"total":80}', "{}"]) {
    holon(adminToken, ["call", slug, "POST", "/price", "-H", "content-type: application/json", "--body", body]);
  }
  // The same version answers every run the same: exit 0.
  expect(ok(adminToken, ["replay", deployed, "--on", slug])).toContain("3 replayed · 3 same");
  // A bigger discount: the member's order differs, exit 1, and nothing is deployed.
  const changed = holon(adminToken, ["replay", file("v2", text.replace("total * 0.9", "total * 0.85")), "--on", slug]);
  expect(changed.status, changed.err).toBe(1);
  expect(changed.out).toContain("3 replayed · 2 same · 1 differs");
  expect(changed.out).toMatch(/^ {2}run \d+ {2}POST \/price {2}differs: its answer$/m);
  expect(ok(adminToken, ["replay", file("v2", text.replace("total * 0.9", "total * 0.85")), "--on", slug, "--runs", "1"])).toContain("1 replayed · 1 same");
  expect(holon(adminToken, ["replay", deployed, "--runs", "0"]).status).toBe(2);
  expect(ok(adminToken, ["call", slug, "POST", "/price", "-H", "content-type: application/json", "--body", '{"total":80,"member":true}'])).toContain('"price":72');
  ok(adminToken, ["stop", slug]);
  ok(adminToken, ["remove", slug, "--yes"]);
});
