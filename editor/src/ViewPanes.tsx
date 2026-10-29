// View mode's panes around the canvas of a deployed flow. The bottom pane shows the flow log
// live (every request, trigger and tick). Left side: a sortable, filterable run list with copy
// and export buttons and a capability-badge column. Right side: two tabs — Details (what the run
// read and produced) and Timeline (one row per node, timed by the server). The width of the
// split is draggable and remembered. The side pane shows the selected run's flow state as of the
// current step, and the selected node: its params, and the inputs and outputs it had in that run
// (replayed, replay.ts).

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { downloadRuns, getCollectionItems, getLog, type LogEntry } from "./server";
import { formatValue, detectRenderer, availableRenderers, type RendererKind } from "./diagnostics";
import { typeLabel } from "./theme";
import type { WParam, WPort, WValue } from "./holon";
import { isCollectionCell, itemChanged, type CollectionAtStep, type Diagnosis, type Issue, type NodeExchange, type NodeTrace } from "./replay";
import { FAIL } from "./PrimitiveNode";
import { useNodeDocs } from "./NodeDocs";

const KEEP = 500;
const POLL_MS = 1500;

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
const dim = "#7e8ea8";
const ink = "#e6eefb";
const good = "#9ad29f";
const bad = "#e0916f";
const changedBg = "#2a2a14";

const time = (ms: number) => new Date(ms).toLocaleTimeString();
const runLabel = (e: LogEntry) =>
  e.kind === "request"
    ? (e.route ?? `${e.method ?? ""} ${e.url ?? ""} (no route)`)
    : e.kind === "resume"
      ? `resumed ${e.route ?? ""}`.trim()
      : e.kind === "tick"
        ? (e.route ?? "clock tick")
        : e.kind === "file"
          ? `${fileRun(e.sourced.find(([cap]) => cap.startsWith("FileArrival("))?.[1])}${e.route ? ` (${e.route})` : ""}`
          : (e.route ?? "trigger");
/** A file run's label: what happened to which file (`deleted file a.csv`). */
const fileRun = (handle: WValue | undefined) => `${text(field(handle, "event")) || "new"} file ${text(field(handle, "name")) || ""}`.trim();
const show = (v: WValue | null | undefined) => (v === null || v === undefined ? "—" : formatValue(v));
const us = (t: number) => (t >= 1000 ? `${(t / 1000).toFixed(2)}ms` : `${t.toFixed(0)}µs`);

/** A record's field, or null. */
const field = (v: WValue | null | undefined, k: string): WValue | null =>
  v?.type === "Record" ? (v.value.find(([key]) => key === k)?.[1] ?? null) : null;
const text = (v: WValue | null) => (v?.type === "Text" ? v.value : v?.type === "Number" ? String(v.value) : "");

/** An outbound request as one line: `GET https://… → 200`, or `→ blocked_host`. */
function exchangeLine(request: WValue, response: WValue): { line: string; ok: boolean } {
  const error = text(field(response, "error"));
  const status = text(field(response, "status"));
  const answer = error !== "" ? error : status;
  const ok = error === "" && Number(status) > 0 && Number(status) < 400;
  return { line: `${text(field(request, "method"))} ${text(field(request, "url"))} → ${answer}`, ok };
}

/** What a view-mode pane needs to know about a canvas node. */
export type ViewNode = {
  id: string;
  data: { name: string; body: { kind: string }; params: WParam[]; inputs: WPort[]; outputs: WPort[] };
};

/** A node's name as the panes show it: with its key, method/path or other frozen params. */
export function nodeLabel(n: ViewNode): string {
  const frozen = n.data.params.flatMap((p) => (p.kind === "Frozen" ? [p.value] : []));
  if (n.data.name === "request" && frozen.length > 0) return `request ${frozen.join(" ")}`;
  return frozen.length > 0 && frozen.join(" ").length <= 24 ? `${n.data.name} ${frozen.join(" ")}` : n.data.name;
}

