// View mode's panes around the canvas of a deployed flow. The bottom pane follows the flow log
// live (every request, trigger and tick) with two tabs: the log itself, and a timeline of the
// selected run, one row per node, timed by the server. The side pane shows the selected run's
// flow state as of the current step, and the selected node: its params, and the inputs and
// outputs it had in that run (replayed, replay.ts).

import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { getCollectionItems, getLog, type LogEntry } from "./server";
import { formatValue } from "./diagnostics";
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
        ? "clock tick"
        : "trigger";
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
  const [error, setError] = useState("");
  const last = useRef<number | undefined>(undefined);
  useEffect(() => {
    last.current = undefined;
    setEntries([]);
    let stopped = false;
    const poll = () =>
      getLog(slug, last.current)
        .then(({ entries: fresh }) => {
          if (stopped || fresh.length === 0) return;
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
  return { entries, error };
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
};
const scroll: CSSProperties = { flex: 1, minHeight: 0, overflow: "auto", padding: 8 };

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
  onPickNode,
  diagnosis,
  onShowNode,
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
  onPickNode: (id: string) => void;
  /** Why the selected run failed, when it did (replay.ts's diagnose). */
  diagnosis: Diagnosis | null;
  /** Bring a node into view and inspect it. */
  onShowNode: (id: string) => void;
}) {
  const { entries, error } = useFlowLog(slug);
  const [tab, setTab] = useState<"log" | "timeline">("log");
  const [live, setLive] = useState(true);

  // Live: always show the newest run.
  const newest = entries[entries.length - 1];
  useEffect(() => {
    if (live && newest && newest.seq !== selected?.seq) onSelect(newest);
  }, [live, newest, selected, onSelect]);

  const pick = (e: LogEntry) => {
    setLive(false);
    onSelect(e);
  };

  return (
    <div className="nodrag nowheel hl-view__bottom" style={paneStyle}>
      <Tabs
        tabs={[
          ["log", "Log"],
          ["timeline", "Timeline"],
        ]}
        tab={tab}
        onTab={setTab}
      >
        <div style={{ display: "flex", gap: 10, alignItems: "center", padding: "4px 0" }}>
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
          <Stepper step={step} steps={order.length} onStep={onStep} />
          <button
            style={{ ...small, color: live ? "#0e1a2f" : "#9fb4d4", background: live ? good : "transparent" }}
            onClick={() => setLive(!live)}
            title="follow the newest run as it happens"
          >
            ● live
          </button>
        </div>
      </Tabs>
      {error && <div style={{ color: bad, padding: "4px 8px" }}>{error}</div>}
      {diagnosis && <FailureBanner diagnosis={diagnosis} nodes={nodes} onShowNode={onShowNode} />}
      {tab === "log" ? (
        <LogTab entries={entries} selected={selected} pick={pick} />
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
          issues={diagnosis?.issues ?? new Map()}
        />
      )}
    </div>
  );
}

/** What went wrong in the selected run, and where it started, with a way there. */
function FailureBanner({ diagnosis, nodes, onShowNode }: { diagnosis: Diagnosis; nodes: ViewNode[]; onShowNode: (id: string) => void }) {
  const label = (id: string) => {
    const n = nodes.find((x) => x.id === id);
    return n ? nodeLabel(n) : id;
  };
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 2,
        margin: "2px 0 4px",
        padding: "4px 8px",
        borderLeft: `3px solid ${FAIL}`,
        background: `${FAIL}14`,
        color: ink,
      }}
    >
      <span style={{ color: FAIL }}>✗ {diagnosis.summary}</span>
      {diagnosis.origins.length === 0 && <span style={{ color: dim }}>no node on the canvas stopped the run</span>}
      {diagnosis.origins.map((id) => (
        <span key={id} style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
          <span>
            failed at <b>{label(id)}</b>: {diagnosis.issues.get(id)?.reason}
          </span>
          <button style={{ ...small, color: FAIL, borderColor: FAIL }} onClick={() => onShowNode(id)} title="show it on the canvas">
            show
          </button>
        </span>
      ))}
    </div>
  );
}

