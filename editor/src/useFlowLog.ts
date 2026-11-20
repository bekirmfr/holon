// A deployment's flow log, polled: everything at first, then only what came after the newest.
// View mode's log and a flow's page read it, each run brief (server.ts's `brief`): its values cut
// to their first part, its timeline left out. A run shown whole (its page, view mode's run, a
// comparison, an export) is read again by its seq (`wholeRun`). Why: whole, the newest 200 runs
// of a flow that builds a page at each step were 33 MB (the owner's hq, 2026-11-20), read on each
// page and tab, and the browser froze. Whoever may see the flow's runs and not read their
// steps (run.steps, R55) is refused the log: then it polls the list of runs instead (`heads`), and
// `steps` keeps the server's refusal, which says who gives it. Covered by another page (covered.tsx),
// it reads nothing, and reads on from where it was once shown again.

import { useEffect, useRef, useState } from "react";
import { useCovered } from "./covered";
import { getLog, getLogHeads, ServerError, type LogEntry, type RunHead, type Tally } from "./server";

const KEEP = 500;
const POLL_MS = 1500;

/** `old` and then `fresh`, the newest KEEP. The first read once shown again (`back`) answers the
 *  newest runs since the last one read, at most a page of them: one that doesn't follow on from
 *  `old` (more came while it was covered) stands alone, so the list never has a hole in it. */
function joined<T extends { seq: number }>(old: T[], fresh: T[], back: boolean): T[] {
  const end = old[old.length - 1];
  if (back && end && fresh[0].seq > end.seq + 1) return fresh.slice(-KEEP);
  return [...old, ...fresh].slice(-KEEP);
}

export function useFlowLog(slug: string) {
  const covered = useCovered();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [heads, setHeads] = useState<RunHead[]>([]);
  const [steps, setSteps] = useState("");
  const [chain, setChain] = useState<{ head: string; breaks: number[] }>({ head: "", breaks: [] });
  const [tallies, setTallies] = useState<Record<string, Tally>>({});
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);
  // Whether the log was refused: from then on, the list of runs.
  const refused = useRef(false);
  // Another flow: its log from the start.
  useEffect(() => {
    last.current = undefined;
    refused.current = false;
    setEntries([]);
    setHeads([]);
    setSteps("");
    setChain({ head: "", breaks: [] });
    setTallies({});
  }, [slug]);
  useEffect(() => {
    if (covered) return;
    let stopped = false;
    // Shown again after a cover, until its first read answers.
    let back = last.current !== undefined;
    const took = (fresh: { seq: number }[], head: string, breaks: number[] | undefined, counted: Record<string, Tally> | undefined) => {
      setChain((old) => (old.head === head && old.breaks.length === (breaks ?? []).length ? old : { head, breaks: breaks ?? [] }));
      setTallies((old) => (JSON.stringify(old) === JSON.stringify(counted ?? {}) ? old : (counted ?? {})));
      if (fresh.length > 0) last.current = fresh[fresh.length - 1].seq;
      setError("");
    };
    const failed = (e: unknown) => !stopped && setError(e instanceof Error ? e.message : String(e));
    const listed = () =>
      getLogHeads(slug, last.current).then(({ entries: fresh, head, breaks, tallies: counted }) => {
        if (stopped) return;
        const was = back;
        back = false;
        took(fresh, head, breaks, counted);
        if (fresh.length > 0) setHeads((old) => joined(old, fresh, was));
      }, failed);
    const poll = () =>
      refused.current
        ? listed()
        : getLog(slug, last.current, { brief: true }).then(
            ({ entries: fresh, head, breaks, tallies: counted }) => {
              if (stopped) return;
              const was = back;
              back = false;
              took(fresh, head, breaks, counted);
              if (fresh.length > 0) setEntries((old) => joined(old, fresh, was));
            },
            (e: unknown) => {
              if (stopped) return;
              if (!(e instanceof ServerError && e.status === 403)) return failed(e);
              refused.current = true;
              setSteps(e.message);
              return listed();
            },
          );
    // One read at a time: a poll while the last is unanswered asks nothing. Why: on a slow server
    // each tick asked the same runs again (the newest isn't known until an answer comes), the
    // reads piled up, each was given up past its time and asked again, and the list stayed empty
    // (a debug server reading 200 runs in 7 s, 2026-11-20).
    let asking = false;
    const next = () => {
      if (asking) return;
      asking = true;
      void poll().finally(() => {
        asking = false;
      });
    };
    next();
    const timer = setInterval(next, POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [slug, covered]);
  return { entries, heads, steps, chain, tallies, error };
}
