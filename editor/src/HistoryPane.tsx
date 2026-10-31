// View mode's history: the deployment's timeline (timeline.ts), what was done to it and the
// versions it was deployed with, newest first (its runs too, when asked: they are the run list's);
// and going back to any point in it (docs/L4s/history-L4.md §4). Each row is picked (its details
// show beside it) and goes back: a run or a version to itself, anything else to how it was right
// after. A row picked shows what can be restored to it: its code, its state, or both; ↺ on a row
// restores what it can. Each restore is a dry run first, its result shown in the dry-run window
// (DryRun.tsx): what it changes, and what the code needs that today's grant doesn't cover, to
// approve as a deploy's is; nothing changes until it is approved.

import { useCallback, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { getEvents, getVersions, restoreDeployment, type DeploymentEvent, type RestoreDone, type RestorePoint, type RestorePreview, type VersionEntry } from "./server";
import {
  filterOf,
  itemText,
  pointLabel,
  restoreDiffs,
  restoredLine,
  restoreReport,
  restoreParts,
  TIMELINE_FILTERS,
  versionWhat,
  type CodeText,
  type RestorePart,
  type Target,
  type TimelineFilter,
  type TimelineItem,
} from "./timeline";
import { useDryRun } from "./DryRun";

const POLL_MS = 3000;
const dim = "#7e8ea8";
const ink = "#e6eefb";
const good = "#9ad29f";
const bad = "#e0916f";
const small: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#9fb4d4",
  background: "transparent",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 8px",
  cursor: "pointer",
};
const time = (ms: number) => new Date(ms).toLocaleTimeString();
const KIND_COLOR: Record<string, string> = { deploy: "#6fd3e0", restore: "#e0c36f", grant: "#b59fe0", secret: "#e0c36f", binding: "#9fb4d4", lifecycle: dim };

/** The deployment's events and versions, polled. */
export function useHistory(slug: string) {
  const [events, setEvents] = useState<DeploymentEvent[]>([]);
  const [versions, setVersions] = useState<VersionEntry[]>([]);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let stopped = false;
    const poll = () =>
      Promise.all([getEvents(slug), getVersions(slug)])
        .then(([e, v]) => {
          if (stopped) return;
          setEvents((old) => (old.length === e.events.length ? old : e.events));
          setVersions((old) => (JSON.stringify(old) === JSON.stringify(v) ? old : v));
          setError("");
        })
        .catch((e: unknown) => !stopped && setError(e instanceof Error ? e.message : String(e)));
    poll();
    const timer = setInterval(poll, POLL_MS);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [slug, tick]);
  const refresh = useCallback(() => setTick((t) => t + 1), []);
  return { events, versions, error, refresh };
}

/** Going back to a target: each part's dry run, their report in the dry-run window, and once
 *  approved each part, the code first. `modal` is the window, to be rendered. */
export function useRestore(slug: string, onDone: () => void, versions: VersionEntry[] = [], codeText?: CodeText) {
  const dryRun = useDryRun();
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const restore = useCallback(
    async (target: Target, what: { code: boolean; state: boolean }) => {
      const parts = restoreParts(target, what);
      if (parts.length === 0) {
        const why = [target.why.code && `its code: ${target.why.code}`, target.why.state && `its state: ${target.why.state}`].filter(Boolean);
        setNote({ text: `nothing to restore to ${target.label}: ${why.join("; ")}`, ok: false });
        return;
      }
      setNote(null);
      setBusy(true);
      const done: { point: RestorePoint; done: RestoreDone }[] = [];
      try {
        const previewed: (RestorePart & { preview: RestorePreview })[] = [];
        for (const part of parts) {
          const preview = (await restoreDeployment(slug, part.point, part.what, { dryRun: true })) as RestorePreview;
          if (preview.refused) {
            setNote({ text: `this host can't deploy that code now: ${preview.refused}`, ok: false });
            return;
          }
          previewed.push({ ...part, preview });
        }
        const report = restoreReport(slug, target, previewed, undefined, versions);
        if (codeText) report.diffs = () => restoreDiffs(previewed, versions, codeText);
        if (!(await dryRun.ask(report))) return;
        for (const p of previewed) {
          done.push({ point: p.point, done: (await restoreDeployment(slug, p.point, p.what, { acknowledge: p.preview.needs_approval ?? [] })) as RestoreDone });
        }
        setNote({ text: restoredLine(done), ok: true });
      } catch (e) {
        const before = done.length > 0 ? `${restoredLine(done)}; then ` : "";
        setNote({ text: before + (e instanceof Error ? e.message : String(e)), ok: false });
      } finally {
        setBusy(false);
        if (done.length > 0) onDone();
      }
    },
    [slug, dryRun.ask, onDone, versions, codeText],
  );
  return { restore, busy, note, setNote, modal: dryRun.modal };
}

/** What ↺ restores of a target: whatever of its code and state it can. */
export const everything = (t: Target) => ({ code: t.why.code === null, state: t.why.state === null });

