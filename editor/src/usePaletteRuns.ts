// The runs the command palette offers (ui/CommandPalette.tsx), read as it opens: the newest across
// flows, from the host's summary (GET /summary's `latest`), and each run waiting for an approval
// this person may decide, with its number in its flow's log (a waiting run is known by its id).

import { useEffect, useMemo, useState } from "react";
import { getLog, getSummary, type Summary } from "./server";

/** How many of the newest runs the palette offers. */
export const NEWEST = 10;

/** A run waiting at an approval point someone may decide, as the palette offers it. */
export type DecidableRun = { slug: string; runId: string; seq: number | null };

/** The newest runs, and the runs in `waiting` by their numbers, while `open` (read again each
 *  time it opens; the last read is kept while it is closed). */
export function usePaletteRuns(open: boolean, waiting: { slug: string; runId: string }[]): { latest: Summary["latest"]; deciding: DecidableRun[] } {
  const [latest, setLatest] = useState<Summary["latest"]>([]);
  const [seqs, setSeqs] = useState<Map<string, number>>(new Map());
  const slugs = [...new Set(waiting.map((w) => w.slug))].sort().join(",");
  useEffect(() => {
    if (!open) return;
    let on = true;
    getSummary(1).then(
      (s) => on && setLatest(s.latest.slice(0, NEWEST)),
      () => undefined,
    );
    // Each waiting run's number: the first line its id is on in its flow's log.
    Promise.all(
      slugs
        .split(",")
        .filter(Boolean)
        .map((slug) =>
          getLog(slug).then(
            ({ entries }) => entries.filter((e) => e.run_id).map((e) => [`${slug}\n${e.run_id}`, e.seq] as const),
            () => [],
          ),
        ),
    ).then((all) => {
      if (!on) return;
      const m = new Map<string, number>();
      for (const [key, seq] of all.flat()) if (!m.has(key)) m.set(key, seq);
      setSeqs(m);
    });
    return () => {
      on = false;
    };
  }, [open, slugs]);
  const deciding = useMemo(() => waiting.map((w) => ({ ...w, seq: seqs.get(`${w.slug}\n${w.runId}`) ?? null })), [waiting, seqs]);
  return useMemo(() => ({ latest, deciding }), [latest, deciding]);
}
