// The flow's deploy controls, in the canvas header next to save: one Deploy (or Update) for the
// whole flow, its live status with pause / stop / run, and the flow's deploy settings (port,
// clock interval). The deployment is named after the flow, so there is no separate slug.

import { useState, type CSSProperties } from "react";
import type { DeploymentEntry } from "./server";

/** A flow's deploy settings, saved with it. `port` is for a flow with routes (a server),
 *  `interval` (seconds) for a flow that reads the clock. Either may be unused. */
export type FlowSettings = { port: string; interval: string };
export const DEFAULT_SETTINGS: FlowSettings = { port: "8080", interval: "" };

const base: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  padding: "3px 8px",
  color: "#9fb4d4",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  cursor: "pointer",
};
const primary: CSSProperties = { ...base, color: "#0e1a2f", background: "#6fd3e0", border: "none", padding: "3px 10px" };
const input: CSSProperties = { ...base, color: "#e6eefb", cursor: "text", width: 64 };

const LOOK = {
  running: { mark: "●", color: "#9ad29f" },
  paused: { mark: "❚❚", color: "#f2b441" },
  stopped: { mark: "■", color: "#7e8ea8" },
} as const;

export function DeployBar({
  entry,
  outOfDate,
  busy,
  settings,
  setSettings,
  onDeploy,
  onAction,
}: {
  /** This flow's deployment, if it has one. */
  entry: DeploymentEntry | undefined;
  /** The canvas no longer matches what is deployed (known for servers; null when unknown). */
  outOfDate: boolean | null;
  busy: boolean;
  settings: FlowSettings;
  setSettings: (s: FlowSettings) => void;
  onDeploy: () => void;
  onAction: (action: "run" | "pause" | "stop") => void;
}) {
  const [open, setOpen] = useState(false);
  const look = entry ? LOOK[entry.status] : null;
  const label = busy ? "deploying…" : entry ? "update" : "deploy";
  const title = entry
    ? "deploy the canvas as it is now; the deployment keeps its state and sessions"
    : "deploy this flow, named after it";

  return (
    <span style={{ display: "inline-flex", gap: 6, alignItems: "center", position: "relative" }}>
      <button
        onClick={onDeploy}
        disabled={busy}
        title={title}
        style={{ ...primary, opacity: busy ? 0.5 : 1, cursor: busy ? "default" : "pointer" }}
      >
        {label}
        {outOfDate && <span title="the canvas has changed since it was deployed"> •</span>}
      </button>
      {entry && look && (
        <>
          <span style={{ color: look.color, fontSize: 11 }} title={`session ${entry.session}`}>
            {look.mark} {entry.status}
            {entry.port !== null && entry.status !== "stopped" ? ` :${entry.port}` : ""}
          </span>
          {entry.status === "running" && (
            <button style={base} onClick={() => onAction("pause")} title="stop answering, keep the session">
              pause
            </button>
          )}
          {entry.status === "paused" && (
            <button style={base} onClick={() => onAction("run")} title="continue the same session">
              resume
            </button>
          )}
          {entry.status !== "stopped" ? (
            <button style={base} onClick={() => onAction("stop")} title="end the session, save its state">
              stop
            </button>
          ) : (
            <button style={base} onClick={() => onAction("run")} title="start a new session from the latest state">
              run
            </button>
          )}
        </>
      )}
      <button style={base} onClick={() => setOpen(!open)} title="deploy settings: port, clock interval">
        ⚙
      </button>
      {open && (
        <div
          className="nodrag"
          style={{
            position: "absolute",
            top: "calc(100% + 6px)",
            right: 0,
            zIndex: 10,
            background: "#0b1527",
            border: "1px solid #1b2a45",
            borderRadius: 4,
            padding: 8,
            display: "grid",
            gridTemplateColumns: "auto auto",
            gap: 6,
            alignItems: "center",
            fontSize: 11,
            color: "#9fb4d4",
            whiteSpace: "nowrap",
          }}
        >
          <span title="the port a flow with routes (request nodes) answers on">port</span>
          <input value={settings.port} onChange={(e) => setSettings({ ...settings, port: e.target.value })} style={input} />
          <span title="how often a flow that reads the clock fires, in seconds">interval (s)</span>
          <input
            value={settings.interval}
            placeholder="clock flows"
            onChange={(e) => setSettings({ ...settings, interval: e.target.value })}
            style={input}
          />
        </div>
      )}
    </span>
  );
}
