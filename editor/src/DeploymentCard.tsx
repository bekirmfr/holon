// One deployment on the deployed page: its status, run / pause / stop, its run sessions (each with
// the state it ended with), and delete, which is only offered once it is stopped. Mirrors the
// server's lifecycle.rs.

import { useEffect, useState, type CSSProperties } from "react";
import { formatOutputs, summarizeState } from "./diagnostics";
import {
  getSessions,
  listWaitingRuns,
  type WaitingRun,
  type DeploymentEntry,
  type DeploymentStatus,
  type SessionEntry,
  type TriggerResult,
} from "./server";

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
const field: CSSProperties = { ...small, color: "#e6eefb", background: "#0e1a2f", cursor: "text" };

const STATUS_LOOK: Record<DeploymentStatus, { mark: string; color: string }> = {
  running: { mark: "●", color: "#9ad29f" },
  paused: { mark: "❚❚", color: "#f2b441" },
  stopped: { mark: "■", color: "#7e8ea8" },
};

const time = (secs: number) => new Date(secs * 1000).toLocaleTimeString();

export function DeploymentCard({
  entry,
  onAction,
  onDelete,
  onView,
  triggerInput,
  onTriggerInputChange,
  onTrigger,
  result,
}: {
  entry: DeploymentEntry;
  /** Run (optionally from a session or fresh), pause or stop. */
  onAction: (action: "run" | "pause" | "stop", from?: "latest" | "fresh" | number) => Promise<void>;
  onDelete: () => void;
  /** Open it in view mode: its deployed canvas, read only, with its live flow log. */
  onView: () => void;
  triggerInput: string;
  onTriggerInputChange: (v: string) => void;
  onTrigger: () => void;
  result?: { ok: true; result: TriggerResult } | { ok: false; message: string };
}) {
  const [open, setOpen] = useState(false);
  const [sessions, setSessions] = useState<SessionEntry[] | null>(null);
  const [from, setFrom] = useState<string>("latest");
  const [msg, setMsg] = useState("");

  // Reload the sessions whenever they're showing and the deployment moved (a new session, a
  // stop that saved a snapshot, more requests).
  useEffect(() => {
    if (!open) return;
    getSessions(entry.slug)
      .then((s) => setSessions(s.sessions))
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  }, [open, entry.slug, entry.status, entry.session, entry.trigger_count]);

  const act = (action: "run" | "pause" | "stop", start?: "latest" | "fresh" | number) => {
    setMsg("");
    onAction(action, start).catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  };
  const runFrom = () => act("run", from === "latest" || from === "fresh" ? from : Number(from));

  const look = STATUS_LOOK[entry.status];
  const stopped = entry.status === "stopped";
  const restorable = (sessions ?? []).filter((s) => s.state !== null);
  const canTrigger = entry.triggers.includes("Ingress") && entry.status === "running";

  return (
    <div style={{ borderTop: "1px solid #1b2a45", padding: "8px 0", fontSize: 11, display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span style={{ color: look.color }} title={entry.status}>
          {look.mark}
        </span>
        <span style={{ color: "#6fd3e0" }}>{entry.slug}</span>
        <span style={{ color: look.color }}>{entry.status}</span>
        <span style={{ color: "#9fb4d4", flex: 1 }}>{entry.triggers.join(", ")}</span>
        {entry.status === "running" && (
          <>
            <button style={small} onClick={() => act("pause")} title="stop answering, keep the port and the session">
              pause
            </button>
            <button style={small} onClick={() => act("stop")} title="end the session, save its state, release the port">
              stop
            </button>
          </>
        )}
        {entry.status === "paused" && (
          <>
            <button style={small} onClick={() => act("run")} title="continue the same session">
              resume
            </button>
            <button style={small} onClick={() => act("stop")} title="end the session, save its state, release the port">
              stop
            </button>
          </>
        )}
        {stopped && (
          <>
            <select
              value={from}
              onChange={(e) => setFrom(e.target.value)}
              style={{ ...field, cursor: "pointer" }}
              title="the state the new session starts from"
              onFocus={() => setOpen(true)}
            >
              <option value="latest">from latest</option>
              <option value="fresh">fresh</option>
              {restorable.map((s) => (
                <option key={s.number} value={String(s.number)}>
                  from session {s.number}
                </option>
              ))}
            </select>
            <button style={{ ...small, color: "#0e1a2f", background: "#6fd3e0", border: "none" }} onClick={runFrom}>
              run
            </button>
          </>
        )}
        <button style={small} onClick={onView} title="open it read only, with its live flow log and replay">
          view
        </button>
        <button
          style={{ ...small, opacity: stopped ? 1 : 0.35, cursor: stopped ? "pointer" : "default" }}
          disabled={!stopped}
          onClick={onDelete}
          title={stopped ? "delete this deployment and all its sessions" : "stop it before deleting"}
        >
          delete
        </button>
      </div>

      <div style={{ color: "#7e8ea8", fontSize: 10 }}>
        {entry.port !== null && <span>:{entry.port} · </span>}
        {entry.has_state && <span>state · </span>}
        session {entry.session} · {entry.trigger_count} fired
        {entry.error_count > 0 ? `, ${entry.error_count} errored` : ""} ·{" "}
        <span style={{ cursor: "pointer", color: "#9fb4d4" }} onClick={() => setOpen(!open)}>
          {entry.sessions} session{entry.sessions === 1 ? "" : "s"} {open ? "▾" : "▸"}
        </span>
      </div>
      {entry.attention && (
        <div style={{ color: "#e0916f", fontSize: 11 }}>⚠ {entry.attention}</div>
      )}
      {(entry.waiting ?? 0) > 0 && <WaitingRuns slug={entry.slug} count={entry.waiting ?? 0} />}

      {open &&
        (sessions ?? []).slice().reverse().map((s) => (
          <div key={s.number} style={{ display: "flex", gap: 8, alignItems: "center", paddingLeft: 14, color: "#9fb4d4" }}>
            <span style={{ color: "#e6eefb", width: 22 }}>#{s.number}</span>
            <span style={{ width: 150 }}>
              {time(s.started_at)} → {s.ended_at === null ? <span style={{ color: look.color }}>now</span> : time(s.ended_at)}
            </span>
            <span style={{ width: 80 }}>
              {s.requests} req · {s.errors} err
            </span>
            <span style={{ flex: 1, color: "#7e8ea8" }} title={`started from ${s.from}`}>
              {s.ended_at === null ? `from ${s.from}` : summarizeState(s.state)}
            </span>
            {stopped && s.state !== null && (
              <button style={small} onClick={() => act("run", s.number)} title="run a new session from this one's state">
                restore
              </button>
            )}
          </div>
        ))}

      {canTrigger && (
        <div style={{ display: "flex", gap: 4 }}>
          <input
            placeholder="request body"
            value={triggerInput}
            onChange={(e) => onTriggerInputChange(e.target.value)}
            style={{ ...field, width: 140 }}
          />
          <button style={{ ...small, color: "#0e1a2f", background: "#6fd3e0", border: "none" }} onClick={onTrigger}>
            trigger
          </button>
        </div>
      )}
      {result && (
        <div style={{ color: result.ok ? "#9ad29f" : "#e0916f", wordBreak: "break-word" }}>
          {result.ok ? formatOutputs(result.result.outputs) : result.message}
        </div>
      )}
      {msg && <div style={{ color: "#e0916f" }}>{msg}</div>}
    </div>
  );
}

/** A deployment's runs waiting for an event: what each waits for, and until when. Never their
 *  tokens (the server doesn't list them). */
function WaitingRuns({ slug, count }: { slug: string; count: number }) {
  const [open, setOpen] = useState(false);
  const [runs, setRuns] = useState<WaitingRun[] | null>(null);
  useEffect(() => {
    if (open) listWaitingRuns(slug).then(setRuns).catch(() => setRuns([]));
  }, [open, slug, count]);
  const when = (s: number) => new Date(s * 1000).toLocaleString();
  return (
    <div style={{ fontSize: 10 }}>
      <span style={{ cursor: "pointer", color: "#e0c36f" }} onClick={() => setOpen(!open)}>
        ⏸ {count} waiting {open ? "▾" : "▸"}
      </span>
      {open &&
        (runs ?? []).map((r) => (
          <div key={r.run_id} style={{ paddingLeft: 14, color: "#9fb4d4" }} title={`run ${r.run_id}`}>
            {r.route ?? r.kind} · since {when(r.parked_at)} ·{" "}
            {r.waiting.map((w) => `waits for "${w.name}" until ${when(w.expires_at)}`).join("; ")}
          </div>
        ))}
    </div>
  );
}
