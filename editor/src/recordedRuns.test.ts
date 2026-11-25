// Every sample's recorded runs, replayed as view mode replays them.
//
// **Guards:** for each run holon's `every_samples_runs_go_to_the_editor` made of each sample it
// deploys (every one that needs nothing but the server and the hosts it calls), the editor's
// replay over the sample as the editor builds it runs the nodes the run's timeline ran, and view
// mode's verdict is not "replay differs from the run". The runs are the server's own, written to
// `recorded.fixture.json` by that test (`HOLON_BLESS=1` writes it again).
//
// **Why:** the replay is the editor's own walk of the canvas, beside core's. When core began to
// make a call tied to a branch's trigger even where the branch's answer goes unread (S1's C4), the
// walk still skipped it, and view mode said cache-aside's run, which took its collection's arm,
// differed from itself (#158, found only by an end-to-end test of that one sample). Nothing held
// the walk to the server's runs across the samples. The first run of this test found a second
// case, older than S1: a route answering a subscription (ticker's, live-board's, chess's, …) was
// said to differ because the answer's effect came with its keys in another order.
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
import { nodeTimings } from "./replay";
import { replayAsViewed } from "./viewReplay";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

type Fixture = { runs: { sample: string; unit: string; entries: LogEntry[] }[] };
const fixture: Fixture = JSON.parse(readFileSync(join(process.cwd(), "src", "recorded.fixture.json"), "utf8"));

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
      if (run.ranSame !== false && run.verdict !== false) continue;
      const ran = new Set(run.order);
      const timed = new Set(nodeTimings(entry, trace).keys());
      const only = (a: Set<string>, b: Set<string>) => [...a].filter((id) => !b.has(id)).map((id) => `${name.get(id)} (${id})`);
      differs.push(
        `${entry.kind} ${entry.route ?? ""}: ranSame ${run.ranSame}, verdict ${run.verdict}; replay only: [${only(ran, timed).join(", ")}]; run only: [${only(timed, ran).join(", ")}]`,
      );
    }
    // The runs in the message itself: vitest folds equal-looking diffs of several samples into one.
    expect(differs, differs.join("\n")).toEqual([]);
  }, 60_000);
});