/** The deployment's flow log, polled: everything at first, then only what came after the newest. */
function useFlowLog(slug: string) {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [chain, setChain] = useState<{ head: string; breaks: number[] }>({ head: "", breaks: [] });
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);
  useEffect(() => {
    last.current = undefined;
    setEntries([]);
    setChain({ head: "", breaks: [] });
    let stopped = false;
    const poll = () =>
      getLog(slug, last.current)
        .then(({ entries: fresh, head, breaks }) => {
          if (stopped) return;
          setChain((old) => (old.head === head && old.breaks.length === (breaks ?? []).length ? old : { head, breaks: breaks ?? [] }));
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
  return { entries, chain, error };
}

function Tabs<T extends string>({
  tabs,
  tab,
  onTab,
  children,
}: {
  tabs: [T, string][];
  tab: T;
  onTab: (t: T) => void;
  children?: ReactNode;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 2, borderBottom: "1px solid #1b2a45", padding: "0 8px" }}>
      {tabs.map(([t, name]) => (
        <button
          key={t}
          onClick={() => onTab(t)}
          style={{
            font: "inherit",
            fontSize: 11,
            letterSpacing: "0.04em",
            padding: "6px 10px",
            background: "transparent",
            border: "none",
            borderBottom: `2px solid ${t === tab ? "#6fd3e0" : "transparent"}`,
            color: t === tab ? ink : dim,
            cursor: "pointer",
          }}
        >
          {name}
        </button>
      ))}
      <div style={{ flex: 1 }} />
      {children}
    </div>
  );
}

const paneStyle: CSSProperties = {
  fontSize: 11,
  color: "#9fb4d4",
  background: "#0b1527",
  display: "flex",
  flexDirection: "column",
  minWidth: 0,
  minHeight: 0,
  position: "relative",
};

const HEIGHT_KEY = "holon.viewHeight";

/** The bottom pane's height, remembered per browser (styles.css `--view-height`; a third of the
 *  canvas when never dragged). */
function rememberedHeight(): number | null {
  try {
    const h = Number(localStorage.getItem(HEIGHT_KEY));
    return Number.isFinite(h) && h > 0 ? h : null;
  } catch {
    return null;
  }
}

/** Where the pane's height may go: never so small the tabs vanish, never most of the window. */
const clampHeight = (h: number) => Math.max(120, Math.min(window.innerHeight * 0.75, h));
const scroll: CSSProperties = { flex: 1, minHeight: 0, overflow: "auto", padding: 8 };

const SPLIT_KEY = "holon.logRightWidth";
function rememberedSplit(): number | null {
  try {
    const w = Number(localStorage.getItem(SPLIT_KEY));
    return Number.isFinite(w) && w > 0 ? w : null;
  } catch {
    return null;
  }
}
const clampSplit = (w: number, total: number) => Math.max(180, Math.min(total - 200, w));

/** Compact capability badges derived from a run's log entry. */
function runActions(e: LogEntry): { label: string; title: string; color: string }[] {
  const acts: { label: string; title: string; color: string }[] = [];
  if (e.sourced.some(([c]) => c === "Random")) acts.push({ label: "rng", title: "random value read", color: dim });
  if ((e.rounds ?? []).some((r) => r.exchanges.length > 0)) acts.push({ label: "http", title: "outbound http request", color: good });
  if (Object.keys(e.secrets ?? {}).length > 0) acts.push({ label: "sec", title: "secret read", color: "#e0c36f" });
  if ((e.reads ?? []).some((r) => r.file != null)) acts.push({ label: "frd", title: "file read", color: "#9fb4d4" });
  if (e.performed.some((p) => /^file/i.test(p.capability))) acts.push({ label: "fwr", title: "file write", color: "#e0c36f" });
  return acts;
}

/** The step controls: back, forward, or the whole run. */
function Stepper({ step, steps, onStep }: { step: number | null; steps: number; onStep: (s: number | null) => void }) {
  if (steps === 0) return null;
  return (
    <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
      <button style={small} onClick={() => onStep(step === null ? 0 : Math.max(0, step - 1))} title="step back one node">
        ◀
      </button>
      <span style={{ minWidth: 86, textAlign: "center" }}>
        {step === null ? `all ${steps} nodes` : `node ${step + 1} of ${steps}`}
      </span>
      <button
        style={small}
        onClick={() => onStep(step === null ? 0 : step + 1 >= steps ? null : step + 1)}
        title="step forward one node"
      >
        ▶
      </button>
      <button style={{ ...small, visibility: step === null ? "hidden" : "visible" }} onClick={() => onStep(null)}>
        all
      </button>
    </div>
  );
}

export function BottomPane({
  slug,
  selected,
  onSelect,
  step,
  onStep,
  order,
  nodes,
  timings,
  exchanges,
  verdict,
  versionNote,
  listenError,
  onPickNode,
  diagnosis,
  onShowNode,
  onFitToNodes,
}: {
  slug: string;
  selected: LogEntry | null;
  onSelect: (e: LogEntry | null) => void;
  /** The step shown (index into `order`), or null for the whole run. */
  step: number | null;
  onStep: (step: number | null) => void;
  /** The nodes the selected run went through, in evaluation order. */
  order: string[];
  nodes: ViewNode[];
  /** When each node ran, by node id, from the server (empty when it didn't record it). */
  timings: Map<string, { start_us: number; end_us: number }>;
  /** The run's outbound requests, by the node that made each. */
  exchanges: NodeExchange[];
  /** Whether replaying the selected run reproduced what was recorded; null when not checkable. */
  verdict: boolean | null;
  /** Which version the run is shown on, when not the deployment's current one (or why it can't be). */
  versionNote?: string;
  /** Why the deployment isn't answering on its port, when it isn't: requests sent there never reach it. */
  listenError?: string;
  onPickNode: (id: string) => void;
  /** Why the selected run failed, when it did (replay.ts's diagnose). */
  diagnosis: Diagnosis | null;
  /** Bring a node into view and inspect it. */
  onShowNode: (id: string) => void;
  /** Fit the canvas to a set of nodes (double-click run). */
  onFitToNodes: (ids: string[]) => void;
}) {
  const { entries, chain, error } = useFlowLog(slug);
  // The pane's height, dragged by its top edge: set on the canvas grid it is a row of
  // (`--view-height`, styles.css), so the canvas above takes the rest.
  const pane = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(rememberedHeight);
  useEffect(() => {
    const grid = pane.current?.parentElement;
    if (!grid) return;
    if (height === null) grid.style.removeProperty("--view-height");
    else grid.style.setProperty("--view-height", `${Math.round(height)}px`);
    return () => {
      grid.style.removeProperty("--view-height");
    };
  }, [height]);
  const onEdge = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = pane.current?.getBoundingClientRect().height ?? height ?? 240;
    let last = startH;
    const move = (m: PointerEvent) => {
      last = clampHeight(startH + startY - m.clientY);
      setHeight(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try {
        localStorage.setItem(HEIGHT_KEY, String(Math.round(last)));
      } catch {
        // Not remembered, then.
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  // Live: always show the newest run. A tab coming back with a run selected keeps it (and its
  // step) instead: live starts only when nothing is selected.
  const [live, setLive] = useState(() => selected === null);

  const newest = entries[entries.length - 1];
  useEffect(() => {
    if (live && newest && newest.seq !== selected?.seq) onSelect(newest);
  }, [live, newest, selected, onSelect]);

  const pick = (e: LogEntry) => {
    setLive(false);
    onSelect(e);
  };

  return (
    <div ref={pane} className="nodrag nowheel hl-view__bottom" style={paneStyle} role="region" aria-label="run">
      <div className="hl-view__edge" onPointerDown={onEdge} title="drag to resize" />
      <div style={{ display: "flex", alignItems: "center", gap: 10, borderBottom: "1px solid #1b2a45", padding: "4px 8px" }}>
        {selected && (
          <span style={{ color: ink }}>
            #{selected.seq} · {runLabel(selected)} · session {selected.session}
          </span>
        )}
        {verdict !== null && (
          <span style={{ color: verdict ? good : bad }}>
            {verdict ? "✓ replay reproduces the run" : "✗ replay differs from the run"}
          </span>
        )}
        {versionNote && <span style={{ color: dim }}>{versionNote}</span>}
        <div style={{ flex: 1 }} />
        <Stepper step={step} steps={order.length} onStep={onStep} />
        <button
          style={{ ...small, color: live ? "#0e1a2f" : "#9fb4d4", background: live ? good : "transparent" }}
          onClick={() => setLive(!live)}
          title="follow the newest run as it happens"
        >
          ● live
        </button>
      </div>
      {error && <div style={{ color: bad, padding: "4px 8px" }}>{error}</div>}
      {listenError && (
        <div className="hl-view__not-listening" style={{ color: bad, padding: "4px 8px" }}>
          ✗ not answering: {listenError}. Requests sent to that port reach whatever holds it, not this
          deployment; free the port, then deploy again.
        </div>
      )}
      <LogTab
        entries={entries}
        selected={selected}
        pick={pick}
        slug={slug}
        head={chain.head}
        breaks={chain.breaks}
        order={order}
        nodes={nodes}
        timings={timings}
        exchanges={exchanges}
        step={step}
        onStep={onStep}
        onPickNode={onPickNode}
        issues={diagnosis?.issues ?? new Map()}
        diagnosis={diagnosis ?? null}
        onShowNode={onShowNode}
        onFitToNodes={onFitToNodes}
      />
    </div>
  );
}

/** The run log as a record: the head it chains up to (what a witness holds), any run changed
 *  outside holon, and its runs exported for `holon verify`, which runs each again, offline. */
function ChainBar({ slug, head, breaks }: { slug: string; head: string; breaks: number[] }) {
  const [note, setNote] = useState("");
  const exportRuns = () =>
    downloadRuns(slug).then(
      () => setNote(`saved ${slug}-runs.json · check it anywhere: holon verify ${slug}-runs.json`),
      (e: unknown) => setNote(e instanceof Error ? e.message : String(e)),
    );
  const copy = () =>
    navigator.clipboard.writeText(head).then(
      () => setNote("head copied"),
      () => setNote(head),
    );
  if (!head) return null;
  return (
    <div className="hl-view__chain" style={{ display: "flex", flexWrap: "wrap", gap: 8, alignItems: "center", padding: "4px 8px", borderBottom: "1px solid #1b2a45" }}>
      {breaks.length > 0 ? (
        <span style={{ color: bad }} title="these runs don't follow the run before them in the log file">
          ✗ chain breaks at run {breaks.join(", ")}: changed outside holon
        </span>
      ) : (
        <span style={{ color: dim }} title="each run names the hash of the one before it">
          chain unbroken
        </span>
      )}
      <span style={{ color: dim }}>
        head <span style={{ color: ink }} title={head}>{head.slice(0, 12)}</span>
      </span>
      <button style={small} onClick={copy} title="copy the head: what a witness holds, and what holon verify --head checks">
        copy
      </button>
      <button style={small} onClick={exportRuns} title="save the runs and the units they ran, for holon verify">
        export runs
      </button>
      {note && <span style={{ color: dim }}>{note}</span>}
    </div>
  );
}

type SortKey = "seq" | "time" | "session" | "label" | "status" | "duration";
type FilterableCol = "session" | "label" | "actions" | "status";

function LogTab({
  entries,
  selected,
  pick,
  slug,
  head,
  breaks,
  order,
  nodes,
  timings,
  exchanges,
  step,
  onStep,
  onPickNode,
  issues,
  diagnosis,
  onShowNode,
  onFitToNodes,
}: {
  entries: LogEntry[];
  selected: LogEntry | null;
  pick: (e: LogEntry) => void;
  slug: string;
  head: string;
  breaks: number[];
  order: string[];
  nodes: ViewNode[];
  timings: Map<string, { start_us: number; end_us: number }>;
  exchanges: NodeExchange[];
  step: number | null;
  onStep: (s: number | null) => void;
  onPickNode: (id: string) => void;
  issues: Map<string, Issue>;
  diagnosis: Diagnosis | null;
  onShowNode: (id: string) => void;
  onFitToNodes: (ids: string[]) => void;
}) {
  const [filter, setFilter] = useState("");
  const [sortBy, setSortBy] = useState<SortKey>("seq");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [rightTab, setRightTab] = useState<"details" | "errors" | "timeline">("details");
  const [rightWidth, setRightWidth] = useState<number | null>(rememberedSplit);
  const [selectedSeqs, setSelectedSeqs] = useState<Set<number>>(new Set());
  const [compareRuns, setCompareRuns] = useState<[LogEntry, LogEntry] | null>(null);

  useEffect(() => {
    if (diagnosis) setRightTab("errors");
    else setRightTab((t) => (t === "errors" ? "details" : t));
  }, [selected?.seq]);

  const fitPendingSeq = useRef<number | null>(null);
  useEffect(() => {
    const seq = fitPendingSeq.current;
    if (seq !== null && selected?.seq === seq && order.length > 0) {
      fitPendingSeq.current = null;
      onFitToNodes(order);
    }
  }, [selected, order, onFitToNodes]);

  const [colFilter, setColFilter] = useState<Partial<Record<FilterableCol, Set<string>>>>({});
  const [openFilter, setOpenFilter] = useState<FilterableCol | null>(null);
  const container = useRef<HTMLDivElement>(null);

  const allColValues = useMemo<Record<FilterableCol, string[]>>(() => ({
    session: [...new Set(entries.map((e) => `s${e.session}`))].sort(),
    label: [...new Set(entries.map((e) => runLabel(e)))].sort(),
    actions: [...new Set(entries.flatMap((e) => { const a = runActions(e); return a.length === 0 ? [""] : a.map((x) => x.label); }))].sort(),
    status: [...new Set(entries.map((e) => (e.status !== null ? String(e.status) : "")))].sort(),
  }), [entries]);

  const toggleSort = (key: SortKey) => {
    if (sortBy === key) setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    else { setSortBy(key); setSortDir("desc"); }
  };

  const filt = filter.toLowerCase();
  const filtered = entries.filter((e) => {
    if (filt && !(
      String(e.seq).includes(filt) ||
      runLabel(e).toLowerCase().includes(filt) ||
      (e.url ?? "").toLowerCase().includes(filt) ||
      String(e.session).includes(filt) ||
      (e.status !== null && String(e.status).includes(filt)) ||
      (e.error ?? "").toLowerCase().includes(filt)
    )) return false;
    if (colFilter.session !== undefined) { if (!colFilter.session.has(`s${e.session}`)) return false; }
    if (colFilter.label !== undefined) { if (!colFilter.label.has(runLabel(e))) return false; }
    if (colFilter.status !== undefined) { const v = e.status !== null ? String(e.status) : ""; if (!colFilter.status.has(v)) return false; }
    if (colFilter.actions !== undefined) { const acts = runActions(e); const eVals = acts.length === 0 ? [""] : acts.map((a) => a.label); if (!eVals.some((v) => colFilter.actions!.has(v))) return false; }
    return true;
  });
  const sorted = [...filtered].sort((a, b) => {
    let cmp = 0;
    if (sortBy === "seq") cmp = a.seq - b.seq;
    else if (sortBy === "time") cmp = a.at_ms - b.at_ms;
    else if (sortBy === "session") cmp = a.session - b.session;
    else if (sortBy === "label") cmp = runLabel(a).localeCompare(runLabel(b));
    else if (sortBy === "status") cmp = (a.status ?? -1) - (b.status ?? -1);
    else if (sortBy === "duration") cmp = a.duration_ms - b.duration_ms;
    return sortDir === "asc" ? cmp : -cmp;
  });

  const copyRun = (e: LogEntry, ev: ReactMouseEvent) => {
    ev.stopPropagation();
    navigator.clipboard.writeText(JSON.stringify(e, null, 2)).catch(() => {});
  };
  const exportRun = (e: LogEntry, ev: ReactMouseEvent) => {
    ev.stopPropagation();
    const blob = new Blob([JSON.stringify(e, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `run-${e.seq}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const allVisibleSelected = sorted.length > 0 && sorted.every((e) => selectedSeqs.has(e.seq));
  const toggleAllVisible = () => {
    if (allVisibleSelected) setSelectedSeqs((prev) => { const n = new Set(prev); sorted.forEach((e) => n.delete(e.seq)); return n; });
    else setSelectedSeqs((prev) => { const n = new Set(prev); sorted.forEach((e) => n.add(e.seq)); return n; });
  };
  const copySelected = () => {
    const runs = entries.filter((e) => selectedSeqs.has(e.seq));
    navigator.clipboard.writeText(JSON.stringify(runs, null, 2)).catch(() => {});
  };
  const exportSelected = () => {
    const runs = entries.filter((e) => selectedSeqs.has(e.seq));
    const seqs = [...selectedSeqs].sort((a, b) => a - b);
    const blob = new Blob([JSON.stringify(runs, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a"); a.href = url; a.download = `runs-${seqs.join("-")}.json`; a.click(); URL.revokeObjectURL(url);
  };
  const openCompare = () => {
    const seqs = [...selectedSeqs].sort((a, b) => a - b);
    const runA = entries.find((e) => e.seq === seqs[0]);
    const runB = entries.find((e) => e.seq === seqs[1]);
    if (runA && runB) setCompareRuns([runA, runB]);
  };

  const onDivider = (ev: ReactPointerEvent<HTMLDivElement>) => {
    ev.preventDefault();
    const total = container.current?.getBoundingClientRect().width ?? 600;
    const startX = ev.clientX;
    const startW = rightWidth ?? total * 0.45;
    let last = startW;
    const move = (m: PointerEvent) => {
      last = clampSplit(startW + startX - m.clientX, total);
      setRightWidth(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try { localStorage.setItem(SPLIT_KEY, String(Math.round(last))); } catch {}
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  useEffect(() => {
    if (!openFilter) return;
    const close = () => setOpenFilter(null);
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [openFilter]);

  const SortHeader = ({ k, label, w }: { k: SortKey; label: string; w?: number }) => (
    <span
      onClick={() => toggleSort(k)}
      style={{ width: w, cursor: "pointer", userSelect: "none", color: sortBy === k ? ink : dim, flexShrink: 0 }}
      title={`sort by ${label}`}
    >
      {label}{sortBy === k ? (sortDir === "asc" ? " ▲" : " ▼") : ""}
    </span>
  );

  const FilterDropdown = ({ col, vals }: { col: FilterableCol; vals: string[] }) => {
    const current = colFilter[col];
    const isChecked = (v: string) => !current || current.has(v);
    const allSelected = !current;
    const toggle = (val: string) =>
      setColFilter((prev) => {
        const all = allColValues[col];
        const prevSet = prev[col] ?? new Set(all);
        const next = new Set(prevSet);
        if (next.has(val)) next.delete(val); else next.add(val);
        const updated = { ...prev };
        if (next.size >= all.length) { delete updated[col]; return updated; }
        updated[col] = next;
        return updated;
      });
    const selectAll = () =>
      setColFilter((prev) => {
        if (!prev[col]) { return { ...prev, [col]: new Set<string>() }; }
        const u = { ...prev }; delete u[col]; return u;
      });
    return (
      <div
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
        style={{ position: "absolute", top: "100%", left: 0, zIndex: 50, background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 3, padding: "2px 0", minWidth: 110, maxHeight: 200, overflowY: "auto", boxShadow: "0 4px 12px rgba(0,0,0,0.5)", fontSize: 10 }}
      >
        <label style={{ display: "flex", gap: 5, padding: "2px 8px", cursor: "pointer", color: ink, alignItems: "center" }}>
          <input type="checkbox" checked={allSelected} onChange={selectAll} style={{ accentColor: "#6fd3e0" }} />
          (Select All)
        </label>
        <div style={{ borderBottom: "1px solid #1b2a45", margin: "2px 0" }} />
        {vals.map((v) => (
          <label key={v || "__empty__"} style={{ display: "flex", gap: 5, padding: "2px 8px", cursor: "pointer", color: v === "" ? dim : ink, alignItems: "center" }}>
            <input type="checkbox" checked={isChecked(v)} onChange={() => toggle(v)} style={{ accentColor: "#6fd3e0" }} />
            {v === "" ? "(empty)" : v}
          </label>
        ))}
      </div>
    );
  };

  const ColHeader = ({ sk, label, col, w, flex1 }: { sk?: SortKey; label: string; col: FilterableCol; w?: number; flex1?: boolean }) => {
    const active = !!(colFilter[col]?.size);
    const open = openFilter === col;
    return (
      <span style={{ width: flex1 ? undefined : w, flex: flex1 ? 1 : undefined, minWidth: flex1 ? 0 : undefined, display: "flex", alignItems: "center", position: "relative", flexShrink: flex1 ? 1 : 0 }}>
        {sk ? (
          <span onClick={() => toggleSort(sk)} style={{ flex: 1, cursor: "pointer", userSelect: "none", color: sortBy === sk ? ink : dim, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }} title={`sort by ${label}`}>
            {label}{sortBy === sk ? (sortDir === "asc" ? " ▲" : " ▼") : ""}
          </span>
        ) : (
          <span style={{ flex: 1, color: dim, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>{label}</span>
        )}
        <button onMouseDown={(e) => e.stopPropagation()} onClick={(ev) => { ev.stopPropagation(); setOpenFilter(open ? null : col); }} style={{ font: "inherit", fontSize: 8, background: "transparent", border: "none", color: active ? "#6fd3e0" : dim, cursor: "pointer", padding: 0, flexShrink: 0, lineHeight: 1 }} title={`filter by ${label}`}>
          {active ? "◆" : "▾"}
        </button>
        {open && <FilterDropdown col={col} vals={allColValues[col]} />}
      </span>
    );
  };

  const rightStyle: CSSProperties = rightWidth !== null
    ? { width: rightWidth, flexShrink: 0 }
    : { flex: "1 1 45%" };

  return (
    <>
    <div style={{ display: "flex", flex: 1, minHeight: 0, flexDirection: "column" }}>
      <ChainBar slug={slug} head={head} breaks={breaks} />
      <div style={{ display: "flex", gap: 6, alignItems: "center", padding: "3px 8px", borderBottom: "1px solid #1b2a45", flexShrink: 0 }}>
        <input
          placeholder="filter runs…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          style={{ ...small, background: "#0e1a2f", color: ink, width: 160 }}
        />
        {filter && <span style={{ color: dim }}>{filtered.length} of {entries.length}</span>}
        <span style={{ flex: 1 }} />
        {selectedSeqs.size > 0 && (
          <>
            <span style={{ color: dim, fontSize: 10 }}>{selectedSeqs.size} selected</span>
            <button style={{ ...small, padding: "0 5px" }} title="copy selected runs as JSON" onClick={copySelected}>⎘</button>
            <button style={{ ...small, padding: "0 5px" }} title="export selected runs as JSON" onClick={exportSelected}>↓</button>
            {selectedSeqs.size === 2 && (
              <button style={{ ...small, padding: "0 5px" }} title="compare selected runs" onClick={openCompare}>⇔</button>
            )}
            <button style={{ ...small, padding: "0 4px" }} title="clear selection" onClick={() => setSelectedSeqs(new Set())}>×</button>
          </>
        )}
      </div>
      <div ref={container} style={{ display: "flex", flex: 1, minHeight: 0 }}>
        {/* Left: run list */}
        <div style={{ display: "flex", flexDirection: "column", flex: rightWidth !== null ? "1 1 0" : "1 1 55%", minWidth: 0, borderRight: "1px solid #1b2a45" }}>
          <div style={{ display: "flex", gap: 10, padding: "2px 6px", borderBottom: "1px solid #1b2a45", fontSize: 10, flexShrink: 0 }}>
            <input type="checkbox" checked={allVisibleSelected} onChange={toggleAllVisible} style={{ width: 12, cursor: "pointer", accentColor: "#6fd3e0", flexShrink: 0 }} title={allVisibleSelected ? "deselect all visible" : "select all visible"} />
            <SortHeader k="seq" label="#" w={30} />
            <SortHeader k="time" label="time" w={84} />
            <ColHeader sk="session" label="s" col="session" w={22} />
            <ColHeader sk="label" label="route / action" col="label" flex1 />
            <ColHeader label="actions" col="actions" w={56} />
            <ColHeader sk="status" label="st" col="status" w={26} />
            <SortHeader k="duration" label="ms" w={52} />
            <span style={{ width: 44, flexShrink: 0 }} />
          </div>
          <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "0 0 8px" }}>
            {sorted.length === 0 && (
              <div style={{ padding: 8, color: dim }}>
                {entries.length === 0 ? "no runs yet: send it a request" : "no runs match the filter"}
              </div>
            )}
            {sorted.map((e) => {
              const acts = runActions(e);
              return (
                <div
                  key={e.seq}
                  onClick={() => pick(e)}
                  onDoubleClick={() => {
                    if (order.length > 0 && selected?.seq === e.seq) onFitToNodes(order);
                    else fitPendingSeq.current = e.seq;
                  }}
                  style={{
                    display: "flex",
                    gap: 10,
                    padding: "2px 6px",
                    cursor: "pointer",
                    borderRadius: 3,
                    background: selectedSeqs.has(e.seq) ? "#0d1e38" : e.seq === selected?.seq ? "#13243f" : "transparent",
                    alignItems: "center",
                  }}
                >
                  <input
                    type="checkbox"
                    checked={selectedSeqs.has(e.seq)}
                    onChange={() => {}}
                    onClick={(ev) => { ev.stopPropagation(); setSelectedSeqs((prev) => { const n = new Set(prev); if (n.has(e.seq)) n.delete(e.seq); else n.add(e.seq); return n; }); }}
                    style={{ width: 12, cursor: "pointer", accentColor: "#6fd3e0", flexShrink: 0 }}
                  />
                  <span style={{ color: dim, width: 30, textAlign: "right", flexShrink: 0 }}>#{e.seq}</span>
                  <span style={{ color: dim, width: 84, whiteSpace: "nowrap", flexShrink: 0 }}>{time(e.at_ms)}</span>
                  <span style={{ color: dim, width: 22, flexShrink: 0 }}>s{e.session}</span>
                  <span style={{ flex: 1, color: ink, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>
                    {runLabel(e)}
                    {e.url && e.route ? <span style={{ color: dim }}> · {e.url}</span> : null}
                  </span>
                  <span style={{ width: 56, display: "flex", gap: 3, alignItems: "center", flexShrink: 0, overflow: "hidden" }}>
                    {acts.map((a) => (
                      <span key={a.label} title={a.title} style={{ color: a.color, fontSize: 9, letterSpacing: "0.02em" }}>{a.label}</span>
                    ))}
                    {e.error && <span style={{ color: bad, fontSize: 9 }}>err</span>}
                    {(e.parked ?? []).length > 0 && <span style={{ color: "#e0c36f", fontSize: 9 }} title="waiting for an event">⏸</span>}
                  </span>
                  <span style={{ color: e.status !== null && e.status < 400 ? good : bad, width: 26, flexShrink: 0 }}>
                    {e.status ?? ""}
                  </span>
                  <span style={{ color: dim, width: 52, textAlign: "right", flexShrink: 0 }}>{e.duration_ms.toFixed(2)}</span>
                  <span style={{ display: "flex", gap: 2, width: 44, flexShrink: 0 }}>
                    <button style={{ ...small, padding: "0px 4px", fontSize: 10 }} title="copy run as JSON" onClick={(ev) => copyRun(e, ev)}>⎘</button>
                    <button style={{ ...small, padding: "0px 4px", fontSize: 10 }} title="download run as JSON" onClick={(ev) => exportRun(e, ev)}>↓</button>
                  </span>
                </div>
              );
            })}
          </div>
        </div>
        {/* Draggable divider */}
        <div
          style={{ width: 5, cursor: "col-resize", background: "transparent", flexShrink: 0 }}
          onPointerDown={onDivider}
          title="drag to resize"
        />
        {/* Right pane: Details | Errors | Timeline tabs */}
        <div style={{ ...rightStyle, display: "flex", flexDirection: "column", minWidth: 0 }}>
          <Tabs
            tabs={[
              ["details", "Details"],
              ...(diagnosis ? [["errors", "Errors"] as ["errors", string]] : []),
              ["timeline", "Timeline"],
            ]}
            tab={rightTab}
            onTab={setRightTab}
          />
          {rightTab === "details" ? (
            <div style={{ ...scroll, display: "flex", flexDirection: "column", gap: 4 }}>
              {selected ? (
                <>
                  {selected.url && (
                    <div style={{ color: ink }}>{selected.method} {selected.url}</div>
                  )}
                  {selected.request && <Field name="request" value={formatValue(selected.request)} rawValue={selected.request} />}
                  {selected.sourced.map(([cap, v]) => (
                    <Field key={cap} name={cap.toLowerCase()} value={formatValue(v)} rawValue={v} />
                  ))}
                  {selected.performed.map((p, i) => (
                    <Field key={i} name={`did ${p.capability}`} value={formatValue(p.payload)} rawValue={p.payload} />
                  ))}
                  {selected.response_body !== null && (
                    <Field name={`answered ${selected.status}`} value={selected.response_body || "(empty)"} rawValue={selected.response_body ? { type: "Text" as const, value: selected.response_body } : undefined} />
                  )}
                  {selected.error && <Field name="error" value={selected.error} color={bad} />}
                  {(selected.rounds ?? []).map((r, i) =>
                    r.exchanges.map((x, j) => {
                      const { line, ok } = exchangeLine(x.request, x.response);
                      return (
                        <Field key={`${i}-${j}`} name={`round ${i + 1} · ${us(x.end_us - x.start_us)}`} value={line} color={ok ? ink : bad} />
                      );
                    }),
                  )}
                  {(selected.reads ?? []).map((r, i) => (
                    <Field key={`r${i}`} name="read" value={`${formatValue(r.request)} → ${r.answer ? formatValue(r.answer) : "nothing"}`} />
                  ))}
                  {(selected.retries ?? 0) > 0 && (
                    <Field name="ran again" value={`${selected.retries}× — what it read changed before it could commit, so it ran again from the new state`} color="#e0c36f" />
                  )}
                  {(selected.parked ?? []).length > 0 && (
                    <Field name="waiting at" value={(selected.parked ?? []).join(", ")} color="#e0c36f" />
                  )}
                  {Object.entries(selected.events ?? {}).map(([name, v]) => (
                    <Field key={name} name={`event ${name}`} value={formatValue(v)} rawValue={v} />
                  ))}
                  {selected.run_id && (
                    <Field name="run" value={`${selected.run_id.slice(0, 16)}… · grant ${(selected.grant ?? "").slice(0, 12)}…`} color={dim} />
                  )}
                </>
              ) : (
                <div>select a run</div>
              )}
            </div>
          ) : rightTab === "errors" && diagnosis ? (
            <div style={{ ...scroll, display: "flex", flexDirection: "column", gap: 6 }}>
              <span style={{ color: FAIL }}>{diagnosis.summary}</span>
              {diagnosis.origins.length === 0 && <span style={{ color: dim }}>no node on the canvas stopped the run</span>}
              {diagnosis.origins.map((id) => {
                const n = nodes.find((x) => x.id === id);
                const lbl = n ? nodeLabel(n) : id;
                return (
                  <span key={id} style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
                    <span>failed at <b>{lbl}</b>: {diagnosis.issues.get(id)?.reason}</span>
                    <button style={{ ...small, color: FAIL, borderColor: FAIL }} onClick={() => onShowNode(id)} title="show it on the canvas">show</button>
                  </span>
                );
              })}
            </div>
          ) : (
            <TimelineTab
              run={selected}
              order={order}
              nodes={nodes}
              timings={timings}
              exchanges={exchanges}
              step={step}
              onStep={onStep}
              onPickNode={onPickNode}
              issues={issues}
            />
          )}
        </div>
      </div>
    </div>
    {compareRuns && <CompareModal a={compareRuns[0]} b={compareRuns[1]} onClose={() => setCompareRuns(null)} />}
    </>
  );
}

function Field({ name, value, color, rawValue }: { name: string; value: string; color?: string; rawValue?: WValue }) {
  const [modalOpen, setModalOpen] = useState(false);
  return (
    <div style={{ display: "flex", gap: 8 }}>
      {/* A long name (a hash used as a key) wraps within its share instead of crowding out the value. */}
      <span style={{ color: dim, minWidth: 90, maxWidth: "40%", flexShrink: 0, overflowWrap: "anywhere" }}>{name}</span>
      <span style={{ color: color ?? ink, overflowWrap: "anywhere", whiteSpace: "pre-wrap", flex: 1, minWidth: 0 }}>{value}</span>
      {rawValue != null && (
        <button onClick={() => setModalOpen(true)} style={{ ...small, padding: "0px 4px", fontSize: 10, lineHeight: 1.4, flexShrink: 0 }} title="view value">⤢</button>
      )}
      {modalOpen && rawValue != null && <ValueModal value={rawValue} portName={name} onClose={() => setModalOpen(false)} />}
    </div>
  );
}

type RunSection = { key: string; value: string; color?: string; rawValue?: WValue };

function runSectionsForCompare(e: LogEntry): RunSection[] {
  const s: RunSection[] = [];
  if (e.url) s.push({ key: `${e.method ?? "GET"} ${e.url}`, value: "" });
  if (e.request) s.push({ key: "request", value: formatValue(e.request), rawValue: e.request });
  for (const [cap, v] of e.sourced) s.push({ key: cap.toLowerCase(), value: formatValue(v), rawValue: v });
  for (const p of e.performed) s.push({ key: `did ${p.capability.toLowerCase()}`, value: formatValue(p.payload), rawValue: p.payload });
  if (e.response_body !== null) s.push({ key: "answered", value: `${e.status ?? ""}: ${e.response_body || "(empty)"}`, rawValue: e.response_body ? { type: "Text" as const, value: e.response_body } : undefined });
  if (e.error) s.push({ key: "error", value: e.error, color: bad });
  const rounds = e.rounds ?? [];
  if (rounds.length > 0) {
    const lines = rounds.flatMap((r, i) => r.exchanges.map((x) => { const { line } = exchangeLine(x.request, x.response); return `round ${i + 1}: ${line}`; }));
    s.push({ key: "rounds", value: lines.join("\n") });
  }
  const reads = e.reads ?? [];
  if (reads.length > 0) {
    const lines = reads.map((r) => `${formatValue(r.request)} → ${r.answer ? formatValue(r.answer) : "nothing"}`);
    s.push({ key: "read", value: lines.join("\n") });
  }
  if ((e.retries ?? 0) > 0) s.push({ key: "ran again", value: `${e.retries}×`, color: "#e0c36f" });
  if ((e.parked ?? []).length > 0) s.push({ key: "waiting at", value: (e.parked ?? []).join(", "), color: "#e0c36f" });
  for (const [name, v] of Object.entries(e.events ?? {})) s.push({ key: `event ${name}`, value: formatValue(v), rawValue: v });
  if (e.run_id) s.push({ key: "run", value: `${e.run_id.slice(0, 16)}… · grant ${(e.grant ?? "").slice(0, 12)}…`, color: dim });
  return s;
}

function CompareModal({ a, b, onClose }: { a: LogEntry; b: LogEntry; onClose: () => void }) {
  const [viewValue, setViewValue] = useState<{ value: WValue; name: string } | null>(null);
  useEffect(() => {
    const fn = (e: KeyboardEvent) => { if (e.key === "Escape") { if (viewValue) setViewValue(null); else onClose(); } };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, [onClose, viewValue]);

  const sa = runSectionsForCompare(a);
  const sb = runSectionsForCompare(b);
  const mapA = new Map(sa.map((s) => [s.key, s]));
  const mapB = new Map(sb.map((s) => [s.key, s]));
  const allKeys: string[] = [];
  const seen = new Set<string>();
  for (const s of [...sa, ...sb]) if (!seen.has(s.key)) { seen.add(s.key); allKeys.push(s.key); }

  return (
    <div
      style={{ position: "fixed", inset: 0, zIndex: 200, background: "rgba(5,10,20,0.75)", display: "flex", alignItems: "center", justifyContent: "center" }}
      onClick={onClose}
    >
      <div
        style={{ background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 6, width: "min(96vw, 1100px)", maxHeight: "85vh", display: "flex", flexDirection: "column", overflow: "hidden" }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ display: "flex", alignItems: "center", padding: "8px 14px", borderBottom: "1px solid #1b2a45", flexShrink: 0, gap: 8 }}>
          <span style={{ color: ink, fontWeight: 600 }}>Compare runs</span>
          <span style={{ flex: 1 }} />
          <button onClick={onClose} style={{ background: "transparent", border: "none", color: dim, cursor: "pointer", fontSize: 16, padding: "0 4px", lineHeight: 1 }} title="close">×</button>
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 110px 1fr", borderBottom: "1px solid #1b2a45", padding: "5px 14px", flexShrink: 0, gap: 8, fontSize: 11 }}>
          <span style={{ color: ink, fontWeight: 600 }}>#{a.seq} · {time(a.at_ms)} · {runLabel(a)}</span>
          <span />
          <span style={{ color: ink, fontWeight: 600 }}>#{b.seq} · {time(b.at_ms)} · {runLabel(b)}</span>
        </div>
        {viewValue && <ValueModal value={viewValue.value} portName={viewValue.name} onClose={() => setViewValue(null)} />}
        <div style={{ overflowY: "auto", flex: 1, padding: "4px 0" }}>
          {allKeys.map((key) => {
            const rowA = mapA.get(key);
            const rowB = mapB.get(key);
            const bothPresent = !!(rowA && rowB);
            const identical = bothPresent && rowA!.value === rowB!.value;
            const keyColor = !bothPresent ? dim : identical ? "#6fd3e0" : "#e0c36f";
            return (
              <div key={key} style={{ display: "grid", gridTemplateColumns: "1fr 110px 1fr", padding: "4px 14px", gap: 8, borderBottom: "1px solid #1b2a4520", alignItems: "start" }}>
                <div style={{ color: rowA?.color ?? (rowA ? ink : dim), fontSize: 12, fontFamily: "var(--mono)", whiteSpace: "pre-wrap", wordBreak: "break-word", minHeight: 16, display: "flex", alignItems: "flex-start", gap: 4 }}>
                  <span style={{ flex: 1 }}>{rowA?.value !== undefined ? (rowA.value || <span style={{ color: dim }}>—</span>) : <span style={{ color: dim }}>—</span>}</span>
                  {rowA?.rawValue && <button onClick={(e) => { e.stopPropagation(); setViewValue({ value: rowA!.rawValue!, name: `${key} · run #${a.seq}` }); }} style={{ ...small, padding: "0px 4px", fontSize: 9, flexShrink: 0, lineHeight: 1.4 }} title="view value">⤢</button>}
                </div>
                <div style={{ color: keyColor, fontSize: 10, textAlign: "center", paddingTop: 3, fontWeight: bothPresent ? 600 : 400, wordBreak: "break-word" }}>{key}</div>
                <div style={{ color: rowB?.color ?? (rowB ? ink : dim), fontSize: 12, fontFamily: "var(--mono)", whiteSpace: "pre-wrap", wordBreak: "break-word", minHeight: 16, display: "flex", alignItems: "flex-start", gap: 4 }}>
                  <span style={{ flex: 1 }}>{rowB?.value !== undefined ? (rowB.value || <span style={{ color: dim }}>—</span>) : <span style={{ color: dim }}>—</span>}</span>
                  {rowB?.rawValue && <button onClick={(e) => { e.stopPropagation(); setViewValue({ value: rowB!.rawValue!, name: `${key} · run #${b.seq}` }); }} style={{ ...small, padding: "0px 4px", fontSize: 9, flexShrink: 0, lineHeight: 1.4 }} title="view value">⤢</button>}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/**
 * The selected run as a timeline: one row per node, in the order it was evaluated, with a bar
 * for when it ran (the server's timings). Without timings (an older server, a node it doesn't
 * time) a row gets an even slot by its place in the order.
 */
function TimelineTab({
  run,
  order,
  nodes,
  timings,
  exchanges,
  step,
  onStep,
  onPickNode,
  issues,
}: {
  run: LogEntry | null;
  /** The nodes in the order the run evaluated them: the steps. */
  order: string[];
  nodes: ViewNode[];
  timings: Map<string, { start_us: number; end_us: number }>;
  exchanges: NodeExchange[];
  step: number | null;
  onStep: (s: number | null) => void;
  onPickNode: (id: string) => void;
  /** Each node's part in a failed run. */
  issues: Map<string, Issue>;
}) {
  const [sort, setSort] = useState<"executed" | "slowest">("executed");
  if (!run) return <div style={scroll}>select a run</div>;
  if (order.length === 0) return <div style={scroll}>this run touched no node on the canvas</div>;
  const timed = timings.size > 0;
  // One scale for the steps and the requests between rounds.
  const span = timed
    ? Math.max(...[...timings.values()].map((t) => t.end_us), ...exchanges.map((x) => x.end_us), 1)
    : order.length;
  const ticks = timed ? [0, span / 2, span] : [];
  // Rows keep their step number (their place in the run) however they are listed.
  const took = (id: string) => {
    const t = timings.get(id);
    return t ? t.end_us - t.start_us : -1;
  };
  const rows = order.map((id, i) => ({ id, i }));
  if (sort === "slowest") rows.sort((a, b) => took(b.id) - took(a.id));
  return (
    <div style={scroll}>
      <div style={{ display: "flex", gap: 6, alignItems: "center", color: dim, marginBottom: 4 }}>
        list
        {(
          [
            ["executed", "in the order they ran"],
            ["slowest", "slowest first"],
          ] as const
        ).map(([key, text]) => (
          <button
            key={key}
            style={{ ...small, color: sort === key ? "#0e1a2f" : "#9fb4d4", background: sort === key ? "#6fd3e0" : "transparent" }}
            onClick={() => setSort(key)}
          >
            {text}
          </button>
        ))}
      </div>
      <div style={{ display: "flex", gap: 8, color: dim, marginBottom: 4 }}>
        <span style={{ width: 22 }} />
        <span style={{ width: 170 }}>node</span>
        <div style={{ flex: 1, position: "relative", height: 14 }}>
          {timed ? (
            ticks.map((t, i) => (
              <span
                key={i}
                style={{
                  position: "absolute",
                  left: `${(t / span) * 100}%`,
                  transform: i === 0 ? "none" : i === ticks.length - 1 ? "translateX(-100%)" : "translateX(-50%)",
                }}
              >
                {us(t)}
              </span>
            ))
          ) : (
            <span>evaluation order (no timings recorded)</span>
          )}
        </div>
        <span style={{ width: 60, textAlign: "right" }}>took</span>
      </div>
      {rows.map(({ id, i }) => {
        const n = nodes.find((x) => x.id === id);
        const issue = issues.get(id);
        const t = timings.get(id);
        const [from, to] = t ? [t.start_us, t.end_us] : timed ? [0, 0] : [i, i + 1];
        const active = step === i;
        const done = step === null || i < step;
        const writes = n?.data.name === "store_write";
        return (
          <div
            key={id}
            onClick={() => {
              onStep(i);
              onPickNode(id);
            }}
            style={{
              display: "flex",
              gap: 8,
              alignItems: "center",
              padding: "2px 0",
              cursor: "pointer",
              background: active ? "#13243f" : "transparent",
              borderRadius: 3,
            }}
          >
            <span style={{ width: 22, textAlign: "right", color: issue?.role === "origin" ? FAIL : dim }}>
              {issue?.role === "origin" ? "✗" : i + 1}
            </span>
            <span
              style={{
                width: 170,
                color: issue?.role === "origin" ? FAIL : issue ? "#d9a596" : done || active ? ink : dim,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
              title={(n ? nodeLabel(n) : id) + (issue ? ` · ${issue.reason}` : "")}
            >
              {n ? nodeLabel(n) : id}
              {writes && <span style={{ color: "#e0c36f" }}> ✎</span>}
              {issue?.role === "starved" && <span> · didn't get its input</span>}
            </span>
            <div style={{ flex: 1, position: "relative", height: 10, background: "#0e1a2f", borderRadius: 2 }}>
              {(t || !timed) && (
                <div
                  style={{
                    position: "absolute",
                    left: `${(from / span) * 100}%`,
                    width: `max(2px, ${((to - from) / span) * 100}%)`,
                    top: 0,
                    bottom: 0,
                    borderRadius: 2,
                    background: issue?.role === "origin"
                      ? FAIL
                      : active
                        ? "#6fd3e0"
                        : writes
                          ? "#e0c36f"
                          : done
                            ? "#3f7fa8"
                            : "#24384f",
                  }}
                />
              )}
            </div>
            <span style={{ width: 60, textAlign: "right", color: dim }}>{t ? us(t.end_us - t.start_us) : "—"}</span>
          </div>
        );
      })}
      {timed && exchanges.length > 0 && (
        <div style={{ color: dim, margin: "6px 0 2px" }}>outbound requests, performed between rounds</div>
      )}
      {timed &&
        exchanges.map((x, i) => {
          const { line, ok } = exchangeLine(x.request, x.response);
          return (
            <div
              key={`x${i}`}
              onClick={() => onPickNode(x.id)}
              style={{ display: "flex", gap: 8, alignItems: "center", padding: "2px 0", cursor: "pointer" }}
              title={line}
            >
              <span style={{ width: 22, textAlign: "right", color: dim }}>↗</span>
              <span style={{ width: 170, color: ok ? ink : bad, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                round {x.round + 1} · {line}
              </span>
              <div style={{ flex: 1, position: "relative", height: 10, background: "#0e1a2f", borderRadius: 2 }}>
                <div
                  style={{
                    position: "absolute",
                    left: `${(x.start_us / span) * 100}%`,
                    width: `max(2px, ${((x.end_us - x.start_us) / span) * 100}%)`,
                    top: 0,
                    bottom: 0,
                    borderRadius: 2,
                    background: ok ? "#c58af2" : bad,
                  }}
                />
              </div>
              <span style={{ width: 60, textAlign: "right", color: dim }}>{us(x.end_us - x.start_us)}</span>
            </div>
          );
        })}
      <div style={{ color: dim, marginTop: 6 }}>
        whole run {run.duration_ms.toFixed(2)}ms · ✎ writes state (committed when the run finishes)
        {exchanges.length > 0 ? " · steps shown from the final round" : ""}
      </div>
    </div>
  );
}

/** What the Inspector shows of a run, in view mode: the flow state as of the step it is at. */
export type InspectorRun = {
  slug: string;
  run: LogEntry | null;
  step: number | null;
  order: string[];
  /** The state with the writes up to `step` applied, and the keys they changed. */
  stateAfter: Record<string, WValue>;
  changed: string[];
  /** The collections the run touched, as of `step` (replay.ts's collectionsAtStep). */
  collections: CollectionAtStep[];
  /** What the node computed in this run. */
  trace: NodeTrace | undefined;
  /** Why it didn't run, when it is in the run's group but a choice didn't need it. */
  notRun?: string;
  timing: { start_us: number; end_us: number } | undefined;
  /** For a holon: its own nodes, as the run went (null for any other node). */
  inside: InsideStep[] | null;
};

type InspectorTab = "node" | "unit" | "state";

/**
 * The Inspector, on the canvas's right in both modes: the selected node (its docs, params and
 * ports, and in view mode what it computed in the run), what its unit is made of, and in view
 * mode the flow state. `unit` is the selected node's unit hash, for the unit tab; `run` is
 * there only in view mode. Folds to a strip, remembered per browser.
 */
export function Inspector({
  node,
  unit,
  run,
  source,
  open,
  onOpen,
}: {
  node: ViewNode | null;
  unit: ReactNode;
  run: InspectorRun | null;
  /** The node's line in the flow as text (holonText.ts), when the flow prints. */
  source?: { line: number; text: string } | null;
  open: boolean;
  onOpen: (open: boolean) => void;
}) {
  const [picked, setTab] = useState<InspectorTab>("node");
  const tab = picked === "state" && !run ? "node" : picked;
  const tabs: [InspectorTab, string][] = [["node", "Node"], ["unit", "Unit"], ...(run ? [["state", "Flow state"] as [InspectorTab, string]] : [])];
  if (!open) {
    return (
      <div className="nodrag nowheel hl-inspector hl-inspector--closed" style={paneStyle}>
        <button className="hl-inspector__fold" onClick={() => onOpen(true)} title="open the Inspector" aria-label="open the Inspector">
          ‹ Inspector
        </button>
      </div>
    );
  }
  return (
    <div className="nodrag nowheel hl-inspector" style={paneStyle} role="complementary" aria-label="Inspector">
      <Tabs tabs={tabs} tab={tab} onTab={setTab}>
        <button className="hl-inspector__close" onClick={() => onOpen(false)} title="fold the Inspector away" aria-label="fold the Inspector">
          ›
        </button>
      </Tabs>
      <div style={{ ...scroll, display: "flex", flexDirection: "column", gap: 6 }}>
        {tab === "state" && run ? (
          <StateTab slug={run.slug} run={run.run} step={run.step} order={run.order} after={run.stateAfter} changed={run.changed} collections={run.collections} />
        ) : tab === "unit" ? (
          node ? unit : <div>select a node to see what its unit is made of</div>
        ) : (
          <ComponentTab node={node} run={run} source={source ?? null} />
        )}
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
      <div style={{ color: dim, letterSpacing: "0.06em", textTransform: "uppercase", fontSize: 10 }}>{title}</div>
      {children}
    </div>
  );
}

function StateTab({
  slug,
  run,
  step,
  order,
  after,
  changed,
  collections,
}: {
  slug: string;
  run: LogEntry | null;
  step: number | null;
  order: string[];
  after: Record<string, WValue>;
  changed: string[];
  collections: CollectionAtStep[];
}) {
  if (!run) return <div>select a run</div>;
  const before = run.state_before;
  // A collection's root is a state cell; it shows as its collection, below, not as a hash.
  const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].filter((k) => !isCollectionCell(k)).sort();
  const items = collections.reduce((n, c) => n + c.items.filter(itemChanged).length, 0);
  const counted = [
    changed.length > 0 ? `${changed.length} key${changed.length === 1 ? "" : "s"}` : "",
    items > 0 ? `${items} item${items === 1 ? "" : "s"}` : "",
  ].filter(Boolean);
  return (
    <>
      <div style={{ color: dim }}>
        {step === null ? `after the whole run (${order.length} nodes)` : `after node ${step + 1} of ${order.length}`}
        {counted.length > 0 ? ` · ${counted.join(" and ")} changed` : " · no change"}
      </div>
      {keys.length === 0 && collections.length === 0 && <div>no state: nothing read or written</div>}
      {keys.map((k) => {
        const moved = changed.includes(k);
        return (
          <div
            key={k}
            style={{ border: "1px solid #1b2a45", borderRadius: 3, padding: 6, background: moved ? changedBg : "transparent" }}
          >
            <div style={{ color: moved ? "#e0c36f" : ink, marginBottom: 2 }}>
              {k}
              {moved ? " · changed" : ""}
            </div>
            <Field name="before" value={show(before[k])} rawValue={before[k]} />
            {moved && <Field name="after" value={show(after[k])} color="#e0c36f" rawValue={after[k]} />}
          </div>
        );
      })}
      {collections.map((c) => (
        <CollectionBox key={c.name} slug={slug} c={c} />
      ))}
      <div style={{ color: dim }}>writes are committed when the run finishes: the next run reads “after”.</div>
    </>
  );
}

/**
 * A collection as the run saw and changed it: each item it read or wrote, before → after (a
 * before the log doesn't hold, for an item it wrote without reading, is fetched as of the root the
 * run started from), the listings it made, and every item as the run found it, a page at a time.
 */
function CollectionBox({ slug, c }: { slug: string; c: CollectionAtStep }) {
  const [fetched, setFetched] = useState<Record<string, WValue | null>>({});
  const [all, setAll] = useState<{ items: [string, WValue][]; next: string | null } | null>(null);
  const [error, setError] = useState("");
  const unread = c.items.filter((i) => i.before === undefined).map((i) => i.key);
  const unreadKey = unread.join("\u0000");
  useEffect(() => {
    if (!c.root) return;
    for (const key of unreadKey ? unreadKey.split("\u0000") : []) {
      getCollectionItems(slug, c.name, { root: c.root, key })
        .then((r) => setFetched((f) => ({ ...f, [key]: r.items[0]?.[1] ?? null })))
        .catch(() => setFetched((f) => ({ ...f, [key]: null })));
    }
  }, [slug, c.name, c.root, unreadKey]);
  useEffect(() => setAll(null), [c.root]);
  const page = (after: string | null) => {
    setError("");
    getCollectionItems(slug, c.name, { root: c.root, after, limit: 20 })
      .then((r) => setAll((a) => ({ items: [...(after ? (a?.items ?? []) : []), ...r.items], next: r.next ?? null })))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  };
  const changedItems = c.items.filter(itemChanged).length;
  return (
    <div style={{ border: "1px solid #1b2a45", borderRadius: 3, padding: 6, background: changedItems > 0 ? changedBg : "transparent" }}>
      <div style={{ color: changedItems > 0 ? "#e0c36f" : ink, marginBottom: 2 }} title={c.root ? `root ${c.root}` : "no items before this run"}>
        {c.name}
        <span style={{ color: dim }}> · collection</span>
        {changedItems > 0 ? ` · ${changedItems} item${changedItems === 1 ? "" : "s"} changed` : ""}
      </div>
      {c.items.map((i) => {
        // An empty collection before the run (no root) had nothing: no need to ask.
        const before = i.before !== undefined ? i.before : c.root ? fetched[i.key] : null;
        const moved = itemChanged(i);
        const was = before === undefined ? "…" : before === null ? "(none)" : show(before);
        return (
          <Field
            key={i.key}
            name={i.key}
            value={i.after === undefined ? `${was} · read` : `${was} → ${i.after === null ? "(deleted)" : show(i.after)}`}
            color={moved ? "#e0c36f" : undefined}
          />
        );
      })}
      {c.listed.map((l, n) => (
        <div key={`l${n}`} style={{ color: dim }}>
          listed {l.count} item{l.count === 1 ? "" : "s"}
          {l.prefix ? ` starting "${l.prefix}"` : ""}
          {l.after ? ` after "${l.after}"` : ""}
        </div>
      ))}
      {c.items.length === 0 && c.listed.length === 0 && <div style={{ color: dim }}>not read or written by this run</div>}
      {c.root && (
        <div style={{ marginTop: 4 }}>
          {all === null ? (
            <span style={{ cursor: "pointer", color: "#9fb4d4" }} onClick={() => page(null)} title="every item, as this run found the collection">
              show all ▸
            </span>
          ) : (
            <>
              <div style={{ color: dim }}>as the run found it:</div>
              {all.items.map(([k, v]) => (
                <Field key={k} name={k} value={show(v)} />
              ))}
              {all.items.length === 0 && <div style={{ color: dim }}>empty</div>}
              <span style={{ display: "flex", gap: 10 }}>
                {all.next && (
                  <span style={{ cursor: "pointer", color: "#9fb4d4" }} onClick={() => page(all.next)}>
                    more ▸
                  </span>
                )}
                <span style={{ cursor: "pointer", color: dim }} onClick={() => setAll(null)}>
                  hide
                </span>
              </span>
            </>
          )}
          {error && <div style={{ color: bad }}>{error}</div>}
        </div>
      )}
    </div>
  );
}

/** One node inside a holon, as a run went: what it is, what it produced, how long it took. */
export type InsideStep = { label: string; outputs: (WValue | null)[] | null; us: number };

function ComponentTab({ node, run, source }: { node: ViewNode | null; run: InspectorRun | null; source: { line: number; text: string } | null }) {
  const describe = useNodeDocs();
  if (!node) return <div>{run ? "click a node on the canvas, or step through the run" : "select a node on the canvas"}</div>;
  const { trace, timing, inside, notRun } = run ?? { trace: undefined, timing: undefined, inside: null, notRun: undefined };
  const { data } = node;
  const doc = describe(data as Parameters<typeof describe>[0]);
  const kind = data.body.kind === "Template" ? "template" : data.body.kind === "Composite" ? "composite" : "primitive";
  return (
    <>
      <div style={{ color: ink, fontSize: 12 }}>{nodeLabel(node)}</div>
      {doc && <div style={{ color: "#9fb4d4", whiteSpace: "normal", lineHeight: 1.45 }}>{doc}</div>}
      <div style={{ color: dim }}>
        {kind}
        {timing ? ` · ran ${us(timing.start_us)} → ${us(timing.end_us)} (${us(timing.end_us - timing.start_us)})` : ""}
        {run && !trace && !notRun ? " · not part of this run" : ""}
      </div>
      {notRun && <div style={{ color: dim }}>↷ {notRun}</div>}
      {source && (
        <Section title={`as text, line ${source.line}`}>
          <div
            className="hl-inspector__source"
            style={{ color: "#cfe3ff", fontFamily: "ui-monospace, Menlo, Consolas, monospace", whiteSpace: "pre-wrap", wordBreak: "break-word" }}
            title="the flow as the language writes it (.holon): this node is part of this line"
          >
            {source.text.trim()}
          </div>
        </Section>
      )}
      {trace?.issue && (
        <div style={{ color: trace.issue.role === "origin" ? FAIL : "#d9a596" }}>
          {trace.issue.role === "origin" ? "✗ failed here: " : "↯ "}
          {trace.issue.reason}
        </div>
      )}
      {trace?.recorded && (
        <div style={{ color: dim }}>outputs as the run recorded them: an input is a secret or token, or it reads from outside within itself, which replay never has</div>
      )}
      {trace?.placeholder && (
        <div style={{ color: "#e0c36f" }}>
          recomputed from a secret's or token's placeholder (the log has no record): may differ from the run
        </div>
      )}
      {data.params.length > 0 && (
        <Section title="params">
          {data.params.map((p) => (
            <Field key={p.name} name={p.name} value={p.kind === "Frozen" ? p.value : `open → ${p.bind}`} />
          ))}
        </Section>
      )}
      {inside && (
        <Section title="inside, as the run went">
          {inside.length === 0 && <div style={{ color: dim }}>this run's log doesn't go inside (logged before it did)</div>}
          {inside.map((st, i) => (
            <Field
              key={i}
              name={st.label}
              value={st.outputs === null ? "(not kept)" : st.outputs.map((o) => (o === null ? "—" : formatValue(o))).join(" · ") + `  ${us(st.us)}`}
            />
          ))}
        </Section>
      )}
      <Section title="inputs">
        {data.inputs.length === 0 && <div>none</div>}
        {data.inputs.map((p, i) => (
          <Port key={p.name} port={p} value={trace?.inputs[i]} ran={!!trace} />
        ))}
      </Section>
      <Section title="outputs">
        {data.outputs.map((p, i) => (
          <Port key={p.name} port={p} value={trace?.outputs[i]} ran={!!trace} />
        ))}
      </Section>
    </>
  );
}

const RENDERER_LABELS: Record<RendererKind, string> = {
  raw: "Raw", text: "Text", json: "JSON", html: "HTML",
  markdown: "Markdown", image: "Image", audio: "Audio",
  table: "Table", record: "Key-value",
};

function MarkdownView({ text }: { text: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.innerHTML = "";
    const parts = text.split(/```[a-z]*\n?/);
    parts.forEach((part, idx) => {
      if (idx % 2 === 1) {
        const pre = document.createElement("pre");
        pre.style.cssText = "margin:0 0 8px;padding:8px;background:#0b1527;border-radius:3px;overflow-x:auto;font-size:12px";
        pre.textContent = part.replace(/\n$/, "");
        el.appendChild(pre);
        return;
      }
      for (const line of part.split(/\n{2,}|\n/).filter((l) => l.trim() !== "")) {
        const m = line.match(/^(#{1,6}) (.*)/);
        if (m) {
          const h = document.createElement(`h${m[1].length}`);
          h.style.cssText = "margin:0 0 6px;font-size:13px";
          h.textContent = m[2];
          el.appendChild(h);
        } else {
          const p = document.createElement("p");
          p.style.cssText = "margin:0 0 6px;line-height:1.6";
          for (const bit of line.split(/(`[^`]*`|\*\*[^*]*\*\*)/)) {
            if (bit.startsWith("`") && bit.endsWith("`") && bit.length > 1) {
              const c = document.createElement("code");
              c.style.cssText = "background:#0b1527;padding:1px 4px;border-radius:2px;font-size:11px";
              c.textContent = bit.slice(1, -1);
              p.appendChild(c);
            } else if (bit.startsWith("**") && bit.endsWith("**") && bit.length > 3) {
              const b = document.createElement("strong");
              b.textContent = bit.slice(2, -2);
              p.appendChild(b);
            } else if (bit) p.appendChild(document.createTextNode(bit));
          }
          el.appendChild(p);
        }
      }
    });
  }, [text]);
  return <div ref={ref} style={{ color: ink, fontSize: 13 }} />;
}

function renderValueContent(v: WValue, renderer: RendererKind, portName: string, onDrillIn?: (v: WValue, name: string) => void): ReactNode {
  const pre = (s: string) => (
    <pre style={{ color: "#cfe3ff", whiteSpace: "pre-wrap", wordBreak: "break-word", margin: 0, fontSize: 12, fontFamily: "var(--mono)", lineHeight: 1.5 }}>{s}</pre>
  );
  if (v.type === "Text") {
    const fm = /^‹file:([^,›]+),\s*([^›,]+),\s*([^›]*)›$/.exec(v.value);
    if (fm) {
      const path = fm[1].trim();
      const sizeStr = fm[2].trim();
      const kept = fm[3].trim();
      const name = path.split("/").pop() ?? path;
      const folder = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : null;
      const ext = (name.split(".").pop() ?? "").toLowerCase();
      const extKind: Record<string, string> = {
        html: "HTML", htm: "HTML", css: "CSS", js: "JavaScript", ts: "TypeScript",
        json: "JSON", csv: "CSV", txt: "Text", md: "Markdown",
        png: "Image", jpg: "Image", jpeg: "Image", gif: "Image", svg: "Image", webp: "Image",
        mp3: "Audio", wav: "Audio", ogg: "Audio",
        pdf: "PDF",
      };
      const kind = extKind[ext] ?? "File";
      const noteText = kept === "kept by hash"
        ? "Content not stored in the run log — this folder keeps a hash only. To view contents here, set the folder’s keep to “bytes”."
        : kept;
      return (
        <div style={{ padding: "24px 16px", textAlign: "center" }}>
          <div style={{ fontSize: 11, letterSpacing: "0.08em", textTransform: "uppercase", color: dim, marginBottom: 8 }}>{kind} file</div>
          <div style={{ color: ink, fontWeight: 600, fontSize: 15, marginBottom: 6, wordBreak: "break-all" }}>{name}</div>
          {folder && (
            <div style={{ fontSize: 11, color: dim, marginBottom: 4 }}>
              folder: <span style={{ color: "#7ab0e0" }}>{folder}</span>
            </div>
          )}
          <div style={{ fontSize: 11, color: dim, marginBottom: 16 }}>{sizeStr}</div>
          <div style={{ fontSize: 11, color: dim, background: "#0f1c30", border: "1px solid #1b2a45", borderRadius: 4, padding: "8px 12px", display: "inline-block", maxWidth: 340, lineHeight: 1.6 }}>
            {noteText}
          </div>
        </div>
      );
    }
  }
  if (renderer === "image" && v.type === "Text")
    return <img src={v.value} alt={portName} style={{ maxWidth: "100%", maxHeight: 400, objectFit: "contain", display: "block", margin: "0 auto" }} />;
  if (renderer === "audio" && v.type === "Text")
    return <audio controls src={v.value} style={{ width: "100%" }} />;
  if (renderer === "html" && v.type === "Text")
    return <iframe srcDoc={v.value} sandbox="allow-scripts" title="html preview" style={{ width: "100%", height: 420, border: "1px solid #1b2a45", borderRadius: 3 }} />;
  if (renderer === "json" && v.type === "Text") {
    try { return pre(JSON.stringify(JSON.parse(v.value), null, 2)); } catch {}
  }
  if (renderer === "markdown" && v.type === "Text") return <MarkdownView text={v.value} />;
  if (renderer === "text" && v.type === "Text") return pre(v.value);
  if (renderer === "table" && v.type === "List") {
    const rows = v.value.filter((r) => r.type === "Record");
    if (rows.length > 0) {
      const keys: string[] = [];
      const seen = new Set<string>();
      for (const r of rows) {
        if (r.type !== "Record") continue;
        for (const [k] of r.value) if (!seen.has(k)) { seen.add(k); keys.push(k); }
      }
      return (
        <div style={{ overflowX: "auto" }}>
          <table style={{ borderCollapse: "collapse", fontSize: 12, color: ink, width: "100%" }}>
            <thead>
              <tr>{keys.map((k) => <th key={k} style={{ padding: "4px 8px", textAlign: "left", borderBottom: "1px solid #1b2a45", color: dim, fontWeight: 500 }}>{k}</th>)}</tr>
            </thead>
            <tbody>
              {rows.map((r, i) => {
                if (r.type !== "Record") return null;
                const map = new Map(r.value);
                return (
                  <tr key={i}>
                    {keys.map((k) => {
                      const cell = map.get(k);
                      return <td key={k} style={{ padding: "3px 8px", verticalAlign: "top", borderBottom: "1px solid #1b2a4533" }}>{cell ? formatValue(cell) : "—"}</td>;
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      );
    }
  }
  if (renderer === "record" && v.type === "Record") {
    return (
      <table style={{ borderCollapse: "collapse", fontSize: 12, color: ink, width: "100%" }}>
        <tbody>
          {v.value.map(([k, val]) => (
            <tr key={k}>
              <td style={{ padding: "3px 8px", color: dim, whiteSpace: "nowrap", verticalAlign: "top", borderBottom: "1px solid #1b2a4533" }}>{k}</td>
              <td style={{ padding: "3px 8px", wordBreak: "break-word", whiteSpace: "pre-wrap", borderBottom: "1px solid #1b2a4533" }}>
                <span style={{ display: "flex", alignItems: "flex-start", gap: 6 }}>
                  <span style={{ flex: 1 }}>{formatValue(val)}</span>
                  {onDrillIn && (
                    <button onClick={() => onDrillIn(val, k)} style={{ ...small, padding: "0px 4px", fontSize: 9, flexShrink: 0 }} title="view value">⤢</button>
                  )}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    );
  }
  return pre(formatValue(v));
}

function ValueModal({ value, portName, onClose }: { value: WValue; portName: string; onClose: () => void }) {
  const [stack, setStack] = useState<Array<{ value: WValue; name: string }>>([]);
  const current = stack.length > 0 ? stack[stack.length - 1] : { value, name: portName };
  const [renderer, setRenderer] = useState<RendererKind>(() => detectRenderer(value));
  const available = useMemo(() => availableRenderers(current.value), [current.value]);

  useEffect(() => {
    const fn = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", fn);
    return () => window.removeEventListener("keydown", fn);
  }, [onClose]);

  const push = useCallback((v: WValue, name: string) => {
    setStack((s) => [...s, { value: v, name }]);
    setRenderer(detectRenderer(v));
  }, []);

  const pop = () => {
    const next = stack.slice(0, -1);
    setStack(next);
    const prev = next.length > 0 ? next[next.length - 1] : { value, name: portName };
    setRenderer(detectRenderer(prev.value));
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
      style={{ position: "fixed", inset: 0, background: "rgba(5, 10, 20, 0.7)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 1000 }}
    >
      <div style={{ background: "#0e1a2f", border: "1px solid #1b2a45", borderRadius: 6, maxWidth: 820, width: "calc(100vw - 32px)", maxHeight: "calc(100vh - 64px)", display: "flex", flexDirection: "column", color: ink, fontSize: 13 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 12px", borderBottom: "1px solid #1b2a45", flexShrink: 0 }}>
          {stack.length > 0 && (
            <button onClick={pop} style={{ background: "transparent", border: "none", color: dim, cursor: "pointer", fontSize: 14, padding: "0 2px", lineHeight: 1 }} title="back">←</button>
          )}
          <span style={{ color: dim, fontSize: 11 }}>{current.name}</span>
          <span style={{ flex: 1 }} />
          <select
            value={renderer}
            onChange={(e) => setRenderer(e.target.value as RendererKind)}
            style={{ font: "inherit", fontSize: 11, color: dim, background: "#0b1527", border: "1px solid #1b2a45", borderRadius: 3, padding: "2px 6px" }}
          >
            {available.map((r) => <option key={r} value={r}>{RENDERER_LABELS[r]}</option>)}
          </select>
          <button onClick={onClose} style={{ background: "transparent", border: "none", color: dim, cursor: "pointer", fontSize: 16, padding: "0 4px", lineHeight: 1 }} title="close">×</button>
        </div>
        <div style={{ overflow: "auto", padding: 12, flex: 1, minHeight: 0 }}>
          {renderValueContent(current.value, renderer, current.name, push)}
        </div>
      </div>
    </div>
  );
}

function Port({ port, value, ran }: { port: WPort; value: WValue | null | undefined; ran: boolean }) {
  const [modalOpen, setModalOpen] = useState(false);
  const canExpand = ran && value != null;
  return (
    <div style={{ display: "flex", flexDirection: "column", border: "1px solid #1b2a45", borderRadius: 3, padding: "3px 6px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
        <span style={{ color: ink }}>{port.name}</span>
        <span style={{ color: dim }}>{typeLabel(port.ty)}</span>
        <span style={{ flex: 1 }} />
        {canExpand && (
          <button onClick={() => setModalOpen(true)} style={{ ...small, padding: "0px 4px", fontSize: 10, lineHeight: 1.4 }} title="view value">⤢</button>
        )}
      </div>
      {ran && (
        <div style={{ color: value === null || value === undefined ? dim : "#cfe3ff", wordBreak: "break-word", whiteSpace: "pre-wrap" }}>
          {show(value)}
        </div>
      )}
      {modalOpen && value != null && <ValueModal value={value} portName={port.name} onClose={() => setModalOpen(false)} />}
    </div>
  );
}
