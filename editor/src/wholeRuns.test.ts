// A run is read whole by its seq, and the lists read runs brief (server.ts's `getRun`, `wholeRun`;
// useFlowLog). Why: the console read the newest 200 runs whole for each list, page and tab, 33 MB
// on the owner's hq flow, and the browser froze; a run's page asked for the newest 200 after the
// run before it, which read as much again and missed any run older than them ("The log here
// holds no run #12" for a run the log holds), 2026-11-20.
// If it fails: a read of runs went back to paging the log from a run on; read one by `getRun` or
// `wholeRun`, and a list's with `brief`.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getRun, setSession, wholeRun, wholeRuns, type LogEntry } from "./server";

/** A log of `RUNS` runs (seq 0 on), paged as the server pages it (deploy.rs's `log_page`): the
 *  newest `limit` (200) after `after`, up to `through`; brief ones say so. */
const RUNS = 300;
const asked: URLSearchParams[] = [];
const answer = (url: string) => {
  const q = new URL(url, location.origin).searchParams;
  asked.push(q);
  const after = q.has("after") ? Number(q.get("after")) : -1;
  const through = q.has("through") ? Number(q.get("through")) : Infinity;
  const limit = Number(q.get("limit") ?? 200);
  const seqs = Array.from({ length: RUNS }, (_, i) => i).filter((s) => s > after && s <= through).slice(-limit);
  return { entries: seqs.map((seq) => ({ seq, ...(q.get("brief") === "1" ? { brief: true } : {}) })), next: RUNS, head: "h", breaks: [] };
};

beforeEach(() => {
  asked.length = 0;
  setSession({ token: "t", name: "cy", kind: "person", role: "viewer" });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => new Response(JSON.stringify(answer(url)), { status: 200 })),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
  setSession(null);
});

describe("a run read whole", () => {
  it("is read by its seq, with the runs either side of it, however old", async () => {
    for (const [seq, before, after] of [
      [12, 11, 13],
      [0, null, 1],
      [RUNS - 1, RUNS - 2, null],
    ] as const) {
      asked.length = 0;
      const r = await getRun("hq", seq);
      expect([r.run?.seq, r.before, r.after], `run ${seq}`).toEqual([seq, before, after]);
      expect(r.run?.brief, "whole").toBeUndefined();
      expect(asked.map((q) => q.get("through")), "one read, up to the run after it").toEqual([String(seq + 1)]);
    }
  });

  it("one read brief is read again by its seq; one read whole is as it is", async () => {
    const brief = { seq: 40, brief: true } as LogEntry;
    const whole = await wholeRun("hq", brief);
    expect(whole).toEqual({ seq: 40 });
    expect(asked.map((q) => [q.get("after"), q.get("through"), q.get("brief")])).toEqual([["39", "40", null]]);
    asked.length = 0;
    expect(await wholeRun("hq", whole!)).toBe(whole);
    expect(asked).toEqual([]);
    expect((await wholeRuns("hq", [brief, { seq: 7, brief: true } as LogEntry])).map((r) => r.seq)).toEqual([40, 7]);
  });

  it("every read of the log is a list's, brief, or one run's by its seq", () => {
    // Each call of getLog outside server.ts: only useFlowLog's, which asks brief.
    const dir = join(process.cwd(), "src");
    const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f) && f !== "server.ts");
    const calls: string[] = [];
    for (const file of files) {
      const source = ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "getLog") {
          calls.push(`${file}: ${node.getText(source).replace(/\s+/g, " ")}`);
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
    expect(calls).toEqual(["useFlowLog.ts: getLog(slug, last.current, { brief: true })"]);
  });
});
