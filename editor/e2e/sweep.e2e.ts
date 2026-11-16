// Every sample deploys from its text and answers on its port (CLAUDE.md), with no browser. Its text
// goes through the command line (`holon deploy`, which builds the unit the editor's Deploy builds:
// holon-author's flow_unit, here native and there through wasm) to the real server, its port moved
// as the editor's tests move it (SAMPLE_SHIFT). A request no route takes is sent to its port and
// found in its own log: it listens, and what it is sent reaches it. Then it is removed, its state,
// its clocks and its port with it, so nothing after it meets it.
//
// A test for every few samples (SAMPLES_PER_TEST), each sample a step with a limit of its own
// (PER_SAMPLE), the test's limit theirs together: the limit follows the work. A sample added adds
// its own time, and a slow runner fails the sample that is slow, by its name, not the sweep at
// whichever sample one bound over them all ran out (one test with ten minutes, at 86 samples).
// A few to a test, not one: CI's two shards (`--fully-parallel --shard`) cut the tests, in file
// order, into halves by count, and a sample takes about 3.6 s on a quiet runner where an editor
// test takes about 16 s (main on 2026-11-16: 86 samples in 306 s, the 108 other tests in 29 min).
// Four samples weigh about as one editor test, so the shards' halves come out about even; one a
// test would put nearly every editor test in the first. The samples a test holds are each tried,
// and their failures reported together, each by its sample's name.
//
// Every sample has a test: the first test here compares the samples the tests hold with the ones
// `holon new --help` lists, which the command reads from samples.rs's own list, compiled in: an
// entry this file's reading of samples.rs misses, or reads wrong, fails it.
//
// Nothing is done once for the tests together: the server is the suite's (playwright.config.ts),
// and a sample's test leaves nothing behind, so they run in any order, in either shard.
//
// What a sample does is held below the browser (the golden record, and a Rust test of its own);
// what the editor makes of each one, by vitest (samples.test.ts builds and derives every sample as
// opening it does); the editor's own Deploy, by the tests that open a sample for what only the
// editor shows.

import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { promisify } from "node:util";
import { expect, HOLON, movedSample, OWN_PORTS, SAMPLE_SHIFT, SAMPLES_RS, SERVER, test } from "./fixtures";

const run = promisify(execFile);

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

/** How many samples a test holds: together about one editor test's time (above). */
const SAMPLES_PER_TEST = 4;
/** A sample's limit: deployed, sent a request, found in its log and removed. 3.6 s on average on a
 *  quiet runner, the slowest (chess) four times that; a runner whose host was busy ran 4 to 6
 *  times slower (pull request 95), and this holds the slowest at that with room. */
const PER_SAMPLE = 120_000;

const GROUPS = Array.from({ length: Math.ceil(SAMPLE_LIST.length / SAMPLES_PER_TEST) }, (_, i) => SAMPLE_LIST.slice(i * SAMPLES_PER_TEST, (i + 1) * SAMPLES_PER_TEST));

/** A call to the test server's control plane, as its admin. */
function control(adminToken: string, path: string, method = "GET") {
  return fetch(`${SERVER}${path}`, { method, headers: { Authorization: `Bearer ${adminToken}`, "content-type": "application/json" }, body: method === "POST" ? "{}" : undefined });
}

test("every sample samples.rs lists has a test here, and none has a port the tests' own flows take", async () => {
  // The samples the command lists, by name, from samples.rs's list as it was compiled.
  const { stdout } = await run(HOLON, ["new", "--help"]);
  const listed = stdout.slice(stdout.indexOf("\nsamples:") + "\nsamples:".length).split(/[\s,]+/).filter(Boolean);
  expect(listed.length, "holon new --help lists the samples").toBeGreaterThan(80);
  for (const s of SAMPLE_LIST) expect(s.name && s.file, `samples.rs read: ${JSON.stringify(s)}`).toBeTruthy();
  expect(GROUPS.flat().map((s) => s.name), "the samples the tests hold, in samples.rs's order").toEqual(listed);
  // A port the tests' own flows take is no sample's: one there would be found taken here, by
  // whichever of those tests ran before it on this server.
  const taken = Object.entries(OWN_PORTS).flatMap(([own, p]) => SAMPLE_LIST.filter((s) => s.port === String(p)).map((s) => `${s.name} has ${p}, the tests' ${own} flow's (fixtures.ts OWN_PORTS)`));
  expect(taken, "move the tests' flow to a port no sample has").toEqual([]);
});

for (const group of GROUPS) {
  test(`${group.map((s) => s.name).join(", ")}: each deploys from its text and answers`, async ({ adminToken }) => {
    test.setTimeout(PER_SAMPLE * group.length + 15_000);
    const failed: string[] = [];
    for (const { name, port, file } of group) {
      if (NEEDS[name] && !process.env[NEEDS[name]]) {
        test.info().annotations.push({ type: "skipped", description: `${name}: its binding needs ${NEEDS[name]}` });
        continue;
      }
      try {
        await test.step(
          name,
          async () => {
            // The flow's port, in its `flow(…)` line, as Settings moves it.
            const { path: copy, port: shifted } = movedSample(file);
            const moved = port && Number(port) + SAMPLE_SHIFT;
            expect(shifted, "its port is in its flow(…) line, as samples.rs says").toBe(moved || null);
            await run(HOLON, ["deploy", copy, "--server", SERVER, "--slug", name, "--approve"], { env: { ...process.env, HOLON_TOKEN: adminToken }, timeout: PER_SAMPLE });
            try {
              const listed = (await (await control(adminToken, "/deployments")).json()) as { deployments: { slug: string; port: number | null }[] };
              const deployment = listed.deployments.find((d) => d.slug === name);
              expect(deployment, "it is listed").toBeDefined();
              expect(deployment?.port, "its port").toBe(moved || null);
              if (moved) {
                // A path no route takes: whatever the flow answers, the request is a run in its own log.
                const probe = `/e2e-sweep-${Date.now()}`;
                await fetch(`http://127.0.0.1:${moved}${probe}`);
                await expect
                  .poll(async () => ((await (await control(adminToken, `/deployments/${name}/log?limit=5`)).json()) as { entries: { url?: string }[] }).entries.some((e) => e.url?.includes(probe)), {
                    message: "the probe is in its log",
                  })
                  .toBe(true);
              }
            } finally {
              // Stopped, then removed (a running one can't be).
              await control(adminToken, `/deployments/${name}/stop`, "POST");
              const removed = await control(adminToken, `/deployments/${name}`, "DELETE");
              expect(removed.ok, `removed: ${await removed.text()}`).toBe(true);
            }
          },
          { timeout: PER_SAMPLE },
        );
      } catch (e) {
        const err = e as { message?: string; stderr?: string };
        failed.push(`${name}: ${(err.stderr || err.message || String(e)).trim().split("\n").slice(0, 6).join("\n  ")}`);
      }
    }
    expect(failed, `${failed.length} of ${group.length} samples`).toEqual([]);
  });
}
