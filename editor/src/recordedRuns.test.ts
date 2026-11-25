// Every sample's recorded runs, replayed as view mode replays them.
//
// **Guards:** for each run holon's `every_samples_runs_go_to_the_editor` made of each sample it
// deploys (every one that needs nothing but the server and the hosts it calls), the editor's
// replay over the sample as the editor builds it runs the nodes the run's timeline ran, and view
// mode's verdict is not "replay differs from the run". And every step core held back (core's
// `held`: its tie didn't arrive, or a wired input it needs didn't) is shown not run, never as run,
// and no step shown not run made a call the record holds. The runs are the server's own, written to
// `recorded.fixture.json` by that test (`HOLON_BLESS=1` writes it again).
//
// **Why:** the replay is the editor's own walk of the canvas, beside core's. When core began to
// make a call tied to a branch's trigger even where the branch's answer goes unread (S1's C4), the
// walk still skipped it, and view mode said cache-aside's run, which took its collection's arm,
// differed from itself (#158, found only by an end-to-end test of that one sample). Nothing held
// the walk to the server's runs across the samples. The first run of this test found a second
// case, older than S1: a route answering a subscription (ticker's, live-board's, chess's, …) was
// said to differ because the answer's effect came with its keys in another order. And since C4
// the record notes a held call as a step (it gave nothing, made nothing): the owner's word is that
// such a step is "not run", so every reader says so from the one rule, and the record stays as it
// is (a record without the step changed what a secret-decided run's record says of an effect it
// skipped: "withheld" became "missing", with where).
//
// **If it fails:** a run names its sample, route and the nodes the two disagree on. Where the
// replay skipped or added a node, its roots or its choices (replay.ts `replay`) no longer match
// how core runs the flow; where it ran the same nodes and answered otherwise, a node's value
// differs. A sample whose unit differs from the fixture's needs the fixture blessed again.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { deriveServer, flowUnitHash } from "./canvasOps";
import { buildSample, samples } from "./samples";
import { restoreUnits } from "./docUnits";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { Edge } from "@xyflow/react";
import type { LogEntry } from "./server";
import { nodeExchanges, nodeTimings } from "./replay";
import { replayAsViewed } from "./viewReplay";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

type Fixture = { runs: { sample: string; unit: string; entries: LogEntry[] }[] };
const fixture: Fixture = JSON.parse(readFileSync(join(process.cwd(), "src", "recorded.fixture.json"), "utf8"));
// The samples where a run held a step back, as the sweep finds them.
const heldSeen = new Set<string>();

describe("every sample's recorded runs, replayed as view mode does", () => {
  it("holds runs of the samples, one that calls a host among them", () => {
    expect(fixture.runs.length).toBeGreaterThan(20);
    expect(fixture.runs.some((r) => r.sample === "cache-aside")).toBe(true);
  });

  it.each(fixture.runs.map((r) => [r.sample, r] as const))("%s", async (_, recorded) => {
    const sample = samples().find((s) => s.name === recorded.sample);
    expect(sample, `no sample ${recorded.sample}`).toBeDefined();
    const ev = new Evaluator();
    const doc = buildSample(sample!, ev);
    restoreUnits(doc, ev);
    const nodes = doc.nodes as PrimitiveNodeType[];
    const edges = doc.edges as Edge[];
    const port = sample!.port ?? "";
    expect(flowUnitHash(nodes, edges, ev, port, doc.server ?? {}), "the sample's unit as the fixture ran it: HOLON_BLESS=1 cargo test -p holon --bin holon every_samples_runs_go_to_the_editor").toBe(recorded.unit);
    const { trace } = deriveServer(nodes, edges, ev, port, doc.server ?? {});
    const name = new Map(nodes.map((n) => [n.id, n.data.name]));
    const differs: string[] = [];
    // The worker's event loop goes round between runs: a big sample's take seconds in all.
    const yieldTurn = () => new Promise((r) => setImmediate(r));
    await yieldTurn();
    for (const entry of recorded.entries) {
      await yieldTurn();
      const run = replayAsViewed(nodes, edges, entry, trace, ev);
      const held = run.held ?? new Map();
      if (held.size > 0) heldSeen.add(recorded.sample);
      // Held means shown not run: in notRun, not among what ran, no values.
      for (const id of held.keys()) {
        if (!run.notRun.has(id) || run.order.includes(id) || run.traces.has(id)) differs.push(`${entry.kind} ${entry.route ?? ""}: ${name.get(id)} (${id}) is held and shown as run`);
      }
      // Among the steps the record notes, the same ones as explain reads as not run off the record
      // (the Rust half writes them). A replay goes on past a waiting point the run is parked at,
      // where the record notes nothing: what it holds there is not run too, and not explain's.
      const explained = (entry as LogEntry & { explain_not_run?: string[] }).explain_not_run ?? [];
      const noted = nodeTimings(entry, trace);
      const mine = [...held.keys()].filter((id) => noted.has(id)).sort();
      if (JSON.stringify(mine) !== JSON.stringify([...explained].sort())) differs.push(`${entry.kind} ${entry.route ?? ""}: held [${mine.join(", ")}], explain reads not run [${explained.join(", ")}]`);
      // No step shown not run made a call the record holds.
      for (const x of nodeExchanges(entry, trace)) {
        if (run.notRun.has(x.id)) differs.push(`${entry.kind} ${entry.route ?? ""}: ${name.get(x.id)} (${x.id}) is shown not run and made a call`);
      }
      if (run.ranSame !== false && run.verdict !== false) continue;
      // What the replay walked to: what it shows as run, and what it walked to and found held.
      const ran = new Set([...run.order, ...held.keys()]);
      const timed = new Set(nodeTimings(entry, trace).keys());
      const only = (a: Set<string>, b: Set<string>) => [...a].filter((id) => !b.has(id)).map((id) => `${name.get(id)} (${id})`);
      differs.push(
        `${entry.kind} ${entry.route ?? ""}: ranSame ${run.ranSame}, verdict ${run.verdict}; replay only: [${only(ran, timed).join(", ")}]; run only: [${only(timed, ran).join(", ")}]`,
      );
    }
    // The runs in the message itself: vitest folds equal-looking diffs of several samples into one.
    expect(differs, differs.join("\n")).toEqual([]);
  }, 60_000);

  // After the sweep (tests run in the order written): one held step at least, cache-aside's call.
  it("held a step back in a run: cache-aside's call, when the name is kept", () => {
    expect(heldSeen.has("cache-aside")).toBe(true);
  });
});
