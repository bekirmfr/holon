// A deployment's flow log, polled: everything at first, then only what came after the newest.
// View mode's log and a flow's page read it. Whoever may see the flow's runs and not read their
// steps (run.steps, R55) is refused the log: then it polls the list of runs instead (`heads`), and
// `steps` keeps the server's refusal, which says who gives it.

import { useEffect, useRef, useState } from "react";
import { getLog, getLogHeads, ServerError, type LogEntry, type RunHead, type Tally } from "./server";

const KEEP = 500;
const POLL_MS = 1500;

export function useFlowLog(slug: string) {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [heads, setHeads] = useState<RunHead[]>([]);
  const [steps, setSteps] = useState("");
  const [chain, setChain] = useState<{ head: string; breaks: number[] }>({ head: "", breaks: [] });
  const [tallies, setTallies] = useState<Record<string, Tally>>({});
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);
  useEffect(() => {
    last.current = undefined;
    setEntries([]);
    setHeads([]);
    setSteps("");
    setChain({ head: "", breaks: [] });
    setTallies({});
    let stopped = false;
    // Whether the log was refused: from then on, the list of runs.
    let refused = false;
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
        took(fresh, head, breaks, counted);
        if (fresh.length > 0) setHeads((old) => [...old, ...fresh].slice(-KEEP));
      }, failed);
    const poll = () =>
      refused
        ? listed()
        : getLog(slug, last.current).then(
            ({ entries: fresh, head, breaks, tallies: counted }) => {
              if (stopped) return;
              took(fresh, head, breaks, counted);
              if (fresh.length > 0) setEntries((old) => [...old, ...fresh].slice(-KEEP));
            },
            (e: unknown) => {
              if (stopped) return;
              if (!(e instanceof ServerError && e.status === 403)) return failed(e);
              refused = true;
              setSteps(e.message);
              return listed();
            },
          );
    void poll();
    const timer = setInterval(() => void poll(), POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [slug]);
  return { entries, heads, steps, chain, tallies, error };
}
