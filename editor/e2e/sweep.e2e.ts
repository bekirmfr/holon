// Every sample deploys from its text and answers on its port (CLAUDE.md), with no browser. Its text
// goes through the command line (`holon deploy`, which builds the unit the editor's Deploy builds:
// holon-author's flow_unit, here native and there through wasm) to the real server, its port moved
// as the editor's tests move it (SAMPLE_SHIFT). A request no route takes is sent to its port and
// found in its own log: it listens, and what it is sent reaches it. Then it is removed, its state,
// its clocks and its port with it, so nothing after it meets it.
//
// One test, a step a sample: a sample takes under a second, and the shards split tests by count, so
// eighty of them would weigh as eighty of the editor's. Every sample is tried; the failures are
// reported together, each with its sample's name.
//
// What a sample does is held below the browser (the golden record, and a Rust test of its own);
// what the editor makes of each one, by vitest (samples.test.ts builds and derives every sample as
// opening it does); the editor's own Deploy, by the tests that open a sample for what only the
// editor shows.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, HOLON, OWN_PORTS, SAMPLE_SHIFT, SAMPLES, SAMPLES_RS, SERVER, test } from "./fixtures";

/** Each sample's name, its port (none for a flow only a clock or a file starts) and its file, as
 *  samples.rs lists them. */
const SAMPLE_LIST = readFileSync(SAMPLES_RS, "utf8")
  .split(/^\s*Sample \{$/m)
  .slice(1)
  .map((entry) => ({
    name: /^\s*name: "([^"]+)",$/m.exec(entry)?.[1] ?? "",
    port: /^\s*port: Some\("(\d+)"\),$/m.exec(entry)?.[1],
    file: /^\s*text: include_str!\("\.\.\/samples\/([^"]+)"\),$/m.exec(entry)?.[1] ?? "",
  }));

/** Samples whose binding needs a server the environment names (server.mjs binds it only then). */
const NEEDS: Record<string, string> = { "atlas-notes": "HOLON_TEST_MONGODB", "ledger-watch": "HOLON_TEST_POSTGRES" };

test("every sample deploys from its text and answers", async ({ adminToken }) => {
  test.setTimeout(600_000);
  expect(SAMPLE_LIST.length, "samples.rs read").toBeGreaterThan(80);
  for (const s of SAMPLE_LIST) expect(s.name && s.file, JSON.stringify(s)).toBeTruthy();
  // A port the tests' own flows take is no sample's: one there would be found taken here, by
  // whichever of those tests ran before it on this server.
  const taken = Object.entries(OWN_PORTS).flatMap(([own, p]) => SAMPLE_LIST.filter((s) => s.port === String(p)).map((s) => `${s.name} has ${p}, the tests' ${own} flow's (fixtures.ts OWN_PORTS)`));
  expect(taken, "move the tests' flow to a port no sample has").toEqual([]);
  const control = (path: string, method = "GET") =>
    fetch(`${SERVER}${path}`, { method, headers: { Authorization: `Bearer ${adminToken}`, "content-type": "application/json" }, body: method === "POST" ? "{}" : undefined });
  const copies = mkdtempSync(join(tmpdir(), "holon-e2e-sweep-"));
  const failed: string[] = [];
  for (const { name, port, file } of SAMPLE_LIST) {
    if (NEEDS[name] && !process.env[NEEDS[name]]) {
      test.info().annotations.push({ type: "skipped", description: `${name}: its binding needs ${NEEDS[name]}` });
      continue;
    }
    await test.step(name, async () => {
      try {
        let text = readFileSync(join(SAMPLES, file), "utf8");
        const moved = port && Number(port) + SAMPLE_SHIFT;
        if (moved) {
          // The flow's port, in its `flow(…)` line, as Settings moves it.
          const shifted = text.replace(new RegExp(`^(flow\\(.*\\bport: )${port}\\b`, "m"), `$1${moved}`);
          expect(shifted, "its port is in its flow(…) line").not.toBe(text);
          text = shifted;
        }
        const copy = join(copies, file);
        writeFileSync(copy, text);
        execFileSync(HOLON, ["deploy", copy, "--server", SERVER, "--slug", name, "--approve"], { env: { ...process.env, HOLON_TOKEN: adminToken }, stdio: "pipe" });
        try {
          const listed = (await (await control("/deployments")).json()) as { deployments: { slug: string; port: number | null }[] };
          const deployment = listed.deployments.find((d) => d.slug === name);
          expect(deployment, "it is listed").toBeDefined();
          expect(deployment?.port, "its port").toBe(moved || null);
          if (moved) {
            // A path no route takes: whatever the flow answers, the request is a run in its own log.
            const probe = `/e2e-sweep-${Date.now()}`;
            await fetch(`http://127.0.0.1:${moved}${probe}`);
            await expect
              .poll(async () => ((await (await control(`/deployments/${name}/log?limit=5`)).json()) as { entries: { url?: string }[] }).entries.some((e) => e.url?.includes(probe)), { message: "the probe is in its log" })
              .toBe(true);
          }
        } finally {
          // Stopped, then removed (a running one can't be).
          await control(`/deployments/${name}/stop`, "POST");
          const removed = await control(`/deployments/${name}`, "DELETE");
          expect(removed.ok, `removed: ${await removed.text()}`).toBe(true);
        }
      } catch (e) {
        const err = e as { message?: string; stderr?: Buffer };
        failed.push(`${name}: ${(err.stderr?.toString() || err.message || String(e)).trim().split("\n").slice(0, 6).join("\n  ")}`);
      }
    });
  }
  expect(failed, `${failed.length} of ${SAMPLE_LIST.length} samples`).toEqual([]);
});
