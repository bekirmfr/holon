// The flow's deploy controls, in the canvas header next to save: one Deploy (or Update) for the
// whole flow, its live status with pause / stop / run, and the flow's deploy settings (port,
// clock interval, and what it needs beyond its graph: limits and exceptions, which become part
// of its manifest). The deployment is named after the flow, so there is no separate slug.

import { useEffect, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { describeItem, manifestTrigger } from "./holon";
import {
  deleteSecret,
  getEnvSecretNames,
  getPermissions,
  getSecretHistory,
  listSecrets,
  rollbackSecret,
  setSecret,
  type DeploymentEntry,
  type HostPolicy,
  type Requirements,
  type SecretHistory,
  type SecretInfo,
} from "./server";

/** A flow's deploy settings, saved with it. `port` is for a flow with routes (a server),
 *  `interval` (seconds) for a flow on a clock. Either may be unused. `requirements`:
 *  limits away from the host's defaults, and exceptions, which the host must grant. */
export type FlowSettings = {
  port: string;
  interval: string;
  requirements?: Requirements;
  /** The web pages from other sites a flow with routes answers (CORS): `*`, or origins separated
   *  by spaces or commas; blank for none (a browser then refuses them). */
  browsers?: string;
};

/** A flow's `browsers` setting as the list the server takes. */
export const browsersOf = (settings: { browsers?: string }): string[] => (settings.browsers ?? "").split(/[\s,]+/).filter(Boolean);

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
  ["inline_file_bytes", "file read whole (bytes)"],
  ["max_file_read_bytes", "file reads per run (bytes)"],
  ["max_file_write_bytes", "file writes per run (bytes)"],
  ["max_files_per_run", "files per run"],
  ["max_pack_bytes", "pack size (bytes)"],
  ["max_packs_per_run", "packs per run"],
  ["channel_keep", "packs a channel keeps"],
  ["channel_buffer", "packs a subscriber may fall behind"],
  ["max_subscribers", "subscribers per channel"],
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
  capabilities,
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
  /** What the flow asks for, by consent name: each but its trigger can be marked optional. */
  capabilities: string[];
  /** The flow's name: its deployment's slug, and whose secrets these are. */
  slug: string | null;
  /** The secret names the canvas reads. */
  secretNames: string[];
}) {
  const [open, setOpen] = useState(false);
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);
  useEffect(() => {
    if (open && !policy) getPermissions().then(setPolicy).catch(() => {});
  }, [open, policy]);
  // The secrets set: versions and sources, never values (the server never sends one back).
  const [secrets, setSecrets] = useState<SecretInfo[]>([]);
  const reloadSecrets = () => {
    if (slug) listSecrets(slug).then(setSecrets).catch(() => setSecrets([]));
  };
  useEffect(reloadSecrets, [open, slug]);
  // The env variables a secret can name, for an admin (the server re-reads its dotenv file, so a
  // line just added shows); anyone else gets none and types the name.
  const [envNames, setEnvNames] = useState<string[]>([]);
  useEffect(() => {
    if (open) getEnvSecretNames().then((r) => setEnvNames(r.names)).catch(() => setEnvNames([]));
  }, [open]);
  const req = settings.requirements;
  const noPlainHttp = policy !== null && !policy.allow_plain_http;
  const hasPlainHttp = (req?.plain_http ?? []).length > 0;
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
          {entry.listen_error && (
            <span style={{ color: "#e0916f", fontSize: 11 }} title={entry.listen_error}>
              ⚠ port in use
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
      {open &&
        // A modal on the page itself: inside the canvas header it would be clipped by the canvas.
        createPortal(
          <div
            role="dialog"
            aria-modal="true"
            aria-label="deploy settings"
            onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}
            style={{
              position: "fixed",
              inset: 0,
              background: "rgba(5, 10, 20, 0.6)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              zIndex: 1000,
            }}
          >
            <div
              style={{
                maxHeight: "calc(100vh - 32px)",
                maxWidth: "calc(100vw - 32px)",
                overflow: "auto",
                background: "#0b1527",
                border: "1px solid #1b2a45",
                borderRadius: 6,
                padding: 14,
                display: "grid",
                gridTemplateColumns: "auto auto",
                gap: 6,
                alignItems: "center",
                fontSize: 11,
                color: "#9fb4d4",
                whiteSpace: "nowrap",
              }}
            >
              <span style={{ gridColumn: "1 / -1", display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
                <span style={{ color: "#e6eefb", fontSize: 13, flex: 1 }}>deploy settings{slug ? ` · ${slug}` : ""}</span>
                <button style={primary} onClick={() => setOpen(false)} title="close (Esc); settings are kept as you type">
                  done
                </button>
              </span>
              <span title="the port a flow with routes (request nodes) answers on">port</span>
              <input value={settings.port} onChange={(e) => setSettings({ ...settings, port: e.target.value })} style={input} />
              <span title="web pages on other sites that may call its routes (CORS): * for any, or origins like https://example.com">web pages from</span>
              <input
                value={settings.browsers ?? ""}
                placeholder="none (* or https://site)"
                onChange={(e) => setSettings({ ...settings, browsers: e.target.value })}
                style={input}
              />
              <span title="how often a flow without routes runs on the clock, in seconds">interval (s)</span>
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
                autoComplete="off"
                // A host this policy refuses stays here to be cleared, not greyed out with it.
                disabled={noPlainHttp && !hasPlainHttp}
                title={noPlainHttp ? "this host never grants plain HTTP" + (hasPlainHttp ? ": clear this to deploy" : "") : "comma-separated host[:port]"}
                onChange={(e) =>
                  setSettings({
                    ...settings,
                    requirements: { ...req, plain_http: e.target.value.split(",").map((h) => h.trim()).filter((h) => h !== "") },
                  })
                }
                style={{ ...input, width: 140, ...(noPlainHttp && hasPlainHttp ? { borderColor: "#e0916f" } : {}) }}
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
              <OptionalPermissions capabilities={capabilities} settings={settings} setSettings={setSettings} />
              {secretNames.length > 0 && (
                <span style={{ gridColumn: "1 / -1", color: "#e6eefb", marginTop: 4 }} title="kept by the server, never shown again">
                  secrets{slug ? "" : " (name the flow first)"}
                </span>
              )}
              {secretNames.map((name) => (
                <SecretRow key={name} slug={slug} name={name} info={secrets.find((x) => x.name === name)} envNames={envNames} onChanged={reloadSecrets} />
              ))}
            </div>
          </div>,
          document.body,
        )}
    </span>
  );
}

/** Where a secret's value comes from, as the row picks it. */
type Kind = "value" | "env" | "vault";
const KINDS: { kind: Kind; label: string; placeholder: (name: string, isSet: boolean) => string; title: string }[] = [
  { kind: "value", label: "value", placeholder: (_, isSet) => (isSet ? "replace…" : "value"), title: "sealed by the server under its master key" },
  {
    kind: "env",
    label: "env",
    placeholder: (name) => `HOLON_SECRET_${name.toUpperCase().replace(/[^A-Z0-9]/g, "_")}`,
    title: "an environment variable of the server's, or its --dotenv file; only HOLON_SECRET_… names",
  },
  { kind: "vault", label: "vault", placeholder: () => "secret/path#field", title: "a HashiCorp Vault KV v2 field, read with the server's VAULT_ADDR / VAULT_TOKEN" },
];

/** The env row's choice that types a name the server didn't list. */
const OTHER = "\u0000other";

const when = (epoch: number) => new Date(epoch * 1000).toLocaleString([], { dateStyle: "short", timeStyle: "short" });
const sourceLabel = (source: string) => (source === "local" ? "sealed here" : source);

/**
 * One secret in the deploy settings: its current version and where it comes from (or why it
 * has no value), a way to set a new version (a value, an env variable, a Vault field), and its
 * history, from which an older version can be rolled back to.
 */
function SecretRow({ slug, name, info, envNames, onChanged }: { slug: string | null; name: string; info?: SecretInfo; envNames: string[]; onChanged: () => void }) {
  const kindOf = (source?: string): Kind => (source?.startsWith("env:") ? "env" : source?.startsWith("vault:") ? "vault" : "value");
  const [kind, setKind] = useState<Kind>(kindOf(info?.source));
  const [text, setText] = useState("");
  const [msg, setMsg] = useState("");
  // An env variable is picked from the server's names when it has some; "another name…" types one.
  const [typing, setTyping] = useState(false);
  const picking = kind === "env" && envNames.length > 0 && !typing;
  const [history, setHistory] = useState<SecretHistory | null>(null);
  // The row follows where the secret comes from once that's known (the list loads after it).
  useEffect(() => {
    if (info) setKind(kindOf(info.source));
  }, [info?.source]);
  const fail = (e: unknown) => setMsg(e instanceof Error ? e.message : String(e));
  const loadHistory = () => slug && getSecretHistory(slug, name).then(setHistory).catch(fail);
  const changed = (what: string) => {
    setMsg(what);
    onChanged();
    if (history) loadHistory();
  };
  const save = () => {
    const typed = text.trim();
    if (!slug || !typed) return;
    const set = kind === "value" ? { value: text } : { from: `${kind}:${typed}` };
    setSecret(slug, name, set)
      .then((v) => {
        setText("");
        changed(`version ${v} set`);
      })
      .catch(fail);
  };
  const remove = () =>
    slug &&
    deleteSecret(slug, name)
      .then(() => changed("removed"))
      .catch(fail);
  const rollback = (to: number) =>
    slug &&
    rollbackSecret(slug, name, to)
      .then((v) => changed(`version ${to} is current again, as version ${v}`))
      .catch(fail);
  const k = KINDS.find((x) => x.kind === kind)!;
  const status = info ? (info.problem ? "#e0916f" : "#9ad29f") : "#e0916f";
  return (
    <>
      <span style={{ display: "flex", flexDirection: "column", gap: 1 }}>
        <span>{name}</span>
        <span style={{ color: status, fontSize: 10 }} title={info?.problem ?? (info ? `set ${when(info.set_at)}` : undefined)}>
          {info ? `v${info.version} · ${sourceLabel(info.source)}${info.problem ? " · can't be read" : ""}` : "not set"}
        </span>
      </span>
      <span style={{ display: "inline-flex", gap: 4 }}>
        <select
          value={kind}
          onChange={(e) => {
            const next = e.target.value as Kind;
            setKind(next);
            // An env variable's name is suggested from the secret's; a value is never carried over.
            const suggested = KINDS[1].placeholder(name, false);
            setText(next !== "env" ? "" : envNames.length === 0 || envNames.includes(suggested) ? suggested : "");
            setTyping(false);
          }}
          disabled={!slug}
          title={k.title}
          style={{ ...base, padding: "3px 4px" }}
        >
          {KINDS.map((x) => (
            <option key={x.kind} value={x.kind}>
              {x.label}
            </option>
          ))}
        </select>
        {picking ? (
          <select
            value={text}
            disabled={!slug}
            onChange={(e) => (e.target.value === OTHER ? (setTyping(true), setText("")) : setText(e.target.value))}
            title="a HOLON_SECRET_… variable of the server's environment or its --dotenv file"
            style={{ ...base, padding: "3px 4px", width: 158 }}
          >
            <option value="" disabled>
              choose a variable…
            </option>
            {envNames.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
            <option value={OTHER}>another name…</option>
          </select>
        ) : (
        <input
          type={kind === "value" ? "password" : "text"}
          // Not a sign-in: a browser's saved login would fill this, and its name the field before.
          autoComplete={kind === "value" ? "new-password" : "off"}
          data-1p-ignore
          data-lpignore="true"
          value={text}
          placeholder={k.placeholder(name, !!info)}
          disabled={!slug}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && save()}
          title={k.title}
          style={{ ...input, width: 150 }}
        />
        )}
        <button style={base} disabled={!slug || !text.trim()} onClick={save}>
          set
        </button>
        {info && (
          <button style={base} onClick={() => (history ? setHistory(null) : loadHistory())} title="versions and changes; roll back to an older version">
            {history ? "▾" : "▸"}
          </button>
        )}
        {info && (
          <button style={base} onClick={remove} title="remove this secret, every version">
            ×
          </button>
        )}
      </span>
      {msg && <span style={{ gridColumn: "1 / -1", color: "#9fb4d4", whiteSpace: "normal", maxWidth: 420 }}>{msg}</span>}
      {history && (
        <span style={{ gridColumn: "1 / -1", display: "grid", gap: 3, padding: "4px 8px", borderLeft: "2px solid #1b2a45", marginLeft: 4 }}>
          {history.versions.map((v) => (
            <span key={v.version} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ color: v.current ? "#9ad29f" : "#9fb4d4", minWidth: 24 }}>v{v.version}</span>
              <span style={{ flex: 1 }}>{sourceLabel(v.source)}</span>
              <span style={{ color: "#7e8ea8" }}>{when(v.set_at)}</span>
              {v.current ? (
                <span style={{ color: "#9ad29f", width: 58, textAlign: "right" }}>current</span>
              ) : (
                <button style={{ ...base, width: 58 }} onClick={() => rollback(v.version)} title={`make version ${v.version} current again`}>
                  roll back
                </button>
              )}
            </span>
          ))}
          {history.events.length > 0 && <span style={{ color: "#e6eefb", marginTop: 4 }}>changes</span>}
          {history.events
            .slice()
            .reverse()
            .map((e, i) => (
              <span key={i} style={{ color: "#7e8ea8", whiteSpace: "normal", maxWidth: 420 }}>
                {when(e.at)} · {e.action}
                {e.version ? ` v${e.version}` : ""}
                {e.source ? ` · ${sourceLabel(e.source)}` : ""}
                {e.detail ? ` · ${e.detail}` : ""}
                {e.by ? ` · by ${e.by}` : ""}
              </span>
            ))}
        </span>
      )}
    </>
  );
}