/** ↺'s words: what it goes back to, or why it can't. */
export function backTitle(t: Target): string {
  const parts = restoreParts(t, everything(t));
  if (parts.length === 0) return `can't go back to ${t.label}: ${[t.why.code, t.why.state].filter(Boolean).join("; ")}`;
  const said = parts.flatMap((p) => [...(p.what.code ? [`${pointLabel(p.point)}'s code`] : []), ...(p.what.state ? [`the state ${pointLabel(p.point)} left`] : [])]);
  return `go back to ${t.label}: ${said.join(", and ")}`;
}

/** The ↺ on a row: go back to it. */
export function BackButton({ target, busy, onBack }: { target: Target; busy: boolean; onBack: () => void }) {
  const can = restoreParts(target, everything(target)).length > 0;
  return (
    <button
      className="hl-history__back"
      style={{ ...small, padding: "0px 4px", fontSize: 10, opacity: can && !busy ? 1 : 0.4, cursor: can && !busy ? "pointer" : "default" }}
      disabled={!can || busy}
      title={backTitle(target)}
      aria-label={`restore to ${target.label}`}
      onClick={(ev) => {
        ev.stopPropagation();
        onBack();
      }}
    >
      ↺
    </button>
  );
}

export function HistoryList({
  timeline,
  query = "",
  pickedKey,
  onPick,
  targetFor,
  busy,
  onRestore,
  onShowVersion,
  error,
}: {
  /** All of it, newest first, its runs too. */
  timeline: TimelineItem[];
  /** The toolbar's filter: rows whose words hold it. */
  query?: string;
  pickedKey: string | null;
  onPick: (item: TimelineItem) => void;
  targetFor: (item: TimelineItem) => Target;
  busy: boolean;
  onRestore: (target: Target, what: { code: boolean; state: boolean }) => void;
  /** Show a version's canvas. */
  onShowVersion?: (v: VersionEntry) => void;
  error: string;
}) {
  const [shown, setShown] = useState<Set<TimelineFilter>>(() => new Set(TIMELINE_FILTERS.map(([f]) => f).filter((f) => f !== "runs")));
  const items = useMemo(() => {
    const q = query.trim().toLowerCase();
    return timeline.filter((i) => shown.has(filterOf(i)) && (!q || itemText(i).toLowerCase().includes(q)));
  }, [timeline, shown, query]);
  const picked = pickedKey === null ? undefined : timeline.find((i) => i.key === pickedKey);
  const pickedTarget = picked && targetFor(picked);
  const pickedVersion = picked && picked.kind !== "run" ? picked.version : undefined;

  const toggle = (f: TimelineFilter) =>
    setShown((old) => {
      const next = new Set(old);
      if (next.has(f)) next.delete(f);
      else next.add(f);
      return next;
    });

  return (
    <div className="hl-history" style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, minWidth: 0 }}>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 4, alignItems: "center", padding: "2px 6px", borderBottom: "1px solid #1b2a45", fontSize: 10, flexShrink: 0 }}>
        {TIMELINE_FILTERS.map(([f, name, title]) => (
          <button
            key={f}
            onClick={() => toggle(f)}
            title={`show ${title}`}
            aria-pressed={shown.has(f)}
            style={{ ...small, fontSize: 10, padding: "0 6px", color: shown.has(f) ? ink : dim, borderColor: shown.has(f) ? "#2c4a6e" : "#1b2a45" }}
          >
            {name}
          </button>
        ))}
      </div>
      {pickedTarget && (
        <RestoreBar
          target={pickedTarget}
          busy={busy}
          onRestore={(what) => onRestore(pickedTarget, what)}
          onShowVersion={pickedVersion?.canvas && onShowVersion ? () => onShowVersion(pickedVersion) : undefined}
        />
      )}
      {error && <div style={{ color: bad, padding: "3px 8px" }}>{error}</div>}
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "0 0 8px" }}>
        {items.length === 0 && <div style={{ padding: 8, color: dim }}>{query.trim() ? "nothing matches the filter" : "nothing to show"}</div>}
        {items.map((item) => {
          const target = targetFor(item);
          return (
            <HistoryRow
              key={item.key}
              item={item}
              active={item.key === pickedKey}
              onClick={() => onPick(item)}
              back={<BackButton target={target} busy={busy} onBack={() => onRestore(target, everything(target))} />}
            />
          );
        })}
      </div>
    </div>
  );
}

