// What another page covers does no work (covered.tsx). The editor's canvas and its bottom panes
// stay drawn under the pages, and view mode, left open under a flow's page, read its log every
// 1.5 s and drew its runs again all the while: on a slow machine the page in front had too little
// time left to answer a click (the e2e's ticker test ran out of its time on the hosted runner).
// Here every poller the bottom panes hold is drawn under a page: covered, none reads the server and
// the editor's renders don't reach them; shown again, each reads again, and the log reads on from
// where it was, never with a hole in it.

import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Coverable } from "./covered";
import { useHistory } from "./HistoryPane";
import type { Evaluator } from "./holon";
import { TryRequest } from "./TryRequest";
import { useFlowLog } from "./useFlowLog";

/** The server: a flow with `runs` runs, its log answered as the server pages it (the newest
 *  200 after `after`), and every read it was asked, by path. */
const server = { runs: 0, asked: [] as string[] };
const PAGE = 200;
const answer = (url: string): unknown => {
  const at = new URL(url, location.origin);
  const path = at.pathname;
  server.asked.push(path);
  if (path.endsWith("/log")) {
    const after = Number(at.searchParams.get("after") ?? 0);
    const seqs = Array.from({ length: Math.max(0, server.runs - after) }, (_, i) => after + i + 1).slice(-PAGE);
    return { entries: seqs.map((seq) => ({ seq })), next: server.runs + 1, head: "h", breaks: [], tallies: {} };
  }
  if (path.endsWith("/events")) return { events: [], head: "", breaks: [] };
  if (path.endsWith("/versions")) return [];
  if (path.endsWith("/feeds")) return { feeds: [] };
  if (path.endsWith("/queues")) return { listening: [], sending: [] };
  return {};
};

/** What the log shows, and how many times it was drawn. */
let log: { seqs: number[]; drawn: number; said: string } = { seqs: [], drawn: 0, said: "" };
function LogPane({ said }: { said: string }) {
  const { entries } = useFlowLog("f");
  log = { seqs: entries.map((e) => e.seq), drawn: log.drawn + 1, said };
  return null;
}
function HistoryPane() {
  useHistory("f");
  return null;
}
/** The bottom panes, as App draws them under a page while `covered`. */
function Panes({ covered, said }: { covered: boolean; said: string }): ReactNode {
  return (
    <Coverable covered={covered}>
      <LogPane said={said} />
      <HistoryPane />
      <TryRequest
        ev={{} as Evaluator}
        endpoints={[]}
        errors={[]}
        feeds={[{ feed: "prices", label: "on feed prices", hash: "h" }]}
        queues={[{ queue: "payouts", label: "on queue payouts", hash: "h" }]}
        tickSlug="f"
        liveSlug="f"
      />
    </Coverable>
  );
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});
let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers();
  server.runs = 10;
  server.asked = [];
  log = { seqs: [], drawn: 0, said: "" };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => new Response(JSON.stringify(answer(url)), { status: 200 })),
  );
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const draw = (covered: boolean, said = "") => act(async () => root.render(<Panes covered={covered} said={said} />));
const wait = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));
/** The paths read since the last look, each once. */
const read = () => {
  const paths = [...new Set(server.asked.map((p) => p.replace("/deployments/f", "")))].sort();
  server.asked = [];
  return paths;
};
const EVERY = ["/events", "/feeds", "/log", "/queues", "/versions"];

describe("what another page covers does no work", () => {
  it("reads nothing while covered, and every poller reads again once shown", async () => {
    await draw(false);
    await wait(4000);
    expect(read(), "shown, each poller reads").toEqual(EVERY);
    await draw(true);
    read();
    await wait(30_000);
    expect(read(), "covered, none reads").toEqual([]);
    await draw(false);
    await wait(0);
    expect(read(), "shown again, each reads at once").toEqual(EVERY);
  });

  it("isn't drawn again by the editor while covered, and shows what it was last given once shown", async () => {
    await draw(false, "first");
    await wait(2000);
    await draw(true, "first");
    const drawn = log.drawn;
    for (const said of ["second", "third"]) await draw(true, said);
    await wait(10_000);
    expect(log.drawn, "not drawn again while covered").toBe(drawn);
    expect(log.said).toBe("first");
    await draw(false, "third");
    expect(log.said, "shown, it is drawn with what it was given last").toBe("third");
  });

  it("reads its log on from where it was once shown, and never with a hole in it", async () => {
    await draw(false);
    await wait(2000);
    expect(log.seqs).toEqual(Array.from({ length: 10 }, (_, i) => i + 1));
    // A few runs while covered: they follow on.
    await draw(true);
    server.runs = 15;
    await wait(10_000);
    expect(log.seqs.length, "covered, it reads none of them").toBe(10);
    await draw(false);
    await wait(0);
    expect(log.seqs).toEqual(Array.from({ length: 15 }, (_, i) => i + 1));
    // More than a page while covered: the newest page stands alone, with no hole before it.
    await draw(true);
    server.runs = 15 + 300;
    await wait(10_000);
    await draw(false);
    await wait(0);
    expect(log.seqs).toEqual(Array.from({ length: PAGE }, (_, i) => 315 - PAGE + i + 1));
    // And on from there, as before.
    server.runs = 317;
    await wait(1500);
    expect(log.seqs.slice(-3)).toEqual([315, 316, 317]);
    expect(log.seqs.length).toBe(PAGE + 2);
  });
});
