// A flow's deploy settings, in the editor's bottom pane (its Settings tab, web-console-L4.md §5.4):
// its port, and what it needs beyond its graph (limits and exceptions, which become part of its
// manifest), its permissions and its secrets, for the version in the tab. They are saved and
// deployed with it. The deployment is named after the flow, so there is no separate slug.

import { useEffect, useState, type CSSProperties } from "react";
import { alwaysTainted, describeItem, manifestTrigger, taintKeyProblem } from "./holon";
import { useDryRun, type DryRunReport } from "./DryRun";
import {
  deleteSecret,
  getEnvSecretNames,
  getPermissions,
  getSecretHistory,
  listSecrets,
  previewSecretRollback,
  rollbackSecret,
  setSecret,
  type HostPolicy,
  type Requirements,
  type SecretHistory,
  type SecretInfo,
  type SecretRollbackPreview,
} from "./server";

/** A flow's deploy settings, saved with it. `port` is for a flow with routes (unused without
 *  one). `requirements`: limits away from the host's defaults, and exceptions, which the host
 *  must grant. */
export type FlowSettings = {
  port: string;
  requirements?: Requirements;
  /** The web pages from other sites a flow with routes answers (CORS): `*`, or origins separated
   *  by spaces or commas; blank for none (a browser then refuses them). */
  browsers?: string;
  /** Its tags, sorted, as its text writes them (`tags: ["payments"]`): part of the unit it
   *  deploys as, so a binding on one covers it. */
  tags?: string[];
  /** The keys whose values it keeps out of its records (`taint: ["x-api-key"]`), as added, beside
   *  those every flow does: part of the unit it deploys as. */
  taint?: string[];
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
  ["max_call_depth", "flows calling flows, deep"],
  ["inline_file_bytes", "file read whole (bytes)"],
  ["max_file_read_bytes", "file reads per run (bytes)"],
  ["max_file_write_bytes", "file writes per run (bytes)"],
  ["max_files_per_run", "files per run"],
  ["max_served_bytes", "file a route answers with (bytes)"],
  ["max_pack_bytes", "pack size (bytes)"],
  ["max_packs_per_run", "packs per run"],
  ["channel_keep", "packs a channel keeps"],
  ["channel_buffer", "packs a subscriber may fall behind"],
  ["max_subscribers", "subscribers per channel"],
  ["max_websockets", "WebSocket connections open"],
  ["max_message_bytes", "WebSocket message size (bytes)"],
  ["messages_per_second", "WebSocket messages a second, each"],
  ["keep_runs", "runs the log keeps"],
  ["keep_run_days", "days the log keeps runs"],
];

/** `requirements` with `limit` set to `text` (blank: back to the host default). */
export function withLimit(requirements: Requirements | undefined, limit: string, text: string): Requirements {
  const limits = { ...(requirements?.limits ?? {}) };
  const n = Number(text);
  if (text.trim() === "" || !Number.isInteger(n) || n < 0) delete limits[limit];
  else limits[limit] = n;
  return { ...requirements, limits };
}
export const DEFAULT_SETTINGS: FlowSettings = { port: "8080" };

const base: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  padding: "3px 8px",
  color: "var(--muted)",
  background: "var(--surface-2)",
  border: "1px solid var(--border)",
  borderRadius: 3,
  cursor: "pointer",
};
const input: CSSProperties = { ...base, color: "var(--ink)", cursor: "text", width: 64 };

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
export const sourceLabel = (source: string) => (source === "local" ? "sealed here" : source);

/** A secret's roll back, as its dry run says it: which version comes back, as what, and what is
 *  kept; each value told apart by its fingerprint, as the log marks it, never shown. */
