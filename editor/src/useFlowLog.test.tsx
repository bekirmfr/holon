// A flow's log, polled (useFlowLog): whoever may read a run's steps gets the log; whoever is refused
// them (run.steps, R55) gets the list of runs instead, with the server's refusal, which says who
// gives it, and the log is not asked again.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { LogEntry, RunHead } from "./server";

const server = vi.hoisted(() => ({ getLog: vi.fn(), getLogHeads: vi.fn() }));
vi.mock("./server", async (original) => ({ ...(await original<typeof import("./server")>()), ...server }));

import { ServerError } from "./server";
import { useFlowLog } from "./useFlowLog";

const REFUSED = "cy may not read \"pay\"'s runs' steps and values (explain, a run's detail, export, replay): no role cy holds there holds run.steps; a person gives it with `holon bind viewer --to cy --flow pay --why …`";
const HEAD: RunHead = { seq: 3, session: 1, at_ms: 0, duration_ms: 2, kind: "request", route: "/check", method: "POST", status: 200, failed: false, left_state: false, by: "cy", run_id: "r3", unit: "u", parked: [], retries: 0, lost_since: null };
const page = <T,>(entries: T[]) => ({ entries, next: 0, head: "h", breaks: [] });

let seen: ReturnType<typeof useFlowLog> | null = null;
function Probe() {
  seen = useFlowLog("pay");
  return null;
}

let root: Root | null = null;
beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  seen = null;
  vi.useRealTimers();
  server.getLog.mockReset();
  server.getLogHeads.mockReset();
});

async function mounted() {
  vi.useFakeTimers();
  root = createRoot(document.createElement("div"));
  await act(async () => root!.render(<Probe />));
  await act(async () => void (await vi.advanceTimersByTimeAsync(0)));
}

describe("a flow's log, polled", () => {
  it("whoever may read a run's steps gets the log, and never the list", async () => {
    server.getLog.mockResolvedValue(page([{ seq: 1 } as LogEntry]));
    await mounted();
    expect(seen!.entries.map((e) => e.seq)).toEqual([1]);
    expect(seen!.steps).toBe("");
    expect(server.getLogHeads).not.toHaveBeenCalled();
    // Brief, its values cut: a run shown whole is read again by its seq.
    expect(server.getLog).toHaveBeenLastCalledWith("pay", undefined, { brief: true });
  });

  it("refused the steps, it says so in the server's words and polls the list of runs instead", async () => {
    server.getLog.mockRejectedValue(new ServerError(403, REFUSED));
    server.getLogHeads.mockResolvedValue(page([HEAD]));
    await mounted();
    expect(seen!.steps).toBe(REFUSED);
    expect(seen!.heads).toEqual([HEAD]);
    expect(seen!.entries).toEqual([]);
    expect(seen!.error).toBe("");
    // The next poll asks the list from the newest it has, and not the log again.
    server.getLogHeads.mockResolvedValue(page([]));
    await act(async () => void (await vi.advanceTimersByTimeAsync(1500)));
    expect(server.getLog).toHaveBeenCalledTimes(1);
    expect(server.getLogHeads).toHaveBeenLastCalledWith("pay", 3);
  });

  it("asks one read at a time: a slow answer is waited for, never asked again over it", async () => {
    let answer: (v: unknown) => void = () => {};
    server.getLog.mockReturnValueOnce(new Promise((resolve) => (answer = resolve)));
    await mounted();
    await act(async () => void (await vi.advanceTimersByTimeAsync(4500)));
    expect(server.getLog, "three ticks while the first read is unanswered").toHaveBeenCalledTimes(1);
    server.getLog.mockResolvedValue(page([]));
    await act(async () => answer(page([{ seq: 5 } as LogEntry])));
    await act(async () => void (await vi.advanceTimersByTimeAsync(1500)));
    expect(server.getLog).toHaveBeenCalledTimes(2);
    expect(server.getLog).toHaveBeenLastCalledWith("pay", 5, { brief: true });
  });

  it("any other failure is an error, not a refusal of the steps", async () => {
    server.getLog.mockRejectedValue(new ServerError(500, "the server failed"));
    await mounted();
    expect(seen!.error).toBe("the server failed");
    expect(seen!.steps).toBe("");
    expect(server.getLogHeads).not.toHaveBeenCalled();
  });
});