function LogTab({ entries, selected, pick }: { entries: LogEntry[]; selected: LogEntry | null; pick: (e: LogEntry) => void }) {
  return (
    <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
      <div style={{ ...scroll, flex: "1 1 55%", borderRight: "1px solid #1b2a45" }}>
        {entries.length === 0 && <div>no runs yet: send it a request</div>}
        {[...entries].reverse().map((e) => (
          <div
            key={e.seq}
            onClick={() => pick(e)}
            style={{
              display: "flex",
              gap: 10,
              padding: "2px 6px",
              cursor: "pointer",
              borderRadius: 3,
              background: e.seq === selected?.seq ? "#13243f" : "transparent",
            }}
          >
            <span style={{ color: dim, width: 30, textAlign: "right" }}>#{e.seq}</span>
            <span style={{ color: dim, width: 84, whiteSpace: "nowrap" }}>{time(e.at_ms)}</span>
            <span style={{ color: dim, width: 18 }}>s{e.session}</span>
            <span style={{ flex: 1, color: ink, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {runLabel(e)}
              {e.url && e.route ? <span style={{ color: dim }}> · {e.url}</span> : null}
            </span>
            {e.status !== null && <span style={{ color: e.status < 400 ? good : bad, width: 26 }}>{e.status}</span>}
            {e.error && <span style={{ color: bad }}>error</span>}
            {(e.parked ?? []).length > 0 && <span style={{ color: "#e0c36f" }} title="waiting for an event">⏸</span>}
            <span style={{ color: dim, width: 52, textAlign: "right" }}>{e.duration_ms.toFixed(2)}ms</span>
          </div>
        ))}
      </div>
      <div style={{ ...scroll, flex: "1 1 45%", display: "flex", flexDirection: "column", gap: 4 }}>
        {selected ? (
          <>
            {selected.url && (
              <div style={{ color: ink }}>
                {selected.method} {selected.url}
              </div>
            )}
            {selected.request && <Field name="request" value={formatValue(selected.request)} />}
            {selected.sourced.map(([cap, v]) => (
              <Field key={cap} name={cap.toLowerCase()} value={formatValue(v)} />
            ))}
            {selected.performed.map((p, i) => (
              <Field key={i} name={`did ${p.capability}`} value={formatValue(p.payload)} />
            ))}
            {selected.response_body !== null && (
              <Field name={`answered ${selected.status}`} value={selected.response_body || "(empty)"} />
            )}
            {selected.error && <Field name="error" value={selected.error} color={bad} />}
            {(selected.rounds ?? []).map((r, i) =>
              r.exchanges.map((x, j) => {
                const { line, ok } = exchangeLine(x.request, x.response);
                return (
                  <Field
                    key={`${i}-${j}`}
                    name={`round ${i + 1} · ${us(x.end_us - x.start_us)}`}
                    value={line}
                    color={ok ? ink : bad}
                  />
                );
              }),
            )}
            {(selected.reads ?? []).map((r, i) => (
              <Field key={`r${i}`} name="read" value={`${formatValue(r.request)} → ${r.answer ? formatValue(r.answer) : "nothing"}`} />
            ))}
            {(selected.retries ?? 0) > 0 && (
              <Field
                name="ran again"
                value={`${selected.retries}× — what it read changed before it could commit, so it ran again from the new state`}
                color="#e0c36f"
              />
            )}
            {(selected.parked ?? []).length > 0 && (
              <Field name="waiting at" value={(selected.parked ?? []).join(", ")} color="#e0c36f" />
            )}
            {Object.entries(selected.events ?? {}).map(([name, v]) => (
              <Field key={name} name={`event ${name}`} value={formatValue(v)} />
            ))}
            {selected.run_id && (
              <Field
                name="run"
                value={`${selected.run_id.slice(0, 16)}… · grant ${(selected.grant ?? "").slice(0, 12)}…`}
                color={dim}
              />
            )}
          </>
        ) : (
          <div>select a run</div>
        )}
      </div>
    </div>
  );
}

function Field({ name, value, color }: { name: string; value: string; color?: string }) {
  return (
    <div style={{ display: "flex", gap: 8 }}>
      <span style={{ color: dim, minWidth: 90 }}>{name}</span>
      <span style={{ color: color ?? ink, wordBreak: "break-word", whiteSpace: "pre-wrap", flex: 1 }}>{value}</span>
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

export function SidePane({
  slug,
  run,
  step,
  order,
  stateAfter,
  changed,
  collections,
  node,
  trace,
  timing,
}: {
  slug: string;
  run: LogEntry | null;
  step: number | null;
  order: string[];
  /** The state with the writes up to `step` applied, and the keys they changed. */
  stateAfter: Record<string, WValue>;
  changed: string[];
  /** The collections the run touched, as of `step` (replay.ts's collectionsAtStep). */
  collections: CollectionAtStep[];
  /** The node the component tab shows, with what it computed in this run. */
  node: ViewNode | null;
  trace: NodeTrace | undefined;
  timing: { start_us: number; end_us: number } | undefined;
}) {
  const [tab, setTab] = useState<"state" | "component">("state");
  return (
    <div className="nodrag nowheel hl-view__side" style={paneStyle}>
      <Tabs
        tabs={[
          ["state", "Flow state"],
          ["component", "Component"],
        ]}
        tab={tab}
        onTab={setTab}
      />
      <div style={{ ...scroll, display: "flex", flexDirection: "column", gap: 6 }}>
        {tab === "state" ? (
          <StateTab slug={slug} run={run} step={step} order={order} after={stateAfter} changed={changed} collections={collections} />
        ) : (
          <ComponentTab node={node} trace={trace} timing={timing} />
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
            <Field name="before" value={show(before[k])} />
            {moved && <Field name="after" value={show(after[k])} color="#e0c36f" />}
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

function ComponentTab({
  node,
  trace,
  timing,
}: {
  node: ViewNode | null;
  trace: NodeTrace | undefined;
  timing: { start_us: number; end_us: number } | undefined;
}) {
  const describe = useNodeDocs();
  if (!node) return <div>click a node on the canvas, or step through the run</div>;
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
        {!trace ? " · not part of this run" : ""}
      </div>
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

function Port({ port, value, ran }: { port: WPort; value: WValue | null | undefined; ran: boolean }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", border: "1px solid #1b2a45", borderRadius: 3, padding: "3px 6px" }}>
      <div>
        <span style={{ color: ink }}>{port.name}</span> <span style={{ color: dim }}>{typeLabel(port.ty)}</span>
      </div>
      {ran && (
        <div style={{ color: value === null || value === undefined ? dim : "#cfe3ff", wordBreak: "break-word", whiteSpace: "pre-wrap" }}>
          {show(value)}
        </div>
      )}
    </div>
  );
}