/** `slug` null: the server's own secret. */
export function secretRollbackReport(slug: string | null, name: string, p: SecretRollbackPreview): DryRunReport {
  const mark = (print?: string | null) => (print ? ` #${print}` : "");
  const same = !!p.to_print && p.to_print === p.current_print;
  return {
    title: `Roll back the secret ${name} of ${slug === null ? "the server" : `"${slug}"`} to version ${p.to}?`,
    changes: [`secret ${name}: v${p.current}${mark(p.current_print)} → v${p.version}${mark(p.to_print)}, version ${p.to}'s value${same ? " (the same value)" : ""}`],
    diffs: async () => [
      {
        title: `secret ${name}`,
        legend: "− current now   + current after",
        lines: [
          { sign: "-", text: `v${p.current}${mark(p.current_print)}  current now; kept, and can be rolled back to` },
          { sign: "+", text: `v${p.version}${mark(p.to_print)}  version ${p.to}'s value (${sourceLabel(p.source)})` },
        ],
        whole: true,
        note: `Values are told apart by their fingerprints, as the log marks them (#…), never shown${same ? ": these two are the same value" : ""}.${p.current_print === null ? ` Version ${p.current}'s value can't be read now.` : ""}`,
      },
    ],
    sections: [
      {
        heading: "secret",
        lines: [
          `Version ${p.to} (${sourceLabel(p.source)}) is current again, as version ${p.version}: the flow reads it from its next run.`,
          `Version ${p.current}, current now, is kept, and can be rolled back to.`,
          "Its value is never shown.",
        ],
      },
    ],
    action: "Roll back",
  };
}

/**
 * One secret in the deploy settings: its current version and where it comes from (or why it
 * has no value), a way to set a new version (a value, an env variable, a Vault field), and its
 * history, from which an older version can be rolled back to.
 */
