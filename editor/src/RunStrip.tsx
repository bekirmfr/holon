// The run strip, in the It may strip's place while runs are viewed (web-console-L4.md §5.5): the
// run shown, by its number and how it ended, what came in, the version it ran on and when, and
// chips for what it did outside the flow: ↗ the requests it sent, ✎ what it wrote.

import { useEffect, useState } from "react";
import { field, outside, runLabel, runOutcome, text } from "./runs";
import { getVersions, type LogEntry, type VersionEntry } from "./server";

/** What the strip says of a run. `version`: `v3`, else the unit's first characters when its
 *  version isn't listed (nothing when the entry names none). */
export function runStripFacts(run: LogEntry, versions: VersionEntry[], now = Date.now()) {
  const o = runOutcome(run);
  const ended = o === "failed" ? `${run.status ?? ""} failed`.trim() : o === "waiting" ? "waiting" : String(run.status ?? "done");
  const came = run.kind === "request" && run.url ? `${run.method ?? ""} ${run.url}`.trim() : runLabel(run);
  const v = run.unit ? versions.find((x) => x.unit === run.unit) : undefined;
  const version = v ? `v${v.number}` : run.unit ? `${run.unit.slice(0, 8)}…` : "";
  const when = new Date(run.at_ms);
  const today = new Date(now).toDateString() === when.toDateString();
  const at = today ? when.toLocaleTimeString() : when.toLocaleString();
  const { sent, did } = outside(run);
  const calls = (run.rounds ?? []).flatMap((r) => r.exchanges).map((x) => `${text(field(x.request, "method"))} ${text(field(x.request, "url"))}`.trim());
  const wrote = run.performed.map((p) => p.capability);
  return { outcome: o, ended, came, version, current: v?.current ?? false, at, sent, did, calls, wrote };
}

export function RunStrip({ slug, run, unit }: { slug: string; run: LogEntry | null; /** The unit deployed now: the versions are read again when it changes. */ unit?: string }) {
  const [versions, setVersions] = useState<VersionEntry[]>([]);
  useEffect(() => {
    let stale = false;
    getVersions(slug).then(
      (vs) => !stale && setVersions(vs),
      () => !stale && setVersions([]),
    );
    return () => {
      stale = true;
    };
  }, [slug, unit]);
  if (!run) {
    return (
      <div className="hl-may hl-runstrip" role="region" aria-label="what ran">
        <span className="hl-may__none">No run picked: pick one in the runs below (b).</span>
      </div>
    );
  }
  const f = runStripFacts(run, versions);
  return (
    <div className="hl-may hl-runstrip" role="region" aria-label="what ran">
      <a className="hl-runstrip__seq" href={`#${slug}.run-${run.seq}`} title="its page">
        Run #{run.seq}
      </a>
      <span className={`hl-runstrip__ended hl-runstrip__ended--${f.outcome}`}>{f.ended}</span>
      <span className="hl-runstrip__came" title="what came in">
        {f.came}
      </span>
      {f.version && (
        <span className="hl-runstrip__version" title={f.current ? "the version deployed now" : "an earlier version: shown as it was then"}>
          {f.version}
          {f.current ? "" : " (earlier)"}
        </span>
      )}
      <span className="hl-runstrip__at">{f.at}</span>
      {f.sent > 0 && (
        <span className="hl-may__chip hl-runstrip__out" title={f.calls.join("\n")}>
          ↗ called out{f.sent > 1 ? ` ×${f.sent}` : ""}
        </span>
      )}
      {f.did > 0 && (
        <span className="hl-may__chip hl-runstrip__wrote" title={f.wrote.join("\n")}>
          ✎ wrote{f.did > 1 ? ` ×${f.did}` : ""}
        </span>
      )}
      {f.sent === 0 && f.did === 0 && <span className="hl-may__none">nothing outside the flow</span>}
    </div>
  );
}
