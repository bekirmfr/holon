// The flow's deploy controls, in the canvas header next to save: one Deploy (or Update) for the
// whole flow, its live status with pause / stop / run, and the flow's deploy settings (port,
// clock interval, and what it needs beyond its graph: limits and exceptions, which become part
// of its manifest). The deployment is named after the flow, so there is no separate slug.

import { useEffect, useState, type CSSProperties } from "react";
import { deleteSecret, getPermissions, listSecrets, setSecret, type DeploymentEntry, type HostPolicy, type Requirements } from "./server";

/** A flow's deploy settings, saved with it. `port` is for a flow with routes (a server),
 *  `interval` (seconds) for a flow that reads the clock. Either may be unused. `requirements`:
 *  limits away from the host's defaults, and exceptions, which the host must grant. */
export type FlowSettings = { port: string; interval: string; requirements?: Requirements };

/** The limits a flow can ask for, with what a person reads for each (permissions.rs's names). */
const LIMITS: [string, string][] = [
  ["request_timeout_ms", "request timeout (ms)"],
  ["connect_timeout_ms", "connect timeout (ms)"],
  ["run_budget_ms", "run budget, request (ms)"],
  ["clock_run_budget_ms", "run budget, clock (ms)"],
  ["max_requests", "requests per run"],
  ["per_host_concurrency", "concurrent per host"],
  ["max_request_bytes", "request size (bytes)"],
  ["max_response_bytes", "response size (bytes)"],
  ["max_header_bytes", "header size (bytes)"],
  ["max_redirects", "redirects"],
  ["max_await_seconds", "longest wait (s)"],
  ["max_waiting_runs", "runs waiting at once"],
];

/** `requirements` with `limit` set to `text` (blank: back to the host default). */
export function withLimit(requirements: Requirements | undefined, limit: string, text: string): Requirements {
  const limits = { ...(requirements?.limits ?? {}) };
  const n = Number(text);
  if (text.trim() === "" || !Number.isInteger(n) || n < 0) delete limits[limit];
  else limits[limit] = n;
  return { ...requirements, limits };
}
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
  slug,
  secretNames,
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
  /** The flow's name: its deployment's slug, and whose secrets these are. */
  slug: string | null;
  /** The secret names the canvas reads. */
  secretNames: string[];
}) {
  const [open, setOpen] = useState(false);
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  useEffect(() => {
    if (open && !policy) getPermissions().then(setPolicy).catch(() => {});
  }, [open, policy]);
  // Which secrets are set (names only: the server never sends a value back).
  const [setNames, setSetNames] = useState<string[]>([]);
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [secretMsg, setSecretMsg] = useState("");
  useEffect(() => {
    if (open && slug) listSecrets(slug).then(setSetNames).catch(() => setSetNames([]));
  }, [open, slug]);
  const saveSecret = (name: string) => {
    if (!slug) return;
    setSecret(slug, name, typed[name] ?? "")
      .then(() => {
        setTyped({ ...typed, [name]: "" });
        setSecretMsg(`${name} set`);
        return listSecrets(slug).then(setSetNames);
      })
      .catch((e: unknown) => setSecretMsg(e instanceof Error ? e.message : String(e)));
  };
  const removeSecret = (name: string) => {
    if (!slug) return;
    deleteSecret(slug, name)
      .then(() => listSecrets(slug).then(setSetNames))
      .catch((e: unknown) => setSecretMsg(e instanceof Error ? e.message : String(e)));
  };
  const req = settings.requirements;
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
          {entry.attention && (
            <span style={{ color: "#e0916f", fontSize: 11 }} title={entry.attention}>
              ⚠ needs approval
            </span>
          )}
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
      <button style={base} onClick={() => setOpen(!open)} title="deploy settings: port, clock interval, what it needs">
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
          <span style={{ gridColumn: "1 / -1", color: "#e6eefb", marginTop: 4 }} title="what it needs beyond its graph; the host must grant it">
            needs (blank: this host's default)
          </span>
          {LIMITS.map(([name, label]) => (
            <label key={name} style={{ display: "contents" }}>
              <span>{label}</span>
              <input
                value={req?.limits?.[name] ?? ""}
                placeholder={policy ? String(policy.defaults[name]) : ""}
                title={policy ? `default ${policy.defaults[name]}, ceiling ${policy.ceilings[name]}` : undefined}
                onChange={(e) => setSettings({ ...settings, requirements: withLimit(req, name, e.target.value) })}
                style={{ ...input, width: 84 }}
              />
            </label>
          ))}
          <span title="hosts it may call over plain HTTP; this host must allow plain HTTP at all">plain HTTP to</span>
          <input
            value={(req?.plain_http ?? []).join(", ")}
            placeholder="none"
            disabled={policy !== null && !policy.allow_plain_http}
            title={policy && !policy.allow_plain_http ? "this host never grants plain HTTP" : "comma-separated host[:port]"}
            onChange={(e) =>
              setSettings({
                ...settings,
                requirements: { ...req, plain_http: e.target.value.split(",").map((h) => h.trim()).filter((h) => h !== "") },
              })
            }
            style={{ ...input, width: 140 }}
          />
          <span title="this machine and its network: loopback, private and link-local addresses">private addresses</span>
          <input
            type="checkbox"
            checked={req?.private_addresses ?? false}
            disabled={policy !== null && !policy.allow_private_addresses}
            title={policy && !policy.allow_private_addresses ? "this host never grants private addresses" : undefined}
            onChange={(e) => setSettings({ ...settings, requirements: { ...req, private_addresses: e.target.checked } })}
            style={{ justifySelf: "start" }}
          />
          {secretNames.length > 0 && (
            <span style={{ gridColumn: "1 / -1", color: "#e6eefb", marginTop: 4 }} title="kept by the server, never shown again">
              secrets{slug ? "" : " (name the flow first)"}
            </span>
          )}
          {secretNames.map((name) => (
            <label key={name} style={{ display: "contents" }}>
              <span>
                {name} <span style={{ color: setNames.includes(name) ? "#9ad29f" : "#e0916f" }}>{setNames.includes(name) ? "set" : "not set"}</span>
              </span>
              <span style={{ display: "inline-flex", gap: 4 }}>
                <input
                  type="password"
                  autoComplete="off"
                  value={typed[name] ?? ""}
                  placeholder={setNames.includes(name) ? "replace…" : "value"}
                  disabled={!slug}
                  onChange={(e) => setTyped({ ...typed, [name]: e.target.value })}
                  style={{ ...input, width: 100 }}
                />
                <button style={base} disabled={!slug || !(typed[name] ?? "")} onClick={() => saveSecret(name)}>
                  set
                </button>
                {setNames.includes(name) && (
                  <button style={base} onClick={() => removeSecret(name)} title="remove this secret">
                    ×
                  </button>
                )}
              </span>
            </label>
          ))}
          {secretMsg && <span style={{ gridColumn: "1 / -1", color: "#9fb4d4" }}>{secretMsg}</span>}
        </div>
      )}
    </span>
  );
}