function HistoryRow({ item, active, onClick, back }: { item: TimelineItem; active: boolean; onClick: () => void; back: ReactNode }) {
  const row: CSSProperties = {
    display: "flex",
    gap: 8,
    padding: "2px 6px",
    cursor: "pointer",
    borderRadius: 3,
    alignItems: "baseline",
    background: active ? "#13243f" : "transparent",
  };
  const badge = (text: string, color: string) => <span style={{ color, width: 58, flexShrink: 0, fontSize: 10 }}>{text}</span>;
  const when = <span style={{ color: dim, width: 76, flexShrink: 0, whiteSpace: "nowrap" }}>{time(item.at_ms)}</span>;
  const by = (who: string | null | undefined) => <span style={{ color: dim, flexShrink: 0 }}>{who ? `by ${who}` : "by the host"}</span>;
  const end = <span style={{ flexShrink: 0 }}>{back}</span>;
  if (item.kind === "run") {
    const r = item.run;
    return (
      <div className="hl-history__row" data-kind="run" aria-selected={active} style={row} onClick={onClick}>
        {when}
        {badge(`run #${r.seq}`, dim)}
        <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "#9fb4d4" }}>
          {r.route ?? r.kind} · s{r.session}
        </span>
        <span style={{ color: r.error || (r.status ?? 0) >= 400 ? bad : good, flexShrink: 0 }}>{r.error ? "err" : (r.status ?? "")}</span>
        {end}
      </div>
    );
  }
  if (item.kind === "version") {
    const v = item.version;
    return (
      <div className="hl-history__row" data-kind="deploy" aria-selected={active} style={row} onClick={onClick}>
        {when}
        {badge("deploy", KIND_COLOR.deploy)}
        <span style={{ flex: 1, minWidth: 0, color: ink }}>
          {versionWhat(v)}
          {v.current ? <span style={{ color: good }}> (now)</span> : null}
        </span>
        {by(v.by)}
        {end}
      </div>
    );
  }
  const e = item.event;
  return (
    <div className="hl-history__row" data-kind={e.kind} aria-selected={active} style={row} onClick={onClick}>
      {when}
      {badge(e.kind, KIND_COLOR[e.kind] ?? dim)}
      <span style={{ flex: 1, minWidth: 0, color: ink, overflowWrap: "anywhere" }}>
        {e.what}
        {item.version?.current ? <span style={{ color: good }}> (now)</span> : null}
      </span>
      {by(e.by)}
      {end}
    </div>
  );
}

/** What can be restored to the row picked, and the buttons that do it. */
function RestoreBar({
  target,
  busy,
  onRestore,
  onShowVersion,
}: {
  target: Target;
  busy: boolean;
  onRestore: (what: { code: boolean; state: boolean }) => void;
  onShowVersion?: () => void;
}) {
  const { code, state } = target.why;
  const button = (label: string, what: { code: boolean; state: boolean }, why: string | null, title: string) => (
    <button
      style={{ ...small, opacity: why || busy ? 0.45 : 1, cursor: why || busy ? "default" : "pointer" }}
      disabled={!!why || busy}
      title={why ?? title}
      onClick={() => onRestore(what)}
    >
      {label}
    </button>
  );
  const codeFrom = target.code ? `${pointLabel(target.code)}'s code` : "its code";
  const stateFrom = target.state ? `the state ${pointLabel(target.state)} left` : "its state";
  return (
    <div className="hl-history__restore" style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center", padding: "3px 8px", borderBottom: "1px solid #1b2a45" }}>
      <span style={{ color: ink }}>{target.label}</span>
      {onShowVersion && (
        <button style={small} onClick={onShowVersion} title="show its canvas">
          canvas
        </button>
      )}
      <span style={{ flex: 1 }} />
      <span style={{ color: dim }}>restore</span>
      {button("code", { code: true, state: false }, code, `deploy ${codeFrom} again, as a new version`)}
      {button("state", { code: false, state: true }, state, `put back ${stateFrom}, in a new session`)}
      {button("both", { code: true, state: true }, code ?? state, `${codeFrom}, then ${stateFrom}`)}
    </div>
  );
}

/** What a history row is, for the details pane: what happened, and where going back to it goes. */
export function itemDetails(item: TimelineItem, target: Target): [string, string][] {
  const fields: [string, string][] = [];
  const when = (ms: number) => new Date(ms).toLocaleString();
  if (item.kind === "event") {
    const e = item.event;
    fields.push(["what", e.what], ["kind", e.kind], ["when", when(e.at_ms)], ["by", e.by ?? "the host"], ["before run", `#${e.run_seq}`]);
    if (e.session !== undefined) fields.push(["session", String(e.session)]);
    if (e.name) fields.push(["about", e.name]);
    if (e.kind === "ask" && e.from) fields.push(["asks", `${e.from}'s ${(e.restores ?? []).join(" and ")}, for a person to restore`]);
    else if (e.from) fields.push(["put back", `the state ${e.from} left`]);
    fields.push(["event", `#${e.seq}${e.prev ? ` · after ${e.prev.slice(0, 12)}…` : ""}`]);
  }
  const v = item.kind === "run" ? undefined : item.version;
  if (v) {
    if (item.kind === "version") fields.push(["what", versionWhat(v)], ["when", when(v.deployed_at * 1000)], ["by", v.by ?? "the host"]);
    fields.push(["version", `${v.number}${v.current ? " (deployed now)" : ""}`], ["unit", v.unit]);
    if (v.restored_from) fields.push(["restores", `version ${v.restored_from}'s code`]);
    if (v.first_seq !== undefined) fields.push(["first run", `#${v.first_seq}`]);
    fields.push(["canvas", v.canvas ? "kept" : "not kept"]);
  }
  fields.push(
    ["back to its code", target.code && target.why.code === null ? `${pointLabel(target.code)}'s` : `no: ${target.why.code}`],
    ["back to its state", target.state && target.why.state === null ? `the state ${pointLabel(target.state)} left` : `no: ${target.why.state}`],
  );
  return fields;
}