export function SecretRow({ slug, name, info, envNames, onChanged, readOnly = false }: { slug: string | null; name: string; info?: SecretInfo; envNames: string[]; onChanged: () => void; readOnly?: boolean }) {
  const kindOf = (source?: string): Kind => (source?.startsWith("env:") ? "env" : source?.startsWith("vault:") ? "vault" : "value");
  const [kind, setKind] = useState<Kind>(kindOf(info?.source));
  const [text, setText] = useState("");
  const [msg, setMsg] = useState("");
  // An env variable is picked from the server's names when it has some; "another name…" types one.
  const [typing, setTyping] = useState(false);
  const picking = kind === "env" && envNames.length > 0 && !typing;
  const [history, setHistory] = useState<SecretHistory | null>(null);
  const dryRun = useDryRun();
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
  // A dry run first, its result shown; rolled back once approved.
  const rollback = async (to: number) => {
    if (!slug) return;
    try {
      if (!(await dryRun.ask(secretRollbackReport(slug, name, await previewSecretRollback(slug, name, to))))) return;
      const v = await rollbackSecret(slug, name, to);
      changed(`version ${to} is current again, as version ${v}`);
    } catch (e) {
      fail(e);
    }
  };
  const k = KINDS.find((x) => x.kind === kind)!;
  const status = info ? (info.problem ? "var(--bad)" : "var(--ok)") : "var(--bad)";
  // The server's, read for want of its own: setting one here overrides it.
  const theirs = info?.server ? info : undefined;
  const mayGo = (to?: string[]) => (to ? (to.length ? `may go only to ${to.map((d) => (d.startsWith("db:") ? `the database "${d.slice(3)}"` : `https://${d}`)).join(", ")}` : "may go nowhere") : "");
  return (
    <>
      <span style={{ display: "flex", flexDirection: "column", gap: 1 }}>
        <span>{name}</span>
        <span
          style={{ color: status, fontSize: 10 }}
          title={info?.problem ?? (theirs ? `the server's, set ${when(theirs.set_at)}${theirs.to ? `; it ${mayGo(theirs.to)}` : ""}; set one here to use this deployment's own` : info ? `set ${when(info.set_at)}` : undefined)}
        >
          {theirs ? `the server's${theirs.to ? ` · ${mayGo(theirs.to)}` : ""}${theirs.problem ? " · can't be read" : ""}` : info ? `v${info.version} · ${sourceLabel(info.source)}${info.problem ? " · can't be read" : ""}` : "not set"}
        </span>
      </span>
      {!readOnly && <span style={{ display: "inline-flex", gap: 4 }}>
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
        {info && !theirs && (
          <button style={base} onClick={() => (history ? setHistory(null) : loadHistory())} title="versions and changes; roll back to an older version">
            {history ? "▾" : "▸"}
          </button>
        )}
        {info && !theirs && (
          <button style={base} onClick={remove} title="remove this secret, every version">
            ×
          </button>
        )}
      </span>}
      {dryRun.modal}
      {msg && <span style={{ gridColumn: "1 / -1", color: "var(--muted)", whiteSpace: "normal", maxWidth: 420 }}>{msg}</span>}
      {history && (
        <span style={{ gridColumn: "1 / -1", display: "grid", gap: 3, padding: "4px 8px", borderLeft: "2px solid var(--border)", marginLeft: 4 }}>
          {history.versions.map((v) => (
            <span key={v.version} style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <span style={{ color: v.current ? "var(--ok)" : "var(--muted)", minWidth: 24 }}>v{v.version}</span>
              <span style={{ flex: 1 }}>{sourceLabel(v.source)}</span>
              <span style={{ color: "var(--faint)" }}>{when(v.set_at)}</span>
              {v.current ? (
                <span style={{ color: "var(--ok)", width: 58, textAlign: "right" }}>current</span>
              ) : (
                <button style={{ ...base, width: 58 }} onClick={() => rollback(v.version)} title={`make version ${v.version} current again`}>
                  roll back
                </button>
              )}
            </span>
          ))}
          {history.events.length > 0 && <span style={{ color: "var(--ink)", marginTop: 4 }}>changes</span>}
          {history.events
            .slice()
            .reverse()
            .map((e, i) => (
              <span key={i} style={{ color: "var(--faint)", whiteSpace: "normal", maxWidth: 420 }}>
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
  clocked,
  fileFolders,
  readOnly = false,
}: {
  capabilities: string[];
  settings: FlowSettings;
  setSettings: (s: FlowSettings) => void;
  /** It has clocks (`every` nodes): the clock starts them, so it is never optional. */
  clocked: boolean;
  /** The folders its file handlers watch: a file there starts them, so that is never optional. */
  fileFolders: string[];
  readOnly?: boolean;
}) {
  const optional = settings.requirements?.optional ?? [];
  const trigger = manifestTrigger(capabilities);
  // An action is what starts its own runs: never optional either.
  const starts = (c: string) => c === trigger || (clocked && c === "Clock") || fileFolders.some((f) => c === `FileArrival(${JSON.stringify(f)})`) || c.startsWith("MailArrival(") || c.startsWith("Action(");
  const listed = [...new Set([...capabilities.filter((c) => !starts(c)), ...optional])].sort();
  if (listed.length === 0) return null;
  const toggle = (item: string, on: boolean) => {
    const next = on ? [...optional, item] : optional.filter((x) => x !== item);
    setSettings({ ...settings, requirements: { ...settings.requirements, optional: next.length > 0 ? next.sort() : undefined } });
  };
  return (
    <>
      <span
        style={{ gridColumn: "1 / -1", color: "var(--ink)", marginTop: 4 }}
        title="a required permission not granted pauses the flow; an optional one leaves it running, those calls refused"
      >
        permissions: optional ones it can run without
      </span>
      {listed.map((item) => {
        const unused = !capabilities.includes(item);
        return (
          <label key={item} style={{ display: "contents" }} title={item}>
            <span style={{ color: unused ? "var(--bad)" : undefined }}>
              {describeItem(item)}
              {unused ? " (no longer used)" : ""}
            </span>
            <span style={{ display: "inline-flex", gap: 6, alignItems: "center" }}>
              <input type="checkbox" checked={optional.includes(item)} onChange={(e) => toggle(item, e.target.checked)} disabled={readOnly} />
              <span style={{ color: optional.includes(item) ? "var(--faint)" : "var(--warn)" }}>{optional.includes(item) ? "optional" : "required"}</span>
            </span>
          </label>
        );
      })}
    </>
  );
}

/** The names of a flow's actions, from its capabilities (`Action("reset")` is `reset`). */
export function actionNames(capabilities: string[]): string[] {
  return capabilities.flatMap((c) => {
    const m = /^Action\((".*")\)$/.exec(c);
    return m ? [JSON.parse(m[1]) as string] : [];
  });
}

/**
 * Which of the flow's actions agents may run (permissions.rs: `agents`). Each one ticked is asked
 * for as `AgentRuns("reset")`, which a person approves; unticked, an agent is refused it. One
 * ticked that the flow no longer has is shown so it can be cleared (deploy refuses it).
 */
function AgentsMayRun({ capabilities, settings, setSettings, readOnly = false }: { capabilities: string[]; settings: FlowSettings; setSettings: (s: FlowSettings) => void; readOnly?: boolean }) {
  const agents = settings.requirements?.agents ?? [];
  const actions = actionNames(capabilities);
  const listed = [...new Set([...actions, ...agents])].sort();
  if (listed.length === 0) return null;
  const toggle = (name: string, on: boolean) => {
    const next = on ? [...agents, name] : agents.filter((x) => x !== name);
    setSettings({ ...settings, requirements: { ...settings.requirements, agents: next.length > 0 ? next.sort() : undefined } });
  };
  return (
    <>
      <span
        style={{ gridColumn: "1 / -1", color: "var(--ink)", marginTop: 4 }}
        title="a person runs any action; an agent only those ticked here, once a person approves it"
      >
        agents may run
      </span>
      {listed.map((name) => {
        const unused = !actions.includes(name);
        return (
          <label key={name} style={{ display: "contents" }} title={`AgentRuns(${JSON.stringify(name)})`}>
            <span style={{ color: unused ? "var(--bad)" : undefined }}>
              action {name}
              {unused ? " (no longer in the flow)" : ""}
            </span>
            <input type="checkbox" aria-label={`agents may run ${name}`} checked={agents.includes(name)} onChange={(e) => toggle(name, e.target.checked)} disabled={readOnly} style={{ justifySelf: "start" }} />
          </label>
        );
      })}
    </>
  );
}

/**
 * Whether agents may read the flow's log lines (permissions.rs: `agent_logs`). Ticked, it is asked for
 * as `AgentReadsLogs`, which a person approves; unticked, an agent is refused them, and a person reads
 * them either way. Shown for a flow that writes lines, or one still asking, so it can be cleared.
 */
function AgentsMayReadLogs({ capabilities, settings, setSettings, readOnly = false }: { capabilities: string[]; settings: FlowSettings; setSettings: (s: FlowSettings) => void; readOnly?: boolean }) {
  const asked = settings.requirements?.agent_logs === true;
  const writes = capabilities.some((c) => c.startsWith("Log("));
  if (!writes && !asked) return null;
  return (
    <>
      <span style={{ gridColumn: "1 / -1", color: "var(--ink)", marginTop: 4 }} title="a person reads its lines; an agent only if this is ticked, once a person approves it">
        agents may read
      </span>
      <label style={{ display: "contents" }} title="AgentReadsLogs">
        <span style={{ color: writes ? undefined : "var(--bad)" }}>its log lines{writes ? "" : " (it writes none now)"}</span>
        <input
          type="checkbox"
          aria-label="agents may read its logs"
          checked={asked}
          onChange={(e) => setSettings({ ...settings, requirements: { ...settings.requirements, agent_logs: e.target.checked ? true : undefined } })}
          disabled={readOnly}
          style={{ justifySelf: "start" }}
        />
      </label>
    </>
  );
}

/**
 * The keys whose values the flow keeps out of its records (`taint`), as a list across the panel:
 * the ones every flow does, fixed, then its own, each removed with its ×, and a field that adds one
 * once core's rule and the list take it. Editing it rewrites the text's `flow(...)` line, as the
 * port does.
 */
function TaintKeys({ settings, setSettings, readOnly = false }: { settings: FlowSettings; setSettings: (s: FlowSettings) => void; readOnly?: boolean }) {
  const [draft, setDraft] = useState("");
  const keys = settings.taint ?? [];
  const key = draft.trim();
  const problem = key === "" ? null : taintKeyProblem(key, keys);
  const set = (next: string[]) => {
    const { taint: _, ...rest } = settings;
    setSettings(next.length > 0 ? { ...rest, taint: next } : rest);
  };
  const add = () => {
    if (key === "" || problem) return;
    set([...keys, key]);
    setDraft("");
  };
  const chip: CSSProperties = { display: "inline-flex", alignItems: "center", gap: 2, padding: "0 4px", border: "1px solid var(--border)", borderRadius: 3 };
  return (
    <>
      <span
        style={{ gridColumn: "1 / -1", marginTop: 4 }}
        title="keys whose values the host never keeps of any run: a header, a query or path parameter, a body field, a message. Its records show ‹taint:key› in their place"
      >
        keeps out of its records
      </span>
      <div style={{ gridColumn: "1 / -1", display: "flex", flexDirection: "column", gap: 4, minWidth: 0 }} aria-label="taint keys">
        <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
          {alwaysTainted().map((k) => (
            <span key={k} style={{ ...chip, color: "var(--faint)", borderStyle: "dashed" }} title="every flow keeps it out">
              {k}
            </span>
          ))}
          {keys.map((k) => (
            <span key={k} style={{ ...chip, color: "var(--ink)" }}>
              {k}
              {!readOnly && (
                <button
                  onClick={() => set(keys.filter((x) => x !== k))}
                  aria-label={`stop keeping ${k} out`}
                  title={`record the values of ${k} again`}
                  style={{ border: "none", background: "none", color: "var(--faint)", cursor: "pointer", padding: 0, fontSize: 11, lineHeight: 1 }}
                >
                  ×
                </button>
              )}
            </span>
          ))}
        </div>
        {!readOnly && (
          <input
            value={draft}
            placeholder="add a key (x-api-key), Enter"
            aria-label="add a taint key"
            autoComplete="off"
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") add();
            }}
            style={{ ...input, width: "100%", ...(problem ? { borderColor: "var(--bad)" } : {}) }}
          />
        )}
        {problem && <span style={{ color: "var(--bad)" }}>{problem}</span>}
      </div>
    </>
  );
}

function GroupHead({ label, open, onToggle, first = false }: {
  label: string; open: boolean; onToggle: () => void; first?: boolean;
}) {
  return (
    <button
      style={{
        gridColumn: "1 / -1",
        display: "flex",
        alignItems: "center",
        width: "100%",
        font: "inherit",
        background: "transparent",
        border: "none",
        borderTop: first ? "none" : "1px solid var(--border)",
        color: "var(--faint)",
        cursor: "pointer",
        textAlign: "left",
        padding: first ? "0 0 5px" : "8px 0 5px",
        fontSize: 10,
        letterSpacing: "0.12em",
        textTransform: "uppercase",
      }}
      onClick={onToggle}
    >
      {label}
      <span style={{ fontSize: 9, opacity: 0.5, marginLeft: "auto" }}>{open ? "▾" : "▸"}</span>
    </button>
  );
}

/**
 * The deploy settings: server, limits, network, permissions and secrets, each a group that folds.
 * `open`: whether they are shown (what the host allows and the secrets are read once they are).
 */
export function DeploySettings({
  open,
  settings,
  setSettings,
  slug,
  secretNames,
  capabilities,
  clocks = [],
  fileFolders = [],
  readOnly = false,
}: {
  open: boolean;
  settings: FlowSettings;
  setSettings: (s: FlowSettings) => void;
  slug: string | null;
  secretNames: string[];
  capabilities: string[];
  clocks?: string[];
  fileFolders?: string[];
  readOnly?: boolean;
}) {
  const [policy, setPolicy] = useState<HostPolicy | null>(null);
  const [secrets, setSecrets] = useState<SecretInfo[]>([]);
  const [envNames, setEnvNames] = useState<string[]>([]);
  const reloadSecrets = () => {
    if (slug) listSecrets(slug).then(setSecrets).catch(() => setSecrets([]));
  };
  useEffect(() => {
    if (open && !policy) getPermissions().then(setPolicy).catch(() => {});
  }, [open, policy]);
  useEffect(reloadSecrets, [open, slug]);
  useEffect(() => {
    if (open) getEnvSecretNames().then((r) => setEnvNames(r.names)).catch(() => setEnvNames([]));
  }, [open]);

  const [openGroups, setOpenGroups] = useState<Set<string>>(
    () => new Set(["server", "limits", "network", "permissions", "secrets"])
  );
  const toggleGroup = (key: string) =>
    setOpenGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  const [hideDefaults, setHideDefaults] = useState(false);

  const req = settings.requirements;
  const noPlainHttp = policy !== null && !policy.allow_plain_http;
  const hasPlainHttp = (req?.plain_http ?? []).length > 0;

  return (
    <div className="hl-settings">
      <label className="hl-settings__defaults" title="hide settings that are still at their defaults">
        <input type="checkbox" checked={hideDefaults} onChange={(e) => setHideDefaults(e.target.checked)} />
        hide defaults
      </label>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "minmax(0, 180px) minmax(0, 360px)",
          gap: "4px 8px",
          alignItems: "start",
          alignContent: "start",
          fontSize: 11,
          color: "var(--muted)",
        }}
      >
        <GroupHead label="server" open={openGroups.has("server")} onToggle={() => toggleGroup("server")} first />
        {openGroups.has("server") && (
          <>
            <span title="the port a flow with routes (request nodes) answers on">port</span>
            <input value={settings.port} onChange={(e) => setSettings({ ...settings, port: e.target.value })} disabled={readOnly} style={{ ...input, width: "100%" }} />
            {(!hideDefaults || !!settings.browsers) && (
              <>
                <span title="web pages on other sites that may call its routes (CORS): * for any, or origins like https://example.com">web pages from</span>
                <input
                  value={settings.browsers ?? ""}
                  placeholder="none (* or https://site)"
                  onChange={(e) => setSettings({ ...settings, browsers: e.target.value })}
                  disabled={readOnly}
                  style={{ ...input, width: "100%" }}
                />
              </>
            )}
            {(!hideDefaults || !!settings.taint?.length) && <TaintKeys settings={settings} setSettings={setSettings} readOnly={readOnly} />}
            {clocks.length > 0 && (
              <>
                <span title="its every nodes: each runs its group on its own period">clocks</span>
                <span style={{ color: "var(--ink)" }}>{clocks.join(", ")}</span>
              </>
            )}
          </>
        )}
        <GroupHead label="limits" open={openGroups.has("limits")} onToggle={() => toggleGroup("limits")} />
        {openGroups.has("limits") && (
          <>
            {LIMITS.filter(([name]) => !hideDefaults || req?.limits?.[name] !== undefined).map(([name, label]) => (
              <label key={name} style={{ display: "contents" }}>
                <span>{label}</span>
                <input
                  value={req?.limits?.[name] ?? ""}
                  placeholder={policy ? String(policy.defaults[name]) : ""}
                  title={policy ? `default ${policy.defaults[name]}, ceiling ${policy.ceilings[name]}` : undefined}
                  onChange={(e) => setSettings({ ...settings, requirements: withLimit(req, name, e.target.value) })}
                  disabled={readOnly}
                  style={{ ...input, width: "100%" }}
                />
              </label>
            ))}
            {hideDefaults && !Object.keys(req?.limits ?? {}).length && (
              <span style={{ gridColumn: "1 / -1", color: "var(--faintest)", fontStyle: "italic" }}>none set</span>
            )}
          </>
        )}
        <GroupHead label="network" open={openGroups.has("network")} onToggle={() => toggleGroup("network")} />
        {openGroups.has("network") && (
          <>
            {(!hideDefaults || hasPlainHttp) && (
              <>
                <span title="hosts it may call over plain HTTP; this host must allow plain HTTP at all">plain HTTP to</span>
                <input
                  value={(req?.plain_http ?? []).join(", ")}
                  placeholder="none"
                  autoComplete="off"
                  disabled={readOnly || (noPlainHttp && !hasPlainHttp)}
                  title={noPlainHttp ? "this host never grants plain HTTP" + (hasPlainHttp ? ": clear this to deploy" : "") : "comma-separated host[:port]"}
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      requirements: { ...req, plain_http: e.target.value.split(",").map((h) => h.trim()).filter((h) => h !== "") },
                    })
                  }
                  style={{ ...input, width: "100%", ...(noPlainHttp && hasPlainHttp ? { borderColor: "var(--bad)" } : {}) }}
                />
              </>
            )}
            {(!hideDefaults || req?.private_addresses) && (
              <>
                <span title="this machine and its network: loopback, private and link-local addresses">private addresses</span>
                <input
                  type="checkbox"
                  checked={req?.private_addresses ?? false}
                  disabled={readOnly || (policy !== null && !policy.allow_private_addresses)}
                  title={policy && !policy.allow_private_addresses ? "this host never grants private addresses" : undefined}
                  onChange={(e) => setSettings({ ...settings, requirements: { ...req, private_addresses: e.target.checked } })}
                  style={{ justifySelf: "start" }}
                />
              </>
            )}
            {hideDefaults && !hasPlainHttp && !req?.private_addresses && (
              <span style={{ gridColumn: "1 / -1", color: "var(--faintest)", fontStyle: "italic" }}>none set</span>
            )}
          </>
        )}
        <GroupHead label="permissions" open={openGroups.has("permissions")} onToggle={() => toggleGroup("permissions")} />
        {openGroups.has("permissions") && (
          <>
            <OptionalPermissions capabilities={capabilities} settings={settings} setSettings={setSettings} clocked={clocks.length > 0} fileFolders={fileFolders} readOnly={readOnly} />
            <AgentsMayRun capabilities={capabilities} settings={settings} setSettings={setSettings} readOnly={readOnly} />
            <AgentsMayReadLogs capabilities={capabilities} settings={settings} setSettings={setSettings} readOnly={readOnly} />
            {capabilities.length === 0 && !(settings.requirements?.optional?.length) && !(settings.requirements?.agents?.length) && !settings.requirements?.agent_logs && (
              <span style={{ gridColumn: "1 / -1", color: "var(--faintest)", fontStyle: "italic" }}>none</span>
            )}
          </>
        )}
        {secretNames.length > 0 && (
          <>
            <GroupHead label="secrets" open={openGroups.has("secrets")} onToggle={() => toggleGroup("secrets")} />
            {openGroups.has("secrets") && (
              <>
                {!slug && (
                  <span style={{ gridColumn: "1 / -1", color: "var(--faint)", fontStyle: "italic" }}>name the flow first</span>
                )}
                {secretNames.map((name) => (
                  <SecretRow key={name} slug={slug} name={name} info={secrets.find((x) => x.name === name)} envNames={envNames} onChanged={reloadSecrets} readOnly={readOnly} />
                ))}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
