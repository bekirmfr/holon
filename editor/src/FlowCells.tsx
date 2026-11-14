// What the Overview's and the Flows page's tables say of each flow (OverviewPage.tsx,
// FlowsPage.tsx), in the same columns (web-console-L4.md §7.1): its version, its state with a
// version that waits for a person, what starts its runs, its last 24 hours, how long its runs
// take (p50), what it may do and when it last ran. The hours, p50 and last run come from the
// host's summary (GET /summary), read again every 10 s while a page shows it.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import type { Approval } from "./approvals";
import { describeItem } from "./holon";
import { lastRun, tookSaid } from "./overview";
import { getFlow, getManifest, getSummary, type DeploymentEntry, type FlowDocument, type Summary } from "./server";
import { Spark } from "./ui/Spark";

export const STATE_SAID: Record<DeploymentEntry["status"], string> = { running: "live", paused: "paused", stopped: "stopped" };
export const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

/** The host's summary of the last 24 hours, read now, again every 10 s and when `again` changes;
 *  why not, when it couldn't be read. */
export function useSummary(again: unknown) {
  const [summary, setSummary] = useState<Summary | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const read = useCallback(() => {
    getSummary(24).then(
      (s) => {
        setSummary(s);
        setFailed(null);
      },
      (e: unknown) => setFailed(e instanceof Error ? e.message : String(e)),
    );
  }, []);
  useEffect(() => {
    read();
    const timer = setInterval(read, 10000);
    return () => clearInterval(timer);
  }, [read, again]);
  return { summary, failed, read };
}

/** What starts a deployment's runs: its port, and its triggers but the one on that port. */
export const startsOn = (d: DeploymentEntry) =>
  [d.port !== null ? `:${d.port}` : null, ...d.triggers.filter((t) => d.port === null || !t.endsWith(`:${d.port}`))].filter(Boolean).join(", ") || "–";

/** The version of `slug` that waits for a person to deploy it, if one does and its number is
 *  known. */
export function versionWaiting(approvals: Approval[], slug: string): number | null {
  const a = approvals.find((x) => x.kind === "update" && x.slug === slug);
  return a?.kind === "update" ? a.version : null;
}

/** A deployment's state, and the version that waits ("v4 waits"), amber. */
export function StateChips({ deployed, waits }: { deployed: DeploymentEntry; waits: number | null }) {
  return (
    <>
      <span className={`hl-chip hl-chip--${deployed.status}`}>{STATE_SAID[deployed.status]}</span>
      {waits !== null && (
        <span className="hl-chip hl-chip--waits" title={`version ${waits} waits for a person to review and deploy it`}>
          v{waits} waits
        </span>
      )}
    </>
  );
}

/** A flow's last 24 hours as bars, its p50, then `may` (its It may cell), then its last run: "…"
 *  until the summary is read, "–" where it has none. `summary` undefined: a flow not deployed. */
export function SummaryCells({ slug, summary, may }: { slug: string; summary: Summary | null | undefined; may: ReactNode }) {
  const s = summary?.deployments.find((d) => d.slug === slug);
  const last = summary && s ? lastRun(slug, summary) : null;
  const none = <span className="hl-table__dim">{summary === null ? "…" : "–"}</span>;
  return (
    <>
      <td>{s ? <Spark runs={s.runs} failed={s.failed} /> : none}</td>
      <td className="hl-table__num" title="how long its runs in the last 24 h took: the middle of them">
        {s && s.p50_ms !== null ? tookSaid(s.p50_ms) : none}
      </td>
      {may}
      <td className="hl-table__dim">{!s ? none : last === null ? "–" : time(last)}</td>
    </>
  );
}

/** How many things a flow may do, with each in words as its title: its deployment's manifest when
 *  deployed, else what deploying its saved copy would ask for. */
export function MayCount({ name, deployed, savedAt, manifestOf }: { name: string; deployed?: DeploymentEntry; savedAt?: number; manifestOf?: (doc: FlowDocument) => { items: string[] } }) {
  const may = useMay(name, deployed, savedAt, manifestOf);
  return (
    <td className="hl-table__num" title={may && "items" in may ? may.items.map(describeItem).join("\n") : may?.error}>
      {may === null ? "…" : "error" in may ? <span className="hl-table__dim">?</span> : may.items.length}
    </td>
  );
}

/** What a flow may do, read again when its deployment or its save changes. */
function useMay(name: string, deployed: DeploymentEntry | undefined, savedAt: number | undefined, manifestOf?: (doc: FlowDocument) => { items: string[] }): { items: string[] } | { error: string } | null {
  const key = deployed ? `d:${deployed.unit}` : `s:${savedAt}`;
  const [may, setMay] = useState<{ key: string; value: { items: string[] } | { error: string } } | null>(null);
  useEffect(() => {
    let fresh = true;
    const read = deployed || !manifestOf ? getManifest(name) : getFlow(name).then((rec) => manifestOf(rec.document));
    read.then(
      (m) => fresh && setMay({ key, value: { items: m.items } }),
      (e: unknown) => fresh && setMay({ key, value: { error: e instanceof Error ? e.message : String(e) } }),
    );
    return () => {
      fresh = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return may?.key === key ? may.value : null;
}