/**
 * Which of the flow's permissions it can run without (permissions.rs: `optional`). A required
 * one not granted pauses the flow; an optional one leaves it running with those calls refused,
 * so the flow should cope with the node staying empty. Its trigger is always required. One
 * marked optional that the flow no longer uses is shown so it can be cleared (deploy refuses it).
 */
function OptionalPermissions({
  capabilities,
  settings,
  setSettings,
}: {
  capabilities: string[];
  settings: FlowSettings;
  setSettings: (s: FlowSettings) => void;
}) {
  const optional = settings.requirements?.optional ?? [];
  const trigger = manifestTrigger(capabilities);
  const listed = [...new Set([...capabilities.filter((c) => c !== trigger), ...optional])].sort();
  if (listed.length === 0) return null;
  const toggle = (item: string, on: boolean) => {
    const next = on ? [...optional, item] : optional.filter((x) => x !== item);
    setSettings({ ...settings, requirements: { ...settings.requirements, optional: next.length > 0 ? next.sort() : undefined } });
  };
  return (
    <>
      <span
        style={{ gridColumn: "1 / -1", color: "#e6eefb", marginTop: 4 }}
        title="a required permission not granted pauses the flow; an optional one leaves it running, those calls refused"
      >
        permissions: optional ones it can run without
      </span>
      {listed.map((item) => {
        const unused = !capabilities.includes(item);
        return (
          <label key={item} style={{ display: "contents" }} title={item}>
            <span style={{ color: unused ? "#e0916f" : undefined }}>
              {describeItem(item)}
              {unused ? " (no longer used)" : ""}
            </span>
            <span style={{ display: "inline-flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={optional.includes(item)} onChange={(e) => toggle(item, e.target.checked)} />
              <span style={{ color: optional.includes(item) ? "#7e8ea8" : "#e0c36f" }}>{optional.includes(item) ? "optional" : "required"}</span>
            </span>
          </label>
        );
      })}
    </>
  );
}
