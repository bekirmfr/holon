// View mode's history: the deployment's timeline (timeline.ts), its runs, versions and what was
// done to it, newest first; and going back to a point in it (docs/L4s/history-L4.md §4). A
// version or a run picked shows what can be restored to it: its code, its state, or both. Each
// restore is a dry run first, confirmed in a dialog that says what it changes and asks to approve
// what the code needs that today's grant doesn't cover, as a deploy does.

import { useCallback, useEffect, useMemo, useState, type CSSProperties } from "react";
import { getEvents, getVersions, restoreDeployment, type DeploymentEvent, type LogEntry, type RestoreDone, type RestorePoint, type RestorePreview, type VersionEntry } from "./server";
import { buildTimeline, filterOf, pointLabel, restoredLine, restoreMessage, runOptions, TIMELINE_FILTERS, versionOptions, type PointOptions, type TimelineFilter, type TimelineItem } from "./timeline";
import { useDialog } from "./Dialog";

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

/** The deployment's events and versions, polled while the history shows. */
function useHistory(slug: string) {
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
          setVersions(v);
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
  return { events, versions, error, refresh: () => setTick((t) => t + 1) };
}

/** A point picked in the timeline: a version, or a run. */
type Picked = { point: RestorePoint; options: PointOptions; version?: VersionEntry; label: string };

export function HistoryList({
  slug,
  runs,
  selected,
  pick,
  onShowVersion,
  onRestored,
}: {
  slug: string;
  /** The runs the log pane holds. */
  runs: LogEntry[];
  selected: LogEntry | null;
  pick: (run: LogEntry) => void;
  /** Show a version's canvas. */
  onShowVersion?: (v: VersionEntry) => void;
  /** After a restore: the deployment changed. */
  onRestored?: () => void;
}) {
  const dialog = useDialog();
  const { events, versions, error, refresh } = useHistory(slug);
  const [shown, setShown] = useState<Set<TimelineFilter>>(() => new Set(TIMELINE_FILTERS.map(([f]) => f)));
  const [picked, setPicked] = useState<Picked | null>(null);
  const [note, setNote] = useState<{ text: string; ok: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const current = versions.find((v) => v.current);
  const items = useMemo(() => buildTimeline(runs, events, versions).filter((i) => shown.has(filterOf(i))), [runs, events, versions, shown]);

  // A picked version follows the list as it changes (a restore makes the current one another).
  const pickedVersion = picked && "version" in picked.point ? (picked.point as { version: number }).version : null;
  useEffect(() => {
    if (pickedVersion === null) return;
    const v = versions.find((x) => x.number === pickedVersion);
    if (v) setPicked({ point: { version: v.number }, options: versionOptions(v), version: v, label: versionLabel(v) });
  }, [versions, pickedVersion]);

  const pickItem = (item: TimelineItem) => {
    setNote(null);
    if (item.kind === "run") {
      pick(item.run);
      setPicked({ point: { run: item.run.seq }, options: runOptions(item.run, current?.unit), label: `run ${item.run.seq}` });
      return;
    }
    const v = item.version;
    setPicked(v ? { point: { version: v.number }, options: versionOptions(v), version: v, label: versionLabel(v) } : null);
  };

  const restore = useCallback(
    async (point: RestorePoint, what: { code: boolean; state: boolean }) => {
      setNote(null);
      setBusy(true);
      try {
        const preview = (await restoreDeployment(slug, point, what, { dryRun: true })) as RestorePreview;
        if (preview.refused) {
          setNote({ text: `this host can't deploy that code now: ${preview.refused}`, ok: false });
          return;
        }
        const { lines, action } = restoreMessage(slug, point, what, preview);
        if (!(await dialog.confirm(lines.join("\n"), action))) return;
        const done = (await restoreDeployment(slug, point, what, { acknowledge: preview.needs_approval ?? [] })) as RestoreDone;
        setNote({ text: restoredLine(point, done), ok: true });
        refresh();
        onRestored?.();
      } catch (e) {
        setNote({ text: e instanceof Error ? e.message : String(e), ok: false });
      } finally {
        setBusy(false);
      }
    },
    [slug, dialog, refresh, onRestored],
  );

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
      {picked && (
        <RestoreBar
          picked={picked}
          busy={busy}
          onRestore={(what) => restore(picked.point, what)}
          onShowVersion={picked.version?.canvas && onShowVersion ? () => onShowVersion(picked.version!) : undefined}
        />
      )}
      {note && (
        <div className="hl-history__note" style={{ color: note.ok ? good : bad, padding: "3px 8px", borderBottom: "1px solid #1b2a45" }}>
          {note.text}
        </div>
      )}
      {error && <div style={{ color: bad, padding: "3px 8px" }}>{error}</div>}
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "0 0 8px" }}>
        {items.length === 0 && <div style={{ padding: 8, color: dim }}>nothing to show</div>}
        {items.map((item) => (
          <HistoryRow
            key={item.key}
            item={item}
            active={
              item.kind === "run"
                ? item.run.seq === selected?.seq
                : !!picked && "version" in picked.point && item.version?.number === picked.point.version
            }
            onClick={() => pickItem(item)}
          />
        ))}
      </div>
    </div>
  );
}

