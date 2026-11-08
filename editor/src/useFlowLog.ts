// A deployment's flow log, polled: everything at first, then only what came after the newest.
// View mode's log and a flow's page read it.

import { useEffect, useRef, useState } from "react";
import { getLog, type LogEntry, type Tally } from "./server";

const KEEP = 500;
const POLL_MS = 1500;

export function useFlowLog(slug: string) {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [chain, setChain] = useState<{ head: string; breaks: number[] }>({ head: "", breaks: [] });
  const [tallies, setTallies] = useState<Record<string, Tally>>({});
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);
  useEffect(() => {
    last.current = undefined;
    setEntries([]);
    setChain({ head: "", breaks: [] });
    setTallies({});
    let stopped = false;
    const poll = () =>
      getLog(slug, last.current)
        .then(({ entries: fresh, head, breaks, tallies: counted }) => {
          if (stopped) return;
          setChain((old) => (old.head === head && old.breaks.length === (breaks ?? []).length ? old : { head, breaks: breaks ?? [] }));
          setTallies((old) => (JSON.stringify(old) === JSON.stringify(counted ?? {}) ? old : (counted ?? {})));
          if (fresh.length === 0) return;
          last.current = fresh[fresh.length - 1].seq;
          setEntries((old) => [...old, ...fresh].slice(-KEEP));
          setError("");
        })
        .catch((e: unknown) => !stopped && setError(e instanceof Error ? e.message : String(e)));
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [slug]);
  return { entries, chain, tallies, error };
}