const versionLabel = (v: VersionEntry) => `version ${v.number}${v.current ? " (now)" : ""}`;

function HistoryRow({ item, active, onClick }: { item: TimelineItem; active: boolean; onClick: () => void }) {
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
  if (item.kind === "run") {
    const r = item.run;
    return (
      <div className="hl-history__row" data-kind="run" style={row} onClick={onClick}>
        {when}
        {badge(`run #${r.seq}`, dim)}
        <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "#9fb4d4" }}>
          {r.route ?? r.kind} · s{r.session}
        </span>
        <span style={{ color: r.error || (r.status ?? 0) >= 400 ? bad : good, flexShrink: 0 }}>{r.error ? "err" : (r.status ?? "")}</span>
      </div>
    );
  }
  if (item.kind === "version") {
    const v = item.version;
    return (
      <div className="hl-history__row" data-kind="deploy" style={row} onClick={onClick}>
        {when}
        {badge("deploy", KIND_COLOR.deploy)}
        <span style={{ flex: 1, minWidth: 0, color: ink }}>
          {v.restored_from ? `restored version ${v.restored_from}'s code, as version ${v.number}` : `deployed version ${v.number}`}
          {v.current ? <span style={{ color: good }}> (now)</span> : null}
        </span>
        {by(v.by)}
      </div>
    );
  }
  const e = item.event;
  return (
    <div className="hl-history__row" data-kind={e.kind} style={row} onClick={onClick}>
      {when}
      {badge(e.kind, KIND_COLOR[e.kind] ?? dim)}
      <span style={{ flex: 1, minWidth: 0, color: ink, overflowWrap: "anywhere" }}>
        {e.what}
        {item.version?.current ? <span style={{ color: good }}> (now)</span> : null}
      </span>
      {by(e.by)}
    </div>
  );
}

/** What can be restored to the point picked, and the buttons that do it. */
function RestoreBar({
  picked,
  busy,
  onRestore,
  onShowVersion,
}: {
  picked: Picked;
  busy: boolean;
  onRestore: (what: { code: boolean; state: boolean }) => void;
  onShowVersion?: () => void;
}) {
  const { code, state } = picked.options;
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
  const what = `${pointLabel(picked.point)}'s`;
  return (
    <div className="hl-history__restore" style={{ display: "flex", flexWrap: "wrap", gap: 6, alignItems: "center", padding: "3px 8px", borderBottom: "1px solid #1b2a45" }}>
      <span style={{ color: ink }}>{picked.label}</span>
      {onShowVersion && (
        <button style={small} onClick={onShowVersion} title="show its canvas">
          canvas
        </button>
      )}
      <span style={{ flex: 1 }} />
      <span style={{ color: dim }}>restore</span>
      {button("code", { code: true, state: false }, code, `deploy ${what} code again, as a new version`)}
      {button("state", { code: false, state: true }, state, `put back the state ${pointLabel(picked.point)} left, in a new session`)}
      {button("both", { code: true, state: true }, code ?? state, `${what} code, then the state it left`)}
    </div>
  );
}
